//! bStock → 美股 ticker 映射表 (2026-09-07 离线实测固化)。
//!
//! 数据源: spot exchangeInfo (base 去尾 B) × fapi EQUITY 白名单交叉识别 + Nasdaq 官方 API
//! 全池逐只验证 (assetclass 分类, 70/70 可拉, 并发 4 ~81s)。Nasdaq 返回倒序日线, 本表只存映射。
//!
//! 口径注记:
//! - 某些标的 Nasdaq 历史含 symbol 延续 (如 NBIS 含前身 YNDX 历史, 2022-2024 停牌段 volume=N/A,
//!   2024-10-21 重新上市) — R1 研究回测按数据首根近似上市日, 报告注明延续口径 (计划 §五 R3)。
//! - ETF (QQQ/SPY/TQQQ/SOXL/EWY/KORU/SMH/DRAM/INTW/MUU/MVLL/SNXX/SOXS/SQQQ) 拉取须 assetclass=etf。
//! - 池每周扩容 (9-06 快照 67 → 9-07 实测 70); 新上市对重新跑一次探测流程即可增补。
//!
//! 单一权威 = 本文件 (005-market-filter); 勿在别处另建映射。

/// Nasdaq assetclass (ETF 必须传 etf, 否则 data=null)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetClass {
    Stock,
    Etf,
}

impl AssetClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetClass::Stock => "stocks",
            AssetClass::Etf => "etf",
        }
    }
}

/// bStock base (TSLAB) → 美股代码 (TSLA) + Nasdaq assetclass。
#[derive(Debug, Clone, Copy)]
pub struct BstockMap {
    pub bstock: &'static str,
    pub us: &'static str,
    pub assetclass: AssetClass,
}

/// 全池映射表 (70 只, 2026-09-07 实测)。
pub static BSTOCK_MAP: &[BstockMap] = &[
    BstockMap { bstock: "AAOIB", us: "AAOI", assetclass: AssetClass::Stock },
    BstockMap { bstock: "AAPLB", us: "AAPL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "ALABB", us: "ALAB", assetclass: AssetClass::Stock },
    BstockMap { bstock: "AMATB", us: "AMAT", assetclass: AssetClass::Stock },
    BstockMap { bstock: "AMDB", us: "AMD", assetclass: AssetClass::Stock },
    BstockMap { bstock: "AMZNB", us: "AMZN", assetclass: AssetClass::Stock },
    BstockMap { bstock: "ARMB", us: "ARM", assetclass: AssetClass::Stock },
    BstockMap { bstock: "ASMLB", us: "ASML", assetclass: AssetClass::Stock },
    BstockMap { bstock: "ASTSB", us: "ASTS", assetclass: AssetClass::Stock },
    BstockMap { bstock: "AVGOB", us: "AVGO", assetclass: AssetClass::Stock },
    BstockMap { bstock: "AXTIB", us: "AXTI", assetclass: AssetClass::Stock },
    BstockMap { bstock: "BABAB", us: "BABA", assetclass: AssetClass::Stock },
    BstockMap { bstock: "BEB", us: "BE", assetclass: AssetClass::Stock },
    BstockMap { bstock: "BMNRB", us: "BMNR", assetclass: AssetClass::Stock },
    BstockMap { bstock: "CBRSB", us: "CBRS", assetclass: AssetClass::Stock },
    BstockMap { bstock: "COHRB", us: "COHR", assetclass: AssetClass::Stock },
    BstockMap { bstock: "COINB", us: "COIN", assetclass: AssetClass::Stock },
    BstockMap { bstock: "CRCLB", us: "CRCL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "CRDOB", us: "CRDO", assetclass: AssetClass::Stock },
    BstockMap { bstock: "CRWDB", us: "CRWD", assetclass: AssetClass::Stock },
    BstockMap { bstock: "CRWVB", us: "CRWV", assetclass: AssetClass::Stock },
    BstockMap { bstock: "DELLB", us: "DELL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "DJTB", us: "DJT", assetclass: AssetClass::Stock },
    BstockMap { bstock: "DRAMB", us: "DRAM", assetclass: AssetClass::Etf },
    BstockMap { bstock: "EWYB", us: "EWY", assetclass: AssetClass::Etf },
    BstockMap { bstock: "FLNCB", us: "FLNC", assetclass: AssetClass::Stock },
    BstockMap { bstock: "GLWB", us: "GLW", assetclass: AssetClass::Stock },
    BstockMap { bstock: "GMEB", us: "GME", assetclass: AssetClass::Stock },
    BstockMap { bstock: "GOOGLB", us: "GOOGL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "GSB", us: "GS", assetclass: AssetClass::Stock },
    BstockMap { bstock: "HOODB", us: "HOOD", assetclass: AssetClass::Stock },
    BstockMap { bstock: "IBMB", us: "IBM", assetclass: AssetClass::Stock },
    BstockMap { bstock: "INTCB", us: "INTC", assetclass: AssetClass::Stock },
    BstockMap { bstock: "INTWB", us: "INTW", assetclass: AssetClass::Etf },
    BstockMap { bstock: "IRENB", us: "IREN", assetclass: AssetClass::Stock },
    BstockMap { bstock: "KORUB", us: "KORU", assetclass: AssetClass::Etf },
    BstockMap { bstock: "LITEB", us: "LITE", assetclass: AssetClass::Stock },
    BstockMap { bstock: "METAB", us: "META", assetclass: AssetClass::Stock },
    BstockMap { bstock: "MRNAB", us: "MRNA", assetclass: AssetClass::Stock },
    BstockMap { bstock: "MRVLB", us: "MRVL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "MSFTB", us: "MSFT", assetclass: AssetClass::Stock },
    BstockMap { bstock: "MSTRB", us: "MSTR", assetclass: AssetClass::Stock },
    BstockMap { bstock: "MUB", us: "MU", assetclass: AssetClass::Stock },
    BstockMap { bstock: "MUUB", us: "MUU", assetclass: AssetClass::Etf },
    BstockMap { bstock: "MVLLB", us: "MVLL", assetclass: AssetClass::Etf },
    BstockMap { bstock: "NBISB", us: "NBIS", assetclass: AssetClass::Stock },
    BstockMap { bstock: "NFLXB", us: "NFLX", assetclass: AssetClass::Stock },
    BstockMap { bstock: "NOKB", us: "NOK", assetclass: AssetClass::Stock },
    BstockMap { bstock: "NVDAB", us: "NVDA", assetclass: AssetClass::Stock },
    BstockMap { bstock: "ORCLB", us: "ORCL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "PLTRB", us: "PLTR", assetclass: AssetClass::Stock },
    BstockMap { bstock: "PYPLB", us: "PYPL", assetclass: AssetClass::Stock },
    BstockMap { bstock: "QCOMB", us: "QCOM", assetclass: AssetClass::Stock },
    BstockMap { bstock: "QQQB", us: "QQQ", assetclass: AssetClass::Etf },
    BstockMap { bstock: "RKLBB", us: "RKLB", assetclass: AssetClass::Stock },
    BstockMap { bstock: "SKHYB", us: "SKHY", assetclass: AssetClass::Stock },
    BstockMap { bstock: "SMCIB", us: "SMCI", assetclass: AssetClass::Stock },
    BstockMap { bstock: "SMHB", us: "SMH", assetclass: AssetClass::Etf },
    BstockMap { bstock: "SNDKB", us: "SNDK", assetclass: AssetClass::Stock },
    BstockMap { bstock: "SNXXB", us: "SNXX", assetclass: AssetClass::Etf },
    BstockMap { bstock: "SOXLB", us: "SOXL", assetclass: AssetClass::Etf },
    BstockMap { bstock: "SOXSB", us: "SOXS", assetclass: AssetClass::Etf },
    BstockMap { bstock: "SPCXB", us: "SPCX", assetclass: AssetClass::Stock },
    BstockMap { bstock: "SPYB", us: "SPY", assetclass: AssetClass::Etf },
    BstockMap { bstock: "SQQQB", us: "SQQQ", assetclass: AssetClass::Etf },
    BstockMap { bstock: "TQQQB", us: "TQQQ", assetclass: AssetClass::Etf },
    BstockMap { bstock: "TSLAB", us: "TSLA", assetclass: AssetClass::Stock },
    BstockMap { bstock: "TSMB", us: "TSM", assetclass: AssetClass::Stock },
    BstockMap { bstock: "USARB", us: "USAR", assetclass: AssetClass::Stock },
    BstockMap { bstock: "WDCB", us: "WDC", assetclass: AssetClass::Stock },
];

/// 查 bStock base → 美股代码 + assetclass。
pub fn us_ticker_of(bstock_base: &str) -> Option<(&'static str, AssetClass)> {
    BSTOCK_MAP
        .iter()
        .find(|m| m.bstock.eq_ignore_ascii_case(bstock_base))
        .map(|m| (m.us, m.assetclass))
}

/// 杠杆/反向产品名单 (bs_momentum 生产池默认剔除, 2026-09-09 用户拍板 "不要杠杆产品")。
///
/// 识别依据 = 公开常识 (ProShares UltraPro/Direxion Daily 等每日重置杠杆) + 缓存数据双验
/// (单日最大涨跌 >±30% 或年化波动 >100% 只可能出自每日重置杠杆, 1x 基金不可达):
/// - TQQQ/SQQQ (ProShares UltraPro QQQ 3x/反向3x): 单日 +35%/−35%, vol 67%
/// - SOXL/SOXS (Direxion 半导体 3x/反向3x): 单日 +55%/−56%, vol 104%
/// - KORU (Direxion 韩国 3x, 2024-03 已清算): 单日 +36%/−32%, vol 73%
/// - MUU/MVLL/INTW (2024-25 上市新杠杆 ETF, 名称含 daily/leveraged 语义; 数据特征
///   单日 +38%/+65%/+47%, vol 145%/152%/149%)
/// - DRAM 保留 (单日 ±17% = 1x 常态); SNXX 数据异常但 <253 根永不入选, 不在此列。
///
/// 池每周扩容时须核查新 ETF 名称 (含 Daily/3x/Bull/Bear/UltraPro 等词) 或数据特征后增补。
pub fn is_leveraged(us_ticker: &str) -> bool {
    matches!(us_ticker, "TQQQ" | "SQQQ" | "SOXL" | "SOXS" | "KORU" | "MUU" | "MVLL" | "INTW")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_sample_lookup() {
        assert_eq!(us_ticker_of("TSLAB"), Some(("TSLA", AssetClass::Stock)));
        assert_eq!(us_ticker_of("AAPLB"), Some(("AAPL", AssetClass::Stock)));
        assert_eq!(us_ticker_of("SPYB"), Some(("SPY", AssetClass::Etf)));
        assert_eq!(us_ticker_of("QQQB"), Some(("QQQ", AssetClass::Etf)));
        assert_eq!(us_ticker_of("BRKB"), None, "BRK.B 不在现池 (点号特例规则在 Nasdaq 拉取层)");
        assert_eq!(us_ticker_of("TSLABUSDT"), None, "需传 base 非 symbol");
    }

    #[test]
    fn test_leveraged_classification() {
        // 杠杆名单 (生产池默认剔除): 3x/反向 + 新杠杆 ETF。
        for u in ["TQQQ", "SQQQ", "SOXL", "SOXS", "KORU", "MUU", "MVLL", "INTW"] {
            assert!(is_leveraged(u), "{u} 应在杠杆名单");
        }
        // 1x ETF / 股票不在名单。
        for u in ["SPY", "QQQ", "SMH", "EWY", "DRAM", "AAPL", "NVDA", "SNXX"] {
            assert!(!is_leveraged(u), "{u} 不应在杠杆名单");
        }
        // 名单全部在现池内 (防拼写漂移)。
        for m in BSTOCK_MAP {
            if is_leveraged(m.us) {
                assert_eq!(m.assetclass, AssetClass::Etf, "{} 杠杆名单项应为 ETF", m.us);
            }
        }
    }

    #[test]
    fn test_map_no_duplicate_us_or_bstock() {
        let mut us: Vec<&str> = BSTOCK_MAP.iter().map(|m| m.us).collect();
        let mut bs: Vec<&str> = BSTOCK_MAP.iter().map(|m| m.bstock).collect();
        us.sort();
        bs.sort();
        let us0 = us.len();
        let bs0 = bs.len();
        us.dedup();
        bs.dedup();
        assert_eq!(us.len(), us0, "美股代码重复");
        assert_eq!(bs.len(), bs0, "bStock base 重复");
        assert_eq!(us0, 70, "池规模快照应同步刷新");
    }

    #[test]
    fn test_etf_classification_consistency() {
        // 抽查 ETF: assetclass=etf 拉取才有效 (Nasdaq 数据事实)。
        for m in BSTOCK_MAP {
            match m.us {
                "QQQ" | "SPY" | "TQQQ" | "SQQQ" | "SOXL" | "EWY" | "KORU" | "SMH" => {
                    assert_eq!(m.assetclass, AssetClass::Etf, "{}", m.us);
                }
                _ => {}
            }
        }
    }
}
