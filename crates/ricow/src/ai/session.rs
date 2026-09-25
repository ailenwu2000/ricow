//! 会话缝 (019-R4) —— 对话业务逻辑与终端 I/O 的**唯一**分隔线。
//!
//! `ChatSession` 持有 root/配置/LLM/pending/history, 对外只有 [`SessionSink`] 一个出口:
//! 本模块**不碰 stdin/stdout**, 也不打印任何东西。`commands::ai`(薄壳)与
//! `commands::chat`(裸入口)只负责"读一行 → 交给 session → 把 sink 收到的东西写出去"。
//!
//! **网页端已落地(025)**: `web/sink.rs` 的 `WebSink` 就是这条缝上的第二个 sink(SSE 帧 + 浏览器
//! 输入区), 会话线程里跑的仍是本模块 —— 业务逻辑零复制。
//! 安全口径不变: 写实动作**没有工具调用面**; 模型只能登记 pending, 执行权在宿主
//! (见 [`super::confirm`])。

use std::path::{Path, PathBuf};

use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;
use rig::message::Message;

use super::confirm::{self, ActionKind, LineDisposition, PendingAction, PendingSlot};
use super::{config, menu, prompt, provider, tools};
use crate::i18n::{t, Lang};

/// 带占位符的文案: [`t`] 只选字面量, 这里选两条已 `format!` 好的串(与 `ai::menu` 同口径)。
fn tf(lang: Lang, zh: impl Into<String>, en: impl Into<String>) -> String {
    match lang {
        Lang::Zh => zh.into(),
        Lang::En => en.into(),
    }
}

/// `/history` 单条往返的字符上限(超出即截断并标注, FR-008)。
const HISTORY_ENTRY_MAX_CHARS: usize = 2_000;

/// 宿主输出级别(025 / FR-013): **由宿主显式标注**, 前端只按级别着色, 不做关键字猜测。
///
/// 终端 sink 忽略级别(输出与 019 逐字一致); 网页端按级别给不同颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// 普通正文(对话内容 / 列表 / 空行)。
    Normal,
    /// 提示(用量、菜单、可操作建议)。
    Notice,
    /// 警告(需留意的边界或可能造成损失的动作)。
    Warn,
    /// 错误(失败、被拒绝、配置问题)。
    Error,
}

impl Severity {
    /// 落库与 SSE 帧共用的级别标识(唯一编码来源; 终端 sink 不用)。
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Normal => "normal",
            Severity::Notice => "notice",
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

/// 会话 I/O 出口(终端 = stdio; 网页端 = SSE 帧 + 浏览器输入区)。
///
/// 前端至少要提供四件事: 流式文本增量、**带级别**的一整行输出、**读一行**用户输入、
/// **不回显**的密钥录入。新增前端不必理解会话内部状态, 但必须能安全地拿到用户粘贴的密钥
/// (不回显、不进日志、不进模型上下文)。
pub trait SessionSink {
    /// 助手文本增量(不保证以换行结尾, 终端实现要 flush)。
    fn text(&mut self, chunk: &str);
    /// 一整行宿主输出 + 级别(FR-013)。文案由宿主给出, 实现不得改写。
    fn line_sev(&mut self, text: &str, sev: Severity);
    /// 一整行**普通**输出(正文 / 列表 / 空行)。
    fn line(&mut self, text: &str) {
        self.line_sev(text, Severity::Normal);
    }
    /// 提示级一行(用量 / 菜单 / 可操作建议)。
    fn notice(&mut self, text: &str) {
        self.line_sev(text, Severity::Notice);
    }
    /// 警告级一行(需留意的边界 / 已作废的动作)。
    fn warn(&mut self, text: &str) {
        self.line_sev(text, Severity::Warn);
    }
    /// 错误级一行(失败 / 配置问题)。
    fn error(&mut self, text: &str) {
        self.line_sev(text, Severity::Error);
    }
    /// 读一行用户输入(REPL 主循环的输入侧)。
    ///
    /// `prompt` 只对**能显示提示符**的前端有意义(终端); 网页端可忽略(输入框自带提示),
    /// 但两侧都必须返回**去掉行尾换行**的一行。
    ///
    /// 返回 `None` = 输入通道已关闭(`Ctrl-D` / 页面关闭) → REPL 据此退出, 与终端 EOF 同义。
    /// 允许阻塞等待(与 [`SessionSink::secret`] 同口径)。
    fn input_line(&mut self, prompt: &str) -> Option<String>;
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
    /// 前端是否已具备可用的**交互式输入通道**(025 / D6): 终端 = `stdin` 是 tty;
    /// 网页端 = 浏览器输入区 + 密钥表单, 不受本机 tty 影响。
    ///
    /// 与 [`Options::interactive`] 取与: 真正的门禁 = 用户主动要求交互 **且** 前端接得住第二轮输入。
    pub has_input_channel: bool,
}

/// 一次会话的全部可变状态。
pub struct ChatSession {
    root: PathBuf,
    resolved: config::Resolved,
    /// 是否可对话内确认 / 静默录入密钥(前端声明有输入通道且为 REPL); 重建客户端时要复用。
    interactive: bool,
    llm: provider::Llm,
    pending: PendingSlot,
    /// 宿主菜单槽(与工具闭包共享): 编号与文案由宿主单一来源产生, 模型只能经 `show_menu` 请求。
    active_menu: menu::MenuSlot,
    history: Vec<Message>,
    plain: bool,
    /// 界面语言(023 F0): 固定文案取词 / 确认词判定 / AI 回复语言的唯一依据。
    lang: Lang,
    /// 已完成的问答轮次(FR-009: 斜杠命令与空行不计入)。
    turn: u64,
    /// 本会话问答往返(仅 [`ChatSession::reply`] 写入), 供 `/history` 回看(FR-008)。
    transcript: Vec<(String, String)>,
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

        let api_key =
            provider::resolve_key(&resolved.base_url, config::api_key(&root, &resolved.provider))?;

        // 界面语言: 首次向导写入的 `[ui].lang`; 未选择时 [`crate::i18n::resolve`] 兜底中文 (FR-001)。
        let lang = crate::i18n::resolve(&file);

        // 工具集: L0 只读 + L1 虚拟。写实动作没有工具面 —— 对话内确认也只登记 pending,
        // 由本会话在用户逐字输入后执行(见 `execute_confirmed`)。
        let pending = confirm::new_slot();
        let active_menu = menu::new_slot();
        let interactive = opts.interactive && opts.has_input_channel;
        let tool_ctx = tools::ToolCtx::new(
            root.clone(),
            interactive,
            pending.clone(),
            active_menu.clone(),
            lang,
        );
        let llm = provider::connect(
            resolved.clone(),
            &api_key,
            &prompt::system_preamble(lang),
            tools::build(tool_ctx),
        )?;

        Ok(Self {
            root,
            resolved,
            interactive,
            llm,
            pending,
            active_menu,
            history: Vec::new(),
            plain: opts.plain,
            lang,
            turn: 0,
            transcript: Vec::new(),
        })
    }

    /// 当前界面语言(薄壳渲染固定文案时要用, 例如 REPL 的帮助横幅)。
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// 恢复旧会话的 AI 上下文(025 / D10 / FR-020): 装入最近若干轮往返, 单条按 [`clip_for_history`]
    /// 同口径截断(不撑爆 token)。
    ///
    /// 除 `history` 外还要接续两处, 否则恢复后的观感与命令自相矛盾:
    /// - `turn` = **页面流水里出现过的最大轮次号**(由调用方从分隔线解析, 见
    ///   [`parse_turn_divider`]); 不能用 `rounds.len()`: 那里丢掉了失败轮 / 斜杠命令
    ///   这类没有 assistant 行的 user 消息, 会比页面刚显示的号小, 新一轮就会重号或倒退;
    /// - `transcript` 补上同一批往返, 否则 `/history` 会声称"本会话还没有问答记录"。
    pub fn resume_history(&mut self, rounds: &[(String, String)], turn: u64) {
        for (q, a) in rounds {
            self.history.push(Message::user(clip_for_history(q, self.lang)));
            self.history.push(Message::assistant(clip_for_history(a, self.lang)));
            self.transcript.push((q.clone(), a.clone()));
        }
        self.turn = turn;
    }

    /// 会话开始信息(FR-011: 只有"用哪个模型 / 密钥从哪来"一行 + 一句边界); 不打印, 只走 sink。
    pub fn welcome(&self, sink: &mut dyn SessionSink) {
        let key_from_env =
            std::env::var(config::ENV_API_KEY).ok().is_some_and(|v| !v.trim().is_empty());
        sink.line(&tf(
            self.lang,
            format!(
                "ricow AI 助手 (供应商: {} / 模型: {}) · 密钥: {}([ai].api_key; 环境变量可覆盖)",
                self.resolved.provider,
                self.resolved.model,
                if key_from_env { "来自环境变量" } else { "来自 ricow.toml" }
            ),
            format!(
                "ricow AI assistant (provider: {} / model: {}) · API key: {}([ai].api_key; env var overrides)",
                self.resolved.provider,
                self.resolved.model,
                if key_from_env { "from environment" } else { "from ricow.toml" }
            ),
        ));
        // 一句边界: 写操作不在模型工具面内 —— 与 help / 确认块同一口径 (FR-010)。
        sink.notice(t(
            self.lang,
            "边界: 我能查资料 / 写策略 / 跑回测; 凡是会改文件或起停实例的动作, 都必须你本人回一句确认词。",
            "Boundary: I can look things up, write strategies and run backtests; anything that changes files or starts/stops an instance needs your own confirmation.",
        ));
        if let Some(hint) = empty_state_hint(&self.root, self.lang) {
            sink.line("");
            sink.notice(&hint);
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
                sink.line(t(self.lang, "再见。", "Bye."));
                Ok(Step::Exit)
            }
            LineInput::Help => {
                sink.line(&help_text(self.lang));
                Ok(Step::Continue)
            }
            LineInput::History => {
                self.handle_history(sink);
                Ok(Step::Continue)
            }
            LineInput::Lang(cmd) => {
                self.handle_lang(cmd, sink)?;
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
                sink.warn(&tf(
                    self.lang,
                    format!(
                        "斜杠命令 /{name} 还没有接入; 现在能用: /help /exit /history /lang /market /keys"
                    ),
                    format!(
                        "The /{name} command is not available; you can use: /help /exit /history /lang /market /keys"
                    ),
                ));
                Ok(Step::Continue)
            }
            LineInput::Ask(q) => {
                // 顺序 (FR-016): 对话内确认状态机优先 → 菜单序号 → 普通提问。
                // 短语只认真实用户输入行, 不经过模型 —— 模型输出永远无法走到执行分支。
                match confirm::consume_line(&self.pending, line, self.lang).await {
                    LineDisposition::Confirm(action) => match self.execute(&action).await {
                        Ok(msg) => {
                            sink.notice(&msg);
                            // 026 T020 / FR-021~FR-023: 执行结果回流 AI 上下文 —— 否则模型看不到
                            // 自己刚确认的动作实际做了什么, 下一轮只能凭空猜(甚至重做)。
                            // 与 sink **同源**: 就是上面显示的那份 `msg`, 只按 history 上限裁剪。
                            self.history
                                .push(Message::assistant(clip_for_history(&msg, self.lang)));
                            self.flush_menu(sink).await;
                        }
                        Err(e) => {
                            // 失败同样注入 (FR-024): 模型必须知道动作失败了, 而不是以为成功。
                            // 先拼好一份, 显示与入 history 用同一份文本(同源, FR-022), 显示文案不变。
                            let shown = tf(
                                self.lang,
                                format!(
                                    "执行失败: {e}\n(这次确认已用掉; 若预览已被批准或用过, 先看一下实际状态, 必要时重新来一次)"
                                ),
                                format!(
                                    "Failed: {e}\n(That confirmation is now spent; if the preview was already approved or used, check the current state and start over if needed.)"
                                ),
                            );
                            sink.error(&shown);
                            self.history
                                .push(Message::assistant(clip_for_history(&shown, self.lang)));
                        }
                    },
                    LineDisposition::Reject(action) => {
                        // 落盘类: 尽力把 preview 置 rejected 终态(失败也不影响本地作废语义)
                        if let (Some(id), true) = (
                            action.preview_id.as_deref(),
                            matches!(action.kind, ActionKind::Deploy | ActionKind::DeployReplace),
                        ) {
                            if let Ok(db) =
                                Database::open(&crate::commands::db_path_in(&self.root)).await
                            {
                                _ = ricow_engine::reject(&db, id).await;
                            }
                        }
                        let shown = action_display(&action, self.lang);
                        sink.warn(&tf(
                            self.lang,
                            format!("已放弃待确认动作「{shown}」, 未执行任何写实操作。"),
                            format!(
                                "Dropped the pending action \"{shown}\" — nothing was written or started."
                            ),
                        ));
                    }
                    LineDisposition::Expired(action) => {
                        // 分钟数取自 `ai::confirm::PENDING_TTL`(与引擎 preview TTL 同源), 不手抄 15。
                        let minutes = crate::ai::confirm::PENDING_TTL.as_secs() / 60;
                        let shown = action_display(&action, self.lang);
                        sink.warn(&tf(
                            self.lang,
                            format!(
                                "待确认动作「{shown}」已超过 {minutes} 分钟, 已作废; 如需继续请重新发起。"
                            ),
                            format!(
                                "The pending action \"{shown}\" expired after {minutes} minutes and was discarded; please start again if you still want it."
                            ),
                        ));
                        self.reply(&q, sink).await;
                    }
                    LineDisposition::Other => {
                        // 有待确认动作在, 而这行"像确认词但不全等"(实测「确认一下」): 匹配是整行
                        // 全等, 不会放行且本来毫无反馈 —— 先补一行提示, 把静默失败变成看得见。
                        // 匹配逻辑不变, 这行照旧按菜单序号 / 普通提问处理(FR-016)。
                        if confirm::looks_like_near_miss_confirmation(line, self.lang) {
                            sink.warn(&near_miss_confirmation_hint(self.lang));
                        }
                        match menu::parse_menu_choice(line) {
                            Some(n) => self.handle_menu_choice(n, &q, sink).await,
                            None => self.reply(&q, sink).await,
                        }
                    }
                    LineDisposition::NoPending => {
                        // 没有待确认动作 → 无非确认词可言; 菜单序号只认"有菜单且在范围内"的
                        // 纯数字行, 其余一律当普通提问 (FR-016)。
                        match menu::parse_menu_choice(line) {
                            Some(n) => self.handle_menu_choice(n, &q, sink).await,
                            None => self.reply(&q, sink).await,
                        }
                    }
                }
                Ok(Step::Continue)
            }
        }
    }

    /// 菜单序号(FR-016): 命中 → 把该选项的 `request` 当作用户提问送出, 菜单随之结束;
    /// 越界 → 提示可选范围并**保留菜单**; 没有菜单 → 该行按普通提问处理。
    async fn handle_menu_choice(&mut self, n: usize, raw: &str, sink: &mut dyn SessionSink) {
        let picked = {
            let guard = self.active_menu.lock().await;
            match guard.as_ref() {
                None => None,
                Some(m) if n >= 1 && n <= m.options.len() => {
                    Some((m.options[n - 1].request.clone(), m.options.len()))
                }
                Some(m) => Some((String::new(), m.options.len())),
            }
        };
        let Some((request, len)) = picked else {
            // 没有菜单在等选择: 纯数字行也可能是正常提问, 不吞掉它。
            self.reply(raw, sink).await;
            return;
        };
        if request.is_empty() {
            sink.warn(&tf(
                self.lang,
                format!("没有第 {n} 项; 请选 1..{len}(或直接说你想做什么)。"),
                format!("There is no option {n}; choose 1..{len} (or just say what you want)."),
            ));
            self.flush_menu(sink).await;
            return;
        }
        // 选中即消费菜单(FR-015): 选项只是"替你说一句话", 写操作仍要过 F4 确认。
        *self.active_menu.lock().await = None;
        self.reply(&request, sink).await;
    }

    /// `/history`(别名 `/log`, FR-008): 渲染本会话全部往返; 不计轮次、不写 transcript。
    fn handle_history(&self, sink: &mut dyn SessionSink) {
        if self.transcript.is_empty() {
            sink.notice(t(
                self.lang,
                "本会话还没有问答记录(斜杠命令和空行不算)。",
                "No exchanges in this session yet (slash commands and blank lines do not count).",
            ));
            return;
        }
        let (you, ai) = (t(self.lang, "你", "You"), t(self.lang, "助手", "AI"));
        for (i, (q, a)) in self.transcript.iter().enumerate() {
            sink.line("");
            sink.line(&turn_divider(self.lang, i as u64 + 1));
            sink.line(&format!("{you}: {}", clip_for_history(q, self.lang)));
            sink.line(&format!("{ai}: {}", clip_for_history(a, self.lang)));
        }
    }

    /// `/lang`(FR-005): 无参 → 当前语言 + 双语选项; 有参 → 写回 `[ui].lang` 并立即生效。
    fn handle_lang(&mut self, cmd: LangCmd, sink: &mut dyn SessionSink) -> CoreResult<()> {
        use crate::commands::config_file::SetValue;

        match cmd {
            LangCmd::Show => {
                sink.line(&tf(
                    self.lang,
                    "当前语言: 中文 (zh)\n切换: /lang zh(中文) · /lang en(English)".to_string(),
                    "Current language: English (en)\nSwitch: /lang zh (中文) · /lang en (English)"
                        .to_string(),
                ));
            }
            LangCmd::Bad(arg) => {
                sink.warn(&tf(
                    self.lang,
                    format!(
                        "/lang 参数只支持 zh / en; 收到: {arg}\n\
                         用法: /lang(查看当前) · /lang zh(中文) · /lang en(English)"
                    ),
                    format!(
                        "/lang only accepts zh / en; got: {arg}\n\
                         Usage: /lang (show current) · /lang zh (中文) · /lang en (English)"
                    ),
                ));
            }
            LangCmd::Set(lang) => {
                crate::commands::config_file::set_values(
                    &self.root,
                    &[("ui", "lang", SetValue::Str(lang.code().to_string()))],
                )?;
                self.lang = lang;
                // 回执用**新**语言(FR-005); 提示词里的语言纪律也随之重建 (FR-006)。
                self.rebuild_llm()?;
                sink.notice(&tf(
                    lang,
                    format!(
                        "已切换界面语言: 中文 (zh)\n  {} 已更新, 立即生效。",
                        crate::commands::config_file::path(&self.root).display()
                    ),
                    format!(
                        "Language switched to English (en)\n  {} updated; effective immediately.",
                        crate::commands::config_file::path(&self.root).display()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 渲染当前菜单(若有)并保留 —— 菜单生命周期只由"被选中"或"被新菜单替换"结束(FR-015)。
    async fn flush_menu(&self, sink: &mut dyn SessionSink) -> bool {
        let guard = self.active_menu.lock().await;
        match guard.as_ref() {
            Some(m) => {
                sink.line("");
                sink.notice(&menu::render(m));
                true
            }
            None => false,
        }
    }

    /// 用当前 root / lang 重建 LLM 客户端(改密钥或改语言后立即生效, 不必退出重开)。
    fn rebuild_llm(&mut self) -> CoreResult<()> {
        let resolved = self.resolved.clone();
        let key = provider::resolve_key(
            &resolved.base_url,
            config::api_key(&self.root, &resolved.provider),
        )?;
        let ctx = tools::ToolCtx::new(
            self.root.clone(),
            self.interactive,
            self.pending.clone(),
            self.active_menu.clone(),
            self.lang,
        );
        self.llm = provider::connect(
            resolved,
            &key,
            &prompt::system_preamble(self.lang),
            tools::build(ctx),
        )?;
        Ok(())
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
                sink.warn(&format!(
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
            sink.notice(&format!(
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
        sink.notice("切换: /market bstock(仅股票类) · /market all(全部交易对)");
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
                sink.warn(&format!(
                    "/keys 参数只支持 ai / demo / live; 收到: {arg}\n\
                     用法: /keys(查看状态) · /keys ai(AI 助手密钥) · /keys demo(测试网凭据) · \
                     /keys live(主网/实盘凭据)"
                ));
                return Ok(());
            }
        };

        // 密钥录入只在交互式会话里开放(与对话内确认同一 tty 门禁): 拿不到静默输入就不写文件。
        if !self.interactive {
            sink.warn(
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
                    sink.warn(&format!(
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
                sink.notice(
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
                sink.warn("币安主网(实盘)凭据: 建议用只开交易、关闭提现的受限 Key 或子账户。");
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
        sink.notice(&format!(
            "已更新 {}: {}",
            config_file::path(&self.root).display(),
            updates.iter().map(|(s, k, _)| format!("[{s}].{k}")).collect::<Vec<_>>().join(", ")
        ));

        // 环境变量覆盖时不重建(重建也只会用环境变量里的值, 报"已生效"就是撒谎)。
        if target == KeysTarget::Ai && !env_override {
            self.rebuild_llm()?;
            sink.notice("已用新密钥重建 LLM 客户端, 下一句话即生效。");
        }
        if let Some(w) = config_file::permission_warning(&self.root) {
            sink.warn(&format!("提示: {w}"));
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
    ///
    /// 轮次可读性(FR-007): 每轮先空行 + 分隔线, 回答后再补一个空行 —— 用户一眼看得出
    /// "这一轮从哪开始、到哪结束"。斜杠命令与空行不经过这里, 自然不计轮次(FR-009)。
    /// 往返同时记进 `transcript` 供 `/history` 回看; 失败也记(否则 `/history` 的轮次号
    /// 会与实际看到的轮次号错位)。
    async fn reply(&mut self, q: &str, sink: &mut dyn SessionSink) {
        self.turn += 1;
        sink.line("");
        sink.line(&turn_divider(self.lang, self.turn));

        let reply = if self.plain {
            self.llm.ask(q).await.inspect(|a| sink.line(&a.text))
        } else {
            self.llm.ask_stream(q, &self.history, sink).await
        };
        match reply {
            Ok(ans) => {
                if let Some(u) = &ans.usage {
                    sink.notice(&format!("[用量] {u}"));
                }
                self.transcript.push((q.to_string(), ans.text.clone()));
                self.history.push(Message::user(q.to_string()));
                self.history.push(Message::assistant(ans.text));
            }
            // 如实报错, 不吞: 网络/鉴权/模型不支持工具调用都会走到这里
            Err(e) => {
                let shown = format!("错误: {e}");
                sink.error(&shown);
                self.transcript.push((q.to_string(), shown));
            }
        }
        sink.line("");
    }

    /// 宿主执行已确认动作(019 R3): 复用与终端完全相同的引擎内核, 不经 shell。
    ///
    /// - Deploy = `engine::approve` 取一次性 token → `engine::execute_strategy` 落盘(与 deploy.rs 同函数);
    /// - StartDemo = `ctrl::start_daemon(demo=true)`(daemon 对 demo 不校验 confirmed/live_enabled)。
    ///
    /// 成功后就地更新菜单(FR-015 / FR-026): 落盘 → 菜单 A「策略就绪」; 删除 → 清空菜单。
    /// 其余动作保持当前菜单(调用方 [`Self::flush_menu`] 会再渲一份), 不凭空造菜单。
    pub async fn execute(&mut self, action: &PendingAction) -> CoreResult<String> {
        let msg = execute_confirmed(action, &self.root, self.lang).await?;
        let next = match action.kind {
            ActionKind::Deploy | ActionKind::DeployReplace => {
                menu::build(menu::KIND_STRATEGY_READY, &action.name, self.lang)
            }
            ActionKind::DeleteStrategy => None,
            _ => return Ok(msg),
        };
        *self.active_menu.lock().await = next;
        Ok(msg)
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
    /// `/history`(别名 `/log`)—— 回看本会话问答往返。
    History,
    /// `/lang` 界面语言。
    Lang(LangCmd),
    /// `/market` 交易对视野。
    Market(MarketCmd),
    /// `/keys` 密钥管理。
    Keys(KeysCmd),
    /// 提问。
    Ask(String),
    /// 尚未接入的斜杠命令。
    Unknown(String),
}

/// `/lang` 的意图(纯解析, 不做 I/O)。
#[derive(Debug, PartialEq, Eq)]
pub enum LangCmd {
    /// `/lang` — 只看当前语言(附双语切换提示)。
    Show,
    /// `/lang zh|en` — 切换界面语言(写回 `[ui].lang` 并立即生效)。
    Set(Lang),
    /// `/lang <其它>` — 参数不认(原样带出, 便于回报)。
    Bad(String),
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
            "history" | "log" => LineInput::History,
            "lang" => LineInput::Lang(match words.next() {
                None => LangCmd::Show,
                Some(w) => match Lang::parse(w) {
                    Some(l) => LangCmd::Set(l),
                    None => LangCmd::Bad(w.to_string()),
                },
            }),
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
/// 文案只讲"会发生什么"(FR-010): 不出现任何终端命令。
pub fn empty_state_hint(root: &Path, lang: Lang) -> Option<String> {
    if !tools::list_toml_stems(&root.join("strategies")).is_empty() {
        return None;
    }
    Some(tf(
        lang,
        "现在还没有任何策略 —— 两条路都行:\n\
         ① 从模板起步: 说\"看看模板\", 我列出内置模板(如 shannon_spot_grid 香农现货网格), 你挑一个, 我再问交易对与参数;\n\
         ② 全新编写: 直接说需求(例: \"给 AAPL 做 50:50 再平衡, 每次 0.01\"), 我按 Lua API 写代码并先跑沙箱回测;\n\
         两条路都要你本人回一句确认词才落盘; 落盘后我可以带你跑 Dry Run(虚拟撮合) / 测试网 demo。\n\
         不知道有哪些可交易对: 输入 /market 看当前视野。"
            .to_string(),
        "There are no strategies yet — two ways to start:\n\
         1) Start from a template: say \"show me the templates\", I'll list the built-in ones (e.g. shannon_spot_grid, Shannon spot grid), you pick one, then I'll ask for the pair and parameters.\n\
         2) Write one from scratch: just describe what you want (e.g. \"50:50 rebalance on AAPL, 0.01 each time\"), I'll write it against the Lua API and run a sandbox backtest first.\n\
         Either way nothing is written until you reply with a confirmation word; after that I can walk you through a dry run (simulated fills) or the testnet demo.\n\
         Not sure which pairs you can trade? Type /market to see the current scope."
            .to_string(),
    ))
}

/// 轮次分隔线(FR-007): 让用户一眼看出"这一轮从哪开始"。
fn turn_divider(lang: Lang, n: u64) -> String {
    tf(lang, format!("── 第 {n} 轮 ──"), format!("── Turn {n} ──"))
}

/// [`turn_divider`] 的逆: 从一行文本里认回轮次号, **中英两种写法都认**(会话切过语言时
/// 恢复仍要认得出旧分隔线)。不是分隔线就返回 `None`。
///
/// 恢复会话时用它扫页面流水取最大轮次号(见 [`ChatSession::resume_history`]) —— 与页面
/// 回放读的是同一份数据, 接续的号因此只会更大, 不会倒退。
pub(crate) fn parse_turn_divider(line: &str) -> Option<u64> {
    let inner = line.trim().strip_prefix("──")?.strip_suffix("──")?.trim();
    let digits = inner
        .strip_prefix('第')
        .and_then(|rest| rest.strip_suffix('轮'))
        .or_else(|| inner.strip_prefix("Turn"))
        .or_else(|| inner.strip_prefix("turn"))?
        .trim();
    digits.parse().ok()
}

/// 近似误输确认词时的兜底提示(判定见 [`confirm::looks_like_near_miss_confirmation`])。
///
/// 只讲两件事: **为什么没生效**(整行只写一个确认词才算), 以及**下一步怎么办**(这行已按普通
/// 提问处理, 待确认动作还在)。词表取自 [`confirm::confirmation_words`], 不手抄。
fn near_miss_confirmation_hint(lang: Lang) -> String {
    let words = confirm::confirmation_words(lang)
        .iter()
        .map(|w| tf(lang, format!("「{w}」"), format!("\"{w}\"")))
        .collect::<Vec<_>>()
        .join(" / ");
    tf(
        lang,
        format!(
            "提示: 这行不算确认词 —— 确认要整行只写一个确认词({words}), 不能带标点或别的字。\n\
             这条已按普通提问处理, 待确认动作仍然有效; 想执行就单独回一行。"
        ),
        format!(
            "Note: that line is not a confirmation word — to go ahead, send a line that contains only \
             one of these, with no punctuation or extra words: {words}.\n\
             This line was treated as a normal question; the pending action is still open."
        ),
    )
}

/// `/history` 单条往返的渲染(FR-008): 超上限时**截断并标注**, 不静默丢内容。
fn clip_for_history(s: &str, lang: Lang) -> String {
    let total = s.chars().count();
    if total <= HISTORY_ENTRY_MAX_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(HISTORY_ENTRY_MAX_CHARS).collect();
    tf(
        lang,
        format!("{head}…(已截断, 原文 {total} 字)"),
        format!("{head}…(truncated; original {total} characters)"),
    )
}

/// 读一个**非空**的静默输入; 空输入 / 前端无法录入 → None(并如实说明原因, 不写文件)。
fn read_secret(sink: &mut dyn SessionSink, prompt: &str) -> Option<String> {
    match sink.secret(prompt) {
        Some(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        Some(_) => {
            sink.warn("输入为空, 未改动任何配置。");
            None
        }
        None => {
            sink.warn("未能读取密钥输入(需要交互式终端), 未改动任何配置。");
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
            sink.warn("本次未改动(两者必须成对写入)。");
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
fn action_display(action: &PendingAction, lang: Lang) -> String {
    let label = action.kind.label(lang);
    if action.name.is_empty() {
        label.to_string()
    } else if action.is_replace() {
        format!("{label}{} / {}", tf(lang, "(受控覆盖)", "(controlled overwrite)"), action.name)
    } else {
        format!("{label} / {}", action.name)
    }
}

/// 帮助文案(与未接入提示共用的边界说明), 随界面语言。
///
/// 面向用户只讲"会发生什么"(FR-010): 不给任何终端命令, 也不让用户去找不存在的子命令。
/// 工具名取自白名单常量(019): 不手抄, 避免白名单扩容后这里静默过期。
pub fn help_text(lang: Lang) -> String {
    match lang {
        Lang::Zh => format!(
            "\
命令: /help 帮助 · /exit 退出 · /history 回看本会话(别名 /log) · /lang 切换语言 · \
/market 交易对视野(查看 / 切 bstock / all) · /keys 密钥(查看 / ai / demo / live)
也可以直接用自然语言提问, 例如: \"我部署了哪些策略\"
只读工具({nread} 个, 我直接调用): {read}
需确认工具({nvirt} 个, 我先说明再请你确认): {virt}
边界: 凡是会改文件或起停实例的动作 —— 落盘部署 / 覆盖部署 / 改参数 / 删除策略 /
启动试跑 / 停止试跑 / 启动测试网 / 停止测试网 / 实盘风险确认 / 启动实盘 / 停止实盘 /
平仓停止实盘 / 重启实盘 —— 都必须由你本人回一句确认词; 我(模型)只能登记待确认, 没有执行权。
常见事怎么办(别去找不存在的命令):
改参数 = 直接说\"把 <策略> 的 <参数> 改成 <值>\", 我回报改前改后, 你确认后生效(会按原模式自动重启);
删除策略 = 直接说\"删除 <策略>\", 你确认后我停实例并删掉策略文件(日志保留);
改密钥 = 用 /keys; 改交易对视野 = 用 /market。",
            nread = tools::READ_ONLY_TOOLS.len(),
            nvirt = tools::VIRTUAL_TOOLS.len(),
            read = tools::READ_ONLY_TOOLS.join(" / "),
            virt = tools::VIRTUAL_TOOLS.join(" / ")
        ),
        Lang::En => format!(
            "\
Commands: /help · /exit · /history (alias /log) · /lang · /market (view / bstock / all) · \
/keys (view / ai / demo / live)
You can also just ask in plain language, e.g. \"which strategies do I have deployed?\"
Read-only tools ({nread}, I run them right away): {read}
Confirmation-required tools ({nvirt}, I explain first and ask you to confirm): {virt}
Boundary: anything that changes files or starts/stops an instance — deploy / replace & deploy /
update parameters / delete a strategy / start or stop a dry run / start or stop the testnet demo /
accept live-trading risk / start live / stop live / close positions & stop / restart live — needs a
confirmation word from you. I (the model) can only register a pending action; I cannot execute it.
Common things, and how to ask (don't go looking for commands that do not exist):
Change parameters = just say \"change <parameter> of <strategy> to <value>\"; I report before and after
and it applies once you confirm (the instance restarts in its original mode).
Delete a strategy = say \"delete <strategy>\"; once you confirm I stop the instance and remove the
strategy files (logs are kept).
Change API keys = use /keys. Change the pair scope = use /market.",
            nread = tools::READ_ONLY_TOOLS.len(),
            nvirt = tools::VIRTUAL_TOOLS.len(),
            read = tools::READ_ONLY_TOOLS.join(" / "),
            virt = tools::VIRTUAL_TOOLS.join(" / ")
        ),
    }
}

/// 当前**正在运行**实例的模式(没在跑 → `None`); 只查视图, 不触盘。
///
/// 用于"停机/重启前先判模式"(FR-024): 例如别把实盘实例当试跑停掉。
async fn running_mode(root: &Path, name: &str) -> Option<String> {
    crate::commands::instances::views(root)
        .await
        .into_iter()
        .find(|v| v.name == name && v.running)
        .and_then(|v| v.mode)
}

/// 宿主执行已确认动作(与终端同内核, 不经 shell); 供会话与测试共用。
///
/// **13 类动作**全部落在同一处: 落盘 / 改参数 / 删除 / 起停的**判据与内核**都与 CLI 共用
/// ([`crate::commands::ctrl`]), 会话只负责把结果翻成给用户看的一段话。
/// 文案只讲"会发生什么"(FR-010): 不出现任何终端命令, 也不让用户去找不存在的子命令。
pub(crate) async fn execute_confirmed(
    action: &PendingAction,
    root: &Path,
    lang: Lang,
) -> CoreResult<String> {
    match action.kind {
        ActionKind::Deploy | ActionKind::DeployReplace => {
            let id = action.preview_id.as_deref().ok_or_else(|| {
                CoreError::InvalidArgument("内部状态错误: deploy 待办缺 preview_id".into())
            })?;
            let db = Database::open(&crate::commands::db_path_in(root))
                .await
                .map_err(|e| CoreError::Exchange(e.to_string()))?;
            let dir = crate::commands::ensure_strategies_dir_in(root)?;
            let token = ricow_engine::approve(&db, id).await?;
            // FR-044: 覆盖部署时引擎会先备份旧脚本再覆盖; 备份路径如实回报, 不省略。
            let replace = action.is_replace();
            let out = ricow_engine::execute_strategy(&db, id, &token, &dir, replace).await?;
            let backup_note = match out.backup.as_ref() {
                Some(p) => tf(
                    lang,
                    format!(
                        "受控覆盖(FR-044): 旧脚本已备份为\n  {}\n(需要回滚就把 .bak 复制回原文件名)\n",
                        p.display()
                    ),
                    format!(
                        "Controlled overwrite: the previous script was backed up to\n  {}\n(to roll back, copy the .bak back to the original file name)\n",
                        p.display()
                    ),
                ),
                None => String::new(),
            };
            // 覆盖后旧实例仍跑旧脚本 —— 只讲"怎么让它生效"(对话口径), 不给终端命令。
            let stale_note = if replace {
                tf(
                    lang,
                    format!(
                        "注意: {} 若正在运行, 执行的仍是旧脚本; 想让新脚本生效, 请在对话里说\
                         \"重启实盘 {}\"(实盘), 或先停再启(试跑 / 测试网)。\n",
                        action.name, action.name
                    ),
                    format!(
                        "Note: if {} is running it still executes the old script; to pick up the new one \
                         say \"restart live trading {}\" (live), or stop and start it again (dry run / testnet).\n",
                        action.name, action.name
                    ),
                )
            } else {
                String::new()
            };
            Ok(format!(
                "{head}\n  {toml}\n  {lua}\n{backup}{stale}",
                head = tf(lang, "已确认并完成落盘:", "Confirmed — written to disk:"),
                toml = out.toml_path.display(),
                lua = out.lua_path.display(),
                backup = backup_note,
                stale = stale_note,
            ))
        }
        ActionKind::UpdateParams => {
            // 参数在写盘之前全部解析完: 任一非法 → 直接报错, 一个字节都不落盘 (FR-024)。
            let mut updates = Vec::with_capacity(action.params.len());
            for raw in &action.params {
                let (k, v) = crate::commands::backtest::parse_param(raw).ok_or_else(|| {
                    CoreError::InvalidArgument(format!(
                        "参数 \"{raw}\" 不是 key=value 形式; 请说\"把 <参数> 改成 <值>\""
                    ))
                })?;
                updates.push((k, v));
            }
            // 只动 `[strategy.params]`, 顶层 live_enabled 等原样保留 (FR-025)。
            let diffs = crate::commands::update_strategy_params_in(root, &action.name, &updates)?;
            let listing =
                diffs.iter().map(|(k, d)| format!("  {k}: {d}")).collect::<Vec<_>>().join("\n");

            // 原模式必须在停机**之前**读: 停机后台账/视图已被覆盖, 无从得知原本跑的是什么。
            let prev_mode = running_mode(root, &action.name).await;
            let restart_note = if matches!(prev_mode.as_deref(), Some("dry_run" | "demo")) {
                // dry_run / demo 按原模式自动重启(FR-024, 不静默降级); 已确认过, 内核无需再要一次确认。
                let demo = prev_mode.as_deref() == Some("demo");
                crate::commands::ctrl::stop_daemon(root, &action.name, false).await?;
                let (pid, mode) =
                    crate::commands::ctrl::start_daemon(root, &action.name, false, demo, true)
                        .await?;
                tf(
                    lang,
                    format!(
                        "\n已按原模式自动重启({}), pid={pid}, 新参数已生效。",
                        crate::commands::instances::mode_text(&mode)
                    ),
                    format!(
                        "\nRestarted in its original mode ({}), pid={pid}; the new parameters are live.",
                        crate::commands::instances::mode_text(&mode)
                    ),
                )
            } else if prev_mode.as_deref() == Some("live") {
                // 实盘**不自动重启** (FR-024): 重启要重过三判据, 值得单独一次确认。
                t(
                    lang,
                    "\n注意: 该策略正在实盘运行, 新参数**尚未生效**; 要让它生效请在对话里说\
                     \"重启实盘 <策略名>\"(会重过实盘门禁)。",
                    "\nNote: this strategy is running live, so the change is **not in effect yet**; \
                     to apply it say \"restart live trading <strategy>\" (the live gates are re-checked).",
                )
                .to_string()
            } else {
                t(
                    lang,
                    "\n(该策略当前未运行; 下次启动即用新参数。)",
                    "\n(This strategy is not running; the new parameters apply the next time it starts.)",
                )
                .to_string()
            };
            Ok(format!(
                "{head}\n{listing}\n{note}",
                head = tf(
                    lang,
                    "已确认并完成改参数(写前已留时间戳备份):",
                    "Confirmed — parameters updated (a timestamped backup was kept):"
                ),
                note = restart_note,
            ))
        }
        ActionKind::DeleteStrategy => {
            // 运行中先停(close_all=false, 不平仓), 再删文件; **不删 logs/** (FR-024/FR-025)。
            let stopped = if running_mode(root, &action.name).await.is_some() {
                crate::commands::ctrl::stop_daemon(root, &action.name, false).await?
            } else {
                String::new()
            };
            let removed = crate::commands::delete_strategy_files_in(root, &action.name)?;
            let listing = removed.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n");
            Ok(format!(
                "{head}\n{stopped}{listing}",
                head = tf(
                    lang,
                    "已确认并完成删除(日志保留在 logs/):",
                    "Confirmed — deleted (logs are kept under logs/):"
                ),
            ))
        }
        ActionKind::StartDryRun => {
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, false, false, true).await?;
            Ok(tf(
                lang,
                format!(
                    "已确认: {} 正在以试跑(Dry Run)启动, pid={pid}(实时行情 + 虚拟下单, 不涉资金)。\n\
                     停机: 在对话里说\"停止试跑 {}\"",
                    crate::commands::instances::mode_text(&mode),
                    action.name
                ),
                format!(
                    "Confirmed: {} started as a dry run, pid={pid} (live market data, simulated fills, no funds).\n\
                     To stop it, say \"stop the dry run {}\".",
                    crate::commands::instances::mode_text(&mode),
                    action.name
                ),
            ))
        }
        ActionKind::StopDryRun => {
            // 停之前先判模式(FR-024): 别把测试网/实盘实例当成试跑停掉。
            if let Some(m) = running_mode(root, &action.name).await {
                if m != "dry_run" {
                    return Err(CoreError::InvalidArgument(tf(
                        lang,
                        format!(
                            "{} 当前以「{}」运行, 不是试跑; 要停它在对话里说\"停止测试网 {}\"或\"停止实盘 {}\"。",
                            action.name,
                            crate::commands::instances::mode_text(&m),
                            action.name,
                            action.name
                        ),
                        format!(
                            "{} is currently running as \"{}\", not as a dry run; to stop it say \
                             \"stop the testnet demo {}\" or \"stop live trading {}\".",
                            action.name,
                            crate::commands::instances::mode_text(&m),
                            action.name,
                            action.name
                        ),
                    )));
                }
            }
            let report = crate::commands::ctrl::stop_daemon(root, &action.name, false).await?;
            Ok(format!(
                "{report}{}",
                t(
                    lang,
                    "(试跑实例已停止; 与真实资金无关)",
                    "(The dry run is stopped; no funds were involved.)"
                )
            ))
        }
        ActionKind::StartDemo => {
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, false, true, false).await?;
            Ok(tf(
                lang,
                format!(
                    "已确认: {} 正在以测试网 demo 启动, pid={pid}(无真实资金; 会真实向测试网下单/撤单)。\n\
                     停机: 在对话里说\"停止测试网 {}\"",
                    crate::commands::instances::mode_text(&mode),
                    action.name
                ),
                format!(
                    "Confirmed: {} started on the testnet demo, pid={pid} (no real funds; orders are really \
                     placed and cancelled on the testnet).\nTo stop it, say \"stop the testnet demo {}\".",
                    crate::commands::instances::mode_text(&mode),
                    action.name
                ),
            ))
        }
        ActionKind::AckRisk => {
            // 018: 用户已在确认块里读过披露全文并回过确认词, 这里只落记录。
            // 记录位置固定为 `project_root()`(子进程 run 读的是同一份, 见 commands::risk_ack_path)。
            let path = crate::commands::write_risk_ack()?;
            Ok(format!(
                "{}\n\n{}",
                ricow_engine::RISK_DISCLOSURE,
                tf(
                    lang,
                    format!(
                        "已确认风险: {} (一次确认长期有效, 后续实盘不再重复要求)",
                        path.display()
                    ),
                    format!(
                        "Risk accepted: {} (a single acceptance stays valid; live trading will not ask again)",
                        path.display()
                    ),
                )
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
                             若缺「首次风险确认」, 请在对话里先完成一次实盘风险确认。)"
                    ))
                })?;
            let notice_text = notice.map(|n| format!("{n}\n")).unwrap_or_default();
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, true, false, true).await?;
            Ok(tf(
                lang,
                format!(
                    "{notice_text}已确认: {} 正在以**实盘**启动, pid={pid}(真实资金, 会真实下单)。\n\
                     停机: 在对话里说\"停止实盘 {}\"(保留持仓) 或\"平仓停止 {}\"(撤单并市价平仓)。",
                    crate::commands::instances::mode_text(&mode),
                    action.name,
                    action.name
                ),
                format!(
                    "{notice_text}Confirmed: {} started **live**, pid={pid} (real funds; orders are really placed).\n\
                     To stop it say \"stop live trading {}\" (keeps positions) or \"close positions and stop {}\" \
                     (cancels and market-closes).",
                    crate::commands::instances::mode_text(&mode),
                    action.name,
                    action.name
                ),
            ))
        }
        ActionKind::RestartLive => {
            // 与 CLI `ricow restart` 同一批内核: 先停(不平仓) → 重过实盘门禁(fail-closed) → 按实盘重启。
            let report = crate::commands::ctrl::stop_daemon(root, &action.name, false).await?;
            let notice = crate::commands::ctrl::live_preflight(root, &action.name, false)
                .await
                .map_err(|e| {
                    CoreError::InvalidArgument(format!(
                        "{e}\n(重启前复核发现实盘门禁未通过: 该实例已停止且**未重新启动**, \
                         也未发生任何下单; 请先处理门禁再试。)"
                    ))
                })?;
            let notice_text = notice.map(|n| format!("{n}\n")).unwrap_or_default();
            let (pid, mode) =
                crate::commands::ctrl::start_daemon(root, &action.name, true, false, true).await?;
            Ok(format!(
                "{report}{}",
                tf(
                    lang,
                    format!(
                        "{notice_text}已确认: {} 已按原模式重启为实盘, pid={pid}(真实资金, 会真实下单)。\n\
                         停机: 在对话里说\"停止实盘 {}\"(保留持仓) 或\"平仓停止 {}\"(撤单并市价平仓)。",
                        crate::commands::instances::mode_text(&mode),
                        action.name,
                        action.name
                    ),
                    format!(
                        "{notice_text}Confirmed: {} restarted **live**, pid={pid} (real funds; orders are really placed).\n\
                         To stop it say \"stop live trading {}\" (keeps positions) or \"close positions and stop {}\" \
                         (cancels and market-closes).",
                        crate::commands::instances::mode_text(&mode),
                        action.name,
                        action.name
                    ),
                )
            ))
        }
        ActionKind::StopDemo | ActionKind::StopLive | ActionKind::CloseLive => {
            let close_all = action.kind == ActionKind::CloseLive;
            let report = crate::commands::ctrl::stop_daemon(root, &action.name, close_all).await?;
            let tail = match action.kind {
                ActionKind::StopDemo => t(
                    lang,
                    "(测试网实例已停止; 与真实资金无关)",
                    "(The testnet instance is stopped; no real funds were involved.)",
                ),
                ActionKind::StopLive => t(
                    lang,
                    "(实盘实例已停止; 持仓仍保留在交易所, 如需离场请说\"平仓停止\")",
                    "(The live instance is stopped; positions remain on the exchange — say \
                     \"close positions and stop\" to exit.)",
                ),
                _ => t(
                    lang,
                    "(已请求撤单并市价平仓; 平仓结果以交易所回报为准, 见日志)",
                    "(Cancellation and market close requested; the exchange report is authoritative — \
                     see the logs.)",
                ),
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
        assert_eq!(classify("/stop shannon_spot_grid"), LineInput::Unknown("stop".into()));
    }

    /// 分隔线解析必须与渲染**同源**: 两种语言的渲染结果都认得出, 且只认完整分隔线
    /// (普通文本不该被误判成轮次号, 否则恢复时会把 `turn` 顶到离谱的值)。
    #[test]
    fn test_parse_turn_divider_round_trips_and_rejects_plain_text() {
        for n in [1u64, 7, 23, 1000] {
            for lang in [Lang::Zh, Lang::En] {
                assert_eq!(parse_turn_divider(&turn_divider(lang, n)), Some(n));
            }
        }
        // 会话切过语言也要认: 与当前 `lang` 无关
        assert_eq!(parse_turn_divider("── 第 12 轮 ──"), Some(12));
        assert_eq!(parse_turn_divider("── Turn 12 ──"), Some(12));
        assert_eq!(parse_turn_divider("  ── 第 3 轮 ──  "), Some(3), "前后空白不影响");
        for line in
            ["第 3 轮", "── 第 3 轮", "第 3 轮 ──", "── 你 ──", "── 第 轮 ──", "── Turn x ──", ""]
        {
            assert_eq!(parse_turn_divider(line), None, "不该把 {line:?} 当成分隔线");
        }
    }

    /// 近似误输提示: 词表与该语言的确认词同源(不手抄), 且只说清"为什么没生效 / 下一步"。
    #[test]
    fn test_near_miss_hint_names_the_same_words_as_the_matcher() {
        for lang in [Lang::Zh, Lang::En] {
            let hint = near_miss_confirmation_hint(lang);
            for w in confirm::confirmation_words(lang) {
                assert!(hint.contains(w), "{lang:?} 提示里没提确认词 {w}: {hint}");
            }
            assert!(!hint.contains("ricow "), "提示不该教终端命令: {hint}");
            // 要点: 说清"没生效"与"动作还在", 否则用户仍不知道该怎么办
            assert!(hint.contains(t(lang, "不算确认词", "not a confirmation word")));
        }
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

    /// 假 sink: 按脚本回答输入侧(`secret` / `input_line` 共用同一脚本队列), 收集所有输出行。
    struct FakeSink {
        answers: std::collections::VecDeque<Option<String>>,
        lines: Vec<String>,
    }

    impl SessionSink for FakeSink {
        fn text(&mut self, chunk: &str) {
            self.lines.push(chunk.to_string());
        }
        fn line_sev(&mut self, text: &str, _sev: Severity) {
            self.lines.push(text.to_string());
        }
        fn input_line(&mut self, _prompt: &str) -> Option<String> {
            self.answers.pop_front().flatten()
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
        let h = help_text(Lang::Zh);
        assert!(h.contains("/exit") && h.contains("确认"), "{h}");
        assert!(h.contains("/market"), "帮助要列出 /market: {h}");
        assert!(h.contains("/keys"), "帮助要列出 /keys: {h}");
    }

    /// SC-007: 单条往返超上限时截断并**标注原文长度**(不静默丢内容), 两种语言各自标注。
    #[test]
    fn test_clip_for_history_truncates_and_annotates() {
        // 上限以内原样返回
        let short = "问 1";
        assert_eq!(clip_for_history(short, Lang::Zh), short);
        let exact = "x".repeat(HISTORY_ENTRY_MAX_CHARS);
        assert_eq!(clip_for_history(&exact, Lang::Zh), exact, "恰好到上限不截断");
        // 超上限: 头部保留到上限 + 标注原文总字数
        let total = HISTORY_ENTRY_MAX_CHARS + 7;
        let long = "字".repeat(total);
        let zh = clip_for_history(&long, Lang::Zh);
        assert!(
            zh.starts_with(&"字".repeat(HISTORY_ENTRY_MAX_CHARS)),
            "应保留前 {HISTORY_ENTRY_MAX_CHARS} 字"
        );
        assert!(zh.contains(&format!("原文 {total} 字")), "应标注原文长度: {zh}");
        // 英文界面走英文标注(FR-031)
        let en = clip_for_history(&long, Lang::En);
        assert!(en.contains("truncated") && en.contains(&total.to_string()), "英文标注缺失: {en}");
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
        let hint = empty_state_hint(&root, Lang::Zh).expect("空目录须给两条路话术");
        for must in ["模板", "全新编写", "确认", "/market"] {
            assert!(hint.contains(must), "空状态话术缺少 {must}: {hint}");
        }
        // 有任意一条策略 toml 就不再是空状态
        let dir = root.join("strategies");
        std::fs::create_dir_all(&dir).expect("建策略目录");
        std::fs::write(dir.join("g1.toml"), "enabled = true").expect("写策略占位");
        assert!(empty_state_hint(&root, Lang::Zh).is_none(), "已有策略不该再提示两条路");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 造一个能 `open()` 的会话: 配置指向本机端点 —— 建会话不联网、不索要密钥。
    /// 只用于验证确认结果的 history 注入, 全程不问模型。
    async fn test_session(tag: &str) -> (ChatSession, std::path::PathBuf) {
        let root = temp_root(tag);
        std::fs::write(
            root.join("ricow.toml"),
            "[ai]\nprovider = \"oai\"\nmodel = \"test-model\"\n\
             base_url = \"http://127.0.0.1:9/v1\"\n",
        )
        .expect("写测试配置");
        let session =
            ChatSession::open(root.clone(), Options::default()).await.expect("建测试会话");
        (session, root)
    }

    /// 取一条 history 消息里的纯文本(非文本块按 Debug 兜底, 只为断言可读)。
    fn message_text(m: &Message) -> String {
        match m {
            Message::Assistant { content, .. } => content
                .iter()
                .map(|c| match c {
                    rig::message::AssistantContent::Text(t) => t.text().to_string(),
                    other => format!("{other:?}"),
                })
                .collect(),
            other => format!("{other:?}"),
        }
    }

    /// SC-005 / FR-021~FR-023: 写动作确认执行后, 结果**同源**注入 `history` ——
    /// 否则下一轮模型看不见自己刚确认的动作实际做了什么。
    #[tokio::test]
    async fn test_confirmed_result_is_injected_into_history() {
        let (mut session, root) = test_session("confirm-history-ok").await;
        // 造一份真实存在的策略文件, 让"删除策略"这个写动作成功(不碰网络/资金)
        let dir = root.join("strategies");
        std::fs::create_dir_all(&dir).expect("建策略目录");
        std::fs::write(dir.join("g1.toml"), "name = \"g1\"\n").expect("写策略文件");
        *session.pending.lock().await = Some(PendingAction::new_delete_strategy("g1"));

        let mut sink = fake(&[]);
        session.handle_line("确认", &mut sink).await.expect("确认行不该报错");

        assert_eq!(session.history.len(), 1, "执行结果必须注入 history 恰好一条");
        let injected = message_text(&session.history[0]);
        assert!(injected.contains("已确认并完成删除"), "history 须含执行结果文案: {injected}");
        // 同源(FR-022): 注入的就是 sink 显示的那份文本, 不是另写一遍
        assert!(
            sink.lines.iter().any(|l| l.contains(&injected)),
            "注入内容必须与显示同源:\n显示={:?}\nhistory={injected}",
            sink.lines
        );
        // 状态机语义不受注入影响: 确认已被消费; 注入文本即使被当输入行回放也不是确认词
        assert!(session.pending.lock().await.is_none(), "确认后 pending 必须清空");
        let slot = confirm::new_slot();
        *slot.lock().await = Some(PendingAction::new_delete_strategy("g2"));
        assert_eq!(
            confirm::consume_line(&slot, &injected, Lang::Zh).await,
            LineDisposition::Other,
            "注入文本不得构成确认"
        );
        assert!(slot.lock().await.is_some(), "非确认输入不得消费 pending");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// FR-024 / SC-005: 失败结果**同样**注入 —— 模型必须知道动作失败了, 而不是以为成功。
    #[tokio::test]
    async fn test_failed_confirmation_result_is_injected_into_history() {
        let (mut session, root) = test_session("confirm-history-fail").await;
        // 不存在的策略 → 删除动作必然失败(不碰网络/资金); 失败也不上抛, 只如实回报
        *session.pending.lock().await = Some(PendingAction::new_delete_strategy("missing"));

        let mut sink = fake(&[]);
        session.handle_line("确认", &mut sink).await.expect("失败也要如实回报, 不上抛");

        assert_eq!(session.history.len(), 1, "失败结果同样注入 history");
        let injected = message_text(&session.history[0]);
        assert!(injected.contains("执行失败"), "history 须含失败文案: {injected}");
        assert!(
            sink.lines.iter().any(|l| l.contains(&injected)),
            "失败文案也必须同源:\n显示={:?}\nhistory={injected}",
            sink.lines
        );
        assert!(session.pending.lock().await.is_none(), "失败后 pending 不得残留");
        let _ = std::fs::remove_dir_all(&root);
    }
}
