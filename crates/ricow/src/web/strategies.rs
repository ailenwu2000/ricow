//! 策略目录端点(031 FR-013 / FR-014; 032 T027 扩展): 只读展示策略清单与参数 schema。
//!
//! 目录的**唯一来源**是 [`crate::strategies::catalog`](内置编译期嵌入 + 用户自写),
//! 本模块只做投影, 不缓存、不另存一份; 用户实例的当前值走 [`super::strategy_io`]。

use std::path::Path;

use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use axum::{Json, Router};

use super::strategy_io;
use super::WebError;
use super::WebState;

/// 策略列表行(前端策略面板用)。
#[derive(serde::Serialize)]
pub(super) struct StrategyRow {
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
    /// 054 D7: 是否 AI 生成(读 `strategies/<id>.toml` 顶部注释判定, **服务端事实**)。
    ///
    /// 只说来源, 不说验证 —— 验证状态由回测结果自己展示(D8), 徽章不越界下结论。
    origin_ai: bool,
    /// 063: 最后修改时间(Unix 秒)。**读不到就是 null** —— 不编造。
    ///
    /// 取 `strategies/<id>.toml`(实例配置)与 `strategies/<market>/<id>.lua`(脚本)
    /// 两个文件 mtime 的**较新**者: 改代码动 .lua, 改参数动 .toml, 只看一个会漏。
    /// 内置策略两个文件都在编译期嵌入(磁盘上没有)→ 恒为 null。
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified: Option<i64>,
    /// 063: 创建时间(Unix 秒)。同样**读不到就是 null**。
    ///
    /// 取实例 TOML 的 mtime —— 用户策略落盘即"创建"(脚本与配置同时写入)。
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<i64>,
}

/// 策略列表(只读): 内置示例 + 用户自写, 按注册顺序。
impl StrategyRow {
    /// 是不是**用户自写**的策略(内置示例不算)。
    ///
    /// 062: 概览页要"用户自写的策略数" —— 但 `source` 字段是私有的, 故给一个只读方法,
    /// 而不是把字段放成 `pub(super)`(那样别的模块就能直接依赖它的取值约定)。
    pub(super) fn is_user(&self) -> bool {
        self.source == "user"
    }
}

/// 063: 读某个文件的 mtime(Unix 秒); **读不到就是 None** —— 不编造时间。
///
/// 刻意**不**在 IO 失败时给 0 或当前时间: 列表页的"最后修改"要么是真事实,
/// 要么空着; 填一个假的时间戳比空着更坏(用户会以为知道实际何时改的)。
fn file_mtime_secs(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(dur.as_secs()).ok()
}

/// 063: 某个用户策略的最后修改 / 创建时间。
///
/// 内置策略两个文件都不在磁盘上(编译期嵌入)→ `(None, None)`。
/// 实例 TOML 缺失(裸 `.lua` 的老文件)→ 创建时间 None; 脚本仍在 → 最后修改有值。
fn strategy_times(root: &Path, id: &str, market: &str) -> (Option<i64>, Option<i64>) {
    let strategies_dir = root.join("strategies");
    let toml_path = strategies_dir.join(format!("{id}.toml"));
    let lua_path = strategies_dir.join(market).join(format!("{id}.lua"));
    let toml_mtime = file_mtime_secs(&toml_path);
    let lua_mtime = file_mtime_secs(&lua_path);
    // 最后修改 = 两个文件里较新的那个(改代码动 .lua, 改参数动 .toml, 只看一个会漏)。
    let last_modified = match (toml_mtime, lua_mtime) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    (last_modified, toml_mtime)
}

pub(super) async fn list_strategies(State(state): State<WebState>) -> Json<Vec<StrategyRow>> {
    let rows = crate::strategies::catalog::all()
        .into_iter()
        .map(|e| {
            let is_user = e.source == crate::strategies::catalog::Source::User;
            let (last_modified, created_at) = if is_user {
                strategy_times(&state.root, &e.manifest.id, &e.manifest.market)
            } else {
                // 内置策略: 磁盘上没有文件 → 没有"最后修改/创建"可言, 诚实给 null。
                (None, None)
            };
            StrategyRow {
                // 内置策略没有用户实例 TOML, 恒为 false(不读盘)。
                origin_ai: is_user && strategy_io::strategy_origin_ai(&state.root, &e.manifest.id),
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
                last_modified,
                created_at,
            }
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
    /// 054 D7: 是否 AI 生成(详见 [`StrategyRow::origin_ai`])。
    origin_ai: bool,
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
    let origin_ai = if entry.source == crate::strategies::catalog::Source::Builtin {
        false
    } else {
        strategy_io::strategy_origin_ai(&state.root, &id)
    };
    Ok(Json(StrategyDetailReply {
        declared: entry.declared,
        duplicate_of: entry.duplicate_of,
        origin_ai,
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
