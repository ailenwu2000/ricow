//! `ricow_strategy` — 策略引擎: Lua 策略运行时 / 回测 / 本地存储 / PnL。
//!
//! 平台不做投资风控(019-R5, 2026-09-16): 盈亏/仓位政策由策略自管;
//! 仅保留固定 100 单/秒的 [`OrderGuard`] 工程护栏防程序失控。
//!
//! 策略层统一 Lua: 内置脚本(shannon_spot_grid 香农现货网格 + paired_grid 现货动态非对称网格)与用户策略
//! 均为 Lua 脚本, 参考实现见 `strategies/spot/`, API 规范见 `specs/lua-api.md`。

mod align;
mod backtest;
mod config;
mod context;
mod db;
pub mod events;
mod exec;
mod fee;
/// 038 P1-D: K 线时间连续性校验(数据缺口检测)—— 纯逻辑, 供回测取数后调用。
pub mod gaps;
mod indicators_api;
pub mod lua;
mod lua_sandbox;
mod metrics;
mod multiframe;
mod name;
mod order_guard;
mod pnl;
mod strategy;

pub use align::{
    align_order, align_price_to_tick, floor_to_step, is_owned, ownership_prefix,
    prepare_live_order, AlignedOrder, MAX_CLIENT_ORDER_ID_LEN,
};
pub use backtest::{BacktestContext, BacktestReport, HedgeSides};
pub use config::{BacktestParams, BacktestToml, ConfigValue, StrategyConfig};
pub use context::{Context, DryRunContext, LiveContext};
pub use db::{
    Database, FillRecord, FillWithMode, OrderRecord, PnlSnapshotRecord, PositionRecord,
    PreviewRecord, SqlxResultExt, WebMessageRecord, WebSessionRecord, WEB_ROLE_ASSISTANT,
    WEB_ROLE_HOST, WEB_ROLE_USER,
};
pub use fee::FeeModel;
pub use lua::{validate_lua, validate_script_source, LuaStrategy};
pub use multiframe::{resample_complete, tf_key, tf_ms_of, TfCache};
pub use name::{
    prefix_conflict, suggest_strategy_name, validate_strategy_name, MAX_STRATEGY_NAME_LEN,
};
pub use order_guard::{OrderGuard, OrderGuardError, DEFAULT_MAX_ORDERS_PER_SEC, RATE_WINDOW_MS};
// 039: 平仓明细类型随 `BacktestReport` 公开字段一起出圈 (明细表用它); `max_drawdown` 公开
// 是为了让图表侧的回撤序列能**机械对照**标量口径 (前端曲线的"最深点 = −最大回撤"由测试锁死)。
pub use pnl::{max_drawdown, ClosedTrade, PnlTracker, MAX_CLOSED_TRADES};
pub use strategy::Strategy;

#[cfg(test)]
mod builtin_tests;
