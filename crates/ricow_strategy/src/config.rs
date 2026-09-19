//! 策略配置: TOML 反序列化 + 参数类型。

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 策略参数值, 支持多种基本类型。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    String(String),
    Float(f64),
    Integer(i64),
    Boolean(bool),
}

impl ConfigValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConfigValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ConfigValue::Float(f) => Some(*f),
            ConfigValue::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ConfigValue::Integer(i) => Some(*i),
            ConfigValue::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_dec(&self) -> Option<Decimal> {
        match self {
            ConfigValue::Float(f) => Decimal::from_f64_retain(*f),
            ConfigValue::Integer(i) => Some(Decimal::from(*i)),
            ConfigValue::String(s) => s.parse::<Decimal>().ok(),
            _ => None,
        }
    }
}

/// 风控参数配置。
///
/// 静态限额(配置了才启用) + 工程护栏(频率上限, 默认启用)。
/// 020 起平台不再有"默认启用的亏损熔断": 盈亏政策属于策略。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RiskConfig {
    pub max_position_notional: Option<f64>,
    pub max_daily_loss_usd: Option<f64>,
    pub min_order_notional: Option<f64>,
    pub max_slippage_bps: Option<u32>,
    /// 下单频率上限 (每秒); 默认 100。
    pub max_orders_per_sec: Option<u32>,
}

/// 策略配置 (TOML 文件反序列化)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub strategy_type: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub exchange: String,
    #[serde(default)]
    pub params: HashMap<String, ConfigValue>,
    #[serde(default)]
    pub risk: Option<RiskConfig>,
    /// DryRun 首次启动时间 (ISO8601)。
    #[serde(default)]
    pub dry_run_started_at: Option<String>,
    /// 是否已通过实盘门禁。
    #[serde(default)]
    pub live_enabled: bool,
    /// 市场类型: "spot" | "futures" (回测与未来实盘共享的策略级语义, 见 specs/backtest.md §五.7)。
    #[serde(default = "default_market")]
    pub market: String,
    /// 持仓模式: "one-way" (单向, 默认) | "hedge" (双向多空并存)。见 specs/backtest.md §五.3。
    #[serde(default = "default_position_mode")]
    pub position_mode: String,
    /// 策略 TOML `[backtest]` 段 (持久默认, 每次回测可被 CLI 覆盖; 三层配置见 specs/backtest.md §三)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backtest: Option<BacktestToml>,
}

fn default_enabled() -> bool {
    true
}

fn default_market() -> String {
    "spot".into()
}

fn default_position_mode() -> String {
    "one-way".into()
}

impl StrategyConfig {
    pub fn from_toml(toml_str: &str) -> Result<Self, toml::de::Error> {
        let raw: RawStrategyToml = toml::from_str(toml_str)?;
        Ok(raw.into_config())
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        let raw = RawStrategyToml {
            strategy: StrategyInner {
                name: self.name.clone(),
                strategy_type: self.strategy_type.clone(),
                enabled: self.enabled,
                exchange: self.exchange.clone(),
                params: self.params.clone(),
                dry_run_started_at: self.dry_run_started_at.clone(),
                live_enabled: self.live_enabled,
                market: self.market.clone(),
                position_mode: self.position_mode.clone(),
            },
            risk: self.risk.clone(),
            backtest: self.backtest.clone(),
        };
        toml::to_string_pretty(&raw)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.params.get(key).and_then(|v| v.as_f64())
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.params.get(key).and_then(|v| v.as_i64())
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.params.get(key).and_then(|v| v.as_bool())
    }

    pub fn get_dec(&self, key: &str) -> Option<Decimal> {
        self.params.get(key).and_then(|v| v.as_dec())
    }

    /// 风控参数校验 (004 FR-009): 非法值在配置装载边界拒绝, 不留到运行期。
    /// 生效值优先级见 `RiskSettings::raw`(--param `risk_*` 键 > `[strategy.risk]` > 默认)。
    pub fn validate_risk(&self) -> Result<(), ricow_core::CoreError> {
        crate::risk::RiskSettings::validate(self).map_err(ricow_core::CoreError::InvalidArgument)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawStrategyToml {
    strategy: StrategyInner,
    #[serde(default)]
    risk: Option<RiskConfig>,
    /// 回测参数持久默认 (可选; 键全 Option, 缺省不覆盖内置默认)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backtest: Option<BacktestToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StrategyInner {
    name: String,
    #[serde(rename = "type")]
    strategy_type: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    exchange: String,
    #[serde(default)]
    params: HashMap<String, ConfigValue>,
    #[serde(default)]
    dry_run_started_at: Option<String>,
    #[serde(default)]
    live_enabled: bool,
    #[serde(default = "default_market")]
    market: String,
    #[serde(default = "default_position_mode")]
    position_mode: String,
}

impl RawStrategyToml {
    fn into_config(self) -> StrategyConfig {
        StrategyConfig {
            name: self.strategy.name,
            strategy_type: self.strategy.strategy_type,
            enabled: self.strategy.enabled,
            exchange: self.strategy.exchange,
            params: self.strategy.params,
            risk: self.risk,
            dry_run_started_at: self.strategy.dry_run_started_at,
            live_enabled: self.strategy.live_enabled,
            market: self.strategy.market,
            position_mode: self.strategy.position_mode,
            backtest: self.backtest,
        }
    }
}

/// 策略 TOML `[backtest]` 段: 回测引擎参数持久默认 (三层配置中间层)。
/// 全字段 Option: 未写的键不覆盖下层 (内置默认)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BacktestToml {
    pub fee_maker_bps: Option<f64>,
    pub fee_taker_bps: Option<f64>,
    pub slippage_bps: Option<f64>,
    pub initial_cash: Option<f64>,
    pub leverage: Option<f64>,
    /// 杠杆上限 (默认 10; 超 10x 需显式放宽 = 知情, 方案 A)。
    pub max_leverage: Option<f64>,
    pub mmr_pct: Option<f64>,
    pub funding_rate_8h: Option<f64>,
}

/// 回测引擎参数 (单次回测的全量有效值, 三层合并后)。
#[derive(Debug, Clone)]
pub struct BacktestParams {
    /// maker 手续费 bps (单边)。
    pub fee_maker_bps: f64,
    /// taker 手续费 bps (单边)。
    pub fee_taker_bps: f64,
    /// 市价单滑点 bps。
    pub slippage_bps: f64,
    /// 初始现金 (quote)。
    pub initial_cash: f64,
    /// 合约杠杆 L (逐仓保证金 = 名义/L)。
    pub leverage: f64,
    /// 杠杆上限 (默认 10; 超限报错, 需显式放宽)。
    pub max_leverage: f64,
    /// 维持保证金率 MMR % (如 2.5 = 2.5%)。
    pub mmr_pct: f64,
    /// 资金费率 / 8h (如 0.0001 = 0.01%)。
    pub funding_rate_8h: f64,
}

/// 内置默认层 (specs/backtest.md §三)。现货费率 = BN 现货 10bps; 合约按 market 分支另定。
impl Default for BacktestParams {
    fn default() -> Self {
        Self {
            fee_maker_bps: 10.0,
            fee_taker_bps: 10.0,
            slippage_bps: 0.0,
            initial_cash: 100_000.0,
            leverage: 1.0,
            max_leverage: 10.0,
            // MMR 默认 1.0%: 内置首档表的未知回落值 (specs/backtest.md §三, 2026-09-05 起;
            // 原 2.5% 为 exchangeInfo 深档误导值, 见 §十一 T7)。CLI 合约路径按 symbol 查表覆盖。
            mmr_pct: 1.0,
            funding_rate_8h: 0.0001,
        }
    }
}

impl BacktestParams {
    /// 校验最终生效的杠杆 (方案 A: 默认上限 10, 超限须显式放宽; L3: ≤0 拒绝, 防除零)。
    /// 调用点: CLI 参数解析后 + Engine::backtest 入口 (params 池为三层合并最终源)。
    pub fn validate_leverage(
        leverage: f64,
        max_leverage: f64,
    ) -> Result<(), ricow_core::CoreError> {
        if !leverage.is_finite() || leverage < 1.0 {
            return Err(ricow_core::CoreError::InvalidArgument(format!(
                "杠杆 {leverage} 非法: 必须 ≥ 1 (0/负值会导致保证金除零)"
            )));
        }
        if leverage > max_leverage {
            return Err(ricow_core::CoreError::InvalidArgument(format!(
                "杠杆 {leverage:.0} 超出上限 {max_leverage:.0}: 超 10x 与长期盈利目标相悖 (强平距离 ≈ 1/杠杆, \
                 125x 下正常波动即爆), 需 --max-leverage 显式放宽 (知情)"
            )));
        }
        Ok(())
    }
}

impl BacktestParams {
    /// 三层合并 (K1): 内置默认 < 策略 TOML `[backtest]` 段 < CLI 覆盖 (`ov` 的 Some 覆盖)。
    /// 合约 (market=futures) 内置费率按 BN USDT-M 标准 (maker 2bps / taker 5bps) 计。
    pub fn resolve(cfg: &StrategyConfig, ov: &BacktestToml) -> Self {
        let mut p = if cfg.market == "futures" {
            Self { fee_maker_bps: 2.0, fee_taker_bps: 5.0, ..Self::default() }
        } else {
            Self::default()
        };
        if let Some(bt) = &cfg.backtest {
            if let Some(v) = bt.fee_maker_bps {
                p.fee_maker_bps = v;
            }
            if let Some(v) = bt.fee_taker_bps {
                p.fee_taker_bps = v;
            }
            if let Some(v) = bt.slippage_bps {
                p.slippage_bps = v;
            }
            if let Some(v) = bt.initial_cash {
                p.initial_cash = v;
            }
            if let Some(v) = bt.leverage {
                p.leverage = v;
            }
            if let Some(v) = bt.max_leverage {
                p.max_leverage = v;
            }
            if let Some(v) = bt.mmr_pct {
                p.mmr_pct = v;
            }
            if let Some(v) = bt.funding_rate_8h {
                p.funding_rate_8h = v;
            }
        }
        p.apply_overrides(ov);
        p
    }

    /// CLI/运行时覆盖: `ov` 中 Some 的键覆盖当前值。
    pub fn apply_overrides(&mut self, ov: &BacktestToml) {
        if let Some(v) = ov.fee_maker_bps {
            self.fee_maker_bps = v;
        }
        if let Some(v) = ov.fee_taker_bps {
            self.fee_taker_bps = v;
        }
        if let Some(v) = ov.slippage_bps {
            self.slippage_bps = v;
        }
        if let Some(v) = ov.initial_cash {
            self.initial_cash = v;
        }
        if let Some(v) = ov.leverage {
            self.leverage = v;
        }
        if let Some(v) = ov.max_leverage {
            self.max_leverage = v;
        }
        if let Some(v) = ov.mmr_pct {
            self.mmr_pct = v;
        }
        if let Some(v) = ov.funding_rate_8h {
            self.funding_rate_8h = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_value_accessors() {
        assert_eq!(ConfigValue::String("hello".into()).as_str(), Some("hello"));
        assert_eq!(ConfigValue::Float(1.5).as_f64(), Some(1.5));
        assert_eq!(ConfigValue::Integer(42).as_i64(), Some(42));
        assert_eq!(ConfigValue::Boolean(true).as_bool(), Some(true));
        assert_eq!(ConfigValue::Float(1.5).as_dec(), Decimal::from_f64_retain(1.5));
    }

    #[test]
    fn test_parse_strategy_toml() {
        let toml_str = r#"
[strategy]
type = "shannon_rebalance"
name = "ETH 中性网格"
enabled = true
exchange = "binance"

[strategy.params]
pair = "ETH"
order_size = 0.01

[risk]
max_position_notional = 10000.0
max_daily_loss_usd = 500.0
"#;
        let config = StrategyConfig::from_toml(toml_str).unwrap();
        assert_eq!(config.name, "ETH 中性网格");
        assert_eq!(config.strategy_type, "shannon_rebalance");
        assert_eq!(config.get_str("pair"), Some("ETH"));
        assert_eq!(config.get_f64("order_size"), Some(0.01));
        assert_eq!(config.risk.as_ref().unwrap().max_position_notional, Some(10000.0));
    }

    #[test]
    fn test_parse_strategy_toml_defaults() {
        let toml_str = r#"
[strategy]
type = "simple"
name = "test"
exchange = "binance"
"#;
        let config = StrategyConfig::from_toml(toml_str).unwrap();
        assert!(config.enabled);
        assert!(config.params.is_empty());
        assert!(config.risk.is_none());
        assert!(!config.live_enabled);
    }

    #[test]
    fn test_toml_roundtrip() {
        let toml_str = r#"
[strategy]
type = "shannon_rebalance"
name = "test"
exchange = "binance"
dry_run_started_at = "2026-07-18T00:00:00Z"
live_enabled = true

[strategy.params]
pair = "ETH"

[backtest]
fee_maker_bps = 5.0
initial_cash = 50000.0
"#;
        let config = StrategyConfig::from_toml(toml_str).unwrap();
        let serialized = config.to_toml().unwrap();
        let reparsed = StrategyConfig::from_toml(&serialized).unwrap();
        assert!(reparsed.live_enabled);
        assert_eq!(reparsed.get_str("pair"), Some("ETH"));
        assert_eq!(reparsed.market, "spot");
        assert_eq!(reparsed.position_mode, "one-way");
        assert_eq!(reparsed.backtest.as_ref().unwrap().fee_maker_bps, Some(5.0));
        assert_eq!(reparsed.backtest.as_ref().unwrap().initial_cash, Some(50000.0));
    }

    #[test]
    fn test_parse_toml_market_position_mode_and_backtest() {
        let toml_str = r#"
[strategy]
type = "simple"
name = "test"
exchange = "binance"
market = "futures"
position_mode = "hedge"

[backtest]
fee_maker_bps = 2.0
fee_taker_bps = 5.0
slippage_bps = 3.0
initial_cash = 200000.0
leverage = 3.0
max_leverage = 20.0
mmr_pct = 1.5
funding_rate_8h = 0.00005
"#;
        let config = StrategyConfig::from_toml(toml_str).unwrap();
        assert_eq!(config.market, "futures");
        assert_eq!(config.position_mode, "hedge");
        let bt = config.backtest.unwrap();
        assert_eq!(bt.fee_maker_bps, Some(2.0));
        assert_eq!(bt.fee_taker_bps, Some(5.0));
        assert_eq!(bt.slippage_bps, Some(3.0));
        assert_eq!(bt.initial_cash, Some(200000.0));
        assert_eq!(bt.leverage, Some(3.0));
        assert_eq!(bt.max_leverage, Some(20.0));
        assert_eq!(bt.mmr_pct, Some(1.5));
        assert_eq!(bt.funding_rate_8h, Some(0.00005));
    }

    #[test]
    fn test_old_toml_without_new_fields_defaults() {
        // 旧 TOML (无 market/position_mode/backtest) → 默认 spot / one-way / None。
        let toml_str = r#"
[strategy]
type = "simple"
name = "test"
exchange = "binance"
"#;
        let config = StrategyConfig::from_toml(toml_str).unwrap();
        assert_eq!(config.market, "spot");
        assert_eq!(config.position_mode, "one-way");
        assert!(config.backtest.is_none());
    }

    fn spot_cfg() -> StrategyConfig {
        StrategyConfig {
            name: "t".into(),
            strategy_type: "simple".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            risk: None,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    #[test]
    fn test_resolve_three_layers_spot_defaults() {
        // 三层: 内置默认 (spot 10/10bps) < TOML [backtest] < CLI overrides。
        let cfg = spot_cfg();
        let p = BacktestParams::resolve(&cfg, &BacktestToml::default());
        assert_eq!(p.fee_maker_bps, 10.0);
        assert_eq!(p.fee_taker_bps, 10.0);
        assert_eq!(p.slippage_bps, 0.0, "规范 §三 滑点默认 0");
        assert_eq!(p.initial_cash, 100_000.0);
        assert_eq!(p.leverage, 1.0);
        assert_eq!(p.max_leverage, 10.0);
        assert_eq!(p.mmr_pct, 1.0);
        assert_eq!(p.funding_rate_8h, 0.0001);

        // TOML [backtest] 覆盖中间层。
        let mut cfg2 = spot_cfg();
        cfg2.backtest = Some(BacktestToml {
            fee_maker_bps: Some(1.0),
            fee_taker_bps: Some(1.0),
            initial_cash: Some(50_000.0),
            ..Default::default()
        });
        let p2 = BacktestParams::resolve(&cfg2, &BacktestToml::default());
        assert_eq!(p2.fee_maker_bps, 1.0);
        assert_eq!(p2.initial_cash, 50_000.0);
        assert_eq!(p2.slippage_bps, 0.0, "未覆盖键保持内置默认");

        // CLI 覆盖最上层。
        let ov = BacktestToml { fee_maker_bps: Some(8.0), ..Default::default() };
        let p3 = BacktestParams::resolve(&cfg2, &ov);
        assert_eq!(p3.fee_maker_bps, 8.0);
        assert_eq!(p3.initial_cash, 50_000.0, "CLI 未覆盖键保持 TOML 值");
    }

    #[test]
    fn test_validate_leverage_bounds() {
        // 合法: 1x 与默认上限内。
        assert!(BacktestParams::validate_leverage(1.0, 10.0).is_ok());
        assert!(BacktestParams::validate_leverage(10.0, 10.0).is_ok());
        // L3: 0/负值 → 报错 (此前除零 panic)。
        let e = BacktestParams::validate_leverage(0.0, 10.0).unwrap_err().to_string();
        assert!(e.contains("≥ 1"), "0 杠杆报错含范围提示: {e}");
        assert!(BacktestParams::validate_leverage(-5.0, 10.0).is_err());
        // 方案 A: 超默认上限 10 拒绝 (含长期盈利提示)。
        let e2 = BacktestParams::validate_leverage(15.0, 10.0).unwrap_err().to_string();
        assert!(e2.contains("15") && e2.contains("10"), "超限报错含数值: {e2}");
        assert!(e2.contains("长期盈利"), "超限报错含产品立场提示: {e2}");
        // 显式放宽后通过 (知情)。
        assert!(BacktestParams::validate_leverage(15.0, 20.0).is_ok());
    }

    #[test]
    fn test_resolve_futures_default_fees() {
        // 合约内置费率 = BN USDT-M 标准 (maker 2 / taker 5 bps); 其余同默认。
        let mut cfg = spot_cfg();
        cfg.market = "futures".into();
        let p = BacktestParams::resolve(&cfg, &BacktestToml::default());
        assert_eq!(p.fee_maker_bps, 2.0);
        assert_eq!(p.fee_taker_bps, 5.0);
        assert_eq!(p.initial_cash, 100_000.0);
        assert_eq!(p.leverage, 1.0);
    }
}
