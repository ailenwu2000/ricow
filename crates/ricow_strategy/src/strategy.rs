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

    fn on_fill(&mut self, _ctx: &mut dyn Context, _fill: OrderFill) {}

    fn on_order_update(&mut self, _ctx: &mut dyn Context, _update: OrderUpdate) {}

    fn on_stop(&mut self, _ctx: &mut dyn Context) {}

    /// 是否实现了停机清理 (`on_stop`)。
    ///
    /// 引擎据此决定是"已执行清理"还是"提示用户手工处理"; 默认 false (无清理)。
    fn has_on_stop(&self) -> bool {
        false
    }
}
