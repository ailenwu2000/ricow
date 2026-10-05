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

    /// 取走策略运行期产生的日志 (`ctx:log` / `print`), 供调用方在回测结果里透出。
    ///
    /// 动机 (2026-10-05): 策略自检 (如缺必填参数 `start_price` 的 `[FATAL]` 停机) 此前只写进
    /// tracing, Web 回测详情完全看不到 —— 用户拿到"0 成交"却不知为何。取走后引擎清零缓冲区
    /// (take 语义, 不重复上报)。默认空 —— 无日志的策略 (Rust 实现) 无需覆盖。
    fn take_logs(&mut self) -> Vec<String> {
        Vec::new()
    }
}
