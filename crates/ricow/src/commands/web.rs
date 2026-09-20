//! `ricow web` — 内置 Web UI 入口 (025): 启动只绑回环的本地网页, 浏览器里完成全部对话与操作。
//!
//! 业务逻辑零复制: 对话仍走 [`crate::ai::session`], 这里只做装配
//! (首次向导 → 会话存储 → axum 服务 → 打开系统默认浏览器)。

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;

use crate::ai::session::{ChatSession, Options, SessionSink, Severity};
use crate::commands::chat::repl;
use crate::commands::config_file;
use crate::i18n::{self, t};
use crate::web::{self, Line, SessionStore, Starter, WebSink, WebState};

#[derive(Args)]
pub struct WebArgs {
    /// 监听端口(默认 0 = 由系统分配空闲端口)
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// 不自动打开浏览器(只打印带 token 的地址)
    #[arg(long)]
    pub no_open: bool,
}

/// 启动 Web UI: 前置检查(与 CLI 对话同源) → 建服务与存储 → 打印带 token 的地址 → 开浏览器。
pub async fn run(args: WebArgs) -> CoreResult<()> {
    let root = crate::commands::project_root();
    let lang = i18n::resolve(&config_file::load(&root)?);
    println!("{}: {}", t(lang, "ricow 数据目录", "ricow data dir"), root.display());
    println!("{}: {}", t(lang, "配置文件", "config"), config_file::path(&root).display());

    // 首次向导: 与 `chat.rs` 走同一条路径, 但这里传 `strict = false`。双击启动的 Web 进程
    // stdin 不是终端, strict 会在**开浏览器之前**直接报错退出, 与 FR-003(启动即出页面)冲突;
    // FR-004 又要求"无 AI 密钥时不得静默失败, 要在浏览器内给出可操作的提示" ——
    // 放行后 `ChatSession::open` 仍会报出带配置路径的错误, 由会话线程写进对话流(见 `Hub::attach`)。
    crate::commands::onboard::run_if_needed(&root, false).await?;

    // 向导可能刚写入 `[ui].lang` → 之后一律用最新语言(与 `chat.rs` 同序)。
    let lang = i18n::resolve(&config_file::load(&root)?);
    if let Some(w) = config_file::permission_warning(&root) {
        eprintln!("{}: {w}", t(lang, "提示", "note"));
    }

    // 会话存储: 复用同一份本地库(D9); `SessionStore` 内部是连接池, 克隆给各 handler 很廉价。
    let db = Database::open(&crate::commands::db_path_in(&root))
        .await
        .map_err(|e| CoreError::Exchange(format!("打开本地库失败: {e}")))?;

    let (listener, port) = web::bind(args.port).await?;
    let token = web::new_token();
    // 同一份存储交给两处: 会话线程(恢复上下文 + 落流水, T021)与 HTTP handler(列表 / 回放)。
    // 交易端点也要读同一份库(D1), 故先把句柄克隆一份下来(连接池克隆零成本)。
    let store = SessionStore::new(db.clone());
    let starter = make_starter(root.clone(), store.clone());
    let state = WebState::new(token.clone(), root, db, store, starter);

    // token 只在内存里(不落盘、不进日志), 所以地址必须打出来才能进页面(FR-003)。
    let url = format!("http://127.0.0.1:{port}/?token={token}");
    println!("{}: {url}", t(lang, "Web UI 地址", "Web UI URL"));
    println!(
        "{}",
        t(
            lang,
            "对话在浏览器里进行; 按 Ctrl-C 停止服务。",
            "the conversation runs in the browser; press Ctrl-C to stop the server."
        )
    );
    if !args.no_open {
        if let Err(e) = open_browser(&url) {
            eprintln!(
                "{}: {e}",
                t(
                    lang,
                    "打开浏览器失败, 请手动访问上面的地址",
                    "could not open a browser; please visit the URL above"
                )
            );
        }
    }

    web::serve(listener, state).await
}

/// 造会话线程体(由 `Hub::attach` 在专用 OS 线程上调用)。
///
/// 线程体是同步闭包, 而开会话与 REPL 都是 async —— 句柄必须在**装配处**(还在 tokio 上下文里)
/// 捕获: `Hub::attach` 用的是 `std::thread`, 线程里没有运行时上下文, 直接 `Handle::current()`
/// 会 panic。捕获到的句柄指向主运行时(`#[tokio::main]` 多线程), `block_on` 只停住本会话线程。
///
/// 连 `store` 一起捕获: 每条会话线程都要恢复**自己的**旧上下文并把新流水落库(T021); 闭包是
/// `Fn`(会被反复调用), 故每次调用各自克隆一份 —— 内部是连接池句柄, 克隆很廉价。
fn make_starter(root: PathBuf, store: SessionStore) -> Starter {
    let handle = tokio::runtime::Handle::current();
    Arc::new(move |session_id: &str, sink: &mut WebSink| {
        handle.block_on(session_loop(root.clone(), store.clone(), session_id, sink))
    })
}

/// 一条会话的完整流程: 开会话(以前端能力声明) → **恢复旧上下文** → 欢迎语 → 复用 CLI 的
/// REPL 循环; 全程把对话流水落库, 供重开页面时回放(FR-020 / FR-021)。
async fn session_loop(
    root: PathBuf,
    store: SessionStore,
    session_id: &str,
    sink: &mut WebSink,
) -> CoreResult<()> {
    let mut session = ChatSession::open(
        root,
        Options {
            plain: false,
            model: None,
            base_url: None,
            // 前端能力声明(D6 / FR-005): 浏览器有输入通道, 但进程的 stdin 未必是终端。
            interactive: true,
            has_input_channel: true,
        },
    )
    .await?;

    // 恢复旧会话的 AI 上下文(D10 / FR-020): 只喂会话, **不往浏览器重发** —— 页面历史由 HTTP
    // `/messages` 独立回放(见 `web/mod.rs`), 两条路径互不依赖。
    // 轮次号独立取"页面流水里的最大号"(而不是 `rounds` 条数): 后者丢过行, 会比页面刚显示的
    // 号小, 新一轮就倒退(用户实测过 23 → 16)。
    let rounds = store.resume_rounds(session_id).await?;
    let turn = store.last_turn(session_id).await?;
    session.resume_history(&rounds, turn);

    // 单写者落库: 无界通道的 `send` 是同步非阻塞的, 故同步签名的 sink 里不必 `.await`;
    // 由这一个任务串行消费, FIFO 天然保序 = 落库顺序与页面顺序一致。
    let sid = session_id.to_string();
    let lang = session.lang();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Row>();
    let writer = tokio::spawn(async move {
        while let Some(row) = rx.recv().await {
            let written = match row {
                Row::User(text) => store.append_user(&sid, &text).await,
                Row::Assistant(text) => store.append_assistant(&sid, &text).await,
                Row::Host(text, sev) => store.append_host(&sid, &text, sev).await,
            };
            if let Err(e) = written {
                // 落库失败不该打断对话: 页面照常, 只是这一条刷新后不再出现。
                eprintln!(
                    "{}: {e}",
                    t(lang, "会话流水落库失败", "failed to persist the session log")
                );
            }
        }
    });

    let outcome = {
        let mut recorder = Recorder::new(&mut *sink, tx);
        session.welcome(&mut recorder);
        repl(&mut session, &mut recorder).await
    };
    // 包装 sink 已析构 → 发送端关闭 → 写者把积压写完即退出。
    let _ = writer.await;
    outcome
}

/// 会话流水的一行(由 [`Recorder`] 送往单写者任务)。
#[derive(Debug, PartialEq, Eq)]
enum Row {
    /// 用户输入(`input_line` 回传的那一行)。
    User(String),
    /// 助手完整回复(由流式增量拼成)。
    Assistant(String),
    /// 宿主提示行 + 级别(重开页面后仍按原级别着色, FR-018 / FR-023)。
    Host(String, Severity),
}

/// 落库包装 sink(T021): 套在 [`WebSink`] 外面, 把对话流水按**页面出现的顺序**写进
/// `web_messages`, 供重开页面时整屏回放(FR-021 / SC-006)。
///
/// 组合而非替换: [`Starter`] 收的是具体类型 `&mut WebSink`, 包装只能在会话线程体内做。
/// 同步签名里**不做 `.await`**(那会把会话线程卡在 I/O 上), 只把 [`Row`] 推进无界通道。
struct Recorder<'a> {
    inner: &'a mut WebSink,
    tx: tokio::sync::mpsc::UnboundedSender<Row>,
    /// 助手增量的暂存: 流式 `text()` 只累积, 遇到下一整行时先作为一条回复落库。
    pending: String,
    /// **首条用户输入之前不落库**: 每次挂载都会重发欢迎语与帮助横幅(见 `Hub::attach`),
    /// 它们不是本次对话的产物, 记进流水只会让每个会话开头的噪声越滚越多。
    recording: bool,
}

impl<'a> Recorder<'a> {
    fn new(inner: &'a mut WebSink, tx: tokio::sync::mpsc::UnboundedSender<Row>) -> Self {
        Self { inner, tx, pending: String::new(), recording: false }
    }

    /// 把暂存的助手增量作为一条回复落库(没有增量则不动)。
    fn flush_assistant(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let _ = self.tx.send(Row::Assistant(std::mem::take(&mut self.pending)));
    }

    /// 落一条宿主行; 首条用户输入之前静默丢弃(见 [`Recorder::recording`])。
    fn record_host(&mut self, text: &str, sev: Severity) {
        if self.recording {
            let _ = self.tx.send(Row::Host(text.to_string(), sev));
        }
    }
}

impl SessionSink for Recorder<'_> {
    /// 助手增量: 缓存待落库的那一份, 同时原样转发给浏览器(流式渲染不受影响)。
    fn text(&mut self, chunk: &str) {
        self.pending.push_str(chunk);
        self.inner.text(chunk);
    }

    /// 先落助手增量再落这一行 —— 顺序必须与页面一致, 否则回放时回答会跑到提示行之后。
    fn line_sev(&mut self, text: &str, sev: Severity) {
        self.flush_assistant();
        self.record_host(text, sev);
        self.inner.line_sev(text, sev);
    }

    fn input_line(&mut self, prompt: &str) -> Option<String> {
        self.flush_assistant();
        match self.inner.next_line(prompt)? {
            // 前端是**乐观渲染**(见 `assets/app.js`): 用户气泡由浏览器自己先画, 实时流不回显,
            // 所以用户行只能在这里补记 —— 否则刷新后整段提问都消失(FR-021 / SC-006)。
            Line::User(text) => {
                self.recording = true;
                let _ = self.tx.send(Row::User(text.clone()));
                Some(text)
            }
            // 宿主控制行(FR-030 切语言): 照旧交给 REPL, 但**不进流水** —— 用户没说过这句话,
            // 记进去会让刷新后的历史与刚看到的对话对不上, 还会顶掉空会话的标题(FR-022)。
            // 它也不算"首条用户输入", 故不解除 [`Recorder::recording`] 的守卫。
            Line::Control(text) => Some(text),
        }
    }

    /// 密钥**不落库**(SC-011): 明文只经密钥通道交给会话, 不进对话流水。
    fn secret(&mut self, prompt: &str) -> Option<String> {
        self.inner.secret(prompt)
    }
}

/// 用系统默认浏览器打开 URL。
///
/// 零新依赖(D17 未引入 opener 类 crate): 各平台用自带命令。打不开不算启动失败 ——
/// 地址已打印, 用户可手动访问。
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        // `start` 是 cmd 内建命令; 它的第一个参数会被当成窗口标题, 故先给一个空标题。
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::Frame;

    /// T021: 落库顺序 = 页面顺序; 挂载时的欢迎语 / 帮助横幅不进流水; 密钥不进流水(SC-011)。
    #[test]
    fn test_recorder_orders_rows_and_skips_prelude() {
        let (frames, _old_rx) = tokio::sync::broadcast::channel(8);
        let frames_tx = frames.clone();
        let (mut sink, input) = WebSink::pair(frames);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut recorder = Recorder::new(&mut sink, tx);

        // 欢迎语 + 帮助横幅: 每次挂载都会重发, 不是对话产物 → 一条都不该落库。
        recorder.line("ricow AI 助手 ...");
        recorder.line("");
        assert!(rx.try_recv().is_err(), "首条用户输入之前不落库");

        // 一轮: 用户输入 → 分隔线 → 助手增量 → 用量 → 收尾空行(顺序同 `ChatSession::reply`)。
        assert!(input.submit("今天行情如何".into()));
        assert_eq!(recorder.input_line("你 > ").as_deref(), Some("今天行情如何"));
        recorder.line("");
        recorder.line("── 第 1 轮 ──");
        recorder.text("涨");
        recorder.text("了");
        recorder.notice("[用量] 12 tokens");
        recorder.line("");

        let rows: Vec<Row> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            rows,
            vec![
                Row::User("今天行情如何".into()),
                Row::Host(String::new(), Severity::Normal),
                Row::Host("── 第 1 轮 ──".into(), Severity::Normal),
                // 增量合并成一条回复, 且落在用量行之前
                Row::Assistant("涨了".into()),
                Row::Host("[用量] 12 tokens".into(), Severity::Notice),
                Row::Host(String::new(), Severity::Normal),
            ]
        );

        // 密钥录入: 明文只经密钥通道, 不落库(SC-011)。
        // 从此刻起**新订阅**再等: 只认密钥提示帧, 不受前面那些行帧的缓冲影响(容量有限)。
        let mut frames_rx = frames_tx.subscribe();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| recorder.secret("API Key: "));
            // `secret` 先备好密钥路由再发提示帧, 故收到提示帧即可安全回传(否则会走普通输入)。
            assert!(matches!(frames_rx.blocking_recv(), Ok(Frame::SecretPrompt { .. })));
            assert!(input.submit("s3cret".into()));
            assert_eq!(worker.join().expect("会话线程").as_deref(), Some("s3cret"));
        });
        assert!(rx.try_recv().is_err(), "密钥不得进对话流水");
    }

    /// T029 / FR-030: 切语言的**控制行**不落库 —— 否则刷新后历史里会凭空多出一条用户没发过的
    /// "/lang en", 空会话的标题也会被它顶掉(FR-022)。
    #[test]
    fn test_recorder_skips_control_line() {
        let (frames, _old_rx) = tokio::sync::broadcast::channel(8);
        let (mut sink, input) = WebSink::pair(frames);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut recorder = Recorder::new(&mut sink, tx);

        // 服务端自己发起的切语言: 文本照给 REPL, 但一条都不落库。
        assert!(input.submit_control("/lang en".into()));
        assert_eq!(recorder.input_line("你 > ").as_deref(), Some("/lang en"));
        assert!(rx.try_recv().is_err(), "控制行不进流水");

        // 它也不算"用户开过口": 切换回执仍按前言处理, 不落库。
        recorder.notice("Language switched to English (en)");
        assert!(rx.try_recv().is_err(), "控制行不解除前言守卫");

        // 用户真正开口 → 从此正常落库。
        assert!(input.submit("今天行情如何".into()));
        assert_eq!(recorder.input_line("你 > ").as_deref(), Some("今天行情如何"));
        assert_eq!(rx.try_recv().expect("用户行"), Row::User("今天行情如何".into()));
    }
}
