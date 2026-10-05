//! 前端静态资产(025 / D4, 032 起拆多份): 页面、样式与各视图脚本。
//!
//! 全部**编译期嵌进二进制**(`include_str!`), 运行期不依赖工作目录、不依赖外部 CDN;
//! 页面里的资源 URL 由 [`index`] 按本次请求的 token 现场填 —— 浏览器取 `<link>` /
//! `<script>` 带不上请求头, 只能把 token 拼进查询串(与 [`super::auth::token_of`] 同一条取值路径)。

use axum::extract::Request;
use axum::http::header;
use axum::response::IntoResponse;

use super::auth::token_of;

/// 单页与各份资产。
const INDEX_HTML: &str = include_str!("assets/index.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
/// lightweight-charts v4 UMD(032 T001): 第三方图表库, 文件头保留 Apache-2.0 许可注释。
const LWC_JS: &str = include_str!("assets/lightweight-charts.js");
/// 公共件: TOKEN / api / TEXT 字典 / 通用 i18n 与纯工具(032 T002)。
const COMMON_JS: &str = include_str!("assets/common.js");
/// hash 路由与视图注册表(032 T003)。
const ROUTER_JS: &str = include_str!("assets/router.js");
/// 对话视图: 会话 / SSE / 三个只读面板逻辑(032 T006)。
const CHAT_JS: &str = include_str!("assets/chat.js");
/// 设置视图(032 US1): 市场视野开关; 密钥配置已迁至 033 密钥视图。
const SETTINGS_JS: &str = include_str!("assets/settings.js");
/// 密钥视图(033): 多套 AI / 币安密钥的别名条目 —— 列表 / 新增 / 选用 / 删除, 读写 `/api/keys*`。
const KEYS_JS: &str = include_str!("assets/keys.js");
/// 市场视图(032 US2): 交易对列表 / 订单簿 / K 线, 读写 `/api/markets*`。
const MARKETS_JS: &str = include_str!("assets/markets.js");
/// 策略管理视图(032 US3): 策略列表 / 源码编辑保存 / AI 改 Lua / 回测作业。
const STRATEGIES_JS: &str = include_str!("assets/strategies.js");
/// 运行管理视图(032 US4): 实例一览 / 启停 / 风险确认 / 行内日志。
const RUNS_JS: &str = include_str!("assets/runs.js");
/// 入口: 初始化对话视图 + router 首跳(032 T006)。
const APP_JS: &str = include_str!("assets/app.js");

/// 页面里各静态资源 URL 的 token 占位符, 由 [`index`] 按本次请求的 token 替换。
const TOKEN_PLACEHOLDER: &str = "__RICOW_TOKEN__";

/// 单页首页(FR-003 / FR-006): 把本次 token 填进各静态资源的 URL(见 [`TOKEN_PLACEHOLDER`])。
pub(super) async fn index(req: Request) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        render_index(&token_of(&req).unwrap_or_default()),
    )
}

/// 样式表: 编译期常量, 不含任何会话内容。
pub(super) async fn style_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE_CSS)
}

/// 第三方图表库(032 T001): 编译期常量, 不含任何会话内容。
pub(super) async fn lwc_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], LWC_JS)
}

/// 前端公共件(032 T002): 编译期常量, 不含任何会话内容。
pub(super) async fn common_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], COMMON_JS)
}

/// 前端 hash 路由(032 T003): 编译期常量, 不含任何会话内容。
pub(super) async fn router_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], ROUTER_JS)
}

/// 前端对话视图(032 T006): 编译期常量, 不含任何会话内容。
pub(super) async fn chat_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], CHAT_JS)
}

/// 前端设置视图(032 US1): 编译期常量, 不含任何会话内容。
pub(super) async fn settings_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], SETTINGS_JS)
}

/// 前端密钥视图(033): 编译期常量, 不含任何会话内容。
pub(super) async fn keys_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], KEYS_JS)
}

/// 前端市场视图(032 US2): 编译期常量, 不含任何会话内容。
pub(super) async fn markets_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], MARKETS_JS)
}

/// 前端策略管理视图(032 US3): 编译期常量, 不含任何会话内容。
pub(super) async fn strategies_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], STRATEGIES_JS)
}

/// 前端运行管理视图(032 US4): 编译期常量, 不含任何会话内容。
pub(super) async fn runs_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], RUNS_JS)
}

/// 前端入口脚本: 编译期常量, 不含任何会话内容。
pub(super) async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], APP_JS)
}

/// 替换页面里的 [`TOKEN_PLACEHOLDER`](单独成函数是为了能直接测)。
fn render_index(token: &str) -> String {
    INDEX_HTML.replace(TOKEN_PLACEHOLDER, token)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **全部**前端脚本 `(文件名, 源码)`: 下面三处纪律扫描只认这一份清单。
    ///
    /// 单一清单是为了根治一类静默漏洞: 之前三处各写一份数组, 033 新增 `keys.js` 时没往任何
    /// 一处追加 —— 存储红线 / 只读红线 / 首页资源清单三项断言集体漏扫该文件, 且全绿(2026-10-05
    /// 审查发现)。新增脚本**必须**进这里, 加漏了 `test_all_js_assets_are_listed_on_index` 会红。
    const ALL_JS: &[(&str, &str)] = &[
        ("lightweight-charts.js", LWC_JS),
        ("common.js", COMMON_JS),
        ("router.js", ROUTER_JS),
        ("chat.js", CHAT_JS),
        ("settings.js", SETTINGS_JS),
        ("keys.js", KEYS_JS),
        ("markets.js", MARKETS_JS),
        ("strategies.js", STRATEGIES_JS),
        ("runs.js", RUNS_JS),
        ("app.js", APP_JS),
    ];

    /// `ALL_JS` 与首页资源引用必须双向对齐: 少一个 → 有脚本没被扫描; 多一个 → 首页漏引用。
    #[test]
    fn test_all_js_assets_are_listed_on_index() {
        for (name, js) in ALL_JS {
            assert!(!js.is_empty(), "{name} 不得为空");
            assert!(
                INDEX_HTML.contains(&format!("/{name}?token=")),
                "首页须引用带 token 的 /{name}"
            );
        }
        // 首页里引用的每份 .js 也都要在 ALL_JS 里(只加首页不更清单 = 该文件漏扫)。
        for chunk in INDEX_HTML.split("src=\"/").skip(1) {
            let url = chunk.split('"').next().unwrap_or("");
            let file = url.split('?').next().unwrap_or("");
            if !file.ends_with(".js") {
                continue;
            }
            assert!(
                ALL_JS.iter().any(|(n, _)| *n == file),
                "/{file} 被首页引用但不在 ALL_JS 清单里(纪律扫描会漏掉它)"
            );
        }
    }

    /// 静态资源与首页(D4): 032 拆分后的各份资产都非空; 首页里每个资源 URL 必须带上本次 token ——
    /// 浏览器取 `<link>` / `<script>` 带不上请求头, 只能靠这里填进去。
    #[test]
    fn test_index_fills_token_into_asset_urls() {
        assert!(!INDEX_HTML.is_empty(), "首页不得为空");
        assert!(!STYLE_CSS.is_empty(), "样式表不得为空");
        for asset in [
            "/style.css",
            "/lightweight-charts.js",
            "/common.js",
            "/router.js",
            "/chat.js",
            "/settings.js",
            "/keys.js",
            "/markets.js",
            "/strategies.js",
            "/runs.js",
            "/app.js",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("{asset}?token=")),
                "首页须引用带 token 的 {asset}"
            );
        }
        let html = render_index("tok-123");
        assert!(html.contains("tok-123"), "资源 URL 应带上本次 token");
        assert!(!html.contains(TOKEN_PLACEHOLDER), "占位符必须全部替换掉");
    }

    /// SC-011(浏览器侧): 页面不往 `localStorage` / `sessionStorage` 写任何东西 —— 明文密钥与
    /// 对话内容都不该在浏览器里留存; 密钥提示一到就切遮蔽输入, 提交时也不画用户气泡。
    ///
    /// 遮蔽**只能**走 CSS class: 输入区是多行 `<textarea>`, 它的 `type` 是只读属性, 赋值会抛
    /// `TypeError: Cannot set property type of #<HTMLTextAreaElement> which has only a getter`
    /// 并把 `openSession` / `submitText` 打断(2026-09-19 实机走查发现)。
    #[test]
    fn test_frontend_stores_nothing_and_masks_secret_input() {
        // 扫描覆盖**全部**前端脚本(含第三方图表库 UMD 与 033 的 keys.js): 一份都不许碰浏览器存储。
        for store in ["localStorage", "sessionStorage"] {
            for (name, js) in ALL_JS {
                assert!(!js.contains(store), "{name} 不得使用 {store}(SC-011)");
            }
        }
        assert!(CHAT_JS.contains(r#"case "secret_prompt""#), "前端要处理密钥提示帧(FR-012)");
        assert!(
            CHAT_JS.contains(r#"els.input.classList.toggle("masked""#),
            "密钥录入期间输入框须遮蔽回显(SC-011)"
        );
        for (name, js) in ALL_JS {
            assert!(
                !js.contains("input.type =") && !js.contains("input.type="),
                "{name}: `<textarea>` 的 type 只读, 赋值会抛 TypeError"
            );
        }
        assert!(STYLE_CSS.contains("#input.masked"), "遮蔽样式须随前端一并内嵌(D4)");
        assert!(
            CHAT_JS.contains("!value.trim() && !state.secret"),
            "密钥期须放行空行(提示语承诺的\"回车放弃\", 服务端按空值不改配置)"
        );
        assert!(
            COMMON_JS.contains("const suffix = R.lang;"),
            "`dataset` 键名是首字母小写驼峰, 取大写得 undefined(静态文案整片空白 / placeholder 变字面量)"
        );
    }

    /// D17 / FR-013(只读红线): 右侧两块面板**只读** —— 页面上没有任何直连交易所的按钮, 前端也
    /// 不对交易 / 日志端点发写请求; 撤单 / 平仓 / 停机仍然只能在对话里确认(D19 不做交易所直连)。
    #[test]
    fn test_frontend_panels_are_read_only() {
        // 先确认面板真在, 否则下面全是空转(032 后面板整体迁入 #view-chat, id 不变)。
        assert!(
            INDEX_HTML.contains(r#"id="trade-panel""#) && INDEX_HTML.contains(r#"id="log-panel""#)
        );
        assert!(CHAT_JS.contains(r#"api("/api/trades/positions")"#), "交易面板数据须来自只读端点");

        // 面板区里没有按钮、没有内联事件 —— 只展示, 不动作。
        let start = INDEX_HTML.find(r#"<aside id="panels">"#).expect("右栏面板");
        let end = INDEX_HTML[start..].find("</aside>").expect("面板收尾") + start;
        let panels = &INDEX_HTML[start..end];
        assert!(!panels.contains("<button"), "面板是只读的, 不得有按钮(D17)");
        assert!(!panels.contains("onclick"), "面板是只读的, 不得有内联事件(D17)");

        // 这两组端点在对话视图里只以 GET 出现(032: 逻辑迁入 chat.js)。
        for line in CHAT_JS.lines().filter(|l| l.contains("/api/trades") || l.contains("/api/logs"))
        {
            assert!(!line.contains("method:"), "交易 / 日志端点只读, 不得带写方法: {line}");
        }
        // 前端**全部脚本**(含第三方图表库与 keys.js)里不出现交易所写动作的入口。
        for (name, js) in ALL_JS {
            for banned in ["cancel_order", "cancelOrder", "close_position", "place_order"] {
                assert!(
                    !js.contains(banned),
                    "{name}: 前端不得出现交易所写动作 `{banned}`(D17 / FR-013)"
                );
            }
        }
    }
}
