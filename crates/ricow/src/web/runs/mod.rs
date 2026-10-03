//! 032 US4 运行控制端点 (FR-022 ~ FR-026):
//!
//! - `GET /api/runs` — 实例运行态一览(契约七字段 + 最近 PnL, 缺失为 null **不伪造**);
//! - `GET /api/strategies/{id}/status` — 单实例运行态(data-model §8: source 三态 daemon|ledger|none);
//! - `POST /api/strategies/{id}/start` — dry_run / demo / live 三模式拉起, 全部门禁在触碰 daemon 之前;
//! - `POST /api/strategies/{id}/stop` — 优雅停机(`close_all` 如实透传 daemon);
//! - `POST /api/risk-ack` — 首次风险披露确认落盘(逐字短语「确认风险」, 常量时间比对)。
//!
//! 启停/预检复用 CLI 同一内核([`crate::commands::ctrl`]), daemon 自举复用
//! [`crate::commands::daemon::ensure_daemon`](幂等、无终端输出, FR-034) —— 本层不复制任何控制逻辑。
//! 确认短语错误文案回显期望串, 与 CLI `require_explicit_phrase` 同口径。

use axum::extract::{rejection::JsonRejection, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ricow_core::CoreError;
use ricow_strategy::PnlSnapshotRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::supervisor::proto::InstanceView;

use super::{WebError, WebState};

// ---- 请求/响应体 (data-model §8, http-api §5) ----

/// `POST .../start` 请求体: 契约里的 `pair`/`params` 字段本轮忽略(serde 默认丢弃未知字段)。
#[derive(Debug, Deserialize)]
struct StartRequest {
    /// dry_run | demo | live(白名单, 其余 400)。
    mode: String,
    /// live 模式必填的逐字确认短语 `确认实盘 <名>`; 缺失/不符一律 400 且零副作用。
    #[serde(default)]
    phrase: Option<String>,
}

/// `POST .../stop` 请求体: 缺省 false(只停策略进程, 不平仓)。
#[derive(Debug, Deserialize)]
struct StopRequest {
    #[serde(default)]
    close_all: bool,
}

/// `POST /api/risk-ack` 请求体: 逐字短语「确认风险」。
#[derive(Debug, Deserialize)]
struct RiskAckRequest {
    phrase: String,
}

/// PnL 摘要(最近一条快照; Decimal 走字符串, 与 `trades_pnl` 的 PnlItem 同口径)。
#[derive(Debug, Serialize)]
struct PnlSummary {
    realized: String,
    fees: String,
    net: String,
    trade_count: i64,
    asof: String,
}

impl From<&PnlSnapshotRecord> for PnlSummary {
    fn from(s: &PnlSnapshotRecord) -> Self {
        Self {
            realized: s.realized_pnl.to_string(),
            fees: s.fees.to_string(),
            net: s.net_pnl.to_string(),
            trade_count: s.trade_count,
            asof: fmt_asof(s.timestamp),
        }
    }
}

/// `GET /api/runs` 单项: 契约七字段 + `pnl`(缺失为 null)。
#[derive(Debug, Serialize)]
pub(super) struct RunItem {
    name: String,
    running: bool,
    mode: String,
    pid: Option<u32>,
    uptime_secs: Option<u64>,
    pair: Option<String>,
    source: &'static str,
    pnl: Option<PnlSummary>,
}

/// `GET .../status` 响应(data-model §8; `pnl` 缺失序列化为 null, 不 skip)。
#[derive(Debug, Serialize)]
pub(super) struct StatusReply {
    name: String,
    running: bool,
    mode: String,
    pid: Option<u32>,
    uptime_secs: Option<u64>,
    pair: Option<String>,
    source: &'static str,
    pnl: Option<PnlSummary>,
}

/// `POST .../start` 成功响应(契约 200 `{pid, mode}`)。
#[derive(Debug, Serialize)]
struct StartReply {
    pid: u64,
    mode: String,
}

/// `POST .../stop` 成功响应: 用户指令的 `{stopped:true}` 与契约停机说明文本合并给出。
#[derive(Debug, Serialize)]
pub(super) struct StopReply {
    stopped: bool,
    report: String,
}

/// `POST /api/risk-ack` 成功响应。
#[derive(Debug, Serialize)]
pub(super) struct RiskAckReply {
    acked: bool,
}

// ---- 纯函数(便于单测; 不碰盘不触网) ----

/// 风险确认短语(018 / AI 确认同源): 逐字。
const RISK_ACK_PHRASE: &str = "确认风险";

/// 确认短语比对(纯函数): 常量时间比较的薄包装; 缺失(None)一律不匹配。
fn phrase_matches(input: Option<&str>, expected: &str) -> bool {
    input.is_some_and(|p| super::auth::ct_eq(p, expected))
}

/// 运行模式缺失/未知 → "unknown"(如实, 不猜; data-model §8)。
fn mode_or_unknown(mode: Option<&str>) -> String {
    mode.map(str::to_string).unwrap_or_else(|| "unknown".to_string())
}

/// 实例三态(data-model §8 / 026 口径): **daemon 在线 = daemon**(它就是权威, 答什么都算);
/// daemon 不在线但台账里有它 = ledger; 连台账都没有(仅 TOML) = none。
///
/// 注意 snapshot 的构成: daemon 在线时 instances 全量来自 daemon, 离线时全量来自台账 ——
/// `has_view` 只在离线时才承载"台账有记录"的含义, 在线时 daemon 说了算。
fn source_of(daemon_online: bool, has_view: bool) -> &'static str {
    if daemon_online {
        "daemon"
    } else if has_view {
        "ledger"
    } else {
        "none"
    }
}

/// 毫秒时间戳 → RFC3339; 无法表示时给空串(前端显示为空, 不伪造时刻)。
fn fmt_asof(ts_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts_ms).map(|t| t.to_rfc3339()).unwrap_or_default()
}

/// 台账视图 + 最近 PnL → 运行项(`GET /api/runs` 单项)。
fn build_run_item(daemon_online: bool, v: &InstanceView, pnl: Option<PnlSummary>) -> RunItem {
    RunItem {
        name: v.name.clone(),
        running: v.running,
        mode: mode_or_unknown(v.mode.as_deref()),
        pid: v.pid,
        uptime_secs: v.uptime_secs,
        pair: v.pair.clone(),
        source: source_of(daemon_online, true),
        pnl,
    }
}

/// 组装 status 响应(纯函数): 有视图取视图字段; 无视图如实给 unknown(不伪造 running),
/// 视图缺 pair 时回退实例 TOML(与 `format_info` 同口径: pair 存 params)。
fn build_status(
    name: &str,
    daemon_online: bool,
    view: Option<&InstanceView>,
    pair_fallback: Option<String>,
    pnl: Option<PnlSummary>,
) -> StatusReply {
    match view {
        Some(v) => StatusReply {
            name: v.name.clone(),
            running: v.running,
            mode: mode_or_unknown(v.mode.as_deref()),
            pid: v.pid,
            uptime_secs: v.uptime_secs,
            pair: v.pair.clone().or(pair_fallback),
            source: source_of(daemon_online, true),
            pnl,
        },
        None => StatusReply {
            name: name.to_string(),
            running: false,
            mode: "unknown".to_string(),
            pid: None,
            uptime_secs: None,
            pair: pair_fallback,
            source: source_of(daemon_online, false),
            pnl,
        },
    }
}

// ---- handler ----

/// 读某实例最近一条 PnL 快照(无记录 → None, 不伪造)。
///
/// sqlx 错误按 500 如实带出(本 crate 不依赖 sqlx, 只展示 Display 文本, D10 诚实)。
async fn latest_pnl(state: &WebState, name: &str) -> Result<Option<PnlSummary>, WebError> {
    let rows = state.db.recent_pnl_snapshots(Some(name), 1).await.map_err(|e| {
        WebError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("读取 PnL 快照失败: {e}"),
            None,
            None,
        )
    })?;
    Ok(rows.first().map(PnlSummary::from))
}

/// 403 需先完成风险披露确认(http-api §5): 不动共享 WebErrorBody, 专用响应体带披露原文。
struct NeedRiskAck;

impl IntoResponse for NeedRiskAck {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": "实盘启动需先完成风险披露确认: 在终端执行 ricow risk-ack, 或在对话内输入「确认风险」。",
            "code": "need_risk_ack",
            "disclosure": ricow_engine::RISK_DISCLOSURE,
        });
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    }
}

/// start 链路的拒绝语义: daemon 明确拒绝(InvalidArgument)→ 400 原文; 其余(连接失败等)→ 500。
///
/// 注意只做**局部**映射: `ensure_daemon` 的拉起失败(含 5s 超时)保持 `?` → 500,
/// 其错误文案自带 daemon.log 指引, 不该伪装成客户端的错。
fn start_reject(e: CoreError) -> WebError {
    match e {
        CoreError::InvalidArgument(msg) => WebError::bad_request(msg),
        other => other.into(),
    }
}

/// `GET /api/runs` (FR-022): 实例运行态一览 + 各自最近 PnL。
pub(super) async fn list_runs(
    State(state): State<WebState>,
) -> Result<Json<Vec<RunItem>>, WebError> {
    let snap = crate::commands::instances::snapshot(&state.root).await;
    let mut items = Vec::with_capacity(snap.instances.len());
    for v in &snap.instances {
        let pnl = latest_pnl(&state, &v.name).await?;
        items.push(build_run_item(snap.daemon_online, v, pnl));
    }
    Ok(Json(items))
}

/// `GET /api/strategies/{id}/status` (FR-023): 单实例运行态; 名字判定与 CLI 同一来源(027)。
pub(super) async fn get_status(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<StatusReply>, WebError> {
    if !crate::commands::instances::strategy_name_exists(&state.root, &id) {
        return Err(WebError::not_found(crate::commands::instances::unknown_name_message(&id)));
    }
    let snap = crate::commands::instances::snapshot(&state.root).await;
    let view = snap.instances.iter().find(|v| v.name == id);
    // pair 回退 TOML(既有口径: pair 存 params, get_str 读取)。
    let pair_fallback = crate::commands::read_strategy_config_in(&state.root, &id)
        .and_then(|cfg| cfg.get_str("pair").map(str::to_string));
    let pnl = latest_pnl(&state, &id).await?;
    Ok(Json(build_status(&id, snap.daemon_online, view, pair_fallback, pnl)))
}

/// `POST /api/strategies/{id}/start` (FR-024): dry_run / demo / live 三模式拉起。
///
/// 门禁顺序(全部在触碰 daemon 之前, 任一不过即零副作用):
/// ①名合法 → ②mode 白名单 → ③(live)逐字短语 → ④(live)首次风险确认 → ⑤(live)TOML
/// live_enabled → ⑥(demo)凭据 → ⑦daemon 自举(幂等) → ⑧已运行 409 → ⑨(live)共享预检
/// → ⑩拉起(confirmed=live, 与 CLI `ricow start --live` 完全同参)。
pub(super) async fn start_strategy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, WebError> {
    let Json(raw) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: StartRequest = serde_json::from_value(raw)
        .map_err(|e| WebError::bad_request(format!("请求体字段不符合要求: {e}")))?;

    // ① 名字合法(防路径穿越; 与保存同口径)。
    if ricow_strategy::validate_strategy_name(&id).is_err() {
        return Err(WebError::bad_request_code(format!("策略名非法: {id}"), "invalid_name"));
    }

    // ② mode 白名单 → (live, demo) 组合。
    let (live, demo) = match req.mode.as_str() {
        "dry_run" => (false, false),
        "demo" => (false, true),
        "live" => (true, false),
        _ => {
            return Err(WebError::bad_request_code(
                format!("mode 必须是 dry_run / demo / live, 收到: {}", req.mode),
                "invalid_mode",
            ));
        }
    };

    // ③ live: 每次启动的逐字确认短语(与 CLI 同口径, 错误文案同样回显期望串)。
    let expected = format!("确认实盘 {id}");
    if live && !phrase_matches(req.phrase.as_deref(), &expected) {
        return Err(WebError::bad_request(format!(
            "未确认: 输入与确认短语不一致(期望逐字: {expected}); 未执行任何动作。"
        )));
    }

    // ④ live: 首次风险披露确认(risk_ack.json 固定写在 project_root(), 与 daemon 子进程同一份)。
    if live && !crate::commands::risk_acked() {
        return Ok(NeedRiskAck.into_response());
    }

    // ⑤ live: TOML 必须显式 live_enabled=true —— daemon 会把假值静默降级 dry_run,
    // Web 层前置拒绝, 不给用户"点了实盘实际是 dry_run"的假成功。
    if live {
        let config = crate::commands::read_strategy_config_in(&state.root, &id);
        let allowed = config.as_ref().is_some_and(|c| c.live_enabled);
        if !allowed {
            return Err(WebError::bad_request_code(
                "未声明实盘 (TOML live_enabled=false 或文件不存在/不可解析); \
                 请先在实例 TOML 里设 live_enabled = true。"
                    .to_string(),
                "live_disabled",
            ));
        }
    }

    // ⑥ demo: 必须已配置 demo 凭据(避免 daemon 拉起后才开始缺 key 报错)。
    if demo {
        let f = crate::commands::config_file::load(&state.root)?;
        let filled = |v: Option<&str>| v.map(str::trim).is_some_and(|s| !s.is_empty());
        let has_key = filled(f.exchange.demo_key.as_deref());
        let has_secret = filled(f.exchange.demo_secret.as_deref());
        if !has_key || !has_secret {
            return Err(WebError::bad_request_code(
                "demo 启动需要先在设置页配置 demo API Key/Secret (设置 → 密钥)。",
                "need_keys",
            ));
        }
    }

    // ⑦ daemon 自举(幂等): 已在跑直接复用, 不在则后台拉起; 失败按 500 原文透出(含日志指引)。
    if crate::supervisor::client::Client::connect(&state.root).await.is_err() {
        crate::commands::daemon::ensure_daemon(&state.root).await?;
    }

    // ⑧ 已在运行 → 409 冲突(运行中的判定只来自 daemon, 台账 running=false 不冒充)。
    let snap = crate::commands::instances::snapshot(&state.root).await;
    if snap.instances.iter().any(|v| v.name == id && v.running) {
        return Err(WebError::conflict(format!("策略 {id} 已在运行, 不重复拉起"), "running"));
    }

    // ⑨ live: 共享预检(018 确认 / 002 时长门禁 / FR-008 时钟); accept_risk=false ——
    // 绝不代用户落确认(确认只能由 ④ 那次显式动作产生)。
    if live {
        let _ = crate::commands::ctrl::live_preflight(&state.root, &id, false)
            .await
            .map_err(start_reject)?;
    }

    // ⑩ 拉起: confirmed = live(照 CLI, 子进程免二次交互)。
    let (pid, mode) = crate::commands::ctrl::start_daemon(&state.root, &id, live, demo, live)
        .await
        .map_err(start_reject)?;
    Ok(Json(StartReply { pid, mode }).into_response())
}

/// `POST /api/strategies/{id}/stop` (FR-025): 优雅停机; `close_all` 如实透传 daemon。
pub(super) async fn stop_strategy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<StopReply>, WebError> {
    let Json(raw) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: StopRequest = serde_json::from_value(raw)
        .map_err(|e| WebError::bad_request(format!("请求体字段不符合要求: {e}")))?;

    // 未在运行 → 400(判定只来自 daemon; 不在线时台账 running=false 不冒充 → 同样给 400)。
    let snap = crate::commands::instances::snapshot(&state.root).await;
    if !snap.instances.iter().any(|v| v.name == id && v.running) {
        return Err(WebError::bad_request_code(
            format!("策略 {id} 未在运行 (无需停止)"),
            "not_running",
        ));
    }
    let report = crate::commands::ctrl::stop_daemon(&state.root, &id, req.close_all).await?;
    Ok(Json(StopReply { stopped: true, report }))
}

/// `POST /api/risk-ack` (FR-026): 首次风险披露确认落盘; 短语必须逐字「确认风险」。
///
/// 契约原文为 204 无体(http-api §5); 本轮按任务口径返回 200 `{acked:true}`(偏离已记录)。
pub(super) async fn post_risk_ack(
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<RiskAckReply>, WebError> {
    let Json(raw) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: RiskAckRequest = serde_json::from_value(raw)
        .map_err(|e| WebError::bad_request(format!("请求体字段不符合要求: {e}")))?;

    // 不符 → 400 且无任何副作用(不落盘)。
    if !phrase_matches(Some(&req.phrase), RISK_ACK_PHRASE) {
        return Err(WebError::bad_request(format!(
            "未确认: 输入与确认短语不一致(期望逐字: {RISK_ACK_PHRASE}); 未执行任何动作。"
        )));
    }
    crate::commands::write_risk_ack()?;
    Ok(Json(RiskAckReply { acked: true }))
}

#[cfg(test)]
mod tests;
