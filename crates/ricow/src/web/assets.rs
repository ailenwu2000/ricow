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
    // 对话视图(032 T006): 会话 / SSE / 三个只读面板逻辑。
    Asset { name: "chat.js", ctype: CTYPE_JS, embedded: include_str!("assets/chat.js") },
    // 设置视图(032 US1): 市场视野开关; 密钥配置已迁至 033 密钥视图。
    Asset { name: "settings.js", ctype: CTYPE_JS, embedded: include_str!("assets/settings.js") },
    // 密钥视图(033): 多套 AI / 币安密钥的别名条目。
    Asset { name: "keys.js", ctype: CTYPE_JS, embedded: include_str!("assets/keys.js") },
    // 市场视图(032 US2, 040 起含 SSE 实时): 交易对列表 / 订单簿 / K 线。
    Asset { name: "markets.js", ctype: CTYPE_JS, embedded: include_str!("assets/markets.js") },
    // 策略管理视图(032 US3): 策略列表 / 源码编辑保存 / AI 改 Lua / 回测作业。
    Asset {
        name: "strategies.js",
        ctype: CTYPE_JS,
        embedded: include_str!("assets/strategies.js"),
    },
    // 运行管理视图(032 US4): 实例一览 / 启停 / 风险确认 / 行内日志。
    Asset { name: "runs.js", ctype: CTYPE_JS, embedded: include_str!("assets/runs.js") },
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
        assert!(!panels.contains("<button"), "面板是只读的, 不得有按钮(D17)");
        assert!(!panels.contains("onclick"), "面板是只读的, 不得有内联事件(D17)");

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

    /// 041 D4: `--web-assets-dir` 的三条拒绝路径都要在**启动时**炸, 且文案指得出是哪个参数。
    /// 放行任何一条, 用户看到的就是"服务起来了但页面白屏", 得自己去猜目录哪里不对。
    #[test]
    fn test_resolve_assets_dir_rejects_bad_dirs() {
        let base = std::env::temp_dir().join(format!("ricow-assets-dir-{}", std::process::id()));
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
}
