//! 策略目录端点(031 FR-013 / FR-014; 032 T027 扩展): 只读展示策略清单与参数 schema。
//!
//! 目录的**唯一来源**是 [`crate::strategies::catalog`](内置编译期嵌入 + 用户自写),
//! 本模块只做投影, 不缓存、不另存一份; 用户实例的当前值走 [`super::strategy_io`]。

use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use axum::{Json, Router};

use super::strategy_io;
use super::WebError;
use super::WebState;

/// 策略列表行(前端策略面板用)。
#[derive(serde::Serialize)]
struct StrategyRow {
    id: String,
    name: String,
    market: String,
    summary: String,
    source: String,
    param_count: usize,
    /// 参数清单是否已由磁盘清单 TOML 声明(裸 `.lua` = false → 表单只有交易对)。
    declared: bool,
    /// 与某内置策略脚本逐字相同时为其 id(冗余副本, 未声明者会被运行门禁拒)。
    #[serde(skip_serializing_if = "Option::is_none")]
    duplicate_of: Option<String>,
}

/// 策略列表(只读): 内置示例 + 用户自写, 按注册顺序。
async fn list_strategies() -> Json<Vec<StrategyRow>> {
    let rows = crate::strategies::catalog::all()
        .into_iter()
        .map(|e| StrategyRow {
            id: e.manifest.id,
            name: e.manifest.name,
            market: e.manifest.market,
            summary: e.manifest.summary,
            source: match e.source {
                crate::strategies::catalog::Source::Builtin => "builtin".to_string(),
                crate::strategies::catalog::Source::User => "user".to_string(),
            },
            param_count: e.manifest.params.len(),
            declared: e.declared,
            duplicate_of: e.duplicate_of,
        })
        .collect();
    Json(rows)
}

/// 策略详情响应(032 T027 扩展): 清单字段整体扁平化(对话视图仍直接读 name/params 等),
/// 另带用户实例的当前交易对 `pair` 与当前参数值 `current`(供控制台表单回填;
/// 内置策略 / 无实例 TOML 时两者为 null, 前端回退清单默认值)。
#[derive(serde::Serialize)]
struct StrategyDetailReply {
    #[serde(flatten)]
    manifest: crate::strategies::catalog::StrategyManifest,
    /// 已部署实例的当前交易对(无实例 → null)。
    pair: Option<String>,
    /// 已部署实例的当前参数值(已剔除 pair/script/script_path; 无实例 → null)。
    current: Option<std::collections::HashMap<String, ricow_strategy::ConfigValue>>,
    /// 参数清单是否已声明(详见 [`StrategyRow::declared`])。
    declared: bool,
    /// 脚本与某内置策略逐字相同时为其 id(详见 [`StrategyRow::duplicate_of`])。
    #[serde(skip_serializing_if = "Option::is_none")]
    duplicate_of: Option<String>,
}

/// 单个策略详情(只读): 完整清单(含参数 schema) + 用户实例当前值, 供前端参数表单渲染。
async fn get_strategy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<StrategyDetailReply>, WebError> {
    // 未知 id → 404(与 `/source`、`/manifest`、回测端点同口径; 此前误用 400, 2026-10-05 统一)。
    let entry = crate::strategies::catalog::find(&id)
        .ok_or_else(|| WebError::not_found(format!("没有策略 {id}")))?;
    // 内置策略只有编译期嵌入清单, 无用户实例; 用户策略读 strategies/<id>.toml 解析当前值。
    let values = if entry.source == crate::strategies::catalog::Source::Builtin {
        None
    } else {
        strategy_io::load_instance_values(&state.root, &id)?
    };
    Ok(Json(StrategyDetailReply {
        declared: entry.declared,
        duplicate_of: entry.duplicate_of,
        manifest: entry.manifest,
        pair: values.as_ref().and_then(|v| v.pair.clone()),
        current: values.map(|v| v.current),
    }))
}

/// 本模块负责的策略目录路由(挂进 [`super::router`])。
///
/// 写入 / 源码 / AI 改 Lua / 回测归 [`super::strategy_io`] 与 [`super::backtest_jobs`],
/// 此处只列清单与详情。
pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/api/strategies", get(list_strategies))
        .route("/api/strategies/{id}", get(get_strategy))
}
