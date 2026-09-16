//! `ricow ai` — 内置 AI 助手入口 (019): 单次模式 + 交互 REPL。
//!
//! 阶段一(T004/T005)范围: 打通"中文一句话 → 模型回答", 含流式与 token 用量。
//! 工具集(T009+)、确认块(T026+)、斜杠命令(实装)在后续阶段接入; 未实装的斜杠命令
//! 如实回"未接入", 不做假动作。

use std::io::{IsTerminal, Write};

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;

use crate::ai::confirm::{new_slot, ActionKind, PendingAction};
use crate::ai::{config, prompt, provider};

#[derive(Args)]
pub struct AiArgs {
    /// 一句话提问(省略则进入交互模式)
    pub prompt: Option<String>,
    /// 临时覆盖模型名(优先于 ai.toml 与 RICOW_AI_MODEL)
    #[arg(long)]
    pub model: Option<String>,
    /// 临时覆盖 base_url(优先于 ai.toml 与 RICOW_AI_BASE_URL)
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
    /// 非流式输出(便于脚本/管道; 也可诊断某些端点在流式下不返回工具调用的问题)
    #[arg(long)]
    pub plain: bool,
}

/// 交互模式下的一行输入分类(纯函数, 便于单测)。
#[derive(Debug, PartialEq, Eq)]
enum Input {
    /// 空行
    Empty,
    /// 退出
    Exit,
    /// 帮助
    Help,
    /// 提问
    Ask(String),
    /// 未接入的斜杠命令
    Unknown(String),
}

/// 分类用户输入。
fn classify(line: &str) -> Input {
    let t = line.trim();
    if t.is_empty() {
        return Input::Empty;
    }
    if let Some(cmd) = t.strip_prefix('/') {
        let name = cmd.split_whitespace().next().unwrap_or("");
        return match name {
            "exit" | "quit" | "q" => Input::Exit,
            "help" | "?" => Input::Help,
            _ => Input::Unknown(name.to_string()),
        };
    }
    Input::Ask(t.to_string())
}

/// 帮助文案(与未接入提示共用的边界说明)。
///
/// 工具名取自白名单常量(019): 不再手抄, 避免白名单扩容后这里静默过期。
fn help_text() -> String {
    format!(
        "\
命令: /help 帮助 · /exit 退出
可用自然语言提问, 例如: \"我部署了哪些策略\"
只读工具({} 个, 可直接调用): {read}
虚拟工具({} 个, 可直接调用但要告知副作用): {virt}
边界: 落盘部署 / 启动测试网 demo 可在对话内逐字确认; 实盘启停 / 停 demo / 平仓 / 改参数仍须你本人在终端执行",
        crate::ai::tools::READ_ONLY_TOOLS.len(),
        crate::ai::tools::VIRTUAL_TOOLS.len(),
        read = crate::ai::tools::READ_ONLY_TOOLS.join(" / "),
        virt = crate::ai::tools::VIRTUAL_TOOLS.join(" / ")
    )
}

/// 打印运行信息(通道 + 只读工具数 + 数据外发一行提示)。
fn print_session_banner(resolved: &config::Resolved, is_local: bool, tool_count: usize) {
    println!("ricow AI 助手 (供应商: {} / 模型: {})", resolved.provider, resolved.model);
    let key_from_env =
        std::env::var(config::ENV_API_KEY).ok().is_some_and(|v| !v.trim().is_empty());
    println!(
        "  密钥: {} ([ai].api_key; 环境变量可覆盖)",
        if key_from_env { "来自环境变量" } else { "来自 ricow.toml" }
    );
    println!("  端点: {}{}", resolved.base_url, if is_local { " (本机)" } else { "" });
    println!(
        "  上限: 每轮最多 {} 次模型调用; 只发送你的问题与工具返回(不含密钥)",
        resolved.max_turns
    );
    println!(
        "  工具: {tool_count} 个(只读 {} + 虚拟 {}); 写操作不在工具内, 必须你本人确认",
        crate::ai::tools::READ_ONLY_TOOLS.len(),
        crate::ai::tools::VIRTUAL_TOOLS.len()
    );
}

pub async fn run(args: AiArgs) -> CoreResult<()> {
    let root = crate::commands::project_root();
    // 唯一配置文件: ricow.toml(缺文件时生成模板); [ai] 段同时含 provider 与 api_key
    let file = crate::commands::config_file::load(&root)?;
    let cfg = config::AiConfig {
        provider: file.ai.provider.clone(),
        model: file.ai.model.clone().unwrap_or_default(),
        base_url: file.ai.base_url.clone(),
        max_turns: file
            .ai
            .max_turns
            .map(config::check_max_turns)
            .transpose()?
            .unwrap_or(config::DEFAULT_MAX_TURNS),
    };

    // 覆盖优先级: 命令行参数 > 环境变量 > ricow.toml > 预设
    let env_base_url = args.base_url.clone().or_else(|| std::env::var(config::ENV_BASE_URL).ok());
    let env_model = args.model.clone().or_else(|| std::env::var(config::ENV_MODEL).ok());
    let resolved = config::resolve(&cfg, env_base_url, env_model)?;

    let is_local = provider::is_local_endpoint(&resolved.base_url);
    let api_key =
        provider::resolve_key(&resolved.base_url, config::api_key(&root, &resolved.provider))?;

    // 配置文件权限提示(含密钥, 只提示不自动改)
    if let Some(w) = crate::commands::config_file::permission_warning(&root) {
        eprintln!("提示: {w}");
    }

    // 工具集: L0 只读 + L1 虚拟。写实动作没有工具面 —— 对话内确认也只登记 pending,
    // 由下面 REPL 宿主在用户逐字输入后执行(见 `crate::ai::confirm`)。
    // 单次模式即便 stdin 是 tty 也不开放对话内确认(没有第二轮输入可承接短语)。
    let pending = new_slot();
    let is_repl = args.prompt.is_none();
    let tool_ctx = crate::ai::tools::ToolCtx::new(
        root.clone(),
        is_repl && std::io::stdin().is_terminal(),
        pending.clone(),
    );
    let tools = crate::ai::tools::build(tool_ctx);
    let tool_count = tools.len();
    print_session_banner(&resolved, is_local, tool_count);
    let llm = provider::connect(resolved.clone(), &api_key, &prompt::system_preamble(), tools)?;

    // 单次模式
    if let Some(q) = args.prompt.as_deref() {
        let q = q.trim();
        if q.is_empty() {
            return Err(CoreError::InvalidArgument("提问为空; 省略参数则进入交互模式".into()));
        }
        let ans = if args.plain {
            let a = llm.ask(q).await?;
            println!("{}", a.text);
            a
        } else {
            llm.ask_stream(q, &[]).await?
        };
        if let Some(u) = &ans.usage {
            println!("[用量] {u}");
        }
        return Ok(());
    }

    // 交互模式
    println!();
    println!("{}", help_text());
    println!();
    let mut history: Vec<rig::message::Message> = Vec::new();
    loop {
        let Some(line) = read_line("你 > ").await else {
            println!();
            println!("输入结束, 退出。");
            break;
        };
        match classify(&line) {
            Input::Empty => continue,
            Input::Exit => {
                println!("再见。");
                break;
            }
            Input::Help => {
                println!("{}", help_text());
                continue;
            }
            Input::Unknown(name) => {
                println!("斜杠命令 /{name} 尚未接入(工具与运行管理在后续阶段实现); 当前可用: /help /exit");
                continue;
            }
            Input::Ask(q) => {
                // 对话内确认状态机优先: 有 pending 时, 这行先判 确认 / 拒绝 / 过期 / 普通提问。
                // 短语只认真实用户输入行, 不经过模型 —— 模型输出永远无法走到执行分支。
                match crate::ai::confirm::consume_line(&pending, &line).await {
                    crate::ai::confirm::LineDisposition::Confirm(action) => {
                        match execute_confirmed(&action, &root).await {
                            Ok(msg) => println!("{msg}"),
                            Err(e) => println!(
                                "执行失败: {e}\n(确认块已消费; 若是落盘预览已被批准/消费, 请用 ricow status / 文件系统核对实际状态, 必要时重新发起)"
                            ),
                        }
                    }
                    crate::ai::confirm::LineDisposition::Reject(action) => {
                        // deploy: 尽力把 preview 置 rejected 终态(失败也不影响本地作废语义)
                        if let (ActionKind::Deploy, Some(id)) =
                            (action.kind, action.preview_id.as_deref())
                        {
                            if let Ok(db) = Database::open(&root.join("ricow.db")).await {
                                _ = ricow_engine::reject(&db, id).await;
                            }
                        }
                        println!(
                            "已放弃待确认动作「{} / {}」, 未执行任何写实操作。",
                            action.kind.label(),
                            action.name
                        );
                    }
                    crate::ai::confirm::LineDisposition::Expired(action) => {
                        println!(
                            "待确认动作「{} / {}」已超过 15 分钟, 已作废; 如需继续请重新发起。",
                            action.kind.label(),
                            action.name
                        );
                        ask_llm(&llm, &args.plain, &q, &mut history).await;
                    }
                    crate::ai::confirm::LineDisposition::Other
                    | crate::ai::confirm::LineDisposition::NoPending => {
                        ask_llm(&llm, &args.plain, &q, &mut history).await;
                    }
                }
            }
        }
    }
    Ok(())
}

/// 把一轮普通提问送模型(流式/非流式), 打印答复与用量, 维护历史; 错误如实打印不吞。
async fn ask_llm(
    llm: &crate::ai::provider::Llm,
    plain: &bool,
    q: &str,
    history: &mut Vec<rig::message::Message>,
) {
    let reply = if *plain {
        llm.ask(q).await.inspect(|a| {
            println!("{}", a.text);
        })
    } else {
        llm.ask_stream(q, history).await
    };
    match reply {
        Ok(ans) => {
            if let Some(u) = &ans.usage {
                println!("[用量] {u}");
            }
            history.push(rig::message::Message::user(q.to_string()));
            history.push(rig::message::Message::assistant(ans.text));
        }
        Err(e) => {
            // 如实报错, 不吞: 网络/鉴权/模型不支持工具调用都会走到这里
            println!("错误: {e}");
        }
    }
}

/// 宿主执行已确认动作(019 R3): 复用与终端完全相同的引擎内核, 不经 shell。
///
/// - Deploy = `engine::approve` 取一次性 token → `engine::execute_strategy` 落盘(与 deploy.rs 同函数);
/// - StartDemo = `ctrl::start_daemon(demo=true)`(daemon 对 demo 不校验 confirmed/live_enabled)。
pub(crate) async fn execute_confirmed(
    action: &PendingAction,
    root: &std::path::Path,
) -> CoreResult<String> {
    match action.kind {
        ActionKind::Deploy => {
            let id = action.preview_id.as_deref().ok_or_else(|| {
                CoreError::InvalidArgument("内部状态错误: deploy 待办缺 preview_id".into())
            })?;
            let db = Database::open(&root.join("ricow.db"))
                .await
                .map_err(|e| CoreError::Exchange(e.to_string()))?;
            let dir = crate::commands::ensure_strategies_dir_in(root)?;
            let token = ricow_engine::approve(&db, id).await?;
            let (toml_path, lua_path) =
                ricow_engine::execute_strategy(&db, id, &token, &dir).await?;
            Ok(format!(
                "已确认并完成落盘:\n  {}\n  {}\n\
                 下一步:\n  Dry Run: ricow run {name}\n  测试网: ricow start {name} --demo",
                toml_path.display(),
                lua_path.display(),
                name = action.name
            ))
        }
        ActionKind::StartDemo => {
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, false, true, false).await?;
            Ok(format!(
                "已确认: {} 正在以测试网 demo 启动, pid={pid}(无真实资金; 会真实向测试网下单/撤单)。\n\
                 停机须你本人执行: ricow stop {name}",
                crate::commands::instances::mode_text(&mode),
                name = action.name
            ))
        }
    }
}

/// 读一行(阻塞读放 spawn_blocking, 不阻塞 tokio runtime; EOF/读失败 → None)。
async fn read_line(prompt: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_input() {
        assert_eq!(classify("   "), Input::Empty);
        assert_eq!(classify("/exit"), Input::Exit);
        assert_eq!(classify("/q"), Input::Exit);
        assert_eq!(classify("/help"), Input::Help);
        assert_eq!(classify("/status"), Input::Unknown("status".into()));
        assert_eq!(classify("回测 ETH 30 天"), Input::Ask("回测 ETH 30 天".into()));
        // 斜杠命令带参数时只取命令名
        assert_eq!(classify("/stop shannon_grid"), Input::Unknown("stop".into()));
    }

    #[test]
    fn test_help_text_mentions_limits() {
        let h = help_text();
        assert!(h.contains("/exit") && h.contains("确认"), "{h}");
    }
}
