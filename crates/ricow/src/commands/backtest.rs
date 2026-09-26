//! `ricow backtest` — 命令行回测。

use std::collections::HashMap;

use chrono::Utc;
use clap::Args;
use ricow_core::{Balance, CoreError, CoreResult};
use ricow_engine::Engine;
use rust_decimal::Decimal;

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
    /// K 线数据源市场 (spot|futures; 缺省跟随策略 market)。仅影响拉哪套 K 线,
    /// 不改策略/引擎的 market 语义 —— 现货↔合约同 symbol 价格有 ~几 bps 基差,
    /// 等价性对照 (现货策略 vs 期货只多头) 必须喂同一序列才能逐笔对齐 (032)。
    #[arg(long = "klines-market")]
    pub klines_market: Option<String>,
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
    /// 期末强制平仓: 回测收尾按期末价平掉所有方向仓, 报告净盈亏为"已实现、干净"口径
    /// (032+ 可观测性; 默认 false = 行为与历史逐位一致)。
    #[arg(long = "close-at-end")]
    pub close_at_end: bool,
    /// 导出目录: 回测参数 / 逐笔成交 / 权益曲线 / 报告全文写入该目录 (032+ 可观测性)。
    #[arg(long = "export-dir")]
    pub export_dir: Option<String>,
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
async fn resolve_config(args: &BacktestArgs) -> CoreResult<StrategyConfig> {
    let strategies_dir = crate::commands::ensure_strategies_dir()?;
    let toml_path = strategies_dir.join(format!("{}.toml", args.strategy));
    if toml_path.exists() {
        let mut config = crate::commands::load_strategy_toml(&strategies_dir, &args.strategy)?;
        for p in &args.params {
            if let Some((k, v)) = parse_param(p) {
                config.params.insert(k, v);
            }
        }
        return Ok(config);
    }
    let config = inline_config(args).await?;
    Ok(config)
}

/// 直跑模式: 只传运行环境信息(pair)与 lua 脚本(--script)。
/// 策略参数全部由 Lua 策略自己的 fallback 默认值决定 —— 不在这里写任何策略参数名/默认值
/// (目标 3: 新增/修改策略不改项目代码)。
async fn inline_config(args: &BacktestArgs) -> CoreResult<StrategyConfig> {
    let pair = args.pair.clone().ok_or_else(|| {
        CoreError::InvalidArgument("直跑模式需要 --pair <pair> (或使用已部署策略名)".into())
    })?;

    let mut params: HashMap<String, ConfigValue> = HashMap::new();
    params.insert("pair".into(), ConfigValue::String(pair.clone()));
    if args.strategy.as_str() == "lua" {
        let script = args
            .script
            .as_ref()
            .ok_or_else(|| CoreError::InvalidArgument("lua 策略需要 --script <path>".into()))?;
        let code = std::fs::read_to_string(script)
            .map_err(|e| CoreError::InvalidArgument(format!("读取脚本失败: {e}")))?;
        params.insert("script".into(), ConfigValue::String(code));
    }

    // --param 透传覆盖(用户显式传参, 非写死默认值)。
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
    let mut config = resolve_config(&args).await?;
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
    if let Some(m) = &args.klines_market {
        if m != "spot" && m != "futures" {
            return Err(CoreError::InvalidArgument(format!(
                "--klines-market 仅支持 spot|futures, 收到 '{m}'"
            )));
        }
    }
    // 三层回测参数 (内置默认 < 策略 TOML [backtest] < CLI): 解析出全量有效值 + 写回 params。
    let params = apply_backtest_cli(&args, &mut config)?;
    // --interval 是主时钟粒度(通用配置, 与 pair 同类): 写入 params 供策略 need_klines("primary", ...)
    // 声明使用。用户显式 --param interval 优先(不覆盖)。若不写, 策略 primary 声明会 fallback "1h",
    // 与 --interval 拉的 K 线粒度错位 → warmup 换算错 → 高周期指标永不就绪(实测 0 成交)。
    // 清单默认值注入 (032 B) 在 resolve_builtin_script 里先落了 interval 默认 → CLI **显式**给出的
    // --interval 必须再覆盖它 (CLI 运行时配置 > 清单展示默认; 实测 2026-09-26: 不覆盖则
    // `--interval 1m` 被清单 "1h" 压制, 回测按 1h 拉线)。
    if args.interval.is_some() {
        config.params.insert("interval".into(), ConfigValue::String(interval.clone()));
    } else {
        config.params.entry("interval".into()).or_insert(ConfigValue::String(interval.clone()));
    }
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
    // 分页取数 (2026-09-22, 030): 币安 K 线**单次请求上限 1000 根** —— 超过必须向前翻页拼接,
    // 否则长窗口 (1m 数天 / 1h 数月) 会拿到错误响应: 表现为 "network error: error decoding
    // response body" (120 天 1m) 或长时间无输出 (3 天/1 天 1m 实测)。此处按 1000 根/批往前翻页。
    const KLINE_PAGE_MAX: u32 = 1000;
    // 数据源市场: 默认跟随策略 market; --klines-market 仅解耦拉数 (策略/引擎语义不变)。
    let kline_market = args.klines_market.as_deref().unwrap_or(&config.market);
    let fapi = if kline_market == "futures" {
        let f = ricow_binance::FuturesDataClient::new()?;
        // MMR 元数据: 用户未显式 --mmr-pct 时, 按 symbol 查内置首档表 (exchangeInfo 公共值不可靠,
        // 见 specs/backtest.md §十一 T7); 表外回落 1.0%。
        if args.mmr_pct.is_none() && config.market == "futures" {
            config
                .params
                .insert("mmr_pct".into(), ConfigValue::Float(ricow_binance::tier1_mmr_pct(&pair)));
        }
        Some(f)
    } else {
        None
    };
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
    let klines = acc;
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
    // (D3: 此前打印恒 2.5, --mmr-pct/TOML 覆盖不反映。)
    let effective_mmr_pct = config.get_f64("mmr_pct").unwrap_or(1.0);
    let report =
        Engine::new().backtest(config.clone(), initial_balance, &klines, args.close_at_end)?;

    // 测试参数区块 (032+ 可观测性): 生效配置全量通用 dump (剔除 script 源码, 太长)。
    let mut test_params: Vec<(String, String)> = config
        .params
        .iter()
        .filter(|(k, _)| k.as_str() != "script")
        .map(|(k, v)| (k.clone(), config_value_repr(v)))
        .collect();
    test_params.push(("market".into(), config.market.clone()));
    test_params.push(("position_mode".into(), config.position_mode.clone()));
    // 回测窗口参数用 bt_ 前缀: 避免与策略自有参数同名撞键 (如网格策略的 interval 主时钟)。
    test_params.push(("bt_days".into(), days.to_string()));
    test_params.push(("bt_interval".into(), interval.clone()));
    test_params.push(("bt_close_at_end".into(), args.close_at_end.to_string()));

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
        &test_params,
    );

    // 导出 (032+ 可观测性, --export-dir): 参数 / 逐笔成交 / 权益曲线 / 报告全文。
    if let Some(dir) = &args.export_dir {
        export_backtest(dir, &args.strategy, &pair, &config, &klines, &report, &text)?;
    }

    Ok((text, report))
}

/// `ConfigValue` → 展示字符串 (报告"测试参数"区块用)。
fn config_value_repr(v: &ConfigValue) -> String {
    match v {
        ConfigValue::String(s) => s.clone(),
        ConfigValue::Float(f) => f.to_string(),
        ConfigValue::Integer(i) => i.to_string(),
        ConfigValue::Boolean(b) => b.to_string(),
    }
}

/// 把回测输入参数与输出明细导出到目录 (032+ 可观测性):
/// - `params.json` —— 生效配置全量 (market/position_mode/params) + 窗口与 K 线根数;
/// - `fills.csv` —— 逐笔成交 (时间戳 = bar 时间, 见 032+ 修复);
/// - `equity.csv` —— 逐 bar 收盘权益曲线;
/// - `report.txt` —— 报告全文。
fn export_backtest(
    dir: &str,
    strategy: &str,
    pair: &str,
    config: &StrategyConfig,
    klines: &[ricow_core::Kline],
    report: &ricow_strategy::BacktestReport,
    text: &str,
) -> CoreResult<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| CoreError::InvalidArgument(format!("导出目录创建失败 {dir}: {e}")))?;
    let stem = format!("{strategy}_{pair}");

    // params.json
    let mut params_map = serde_json::Map::new();
    for (k, v) in &config.params {
        let jv = match v {
            ConfigValue::String(s) => serde_json::Value::String(s.clone()),
            ConfigValue::Float(f) => serde_json::json!(f),
            ConfigValue::Integer(i) => serde_json::json!(i),
            ConfigValue::Boolean(b) => serde_json::Value::Bool(*b),
        };
        params_map.insert(k.clone(), jv);
    }
    let root = serde_json::json!({
        "strategy": strategy,
        "pair": pair,
        "market": config.market,
        "position_mode": config.position_mode,
        "kline_bars": klines.len(),
        "kline_first_open_time": klines.first().map(|k| k.open_time.to_rfc3339()),
        "kline_last_close_time": klines.last().map(|k| k.close_time.to_rfc3339()),
        "close_at_end_applied": report.close_at_end_applied,
        "params": serde_json::Value::Object(params_map),
    });
    std::fs::write(
        std::path::Path::new(dir).join(format!("{stem}_params.json")),
        serde_json::to_string_pretty(&root).unwrap_or_default(),
    )
    .map_err(|e| CoreError::InvalidArgument(format!("导出 params.json 失败: {e}")))?;

    // fills.csv
    let mut csv = String::from(
        "index,timestamp,pair,side,position_side,fill_price,fill_size,fee,client_order_id,exchange_order_id\n",
    );
    for (i, f) in report.fills.iter().enumerate() {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            i,
            f.timestamp.to_rfc3339(),
            f.pair,
            match f.side {
                ricow_core::OrderSide::Buy => "buy",
                ricow_core::OrderSide::Sell => "sell",
            },
            f.position_side.clone().unwrap_or_default(),
            f.fill_price,
            f.fill_size,
            f.fee,
            f.client_order_id,
            f.exchange_order_id
        ));
    }
    std::fs::write(std::path::Path::new(dir).join(format!("{stem}_fills.csv")), csv)
        .map_err(|e| CoreError::InvalidArgument(format!("导出 fills.csv 失败: {e}")))?;

    // equity.csv
    let mut eq = String::from("bar_index,equity\n");
    for (i, e) in report.equity_curve.iter().enumerate() {
        eq.push_str(&format!("{i},{e}\n"));
    }
    std::fs::write(std::path::Path::new(dir).join(format!("{stem}_equity.csv")), eq)
        .map_err(|e| CoreError::InvalidArgument(format!("导出 equity.csv 失败: {e}")))?;

    // report.txt
    std::fs::write(std::path::Path::new(dir).join(format!("{stem}_report.txt")), text)
        .map_err(|e| CoreError::InvalidArgument(format!("导出 report.txt 失败: {e}")))?;

    tracing::info!(target: "backtest", "回测导出完成: {dir}/{stem}_* (params.json / fills.csv / equity.csv / report.txt)");
    Ok(())
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
            parse_param("enter_at_start=false"),
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
            close_at_end: false,
            export_dir: None,
            klines_market: None,
        };
        let cfg = resolve_config(&args).await.unwrap();
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
type = "shannon_spot_grid"
enabled = true
exchange = "binance"
"#,
        )
        .unwrap();
        let _exchange = crate::commands::bn_exchange().unwrap();
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
            close_at_end: false,
            export_dir: None,
            klines_market: None,
        };
        let cfg = resolve_config(&args).await.unwrap();
        assert!(cfg.get_str("pair").is_none(), "TOML 无 pair 时 resolve 不注入");
        std::env::remove_var("RICOW_ROOT");
    }

    #[test]
    fn test_export_backtest_writes_four_files() {
        // 032+ 可观测性: export_backtest 落盘 params.json / fills.csv / equity.csv / report.txt。
        use ricow_core::{Kline, OrderFill, OrderSide};
        use rust_decimal_macros::dec;
        let dir = "/tmp/ricow-export-test";
        let _ = std::fs::remove_dir_all(dir);
        let mut config = StrategyConfig {
            name: "demo".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "futures".into(),
            position_mode: "hedge".into(),
            backtest: None,
        };
        config.params.insert("pair".into(), ConfigValue::String("SOLUSDT".into()));
        config.params.insert("order_amount".into(), ConfigValue::Float(100.0));
        let ts = chrono::DateTime::from_timestamp(3600, 0).unwrap();
        let klines = vec![Kline {
            open_time: ts,
            open: Decimal::ONE,
            high: Decimal::ONE,
            low: Decimal::ONE,
            close: Decimal::ONE,
            volume: Decimal::ONE,
            close_time: ts + chrono::Duration::hours(1),
        }];
        let mut report = ricow_strategy::BacktestReport::default();
        report.fills.push(OrderFill {
            trade_id: None,
            exchange_order_id: "e1".into(),
            client_order_id: "c1".into(),
            pair: "SOLUSDT".into(),
            side: OrderSide::Buy,
            fill_price: dec!(100),
            fill_size: dec!(1),
            fee: dec!(0.05),
            timestamp: ts,
            position_side: Some("long".into()),
        });
        report.equity_curve = vec![dec!(1000), dec!(1001)];
        let res = export_backtest(dir, "demo", "SOLUSDT", &config, &klines, &report, "报告全文");
        assert!(res.is_ok(), "导出应成功: {:?}", res.err());
        let params: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{dir}/demo_SOLUSDT_params.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(params["strategy"], "demo");
        assert_eq!(params["pair"], "SOLUSDT");
        assert_eq!(params["market"], "futures");
        assert_eq!(params["kline_bars"], 1);
        assert_eq!(params["params"]["order_amount"], 100.0);
        let fills = std::fs::read_to_string(format!("{dir}/demo_SOLUSDT_fills.csv")).unwrap();
        assert!(fills.starts_with("index,timestamp,pair,side,position_side"), "CSV 表头");
        assert!(fills.contains(",100,1,0.05,c1,e1\n"), "逐笔行含价格/数量/费/id");
        let eq = std::fs::read_to_string(format!("{dir}/demo_SOLUSDT_equity.csv")).unwrap();
        assert_eq!(eq, "bar_index,equity\n0,1000\n1,1001\n");
        assert_eq!(
            std::fs::read_to_string(format!("{dir}/demo_SOLUSDT_report.txt")).unwrap(),
            "报告全文"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
