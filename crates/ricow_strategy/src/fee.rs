//! 费用模型: 按成交金额计算 maker/taker 手续费。

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// 费用模型。默认费率为 Binance 现货 (maker/taker 均 10bps)。
///
/// 实盘不建模 — 交易所直接返回实际手续费, 此模型仅用于 DryRun / 回测。
#[derive(Debug, Clone)]
pub struct FeeModel {
    maker_bps: Decimal,
    taker_bps: Decimal,
}

impl FeeModel {
    /// Binance 现货费率: maker/taker 均 10bps (当前默认交易所)。
    pub fn binance_default() -> Self {
        Self { maker_bps: dec!(10), taker_bps: dec!(10) }
    }

    pub fn new(maker_bps: Decimal, taker_bps: Decimal) -> Self {
        Self { maker_bps, taker_bps }
    }

    /// 计算一笔成交的手续费 (报价资产计): fill_price × fill_size × bps / 10000。
    pub fn calc_fee(&self, fill_price: Decimal, fill_size: Decimal, is_maker: bool) -> Decimal {
        let bps = if is_maker { self.maker_bps } else { self.taker_bps };
        if bps.is_zero() {
            return Decimal::ZERO;
        }
        fill_price * fill_size * bps / dec!(10000)
    }

    pub fn maker_bps(&self) -> Decimal {
        self.maker_bps
    }

    pub fn taker_bps(&self) -> Decimal {
        self.taker_bps
    }
}

impl Default for FeeModel {
    fn default() -> Self {
        Self::binance_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_binance_spot() {
        let fm = FeeModel::default();
        assert_eq!(fm.maker_bps(), dec!(10));
        assert_eq!(fm.taker_bps(), dec!(10));
    }

    #[test]
    fn test_maker_fee() {
        let fm = FeeModel::default();
        assert_eq!(fm.calc_fee(dec!(3000), dec!(1), true), dec!(3.0));
    }

    #[test]
    fn test_taker_fee() {
        let fm = FeeModel::default();
        assert_eq!(fm.calc_fee(dec!(3000), dec!(1), false), dec!(3.0));
    }

    #[test]
    fn test_zero_bps() {
        let fm = FeeModel::new(Decimal::ZERO, Decimal::ZERO);
        assert_eq!(fm.calc_fee(dec!(3000), dec!(1), true), Decimal::ZERO);
    }
}
