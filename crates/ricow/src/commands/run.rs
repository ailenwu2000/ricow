//! `ricow run` — 启动策略 (默认 Dry Run; 实盘需配置声明 + `--live` 双条件)。

use std::collections::HashMap;
use std::sync::Arc;

use clap::Args;
use ricow_core::{Balance, CoreError, CoreResult, Exchange};
use ricow_engine::{live_gate, Engine, LiveGate, RunMode, RunOutcome, StopReason, StopRequest};
use ricow_strategy::{ConfigValue, Database, StrategyConfig};
use rust_decimal::Decimal;

#[derive(Args)]
pub struct RunArgs {
    /// 首次实盘使用需读风险披露后确认一次 (018; product.md §十)
    #[arg(long = "accept-risk")]
    pub accept_risk: bool,
    /// 策略名 (已部署 TOML, 如 <项目根>/strategies/<name>.toml) 或策略类型直跑 (shannon_grid/dca/twap/vwap, 其余为执行模式示例)
    pub strategy: String,
    /// 交易对 (直跑模式必填; TOML 加载模式忽略)
    #[arg(long)]
    pub pair: Option<String>,
    /// 预设名 (当前未实现差异化, 使用默认参数)
    #[arg(long)]
    pub preset: Option<String>,
    /// lua 脚本路径 (strategy=lua 时必填)
    #[arg(long)]
    pub script: Option<String>,
    /// 显式要求实盘: 须同时有 TOML `live_enabled = true` (双条件缺一即按 Dry Run 运行)
    #[arg(long)]
    pub live: bool,
    /// 用币安测试网(demo)真实下单验证: 不涉真实资金, 故不适用实盘三判据(018 风险确认 / 002 时长门禁),
    /// 但仍需 demo 凭据与时钟预检。与 `--live` 互斥使用(同时给则报错)。
    #[arg(long)]
    pub demo: bool,
    /// 内部: 实盘确认已在交互终端完成(由 daemon 派生时注入); 单独传无效, 须同时有 daemon 注入的
    /// `RICOW_DAEMON_SPAWNED` 环境变量 —— 用户手工传这个标志仍会被要求逐字确认
    #[arg(long, hide = true)]
    pub live_confirmed: bool,
    /// 实盘停机时市价平掉策略持仓 (仅实盘生效; Dry Run 无效果)
    #[arg(long)]
    pub close_all: bool,
}

/// 内部通道判定 (纯函数, 单测锁定边界): `--live-confirmed` 标志与 daemon 注入的环境变量
/// **两者齐备**才认账; 缺一即回到"逐字确认"这条正常路径。
fn daemon_confirmation_accepted(flag: bool, daemon_marked: bool) -> bool {
    flag && daemon_marked
}

pub async fn run(args: RunArgs) -> CoreResult<()> {
    // daemon 派生链的内部通道: 标志 + daemon 注入的环境变量**两者齐备**才认账。
    // 只认标志的话, 任何人在 shell 里敲 `ricow run <名> --live --live-confirmed` 就能跳过逐字确认,
    // 等于把"每次实盘启动都必须用户逐字确认"这条规则变成一个可绕过的开关。
    let confirmed_by_daemon = daemon_confirmation_accepted(
        args.live_confirmed,
        crate::supervisor::procs::spawned_by_daemon(),
    );
    // 实盘二次分离 (019 D4 / T029-T030): 未走内部通道时, 前台也要求逐字确认(零副作用)。
    if args.live && !confirmed_by_daemon {
        crate::commands::require_explicit_phrase(
            &format!("即将启动 **实盘**(真实资金): 策略 {}", args.strategy),
            &format!("确认实盘 {}", args.strategy),
        )?;
    }

    let exchange = crate::commands::bn_exchange()?;

    // 判定: name 命中已部署 TOML → TOML 加载; 未命中 → 策略类型直跑 (向后兼容)。
    let strategies_dir = crate::commands::ensure_strategies_dir()?;
    let config = if strategies_dir.join(format!("{}.toml", args.strategy)).exists() {
        crate::commands::load_strategy_toml(&strategies_dir, &args.strategy)?
    } else {
        inline_config(&args, &exchange).await?
    };

    let pair = config.get_str("pair").unwrap_or("ETHUSDT").to_string();

    let db = Database::open(&crate::commands::default_db_path())
        .await
        .map_err(|e| CoreError::InvalidArgument(format!("打开数据库失败: {e}")))?;

    // 停机信号: stdin 指令 (含 daemon 下发的 stop 与管道 EOF) + Ctrl-C
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(None);
    spawn_stdin_stop_watcher(stop_tx.clone());
    spawn_ctrl_c_stop_watcher(stop_tx);

    // demo(测试网)模式: 真实调用币安测试网下单接口, **不涉真实资金**。
    // 因此实盘三判据里的 018 风险确认与 002 Dry Run 时长门禁**不适用**(它们保护的是真实资金);
    // demo 凭据与时钟预检仍是硬要求, 且打印时绝不把它说成实盘。
    if args.demo {
        if args.live {
            return Err(CoreError::InvalidArgument(
                "--demo 与 --live 不能同时使用: demo 是测试网模拟盘, --live 是主网真实资金".into(),
            ));
        }
        // 凭据先校验(快速失败): 缺 demo key 时不应先打印"启动策略"
        crate::commands::load_credentials(crate::commands::Mode::Demo)?;
        // FR-008 时钟预检走共享内核 (与实盘同一份判定, 只换服务器: demo 端点)
        let skew_ms =
            crate::commands::ctrl::clock_gate_shared(&config.market, crate::commands::Mode::Demo)
                .await?;
        println!("时钟预检通过(按 demo 服务器): 本机比服务器 {skew_ms:+} ms");
        println!(
            "启动策略 {} ({}, {} · 真实调用测试网下单接口, 无真实资金)。",
            config.name,
            crate::commands::Mode::Demo.label(),
            if config.market.eq_ignore_ascii_case("futures") { "合约 USDT-M" } else { "现货" }
        );
        println!(
            "说明: 测试网模拟盘不适用实盘三判据(风险确认 / Dry Run 时长门禁); 仍需 demo 凭据与时钟预检。"
        );
        println!(
            "端点: {}",
            if config.market.eq_ignore_ascii_case("futures") {
                crate::commands::DEMO_FAPI_URL
            } else {
                crate::commands::DEMO_SPOT_URL
            }
        );
        let demo_exchange = build_exchange(&config, crate::commands::Mode::Demo).await?;
        if !args.close_all {
            println!("提示: 未带 --close-all, 停机时只做撤单兜底, 持仓保留");
        }
        let outcome = Engine::new()
            .run_live(
                config,
                demo_exchange,
                Some(&db),
                Some(stop_rx),
                args.close_all,
                RunMode::Demo,
            )
            .await?;
        print_run_outcome(&outcome);
        if outcome.stop_reason == Some(StopReason::StreamEnded) {
            return Err(CoreError::Network("demo 数据流中断, 策略已停止".into()));
        }
        return Ok(());
    }

    // 实盘门禁 (FR-013 / D2): 配置声明 **且** 命令行 --live, 缺一即按 Dry Run 运行并说明原因
    match live_gate(config.live_enabled, args.live) {
        LiveGate::Live => {
            // 实盘三判据走共享内核 (019 R4): 与 `ricow start --live`、对话内执行路径同一份实现,
            // 判据顺序/取参默认值/拒绝话术不可能各走各的。
            // 018 首次使用风险确认 (product.md §十): 真实资金前先过一次(确认一次即长期有效)
            if let Some(notice) = crate::commands::ctrl::risk_gate_shared(args.accept_risk)? {
                println!("{notice}");
            }
            // 002 Dry Run 时长门禁 (FR-007): 声明实盘前须先在 Dry Run 下观察足够久
            // (可 params 调低/设 0 关闭)。拒绝而非降级。
            crate::commands::ctrl::dry_run_gate_shared(&config)
                .map_err(CoreError::InvalidArgument)?;

            let is_futures = config.market.eq_ignore_ascii_case("futures");
            // FR-008 时钟预检: 按**本市场**取数 (现货/合约服务器时间不同步), 不通过即退出
            let skew_ms = crate::commands::ctrl::clock_gate_shared(
                &config.market,
                crate::commands::Mode::Live,
            )
            .await?;
            println!("时钟预检通过: 本机比交易所服务器 {skew_ms:+} ms");

            let live_exchange = build_exchange(&config, crate::commands::Mode::Live).await?;
            println!(
                "启动策略 {} (实盘{}, 真实资金)。停机方式: stdin 输入 stop / 管道关闭 / Ctrl-C",
                config.name,
                if is_futures { " · 合约 USDT-M" } else { " · 现货" }
            );
            if !args.close_all {
                println!("提示: 未带 --close-all, 停机时只做撤单兜底, 持仓保留");
            }

            let outcome = Engine::new()
                .run_live(
                    config,
                    live_exchange,
                    Some(&db),
                    Some(stop_rx),
                    args.close_all,
                    RunMode::Live,
                )
                .await?;
            print_run_outcome(&outcome);
            if outcome.stop_reason == Some(StopReason::StreamEnded) {
                return Err(CoreError::Network("实盘数据流中断, 策略已停止".into()));
            }
            Ok(())
        }
        LiveGate::DryRun { reason } => {
            if args.live {
                println!("{reason}");
            }
            // Dry Run 起点留痕 (002 FR-006): 只在首次记录, 供实盘时长门禁核验
            if config.dry_run_started_at.is_none() {
                if let Some(p) = crate::commands::record_dry_run_start(&config)? {
                    println!("已记录 Dry Run 起点到 {} (实盘时长门禁依据)", p.display());
                }
            }
            // Dry Run 初始虚拟资金按交易对的报价资产配平 (否则 USDT 对本金记在 USDC 上, 策略判定"无可用资金");
            // 金额可配 (016: params.initial_cash, 默认 100k) —— 让虚拟本金贴近真实可投入资金, 预演才有参考价值。
            let dry_run_cash = ricow_engine::dry_run_initial_cash(config.get_f64("initial_cash"))
                .map_err(CoreError::InvalidArgument)?;
            let initial_balance =
                Balance { asset: quote_asset_of(&pair), free: dry_run_cash, locked: Decimal::ZERO };
            println!("Dry Run 虚拟本金: {dry_run_cash} {}", quote_asset_of(&pair));
            println!(
                "启动策略 {} (Dry Run, 前台)。停机方式: stdin 输入 stop / 管道关闭 / Ctrl-C",
                config.name
            );
            let outcome = Engine::new()
                .run_dry_run(config, exchange, initial_balance, Some(&db), Some(stop_rx))
                .await?;
            print_run_outcome(&outcome);
            // 行情流中断属异常: 非零退出, 供管理器/用户识别 (不静默 Ok)
            if outcome.stop_reason == Some(StopReason::StreamEnded) {
                return Err(CoreError::Network("行情流中断 (WebSocket 断开), 策略已停止".into()));
            }
            Ok(())
        }
    }
}

/// 按模式构造带凭据的交易所 (现货/合约); 合约附带启动预配置 (杠杆 / 保证金模式 / 持仓方向)。
///
/// 同一函数服务实盘与 demo —— 两者的差别只有**域名与凭据来源**, 不复制业务逻辑。
async fn build_exchange(
    config: &StrategyConfig,
    mode: crate::commands::Mode,
) -> CoreResult<Arc<dyn Exchange>> {
    let pair = config.get_str("pair").unwrap_or("ETHUSDT").to_string();
    if config.market.eq_ignore_ascii_case("futures") {
        let ex = crate::commands::bn_futures_signed_mode(mode)?;
        let leverage = config.get_f64("leverage").unwrap_or(1.0).max(1.0) as u32;
        let isolated =
            config.get_str("margin_type").map(|s| !s.eq_ignore_ascii_case("cross")).unwrap_or(true);
        let hedge = config.position_mode.eq_ignore_ascii_case("hedge");
        ex.prepare(&pair, leverage, hedge, isolated).await?;
        println!(
            "合约预配置: 杠杆 {leverage}x / {} / {}",
            if isolated { "逐仓" } else { "全仓" },
            if hedge { "hedge 双向" } else { "one-way" }
        );
        Ok(Arc::new(ex))
    } else {
        crate::commands::bn_spot_signed_mode(mode)
    }
}

/// 解析停机指令行 (纯函数, 便于单测): `stop` / `stop --close-all` (大小写不敏感, 允许两端空白)。
///
/// `--close-all` = 停机清理时额外平掉策略持仓 (仅实盘生效)。
pub(crate) fn parse_stop_command(line: &str) -> Option<StopRequest> {
    match line.trim().to_ascii_lowercase().as_str() {
        "stop" => Some(StopRequest { reason: Some(StopReason::Requested), close_all: false }),
        "stop --close-all" => {
            Some(StopRequest { reason: Some(StopReason::Requested), close_all: true })
        }
        _ => None,
    }
}

/// 监听 stdin: `stop` 视为停机指令 (`stop --close-all` 附带平仓); 管道关闭 (EOF) 视为管理器已退出 → 自愈停机。
fn spawn_stdin_stop_watcher(tx: tokio::sync::watch::Sender<Option<StopRequest>>) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if let Some(req) = parse_stop_command(&l) {
                        let _ = tx.send(Some(req));
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        // 读到 EOF: 管理器进程已不在
        let _ =
            tx.send(Some(StopRequest { reason: Some(StopReason::ManagerGone), close_all: false }));
    });
}

/// 前台调试: Ctrl-C 也走同一条优雅停机路径。
fn spawn_ctrl_c_stop_watcher(tx: tokio::sync::watch::Sender<Option<StopRequest>>) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = tx.send(Some(StopRequest {
                reason: Some(StopReason::Interrupted),
                close_all: false,
            }));
        }
    });
}

/// 交易对报价资产 (Dry Run 初始虚拟资金用): ETHUSDT → USDT; 识别不到时回退 USDC。
fn quote_asset_of(pair: &str) -> String {
    let up = pair.to_uppercase();
    for q in ["USDT", "USDC", "FDUSD", "TUSD", "BUSD"] {
        if up.ends_with(q) {
            return q.to_string();
        }
    }
    "USDC".to_string()
}

/// 运行结果如实输出 (数字即实况, 不含推测)。
fn print_run_outcome(o: &RunOutcome) {
    let reason = o.stop_reason.map(|r| r.to_string()).unwrap_or_else(|| "未知".into());
    println!("停机原因: {reason}");
    println!(
        "统计: tick={} 提交订单={} 下单失败={} 拒单={} 成交={} 落库失败={}",
        o.ticks, o.orders_submitted, o.order_errors, o.rejections, o.fills, o.persist_errors
    );
    if let Some(e) = &o.last_error {
        println!("最近错误: {e}");
    }
    if let Some(c) = &o.cleanup {
        println!(
            "停机清理: 已撤挂单={} 撤单失败={} 平仓单={} 平仓失败={} 残留挂单={} 残留持仓={}",
            c.canceled.len(),
            c.cancel_failed.len(),
            if c.close_done.is_empty() { "未执行".to_string() } else { c.close_done.join(",") },
            c.close_error.len(),
            c.residual.len(),
            c.residual_position.map(|d| d.to_string()).unwrap_or_else(|| "无".into()),
        );
        for n in &c.close_notes {
            println!("平仓说明: {n}");
        }
        for (cid, why) in &c.cancel_failed {
            println!("撤单失败: {cid} — {why}");
        }
        for (cid, why) in &c.close_error {
            println!("平仓失败: {cid} — {why}");
        }
        for cid in &c.residual {
            println!("注意: 残留挂单未撤 {cid}");
        }
        if c.has_residual() {
            println!("注意: 存在残留, 请在交易所账户侧人工核对处理");
        }
    }
    if !o.on_stop_implemented {
        println!("提示: 该策略未实现清理 (on_stop); 如仍有挂单或持仓, 请手工处理");
    }
}

/// 旧逻辑: 按策略类型构造内联配置 (直跑调试通道)。
async fn inline_config(
    args: &RunArgs,
    exchange: &std::sync::Arc<dyn ricow_core::Exchange>,
) -> CoreResult<StrategyConfig> {
    let pair = args.pair.clone().ok_or_else(|| {
        CoreError::InvalidArgument("直跑模式需要 --pair <pair> (或使用已部署策略名)".into())
    })?;

    // 网格区间围绕当前价 ±10% (动态)
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

    // 各策略的默认参数 (示例, 可后续接 preset)
    match args.strategy.as_str() {
        "shannon_grid" => {
            params.entry("order_size".into()).or_insert(ConfigValue::Float(0.01));
            params.entry("rebalance_band".into()).or_insert(ConfigValue::Float(0.005));
            params.entry("target_ratio".into()).or_insert(ConfigValue::Float(0.5));
            params.entry("atr_period".into()).or_insert(ConfigValue::Integer(14));
            params.entry("atr_mult".into()).or_insert(ConfigValue::Float(1.0));
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
        other => return Err(CoreError::InvalidArgument(format!("unsupported strategy: {other}"))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 内部通道边界 (019 审核): 只有标志或只有 daemon 标记都不算确认 ——
    /// 防止日后有人把 `&&` 改成 `||`, 让手工传 `--live-confirmed` 变成静默绕过逐字确认。
    #[test]
    fn live_confirmation_requires_both_flag_and_daemon_marker() {
        assert!(daemon_confirmation_accepted(true, true), "daemon 派生且带标志: 认账");
        assert!(!daemon_confirmation_accepted(true, false), "仅手工传标志: 不认账, 仍要逐字确认");
        assert!(!daemon_confirmation_accepted(false, true), "仅继承标记: 不认账");
        assert!(!daemon_confirmation_accepted(false, false), "都没有: 不认账");
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        // 每测试独立子目录: 避免并行测试互相 remove/create 竞争。
        let d = std::env::temp_dir().join(format!("ricow-run-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("create tmp dir");
        d
    }

    fn valid_toml(enabled: bool) -> String {
        format!(
            r#"
[strategy]
name = "demo"
type = "shannon_grid"
enabled = {enabled}
exchange = "binance"

[strategy.params]
pair = "ETH"
order_size = 1.0
"#
        )
    }

    #[test]
    fn parse_stop_command_handles_close_all() {
        let plain = parse_stop_command("stop").expect("应识别 stop");
        assert_eq!(plain.reason, Some(StopReason::Requested));
        assert!(!plain.close_all, "普通停机不带平仓意图");

        let with_close =
            parse_stop_command("  STOP --Close-All  ").expect("应识别 stop --close-all");
        assert_eq!(with_close.reason, Some(StopReason::Requested));
        assert!(with_close.close_all, "指令文本带 --close-all 时须置平仓意图");

        assert!(parse_stop_command("hello").is_none(), "非停机指令应忽略");
        assert!(parse_stop_command("stop now").is_none(), "未知后缀不应误判");
    }

    #[test]
    fn loads_valid_toml() {
        let dir = tmp_dir("valid");
        fs::write(dir.join("demo.toml"), valid_toml(true)).unwrap();
        let cfg = crate::commands::load_strategy_toml(&dir, "demo").expect("should load");
        assert_eq!(cfg.name, "demo");
        assert_eq!(cfg.strategy_type, "lua", "内置名 TOML 应 Lua 化");
        assert!(cfg.enabled);
        assert_eq!(cfg.get_str("pair"), Some("ETH"));
        assert!(cfg.get_str("script").unwrap().contains("on_tick"), "应注入内置脚本");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_errors() {
        let dir = tmp_dir("missing");
        let err = crate::commands::load_strategy_toml(&dir, "nope").expect_err("should error");
        assert!(err.to_string().contains("读取策略 nope 失败"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_rejected() {
        let dir = tmp_dir("disabled");
        fs::write(dir.join("off.toml"), valid_toml(false)).unwrap();
        let err = crate::commands::load_strategy_toml(&dir, "off").expect_err("should reject");
        assert!(err.to_string().contains("未启用"));
        let _ = fs::remove_dir_all(&dir);
    }
}
