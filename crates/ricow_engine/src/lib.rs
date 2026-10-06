//! `ricow_engine` — headless 核心: 命令分发 / 行情循环 / 策略装载 / 回测执行 / 密钥加载。

mod backtest_runner;
mod command;
mod confirm;
/// 跨策略组合敞口只读聚合 (037 P0-C): 从本地库 `positions`/`orders` 汇总各标的净头寸与挂单数。
pub mod exposure;
mod live;
mod loader;
mod market;
mod market_class;
mod nasdaq;
mod notify;
/// 引擎级最小订单登记 (037 P0-B): 会话内订单生命周期 + 结果未知入账 + 重复单号检测。
pub mod oms;
mod strategy;
mod us_tickers;

pub use backtest_runner::{
    build_daily_ticks, build_interval_ticks, run_backtest, run_portfolio_backtest,
};
pub use command::{Engine, RunMode, RunOutcome, RunTelemetry, StopReason, StopRequest, StopSignal};
pub use confirm::{approve, consume, create_preview, get_preview, reject, PREVIEW_TTL_SECS};
/// 跨策略组合敞口聚合 (037 P0-C): 引擎侧只读账目, CLI/面板共用同一份算法。
pub use exposure::{
    aggregate, aggregate_with, is_open_status, ExposureFilter, ExposureView, PairExposure,
    StrategyExposure,
};
pub use live::{
    check_clock_skew, clock_align_guidance, dry_run_gate, dry_run_initial_cash,
    liquidation_distance, live_gate, must_refuse_start, orphan_query_refuse_message,
    orphan_refuse_message, plan_cleanup, residual_owned, risk_gate, skew_ms, split_owned,
    CleanupOutcome, CleanupPlan, ClockVerdict, LiveGate, OnceGate, RiskGate, DEFAULT_DRY_RUN_CASH,
    DEFAULT_MIN_DRY_RUN_HOURS, RISK_DISCLOSURE,
};
pub use loader::load_strategy;
pub use market::{fetch_orderbook, subscribe_orderbook, to_orderbook};
pub use market_class::{bstock_spot_pool, build_view, filter_view, is_bstock_base, PairsView};
pub use nasdaq::{parse_historical, NasdaqClient};
pub use notify::{EventKind, Notifier, NotifyConfig, NotifyEvent};
/// 会话内订单登记 (037 P0-B): 引擎视角的订单生命周期账目。
pub use oms::{OmsCounts, OrderEntry, OrderRegistry, OrderState, RecordVerdict, UnknownSubmission};
/// 订单号归属判定 (011 D6): 实现见 `ricow_strategy::align`, 此处转发便于引擎/CLI 直接用。
pub use ricow_strategy::{is_owned, ownership_prefix};
pub use strategy::{
    create_strategy, execute_strategy, extract_code, write_strategy_files, DeployedStrategy,
};
pub use us_tickers::{is_leveraged, us_ticker_of, AssetClass, BstockMap, BSTOCK_MAP};
