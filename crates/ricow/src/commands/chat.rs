//! 裸入口 + REPL 薄壳 (019-R4) —— 用户只需要记一个命令: `ricow`。
//!
//! 本模块只有终端 I/O: 读一行、把 [`SessionSink`] 收到的东西写出去、判退出。
//! **所有业务逻辑在 [`crate::ai::session`]** —— 将来接网页端换个 sink 即可, 这里零改动。
//!
//! 启动顺序(与计划 §二 A 一致): 打印数据目录 → 加载/生成 ricow.toml → 首次向导(缺语言/密钥才问)
//! → 开会话 → REPL; 固定文案按 `[ui].lang` 呈现(023: 向导里刚选定的语言立即生效)。

use std::io::{IsTerminal, Write};

use ricow_core::CoreResult;

use crate::ai::session::{self, ChatSession, Options, SessionSink, Step};
use crate::i18n::{self, t};

use super::config_file;

/// 终端输出出口: 助手增量直写 stdout(流式), 整行走 stdout 一行。
pub struct StdioSink;

impl SessionSink for StdioSink {
    fn text(&mut self, chunk: &str) {
        print!("{chunk}");
        let _ = std::io::stdout().flush();
    }

    fn line(&mut self, text: &str) {
        println!("{text}");
    }

    /// 密钥静默录入: `rpassword`(Windows/Linux/macOS 同一实现), 不回显、不进日志。
    /// 只在交互式终端里会被调到(会话侧有 tty 门禁), 因此这里不做额外校验。
    fn secret(&mut self, prompt: &str) -> Option<String> {
        let _ = std::io::stdout().flush();
        rpassword::prompt_password(prompt).ok()
    }
}

/// 裸入口: `ricow`(无子命令)。
pub async fn run() -> CoreResult<()> {
    let root = crate::commands::project_root();
    // 语言(023): 已有配置按 `[ui].lang`; 首次运行尚未选择 → 默认中文(向导最前会问一次, FR-002/FR-003)。
    let lang = i18n::resolve(&config_file::load(&root)?);
    println!("{}: {}", t(lang, "ricow 数据目录", "ricow data dir"), root.display());
    println!("{}: {}", t(lang, "配置文件", "config"), config_file::path(&root).display());

    // 首次向导: 缺语言/AI 密钥/demo 凭据时才问; 非交互式终端在这里报错并给出路径(strict)。
    crate::commands::onboard::run_if_needed(&root, true).await?;

    // 向导可能刚写入 `[ui].lang` → 之后一律用最新语言。
    let lang = i18n::resolve(&config_file::load(&root)?);

    // 权限提示(含密钥, 只提示不自动改)
    if let Some(w) = config_file::permission_warning(&root) {
        eprintln!("{}: {w}", t(lang, "提示", "note"));
    }

    let mut session = ChatSession::open(
        root,
        Options { plain: false, model: None, base_url: None, interactive: true },
    )
    .await?;
    let mut sink = StdioSink;
    session.welcome(&mut sink);

    // 非交互式 stdin(管道/CI): 只跑会话启动, 不进入读行循环(REPL 会立刻 EOF 退出)。
    if !std::io::stdin().is_terminal() {
        sink.line(t(lang, "非交互式输入: 已退出会话。", "non-interactive input: session ended."));
        return Ok(());
    }
    repl(&mut session, &mut sink).await
}

/// 交互循环(裸入口与 `ricow ai` 共用): 读一行 → 交会话 → 判退出。
pub async fn repl(session: &mut ChatSession, sink: &mut StdioSink) -> CoreResult<()> {
    sink.line("");
    sink.line(&session::help_text(session.lang()));
    sink.line("");
    loop {
        let Some(line) = read_line(t(session.lang(), "你 > ", "you > ")).await else {
            sink.line("");
            sink.line(t(session.lang(), "输入结束, 退出。", "end of input, exiting."));
            break;
        };
        if session.handle_line(&line, sink).await? == Step::Exit {
            break;
        }
    }
    Ok(())
}

/// 读一行(阻塞读放 spawn_blocking, 不阻塞 tokio runtime; EOF/读失败 → None)。
pub async fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        match std::io::stdin().read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s),
        }
    })
    .await
    .ok()
    .flatten()
    .map(|s| s.trim_end().to_string())
}
