//! 市场分类 — bStock (美股代币) 现货池识别。
//!
//! 识别规则 (2026-09-06 免 key 实测定稿, specs/changes/005-market-filter):
//! spot bStock = spot TRADING 且 base 以 B 结尾, 且 **base 去尾 B ∈ fapi EQUITY 池**
//! (underlyingType=EQUITY 永续 base 白名单, 见 ricow_binance::FuturesDataClient::get_equity_pool)。
//!
//! 坑: 单靠 "base 以 B 结尾" 有 crypto 假阳性 — ARBUSDT(Arbitrum)/STXBUSDT(Stacks)/
//! QNTBUSDT(Quant) 等 base 均以 B 结尾但非股票; 必须与 fapi EQUITY 白名单交叉 (实测层结论)。
//!
//! 交易对视野 (G7): 默认只展示股票类 (bStock 现货 + 美股永续), `[market] show_all_pairs=true`
//! 放开为全量。组装与检索都是纯函数 (本模块), 网络拉取在 `ricow::commands::pairs`。

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

/// 交易对视野: 现货 / 合约两组符号 + 是否为"仅股票类"过滤视野。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PairsView {
    pub spot: Vec<String>,
    pub futures: Vec<String>,
    /// true = 默认过滤视野(仅 bStock 现货 + 美股永续); false = 全量。
    pub filtered: bool,
}

impl PairsView {
    /// 视野内合计数量。
    pub fn total(&self) -> usize {
        self.spot.len() + self.futures.len()
    }
}

/// 组装视野 (纯函数)。
///
/// - `show_all = false`(默认): 现货只留 bStock(XxxB 形态且去尾 B ∈ EQUITY 白名单),
///   合约只留 EQUITY 池 base(TSLAUSDT 形态);
/// - `show_all = true`: 现货全 TRADING, 合约全永续(PERPETUAL ∪ TRADIFI_PERPETUAL)。
///
/// `spots` / `futures` 均为交易所解析结果(TRADING 已由解析层过滤)。
pub fn build_view(
    spots: &[Market],
    futures: &[Market],
    equity_bases: &[String],
    show_all: bool,
) -> PairsView {
    if show_all {
        return PairsView {
            spot: symbols_where(spots, |_| true),
            futures: symbols_where(futures, |_| true),
            filtered: false,
        };
    }
    let spot_bases = bstock_spot_pool(spots, equity_bases);
    PairsView {
        spot: symbols_where(spots, |m| spot_bases.iter().any(|b| b == &m.base_asset)),
        futures: symbols_where(futures, |m| {
            equity_bases.iter().any(|e| e.eq_ignore_ascii_case(&m.base_asset))
        }),
        filtered: true,
    }
}

/// 视图检索 (纯函数): 市场限定 + 交易对子串(大小写不敏感)。
///
/// `market` 只识别 `"spot"` / `"futures"`(大小写与前后空白容忍), 其余或 `None` = 两组都返回。
pub fn filter_view(view: &PairsView, market: Option<&str>, q: Option<&str>) -> PairsView {
    let market = market.map(|s| s.trim().to_ascii_lowercase());
    let want_spot = market.as_deref() != Some("futures");
    let want_futures = market.as_deref() != Some("spot");
    let needle = q.map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty());
    let pick = |list: &[String]| -> Vec<String> {
        match &needle {
            None => list.to_vec(),
            Some(n) => list
                .iter()
                .filter(|s| s.to_ascii_uppercase().contains(n.as_str()))
                .cloned()
                .collect(),
        }
    };
    PairsView {
        spot: if want_spot { pick(&view.spot) } else { Vec::new() },
        futures: if want_futures { pick(&view.futures) } else { Vec::new() },
        filtered: view.filtered,
    }
}

/// 按谓词取符号(去重 + 排序, 保证输出稳定)。
fn symbols_where(markets: &[Market], keep: impl Fn(&Market) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in markets.iter().filter(|m| keep(m)) {
        if !out.iter().any(|s| s == &m.symbol) {
            out.push(m.symbol.clone());
        }
    }
    out.sort();
    out
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

    /// 默认视野用到的市场全集: 2 个 bStock 现货 + 1 个普通现货 + 2 个股票永续 + 1 个普通永续。
    fn sample() -> (Vec<Market>, Vec<Market>, Vec<String>) {
        let spots = vec![market("AAPLB", false), market("TSLAB", false), market("BTC", false)];
        let futures = vec![market("AAPL", true), market("TSLA", true), market("BTC", true)];
        let equity = ["AAPL", "TSLA"].iter().map(|s| s.to_string()).collect();
        (spots, futures, equity)
    }

    #[test]
    fn test_build_view_default_is_stock_only() {
        let (spots, futures, equity) = sample();
        let v = build_view(&spots, &futures, &equity, false);
        assert!(v.filtered);
        assert_eq!(v.spot, vec!["AAPLBUSDT".to_string(), "TSLABUSDT".to_string()]);
        assert_eq!(v.futures, vec!["AAPLUSDT".to_string(), "TSLAUSDT".to_string()]);
        assert_eq!(v.total(), 4);
    }

    #[test]
    fn test_build_view_all_is_unfiltered() {
        let (spots, futures, equity) = sample();
        let v = build_view(&spots, &futures, &equity, true);
        assert!(!v.filtered);
        assert_eq!(v.spot, vec!["AAPLBUSDT", "BTCUSDT", "TSLABUSDT"]);
        assert_eq!(v.futures, vec!["AAPLUSDT", "BTCUSDT", "TSLAUSDT"]);
    }

    #[test]
    fn test_filter_view_market_and_query() {
        let (spots, futures, equity) = sample();
        let v = build_view(&spots, &futures, &equity, false);

        // 只现货 / 只合约
        let only_spot = filter_view(&v, Some("spot"), None);
        assert_eq!(only_spot.spot.len(), 2);
        assert!(only_spot.futures.is_empty());
        let only_fut = filter_view(&v, Some("FUTURES"), None);
        assert!(only_fut.spot.is_empty());
        assert_eq!(only_fut.futures.len(), 2);

        // 子串(大小写不敏感) + 组内命中也算
        let q = filter_view(&v, None, Some("tsl"));
        assert_eq!(q.spot, vec!["TSLABUSDT".to_string()]);
        assert_eq!(q.futures, vec!["TSLAUSDT".to_string()]);
        // 无命中 → 空(不是全量回落)
        assert_eq!(filter_view(&v, None, Some("ZZZ")).total(), 0);
        // 空串/空白 = 不检索
        assert_eq!(filter_view(&v, None, Some("  ")).total(), v.total());
        // 视野标志原样透传
        assert!(q.filtered);
    }
}
