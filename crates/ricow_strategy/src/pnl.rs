//! 策略盈亏追踪器。

use ricow_core::OrderFill;
use rust_decimal::Decimal;

/// 保留的成交记录上限。
const MAX_FILLS: usize = 1000;

#[derive(Debug, Clone, Default)]
pub struct PnlTracker {
    realized_pnl: Decimal,
    total_fees: Decimal,
    trade_count: u64,
    winning_trades: u64,
    losing_trades: u64,
    /// 盈利交易毛额合计 (正盈亏累加)。
    gross_profit: Decimal,
    /// 亏损交易毛额合计 (负盈亏绝对值累加)。
    gross_loss: Decimal,
    fills: Vec<OrderFill>,
}

impl PnlTracker {
    /// 记录一笔成交 (只记 fee 与计数, PnL 由 `record_pnl` 单独更新)。
    pub fn record_fill(&mut self, fill: &OrderFill) {
        self.fills.push(fill.clone());
        if self.fills.len() > MAX_FILLS {
            let excess = self.fills.len() - MAX_FILLS;
            self.fills.drain(..excess);
        }
        self.trade_count += 1;
        self.total_fees += fill.fee;
    }

    /// 记录已实现盈亏。
    pub fn record_pnl(&mut self, amount: Decimal) {
        self.realized_pnl += amount;
        if amount > Decimal::ZERO {
            self.winning_trades += 1;
            self.gross_profit += amount;
        } else if amount < Decimal::ZERO {
            self.losing_trades += 1;
            self.gross_loss += -amount;
        }
    }

    pub fn realized_pnl(&self) -> Decimal {
        self.realized_pnl
    }

    pub fn total_fees(&self) -> Decimal {
        self.total_fees
    }

    pub fn net_pnl(&self) -> Decimal {
        self.realized_pnl - self.total_fees
    }

    pub fn trade_count(&self) -> u64 {
        self.trade_count
    }

    pub fn win_rate(&self) -> f64 {
        let total = self.winning_trades + self.losing_trades;
        if total == 0 {
            0.0
        } else {
            self.winning_trades as f64 / total as f64
        }
    }

    pub fn gross_profit(&self) -> Decimal {
        self.gross_profit
    }

    pub fn gross_loss(&self) -> Decimal {
        self.gross_loss
    }

    pub fn winning_trades(&self) -> u64 {
        self.winning_trades
    }

    pub fn losing_trades(&self) -> u64 {
        self.losing_trades
    }

    pub fn fills(&self) -> &[OrderFill] {
        &self.fills
    }
}

/// 最大回撤 (比率): 权益序列峰值→谷值最大跌幅。空/单点序列返回 0。
///
/// 例: [100, 110, 90, 95, 80] → 峰值 110, 谷值 80, 回撤 (110-80)/110。
pub fn max_drawdown(equity: &[Decimal]) -> Decimal {
    let mut peak = Decimal::ZERO;
    let mut max_dd = Decimal::ZERO;
    for &e in equity {
        if e > peak {
            peak = e;
        }
        if peak > Decimal::ZERO {
            let dd = (peak - e) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ricow_core::OrderSide;
    use rust_decimal_macros::dec;

    fn sample_fill(side: OrderSide, fee: Decimal) -> OrderFill {
        OrderFill {
            trade_id: None,
            exchange_order_id: "test-id".into(),
            client_order_id: "client-id".into(),
            pair: "ETH".into(),
            side,
            fill_price: dec!(3000),
            fill_size: dec!(1),
            fee,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_record_pnl_and_win_rate() {
        let mut pnl = PnlTracker::default();
        pnl.record_pnl(dec!(100));
        pnl.record_pnl(dec!(-50));
        pnl.record_pnl(dec!(200));
        assert_eq!(pnl.realized_pnl(), dec!(250));
        assert_eq!(pnl.win_rate(), 2.0 / 3.0);
        assert_eq!(pnl.gross_profit(), dec!(300));
        assert_eq!(pnl.gross_loss(), dec!(50));
    }

    #[test]
    fn test_net_pnl_with_fees() {
        let mut pnl = PnlTracker::default();
        pnl.record_fill(&sample_fill(OrderSide::Buy, dec!(2)));
        pnl.record_pnl(dec!(200));
        assert_eq!(pnl.net_pnl(), dec!(198));
    }

    #[test]
    fn test_fills_capped() {
        let mut pnl = PnlTracker::default();
        for _ in 0..(MAX_FILLS + 10) {
            pnl.record_fill(&sample_fill(OrderSide::Buy, dec!(1)));
        }
        assert_eq!(pnl.fills().len(), MAX_FILLS);
        assert_eq!(pnl.trade_count(), (MAX_FILLS + 10) as u64);
    }

    #[test]
    fn test_max_drawdown_known_sequence() {
        // [100, 110, 90, 95, 80]: 峰值 110 → 谷值 80, 回撤 (110-80)/110 ≈ 0.2727。
        let eq = vec![dec!(100), dec!(110), dec!(90), dec!(95), dec!(80)];
        let dd = max_drawdown(&eq);
        assert!(dd > dec!(0.27) && dd < dec!(0.28), "max_drawdown = {dd}");
        // 空/单点序列: 0。
        assert_eq!(max_drawdown(&[]), Decimal::ZERO);
        assert_eq!(max_drawdown(&[dec!(100)]), Decimal::ZERO);
        // 单调上涨: 0。
        assert_eq!(max_drawdown(&[dec!(100), dec!(110), dec!(120)]), Decimal::ZERO);
    }
}
