//! `ricow backtest` — 命令行回测。

use std::collections::HashMap;

use chrono::Utc;
use clap::Args;
use ricow_core::{Balance, CoreError, CoreResult};
use ricow_engine::Engine;
use rust_decimal::Decimal;

use crate::commands::format_backtest_report;
use ricow_strategy::{BacktestParams, BacktestToml, ConfigValue, StrategyConfig};

/// `YYYY-MM-DD` -> 当日 00:00 UTC 毫秒 (回测窗口边界用)。
fn parse_ymd_ms(s: &str) -> CoreResult<i64> {
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| CoreError::InvalidArgument(format!("--start/--end 需 YYYY-MM-DD: {e}")))?;
    Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis())
}

#[derive(Args, Default)]
pub struct BacktestArgs {
    /// 策略类型 (shannon_rebalance/dca/twap/vwap/pullback/ladder/lua 或已部署策略名; 其余为执行模式示例, exec API 见 specs/lua-api.md)
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
    /// 初始现金 (quote)
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

/// Option<f64> 格式化: None → "n/a"。
/// 构造回测配置: 命中 strategies/<name>.toml → TOML 加载 (--param 透传覆盖);
/// 未命中 → 策略类型直跑 (旧逻辑, 默认参数)。
async fn resolve_config(
    args: &BacktestArgs,
    exchange: &std::sync::Arc<dyn ricow_core::Exchange>,
) -> CoreResult<StrategyConfig> {
    let strategies_dir = crate::commands::ensure_strategies_dir()?;
    let toml_path = strategies_dir.join(format!("{}.toml", args.strategy));
    if toml_path.exists() {
        let mut config = crate::commands::load_strategy_toml(&strategies_dir, &args.strategy)?;
        for p in &args.params {
            if let Some((k, v)) = parse_param(p) {
                config.params.insert(k, v);
            }
        }
        config.validate_risk()?;
        return Ok(config);
    }
    let config = inline_config(args, exchange).await?;
    // 风控参数校验 (004 FR-009): 非法值在此拒绝, 不留到运行期。
    config.validate_risk()?;
    Ok(config)
}

/// 直跑模式: 按策略类型构造内联配置。
async fn inline_config(
    args: &BacktestArgs,
    exchange: &std::sync::Arc<dyn ricow_core::Exchange>,
) -> CoreResult<StrategyConfig> {
    let pair = args.pair.clone().ok_or_else(|| {
        CoreError::InvalidArgument("直跑模式需要 --pair <pair> (或使用已部署策略名)".into())
    })?;

    // 网格区间围绕当前价 ±10% (与 run 直跑口径一致; 取不到盘口回退 3000)。
    let ref_price = exchange
        .get_orderbook(&pair, 1)
        .await
        .ok()
        .and_then(|ob| ob.mid_price())
        .unwrap_or_else(|| Decimal::from(3000));
    let lower = ref_price * Decimal::new(9, 1);
    let upper = ref_price * Decimal::new(11, 1);

    let mut params: HashMap<String, ConfigValue> = HashMap::new();
    params.insert("pair".into(), ConfigValue::String(pair.clone()));
    match args.strategy.as_str() {
        "shannon_rebalance" => {
            params.entry("order_size".into()).or_insert(ConfigValue::Float(0.01));
            params.entry("rebalance_band".into()).or_insert(ConfigValue::Float(0.005));
            params.entry("target_ratio".into()).or_insert(ConfigValue::Float(0.5));
            params.entry("atr_period".into()).or_insert(ConfigValue::Integer(14));
            params.entry("atr_mult".into()).or_insert(ConfigValue::Float(1.0));
        }
        "shannon_etf_accum" => {
            params.entry("atr_interval".into()).or_insert(ConfigValue::String("1h".into()));
            params.entry("atr_period".into()).or_insert(ConfigValue::Integer(14));
            params.entry("atr_mult".into()).or_insert(ConfigValue::Float(2.0));
            params.entry("ema_fast".into()).or_insert(ConfigValue::Integer(10));
            params.entry("ema_slow".into()).or_insert(ConfigValue::Integer(20));
            params.entry("target_ratio".into()).or_insert(ConfigValue::Float(0.5));
            params.entry("min_notional".into()).or_insert(ConfigValue::Float(5.0));
            params.entry("rehang_secs".into()).or_insert(ConfigValue::Integer(3600));
            params.entry("fee_bps".into()).or_insert(ConfigValue::Float(10.0));
            // 023 v3: 虚拟账本口径(真实 1 万 × 10 = 虚拟 10 万)+ 保本线 + 建仓后通道开关。
            params.entry("real_cash".into()).or_insert(ConfigValue::Float(10000.0));
            params.entry("leverage_mult".into()).or_insert(ConfigValue::Float(10.0));
            params.entry("min_spacing_pct".into()).or_insert(ConfigValue::Float(0.004));
            // 建仓后不再使用金叉/死叉(用户 2026-09-18 定稿): 默认只跑 平衡价 ± 2ATR 网格挂单;
            // true = 仅作历史对照(交叉通道市价进出, 不挂网格)。
            params.entry("enable_cross".into()).or_insert(ConfigValue::Boolean(false));
            // 日线趋势判据(用户 2026-09-18 定稿): BULL(`close > EMA200×1.03`) 可以买不卖 /
            // BEAR(`close < EMA200×0.97`) 可以卖不买 / RANGE(带内) 正常。判据序列 = 日线
            // (`ctx:close_tf` + `ctx:ema_tf`), 预热见下方 warmup。
            params.entry("regime_filter".into()).or_insert(ConfigValue::String("ema200".into()));
            params.entry("regime_interval".into()).or_insert(ConfigValue::String("1d".into()));
            params.entry("regime_ema_period".into()).or_insert(ConfigValue::Integer(200));
            params.entry("regime_band_pct".into()).or_insert(ConfigValue::Float(0.03));
            // 入口对齐开关: 第一根 K 线即建仓(不等金叉), 供不同粒度/参数对照回测
            params.entry("enter_at_start".into()).or_insert(ConfigValue::Boolean(false));
            // 旧 ER 判据的参数(§二十二 实测无效, 保留; 仅当显式设 `regime_filter=er` 时才生效)。
            params.entry("er_period".into()).or_insert(ConfigValue::Integer(20));
            params.entry("er_threshold".into()).or_insert(ConfigValue::Float(0.25));
            params.entry("regime_sma".into()).or_insert(ConfigValue::Integer(50));
            params.entry("leverage_basis".into()).or_insert(ConfigValue::String("cash".into()));
            // 信号通道与过滤器(2026-09-18 实测: RSI 优于金叉死叉; "价 < SMA(n) 不买"提升最大)
            params.entry("signal".into()).or_insert(ConfigValue::String("cross".into()));
            params.entry("rsi_n".into()).or_insert(ConfigValue::Float(14.0));
            params.entry("rsi_buy".into()).or_insert(ConfigValue::Float(30.0));
            params.entry("rsi_sell".into()).or_insert(ConfigValue::Float(70.0));
            params.entry("boll_n".into()).or_insert(ConfigValue::Float(20.0));
            params.entry("boll_dev".into()).or_insert(ConfigValue::Float(2.0));
            params.entry("trend_filter_sma".into()).or_insert(ConfigValue::Float(0.0));
        }
        "dca" => {
            params.insert("order_size".into(), ConfigValue::Float(0.01));
            params.insert("interval_secs".into(), ConfigValue::Integer(3600));
        }
        "twap" => {
            params.insert("total_size".into(), ConfigValue::Float(1.0));
            params.insert("num_slices".into(), ConfigValue::Integer(10));
            params.insert("slice_interval_secs".into(), ConfigValue::Integer(60));
        }
        "vwap" => {
            params.insert("total_size".into(), ConfigValue::Float(1.0));
            params.insert("num_slices".into(), ConfigValue::Integer(10));
            params.insert("slice_interval_secs".into(), ConfigValue::Integer(60));
        }
        "pullback" => {
            params.insert("pullback_pct".into(), ConfigValue::Float(0.03));
            params.insert("order_size".into(), ConfigValue::Float(0.01));
        }
        "ladder" => {
            params.insert("total_size".into(), ConfigValue::Float(1.0));
            params.insert("num_levels".into(), ConfigValue::Integer(10));
            params.insert("lower_price".into(), ConfigValue::String(lower.to_string()));
            params.insert("upper_price".into(), ConfigValue::String(upper.to_string()));
        }
        "lua" => {
            let script = args
                .script
                .as_ref()
                .ok_or_else(|| CoreError::InvalidArgument("lua 策略需要 --script <path>".into()))?;
            let code = std::fs::read_to_string(script)
                .map_err(|e| CoreError::InvalidArgument(format!("读取脚本失败: {e}")))?;
            params.insert("script".into(), ConfigValue::String(code));
        }
        other => {
            return Err(CoreError::InvalidArgument(format!("unsupported strategy: {other}")));
        }
    }
    // --param 透传覆盖默认 (必须在默认值插入之后; 曾置于 match 前被 dca 等分支的
    // params.insert 默认值覆盖, 用户参数静默失效)。
    for p in &args.params {
        if let Some((k, v)) = parse_param(p) {
            params.insert(k, v);
        }
    }

    crate::commands::resolve_builtin_script(StrategyConfig {
        name: format!("{}-{}", args.strategy, pair),
        strategy_type: args.strategy.clone(),
        enabled: true,
        exchange: "binance".into(),
        params,
        risk: None,
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    })
}

/// CLI 回测参数三层覆盖 (内置默认 < 策略 TOML [backtest] < CLI flags, 见 specs/backtest.md
/// §三): resolve 出全量有效值, futures 校验杠杆 (方案 A + L3), 把覆盖写回 config.params
/// (BacktestContext::new 内 resolve 后 params 池同名键覆盖生效 — 单次回测覆盖全链路)。
/// 单标的与 bs_momentum 组合入口共用 (2026-09-09 抽离)。
fn apply_backtest_cli(
    args: &BacktestArgs,
    config: &mut StrategyConfig,
) -> CoreResult<BacktestParams> {
    let mut ov = BacktestToml::default();
    if let Some(v) = args.fee {
        ov.fee_maker_bps = Some(v);
        ov.fee_taker_bps = Some(v);
    }
    if let Some(v) = args.fee_maker {
        ov.fee_maker_bps = Some(v);
    }
    if let Some(v) = args.fee_taker {
        ov.fee_taker_bps = Some(v);
    }
    if let Some(v) = args.slippage_bps {
        ov.slippage_bps = Some(v);
    }
    if let Some(v) = args.cash {
        ov.initial_cash = Some(v);
    }
    if let Some(v) = args.leverage {
        ov.leverage = Some(v);
    }
    if let Some(v) = args.max_leverage {
        ov.max_leverage = Some(v);
    }
    if let Some(v) = args.mmr_pct {
        ov.mmr_pct = Some(v);
    }
    if let Some(v) = args.funding_rate {
        ov.funding_rate_8h = Some(v);
    }
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

pub(crate) async fn run_backtest(
    args: BacktestArgs,
) -> CoreResult<(String, ricow_strategy::BacktestReport)> {
    let days = args.days.unwrap_or(90);
    let interval = args.interval.clone().unwrap_or_else(|| "1h".to_string());
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
    // --start/--end: 显式窗口 (自然年月分段); --end 缺省 = 现在。
    let end_ms: Option<i64> = match args.end.as_deref() {
        Some(d) => Some(parse_ymd_ms(d)?),
        None => None,
    };
    let (days, end_ms) = match args.start.as_deref() {
        Some(s) => {
            let s_ms = parse_ymd_ms(s)?;
            let e_ms = end_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
            let d = (((e_ms - s_ms) as f64) / 86_400_000.0).round().max(1.0) as u32;
            (d, Some(e_ms))
        }
        None => (days, end_ms),
    };
    let limit = ((days as f64) * 24.0 / hours_per_bar) as u32;

    let exchange = crate::commands::bn_exchange()?;
    let mut config = resolve_config(&args, &exchange).await?;
    // CLI 覆盖 market/position_mode (三层最上层; market 同时决定数据源分支, 见下)。
    if let Some(m) = &args.market {
        if m != "spot" && m != "futures" {
            return Err(CoreError::InvalidArgument(format!(
                "--market 仅支持 spot|futures, 收到 '{m}'"
            )));
        }
        config.market = m.clone();
    }
    if let Some(p) = &args.position_mode {
        if p != "one-way" && p != "hedge" {
            return Err(CoreError::InvalidArgument(format!(
                "--position-mode 仅支持 one-way|hedge, 收到 '{p}'"
            )));
        }
        config.position_mode = p.clone();
    }
    // 三层回测参数 (内置默认 < 策略 TOML [backtest] < CLI): 解析出全量有效值 + 写回 params。
    let params = apply_backtest_cli(&args, &mut config)?;
    // 023 高周期预热: 策略声明 `atr_interval`(网格间距) 与/或 `regime_interval`(日线趋势判据) 时,
    // 多取主序列做预热, 使高周期指标从窗口首根起就就绪(预热段由引擎跳过, 不进 tick 循环与报告)。
    //   · ATR: 24h 或 (period+1) 根高周期 bar;
    //   · 趋势判据 EMA: 3×(period+1) 根高周期 bar —— `ta` 的 EMA 用**首值种**, 种子残差 =
    //     (1−2/(n+1))^k ⇒ n=200 时: 201 根 13.5% / 402 根 1.8% / 603 根 0.25%;
    //     ±3% 带下 13.5% 的漂移会判错边界, 故取 3×(period+1)=603 天。交易所历史不足时
    //     分页自然取到多少算多少(预热段只喂指标, 不影响报告窗口)。
    let atr_warmup = if let Some(atr_iv) = config.get_str("atr_interval") {
        // 24h 覆盖 1h ATR(24 根 ≥ 14 根); 4h/1d 主序列上 24h 只有 1~6 根 → 按
        // "ATR 就绪所需 (period+1) 根" 取上限, 避免开窗前 15 根空转(2026-09-18)。
        let period = config.get_f64("atr_period").unwrap_or(14.0);
        let atr_ms = ricow_strategy::tf_ms_of(atr_iv).unwrap_or(3_600_000) as f64;
        let bar_ms = hours_per_bar * 3_600_000.0;
        let need = (((period + 1.0) * atr_ms / bar_ms).ceil() as u32).max(1);
        need.max(((24.0 / hours_per_bar) as u32).max(1))
    } else {
        0
    };
    // 趋势判据只在 `regime_filter = ema200` 时需要日线序列 —— off/er 不额外取数。
    let regime_warmup = if matches!(config.get_str("regime_filter"), Some("ema200")) {
        match config.get_str("regime_interval") {
            Some(tf) => {
                let period = config.get_f64("regime_ema_period").unwrap_or(200.0);
                let tf_ms = ricow_strategy::tf_ms_of(tf).unwrap_or(86_400_000) as f64;
                let bar_ms = hours_per_bar * 3_600_000.0;
                (((3.0 * (period + 1.0)) * tf_ms / bar_ms).ceil() as u32).max(1)
            }
            None => 0,
        }
    } else {
        0
    };
    let warmup_bars = atr_warmup.max(regime_warmup);
    if warmup_bars > 0 {
        config.params.insert("warmup_bars".into(), ConfigValue::Integer(warmup_bars as i64));
    }
    let fetch_limit = limit + warmup_bars;
    // TOML 策略缺 pair 时用 --pair 兜底; 两者皆无报错。
    if config.get_str("pair").is_none() {
        let pair = args.pair.clone().ok_or_else(|| {
            CoreError::InvalidArgument("TOML 策略缺 pair 参数, 需 --pair <pair>".into())
        })?;
        config.params.insert("pair".into(), ConfigValue::String(pair));
    }
    let pair = config.get_str("pair").unwrap().to_string();
    // 数据源分支 (三层配置的 market 决定, specs/backtest.md §五): 合约用 fapi 公共数据源,
    // 现货沿用交易所客户端。K 线 JSON 同构, 直接喂同一回测引擎。
    let klines = if config.market == "futures" {
        let fapi = ricow_binance::FuturesDataClient::new()?;
        // MMR 元数据: 用户未显式 --mmr-pct 时, 按 symbol 查内置首档表 (exchangeInfo 公共值不可靠,
        // 见 specs/backtest.md §十一 T7); 表外回落 1.0%。
        if args.mmr_pct.is_none() {
            config
                .params
                .insert("mmr_pct".into(), ConfigValue::Float(ricow_binance::tier1_mmr_pct(&pair)));
        }
        match end_ms {
            Some(e) => fapi.get_klines_ending_at(&pair, &interval, fetch_limit, e).await?,
            None => fapi.get_klines(&pair, &interval, fetch_limit).await?,
        }
    } else {
        match end_ms {
            Some(e) => exchange.get_klines_until(&pair, &interval, fetch_limit, e).await?,
            None => exchange.get_klines(&pair, &interval, fetch_limit).await?,
        }
    };
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
        let available = klines.partition_point(|k| k.open_time.timestamp_millis() < start_ms) as u32;
        let actual = warmup_bars.min(available);
        if actual != warmup_bars {
            tracing::warn!(
                target: "multiframe",
                requested = warmup_bars,
                actual = actual,
                "交易所历史不足以填满预热段 → 按实际可取根数裁剪(不缩短报告窗口)"
            );
            config.params.insert("warmup_bars".into(), ConfigValue::Integer(actual as i64));
        }
    }

    let initial_cash = Decimal::from_f64_retain(params.initial_cash)
        .ok_or_else(|| CoreError::InvalidArgument("initial_cash 非法".into()))?;
    let initial_balance =
        Balance { asset: "USDT".into(), free: initial_cash, locked: Decimal::ZERO };

    let is_futures = config.market == "futures";
    // 生效 MMR (报告显示用): 三层解析 + 可能的交易所首档拉取已写回 params; config move 前取出。
    // (D3: 此前打印恒 2.5, --mmr-pct/TOML 覆盖不反映。)
    let effective_mmr_pct = config.get_f64("mmr_pct").unwrap_or(1.0);
    let report = Engine::new().backtest(config, initial_balance, &klines)?;

    let text = format_backtest_report(
        &report,
        &format!(
            "回测报告: {} {pair} ({days} 天, {interval} K 线, {})",
            args.strategy,
            if is_futures {
                format!(
                    "合约 USDT-M · {} 持仓 · {:.0}x · MMR {:.2}%",
                    report.position_mode.as_deref().unwrap_or("one-way"),
                    report.leverage.unwrap_or(1.0),
                    effective_mmr_pct
                )
            } else {
                "现货".to_string()
            }
        ),
        initial_cash,
        is_futures,
    );

    Ok((text, report))
}

/// CLI 入口: 跑回测并打印报告(与 AI 工具 `run_backtest` 共用同一主体与同一份格式化)。
pub async fn run(args: BacktestArgs) -> CoreResult<()> {
    let (text, _report) = run_backtest(args).await?;
    print!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_util::ENV_LOCK;

    #[test]
    fn test_parse_param_bool_and_number() {
        // 回归(2026-09-18): true/false 必须解析成 Boolean —— 落到 String 时 Lua 的
        // ctx:config_bool 会恒读成 false, 导致 `--param xxx=true` 静默失效。
        assert!(matches!(
            parse_param("enter_at_start=true"),
            Some((_, ConfigValue::Boolean(true)))
        ));
        assert!(matches!(
            parse_param("enable_cross=FALSE"),
            Some((_, ConfigValue::Boolean(false)))
        ));
        assert!(matches!(parse_param("atr_mult=2.5"), Some((_, ConfigValue::Float(_)))));
        assert!(matches!(parse_param("pair=SOLUSDT"), Some((_, ConfigValue::String(_)))));
        assert!(parse_param("novalue").is_none());
    }

    #[tokio::test]
    // 测试专用: ENV_LOCK 串行化 RICOW_ROOT 的读写; 这里**有意**跨 await 持有整个测试体,
    // 否则并行测试会读到彼此的环境变量(见 mod.rs 的 ENV_LOCK 说明)。
    #[allow(clippy::await_holding_lock)]
    async fn test_resolve_config_toml_priority() {
        // strategies/<name>.toml 命中 → TOML 加载; --param 透传覆盖 TOML 参数。
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", "/tmp/ricow-bt-test");
        let dir = crate::commands::ensure_strategies_dir().unwrap();
        std::fs::write(
            dir.join("demo.toml"),
            r#"
[strategy]
name = "demo"
type = "shannon_rebalance"
enabled = true
exchange = "binance"

[strategy.params]
pair = "ETH"
order_size = 0.02
"#,
        )
        .unwrap();
        let exchange = crate::commands::bn_exchange().unwrap();
        let args = BacktestArgs {
            start: None,
            end: None,
            strategy: "demo".into(),
            pair: None,
            days: None,
            interval: None,
            script: None,
            params: vec!["rebalance_band=0.01".into()],
            fee: None,
            fee_maker: None,
            fee_taker: None,
            slippage_bps: None,
            cash: None,
            leverage: None,
            max_leverage: None,
            mmr_pct: None,
            funding_rate: None,
            market: None,
            position_mode: None,
        };
        let cfg = resolve_config(&args, &exchange).await.unwrap();
        assert_eq!(cfg.strategy_type, "lua", "内置名 TOML 应 Lua 化");
        assert_eq!(cfg.get_str("pair"), Some("ETH"));
        assert_eq!(cfg.get_f64("order_size"), Some(0.02));
        assert_eq!(cfg.get_f64("rebalance_band"), Some(0.01), "--param 应覆盖 TOML");
        assert!(cfg.get_str("script").unwrap().contains("on_tick"), "应注入内置脚本");
        std::env::remove_var("RICOW_ROOT");
    }

    #[tokio::test]
    // 测试专用: ENV_LOCK 串行化 RICOW_ROOT 的读写; 这里**有意**跨 await 持有整个测试体,
    // 否则并行测试会读到彼此的环境变量(见 mod.rs 的 ENV_LOCK 说明)。
    #[allow(clippy::await_holding_lock)]
    async fn test_resolve_config_missing_pair_falls_back_to_args() {
        // TOML 无 pair → run 阶段用 --pair 兜底 (resolve_config 本身不报错)。
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", "/tmp/ricow-bt-test2");
        let dir = crate::commands::ensure_strategies_dir().unwrap();
        std::fs::write(
            dir.join("nopair.toml"),
            r#"
[strategy]
name = "nopair"
type = "shannon_rebalance"
enabled = true
exchange = "binance"
"#,
        )
        .unwrap();
        let exchange = crate::commands::bn_exchange().unwrap();
        let args = BacktestArgs {
            start: None,
            end: None,
            strategy: "nopair".into(),
            pair: Some("BNBUSDT".into()),
            days: None,
            interval: None,
            script: None,
            params: vec![],
            fee: None,
            fee_maker: None,
            fee_taker: None,
            slippage_bps: None,
            cash: None,
            leverage: None,
            max_leverage: None,
            mmr_pct: None,
            funding_rate: None,
            market: None,
            position_mode: None,
        };
        let cfg = resolve_config(&args, &exchange).await.unwrap();
        assert!(cfg.get_str("pair").is_none(), "TOML 无 pair 时 resolve 不注入");
        std::env::remove_var("RICOW_ROOT");
    }
}
