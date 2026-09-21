//! 策略生命周期 trait。

use ricow_core::{Kline, OrderFill, OrderRequest, OrderUpdate};

use crate::context::Context;
use crate::lua::OrderIntents;
use crate::series::{SeriesDecl, SeriesInfo};
use crate::timers::TimerDecl;

/// 策略生命周期 trait。
///
/// 所有方法提供默认空实现, 策略只需覆盖关心的回调。
///
/// 7 个回调 (028 T014): `on_init` / `on_bar` / `on_quote` / `on_timer` / `on_tick` /
/// `on_fill` / `on_stop`。其中 `on_tick` 是**盘口驱动**的历史名字, `on_quote` 是同一语义的
/// 新名字; 引擎按"实现了哪个"选路径(`has_callback`), 两者不必都实现。
/// `on_bar` / `on_quote` / `on_timer` / `on_tick` 都可以返回订单数组 —— 同一条出口。
pub trait Strategy: Send + Sync {
    fn on_init(&mut self, _ctx: &mut dyn Context) {}

    fn on_tick(&mut self, _ctx: &mut dyn Context) -> Vec<OrderRequest> {
        vec![]
    }

    /// 序列收盘驱动 (028 FR-005): `series` = 序列描述(id / 来源 / 标的 / 周期 / 口径),
    /// `bar` = 该根**已收盘** K 线 —— 策略据此区分多条序列。
    ///
    /// 无前视由引擎保证: 只有 `close_time` 已到的 bar 才派发(FR-013)。
    fn on_bar(
        &mut self,
        _ctx: &mut dyn Context,
        _series: &SeriesInfo,
        _bar: &Kline,
    ) -> Vec<OrderRequest> {
        vec![]
    }

    /// 盘口驱动 (028 T014): 语义与 `on_tick` 相同, 新名字。
    fn on_quote(&mut self, _ctx: &mut dyn Context, _pair: &str) -> Vec<OrderRequest> {
        vec![]
    }

    /// 定时器驱动 (028 T014/D5): `label` = 策略声明的节奏标签(如 `"30s"` / `"09:35"`)。
    fn on_timer(&mut self, _ctx: &mut dyn Context, _label: &str) -> Vec<OrderRequest> {
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

    /// 实现里是否定义了某个回调(引擎据此选择派发路径)。
    ///
    /// 默认实现 = 只有 `on_tick`(既有策略的行为不变); Lua 策略覆写为"脚本里有没有这个函数"。
    fn has_callback(&self, name: &str) -> bool {
        name == "on_tick"
    }

    /// 策略声明的数据序列 (028 FR-002/FR-008)。
    ///
    /// 默认空 = **旧路径**(引擎按 `config.pair` 喂 `ctx:klines`); 非空 = 引擎按声明装配
    /// 数据面(序列装载 / `on_bar` 驱动 / 定时器), 不再要求策略"填空"。
    fn declarations(&self) -> Vec<SeriesDecl> {
        Vec::new()
    }

    /// 需要引擎**驱动**(推 `on_bar`)的序列 (028 FR-004)。
    ///
    /// 默认实现 = `declarations()` 里 `drive = true` 的那些 —— 只声明句柄(自己按需读)的
    /// 序列不会被推进, 因此不会白占内存、也不会在策略没要的地方回调。
    fn driving_declarations(&self) -> Vec<SeriesDecl> {
        self.declarations().into_iter().filter(|d| d.drive).collect()
    }

    /// 盘口订阅声明 (028 FR-023): 策略要盯的 pair 集合。
    fn quote_subscriptions(&self) -> Vec<String> {
        Vec::new()
    }

    /// 定时器声明 (028 FR-006): 策略自定的节奏(`secs` 相对 / `at="HH:MM"` 绝对)。
    ///
    /// 回测按虚拟时钟、实盘按真实时钟触发, 调度口径同一份实现(见 `TimerScheduler`)。
    fn timer_declarations(&self) -> Vec<TimerDecl> {
        Vec::new()
    }

    /// 取走主动订单意图 (`ctx:place_order` / `ctx:cancel_order`), 取后清空 (FR-002)。
    fn take_intents(&mut self) -> OrderIntents {
        OrderIntents::default()
    }

    /// 置/清某条序列的 `stale` 位 (028 FR-016): 引擎增量取数失败 → true, 恢复 → false。
    ///
    /// 策略侧 `s:stale()` 读它, 自己决定是收敛还是继续(平台不替策略做这个判断)。
    fn mark_series_stale(&mut self, _id: &str, _stale: bool) {}
}
