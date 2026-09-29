//! 策略生命周期 trait。

use ricow_core::{OrderFill, OrderRequest, OrderUpdate};

use crate::context::Context;

/// 策略生命周期 trait。
///
/// 所有方法提供默认空实现, 策略只需覆盖关心的回调。
pub trait Strategy: Send + Sync {
    fn on_init(&mut self, _ctx: &mut dyn Context) {}

    fn on_tick(&mut self, _ctx: &mut dyn Context) -> Vec<OrderRequest> {
        vec![]
    }

    /// 成交回调 (034 事件驱动): 每笔成交入账后立即派发。
    ///
    /// 可返回订单 —— 引擎会立即下单, 且新成交会**再次**触发本回调(有界递归,
    /// 深度上限见引擎 `MAX_FILL_DECISION_DEPTH`), 实现"成交→决策→挂单"零节流闭环。
    /// 返回空(默认)则与旧语义一致: 决策等待下一次 `on_tick`。
    fn on_fill(&mut self, _ctx: &mut dyn Context, _fill: OrderFill) -> Vec<OrderRequest> {
        vec![]
    }

    fn on_order_update(&mut self, _ctx: &mut dyn Context, _update: OrderUpdate) {}

    fn on_stop(&mut self, _ctx: &mut dyn Context) {}

    /// 是否实现了停机清理 (`on_stop`)。
    ///
    /// 引擎据此决定是"已执行清理"还是"提示用户手工处理"; 默认 false (无清理)。
    /// 策略状态快照 (030 断点续接): 引擎在成交后 / 停机时取走并落库, 重启时经
    /// [`Strategy::state_restore`] 原样注回。默认空 —— 无状态策略无需实现。
    fn state_snapshot(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// 注入上次会话保存的状态 (030): 引擎在 `on_init` **之前**调用。默认 no-op。
    fn state_restore(&mut self, _items: Vec<(String, String)>) {}

    fn has_on_stop(&self) -> bool {
        false
    }

    /// 策略是否已置停机标记 (042): 引擎据此派发 `halted` 通知。
    ///
    /// Lua 策略约定: 全局 `fatal = 1` / `halted = 1` (或 `_RICOW_STATE` 同名键 "1")。
    /// 默认 false —— 非 Lua 策略 / 未置标记。
    fn halted(&self) -> bool {
        false
    }

    /// 策略停摆计数 (042): 引擎据此派发 `stall` 通知。
    ///
    /// Lua 策略约定: `_RICOW_STATE["stat_stall_bars"]` (字符串)。默认 None —— 不监控停摆。
    fn stall_bars(&self) -> Option<u64> {
        None
    }
}
