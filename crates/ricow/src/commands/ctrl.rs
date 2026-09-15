//! `ricow start` / `stop` / `restart` — 策略进程控制 (008: 经 daemon 控制通道)。
//!
//! 与前台调试入口 `ricow run` 的分工:
//! - `start/stop/restart`: 由 daemon 托管的唯一受管路径 (状态可查、日志落文件、停机走清理)
//! - `run`: 前台调试, 进程内自带停机监听 (stdin `stop` / 管道 EOF / Ctrl-C), 不被 daemon 管理

use clap::Args;
use ricow_core::{CoreError, CoreResult};

use crate::supervisor::client::Client;
use crate::supervisor::proto::{Request, StopReport};

#[derive(Args)]
pub struct StartArgs {
    /// 首次实盘使用需读风险披露后确认一次 (018)
    #[arg(long = "accept-risk")]
    pub accept_risk: bool,
    /// 策略名 (对应 strategies/<name>.toml)
    pub name: String,
    /// 显式要求实盘: 须同时有 TOML `live_enabled = true` (双条件缺一即按 Dry Run 启动)
    #[arg(long)]
    pub live: bool,
    /// 用币安测试网(demo)跑: 真实调用测试网下单接口, 不涉真实资金, 不适用实盘三判据
    #[arg(long)]
    pub demo: bool,
}

#[derive(Args)]
pub struct StopArgs {
    /// 策略名 (对应 strategies/<name>.toml)
    pub name: String,
    /// 停机时平掉策略持仓 (仅实盘实例有意义; Dry Run 无交易所持仓)
    #[arg(long)]
    pub close_all: bool,
}

#[derive(Args)]
pub struct RestartArgs {
    /// 策略名 (对应 strategies/<name>.toml)
    pub name: String,
}

/// 启动策略 (daemon 后台托管; 已在运行则拒绝, 不重复拉起)。
///
/// 实盘门禁与前台 `run` 同一口径 (双条件): 只有 `--live` **且** TOML 声明 `live_enabled=true`
/// 才写 live 台账并透传 `--live`; 缺一按 Dry Run 启动并如实说明。
pub async fn start(args: StartArgs) -> CoreResult<()> {
    // Dry Run 时长门禁前置检查 (002 FR-007): 在**发起启动请求前**同步拒绝, 避免子进程起来再死掉
    // (子进程侧 `run` 亦有同一门禁 —— 双保险, 与 011 的双条件门禁同思路)。
    if args.live && !args.demo {
        // 首次使用风险确认 (018): 在发起启动请求前同步落地确认记录(子进程据此放行)
        match ricow_engine::risk_gate(crate::commands::risk_acked(), args.accept_risk) {
            ricow_engine::RiskGate::Refuse { message } => {
                return Err(CoreError::InvalidArgument(message))
            }
            ricow_engine::RiskGate::JustAcked => {
                let p = crate::commands::write_risk_ack()?;
                println!("已记录风险确认: {} (后续实盘不再要求)", p.display());
            }
            ricow_engine::RiskGate::Proceed => {}
        }
        if let Some(config) = crate::commands::read_strategy_config(&args.name) {
            if config.live_enabled {
                let min_hours = config
                    .get_f64("min_dry_run_hours")
                    .unwrap_or(ricow_engine::DEFAULT_MIN_DRY_RUN_HOURS);
                ricow_engine::dry_run_gate(
                    config.dry_run_started_at.as_deref(),
                    chrono::Utc::now(),
                    min_hours,
                )
                .map_err(CoreError::InvalidArgument)?;
            }
        }
    }
    // 实盘二次分离 (019 D4 / T029-T030): 018 首次披露之外, **每次启动**都要逐字确认;
    // 确认发生在**交互终端(本进程)**, 不进入 daemon 协议的子进程 stdin(那里只收 stop)。
    if args.live {
        crate::commands::require_explicit_phrase(
            &format!(
                "即将启动 **实盘**(真实资金): 策略 {}\n  三判据(018 风险确认 → Dry Run 时长门禁 → 时钟预检)在子进程中原样生效; 启动后按真实资金下单",
                args.name
            ),
            &format!("确认实盘 {}", args.name),
        )?;
    }

    let (pid, mode) = start_daemon(&args.name, args.live, args.demo, args.live).await?;
    println!(
        "已启动策略 {} (pid={pid}, 模式: {}, 日志: logs/{}.log)",
        args.name,
        match mode.as_str() {
            "live" => "实盘",
            "demo" => "测试网模拟盘(demo)",
            _ => "Dry Run",
        },
        args.name
    );
    if args.live && mode != "live" {
        println!(
            "注意: 命令行要求实盘, 但配置未声明实盘 (TOML live_enabled=false) —— 已按 Dry Run 启动"
        );
    }
    if mode == "live" {
        println!("实盘运行中 (真实资金): 停机用 ricow stop {} [--close-all]", args.name);
    } else if mode == "demo" {
        println!("测试网模拟盘运行中 (无真实资金): 停机用 ricow stop {} [--close-all]", args.name);
    } else {
        println!("查看状态: ricow status {}", args.name);
    }
    Ok(())
}

/// 停止策略: daemon 下发停机指令并等待清理; 超时如实报告 (不静默强杀)。
pub async fn stop(args: StopArgs) -> CoreResult<()> {
    print!("{}", stop_daemon(&args.name, args.close_all).await?);
    Ok(())
}

/// 启动内核(**不打印**): 走既有 daemon 控制通道, 返回 `(pid, mode)`。
///
/// CLI `ricow start` 与 AI 的 L1 工具共用(019 T031); 门禁与打印留在各自调用方。
pub(crate) async fn start_daemon(
    name: &str,
    live: bool,
    demo: bool,
    confirmed: bool,
) -> CoreResult<(u64, String)> {
    let root = crate::commands::project_root();
    let client = Client::connect(&root).await?;
    let data =
        client.call_ok(Request::Start { name: name.to_string(), live, demo, confirmed }).await?;
    let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or_default();
    let mode = data.get("mode").and_then(|v| v.as_str()).unwrap_or("dry_run").to_string();
    Ok((pid, mode))
}

/// 停机内核(**不打印**): 返回面向用户的说明文本(CLI 打印 / AI 工具返回同一份, FR-016 同口径)。
pub(crate) async fn stop_daemon(name: &str, close_all: bool) -> CoreResult<String> {
    use std::fmt::Write as _;
    let root = crate::commands::project_root();
    let client = Client::connect(&root).await?;
    let data =
        client.call_ok(Request::Stop { name: name.to_string(), close_all }).await?;
    let report: StopReport = serde_json::from_value(data)
        .map_err(|e| CoreError::Parse(format!("停机结果解析失败: {e}")))?;
    let mut out = String::new();
    if report.exited {
        match report.exit_code {
            Some(0) => {
                let _ = writeln!(out, "策略 {} 已停止 (exit=0, 用时 {}ms)", report.name, report.waited_ms);
            }
            Some(code) => {
                let _ = writeln!(
                    out,
                    "策略 {} 已停止, 但退出码 {code} (非正常退出; 详见 logs/{}.log)",
                    report.name, report.name
                );
            }
            None => {
                let _ = writeln!(out, "策略 {} 已停止 (用时 {}ms)", report.name, report.waited_ms);
            }
        }
    } else {
        let _ = writeln!(out, "策略 {} 未观测到退出 (用时 {}ms)", report.name, report.waited_ms);
    }
    if let Some(note) = report.note {
        let _ = writeln!(out, "{note}");
    }
    // 清理结果由策略进程如实写入日志; 这里不臆测结果
    Ok(out)
}

/// 重启 = 停止 (等清理完成) + 启动。
pub async fn restart(args: RestartArgs) -> CoreResult<()> {
    stop(StopArgs { name: args.name.clone(), close_all: false }).await?;
    start(StartArgs { name: args.name, live: false, demo: false, accept_risk: false }).await
}
