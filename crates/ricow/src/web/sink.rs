//! `WebSink` (025 / D5): 会话 I/O 的浏览器版 —— 出站走 SSE 帧, 入站走浏览器输入区。
//!
//! 会话逻辑**零复制**: 本文件只实现 [`SessionSink`], 全部判断仍在 [`crate::ai::session`]。
//!
//! 阻塞语义是刻意的: [`SessionSink::input_line`] / [`SessionSink::secret`] 是同步签名(019
//! 已定"允许阻塞等待"), 而会话在**专用 OS 线程**上跑 [`crate::commands::chat::repl`](见
//! `commands/web.rs` 装配), 因此这里的 `recv()` 只停住会话线程, 不占 tokio 工作线程。

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::ai::session::{SessionSink, Severity};

/// 送往浏览器的一帧(SSE 的 `data:` 载荷; 前端按 `type` 分派)。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// 助手文本增量(前端追加到当前回复)。
    Delta { text: String },
    /// 一整行宿主输出 + 级别(前端按 `sev` 着色, FR-013 / FR-023)。
    Line { text: String, sev: &'static str },
    /// 会话要读一次**不回显**的密钥: 前端应切到遮蔽输入(FR-012)。
    SecretPrompt { prompt: String },
    /// 会话已就绪、正等下一行输入 = 上一轮收尾: 前端据此解除输入区禁用(FR-009 / R11)。
    TurnEnd,
    /// 会话结束: 前端提示新建会话(见 [`Drop`] 实现)。
    Closed,
}

impl Frame {
    /// 编码为 SSE 的 `data:` 载荷(纯数据结构, 编码不会失败)。
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"type":"closed"}"#.to_string())
    }
}

/// 出站帧通道: **广播**给所有在连的 SSE 连接。
///
/// 用广播而非单播: 页面刷新会新开一条 SSE 连接(旧连接尚未被服务端察觉), 广播让新连接
/// 立刻接上; 没有连接时帧直接丢弃(帧只是实时视图, 落库由 `web/store.rs` 负责)。
pub type FrameSender = tokio::sync::broadcast::Sender<Frame>;

/// 浏览器 → 会话方向的一行。
///
/// 分两类是因为**落库层要认来源**(`commands/web.rs` 的 `Recorder`): 用户输入要进对话流水,
/// 而宿主自己发起的控制行(FR-030 的 `/lang`)只是"替 REPL 认一条命令" —— 记进流水会让
/// 刷新后的历史凭空多出一句用户没说过的话, 还会顶掉空会话的标题(FR-022)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// 浏览器输入区发来的一行(= 用户说的话)。
    User(String),
    /// 宿主发起的控制行(见 [`InputChannel::submit_control`])。
    Control(String),
}

impl Line {
    /// 会话要处理的那串文本(两类一视同仁: 都要过 REPL 的斜杠命令解析)。
    pub fn text(self) -> String {
        match self {
            Line::User(text) | Line::Control(text) => text,
        }
    }
}

/// 浏览器 → 会话方向的**路由**: 普通一行与密钥**必须分流**(SC-011: 明文密钥不进对话流)。
struct Inbound {
    lines: Sender<Line>,
    secrets: Sender<String>,
    /// 会话正在等一次密钥录入 —— 由 [`WebSink::secret`] 置位。
    awaiting_secret: bool,
}

/// 浏览器 → 会话方向的句柄(HTTP handler 持有, 与 [`WebSink`] 成对)。
#[derive(Clone)]
pub struct InputChannel {
    inner: Arc<Mutex<Inbound>>,
}

impl InputChannel {
    /// 送入一条**用户**输入; 返回 `false` = 会话线程已结束(通道断开), 前端应提示重开会话。
    ///
    /// 路由由**服务端**决定(不信前端自称): 会话正等密钥时这一条走密钥通道。
    pub fn submit(&self, text: String) -> bool {
        let mut inner = lock(&self.inner);
        if inner.awaiting_secret {
            inner.awaiting_secret = false;
            return inner.secrets.send(text).is_ok();
        }
        inner.lines.send(Line::User(text)).is_ok()
    }

    /// 送入一条**宿主控制行**(FR-030 切语言用): 与用户输入走同一条 REPL 命令解析, 但
    /// **无视密钥路由** —— 控制行永远不是密钥, 会话正等密钥时也不许被塞进密钥通道(SC-011)。
    pub fn submit_control(&self, text: String) -> bool {
        lock(&self.inner).lines.send(Line::Control(text)).is_ok()
    }
}

/// 会话线程侧的 sink: 出站 → SSE 帧; 入站 → 阻塞读浏览器输入。
pub struct WebSink {
    frames: FrameSender,
    inbound: Arc<Mutex<Inbound>>,
    lines: Receiver<Line>,
    secrets: Receiver<String>,
}

impl WebSink {
    /// 建一对: 会话线程拿 [`WebSink`], HTTP handler 拿 [`InputChannel`]。
    pub fn pair(frames: FrameSender) -> (Self, InputChannel) {
        let (line_tx, lines) = mpsc::channel();
        let (secret_tx, secrets) = mpsc::channel();
        let inbound = Arc::new(Mutex::new(Inbound {
            lines: line_tx,
            secrets: secret_tx,
            awaiting_secret: false,
        }));
        (
            Self { frames, inbound: Arc::clone(&inbound), lines, secrets },
            InputChannel { inner: inbound },
        )
    }

    /// 发一帧; 当前没有 SSE 连接时静默丢弃(帧只是实时视图)。
    fn send(&self, frame: Frame) {
        let _ = self.frames.send(frame);
    }

    /// 读一行输入并**保留来源标记**(落库层用, 见 [`Line`]): 与 [`SessionSink::input_line`]
    /// 是同一个入口, 只是多交一个"谁发的"。
    ///
    /// 两者都必须先发 [`Frame::TurnEnd`] —— 本方法是**唯一**的轮次分界线(D16)。
    pub fn next_line(&mut self, _prompt: &str) -> Option<Line> {
        // 输入框自带提示, `prompt` 用不上(见 trait 文档)。
        self.send(Frame::TurnEnd);
        self.lines.recv().ok()
    }
}

/// sink 的生命周期 = 会话线程的生命周期(装配处仅在会话线程作用域内持有它): 线程收摊时
/// 必发一帧 [`Frame::Closed`], 前端据此提示"会话已结束, 请新建会话"。
impl Drop for WebSink {
    fn drop(&mut self) {
        self.send(Frame::Closed);
    }
}

impl SessionSink for WebSink {
    /// 助手增量 → 立即一帧 = 浏览器里的流式渲染。
    fn text(&mut self, chunk: &str) {
        self.send(Frame::Delta { text: chunk.to_string() });
    }

    /// 整行 + 级别; 文案原样透传(宿主给什么发什么, 实现不得改写)。
    fn line_sev(&mut self, text: &str, sev: Severity) {
        self.send(Frame::Line { text: text.to_string(), sev: sev.as_str() });
    }

    /// 读一行浏览器输入; 通道断开(会话被替换 / 服务收摊) → `None`, 与终端 EOF 同义。
    ///
    /// 本方法是**唯一**的轮次分界线: `repl` 只在"上一轮处理完毕、准备读下一行"时调用它,
    /// 故在此发 [`Frame::TurnEnd`] 即可让前端精确解除输入区禁用, 无需改动 `repl`(D16)。
    fn input_line(&mut self, prompt: &str) -> Option<String> {
        self.next_line(prompt).map(Line::text)
    }

    /// 读一次**不回显**的密钥: 先告知前端(切遮蔽输入), 再把路由让给密钥通道, 最后阻塞等回传。
    ///
    /// `None`(通道断开)与终端 Ctrl-C 同义 → 会话**放弃本次修改且不写文件**(FR-015)。
    fn secret(&mut self, prompt: &str) -> Option<String> {
        // 先备好路由再提示前端: 前端一收到提示就可能立刻回传, 顺序颠倒会把密钥当普通输入。
        lock(&self.inbound).awaiting_secret = true;
        self.send(Frame::SecretPrompt { prompt: prompt.to_string() });
        self.secrets.recv().ok()
    }
}

/// 取锁并**容忍中毒**(同 `supervisor::server::lock_state`): 持锁线程 panic 不该让之后每次输入都失败。
fn lock(inner: &Mutex<Inbound>) -> std::sync::MutexGuard<'_, Inbound> {
    inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 帧编码: 前端按 `type` 分派、按 `sev` 着色, 故字段名与取值都是对外契约。
    #[test]
    fn test_frame_encode_shape() {
        let value = |f: &Frame| -> serde_json::Value {
            serde_json::from_str(&f.encode()).expect("帧是合法 JSON")
        };
        let delta = value(&Frame::Delta { text: "hi".into() });
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["text"], "hi");

        let line = value(&Frame::Line { text: "注意".into(), sev: Severity::Warn.as_str() });
        assert_eq!(line["type"], "line");
        assert_eq!(line["text"], "注意");
        assert_eq!(line["sev"], "warn");

        assert_eq!(value(&Frame::TurnEnd)["type"], "turn_end");
        assert_eq!(value(&Frame::Closed)["type"], "closed");
    }

    #[test]
    fn test_line_sev_carries_level_and_text() {
        let (frames, mut rx) = tokio::sync::broadcast::channel(8);
        let (mut sink, _input) = WebSink::pair(frames);
        sink.line_sev("出错了", Severity::Error);
        sink.text("增量");
        assert_eq!(
            rx.try_recv().expect("行帧"),
            Frame::Line { text: "出错了".into(), sev: "error" }
        );
        assert_eq!(rx.try_recv().expect("增量帧"), Frame::Delta { text: "增量".into() });
    }

    #[test]
    fn test_input_line_mark_turn_end_and_route_secret() {
        let (frames, mut rx) = tokio::sync::broadcast::channel(8);
        let (mut sink, input) = WebSink::pair(frames);

        // 平时: 走普通通道; 读行前先发 TurnEnd(前端据此解除禁用)。
        assert!(input.submit("/help".into()));
        assert_eq!(sink.input_line("你 > "), Some("/help".into()));
        assert_eq!(rx.try_recv().expect("结束帧"), Frame::TurnEnd);

        // 密钥录入期间: 同一入口把这一条路由到密钥通道(前端无需自报)。
        let worker = std::thread::spawn(move || {
            let got = sink.secret("API Key: "); // sink 移入线程, 用完原样还回。
            (sink, got)
        });
        assert_eq!(
            rx.blocking_recv().expect("密钥提示帧"),
            Frame::SecretPrompt { prompt: "API Key: ".into() }
        );
        assert!(input.submit("s3cret".into()));
        let (mut sink, got) = worker.join().expect("会话线程");
        assert_eq!(got, Some("s3cret".into()));

        // 用掉一次即复位: 下一条仍走普通通道, 且密钥不进对话流。
        assert!(input.submit("再来".into()));
        assert_eq!(sink.input_line(""), Some("再来".into()));
        assert_eq!(rx.try_recv().expect("第二轮的结束帧"), Frame::TurnEnd);

        // sink 丢下即会话结束: 前端收到 Closed。
        drop(sink);
        assert_eq!(rx.try_recv().expect("会话结束帧"), Frame::Closed);
    }

    /// FR-012 / SC-011: `/keys demo` 这类要**连读两个密钥**的流程(见 `ai::session::read_pair`)
    /// 也得走密钥通道 —— 第二次录入前重新备好路由, 且不给前端留"回传当普通输入"的缝。
    #[test]
    fn test_consecutive_secret_reads_route_to_secret_channel() {
        let (frames, mut rx) = tokio::sync::broadcast::channel(8);
        let (mut sink, input) = WebSink::pair(frames);

        std::thread::scope(|scope| {
            let worker =
                scope.spawn(|| (sink.secret("Demo API Key: "), sink.secret("Demo API Secret: ")));
            for prompt in ["Demo API Key: ", "Demo API Secret: "] {
                match rx.blocking_recv() {
                    Ok(Frame::SecretPrompt { prompt: got }) => assert_eq!(got, prompt),
                    other => panic!("应收到密钥提示帧({prompt}), 实际: {other:?}"),
                }
                // 收到提示帧即说明路由已备好, 此刻回传才不会被当成普通输入。
                assert!(input.submit("S".into()));
            }
            let (key, secret) = worker.join().expect("会话线程");
            assert_eq!((key.as_deref(), secret.as_deref()), (Some("S"), Some("S")));
        });

        // 两次都走密钥通道 → 复位后普通输入照旧可用(证明复用同一入口不串道)。
        assert!(input.submit("普通输入".into()));
        assert_eq!(sink.input_line(""), Some("普通输入".into()));
    }

    /// FR-030 / SC-011: 控制行**绕开密钥路由** —— 会话正等密钥时切语言, 那条 `/lang` 绝不能
    /// 被当成密钥交出去(它会变成一次真实的密钥写入尝试)。
    #[test]
    fn test_control_line_bypasses_secret_route() {
        let (frames, mut rx) = tokio::sync::broadcast::channel(8);
        let (mut sink, input) = WebSink::pair(frames);

        let worker = std::thread::spawn(move || {
            let got = sink.secret("API Key: ");
            (sink, got)
        });
        // `secret` 先置位路由再发提示帧, 故看到提示帧时"正等密钥"已经成立。
        assert_eq!(
            rx.blocking_recv().expect("密钥提示帧"),
            Frame::SecretPrompt { prompt: "API Key: ".into() }
        );
        assert!(input.submit_control("/lang en".into()));
        assert!(input.submit("s3cret".into()));
        let (mut sink, got) = worker.join().expect("会话线程");
        assert_eq!(got, Some("s3cret".into()), "密钥通道只该收到真正的密钥");

        // 控制行安然排在普通通道里, 且来源标记为宿主(`Recorder` 据此不记用户流水)。
        assert_eq!(sink.next_line(""), Some(Line::Control("/lang en".into())));
        assert_eq!(rx.try_recv().expect("结束帧"), Frame::TurnEnd);
        drop(sink);
        assert_eq!(rx.try_recv().expect("会话结束帧"), Frame::Closed);
    }
}
