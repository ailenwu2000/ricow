//! Demo 现货真实下单闭环集成测试 (#[ignore, 需 env key)。
//!
//! 纪律 (specs/constitution.md §三): 交易流程 testnet 真实调用, 禁 mock。无 env key 时本文件
//! 不跑 (cargo test 默认跳过 #[ignore]); 需显式 `-- --ignored`。
//!
//! 运行:
//! ```bash
//! export RICOW_BN_API_KEY=<demo api key>
//! export RICOW_BN_SECRET_KEY=<demo secret>
//! export RICOW_BN_BASE_URL=https://demo-api.binance.com
//! cargo test -p ricow_binance --test demo_spot_live -- --ignored --nocapture
//! ```
//!
//! 多交易对: 从 exchangeInfo 候选池 (quote=USDT 且 TRADING) 固定 seed 伪随机选 3 对,
//! 每对跑 限价挂单→撤单→市价买→余额方向断言→市价卖平仓。seed 固定保证可复现。
//!
//! 2026-09-04 建 (specs/plans/2026-09-04_213220-bn-demo-live-test checklist T2)。

use ricow_binance::{validate_quantity, BinanceClient};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// 从 env 装配 demo 现货客户端 (key 缺失 panic; base_url 由 RICOW_BN_BASE_URL 注入)。
fn demo_client() -> BinanceClient {
    let api_key = std::env::var("RICOW_BN_API_KEY")
        .expect("RICOW_BN_API_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    let secret = std::env::var("RICOW_BN_SECRET_KEY")
        .expect("RICOW_BN_SECRET_KEY 未设置 (demo 测试网 key, 见 specs/testnet.md)");
    BinanceClient::new()
        .expect("BinanceClient 构造失败")
        .with_credentials(api_key, secret)
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

/// 按目标名义价值算合法数量: qty = 名义/价, 对齐 step_size 整数倍, 且 ≥ min_qty。
fn qty_for_notional(price: Decimal, notional: Decimal, min_qty: Decimal, step: Option<Decimal>) -> Decimal {
    let raw = (notional / price).max(min_qty);
    let aligned = match step {
        Some(s) if !s.is_zero() => {
            let mult = (raw / s).floor();
            (mult * s).max(min_qty)
        }
        _ => raw,
    };
    assert!(
        validate_quantity(min_qty, step, aligned),
        "数量 {aligned} 不合法 (min_qty={min_qty}, step={step:?})"
    );
    aligned
}

/// 从 balances 数组取某资产 free 余额。
fn free_balance(balances: &serde_json::Value, asset: &str) -> Decimal {
    balances
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|b| b["asset"].as_str() == Some(asset))
                .map(|b| {
                    Decimal::from_str_exact(b["free"].as_str().unwrap_or("0"))
                        .unwrap_or(Decimal::ZERO)
                })
        })
        .unwrap_or(Decimal::ZERO)
}

#[tokio::test]
#[ignore = "需 demo 测试网 env key + 网络; 禁 mock 纪律, 真实调用"]
async fn demo_spot_roundtrip_multiple_pairs() {
    let client = demo_client();

    // ① 账户基线: 打印 balances (不预设初始资产)。
    let account = client.get_account().await.expect("get_account 失败 (检查 key 权限)");
    let balances = &account["balances"];
    println!("== 账户基线 balances (free/locked) ==");
    if let Some(arr) = balances.as_array() {
        for b in arr.iter().filter(|b| {
            b["free"].as_str().map(|f| f != "0").unwrap_or(false)
                || b["locked"].as_str().map(|l| l != "0").unwrap_or(false)
        }) {
            println!("  {} free={} locked={}", b["asset"].as_str().unwrap_or("?"),
                b["free"].as_str().unwrap_or("0"), b["locked"].as_str().unwrap_or("0"));
        }
    }

    // ② 候选池: quote=USDT 且 TRADING (get_exchange_info 已过滤 TRADING)。
    let markets = client.get_exchange_info().await.expect("exchangeInfo 失败");
    let pool: Vec<String> = markets
        .iter()
        .filter(|m| m.quote_asset == "USDT" && !m.is_delisted)
        .map(|m| m.symbol.clone())
        .collect();
    assert!(pool.len() >= 3, "候选池不足 3 对: {}", pool.len());

    // 固定 seed 选 3 对不同对。
    let symbols = pick_symbols(&pool, 3);
    println!("== 本次测试交易对 (seed 固定可复现) ==");
    for s in &symbols {
        println!("  {s}");
    }

    let usdt_before = free_balance(balances, "USDT");

    for symbol in symbols {
        let m = markets.iter().find(|m| &m.symbol == symbol).unwrap();
        let min_qty = m.min_size;
        let step = m.step_size;
        let base = &m.base_asset;
        println!("\n--- 交易对 {symbol} (min_qty={min_qty}, step={step:?}) ---");

        // 取最新收盘价作参考。
        let klines = client.get_klines(symbol, "1h", 1).await.expect("klines 失败");
        let last = klines.last().expect("无 K 线");
        let px = last.close;
        println!("  参考价 close={px}");

        let target_notional = dec!(20); // 名义 ~20 USDT/笔
        let qty = qty_for_notional(px, target_notional, min_qty, step);
        let base_before = free_balance(balances, base);

        // ③ 限价买挂单 (价低于市价, 保证 Open 不立即成交)。
        let limit_price = (px * dec!(0.95)).round_dp(2);
        let cid = format!("ricow-demo-{symbol}-{}", std::process::id());
        let resp = client
            .place_order(symbol, "BUY", "LIMIT", &qty.to_string(), Some(&limit_price.to_string()), &cid)
            .await
            .expect("限价挂单失败");
        assert_eq!(resp["status"].as_str(), Some("NEW"), "限价单应 Open: {resp}");
        println!("  限价挂单 OK orderId={} qty={qty} price={limit_price}", resp["orderId"]);

        // ④ 撤单。
        client.cancel_order(symbol, &cid).await.expect("撤单失败");
        println!("  撤单 OK");

        // ⑤ 市价买 → Filled。
        let resp2 = client
            .place_order(symbol, "BUY", "MARKET", &qty.to_string(), None, &cid)
            .await
            .expect("市价买单失败");
        let filled: Decimal =
            Decimal::from_str_exact(resp2["executedQty"].as_str().unwrap_or("0")).unwrap();
        assert!(filled > Decimal::ZERO, "市价买应成交: {resp2}");
        println!("  市价买 Filled qty={filled} 均价={}", resp2["price"].as_str().unwrap_or("?"));

        // ⑥ 余额方向断言: base 增加。
        let account2 = client.get_account().await.expect("get_account 2 失败");
        let balances2 = &account2["balances"];
        let base_after = free_balance(balances2, base);
        assert!(
            base_after > base_before,
            "base {base} 应增加: before={base_before} after={base_after}"
        );
        println!("  balance {base}: {base_before} → {base_after}");

        // ⑦ 市价卖平仓, 恢复零仓。余额含手续费扣减, 未必是 step 整数倍 → 向下对齐 (LOT_SIZE)。
        let mut sell_qty = base_after;
        if let Some(s) = step {
            if !s.is_zero() {
                sell_qty = ((sell_qty / s).floor() * s).max(Decimal::ZERO);
            }
        }
        if sell_qty >= min_qty {
            let cid_sell = format!("ricow-demo-sell-{symbol}-{}", std::process::id());
            let resp3 = client
                .place_order(symbol, "SELL", "MARKET", &sell_qty.to_string(), None, &cid_sell)
                .await
                .expect("市价卖失败");
            let sold: Decimal =
                Decimal::from_str_exact(resp3["executedQty"].as_str().unwrap_or("0")).unwrap();
            println!("  市价卖平仓 Filled qty={sold} (余额 {base_after} 向下对齐 step)");
            assert!(sold > Decimal::ZERO, "平仓应成交: {resp3}");
        } else {
            println!("  余额 {base_after} 低于 min_qty={min_qty}, 跳过平仓 (残留 dust 可忽略)");
        }

        // 平仓后余额回落 (残留 dust < min_qty 可接受)。
        let account_after_sell = client.get_account().await.expect("get_account 卖后失败");
        let balances_after = &account_after_sell["balances"];
        let base_final = free_balance(balances_after, base);
        assert!(
            base_final < min_qty,
            "{base} 平仓后应归零 (残留 < min_qty={min_qty}), 实际 {base_final}"
        );
        println!("  平仓后 {base} 余额 = {base_final} (归零 ✓)");
    }

    // 汇总: USDT 变化仅参考 (demo 账户可能有历史残留仓被本测试平掉变现 → 变化方向不定)。
    let account3 = client.get_account().await.expect("get_account 3 失败");
    let balances3 = &account3["balances"];
    let usdt_after = free_balance(balances3, "USDT");
    let delta = usdt_after - usdt_before;
    println!("\n== 汇总: USDT {usdt_before} → {usdt_after} (Δ={delta}, 含手续费+滑点+历史残留变现) ==");
    if delta.abs() > dec!(5) {
        println!("  提示: USDT 变化 > 5, 通常因 demo 账户存在历史残留仓位被本测试顺手平掉变现, 非异常。");
    }
    println!("PASS: 多交易对现货真实下单闭环全部通过 (每对均完成 挂单→撤单→市价买→平仓归零)");
}
