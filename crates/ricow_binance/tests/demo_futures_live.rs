//! Demo USDT-M 合约真实交易集成测试 (#[ignore, 需 env key)。
//!
//! 纪律 (specs/constitution.md §三): 交易流程 testnet 真实调用, 禁 mock。无 env key 时不跑
//! (cargo test 默认跳过 #[ignore]); 需显式 `-- --ignored`。
//!
//! 运行:
//! ```bash
//! export RICOW_BN_API_KEY=<demo api key>
//! export RICOW_BN_SECRET_KEY=<demo secret>
//! export RICOW_FAPI_BASE_URL=https://demo-fapi.binance.com
//! cargo test -p ricow_binance --test demo_futures_live -- --ignored --nocapture
//! ```
//!
//! 覆盖: T5 one-way 多交易对 开多→查仓→平多→限价开空→撤单→资金费率;
//! T6 双向持仓 dual (同对 LONG+SHORT 并存); T7 真实强平观察 (高杠杆小额, 30 分钟窗口)。
//!
//! 2026-09-04 建 (specs/plans/2026-09-04_213220-bn-demo-live-test checklist T5/T6/T7)。

use ricow_binance::{
    parse_available_balance, position_amount, position_liquidation_price, FuturesClient,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// 从 env 装配 demo 合约客户端 (base_url 由 RICOW_FAPI_BASE_URL 注入)。
fn demo_client() -> FuturesClient {
    let api_key = std::env::var("RICOW_BN_API_KEY")
        .expect("RICOW_BN_API_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    let secret = std::env::var("RICOW_BN_SECRET_KEY")
        .expect("RICOW_BN_SECRET_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    FuturesClient::with_credentials(api_key, secret, None).expect("FuturesClient 构造失败")
}

/// 容错 marginType 幂等错误 ("No need to change margin type" = 已是目标类型, 非错误)。
async fn set_margin_type_ok(c: &FuturesClient, symbol: &str, is_isolated: bool) {
    match c.set_margin_type(symbol, is_isolated).await {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("No need to change margin type"),
                "set_margin_type 意外失败: {msg}"
            );
        }
    }
}

/// 容错 positionSide/dual 幂等错误 ("No need to change position side" = 已是目标模式)。
async fn set_dual_ok(c: &FuturesClient, dual: bool) {
    match c.set_position_side_dual(dual).await {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("No need to change position side"),
                "set_position_side_dual 意外失败: {msg}"
            );
        }
    }
}

/// 清理所有残留持仓 (幂等测试前置): 遍历 account.positions, 非零仓平掉。
/// one-way 模式 (BOTH) 平仓必须 reduce_only=true (防反手开新仓);
/// hedge 模式 (LONG/SHORT) 方向单天然平仓, 带 reduceOnly 会被 fapi 拒绝。
async fn close_all_positions(c: &FuturesClient) {
    let dual = c.position_side_dual().await.unwrap_or(false);
    let acct = c.get_account().await.expect("get_account 失败");
    if let Some(positions) = acct["positions"].as_array() {
        for p in positions.iter() {
            let sym = p["symbol"].as_str().unwrap_or("");
            let amt_str = p["positionAmt"].as_str().unwrap_or("0");
            let amt: f64 = amt_str.parse().unwrap_or(0.0);
            if sym.is_empty() || amt.abs() < 1e-12 {
                continue;
            }
            let side = if amt > 0.0 { "SELL" } else { "BUY" };
            let size = amt.abs().to_string();
            let ps = p["positionSide"].as_str().unwrap_or("BOTH").to_string();
            let reduce = if dual {
                false // hedge 方向单已隐含平仓
            } else {
                true // one-way 必须 reduce_only 防反手
            };
            let ps_opt = if ps == "BOTH" { None } else { Some(ps.as_str()) };
            println!(
                "  清理残留仓: {sym} amt={amt_str} → {side} {size} ({ps}, reduce_only={reduce})"
            );
            let _ = c.place_order(sym, side, "MARKET", &size, None, ps_opt, reduce, None).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }
}

/// 固定 seed 伪随机 (LCG, 不引依赖): 从候选池选 n 个不同 symbol。
fn pick_symbols(pool: &[String], n: usize) -> Vec<&String> {
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut next = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    let mut chosen: Vec<&String> = Vec::new();
    while chosen.len() < n && chosen.len() < pool.len() {
        let s = &pool[next() % pool.len()];
        if !chosen.contains(&s) {
            chosen.push(s);
        }
    }
    chosen
}

/// 标记价格 (premiumIndex.markPrice)。
async fn mark_price(c: &FuturesClient, symbol: &str) -> Decimal {
    let v = c.get_premium_index(symbol).await.expect("premiumIndex 失败");
    v["markPrice"].as_str().and_then(|s| Decimal::from_str_exact(s).ok()).expect("markPrice 缺失")
}

/// 按目标名义价值算合法数量 (对齐 step, ≥ min_qty)。合约有 MIN_NOTIONAL (≈50-100 USDT)。
fn qty_for_notional(
    price: Decimal,
    notional: Decimal,
    min_qty: Decimal,
    step: Option<Decimal>,
) -> Decimal {
    let raw = (notional / price).max(min_qty);
    match step {
        Some(s) if !s.is_zero() => {
            let mult = (raw / s).floor();
            (mult * s).max(min_qty)
        }
        _ => raw,
    }
}

// ============ T5: one-way 多交易对 开多→平多→限价开空→撤单 ============

#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_futures_oneway_roundtrip_multiple_pairs() {
    let c = demo_client();

    // 幂等前置: 清理上次运行可能的残留持仓。
    close_all_positions(&c).await;

    // ① 账户基线。
    let account = c.get_account().await.expect("get_account 失败 (检查 key 权限)");
    let bal0 = parse_available_balance(&account).expect("availableBalance 缺失");
    println!("== 合约账户基线 availableBalance = {bal0} ==");

    // ② 候选池: PERPETUAL + quote=USDT。
    let markets = c.get_exchange_info().await.expect("exchangeInfo 失败");
    let pool: Vec<String> =
        markets.iter().filter(|m| m.quote_asset == "USDT").map(|m| m.symbol.clone()).collect();
    assert!(pool.len() >= 3, "合约候选池不足 3 对: {}", pool.len());

    // 固定 seed 选 3 对不同对。
    let symbols = pick_symbols(&pool, 3);
    println!("== 本次测试交易对 (seed 固定可复现) ==");
    for s in &symbols {
        println!("  {s}");
    }

    for symbol in &symbols {
        let m = markets.iter().find(|m| &m.symbol == *symbol).unwrap();
        let px = mark_price(&c, symbol).await;
        // 目标名义 ~100 USDT (合约 MIN_NOTIONAL 50-100, 留余量)。
        let qty = qty_for_notional(px, dec!(100), m.min_size, m.step_size);
        println!(
            "\n--- {symbol}  mark={px} qty={qty} (min_qty={}, step={:?}) ---",
            m.min_size, m.step_size
        );

        // 杠杆 1x + 逐仓。
        c.set_leverage(symbol, 1).await.expect("set_leverage 失败");
        set_margin_type_ok(&c, symbol, true).await;
        println!("  杠杆 1x + ISOLATED 设置 OK");

        // 市价开多。
        let open = c
            .place_order(symbol, "BUY", "MARKET", &qty.to_string(), None, None, false, None)
            .await
            .expect("市价开多失败");
        println!("  开多 OK orderId={}", open["orderId"]);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // 查仓: one-way 模式 positionSide=BOTH (实测, 非 LONG)。
        let pos = c.get_positions(symbol).await.expect("positionRisk 失败");
        let amt = position_amount(&pos, "BOTH");
        assert!(amt > Decimal::ZERO, "{symbol} 开多后应有 BOTH 持仓: {pos}");
        println!("  positionRisk BOTH amount={amt} (≈{qty})");

        // 市价平多 (reduce_only, 反手 SELL 会开空 → 必须 reduceOnly=true)。
        let close = c
            .place_order(symbol, "SELL", "MARKET", &amt.to_string(), None, None, true, None)
            .await
            .expect("市价平多失败");
        println!("  平多 OK orderId={}", close["orderId"]);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let pos2 = c.get_positions(symbol).await.expect("positionRisk 2 失败");
        let amt_after = position_amount(&pos2, "BOTH");
        assert!(
            amt_after.abs() < m.min_size,
            "{symbol} 平多后 BOTH 应归零 (< min_qty={}), 实际 {amt_after}",
            m.min_size
        );
        println!("  平多后 BOTH amount={amt_after} (归零 ✓)");
    }

    // ③ 随机挑 1 对限价开空挂单 → 撤单。
    let s1 = symbols[0];
    let px = mark_price(&c, s1).await;
    let m1 = markets.iter().find(|m| &m.symbol == s1).unwrap();
    let qty = qty_for_notional(px, dec!(100), m1.min_size, m1.step_size);
    let limit_price = (px * dec!(1.02)).round_dp(2); // 高于市价, 空单不立即成交
    let placed = c
        .place_order(
            s1,
            "SELL",
            "LIMIT",
            &qty.to_string(),
            Some(&limit_price.to_string()),
            None,
            false,
            None,
        )
        .await
        .expect("限价开空失败");
    let cid = placed["clientOrderId"].as_str().expect("clientOrderId 缺失").to_string();
    assert_eq!(placed["status"].as_str(), Some("NEW"), "限价空单应 Open: {placed}");
    println!("\n== {s1} 限价开空挂单 OK clientOrderId={cid} price={limit_price}");

    c.cancel_order(s1, &cid).await.expect("撤单失败");
    println!("  撤单 OK");

    // ④ 资金费率观察。
    let pi = c.get_premium_index(s1).await.expect("premiumIndex 失败");
    println!(
        "== {s1} funding: rate={} nextFundingTime={} ==",
        pi["lastFundingRate"].as_str().unwrap_or("?"),
        pi["nextFundingTime"].as_str().unwrap_or("?")
    );

    let account_end = c.get_account().await.expect("get_account end 失败");
    let bal_end = parse_available_balance(&account_end).expect("availableBalance 缺失");
    let delta = bal_end - bal0;
    println!("\n== 汇总: availableBalance {bal0} → {bal_end} (Δ={delta}, 含手续费/滑点) ==");
    println!("PASS: 合约 one-way 多交易对真实联调通过 (开多→查仓→平多→限价挂撤)");
}

// ============ T6: 双向持仓 dual — 同对 LONG+SHORT 并存 ============

#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_futures_dual_position_both_sides() {
    let c = demo_client();

    // 幂等前置: 先确保 one-way + 无残留仓, 再开双向。
    close_all_positions(&c).await;
    let _ = set_dual_ok(&c, false).await;
    close_all_positions(&c).await;

    // 切换双向模式 (需账户无持仓/无挂单; one-way 测试已平仓)。
    set_dual_ok(&c, true).await;
    println!("== 双向持仓模式 (dualSidePosition=true) 开启 OK ==");

    let markets = c.get_exchange_info().await.expect("exchangeInfo 失败");
    let pool: Vec<String> =
        markets.iter().filter(|m| m.quote_asset == "USDT").map(|m| m.symbol.clone()).collect();
    // 随机 2 对。
    let pairs = pick_symbols(&pool, 2);

    for symbol in &pairs {
        let m = markets.iter().find(|m| &m.symbol == *symbol).unwrap();
        let px = mark_price(&c, symbol).await;
        let qty = qty_for_notional(px, dec!(100), m.min_size, m.step_size);
        println!("\n--- {symbol} dual 双向: qty={qty} ---");

        c.set_leverage(symbol, 1).await.expect("set_leverage 失败");
        set_margin_type_ok(&c, symbol, true).await;

        // 同对同时开多 (LONG) + 开空 (SHORT)。
        let o1 = c
            .place_order(symbol, "BUY", "MARKET", &qty.to_string(), None, Some("LONG"), false, None)
            .await
            .expect("dual 开多失败");
        println!("  开多 (LONG) OK orderId={}", o1["orderId"]);
        let o2 = c
            .place_order(
                symbol,
                "SELL",
                "MARKET",
                &qty.to_string(),
                None,
                Some("SHORT"),
                false,
                None,
            )
            .await
            .expect("dual 开空失败");
        println!("  开空 (SHORT) OK orderId={}", o2["orderId"]);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let pos = c.get_positions(symbol).await.expect("positionRisk 失败");
        let long = position_amount(&pos, "LONG");
        let short = position_amount(&pos, "SHORT");
        println!("  positionRisk: LONG={long} SHORT={short}");
        assert!(long > Decimal::ZERO, "{symbol} 应有 LONG 持仓");
        assert!(short < Decimal::ZERO, "{symbol} 应有 SHORT 持仓 (amount 为负): {pos}");

        // 逐仓钱包语义观察: 双向各占独立逐仓保证金 → 可用余额变化打印 (对比 account)。
        let acct = c.get_account().await.expect("account 失败");
        println!(
            "  account availableBalance={}",
            parse_available_balance(&acct).unwrap_or_default()
        );

        // 平仓: hedge 模式方向单天然平对应仓, 不带 reduceOnly (带 positionSide 时 fapi 拒绝 reduceonly)。
        let cl1 = c
            .place_order(
                symbol,
                "SELL",
                "MARKET",
                &long.to_string(),
                None,
                Some("LONG"),
                false,
                None,
            )
            .await
            .expect("平 LONG 失败");
        println!("  平 LONG OK orderId={}", cl1["orderId"]);
        let cl2 = c
            .place_order(
                symbol,
                "BUY",
                "MARKET",
                &short.abs().to_string(),
                None,
                Some("SHORT"),
                false,
                None,
            )
            .await
            .expect("平 SHORT 失败");
        println!("  平 SHORT OK orderId={}", cl2["orderId"]);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let pos2 = c.get_positions(symbol).await.expect("positionRisk 2 失败");
        let long2 = position_amount(&pos2, "LONG");
        let short2 = position_amount(&pos2, "SHORT");
        assert!(long2.abs() < m.min_size, "LONG 应归零: {long2}");
        assert!(short2.abs() < m.min_size, "SHORT 应归零: {short2}");
        println!("  双向平仓后 LONG={long2} SHORT={short2} (归零 ✓)");
    }

    // 恢复 one-way (需无持仓)。
    set_dual_ok(&c, false).await;
    println!("\nPASS: 合约双向持仓 dual 真实联调通过 (同对 LONG+SHORT 并存 → 双向平仓归零)");
}

// ============ T7: 真实强平观察 (用户 D3 拍板) ============
//
// 触发策略 (2026-09-04 两轮单边 LONG 30 分钟未触发后改进):
// 单向 LONG 需行情朝不利方向走 ~0.6% (125x 强平价距现价), 依赖方向赌运气;
// 改为 **dual 双向对冲**: 同对同额 125x 同时 LONG+SHORT → 价格任意方向波动都会
// 触发其中一侧强平 (另一侧浮盈在隔离钱包, 不补贴亏损侧), 触发概率翻倍且不赌方向。
// 每侧保证金 ~10 USDT, 总测试金损耗上限 ~20 USDT + 手续费。
// 窗口 15 分钟 (双向对冲任一侧 0.55% 波动即触发; 首轮单边 30 分钟曾逼近 0.16% 未触发,
// 双面对冲不再赌方向, 15 分钟足够; 若仍未触发则记录并人工平仓)。

#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用; 强平观察最长 15 分钟"]
async fn demo_futures_liquidation_observation() {
    let c = demo_client();

    // 幂等前置: 清理残留仓并切到双向模式。
    let _ = set_dual_ok(&c, false).await;
    close_all_positions(&c).await;
    set_dual_ok(&c, true).await;
    println!("== T7: dual 双向对冲强平观察 ==");

    // 选 BTCUSDT (波动适中, 125x 杠杆上限最大 → 强平价最贴近现价)。
    let markets = c.get_exchange_info().await.expect("exchangeInfo 失败");
    let symbol = "BTCUSDT";
    let m = markets.iter().find(|m| m.symbol == symbol).expect("BTCUSDT 不在 demo 合约列表");
    let px = mark_price(&c, symbol).await;
    println!("== {symbol} mark={px} ==");

    let max_lev: u32 = 125;
    c.set_leverage(symbol, max_lev).await.expect("set_leverage(125) 失败");
    set_margin_type_ok(&c, symbol, true).await;
    let margin_usdt = dec!(10);
    let qty = qty_for_notional(px, margin_usdt * Decimal::from(max_lev), m.min_size, m.step_size);
    println!("  杠杆 {max_lev}x, 每侧保证金 ~{margin_usdt} USDT, qty={qty}");

    // 同时开多 (LONG) + 开空 (SHORT)。
    let o1 = c
        .place_order(symbol, "BUY", "MARKET", &qty.to_string(), None, Some("LONG"), false, None)
        .await
        .expect("dual 开多失败");
    println!("  开多 (LONG) OK orderId={}", o1["orderId"]);
    let o2 = c
        .place_order(symbol, "SELL", "MARKET", &qty.to_string(), None, Some("SHORT"), false, None)
        .await
        .expect("dual 开空失败");
    println!("  开空 (SHORT) OK orderId={}", o2["orderId"]);
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let pos0 = c.get_positions(symbol).await.expect("positionRisk 失败");
    println!(
        "  初始持仓: LONG={} (liq={:?}) SHORT={} (liq={:?})",
        position_amount(&pos0, "LONG"),
        position_liquidation_price(&pos0, "LONG"),
        position_amount(&pos0, "SHORT"),
        position_liquidation_price(&pos0, "SHORT"),
    );

    // 轮询: 任一侧持仓归零 = 该侧被强平 (或人工平仓前的正常结束)。
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(15 * 60);
    let mut liq_side: Option<String> = None;
    let mut last_px = px;
    while std::time::Instant::now() < deadline {
        let pos = c.get_positions(symbol).await.expect("positionRisk 失败");
        let long = position_amount(&pos, "LONG");
        let short = position_amount(&pos, "SHORT");
        let now_px = mark_price(&c, symbol).await;
        last_px = now_px;
        let lliq = position_liquidation_price(&pos, "LONG")
            .map(|d| d.to_string())
            .unwrap_or_else(|| "?".into());
        let sliq = position_liquidation_price(&pos, "SHORT")
            .map(|d| d.to_string())
            .unwrap_or_else(|| "?".into());
        println!(
            "  [t={:.0}s] mark={now_px} LONG={long}(liq={lliq}) SHORT={short}(liq={sliq})",
            start.elapsed().as_secs_f64()
        );

        if long == Decimal::ZERO && short == Decimal::ZERO {
            // 两侧同时归零 (理论不可能同时强平, 可能为双向平仓兜底前); 视为异常, 继续看。
            println!("  两侧都归零, 检查是否测试自身平仓逻辑触发");
        } else if long == Decimal::ZERO {
            liq_side = Some("LONG".into());
            break;
        } else if short == Decimal::ZERO {
            liq_side = Some("SHORT".into());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }

    // 记录结果。
    let acct = c.get_account().await.expect("account 失败");
    let bal_after = parse_available_balance(&acct).unwrap_or_default();

    if let Some(side) = liq_side {
        println!("\n== 强平触发! {side} 侧持仓归零, 触发前 mark={last_px} ==");
        println!("== 触发后 availableBalance={bal_after} ==");
        println!("结论: 真实强平发生 (双向对冲: 价格朝 {side} 反方向波动触发), 对照回测 liq 判定, 记录入 specs/backtest.md §十一");
    } else {
        // 超时: 人工平掉双侧。
        println!("\n== 10 分钟未触发 (双向对冲仍无 0.6% 波动) ==");
        close_all_positions(&c).await;
        println!("  已人工平仓, 触发后 availableBalance={bal_after}");
        println!(
            "结论: 双向对冲 10 分钟仍未触发 — 记录理论强平价 vs 现价差距, 供回测对照 (不视为失败)"
        );
    }

    // 收尾: 恢复 one-way + 清仓。
    close_all_positions(&c).await;
    let _ = set_dual_ok(&c, false).await;
}
