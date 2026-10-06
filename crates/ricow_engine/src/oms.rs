//! 引擎级最小订单登记 (037 P0-B) —— 本实例在**本次会话内**提交的订单及其生命周期。
//!
//! **为什么需要它** (背景见 `specs/research/framework-vs-commercial-2026-10.md` §四 P0-B):
//! 商用框架把"订单状态"作引擎级一等公民(OMS), ricow 此前把它下放给 Lua 脚本 —— 引擎在
//! 下单之后**不再持有任何订单视图**, 于是两个问题无人回答:
//! 1. `orders` 表**只写不读**: 交易所在运行期的订单状态与本地无对账面;
//! 2. 下单**传输层失败**时, 结果未知 —— 交易所可能已收到并挂单, 也可能没有; 此前不落任何
//!    记录, 事后无法回答"这一单到底提交成功没有", 用户只能靠运气。
//!
//! 本模块只做三件事, **刻意不做**完整 FSA/改单/批量下单(那些属 P2 能力边界):
//! - **登记**: 本次会话提交过的每张单 + 生命周期状态;
//! - **未知入账**: 传输失败 → `UnknownSubmission` 单独留痕, 收尾醒目提示用户核对交易所;
//! - **重复单号检测**: 同一 `client_order_id` 在非终态时被再次提交 = 策略 bug 信号。
//!
//! 口径:
//! - **纯内存、无 IO、无 await** —— 它活在实盘主循环里, 不能给交易路径增加任何阻塞面;
//! - 一切失败都**如实降级不抛出**(与 `events.rs` 的旁路哲学一致), 交易比观测重要;
//! - 键 = **最终** `clientOrderId`(已含 `<策略名>-` 归属前缀), 与用户流回报同源, 可精确对账。

use std::collections::HashMap;

use ricow_core::{OrderAck, OrderSide, OrderStatus};
use rust_decimal::Decimal;

/// 订单在**引擎视角**下的生命周期状态。
///
/// 比 `ricow_core::OrderStatus` 多一个 [`OrderState::Unknown`] —— 交易所没有这个概念,
/// 它是"我们不知道交易所那边发生了什么"的如实表达, 是本模块存在的主要理由之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderState {
    /// 已提交, 交易所回报为挂单中 (NEW)。
    Open,
    /// 部分成交。
    PartiallyFilled,
    /// 全部成交 (终态)。
    Filled,
    /// 已撤销 / 已过期 (终态)。
    Canceled,
    /// 被拒 (终态)。
    Rejected,
    /// 结果未知: 提交时传输层失败 (无法判定交易所是否收到; **不是终态**, 需人工核对)。
    Unknown,
}

impl OrderState {
    /// 终态: 不会再变化。
    pub fn is_terminal(self) -> bool {
        matches!(self, OrderState::Filled | OrderState::Canceled | OrderState::Rejected)
    }

    /// 是否"未了结" —— 收尾需要向用户提示(非终态 或 结果未知)。
    pub fn is_unresolved(self) -> bool {
        !self.is_terminal()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OrderState::Open => "open",
            OrderState::PartiallyFilled => "partially_filled",
            OrderState::Filled => "filled",
            OrderState::Canceled => "canceled",
            OrderState::Rejected => "rejected",
            OrderState::Unknown => "unknown",
        }
    }
}

/// `OrderStatus`(交易所口径) → `OrderState`(引擎账目口径)。
///
/// `Expired` 归入 `Canceled`(同为"未成交而终结"), 保留原状态到 `note` 里以免信息丢失。
fn state_of_status(status: OrderStatus) -> (OrderState, Option<&'static str>) {
    match status {
        OrderStatus::Open => (OrderState::Open, None),
        OrderStatus::PartiallyFilled => (OrderState::PartiallyFilled, None),
        OrderStatus::Filled => (OrderState::Filled, None),
        OrderStatus::Cancelled => (OrderState::Canceled, None),
        OrderStatus::Rejected => (OrderState::Rejected, None),
        OrderStatus::Expired => (OrderState::Canceled, Some("交易所口径: expired")),
    }
}

/// 一张订单的登记条目。
#[derive(Debug, Clone, PartialEq)]
pub struct OrderEntry {
    pub client_order_id: String,
    pub exchange_order_id: String,
    pub pair: String,
    pub side: String,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub state: OrderState,
    pub submitted_at_ms: i64,
    pub updated_at_ms: i64,
    /// 最近一次状态变化的说明 (如 expired / 未知原因); 无则 None。
    pub note: Option<String>,
}

/// 一次"结果未知"的提交 (传输层失败, 无 ack 可登记)。
///
/// 单独放一个列表而**不混进主表**: 这类提交没有最终 `clientOrderId`, 无法与用户流回报匹配,
/// 混进去只会污染主表的键空间。
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownSubmission {
    /// 策略侧原始单号 (未注入归属前缀; 仅供人工核对时指认)。
    pub client_order_id_hint: String,
    pub pair: String,
    pub side: String,
    pub size: Decimal,
    pub error: String,
    pub at_ms: i64,
}

/// 登记一条 ack 的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordVerdict {
    /// 新订单 (首次登记该单号)。
    New,
    /// 同一单号**仍在非终态**时被再次提交 —— 策略 bug 信号, 计入 `duplicates`。
    DuplicateLive,
    /// 单号此前已终态, 本次是**复用** (交易所允许); 只留痕不告警。
    DuplicateTerminal,
}

/// 会话订单账目 (收尾输出用)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OmsCounts {
    /// 登记过的订单总数 (含未知提交)。
    pub submitted: u64,
    /// 收尾时仍非终态 (挂单中/部分成交)。
    pub live: u64,
    pub filled: u64,
    pub canceled: u64,
    pub rejected: u64,
    /// 结果未知的提交数 (需用户核对交易所)。
    pub unknown: u64,
    /// 重复单号提交次数。
    pub duplicates: u64,
    /// 需要提示用户的未了结项 = `live + unknown`。
    pub unresolved: u64,
}

/// 本实例**会话内**订单登记表 (纯内存)。
#[derive(Debug, Default)]
pub struct OrderRegistry {
    entries: HashMap<String, OrderEntry>,
    /// 插入顺序, 保证输出稳定 (HashMap 迭代顺序不稳定)。
    order: Vec<String>,
    /// 结果未知的提交。
    unknowns: Vec<UnknownSubmission>,
    submitted: u64,
    duplicates: u64,
}

impl OrderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一条下单 ack。空 `client_order_id`(撤单指令的合成 ack / 拒单模板)一律跳过并返回 `None`。
    pub fn record_ack(&mut self, ack: &OrderAck, now_ms: i64) -> Option<RecordVerdict> {
        if ack.client_order_id.is_empty() {
            // 撤单指令 (OrderAction::CancelPending) 与对齐拒单模板都不带单号, 不是"一张订单"。
            return None;
        }
        self.submitted += 1;
        let (state, note) = state_of_status(ack.status);

        let verdict = match self.entries.get(&ack.client_order_id) {
            Some(prev) if !prev.state.is_terminal() => {
                // 非终态下同号再提交: 交易所会以 DUPLICATE_ORDER 拒掉, 但根因在策略。
                self.duplicates += 1;
                RecordVerdict::DuplicateLive
            }
            Some(_) => RecordVerdict::DuplicateTerminal,
            None => RecordVerdict::New,
        };

        // DuplicateLive: **保留原条目**(先到的那张才是真正挂在交易所的单), 不覆盖。
        if verdict == RecordVerdict::DuplicateLive {
            return Some(verdict);
        }

        let entry = OrderEntry {
            client_order_id: ack.client_order_id.clone(),
            exchange_order_id: ack.exchange_order_id.clone(),
            pair: ack.pair.clone(),
            side: side_str(ack.side),
            size: ack.size,
            filled_size: ack.filled_size,
            state,
            submitted_at_ms: now_ms,
            updated_at_ms: now_ms,
            note: note.map(|s| s.to_string()),
        };
        if !self.entries.contains_key(&ack.client_order_id) {
            self.order.push(ack.client_order_id.clone());
        }
        self.entries.insert(ack.client_order_id.clone(), entry);
        Some(verdict)
    }

    /// 登记一次**结果未知**的提交 (传输层失败)。
    pub fn record_unknown(
        &mut self,
        client_order_id_hint: &str,
        pair: &str,
        side: OrderSide,
        size: Decimal,
        error: &str,
        now_ms: i64,
    ) {
        self.submitted += 1;
        self.unknowns.push(UnknownSubmission {
            client_order_id_hint: client_order_id_hint.to_string(),
            pair: pair.to_string(),
            side: side_str(side),
            size,
            error: error.to_string(),
            at_ms: now_ms,
        });
    }

    /// 应用一次用户流**订单状态回报**。返回 true = 状态确实被更新 (否则为未知单号或终态锁定)。
    ///
    /// 终态锁定: 已终结的订单不被后续回报"复活"(交易所乱序/重放不该改变既成事实)。
    pub fn apply_user_order(
        &mut self,
        client_order_id: &str,
        status: OrderStatus,
        filled_size: Decimal,
        now_ms: i64,
    ) -> bool {
        let Some(e) = self.entries.get_mut(client_order_id) else {
            return false;
        };
        let (next, note) = state_of_status(status);
        if e.state.is_terminal() {
            return false;
        }
        e.state = next;
        if filled_size > e.filled_size {
            e.filled_size = filled_size;
        }
        e.updated_at_ms = now_ms;
        e.note = note.map(|s| s.to_string());
        true
    }

    /// 应用一次**成交回报**: `fill_size` 为**本笔**成交量 (增量, 与 `OrderFill.fill_size` 同口径);
    /// 累加后 ≥ 下单量 → 全部成交。零/负成交量不推进状态 (返回 false)。
    ///
    /// 注意与 [`Self::apply_user_order`] 的区别: 后者拿的是**累计**量 (`OrderUpdate.filled_size`),
    /// 前者是**增量**。两者都不可越界: 增量用加、累计用取大。
    pub fn apply_fill(&mut self, client_order_id: &str, fill_size: Decimal, now_ms: i64) -> bool {
        let Some(e) = self.entries.get_mut(client_order_id) else {
            return false;
        };
        if e.state.is_terminal() || fill_size <= Decimal::ZERO {
            return false;
        }
        e.filled_size += fill_size;
        e.state =
            if e.filled_size >= e.size { OrderState::Filled } else { OrderState::PartiallyFilled };
        e.updated_at_ms = now_ms;
        true
    }

    /// 标记撤销 (引擎侧撤单成功时调用, 如停机清理 / 启动接管)。
    pub fn mark_canceled(&mut self, client_order_id: &str, now_ms: i64) -> bool {
        let Some(e) = self.entries.get_mut(client_order_id) else {
            return false;
        };
        if e.state.is_terminal() {
            return false;
        }
        e.state = OrderState::Canceled;
        e.updated_at_ms = now_ms;
        true
    }

    /// 标记**全部非终态订单**为已撤销, 返回受影响笔数。
    ///
    /// 用于策略的撤单指令 (`OrderAction::CancelPending`): 该指令的语义是"撤销本实例全部挂单"
    /// (`ricow_core::OrderAction` 文档), 但它不回传单号 —— 引擎只能按语义把登记表里所有在途
    /// 条目一并标记为已撤, 否则收尾账目会把它们误报成"仍在挂单"。
    pub fn mark_all_canceled(&mut self, now_ms: i64) -> usize {
        let mut n = 0;
        for e in self.entries.values_mut() {
            if !e.state.is_terminal() {
                e.state = OrderState::Canceled;
                e.updated_at_ms = now_ms;
                n += 1;
            }
        }
        n
    }

    pub fn get(&self, client_order_id: &str) -> Option<&OrderEntry> {
        self.entries.get(client_order_id)
    }

    /// 按插入顺序列出全部条目。
    pub fn entries(&self) -> impl Iterator<Item = &OrderEntry> {
        self.order.iter().filter_map(|k| self.entries.get(k))
    }

    /// 结果未知的提交 (收尾提示用)。
    pub fn unknowns(&self) -> &[UnknownSubmission] {
        &self.unknowns
    }

    /// 收尾时仍**未了结**的单号 (非终态 或 未知), 供停机提示。
    pub fn unresolved_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .entries()
            .filter(|e| e.state.is_unresolved())
            .map(|e| e.client_order_id.clone())
            .collect();
        v.extend(self.unknowns.iter().map(|u| u.client_order_id_hint.clone()));
        v
    }

    pub fn counts(&self) -> OmsCounts {
        let mut c = OmsCounts {
            submitted: self.submitted,
            unknown: self.unknowns.len() as u64,
            duplicates: self.duplicates,
            ..Default::default()
        };
        for e in self.entries.values() {
            match e.state {
                OrderState::Filled => c.filled += 1,
                OrderState::Canceled => c.canceled += 1,
                OrderState::Rejected => c.rejected += 1,
                OrderState::Open | OrderState::PartiallyFilled => c.live += 1,
                // 主表里不应出现 Unknown (它只活在 unknowns 列表); 出现也算未了结, 如实计数。
                OrderState::Unknown => c.unknown += 1,
            }
        }
        c.unresolved = c.live + c.unknown;
        c
    }
}

fn side_str(side: OrderSide) -> String {
    match side {
        OrderSide::Buy => "buy".to_string(),
        OrderSide::Sell => "sell".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn ack(cid: &str, status: OrderStatus, size: Decimal, filled: Decimal) -> OrderAck {
        OrderAck {
            exchange_order_id: format!("EX-{cid}"),
            client_order_id: cid.to_string(),
            pair: "ETHUSDT".into(),
            side: OrderSide::Buy,
            price: dec!(3000),
            size,
            filled_size: filled,
            status,
        }
    }

    #[test]
    fn test_record_ack_maps_status_and_counts() {
        let mut r = OrderRegistry::new();
        assert_eq!(
            r.record_ack(&ack("g-1", OrderStatus::Open, dec!(1), Decimal::ZERO), 100),
            Some(RecordVerdict::New)
        );
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Open);
        assert_eq!(r.get("g-1").unwrap().submitted_at_ms, 100);

        r.record_ack(&ack("g-2", OrderStatus::Filled, dec!(1), dec!(1)), 200);
        r.record_ack(&ack("g-3", OrderStatus::Rejected, dec!(1), Decimal::ZERO), 300);
        let c = r.counts();
        assert_eq!(c.submitted, 3);
        assert_eq!(c.live, 1);
        assert_eq!(c.filled, 1);
        assert_eq!(c.rejected, 1);
        assert_eq!(c.unresolved, 1);
    }

    #[test]
    fn test_record_ack_skips_empty_client_order_id() {
        // 撤单指令的合成 ack (client_order_id 为空) 不是"一张订单", 不得登记
        let mut r = OrderRegistry::new();
        let mut a = ack("", OrderStatus::Cancelled, Decimal::ZERO, Decimal::ZERO);
        a.client_order_id = String::new();
        assert_eq!(r.record_ack(&a, 1), None);
        assert_eq!(r.counts().submitted, 0);
        assert_eq!(r.entries().count(), 0);
    }

    #[test]
    fn test_duplicate_live_is_flagged_and_original_kept() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Open, dec!(1), Decimal::ZERO), 100);
        // 同号再次提交 (非终态) → DuplicateLive, 且**不覆盖**原条目
        let v = r.record_ack(&ack("g-1", OrderStatus::Open, dec!(5), Decimal::ZERO), 150);
        assert_eq!(v, Some(RecordVerdict::DuplicateLive));
        assert_eq!(r.get("g-1").unwrap().size, dec!(1), "先到的那张才是真挂在交易所的单");
        assert_eq!(r.get("g-1").unwrap().submitted_at_ms, 100, "不得被后到的覆盖");
        assert_eq!(r.counts().duplicates, 1);
    }

    #[test]
    fn test_duplicate_after_terminal_is_reuse_not_double_count() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Filled, dec!(1), dec!(1)), 100);
        let v = r.record_ack(&ack("g-1", OrderStatus::Open, dec!(2), Decimal::ZERO), 200);
        assert_eq!(v, Some(RecordVerdict::DuplicateTerminal));
        assert_eq!(r.counts().duplicates, 0, "终态后的复用不算重复告警");
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Open, "复用应作为新单生效");
        assert_eq!(r.get("g-1").unwrap().size, dec!(2));
    }

    #[test]
    fn test_apply_user_order_advances_and_locks_terminal() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Open, dec!(1), Decimal::ZERO), 100);
        assert!(r.apply_user_order("g-1", OrderStatus::PartiallyFilled, dec!(0.4), 200));
        assert_eq!(r.get("g-1").unwrap().state, OrderState::PartiallyFilled);
        assert_eq!(r.get("g-1").unwrap().filled_size, dec!(0.4));
        assert!(r.apply_user_order("g-1", OrderStatus::Filled, dec!(1), 300));
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Filled);
        // 终态锁定: 后续乱序回报不得"复活"
        assert!(!r.apply_user_order("g-1", OrderStatus::Open, dec!(1), 400));
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Filled);
        // 未知单号 → false
        assert!(!r.apply_user_order("nope", OrderStatus::Filled, dec!(1), 500));
    }

    #[test]
    fn test_apply_fill_incremental_until_full() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Open, dec!(10), Decimal::ZERO), 100);
        assert!(r.apply_fill("g-1", dec!(3), 200));
        assert_eq!(r.get("g-1").unwrap().state, OrderState::PartiallyFilled);
        assert_eq!(r.get("g-1").unwrap().filled_size, dec!(3));
        // 增量累加 (不是取大): 第二笔 7 → 累计 10 = 全部成交
        assert!(r.apply_fill("g-1", dec!(7), 300));
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Filled);
        assert_eq!(r.get("g-1").unwrap().filled_size, dec!(10));
        // 已终态: 后续成交不再改变
        assert!(!r.apply_fill("g-1", dec!(1), 400));
        assert_eq!(r.get("g-1").unwrap().filled_size, dec!(10));
        // 零成交量不得把状态推成"部分成交"
        let mut r2 = OrderRegistry::new();
        r2.record_ack(&ack("h-1", OrderStatus::Open, dec!(1), Decimal::ZERO), 1);
        assert!(!r2.apply_fill("h-1", Decimal::ZERO, 2));
        assert_eq!(r2.get("h-1").unwrap().state, OrderState::Open);
        // 未知单号 → false
        assert!(!r2.apply_fill("nope", dec!(1), 3));
    }

    #[test]
    fn test_expired_maps_to_canceled_with_note() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Expired, dec!(1), Decimal::ZERO), 100);
        let e = r.get("g-1").unwrap();
        assert_eq!(e.state, OrderState::Canceled);
        assert!(e.note.as_deref().unwrap().contains("expired"), "原状态须留痕: {:?}", e.note);
    }

    #[test]
    fn test_unknown_captured_separately_and_surfaced() {
        let mut r = OrderRegistry::new();
        r.record_unknown("grid-1", "ETHUSDT", OrderSide::Sell, dec!(0.5), "connect timeout", 777);
        let c = r.counts();
        assert_eq!(c.unknown, 1);
        assert_eq!(c.submitted, 1);
        assert_eq!(c.unresolved, 1);
        assert_eq!(r.unknowns().len(), 1);
        assert_eq!(r.unknowns()[0].error, "connect timeout");
        assert_eq!(r.unknowns()[0].at_ms, 777);
        // 未了结清单须同时含主表未终态与未知提交
        assert_eq!(r.unresolved_ids(), vec!["grid-1".to_string()]);
    }

    #[test]
    fn test_mark_canceled_and_unresolved_clears() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Open, dec!(1), Decimal::ZERO), 100);
        assert_eq!(r.counts().unresolved, 1);
        assert!(r.mark_canceled("g-1", 200));
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Canceled);
        assert_eq!(r.counts().unresolved, 0);
        // 终态后重复标记 → false (幂等)
        assert!(!r.mark_canceled("g-1", 300));
    }

    #[test]
    fn test_entries_iterate_in_insertion_order() {
        let mut r = OrderRegistry::new();
        for cid in ["g-3", "g-1", "g-2"] {
            r.record_ack(&ack(cid, OrderStatus::Open, dec!(1), Decimal::ZERO), 1);
        }
        let got: Vec<&str> = r.entries().map(|e| e.client_order_id.as_str()).collect();
        assert_eq!(got, vec!["g-3", "g-1", "g-2"], "输出顺序须稳定 (按登记顺序)");
    }

    #[test]
    fn test_empty_registry_counts_are_zero() {
        let r = OrderRegistry::new();
        assert_eq!(r.counts(), OmsCounts::default());
        assert!(r.unresolved_ids().is_empty());
    }

    #[test]
    fn test_mark_all_canceled_clears_live_but_keeps_terminal() {
        let mut r = OrderRegistry::new();
        r.record_ack(&ack("g-1", OrderStatus::Open, dec!(1), Decimal::ZERO), 1);
        r.record_ack(&ack("g-2", OrderStatus::PartiallyFilled, dec!(1), dec!(0.3)), 1);
        r.record_ack(&ack("g-3", OrderStatus::Filled, dec!(1), dec!(1)), 1);
        let n = r.mark_all_canceled(500);
        assert_eq!(n, 2, "只影响非终态条目");
        assert_eq!(r.get("g-1").unwrap().state, OrderState::Canceled);
        assert_eq!(r.get("g-2").unwrap().state, OrderState::Canceled);
        assert_eq!(r.get("g-3").unwrap().state, OrderState::Filled, "终态不受影响");
        assert_eq!(r.counts().live, 0);
        assert_eq!(r.counts().canceled, 2);
    }
}
