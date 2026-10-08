//! Web 层的**共享测试脚手架**(036 拆文件时下沉): 裸 HTTP 客户端 + 临时目录 + 服务启动。
//!
//! 本 crate 没有 HTTP 客户端依赖, 各端点用例一律**直接写回环套接字** —— 中间件、路由、
//! 响应体一体验证, 不外引测试依赖。放在这里是为了让各子模块的测试不再各抄一份。
//!
//! 只在 `cfg(test)` 下编译(见 `mod.rs` 的声明)。

use std::path::{Path, PathBuf};

use ricow_strategy::Database;

use super::store::SessionStore;
use super::{bind, serve, Starter, WebSink, WebState, BIND_ADDR};

/// 取 HTTP 响应体(首个空行之后); 供鉴权用例断言「响应体为空 / 不含会话内容」。
pub(super) fn body_of(res: &str) -> &str {
    res.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// 裸 HTTP/1.1 请求(本 crate 没有 HTTP 客户端依赖, 直接写回环套接字)。
pub(super) async fn raw_request(port: u16, req: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
    stream.write_all(req.as_bytes()).await.expect("发出请求");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("读回响应");
    String::from_utf8_lossy(&buf).to_string()
}

/// 裸 HTTP/1.1 GET。
pub(super) async fn get_raw(port: u16, path: &str) -> String {
    raw_request(
        port,
        format!("GET {path} HTTP/1.1\r\nHost: {BIND_ADDR}\r\nConnection: close\r\n\r\n"),
    )
    .await
}

/// 裸 HTTP/1.1 POST(JSON 体); `token` 拼进查询串。
pub(super) async fn post_raw(port: u16, path: &str, body: &str) -> String {
    raw_request(
        port,
        format!(
            "POST {path} HTTP/1.1\r\nHost: {BIND_ADDR}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

/// 起一个真实监听的服务(中间件 + 路由 + 响应体一体验证), 返回系统分配的端口。
///
/// 顺手断言监听地址仍是 `127.0.0.1`: FR-011 要求新增端点**不**改变这张网的面 ——
/// 依旧只在回环上服务。token 固定 `tok-ok`(与各用例的请求一致)。
pub(super) async fn serve_test(root: PathBuf, db: Database) -> u16 {
    let store = SessionStore::new(db.clone());
    let starter: Starter = std::sync::Arc::new(|_id: &str, _sink: &mut WebSink| Ok(()));
    let state = WebState::new("tok-ok".to_string(), root, db, store, starter);
    let (listener, port) = bind(0).await.expect("绑定回环端口");
    let addr = listener.local_addr().expect("读取监听地址");
    assert_eq!(addr.ip(), std::net::IpAddr::V4(BIND_ADDR), "只应绑回环, 实际 {addr}");
    tokio::spawn(serve(listener, state));
    port
}

/// 干净的临时数据目录(`daemon.json` 的有无由用例掌握); 同名目录先清空。
pub(super) fn tmp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ricow-web-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时数据目录");
    dir
}

/// 往库里种成交 / 订单 / 持仓 / PnL 各一行 —— 让「401 不泄漏交易数据」有真数据可泄漏。
pub(super) async fn seed_trade_rows(db: &Database) {
    use ricow_core::{OrderFill, OrderSide};

    const TS: i64 = 1_700_000_000_000;
    let at = chrono::DateTime::from_timestamp_millis(TS).expect("时间戳");
    let dec = |s: &str| s.parse::<rust_decimal::Decimal>().expect("Decimal");

    db.insert_fill(
        "s1",
        &OrderFill {
            trade_id: None,
            exchange_order_id: "EX-SECRET".into(),
            client_order_id: "C-1".into(),
            pair: "BTCUSDT".into(),
            side: OrderSide::Buy,
            fill_price: dec("100"),
            fill_size: dec("2"),
            fee: dec("0.1"),
            timestamp: at,
            position_side: None,
        },
    )
    .await
    .expect("落成交");
    db.upsert_order(&ricow_strategy::OrderRecord {
        strategy_id: "s1".into(),
        exchange_order_id: "EX-SECRET".into(),
        client_order_id: "C-1".into(),
        pair: "BTCUSDT".into(),
        side: "buy".into(),
        price: dec("100"),
        size: dec("2"),
        filled_size: dec("2"),
        status: "filled".into(),
        mode: "demo".into(),
        created_at: TS,
        updated_at: TS,
    })
    .await
    .expect("落订单");
    db.upsert_position(&ricow_strategy::PositionRecord {
        strategy_id: "s1".into(),
        pair: "BTCUSDT".into(),
        size: dec("2"),
        entry_price: dec("100"),
        mode: "demo".into(),
        updated_at: TS,
    })
    .await
    .expect("落持仓");
    db.insert_pnl_snapshot(&ricow_strategy::PnlSnapshotRecord {
        strategy_id: "s1".into(),
        timestamp: TS,
        realized_pnl: dec("5"),
        fees: dec("0.1"),
        net_pnl: dec("4.9"),
        trade_count: 1,
    })
    .await
    .expect("落 PnL 快照");
}

/// 造一个「daemon 在线」的最小替身: 绑本机端口 + 落 `run/daemon.json`。
///
/// 探活口径 = `read_daemon_info` + TCP connect(唯一口径, D10), 握手成功即算在跑 ——
/// 所以只需保住监听器(连上即丢), 不必应答任何协议。
pub(super) async fn seed_online_daemon(root: &Path) {
    use crate::supervisor::ledger;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑本机端口");
    let port = listener.local_addr().expect("读端口").port();
    ledger::ensure_dirs(root).expect("建 run/logs 目录");
    ledger::write_daemon_info(
        root,
        &ledger::DaemonInfo {
            pid: std::process::id(),
            port,
            token: "tok-daemon".into(),
            started_at: ledger::now_str(),
        },
    )
    .expect("写 run/daemon.json");
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            drop(sock);
        }
    });
}

/// 写一份策略日志(模拟 `supervisor::procs` 写的 `logs/<name>.log`)。**不启任何进程** ——
/// FR-017 要的正是"策略没在跑也看得到历史日志"。
pub(super) fn write_log(root: &Path, name: &str, lines: &[String]) {
    let dir = crate::supervisor::ledger::logs_dir(root);
    std::fs::create_dir_all(&dir).expect("建 logs 目录");
    std::fs::write(dir.join(format!("{name}.log")), lines.join("\n") + "\n").expect("写日志");
}
