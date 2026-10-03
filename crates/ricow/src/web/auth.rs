//! Web 鉴权与来源校验(025 / D3 / FR-002; 036 补来源门).
//!
//! 两道门, 顺序固定:
//! 1. **token**(一次性随机, 不落盘不进日志): 缺失或错误一律 `401` **且响应体为空**;
//! 2. **来源**(036): 写方法只接受**回环来源** —— token 因 `EventSource` 的限制会出现在
//!    URL 查询串里(可能经 Referer / 浏览器历史外流), 来源门是它外流后的一道闸: 别的站点
//!    拿着 token 也发不出**写**请求(读请求不改状态, 不设限以保 `curl` / 脚本可用)。

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::WebState;

/// token 校验 + 来源校验中间件: 缺失或错误**一律 `401` 且响应体为空**(不回任何内容, D3 / FR-002)。
pub(super) async fn require_token(
    State(state): State<WebState>,
    req: Request,
    next: Next,
) -> Response {
    match token_of(&req) {
        Some(token) if ct_eq(&token, &state.token) => {}
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    }
    // 来源门(036): 只看**写方法**; 非回环来源一律 403(同样空体, 不解释原因)。
    if !origin_allowed(&req) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

/// 从请求中取 token: 先 `Authorization: Bearer <token>`, 再查询参数 `?token=<token>`。
///
/// 两种都收是因为浏览器 `EventSource` **无法自定请求头**, 只能把 token 挂在 URL 上。
pub(super) fn token_of(req: &Request) -> Option<String> {
    if let Some(value) = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(token) = value.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }
    req.uri().query()?.split('&').find_map(|kv| {
        let (key, value) = kv.split_once('=')?;
        (key == "token").then(|| value.to_string())
    })
}

/// 常量时间比较 (Web token / 风险确认短语) —— **逐字搬抄** `supervisor/server.rs` 的同名函数 (D3),
/// 因 `supervisor` 为 crate 私有模块, 跨模块无法复用, 故在源处同步维护这份实现。
///
/// 逐字节短路比较 (字符串 `==`) 会按第一个不同的字节提前返回, 在回环网络上仍可能被
/// 反复试探出前缀; 这里对全长度做定长累加, 不提前退出。长度不等直接判否 (长度本身不敏感)。
pub(super) fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 会改状态的方法(需要过来源门)。`GET` / `HEAD` / `OPTIONS` 不改状态 —— 放行。
fn is_state_changing(method: &Method) -> bool {
    matches!(*method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE)
}

/// 来源是否可信: 写方法要求 `Origin`(缺失时退 `Referer`)**是回环来源**; 两者都没有 → 放行。
///
/// 为什么"两者都没有也放行": 浏览器对 `POST` 一定会带 `Origin`(表单 / fetch / XHR 皆然),
/// 所以缺失就意味着请求**不是浏览器发的** —— 那是 `curl` / 脚本 / 本仓库自己的测试,
/// 它们没有"被第三方页面借用身份"的风险。真正要拦的是**带外站 Origin 的浏览器请求**。
fn origin_allowed(req: &Request) -> bool {
    if !is_state_changing(req.method()) {
        return true;
    }
    for name in [header::ORIGIN, header::REFERER] {
        if let Some(value) = req.headers().get(&name).and_then(|v| v.to_str().ok()) {
            if !value.is_empty() {
                return is_loopback_origin(value);
            }
        }
    }
    true
}

/// `http(s)://<loopback>[:port][/...]` → true; 其余(含 `Origin: null` 的不透明来源) → false。
fn is_loopback_origin(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        // `null` / 裸主机名等: 不是可解析的绝对来源, 一律不可信。
        return false;
    };
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // 去掉 userinfo(`user@host`)。
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 用 `[...]` 括起来; 其余按第一个 `:` 切端口。
    let host = if let Some(tail) = authority.strip_prefix('[') {
        tail.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn req(method: &str, uri: &str, headers: &[(&str, &str)]) -> Request {
        let mut builder = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::empty()).expect("构造请求")
    }

    #[test]
    fn test_ct_eq_matches_only_identical() {
        assert!(ct_eq("", ""));
        assert!(ct_eq("3f2a-9c", "3f2a-9c"));
        assert!(!ct_eq("3f2a-9c", "3f2a-9d"));
        assert!(!ct_eq("3f2a-9c", "3f2a-9c-"));
        assert!(!ct_eq("", "3f2a-9c"));
    }

    #[test]
    fn test_token_of_reads_header_then_query() {
        // Authorization: Bearer 优先。
        assert_eq!(
            token_of(&req("GET", "/api/ping", &[("authorization", "Bearer abc")])).as_deref(),
            Some("abc")
        );
        assert_eq!(token_of(&req("GET", "/api/ping", &[("authorization", "Basic abc")])), None);
        // 无头时退回查询参数; 只有名为 token 的那个键算数。
        assert_eq!(token_of(&req("GET", "/api/ping?token=xyz", &[])).as_deref(), Some("xyz"));
        assert_eq!(token_of(&req("GET", "/api/ping?a=b&token=xyz", &[])).as_deref(), Some("xyz"));
        assert_eq!(token_of(&req("GET", "/api/ping?tokens=xyz", &[])), None);
        assert_eq!(token_of(&req("GET", "/api/ping", &[])), None);
    }

    #[test]
    fn test_is_loopback_origin_accepts_only_loopback() {
        for ok in [
            "http://127.0.0.1:8787",
            "http://localhost:8787",
            "http://[::1]:8787",
            "https://127.0.0.1",
            "http://127.0.0.1:8787/api/x?y=1",
        ] {
            assert!(is_loopback_origin(ok), "{ok} 应判为回环");
        }
        for bad in [
            "http://evil.example",
            "https://127.0.0.1.evil.example",
            "http://127.0.0.1.evil.example:8787",
            "null",
            "file:///tmp/x.html",
            "http://2130706433:8787", // 十进制 IP 写法 (不是 127.0.0.1 字面量)
            "ftp://127.0.0.1",
        ] {
            assert!(!is_loopback_origin(bad), "{bad} 不得判为回环");
        }
    }

    #[test]
    fn test_origin_gate_blocks_cross_site_writes_only() {
        // 读方法不受来源门约束(脚本 / curl 照常)。
        assert!(origin_allowed(&req("GET", "/api/ping", &[("origin", "http://evil.example")])));
        // 写方法 + 外站 Origin → 拒。
        assert!(!origin_allowed(&req("POST", "/api/x", &[("origin", "http://evil.example")])));
        // 写方法 + 回环 Origin → 放行(页面自己的同源请求)。
        assert!(origin_allowed(&req("POST", "/api/x", &[("origin", "http://127.0.0.1:8787")])));
        // 写方法 + 无 Origin: 非浏览器请求(curl / 测试) → 放行。
        assert!(origin_allowed(&req("POST", "/api/x", &[])));
        // 只有 Referer 时按其判断。
        assert!(!origin_allowed(&req("DELETE", "/api/x", &[("referer", "http://evil.example/p")])));
        assert!(origin_allowed(&req(
            "DELETE",
            "/api/x",
            &[("referer", "http://127.0.0.1:8787/p")]
        )));
        // Origin 在场即以其为准(即便 Referer 是回环)。
        assert!(!origin_allowed(&req(
            "POST",
            "/api/x",
            &[("origin", "http://evil.example"), ("referer", "http://127.0.0.1:8787/")]
        )));
    }
}
