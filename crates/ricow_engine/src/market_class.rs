//! 市场分类 — bStock (美股代币) 现货池识别。
//!
//! 识别规则 (2026-09-06 免 key 实测定稿, specs/changes/005-market-filter):
//! spot bStock = spot TRADING 且 base 以 B 结尾, 且 **base 去尾 B ∈ fapi EQUITY 池**
//! (underlyingType=EQUITY 永续 base 白名单, 见 ricow_binance::FuturesDataClient::get_equity_pool)。
//!
//! 坑: 单靠 "base 以 B 结尾" 有 crypto 假阳性 — ARBUSDT(Arbitrum)/STXBUSDT(Stacks)/
//! QNTBUSDT(Quant) 等 base 均以 B 结尾但非股票; 必须与 fapi EQUITY 白名单交叉 (实测层结论)。

use ricow_core::Market;

/// base 去尾 B 后是否 ∈ fapi EQUITY 白名单 (且本身以 B 结尾)。
///
/// `equity_bases` 为免 key 拉取的 fapi EQUITY base 列表 (如 ["AAPL","NVDA","TSLA"]),
/// 调用方负责拉取/缓存; 本函数纯逻辑可单测。
pub fn is_bstock_base(base: &str, equity_bases: &[String]) -> bool {
    let base = base.trim();
    let Some(stripped) = base.strip_suffix('B') else {
        return false;
    };
    if stripped.is_empty() {
        return false;
    }
    equity_bases.iter().any(|e| e.eq_ignore_ascii_case(stripped))
}

/// 从 spot 市场全集中筛出 bStock 现货池 (base 列表, 去重保序)。
///
/// `markets` = 现货 exchangeInfo 解析结果 (TRADING 已由解析层过滤);
/// `equity_bases` = fapi EQUITY 白名单。
pub fn bstock_spot_pool(markets: &[Market], equity_bases: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for m in markets {
        if m.is_perpetual {
            continue; // 仅现货池; 永续另走 EQUITY 池枚举
        }
        if !is_bstock_base(&m.base_asset, equity_bases) {
            continue;
        }
        if !seen.iter().any(|s| s == &m.base_asset) {
            seen.push(m.base_asset.clone());
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market(base: &str, is_perp: bool) -> Market {
        Market {
            symbol: format!("{base}USDT"),
            base_asset: base.to_string(),
            quote_asset: "USDT".into(),
            is_perpetual: is_perp,
            min_size: rust_decimal::Decimal::ONE,
            tick_size: rust_decimal::Decimal::ONE,
            step_size: None,
            min_notional: None,
            max_leverage: None,
            margin_mode: None,
            is_delisted: false,
        }
    }

    #[test]
    fn test_is_bstock_base_hits() {
        let equity =
            ["AAPL", "NVDA", "TSLA", "SPY"].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_bstock_base("AAPLB", &equity));
        assert!(is_bstock_base("TSLAB", &equity));
        // 大小写不敏感
        assert!(is_bstock_base("nvdaB", &equity));
    }

    #[test]
    fn test_is_bstock_base_excludes_crypto_false_positives() {
        // ARBUSDT/STXBUSDT/QNTBUSDT 的 base 以 B 结尾但非股票 (crypto 撞车)。
        let equity =
            ["AAPL", "NVDA", "TSLA", "SPY"].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(!is_bstock_base("ARB", &equity), "ARB 不在白名单");
        assert!(!is_bstock_base("STX", &equity));
        assert!(!is_bstock_base("QNT", &equity));
        // 无 B 后缀 / 空尾
        assert!(!is_bstock_base("AAPL", &equity));
        assert!(!is_bstock_base("B", &equity));
        // 去尾后不在白名单 (如某 crypto 恰以 B 结尾但列表无对应股票)
        assert!(!is_bstock_base("ZZZB", &equity));
    }

    #[test]
    fn test_bstock_spot_pool_filters_and_dedups() {
        let equity = ["AAPL", "NVDA", "TSLA"].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let markets = vec![
            market("AAPLB", false),
            market("TSLAB", false),
            market("NVDA", false), // 普通 NVDAUSDT (非股票代币) — 无 B 后缀, 排除
            market("ARB", false),  // Arbitrum — base 无 B 后缀(ARBUSDT), 排除
            market("STXB", false), // Stacks 代币恰以 B 结尾? STX 永续/现货 base=STX → 不会出现 STXB
            market("TSLAB", false), // 重复
            market("BTCB", true),  // 永续 — 跳过 (仅现货)
        ];
        let pool = bstock_spot_pool(&markets, &equity);
        assert_eq!(pool, vec!["AAPLB".to_string(), "TSLAB".to_string()]);
    }
}
