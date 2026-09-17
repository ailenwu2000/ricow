//! 会话缝 (019-R4) —— 对话业务逻辑与终端 I/O 的**唯一**分隔线。
//!
//! `ChatSession` 持有 root/配置/LLM/pending/history, 对外只有 [`SessionSink`] 一个出口:
//! 本模块**不碰 stdin/stdout**, 也不打印任何东西。`commands::ai`(薄壳)与
//! `commands::chat`(裸入口)只负责"读一行 → 交给 session → 把 sink 收到的东西写出去"。
//!
//! 将来接网页端 = 新写一个 WS/HTTP sink 并复用本文件, 业务逻辑零复制。
//! 安全口径不变: 写实动作**没有工具调用面**; 模型只能登记 pending, 执行权在宿主
//! (见 [`super::confirm`])。

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;
use rig::message::Message;

use super::confirm::{self, ActionKind, LineDisposition, PendingAction, PendingSlot};
use super::{config, prompt, provider, tools};

/// 会话输出出口(终端 = stdio; 将来网页端 = WS 帧)。
///
/// 两个输出方法(流式文本增量与完整一行) + 一个**密钥录入**方法。新增前端不必理解
/// 会话内部状态, 但必须能安全地拿到用户粘贴的密钥(不回显、不进日志、不进模型上下文)。
pub trait SessionSink {
    /// 助手文本增量(不保证以换行结尾, 终端实现要 flush)。
    fn text(&mut self, chunk: &str);
    /// 一整行宿主输出(提示 / 用量 / 空行 / 错误)。
    fn line(&mut self, text: &str);
    /// 读一行**不回显**的密钥输入(`/keys` 用; 实现可以是阻塞读, 与终端语义一致)。
    ///
    /// 返回 `None` = 本前端无法安全录入(无输入通道); 会话据此**放弃本次修改且不写文件**。
    fn secret(&mut self, prompt: &str) -> Option<String>;
}

/// 一行输入的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 会话继续。
    Continue,
    /// 用户要求退出(`/exit` `/quit` `/q`)。
    Exit,
}

/// 会话启动参数(网页端在此扩展, 不动会话逻辑)。
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// 非流式输出(脚本/管道友好)。
    pub plain: bool,
    /// 临时覆盖模型(命令行 > `RICOW_AI_MODEL`)。
    pub model: Option<String>,
    /// 临时覆盖 base_url(命令行 > `RICOW_AI_BASE_URL`)。
    pub base_url: Option<String>,
    /// 是否允许**对话内确认**: 单次模式(false)没有第二轮输入承接确认短语,
    /// 因此工具侧不登记 pending(tty 门禁的另一半)。
    pub interactive: bool,
}

/// 一次会话的全部可变状态。
pub struct ChatSession {
    root: PathBuf,
    resolved: config::Resolved,
    is_local: bool,
    /// 是否可对话内确认 / 静默录入密钥(stdin 是 tty 且为 REPL); 重建客户端时要复用。
    interactive: bool,
    llm: provider::Llm,
    pending: PendingSlot,
    history: Vec<Message>,
    plain: bool,
}

impl ChatSession {
    /// 装载配置 + 建会话。任何配置问题都在这里**报错并给解法**(不静默回落)。
    pub async fn open(root: PathBuf, opts: Options) -> CoreResult<Self> {
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
        let env_base_url =
            opts.base_url.clone().or_else(|| std::env::var(config::ENV_BASE_URL).ok());
        let env_model = opts.model.clone().or_else(|| std::env::var(config::ENV_MODEL).ok());
        let resolved = config::resolve(&cfg, env_base_url, env_model)?;

        let is_local = provider::is_local_endpoint(&resolved.base_url);
        let api_key =
            provider::resolve_key(&resolved.base_url, config::api_key(&root, &resolved.provider))?;

        // 工具集: L0 只读 + L1 虚拟。写实动作没有工具面 —— 对话内确认也只登记 pending,
        // 由本会话在用户逐字输入后执行(见 `execute_confirmed`)。
        let pending = confirm::new_slot();
        let interactive = opts.interactive && std::io::stdin().is_terminal();
        let tool_ctx = tools::ToolCtx::new(root.clone(), interactive, pending.clone());
        let llm = provider::connect(
            resolved.clone(),
            &api_key,
            &prompt::system_preamble(),
            tools::build(tool_ctx),
        )?;

        Ok(Self {
            root,
            resolved,
            is_local,
            interactive,
            llm,
            pending,
            history: Vec::new(),
            plain: opts.plain,
        })
    }

    /// 会话开始信息(通道 / 密钥来源 / 端点 / 上限 / 工具面); 不打印, 只走 sink。
    pub fn welcome(&self, sink: &mut dyn SessionSink) {
        sink.line(&format!(
            "ricow AI 助手 (供应商: {} / 模型: {})",
            self.resolved.provider, self.resolved.model
        ));
        let key_from_env =
            std::env::var(config::ENV_API_KEY).ok().is_some_and(|v| !v.trim().is_empty());
        sink.line(&format!(
            "  密钥: {} ([ai].api_key; 环境变量可覆盖)",
            if key_from_env { "来自环境变量" } else { "来自 ricow.toml" }
        ));
        sink.line(&format!(
            "  端点: {}{}",
            self.resolved.base_url,
            if self.is_local { " (本机)" } else { "" }
        ));
        sink.line(&format!(
            "  上限: 每轮最多 {} 次模型调用; 只发送你的问题与工具返回(不含密钥)",
            self.resolved.max_turns
        ));
        sink.line(&format!(
            "  工具: {} 个(只读 {} + 虚拟 {}); 写操作不在工具内, 必须你本人确认",
            tools::READ_ONLY_TOOLS.len() + tools::VIRTUAL_TOOLS.len(),
            tools::READ_ONLY_TOOLS.len(),
            tools::VIRTUAL_TOOLS.len()
        ));
        if let Some(hint) = empty_state_hint(&self.root) {
            sink.line("");
            sink.line(&hint);
        }
    }

    /// 处理一行用户输入(不含任何 I/O); REPL 只关心返回值是不是 [`Step::Exit`]。
    pub async fn handle_line(
        &mut self,
        line: &str,
        sink: &mut dyn SessionSink,
    ) -> CoreResult<Step> {
        match classify(line) {
            LineInput::Empty => Ok(Step::Continue),
            LineInput::Exit => {
                sink.line("再见。");
                Ok(Step::Exit)
            }
            LineInput::Help => {
                sink.line(&help_text());
                Ok(Step::Continue)
            }
            LineInput::Market(cmd) => {
                self.handle_market(cmd, sink).await?;
                Ok(Step::Continue)
            }
            LineInput::Keys(cmd) => {
                self.handle_keys(cmd, sink).await?;
                Ok(Step::Continue)
            }
            LineInput::Unknown(name) => {
                sink.line(&format!(
                    "斜杠命令 /{name} 尚未接入; 当前可用: /help /exit /market /keys"
                ));
                Ok(Step::Continue)
            }
            LineInput::Ask(q) => {
                // 对话内确认状态机优先: 有 pending 时, 这行先判 确认 / 拒绝 / 过期 / 普通提问。
                // 短语只认真实用户输入行, 不经过模型 —— 模型输出永远无法走到执行分支。
                match confirm::consume_line(&self.pending, line).await {
                    LineDisposition::Confirm(action) => match self.execute(&action).await {
                        Ok(msg) => sink.line(&msg),
                        Err(e) => sink.line(&format!(
                            "执行失败: {e}\n(确认块已消费; 若是落盘预览已被批准/消费, 请用 ricow status / 文件系统核对实际状态, 必要时重新发起)"
                        )),
                    },
                    LineDisposition::Reject(action) => {
                        // deploy: 尽力把 preview 置 rejected 终态(失败也不影响本地作废语义)
                        if let (ActionKind::Deploy, Some(id)) = (action.kind, action.preview_id.as_deref())
                        {
                            if let Ok(db) = Database::open(&crate::commands::db_path_in(&self.root))
                                .await
                            {
                                _ = ricow_engine::reject(&db, id).await;
                            }
                        }
                        sink.line(&format!(
                            "已放弃待确认动作「{}」, 未执行任何写实操作。",
                            action_display(&action)
                        ));
                    }
                    LineDisposition::Expired(action) => {
                        // 分钟数取自 `ai::confirm::PENDING_TTL`(与引擎 preview TTL 同源), 不手抄 15。
                        sink.line(&format!(
                            "待确认动作「{}」已超过 {} 分钟, 已作废; 如需继续请重新发起。",
                            action_display(&action),
                            crate::ai::confirm::PENDING_TTL.as_secs() / 60
                        ));
                        self.reply(&q, sink).await;
                    }
                    LineDisposition::Other | LineDisposition::NoPending => self.reply(&q, sink).await,
                }
                Ok(Step::Continue)
            }
        }
    }

    /// `/market` 交易对视野: 只读展示 / 切换默认与全量(切换**落盘** `[market] show_all_pairs`)。
    ///
    /// 与 CLI `ricow pairs`、AI 工具 `list_pairs` 共用同一份核心 (`commands::pairs`),
    /// 因此视野口径永远一致; 这里不碰 stdio, 只把结果交给 sink。
    async fn handle_market(
        &mut self,
        cmd: MarketCmd,
        sink: &mut dyn SessionSink,
    ) -> CoreResult<()> {
        use crate::commands::{config_file::SetValue, pairs};

        let switch = match cmd {
            MarketCmd::Bad(arg) => {
                sink.line(&format!(
                    "/market 参数只支持 bstock / all; 收到: {arg}\n\
                     用法: /market(查看当前视野) · /market bstock(仅股票类, 默认) · /market all(全部交易对)"
                ));
                return Ok(());
            }
            MarketCmd::Show => None,
            MarketCmd::Bstock => Some(false),
            MarketCmd::All => Some(true),
        };

        if let Some(show_all) = switch {
            crate::commands::config_file::set_values(
                &self.root,
                &[("market", "show_all_pairs", SetValue::Bool(show_all))],
            )?;
            // 切换后立刻按新配置重算规模(快照走缓存, 不额外联网), 只报规模不刷全表。
            let view = pairs::current_view(&self.root, false).await?;
            sink.line(&format!(
                "已切换交易对视野: {}\n  ricow.toml [market] show_all_pairs = {show_all}\n  \
                 现货 {} 个 / 合约 {} 个 / 合计 {}\n列表: /market",
                pairs::scope_text(view.filtered),
                view.spot.len(),
                view.futures.len(),
                view.total()
            ));
            return Ok(());
        }

        let view = pairs::current_view(&self.root, false).await?;
        sink.line(&pairs::render(&view, None));
        sink.line("切换: /market bstock(仅股票类) · /market all(全部交易对)");
        Ok(())
    }

    /// `/keys` 密钥管理: 查看状态 / **静默录入**并外科式更新 `ricow.toml`(只改对应行, 注释保留)。
    ///
    /// 与首次向导([`crate::commands::onboard`])同一条落盘路径([`config_file::set_values`]):
    /// 键名白名单 / 原子写 / 0600 完全一致; 这里多做一件事 —— 改 AI 密钥后**本会话立即重建
    /// 客户端**(不必退出重开), 并如实说明环境变量覆盖时文件值是无效的。
    async fn handle_keys(&mut self, cmd: KeysCmd, sink: &mut dyn SessionSink) -> CoreResult<()> {
        use crate::commands::config_file::{self, SetValue};

        let before = config_file::load(&self.root)?;
        let target = match cmd {
            KeysCmd::Show => {
                show_keys(&before, &self.root, sink);
                return Ok(());
            }
            KeysCmd::Ai => KeysTarget::Ai,
            KeysCmd::Demo => KeysTarget::Demo,
            KeysCmd::Live => KeysTarget::Live,
            KeysCmd::Bad(arg) => {
                sink.line(&format!(
                    "/keys 参数只支持 ai / demo / live; 收到: {arg}\n\
                     用法: /keys(查看状态) · /keys ai(AI 助手密钥) · /keys demo(测试网凭据) · \
                     /keys live(主网/实盘凭据)"
                ));
                return Ok(());
            }
        };

        // 密钥录入只在交互式会话里开放(与对话内确认同一 tty 门禁): 拿不到静默输入就不写文件。
        if !self.interactive {
            sink.line(
                "当前不是交互式终端, 无法安全录入密钥; 请在终端直接运行 ricow 后再用 /keys。",
            );
            return Ok(());
        }

        let env_override =
            std::env::var(config::ENV_API_KEY).ok().is_some_and(|v| !v.trim().is_empty());
        let mut updates: Vec<(&'static str, &'static str, SetValue)> = Vec::new();
        match target {
            KeysTarget::Ai => {
                if env_override {
                    sink.line(&format!(
                        "注意: 环境变量 {} 已设置且**优先于**本文件; 写入后需取消该变量才生效。",
                        config::ENV_API_KEY
                    ));
                }
                match read_secret(sink, "粘贴新的 AI API Key(不回显; 回车放弃): ") {
                    Some(k) => updates.push(("ai", "api_key", SetValue::Str(k))),
                    None => return Ok(()),
                }
            }
            KeysTarget::Demo => {
                sink.line(
                    "币安测试网(demo)凭据: demo.binance.com → API 管理 → 创建 Key(只勾交易, 不要提现)。",
                );
                match read_pair(sink, "demo_key", "demo_secret") {
                    Some((k, s)) => {
                        updates.push(("exchange", "demo_key", SetValue::Str(k)));
                        updates.push(("exchange", "demo_secret", SetValue::Str(s)));
                    }
                    None => return Ok(()),
                }
            }
            KeysTarget::Live => {
                sink.line("币安主网(实盘)凭据: 建议用只开交易、关闭提现的受限 Key 或子账户。");
                match read_pair(sink, "binance_key", "binance_secret") {
                    Some((k, s)) => {
                        updates.push(("exchange", "binance_key", SetValue::Str(k)));
                        updates.push(("exchange", "binance_secret", SetValue::Str(s)));
                    }
                    None => return Ok(()),
                }
            }
        }

        config_file::set_values(&self.root, &updates)?;
        sink.line(&format!(
            "已更新 {}: {}",
            config_file::path(&self.root).display(),
            updates.iter().map(|(s, k, _)| format!("[{s}].{k}")).collect::<Vec<_>>().join(", ")
        ));

        // 环境变量覆盖时不重建(重建也只会用环境变量里的值, 报"已生效"就是撒谎)。
        if target == KeysTarget::Ai && !env_override {
            let resolved = self.resolved.clone();
            let key = provider::resolve_key(
                &resolved.base_url,
                config::api_key(&self.root, &resolved.provider),
            )?;
            let ctx =
                tools::ToolCtx::new(self.root.clone(), self.interactive, self.pending.clone());
            self.llm =
                provider::connect(resolved, &key, &prompt::system_preamble(), tools::build(ctx))?;
            sink.line("已用新密钥重建 LLM 客户端, 下一句话即生效。");
        }
        if let Some(w) = config_file::permission_warning(&self.root) {
            sink.line(&format!("提示: {w}"));
        }
        Ok(())
    }

    /// 单次提问(非 REPL): 不消费 pending、不写 history —— 与 `ricow ai "<问题>"` 语义一致。
    pub async fn ask_once(&mut self, question: &str, sink: &mut dyn SessionSink) -> CoreResult<()> {
        let q = question.trim();
        if q.is_empty() {
            return Err(CoreError::InvalidArgument("提问为空; 省略参数则进入交互模式".into()));
        }
        self.reply(q, sink).await;
        Ok(())
    }

    /// 把一轮普通提问送模型(流式/非流式), 用量与错误都如实走 sink, 维护 history。
    async fn reply(&mut self, q: &str, sink: &mut dyn SessionSink) {
        let reply = if self.plain {
            self.llm.ask(q).await.inspect(|a| sink.line(&a.text))
        } else {
            self.llm.ask_stream(q, &self.history, sink).await
        };
        match reply {
            Ok(ans) => {
                if let Some(u) = &ans.usage {
                    sink.line(&format!("[用量] {u}"));
                }
                self.history.push(Message::user(q.to_string()));
                self.history.push(Message::assistant(ans.text));
            }
            // 如实报错, 不吞: 网络/鉴权/模型不支持工具调用都会走到这里
            Err(e) => sink.line(&format!("错误: {e}")),
        }
    }

    /// 宿主执行已确认动作(019 R3): 复用与终端完全相同的引擎内核, 不经 shell。
    ///
    /// - Deploy = `engine::approve` 取一次性 token → `engine::execute_strategy` 落盘(与 deploy.rs 同函数);
    /// - StartDemo = `ctrl::start_daemon(demo=true)`(daemon 对 demo 不校验 confirmed/live_enabled)。
    pub async fn execute(&self, action: &PendingAction) -> CoreResult<String> {
        execute_confirmed(action, &self.root).await
    }
}

/// 会话内一行的分类(纯函数, 便于单测)。
#[derive(Debug, PartialEq, Eq)]
pub enum LineInput {
    /// 空行。
    Empty,
    /// 退出。
    Exit,
    /// 帮助。
    Help,
    /// `/market` 交易对视野。
    Market(MarketCmd),
    /// `/keys` 密钥管理。
    Keys(KeysCmd),
    /// 提问。
    Ask(String),
    /// 尚未接入的斜杠命令。
    Unknown(String),
}

/// `/market` 的意图(纯解析, 不做 I/O)。
#[derive(Debug, PartialEq, Eq)]
pub enum MarketCmd {
    /// `/market` — 只看当前视野。
    Show,
    /// `/market bstock` — 切回默认(仅股票类)。
    Bstock,
    /// `/market all` — 放开为全部交易对。
    All,
    /// `/market <其它>` — 参数不认(原样带出, 便于回报)。
    Bad(String),
}

/// `/keys` 的意图(纯解析, 不做 I/O)。
#[derive(Debug, PartialEq, Eq)]
pub enum KeysCmd {
    /// `/keys` — 只看状态(不回显密钥全文)。
    Show,
    /// `/keys ai` — AI 助手密钥 `[ai].api_key`。
    Ai,
    /// `/keys demo` — 测试网凭据 `[exchange].demo_key/demo_secret`。
    Demo,
    /// `/keys live` — 主网(实盘)凭据 `[exchange].binance_key/binance_secret`。
    Live,
    /// `/keys <其它>` — 参数不认(原样带出, 便于回报)。
    Bad(String),
}

/// 一次 `/keys` 要改哪一组凭据(与 [`KeysCmd`] 的写入臂一一对应)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeysTarget {
    Ai,
    Demo,
    Live,
}

/// 分类用户输入。
pub fn classify(line: &str) -> LineInput {
    let t = line.trim();
    if t.is_empty() {
        return LineInput::Empty;
    }
    if let Some(cmd) = t.strip_prefix('/') {
        let mut words = cmd.split_whitespace();
        let name = words.next().unwrap_or("");
        return match name {
            "exit" | "quit" | "q" => LineInput::Exit,
            "help" | "?" => LineInput::Help,
            "market" => LineInput::Market(match words.next().map(|s| s.to_ascii_lowercase()) {
                None => MarketCmd::Show,
                Some(w) if w == "bstock" || w == "stock" || w == "default" => MarketCmd::Bstock,
                Some(w) if w == "all" => MarketCmd::All,
                Some(other) => MarketCmd::Bad(other),
            }),
            "keys" => LineInput::Keys(match words.next().map(|s| s.to_ascii_lowercase()) {
                None => KeysCmd::Show,
                Some(w) if w == "ai" => KeysCmd::Ai,
                Some(w) if w == "demo" => KeysCmd::Demo,
                Some(w) if w == "live" => KeysCmd::Live,
                Some(other) => KeysCmd::Bad(other),
            }),
            _ => LineInput::Unknown(name.to_string()),
        };
    }
    LineInput::Ask(t.to_string())
}

/// 空状态提示: 还没有任何策略(无 *.toml)时给"两条路"话术, 否则 None。
///
/// 纯本地目录检查(不触网): 会话启动即打印, 语义是"策略是空的, 从哪开始"。
pub fn empty_state_hint(root: &Path) -> Option<String> {
    if !tools::list_toml_stems(&root.join("strategies")).is_empty() {
        return None;
    }
    Some(
        "现在还没有任何策略 —— 两条路都行:\n\
         ① 从模板起步: 说\"看看模板\", 我列出内置模板(如 shannon_grid 中轴再平衡), 你挑一个, 我再问交易对与参数;\n\
         ② 全新编写: 直接说需求(例: \"给 AAPL 做 50:50 再平衡, 每次 0.01\"), 我按 Lua API 写代码并先跑沙箱回测;\n\
         两条路都要你本人逐字确认才落盘; 落盘后我可以带你跑 Dry Run(虚拟撮合) / 测试网 demo。\n\
         不知道有哪些可交易对: 输入 /market 看当前视野。"
            .to_string(),
    )
}

/// 读一个**非空**的静默输入; 空输入 / 前端无法录入 → None(并如实说明原因, 不写文件)。
fn read_secret(sink: &mut dyn SessionSink, prompt: &str) -> Option<String> {
    match sink.secret(prompt) {
        Some(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        Some(_) => {
            sink.line("输入为空, 未改动任何配置。");
            None
        }
        None => {
            sink.line("未能读取密钥输入(需要交互式终端), 未改动任何配置。");
            None
        }
    }
}

/// 读一对凭据(key + secret); 任一为空 → 整体放弃(两者必须成对, 与首次向导同口径)。
fn read_pair(
    sink: &mut dyn SessionSink,
    key_name: &str,
    secret_name: &str,
) -> Option<(String, String)> {
    let k = read_secret(sink, &format!("{key_name}(不回显; 回车放弃): "))?;
    let s = match read_secret(sink, &format!("{secret_name}(不回显; 回车放弃): ")) {
        Some(s) => s,
        None => {
            sink.line("本次未改动(两者必须成对写入)。");
            return None;
        }
    };
    Some((k, s))
}

/// 密钥状态一览(只报"是否已配置 + 尾 4 位", **不回显全文**)。
fn show_keys(file: &crate::commands::config_file::File, root: &Path, sink: &mut dyn SessionSink) {
    let env_override =
        std::env::var(config::ENV_API_KEY).ok().is_some_and(|v| !v.trim().is_empty());
    sink.line(&format!(
        "密钥状态({}; 含密钥, {}, 不入 git)\n\
         \x20 [ai].api_key              {}{}\n\
         \x20 [exchange].demo_key       {}\n\
         \x20 [exchange].demo_secret    {}\n\
         \x20 [exchange].binance_key    {}\n\
         \x20 [exchange].binance_secret {}",
        crate::commands::config_file::path(root).display(),
        crate::commands::config_file::permission_summary(root),
        key_status(file.ai.api_key.as_deref()),
        if env_override {
            format!(" — 被环境变量 {} 覆盖", config::ENV_API_KEY)
        } else {
            String::new()
        },
        key_status(file.exchange.demo_key.as_deref()),
        key_status(file.exchange.demo_secret.as_deref()),
        key_status(file.exchange.binance_key.as_deref()),
        key_status(file.exchange.binance_secret.as_deref())
    ));
    sink.line("改法: /keys ai · /keys demo · /keys live(输入不回显; 只改对应行, 注释保留)");
}

/// 密钥状态文案(纯函数): 未配置 / 已配置(尾 4 位 …); 短串整体就是密钥 → 一个字符都不显示。
fn key_status(v: Option<&str>) -> String {
    match v.map(str::trim).filter(|s| !s.is_empty()) {
        None => "未配置".to_string(),
        Some(s) if s.chars().count() <= 8 => "已配置(过短, 不回显)".to_string(),
        Some(s) => {
            let tail: String =
                s.chars().rev().take(4).collect::<Vec<char>>().into_iter().rev().collect();
            format!("已配置(尾 4 位 …{tail})")
        }
    }
}

/// 待确认动作的显示名: 只有风险确认没有策略名, 不能渲染成「标签 / 」。
fn action_display(action: &PendingAction) -> String {
    if action.name.is_empty() {
        action.kind.label().to_string()
    } else if action.replace {
        format!("{}(受控覆盖) / {}", action.kind.label(), action.name)
    } else {
        format!("{} / {}", action.kind.label(), action.name)
    }
}

/// 帮助文案(与未接入提示共用的边界说明)。
///
/// 工具名取自白名单常量(019): 不手抄, 避免白名单扩容后这里静默过期。
pub fn help_text() -> String {
    format!(
        "\
命令: /help 帮助 · /exit 退出 · /market 交易对视野(查看 / 切 bstock / all) · /keys 密钥(查看 / ai / demo / live)
可用自然语言提问, 例如: \"我部署了哪些策略\"
只读工具({} 个, 可直接调用): {read}
虚拟工具({} 个, 可直接调用但要告知副作用): {virt}
边界: R4 起七类写实动作都能在对话内完成 —— 落盘部署 / 启动测试网 demo / 首次实盘风险确认 /
启动实盘 / 停止测试网 / 停止实盘 / 平仓停止实盘, 但都必须由你本人逐字输入确认短语; 我(模型)只能登记待确认, 没有执行权。
七类之外的常见事(平台没有对应子命令, 如实指引如下, 别让用户去找不存在的命令):
改参数 = 用编辑器改 strategies/<名字>.toml 的 [strategy.params] 段, 再 `ricow restart <名字>` 生效(无热改);
改配置 = 密钥用 `/keys`、交易对视野用 `/market`(就地改写 ricow.toml, 保留注释);
删除策略 = 先 `ricow stop <名字>` 停机, 再手工删 strategies/<名字>.toml(与同名 .lua; 平台不代删)。",
        tools::READ_ONLY_TOOLS.len(),
        tools::VIRTUAL_TOOLS.len(),
        read = tools::READ_ONLY_TOOLS.join(" / "),
        virt = tools::VIRTUAL_TOOLS.join(" / ")
    )
}

/// 宿主执行已确认动作(与终端同内核, 不经 shell); 供会话与测试共用。
///
/// 七类动作全部落在同一处: 落盘 / 起停的**判据与内核**都与 CLI 共用
/// ([`crate::commands::ctrl`]), 会话只负责把结果翻成给用户看的一段话。
pub(crate) async fn execute_confirmed(action: &PendingAction, root: &Path) -> CoreResult<String> {
    match action.kind {
        ActionKind::Deploy => {
            let id = action.preview_id.as_deref().ok_or_else(|| {
                CoreError::InvalidArgument("内部状态错误: deploy 待办缺 preview_id".into())
            })?;
            let db = Database::open(&crate::commands::db_path_in(root))
                .await
                .map_err(|e| CoreError::Exchange(e.to_string()))?;
            let dir = crate::commands::ensure_strategies_dir_in(root)?;
            let token = ricow_engine::approve(&db, id).await?;
            // FR-044: replace=true 时引擎会先备份旧脚本再覆盖; 备份路径如实回报, 不省略。
            let out = ricow_engine::execute_strategy(&db, id, &token, &dir, action.replace).await?;
            let backup_note = match out.backup.as_ref() {
                Some(p) => format!(
                    "受控覆盖(FR-044): 旧脚本已备份为\n  {}\n(需要回滚就把 .bak 复制回原文件名)\n",
                    p.display()
                ),
                None => String::new(),
            };
            Ok(format!(
                "已确认并完成落盘:\n  {}\n  {}\n{}{}\
                 下一步:\n  Dry Run: ricow run {name}\n  测试网: ricow start {name} --demo",
                out.toml_path.display(),
                out.lua_path.display(),
                backup_note,
                if action.replace {
                    format!(
                        "注意: {} 若正在运行, 仍执行旧代码; 需 `ricow restart {}` 才换新脚本\n",
                        action.name, action.name
                    )
                } else {
                    String::new()
                },
                name = action.name
            ))
        }
        ActionKind::StartDemo => {
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, false, true, false).await?;
            Ok(format!(
                "已确认: {} 正在以测试网 demo 启动, pid={pid}(无真实资金; 会真实向测试网下单/撤单)。\n\
                 停机: 在对话里说\"停止测试网 {name}\", 或终端 ricow stop {name}",
                crate::commands::instances::mode_text(&mode),
                name = action.name
            ))
        }
        ActionKind::AckRisk => {
            // 018: 用户已在确认块里读过披露全文并逐字输入"确认风险", 这里只落记录。
            // 记录位置固定为 `project_root()`(子进程 run 读的是同一份, 见 commands::risk_ack_path)。
            let path = crate::commands::write_risk_ack()?;
            Ok(format!(
                "{}\n\n已确认风险: {} (一次确认长期有效, 后续实盘不再重复要求)",
                ricow_engine::RISK_DISCLOSURE,
                path.display()
            ))
        }
        ActionKind::StartLive => {
            // 权威判定 = 宿主的共享预检(018 → 002 时长门禁 → FR-008 时钟), 与 CLI 完全同一条路径。
            // 工具侧只做了本地确定性检查, 不联网; 这里才是真正放行/拒绝的地方。
            //
            // `accept_risk = false` 是**刻意**的: 宿主绝不在这里替用户补写风险确认记录。
            // 披露全文只在「确认风险」动作里展示, 登记这个 pending 时也已经要求 ack 存在
            // (见 `tools::prepare_start_live`)。若登记后 ack 记录消失(换 root / 手工清理),
            // 宁可直接拒绝并让用户重走确认, 也不静默补记后放行 —— 实盘门禁只许 fail-closed。
            let notice = crate::commands::ctrl::live_preflight(root, &action.name, false)
                .await
                .map_err(|e| {
                    CoreError::InvalidArgument(format!(
                        "{e}\n(执行前复核发现实盘门禁未通过, 未发生任何下单; \
                             若缺「首次风险确认」, 请在对话里先走一遍确认风险。)"
                    ))
                })?;
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, true, false, true).await?;
            Ok(format!(
                "{}已确认: {} 正在以**实盘**启动, pid={pid}(真实资金, 会真实下单)。\n\
                 停机: 在对话里说\"停止实盘 {name}\"(保留持仓) 或\"平仓停止 {name}\"(撤单并市价平仓)。",
                notice.map(|n| format!("{n}\n")).unwrap_or_default(),
                crate::commands::instances::mode_text(&mode),
                name = action.name
            ))
        }
        ActionKind::StopDemo | ActionKind::StopLive | ActionKind::CloseLive => {
            let close_all = action.kind == ActionKind::CloseLive;
            let report = crate::commands::ctrl::stop_daemon(root, &action.name, close_all).await?;
            let tail = match action.kind {
                ActionKind::StopDemo => "(测试网实例已停止; 与真实资金无关)",
                ActionKind::StopLive => {
                    "(实盘实例已停止; 持仓仍保留在交易所, 如需离场请说\"平仓停止\")"
                }
                _ => "(已请求撤单并市价平仓; 平仓结果以交易所回报为准, 见日志)",
            };
            Ok(format!("{report}{tail}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_input() {
        assert_eq!(classify("   "), LineInput::Empty);
        assert_eq!(classify("/exit"), LineInput::Exit);
        assert_eq!(classify("/q"), LineInput::Exit);
        assert_eq!(classify("/help"), LineInput::Help);
        assert_eq!(classify("/status"), LineInput::Unknown("status".into()));
        assert_eq!(classify("回测 ETH 30 天"), LineInput::Ask("回测 ETH 30 天".into()));
        // 斜杠命令带参数时只取命令名
        assert_eq!(classify("/stop shannon_grid"), LineInput::Unknown("stop".into()));
    }

    #[test]
    fn test_classify_market_variants() {
        assert_eq!(classify("/market"), LineInput::Market(MarketCmd::Show));
        assert_eq!(classify("  /market   "), LineInput::Market(MarketCmd::Show));
        // 别名与大小写都归一到同一意图
        assert_eq!(classify("/market bstock"), LineInput::Market(MarketCmd::Bstock));
        assert_eq!(classify("/market stock"), LineInput::Market(MarketCmd::Bstock));
        assert_eq!(classify("/market DEFAULT"), LineInput::Market(MarketCmd::Bstock));
        assert_eq!(classify("/market all"), LineInput::Market(MarketCmd::All));
        assert_eq!(classify("/market ALL extra"), LineInput::Market(MarketCmd::All));
        // 不认的参数原样带出(交给会话回报, 不静默当成默认)
        assert_eq!(classify("/market crypto"), LineInput::Market(MarketCmd::Bad("crypto".into())));
    }

    #[test]
    fn test_classify_keys_variants() {
        assert_eq!(classify("/keys"), LineInput::Keys(KeysCmd::Show));
        assert_eq!(classify("  /keys   "), LineInput::Keys(KeysCmd::Show));
        assert_eq!(classify("/keys ai"), LineInput::Keys(KeysCmd::Ai));
        // 大小写归一
        assert_eq!(classify("/keys DEMO"), LineInput::Keys(KeysCmd::Demo));
        assert_eq!(classify("/keys live"), LineInput::Keys(KeysCmd::Live));
        // 不认的参数原样带出(不静默当成"查看")
        assert_eq!(classify("/keys binance"), LineInput::Keys(KeysCmd::Bad("binance".into())));
    }

    #[test]
    fn test_key_status_never_echoes_full_key() {
        assert_eq!(key_status(None), "未配置");
        assert_eq!(key_status(Some("   ")), "未配置");
        assert_eq!(key_status(Some("sk-abcdefghijklmn")), "已配置(尾 4 位 …klmn)");
        assert_eq!(key_status(Some("  sk-abcdefghijklmn  ")), "已配置(尾 4 位 …klmn)");
        // 短串整体就是密钥 → 一个字符都不显示
        let short = key_status(Some("short"));
        assert_eq!(short, "已配置(过短, 不回显)");
        assert!(!short.contains("short"), "短密钥不得回显: {short}");
    }

    /// 假 sink: 按脚本回答 `secret`, 收集所有输出行(用于测 `/keys` 的输入语义)。
    struct FakeSink {
        answers: std::collections::VecDeque<Option<String>>,
        lines: Vec<String>,
    }

    impl SessionSink for FakeSink {
        fn text(&mut self, chunk: &str) {
            self.lines.push(chunk.to_string());
        }
        fn line(&mut self, text: &str) {
            self.lines.push(text.to_string());
        }
        fn secret(&mut self, _prompt: &str) -> Option<String> {
            self.answers.pop_front().flatten()
        }
    }

    fn fake(answers: &[Option<&str>]) -> FakeSink {
        FakeSink {
            answers: answers.iter().map(|a| a.map(|s| s.to_string())).collect(),
            lines: Vec::new(),
        }
    }

    #[test]
    fn test_read_secret_trims_and_rejects_blank_or_unavailable() {
        let mut s = fake(&[Some("  k  ")]);
        assert_eq!(read_secret(&mut s, "p: "), Some("k".into()));
        // 空输入 → 放弃(不是"写入空串")
        let mut s = fake(&[Some("   ")]);
        assert_eq!(read_secret(&mut s, "p: "), None);
        assert!(s.lines.iter().any(|l| l.contains("未改动")), "{:?}", s.lines);
        // 前端无法录入 → 放弃
        let mut s = fake(&[None]);
        assert_eq!(read_secret(&mut s, "p: "), None);
    }

    #[test]
    fn test_read_pair_requires_both_and_never_writes_half() {
        // 两者都非空 → 通过并按对返回
        let mut s = fake(&[Some(" DK "), Some("DS")]);
        assert_eq!(read_pair(&mut s, "demo_key", "demo_secret"), Some(("DK".into(), "DS".into())));
        // key 有 / secret 空 → 整体放弃(不成对写入)
        let mut s = fake(&[Some("DK"), Some("   ")]);
        assert_eq!(read_pair(&mut s, "demo_key", "demo_secret"), None);
        // key 空 → 直接放弃, 不再追问 secret
        let mut s = fake(&[Some(""), Some("DS")]);
        assert_eq!(read_pair(&mut s, "demo_key", "demo_secret"), None);
        assert_eq!(s.answers.len(), 1, "key 为空时不应继续消费输入");
    }

    #[test]
    fn test_help_text_mentions_limits() {
        let h = help_text();
        assert!(h.contains("/exit") && h.contains("确认"), "{h}");
        assert!(h.contains("/market"), "帮助要列出 /market: {h}");
        assert!(h.contains("/keys"), "帮助要列出 /keys: {h}");
    }

    fn temp_root(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("ricow-session-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn test_empty_state_hint_only_when_no_strategy() {
        let root = temp_root("empty");
        let hint = empty_state_hint(&root).expect("空目录须给两条路话术");
        for must in ["模板", "全新编写", "确认", "/market"] {
            assert!(hint.contains(must), "空状态话术缺少 {must}: {hint}");
        }
        // 有任意一条策略 toml 就不再是空状态
        let dir = root.join("strategies");
        std::fs::create_dir_all(&dir).expect("建策略目录");
        std::fs::write(dir.join("g1.toml"), "enabled = true").expect("写策略占位");
        assert!(empty_state_hint(&root).is_none(), "已有策略不该再提示两条路");
        let _ = std::fs::remove_dir_all(&root);
    }
}
