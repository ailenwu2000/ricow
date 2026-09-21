//! K 线周期标签与毫秒换算的**唯一定义** (028 T004)。
//!
//! 此前同一张表散落在两处 —— `ricow_binance::client::interval_ms` 与
//! `ricow_binance::futures_data::get_klines` 内部 match —— 两处支持集不同且容易长期背离。
//! 现统一到本模块: 其余位置一律经 [`Interval`] 取毫秒, 不再自带 match 表。
//!
//! 支持标签 = 币安 REST `/api/v3/klines` 与 `/fapi/v1/klines` 的公共子集:
//! `1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w`。
//! 标签**区分大小写** (小写, 与交易所请求参数一致), 未知名返回 `None`, 由调用方报错。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 单分钟毫秒数 (周期换算的基准单位)。
pub const MINUTE_MS: i64 = 60_000;

/// K 线周期 (标签 ↔ 毫秒)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Interval {
    M1,
    M3,
    M5,
    M15,
    M30,
    H1,
    H2,
    H4,
    H6,
    H8,
    H12,
    D1,
    D3,
    W1,
}

impl Interval {
    /// 全部受支持周期, 顺序 = 由短到长。
    pub const ALL: [Interval; 14] = [
        Interval::M1,
        Interval::M3,
        Interval::M5,
        Interval::M15,
        Interval::M30,
        Interval::H1,
        Interval::H2,
        Interval::H4,
        Interval::H6,
        Interval::H8,
        Interval::H12,
        Interval::D1,
        Interval::D3,
        Interval::W1,
    ];

    /// 周期标签 (交易所请求参数口径)。
    pub fn label(self) -> &'static str {
        match self {
            Interval::M1 => "1m",
            Interval::M3 => "3m",
            Interval::M5 => "5m",
            Interval::M15 => "15m",
            Interval::M30 => "30m",
            Interval::H1 => "1h",
            Interval::H2 => "2h",
            Interval::H4 => "4h",
            Interval::H6 => "6h",
            Interval::H8 => "8h",
            Interval::H12 => "12h",
            Interval::D1 => "1d",
            Interval::D3 => "3d",
            Interval::W1 => "1w",
        }
    }

    /// 周期毫秒数。
    pub fn ms(self) -> i64 {
        match self {
            Interval::M1 => MINUTE_MS,
            Interval::M3 => 3 * MINUTE_MS,
            Interval::M5 => 5 * MINUTE_MS,
            Interval::M15 => 15 * MINUTE_MS,
            Interval::M30 => 30 * MINUTE_MS,
            Interval::H1 => 60 * MINUTE_MS,
            Interval::H2 => 120 * MINUTE_MS,
            Interval::H4 => 240 * MINUTE_MS,
            Interval::H6 => 360 * MINUTE_MS,
            Interval::H8 => 480 * MINUTE_MS,
            Interval::H12 => 720 * MINUTE_MS,
            Interval::D1 => 1440 * MINUTE_MS,
            Interval::D3 => 4320 * MINUTE_MS,
            Interval::W1 => 10080 * MINUTE_MS,
        }
    }

    /// 标签 → 周期; 未知标签返回 `None` (区分大小写)。
    pub fn from_label(label: &str) -> Option<Interval> {
        Some(match label {
            "1m" => Interval::M1,
            "3m" => Interval::M3,
            "5m" => Interval::M5,
            "15m" => Interval::M15,
            "30m" => Interval::M30,
            "1h" => Interval::H1,
            "2h" => Interval::H2,
            "4h" => Interval::H4,
            "6h" => Interval::H6,
            "8h" => Interval::H8,
            "12h" => Interval::H12,
            "1d" => Interval::D1,
            "3d" => Interval::D3,
            "1w" => Interval::W1,
            _ => return None,
        })
    }

    /// 标签 → 毫秒; 未知标签返回 `None`。
    pub fn ms_of_label(label: &str) -> Option<i64> {
        Interval::from_label(label).map(Interval::ms)
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_roundtrip_for_all_intervals() {
        for iv in Interval::ALL {
            let back = Interval::from_label(iv.label()).expect("标签必须可反解");
            assert_eq!(back, iv, "{} 往返不一致", iv.label());
        }
    }

    #[test]
    fn test_ms_known_vectors() {
        assert_eq!(Interval::M1.ms(), 60_000);
        assert_eq!(Interval::M5.ms(), 300_000);
        assert_eq!(Interval::M15.ms(), 900_000);
        assert_eq!(Interval::H1.ms(), 3_600_000);
        assert_eq!(Interval::H4.ms(), 14_400_000);
        assert_eq!(Interval::D1.ms(), 86_400_000);
        assert_eq!(Interval::W1.ms(), 604_800_000);
    }

    #[test]
    fn test_ms_of_label_matches_enum() {
        for iv in Interval::ALL {
            assert_eq!(Interval::ms_of_label(iv.label()), Some(iv.ms()));
        }
    }

    #[test]
    fn test_unknown_and_case_sensitive_labels_rejected() {
        assert_eq!(Interval::from_label("7m"), None);
        assert_eq!(Interval::from_label(""), None);
        assert_eq!(Interval::from_label(" 1m"), None);
        assert_eq!(Interval::from_label("1H"), None, "标签区分大小写 (交易所请求口径)");
        assert_eq!(Interval::ms_of_label("bogus"), None);
    }

    #[test]
    fn test_covers_previous_two_tables() {
        // 原 client.rs 表 (14 个) 与 futures_data.rs 表 (6 个) 的并集必须全部仍受支持,
        // 否则本次收敛会静默缩窄既有能力。
        for label in
            ["1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d", "3d", "1w"]
        {
            assert!(Interval::ms_of_label(label).is_some(), "{label} 丢失");
        }
    }

    #[test]
    fn test_display_is_label() {
        assert_eq!(Interval::H1.to_string(), "1h");
        assert_eq!(Interval::D1.to_string(), "1d");
    }
}
