//! 038 P1-B: Prometheus **只读**指标端点 (`GET /metrics`)。
//!
//! 定位: 让外部监控(Prometheus / Grafana / 自建脚本)**看得见**运行状态 —— 之前是零对接,
//! 出问题只能靠人盯终端。三条硬约束:
//!
//! 1. **不引第三方依赖**: 暴露格式就是 `name{label="v"} value\n`, 手写即可 ——
//!    为一个这么简单的文本格式引入 `prometheus` crate 不划算(宪法"少而精")。
//! 2. **与全部端点同一道 token 门**: 不新开放行口、不新开端口(D5)。
//! 3. **只读且不直连交易所**: 数据全部来自本地库与 daemon 台账 —— 与 026 确立的
//!    "本地库是唯一来源"一致; 本端点**不读密钥、不落盘、不触发任何动作**。
//!
//! 明确**不暴露**的: 对账修正次数(只在 `RunOutcome` 内存里、未持久化 —— 暴露它等于给假数字)、
//! 单号、密钥、策略源码。

use std::fmt::Write as _;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use super::WebState;

/// 本模块负责的只读路由(挂进 [`super::router`], 天然在 token 中间件之后)。
pub(super) fn routes() -> Router<WebState> {
    Router::new().route("/metrics", get(metrics))
}

/// `GET /metrics`: 采集一次本地快照并渲染成 Prometheus 文本格式。
async fn metrics(State(state): State<WebState>) -> Response {
    let snap = collect(&state).await;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        render(&snap),
    )
        .into_response()
}

/// 一次采集的结果 —— 纯数据, 渲染不做任何 IO(于是渲染逻辑可单测)。
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct Snapshot {
    /// daemon 是否在跑(读 `run/daemon.json` + 回环 TCP 探测)。
    pub daemon_up: bool,
    /// 台账里的实例 `(名字, 模式, 推断是否在运行)`。
    pub instances: Vec<(String, String, bool)>,
    /// 各策略**未了结**挂单数 `(策略, 条数)`。
    pub open_orders: Vec<(String, u64)>,
    /// 未平仓位 `(策略, 交易对, 模式, size)`。
    pub positions: Vec<(String, String, String, String)>,
    /// 各策略最新净盈亏快照 `(策略, net_pnl)`。
    pub net_pnl: Vec<(String, String)>,
    /// 本地库累计成交笔数。
    pub fills_total: i64,
    /// 回测作业 `(运行中, 总数)`。
    pub jobs: (usize, usize),
}

/// 采集本地快照。**任何一路读失败都降级为空**, 绝不让端点 500 ——
/// 监控端点的价值在于"始终能给出当前看到的东西", 而不是"要么全有要么全无"。
async fn collect(state: &WebState) -> Snapshot {
    let mut snap = Snapshot {
        daemon_up: crate::supervisor::daemon_is_running(&state.root).await,
        ..Snapshot::default()
    };

    // 台账: 实例清单。`last_exit.is_none()` 表示"最后一条记录没有退出码" —— 推断为在跑,
    // 但这是**台账推断**, 权威判断仍是 `ricow list`(HELP 里写明了这一点)。
    for rec in crate::supervisor::ledger::list_instances(&state.root) {
        snap.instances.push((
            rec.name,
            rec.mode.unwrap_or_else(|| "unknown".into()),
            rec.last_exit.is_none(),
        ));
    }

    // 本地库: 挂单 / 持仓 / 净盈亏 / 成交总数。读不到就留空(端点照常 200)。
    if let Ok(rows) = state.db.recent_orders(None, 5_000).await {
        let mut per: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for r in rows {
            if ricow_engine::exposure::is_open_status(&r.status) {
                *per.entry(r.strategy_id).or_insert(0) += 1;
            }
        }
        snap.open_orders = per.into_iter().collect();
    }
    if let Ok(rows) = state.db.current_positions(None).await {
        snap.positions = rows
            .into_iter()
            .map(|p| (p.strategy_id, p.pair, p.mode, p.size.normalize().to_string()))
            .collect();
    }
    if let Ok(rows) = state.db.recent_pnl_snapshots(None, 500).await {
        // 每策略只取**最新**一条(查询已按 timestamp DESC, 首次出现即最新)。
        let mut seen = std::collections::HashSet::new();
        for p in rows {
            if seen.insert(p.strategy_id.clone()) {
                snap.net_pnl.push((p.strategy_id, p.net_pnl.normalize().to_string()));
            }
        }
    }
    snap.fills_total = state.db.fill_count().await.unwrap_or(0);
    snap.jobs = state.jobs.counts();
    snap
}

/// 渲染成 Prometheus 文本暴露格式(纯函数)。
pub(super) fn render(s: &Snapshot) -> String {
    let mut out = String::new();

    help(&mut out, "ricow_daemon_up", "1 = daemon 在运行 (读台账 + 回环探测), 0 = 未运行");
    metric_type(&mut out, "ricow_daemon_up", "gauge");
    metric(&mut out, "ricow_daemon_up", &[], u8::from(s.daemon_up));

    help(
        &mut out,
        "ricow_instance",
        "台账里的策略实例; running 由台账推断(最后一条记录无退出码), 权威判断请用 ricow list",
    );
    metric_type(&mut out, "ricow_instance", "gauge");
    for (name, mode, running) in &s.instances {
        metric(
            &mut out,
            "ricow_instance",
            &[("name", name.as_str()), ("mode", mode.as_str())],
            u8::from(*running),
        );
    }

    help(&mut out, "ricow_open_orders", "本地库中该策略**未了结**订单(挂单中/部分成交)条数");
    metric_type(&mut out, "ricow_open_orders", "gauge");
    for (sid, n) in &s.open_orders {
        metric(&mut out, "ricow_open_orders", &[("strategy", sid.as_str())], *n);
    }

    help(&mut out, "ricow_position_size", "本地库中该 (策略, 交易对, 模式) 的持仓 size");
    metric_type(&mut out, "ricow_position_size", "gauge");
    for (sid, pair, mode, size) in &s.positions {
        metric(
            &mut out,
            "ricow_position_size",
            &[("strategy", sid.as_str()), ("pair", pair.as_str()), ("mode", mode.as_str())],
            Raw(size),
        );
    }

    help(&mut out, "ricow_net_pnl", "该策略最新一条净盈亏快照(计价币)");
    metric_type(&mut out, "ricow_net_pnl", "gauge");
    for (sid, v) in &s.net_pnl {
        metric(&mut out, "ricow_net_pnl", &[("strategy", sid.as_str())], Raw(v));
    }

    help(&mut out, "ricow_fills_total", "本地库累计成交笔数");
    metric_type(&mut out, "ricow_fills_total", "counter");
    metric(&mut out, "ricow_fills_total", &[], s.fills_total);

    help(&mut out, "ricow_backtest_jobs", "本进程内存中的回测作业数(running / 全部)");
    metric_type(&mut out, "ricow_backtest_jobs", "gauge");
    metric(&mut out, "ricow_backtest_jobs", &[("state", "running")], s.jobs.0);
    metric(&mut out, "ricow_backtest_jobs", &[("state", "all")], s.jobs.1);

    out
}

/// 已经是合法数字字面量的值(Decimal 归一化后的字符串), 直接拼, 不加引号。
struct Raw<'a>(&'a str);

trait Num {
    fn prom(self) -> String;
}
impl Num for u64 {
    fn prom(self) -> String {
        self.to_string()
    }
}
impl Num for i64 {
    fn prom(self) -> String {
        self.to_string()
    }
}
impl Num for u8 {
    fn prom(self) -> String {
        self.to_string()
    }
}
impl Num for usize {
    fn prom(self) -> String {
        self.to_string()
    }
}
impl Num for Raw<'_> {
    fn prom(self) -> String {
        self.0.to_string()
    }
}

fn help(out: &mut String, name: &str, text: &str) {
    let _ = writeln!(out, "# HELP {name} {}", escape_help(text));
}

fn metric_type(out: &mut String, name: &str, kind: &str) {
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

/// 输出一条样本。标签值按 Prometheus 文本格式转义(`\` → `\\`, `"` → `\"`, 换行 → `\n`);
/// **指标名与标签名不转义**(它们是本项目写死的字面量, 不是外部输入)。
fn metric<T: Num>(out: &mut String, name: &str, labels: &[(&str, &str)], value: T) {
    out.push_str(name);
    if !labels.is_empty() {
        out.push('{');
        for (i, (k, v)) in labels.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{k}=\"{}\"", escape_label(v));
        }
        out.push('}');
    }
    let _ = writeln!(out, " {}", value.prom());
}

/// 标签值转义(Prometheus 文本格式): `\` → `\\`, `"` → `\"`, LF → `\n`。
fn escape_label(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '"' => s.push_str("\\\""),
            '\n' => s.push_str("\\n"),
            _ => s.push(c),
        }
    }
    s
}

/// HELP 文本转义: 只处理反斜杠与换行(HELP 里没有引号语义)。
fn escape_help(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            daemon_up: true,
            instances: vec![("grid_eth".into(), "live".into(), true)],
            open_orders: vec![("grid_eth".into(), 3)],
            positions: vec![("grid_eth".into(), "ETHUSDT".into(), "live".into(), "1.5".into())],
            net_pnl: vec![("grid_eth".into(), "-12.34".into())],
            fills_total: 42,
            jobs: (1, 4),
        }
    }

    #[test]
    fn test_render_emits_prometheus_exposition_format() {
        let out = render(&sample());
        // 每类指标都要有 HELP + TYPE, 否则 Prometheus 侧解析不完整。
        for name in [
            "ricow_daemon_up",
            "ricow_instance",
            "ricow_open_orders",
            "ricow_position_size",
            "ricow_net_pnl",
            "ricow_fills_total",
            "ricow_backtest_jobs",
        ] {
            assert!(out.contains(&format!("# HELP {name} ")), "缺 HELP: {name}\n{out}");
        }
        assert!(out.contains("# TYPE ricow_daemon_up gauge"), "{out}");
        assert!(out.contains("ricow_daemon_up 1"), "{out}");
        assert!(out.contains("ricow_instance{name=\"grid_eth\",mode=\"live\"} 1"), "{out}");
        assert!(out.contains("ricow_open_orders{strategy=\"grid_eth\"} 3"), "{out}");
        assert!(out.contains("ricow_fills_total 42"), "{out}");
        assert!(out.contains("ricow_backtest_jobs{state=\"running\"} 1"), "{out}");
        assert!(out.contains("ricow_backtest_jobs{state=\"all\"} 4"), "{out}");
        // 每个样本行都必须以数字结尾(格式硬要求); 注释行不以数字结尾。
        for line in out.lines().filter(|l| !l.starts_with('#')) {
            let last = line.rsplit(' ').next().unwrap_or("");
            assert!(last.parse::<f64>().is_ok() || last == "NaN", "样本行末位必须是数字: {line:?}");
        }
    }

    #[test]
    fn test_decimal_values_are_passed_verbatim_not_as_quotes() {
        let out = render(&sample());
        // Decimal 保留精度, 且**不加引号**(Prometheus 里带引号就是解析错误)。
        assert!(out.contains("ricow_net_pnl{strategy=\"grid_eth\"} -12.34"), "{out}");
        assert!(
            out.contains(
                "ricow_position_size{strategy=\"grid_eth\",pair=\"ETHUSDT\",mode=\"live\"} 1.5"
            ),
            "{out}"
        );
    }

    #[test]
    fn test_empty_snapshot_still_renders_valid_skeleton() {
        let out = render(&Snapshot::default());
        assert!(out.contains("ricow_daemon_up 0"), "{out}");
        assert!(out.contains("ricow_fills_total 0"), "{out}");
        // 没有实例时不该凭空造样本行。
        assert!(!out.contains("ricow_instance{"), "{out}");
        assert!(!out.contains("ricow_open_orders{"), "{out}");
    }

    #[test]
    fn test_label_values_are_escaped() {
        let s = Snapshot {
            instances: vec![("we\"ird\\name\nx".into(), "live".into(), false)],
            ..Snapshot::default()
        };
        let out = render(&s);
        assert!(
            out.contains(r#"ricow_instance{name="we\"ird\\name\nx",mode="live"} 0"#),
            "反斜杠/引号/换行都必须转义, 否则整段暴露文本会解析失败:\n{out}"
        );
        // 转义后仍不得把一条样本劈成两行 —— 判据是"样本行只有一条, 且以值结尾",
        // **不能**写成 `!line.contains("x\"")`: 转义后的 `\n` 后面紧跟的就是用户数据的 `x`,
        // 再接标签值的收尾引号, 那个 `x"` 是合法输出, 拿它当"裸换行"会误报。
        let rows: Vec<&str> = out.lines().filter(|l| l.starts_with("ricow_instance{")).collect();
        assert_eq!(rows.len(), 1, "换行被劈成了多行: {rows:?}");
        assert!(rows[0].ends_with(" 0"), "样本行应以数值收尾: {:?}", rows[0]);
    }

    #[test]
    fn test_running_flag_tracks_ledger_not_daemon() {
        // 台账有记录 + daemon 没跑: instance 仍按台账报 —— 端点如实反映"台账怎么说",
        // 不做"daemon 没跑就全都算停"的二次推断(那会把"读不到"说成"没有")。
        let s = Snapshot {
            daemon_up: false,
            instances: vec![("a".into(), "demo".into(), true)],
            ..Snapshot::default()
        };
        let out = render(&s);
        assert!(out.contains("ricow_daemon_up 0"), "{out}");
        assert!(out.contains("ricow_instance{name=\"a\",mode=\"demo\"} 1"), "{out}");
    }

    /// FR-2.3: `/metrics` 与全部端点同一道 token 门 —— 无 token `401` **空体**;
    /// 带对 token 才 200 且含关键指标名。
    #[tokio::test]
    async fn test_metrics_requires_token_and_never_leaks_without_one() {
        use crate::web::test_support::{
            body_of, get_raw, seed_online_daemon, seed_trade_rows, serve_test, tmp_root,
        };

        let root = tmp_root("metrics-auth");
        // 台账里种一个在跑的实例: 有真数据, "无 token 不泄漏"的断言才成立。
        seed_online_daemon(&root).await;
        let db = ricow_strategy::Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&db).await;
        let port = serve_test(root, db).await;

        let unauth = get_raw(port, "/metrics").await;
        assert!(unauth.contains(" 401 "), "无 token 必须 401: {unauth}");
        assert!(body_of(&unauth).trim().is_empty(), "401 响应体必须为空(不解释原因)");
        assert!(!unauth.contains("ricow_daemon_up"), "无 token 不得泄漏任何指标: {unauth}");

        let ok = get_raw(port, "/metrics?token=tok-ok").await;
        assert!(ok.contains(" 200 "), "带对 token 应 200: {ok}");
        let body = body_of(&ok);
        assert!(body.contains("ricow_daemon_up"), "{body}");
        assert!(body.contains("ricow_fills_total"), "{body}");
    }
}
