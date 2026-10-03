//! 032 US3 回测异步作业 (FR-019, data-model §7 / http-api §4):
//!
//! - `POST /api/backtest` — 页面选标的/周期/天数/参数覆盖发起回测, 立即 `202 {job_id}`,
//!   内核在 tokio 任务里跑(与 CLI **同一个** [`run_backtest_core`], 报告同一口径);
//! - `GET /api/backtest/{job_id}` — 轮询 `running|done|error`;done 带报告, error 带中文原因。
//!
//! 并发规则: **同一策略名同时只允许一个 running**(再次发起 → 409 `code:"busy"`);
//! 不同策略可并行。状态只允许 `running → done|error` 单向转移。
//! 生命周期: done/error 结果保留 5 分钟, 每次发起/查询时惰性清理; 再查即 404。
//! 作业表纯内存(Mutex<HashMap>), 进程重启即清空 —— 不做持久化(规格明确"内存")。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{rejection::JsonRejection, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::strategy_io::{config_values, parse_market};
use super::{WebError, WebState};
use crate::commands::backtest::{run_backtest_core, BacktestRunSpec};

/// done/error 结果保留时长(data-model §7: 5 分钟后惰性删除)。
const RETAIN: Duration = Duration::minutes(5);

// ---- 作业存储(纯内存, 时间可注入便于单测) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Running,
    Done,
    Error,
}

impl JobStatus {
    fn as_str(self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Error => "error",
        }
    }
}

/// 一个回测作业的内部记录(不直接序列化: 对客户端只回 [`JobReply`])。
#[derive(Debug, Clone)]
pub(super) struct BacktestJob {
    /// 发起时定位的策略名(busy 判定按它, 不按 job_id)。
    strategy: String,
    status: JobStatus,
    report: Option<String>,
    error: Option<String>,
    // 登记时刻(规格 §7 作业字段; 清理以 finished_at 为准, 此字段留作观测/审计, 单测会断言)。
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
    /// 完成时间: running 为 None; done/error 后作为 5 分钟保留期的起算点。
    finished_at: Option<DateTime<Utc>>,
}

/// 发起冲突: 同一策略已有 running 作业(调用法据此回 409 busy)。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct JobBusy;

/// `GET /api/backtest/{job_id}` 响应(data-model §7): running 只回 status;
/// done 带 report; error 带 error(缺省字段直接不出现)。
#[derive(Debug, Serialize)]
pub(super) struct JobReply {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl From<&BacktestJob> for JobReply {
    fn from(j: &BacktestJob) -> Self {
        Self {
            status: j.status.as_str().to_string(),
            report: j.report.clone(),
            error: j.error.clone(),
        }
    }
}

/// 内存作业表: web 层持有 `Arc<JobStore>`(随 [`WebState`] 一份, spawn 出去的内核任务持克隆)。
pub(super) struct JobStore {
    inner: Mutex<HashMap<String, BacktestJob>>,
}

/// 取锁并**容忍毒化**。
///
/// 作业表是纯内存、进程重启即清空的缓存, 毒化(另一线程持锁时 panic)不构成正确性威胁;
/// 而 `.expect("作业表锁")` 会让**此后每个**回测请求都 500 —— daemon 里一处 panic 放大成
/// 整个回测面板永久停摆。宁可带毒继续, 也不要级联失败。
fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl JobStore {
    pub(super) fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    /// 惰性清理: 删除"完成时刻"已过保留期的 done/error; running 永不因时间被清。
    fn prune(map: &mut HashMap<String, BacktestJob>, now: DateTime<Utc>) {
        let cutoff = now - RETAIN;
        map.retain(|_, job| match job.finished_at {
            Some(finished) => finished > cutoff,
            None => true,
        });
    }

    /// 发起作业: 先清过期记录, 再查同名 running; 通过则登记 running 并回新 job_id。
    pub(super) fn start(&self, strategy: &str, now: DateTime<Utc>) -> Result<String, JobBusy> {
        let mut map = lock_or_recover(&self.inner);
        Self::prune(&mut map, now);
        if map.values().any(|j| j.strategy == strategy && j.status == JobStatus::Running) {
            return Err(JobBusy);
        }
        let job_id = uuid::Uuid::new_v4().to_string();
        map.insert(
            job_id.clone(),
            BacktestJob {
                strategy: strategy.to_string(),
                status: JobStatus::Running,
                report: None,
                error: None,
                created_at: now,
                finished_at: None,
            },
        );
        Ok(job_id)
    }

    /// 作业成功: running → done, 记报告与完成时刻(单向; 非 running 记录一律不改)。
    pub(super) fn finish_done(&self, job_id: &str, report: String, now: DateTime<Utc>) {
        self.finish(job_id, JobStatus::Done, Some(report), None, now);
    }

    /// 作业失败: running → error, 记中文原因与完成时刻(单向; 非 running 记录一律不改)。
    pub(super) fn finish_error(&self, job_id: &str, error: String, now: DateTime<Utc>) {
        self.finish(job_id, JobStatus::Error, None, Some(error), now);
    }

    fn finish(
        &self,
        job_id: &str,
        status: JobStatus,
        report: Option<String>,
        error: Option<String>,
        now: DateTime<Utc>,
    ) {
        let mut map = lock_or_recover(&self.inner);
        if let Some(job) = map.get_mut(job_id) {
            if job.status == JobStatus::Running {
                job.status = status;
                job.report = report;
                job.error = error;
                job.finished_at = Some(now);
            }
        }
    }

    /// 查询: 先清过期, 再取记录克隆(Unknown/已清理 → None, handler 回 404)。
    pub(super) fn get(&self, job_id: &str, now: DateTime<Utc>) -> Option<JobReply> {
        let mut map = lock_or_recover(&self.inner);
        Self::prune(&mut map, now);
        map.get(job_id).map(JobReply::from)
    }
}

// ---- HTTP 层 ----

/// `POST /api/backtest` 请求体(data-model §7 + http-api §4): 用 `strategy` 定位内置/用户策略,
/// 其余全部可选(缺省与 CLI 一致: 90 天 / 1h / spot / 策略 TOML 自带参数)。
#[derive(Debug, Deserialize)]
pub(super) struct BacktestRequest {
    /// 策略名(内置 id 或已部署用户策略名)。
    strategy: String,
    /// 交易对(TOML 已含 pair 时可省)。
    #[serde(default)]
    pair: Option<String>,
    /// 市场 spot|futures(缺省随策略)。
    #[serde(default)]
    market: Option<String>,
    /// K 线间隔 1m/5m/15m/1h/4h/1d(缺省 1h)。
    #[serde(default)]
    interval: Option<String>,
    /// 回测天数(缺省 90)。
    #[serde(default)]
    days: Option<u32>,
    /// 窗口起点 YYYY-MM-DD(给了 start 即覆盖 days 窗口语义, 与 CLI 一致)。
    #[serde(default)]
    start: Option<String>,
    /// 窗口终点 YYYY-MM-DD(不含)。
    #[serde(default)]
    end: Option<String>,
    /// 手续费 bps(maker/taker 同设)。
    #[serde(default)]
    fee: Option<f64>,
    /// 初始现金(quote)。
    #[serde(default)]
    cash: Option<f64>,
    /// 合约杠杆。
    #[serde(default)]
    leverage: Option<f64>,
    /// 策略参数覆盖(数字/字符串/布尔)。
    #[serde(default)]
    params: HashMap<String, Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct StartedReply {
    job_id: String,
}

/// K 线间隔白名单(与 CLI 内核、markets 端点同口径)。
const INTERVALS: [&str; 6] = ["1m", "5m", "15m", "1h", "4h", "1d"];

/// 间隔解析(纯函数): 缺省/空白 → 1h; 白名单外 → 中文错误。
fn parse_interval(raw: Option<&str>) -> Result<&'static str, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok("1h"),
        Some(s) => INTERVALS
            .iter()
            .copied()
            .find(|iv| *iv == s)
            .ok_or_else(|| format!("interval 仅支持 {} , 收到: {s}", INTERVALS.join("/"))),
    }
}

/// 非负有限数校验(fee 允许 0; 纯函数)。
fn check_non_negative(field: &str, v: f64) -> Result<(), String> {
    if v.is_finite() && v >= 0.0 {
        Ok(())
    } else {
        Err(format!("{field} 必须是不小于 0 的有限数, 收到: {v}"))
    }
}

/// 去空白并过滤空串。
fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `POST /api/backtest`: 同步只做参数校验/登记作业(202), 回测内核在 tokio 任务里跑。
///
/// 参数问题同步 400/404/409;内核自身的失败(取数/预热/撮合)不挡发起, 稍后经 GET 的
/// `status:"error"` 如实取回 —— 回测可能耗时数十秒, 不能挂在 HTTP 请求上。
pub(super) async fn start_backtest(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<(StatusCode, Json<StartedReply>), WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: BacktestRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!(
            "请求体字段非法: 需要 strategy 字符串, pair/market/interval/start/end 可选字符串, \
             days 整数, fee/cash/leverage 数值, params 对象: {e}"
        ))
    })?;

    // ---- 同步校验(全部离线, 不触网) ----
    let strategy = req.strategy.trim();
    if strategy.is_empty() {
        return Err(WebError::bad_request("strategy(策略名)不能为空"));
    }
    // 名字规范同时是防路径穿越(内核按名拼 strategies/<name>.toml)。
    ricow_strategy::validate_strategy_name(strategy)
        .map_err(|e| WebError::bad_request_code(e, "invalid_name"))?;
    // 必须是内置策略或数据目录里已部署的用户策略 —— 未知策略当场 404, 不开空头作业。
    let known = crate::strategies::catalog::is_builtin_id(strategy)
        || state.root.join("strategies").join(format!("{strategy}.toml")).is_file();
    if !known {
        return Err(WebError::not_found(format!(
            "没有策略 {strategy}: 回测只接受内置策略或已保存的用户策略"
        )));
    }
    let interval = parse_interval(req.interval.as_deref()).map_err(WebError::bad_request)?;
    if let Some(d) = req.days {
        if !(1..=3650).contains(&d) {
            return Err(WebError::bad_request(format!("days 必须是 1..=3650 的整数, 收到: {d}")));
        }
    }
    let market = match req.market.as_deref() {
        Some(m) => Some(parse_market(m).map_err(WebError::bad_request)?),
        None => None,
    };
    check_non_negative("fee", req.fee.unwrap_or(0.0)).map_err(WebError::bad_request)?;
    check_non_negative("cash", req.cash.unwrap_or(0.0)).map_err(WebError::bad_request)?;
    if let Some(lv) = req.leverage {
        if !lv.is_finite() || lv <= 0.0 {
            return Err(WebError::bad_request(format!(
                "leverage 必须是大于 0 的有限数, 收到: {lv}"
            )));
        }
    }
    // 参数值类型与保存端点同一套转换(键名错误带键名 400)。
    let params = config_values(&req.params).map_err(WebError::bad_request)?;

    // ---- 组装内核输入(与 CLI from_cli_args 同结构) ----
    let fee = req.fee;
    let spec = BacktestRunSpec {
        root: (*state.root).clone(),
        strategy: strategy.to_string(),
        pair: clean(req.pair),
        days: req.days.unwrap_or(90),
        interval: interval.to_string(),
        start: clean(req.start),
        end: clean(req.end),
        market: market.map(str::to_string),
        position_mode: None,
        params,
        script_path: None,
        fee,
        fee_maker: None,
        fee_taker: None,
        slippage_bps: None,
        cash: req.cash,
        leverage: req.leverage,
        max_leverage: None,
        mmr_pct: None,
        funding_rate: None,
    };

    // ---- 登记作业(同名 running → 409 busy); 惰性清理也发生在这里 ----
    let job_id = state.jobs.start(strategy, Utc::now()).map_err(|_busy| {
        WebError::conflict(
            format!(
                "策略 {strategy} 已有回测在运行: 同一策略同时只允许一个回测作业 (FR-019), \
                     请等当前作业结束后再发起(可轮询原 job_id 取结果)"
            ),
            "busy",
        )
    })?;

    // ---- 后台跑内核(async 全链路: HTTP 取数是 reqwest 异步, 撮合为短暂同步 CPU) ----
    let jobs = Arc::clone(&state.jobs);
    let id = job_id.clone();
    tokio::spawn(async move {
        match run_backtest_core(spec).await {
            Ok(report) => jobs.finish_done(&id, report, Utc::now()),
            // CoreError 的中文 Display 直接作为 error 文本(含"预热段不足/窗口日期非法"等)。
            Err(e) => jobs.finish_error(&id, e.to_string(), Utc::now()),
        }
    });

    Ok((StatusCode::ACCEPTED, Json(StartedReply { job_id })))
}

/// `GET /api/backtest/{job_id}`: running/done/error 三态;未知或已过期清理 → 404。
pub(super) async fn get_backtest(
    State(state): State<WebState>,
    UrlPath(job_id): UrlPath<String>,
) -> Result<Json<JobReply>, WebError> {
    state
        .jobs
        .get(&job_id, Utc::now())
        .map(Json)
        .ok_or_else(|| WebError::not_found("回测作业不存在或已过期: done/error 结果只保留 5 分钟"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use ricow_strategy::Database;

    // ---- JobStore 纯逻辑(时间注入, 不触网) ----

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).expect("固定时间戳")
    }

    #[test]
    fn test_job_lifecycle_running_then_done_then_expire() {
        let store = JobStore::new();
        let t0 = t(1_000_000);

        // 发起 → running 可查。
        let id = store.start("my-grid", t0).unwrap();
        let r = store.get(&id, t0).expect("running 作业可查");
        assert_eq!(r.status, "running");
        assert!(r.report.is_none() && r.error.is_none());
        // 登记时刻按注入值落记录(观测/审计字段)。
        assert_eq!(store.inner.lock().unwrap().get(&id).expect("记录存在").created_at, t0);

        // 未知 id → None(handler 据此 404)。
        assert!(store.get("no-such", t0).is_none());

        // 完成 → done 带报告; 完成后保留期内任意时刻可查。
        store.finish_done(&id, "回测报告正文".into(), t0 + Duration::seconds(60));
        let r = store.get(&id, t0 + Duration::seconds(60)).unwrap();
        assert_eq!(r.status, "done");
        assert_eq!(r.report.as_deref(), Some("回测报告正文"));
        assert!(r.error.is_none());
        // 4分59秒仍在(5 分钟保留期)。
        assert!(store.get(&id, t0 + Duration::seconds(359)).is_some());
        // 满 5 分钟后查询触发惰性清理 → 已删除。
        assert!(store.get(&id, t0 + Duration::seconds(360)).is_none());
    }

    #[test]
    fn test_busy_is_per_strategy_and_blocks_only_running() {
        let store = JobStore::new();
        let t0 = t(2_000_000);

        let id_a1 = store.start("grid-a", t0).unwrap();
        // 同名再发起 → busy。
        assert_eq!(store.start("grid-a", t0 + Duration::seconds(1)), Err(JobBusy));
        // 不同名可并行。
        let id_b = store.start("grid-b", t0 + Duration::seconds(1)).unwrap();
        assert_ne!(id_a1, id_b);

        // a 完成后, 同名可再次发起; 旧作业进入保留期但不再占 running。
        store.finish_done(&id_a1, "r".into(), t0 + Duration::seconds(2));
        let id_a2 = store.start("grid-a", t0 + Duration::seconds(3)).unwrap();
        assert_ne!(id_a1, id_a2);
        // b 仍在 running, 名字维度独立, 不受影响。
        assert_eq!(store.start("grid-b", t0 + Duration::seconds(3)), Err(JobBusy));
    }

    #[test]
    fn test_error_terminal_and_one_way_transition() {
        let store = JobStore::new();
        let t0 = t(3_000_000);
        let id = store.start("grid-e", t0).unwrap();

        store.finish_error(&id, "no klines for X".into(), t0 + Duration::seconds(5));
        let r = store.get(&id, t0 + Duration::seconds(5)).unwrap();
        assert_eq!(r.status, "error");
        assert_eq!(r.error.as_deref(), Some("no klines for X"));
        assert!(r.report.is_none());

        // 单向: 终结后再 finish(任何结果)都不得翻转/覆盖。
        store.finish_done(&id, "迟到的报告".into(), t0 + Duration::seconds(6));
        let r = store.get(&id, t0 + Duration::seconds(6)).unwrap();
        assert_eq!(r.status, "error", "done 不得覆盖已 error 的作业");
        assert!(r.report.is_none());

        // 未知 job_id 的 finish 静默忽略(不制造僵尸记录)。
        store.finish_error("ghost", "x".into(), t0 + Duration::seconds(6));
        assert!(store.get("ghost", t0 + Duration::seconds(6)).is_none());

        // error 同样保留 5 分钟后被清。
        assert!(store.get(&id, t0 + Duration::seconds(304)).is_some());
        assert!(store.get(&id, t0 + Duration::seconds(305)).is_none());
    }

    #[test]
    fn test_running_never_expires_and_prune_happens_on_start() {
        let store = JobStore::new();
        let t0 = t(4_000_000);
        let running_id = store.start("long-run", t0).unwrap();
        // 跑很久(running 无 finished_at), 任何时刻都不能被惰性清理。
        assert!(store.get(&running_id, t0 + Duration::seconds(3_600)).is_some());

        // 另一个作业完成后过期; 发起第三个作业(触发 prune)只清过期者, running 保留。
        let done_id = store.start("old-one", t0).unwrap();
        store.finish_done(&done_id, "r".into(), t0 + Duration::seconds(10));
        let expire_at = t0 + Duration::seconds(310);
        let _new = store.start("new-one", expire_at).unwrap();
        assert!(store.get(&done_id, expire_at).is_none(), "过期 done 应在 start 时被清");
        assert!(store.get(&running_id, expire_at).is_some(), "running 不应被清");
    }

    #[test]
    fn test_parse_interval_and_numeric_checks() {
        assert_eq!(parse_interval(None).unwrap(), "1h");
        assert_eq!(parse_interval(Some(" 4h ")).unwrap(), "4h");
        assert!(parse_interval(Some("2h")).unwrap_err().contains("interval"));
        assert!(check_non_negative("fee", 0.0).is_ok());
        assert!(check_non_negative("cash", -0.01).is_err());
        assert!(check_non_negative("fee", f64::NAN).is_err());
        assert!(check_non_negative("fee", f64::INFINITY).is_err());
        assert_eq!(clean(Some("  ETHUSDT ".into())).as_deref(), Some("ETHUSDT"));
        assert_eq!(clean(Some("   ".into())), None);
        assert_eq!(clean(None), None);
    }

    // ---- 端到端(真实回环套接字; 全程离线: 用坏日期让内核在任何取数前失败) ----

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-web-btjobs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建临时数据目录");
        d
    }

    async fn boot(root: PathBuf) -> u16 {
        let db = Database::open_in_memory().await.expect("开内存库");
        let store = super::super::SessionStore::new(db.clone());
        let starter: super::super::Starter =
            Arc::new(|_id: &str, _sink: &mut super::super::WebSink| Ok(()));
        let state = super::super::WebState::new("tok-ok".to_string(), root, db, store, starter);
        let (listener, port) = super::super::bind(0).await.expect("绑定回环端口");
        tokio::spawn(super::super::serve(listener, state));
        port
    }

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

    /// 轮询直到作业离开 running(或超时), 返回最后一次响应体。
    async fn wait_terminal(port: u16, job_id: &str) -> String {
        for _ in 0..100 {
            let res = raw(port, "GET", &format!("/api/backtest/{job_id}?token=tok-ok"), None).await;
            let body = body_of(&res).to_string();
            if !body.contains(r#""status":"running""#) {
                return body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("作业迟迟未离开 running(可能误触网络): {job_id}");
    }

    #[tokio::test]
    async fn test_backtest_endpoints_require_token_offline() {
        let root = tmp_root("auth");
        let port = boot(root).await;
        const MARK: &str = "BT-MARK-shannon_spot_grid";
        let body = format!(r#"{{"strategy":"{MARK}"}}"#);

        // POST 发起 / GET 查询都在 token 门禁之后: 无/错 token 一律 401 空体。
        for probe in ["/api/backtest", "/api/backtest?token=wrong"] {
            let res = raw(port, "POST", probe, Some(&body)).await;
            assert!(res.starts_with("HTTP/1.1 401"), "{probe}: {res}");
            assert!(body_of(&res).is_empty());
            assert!(!res.contains(MARK), "401 不得回显请求体: {res}");
        }
        for probe in ["/api/backtest/abc", "/api/backtest/abc?token=wrong"] {
            let res = raw(port, "GET", probe, None).await;
            assert!(res.starts_with("HTTP/1.1 401"), "{probe}: {res}");
            assert!(body_of(&res).is_empty());
        }
    }

    #[tokio::test]
    async fn test_start_backtest_validation_offline() {
        let root = tmp_root("validate");
        let port = boot(root.clone()).await;
        let post = |json: &'static str| async move {
            raw(port, "POST", "/api/backtest?token=tok-ok", Some(json)).await
        };

        // ① 空 strategy / 非法名 → 400。
        let res = post(r#"{"strategy":"   "}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        let res = post(r#"{"strategy":"../evil"}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains(r#""code":"invalid_name""#), "{res}");

        // ② 未知策略(名合法但既非内置也无 TOML)→ 404。
        let res = post(r#"{"strategy":"no-such-xyz","pair":"ETHUSDT"}"#).await;
        assert_eq!(status_of(&res), 404, "{res}");
        assert!(body_of(&res).contains("没有策略"), "{res}");

        // ③ interval/days/market/数值/参数各类 400。
        let res = post(r#"{"strategy":"shannon_spot_grid","interval":"2h"}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("interval"), "{res}");
        let res = post(r#"{"strategy":"shannon_spot_grid","days":0}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("days"), "{res}");
        let res = post(r#"{"strategy":"shannon_spot_grid","market":"fx"}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        let res = post(r#"{"strategy":"shannon_spot_grid","cash":-1}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("cash"), "{res}");
        let res = post(r#"{"strategy":"shannon_spot_grid","leverage":0}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        let res = post(r#"{"strategy":"shannon_spot_grid","params":{"weird":null}}"#).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("weird"), "参数错误要带键名: {res}");

        // ④ 非 JSON → 400。
        let res = post("{oops").await;
        assert_eq!(status_of(&res), 400, "{res}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 202 发起 → job_id 可查 → 内核在取数前因坏日期失败 → GET 转 error(全程零网络)。
    #[tokio::test]
    async fn test_backtest_job_accepted_then_error_without_network() {
        let root = tmp_root("job");
        let port = boot(root).await;

        let res = raw(
            port,
            "POST",
            "/api/backtest?token=tok-ok",
            // start 日期非法: 内核窗口解析(parse_ymd_ms)先于任何网络取数, 立即失败。
            Some(r#"{"strategy":"shannon_spot_grid","pair":"ETHUSDT","start":"not-a-date"}"#),
        )
        .await;
        assert_eq!(status_of(&res), 202, "应立即 202: {res}");
        let body = body_of(&res);
        let job_id = serde_json::from_str::<serde_json::Value>(body)
            .expect("202 体应为 JSON")
            .get("job_id")
            .and_then(|v| v.as_str())
            .expect("含 job_id")
            .to_string();
        assert!(!job_id.is_empty());

        // 未知作业 → 404 中文。
        let res = raw(port, "GET", "/api/backtest/deadbeef?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 404, "{res}");
        assert!(body_of(&res).contains("作业不存在或已过期"), "{res}");

        // 轮询到终态: error 且错误说日期格式(证明跑的是 CLI 同一内核, 且没卡在取数)。
        let terminal = wait_terminal(port, &job_id).await;
        assert!(terminal.contains(r#""status":"error""#), "{terminal}");
        assert!(terminal.contains("YYYY-MM-DD"), "错误应是窗口日期解析原文: {terminal}");
    }
}
