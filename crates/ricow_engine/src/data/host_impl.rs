//! 宿主服务的引擎实现 (028 T016/T023/T024): 把 [`DataHub`] 与 HTTP 能力接到策略侧的
//! [`HostServices`] 接缝上。
//!
//! 三个关键设计点:
//!
//! 1. **同步 ↔ 异步桥接**: 策略侧(Lua 沙箱)是同步的, 而取数是异步的。这里用
//!    `block_in_place` + 当前运行时阻塞等待 —— 只在**多线程** tokio 运行时下成立
//!    (ricow 的 CLI 入口即此)。单线程运行时会明确报错, 而不是死锁。
//! 2. **可见时刻是状态, 不是参数**: `data:history` 之类的策略调用不带时刻, 平台按
//!    "当前可见时刻"割断(`close_time <= now`)。回测里这个值由虚拟时钟推进 —— 这是
//!    回测无前视的实现要点(FR-013); 实盘里就是墙钟。
//! 3. **HTTP 走独立线程 + 独立 runtime**: 策略的 `http:get` 不能占用引擎的工作线程,
//!    否则一个慢请求会卡住整个行情处理。墙钟超时与响应体上限都在这一层兜住(FR-027)。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ricow_core::{CoreError, CoreResult, Kline, PriceMode, SeriesKey, SeriesWindow};
use tokio::runtime::{Handle, RuntimeFlavor};

use ricow_strategy::{HostServices, SeriesDecl};

use super::DataHub;

/// 策略 HTTP 的限额 (FR-027: 墙钟超时默认 10s, 响应体上限默认 5MB)。
#[derive(Debug, Clone, Copy)]
pub struct HttpPolicy {
    pub timeout: Duration,
    pub max_bytes: usize,
}

impl Default for HttpPolicy {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(10), max_bytes: 5 * 1024 * 1024 }
    }
}

/// `HostServices` 的引擎实现。
pub struct EngineHost {
    hub: Arc<DataHub>,
    /// 当前可见时刻(ms) —— 回测虚拟时钟 / 实盘墙钟。
    now_ms: AtomicI64,
    /// 是否允许回源取数: 回测 `false`(只读本地库, D3), Dry Run / 实盘 `true`。
    allow_fetch: bool,
    http: HttpPolicy,
    rt: Handle,
}

impl EngineHost {
    /// 装配宿主。必须在 tokio 运行时上下文内构造(要捕获 `Handle`)。
    pub fn new(hub: Arc<DataHub>, now_ms: i64, allow_fetch: bool) -> CoreResult<Self> {
        let rt = Handle::try_current().map_err(|_| {
            CoreError::InvalidArgument(
                "装配 EngineHost 需要处于 tokio 运行时上下文(取数需要运行时)".to_string(),
            )
        })?;
        Ok(Self {
            hub,
            now_ms: AtomicI64::new(now_ms),
            allow_fetch,
            http: HttpPolicy::default(),
            rt,
        })
    }

    /// 覆盖 HTTP 限额(测试与将来的配置项用)。
    pub fn with_http_policy(mut self, policy: HttpPolicy) -> Self {
        self.http = policy;
        self
    }

    /// 推进可见时刻(回测: 每个刻度推进一次; 实盘: 每次循环刷新墙钟)。
    pub fn set_now_ms(&self, ms: i64) {
        self.now_ms.store(ms, Ordering::SeqCst);
    }

    /// 当前可见时刻(ms)。
    pub fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    /// 是否允许回源。
    pub fn allow_fetch(&self) -> bool {
        self.allow_fetch
    }

    /// 底层数据服务(装配层与驱动用)。
    pub fn hub(&self) -> &Arc<DataHub> {
        &self.hub
    }

    /// 装载序列窗口(带口径与可见性割断) —— 装配/驱动的入口。
    ///
    /// `n` = 窗口根数, `before_ms` = 可见时刻(不传就用宿主当前可见时刻)。
    pub fn load_window(
        &self,
        decl: &SeriesDecl,
        n: u32,
        before_ms: i64,
    ) -> CoreResult<SeriesWindow> {
        let fut =
            self.hub.load_series_window(&decl.key, n, before_ms, decl.price_mode, self.allow_fetch);
        self.block_on(fut)?
    }

    /// 同步等待异步取数。
    fn block_on<F: std::future::Future>(&self, fut: F) -> CoreResult<F::Output> {
        match Handle::try_current() {
            Ok(h) if h.runtime_flavor() == RuntimeFlavor::MultiThread => {
                Ok(tokio::task::block_in_place(|| h.block_on(fut)))
            }
            Ok(_) => Err(CoreError::InvalidArgument(
                "数据服务需要多线程 tokio 运行时(当前为单线程运行时, 同步等待取数会死锁); \
                 请用 (#[tokio::main]) 默认的多线程入口"
                    .to_string(),
            )),
            Err(_) => Ok(self.rt.block_on(fut)),
        }
    }
}

/// 数据缺失的可执行提示 (FR-031): 直接给出该敲的命令。
fn pull_hint(key: &SeriesKey) -> String {
    format!(
        "ricow data pull --source {} --symbol {} --interval {}",
        key.source,
        key.symbol,
        key.interval.label()
    )
}

impl HostServices for EngineHost {
    fn load_series(&self, decl: &SeriesDecl) -> CoreResult<Vec<Kline>> {
        let n = decl.effective_window() as u32;
        let before = self.now_ms();
        let window = self.load_window(decl, n, before)?;
        // 声明期(脚本顶层/on_init)拿到空窗是**合法**的: "此刻确实还没有已收盘的 bar"
        // (例如回测第一根 bar 之前)。缺数据的硬错误由装配层负责 —— 见 `SeriesDriver::load`
        // 与 `EngineHost::history`: 那里才是"策略明确要数据却没有"的场合(FR-031)。
        if window.bars.is_empty() {
            tracing::warn!(
                target: "data",
                "序列 {} 在可见时刻 {} 还没有已收盘 bar(策略句柄初始为空)",
                decl.key,
                before
            );
            return Ok(Vec::new());
        }
        if let Some(min) = decl.min_bars {
            if window.bars.len() < min {
                tracing::warn!(
                    target: "data",
                    "序列 {} 初始窗口只有 {} 根, 声明的最小预热 {} 根不足; 若装配期仍不足会硬报错",
                    decl.key,
                    window.bars.len(),
                    min
                );
            }
        }
        Ok(window.bars)
    }

    fn history(&self, key: &SeriesKey, limit: Option<usize>) -> CoreResult<Vec<Kline>> {
        let n = limit
            .unwrap_or(ricow_strategy::SERIES_WINDOW_DEFAULT)
            .clamp(ricow_strategy::SERIES_WINDOW_MIN, ricow_strategy::SERIES_WINDOW_MAX)
            as u32;
        let window = self.block_on(self.hub.load_series_window(
            key,
            n,
            self.now_ms(),
            PriceMode::Close,
            self.allow_fetch,
        ))??;
        if window.bars.is_empty() {
            return Err(CoreError::InvalidArgument(format!(
                "序列 {key} 在本地库没有截至当前可见时刻的数据; 先拉取: {}",
                pull_hint(key)
            )));
        }
        Ok(window.bars)
    }

    fn http_get(&self, url: &str) -> CoreResult<String> {
        // 计划 d13: 独立线程 + 独立 runtime + 墙钟超时 + 响应体上限。
        // 该线程不共享引擎工作线程, 因此慢请求不会阻塞行情处理。
        let policy = self.http;
        let url = url.to_string();
        let thread_url = url.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("ricow-strategy-http".to_string())
            .spawn(move || {
                let out = fetch_blocking(&thread_url, policy);
                let _ = tx.send(out);
            })
            .map_err(|e| CoreError::InvalidArgument(format!("network: 无法创建 HTTP 线程: {e}")))?;

        // 外层兜底: 线程内部已有 reqwest 超时, 这里再宽限 5s 防止线程卡死拖住策略。
        let wait = policy.timeout + Duration::from_secs(5);
        match rx.recv_timeout(wait) {
            Ok(res) => res,
            Err(_) => Err(CoreError::InvalidArgument(format!(
                "timeout: 策略 HTTP 请求超过 {:?} 未返回(已放弃等待): {url}",
                policy.timeout
            ))),
        }
        .inspect_err(|_e: &CoreError| {
            // 线程句柄不 joined(超时分支里线程可能仍在跑), 显式 detach 语义。
            drop(handle);
        })
    }
}

/// 在线程内执行一次 GET(Mini runtime), 并把失败分类成可判别的错误前缀(FR-027/T034)。
fn fetch_blocking(url: &str, policy: HttpPolicy) -> CoreResult<String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(CoreError::InvalidArgument(format!(
            "invalid_url: 只支持 http/https, 收到 '{url}'"
        )));
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CoreError::InvalidArgument(format!("network: 无法创建 HTTP 运行时: {e}")))?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(policy.timeout)
            .build()
            .map_err(|e| CoreError::InvalidArgument(format!("network: 客户端构建失败: {e}")))?;
        let mut resp = client
            .get(url)
            .header("Accept", "application/json, text/csv, text/plain, */*")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    CoreError::InvalidArgument(format!("timeout: 请求超时: {url}"))
                } else {
                    CoreError::InvalidArgument(format!("network: 请求失败: {e}"))
                }
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::InvalidArgument(format!(
                "status: HTTP {} ({url})",
                status.as_u16()
            )));
        }
        // 体积上限要**边读边判**(审核发现): 先 `resp.bytes()` 读完再比长度, 6MiB 的响应会被
        // 完整下载并读进内存后才报 too_large —— 那叫"读完再判", 不是体积/内存护栏。
        // 这里改成流式累加: 超过上限立刻断开, 不把多余数据读进内存。
        // 有 `Content-Length` 时先预检(连读都不读), 没有时按块累加兜底。
        if let Some(len) = resp.content_length() {
            if len as usize > policy.max_bytes {
                return Err(CoreError::InvalidArgument(format!(
                    "too_large: 响应体 {len} 字节超过上限 {} 字节(Content-Length 预检)",
                    policy.max_bytes
                )));
            }
        }
        let mut body: Vec<u8> = Vec::new();
        // 用 `chunk()` 逐块读(reqwest 默认可用), 因此不需要为此启用 `stream` feature。
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| CoreError::InvalidArgument(format!("network: 读取响应失败: {e}")))?
        {
            if body.len() + chunk.len() > policy.max_bytes {
                return Err(CoreError::InvalidArgument(format!(
                    "too_large: 响应体超过上限 {} 字节(读取中截断, 已读 {} + 本块 {})",
                    policy.max_bytes,
                    body.len(),
                    chunk.len()
                )));
            }
            body.extend_from_slice(&chunk);
        }
        String::from_utf8(body)
            .map_err(|e| CoreError::InvalidArgument(format!("network: 响应不是合法 UTF-8: {e}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_core::SourceRegistry;
    use ricow_strategy::Database;

    /// 测试用自己的本地 HTTP 服务(真实 socket, 不替身我们的代码路径)。
    ///
    /// 为什么不用外部站点: 单元测试不能依赖公网可达性; 这里要验的是**我们的超时/体积/状态码
    /// 分类逻辑**, 用本地服务器才能精确构造"慢/大/非 2xx"三种情形。
    async fn spawn_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        // all_sent: 服务器是否把 /big 的整块响应都写出去了(客户端提前断开 → 写失败/半截)。
        // 用它证明"体积上限是边读边判"而不是"读完再判"(审核发现旧实现会把 6MiB 全读进内存)。
        let all_sent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let all_sent_srv = all_sent.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let all_sent = all_sent_srv;
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let all_sent = all_sent.clone(); // 每连接一份(外层 loop 还要用)
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let resp = match path.as_str() {
                        "/ok" => "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_string(),
                        "/500" => {
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 3\r\n\r\nbad"
                                .to_string()
                        }
                        "/big" => {
                            let body = "x".repeat(8 * 1024 * 1024);
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                                body.len()
                            )
                        }
                        "/slow" => {
                            tokio::time::sleep(Duration::from_millis(800)).await;
                            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()
                        }
                        _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\n404".to_string(),
                    };
                    if path == "/big" {
                        all_sent
                            .store(sock.write_all(resp.as_bytes()).await.is_ok(), Ordering::SeqCst);
                    } else {
                        let _ = sock.write_all(resp.as_bytes()).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), all_sent)
    }

    async fn host_with(policy: HttpPolicy) -> EngineHost {
        let db = Database::open_in_memory().await.unwrap();
        let hub = Arc::new(DataHub::new(SourceRegistry::new(), db));
        EngineHost::new(hub, 0, false).unwrap().with_http_policy(policy)
    }

    /// T033: 正常 GET 返回响应体文本(本地服务器, 真实 socket)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn http_fetch_returns_body() {
        let (base, _all_sent) = spawn_server().await;
        let host = host_with(HttpPolicy::default()).await;
        assert_eq!(host.http_get(&format!("{base}/ok")).unwrap(), "hello");
    }

    /// T034: 四类失败**可判别**(错误文案带类别前缀)、且都不 panic、不中断调用方。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn http_errors_are_distinguishable() {
        let (base, all_sent) = spawn_server().await;

        // ① 非法 URL: 不发请求就能判(前缀 invalid_url)
        let host = host_with(HttpPolicy::default()).await;
        let e = host.http_get("ftp://example.com/x").unwrap_err().to_string();
        // 文案前缀由 CoreError 的 Display 再加一层("invalid argument: "), 所以判 contains。
        assert!(e.contains("invalid_url"), "应能判别为非法 URL, got: {e}");

        // ② 非 2xx: 前缀 status + 状态码
        let e = host.http_get(&format!("{base}/500")).unwrap_err().to_string();
        assert!(e.contains("status") && e.contains("500"), "got: {e}");

        // ③ 超体积: 上限设 512 字节, 服务器给 8MiB。
        //    ⚠️ 这里不只看错误前缀 —— 还断言**服务器没能把整块发完**: 旧实现是 `resp.bytes()`
        //    读完再比长度(8MiB 会被完整下载并读进内存后才报 too_large), 那叫"读完再判";
        //    现在是 Content-Length 预检 + 逐块累加, 客户端提前断开, 服务器写不完。
        let small = host_with(HttpPolicy { timeout: Duration::from_secs(5), max_bytes: 512 }).await;
        let e = small.http_get(&format!("{base}/big")).unwrap_err().to_string();
        assert!(e.contains("too_large"), "got: {e}");
        assert!(
            !all_sent.load(Ordering::SeqCst),
            "超上限的响应不得被整块读走(否则体积上限只是'读完再判', 不是护栏)"
        );

        // ④ 超时: 上限 150ms, 服务器 800ms 才回
        let impatient =
            host_with(HttpPolicy { timeout: Duration::from_millis(150), max_bytes: 1024 }).await;
        let e = impatient.http_get(&format!("{base}/slow")).unwrap_err().to_string();
        assert!(e.contains("timeout"), "got: {e}");

        // ⑤ 连不上(端口无人监听): 也必须被**分类**(network 或 timeout —— 取决于本机对
        //    黑洞端口是 RST 还是丢包), 不 panic、不让调用方无法判别。
        let dead =
            host_with(HttpPolicy { timeout: Duration::from_millis(300), max_bytes: 1024 }).await;
        let e = dead.http_get("http://127.0.0.1:1/").unwrap_err().to_string();
        assert!(
            e.contains("network") || e.contains("timeout"),
            "连不上应归类为 network/timeout, got: {e}"
        );

        // 失败之后同一宿主仍可继续工作(不中毒):
        assert_eq!(host.http_get(&format!("{base}/ok")).unwrap(), "hello");
    }
}
