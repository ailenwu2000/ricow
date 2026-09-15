//! `ricow create` — AI/用户提交 Lua 策略 (002): 编译门禁 → 真实 K 线沙箱回测 → 两步确认 preview。
//!
//! 三步链路 (写操作**不得一步落盘**):
//! 1. `ricow create --name <n> --pair <p> [--script <file|->]` —— 本命令: 只产出 preview, 不写任何策略文件
//! 2. `ricow approve <preview_id>` —— 用户批准, 得一次性 token (15 分钟有效)
//! 3. `ricow deploy <preview_id> --token <t>` —— 落盘 `strategies/<name>.toml` + `<name>.lua`
//!
//! 代码来源支持 AI 响应全文(带 ``` 围栏也可), `extract_code` 会剥出代码块。
//!
//! 内核 `create_preview()` 与 AI 工具 `preview_strategy` **共用同一实现**(019 T019 / FR-016 同口径):
//! CLI 打印与 AI 工具返回的报告文本都出自同一次 `format_backtest_report`。

use std::collections::HashMap;
use std::io::Read;

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_engine::Engine;
use ricow_strategy::{ConfigValue, Database};
use rust_decimal::Decimal;

use crate::commands::backtest::parse_param;
use crate::commands::{default_db_path, format_backtest_report};

#[derive(Args)]
pub struct CreateArgs {
    /// 策略名 (部署文件名, 需唯一)
    #[arg(long)]
    pub name: String,
    /// 交易对 (现货如 ETHUSDT; bStock 现货形如 <代码>BUSDT)
    #[arg(long)]
    pub pair: String,
    /// Lua 文件路径; 省略或 `-` = 从 stdin 读 (可直接喂 AI 响应全文)
    #[arg(long)]
    pub script: Option<String>,
    /// 策略参数透传 (可重复): --param order_size=0.01
    #[arg(long = "param")]
    pub params: Vec<String>,
    /// 市场 (spot|futures; futures 走 fapi 公共数据源)
    #[arg(long, default_value = "spot")]
    pub market: String,
    /// 沙箱回测天数 (默认 90)
    #[arg(long)]
    pub days: Option<u32>,
    /// K 线间隔 (1m/5m/15m/1h/4h/1d, 默认 1h)
    #[arg(long)]
    pub interval: Option<String>,
}

/// 创建预览的产出: 报告文本(与 CLI 同一渲染) + preview_id(15 分钟一次性 token)。
pub struct PreviewOutcome {
    pub report_text: String,
    pub preview_id: String,
}

/// **创建内核**(019 T019): 名字规范 → 参数/市场校验 → 编译门禁 → 真实 K 线 → 沙箱回测 → preview。
///
/// - **零落盘**: 只写 `previews` 记录, 绝不写策略文件; 落盘必须经 `approve` + `deploy`。
/// - 与 CLI `ricow create` 同一实现, AI 工具 `preview_strategy` 直接复用它(FR-016 同口径)。
pub async fn create_preview(
    name: &str,
    raw_code: &str,
    pair: &str,
    market: &str,
    params: Vec<(String, ConfigValue)>,
    days: u32,
    interval: &str,
) -> CoreResult<PreviewOutcome> {
    // 策略名规范(创建期第一道防线; 部署期 execute_strategy 用同一校验器复核)
    ricow_strategy::validate_strategy_name(name).map_err(CoreError::InvalidArgument)?;

    // ② 参数与市场校验
    let mut param_map: HashMap<String, ConfigValue> = HashMap::new();
    for (k, v) in params {
        param_map.insert(k, v);
    }
    if market != "spot" && market != "futures" {
        return Err(CoreError::InvalidArgument(format!(
            "market 仅支持 spot|futures, 收到 '{market}'"
        )));
    }

    // ③ 编译门禁 (第一关) —— 不通过即退出: 不拉 K 线、不产生 preview、不写文件
    let mut config = ricow_engine::create_strategy(name, raw_code, pair, param_map)
        .map_err(CoreError::InvalidArgument)?;
    config.market = market.to_string();

    // ④ 真实 K 线 (与 `ricow backtest` 同口径的数据源分支; 现货/合约公共端点, 免 key)
    let hours_per_bar = match interval {
        "1m" => 1.0 / 60.0,
        "5m" => 5.0 / 60.0,
        "15m" => 0.25,
        "1h" => 1.0,
        "4h" => 4.0,
        "1d" => 24.0,
        other => {
            return Err(CoreError::InvalidArgument(format!(
                "interval 需为 1m/5m/15m/1h/4h/1d, 收到 '{other}'"
            )))
        }
    };
    let limit = ((days as f64) * 24.0 / hours_per_bar) as u32;
    let is_futures = config.market == "futures";
    let klines = if is_futures {
        let fapi = ricow_binance::FuturesDataClient::new()?;
        // MMR: 按 symbol 内置首档表 (exchangeInfo 公共值不可靠, specs/backtest.md §十一 T7)
        config
            .params
            .insert("mmr_pct".into(), ConfigValue::Float(ricow_binance::tier1_mmr_pct(pair)));
        fapi.get_klines(pair, interval, limit).await?
    } else {
        crate::commands::bn_exchange()?.get_klines(pair, interval, limit).await?
    };
    if klines.is_empty() {
        return Err(CoreError::Exchange(format!(
            "{pair} 无 K 线数据 (检查交易对是否存在/拼写)"
        )));
    }

    // ⑤ 沙箱回测 (第二关) + preview (第三关的第一步)
    let db =
        Database::open(&default_db_path()).await.map_err(|e| CoreError::Exchange(e.to_string()))?;
    // 预览记录清理(019 T040 / FR-046): 迭代会不断新增预览行, 生成新预览前先清过期与终态行, 避免单调增长。
    if let Err(e) = db.prune_previews(chrono::Utc::now().timestamp()).await {
        eprintln!("提示: 预览记录清理失败(不影响本次生成): {e}");
    }
    let effective_mmr_pct = config.get_f64("mmr_pct").unwrap_or(1.0);
    let initial_cash = config.get_f64("cash").unwrap_or(100_000.0);
    let (report, preview_id) = Engine::new().backtest_and_preview(&db, config, &klines).await?;

    let report_text = format_backtest_report(
        &report,
        &format!(
            "沙箱回测: {name} {pair} ({days} 天, {interval} K 线, {})",
            if is_futures {
                format!(
                    "合约 USDT-M · {:.0}x · MMR {:.2}%",
                    report.leverage.unwrap_or(1.0),
                    effective_mmr_pct
                )
            } else {
                "现货".to_string()
            }
        ),
        Decimal::from_f64_retain(initial_cash).unwrap_or(Decimal::ZERO),
        is_futures,
    );
    Ok(PreviewOutcome { report_text, preview_id })
}

pub async fn run(args: CreateArgs) -> CoreResult<()> {
    // ⓪ 策略名规范 (019 D18 / FR-043): 字符集/长度 + 与既有策略名**不得互为前缀**
    // (前缀互为前缀 → is_owned 互相命中 → 停机清理会撤掉对方挂单)。失败即退出, 不读码不拉 K 线。
    ricow_strategy::validate_strategy_name(&args.name).map_err(CoreError::InvalidArgument)?;
    let existing = crate::commands::deployed_strategy_names();
    if let Some(conflict) = ricow_strategy::prefix_conflict(&args.name, &existing) {
        return Err(CoreError::InvalidArgument(format!(
            "策略名 '{}' 与既有策略 '{}' 互为前缀: 两者的订单归属前缀会互相命中, 停机清理可能撤掉对方的挂单。\n\
             请换一个不与既有名字互为前缀的名字(例如加后缀: {}-2, 或改名: {})",
            args.name,
            conflict,
            args.name,
            ricow_strategy::suggest_strategy_name(&format!("{}-x", args.name))
        )));
    }

    // ① 读码: 文件 / stdin
    let raw = match args.script.as_deref() {
        Some("-") | None => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| CoreError::Parse(format!("读 stdin 失败: {e}")))?;
            s
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| CoreError::InvalidArgument(format!("读脚本 {path} 失败: {e}")))?,
    };
    if raw.trim().is_empty() {
        return Err(CoreError::InvalidArgument(
            "提交的 Lua 代码为空 (用 --script <file> 或管道送入, 如 `cat x.lua | ricow create ...`)".into(),
        ));
    }

    // ② 参数解析 (CLI 形态: key=value)
    let mut params: Vec<(String, ConfigValue)> = Vec::new();
    for p in &args.params {
        let (k, v) = parse_param(p)
            .ok_or_else(|| CoreError::InvalidArgument(format!("--param 需为 key=value, 收到 '{p}'")))?;
        params.push((k, v));
    }

    // ③④⑤ 与 AI 工具共用的创建内核
    let days = args.days.unwrap_or(90);
    let interval = args.interval.clone().unwrap_or_else(|| "1h".to_string());
    let out = create_preview(&args.name, &raw, &args.pair, &args.market, params, days, &interval).await?;

    print!("{}", out.report_text);
    println!();
    println!("编译门禁 ✓  沙箱回测 ✓  —— **尚未部署**(未写入任何策略文件)");
    println!("preview_id: {}  (15 分钟内有效, 一次性 token)", out.preview_id);
    println!("下一步:");
    println!("  1) 人工确认上述报告与策略内容是否可用: ricow approve {}", out.preview_id);
    println!("  2) 携一次性 token 落盘: ricow deploy {} --token <token>", out.preview_id);
    Ok(())
}
