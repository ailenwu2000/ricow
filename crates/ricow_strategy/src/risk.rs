//! 风控引擎: 下单前的硬检查规则。
//!
//! 规则分两类:
//! - **工程护栏**(下单频率上限, 默认 100/s): **默认启用** —— 防程序/接口层面失控 (风暴下单被交易所限流封禁);
//! - **静态限额**(最大持仓/单日亏损/最小订单/最大滑点): `[strategy.risk]` **显式配置才启用** ——
//!   平台不替用户定政策, 只执行用户自己写下的政策。
//!
//! **平台不做投资判断**(020-platform-scope-trim): 原两级亏损熔断(连续亏损停开仓 / 峰值回撤停全部新交易)已删除。
//! 盈亏政策属于策略: 策略可用 `ctx:net_pnl()` 与 `ctx:equity()` 自行实现回撤/止损规则。
//!
//! 接入点: `BacktestContext` / `DryRunContext` / `LiveContext` 三条 `place_order` 顶部 — 同一 `RiskEngine`,
//! 同一构造函数(`RiskEngine::from_config`), 保证回测 / Dry Run / 实盘三态语义一致。

use std::collections::{HashMap, VecDeque};
use std::fmt;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ricow_core::{OrderRequest, OrderSide, OrderType};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::config::StrategyConfig;
use crate::context::Context;

/// 默认下单频率上限 (每秒)。参照 NautilusTrader 默认 100/s; 实测依据见
/// `specs/changes/004-risk-guards/tasks.md` T009(内置策略单 tick 最大下单数远低于该值)。
pub const DEFAULT_MAX_ORDERS_PER_SEC: u32 = 100;
/// 频率上限的滑动窗口长度 (毫秒)。
pub const RATE_WINDOW_MS: i64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub enum RiskError {
    MaxPositionExceeded { current: Decimal, limit: Decimal },
    MaxDailyLossExceeded { loss: Decimal, limit: Decimal },
    MinOrderSizeNotMet { notional: Decimal, min: Decimal },
    MaxSlippageExceeded { slippage_bps: u32, max_bps: u32 },
    /// 下单频率超限 (004): 滑动窗口内请求数超过上限。
    OrderRateLimited { limit: u32, count: u32, window_ms: i64 },
}

impl fmt::Display for RiskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskError::MaxPositionExceeded { current, limit } => {
                write!(f, "max position exceeded: current={current}, limit={limit}")
            }
            RiskError::MaxDailyLossExceeded { loss, limit } => {
                write!(f, "max daily loss exceeded: loss={loss}, limit={limit}")
            }
            RiskError::MinOrderSizeNotMet { notional, min } => {
                write!(f, "min order size not met: notional={notional}, min={min}")
            }
            RiskError::MaxSlippageExceeded { slippage_bps, max_bps } => {
                write!(f, "max slippage exceeded: slippage={slippage_bps}bps, max={max_bps}bps")
            }
            RiskError::OrderRateLimited { limit, count, window_ms } => write!(
                f,
                "下单频率超限: {count} 单 / {window_ms}ms > 上限 {limit} 单/秒"
            ),
        }
    }
}

pub trait RiskRule: Send + Sync {
    /// 规则名 (拒单统计与日志用)。
    fn name(&self) -> &'static str;
    /// 检查请求; `Err` = 拒单。
    ///
    /// 可变接收者: 频率窗口等规则需在检查时推进内部状态 (004)。
    fn check(&mut self, req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError>;
}

/// 最大持仓 (名义价值)。
pub struct MaxPositionLimit {
    pub max_notional: Decimal,
}

impl RiskRule for MaxPositionLimit {
    fn name(&self) -> &'static str {
        "max_position"
    }

    fn check(&mut self, req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError> {
        let current_pos = ctx.position(&req.pair);
        let current_notional = current_pos
            .as_ref()
            .and_then(|p| ctx.price(&req.pair).map(|price| p.size * price))
            .unwrap_or(Decimal::ZERO);

        let price = req.price.unwrap_or_else(|| ctx.price(&req.pair).unwrap_or(Decimal::ZERO));
        let new_notional = req.size * price;

        let total_notional = match (&current_pos, req.side) {
            (Some(pos), OrderSide::Buy) if pos.side == OrderSide::Sell => {
                let close_size = req.size.min(pos.size);
                let remaining = req.size - close_size;
                (pos.size - close_size) * price + remaining * price
            }
            (Some(pos), OrderSide::Sell) if pos.side == OrderSide::Buy => {
                let close_size = req.size.min(pos.size);
                let remaining = req.size - close_size;
                (pos.size - close_size) * price + remaining * price
            }
            _ => current_notional + new_notional,
        };

        if total_notional > self.max_notional {
            Err(RiskError::MaxPositionExceeded {
                current: total_notional,
                limit: self.max_notional,
            })
        } else {
            Ok(())
        }
    }
}

/// 单日最大亏损。
pub struct MaxDailyLoss {
    pub max_loss: Decimal,
}

impl RiskRule for MaxDailyLoss {
    fn name(&self) -> &'static str {
        "max_daily_loss"
    }

    fn check(&mut self, _req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError> {
        let net_pnl = ctx.pnl().net_pnl();
        let daily_loss = if net_pnl < Decimal::ZERO { -net_pnl } else { Decimal::ZERO };
        if daily_loss > self.max_loss {
            Err(RiskError::MaxDailyLossExceeded { loss: daily_loss, limit: self.max_loss })
        } else {
            Ok(())
        }
    }
}

/// 最小订单 (名义价值)。
pub struct MinOrderSize {
    pub min_notional: Decimal,
}

impl RiskRule for MinOrderSize {
    fn name(&self) -> &'static str {
        "min_order_size"
    }

    fn check(&mut self, req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError> {
        let price = req.price.unwrap_or_else(|| ctx.price(&req.pair).unwrap_or(Decimal::ZERO));
        let notional = req.size * price;
        if notional < self.min_notional {
            Err(RiskError::MinOrderSizeNotMet { notional, min: self.min_notional })
        } else {
            Ok(())
        }
    }
}

/// 最大滑点 (bps)。
pub struct MaxSlippage {
    pub max_bps: u32,
}

impl RiskRule for MaxSlippage {
    fn name(&self) -> &'static str {
        "max_slippage"
    }

    fn check(&mut self, req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError> {
        if req.order_type == OrderType::Market {
            return Ok(());
        }
        let order_price = match req.price {
            Some(p) => p,
            None => return Ok(()),
        };
        let best_price = match ctx.price(&req.pair) {
            Some(p) => p,
            None => return Ok(()),
        };
        if best_price == Decimal::ZERO || order_price == Decimal::ZERO {
            return Ok(());
        }
        let diff = (order_price - best_price).abs();
        let bps = diff / best_price * Decimal::from(10000);
        let slippage_bps = bps.floor().to_u32().unwrap_or(u32::MAX);
        if slippage_bps > self.max_bps {
            Err(RiskError::MaxSlippageExceeded { slippage_bps, max_bps: self.max_bps })
        } else {
            Ok(())
        }
    }
}

// ---- 004: 下单频率上限 ----

/// 下单频率上限 (滑动窗口 1 秒)。
///
/// 语义: 窗口内(含本次)请求数 > 上限即拒本次请求。窗口按 `ctx.now_utc()` 推进 ——
/// 回测 = 虚拟 tick 时间(可复现), Dry Run / 实盘 = 真实 UTC。计数对全部 pair 共享
/// (防的是策略级风暴), 不区分开仓/平仓。
pub struct OrderRateLimit {
    /// 每秒请求上限 (≥1)。
    pub max_per_sec: u32,
    window: VecDeque<DateTime<Utc>>,
}

impl OrderRateLimit {
    pub fn new(max_per_sec: u32) -> Self {
        Self { max_per_sec: max_per_sec.max(1), window: VecDeque::new() }
    }

    /// 当前窗口内已计入的请求数 (诊断/测试用)。
    pub fn window_len(&self) -> usize {
        self.window.len()
    }
}

impl RiskRule for OrderRateLimit {
    fn name(&self) -> &'static str {
        "order_rate_limit"
    }

    fn check(&mut self, _req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError> {
        let now = ctx.now_utc().unwrap_or_else(Utc::now);
        let cutoff = now - ChronoDuration::milliseconds(RATE_WINDOW_MS);
        while let Some(front) = self.window.front() {
            if *front <= cutoff {
                self.window.pop_front();
            } else {
                break;
            }
        }
        self.window.push_back(now);
        let count = self.window.len() as u32;
        if count > self.max_per_sec {
            Err(RiskError::OrderRateLimited {
                limit: self.max_per_sec,
                count,
                window_ms: RATE_WINDOW_MS,
            })
        } else {
            Ok(())
        }
    }
}

/// 生效的风控参数 (解析后的全量值)。
///
/// 优先级 (004 plan P2): `--param` 的 `risk_*` 键 **>** `[strategy.risk]` TOML **>** 内置默认。
/// 单一解析入口: `RiskEngine::from_config` 与 `StrategyConfig::validate_risk` 都用它, 避免两套机制漂移。
#[derive(Debug, Clone, PartialEq)]
pub struct RiskSettings {
    pub max_orders_per_sec: u32,
    // 静态限额 (None = 该规则不启用; 用户显式配置才生效)。
    pub max_position_notional: Option<f64>,
    pub max_daily_loss_usd: Option<f64>,
    pub min_order_notional: Option<f64>,
    pub max_slippage_bps: Option<u32>,
}

/// 解析后的生效风控参数。
impl RiskSettings {
    /// 解析生效值 (校验前的原始值, 供 validate 报告非法项; 本函数只做"缺省填充")。
    pub fn raw(config: &StrategyConfig) -> RiskSettings {
        let risk = config.risk.clone().unwrap_or_default();
        let rate = config
            .get_i64("risk_max_orders_per_sec")
            .map(|v| v as u32)
            .or(risk.max_orders_per_sec)
            .unwrap_or(DEFAULT_MAX_ORDERS_PER_SEC);
        RiskSettings {
            max_orders_per_sec: rate,
            max_position_notional: risk.max_position_notional,
            max_daily_loss_usd: risk.max_daily_loss_usd,
            min_order_notional: risk.min_order_notional,
            max_slippage_bps: risk.max_slippage_bps,
        }
    }

    /// 参数校验 (spec FR-009): 返回首个非法项的中文说明。
    pub fn validate(config: &StrategyConfig) -> Result<(), String> {
        let s = RiskSettings::raw(config);
        if s.max_orders_per_sec < 1 {
            return Err(format!(
                "risk_max_orders_per_sec={} 非法: 必须 ≥ 1 (0 会让所有下单被拒)",
                s.max_orders_per_sec
            ));
        }
        if let Some(v) = s.max_position_notional {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("risk.max_position_notional={v} 非法: 必须 > 0"));
            }
        }
        if let Some(v) = s.max_daily_loss_usd {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("risk.max_daily_loss_usd={v} 非法: 必须 > 0"));
            }
        }
        if let Some(v) = s.min_order_notional {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("risk.min_order_notional={v} 非法: 必须 > 0"));
            }
        }
        if let Some(v) = s.max_slippage_bps {
            if v == 0 {
                return Err("risk.max_slippage_bps=0 非法: 必须 ≥ 1bps".into());
            }
        }
        Ok(())
    }

    /// 防御性收敛 (绕过校验的构造路径): 保证规则不会因非法值而静默失效。
    fn sanitized(self) -> RiskSettings {
        RiskSettings {
            max_orders_per_sec: self.max_orders_per_sec.max(1),
            ..self
        }
    }
}

/// 风控引擎 — 依次执行所有规则; 记录各规则拒单次数 (诊断/验证用)。
pub struct RiskEngine {
    rules: Vec<Box<dyn RiskRule>>,
    rejections: HashMap<&'static str, u64>,
    last_reason: Option<String>,
}

impl RiskEngine {
    pub fn new() -> Self {
        Self { rules: vec![], rejections: HashMap::new(), last_reason: None }
    }

    pub fn add_rule(&mut self, rule: Box<dyn RiskRule>) {
        self.rules.push(rule);
    }

    /// 按策略配置装配全部护栏 (三条下单路径共用同一入口, 保证三态一致)。
    pub fn from_config(config: &StrategyConfig) -> Self {
        let s = RiskSettings::raw(config).sanitized();
        let mut engine = RiskEngine::new();
        // 静态护栏: 配置了才启用 (与 004 之前语义一致)。
        if let Some(v) = s.max_position_notional {
            if let Some(d) = Decimal::from_f64_retain(v) {
                engine.add_rule(Box::new(MaxPositionLimit { max_notional: d }));
            }
        }
        if let Some(v) = s.max_daily_loss_usd {
            if let Some(d) = Decimal::from_f64_retain(v) {
                engine.add_rule(Box::new(MaxDailyLoss { max_loss: d }));
            }
        }
        if let Some(v) = s.min_order_notional {
            if let Some(d) = Decimal::from_f64_retain(v) {
                engine.add_rule(Box::new(MinOrderSize { min_notional: d }));
            }
        }
        if let Some(v) = s.max_slippage_bps {
            engine.add_rule(Box::new(MaxSlippage { max_bps: v }));
        }
        // 工程护栏: 频率上限 (默认启用)。
        engine.add_rule(Box::new(OrderRateLimit::new(s.max_orders_per_sec)));
        engine
    }

    pub fn check(&mut self, req: &OrderRequest, ctx: &dyn Context) -> Result<(), RiskError> {
        for i in 0..self.rules.len() {
            if let Err(e) = self.rules[i].check(req, ctx) {
                let name = self.rules[i].name();
                *self.rejections.entry(name).or_insert(0) += 1;
                self.last_reason = Some(format!("[{name}] {e}"));
                return Err(e);
            }
        }
        Ok(())
    }

    /// 规则数 (含未启用的静态护栏? 否 —— 仅已装配的)。
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// 累计拒单次数 (全部规则)。
    pub fn rejection_count(&self) -> u64 {
        self.rejections.values().sum()
    }

    /// 各规则拒单次数 (按规则名排序, 便于对照与测试)。
    pub fn rejection_stats(&self) -> Vec<(&'static str, u64)> {
        let mut v: Vec<(&'static str, u64)> = self.rejections.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort();
        v
    }

    /// 最近一次拒单原因 (含规则名)。
    pub fn last_reason(&self) -> Option<&str> {
        self.last_reason.as_deref()
    }
}

impl Default for RiskEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // 最小化的测试 Context 用于风控演算。`now` 控制虚拟时间 (频率窗口 / 结算周期)。
    struct TestCtx {
        price: Decimal,
        pos: Option<ricow_core::Position>,
        pnl: crate::pnl::PnlTracker,
        now: Option<DateTime<Utc>>,
    }

    impl TestCtx {
        fn new() -> Self {
            Self { price: dec!(3000), pos: None, pnl: Default::default(), now: None }
        }

        fn at(ts: i64) -> Self {
            let mut c = Self::new();
            c.now = DateTime::from_timestamp(ts, 0);
            c
        }
    }

    impl Context for TestCtx {
        fn price(&self, _pair: &str) -> Option<Decimal> {
            Some(self.price)
        }
        fn orderbook(&self, _pair: &str) -> Option<ricow_core::OrderBook> {
            None
        }
        fn position(&self, _pair: &str) -> Option<ricow_core::Position> {
            self.pos.clone()
        }
        fn balance(&self, _asset: &str) -> Option<Decimal> {
            None
        }
        fn place_order(
            &mut self,
            _req: OrderRequest,
        ) -> ricow_core::CoreResult<ricow_core::OrderAck> {
            unreachable!()
        }
        fn cancel_order(&mut self, _pair: &str, _order_id: &str) -> ricow_core::CoreResult<()> {
            unreachable!()
        }
        fn update_orderbook(&mut self, _pair: &str, _ob: ricow_core::OrderBook) {}
        fn log(&self, _msg: &str) {}
        fn pnl(&self) -> &crate::pnl::PnlTracker {
            &self.pnl
        }
        fn config(&self) -> &crate::config::StrategyConfig {
            unreachable!()
        }
        fn set_config(&mut self, _config: crate::config::StrategyConfig) {}
        fn now_utc(&self) -> Option<DateTime<Utc>> {
            self.now
        }
    }

    fn buy() -> OrderRequest {
        OrderRequest::new_limit("ETH", OrderSide::Buy, dec!(3000), dec!(0.01))
    }

    #[test]
    fn test_min_order_size() {
        let ctx = TestCtx::new();
        let mut rule = MinOrderSize { min_notional: dec!(100) };
        // notional = 30 < 100
        assert!(rule.check(&buy(), &ctx).is_err());
    }

    #[test]
    fn test_max_slippage() {
        let ctx = TestCtx::new();
        let mut rule = MaxSlippage { max_bps: 10 };
        // 挂单 3030 vs 最优 3000 = 1% = 100bps > 10bps
        let req = OrderRequest::new_limit("ETH", OrderSide::Buy, dec!(3030), dec!(0.01));
        assert!(rule.check(&req, &ctx).is_err());
    }

    // ---- 004: 频率上限 ----

    #[test]
    fn test_rate_limit_rejects_past_limit_within_window() {
        let ctx = TestCtx::at(1_000);
        let mut rule = OrderRateLimit::new(1);
        assert!(rule.check(&buy(), &ctx).is_ok(), "第 1 单放行");
        let err = rule.check(&buy(), &ctx).unwrap_err();
        assert_eq!(
            err,
            RiskError::OrderRateLimited { limit: 1, count: 2, window_ms: RATE_WINDOW_MS },
            "第 2 单(同一秒内)应被拒"
        );
        assert!(err.to_string().contains("下单频率超限"));
    }

    #[test]
    fn test_rate_limit_window_expires() {
        let mut rule = OrderRateLimit::new(1);
        let ctx = TestCtx::at(1_000);
        assert!(rule.check(&buy(), &ctx).is_ok());
        // 1.1s 后窗口内旧记录过期 → 放行。
        let later = TestCtx::at(2_100);
        assert!(rule.check(&buy(), &later).is_ok(), "窗口过期后应恢复放行");
        assert_eq!(rule.window_len(), 1, "过期记录应被清理");
    }

    #[test]
    fn test_rate_limit_default_is_generous() {
        let mut rule = OrderRateLimit::new(DEFAULT_MAX_ORDERS_PER_SEC);
        let ctx = TestCtx::at(1_000);
        for i in 0..DEFAULT_MAX_ORDERS_PER_SEC {
            assert!(rule.check(&buy(), &ctx).is_ok(), "第 {i} 单不应被拒");
        }
        assert!(rule.check(&buy(), &ctx).is_err(), "超过默认上限应被拒");
    }

    #[test]
    fn test_rate_limit_zero_clamped_to_one() {
        let mut rule = OrderRateLimit::new(0);
        let ctx = TestCtx::at(1_000);
        assert!(rule.check(&buy(), &ctx).is_ok());
        assert!(rule.check(&buy(), &ctx).is_err(), "0 收敛为 1: 第 2 单被拒");
    }

    // ---- 004: 引擎装配 ----

    fn cfg_with(risk: Option<crate::config::RiskConfig>, params: Vec<(&str, crate::config::ConfigValue)>) -> StrategyConfig {
        StrategyConfig {
            name: "t".into(),
            strategy_type: "simple".into(),
            enabled: true,
            exchange: "binance".into(),
            params: params.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            risk,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    #[test]
    fn test_from_config_defaults_only_rate_limit() {
        let cfg = cfg_with(None, vec![]);
        let engine = RiskEngine::from_config(&cfg);
        // 020: 默认只装配工程护栏一条 (频率上限); 静态限额未配置不启用。
        assert_eq!(engine.rule_count(), 1);
        assert_eq!(RiskSettings::raw(&cfg).max_orders_per_sec, DEFAULT_MAX_ORDERS_PER_SEC);
    }

    #[test]
    fn test_from_config_static_rules_and_param_priority() {
        use crate::config::{ConfigValue, RiskConfig};
        let risk = RiskConfig {
            max_position_notional: Some(1000.0),
            max_orders_per_sec: Some(5),
            ..Default::default()
        };
        let cfg = cfg_with(Some(risk), vec![("risk_max_orders_per_sec", ConfigValue::Integer(7))]);
        let s = RiskSettings::raw(&cfg);
        assert_eq!(s.max_orders_per_sec, 7, "--param 覆盖 TOML");
        assert_eq!(s.max_position_notional, Some(1000.0));
        assert_eq!(RiskEngine::from_config(&cfg).rule_count(), 2);
    }

    #[test]
    fn test_engine_stops_at_first_reject_and_counts() {
        let mut engine = RiskEngine::new();
        engine.add_rule(Box::new(MinOrderSize { min_notional: dec!(1000) }));
        engine.add_rule(Box::new(OrderRateLimit::new(5)));
        let ctx = TestCtx::at(1_000);
        assert!(engine.check(&buy(), &ctx).is_err());
        assert_eq!(engine.rejection_count(), 1);
        assert_eq!(engine.rejection_stats(), vec![("min_order_size", 1)]);
        assert!(engine.last_reason().unwrap().contains("min_order_size"));
    }

    #[test]
    fn test_settings_validate_rejects_illegal_values() {
        use crate::config::RiskConfig;
        // 频率 0
        let cfg = cfg_with(Some(RiskConfig { max_orders_per_sec: Some(0), ..Default::default() }), vec![]);
        assert!(RiskSettings::validate(&cfg).unwrap_err().contains("risk_max_orders_per_sec"));
        // 合法值通过
        let cfg = cfg_with(None, vec![]);
        assert!(RiskSettings::validate(&cfg).is_ok());
    }
}
