//! Binance REST 请求的**统一重试 / 退避 / 限流**处理 (035)。
//!
//! 背景: 此前所有 REST 调用 (现货 / 合约 / fapi 公共数据) 都是"一次 `send()`, 失败即
//! `Network` 错误" —— 长窗口回测分页时任意一页抖动就让整轮失败, 且对 429/418 无限流处理
//! (`CoreError::RateLimit` 定义了很久却从未被构造)。
//!
//! 重试策略按**请求语义**分档 (决策 Q2, 安全红线):
//! - [`RequestKind::Read`] (K线 / 行情 / 查询): 传输错误、429、418、408、5xx 均可重试。
//! - [`RequestKind::Write`] (下单 / 撤单 / 改杠杆等改账户状态): **仅** 重试"连接层失败"
//!   (请求确定未送达); 响应超时、5xx 一律不重试 —— 避免"其实已成交, 重放变重复下单"。
//!
//! 429/418 尊重响应 `Retry-After` (仅秒形式), 上限 30s 防长时间挂起; 最终仍失败的限流
//! 响应由调用方经 [`status_error`] 映射为 [`CoreError::RateLimit`]。

use std::time::Duration;

use ricow_core::{CoreError, CoreResult};

/// 重试策略。
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    /// 总尝试次数 (含首次)。
    pub max_attempts: u32,
    /// 首次退避时长。
    pub base_delay: Duration,
    /// 退避上限。
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(8),
        }
    }
}

/// 请求语义: 决定可重试的错误面 (写操作严格收窄)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestKind {
    /// 只读 (K线 / exchangeInfo / depth / time / 查询类签名端点)。
    Read,
    /// 写 (下单 / 撤单 / 改杠杆 / 改保证金等改账户状态)。
    Write,
}

/// 按策略发送请求并重试: 返回**最终一次**响应 (可能仍是 4xx/5xx, 交由调用方处理)。
///
/// `build` 每次调用都重新构造请求 —— 币安这些端点均为 GET/DELETE/POST **无 body**, 重建安全。
/// 重试耗尽后不吞错: 若最后一次是传输错误则返回 [`CoreError::Network`], 否则把响应原样返回,
/// 由调用方经 [`status_error`] 归一 (含 429/418 → `RateLimit`)。
pub(crate) async fn send_with_retry<F>(
    build: F,
    kind: RequestKind,
    policy: RetryPolicy,
) -> CoreResult<reqwest::Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt: u32 = 1;
    loop {
        match build().send().await {
            Ok(resp) => {
                let status = resp.status();
                let can_retry = attempt < policy.max_attempts && is_retryable_status(status, kind);
                if !can_retry {
                    return Ok(resp);
                }
                let wait = retry_after_wait(resp.headers())
                    .unwrap_or_else(|| backoff_delay(&policy, attempt));
                drop(resp);
                tracing::warn!(
                    attempt,
                    status = status.as_u16(),
                    wait_ms = wait.as_millis() as u64,
                    "BN REST 可重试状态, 退避后重试"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(e) => {
                if attempt < policy.max_attempts && transport_retryable(&e, kind) {
                    let wait = backoff_delay(&policy, attempt);
                    tracing::warn!(
                        attempt,
                        wait_ms = wait.as_millis() as u64,
                        error = %e,
                        "BN REST 传输错误, 退避后重试"
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                    continue;
                }
                return Err(CoreError::Network(e.to_string()));
            }
        }
    }
}

/// 状态码层面是否可重试 (写操作一律 false —— 安全红线)。
fn is_retryable_status(status: reqwest::StatusCode, kind: RequestKind) -> bool {
    if kind == RequestKind::Write {
        return false;
    }
    let c = status.as_u16();
    c == 429 || c == 418 || c == 408 || (500..=599).contains(&c)
}

/// 传输层错误是否可重试。
///
/// - 连接层失败 (`is_connect`) → 请求**确定未送达**, 读写皆可重试。
/// - 写操作: 除连接失败外一律不重试 (超时/中断可能已送达并成交)。
/// - 读操作: 超时与其他传输错误均可重试 (幂等)。
fn transport_retryable(e: &reqwest::Error, kind: RequestKind) -> bool {
    if e.is_connect() {
        return true;
    }
    if kind == RequestKind::Write {
        return false;
    }
    e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
}

/// 指数退避 + ±20% 抖动 (用纳秒低位做伪随机源, 免引入 rand 依赖)。
fn backoff_delay(policy: &RetryPolicy, attempt: u32) -> Duration {
    let shift = (attempt.saturating_sub(1)).min(5);
    let exp = policy.base_delay.saturating_mul(1u32 << shift).min(policy.max_delay);
    let ms = exp.as_millis() as i64;
    let jittered = (ms * (100 + pseudo_jitter_pct()) / 100).max(1) as u64;
    Duration::from_millis(jittered)
}

/// -20..=20 的伪随机抖动百分比。
fn pseudo_jitter_pct() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as i64)
        .unwrap_or(0);
    (nanos % 41) - 20
}

/// 解析 `Retry-After` 响应头 (仅秒形式; 币安限流用秒)。上限 30s 防挂起。
fn retry_after_wait(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(Duration::from_secs(secs.min(30)))
}

/// 把非 2xx 响应归一为 `CoreError`: 429/418 → [`CoreError::RateLimit`], 其余 → `Exchange`。
///
/// 消息优先取币安 JSON 的 `msg` 字段, 无则用原始响应体 —— 与既有 `check_bn_response` 同格式。
pub(crate) fn status_error(status: reqwest::StatusCode, body: &str) -> CoreError {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["msg"].as_str().map(String::from))
        .unwrap_or_else(|| body.to_string());
    if status.as_u16() == 429 || status.as_u16() == 418 {
        CoreError::RateLimit(format!("BN {status}: {msg}"))
    } else {
        CoreError::Exchange(format!("BN {status}: {msg}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_writable_states_never_retry() {
        // 写操作: 任何状态码都不重试 (安全红线)。
        for c in [429u16, 418, 500, 503, 408] {
            let s = reqwest::StatusCode::from_u16(c).unwrap();
            assert!(!is_retryable_status(s, RequestKind::Write), "写操作 {c} 不应重试");
        }
    }

    #[test]
    fn test_readable_states_retry_matrix() {
        let ok = |c: u16| {
            is_retryable_status(reqwest::StatusCode::from_u16(c).unwrap(), RequestKind::Read)
        };
        assert!(ok(429));
        assert!(ok(418));
        assert!(ok(408));
        assert!(ok(500));
        assert!(ok(503));
        assert!(!ok(400), "参数错误不应重试");
        assert!(!ok(401), "鉴权错误不应重试");
        assert!(!ok(404));
    }

    #[test]
    fn test_status_error_maps_rate_limit() {
        let e = status_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"code":-1003,"msg":"Too many requests"}"#,
        );
        assert!(matches!(e, CoreError::RateLimit(_)), "429 应映射为 RateLimit");
        let e2 = status_error(reqwest::StatusCode::BAD_REQUEST, r#"{"msg":"bad"}"#);
        assert!(matches!(e2, CoreError::Exchange(_)));
        assert!(format!("{e2}").contains("bad"), "应取 msg 字段");
    }

    #[test]
    fn test_backoff_grows_and_is_capped() {
        let p = RetryPolicy::default();
        let d1 = backoff_delay(&p, 1);
        let d3 = backoff_delay(&p, 3);
        // 抖动 ±20% 下, 第 3 次退避仍显著大于第 1 次。
        assert!(d3 > d1, "退避应随尝试次数增长");
        let d9 = backoff_delay(&p, 9);
        assert!(d9 <= p.max_delay.mul_f64(1.21), "退避须封顶在 max_delay 附近, 实得 {:?}", d9);
    }
}
