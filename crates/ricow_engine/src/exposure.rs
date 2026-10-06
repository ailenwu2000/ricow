//! 跨策略组合敞口只读聚合 (037 P0-C / FR-3)。
//!
//! **为什么需要它** (背景见 `specs/research/framework-vs-commercial-2026-10.md` §四 P0-C):
//! daemon 可以同时托管 N 个策略进程, 但没有任何地方能看到"合起来"的敞口 —— 每个策略各自
//! 下单、各自记账, 多策略场景下**总净头寸是盲区**。两个策略在同一标的上反向持仓时尤其危险:
//! 单看任一策略都正常, 合起来却是自相对冲 —— 两边的手续费一分不少地付了两遍。
//!
//! 本模块只做**只读聚合**, 刻意不做任何判断:
//! - **只展示, 不拦截** (宪法: 平台不做投资判断) —— 没有写路径、没有新的确认面;
//! - 数据源 = 本地库的 `positions` / `orders` 两张表 (与 026 同口径: 本地库是唯一来源),
//!   **不直连交易所** —— 这条命令不需要密钥, 也不该因为网络问题而给不出答案;
//! - 聚合键 = `(标的, 模式)`: **绝不跨模式相加** —— dry_run 的持仓是模拟出来的, 把它和实盘
//!   的数字加在一起, 会得到一个看似精确、实则错误的"总敞口"; 那比不给数字更危险。
//!
//! **刻意不做跨交易对的总额汇总**: 不同交易对的报价资产未必一致 (USDT / USDC / BTC …),
//! 相加无意义 —— 宁缺勿错, 由展示层逐 (标的, 模式) 给出总名义。
//!
//! 口径:
//! - 头寸 `size` 已是**带符号净额** (多正空负), 由 `command::position_row_of` 落库时算好
//!   (合约按 buy 加 / sell 减; 现货只有 buy 一条) —— 本模块不再重算方向;
//! - 总名义 = `Σ |size| × entry_price`, 用 **开仓均价估算**(本地库没有实时价);
//!   某行有头寸但 `entry_price = 0`(本地未记录) 时, 该行的名义无法计入 → 置
//!   [`PairExposure::notional_incomplete`] 让展示层如实标注"被低估", 不假装数字是完整的;
//! - 挂单 = `orders.status ∈ {open, partially_filled}` 的**未终结**订单, 且只覆盖调用方
//!   传入的那一段订单 (展示层须如实说明"只统计了最近 N 条")。

use std::collections::BTreeMap;

use ricow_strategy::{OrderRecord, PositionRecord};
use rust_decimal::Decimal;

/// 本地库订单状态里"未终结"的两个值。
///
/// `orders.status` 由 `OrderStatus::to_string()` 写入 (`open` / `partially_filled` /
/// `filled` / `cancelled` / `rejected` / `expired`)。
///
/// **唯一判定来源**: CLI 的挂单视图与本模块共用它, 避免两处各抄一份白名单而漂移
/// (漂移的后果是"组合视图说有挂单、挂单视图说没有")。
pub fn is_open_status(status: &str) -> bool {
    matches!(status, "open" | "partially_filled")
}

/// 视图过滤条件 —— 只决定"看哪些行", 不改变任何状态 (本模块本就无状态)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExposureFilter {
    /// 只看某个标的 (大小写不敏感); `None` = 全部。
    pub pair: Option<String>,
    /// 只看某个模式 (`live` / `demo` / `dry_run`, 大小写不敏感); `None` = 全部。
    pub mode: Option<String>,
}

impl ExposureFilter {
    /// 该 (标的, 模式) 是否在视野内。
    pub fn matches(&self, pair: &str, mode: &str) -> bool {
        let pair_ok = self.pair.as_deref().is_none_or(|p| p.eq_ignore_ascii_case(pair));
        let mode_ok = self.mode.as_deref().is_none_or(|m| m.eq_ignore_ascii_case(mode));
        pair_ok && mode_ok
    }

    /// 是否有任何筛选 (展示层据此决定要不要打印"筛选:"那行)。
    pub fn is_active(&self) -> bool {
        self.pair.is_some() || self.mode.is_some()
    }
}

/// 单个 (`策略` × `标的` × `模式`) 的明细行。
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyExposure {
    pub strategy_id: String,
    pub pair: String,
    pub mode: String,
    /// 带符号净头寸 (多正空负)。
    pub size: Decimal,
    /// 开仓均价 (原样带出; `0` = 本地未记录, 不猜)。
    pub entry_price: Decimal,
    /// 持仓与订单里较新的那个时间戳 (毫秒); 都缺则为 0。
    pub updated_at: i64,
    /// 该组合下的未终结挂单数。
    pub open_orders: usize,
}

impl StrategyExposure {
    /// 该行是否"有敞口": 头寸非零 或 有挂单。
    pub fn is_active(&self) -> bool {
        !self.size.is_zero() || self.open_orders > 0
    }
}

/// 单个 (`标的` × `模式`) 的合计行。
#[derive(Debug, Clone, PartialEq)]
pub struct PairExposure {
    pub pair: String,
    pub mode: String,
    /// 净头寸 = Σ `size` (多空自动对冲后的结果)。
    pub net_size: Decimal,
    /// 多头合计 = Σ `max(size, 0)`。
    pub long_size: Decimal,
    /// 空头合计 = Σ `min(size, 0)` (负值)。
    pub short_size: Decimal,
    /// 总名义 (按开仓均价**估算**) = Σ `|size| × entry_price`。
    pub gross_notional: Decimal,
    /// `gross_notional` 是否**低估**: 存在头寸非零但开仓均价为 0 的明细行。
    pub notional_incomplete: bool,
    /// 有敞口的策略数 (头寸非零 或 有挂单)。
    pub strategies: usize,
    /// 未终结挂单数 (跨策略合计)。
    pub open_orders: usize,
    /// 该组合下最近一次变动时间 (毫秒)。
    pub updated_at: i64,
}

/// 聚合结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExposureView {
    /// 逐 (标的, 模式) 合计, 排序 = (标的, 模式)。
    pub pairs: Vec<PairExposure>,
    /// 逐 (策略, 标的, 模式) 明细 (视图过滤后), 排序 = (标的, 模式, 策略)。
    pub rows: Vec<StrategyExposure>,
    /// 实际参与聚合的持仓行数 (含已平仓的 `size = 0` 行)。
    pub positions_scanned: usize,
    /// 实际参与聚合的订单条数 —— 挂单数**只覆盖**这些订单。
    pub orders_scanned: usize,
    /// 其中未终结的订单总数。
    pub open_orders_total: usize,
}

impl ExposureView {
    /// 无任何 (标的, 模式) 组合 = 本地库既无持仓也无挂单。
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// 有敞口的明细行数 (头寸非零 或 有挂单) —— 用于判断"是否只有一堆已平仓的陈旧行"。
    pub fn active_rows(&self) -> usize {
        self.rows.iter().filter(|r| r.is_active()).count()
    }
}

/// 只读聚合 (无筛选)。
pub fn aggregate(positions: &[PositionRecord], orders: &[OrderRecord]) -> ExposureView {
    aggregate_with(positions, orders, &ExposureFilter::default())
}

/// 只读聚合 (带筛选)。
///
/// 纯函数: 不改入参、不做 IO、无副作用 —— 同一份输入必然得到同一份输出。
pub fn aggregate_with(
    positions: &[PositionRecord],
    orders: &[OrderRecord],
    filter: &ExposureFilter,
) -> ExposureView {
    let mut view = ExposureView::default();

    // 明细用 `BTreeMap` 而非 `HashMap`: 输出顺序必须稳定 (HashMap 迭代顺序在进程间不固定,
    // 会让"同一份库两次跑出不同顺序", 也做不了确定性单测)。键序 = (标的, 模式) → 策略。
    let mut rows: BTreeMap<(String, String), BTreeMap<String, StrategyExposure>> = BTreeMap::new();

    for p in positions {
        if !filter.matches(&p.pair, &p.mode) {
            continue;
        }
        view.positions_scanned += 1;
        let row = rows
            .entry((p.pair.clone(), p.mode.clone()))
            .or_default()
            .entry(p.strategy_id.clone())
            .or_insert_with(|| StrategyExposure {
                strategy_id: p.strategy_id.clone(),
                pair: p.pair.clone(),
                mode: p.mode.clone(),
                size: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                updated_at: 0,
                open_orders: 0,
            });
        // 同一 (策略, 标的, 模式) 在表里按主键只应有一行(upsert); 真出现多行也不累加 ——
        // `positions` 是"当前持仓"快照不是流水, 取最新那条才是语义正确的做法。
        if p.updated_at >= row.updated_at {
            row.size = p.size;
            row.entry_price = p.entry_price;
        }
        row.updated_at = row.updated_at.max(p.updated_at);
    }

    for o in orders {
        if !filter.matches(&o.pair, &o.mode) {
            continue;
        }
        view.orders_scanned += 1;
        if !is_open_status(&o.status) {
            continue;
        }
        view.open_orders_total += 1;
        let row = rows
            .entry((o.pair.clone(), o.mode.clone()))
            .or_default()
            .entry(o.strategy_id.clone())
            .or_insert_with(|| StrategyExposure {
                strategy_id: o.strategy_id.clone(),
                pair: o.pair.clone(),
                mode: o.mode.clone(),
                size: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                updated_at: 0,
                open_orders: 0,
            });
        row.open_orders += 1;
        row.updated_at = row.updated_at.max(o.updated_at);
    }

    for ((pair, mode), by_strategy) in rows {
        let mut agg = PairExposure {
            pair,
            mode,
            net_size: Decimal::ZERO,
            long_size: Decimal::ZERO,
            short_size: Decimal::ZERO,
            gross_notional: Decimal::ZERO,
            notional_incomplete: false,
            strategies: 0,
            open_orders: 0,
            updated_at: 0,
        };
        for row in by_strategy.values() {
            agg.net_size += row.size;
            if row.size > Decimal::ZERO {
                agg.long_size += row.size;
            } else if row.size < Decimal::ZERO {
                agg.short_size += row.size;
            }
            if !row.size.is_zero() {
                if row.entry_price.is_zero() {
                    // 有头寸却没成本价 → 这一块名义算不出来, 如实标记"被低估"。
                    agg.notional_incomplete = true;
                } else {
                    agg.gross_notional += row.size.abs() * row.entry_price;
                }
            }
            if row.is_active() {
                agg.strategies += 1;
            }
            agg.open_orders += row.open_orders;
            agg.updated_at = agg.updated_at.max(row.updated_at);
            view.rows.push(row.clone());
        }
        view.pairs.push(agg);
    }

    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn pos(
        strategy: &str,
        pair: &str,
        mode: &str,
        size: Decimal,
        entry: Decimal,
        at: i64,
    ) -> PositionRecord {
        PositionRecord {
            strategy_id: strategy.to_string(),
            pair: pair.to_string(),
            size,
            entry_price: entry,
            mode: mode.to_string(),
            updated_at: at,
        }
    }

    fn order(strategy: &str, pair: &str, mode: &str, status: &str, at: i64) -> OrderRecord {
        OrderRecord {
            strategy_id: strategy.to_string(),
            exchange_order_id: format!("EX-{strategy}-{at}"),
            client_order_id: format!("{strategy}-{at}"),
            pair: pair.to_string(),
            side: "buy".to_string(),
            price: dec!(3000),
            size: dec!(1),
            filled_size: Decimal::ZERO,
            status: status.to_string(),
            mode: mode.to_string(),
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn test_is_open_status_covers_only_unsettled() {
        assert!(is_open_status("open"));
        assert!(is_open_status("partially_filled"));
        for closed in ["filled", "cancelled", "rejected", "expired", "", "OPEN"] {
            assert!(!is_open_status(closed), "{closed} 不该算未终结");
        }
    }

    #[test]
    fn test_empty_input_yields_empty_view() {
        let v = aggregate(&[], &[]);
        assert!(v.is_empty());
        assert_eq!(v.positions_scanned, 0);
        assert_eq!(v.orders_scanned, 0);
        assert_eq!(v.open_orders_total, 0);
        assert!(v.rows.is_empty());
    }

    #[test]
    fn test_net_size_aggregates_across_strategies() {
        // 两个策略同向 → 净 = 相加
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1.5), dec!(2000), 10),
            pos("b", "ETHUSDT", "live", dec!(0.5), dec!(2400), 20),
        ];
        let v = aggregate(&ps, &[]);
        let e = &v.pairs[0];
        assert_eq!(e.net_size, dec!(2.0));
        assert_eq!(e.long_size, dec!(2.0));
        assert_eq!(e.short_size, Decimal::ZERO);
        assert_eq!(e.strategies, 2);
        // 总名义 = |1.5|*2000 + |0.5|*2400 = 3000 + 1200
        assert_eq!(e.gross_notional, dec!(4200));
        assert!(!e.notional_incomplete);
        assert_eq!(e.updated_at, 20, "取最近一次变动");
    }

    #[test]
    fn test_hedged_pair_nets_to_zero_but_gross_notional_remains() {
        // **本模块存在的核心理由**: 两策略反向 → 净头寸 0, 但两边手续费都真实发生了。
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1), dec!(2000), 10),
            pos("b", "ETHUSDT", "live", dec!(-1), dec!(2000), 10),
        ];
        let v = aggregate(&ps, &[]);
        let e = &v.pairs[0];
        assert_eq!(e.net_size, Decimal::ZERO, "净头寸对冲归零");
        assert_eq!(e.long_size, dec!(1));
        assert_eq!(e.short_size, dec!(-1));
        assert_eq!(e.gross_notional, dec!(4000), "名义敞口并不为零 —— 正是盲区所在");
    }

    #[test]
    fn test_modes_are_never_summed_together() {
        // dry_run 是模拟的: 与实盘相加会造出"看似精确实则错误"的总敞口, 必须分开成两行。
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1), dec!(2000), 10),
            pos("b", "ETHUSDT", "dry_run", dec!(1), dec!(2000), 10),
        ];
        let v = aggregate(&ps, &[]);
        assert_eq!(v.pairs.len(), 2, "同一标的不同模式 = 两行");
        for e in &v.pairs {
            assert_eq!(e.net_size, dec!(1), "各自只算自己模式的 1, 不得变成 2");
            assert_eq!(e.gross_notional, dec!(2000));
        }
        let modes: Vec<&str> = v.pairs.iter().map(|e| e.mode.as_str()).collect();
        assert_eq!(modes, vec!["dry_run", "live"], "排序稳定 = (标的, 模式)");
    }

    #[test]
    fn test_open_orders_counted_and_pair_appears_without_position() {
        // 只挂了限价单、还没成交 → 没有持仓行, 但这一对必须出现在组合视图里 (挂单也是敞口)。
        let os = [
            order("a", "BTCUSDT", "live", "open", 10),
            order("a", "BTCUSDT", "live", "partially_filled", 11),
            order("a", "BTCUSDT", "live", "filled", 12),
            order("b", "BTCUSDT", "live", "cancelled", 13),
            order("b", "BTCUSDT", "live", "open", 14),
        ];
        let v = aggregate(&[], &os);
        assert_eq!(v.orders_scanned, 5);
        assert_eq!(v.open_orders_total, 3, "只数 open / partially_filled");
        let e = &v.pairs[0];
        assert_eq!(e.pair, "BTCUSDT");
        assert_eq!(e.open_orders, 3);
        assert_eq!(e.strategies, 2, "有挂单的策略也算参与");
        assert_eq!(e.net_size, Decimal::ZERO);
        assert_eq!(v.active_rows(), 2);
    }

    #[test]
    fn test_notional_incomplete_flagged_when_entry_price_unknown() {
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1), Decimal::ZERO, 10),
            pos("b", "ETHUSDT", "live", dec!(1), dec!(100), 10),
        ];
        let v = aggregate(&ps, &[]);
        let e = &v.pairs[0];
        assert!(e.notional_incomplete, "有头寸无成本价 → 必须标记名义被低估");
        assert_eq!(e.gross_notional, dec!(100), "只有能算的那部分被计入");
    }

    #[test]
    fn test_flat_position_still_listed_in_detail_but_not_counted_active() {
        // 已平仓行 (size = 0) 保留在明细里 (如实呈现本地库内容), 但不计入"有敞口的策略数"。
        let ps = [
            pos("a", "ETHUSDT", "live", Decimal::ZERO, Decimal::ZERO, 10),
            pos("b", "ETHUSDT", "live", dec!(2), dec!(100), 11),
        ];
        let v = aggregate(&ps, &[]);
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.active_rows(), 1);
        assert_eq!(v.pairs[0].strategies, 1);
        assert_eq!(v.pairs[0].net_size, dec!(2));
    }

    #[test]
    fn test_filter_by_pair_and_mode_is_case_insensitive() {
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1), dec!(100), 10),
            pos("b", "BTCUSDT", "live", dec!(1), dec!(100), 10),
            pos("c", "ETHUSDT", "demo", dec!(1), dec!(100), 10),
        ];
        let by_pair =
            aggregate_with(&ps, &[], &ExposureFilter { pair: Some("ethusdt".into()), mode: None });
        assert_eq!(by_pair.pairs.len(), 2, "标的过滤下 ETHUSDT 的 live/demo 两行都在");
        assert_eq!(by_pair.positions_scanned, 2, "扫描条数须反映**过滤后**参与聚合的行数");

        let both = aggregate_with(
            &ps,
            &[],
            &ExposureFilter { pair: Some("ETHUSDT".into()), mode: Some("LIVE".into()) },
        );
        assert_eq!(both.pairs.len(), 1);
        assert_eq!(both.pairs[0].mode, "live");
        assert_eq!(both.positions_scanned, 1);
        assert_eq!(both.pairs[0].gross_notional, dec!(100));
    }

    #[test]
    fn test_filter_does_not_hide_scale_of_scan() {
        // 过滤后没有命中 → 空视图 (展示层据此说"该筛选下无可展示内容"), 而不是报错。
        let ps = [pos("a", "ETHUSDT", "live", dec!(1), dec!(100), 10)];
        let v =
            aggregate_with(&ps, &[], &ExposureFilter { pair: Some("DOGEUSDT".into()), mode: None });
        assert!(v.is_empty());
        assert_eq!(v.positions_scanned, 0);
        assert!(!ExposureFilter::default().is_active());
        assert!(ExposureFilter { pair: Some("X".into()), mode: None }.is_active());
    }

    #[test]
    fn test_rows_sorted_by_pair_mode_strategy() {
        let ps = [
            pos("z", "ETHUSDT", "live", dec!(1), dec!(1), 1),
            pos("a", "BTCUSDT", "live", dec!(1), dec!(1), 1),
            pos("m", "ETHUSDT", "live", dec!(1), dec!(1), 1),
        ];
        let v = aggregate(&ps, &[]);
        let got: Vec<(&str, &str)> =
            v.rows.iter().map(|r| (r.pair.as_str(), r.strategy_id.as_str())).collect();
        assert_eq!(
            got,
            vec![("BTCUSDT", "a"), ("ETHUSDT", "m"), ("ETHUSDT", "z")],
            "顺序必须稳定: (标的, 模式, 策略)"
        );
    }

    #[test]
    fn test_duplicate_position_row_takes_latest_not_sum() {
        // `positions` 是"当前持仓"快照不是流水: 万一同键出现多行, 取最新那条才是语义正确的,
        // 累加会凭空放大敞口。
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1), dec!(100), 10),
            pos("a", "ETHUSDT", "live", dec!(3), dec!(200), 20),
        ];
        let v = aggregate(&ps, &[]);
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.pairs[0].net_size, dec!(3));
        assert_eq!(v.pairs[0].gross_notional, dec!(600));
    }

    #[test]
    fn test_aggregate_is_pure_repeated_calls_identical() {
        let ps = [pos("a", "ETHUSDT", "live", dec!(1), dec!(100), 10)];
        let os = [order("a", "ETHUSDT", "live", "open", 11)];
        assert_eq!(aggregate(&ps, &os), aggregate(&ps, &os));
        assert_eq!(aggregate(&ps, &os), aggregate_with(&ps, &os, &ExposureFilter::default()));
    }
}
