//! `ricow_engine` — headless 核心: 命令分发 / 行情循环 / 策略装载 / 回测执行 / 密钥加载。

mod backtest_runner;
mod command;
mod confirm;
mod live;
mod loader;
mod market;
mod nasdaq;
mod notify;
mod strategy;
mod us_tickers;

pub use backtest_runner::{
    build_daily_ticks, build_interval_ticks, run_backtest, run_portfolio_backtest,
};
pub use command::{Engine, RunOutcome, StopReason, StopRequest, StopSignal};
pub use confirm::{approve, consume, create_preview, get_preview, reject};
pub use live::{
    check_clock_skew, clock_align_guidance, dry_run_gate, dry_run_initial_cash,
    liquidation_distance, live_gate, plan_cleanup, residual_owned, risk_gate, skew_ms,
    CleanupOutcome, CleanupPlan, ClockVerdict, LiveGate, OnceGate, RiskGate, DEFAULT_DRY_RUN_CASH,
    DEFAULT_MIN_DRY_RUN_HOURS, RISK_DISCLOSURE,
};
pub use loader::load_strategy;
pub use market::{fetch_orderbook, subscribe_orderbook, to_orderbook};
pub use nasdaq::{parse_historical, NasdaqClient};
pub use notify::{EventKind, Notifier, NotifyConfig, NotifyEvent};
/// 订单号归属判定 (011 D6): 实现见 `ricow_strategy::align`, 此处转发便于引擎/CLI 直接用。
pub use ricow_strategy::{is_owned, ownership_prefix};
pub use strategy::{create_strategy, execute_strategy, extract_code};
pub use us_tickers::{is_leveraged, us_ticker_of, AssetClass, BstockMap, BSTOCK_MAP};
