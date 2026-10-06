//! `ricow backtest` — 命令行回测。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::Args;
use ricow_core::{Balance, CoreError, CoreResult};
use ricow_engine::Engine;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

use crate::commands::format_backtest_report;
use ricow_strategy::{BacktestParams, BacktestToml, ConfigValue, Context, StrategyConfig};

/// `YYYY-MM-DD` -> 当日 00:00 UTC 毫秒 (回测窗口边界用)。
fn parse_ymd_ms(s: &str) -> CoreResult<i64> {
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| CoreError::InvalidArgument(format!("--start/--end 需 YYYY-MM-DD: {e}")))?;
    Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis())
}

#[derive(Args, Default)]
pub struct BacktestArgs {
    /// 策略 (内置策略 id 或已部署策略名; 内置策略与中文名见 `ricow ai` 的 list_templates 或 Web 策略面板; exec API 见 specs/lua-api.md)
    #[arg(long)]
    pub strategy: String,
    /// 交易对 (TOML 策略已含 pair 时可省略; 直跑模式必填)
    #[arg(long)]
    pub pair: Option<String>,
    /// 回测天数 (默认 90)
    #[arg(long)]
    pub days: Option<u32>,

    /// 回测窗口起点 (YYYY-MM-DD, UTC); 与 --end 配套, 覆盖 --days (按自然年月分段用)。
    #[arg(long)]
    pub start: Option<String>,

    /// 回测窗口终点 (YYYY-MM-DD, UTC, 不含); 缺省 = 现在。
    #[arg(long)]
    pub end: Option<String>,
    /// K 线间隔 (1m/5m/15m/1h/4h/1d, 默认 1h)
    #[arg(long)]
    pub interval: Option<String>,
    /// lua 脚本路径 (strategy=lua 时必填)
    #[arg(long)]
    pub script: Option<String>,
    /// 策略参数透传, 格式 key=value (可重复)
    #[arg(long = "param")]
    pub params: Vec<String>,
    // ---- 回测参数覆盖 (三层配置最上层, 见 specs/backtest.md §三; None = 不覆盖) ----
    /// maker/taker 手续费同设 (bps)
    #[arg(long)]
    pub fee: Option<f64>,
    /// maker 手续费 (bps)
    #[arg(long = "fee-maker")]
    pub fee_maker: Option<f64>,
    /// taker 手续费 (bps)
    #[arg(long = "fee-taker")]
    pub fee_taker: Option<f64>,
    /// 市价滑点 (bps)
    #[arg(long = "slippage-bps")]
    pub slippage_bps: Option<f64>,
    /// 初始现金 (quote; 默认 100000, 见内核 BacktestParams::default)
    #[arg(long)]
    pub cash: Option<f64>,
    /// 合约杠杆 (逐仓, 1-10; 超 10x 需 --max-leverage 显式放宽)
    #[arg(long)]
    pub leverage: Option<f64>,
    /// 合约杠杆上限 (默认 10; 放宽 = 知情, 超 10x 与长期盈利目标相悖)
    #[arg(long = "max-leverage")]
    pub max_leverage: Option<f64>,
    /// 维持保证金率 MMR (%, 默认随 symbol 内置首档表, 表外 1.0)
    #[arg(long = "mmr-pct")]
    pub mmr_pct: Option<f64>,
    /// 资金费率 /8h (比率, 默认 0.0001; 0 = 关闭)
    #[arg(long = "funding-rate")]
    pub funding_rate: Option<f64>,
    /// 市场类型 spot|futures (默认随策略 TOML / spot)
    #[arg(long)]
    pub market: Option<String>,
    /// 持仓模式 one-way|hedge (默认随策略 TOML / one-way)
    #[arg(long = "position-mode")]
    pub position_mode: Option<String>,
    // ---- 敏感性扫描 (036; 分析文档第四节) ----
    /// 滑点敏感性: 同一策略同一窗口按多档滑点(bps)各跑一次, 看结论是否翻转。
    /// 省略档位即用默认阶梯 0,5,10; 例: `--sensitivity 0,3,6,12`
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub sensitivity: Option<String>,
    /// 费用敏感性: 按多档手续费(bps)各跑一次 (每档把 maker/taker 同设, 与 `--fee` 同口径)。
    /// 省略档位即用默认阶梯 5,10,20; 例: `--sensitivity-fee 2,5,10,20`
    #[arg(long = "sensitivity-fee", num_args = 0..=1, default_missing_value = "")]
    pub sensitivity_fee: Option<String>,
}

/// 解析 `key=value` 参数: 值按 f64 优先, 否则字符串。
pub(crate) fn parse_param(s: &str) -> Option<(String, ConfigValue)> {
    let (key, value) = s.split_once('=')?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return None;
    }
    let value = value.trim();
    // true/false(不区分大小写)→ Boolean: Lua 侧 ctx:config_bool 只认 ConfigValue::Boolean,
    // 若落到 String 就会恒读成 false(2026-09-18 踩坑: `--param enter_at_start=true` 不生效)。
    let cv = match value.to_ascii_lowercase().as_str() {
        "true" => ConfigValue::Boolean(true),
        "false" => ConfigValue::Boolean(false),
        _ => match value.parse::<f64>() {
            Ok(f) => ConfigValue::Float(f),
            Err(_) => ConfigValue::String(value.to_string()),
        },
    };
    Some((key, cv))
}

/// Web 指标卡结构化摘要 (P0-2/P1-6): 与文本报告同一份 [`ricow_strategy::BacktestReport`],
/// 只做标量投影 —— 前端不再解析报告文本。Decimal 一律转 f64 (展示用, 非清算口径)。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct MetricsSummary {
    pub pair: String,
    pub interval: String,
    pub days: u32,
    pub is_futures: bool,
    pub total_bars: usize,
    pub total_trades: u64,
    pub rejected_count: u64,
    pub net_pnl: f64,
    pub total_fees: f64,
    pub win_rate: f64,
    pub max_drawdown_pct: f64,
    pub equity_change_pct: f64,
    pub benchmark_return_pct: Option<f64>,
    pub benchmark_max_drawdown_pct: Option<f64>,
    pub annual_return_pct: Option<f64>,
    pub annual_volatility_pct: Option<f64>,
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub calmar: Option<f64>,
    pub profit_factor: Option<f64>,
    pub turnover_ratio: f64,
    pub final_equity: f64,
    pub initial_cash: f64,
    pub data_source: String,
    pub fee_ratio_pct: f64,
}

impl BacktestOutcome {
    pub(crate) fn metrics(&self) -> MetricsSummary {
        let r = &self.report;
        MetricsSummary {
            pair: self.pair.clone(),
            interval: self.interval.clone(),
            days: self.days,
            is_futures: self.is_futures,
            total_bars: r.total_bars,
            total_trades: r.total_trades,
            rejected_count: r.rejected_count,
            net_pnl: r.net_pnl.to_f64().unwrap_or(0.0),
            total_fees: r.total_fees.to_f64().unwrap_or(0.0),
            win_rate: r.win_rate,
            max_drawdown_pct: r.max_drawdown.to_f64().unwrap_or(0.0) * 100.0,
            equity_change_pct: r.equity_change_pct,
            benchmark_return_pct: r.benchmark_return_pct,
            benchmark_max_drawdown_pct: r
                .benchmark_max_drawdown
                .and_then(|v| v.to_f64())
                .map(|v| v * 100.0),
            annual_return_pct: r.annual_return.map(|v| v * 100.0),
            annual_volatility_pct: r.annual_volatility.map(|v| v * 100.0),
            sharpe: r.sharpe,
            sortino: r.sortino,
            calmar: r.calmar,
            profit_factor: r.profit_factor,
            turnover_ratio: r.turnover_ratio,
            final_equity: r.final_equity.to_f64().unwrap_or(0.0),
            initial_cash: self.initial_cash.to_f64().unwrap_or(0.0),
            data_source: self.data_source.clone(),
            fee_ratio_pct: r.fee_ratio.to_f64().unwrap_or(0.0) * 100.0,
        }
    }
}

/// 回测的「解析后执行输入」(032 US3 T025): CLI `ricow backtest` 与 Web 异步回测作业
/// (`POST /api/backtest`) 先各自把入参组装成本结构, 再调**同一个** [`run_backtest_core`],
/// 保证页面与命令行的 K 线拉取/预热/撮合/报告口径零分叉 (FR-019, 报告唯一来源 format_backtest_report)。
#[derive(Debug, Clone)]
pub(crate) struct BacktestRunSpec {
    /// 数据目录(定位 `strategies/<name>.toml`; CLI=project_root, Web=WebState.root)。
    pub(crate) root: PathBuf,
    /// 策略: 已部署策略名/内置 id(命中实例 TOML)或可直跑的内置类型名 / "lua"。
    pub(crate) strategy: String,
    /// 交易对(TOML 已含 pair 时可缺省; 直跑模式必填)。
    pub(crate) pair: Option<String>,
    /// 回测天数(默认 90; start/end 显式窗口会覆盖)。
    pub(crate) days: u32,
    /// K 线间隔: 1m/5m/15m/1h/4h/1d(默认 1h)。
    pub(crate) interval: String,
    /// 窗口起点 YYYY-MM-DD(UTC)。
    pub(crate) start: Option<String>,
    /// 窗口终点 YYYY-MM-DD(UTC, 不含); 缺省=现在。
    pub(crate) end: Option<String>,
    /// 市场 spot|futures(None=随策略 TOML / spot)。
    pub(crate) market: Option<String>,
    /// 持仓模式 one-way|hedge(None=随策略 TOML / one-way)。
    pub(crate) position_mode: Option<String>,
    /// 策略参数覆盖(已按类型解析; CLI 来自 key=value, Web 来自 JSON)。
    pub(crate) params: HashMap<String, ConfigValue>,
    /// 直跑 lua 模式的脚本路径(仅 CLI `--script`; Web 首期不提供)。
    pub(crate) script_path: Option<PathBuf>,
    // ---- 回测参数三层覆盖的最上层 (None=不覆盖, 见 specs/backtest.md §三) ----
    /// maker/taker 手续费同设 (bps)。
    pub(crate) fee: Option<f64>,
    /// maker 手续费 (bps)。
    pub(crate) fee_maker: Option<f64>,
    /// taker 手续费 (bps)。
    pub(crate) fee_taker: Option<f64>,
    /// 市价滑点 (bps)。
    pub(crate) slippage_bps: Option<f64>,
    /// 初始现金 (quote)。
    pub(crate) cash: Option<f64>,
    /// 合约杠杆 (逐仓)。
    pub(crate) leverage: Option<f64>,
    /// 合约杠杆上限。
    pub(crate) max_leverage: Option<f64>,
    /// 维持保证金率 MMR (%)。
    pub(crate) mmr_pct: Option<f64>,
    /// 资金费率 /8h。
    pub(crate) funding_rate: Option<f64>,
}

impl BacktestRunSpec {
    /// CLI 组装: 解析 `key=value` 参数、填默认值(与旧 `run_backtest` 的 unwrap_or 同值)。
    pub(crate) fn from_cli_args(root: PathBuf, args: BacktestArgs) -> Self {
        let BacktestArgs {
            strategy,
            pair,
            days,
            start,
            end,
            interval,
            script,
            params,
            fee,
            fee_maker,
            fee_taker,
            slippage_bps,
            cash,
            leverage,
            max_leverage,
            mmr_pct,
            funding_rate,
            market,
            position_mode,
            // 敏感性是 CLI 编排层的事(扫多轮), 不进单次回测的输入 -> 此处刻意丢弃。
            sensitivity: _,
            sensitivity_fee: _,
        } = args;
        let mut overrides = HashMap::new();
        for p in params {
            if let Some((k, v)) = parse_param(&p) {
                overrides.insert(k, v);
            }
        }
        Self {
            root,
            strategy,
            pair,
            days: days.unwrap_or(90),
            interval: interval.unwrap_or_else(|| "1h".to_string()),
            start,
            end,
            market,
            position_mode,
            params: overrides,
            script_path: script.map(PathBuf::from),
            fee,
            fee_maker,
            fee_taker,
            slippage_bps,
            cash,
            leverage,
            max_leverage,
            mmr_pct,
            funding_rate,
        }
    }

    /// 组装回测参数三层覆盖表(原 `apply_backtest_cli` 的映射段; CLI/Web 共用, 不重复)。
    fn backtest_overrides(&self) -> BacktestToml {
        let mut ov = BacktestToml::default();
        if let Some(v) = self.fee {
            ov.fee_maker_bps = Some(v);
            ov.fee_taker_bps = Some(v);
        }
        if let Some(v) = self.fee_maker {
            ov.fee_maker_bps = Some(v);
        }
        if let Some(v) = self.fee_taker {
            ov.fee_taker_bps = Some(v);
        }
        if let Some(v) = self.slippage_bps {
            ov.slippage_bps = Some(v);
        }
        if let Some(v) = self.cash {
            ov.initial_cash = Some(v);
        }
        if let Some(v) = self.leverage {
            ov.leverage = Some(v);
        }
        if let Some(v) = self.max_leverage {
            ov.max_leverage = Some(v);
        }
        if let Some(v) = self.mmr_pct {
            ov.mmr_pct = Some(v);
        }
        if let Some(v) = self.funding_rate {
            ov.funding_rate_8h = Some(v);
        }
        ov
    }
}

/// 构造回测配置(可注入数据目录): 命中 `<root>/strategies/<name>.toml` → TOML 加载并叠加
/// 参数覆盖; 未命中 → 内置类型直跑(pair 必填; `strategy=lua` 时读 script_path 文件)。
///
/// 引擎/CLI 生产代码不写死任何策略参数名(`pair`/`script`/`interval` 为通用键豁免)。
fn load_run_config(
    root: &Path,
    strategy: &str,
    pair: Option<&str>,
    script_path: Option<&Path>,
    overrides: HashMap<String, ConfigValue>,
) -> CoreResult<StrategyConfig> {
    let strategies_dir = crate::commands::ensure_strategies_dir_in(root)?;
    let toml_path = strategies_dir.join(format!("{strategy}.toml"));
    if toml_path.exists() {
        let mut config = crate::commands::load_strategy_toml(&strategies_dir, strategy)?;
        config.params.extend(overrides);
        return Ok(config);
    }

    // #020: 错策略名不冒"unsupported strategy" —— 未命中 TOML 且不是可直跑 id(lua/目录内策略)
    // 时, 如实报不存在并列出可用策略名 (与 --strategy 帮助同源: strategies::catalog)。
    if strategy != "lua" && crate::commands::templates::find(strategy).is_none() {
        let mut names = crate::commands::templates::names();
        names.sort();
        return Err(CoreError::InvalidArgument(format!(
            "策略 {strategy} 不存在: strategies/{strategy}.toml 不存在, 也不是内置模板 id。\
             可用策略({}): {} (说明与参数见 Web 策略面板或 `ricow ai` 的 list_templates)",
            names.len(),
            names.join(", ")
        )));
    }

    // 直跑模式: 只传运行环境信息(pair)与 lua 脚本; 参数全部由 Lua 自己的 fallback 默认决定。
    let pair = pair.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| {
        CoreError::InvalidArgument("直跑模式需要 --pair <pair> (或使用已部署策略名)".into())
    })?;
    let mut params: HashMap<String, ConfigValue> = HashMap::new();
    params.insert("pair".into(), ConfigValue::String(pair.to_string()));
    if strategy == "lua" {
        let script = script_path
            .ok_or_else(|| CoreError::InvalidArgument("lua 策略需要 --script <path>".into()))?;
        let code = std::fs::read_to_string(script)
            .map_err(|e| CoreError::InvalidArgument(format!("读取脚本失败: {e}")))?;
        params.insert("script".into(), ConfigValue::String(code));
    }
    params.extend(overrides);

    crate::commands::resolve_builtin_script(StrategyConfig {
        name: format!("{strategy}-{pair}"),
        strategy_type: strategy.to_string(),
        enabled: true,
        exchange: "binance".into(),
        params,
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    })
}

/// 构造回测配置: 命中 strategies/<name>.toml → TOML 加载 (--param 透传覆盖);
/// 未命中 → 策略类型直跑 (旧逻辑, 默认参数)。
///
/// 仅供既有单测使用(T025 后生产路径走 `BacktestRunSpec` + [`load_run_config`]);
/// 数据目录取全局 project_root, 参数来自 CLI `key=value`。
#[cfg(test)]
fn resolve_config(args: &BacktestArgs) -> CoreResult<StrategyConfig> {
    let mut overrides: HashMap<String, ConfigValue> = HashMap::new();
    for p in &args.params {
        if let Some((k, v)) = parse_param(p) {
            overrides.insert(k, v);
        }
    }
    load_run_config(
        &crate::commands::project_root(),
        &args.strategy,
        args.pair.as_deref(),
        args.script.as_deref().map(Path::new),
        overrides,
    )
}

/// 回测参数三层覆盖 (内置默认 < 策略 TOML [backtest] < 调用方覆盖, 见 specs/backtest.md
/// §三): resolve 出全量有效值, futures 校验杠杆 (方案 A + L3), 把覆盖写回 config.params
/// (BacktestContext::new 内 resolve 后 params 池同名键覆盖生效 — 单次回测覆盖全链路)。
/// 单标的与 bs_momentum 组合入口共用 (2026-09-09 抽离; 032 T025 改为吃已组装的覆盖表, CLI/Web 共用)。
fn apply_backtest_overrides(
    ov: BacktestToml,
    config: &mut StrategyConfig,
) -> CoreResult<BacktestParams> {
    let params = BacktestParams::resolve(config, &ov);
    // 杠杆校验 (方案 A + L3): resolve 后立即拦, 避免无效参数白拉 K 线。
    // 上限 = 三层合并结果 (默认 10); 校验后 params 池写回值即引擎消费值。
    if config.market == "futures" {
        BacktestParams::validate_leverage(params.leverage, params.max_leverage)?;
    }
    // 引擎消费路径: BacktestContext::new 内 resolve(内置默认 + config.backtest)后,
    // 被 params 池同名 key 覆盖 —— 把 CLI 层全量结果写回 params 池, 单次回测覆盖全链路生效。
    config.params.insert("fee_maker_bps".into(), ConfigValue::Float(params.fee_maker_bps));
    config.params.insert("fee_taker_bps".into(), ConfigValue::Float(params.fee_taker_bps));
    config.params.insert("slippage_bps".into(), ConfigValue::Float(params.slippage_bps));
    config.params.insert("initial_cash".into(), ConfigValue::Float(params.initial_cash));
    config.params.insert("leverage".into(), ConfigValue::Float(params.leverage));
    config.params.insert("max_leverage".into(), ConfigValue::Float(params.max_leverage));
    config.params.insert("mmr_pct".into(), ConfigValue::Float(params.mmr_pct));
    config.params.insert("funding_rate_8h".into(), ConfigValue::Float(params.funding_rate_8h));
    Ok(params)
}

/// 一次回测的结构化结果 (036 抽出): 文本报告 / run card / 敏感性扫描共用同一内核。
///
/// 拆出来的动机 = 敏感性扫描要**逐档读指标**, 不该去解析格式化后的文本。
pub(crate) struct BacktestOutcome {
    /// 引擎产出的一次完整报告。
    pub(crate) report: ricow_strategy::BacktestReport,
    /// 生效的标的 / 窗口 / 粒度 / 市场 (报告标题与敏感性表头共用)。
    pub(crate) pair: String,
    pub(crate) days: u32,
    pub(crate) interval: String,
    pub(crate) is_futures: bool,
    pub(crate) initial_cash: Decimal,
    pub(crate) effective_mmr_pct: f64,
    /// 三层合并后的全量回测参数 (敏感性表头如实打印生效成本)。
    pub(crate) params: BacktestParams,
    /// 数据来源 (`binance-rest` / `local-cache`)。
    pub(crate) data_source: String,
    /// 证据卡 (未落盘; 由调用方决定写不写 —— 敏感性扫描不为每档都落一张卡)。
    card: RunCard,
    // ---- Web 可视化数据 (P0-2): 与净值曲线同窗对齐, 供回测作业回结构化图表 ----
    /// 每根 bar 的收盘价 (f64; 与 `close_times_ms` 一一对应)。
    pub(crate) closes: Vec<f64>,
    /// 每根 bar 的 close_time 毫秒 (epoch; 价格轴时间戳)。
    pub(crate) close_times_ms: Vec<i64>,
}

/// Web 回测图表的单笔成交标记 (P0-2)。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ChartFill {
    /// 成交时刻 epoch 毫秒。
    pub t: i64,
    pub price: f64,
    pub size: f64,
    /// "buy" / "sell"。
    pub side: String,
}

/// Web 回测图表数据 (P0-2): 价格轴 = 评测窗口内每根 bar 的收盘价, 权益轴 = 同一批 bar 的
/// 收盘估值 (**不含**曲线初始现金点)。三条序列 (times/price/equity) 等长且逐点同刻 —— 前端
/// 可直接按下标配对绘制。时间轴统一 epoch 毫秒; 点数超限时按同一步长抽稀 (三条同步, 保对齐)。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct BacktestChart {
    pub times: Vec<i64>,
    pub price: Vec<f64>,
    pub equity: Vec<f64>,
    pub fills: Vec<ChartFill>,
}

/// 曲线抽稀上限 (点): 超过按整步抽稀, 覆盖 3650 天 1m 的极端窗口也不至于拖垮前端。
const CHART_MAX_POINTS: usize = 2000;
/// 成交标记上限: 超长回测的成交清单只保前 N 笔 (表格另见报告文本)。
const CHART_MAX_FILLS: usize = 1000;

/// 本地 K 线缓存的保留期 (天, 审计 资源-4): 早于该天数的缓存行在回填后清理。
///
/// K 线表只增不删会缓慢膨胀磁盘; 它是**只读缓存**(随时可从交易所重拉), 故可安全过期。
/// 取 400 天: 覆盖一年整的回测窗口还留余量, 又足够把长期积累的陈数据清出去。
/// 过期只删本地副本 —— 需要更早的历史时, 回测会按窗口自动向前翻页重新取数(见上方分页逻辑)。
const KLINE_RETENTION_DAYS: i64 = 400;

/// 单次回测允许拉取的 **K 线根数上限** (审计 低危 #4)。
///
/// 回测把整段 K 线一次性读进 `Vec<Kline>` 并交给引擎逐 bar 回放 —— 内存随根数线性增长,
/// 而根数 = `天数 × 24 / bar 小时数` 原样由调用方给: CLI `--days` 无上界, 一个笔误
/// (`--days 100000 --interval 1m` = 1.44 亿根) 就能在拉数阶段把进程 OOM 掉, 而用户只看到
/// "卡住/被杀"。这里在**拉数据之前**竖起上限, 报错说明按哪个口径超了, 让用户改窗口而不是等 OOM。
///
/// 取值 13,000,000 根: 约对应 1m × 90 天 (≈12.96M) / 1h × 1480 天;`Kline` 约 100 字节/根,
/// 该量级下峰值约 1.3 GB(含预热与副本) —— 已是普通机器能承受、且远超实际策略回测所需的边界。
const MAX_BACKTEST_BARS: u64 = 13_000_000;

/// 审计 低危 #4: 纯函数形式的上限校验(便于单测, 不碰网络)。
///
/// `limit` 是**已换算好的根数**, `days`/`interval` 只用于报错文案。
fn check_bar_budget(limit: u32, days: u32, interval: &str) -> CoreResult<()> {
    if u64::from(limit) > MAX_BACKTEST_BARS {
        return Err(CoreError::InvalidArgument(format!(
            "回测窗口过大: {days} 天 × {interval} 约需 {limit} 根 K 线, 超过上限 {MAX_BACKTEST_BARS} 根 \
             —— 一次性载入会耗尽内存。请缩小 --days / --start..--end 窗口, 或改用更大的 --interval。"
        )));
    }
    Ok(())
}

impl BacktestOutcome {
    /// 抽稀一维序列 (保首尾): `stride` = 1 时原样返回。
    fn downsample(v: &[f64], stride: usize) -> Vec<f64> {
        if stride <= 1 {
            return v.to_vec();
        }
        let mut out: Vec<f64> = v.iter().step_by(stride).copied().collect();
        // 末点强制保留 (与另一条曲线的末点对齐语义一致)。
        if !(v.len() - 1).is_multiple_of(stride) {
            out.push(v[v.len() - 1]);
        }
        out
    }

    /// 组装 Web 可视化数据 (纯转换, 不重跑回测)。
    ///
    /// 对齐口径 (2026-10-05 实测修正): 引擎的 `klines` 是 `[前端预热段] + [评测窗口]` ——
    /// 预热段 (实测 1h×30d = 24 根) 只用于初始化指标, **不产生估值点**; `report.total_bars`
    /// = 评测窗口 bar 数。因此价格/时间必须从**尾部**取 `total_bars` 根, 再与净值曲线逐点对齐;
    /// 若从首部取, 价格会比权益早 24 根 (曲线画出来是错的)。
    pub(crate) fn chart(&self) -> BacktestChart {
        let (times, price, equity) = Self::align_window(
            self.report.total_bars,
            &self.closes,
            &self.close_times_ms,
            &self.report.equity_curve,
        );
        let n = times.len();
        let stride = if n == 0 { 1 } else { n.div_ceil(CHART_MAX_POINTS) };
        let fills = self
            .report
            .fills
            .iter()
            .take(CHART_MAX_FILLS)
            .filter_map(|f| {
                Some(ChartFill {
                    t: f.timestamp.timestamp_millis(),
                    price: f.fill_price.to_f64()?,
                    size: f.fill_size.to_f64()?,
                    side: match f.side {
                        ricow_core::OrderSide::Buy => "buy".into(),
                        ricow_core::OrderSide::Sell => "sell".into(),
                    },
                })
            })
            .collect();
        BacktestChart {
            times: Self::downsample_i64(&times, stride),
            price: Self::downsample(&price, stride),
            equity: Self::downsample(&equity, stride),
            fills,
        }
    }

    /// 把引擎原始序列对齐成「同刻等长」三序列 (纯函数, 便于单测)。
    ///
    /// 口径: 引擎给出 `[前端预热段] + [评测窗口]` 的 `closes/times`, 而 `equity_curve`
    /// 只覆盖评测窗口且**含首点初始现金** (`= [初始] + 逐 bar 估值`)。于是:
    /// - 窗口 bar 数 `n = min(total_bars, 可画点数, equity.len()-1)`;
    /// - 跳前端预热段 `skip = n_price - n`;
    /// - `equity` 丢初始点, 取 `[1..=n]`, 与价格窗口逐点同刻。
    ///
    /// 返回 `(times, price, equity)`, 三者恒等长 `n` (退化时可为 0)。
    fn align_window(
        total_bars: usize,
        closes: &[f64],
        close_times_ms: &[i64],
        equity_curve: &[Decimal],
    ) -> (Vec<i64>, Vec<f64>, Vec<f64>) {
        let n_price = closes.len().min(close_times_ms.len());
        let n = total_bars.min(n_price).min(equity_curve.len().saturating_sub(1));
        if n == 0 {
            return (Vec::new(), Vec::new(), Vec::new());
        }
        let skip = n_price - n; // 跳过前端预热段
        let times = close_times_ms[skip..skip + n].to_vec();
        let price = closes[skip..skip + n].to_vec();
        let equity = equity_curve[1..=n].iter().map(|v| v.to_f64().unwrap_or(0.0)).collect();
        (times, price, equity)
    }

    /// i64 版抽稀 (与 [`Self::downsample`] 同规则, 保首尾)。
    fn downsample_i64(v: &[i64], stride: usize) -> Vec<i64> {
        if stride <= 1 {
            return v.to_vec();
        }
        let mut out: Vec<i64> = v.iter().step_by(stride).copied().collect();
        if !(v.len() - 1).is_multiple_of(stride) {
            out.push(v[v.len() - 1]);
        }
        out
    }

    /// 报告标题 (单一口径: 回测正文与敏感性表头都从这里取)。
    pub(crate) fn header(&self, strategy: &str) -> String {
        format!(
            "回测报告: {} {} ({} 天, {} K 线, {})",
            strategy,
            self.pair,
            self.days,
            self.interval,
            if self.is_futures {
                format!(
                    "合约 USDT-M · {} 持仓 · {:.0}x · MMR {:.2}%",
                    self.report.position_mode.as_deref().unwrap_or("one-way"),
                    self.report.leverage.unwrap_or(1.0),
                    self.effective_mmr_pct
                )
            } else {
                "现货".to_string()
            }
        )
    }
}

/// 回测内核 (032 T025): 窗口计算 → 装载策略(TOML/直跑)→ 三层回测参数 → warmup 声明收集 →
/// 分页拉 K 线(与 CLI 同一数据源, 035 起优先命中本地缓存)→ 引擎撮合 → 结构化结果。
///
/// CLI(`run_backtest`)与 Web 异步作业(`web::backtest_jobs`)共用本函数; 不含任何 stdout。
/// 报告文本、run card 落盘、敏感性扫描都建立在本函数之上 ([`BacktestOutcome`])。
pub(crate) async fn run_backtest_inner(spec: &BacktestRunSpec) -> CoreResult<BacktestOutcome> {
    let days = spec.days;
    let interval = spec.interval.clone();
    let hours_per_bar = match interval.as_str() {
        "1m" => 1.0 / 60.0,
        "5m" => 5.0 / 60.0,
        "15m" => 0.25,
        "1h" => 1.0,
        "4h" => 4.0,
        "1d" => 24.0,
        other => {
            return Err(CoreError::InvalidArgument(format!(
                "unsupported interval: {other} (1m/5m/15m/1h/4h/1d)"
            )))
        }
    };
    // start/end: 显式窗口 (自然年月分段); end 缺省 = 现在。
    let end_ms: Option<i64> = match spec.end.as_deref() {
        Some(d) => Some(parse_ymd_ms(d)?),
        None => None,
    };
    let (days, end_ms) = match spec.start.as_deref() {
        Some(s) => {
            let s_ms = parse_ymd_ms(s)?;
            let e_ms = end_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
            let d = (((e_ms - s_ms) as f64) / 86_400_000.0).round().max(1.0) as u32;
            (d, Some(e_ms))
        }
        None => (days, end_ms),
    };
    let limit = ((days as f64) * 24.0 / hours_per_bar) as u32;
    // 审计 低危 #4: 在拉数之前挡住"根数爆炸"。`limit` 由调用方的 `days` 直接换算, CLI
    // 未设上界 —— 超大窗口会在 `Vec<Kline>` 累积阶段 OOM。这里提前报错并给出建议, 让用户
    // 改窗口/粒度, 而不是等进程被杀。
    check_bar_budget(limit, days, &interval)?;

    let exchange = crate::commands::bn_exchange()?;
    let mut config = load_run_config(
        &spec.root,
        &spec.strategy,
        spec.pair.as_deref(),
        spec.script_path.as_deref(),
        spec.params.clone(),
    )?;
    // 覆盖 market/position_mode (三层最上层; market 同时决定数据源分支, 见下)。
    // CLI 文案保持 "--market/--position-mode" 原样(终端输出逐字不变); Web 层在发起前已先拦非法值。
    if let Some(m) = &spec.market {
        if m != "spot" && m != "futures" {
            return Err(CoreError::InvalidArgument(format!(
                "--market 仅支持 spot|futures, 收到 '{m}'"
            )));
        }
        config.market = m.clone();
    }
    if let Some(p) = &spec.position_mode {
        if p != "one-way" && p != "hedge" {
            return Err(CoreError::InvalidArgument(format!(
                "--position-mode 仅支持 one-way|hedge, 收到 '{p}'"
            )));
        }
        config.position_mode = p.clone();
    }
    // 三层回测参数 (内置默认 < 策略 TOML [backtest] < 调用方覆盖): 解析全量有效值 + 写回 params。
    let params = apply_backtest_overrides(spec.backtest_overrides(), &mut config)?;
    // --interval 是主时钟粒度(通用配置, 与 pair 同类): 写入 params 供策略 need_klines("primary", ...)
    // 声明使用。用户显式 --param interval 优先(不覆盖)。若不写, 策略 primary 声明会 fallback "1h",
    // 与 --interval 拉的 K 线粒度错位 → warmup 换算错 → 高周期指标永不就绪(实测 0 成交)。
    config.params.entry("interval".into()).or_insert(ConfigValue::String(interval.clone()));
    // 030 数据需求声明收集: 构造空 ctx 跑一次 on_init, 策略 need_klines 写入 declarations;
    // 据此推 warmup(预热根数)。引擎不再读 atr_interval/regime_interval 等策略参数名。
    // on_init 幂等约定: 声明阶段只依赖 config, 不依赖 balance/K 线(见 specs/architecture.md)。
    let mut declare_strategy = ricow_engine::load_strategy(&config)?;
    let mut declare_ctx = ricow_strategy::BacktestContext::new(
        config.clone(),
        Balance { asset: "USDT".into(), free: Decimal::ZERO, locked: Decimal::ZERO },
    );
    declare_strategy.on_init(&mut declare_ctx);
    let declarations = declare_ctx.declarations();
    // 主时钟周期 = primary 声明; 未声明(异常)时回退 CLI --interval 粒度。
    let main_tf_ms = declarations
        .iter()
        .find(|d| d.role == "primary")
        .and_then(|d| ricow_strategy::tf_ms_of(&d.tf))
        .unwrap_or((hours_per_bar * 3_600_000.0) as i64);
    let mut warmup_bars: u32 = 0;
    for d in &declarations {
        if d.role != "aux" {
            continue;
        }
        let Some(tf_ms) = ricow_strategy::tf_ms_of(&d.tf) else { continue };
        // 向上取整: aux 最少根数换算成主时钟根数 (与引擎 run_backtest 同口径)。
        let need = ((i64::from(d.min_bars) * tf_ms + main_tf_ms - 1) / main_tf_ms) as u32;
        warmup_bars = warmup_bars.max(need.max(1));
    }
    let fetch_limit = limit + warmup_bars;
    // TOML 策略缺 pair 时用调用方给的 pair 兜底; 两者皆无报错。
    if config.get_str("pair").is_none() {
        let pair = spec.pair.clone().ok_or_else(|| {
            CoreError::InvalidArgument("TOML 策略缺 pair 参数, 需 --pair <pair>".into())
        })?;
        config.params.insert("pair".into(), ConfigValue::String(pair));
    }
    let pair = config.get_str("pair").unwrap().to_string();
    // #010: 默认只接受视野内交易对(默认仅股票类, [market] show_all_pairs 放开)。
    // 越界与拼错都在拉 K 线**之前**拒绝 —— 不给交易所原始 400 报文 (#20 的 pair 分支)。
    crate::commands::pairs::ensure_pair_in_scope(&spec.root, &pair, &config.market).await?;
    // 数据源分支 (三层配置的 market 决定, specs/backtest.md §五): 合约用 fapi 公共数据源,
    // 现货沿用交易所客户端。K 线 JSON 同构, 直接喂同一回测引擎。
    // 分页取数 (2026-09-22, 030): 币安 K 线**单次请求上限 1000 根** —— 超过必须向前翻页拼接,
    // 否则长窗口 (1m 数天 / 1h 数月) 会拿到错误响应: 表现为 "network error: error decoding
    // response body" (120 天 1m) 或长时间无输出 (3 天/1 天 1m 实测)。此处按 1000 根/批往前翻页。
    const KLINE_PAGE_MAX: u32 = 1000;
    let fapi = if config.market == "futures" {
        let f = ricow_binance::FuturesDataClient::new()?;
        // MMR 元数据: 调用方未显式给 mmr_pct 时, 按 symbol 查内置首档表 (exchangeInfo 公共值不可靠,
        // 见 specs/backtest.md §十一 T7); 表外回落 1.0%。
        if spec.mmr_pct.is_none() {
            config
                .params
                .insert("mmr_pct".into(), ConfigValue::Float(ricow_binance::tier1_mmr_pct(&pair)));
        }
        Some(f)
    } else {
        None
    };
    // 035: 取数优先命中本地 klines 缓存 —— 仅当窗口**已全部收盘**时启用
    // (`end` 早于 now 至少一根 bar), 保证"实时尾 bar 可能未收盘"不会被缓存固化。
    // 未命中/根数不足 → 直连交易所取数并**回填**缓存 (best-effort, 失败不影响回测)。
    let market_key = config.market.clone();
    let step_ms_i = (hours_per_bar * 3_600_000.0) as i64;
    let now_ms = Utc::now().timestamp_millis();
    let window_end = end_ms.unwrap_or(now_ms);
    let cache_eligible = window_end <= now_ms - step_ms_i;
    let cache_db = crate::commands::open_cache_db(&spec.root).await;
    let mut data_source = "binance-rest";
    let mut klines: Vec<ricow_core::Kline> = Vec::new();
    if cache_eligible {
        if let Some(db) = &cache_db {
            let start_ms = window_end - (fetch_limit as i64) * step_ms_i;
            match db
                .get_klines_range(&market_key, &pair, &interval, start_ms, window_end, fetch_limit)
                .await
            {
                // 缓存覆盖整个窗口 (根数与请求一致) → 直接用, 零网络。
                Ok(cached) if cached.len() as u32 >= fetch_limit => {
                    klines = cached;
                    data_source = "local-cache";
                    tracing::info!(pair = %pair, interval = %interval, bars = klines.len(), "回测命中本地 K 线缓存");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "读 K 线缓存失败, 回退直连交易所"),
            }
        }
    }
    if klines.is_empty() {
        let mut acc: Vec<ricow_core::Kline> = Vec::new();
        let mut cursor = end_ms; // None = 到"现在"为止
        while acc.len() < fetch_limit as usize {
            let want = (fetch_limit as usize - acc.len()).min(KLINE_PAGE_MAX as usize) as u32;
            let batch = match (&fapi, cursor) {
                (Some(f), Some(e)) => f.get_klines_ending_at(&pair, &interval, want, e).await?,
                (Some(f), None) => f.get_klines(&pair, &interval, want).await?,
                (None, Some(e)) => exchange.get_klines_until(&pair, &interval, want, e).await?,
                (None, None) => exchange.get_klines(&pair, &interval, want).await?,
            };
            if batch.is_empty() {
                break;
            }
            cursor = Some(batch[0].open_time.timestamp_millis() - 1);
            let mut merged = batch;
            merged.extend(acc);
            acc = merged;
        }
        klines = acc;
        // 回填缓存 (best-effort): 下次同窗口回测即可零网络命中。
        if let Some(db) = &cache_db {
            if let Err(e) = db.insert_klines(&market_key, &pair, &interval, &klines).await {
                tracing::warn!(error = %e, "K 线回填缓存失败 (不影响回测)");
            }
            // 顺手清理过期缓存 (审计 资源-4): 只增不删会让磁盘缓慢膨胀。best-effort, 失败不影响回测。
            let cutoff =
                (Utc::now() - chrono::Duration::days(KLINE_RETENTION_DAYS)).timestamp_millis();
            match db.prune_klines(cutoff).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(removed = n, "已清理过期 K 线缓存"),
                Err(e) => tracing::warn!(error = %e, "K 线缓存清理失败 (不影响回测)"),
            }
        }
    }
    if klines.is_empty() {
        return Err(CoreError::Exchange(format!("no klines for {pair}")));
    }
    // `--end` 的文档语义是"不含", 但交易所 `endTime` 是**闭区间** —— 会把恰好落在窗口终点的
    // 那根 bar 也取回, 于是"预热带满"与"历史不足(预热被裁剪)"两种路径会差 1 根
    // (2026-09-18 实测: on-4h 8,767 根 vs off-4h 8,766 根) → 按文档语义裁掉终点那一根。
    let klines = match end_ms {
        Some(e) => {
            klines.into_iter().filter(|k| k.open_time.timestamp_millis() < e).collect::<Vec<_>>()
        }
        None => klines,
    };
    if klines.is_empty() {
        return Err(CoreError::Exchange(format!("no klines in window for {pair}")));
    }
    // 预热段"取到多少算多少": 交易所历史短于请求的 `warmup_bars` 时(例: 2022-01-01 起算 + 603 天
    // 日线趋势判据预热, 而 SOL 现货自 2020-08 才上线), 若不裁剪, 引擎的 `skip` 会把**请求窗口的
    // 开头**当成预热吃掉 —— 静默缩短报告窗口, 令开/关两组不可比(2026-09-18 实测: on-4h 少 95 天)。
    // 裁剪口径 = 实际取到的、早于窗口起点的 bar 数。
    if warmup_bars > 0 {
        let step_ms = (hours_per_bar * 3_600_000.0) as i64;
        let e_ms = end_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
        let start_ms = e_ms - (limit as i64) * step_ms;
        let available =
            klines.partition_point(|k| k.open_time.timestamp_millis() < start_ms) as u32;
        let actual = warmup_bars.min(available);
        if actual != warmup_bars {
            // 030(2026-09-23): 预热段不足 = 指标初值不可信, 甚至会让判据**永久未就绪**而静默不下单。
            // 按项目纪律"错误必须暴露, 不许静默降级", 这里**硬报错**, 不再 WARN + 裁剪继续跑。
            // (旧行为导致 QQQBUSDT 回测: 日线只有 84 根 < EMA200 需求, 判据永久未就绪 → 0 成交,
            //  却产出一份看起来正常的报告。)
            return Err(CoreError::InvalidArgument(format!(
                "预热段不足, 拒绝回测: 请求 {warmup_bars} 根高周期历史, 交易所只有 {actual} 根 —— \
                 指标初值会失真(用到日线判据时很可能永久未就绪而静默不下单)。\
                 请扩大窗口或减少高周期数据需求(见所用策略的数据声明与周期参数)。"
            )));
        }
    }

    let initial_cash = Decimal::from_f64_retain(params.initial_cash)
        .ok_or_else(|| CoreError::InvalidArgument("initial_cash 非法".into()))?;
    let initial_balance =
        Balance { asset: "USDT".into(), free: initial_cash, locked: Decimal::ZERO };

    let is_futures = config.market == "futures";
    // 生效 MMR (报告显示用): 三层解析 + 可能的交易所首档拉取已写回 params; config move 前取出。
    // (D3: 此前打印恒 2.5, mmr_pct/TOML 覆盖不反映。)
    let effective_mmr_pct = config.get_f64("mmr_pct").unwrap_or(1.0);
    // 035: run card 的策略/参数快照须在 config move 进引擎前采集。
    let card_strategy = RunCardStrategy {
        name: spec.strategy.clone(),
        kind: config.strategy_type.clone(),
        source_sha256: config
            .get_str("script")
            .map(|s| sha256_hex(s.as_bytes()))
            .unwrap_or_else(|| "none".to_string()),
        source_bytes: config.get_str("script").map(str::len).unwrap_or(0),
    };
    let card_params = params_json(&config);
    let card_window_meta = (config.market.clone(), config.position_mode.clone());
    // Web 可视化数据 (P0-2): bar 收盘价与收盘时刻 (与净值曲线同窗对齐)。
    let closes: Vec<f64> = klines.iter().map(|k| k.close.to_f64().unwrap_or(0.0)).collect();
    let close_times_ms: Vec<i64> = klines.iter().map(|k| k.close_time.timestamp_millis()).collect();
    let report = Engine::new().backtest(config, initial_balance, &klines)?;

    // 035: 可复现 run card —— 策略源码指纹 + 数据窗口 + 参数 + 指标, 供归档/diff/复现。
    // **不在此落盘**: 敏感性扫描会跑多轮, 每轮都落一张卡会污染归档目录; 落盘由调用方决定。
    let card = RunCard {
        schema_version: RUN_CARD_SCHEMA_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        strategy: card_strategy,
        params: card_params,
        window: RunCardWindow {
            pair: pair.clone(),
            interval: interval.clone(),
            market: card_window_meta.0,
            position_mode: card_window_meta.1,
            requested_bars: fetch_limit,
            warmup_bars,
            bars: klines.len(),
            first_open_time_ms: klines.first().map(|k| k.open_time.timestamp_millis()),
            last_open_time_ms: klines.last().map(|k| k.open_time.timestamp_millis()),
            end_ms,
            data_source: data_source.to_string(),
        },
        metrics: RunCardMetrics {
            total_trades: report.total_trades,
            rejected_count: report.rejected_count,
            net_pnl: report.net_pnl.to_string(),
            realized_pnl: report.realized_pnl.to_string(),
            total_fees: report.total_fees.to_string(),
            win_rate: report.win_rate,
            max_drawdown: report.max_drawdown.to_string(),
            annual_return: report.annual_return,
            annual_volatility: report.annual_volatility,
            sharpe: report.sharpe,
            sortino: report.sortino,
            calmar: report.calmar,
            profit_factor: report.profit_factor,
            turnover_ratio: report.turnover_ratio,
            equity_change_pct: report.equity_change_pct,
            benchmark_return_pct: report.benchmark_return_pct,
        },
    };
    Ok(BacktestOutcome {
        report,
        pair,
        days,
        interval,
        is_futures,
        initial_cash,
        effective_mmr_pct,
        params,
        data_source: data_source.to_string(),
        card,
        closes,
        close_times_ms,
    })
}

/// 单次回测的完整入口 (Web 可视化用): 跑内核 → 落 run card → 返回**结构化**结果,
/// 文本报告由调用方按需 [`format_backtest_report`] 生成 (P0-2: Web 作业还要图表/指标数据)。
pub(crate) async fn run_backtest_full(spec: BacktestRunSpec) -> CoreResult<BacktestOutcome> {
    let out = run_backtest_inner(&spec).await?;
    // 035: run card 落盘失败只 warn (回测结果本身仍有效, 不因辅助产物回滚一次成功的回测)。
    match write_run_card(&spec.root, &out.card) {
        Ok(p) => tracing::info!(path = %p.display(), "回测 run card 已落盘"),
        Err(e) => tracing::warn!(error = %e, "回测 run card 落盘失败 (不影响回测结果)"),
    }
    Ok(out)
}

/// 单次回测的文本入口 (CLI `ricow backtest` / AI 工具 / Web 作业共用): 跑内核 → 落 run card
/// → 出**唯一口径**文本报告。
pub(crate) async fn run_backtest_core(spec: BacktestRunSpec) -> CoreResult<String> {
    let strategy = spec.strategy.clone();
    let out = run_backtest_full(spec).await?;
    Ok(format_backtest_report(
        &out.report,
        &out.header(&strategy),
        out.initial_cash,
        out.is_futures,
    ))
}

/// CLI/AI 工具入口包装: 把 [`BacktestArgs`] 组装成 [`BacktestRunSpec`] (数据目录=全局 project_root)
/// 后跑同一内核; 未给敏感性开关时返回的报告文本与 T025 前逐字一致。
pub(crate) async fn run_backtest(args: BacktestArgs) -> CoreResult<String> {
    // 运行门禁(终端 / 对话渠道): 目录审计不通过的 id(未声明的内置副本、清单解析失败的策略…)
    // 一律拒绝 —— 与 Web 端点同一份判定 [`crate::strategies::catalog::run_block`], 免得
    // "网页不让跑但命令行能跑"这种两个口径(2026-10-05)。
    if let Some(why) = crate::strategies::catalog::run_block(&args.strategy) {
        return Err(CoreError::InvalidArgument(why));
    }
    let slippage_ladder = parse_ladder(args.sensitivity.as_deref(), DEFAULT_SLIPPAGE_LADDER)?;
    let fee_ladder = parse_ladder(args.sensitivity_fee.as_deref(), DEFAULT_FEE_LADDER)?;
    let spec = BacktestRunSpec::from_cli_args(crate::commands::project_root(), args);
    if slippage_ladder.is_some() || fee_ladder.is_some() {
        return run_backtest_sensitivity(spec, slippage_ladder, fee_ladder).await;
    }
    run_backtest_core(spec).await
}

/// CLI 入口: 跑回测并打印报告(与 AI 工具 `run_backtest` 共用同一主体与同一份格式化)。
pub async fn run(args: BacktestArgs) -> CoreResult<()> {
    let text = run_backtest(args).await?;
    print!("{text}");
    Ok(())
}

// ---- 滑点/费用敏感性 (036; 分析文档第四节「回测乐观多少」) ----
//
// 不引入复杂撮合模型 (YAGNI), 只做一件可直接回答问题的扫描: **同一策略、同一数据窗口**,
// 沿一个成本轴跑多档, 看"成本后是否盈利"这个结论在哪一档翻转。
//
// 为什么值得: 回测里 `slippage_bps` 默认 0 (理想成交), 手续费也只是交易所挂牌价 ——
// 实盘的冲击成本/滑点通常高于假设。跑一遍阶梯就能知道"结论对成本有多敏感"。

/// 滑点阶梯的默认档位 (bps): 0 = 理想(完全无滑点) / 5 / 10。
const DEFAULT_SLIPPAGE_LADDER: &[f64] = &[0.0, 5.0, 10.0];

/// 费用阶梯的默认档位 (bps, 每档 maker=taker 同设): 5 (BN 合约 taker) / 10 (BN 现货基准) / 20 (双倍)。
const DEFAULT_FEE_LADDER: &[f64] = &[5.0, 10.0, 20.0];

/// 解析敏感性阶梯: `None` = 不扫该轴; `Some("")` = 用默认档位; `Some("0,5,10")` = 显式档位。
///
/// 至少 2 档 —— 单档不构成"敏感性"(扫一档等于什么都没说), 宁可当场报错也不给一份假扫描。
fn parse_ladder(raw: Option<&str>, default_ladder: &[f64]) -> CoreResult<Option<Vec<f64>>> {
    let Some(raw) = raw else { return Ok(None) };
    let trimmed = raw.trim();
    let mut levels: Vec<f64> = Vec::new();
    if trimmed.is_empty() {
        levels.extend_from_slice(default_ladder);
    } else {
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let value: f64 = part.parse().map_err(|_| {
                CoreError::InvalidArgument(format!("敏感性档位不是数字: '{part}' (bps, 逗号分隔)"))
            })?;
            if !value.is_finite() || value < 0.0 {
                return Err(CoreError::InvalidArgument(format!(
                    "敏感性档位须为有限非负数 (bps): '{part}'"
                )));
            }
            levels.push(value);
        }
    }
    if levels.len() < 2 {
        return Err(CoreError::InvalidArgument(
            "敏感性阶梯至少需要 2 档 (单档不成敏感性); 例: --sensitivity 0,5,10".into(),
        ));
    }
    Ok(Some(levels))
}

/// 敏感性表的一行 = 一次完整回测的标量。
struct SensitivityRow {
    /// 档位展示文本: 基准行为 `基准`, 其余为 bps 值。
    label: String,
    total_trades: u64,
    net_pnl: Decimal,
    equity_change_pct: f64,
    max_drawdown: Decimal,
    sharpe: Option<f64>,
    win_rate: f64,
    total_fees: Decimal,
}

impl SensitivityRow {
    fn from_outcome(label: String, out: &BacktestOutcome) -> Self {
        Self {
            label,
            total_trades: out.report.total_trades,
            net_pnl: out.report.net_pnl,
            equity_change_pct: out.report.equity_change_pct,
            max_drawdown: out.report.max_drawdown,
            sharpe: out.report.sharpe,
            win_rate: out.report.win_rate,
            total_fees: out.report.total_fees,
        }
    }

    /// 这一档的结论 = **成本后是否盈利** (净盈亏符号)。这是回测要回答的唯一问题, 不另造阈值。
    fn verdict(&self) -> &'static str {
        match self.net_pnl.cmp(&Decimal::ZERO) {
            std::cmp::Ordering::Greater => "盈利",
            std::cmp::Ordering::Less => "亏损",
            std::cmp::Ordering::Equal => "持平",
        }
    }

    fn sign(&self) -> i8 {
        match self.net_pnl.cmp(&Decimal::ZERO) {
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
        }
    }
}

/// 一个成本轴的扫描结果。
struct SensitivityAxis {
    /// 轴名(`滑点` / `费用`)。
    name: &'static str,
    /// 轴补充说明(费用轴: `maker=taker 同设`)。
    note: &'static str,
    rows: Vec<SensitivityRow>,
}

/// `bps` 显示: 整数不带小数点, 小数保留两位 (0 → "0", 2.5 → "2.50")。
fn fmt_bps(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

/// 终端显示宽度: CJK 全角字符算 2 列 (只为把表头对齐, 不外引依赖)。
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let u = c as u32;
            let wide = (0x1100..=0x115F).contains(&u)
                || (0x2E80..=0xA4CF).contains(&u)
                || (0xAC00..=0xD7A3).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
                || (0xFE30..=0xFE6F).contains(&u)
                || (0xFF00..=0xFF60).contains(&u)
                || (0xFFE0..=0xFFE6).contains(&u);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

/// 按显示宽度右补空格 (不足处补到 `width` 列)。
fn pad_display(s: &str, width: usize) -> String {
    let w = display_width(s);
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// 表格列宽 (显示宽度): 档位 / 成交 / 净盈亏 / 权益变化 / 最大回撤 / 夏普 / 胜率 / 手续费 / 结论。
const SENS_COLS: [usize; 9] = [8, 8, 14, 14, 12, 8, 10, 12, 8];
const SENS_HEADERS: [&str; 9] =
    ["档位", "成交", "净盈亏", "权益变化%", "最大回撤%", "夏普", "胜率%", "手续费", "结论"];

/// 一行表格。
fn sens_line(cells: [String; 9]) -> String {
    let mut out = String::from("  ");
    for (i, cell) in cells.iter().enumerate() {
        out.push_str(&pad_display(cell, SENS_COLS[i]));
    }
    out.trim_end().to_string()
}

impl SensitivityRow {
    fn to_cells(&self) -> [String; 9] {
        [
            self.label.clone(),
            self.total_trades.to_string(),
            self.net_pnl.to_string(),
            format!("{:+.2}%", self.equity_change_pct),
            format!("{:.2}%", self.max_drawdown * Decimal::from(100)),
            self.sharpe.map(|v| format!("{v:.2}")).unwrap_or_else(|| "n/a".into()),
            format!("{:.2}%", self.win_rate * 100.0),
            self.total_fees.to_string(),
            self.verdict().to_string(),
        ]
    }
}

/// 一个轴的结论段: 阶梯内净盈亏符号是否一致; 不一致就点名翻转处。
fn axis_summary(axis: &SensitivityAxis) -> String {
    if axis.rows.iter().all(|r| r.sign() > 0) {
        return "结论: 阶梯内净盈亏始终为正 —— 该成本区间内结论稳健。".into();
    }
    if axis.rows.iter().all(|r| r.sign() < 0) {
        return "结论: 阶梯内净盈亏始终为负 —— 结论稳健(该窗口本就不赚), 与成本档无关。".into();
    }
    let flips: Vec<String> = axis
        .rows
        .windows(2)
        .filter(|w| w[0].sign() != w[1].sign())
        .map(|w| format!("{} → {} bps", w[0].label, w[1].label))
        .collect();
    format!(
        "结论: 净盈亏符号在 {} 处翻转 —— 结论对成本假设敏感, 回测口径须按实际成本复核后再定。",
        flips.join(" / ")
    )
}

/// 敏感性表头信息 (由基准那次回测取出, 与表格本身解耦以便单测)。
struct SensitivityBase {
    /// 基准报告标题 (与单次回测报告同款)。
    header: String,
    /// 生效成本一行 (三层合并结果)。
    cost_line: String,
    /// 数据来源 (`binance-rest` / `local-cache`)。
    data_source: String,
}

/// 敏感性报告文本 (纯函数, 便于单测)。
fn format_sensitivity_report(base: &SensitivityBase, axes: &[SensitivityAxis]) -> String {
    let mut out = String::new();
    out.push_str("=== 回测敏感性 (滑点/费用) ===\n");
    out.push_str(&base.header);
    out.push('\n');
    out.push_str(&format!(
        "  基准成本 (三层合并: 内置默认 < 策略 [backtest] < 本次覆盖): {}\n",
        base.cost_line
    ));
    out.push_str(&format!("  数据来源: {}\n", base.data_source));
    for axis in axes {
        out.push('\n');
        out.push_str(&format!(
            "--- {}敏感性 (bps{}) ---\n",
            axis.name,
            if axis.note.is_empty() { String::new() } else { format!(", {}", axis.note) }
        ));
        out.push_str(&sens_line(SENS_HEADERS.map(str::to_string)));
        out.push('\n');
        for row in &axis.rows {
            out.push_str(&sens_line(row.to_cells()));
            out.push('\n');
        }
        out.push_str(&format!("  {}\n", axis_summary(axis)));
    }
    out.push('\n');
    out.push_str(
        "说明: 每档都是一次完整回测(同一策略、同一数据窗口); 净盈亏为**成本后**口径(已含手续费与滑点);\n",
    );
    out.push_str("      「基准」行 = 未加敏感性覆盖的那一次(即默认 `ricow backtest` 的口径)。\n");
    out
}

/// 敏感性扫描入口: 先跑一次未覆盖的基准, 再按各轴阶梯逐档跑, 输出对照表。
///
/// 逐档都走 [`run_backtest_inner`](同一撮合与指标口径), 不复制任何回测逻辑;
/// **刻意不为每档落 run card** —— 扫描是探索手段, 归档证据请走单次回测(会落卡)。
pub(crate) async fn run_backtest_sensitivity(
    spec: BacktestRunSpec,
    slippage_ladder: Option<Vec<f64>>,
    fee_ladder: Option<Vec<f64>>,
) -> CoreResult<String> {
    let base = run_backtest_inner(&spec).await?;
    let base_info = SensitivityBase {
        header: base.header(&spec.strategy),
        cost_line: format!(
            "手续费 maker {} / taker {} bps · 滑点 {} bps",
            fmt_bps(base.params.fee_maker_bps),
            fmt_bps(base.params.fee_taker_bps),
            fmt_bps(base.params.slippage_bps),
        ),
        data_source: base.data_source.clone(),
    };
    let mut axes = Vec::new();
    if let Some(ladder) = slippage_ladder {
        let mut rows = vec![SensitivityRow::from_outcome("基准".into(), &base)];
        for bps in ladder {
            let mut level_spec = spec.clone();
            level_spec.slippage_bps = Some(bps);
            let out = run_backtest_inner(&level_spec).await?;
            rows.push(SensitivityRow::from_outcome(fmt_bps(bps), &out));
        }
        axes.push(SensitivityAxis { name: "滑点", note: "", rows });
    }
    if let Some(ladder) = fee_ladder {
        let mut rows = vec![SensitivityRow::from_outcome("基准".into(), &base)];
        for bps in ladder {
            let mut level_spec = spec.clone();
            // 该档**完全决定**费用: 清掉同义覆盖, maker/taker 同设 (与 `--fee` 同口径)。
            level_spec.fee = None;
            level_spec.fee_maker = Some(bps);
            level_spec.fee_taker = Some(bps);
            let out = run_backtest_inner(&level_spec).await?;
            rows.push(SensitivityRow::from_outcome(fmt_bps(bps), &out));
        }
        axes.push(SensitivityAxis { name: "费用", note: "maker=taker 同设", rows });
    }
    Ok(format_sensitivity_report(&base_info, &axes))
}

// ---- 回测 run card (035): 可复现证据落盘 ----
//
// 对标 Vibe-Trading 的 trust-layer run card: 把"这次回测用了什么策略源码 / 什么数据窗口 /
// 什么参数 / 得到什么指标"固化成一份可归档、可 diff 的 JSON, 服务 P4 dogfood 的证据留档。

/// run card 结构版本 (字段增删时 +1; 与 `specs/backtest.md` 同步)。
const RUN_CARD_SCHEMA_VERSION: u32 = 1;

/// 策略源码 SHA-256 (小写 hex)。
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 文件名安全化: 非 `[A-Za-z0-9._-]` 一律替换为 `_`。
fn sanitize_ident(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

/// 策略参数 JSON (剔除 `script` —— Lua 源码由 `source_sha256` 代表, 不重复落盘)。
fn params_json(config: &StrategyConfig) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for (k, v) in &config.params {
        if k == "script" {
            continue;
        }
        if let Ok(j) = serde_json::to_value(v) {
            out.insert(k.clone(), j);
        }
    }
    out
}

/// run card 的策略区。
#[derive(serde::Serialize)]
struct RunCardStrategy {
    /// 调用方给出的策略名 (内置 id 或已部署策略名)。
    name: String,
    /// 策略类型 (`lua` / 内置类型名)。
    #[serde(rename = "type")]
    kind: String,
    /// Lua 源码 SHA-256 (小写 hex); 无源码 (不应发生) 时为 `none`。
    source_sha256: String,
    /// Lua 源码字节数。
    source_bytes: usize,
}

/// run card 的数据窗口区。
#[derive(serde::Serialize)]
struct RunCardWindow {
    pair: String,
    interval: String,
    market: String,
    position_mode: String,
    /// 请求取数根数 (窗口 + 预热)。
    requested_bars: u32,
    warmup_bars: u32,
    /// 实际喂给引擎的 K 线根数 (裁掉终点 bar 后)。
    bars: usize,
    first_open_time_ms: Option<i64>,
    last_open_time_ms: Option<i64>,
    /// 窗口终点 (毫秒, 不含); None = 到"现在"。
    end_ms: Option<i64>,
    /// 数据来源 (`binance-rest`; 2.2 本地缓存落地后可命中 `local-cache`)。
    data_source: String,
}

/// run card 的指标区 (回测报告标量; Decimal 一律转字符串保精度)。
#[derive(serde::Serialize)]
struct RunCardMetrics {
    total_trades: u64,
    rejected_count: u64,
    net_pnl: String,
    realized_pnl: String,
    total_fees: String,
    win_rate: f64,
    max_drawdown: String,
    annual_return: Option<f64>,
    annual_volatility: Option<f64>,
    sharpe: Option<f64>,
    sortino: Option<f64>,
    calmar: Option<f64>,
    profit_factor: Option<f64>,
    turnover_ratio: f64,
    equity_change_pct: f64,
    benchmark_return_pct: Option<f64>,
}

/// 一次回测的可复现证据卡。
#[derive(serde::Serialize)]
struct RunCard {
    schema_version: u32,
    generated_at: String,
    engine_version: String,
    strategy: RunCardStrategy,
    params: serde_json::Map<String, serde_json::Value>,
    window: RunCardWindow,
    metrics: RunCardMetrics,
}

/// run card 保留份数上限 (审计 资源-5): 每次回测写一个时间戳 JSON, 文件数无限增长。
///
/// 保留最近 N 份(按文件名里的时间戳前缀排序, 即写入时间序), 更早的随写入顺手删掉。
/// 取 200: 足够回看近期对比(证据卡单文件很小), 又给目录一个明确上界。
const RUN_CARD_KEEP: usize = 200;

/// 把 run card 写到 `<root>/run/backtest/<ts>-<strategy>-<pair>.json`, 返回落盘路径。
fn write_run_card(root: &Path, card: &RunCard) -> CoreResult<PathBuf> {
    let dir = root.join("run").join("backtest");
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoreError::InvalidArgument(format!("创建 run card 目录失败: {e}")))?;
    let fname = format!(
        "{}-{}-{}.json",
        Utc::now().timestamp_millis(),
        sanitize_ident(&card.strategy.name),
        sanitize_ident(&card.window.pair)
    );
    let path = dir.join(fname);
    let json = serde_json::to_string_pretty(card)
        .map_err(|e| CoreError::Parse(format!("run card 序列化失败: {e}")))?;
    std::fs::write(&path, json)
        .map_err(|e| CoreError::InvalidArgument(format!("写 run card 失败: {e}")))?;
    // 顺手清理超量旧卡 (best-effort): 不影响本次写入结果, 失败只记日志。
    prune_run_cards(&dir);
    Ok(path)
}

/// 只保留最近 [`RUN_CARD_KEEP`] 份 run card, 更早的删除 (审计 资源-5)。返回删除数。
///
/// 文件名以 `timestamp_millis-` 开头 → 按名字**字符串排序**即等价于按写入时间排序
/// (同位数毫秒时间戳字典序 = 数值序), 无需解析时间。只认 `.json` 且以数字前缀命名的
/// 文件 —— 别的一概不碰(目录里若混入用户手放的东西不能误删)。
fn prune_run_cards(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut cards: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .filter(|p| {
            // 只认 `<数字>-...json` 形态(本函数自己写的卡), 别的 json 不碰。
            std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| {
                        n.split('-')
                            .next()
                            .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_digit()))
                    })
                    .unwrap_or(false)
        })
        .collect();
    if cards.len() <= RUN_CARD_KEEP {
        return 0;
    }
    cards.sort();
    let mut removed = 0;
    for p in &cards[..cards.len() - RUN_CARD_KEEP] {
        if std::fs::remove_file(p).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_util::ENV_LOCK;

    /// 审计 低危 #4: 根数上限校验 —— 常规窗口放行, 越界窗口报错且文案点名建议。
    #[test]
    fn test_check_bar_budget_allows_reasonable_and_rejects_huge() {
        // 90 天 1h ≈ 2160 根 → 放行。
        assert!(check_bar_budget(2_160, 90, "1h").is_ok());
        // 恰好在上限 → 放行(边界闭区间)。
        assert!(check_bar_budget(MAX_BACKTEST_BARS as u32, 90, "1m").is_ok());
        // 超上限一根 → 拒。
        let err = check_bar_budget(MAX_BACKTEST_BARS as u32 + 1, 100_000, "1m").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("回测窗口过大"), "{msg}");
        assert!(msg.contains("--days"), "报错应给出可执行的改法: {msg}");
        // 极端笔误: 100000 天 1m (u32 内可表示) 也应被挡。
        let huge = ((100_000f64) * 24.0 / (1.0 / 60.0)) as u32;
        assert!(check_bar_budget(huge, 100_000, "1m").is_err());
    }

    /// P0-2 回归(2026-10-05): 复刻实测口径 —— 744 根 klines(前 24 根预热) + 720 评测 bar。
    /// 价格必须取**尾部** 720 根与权益逐点对齐, 且丢掉曲线初始现金点。
    #[test]
    fn test_align_window_skips_front_warmup_and_drops_initial_equity() {
        let n_price = 744usize;
        let total_bars = 720usize;
        let closes: Vec<f64> = (0..n_price).map(|i| 100.0 + i as f64).collect();
        let times: Vec<i64> = (0..n_price).map(|i| 1_000_000 + i as i64 * 3_600_000).collect();
        // equity = [初始现金] + 720 个 per-bar 估值。
        let mut eq: Vec<Decimal> = vec![Decimal::from(10_000i64)];
        for i in 0..total_bars {
            eq.push(Decimal::from(10_000i64 + i as i64));
        }
        let (t, p, e) = BacktestOutcome::align_window(total_bars, &closes, &times, &eq);
        assert_eq!((t.len(), p.len(), e.len()), (total_bars, total_bars, total_bars));
        // 价格窗口 = 尾部 720 根(首个 = closes[24] 而非 closes[0])。
        assert_eq!(p[0], closes[24]);
        assert_eq!(t[0], times[24]);
        assert_eq!(p[p.len() - 1], closes[743]);
        assert_eq!(t[t.len() - 1], times[743]);
        // 权益丢初始点: 首值 = eq[1] = 初始现金; 末值 = 第 720 个 per-bar 估值。
        assert_eq!(e[0], 10_000.0);
        assert_eq!(e[e.len() - 1], 10_719.0);
    }

    /// 无预热段: 窗口 = 全部, 三序列等长且逐点同刻。
    #[test]
    fn test_align_window_no_warmup() {
        let closes: Vec<f64> = (0..5).map(|i| i as f64).collect();
        let times: Vec<i64> = (0..5).map(|i| i as i64).collect();
        let mut eq = vec![Decimal::from(0i64)];
        for i in 0..5 {
            eq.push(Decimal::from(i as i64));
        }
        let (t, p, e) = BacktestOutcome::align_window(5, &closes, &times, &eq);
        assert_eq!((t.len(), p.len(), e.len()), (5, 5, 5));
        assert_eq!(p[0], closes[0]);
        assert_eq!(t[0], times[0]);
        assert_eq!(e[0], 0.0);
    }

    /// 退化输入一律回空(前端据此不画图), 不 panic、不越界。
    #[test]
    fn test_align_window_degenerate_is_empty() {
        let (t, p, e) = BacktestOutcome::align_window(0, &[], &[], &[]);
        assert!(t.is_empty() && p.is_empty() && e.is_empty());
        // 只有初始点、无 per-bar 估值 → 0 点。
        let (t2, p2, e2) =
            BacktestOutcome::align_window(5, &[1.0, 2.0], &[1, 2], &[Decimal::from(1i64)]);
        assert!(t2.is_empty() && p2.is_empty() && e2.is_empty());
        // total_bars 大于价格序列 → 被夹到可用长度, 不越界。
        let (t3, p3, e3) = BacktestOutcome::align_window(
            999,
            &[1.0, 2.0, 3.0],
            &[10, 20, 30],
            &[Decimal::from(0i64), Decimal::from(1i64), Decimal::from(2i64), Decimal::from(3i64)],
        );
        assert_eq!((t3.len(), p3.len(), e3.len()), (3, 3, 3));
        assert_eq!(t3, vec![10i64, 20, 30]);
    }

    #[test]
    fn test_parse_param_bool_and_number() {
        // 回归(2026-09-18): true/false 必须解析成 Boolean —— 落到 String 时 Lua 的
        // ctx:config_bool 会恒读成 false, 导致 `--param xxx=true` 静默失效。
        assert!(matches!(
            parse_param("enter_at_start=true"),
            Some((_, ConfigValue::Boolean(true)))
        ));
        assert!(matches!(
            parse_param("enter_at_start=false"),
            Some((_, ConfigValue::Boolean(false)))
        ));
        assert!(matches!(parse_param("atr_mult=2.5"), Some((_, ConfigValue::Float(_)))));
        assert!(matches!(parse_param("pair=SOLUSDT"), Some((_, ConfigValue::String(_)))));
        assert!(parse_param("novalue").is_none());
    }

    #[test]
    // ENV_LOCK 串行化 RICOW_ROOT 的读写, 防止并行测试读到彼此的环境变量(见 mod.rs 的 ENV_LOCK 说明)。
    fn test_resolve_config_toml_priority() {
        // strategies/<name>.toml 命中 → TOML 加载; --param 透传覆盖 TOML 参数。
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", "/tmp/ricow-bt-test");
        let dir = crate::commands::ensure_strategies_dir().unwrap();
        std::fs::write(
            dir.join("demo.toml"),
            r#"
[strategy]
name = "demo"
type = "shannon_spot_grid"
enabled = true
exchange = "binance"

[strategy.params]
pair = "ETH"
order_size = 0.02
"#,
        )
        .unwrap();
        let _exchange = crate::commands::bn_exchange().unwrap();
        let args = BacktestArgs {
            strategy: "demo".into(),
            params: vec!["rebalance_band=0.01".into()],
            ..Default::default()
        };
        let cfg = resolve_config(&args).unwrap();
        assert_eq!(cfg.strategy_type, "lua", "内置名 TOML 应 Lua 化");
        assert_eq!(cfg.get_str("pair"), Some("ETH"));
        assert_eq!(cfg.get_f64("order_size"), Some(0.02));
        assert_eq!(cfg.get_f64("rebalance_band"), Some(0.01), "--param 应覆盖 TOML");
        assert!(cfg.get_str("script").unwrap().contains("on_tick"), "应注入内置脚本");
        std::env::remove_var("RICOW_ROOT");
    }

    #[test]
    // ENV_LOCK 串行化 RICOW_ROOT 的读写, 防止并行测试读到彼此的环境变量(见 mod.rs 的 ENV_LOCK 说明)。
    fn test_resolve_config_missing_pair_falls_back_to_args() {
        // TOML 无 pair → run 阶段用 --pair 兜底 (resolve_config 本身不报错)。
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", "/tmp/ricow-bt-test2");
        let dir = crate::commands::ensure_strategies_dir().unwrap();
        std::fs::write(
            dir.join("nopair.toml"),
            r#"
[strategy]
name = "nopair"
type = "shannon_spot_grid"
enabled = true
exchange = "binance"
"#,
        )
        .unwrap();
        let _exchange = crate::commands::bn_exchange().unwrap();
        let args = BacktestArgs {
            strategy: "nopair".into(),
            pair: Some("BNBUSDT".into()),
            ..Default::default()
        };
        let cfg = resolve_config(&args).unwrap();
        assert!(cfg.get_str("pair").is_none(), "TOML 无 pair 时 resolve 不注入");
        std::env::remove_var("RICOW_ROOT");
    }

    #[test]
    fn test_run_card_sha256_and_sanitize() {
        // 空串 SHA-256 已知向量。
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sanitize_ident("shannon_spot_grid"), "shannon_spot_grid");
        assert_eq!(sanitize_ident("ETH/USDT:x"), "ETH_USDT_x", "非法字符应替换为 _");
    }

    /// 审计 资源-5: run card 只保留最近 N 份, 更早的删掉; 非本函数产物不碰。
    #[test]
    fn test_prune_run_cards_keeps_latest_and_ignores_foreign_files() {
        let dir = std::env::temp_dir().join(format!(
            "ricow-runcard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // 造 RUN_CARD_KEEP + 10 份合法卡 (时间戳递增) + 2 份"外来文件"。
        let total = RUN_CARD_KEEP + 10;
        for i in 0..total {
            let ts = 1_700_000_000_000i64 + i as i64;
            std::fs::write(dir.join(format!("{ts}-s-ETHUSDT.json")), "{}").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "keep me").unwrap();
        std::fs::write(dir.join("manual.json"), "{}").unwrap();

        let removed = prune_run_cards(&dir);
        assert_eq!(removed, 10, "应删掉超量的 10 份");

        let cards: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        assert_eq!(cards.iter().filter(|n| n.ends_with("-s-ETHUSDT.json")).count(), RUN_CARD_KEEP);
        // 最早的 10 份被删, 最新的仍在。
        assert!(
            !cards.contains(&format!("{}-s-ETHUSDT.json", 1_700_000_000_000i64)),
            "最早的应被删"
        );
        assert!(
            cards.contains(&format!("{}-s-ETHUSDT.json", 1_700_000_000_000i64 + total as i64 - 1)),
            "最新的必须留下"
        );
        // 外来文件不动。
        assert!(cards.contains(&"notes.txt".to_string()), "非 json 不该碰");
        assert!(cards.contains(&"manual.json".to_string()), "非时间戳命名的 json 不该碰");

        // 未超量时一条不删。
        assert_eq!(prune_run_cards(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_run_card_params_json_excludes_script() {
        let mut params = HashMap::new();
        params.insert("pair".into(), ConfigValue::String("ETHUSDT".into()));
        params.insert("script".into(), ConfigValue::String("-- lua 源码".into()));
        params.insert("order_size".into(), ConfigValue::Float(0.02));
        let cfg = StrategyConfig {
            name: "t".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let j = params_json(&cfg);
        assert!(!j.contains_key("script"), "script 不应落盘 (由 source_sha256 代表)");
        assert_eq!(j.get("pair").and_then(|v| v.as_str()), Some("ETHUSDT"));
        assert_eq!(j.get("order_size").and_then(|v| v.as_f64()), Some(0.02));
    }

    // ---- 敏感性扫描 (036) ----

    /// 造一行敏感性结果 (只看净盈亏符号与展示)。
    fn sens_row(label: &str, net: i64) -> SensitivityRow {
        SensitivityRow {
            label: label.into(),
            total_trades: 1,
            net_pnl: Decimal::from(net),
            equity_change_pct: 0.0,
            max_drawdown: Decimal::ZERO,
            sharpe: None,
            win_rate: 0.0,
            total_fees: Decimal::ZERO,
        }
    }

    #[test]
    fn test_parse_ladder_defaults_and_errors() {
        // 未给开关 = 不扫。
        assert!(parse_ladder(None, DEFAULT_SLIPPAGE_LADDER).unwrap().is_none());
        // 只给开关不给档位 = 用默认阶梯。
        let d = parse_ladder(Some(""), DEFAULT_SLIPPAGE_LADDER).unwrap().unwrap();
        assert_eq!(d, vec![0.0, 5.0, 10.0]);
        let f = parse_ladder(Some("  "), DEFAULT_FEE_LADDER).unwrap().unwrap();
        assert_eq!(f, vec![5.0, 10.0, 20.0], "空白同样落默认");
        // 显式档位(含小数与多余空格)。
        let e = parse_ladder(Some(" 0, 2.5 ,10 "), DEFAULT_SLIPPAGE_LADDER).unwrap().unwrap();
        assert_eq!(e, vec![0.0, 2.5, 10.0]);
        // 单档不成敏感性。
        assert!(parse_ladder(Some("5"), DEFAULT_SLIPPAGE_LADDER).is_err());
        // 非数字 / 负数 / 默认档位被逗号清空 → 都当场报错, 不给假扫描。
        assert!(parse_ladder(Some("a,b"), DEFAULT_SLIPPAGE_LADDER).is_err());
        assert!(parse_ladder(Some("-1,5"), DEFAULT_SLIPPAGE_LADDER).is_err());
        assert!(parse_ladder(Some(","), DEFAULT_SLIPPAGE_LADDER).is_err());
    }

    #[test]
    fn test_axis_summary_steady_and_flip() {
        let steady_up = SensitivityAxis {
            name: "滑点",
            note: "",
            rows: vec![sens_row("基准", 100), sens_row("5", 40), sens_row("10", 3)],
        };
        assert!(axis_summary(&steady_up).contains("始终为正"));

        let steady_down = SensitivityAxis {
            name: "滑点",
            note: "",
            rows: vec![sens_row("基准", -100), sens_row("5", -40)],
        };
        assert!(axis_summary(&steady_down).contains("始终为负"));

        // 翻转: 点名翻转落在哪两档之间 (基准 → 10 bps)。
        let flip = SensitivityAxis {
            name: "费用",
            note: "maker=taker 同设",
            rows: vec![sens_row("基准", 100), sens_row("5", 20), sens_row("10", -30)],
        };
        let s = axis_summary(&flip);
        assert!(s.contains("翻转"), "{s}");
        assert!(s.contains("5 → 10 bps"), "{s}");
    }

    #[test]
    fn test_fmt_bps_and_display_width() {
        assert_eq!(fmt_bps(0.0), "0");
        assert_eq!(fmt_bps(10.0), "10");
        assert_eq!(fmt_bps(2.5), "2.50");
        // CJK 全角按 2 列算, 表头才能对齐。
        assert_eq!(display_width("档位"), 4);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(pad_display("档位", 8).chars().count(), 6, "2 个全角字 + 4 个补位空格");
        assert_eq!(display_width(&pad_display("档位", 8)), 8);
    }

    #[test]
    fn test_sensitivity_table_includes_axes_and_rows() {
        let base = SensitivityBase {
            header: "回测报告: demo ETHUSDT (90 天, 1h K 线, 现货)".into(),
            cost_line: "手续费 maker 10 / taker 10 bps · 滑点 0 bps".into(),
            data_source: "local-cache".into(),
        };
        let axes = vec![
            SensitivityAxis {
                name: "滑点",
                note: "",
                rows: vec![sens_row("基准", 100), sens_row("10", -5)],
            },
            SensitivityAxis {
                name: "费用",
                note: "maker=taker 同设",
                rows: vec![sens_row("10", -5)],
            },
        ];
        let text = format_sensitivity_report(&base, &axes);
        assert!(text.contains("回测敏感性"));
        assert!(text.contains("local-cache"), "数据来源要如实打印: {text}");
        assert!(text.contains("--- 滑点敏感性 (bps) ---"), "{text}");
        assert!(text.contains("--- 费用敏感性 (bps, maker=taker 同设) ---"), "{text}");
        // 两轴都在, 每轴都有表头行与结论段。
        assert_eq!(text.matches("档位").count(), 2);
        assert_eq!(text.matches("结论:").count(), 2);
        assert!(text.contains("净盈亏为**成本后**口径"));
    }
}
