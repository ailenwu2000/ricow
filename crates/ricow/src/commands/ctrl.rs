//! `ricow start` / `stop` / `restart` — 策略进程控制 (008: 经 daemon 控制通道)。
//!
//! 与前台调试入口 `ricow run` 的分工:
//! - `start/stop/restart`: 由 daemon 托管的唯一受管路径 (状态可查、日志落文件、停机走清理)
//! - `run`: 前台调试, 进程内自带停机监听 (stdin `stop` / 管道 EOF / Ctrl-C), 不被 daemon 管理
//!
//! 023 D14: **终端渠道一行不改** —— 实盘启动仍要求逐字短语(`确认实盘 <名>`)。
//! 对话渠道用当前语言的口语确认词, 见 [`crate::ai::confirm`]; 两条渠道各自独立, 不互相放宽。

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
    // 实盘二次分离 (019 D4 / T029-T030): 018 首次披露之外, **每次启动**都要逐字确认;
    // 确认发生在**交互终端(本进程)**, 不进入 daemon 协议的子进程 stdin(那里只收 stop)。
    //
    // 顺序要求: 短语确认必须**先于**三判据预检 —— 018 预检带 `--accept-risk` 时会落盘
    // `risk_ack.json`, 若先预检再确认, 用户中途放弃(或非交互终端被拒)也会留下确认记录,
    // 等于替用户做了他从未做过的确认。与前台 `run`(先确认后跑门禁)保持同一顺序。
    if args.live {
        crate::commands::require_explicit_phrase(
            &format!(
                "即将启动 **实盘**(真实资金): 策略 {}\n  三判据(018 风险确认 → Dry Run 时长门禁 → 时钟预检)在子进程中原样生效; 启动后按真实资金下单",
                args.name
            ),
            &format!("确认实盘 {}", args.name),
        )?;
    }
    // 实盘三判据前置检查 (002 FR-007 / 018 / FR-008): 在**发起启动请求前**同步拒绝,
    // 避免子进程起来再死掉 (子进程侧 `run` 亦有同一份门禁 —— 双保险, 与 011 的双条件门禁同思路)。
    if args.live && !args.demo {
        if let Some(notice) =
            live_preflight(&crate::commands::project_root(), &args.name, args.accept_risk).await?
        {
            println!("{notice}");
        }
    }

    let (pid, mode) =
        start_daemon(&crate::commands::project_root(), &args.name, args.live, args.demo, args.live)
            .await?;
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
    print!("{}", stop_daemon(&crate::commands::project_root(), &args.name, args.close_all).await?);
    Ok(())
}

// ———————— 实盘三判据共享内核 (019 R4) ————————
//
// 018 风险确认 / 002 Dry Run 时长门禁 / FR-008 时钟预检 原本在三条实盘路径各写一遍
// (`ricow start --live` / 前台 `ricow run --live` / 对话内 AI 预检与执行)。三份平行实现
// 一旦判据顺序、取参默认值或降级行为漂移, 就会出现"某条路径更松"的静默漏洞。
// 这里收敛成三个小内核, 全部实盘路径只准从这里取判据。

/// 018 首次实盘风险确认 (共享内核): 已确认 → 放行; 带 `--accept-risk` → 落记录并放行; 否则拒绝。
///
/// 对话内路径只能传 `accept_risk = false`(确认必须在交互终端逐字完成, 见 `ai::confirm`),
/// 因此本内核在对话侧等价于"是否已确认过"。
///
/// 返回刚记录确认时的提示文本(供调用方按自己的输出通道回显)。
pub(crate) fn risk_gate_shared(accept_risk: bool) -> CoreResult<Option<String>> {
    match ricow_engine::risk_gate(crate::commands::risk_acked(), accept_risk) {
        ricow_engine::RiskGate::Refuse { message } => Err(CoreError::InvalidArgument(message)),
        ricow_engine::RiskGate::JustAcked => {
            let p = crate::commands::write_risk_ack()?;
            Ok(Some(format!("已记录风险确认: {} (后续实盘不再要求)", p.display())))
        }
        ricow_engine::RiskGate::Proceed => Ok(None),
    }
}

/// 002 Dry Run 时长门禁 (共享内核): 生效门限 = TOML `min_dry_run_hours`, 未配置回落
/// [`ricow_engine::DEFAULT_MIN_DRY_RUN_HOURS`]。
///
/// 返回**实际生效门限**(小时, 供确认块/日志如实回显); 未通过时返回拒绝说明 ——
/// **拒绝而非降级**: 用户已显式要求实盘, 静默降级更危险。
pub(crate) fn dry_run_gate_shared(config: &ricow_strategy::StrategyConfig) -> Result<f64, String> {
    let min_hours =
        config.get_f64("min_dry_run_hours").unwrap_or(ricow_engine::DEFAULT_MIN_DRY_RUN_HOURS);
    ricow_engine::dry_run_gate(
        config.dry_run_started_at.as_deref(),
        chrono::Utc::now(),
        min_hours,
    )?;
    Ok(min_hours)
}

/// FR-008 时钟预检 (共享内核, 实盘判据里**唯一联网项**): 按**本市场**取数
/// (现货/合约服务器时间不同步), 不通过即拒绝。
///
/// 返回本机相对交易所服务器的偏差(ms), 供调用方如实回显 (不通过时偏差在拒绝说明里)。
pub(crate) async fn clock_gate_shared(
    market: &str,
    mode: crate::commands::Mode,
) -> CoreResult<i64> {
    let skew = crate::commands::fetch_clock_skew(market, mode).await?;
    match ricow_engine::check_clock_skew(skew) {
        ricow_engine::ClockVerdict::Reject { message, .. } => {
            Err(CoreError::InvalidArgument(message))
        }
        ricow_engine::ClockVerdict::Ok { skew_ms } => Ok(skew_ms),
    }
}

/// 实盘共享预检 (019 R4 / T031): 018 首次风险确认 → 002 Dry Run 时长门禁 → FR-008 时钟预检。
///
/// **同一份实现**服务命令行 `ricow start <名> --live` 与对话内 `确认实盘 <名>` ——
/// 两条路径的门禁顺序、拒绝话术与降级行为不可能走偏。子进程 `run` 的实盘分支仍原样再跑一遍
/// (双保险: 父进程是快速失败, 子进程是最后一道), 且跑的是同一批内核函数。
///
/// - `root`: 数据目录 (读 `strategies/<名>.toml` 取市场与时长门禁参数);
/// - 018 的确认记录**固定写在 `project_root()`** —— 子进程 `run` 读的也是那一份, 必须同一个位置。
///
/// 返回需要展示给用户的提示(如"已记录风险确认"), 由调用方决定输出通道 (CLI 直接打印 / 会话走 sink)。
pub(crate) async fn live_preflight(
    root: &std::path::Path,
    name: &str,
    accept_risk: bool,
) -> CoreResult<Option<String>> {
    // 018 首次使用风险确认: 确认一次即长期有效 (子进程据此放行)
    let notice = risk_gate_shared(accept_risk)?;
    // TOML 不存在时无从得知市场与时长门禁参数: 只跑 018, 让子进程给出更明确的报错
    let Some(config) = crate::commands::read_strategy_config_in(root, name) else {
        return Ok(notice);
    };
    // TOML live_enabled=false 时 daemon 会按 Dry Run 启动(双条件缺一), 002/FR-008 不适用 ——
    // 与子进程 `run` 的 LiveGate 口径一致: 只有真会进实盘才跑这两项, 不在降级路径上徒增网络依赖。
    if !config.live_enabled {
        return Ok(notice);
    }
    dry_run_gate_shared(&config).map_err(CoreError::InvalidArgument)?;
    clock_gate_shared(&config.market, crate::commands::Mode::Live).await?;
    Ok(notice)
}

/// 启动内核(**不打印**): 走既有 daemon 控制通道, 返回 `(pid, mode)`。
///
/// CLI `ricow start` 与 AI 的 L1 工具共用(019 T031); 实盘三判据见 [`live_preflight`],
/// 打印留在各自调用方 (CLI stdout / 会话 sink)。
pub(crate) async fn start_daemon(
    root: &std::path::Path,
    name: &str,
    live: bool,
    demo: bool,
    confirmed: bool,
) -> CoreResult<(u64, String)> {
    let client = Client::connect(root).await?;
    let data =
        client.call_ok(Request::Start { name: name.to_string(), live, demo, confirmed }).await?;
    let pid = data.get("pid").and_then(|v| v.as_u64()).unwrap_or_default();
    let mode = data.get("mode").and_then(|v| v.as_str()).unwrap_or("dry_run").to_string();
    Ok((pid, mode))
}

/// 停机内核(**不打印**): 返回面向用户的说明文本(CLI 打印 / AI 工具返回同一份, FR-016 同口径)。
pub(crate) async fn stop_daemon(
    root: &std::path::Path,
    name: &str,
    close_all: bool,
) -> CoreResult<String> {
    let client = Client::connect(root).await?;
    let data = client.call_ok(Request::Stop { name: name.to_string(), close_all }).await?;
    let report: StopReport = serde_json::from_value(data)
        .map_err(|e| CoreError::Parse(format!("停机结果解析失败: {e}")))?;
    Ok(format_stop_report(&report))
}

/// 停机回执 → 面向用户的说明文本 (纯函数, 便于单测; 027 T012)。
///
/// 分派规则见 [contracts/cli-stop.md](../../../specs/changes/027-stop-unknown-instance/contracts/cli-stop.md) §二。
fn format_stop_report(report: &StopReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    // 名字存在但本来就没在跑 (027): 只陈述"未在运行 (无需停止)"这一个事实, 不套"已停止"头衔 ——
    // 同一份回执里"已停止"与"未在运行"互相打架会摧毁用户对停机通道的信任。
    if !report.already_stopped {
        if report.exited {
            match report.exit_code {
                Some(0) => {
                    let _ = writeln!(
                        out,
                        "策略 {} 已停止 (exit=0, 用时 {}ms)",
                        report.name, report.waited_ms
                    );
                }
                Some(code) => {
                    let _ = writeln!(
                        out,
                        "策略 {} 已停止, 但退出码 {code} (非正常退出; 详见 logs/{}.log)",
                        report.name, report.name
                    );
                }
                None => {
                    let _ =
                        writeln!(out, "策略 {} 已停止 (用时 {}ms)", report.name, report.waited_ms);
                }
            }
        } else {
            let _ =
                writeln!(out, "策略 {} 未观测到退出 (用时 {}ms)", report.name, report.waited_ms);
        }
    }
    if let Some(note) = &report.note {
        let _ = writeln!(out, "{note}");
    }
    // 清理结果由策略进程如实写入日志; 这里不臆测结果
    out
}

/// 重启 = 停止 (等清理完成) + 启动。
///
/// **按原实例模式重启, 不静默降级**: 此前无条件 `live:false, demo:false` —— 一个正在跑
/// 测试网 demo 或实盘的实例, 重启后会变成 Dry Run, 而用户以为它还在原位跑 (真实资金策略
/// 悄悄下线 / 测试网验证被换掉)。这里先读停机前的运行模式, 原样传回 `start`; 实盘仍会
/// 重过三判据并**再逐字确认一次** (`start` 内既有逻辑), 拒绝而非静默降级。
pub async fn restart(args: RestartArgs) -> CoreResult<()> {
    // 必须在 stop 之前读: 停机后台账/daemon 视图已被覆盖, 无从得知原本的模式。
    let prev_mode = crate::commands::instances::views(&crate::commands::project_root())
        .await
        .into_iter()
        .find(|v| v.name == args.name && v.running)
        .and_then(|v| v.mode);
    let (live, demo) = restart_flags(prev_mode.as_deref());
    if live {
        println!(
            "注意: {} 原本以**实盘**运行, 重启将再次执行实盘三判据并逐字确认 (不会静默降级为 Dry Run)",
            args.name
        );
    } else if demo {
        println!("注意: {} 原本以**测试网模拟盘(demo)**运行, 重启后仍是 demo", args.name);
    }
    stop(StopArgs { name: args.name.clone(), close_all: false }).await?;
    start(StartArgs { name: args.name, live, demo, accept_risk: false }).await
}

/// 重启模式判定 (**纯函数**, 便于单测): 按停机前的运行模式原样重启。
///
/// 未知模式 (`None` / 未运行 / 无法识别的字符串) 一律回落到 Dry Run —— 与 `start` 的
/// 双条件门禁一致 (说不清就按最保守的来)。
fn restart_flags(prev_mode: Option<&str>) -> (bool, bool) {
    match prev_mode {
        Some("live") => (true, false),
        Some("demo") => (false, true),
        _ => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_stop_report, restart_flags};
    use crate::supervisor::proto::StopReport;

    /// 027 T012: 停机回执文案分派 4 组合 —— 一条"本来就没在跑"不得套"已停止"头衔;
    /// 其余三条**逐字**等于改动前 (contracts/cli-stop.md §2.3 / §2.2)。
    #[test]
    fn stop_report_text_dispatch() {
        let base =
            |exited: bool, exit_code: Option<i32>, note: Option<&str>, already_stopped: bool| {
                StopReport {
                    name: "grid".into(),
                    exited,
                    graceful: true,
                    exit_code,
                    waited_ms: 12,
                    note: note.map(str::to_string),
                    already_stopped,
                }
            };

        // ① 名字存在但本来就没在跑: 只陈述一个事实, 不含"已停止"
        let already =
            format_stop_report(&base(true, None, Some("该策略未在运行 (无需停止)"), true));
        assert_eq!(already, "该策略未在运行 (无需停止)\n", "只输出 note 一行");
        assert!(!already.contains("已停止"), "{already}");
        assert!(!already.contains("未观测到退出"), "{already}");

        // ② 优雅退出 exit=0 (逐字, 含 note)
        let zero = format_stop_report(&base(true, Some(0), Some("测试网停机"), false));
        assert_eq!(zero, "策略 grid 已停止 (exit=0, 用时 12ms)\n测试网停机\n", "{zero}");

        // ③ 退出码非 0 (逐字)
        let bad = format_stop_report(&base(true, Some(7), None, false));
        assert_eq!(bad, "策略 grid 已停止, 但退出码 7 (非正常退出; 详见 logs/grid.log)\n", "{bad}");

        // ④ 退出码未知 / 超时未观测到退出 (逐字)
        let unknown = format_stop_report(&base(true, None, None, false));
        assert_eq!(unknown, "策略 grid 已停止 (用时 12ms)\n", "{unknown}");
        let timeout = format_stop_report(&base(
            false,
            None,
            Some("停机超时 (30s) 未观测到退出; 未强制终止, 请手工核对 (pid 123)"),
            false,
        ));
        assert_eq!(
            timeout,
            "策略 grid 未观测到退出 (用时 12ms)\n停机超时 (30s) 未观测到退出; 未强制终止, 请手工核对 (pid 123)\n",
            "{timeout}"
        );
    }

    #[test]
    fn test_restart_flags_keeps_previous_mode() {
        assert_eq!(restart_flags(Some("live")), (true, false), "原实盘 → 仍实盘(重过门禁)");
        assert_eq!(restart_flags(Some("demo")), (false, true), "原 demo → 仍 demo");
        assert_eq!(restart_flags(Some("dry_run")), (false, false), "原 Dry Run → 仍 Dry Run");
    }

    #[test]
    fn test_restart_flags_unknown_falls_back_to_dry_run() {
        assert_eq!(restart_flags(None), (false, false), "未运行/无记录 → Dry Run");
        assert_eq!(restart_flags(Some("weird")), (false, false), "无法识别 → Dry Run");
    }
}
