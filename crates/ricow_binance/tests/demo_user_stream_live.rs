//! Demo 现货用户数据流真实调用集成测试 (#[ignore, 需 env key) —— 011 T013 第一验证目标。
//!
//! 验证链路: `subscribe_user_data()` (现货 WebSocket API `userDataStream.subscribe.signature`)
//! → 真实下一笔小额市价单 → 收到成交通知并成功转为 `OrderFill` → 按账户余额对齐回卖清理。
//!
//! 为什么单独验它: 011 的成交回写 (plan D4) 完全依赖该数据流, 而此前只验证过 **TCP 连通**。
//! 2026-09-13 实测确认: legacy listenKey (`POST /api/v3/userDataStream`) 已被币安永久下线
//! (主网/demo 均 410 Gone), 现货用户流只能走 WS-API 订阅 —— 本测试即该路径的真实调用证据。
//!
//! 纪律 (specs/constitution.md §三): 真实调用, 禁 mock、禁假 token、禁主网。
//! 运行前必须按 specs/testnet.md 对齐现货 demo 时钟 (本机超前 >1000ms 会被币安硬拒), 现货单独跑:
//!
//! ```bash
//! export RICOW_BN_API_KEY=<demo api key>
//! export RICOW_BN_SECRET_KEY=<demo secret>
//! export RICOW_BN_BASE_URL=https://demo-api.binance.com
//! cargo test -p ricow_binance --test demo_user_stream_live -- --ignored --nocapture
//! ```

use std::str::FromStr;
use std::time::Duration;

use futures::StreamExt;
use ricow_binance::BinanceClient;
use ricow_core::{OrderSide, UserEvent};
use rust_decimal::Decimal;

fn demo_client() -> BinanceClient {
    let api_key = std::env::var("RICOW_BN_API_KEY")
        .expect("RICOW_BN_API_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    let secret = std::env::var("RICOW_BN_SECRET_KEY")
        .expect("RICOW_BN_SECRET_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    BinanceClient::new().expect("BinanceClient 构造失败").with_credentials(api_key, secret)
}

/// 按目标名义价值算合法数量 (对齐 step, 且 ≥ min_qty)。
fn qty_for_notional(
    price: Decimal,
    notional: Decimal,
    min_qty: Decimal,
    step: Option<Decimal>,
) -> Decimal {
    let raw = (notional / price).max(min_qty);
    match step {
        Some(s) if !s.is_zero() => ((raw / s).floor() * s).max(min_qty),
        _ => raw,
    }
}

#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_spot_user_stream_delivers_fill() {
    let c = demo_client();
    let symbol = "ETHUSDT";

    let markets = c.get_exchange_info().await.expect("现货 exchangeInfo 失败");
    let m = markets.iter().find(|m| m.symbol == symbol).expect("现货应有 ETHUSDT");
    // T001 数据面真实核对: 现货最小名义过滤器已解析
    println!(
        "== {symbol} min_qty={} step={:?} min_notional={:?}",
        m.min_size, m.step_size, m.min_notional
    );
    assert!(
        m.min_notional.is_some(),
        "现货 MIN_NOTIONAL/NOTIONAL 过滤器未解析 (T001 不达标): {m:?}"
    );

    let ask = c.get_depth(symbol, 1).await.expect("depth 失败").best_ask().expect("无卖一价").price;
    let notional = Decimal::from(10).max(m.min_notional.unwrap_or(Decimal::from(5)));
    let qty = qty_for_notional(ask, notional, m.min_size, m.step_size);
    println!("== 计划市价买入 {qty} {symbol} (ask≈{ask}, 目标名义≈{notional})");

    // 先订阅用户数据流 (listenKey 在 demo 侧签发 → WS 必须走 demo-stream)
    let mut stream = c.subscribe_user_data().await.expect("订阅用户数据流失败");
    println!("用户数据流已订阅");

    // 预热 (WS 建连 + 首帧), 再下单
    tokio::time::sleep(Duration::from_secs(3)).await;

    let cid = format!("ricow-u-{}", chrono::Utc::now().timestamp_millis());
    let ack = c
        .place_order(symbol, "BUY", "MARKET", &qty.to_string(), None, &cid)
        .await
        .expect("市价买入失败");
    println!("市价买入 OK orderId={} cid={cid}", ack["orderId"]);

    // 等成交事件 (≤25s; 期间打印订单事件, 便于区分"事件到了但映射错"与"根本没事件")
    let mut got_fill = None;
    let mut order_events = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(UserEvent::Fill(f))) => {
                println!(
                    "收到成交事件: cid={} pair={} side={:?} price={} size={} fee={} trade_id={:?} ts={}",
                    f.client_order_id, f.pair, f.side, f.fill_price, f.fill_size, f.fee, f.trade_id, f.timestamp
                );
                if f.client_order_id == cid {
                    got_fill = Some(f);
                    break;
                }
                println!("  (非本次订单, 继续等)");
            }
            Ok(Some(UserEvent::Order(u))) => {
                order_events += 1;
                println!(
                    "收到订单事件: cid={} pair={} status={:?} filled={} remaining={}",
                    u.client_order_id, u.pair, u.status, u.filled_size, u.remaining_size
                );
            }
            Ok(None) => {
                println!("用户数据流已关闭");
                break;
            }
            Err(_) => continue, // 本轮 5s 无事件, 继续等
        }
    }

    let fill = got_fill.unwrap_or_else(|| {
        panic!(
            "25s 内未收到本次订单的成交事件 (订单事件 {order_events} 条) —— 用户数据流不可用, 触发 plan R1 回退讨论"
        )
    });
    assert_eq!(fill.pair, symbol);
    assert_eq!(fill.side, OrderSide::Buy);
    assert!(fill.fill_price > Decimal::ZERO, "成交价应为正: {fill:?}");
    assert!(fill.fill_size > Decimal::ZERO, "成交量应为正: {fill:?}");
    assert!(fill.fill_size <= qty, "成交量不应超过下单量: {} > {qty}", fill.fill_size);

    // 清理: 按账户实际可用余额对齐卖出 (手续费按 base 扣, 直接卖成交额会超余额 → -2010)
    let free = c.get_account().await.expect("account 查询失败")["balances"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|b| b["asset"].as_str() == Some(m.base_asset.as_str()))
                .and_then(|b| b["free"].as_str())
                .and_then(|s| Decimal::from_str(s).ok())
        })
        .unwrap_or_default();
    let aligned = match m.step_size {
        Some(step) if !step.is_zero() => (free / step).floor() * step,
        _ => free,
    };
    let base_asset = m.base_asset.clone();
    println!("== 清理: {base_asset} 可用 {free} → 对齐后卖出 {aligned} (剩余为手续费尘埃)");
    if aligned >= m.min_size {
        let close_cid = format!("ricow-uc-{}", chrono::Utc::now().timestamp_millis());
        let sell = c
            .place_order(symbol, "SELL", "MARKET", &aligned.to_string(), None, &close_cid)
            .await
            .expect("清理平仓失败");
        println!("== 清理平仓 OK orderId={} cid={close_cid} qty={aligned}", sell["orderId"]);
    } else {
        println!("== 余额低于 min_qty, 未下平仓单 (遗留 {} )", free);
    }
    println!("PASS: demo 现货用户数据流可用, 成交可转 OrderFill (T013)");
}

/// 反例护栏: 用户流事件只认本实例订单 (clientOrderId 归属), 非本实例成交须可辨识。
/// 同一数据流下再下一笔**不同 cid** 的单, 断言收到的成交 cid 与之一致 (不会张冠李戴)。
#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_spot_user_stream_events_carry_client_order_id() {
    let c = demo_client();
    let symbol = "ETHUSDT";
    let markets = c.get_exchange_info().await.expect("exchangeInfo 失败");
    let m = markets.iter().find(|m| m.symbol == symbol).expect("ETHUSDT");

    let ask = c.get_depth(symbol, 1).await.expect("depth").best_ask().expect("ask").price;
    let qty = qty_for_notional(
        ask,
        Decimal::from(10).max(m.min_notional.unwrap_or(Decimal::from(5))),
        m.min_size,
        m.step_size,
    );

    let mut stream = c.subscribe_user_data().await.expect("订阅失败");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let cid = format!("ricow-cid-{}", chrono::Utc::now().timestamp_millis());
    c.place_order(symbol, "BUY", "MARKET", &qty.to_string(), None, &cid).await.expect("下单失败");

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let mut filled = Decimal::ZERO;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(UserEvent::Fill(f))) => {
                seen.push(f.client_order_id.clone());
                if f.client_order_id == cid {
                    filled += f.fill_size;
                    // 成交可能分笔, 继续收一会儿
                    if filled >= qty {
                        break;
                    }
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(!seen.is_empty(), "未收到任何成交事件");
    assert!(seen.iter().any(|c| c == &cid), "收到的成交 cid 未包含本次订单: {seen:?}");
    println!("收到的成交 cid: {seen:?} (本次 cid={cid}), 累计成交 {filled}");

    if filled > Decimal::ZERO {
        let close_cid = format!("ricow-cc-{}", chrono::Utc::now().timestamp_millis());
        let sell = c
            .place_order(symbol, "SELL", "MARKET", &filled.to_string(), None, &close_cid)
            .await
            .expect("清理平仓失败");
        println!("清理平仓 OK orderId={}", sell["orderId"]);
    }
}
