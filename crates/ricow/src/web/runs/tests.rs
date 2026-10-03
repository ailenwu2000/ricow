//! `runs` 的端点/纯函数测试(036 拆文件下沉): 逻辑在 `super`, 这里只组织用例。

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

    let mut stream =
        tokio::net::TcpStream::connect((super::super::BIND_ADDR, port)).await.expect("连上服务");
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
        ("POST", "/api/strategies/grid-1/start?token=wrong", Some(r#"{"mode":"dry_run"}"#.into())),
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
    let res =
        raw(port, "POST", "/api/strategies/grid-1/start?token=tok-ok", Some(r#"{"mode":"real"}"#))
            .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains(r#""code":"invalid_mode""#), "{res}");

    // ② live 缺短语 → 400, 回显期望串, 零副作用(不写 run/daemon.json)。
    let res =
        raw(port, "POST", "/api/strategies/grid-1/start?token=tok-ok", Some(r#"{"mode":"live"}"#))
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
    let res =
        raw(port, "POST", "/api/strategies/grid-1/start?token=tok-ok", Some(r#"{"mode":"demo"}"#))
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
    let res = raw(port, "POST", "/api/strategies/grid-1/stop?token=tok-ok", Some("{oops")).await;
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
        raw(port, "POST", "/api/risk-ack?token=tok-ok", Some(r#"{"phrase":"确认 风险"}"#)).await;
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
