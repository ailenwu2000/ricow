//! 前端静态资产(025 / D4, 032 起拆多份): 页面、样式与各视图脚本。
//!
//! 默认全部**编译期嵌进二进制**(`include_str!`), 运行期不依赖工作目录、不依赖外部 CDN;
//! 页面里的资源 URL 由 [`index`] 按本次请求的 token 现场填 —— 浏览器取 `<link>` /
//! `<script>` 带不上请求头, 只能把 token 拼进查询串(与 [`super::auth::token_of`] 同一条取值路径)。
//!
//! 041 加一条**开发回路**: `ricow web --web-assets-dir <DIR>` 时改从磁盘热读**同一批**文件
//! —— 改一行 JS 刷新即见, 免 `cargo build` 全量重编。发布形态不变(不给该开关就仍是内嵌副本)。

use std::path::{Path, PathBuf};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use ricow_core::{CoreError, CoreResult};

use super::auth::token_of;
use super::{WebError, WebState};

/// 一份可服务的前端资产。
pub(super) struct Asset {
    /// 文件名, 同时是 URL 路径(`/{name}`)。**磁盘寻址只认这个名字** —— 请求里的路径
    /// 从不参与拼路径, 故不存在 `..` 穿越与任意文件读取的入口(041 FR-2)。
    pub name: &'static str,
    /// 响应 `Content-Type`。
    pub ctype: &'static str,
    /// 编译期内嵌副本 —— 发布形态: 单二进制自带, 不依赖工作目录。
    pub embedded: &'static str,
}

const CTYPE_JS: &str = "text/javascript; charset=utf-8";

/// **全部可服务资产**。运行时路由表(见 [`routes`])与 `--web-assets-dir` 的磁盘白名单
/// **都由这一份派生** —— 加一份前端脚本只改这里, 不可能出现"路由与清单脱钩"。
///
/// 单一清单是为了根治一类**静默**漏洞: 032 新增 `keys.js` 时, 三处各自手写的清单没有
/// 一处被追加 —— 存储红线 / 只读红线 / 首页资源清单三项断言**集体漏扫该文件且全绿**
/// (2026-10-05 审查发现)。041 把"路由"也并进这份派生关系, 少了最后一处手抄点。
pub(super) const ASSETS: &[Asset] = &[
    Asset {
        name: "style.css",
        ctype: "text/css; charset=utf-8",
        embedded: include_str!("assets/style.css"),
    },
    // lightweight-charts v4 UMD(032 T001): 第三方图表库, 文件头保留 Apache-2.0 许可注释。
    Asset {
        name: "lightweight-charts.js",
        ctype: CTYPE_JS,
        embedded: include_str!("assets/lightweight-charts.js"),
    },
    // 公共件(032 T002): TOKEN / api / TEXT 字典 / 通用 i18n 与纯工具。
    Asset { name: "common.js", ctype: CTYPE_JS, embedded: include_str!("assets/common.js") },
    // hash 路由与视图注册表(032 T003)。
    Asset { name: "router.js", ctype: CTYPE_JS, embedded: include_str!("assets/router.js") },
    // 受限 markdown 渲染器(044): AI 助手气泡的结构化渲染, 只产 DOM(chart.js 在 renderRich 里用)。
    // **必须排在 chat.js 之前** —— 加载顺序即契约, 见本文件末尾的顺序测试。
    Asset { name: "markdown.js", ctype: CTYPE_JS, embedded: include_str!("assets/markdown.js") },
    // 对话视图(032 T006): 会话 / SSE / 三个只读面板逻辑。
    Asset { name: "chat.js", ctype: CTYPE_JS, embedded: include_str!("assets/chat.js") },
    // 设置视图(032 US1): 市场视野开关; 密钥配置已迁至 033 密钥视图。
    Asset { name: "settings.js", ctype: CTYPE_JS, embedded: include_str!("assets/settings.js") },
    // 密钥视图(033): 多套 AI / 币安密钥的别名条目。
    Asset { name: "keys.js", ctype: CTYPE_JS, embedded: include_str!("assets/keys.js") },
    // 市场视图(032 US2, 040 起含 SSE 实时): 交易对列表 / 订单簿 / K 线。
    Asset { name: "markets.js", ctype: CTYPE_JS, embedded: include_str!("assets/markets.js") },
    // 策略管理视图(032 US3, 042 拆四份): 外壳 —— 命名空间/工具/常量/共享状态 + 列表态 + 新建/复制。
    // **必须排在三份子文件之前**: 子文件在加载期从 `R.sg` 取工具与常量, 顺序即契约。
    Asset {
        name: "strategies.js",
        ctype: CTYPE_JS,
        embedded: include_str!("assets/strategies.js"),
    },
    // 042: 策略视图拆出的三份子文件(顺序即加载顺序, 见上面那条注释)。
    Asset {
        name: "strategy_form.js",
        ctype: CTYPE_JS,
        embedded: include_str!("assets/strategy_form.js"),
    },
    Asset {
        name: "strategy_editor.js",
        ctype: CTYPE_JS,
        embedded: include_str!("assets/strategy_editor.js"),
    },
    Asset {
        name: "strategy_backtest.js",
        ctype: CTYPE_JS,
        embedded: include_str!("assets/strategy_backtest.js"),
    },
    // 运行管理视图(032 US4): 实例一览 / 启停 / 风险确认 / 行内日志。
    Asset { name: "runs.js", ctype: CTYPE_JS, embedded: include_str!("assets/runs.js") },
    // 帮助抽屉(帮助计划第 1 条): 顶栏常驻入口 + 按当前视图组织的帮助内容。不注册视图,
    // 只绑按钮, 但要用公共件与 `R.navigate`, 故排在 common.js 之后、app.js(入口)之前。
    Asset { name: "help.js", ctype: CTYPE_JS, embedded: include_str!("assets/help.js") },
    // 命令面板(B7): Ctrl/⌘+K 的"去哪儿"入口。与 help.js 同一位置约束(用公共件 / R.navigate /
    // R.glossary 与 `R.help`, 不注册视图), 故排在 help.js 之后、app.js(入口)之前。
    Asset { name: "palette.js", ctype: CTYPE_JS, embedded: include_str!("assets/palette.js") },
    // 入口: 初始化对话视图 + router 首跳(032 T006)。
    Asset { name: "app.js", ctype: CTYPE_JS, embedded: include_str!("assets/app.js") },
];

/// 单页首页: 单独一条 —— 它要按本次 token 现填各资源 URL, 不是纯静态文件。
const INDEX_HTML: &str = include_str!("assets/index.html");

const CTYPE_HTML: &str = "text/html; charset=utf-8";

/// 页面里各静态资源 URL 的 token 占位符, 由 [`render_index`] 按本次请求的 token 替换。
const TOKEN_PLACEHOLDER: &str = "__RICOW_TOKEN__";

/// 资产来源(041): 默认内嵌; `--web-assets-dir` 给出时改从磁盘热读。
///
/// 这是**调试视图 vs 发布形态**的边界: 磁盘热读不改变什么会被发布 —— `cargo build` 内嵌的
/// 仍是仓库里那份文件, 纪律扫描(见本文件测试)照扫内嵌副本。磁盘目录按约定就是
/// `crates/ricow/src/web/assets/`, 故违规会在下一次构建 + 测试时变红。
#[derive(Clone, Default)]
pub(super) enum AssetSource {
    /// 发布形态: 只认编译期内嵌副本。
    #[default]
    Embedded,
    /// 开发形态: 从该目录热读。目录已由 [`resolve_assets_dir`] 规范化。
    Disk(PathBuf),
}

/// 校验并规范化 `--web-assets-dir` 目录(041 D4)。
///
/// 三条校验都放在**启动时**: 给错目录是配置错误, 该在终端一句话说清, 而不是等浏览器白屏
/// 后再让人猜。**不静默退回内嵌** —— 那会让"我明明传了目录怎么没生效"变成谜。
pub(super) fn resolve_assets_dir(dir: &Path) -> CoreResult<PathBuf> {
    let canon = std::fs::canonicalize(dir).map_err(|e| {
        CoreError::InvalidArgument(format!("--web-assets-dir 目录不可用({}): {e}", dir.display()))
    })?;
    if !canon.is_dir() {
        return Err(CoreError::InvalidArgument(format!(
            "--web-assets-dir 不是目录: {}",
            canon.display()
        )));
    }
    if !canon.join("index.html").is_file() {
        return Err(CoreError::InvalidArgument(format!(
            "--web-assets-dir 目录里没有 index.html, 不像前端资产目录: {}",
            canon.display()
        )));
    }
    Ok(canon)
}

/// 一次资产读取的结果。
///
/// 内嵌副本保持**零拷贝**(直接是 `&'static str`): 内嵌才是生产形态, 不该为调试能力付出
/// 每请求一次 17 万字符的拷贝(041 D10)。
enum Loaded {
    Static(&'static str),
    Owned(String),
}

impl IntoResponse for Loaded {
    fn into_response(self) -> Response {
        match self {
            Loaded::Static(s) => s.into_response(),
            Loaded::Owned(s) => s.into_response(),
        }
    }
}

/// 组装一份文本响应。
///
/// `no_store` **只对磁盘来源**为真: 开发回路要的是"改完刷新即见", 而浏览器缓存正是这件事的
/// 天敌。内嵌来源保持原有响应头, 不为调试能力改动发布行为(041 D5, 有意的不对称)。
fn text_response(body: Loaded, ctype: &'static str, no_store: bool) -> Response {
    let mut resp = body.into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static(ctype));
    if no_store {
        headers.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    }
    resp
}

/// 从磁盘读一份资产。
///
/// 缺文件 / 读不动 → 500 且报出**文件名与真实路径**, **不静默退回内嵌副本**(041 FR-4):
/// 退回会让"我改了怎么没生效"变成谜 —— 同 040 的"绝不静默降级"口径。
async fn read_file(dir: &Path, name: &str) -> Result<String, WebError> {
    let path = dir.join(name);
    tokio::fs::read_to_string(&path).await.map_err(|e| {
        WebError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("--web-assets-dir 里读不到 {name} ({}): {e}", path.display()),
            None,
            None,
        )
    })
}

/// 取一份资产(内嵌或磁盘)。`name` 由路由闭包按**编译期常量**传入 —— 从不来自请求。
async fn serve(State(state): State<WebState>, name: &'static str) -> Response {
    match load(&state, name).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

async fn load(state: &WebState, name: &str) -> Result<Response, WebError> {
    let Some(asset) = ASSETS.iter().find(|a| a.name == name) else {
        // 只有本文件的路由会调到 `serve`, 名字必然在表里; 真发生说明路由与表脱钩了。
        return Err(WebError::not_found(format!("未知前端资源: {name}")));
    };
    match &state.assets {
        AssetSource::Embedded => {
            Ok(text_response(Loaded::Static(asset.embedded), asset.ctype, false))
        }
        AssetSource::Disk(dir) => {
            let text = read_file(dir, asset.name).await?;
            Ok(text_response(Loaded::Owned(text), asset.ctype, true))
        }
    }
}

/// 单页首页(FR-003 / FR-006): 把本次 token 填进各静态资源的 URL(见 [`TOKEN_PLACEHOLDER`])。
///
/// **两种来源都要做 token 替换** —— 磁盘模式的 `index.html` 少了这一步, 页面会带着字面量
/// `__RICOW_TOKEN__` 去取资源, 结果整页 401 白屏。
pub(super) async fn index(State(state): State<WebState>, req: Request) -> Response {
    let token = token_of(&req).map(|(t, _)| t).unwrap_or_default();
    match &state.assets {
        AssetSource::Embedded => {
            text_response(Loaded::Owned(render_index(INDEX_HTML, &token)), CTYPE_HTML, false)
        }
        AssetSource::Disk(dir) => match read_file(dir, "index.html").await {
            Ok(text) => text_response(Loaded::Owned(render_index(&text, &token)), CTYPE_HTML, true),
            Err(e) => e.into_response(),
        },
    }
}

/// 资产路由表: **由 [`ASSETS`] 循环生成** —— 路由与磁盘白名单共用同一份清单, 不可能脱钩。
///
/// 全部挂在同一道 token 中间件之后(041 FR-7): 不新开端口、不新增放行口。
pub(super) fn routes() -> Router<WebState> {
    let mut router = Router::new().route("/", get(index));
    for asset in ASSETS {
        let name = asset.name;
        router = router.route(&format!("/{name}"), get(move |st: State<WebState>| serve(st, name)));
    }
    router
}

/// 替换页面里的 token 占位符(单独成函数是为了能直接测)。
fn render_index(template: &str, token: &str) -> String {
    template.replace(TOKEN_PLACEHOLDER, token)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────────
    // 这份用例集合是**防手抄漂移的门禁**(051 C4 明确表述边界), 不是行为验证。
    //
    // 它们断言的是"某个字面量/结构仍在源码里" —— 例如 `assert!(common.contains("R.emptyState = function"))`。
    // 价值在于: 清单、加载顺序、禁用 API、只读红线这些**结构性约定**一旦被手滑改掉就立刻变红。
    // **不要**把它们当成"功能被验证了": 重构只要保留那些字符串就全绿, 改个变量名反而可能假红。
    //
    // 真正验行为的只有两处, 分工不同:
    //   · `scripts/web_frontend_check.mjs` —— 静态一致性(双语键、逐标签配对、死文案、err toast 口径、parseHash 纯函数表);
    //   · `scripts/probe_web_ui.mjs` —— **真机**(CDP 驱动真浏览器: 逐视图激活 + 未捕获异常 + 窄屏抽屉 + 主题)。
    // 042 的 `S.onUndo` 漏导出教训就是: 上面几道静态门全绿, 而页面整片 ReferenceError。
    // ─────────────────────────────────────────────────────────────────────────────

    /// 按名取资产的**内嵌**副本(测试里断言具体内容用)。
    fn src_of(name: &str) -> &'static str {
        ASSETS.iter().find(|a| a.name == name).expect("资产表里有这份").embedded
    }

    /// 全部前端**脚本** `(文件名, 源码)`: 下面三条纪律扫描只认这一份(派生自 [`ASSETS`], 滤掉样式表)。
    fn all_js() -> impl Iterator<Item = (&'static str, &'static str)> {
        ASSETS.iter().filter(|a| a.name.ends_with(".js")).map(|a| (a.name, a.embedded))
    }

    /// 资产表自检(041 D2 的结构保证): 名字唯一、平铺(不含路径分隔符)、类型是文本。
    ///
    /// 平铺这一条是安全属性而非风格: 磁盘寻址是 `目录.join(名字)`, 名字里一旦能出现
    /// `/` 或 `..`, 白名单就被绕过了。名字来自本文件的常量表, 故这里守住即可。
    #[test]
    fn test_asset_table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for a in ASSETS {
            assert!(seen.insert(a.name), "资产名重复: {}", a.name);
            assert!(!a.name.contains('/') && !a.name.contains('\\'), "资产名须平铺: {}", a.name);
            assert!(
                a.name.ends_with(".js") || a.name.ends_with(".css"),
                "未知资产类型: {}",
                a.name
            );
            assert!(a.ctype.starts_with("text/"), "{} 应是文本类型: {}", a.name, a.ctype);
            assert!(!a.embedded.is_empty(), "{} 不得为空", a.name);
        }
    }

    /// `ASSETS` 与首页资源引用必须双向对齐: 少一个 → 该资产没人引用(等于死代码);
    /// 多一个 → 首页引用了一个**路由不会生成**的名字(浏览器 404), 或清单漏了该文件。
    #[test]
    fn test_all_js_assets_are_listed_on_index() {
        for (name, js) in ASSETS.iter().map(|a| (a.name, a.embedded)) {
            assert!(!js.is_empty(), "{name} 不得为空");
            assert!(
                INDEX_HTML.contains(&format!("/{name}?token=")),
                "首页须引用带 token 的 /{name}"
            );
        }
        // 首页里引用的每份 .js 也都要在清单里(只加首页不更清单 = 该文件漏扫, 且路由不存在)。
        for chunk in INDEX_HTML.split("src=\"/").skip(1) {
            let url = chunk.split('"').next().unwrap_or("");
            let file = url.split('?').next().unwrap_or("");
            if !file.ends_with(".js") {
                continue;
            }
            assert!(
                ASSETS.iter().any(|a| a.name == file),
                "/{file} 被首页引用但不在资产表里(纪律扫描会漏掉它, 且路由不存在)"
            );
        }
    }

    /// 静态资源与首页(D4): 各份资产都非空; 首页里每个资源 URL 必须带上本次 token ——
    /// 浏览器取 `<link>` / `<script>` 带不上请求头, 只能靠这里填进去。
    #[test]
    fn test_index_fills_token_into_asset_urls() {
        assert!(!INDEX_HTML.is_empty(), "首页不得为空");
        for asset in ASSETS {
            assert!(
                INDEX_HTML.contains(&format!("/{}?token=", asset.name)),
                "首页须引用带 token 的 /{}",
                asset.name
            );
        }
        let html = render_index(INDEX_HTML, "tok-123");
        assert!(html.contains("tok-123"), "资源 URL 应带上本次 token");
        assert!(!html.contains(TOKEN_PLACEHOLDER), "占位符必须全部替换掉");
    }

    /// 043: 顶部导航的 303 换发会把 token 从地址栏拿掉, 而 SSE/WS **只能**把 token 挂在 URL 上 ——
    /// 故页面必须有第二处 token 载体, 且 `R.TOKEN` 必须真的读它。
    ///
    /// 回归背景(`f0e65f9`, 2026-10-06): 303 换发上线后 `location.search` 里不再有 token →
    /// `R.TOKEN` 为空 → `"?token=" + ""` 拼出的**空 query token** 顶掉了后面那条有效 cookie
    /// (`auth.rs::token_of` 里 query 优先于 cookie 且不判空) → 页面上**所有** EventSource 401,
    /// 而 `R.sse` 的 `onfail` 是空的, 于是全程静默: 对话收不到流式回复、日志/行情实时全断、
    /// 发完第一条消息输入区永久禁用。
    ///
    /// 为什么当时没红: `curl` 与 `e2e_web.py` 都不带 `Sec-Fetch-Mode: navigate`, 触发不到 303,
    /// 所以那句"真机冒烟 144 passed"**结构性**覆盖不到这条路径。这条单测是补洞的第一道闸,
    /// 第二道在 `e2e_web.py` 的"模拟浏览器导航"段。
    #[test]
    fn test_index_carries_token_for_js_when_query_is_gone() {
        assert!(
            INDEX_HTML.contains(r#"name="ricow-token""#),
            "首页须带 token 载体(043): 303 换发后 location.search 里没有 token, 而 SSE/WS 只能挂 URL"
        );
        assert!(
            INDEX_HTML.contains(r#"name="ricow-token" content="__RICOW_TOKEN__""#),
            "载体的内容必须是服务端会替换的占位符, 否则浏览器拿到的是字面量"
        );
        let html = render_index(INDEX_HTML, "tok-123");
        assert!(
            html.contains(r#"name="ricow-token" content="tok-123""#),
            "渲染后载体须填上本次请求的 token"
        );

        let common = src_of("common.js");
        assert!(
            common.contains(r#"meta[name="ricow-token"]"#),
            "common.js 的 R.TOKEN 必须回落到 meta 载体, 否则 303 之后 token 为空"
        );
        // 下游确实靠 `R.TOKEN` 拼流式地址 —— 载体失效时这一串会静默 401, 故在这里点名。
        for name in ["chat.js", "markets.js", "runs.js"] {
            assert!(src_of(name).contains("R.TOKEN"), "{name} 的流式地址要用 R.TOKEN");
        }
    }

    /// 044: markdown 渲染器**只产 DOM**, 不碰 `innerHTML`。
    ///
    /// AI 的回答是**不可信内容**。走 `innerHTML` 就得维护一套转义 + 白名单, 漏一处就是 XSS;
    /// `createElement` / `textContent` 路线从结构上就没有注入面, 也不需要给安全扫描开白名单
    /// —— 分析文档原本建议"只对渲染器产物放行 innerHTML 并改 `assets.rs` 白名单", 已明确**不采纳**。
    #[test]
    fn test_markdown_renderer_is_dom_only() {
        let md = src_of("markdown.js");
        for banned in
            [".innerHTML", "innerHTML =", ".outerHTML", "insertAdjacentHTML", "document.write"]
        {
            assert!(
                !md.contains(banned),
                "markdown.js 不得使用 `{banned}`(AI 输出不可信, 只走 DOM API)"
            );
        }
        // 链接协议白名单: 只放行 http(s), 其余(`javascript:` / `data:`)降级为纯文本。
        assert!(md.contains("https?"), "markdown.js 须含链接协议白名单");
        assert!(md.contains("R.markdown = { render: render }"), "渲染入口须是 R.markdown.render");
        assert!(src_of("chat.js").contains("R.markdown"), "chat.js 须真的调用渲染器");
    }

    /// 044: 渲染器必须**先于**消费者加载 —— 与 042 的策略外壳同一条契约(加载期取符号,
    /// 顺序反了就是整片 `undefined`, 且只在用户打开对话页时才静默现形)。
    /// 顺手把 `common.js`(工具/i18n/主题)必须先于所有视图脚本这条既有约定也钉住。
    #[test]
    fn test_markdown_loads_before_chat_and_common_loads_first() {
        let idx = |name: &str| {
            ASSETS
                .iter()
                .position(|a| a.name == name)
                .unwrap_or_else(|| panic!("资产表里缺 {name}"))
        };
        let pos = |name: &str| {
            INDEX_HTML.find(&format!("/{name}?token=")).unwrap_or_else(|| panic!("首页缺 /{name}"))
        };
        assert!(
            idx("markdown.js") < idx("chat.js"),
            "markdown.js 必须排在 chat.js 之前(加载顺序即契约)"
        );
        assert!(pos("markdown.js") < pos("chat.js"), "首页里 markdown.js 必须排在 chat.js 之前");
        let common = idx("common.js");
        for view in [
            "chat.js",
            "settings.js",
            "keys.js",
            "markets.js",
            "strategies.js",
            "runs.js",
            "help.js",
            "palette.js",
            "app.js",
        ] {
            assert!(common < idx(view), "common.js 必须排在所有视图脚本之前(缺 {view})");
            assert!(pos("common.js") < pos(view), "首页里 common.js 必须排在 {view} 之前");
        }
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
            for (name, js) in all_js() {
                assert!(!js.contains(store), "{name} 不得使用 {store}(SC-011)");
            }
        }
        let chat = src_of("chat.js");
        assert!(chat.contains(r#"case "secret_prompt""#), "前端要处理密钥提示帧(FR-012)");
        assert!(
            chat.contains(r#"els.input.classList.toggle("masked""#),
            "密钥录入期间输入框须遮蔽回显(SC-011)"
        );
        for (name, js) in all_js() {
            assert!(
                !js.contains("input.type =") && !js.contains("input.type="),
                "{name}: `<textarea>` 的 type 只读, 赋值会抛 TypeError"
            );
        }
        assert!(src_of("style.css").contains("#input.masked"), "遮蔽样式须随前端一并内嵌(D4)");
        assert!(
            chat.contains("!value.trim() && !state.secret"),
            "密钥期须放行空行(提示语承诺的\"回车放弃\", 服务端按空值不改配置)"
        );
        assert!(
            src_of("common.js").contains("const suffix = R.lang;"),
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
        let chat = src_of("chat.js");
        assert!(chat.contains(r#"api("/api/trades/positions")"#), "交易面板数据须来自只读端点");

        // 面板区里没有按钮、没有内联事件 —— 只展示, 不动作。
        let start = INDEX_HTML.find(r#"<aside id="panels">"#).expect("右栏面板");
        let end = INDEX_HTML[start..].find("</aside>").expect("面板收尾") + start;
        let panels = &INDEX_HTML[start..end];
        // 面板区里的**交互控件只能"看"**: 044 在日志面板加了搜索 / 级别 / 跟随三个**视图**控件,
        // 于是原来那句"一个 `<button>` 都不许有"的**代理**断言要换成显式允许清单 ——
        // 意图(D17: 面板没有直连交易所的动作)保住, 例外显式化: 往这里塞一个交易动作按钮依然会红。
        let allowed_view_controls = ["log-follow"];
        for chunk in panels.split("<button").skip(1) {
            let head = &chunk[..chunk.find('>').unwrap_or(chunk.len())];
            assert!(
                allowed_view_controls.iter().any(|id| head.contains(&format!(r#"id="{id}""#))),
                "面板区只允许视图控件按钮(白名单 {allowed_view_controls:?}), 这个不在其列: <button{head}>"
            );
        }
        for inline in ["onclick", "oninput", "onchange", "onsubmit", "onload"] {
            assert!(!panels.contains(inline), "面板是只读的, 不得有内联事件 `{inline}`(D17)");
        }

        // 这两组端点在对话视图里只以 GET 出现(032: 逻辑迁入 chat.js)。
        for line in chat.lines().filter(|l| l.contains("/api/trades") || l.contains("/api/logs")) {
            assert!(!line.contains("method:"), "交易 / 日志端点只读, 不得带写方法: {line}");
        }
        // 前端**全部脚本**(含第三方图表库与 keys.js)里不出现交易所写动作的入口。
        for (name, js) in all_js() {
            for banned in ["cancel_order", "cancelOrder", "close_position", "place_order"] {
                assert!(
                    !js.contains(banned),
                    "{name}: 前端不得出现交易所写动作 `{banned}`(D17 / FR-013)"
                );
            }
        }
    }

    /// 帮助计划第 7 条: 内置策略的**适用性**(清单里的 summary/description/suitable/unsuitable)
    /// 必须在**只读详情页**可见。
    ///
    /// 背景: 这四个字段早就写在策略 TOML 里(如 `shannon_grid.toml` 的"适用/不适用场景"),
    /// 但此前**只**出现在「清单编辑器」里 —— 那是用户策略新建/复制时才展开的表单, 而 8 个
    /// 内置策略的列表行只有一个「复制」按钮(主点击也被绑成复制), 用户**根本走不进只读详情页**,
    /// 于是选型只能靠名字猜。三条一起钉住:
    ///   ① 详情骨架有承载容器(`sg-fit-card` / `sg-fit-body`);
    ///   ② 渲染函数真的读清单那四个字段并填进去;
    ///   ③ 内置行有「详情」入口(否则容器是死的, 用户到不了)。
    /// 数据源是 `GET /api/strategies/{id}` 的扁平化 manifest, 后端零改动 —— 故这里只钉前端。
    #[test]
    fn test_strategy_suitability_visible_in_read_only_detail() {
        let shell = src_of("strategies.js");
        let form = src_of("strategy_form.js");
        assert!(shell.contains(r#"id="sg-fit-card""#), "详情骨架须有适用性卡片容器");
        assert!(shell.contains(r#"id="sg-fit-body""#), "详情骨架须有适用性卡片正文容器");
        assert!(form.contains("function renderFit("), "须有适用性渲染函数");
        assert!(form.contains("renderFit(detail);"), "renderDetail 须真的调用它");
        for field in ["summary", "description", "suitable", "unsuitable"] {
            assert!(
                form.contains(&format!("detail.{field}")),
                "适用性渲染须读取清单字段 `detail.{field}`"
            );
        }
        // 内置行的详情入口: 精确到这一行, 免得"用户策略本来就有 openDetail"把断言蒙过去。
        assert!(
            shell.contains(r#"h("button", "sg-btn sg-btn-mini", t("sgDetail"))"#),
            "内置策略列表行须有「详情」入口按钮(否则用户到不了只读详情页)"
        );
    }

    /// 帮助计划第 1 条: 顶栏常驻帮助入口 + 抽屉, 且**不复述后端的能力清单**。
    ///
    /// 单一真相是本条最容易做坏的地方: 在抽屉里手抄一份"13 类写动作 / N 个工具"最省事,
    /// 但后端 `help_text()` 一改这里就成假话。故钉住: 抽屉把能力清单**交回对话**(填 `/help`),
    /// 自己只拥有三块不与后端重叠的内容(上手步骤 / 常见问题 / 术语表)。
    #[test]
    fn test_help_drawer_defers_capability_list_to_backend() {
        let help = src_of("help.js");
        assert!(INDEX_HTML.contains(r#"id="help-drawer""#), "首页须有帮助抽屉骨架");
        assert!(INDEX_HTML.contains(r#"id="help-btn""#), "顶栏须有常驻帮助入口");
        assert!(help.contains("R.help = { open: open, close: close }"), "须导出 R.help");
        // 上下文相关: 抽屉按当前视图挑段落 —— 不是一张放之四海皆准的说明。
        for view in ["strategies", "runs", "chat", "markets"] {
            assert!(help.contains(&format!("{view}:")), "帮助抽屉的上下文映射须覆盖 {view} 视图");
        }
        // 单一真相: 能力清单只从对话取(`/help`), 这里不许出现后端清单的影子。
        assert!(help.contains(r#""/help""#), "帮助面板须把能力清单交回对话(`/help`)");
        for banned in ["READ_ONLY_TOOLS", "VIRTUAL_TOOLS", "写动作"] {
            assert!(!help.contains(banned), "帮助抽屉不得复述后端能力清单: `{banned}`");
        }
        // 术语表来自公共件的全局单例(A2): 前端**只此一处**取 `/api/terms`, 抽屉复用它 ——
        // 抽屉自己再取一份, 就会出现"抽屉里搜得到、页面上点不动"这种说不清的状态。
        assert!(help.contains("R.glossary"), "术语速查须复用全局术语单例(A2)");
        assert!(!help.contains(r#""/api/terms""#), "抽屉不得自行再取一份术语表(单一来源)");
    }

    /// A2: 术语表是**全局单例** —— 前端只此一处取 `/api/terms`, 各视图共享同一份索引。
    ///
    /// 背景: 此前术语只活在对话视图(索引是私有的, 解释浮层也挂在对话气泡上), 于是同一个
    /// 「最大回撤」在回测指标卡上就点不了 —— 信息都在, 只是到不了那层 UI。做法是把取数 /
    /// 索引 / 浮层上提到 `common.js` 的 `R.glossary`, 各视图只负责 `decorate` 挂载。
    /// 三条一起钉住, 少一条都会退回"各取一份、各自过期"：
    ///   ① 公共件有单例且取自接口(唯一来源);
    ///   ② 对话视图不再自建索引(否则两份索引谁先过期说不清);
    ///   ③ 回测结果的指标名真的挂了术语, 且完整报告显式 `allowPre`(默认跳过 `<pre>` 是给代码块的)。
    #[test]
    fn test_glossary_is_a_single_global_source() {
        let common = src_of("common.js");
        assert!(common.contains("R.glossary = (function ()"), "术语单例须落在公共件里");
        assert!(common.contains(r#""/api/terms""#), "术语索引须由公共件单点取用(唯一来源)");
        assert!(
            common.contains("if (R.glossary) R.glossary.refresh();"),
            "applyLang 须就地刷新解释浮层(浮层随索引一起上提了)"
        );

        let chat = src_of("chat.js");
        assert!(!chat.contains("const terms = new Map()"), "对话视图不得再自建术语索引");
        assert!(!chat.contains("function decorateTerms("), "装饰逻辑须复用公共件, 不复述一份");
        assert!(chat.contains("R.glossary.decorate(el)"), "对话气泡须挂全局术语");
        assert!(chat.contains("await R.glossary.load()"), "对话启动须预热全局术语表");

        // 回测页是指标名最集中的地方, 每处都要挂上; 报告在 `<pre>` 里, 必须显式放行。
        let bt = src_of("strategy_backtest.js");
        assert!(bt.contains("R.glossary.decorate(lbl)"), "指标卡名须挂术语");
        assert!(bt.contains("R.glossary.decorate(thPnl)"), "平仓明细表头须挂术语");
        assert!(bt.contains("allowPre: true"), "完整文本报告须显式放行 `<pre>`");
    }

    /// B4 + B6: 空状态要有"下一步", 成功提示要有"单一出口"。
    ///
    /// 两条针对同一个毛病 —— 提示明明指出了**去哪儿**, 却在用户真走过去的那一刻消失:
    /// · 空状态(B4): 一行灰字("无策略" / "—" / "无")说完就没了。统一改成 说明 + 导航按钮;
    ///   按钮只做**导航 / 打开已有入口**, 不替用户写入(与"写动作必须本人确认"同一姿态)。
    /// · 提示(B6): 各视图把成功提示写进自己的 msg 区, 一跨视图(复制完自动开详情、保存完回列表)
    ///   就丢。成功一路改走挂在 `body` 上的全局 toast; **错误留在原地** —— 错误是"这一屏此刻的
    ///   状态", 得贴着出错的控件看, 飘到右下角反而不对。
    #[test]
    fn test_empty_states_offer_next_step_and_toast_is_the_only_success_outlet() {
        let common = src_of("common.js");
        assert!(common.contains("R.emptyState = function"), "空状态构造器须落在公共件里");
        assert!(common.contains("R.toast = function"), "全局 toast 须落在公共件里");
        assert!(
            common.contains(r#"toastHost.setAttribute("role", "status")"#),
            "toast 须保留可访问性(role=status)"
        );
        assert!(
            common.contains(r#"toastHost.setAttribute("aria-live", "polite")"#),
            "toast 须 aria-live, 否则屏幕阅读器不会念"
        );
        assert!(common.contains("item.textContent = msg;"), "toast 文本只走 textContent");
        // 没有下一步动作时原样退回一行灰字 —— 调用点不必分两套代码。
        assert!(common.contains("if (!cta || !cta.label) return note;"), "空状态须兼容无 CTA");

        // 四个视图的空状态都要给出下一步(少一处就退回"一行灰字")。
        let chat = src_of("chat.js");
        assert!(chat.contains(r#"R.emptyState(t("emptySessions")"#), "零会话空态须给「新建」入口");
        assert!(
            chat.contains(r#"emptyRow(t("noStrategies"), { label: t("goStrategies")"#),
            "策略面板空态须带导航按钮"
        );
        assert!(
            src_of("runs.js").contains(r#"R.emptyState(t("rnNoInstances")"#),
            "零实例空态须带去策略页的入口"
        );
        assert!(
            src_of("markets.js").contains(r#"R.emptyState(t("mkEmptySymbols")"#),
            "视野空态须给「显示全部」入口"
        );
        assert!(
            src_of("strategies.js").contains(r#"h("span", "sg-firststep-t""#),
            "零自有策略时须给一条下一步(否则会误以为'反正内置都有了')"
        );

        // 成功一路走 toast: 这几处跨视图 / 长寿命的成功提示不再写本地 msg 区。
        let keys = src_of("keys.js");
        assert!(keys.contains(r#"R.toast(okText, "ok")"#), "密钥保存成功须走全局 toast");
        assert!(
            keys.contains(r#"R.toast(t("keysSaved"), "ok")"#),
            "两处密钥保存成功都须走全局 toast"
        );
        assert!(!keys.contains("showMsg(okText, true)"), "成功不得再写本地 msg 区");
        assert!(
            src_of("settings.js").contains(r#"R.toast(t("marketSaved"), "ok")"#),
            "设置保存成功须走全局 toast"
        );
        assert!(
            src_of("strategy_editor.js").contains(r#"R.toast(savedMsg, "ok")"#),
            "策略保存成功须走全局 toast"
        );
        assert!(
            src_of("strategies.js").contains(r#"R.toast(t("sgCopiedName") + name, "ok")"#),
            "复制成功后要跳到详情, 提示必须挂在全局才不丢"
        );
    }

    /// B5: 首次引导条 —— 只在**全新装置**上出现, 且判据全部是**服务端事实**。
    ///
    /// 为什么不能靠"是不是第一次打开": 那个状态只能存在浏览器里(纪律禁), 或者靠猜 —— 两者
    /// 错了都会变成"对着老用户喊你还没配密钥"。这里三条**稳定**事实各取一次: 零密钥 /
    /// 零自有策略 / 零实例; 任一条不成立就整条收起。而**探针读不到时闭嘴**: "读不到"不等于
    /// "还没开始用", 服务异常时弹一条"看起来是第一次用"比不显示更糟。
    ///
    /// ★ 曾经把"零会话"也当一条判据 —— 那是**竞态**: 对话视图一启动就自建一个会话, "有没有
    /// 会话"取决于探针与建会话谁先跑, 于是首屏偶尔能看到、任意一次 hashchange 重探后即消失
    /// (慢机上从不出现)。故这里**反过来钉住**: 探针不得再依赖 `/api/sessions`。
    #[test]
    fn test_onboard_bar_is_driven_by_server_facts_only() {
        assert!(INDEX_HTML.contains(r#"id="onboard-bar""#), "首页须有引导条骨架");
        let help = src_of("help.js");
        assert!(help.contains("async function probeOnboard("), "须有探针");
        for path in ["/api/keys", "/api/strategies", "/api/runs"] {
            assert!(help.contains(&format!(r#""{path}""#)), "探针须读服务端事实 {path}");
        }
        assert!(
            !help.contains(r#""/api/sessions""#),
            "引导条判据**不得**依赖会话数: chat 启动即自建会话, 拿它当'全新'判据是竞态(B5 回归)"
        );
        // 内容复用抽屉那份 GUIDE —— 两处各写一份"上手步骤"迟早对不上。
        assert!(
            help.contains("for (const step of GUIDE[R.lang] || GUIDE.zh)"),
            "引导条须复用 GUIDE"
        );
        // 关闭只记内存: 一旦真配了密钥, 服务端事实会让它自己消失, 不需要第二处状态。
        assert!(help.contains("let onboardDismissed = false;"), "关闭须只记内存");
        // 探针失败不引导(注释与实现都要在, 免得后来者顺手改成"读不到也显示")。
        assert!(
            help.contains("catch (_) {\n      bar.hidden = true;\n      return;\n    }"),
            "探针失败须闭嘴"
        );
        // 帮助层清空一律走 `removeChild`(`clear`), 不碰 innerHTML —— 与 palette.js 同一条纪律。
        assert!(
            !help.contains("innerHTML"),
            "help.js 清空须走 DOM API(clear/removeChild), 不得出现 innerHTML(与 palette.js 一致)"
        );
        for (name, js) in all_js() {
            assert!(!js.contains("localStorage"), "{name} 不得用 localStorage(引导状态也是状态)");
        }
    }

    /// D13: 前端有个**可选**的零依赖静态检查脚本 (`scripts/web_frontend_check.mjs`) ——
    /// 它不进 `cargo test`(免得给 CI 引 node 依赖), 所以这里只钉住**它赖以工作的两个钩子**:
    ///   ① 脚本本身还在, 且覆盖"双语字典"与"路由解析"两块;
    ///   ② `router.js` 仍导出 `R.parseHash` —— 少了这个导出, 脚本的 E 段会静默变成"没测",
    ///      而不是报红(它只会跳过那一段), 正是本仓库最忌讳的那类"看起来绿了"。
    #[test]
    fn test_frontend_check_script_still_hooked_up() {
        let script = include_str!("../../../../scripts/web_frontend_check.mjs");
        assert!(script.contains("R.TEXT"), "脚本须覆盖双语字典");
        assert!(script.contains("parseHash"), "脚本须覆盖路由解析");
        let router = src_of("router.js");
        assert!(
            router.contains("R.parseHash = function (hash)"),
            "router.js 须导出 R.parseHash(可传 hash)"
        );
        assert!(
            router.contains("if (hash === undefined) return parseHash();"),
            "R.parseHash 无参时须退回真实 location.hash, 别把生产路径测成另一条"
        );
    }

    /// D14: 主题下拉的选项、`/api/theme` 的白名单、配置文件解析的白名单 —— **必须是同一个集合**。
    ///
    /// 三处各写一份最典型的翻车是"下拉里能选、写盘却被拒"(或反过来, 新档选不到)。后两者现在
    /// 都派生自 `config_file::SUPPORTED_THEMES`, 这条把第三处(首页 `<option>`)也钉上。
    ///
    /// 同时钉住 `auto` 的**解法**: 写进 `<html data-theme>` 的必须是解析后的 `dark`/`light` ——
    /// 直接把 `"auto"` 写进去, CSS 里没有 `[data-theme="auto"]` 会退回 `:root` 深色,
    /// 用户选了"跟随系统"却永远深色, 而且看不出哪里错。
    #[test]
    fn test_theme_options_match_backend_whitelist() {
        let themes = crate::web::settings::SUPPORTED_THEMES;
        let rest = INDEX_HTML.split(r#"id="theme-select""#).nth(1).expect("首页须有主题下拉");
        let sel = &rest[..rest.find("</select>").expect("select 收尾")];
        for theme in themes {
            assert!(
                sel.contains(&format!(r#"<option value="{theme}""#)),
                "主题下拉缺选项 {theme}(后端白名单里有, 用户却选不到)"
            );
        }
        assert_eq!(
            sel.matches("<option").count(),
            themes.len(),
            "下拉选项数与白名单不一致(多了会 400, 少了选不到)"
        );
        // `auto` 是**偏好**: 前端要按系统偏好解出实际配色, 而不是把 "auto" 直接写进 data-theme。
        let common = src_of("common.js");
        assert!(
            common.contains("prefers-color-scheme: light"),
            "auto 须按 `prefers-color-scheme` 解出实际配色"
        );
        assert!(
            common.contains("document.documentElement.dataset.theme = applied;"),
            "写进 data-theme 的须是解析后的配色, 不是偏好本身"
        );
        assert!(
            common.contains(r#"window.dispatchEvent(new CustomEvent("ricow:theme""#),
            "换色须继续广播 ricow:theme(图表要按新变量重取配色)"
        );
    }

    /// 帮助计划第 2 条: 空会话给一组**可点就填**的示例(解决"不知道怎么开口")。
    ///
    /// 三条要点一起钉: ① 只在"一条消息都没有"时出现(有历史就别挡着); ② 点击**只填不发**
    /// —— 用户始终握着发送权, 与"写操作必须本人确认"同一姿态; ③ 有内容就撤(撤在 `append`
    /// 这个唯一入口上, 而不是散在 N 个地方)。
    #[test]
    fn test_chat_starter_card_is_click_to_fill_only() {
        let chat = src_of("chat.js");
        assert!(chat.contains("function renderStarter("), "须有起手卡渲染函数");
        assert!(
            chat.contains("if (!messages.length) renderStarter();"),
            "起手卡只在空会话(零消息)时出现"
        );
        assert!(
            chat.contains("clearStarter(); // 有内容进来"),
            "append 须在有内容时撤掉起手卡(唯一入口)"
        );
        // 点击只填进输入框并聚焦 —— 不许直接调发送。
        assert!(chat.contains("els.input.value = item.q;"), "点击须把示例填进输入框");
        assert!(!chat.contains("submitText(item.q)"), "点击不得替用户发送");
        // 示例随"有没有自己的策略"变; 目录比会话晚到, 回来后要补画一次。
        assert!(chat.contains("if (starterEl) renderStarter();"), "目录回来后须补画起手卡");
    }

    /// 041 D4: `--web-assets-dir` 的三条拒绝路径都要在**启动时**炸, 且文案指得出是哪个参数。
    /// 放行任何一条, 用户看到的就是"服务起来了但页面白屏", 得自己去猜目录哪里不对。
    #[test]
    fn test_resolve_assets_dir_rejects_bad_dirs() {
        // 051 复核 B13: 目录名里带**纳秒时间戳** —— 早先只用 `process::id()`, 而 Windows 会复用
        // pid, 于是上一轮残留的目录会让开头的 `remove_dir_all` 变成一次真实删除(冷启动 +
        // 实时防护扫描时实测可拖到 ~60s)。带上时间戳后永远是"新目录", 开头那句退化成快路径。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base =
            std::env::temp_dir().join(format!("ricow-assets-dir-{}-{stamp}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("建临时目录");

        // ① 目录不存在。
        let err = resolve_assets_dir(&base.join("nope")).expect_err("不存在的目录应被拒");
        let msg = err.to_string();
        assert!(msg.contains("--web-assets-dir"), "文案须点出参数名: {msg}");

        // ② 存在但是文件, 不是目录。
        let file = base.join("a.txt");
        std::fs::write(&file, "x").expect("写文件");
        let msg = resolve_assets_dir(&file).expect_err("文件应被拒").to_string();
        assert!(msg.contains("不是目录"), "文案须说明不是目录: {msg}");

        // ③ 是目录但不含 index.html(给错目录最常见的形态)。
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).expect("建子目录");
        let msg = resolve_assets_dir(&empty).expect_err("缺 index.html 应被拒").to_string();
        assert!(msg.contains("index.html"), "文案须说明缺什么: {msg}");

        // ④ 合法目录: 返回规范化后的绝对路径。
        let good = base.join("web");
        std::fs::create_dir_all(&good).expect("建目录");
        std::fs::write(good.join("index.html"), "<html></html>").expect("写首页");
        let dir = resolve_assets_dir(&good).expect("合法目录应通过");
        assert!(dir.is_absolute(), "应返回规范化后的绝对路径: {}", dir.display());
        assert!(dir.join("index.html").is_file(), "返回的路径要能直接读首页");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 041: 磁盘来源是"同一批名字换一个根目录", 不是"任意路径" —— 表外的名字永远读不到。
    #[test]
    fn test_asset_source_default_is_embedded() {
        assert!(
            matches!(AssetSource::default(), AssetSource::Embedded),
            "默认必须是内嵌: 不给开关时发布行为不得变化(FR-1)"
        );
    }

    /// 042: 策略视图拆成"外壳 + 三份子文件"后 **加载顺序成了正确性条件** —— 子文件在加载期
    /// 从 `R.sg` 取工具与常量, 外壳没先跑就是整片 `undefined`; 而那种失败只在用户点开策略页
    /// 时才现形(且是静默的), 离 `cargo test` 很远。`ASSETS` 的顺序 = 路由生成顺序 =
    /// `index.html` 的 `<script>` 顺序, 故在这里把这条约定钉死。
    #[test]
    fn test_strategy_shell_loads_before_its_parts() {
        let idx = |name: &str| {
            ASSETS
                .iter()
                .position(|a| a.name == name)
                .unwrap_or_else(|| panic!("资产表里缺 {name}"))
        };
        let shell = idx("strategies.js");
        let parts = ["strategy_form.js", "strategy_editor.js", "strategy_backtest.js"];
        for part in parts {
            assert!(idx(part) > shell, "{part} 必须排在 strategies.js 之后(加载顺序即契约)");
        }
        // 首页的 <script> 顺序也要与表里一致 —— 两处顺序不一致时, 表里的断言就白做了。
        let pos = |name: &str| {
            INDEX_HTML.find(&format!("/{name}?token=")).unwrap_or_else(|| panic!("首页缺 /{name}"))
        };
        for part in parts {
            assert!(pos(part) > pos("strategies.js"), "首页里 {part} 必须排在 strategies.js 之后");
        }
    }

    /// C 批次(Web 体验优化第二轮): 响应式 / 格式化统一 / 盘口深度条 / 结果对比。
    /// 这些都是纯前端改动, 后端测不到 —— 在这里钉住**接线形状**(骨架 id / 出口 / 事件),
    /// 防止后续重构把"看得见的功能"静默拆断。
    #[test]
    fn test_c_batch_frontend_wiring() {
        // C12: 窄屏抽屉 —— ☰ 按钮、遮罩、开合状态类, 以及断点样式块真的存在。
        assert!(INDEX_HTML.contains(r#"id="side-toggle""#), "顶栏须有侧栏开合钮");
        assert!(INDEX_HTML.contains(r#"id="side-scrim""#), "侧栏抽屉须有遮罩层");
        let app = src_of("app.js");
        assert!(app.contains("side-open"), "app.js 须切 #app 的 side-open 状态类");
        let css = src_of("style.css");
        assert!(css.contains("@media (max-width: 900px)"), "C12 断点(≤900px)样式块缺失");
        assert!(css.contains("prefers-reduced-motion"), "抽屉过渡须尊重减动效偏好");

        // C11: 统一格式化出口 —— R.fmt 必须在 common.js 定义, 各视图委托而不是自留副本。
        assert!(src_of("common.js").contains("R.fmt = "), "common.js 须定义 R.fmt 统一出口");
        assert!(
            src_of("strategy_backtest.js").contains("R.fmt.pct(")
                && src_of("strategy_backtest.js").contains("R.fmt.num("),
            "回测视图的数值/百分比须走 R.fmt"
        );
        assert!(src_of("chat.js").contains("R.fmt.time("), "对话时间戳须走 R.fmt.time(毫秒口径)");

        // C10: 盘口深度条 —— JS 只算 `--depth-w`(累计量占比), 颜色与渐变交给样式表。
        let mk = src_of("markets.js");
        assert!(mk.contains("--depth-w"), "盘口行须按累计量设置 --depth-w");
        assert!(css.contains("linear-gradient"), "深度条渐变背景须在样式表里(买卖各用主题变量)");

        // C9: 会话内结果对比 —— 有对比开关、差值着色沿用涨红跌绿。
        let bt = src_of("strategy_backtest.js");
        assert!(bt.contains("sg-cmp"), "回测结果区须有对比开关(sg-cmp)");
    }

    /// C8: 多维参数寻优 —— 前端骨架 / 发起端点 / 进度通道与后端进度字段一一对应。
    #[test]
    fn test_c8_optimize_frontend_wiring() {
        let shell = src_of("strategies.js");
        let form = src_of("strategy_form.js");
        let bt = src_of("strategy_backtest.js");
        // 骨架: 维度行容器 + 添加钮 + 发起钮 + 独立消息/结果区(不与单参数 sweep 混用)。
        for id in ["sg-opt-dims", "sg-opt-add", "sg-opt-go", "sg-opt-msg", "sg-opt-result"] {
            assert!(shell.contains(&format!(r#"id="{id}""#)), "多维寻优骨架缺 #{id}");
        }
        // 维度行由表单子文件管理(增删 / 读回), 外壳经延迟别名调用。
        assert!(form.contains("function addOptDim("), "strategy_form.js 须有维度行构造");
        assert!(form.contains("S.readOptDims = readOptDims;"), "维度读回须导出给回测子文件");
        assert!(bt.contains("S.readOptDims()"), "发起寻优须经 readOptDims 收集维度");
        // 发起端点与进度: 只认 /api/backtest/optimize, 进度来自后端 progress 字段。
        assert!(
            bt.contains(r#""/api/backtest/optimize""#),
            "多维寻优必须打到专属端点(不是拿 sweep 拼多次)"
        );
        assert!(bt.contains("job.progress"), "轮询须把后端 progress 透传给渲染");
        assert!(bt.contains("sg-opt-prog-fill"), "运行态须渲染进度条");
        // 换策略必须把扫描/寻优的**结果与折叠态一并复位** —— 详情骨架只在 mount 时建一次,
        // 不清就是上一策略的结果留在原地(A 的结果挂到 B 名下; 真机走查复现过)。
        assert!(
            form.contains(r#"$("sg-opt-box").open = false;"#)
                && form.contains(r#"$("sg-sweep-box").open = false;"#),
            "enterDetail 须复位扫优折叠框(否则上一策略结果原地留存)"
        );
    }

    /// B7: 命令面板(Ctrl/⌘+K) —— 骨架 / **数据源单一真相** / 按键纪律。
    ///
    /// 单一真相是本条最容易做坏的地方: 手抄一份"有哪些视图""主题都叫什么"最省事, 但左侧导航
    /// 或主题下拉一改, 面板就成了假话。故钉住它现取(nav 的 data-zh/en、`#theme-select` 的
    /// option、`R.glossary`、`R.help`), 而不是在 palette.js 里再写一份。
    #[test]
    fn test_command_palette_wiring_and_key_discipline() {
        for id in ["palette", "palette-input", "palette-list", "palette-btn", "palette-detail"] {
            assert!(INDEX_HTML.contains(&format!(r#"id="{id}""#)), "首页缺命令面板骨架 #{id}");
        }
        let p = src_of("palette.js");
        // ① 单一真相: 五类条目全部来自既有来源, 不新写文案。
        assert!(p.contains(r##"querySelectorAll("#nav a")"##), "视图名须取自左侧 nav 的双语标签");
        assert!(p.contains("#theme-select"), "主题名须取自顶栏主题下拉的 option");
        assert!(p.contains("R.glossary"), "术语须走公共件的全局单例(前端只此一处取 /api/terms)");
        assert!(p.contains("R.help"), "帮助段须复用抽屉, 不另写一份帮助内容");
        assert!(p.contains(r#"R.api("/api/strategies")"#), "策略须取既有列表接口");
        // "当前语言"的标记也要本地化 —— 写死中文词会让英文界面冒出 "Switch UI language · 中文"。
        assert!(
            p.contains("cmdCurrent") && p.contains(r#"t("cmdCurrent")"#),
            "语言条目的'当前'标记须走文案字典(cmdCurrent), 不得写死某个语言名"
        );
        // ② 按键纪律: 全局只认 Ctrl/⌘+K; 方向键 / Enter / Esc 在**面板输入框自己的** keydown 里,
        //    面板关着时不碰这些键 —— 否则会抢走对话输入框与回测表单里的回车。
        assert!(
            p.contains("ctrlKey") && p.contains("metaKey") && p.contains(r#"ev.key === "k""#),
            "打开键必须是 Ctrl/⌘+K"
        );
        assert!(
            p.contains(r#"input.addEventListener("keydown""#),
            "导航键须绑在面板输入框上(不是全局)"
        );
        // ③ 红线: 不落浏览器存储; 条目文本全走 textContent, 不拼 HTML。
        assert!(!p.contains("localStorage") && !p.contains("sessionStorage"), "禁浏览器存储");
        assert!(!p.contains("innerHTML") && !p.contains("insertAdjacentHTML"), "禁拼 HTML");
    }
}
