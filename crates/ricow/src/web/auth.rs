//! Web 鉴权与来源校验(025 / D3 / FR-002; 036 补来源门).
//!
//! 两道门, 顺序固定:
//! 1. **token**(一次性随机, 不落盘不进日志): 缺失或错误一律 `401` **且响应体为空**;
//!    携带方式: `Authorization: Bearer`(脚本) → 查询参数 `?token=`(首次进入 / SSE) →
//!    `ricow_token` cookie(浏览器会话)。查询串 token 校验通过时会**换发 HttpOnly cookie**:
//!    顶部导航(带 `Sec-Fetch-Mode: navigate`)直接 `302` 到去掉 token 的路径, 地址栏/历史
//!    里不再留 token(审计 安全-11); 其余请求(脚本/SSE/静态资源)原样放行并顺手种 cookie。
//! 2. **来源**(036): 写方法只接受**回环来源** —— token 因 `EventSource` 的限制会出现在
//!    URL 查询串里(可能经 Referer / 浏览器历史外流), 来源门是它外流后的一道闸: 别的站点
//!    拿着 token 也发不出**写**请求(读请求不改状态, 不设限以保 `curl` / 脚本可用)。

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::WebState;

/// 会话 cookie 名 (HttpOnly, SameSite=Strict; 审计 安全-11)。
const COOKIE_NAME: &str = "ricow_token";
/// cookie 有效期: token 本身随进程生命周期, 7 天后用启动时打印的 URL 重进即可。
const COOKIE_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// token 的携带方式(决定是否要换发 cookie)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenSource {
    /// `Authorization: Bearer`(脚本) — 不种 cookie。
    Bearer,
    /// 查询参数 `?token=` — 种 cookie; 顶部导航另做 302 换发。
    Query,
    /// `ricow_token` cookie — 已是会话态, 无需再种。
    Cookie,
}

/// token 校验 + 来源校验中间件: 缺失或错误**一律 `401` 且响应体为空**(不回任何内容, D3 / FR-002)。
pub(super) async fn require_token(
    State(state): State<WebState>,
    req: Request,
    next: Next,
) -> Response {
    let found = token_of(&req);
    match &found {
        Some((token, _)) if ct_eq(token, &state.token) => {}
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    }
    // 来源门(036): 只看**写方法**; 非回环来源一律 403(同样空体, 不解释原因)。
    if !origin_allowed(&req) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (token, source) = found.expect("上方已匹配 Some 且 token 相等");
    if source == TokenSource::Query {
        // 顶部导航: 302 到去掉 token 的路径 + 种 cookie, 地址栏/历史不留 token。
        if is_top_level_navigation(&req) {
            return (StatusCode::SEE_OTHER, redirect_headers(&req, &token)).into_response();
        }
        // 脚本/SSE/静态资源: 原样放行, 顺手把会话 cookie 种上(下次可不带 query token)。
        let mut resp = next.run(req).await;
        if let Ok(v) = header::HeaderValue::from_str(&session_cookie(&token)) {
            resp.headers_mut().insert(header::SET_COOKIE, v);
        }
        return resp;
    }
    next.run(req).await
}

/// `Set-Cookie` 值: HttpOnly(JS 读不到) + SameSite=Strict(跨站不带) + 限定路径。
fn session_cookie(token: &str) -> String {
    format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE_SECS}"
    )
}

/// 302 换发的响应头: `Location` 去掉 token 参数 + `Set-Cookie`。
fn redirect_headers(req: &Request, token: &str) -> [(header::HeaderName, String); 2] {
    let path = req.uri().path().to_string();
    let kept: Vec<&str> = req
        .uri()
        .query()
        .map(|q| {
            q.split('&')
                .filter(|kv| kv.split_once('=').map_or(!kv.is_empty(), |(k, _)| k != "token"))
                .collect()
        })
        .unwrap_or_default();
    let location = if kept.is_empty() { path } else { format!("{path}?{}", kept.join("&")) };
    [(header::LOCATION, location), (header::SET_COOKIE, session_cookie(token))]
}

/// 是否浏览器**顶部导航**(`Sec-Fetch-Mode: navigate` 或 `Sec-Fetch-Dest: document`)。
///
/// 只对导航做 302: `curl` / e2e 脚本 / SSE(`cors`) / WS 握手(`websocket`)没有这些头,
/// 照常直接返回 —— 重定向对非导航请求要么无意义要么直接破坏(SSE/WS 不能跟 302)。
fn is_top_level_navigation(req: &Request) -> bool {
    for name in ["sec-fetch-mode", "sec-fetch-dest"] {
        if let Some(v) = req.headers().get(name).and_then(|v| v.to_str().ok()) {
            let v = v.trim().to_ascii_lowercase();
            if (name == "sec-fetch-mode" && v == "navigate")
                || (name == "sec-fetch-dest" && v == "document")
            {
                return true;
            }
        }
    }
    false
}

/// 从请求中取 token 与携带方式: `Authorization: Bearer` → 查询参数 `?token=` → cookie。
///
/// 三种都收是因为: 浏览器 `EventSource` **无法自定请求头**, 只能把 token 挂在 URL 上;
/// cookie 是查询串 token 换发后的会话态。**查询参数优先于 cookie**: 进程重启后 token
/// 换新, 用户点的新链接必须能盖掉浏览器里残留的旧 cookie, 否则旧 cookie 会把新链接
/// 顶成 401, 用户明明拿对了地址却进不来。
pub(super) fn token_of(req: &Request) -> Option<(String, TokenSource)> {
    if let Some(value) = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(token) = value.strip_prefix("Bearer ") {
            return Some((token.to_string(), TokenSource::Bearer));
        }
    }
    if let Some(q) = req.uri().query() {
        let from_query = q.split('&').find_map(|kv| {
            let (key, value) = kv.split_once('=')?;
            (key == "token").then(|| value.to_string())
        });
        if let Some(token) = from_query {
            return Some((token, TokenSource::Query));
        }
    }
    let cookies = req.headers().get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|kv| {
        let (key, value) = kv.trim().split_once('=')?;
        (key == COOKIE_NAME).then(|| (value.to_string(), TokenSource::Cookie))
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
    fn test_token_of_reads_header_then_query_then_cookie() {
        // Authorization: Bearer 优先。
        let (t, s) =
            token_of(&req("GET", "/api/ping", &[("authorization", "Bearer abc")])).unwrap();
        assert_eq!(t, "abc");
        assert_eq!(s, TokenSource::Bearer);
        assert_eq!(token_of(&req("GET", "/api/ping", &[("authorization", "Basic abc")])), None);
        // 无头时退回查询参数; 只有名为 token 的那个键算数。
        let (t, s) = token_of(&req("GET", "/api/ping?token=xyz", &[])).unwrap();
        assert_eq!(t, "xyz");
        assert_eq!(s, TokenSource::Query);
        assert_eq!(token_of(&req("GET", "/api/ping?tokens=xyz", &[])), None);
        // cookie 再退一层; 多个 cookie 混排也能取到。
        let (t, s) =
            token_of(&req("GET", "/api/ping", &[("cookie", "other=1; ricow_token=ck; x=2")]))
                .unwrap();
        assert_eq!(t, "ck");
        assert_eq!(s, TokenSource::Cookie);
        // 全无 → None。
        assert_eq!(token_of(&req("GET", "/api/ping", &[])), None);
    }

    #[test]
    fn test_query_token_wins_over_stale_cookie() {
        // 进程重启后 token 换新: 新链接(query)必须盖过浏览器残留的旧 cookie,
        // 否则用户拿对了新地址也会被旧 cookie 顶成 401。
        let (t, s) =
            token_of(&req("GET", "/?token=fresh", &[("cookie", "ricow_token=stale")])).unwrap();
        assert_eq!(t, "fresh");
        assert_eq!(s, TokenSource::Query);
    }

    #[test]
    fn test_session_cookie_is_httponly_strict() {
        let c = session_cookie("tok");
        assert!(c.starts_with("ricow_token=tok;"), "{c}");
        assert!(c.contains("HttpOnly"), "{c}");
        assert!(c.contains("SameSite=Strict"), "{c}");
        assert!(c.contains("Path=/"), "{c}");
        assert!(c.contains("Max-Age="), "{c}");
    }

    #[test]
    fn test_redirect_headers_strip_only_token() {
        let r = req("GET", "/?token=t1&a=b&token=t2", &[]);
        let [(name_loc, loc), (name_ck, ck)] = redirect_headers(&r, "t1");
        assert_eq!(name_loc, header::LOCATION);
        assert_eq!(loc, "/?a=b", "全部 token 参数都应剥掉, 其余保留");
        assert_eq!(name_ck, header::SET_COOKIE);
        assert!(ck.contains("ricow_token=t1;"), "{ck}");

        // 只有 token → Location 不带问号。
        let r = req("GET", "/?token=t1", &[]);
        let [(name_loc, loc), _] = redirect_headers(&r, "t1");
        assert_eq!(name_loc, header::LOCATION);
        assert_eq!(loc, "/");

        // 无查询串 → 原路径。
        let r = req("GET", "/console", &[]);
        let [(_, loc), _] = redirect_headers(&r, "t1");
        assert_eq!(loc, "/console");
    }

    #[test]
    fn test_top_level_navigation_needs_sec_fetch() {
        assert!(is_top_level_navigation(&req("GET", "/", &[("sec-fetch-mode", "navigate")])));
        assert!(is_top_level_navigation(&req("GET", "/", &[("sec-fetch-dest", "document")])));
        // curl / e2e / SSE / WS 都不是导航 → 不做 302。
        assert!(!is_top_level_navigation(&req("GET", "/", &[])));
        assert!(!is_top_level_navigation(&req(
            "GET",
            "/api/logs/x/stream?token=t",
            &[("sec-fetch-mode", "cors")]
        )));
        assert!(!is_top_level_navigation(&req(
            "GET",
            "/ws?token=t",
            &[("sec-fetch-mode", "websocket"), ("sec-fetch-dest", "websocket")]
        )));
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
