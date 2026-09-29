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
    input.is_some_and(|p| super::ct_eq(p, expected))
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
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use ricow_strategy::Database;
    use rust_decimal_macros::dec;

    // ---- 纯函数单测 ----

    #[test]
    fn test_phrase_matches() {
        assert!(phrase_matches(Some("确认风险"), "确认风险"));
        assert!(phrase_matches(Some("确认实盘 grid-1"), "确认实盘 grid-1"));

        // 错短语 / 空串 / 缺失 / 长度差 / 前后缀 → 一律不匹配(逐字, 不做 trim 与包含)。
        assert!(!phrase_matches(Some("确认 风险"), "确认风险"));
        assert!(!phrase_matches(Some(""), "确认风险"));
        assert!(!phrase_matches(None, "确认风险"));
        assert!(!phrase_matches(Some("确认"), "确认风险"));
        assert!(!phrase_matches(Some("确认风险 "), "确认风险"));
        assert!(!phrase_matches(Some("确认风险。"), "确认风险"));
        assert!(!phrase_matches(Some("确认实盘 grid-2"), "确认实盘 grid-1"));
    }

    #[test]
    fn test_source_of_matrix() {
        // daemon 在线 = 权威(它答"没跑"也是 daemon 来源)。
        assert_eq!(source_of(true, true), "daemon");
        assert_eq!(source_of(true, false), "daemon");
        // 离线: 台账有记录 = ledger, 无记录 = none。
        assert_eq!(source_of(false, true), "ledger");
        assert_eq!(source_of(false, false), "none");
    }

    #[test]
    fn test_mode_or_unknown() {
        assert_eq!(mode_or_unknown(Some("dry_run")), "dry_run");
        assert_eq!(mode_or_unknown(Some("live")), "live");
        assert_eq!(mode_or_unknown(None), "unknown");
    }

    #[test]
    fn test_fmt_asof() {
        assert_eq!(fmt_asof(1_690_000_000_000), "2023-07-22T04:26:40+00:00");
        // 不可表示的时刻 → 空串(不伪造)。
        assert_eq!(fmt_asof(i64::MIN), "");
    }

    #[test]
    fn test_pnl_summary_from_record() {
        let rec = PnlSnapshotRecord {
            strategy_id: "grid-1".into(),
            timestamp: 1_690_000_000_000,
            realized_pnl: dec!(12.5),
            fees: dec!(0.07),
            net_pnl: dec!(12.43),
            trade_count: 3,
        };
        let s = PnlSummary::from(&rec);
        assert_eq!(s.realized, "12.5");
        assert_eq!(s.fees, "0.07");
        assert_eq!(s.net, "12.43");
        assert_eq!(s.trade_count, 3);
        assert_eq!(s.asof, "2023-07-22T04:26:40+00:00");
    }

    fn sample_view(name: &str) -> InstanceView {
        InstanceView {
            name: name.to_string(),
            running: true,
            pid: Some(4242),
            started_at: None,
            uptime_secs: Some(65),
            mode: Some("demo".into()),
            pair: Some("ETHUSDT".into()),
            market: None,
            last_exit: None,
            last_exit_at: None,
            last_reason: None,
        }
    }

    #[test]
    fn test_build_status_matrix() {
        // daemon 在线 + 有视图 → source=daemon, 字段照抄视图。
        let view = sample_view("grid-1");
        let s = build_status("grid-1", true, Some(&view), None, None);
        assert_eq!(s.source, "daemon");
        assert!(s.running);
        assert_eq!(s.mode, "demo");
        assert_eq!(s.pid, Some(4242));
        assert_eq!(s.uptime_secs, Some(65));
        assert_eq!(s.pair.as_deref(), Some("ETHUSDT"));
        assert_eq!(s.name, "grid-1");

        // daemon 在线即权威: 即使它报"未运行", 信息也来自 daemon → source=daemon。
        let mut idle_view = sample_view("grid-1");
        idle_view.running = false;
        let s = build_status("grid-1", true, Some(&idle_view), Some("BTCUSDT".into()), None);
        assert_eq!(s.source, "daemon");
        assert!(!s.running);
        assert_eq!(s.pair.as_deref(), Some("ETHUSDT"), "视图有 pair 时不回退 TOML");

        // daemon 离线但台账有记录 → source=ledger; 台账视图 pair 缺失时回退 TOML。
        let mut ledger_view = sample_view("grid-1");
        ledger_view.running = false;
        ledger_view.pair = None;
        let s = build_status("grid-1", false, Some(&ledger_view), Some("BTCUSDT".into()), None);
        assert_eq!(s.source, "ledger");
        assert!(!s.running);
        assert_eq!(s.pair.as_deref(), Some("BTCUSDT"), "视图无 pair 时回退 TOML");

        // 无视图(daemon 离线, 仅 TOML)→ source=none / mode=unknown / 不伪造 running。
        let s = build_status("grid-1", false, None, Some("BTCUSDT".into()), None);
        assert_eq!(s.source, "none");
        assert_eq!(s.mode, "unknown");
        assert!(!s.running);
        assert_eq!(s.pid, None);
        assert_eq!(s.pair.as_deref(), Some("BTCUSDT"));
        assert!(s.pnl.is_none(), "缺 PnL 必须给 null, 不伪造");
    }

    #[test]
    fn test_build_run_item() {
        let view = InstanceView {
            name: "grid-2".into(),
            running: false,
            pid: None,
            started_at: None,
            uptime_secs: None,
            mode: None,
            pair: None,
            market: None,
            last_exit: None,
            last_exit_at: None,
            last_reason: None,
        };
        let item = build_run_item(false, &view, None);
        assert_eq!(item.name, "grid-2");
        assert!(!item.running);
        assert_eq!(item.mode, "unknown");
        assert_eq!(item.source, "ledger", "daemon 离线但台账有此实例");
        assert_eq!(item.pid, None);
        assert!(item.pnl.is_none());
    }

    // ---- 端到端(真实回环套接字, 全程离线) ----

    /// 临时数据目录。
    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-web-runs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建临时数据目录");
        d
    }

    /// 起真实监听服务(中间件 + 路由一体), 返回端口; db 由调用方注入(runs 用例要预插 PnL)。
    async fn boot(root: PathBuf, db: Database) -> u16 {
        let store = super::super::SessionStore::new(db.clone());
        let starter: super::super::Starter =
            Arc::new(|_id: &str, _sink: &mut super::super::WebSink| Ok(()));
        let state = super::super::WebState::new("tok-ok".to_string(), root, db, store, starter);
        let (listener, port) = super::super::bind(0).await.expect("绑定回环端口");
        tokio::spawn(super::super::serve(listener, state));
        port
    }

    /// 裸 HTTP/1.1 请求(本 crate 无 HTTP 客户端依赖), 回完整响应文本。
    async fn raw(port: u16, method: &str, target: &str, body: Option<&str>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect((super::super::BIND_ADDR, port))
            .await
            .expect("连上服务");
        let head = match body {
            Some(b) => format!(
                "{method} {target} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{b}",
                super::super::BIND_ADDR,
                b.len()
            ),
            None => format!(
                "{method} {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                super::super::BIND_ADDR
            ),
        };
        stream.write_all(head.as_bytes()).await.expect("发请求");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("读响应");
        String::from_utf8_lossy(&buf).to_string()
    }

    fn status_of(res: &str) -> u16 {
        res.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn body_of(res: &str) -> &str {
        res.split_once("\r\n\r\n").map_or("", |(_, b)| b)
    }

    /// 401 矩阵: 五个新端点全部在 token 门禁之后 —— 无/错 token 一律 401 空体,
    /// 且不回显请求体(照 markets.rs / strategy_io.rs 离线风格: 401 在中间件短路)。
    #[tokio::test]
    async fn test_run_endpoints_require_token_offline() {
        let root = tmp_root("auth");
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = boot(root, db).await;

        let probes: Vec<(&str, &str, Option<String>)> = vec![
            ("GET", "/api/runs", None),
            ("GET", "/api/runs?token=wrong", None),
            ("GET", "/api/strategies/grid-1/status", None),
            ("GET", "/api/strategies/grid-1/status?token=wrong", None),
            ("POST", "/api/strategies/grid-1/start", Some(r#"{"mode":"dry_run"}"#.into())),
            (
                "POST",
                "/api/strategies/grid-1/start?token=wrong",
                Some(r#"{"mode":"dry_run"}"#.into()),
            ),
            ("POST", "/api/strategies/grid-1/stop", Some("{}".into())),
            ("POST", "/api/strategies/grid-1/stop?token=wrong", Some("{}".into())),
            ("POST", "/api/risk-ack", Some(r#"{"phrase":"确认风险"}"#.into())),
            ("POST", "/api/risk-ack?token=wrong", Some(r#"{"phrase":"确认风险"}"#.into())),
        ];
        for (method, target, body) in probes {
            let res = raw(port, method, target, body.as_deref()).await;
            assert!(res.starts_with("HTTP/1.1 401"), "{method} {target} 应 401, 实际: {res}");
            assert!(body_of(&res).is_empty(), "401 响应体必须为空: {target}");
        }

        // 正确 token 下 /api/runs 可达(证明上面 401 是门禁拦截而非路由缺失): 空目录 → []。
        let res = raw(port, "GET", "/api/runs?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        assert_eq!(body_of(&res), "[]", "空目录无实例: {res}");
    }

    /// status 离线矩阵: 台账实例 → source=ledger; 仅 TOML → source=none + pair 回退; 未知名 → 404。
    #[tokio::test]
    async fn test_status_offline_matrix() {
        use crate::supervisor::ledger::{ensure_dirs, write_instance, InstanceRecord};

        let root = tmp_root("status");
        let db = Database::open_in_memory().await.expect("开内存库");

        // 台账实例(无 TOML): daemon 离线 → snapshot 退回台账, running=false 无权威性。
        ensure_dirs(&root).expect("建 run/ logs/");
        write_instance(
            &root,
            &InstanceRecord {
                name: "grid-ledger".into(),
                mode: Some("dry_run".into()),
                pair: Some("ETHUSDT".into()),
                ..Default::default()
            },
        )
        .expect("写台账");

        // 仅 TOML 实例: pair 存 params(get_str 读取)。
        let toml = "[strategy]\nname = \"grid-toml\"\ntype = \"shannon_spot_grid\"\n\
                    exchange = \"binance\"\n\n[strategy.params]\npair = \"BTCUSDT\"\n";
        std::fs::create_dir_all(root.join("strategies")).expect("建 strategies/");
        std::fs::write(root.join("strategies/grid-toml.toml"), toml).expect("写 TOML");

        let port = boot(root, db).await;

        // ① 台账实例: source=ledger, mode 如实, pnl=null(库空不伪造)。
        let res = raw(port, "GET", "/api/strategies/grid-ledger/status?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""name":"grid-ledger""#), "{body}");
        assert!(body.contains(r#""source":"ledger""#), "{body}");
        assert!(body.contains(r#""running":false"#), "{body}");
        assert!(body.contains(r#""mode":"dry_run""#), "{body}");
        assert!(body.contains(r#""pnl":null"#), "{body}");

        // ② 仅 TOML: source=none / mode=unknown / pair 回退 TOML params。
        let res = raw(port, "GET", "/api/strategies/grid-toml/status?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""source":"none""#), "{body}");
        assert!(body.contains(r#""mode":"unknown""#), "{body}");
        assert!(body.contains(r#""pair":"BTCUSDT""#), "{body}");

        // ③ 未知名 → 404(027 唯一文案)。
        let res = raw(port, "GET", "/api/strategies/no-such/status?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 404, "{res}");
        assert!(body_of(&res).contains("未找到策略或实例 no-such"), "{res}");
    }

    /// runs 列表离线: 台账实例进列表; 库里有快照 → pnl 带值(Decimal 字符串 + asof); 空目录已在 401 用例覆盖。
    #[tokio::test]
    async fn test_list_runs_offline() {
        use crate::supervisor::ledger::{ensure_dirs, write_instance, InstanceRecord};

        let root = tmp_root("runs");
        let db = Database::open_in_memory().await.expect("开内存库");
        ensure_dirs(&root).expect("建 run/ logs/");
        write_instance(&root, &InstanceRecord { name: "grid-ledger".into(), ..Default::default() })
            .expect("写台账");

        // 预插一条 PnL 快照(时间戳固定, asof 断言可精确)。
        db.insert_pnl_snapshot(&PnlSnapshotRecord {
            strategy_id: "grid-ledger".into(),
            timestamp: 1_690_000_000_000,
            realized_pnl: dec!(1.5),
            fees: dec!(0.02),
            net_pnl: dec!(1.48),
            trade_count: 2,
        })
        .await
        .expect("插 PnL 快照");

        let port = boot(root, db).await;
        let res = raw(port, "GET", "/api/runs?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""name":"grid-ledger""#), "{body}");
        assert!(body.contains(r#""source":"ledger""#), "daemon 离线 → 台账来源: {body}");
        assert!(body.contains(r#""realized":"1.5""#), "Decimal 走字符串: {body}");
        assert!(body.contains(r#""asof":"2023-07-22T04:26:40+00:00""#), "{body}");
    }

    /// start 门禁离线矩阵(全部在 daemon 自举之前被拒, 绝不真拉进程):
    /// 错 mode → 400; live 缺/错短语 → 400 且零副作用; live 正确短语未确认 → 403 need_risk_ack;
    /// 已确认但 TOML 未声明实盘 → 400 live_disabled; demo 未配凭据 → 400 need_keys。
    // 有意持 ENV_LOCK 跨 await: ④ risk_acked() 读进程级 RICOW_ROOT, 必须串行化。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_start_gates_offline() {
        let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
        let root = tmp_root("start");
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = boot(root.clone(), db).await;
        std::env::set_var("RICOW_ROOT", &root);

        // ① mode 白名单。
        let res = raw(
            port,
            "POST",
            "/api/strategies/grid-1/start?token=tok-ok",
            Some(r#"{"mode":"real"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains(r#""code":"invalid_mode""#), "{res}");

        // ② live 缺短语 → 400, 回显期望串, 零副作用(不写 run/daemon.json)。
        let res = raw(
            port,
            "POST",
            "/api/strategies/grid-1/start?token=tok-ok",
            Some(r#"{"mode":"live"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("确认实盘 grid-1"), "错误文案回显期望串: {res}");
        assert!(!root.join("run/daemon.json").exists(), "不得自举 daemon: {res}");

        // ③ live 错短语 → 同上 400。
        let res = raw(
            port,
            "POST",
            "/api/strategies/grid-1/start?token=tok-ok",
            Some(r#"{"mode":"live","phrase":"确认实盘 grid-2"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");

        // ④ live 正确短语但未做首次风险确认 → 403 need_risk_ack + 披露原文, 不代落确认。
        let res = raw(
            port,
            "POST",
            "/api/strategies/grid-1/start?token=tok-ok",
            Some(r#"{"mode":"live","phrase":"确认实盘 grid-1"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 403, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""code":"need_risk_ack""#), "{body}");
        assert!(body.contains("disclosure"), "{body}");
        assert!(body.contains("不构成投资建议"), "应带披露原文片段: {body}");
        assert!(!root.join("risk_ack.json").exists(), "不得代落确认: {res}");

        // ⑤ 已确认 + TOML 未声明实盘 → 400 live_disabled(预置最小确认记录)。
        std::fs::write(root.join("risk_ack.json"), r#"{"version":1}"#).expect("预置确认记录");
        let res = raw(
            port,
            "POST",
            "/api/strategies/grid-1/start?token=tok-ok",
            Some(r#"{"mode":"live","phrase":"确认实盘 grid-1"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains(r#""code":"live_disabled""#), "{res}");

        // ⑥ demo 未配凭据(load 会生成空模板) → 400 need_keys。
        let res = raw(
            port,
            "POST",
            "/api/strategies/grid-1/start?token=tok-ok",
            Some(r#"{"mode":"demo"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains(r#""code":"need_keys""#), "{res}");

        std::env::remove_var("RICOW_ROOT");
    }

    /// stop 离线: 未运行 → 400 not_running, 绝不触 daemon; 非法 JSON → 400(先于业务判定)。
    #[tokio::test]
    async fn test_stop_not_running_offline() {
        let root = tmp_root("stop");
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = boot(root, db).await;

        // 空 body(close_all 缺省 false) + 未在运行 → 400 not_running。
        let res = raw(port, "POST", "/api/strategies/grid-1/stop?token=tok-ok", Some("{}")).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains(r#""code":"not_running""#), "{res}");
        assert!(body_of(&res).contains("未在运行"), "{res}");

        // 非法 JSON → 400。
        let res =
            raw(port, "POST", "/api/strategies/grid-1/stop?token=tok-ok", Some("{oops")).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("JSON"), "{res}");
    }

    /// risk-ack 离线: 错/空短语 → 400 且 risk_ack.json 不落盘; 正确短语 → 200 + version=1 落盘。
    // 有意持 ENV_LOCK 跨 await: 写入走 project_root()(RICOW_ROOT), 必须串行化。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_risk_ack_offline() {
        use crate::commands::{risk_ack_path, risk_acked};

        let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
        let root = tmp_root("ack");
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = boot(root.clone(), db).await;
        std::env::set_var("RICOW_ROOT", &root);

        // ① 空短语 → 400, 不落盘。
        let res = raw(port, "POST", "/api/risk-ack?token=tok-ok", Some(r#"{"phrase":""}"#)).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("确认风险"), "错误文案回显期望串: {res}");
        assert!(!root.join("risk_ack.json").exists(), "错短语不得落盘: {res}");

        // ② 错短语 → 同上。
        let res =
            raw(port, "POST", "/api/risk-ack?token=tok-ok", Some(r#"{"phrase":"确认 风险"}"#))
                .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(!root.join("risk_ack.json").exists(), "{res}");

        // ③ 正确短语 → 200 {acked:true} + version=1 落盘(契约 204, 本轮按 200 偏离已记录)。
        let res =
            raw(port, "POST", "/api/risk-ack?token=tok-ok", Some(r#"{"phrase":"确认风险"}"#)).await;
        assert_eq!(status_of(&res), 200, "{res}");
        assert!(body_of(&res).contains(r#""acked":true"#), "{res}");
        assert!(risk_acked(), "version=1 记录应已落盘");
        assert!(risk_ack_path().exists());

        std::env::remove_var("RICOW_ROOT");
    }
}
