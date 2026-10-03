//! 界面设置端点: 语言(023 FR-029 ~ FR-031)与主题(034)。
//!
//! 两者都与 CLI **共用同一份 `ricow.toml`**(`[ui].lang` / `[ui].theme`), 不新增第二处状态;
//! 差别只在生效方式 —— 语言要顺手给在跑的会话送一条控制行, 主题纯外观、不必惊动会话线程。

use std::path::Path;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use super::WebError;
use super::WebState;
use crate::commands::config_file::{self, SetValue};
use crate::i18n::{t, Lang};

/// 语言读写(FR-029)。
#[derive(serde::Serialize)]
struct LangReply {
    lang: &'static str,
    languages: [&'static str; 2],
}

#[derive(serde::Deserialize)]
struct LangBody {
    lang: String,
}

async fn get_lang(State(state): State<WebState>) -> Result<Json<LangReply>, WebError> {
    Ok(Json(LangReply { lang: current_lang(&state.root)?.code(), languages: SUPPORTED_LANGS }))
}

/// 写入新语言: 非法值**硬失败且不写盘**(023 既有规则, FR-031)。
async fn set_lang(
    State(state): State<WebState>,
    Json(body): Json<LangBody>,
) -> Result<Json<LangReply>, WebError> {
    let Some(lang) = Lang::parse(&body.lang) else {
        // 回执按**当前**界面语言(此刻 `body.lang` 非法, 不能拿它取语言)。
        let lang = current_lang(&state.root).unwrap_or(Lang::Zh);
        return Err(WebError::bad_request(format!(
            "{}: {}",
            t(lang, "不支持的语言", "unsupported language"),
            body.lang
        )));
    };
    // 先确保配置存在(首次运行会生成模板), 再按白名单写回并保留注释(与 `handle_lang` 同路径)。
    config_file::load(&state.root)?;
    config_file::set_values(
        &state.root,
        &[("ui", "lang", SetValue::Str(lang.code().to_string()))],
    )?;
    // 立即生效(FR-030): 写盘只改了配置, **已在跑的会话**手里还攥着旧语言与旧提示词 ——
    // 给它送一条控制行 `/lang`, 走的就是它自己那条"改配置 → 换语言 → 重建 LLM"
    // (`ChatSession::handle_lang` → `rebuild_llm`), 不重启服务、不换会话线程、不丢上下文。
    // (`false` = 当前没有会话在跑: 那就不用管, 下次开会话读配置自然是新语言。)
    state.hub.submit_control(format!("/lang {}", lang.code()));
    Ok(Json(LangReply { lang: lang.code(), languages: SUPPORTED_LANGS }))
}

/// 可选语言(单一来源: 前端语言切换控件按它渲染)。
const SUPPORTED_LANGS: [&str; 2] = ["zh", "en"];

/// 当前界面语言 = 配置里的 `[ui].lang`(缺省中文)。每次现读, 与 CLI 看到的永远一致。
///
/// 跨模块复用(日志流的宿主提示语也跟着界面语言走), 故为 `pub(super)`。
pub(super) fn current_lang(root: &Path) -> ricow_core::CoreResult<Lang> {
    Ok(crate::i18n::resolve(&config_file::load(root)?))
}

/// 可选主题(单一来源: 前端主题选择控件按它渲染; 白名单与 `config_file` 的 `[ui].theme` 一致)。
const SUPPORTED_THEMES: [&str; 3] = ["dark", "light", "red"];

#[derive(serde::Serialize)]
struct ThemeReply {
    theme: String,
}

#[derive(serde::Deserialize)]
struct ThemeBody {
    theme: String,
}

/// 当前主题 = 配置里的 `[ui].theme`(缺省深色)。每次现读, 与配置文件永远一致。
async fn get_theme(State(state): State<WebState>) -> Result<Json<ThemeReply>, WebError> {
    let file = config_file::load(&state.root)?;
    let theme = file.ui.theme.unwrap_or_else(|| "dark".to_string());
    Ok(Json(ThemeReply { theme }))
}

async fn set_theme(
    State(state): State<WebState>,
    Json(body): Json<ThemeBody>,
) -> Result<Json<ThemeReply>, WebError> {
    let theme = body.theme.trim().to_string();
    if !SUPPORTED_THEMES.contains(&theme.as_str()) {
        return Err(WebError::bad_request(format!(
            "不支持的主题: {} (可选: {})",
            body.theme,
            SUPPORTED_THEMES.join(" / ")
        )));
    }
    // 先确保配置存在(首次运行会生成模板), 再按白名单写回并保留注释(与 `set_lang` 同路径);
    // 非法值在 `load` 里本就会硬失败, 这里再挡一道是为了给出统一的 400 报错。
    config_file::load(&state.root)?;
    config_file::set_values(&state.root, &[("ui", "theme", SetValue::Str(theme.clone()))])?;
    Ok(Json(ThemeReply { theme }))
}

/// 本模块负责的设置路由(挂进 [`super::router`])。
pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/api/lang", get(get_lang).post(set_lang))
        .route("/api/theme", get(get_theme).post(set_theme))
}

#[cfg(test)]
mod tests {
    use crate::commands::config_file;
    use crate::web::test_support::{body_of, serve_test, tmp_root};

    /// 034: `/api/theme` 读写 —— 缺省 dark; 合法值写盘并回读; 非法值 400 且不落盘。
    #[tokio::test]
    async fn test_theme_endpoint_roundtrip_and_reject() {
        async fn raw(port: u16, req: String) -> String {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let mut stream = tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port))
                .await
                .expect("连上服务");
            stream.write_all(req.as_bytes()).await.expect("发出请求");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("读回响应");
            String::from_utf8_lossy(&buf).to_string()
        }
        async fn get(port: u16, path: &str) -> String {
            raw(
                port,
                format!(
                    "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    crate::web::BIND_ADDR
                ),
            )
            .await
        }
        async fn post(port: u16, path: &str, body: &str) -> String {
            raw(
                port,
                format!(
                    "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    crate::web::BIND_ADDR,
                    body.len()
                ),
            )
            .await
        }

        let root = tmp_root("theme");
        let db = ricow_strategy::Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root.clone(), db).await;

        // 缺省 = 深色(与前端兜底一致)。
        let res = get(port, "/api/theme?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "GET /api/theme 应 200, 实际: {res}");
        assert!(body_of(&res).contains(r#""theme":"dark""#), "缺省主题应为 dark: {res}");

        // 合法值: 200, 回读一致; ricow.toml 里真有 theme = "light"。
        let res = post(port, "/api/theme?token=tok-ok", r#"{"theme":"light"}"#).await;
        assert!(res.starts_with("HTTP/1.1 200"), "POST 合法主题应 200, 实际: {res}");
        let res = get(port, "/api/theme?token=tok-ok").await;
        assert!(body_of(&res).contains(r#""theme":"light""#), "写盘后应回读 light: {res}");
        let toml_text = std::fs::read_to_string(config_file::path(&root)).expect("读 ricow.toml");
        assert!(toml_text.contains("theme = \"light\""), "配置应落盘: {toml_text}");

        // 非法值: 400, 且不得覆盖已有主题。
        let res = post(port, "/api/theme?token=tok-ok", r#"{"theme":"blue"}"#).await;
        assert!(res.starts_with("HTTP/1.1 400"), "非法主题应 400, 实际: {res}");
        let res = get(port, "/api/theme?token=tok-ok").await;
        assert!(body_of(&res).contains(r#""theme":"light""#), "非法值不得覆盖已有主题: {res}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
