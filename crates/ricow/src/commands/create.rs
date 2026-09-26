//! `ricow create` — AI/用户提交 Lua 策略 (002): 编译门禁 → 真实 K 线沙箱回测 → 两步确认 preview。
//!
//! 三步链路 (写操作**不得一步落盘**):
//! 1. `ricow create --name <n> --pair <p> [--script <file|->]` —— 本命令: 只产出 preview, 不写任何策略文件
//! 2. `ricow approve <preview_id>` —— 用户批准, 得一次性 token (15 分钟有效)
//! 3. `ricow deploy <preview_id> --token <t>` —— 落盘实例 `strategies/<name>.toml` + 脚本 `strategies/{market}/<name>.lua` (031)
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

/// 策略名可用性校验 (**纯函数**, 供单测): 字符集/长度 + 与既有策略名**不得互为前缀** (019 D18 / FR-043)。
///
/// `existing` 由调用方提供(生产用 [`crate::commands::deployed_strategy_names`] → 可注入测试数据)。
/// 互为前缀 → `ownership_prefix` 互相命中 → 停机撤单兜底会撤掉对方挂单, 故必须在 create 阶段拒绝。
pub(crate) fn validate_new_name(name: &str, existing: &[String]) -> CoreResult<()> {
    ricow_strategy::validate_strategy_name(name).map_err(CoreError::InvalidArgument)?;
    if let Some(conflict) = ricow_strategy::prefix_conflict(name, existing) {
        return Err(CoreError::InvalidArgument(format!(
            "策略名 '{name}' 与既有策略 '{conflict}' 互为前缀: 两者的订单归属前缀会互相命中, 停机清理可能撤掉对方的挂单。\n\
             请换一个与既有名字**不互为前缀**的名字 —— 新名不得以 '{conflict}' 开头, 也不能让 '{conflict}' 以新名开头\n\
             (例如既有 'eth-grid' 时, 'eth-grid-300' 与 'eth' 都不行, 可改用 'grid-eth-300')。"
        )));
    }
    Ok(())
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
    // 策略名规范 (创建期第一道防线): 字符集/长度 + 与既有策略**不得互为前缀** (019 D18 / FR-043)。
    // 校验必须落在本内核里 —— CLI `ricow create` 与 AI 工具 `preview_strategy` 共用它:
    // 此前互前缀只查 CLI 侧, 对话内可生成 'eth-grid' 与 'eth-grid-300' 并互相误撤挂单。
    // 顺序上先于拉 K 线, 失败即零副作用; 部署期 execute_strategy 另用同一校验器复核。
    validate_new_name(name, &crate::commands::deployed_strategy_names())?;

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
        return Err(CoreError::Exchange(format!("{pair} 无 K 线数据 (检查交易对是否存在/拼写)")));
    }

    // ⑤ 沙箱回测 (第二关) + preview (第三关的第一步)
    let db =
        Database::open(&default_db_path()).await.map_err(|e| CoreError::Exchange(e.to_string()))?;
    // 预览记录清理(019 T040 / FR-046): 迭代会不断新增预览行, 生成新预览前先清过期与终态行, 避免单调增长。
    if let Err(e) = db.prune_previews(chrono::Utc::now().timestamp()).await {
        eprintln!("提示: 预览记录清理失败(不影响本次生成): {e}");
    }
    let effective_mmr_pct = config.get_f64("mmr_pct").unwrap_or(1.0);
    // 回测本金口径 (FR-016 同口径): 唯一权威 = `BacktestParams::resolve` 三层合并
    // (内置默认 100_000 < 策略 TOML `[backtest].initial_cash` < 显式覆盖)。
    // 显式 `cash`(`--param cash=` / AI `params.cash`) 与 CLI `ricow backtest --cash` 同名同义,
    // 作为**最上层覆盖**参与同一次 resolve —— 于是报告表头与实喂回测的本金同源,
    // 不再一处 `config.get_f64("cash")` 一处硬编码常量(cash≠100000 时表头与本金各说一套)。
    let cash_override =
        ricow_strategy::BacktestToml { initial_cash: config.get_f64("cash"), ..Default::default() };
    let initial_cash = Decimal::from_f64_retain(
        ricow_strategy::BacktestParams::resolve(&config, &cash_override).initial_cash,
    )
    .ok_or_else(|| CoreError::InvalidArgument("回测本金 initial_cash 非法".into()))?;
    let (report, preview_id) =
        Engine::new().backtest_and_preview(&db, config, initial_cash, &klines).await?;

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
        initial_cash,
        is_futures,
        &[],
    );
    Ok(PreviewOutcome { report_text, preview_id })
}

pub async fn run(args: CreateArgs) -> CoreResult<()> {
    // ⓪ 策略名规范 (019 D18 / FR-043): 与 `create_preview` **共用同一校验**(字符集/长度 + 与既有名
    // 不互为前缀)。在此先拦一道, 好处是失败即退出 —— 不读 stdin/不读文件、不拉 K 线、零副作用。
    validate_new_name(&args.name, &crate::commands::deployed_strategy_names())?;

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
        let (k, v) = parse_param(p).ok_or_else(|| {
            CoreError::InvalidArgument(format!("--param 需为 key=value, 收到 '{p}'"))
        })?;
        params.push((k, v));
    }

    // ③④⑤ 与 AI 工具共用的创建内核
    let days = args.days.unwrap_or(90);
    let interval = args.interval.clone().unwrap_or_else(|| "1h".to_string());
    let out =
        create_preview(&args.name, &raw, &args.pair, &args.market, params, days, &interval).await?;

    print!("{}", out.report_text);
    println!();
    println!("编译门禁 ✓  沙箱回测 ✓  —— **尚未部署**(未写入任何策略文件)");
    // 分钟数从引擎常量派生, 不手抄 15 —— 常量改了这个提示不会跟着过期。
    println!(
        "preview_id: {}  ({} 分钟内有效, 一次性 token)",
        out.preview_id,
        ricow_engine::PREVIEW_TTL_SECS / 60
    );
    println!("下一步:");
    println!("  1) 人工确认上述报告与策略内容是否可用: ricow approve {}", out.preview_id);
    println!("  2) 携一次性 token 落盘: ricow deploy {} --token <token>", out.preview_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FR-043 回归: 创建内核 (CLI 与 AI 工具共用) 必须拒**互前缀**名 ——
    /// 否则对话内可生成 'eth-grid' 与 'eth-grid-300', 停机撤单兜底按前缀判归属会误撤对方挂单。
    #[test]
    fn test_validate_new_name_rejects_prefix_conflict() {
        let existing = vec!["eth-grid".to_string()];

        // 既有名是候选名的前缀 (eth-grid vs eth-grid-300): 任一方向都必须拒。
        let e = validate_new_name("eth-grid-300", &existing).unwrap_err().to_string();
        assert!(e.contains("互为前缀"), "{e}");
        // 候选名是既有名的前缀 (eth vs eth-grid)。
        assert!(validate_new_name("eth", &existing).is_err(), "候选名是既有名前缀须拒");
        // 同名 (部署会覆盖同名策略) 亦属冲突。
        assert!(validate_new_name("eth-grid", &existing).is_err(), "同名须拒");
        // 不互为前缀 → 放行 (换前缀即可规避)。
        assert!(validate_new_name("grid-eth-300", &existing).is_ok());
        // 无既有策略时任何合法名都放行。
        assert!(validate_new_name("eth-grid", &[]).is_ok());
    }

    /// 字符集/长度规范沿用既有校验器 (互前缀是追加条件, 不是替代)。
    #[test]
    fn test_validate_new_name_keeps_charset_and_len_rules() {
        assert!(validate_new_name("网格A", &[]).is_err(), "中文须拒");
        assert!(validate_new_name("", &[]).is_err(), "空名须拒");
        assert!(validate_new_name(&"a".repeat(25), &[]).is_err(), "超 24 字符须拒");
        assert!(validate_new_name(&"a".repeat(24), &[]).is_ok());
    }
}
