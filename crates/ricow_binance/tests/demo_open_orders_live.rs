//! Demo 挂单查询闭环真实调用集成测试 (#[ignore, 需 env key)。
//!
//! 覆盖 008 T001(`Exchange::get_open_orders` / `FuturesClient::get_open_orders`):
//! 现货 `GET /api/v3/openOrders` 与合约 `GET /fapi/v1/openOrders` 的真实调用、解析、与撤单闭环。
//!
//! 纪律 (specs/constitution.md §三): 交易流程 testnet 真实调用, 禁 mock 替身、禁假 token、禁主网。
//! 用例只挂**远离市价的限价单**(不会成交), 并在结束前撤掉自己产生的挂单(`ricow-` 前缀)。
//!
//! 运行:
//! ```bash
//! export RICOW_BN_API_KEY=<demo api key>      # 见 specs/testnet.md
//! export RICOW_BN_SECRET_KEY=<demo secret>
//! export RICOW_BN_BASE_URL=https://demo-api.binance.com
//! export RICOW_FAPI_BASE_URL=https://demo-fapi.binance.com
//! cargo test -p ricow_binance --test demo_open_orders_live -- --ignored --nocapture
//! ```
//!
//! 2026-09-12 建 (specs/changes/008-platform-process-model 验收 A3)。

use ricow_binance::{BnSpotExchange, BinanceClient, FuturesClient};
use ricow_core::{Exchange, OrderRequest, OrderSide, OrderStatus};
use rust_decimal::Decimal;
use std::str::FromStr;

fn demo_spot_client() -> BinanceClient {
    let api_key = std::env::var("RICOW_BN_API_KEY")
        .expect("RICOW_BN_API_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    let secret = std::env::var("RICOW_BN_SECRET_KEY")
        .expect("RICOW_BN_SECRET_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    BinanceClient::new()
        .expect("BinanceClient 构造失败")
        .with_credentials(api_key, secret)
}

fn demo_futures_client() -> FuturesClient {
    let api_key = std::env::var("RICOW_BN_API_KEY")
        .expect("RICOW_BN_API_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    let secret = std::env::var("RICOW_BN_SECRET_KEY")
        .expect("RICOW_BN_SECRET_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    FuturesClient::with_credentials(api_key, secret, None).expect("FuturesClient 构造失败")
}

/// 按 PRICE_FILTER.tickSize 向下对齐价格 (交易所硬约束: 价格须为 tick 整数倍)。
fn price_on_tick(price: Decimal, tick: Decimal) -> Decimal {
    if tick.is_zero() {
        return price;
    }
    (price / tick).floor() * tick
}

/// 按目标名义价值算合法数量 (对齐 step_size, 且 ≥ min_qty)。
fn qty_for_notional(
    price: Decimal,
    notional: Decimal,
    min_qty: Decimal,
    step: Option<Decimal>,
) -> Decimal {
    let raw = (notional / price).max(min_qty);
    let aligned = match step {
        Some(s) if !s.is_zero() => ((raw / s).floor() * s).max(min_qty),
        _ => raw,
    };
    aligned
}

/// 现货: 挂远离市价的限价买单 → `get_open_orders` 必须能看到 → 撤单 → 必须看不到了。
#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_spot_open_orders_roundtrip() {
    let exchange = BnSpotExchange::new(demo_spot_client());
    let symbol = "BTCUSDT";

    let markets = exchange.get_markets().await.expect("get_markets 失败");
    let m = markets.iter().find(|m| m.symbol == symbol).expect("demo 应有 BTCUSDT");
    let px = exchange
        .get_klines(symbol, "1h", 1)
        .await
        .expect("get_klines 失败")
        .last()
        .expect("无 K 线")
        .close;

    // 先清理本测试历史遗留的 ricow- 挂单 (demo 账户共享, 保持干净)
    for o in exchange.get_open_orders(symbol).await.expect("openOrders(清理) 失败") {
        if o.client_order_id.starts_with("ricow-") {
            exchange.cancel_order(symbol, &o.client_order_id).await.expect("清理撤单失败");
            println!("  清理遗留挂单 {}", o.client_order_id);
        }
    }

    let qty = qty_for_notional(px, Decimal::from(20), m.min_size, m.step_size);
    // 买单价压到市价的 90% → 不会立即成交, 稳定处于 Open (价格须落在 tick 上)
    let limit_price = price_on_tick(px * Decimal::from_str("0.90").unwrap(), m.tick_size);
    let ack = exchange
        .place_order(OrderRequest::new_limit(symbol, OrderSide::Buy, limit_price, qty))
        .await
        .expect("限价挂单失败");
    assert_eq!(ack.status, OrderStatus::Open, "挂单应为 Open: {ack:?}");
    println!("== 现货 {symbol}: 挂单 OK cid={} qty={qty} price={limit_price}", ack.client_order_id);

    // ① get_open_orders 真实调用返回该挂单, 字段可解析且正确
    let orders = exchange.get_open_orders(symbol).await.expect("get_open_orders 失败");
    println!("  查询到 {} 笔挂单", orders.len());
    let mine = orders
        .iter()
        .find(|o| o.client_order_id == ack.client_order_id)
        .unwrap_or_else(|| panic!("get_open_orders 未返回刚挂的单: {:?}", orders));
    assert_eq!(mine.pair, symbol);
    assert_eq!(mine.side, OrderSide::Buy);
    assert_eq!(mine.status, OrderStatus::Open);
    assert_eq!(mine.size, qty, "解析数量应与提交一致");
    assert_eq!(mine.price, limit_price, "解析价格应与提交一致");
    assert_eq!(mine.filled_size, Decimal::ZERO, "未成交单 filled 应为 0");
    assert!(!mine.exchange_order_id.is_empty(), "交易所订单号应存在");
    println!("  字段校验通过: side={:?} price={} size={} status={:?}", mine.side, mine.price, mine.size, mine.status);

    // ② 撤单后不再出现在挂单列表
    exchange.cancel_order(symbol, &ack.client_order_id).await.expect("撤单失败");
    let after = exchange.get_open_orders(symbol).await.expect("get_open_orders(撤后) 失败");
    assert!(
        !after.iter().any(|o| o.client_order_id == ack.client_order_id),
        "撤单后仍出现在挂单列表: {:?}",
        after
    );
    println!("  撤单后挂单列表已不含该单 (剩余 {} 笔) —— 现货闭环 PASS", after.len());
}

/// 合约: 同样闭环 (fapi openOrders), one-way 模式 + 1x 杠杆。
#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_futures_open_orders_roundtrip() {
    let c = demo_futures_client();
    let symbol = "BTCUSDT";

    // one-way 模式 (positionSide 语义依赖账户模式); 幂等错误容忍
    if let Err(e) = c.set_position_side_dual(false).await {
        assert!(
            e.to_string().contains("No need to change position side"),
            "set_position_side_dual 意外失败: {e}"
        );
    }
    c.set_leverage(symbol, 1).await.expect("set_leverage 失败");

    let mark = Decimal::from_str(
        c.get_premium_index(symbol).await.expect("premiumIndex 失败")["markPrice"]
            .as_str()
            .unwrap_or("0"),
    )
    .unwrap_or_default();
    assert!(mark > Decimal::ZERO, "markPrice 解析异常: {mark}");

    let markets = c.get_exchange_info().await.expect("fapi exchangeInfo 失败");
    let m = markets.iter().find(|m| m.symbol == symbol).expect("fapi 应有 BTCUSDT");
    // 合约 MIN_NOTIONAL ≈ 50 USDT → 名义 200 USDT 留足余量
    let qty = qty_for_notional(mark, Decimal::from(200), m.min_size, m.step_size);
    let limit_price = price_on_tick(mark * Decimal::from_str("0.90").unwrap(), m.tick_size);

    for o in c.get_open_orders(symbol).await.expect("openOrders(清理) 失败") {
        if o.client_order_id.starts_with("ricow-") {
            c.cancel_order(symbol, &o.client_order_id).await.expect("清理撤单失败");
            println!("  清理遗留挂单 {}", o.client_order_id);
        }
    }

    // F1 (011): 显式传入订单号 —— 修复前合约客户端硬编码丢弃调用方 id; 响应须原样回传。
    let cid_in = format!(
        "ricow-f-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
    );
    let resp = c
        .place_order(
            symbol,
            "BUY",
            "LIMIT",
            &qty.to_string(),
            Some(&limit_price.to_string()),
            None,
            false,
            Some(&cid_in),
        )
        .await
        .expect("合约限价挂单失败");
    let cid = resp["clientOrderId"].as_str().unwrap_or_default().to_string();
    assert_eq!(cid, cid_in, "传入 clientOrderId 须原样回传 (F1 修复): {resp}");
    println!("== 合约 {symbol}: 挂单 OK cid={cid} qty={qty} price={limit_price} mark={mark}");

    let orders = c.get_open_orders(symbol).await.expect("fapi get_open_orders 失败");
    println!("  查询到 {} 笔挂单", orders.len());
    let mine = orders
        .iter()
        .find(|o| o.client_order_id == cid)
        .unwrap_or_else(|| panic!("fapi get_open_orders 未返回刚挂的单: {:?}", orders));
    assert_eq!(mine.pair, symbol);
    assert_eq!(mine.side, OrderSide::Buy);
    assert_eq!(mine.status, OrderStatus::Open);
    assert_eq!(mine.size, qty, "解析数量应与提交一致");
    assert_eq!(mine.price, limit_price, "解析价格应与提交一致");
    println!("  字段校验通过: side={:?} price={} size={}", mine.side, mine.price, mine.size);

    c.cancel_order(symbol, &cid).await.expect("合约撤单失败");
    let after = c.get_open_orders(symbol).await.expect("fapi get_open_orders(撤后) 失败");
    assert!(
        !after.iter().any(|o| o.client_order_id == cid),
        "撤单后仍出现在挂单列表: {:?}",
        after
    );
    println!("  撤单后挂单列表已不含该单 (剩余 {} 笔) —— 合约闭环 PASS", after.len());
}

/// 合约: 必然被交易所拒的单必须把错误**如实抛出**, 不得静默吞掉 (008 T004 真实调用面)。
///
/// 判据: 返回 Err 且携带币安原始拒绝原因(非空/非超时); 请求未在交易所留下挂单残留。
#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_futures_rejected_order_surfaces_error() {
    let c = demo_futures_client();
    let symbol = "BTCUSDT";

    if let Err(e) = c.set_position_side_dual(false).await {
        assert!(
            e.to_string().contains("No need to change position side"),
            "set_position_side_dual 意外失败: {e}"
        );
    }
    c.set_leverage(symbol, 1).await.expect("set_leverage 失败");

    let mark = Decimal::from_str(
        c.get_premium_index(symbol).await.expect("premiumIndex 失败")["markPrice"]
            .as_str()
            .unwrap_or("0"),
    )
    .unwrap_or_default();
    let markets = c.get_exchange_info().await.expect("fapi exchangeInfo 失败");
    let m = markets.iter().find(|m| m.symbol == symbol).expect("fapi 应有 BTCUSDT");
    let qty = qty_for_notional(mark, Decimal::from(200), m.min_size, m.step_size);

    // 故意违反对齐: 合法 tick 价格 + 0.001 个最小单位 → PRICE_FILTER 必然拒绝
    let bad_price =
        price_on_tick(mark * Decimal::from_str("0.90").unwrap(), m.tick_size)
            + Decimal::from_str("0.001").unwrap();

    let err = c
        .place_order(
            symbol,
            "BUY",
            "LIMIT",
            &qty.to_string(),
            Some(&bad_price.to_string()),
            None,
            false,
            None,
        )
        .await
        .expect_err("价格违反对齐的单竟被接受");
    let msg = err.to_string();
    println!("== 合约拒单已如实抛出: {msg}");
    // 判据: 必须是交易所明确拒绝 (BN 4xx + 原始原因), 而非超时/本地错误被吞
    assert!(msg.contains("BN 400"), "错误未携带交易所 HTTP 拒绝: {msg}");
    assert!(
        msg.contains("tick") || msg.contains("Price") || msg.contains("Precision"),
        "错误未携带交易所原始原因: {msg}"
    );
    assert!(!msg.contains("timeout"), "错误是超时而非交易所拒绝: {msg}");

    let after = c.get_open_orders(symbol).await.expect("fapi get_open_orders(拒单后) 失败");
    assert!(
        after.iter().all(|o| o.price != bad_price),
        "被拒的单竟出现在挂单列表: {:?}",
        after
    );
    println!("  交易所正常拒绝且无残单 (当前挂单 {} 笔) —— 错误上报 PASS", after.len());
}
