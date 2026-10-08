//! 策略盈亏追踪器。

use chrono::{DateTime, Utc};
use ricow_core::OrderFill;
use rust_decimal::Decimal;

/// 保留的成交记录上限 (032+ 可观测性: 1000 → 100_000, 保证逐笔导出完整)。
const MAX_FILLS: usize = 100_000;

/// 保留的平仓事件记录上限 (039 FR-8): 与成交记录同一取舍 —— 明细只保最近 N 条,
/// **总数另计**(`closed_total`), 由展示方决定是否说明"仅显示最近 N 条"。
pub const MAX_CLOSED_TRADES: usize = 1000;

/// 一次平仓事件的已实现盈亏 (039 FR-7)。
///
/// 语义与 [`PnlTracker::record_pnl`] 完全一致: **一次调用 = 一次平仓**(可以是一笔部分平仓),
/// 因此明细与聚合指标(胜/负笔数、已实现盈亏)天然自洽 —— 这是"逐笔盈亏"能机械核对的前提。
/// 不做开→平的 round-trip 配对: 合约对冲/组合模式下"这笔卖是开空还是平多"无唯一解。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedTrade {
    /// 平仓发生的时刻 (回测: 该 bar 的收盘时刻; live/dry-run: 成交时刻)。
    pub time: DateTime<Utc>,
    /// 本次平仓的已实现盈亏 (quote 计价; 未扣手续费, 费在 `total_fees` 单列)。
    pub pnl: Decimal,
}

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
    /// 最佳/最差单笔盈亏 (回测规范 v1 B 区; record_pnl 时跟踪极值)。
    best_trade: Option<Decimal>,
    worst_trade: Option<Decimal>,
    fills: Vec<OrderFill>,
    /// 平仓事件明细 (只保最近 `MAX_CLOSED_TRADES` 条)。
    closed_trades: Vec<ClosedTrade>,
    /// 平仓事件**总**次数 (不受上限影响, 供展示方如实报截断)。
    closed_total: u64,
    /// 分侧已实现盈亏 (032+ 可观测性, hedge 网格两侧各赚多少): key = "long"/"short"。
    realized_by_side: std::collections::HashMap<String, Decimal>,
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

    /// 记录已实现盈亏 (一次调用 = 一次平仓事件)。
    ///
    /// `ts` = 该次平仓的时刻 (回测传虚拟 bar 时间, 不许用墙钟 `Utc::now()`, 否则明细表
    /// 与图表时间轴对不上; live/dry-run 传成交自身的 `timestamp`)。
    pub fn record_pnl(&mut self, amount: Decimal, ts: DateTime<Utc>) {
        self.realized_pnl += amount;
        if amount > Decimal::ZERO {
            self.winning_trades += 1;
            self.gross_profit += amount;
        } else if amount < Decimal::ZERO {
            self.losing_trades += 1;
            self.gross_loss += -amount;
        }
        // 平仓明细: 总数无条件累加, 明细按上限保留**最近**的 (与 fills 同款 drain 语义)。
        self.closed_total += 1;
        self.closed_trades.push(ClosedTrade { time: ts, pnl: amount });
        if self.closed_trades.len() > MAX_CLOSED_TRADES {
            let excess = self.closed_trades.len() - MAX_CLOSED_TRADES;
            self.closed_trades.drain(..excess);
        }
        // 最佳/最差单笔极值 (0 为尘, 不参与极值以免覆盖真实极值)。
        if amount != Decimal::ZERO {
            self.best_trade = Some(self.best_trade.map_or(amount, |b: Decimal| b.max(amount)));
            self.worst_trade = Some(self.worst_trade.map_or(amount, |w: Decimal| w.min(amount)));
        }
    }

    /// 记录已实现盈亏并按方向仓侧累加 (032+ 可观测性; side_key = "long"/"short")。
    pub fn record_pnl_side(&mut self, amount: Decimal, ts: DateTime<Utc>, side_key: &str) {
        self.record_pnl(amount, ts);
        *self.realized_by_side.entry(side_key.to_string()).or_insert(Decimal::ZERO) += amount;
    }

    pub fn realized_by_side(&self) -> &std::collections::HashMap<String, Decimal> {
        &self.realized_by_side
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

    /// 最佳单笔盈亏 (回测规范 v1 B 区)。
    pub fn best_trade(&self) -> Option<Decimal> {
        self.best_trade
    }

    /// 最差单笔盈亏 (回测规范 v1 B 区)。
    pub fn worst_trade(&self) -> Option<Decimal> {
        self.worst_trade
    }

    pub fn fills(&self) -> &[OrderFill] {
        &self.fills
    }

    /// 平仓事件明细 (最多 [`MAX_CLOSED_TRADES`] 条, 最近优先保留)。
    pub fn closed_trades(&self) -> &[ClosedTrade] {
        &self.closed_trades
    }

    /// 平仓事件总次数 (不受明细上限影响; 与 `winning_trades + losing_trades` 同源,
    /// 差别仅在 `amount == 0` 的事件: 它们进总数、不进胜/负分类)。
    pub fn closed_trades_total(&self) -> u64 {
        self.closed_total
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
    use chrono::TimeZone;
    use ricow_core::OrderSide;
    use rust_decimal_macros::dec;

    /// 固定时刻 (避免测试依赖墙钟; 秒级递增便于断言顺序)。
    fn ts(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + sec, 0).single().expect("合法时间戳")
    }

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
            timestamp: ts(0),
            position_side: None,
        }
    }

    #[test]
    fn test_record_pnl_and_win_rate() {
        let mut pnl = PnlTracker::default();
        pnl.record_pnl(dec!(100), ts(1));
        pnl.record_pnl(dec!(-50), ts(2));
        pnl.record_pnl(dec!(200), ts(3));
        assert_eq!(pnl.realized_pnl(), dec!(250));
        assert_eq!(pnl.win_rate(), 2.0 / 3.0);
        assert_eq!(pnl.gross_profit(), dec!(300));
        assert_eq!(pnl.gross_loss(), dec!(50));
    }

    #[test]
    fn test_closed_trades_mirror_aggregates() {
        // 039 FR-7: 明细是聚合的**同源**展开 —— 顺序、时刻、正负号必须一一对应。
        let mut pnl = PnlTracker::default();
        pnl.record_pnl(dec!(10), ts(1));
        pnl.record_pnl(dec!(-4), ts(2));
        pnl.record_pnl(dec!(6), ts(3));
        let closed = pnl.closed_trades();
        assert_eq!(closed.len(), 3);
        assert_eq!(closed[0], ClosedTrade { time: ts(1), pnl: dec!(10) });
        assert_eq!(closed[2], ClosedTrade { time: ts(3), pnl: dec!(6) });
        assert_eq!(pnl.closed_trades_total(), 3);
        // 求和 = 已实现盈亏; 正数条数 = 胜, 负数条数 = 负。
        let sum: Decimal = closed.iter().map(|c| c.pnl).sum();
        assert_eq!(sum, pnl.realized_pnl());
        assert_eq!(
            closed.iter().filter(|c| c.pnl > Decimal::ZERO).count() as u64,
            pnl.winning_trades()
        );
        assert_eq!(
            closed.iter().filter(|c| c.pnl < Decimal::ZERO).count() as u64,
            pnl.losing_trades()
        );
        assert_eq!(pnl.winning_trades() + pnl.losing_trades(), pnl.closed_trades_total());
    }

    #[test]
    fn test_closed_trades_capped_but_total_kept() {
        // 039 FR-8: 超上限只裁**最旧**的, 总数照实累加 (展示方据此说明截断)。
        let mut pnl = PnlTracker::default();
        for i in 0..(MAX_CLOSED_TRADES + 10) {
            pnl.record_pnl(dec!(1), ts(i as i64));
        }
        assert_eq!(pnl.closed_trades().len(), MAX_CLOSED_TRADES);
        assert_eq!(pnl.closed_trades_total(), (MAX_CLOSED_TRADES + 10) as u64);
        // 保的是最近那批: 末条时刻 = 最后一次调用。
        assert_eq!(
            pnl.closed_trades().last().map(|c| c.time),
            Some(ts((MAX_CLOSED_TRADES + 9) as i64))
        );
        // 聚合指标不受明细上限影响。
        assert_eq!(pnl.realized_pnl(), Decimal::from(MAX_CLOSED_TRADES + 10));
    }

    #[test]
    fn test_net_pnl_with_fees() {
        let mut pnl = PnlTracker::default();
        pnl.record_fill(&sample_fill(OrderSide::Buy, dec!(2)));
        pnl.record_pnl(dec!(200), ts(1));
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
