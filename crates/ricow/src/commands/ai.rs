//! `ricow ai` — 内置 AI 助手入口 (019): 单次模式 + 交互 REPL。
//!
//! 本文件只剩 **clap 参数 + 终端薄壳**: 业务逻辑全部在 [`crate::ai::session`]
//! (`ChatSession` + [`crate::commands::chat::StdioSink`]), 与裸入口 `ricow` 完全同一条路径。
//! 工具集(T009+)、确认块、斜杠命令的边界说明见 session 模块头。

use clap::Args;
use ricow_core::CoreResult;

use crate::ai::session::{ChatSession, Options};

#[derive(Args)]
pub struct AiArgs {
    /// 一句话提问(省略则进入交互模式)
    pub prompt: Option<String>,
    /// 临时覆盖模型名(优先于 ricow.toml 与 RICOW_AI_MODEL)
    #[arg(long)]
    pub model: Option<String>,
    /// 临时覆盖 base_url(优先于 ricow.toml 与 RICOW_AI_BASE_URL)
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
    /// 非流式输出(便于脚本/管道; 也可诊断某些端点在流式下不返回工具调用的问题)
    #[arg(long)]
    pub plain: bool,
}

pub async fn run(args: AiArgs) -> CoreResult<()> {
    let root = crate::commands::project_root();

    // 配置文件权限提示(含密钥, 只提示不自动改)
    if let Some(w) = crate::commands::config_file::permission_warning(&root) {
        eprintln!("提示: {w}");
    }

    // 单次模式即便 stdin 是 tty 也不开放对话内确认(没有第二轮输入可承接短语)。
    let interactive = args.prompt.is_none();
    let mut session = ChatSession::open(
        root,
        Options { plain: args.plain, model: args.model, base_url: args.base_url, interactive },
    )
    .await?;

    let mut sink = super::chat::StdioSink;
    session.welcome(&mut sink);

    match args.prompt.as_deref() {
        Some(q) => session.ask_once(q, &mut sink).await,
        None => super::chat::repl(&mut session, &mut sink).await,
    }
}
