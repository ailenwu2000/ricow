//! `ricow_strategy` — 策略引擎: Lua 策略运行时 / 调度 / 风控 / 回测 / 本地存储 / PnL。
//!
//! 策略层统一 Lua: 内置脚本(shannon_grid 策略样板 + executors/ 执行模式示例)与用户策略
//! 均为 Lua 脚本, 参考实现见 `strategies/builtin/`, API 规范见 `specs/lua-api.md`。

mod align;
mod backtest;
mod config;
mod context;
mod db;
mod exec;
mod fee;
mod indicators_api;
pub mod lua;
mod lua_sandbox;
mod metrics;
mod name;
mod pnl;
mod risk;
mod scheduler;
mod strategy;

pub use align::{
    align_order, align_price_to_tick, floor_to_step, is_owned, ownership_prefix,
    prepare_live_order, AlignedOrder, MAX_CLIENT_ORDER_ID_LEN,
};
pub use backtest::{BacktestContext, BacktestReport, HedgeSides};
pub use config::{BacktestParams, BacktestToml, ConfigValue, RiskConfig, StrategyConfig};
pub use context::{Context, DryRunContext, LiveContext};
pub use db::{Database, FillRecord, PreviewRecord};
pub use fee::FeeModel;
pub use lua::{validate_lua, validate_script_source, LuaStrategy};
pub use name::{
    prefix_conflict, suggest_strategy_name, validate_strategy_name, MAX_STRATEGY_NAME_LEN,
};
pub use pnl::PnlTracker;
pub use risk::{
    MaxDailyLoss, MaxPositionLimit, MaxSlippage, MinOrderSize, OrderRateLimit, RiskEngine,
    RiskError, RiskRule, RiskSettings,
};
pub use scheduler::StrategyScheduler;
pub use strategy::Strategy;

#[cfg(test)]
mod builtin_tests;
