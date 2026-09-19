//! `ricow` CLI 入口 — clap 命令分发。

mod ai;
mod commands;
mod supervisor;

use clap::{Parser, Subcommand};

use commands::{
    agentkit, approve, backtest, create, ctrl, daemon, db, deploy, instances, logs, market, run,
};

#[derive(Parser)]
#[command(name = "ricow", version, about = "ricow 本地量化终端 (回测 / Dry Run / 实盘)")]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // CLI 子命令参数结构体天生偏大, 变体大小差异无实际代价
enum Command {
    /// 启动策略 (经 daemon 后台运行; 前台调试用 run)
    Start(ctrl::StartArgs),
    /// 停止策略 (优雅停机; 清理由策略 on_stop 执行)
    Stop(ctrl::StopArgs),
    /// 重启策略 (stop + start)
    Restart(ctrl::RestartArgs),
    /// 列出策略实例与状态
    List(instances::ListArgs),
    /// 查看状态 (无参列出全部; 带 name 查看单实例详情)
    Status(instances::StatusArgs),
    /// 查看单实例运行信息 (含成交统计与日志路径)
    Info(instances::InfoArgs),
    /// 查看成交记录 (按策略过滤)
    Fills(instances::FillsArgs),
    /// 策略进程管理器 (start/stop/status/run)
    Daemon(daemon::DaemonArgs),
    /// 前台运行策略 (Dry Run, 退出前无自动重启)
    Run(run::RunArgs),
    /// 命令行回测
    Backtest(backtest::BacktestArgs),
    /// 实时行情
    Ticker(market::TickerArgs),
    /// 盘口
    Orderbook(market::OrderbookArgs),
    /// K 线库管理
    Db(db::DbArgs),
    /// 日志 (读 logs/<name>.log, --follow 尾随)
    Logs(logs::LogsArgs),
    /// 批准待确认操作 (两步确认)
    Approve(approve::ApproveArgs),
    /// 提交 Lua 策略: 编译门禁 → 真实 K 线沙箱回测 → 生成待确认 preview (不落盘)
    Create(create::CreateArgs),
    /// 落盘已批准的建策略 preview (需 `ricow approve` 得到的一次性 token)
    Deploy(deploy::DeployArgs),
    /// AI 助手对话 (019): 自然语言提问; 省略 prompt 进入交互模式
    Ai(commands::ai::AiArgs),
    /// 生成外部 agent 操作手册 (019): AGENTS.md / SKILL.md / CLAUDE.md / lua-api.md; 内容与内置 AI 同源
    AgentKit(agentkit::AgentKitArgs),
}

#[tokio::main]
async fn main() {
    // rustls 的 crypto provider 必须**显式**选择: 依赖图里 aws-lc-rs(经 tokio-rustls 默认特性)与
    // ring(reqwest 的 rustls-tls)并存时, rustls 无法自动判定, WebSocket 建 TLS 会 panic
    // ("Could not automatically determine the process-level CryptoProvider")。
    // 实测(2026-09-14, 真实 demo 冒烟): 019 引入 rig/reqwest 0.13 后暴露; REST 侧因显式指定 provider 而不受影响。
    if rustls::crypto::ring::default_provider().install_default().is_err() {
        // 已被其它组件装过 → 保持既有选择(clap 的 --help 不该因此受影响, 静默即可)
    }

    // 日志一律写 stderr: stdout 留给命令输出与 AI 对话(`ricow mcp` 的 stdout 是 JSON-RPC 协议通道)。
    // 默认静默 rig 的 INFO 噪声(它会逐轮打印对话/工具调用细节), 需要时用 RUST_LOG 打开。
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rig=warn,rig_agent=warn,rig_core=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Start(args) => ctrl::start(args).await,
        Command::Stop(args) => ctrl::stop(args).await,
        Command::Restart(args) => ctrl::restart(args).await,
        Command::List(args) => instances::list(args).await,
        Command::Status(args) => instances::status(args).await,
        Command::Info(args) => instances::info(args).await,
        Command::Fills(args) => instances::fills(args).await,
        Command::Daemon(args) => daemon::run(args).await,
        Command::Run(args) => run::run(args).await,
        Command::Backtest(args) => backtest::run(args).await,
        Command::Ticker(args) => market::ticker(args).await,
        Command::Orderbook(args) => market::orderbook(args).await,
        Command::Db(args) => db::run(args).await,
        Command::Logs(args) => logs::run(args),
        Command::Approve(args) => approve::run(args).await,
        Command::Create(args) => create::run(args).await,
        Command::Deploy(args) => deploy::run(args).await,
        Command::Ai(args) => commands::ai::run(args).await,
        Command::AgentKit(args) => commands::agentkit::run(args),
    };

    if let Err(e) = result {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}
