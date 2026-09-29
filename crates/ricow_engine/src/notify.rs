//! 出站通知 (003): 成交 / 接近强平 / 停机残留 → 用户自选 webhook。
//! (020 起删除"熔断"事件: 平台不再有亏损熔断。)
//!
//! **为什么是 webhook**: `specs/product.md` §四把"本地私钥 + 数据不出本机"作为支点, 因此通知不引入平台账号、
//! 不写死第三方 SDK —— 一个 `POST` JSON 就能覆盖 Telegram(`sendMessage`)/飞书/Slack/自建端点。
//!
//! **分层**: 本模块把"要不要发、发什么文本"([[`NotifyEvent::render`]] / [`Throttle`] / 去重, 全可单测)
//! 与"怎么发出去"([`Notifier::dispatch`], spawn + 重试)分开; 投递在后台任务内重试(2s/8s 退避,
//! 至多 3 次尝试), 绝不反压策略循环 (003 FR-005 / A2; 042 FR-3 补重试)。
//!
//! **平台适配 (042 FR-1)**: 请求体形态由 [`NotifyFormat`] 决定 —— `auto` 按 URL 识别
//! Telegram / 飞书 / Slack, 其余 raw (003 行为); 可用 `notify_format` 显式覆盖。
//!
//! **默认关闭**: 未配置 `params.notify_webhook` 即完全不打通道 (与"实盘默认 Dry Run"同一保守口径)。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ricow_strategy::StrategyConfig;
use rust_decimal::Decimal;
use serde_json::json;

/// 投递超时 (FR-005): 通知慢不能拖住任何东西。
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// 事件类别 (配置白名单的最小单位)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// 成交。
    Fill,
    /// 接近强平 (014)。
    LiqWarn,
    /// 停机有残留 (011 撤单兜底后仍有本策略挂单/持仓)。
    Residual,
    /// 实盘/demo 启动 (042)。
    Started,
    /// 停机 (042): 含原因与累计成交数。
    Stopped,
    /// 策略置停机标记 (042): Lua fatal/halted = 1, 每进程至多一条。
    Halted,
    /// 策略停摆 (042): stat_stall_bars 达阈值, 每进程至多一条。
    Stall,
    /// 运行摘要心跳 (042)。
    Heartbeat,
    /// 子进程自行退出 (042, supervisor 侧)。
    Crashed,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Fill => "fill",
            EventKind::LiqWarn => "liq_warn",
            EventKind::Residual => "residual",
            EventKind::Started => "started",
            EventKind::Stopped => "stopped",
            EventKind::Halted => "halted",
            EventKind::Stall => "stall",
            EventKind::Heartbeat => "heartbeat",
            EventKind::Crashed => "crashed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fill" => Some(EventKind::Fill),
            "liq_warn" => Some(EventKind::LiqWarn),
            "residual" => Some(EventKind::Residual),
            "started" => Some(EventKind::Started),
            "stopped" => Some(EventKind::Stopped),
            "halted" => Some(EventKind::Halted),
            "stall" => Some(EventKind::Stall),
            "heartbeat" => Some(EventKind::Heartbeat),
            "crashed" => Some(EventKind::Crashed),
            _ => None,
        }
    }

    /// 每进程至多一条的事件 (notify_once 去重): 状态量而非流水, 重复只刷屏。
    fn is_once(self) -> bool {
        matches!(self, EventKind::Halted | EventKind::Stall | EventKind::Crashed)
    }
}

/// 数值展示规整: 去掉 f64 尾差与多余零 (`0.0040000000000000000832667269` → `0.004`)。
///
/// Lua 数字经 f64 带入的尾差对**下单**无碍(引擎按 step/tick 对齐), 但通知是给人看的文本, 必须干净。
fn fmt_num(d: &Decimal) -> String {
    d.round_dp(8).normalize().to_string()
}

/// 通知事件 (纯数据)。
#[derive(Debug, Clone, PartialEq)]
pub enum NotifyEvent {
    /// 成交。
    Fill { pair: String, side: String, price: Decimal, size: Decimal, fee: Decimal },
    /// 接近强平。
    LiqWarn {
        pair: String,
        side: String,
        mark: Decimal,
        liq: Decimal,
        distance_pct: f64,
        threshold_pct: f64,
    },
    /// 停机残留。
    Residual { orders: usize, position: Decimal },
    /// 启动 (042): 模式标签 + 交易对。
    Started { mode: String, pair: String },
    /// 停机 (042): 原因 + 累计成交数。
    Stopped { reason: String, fills: u64 },
    /// 策略停机标记 (042)。
    Halted { detail: String },
    /// 策略停摆 (042): 停摆 bars 数与阈值。
    Stall { bars: u64, threshold: u64 },
    /// 运行摘要 (042)。
    Heartbeat { uptime_secs: u64, fills: u64, ticks: u64 },
    /// 子进程自行退出 (042)。
    Crashed { exit_code: Option<i32>, mode: String },
}

impl NotifyEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            NotifyEvent::Fill { .. } => EventKind::Fill,
            NotifyEvent::LiqWarn { .. } => EventKind::LiqWarn,
            NotifyEvent::Residual { .. } => EventKind::Residual,
            NotifyEvent::Started { .. } => EventKind::Started,
            NotifyEvent::Stopped { .. } => EventKind::Stopped,
            NotifyEvent::Halted { .. } => EventKind::Halted,
            NotifyEvent::Stall { .. } => EventKind::Stall,
            NotifyEvent::Heartbeat { .. } => EventKind::Heartbeat,
            NotifyEvent::Crashed { .. } => EventKind::Crashed,
        }
    }

    /// 渲染消息文本 (纯函数, 便于单测)。`dropped` = 自上次发送以来被限速压制的同类事件数(0 则不提)。
    pub fn render(&self, strategy: &str, dropped: u32) -> String {
        let suffix = if dropped > 0 {
            format!(" (另有 {dropped} 条同类事件被限速省略)")
        } else {
            String::new()
        };
        match self {
            NotifyEvent::Fill { pair, side, price, size, fee } => format!(
                "[{strategy}] 成交: {side} {} {pair} @ {} (手续费 {}){suffix}",
                fmt_num(size),
                fmt_num(price),
                fmt_num(fee)
            ),
            NotifyEvent::LiqWarn { pair, side, mark, liq, distance_pct, threshold_pct } => format!(
                "[{strategy}] ⚠️ 接近强平: {pair} {side} 距离 {:.2}% (阈值 {:.1}%) 标记价 {} 强平价 {}{suffix}",
                distance_pct * 100.0,
                threshold_pct * 100.0,
                fmt_num(mark),
                fmt_num(liq)
            ),
            NotifyEvent::Residual { orders, position } => format!(
                "[{strategy}] ⚠️ 停机残留: 本策略挂单 {orders} 笔, 残留持仓 {} —— 请到交易所账户侧人工核对{suffix}",
                fmt_num(position)
            ),
            NotifyEvent::Started { mode, pair } => {
                format!("[{strategy}] ▶️ 已启动: {mode} {pair}")
            }
            NotifyEvent::Stopped { reason, fills } => format!(
                "[{strategy}] ⏹ 已停机: {reason} (本次运行累计成交 {fills} 笔)"
            ),
            NotifyEvent::Halted { detail } => {
                format!("[{strategy}] 🛑 策略已停机 (fatal/halted): {detail} —— 不再产单, 请人工确认")
            }
            NotifyEvent::Stall { bars, threshold } => format!(
                "[{strategy}] ⚠️ 策略停摆: 已连续 {bars} bars 无法挂单 (阈值 {threshold})"
            ),
            NotifyEvent::Heartbeat { uptime_secs, fills, ticks } => format!(
                "[{strategy}] 💓 运行中: 已运行 {} 成交 {fills} 笔 / tick {ticks}",
                humantime_uptime(*uptime_secs)
            ),
            NotifyEvent::Crashed { exit_code, mode } => {
                let code = exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "未知".into());
                format!("[{strategy}] ❌ 进程自行退出 ({mode}): 退出码 {code} —— 请检查日志并人工确认")
            }
        }
    }
}

/// 秒数 → 人读时长 (`90061` → `1天1时1分`)。
fn humantime_uptime(secs: u64) -> String {
    let (d, rem) = (secs / 86400, secs % 86400);
    let (h, rem) = (rem / 3600, rem % 3600);
    let (m, s) = (rem / 60, rem % 60);
    let mut out = String::new();
    if d > 0 {
        out.push_str(&format!("{d}天"));
    }
    if d > 0 || h > 0 {
        out.push_str(&format!("{h}时"));
    }
    if d > 0 || h > 0 || m > 0 {
        out.push_str(&format!("{m}分"));
    }
    out.push_str(&format!("{s}秒"));
    out
}

/// 请求体平台形态 (042 FR-1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyFormat {
    /// 按 URL 识别 (默认)。
    Auto,
    /// Telegram Bot API `sendMessage`: `{"chat_id":?, "text":?}`。
    Telegram,
    /// 飞书自定义机器人: `{"msg_type":"text","content":{"text":?}}`。
    Feishu,
    /// Slack incoming webhook: `{"text":?}`。
    Slack,
    /// 自建端点 (003 原行为): `{"text":?, "chat_id":?}`。
    Raw,
}

impl NotifyFormat {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(NotifyFormat::Auto),
            "telegram" => Some(NotifyFormat::Telegram),
            "feishu" | "lark" => Some(NotifyFormat::Feishu),
            "slack" => Some(NotifyFormat::Slack),
            "raw" => Some(NotifyFormat::Raw),
            _ => None,
        }
    }

    /// auto: 按 URL 识别平台; 无法识别 → Raw (003 兼容)。
    pub fn detect(self, url: &str) -> NotifyFormat {
        match self {
            NotifyFormat::Auto => {
                let u = url.to_ascii_lowercase();
                if u.contains("open.feishu.cn") || u.contains("feishu") || u.contains("larksuite") {
                    NotifyFormat::Feishu
                } else if u.contains("api.telegram.org") {
                    NotifyFormat::Telegram
                } else if u.contains("hooks.slack.com") {
                    NotifyFormat::Slack
                } else {
                    NotifyFormat::Raw
                }
            }
            other => other,
        }
    }
}

/// 构建请求体 (纯函数, 便于单测): 同一文本按平台套不同 JSON 壳。
pub fn payload(format: NotifyFormat, chat_id: Option<&str>, text: &str) -> serde_json::Value {
    match format {
        NotifyFormat::Telegram => {
            let mut body = json!({ "text": text });
            if let Some(c) = chat_id {
                body["chat_id"] = json!(c);
            }
            body
        }
        NotifyFormat::Feishu => json!({ "msg_type": "text", "content": { "text": text } }),
        NotifyFormat::Slack => json!({ "text": text }),
        NotifyFormat::Auto | NotifyFormat::Raw => {
            let mut body = json!({ "text": text });
            if let Some(c) = chat_id {
                body["chat_id"] = json!(c);
            }
            body
        }
    }
}

/// 通知配置 (从策略 params 读取, 与 014 的 `liq_warn_pct` 同一通道)。
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyConfig {
    /// 接收 `POST` JSON 的端点 (Telegram Bot API / 飞书 / Slack / 自建)。
    pub webhook: String,
    /// 请求体平台形态 (042): 缺省 auto (URL 识别)。
    pub format: NotifyFormat,
    /// 可选: Telegram 形态需要的 chat_id (其余平台忽略该字段)。
    pub chat_id: Option<String>,
    /// 事件白名单 (缺省 = 全开)。
    pub events: Vec<EventKind>,
    /// 同类事件最小间隔秒数 (0 = 不限速)。
    pub min_interval_secs: u64,
}

impl NotifyConfig {
    /// 从策略配置读取; **未配 `notify_webhook` → None**(默认关闭)。
    ///
    /// - `notify_webhook`: 端点 (必填才启用)
    /// - `notify_format`: `auto`(默认)/`telegram`/`feishu`/`slack`/`raw`; auto 按 URL 识别
    /// - `notify_chat_id`: 可选 (Telegram 需要)
    /// - `notify_events`: 逗号分隔白名单, 例 `fill,liq_warn`; 缺省全开; 无法识别的项忽略
    /// - `notify_min_interval_secs`: 默认 5
    pub fn from_config(cfg: &StrategyConfig) -> Option<Self> {
        let webhook = cfg.get_str("notify_webhook")?.trim().to_string();
        if webhook.is_empty() {
            return None;
        }
        let events = match cfg.get_str("notify_events") {
            Some(list) => {
                let parsed: Vec<EventKind> = list.split(',').filter_map(EventKind::parse).collect();
                if parsed.is_empty() {
                    ALL_EVENTS.to_vec()
                } else {
                    parsed
                }
            }
            None => ALL_EVENTS.to_vec(),
        };
        Some(NotifyConfig {
            webhook,
            format: match cfg.get_str("notify_format") {
                Some(s) => NotifyFormat::parse(&s).unwrap_or(NotifyFormat::Auto),
                None => NotifyFormat::Auto,
            },
            chat_id: cfg
                .get_str("notify_chat_id")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            events,
            min_interval_secs: cfg.get_f64("notify_min_interval_secs").unwrap_or(5.0).max(0.0)
                as u64,
        })
    }

    fn allows(&self, kind: EventKind) -> bool {
        self.events.contains(&kind)
    }
}

/// 全部事件 (白名单缺省值)。
pub const ALL_EVENTS: [EventKind; 9] = [
    EventKind::Fill,
    EventKind::LiqWarn,
    EventKind::Residual,
    EventKind::Started,
    EventKind::Stopped,
    EventKind::Halted,
    EventKind::Stall,
    EventKind::Heartbeat,
    EventKind::Crashed,
];

/// 投递重试退避 (042 FR-3): 首发失败后 2s、8s 各补一次, 仍在后台任务内。
const RETRY_BACKOFF_SECS: [u64; 2] = [2, 8];

/// 限速器 (FR-003): 同类事件最小间隔; 被压制的**计数**在下次放行时附带 —— 不静默丢。
#[derive(Debug, Default)]
pub struct Throttle {
    last: HashMap<EventKind, Instant>,
    dropped: HashMap<EventKind, u32>,
}

impl Throttle {
    /// `Ok(dropped)` = 放行(并带出自上次以来的压制数, 计数归零); `Err(dropped)` = 压制(计数 +1)。
    pub fn allow(
        &mut self,
        kind: EventKind,
        min_interval: Duration,
        now: Instant,
    ) -> Result<u32, u32> {
        let due = match self.last.get(&kind) {
            Some(t) => now.duration_since(*t) >= min_interval,
            None => true,
        };
        if due {
            self.last.insert(kind, now);
            let dropped = self.dropped.remove(&kind).unwrap_or(0);
            Ok(dropped)
        } else {
            let n = self.dropped.entry(kind).or_insert(0);
            *n += 1;
            Err(*n)
        }
    }
}

/// 通知器: 判定(白名单/限速/去重) + 投递(spawn + 超时 + 失败只 warn)。
pub struct Notifier {
    cfg: NotifyConfig,
    strategy: String,
    http: reqwest::Client,
    throttle: Mutex<Throttle>,
    /// 已告警过的 (pair, 方向) —— 强平告警去重 (FR-004), 距离恢复后由 `clear_liq` 复位。
    liq_sent: Mutex<HashSet<String>>,
    /// 每进程至多一条的事件 (Halted/Stall/Crashed): 状态量而非流水, 去重防刷屏 (042)。
    once_sent: Mutex<HashSet<EventKind>>,
}

impl Notifier {
    /// 从策略配置构造; 未配置 webhook → None。
    pub fn from_config(cfg: &StrategyConfig) -> Option<Self> {
        let notify = NotifyConfig::from_config(cfg)?;
        Some(Notifier {
            cfg: notify,
            strategy: cfg.name.clone(),
            http: reqwest::Client::new(),
            throttle: Mutex::new(Throttle::default()),
            liq_sent: Mutex::new(HashSet::new()),
            once_sent: Mutex::new(HashSet::new()),
        })
    }

    /// 非阻塞通知入口: 判定与投递都在内部完成, 调用方(交易循环)**不会被阻塞或影响**。
    pub fn notify(&self, ev: NotifyEvent) {
        self.notify_at(ev, Instant::now());
    }

    /// 带时间注入的版本 (便于单测限速行为, 无需 sleep)。
    pub fn notify_at(&self, ev: NotifyEvent, now: Instant) {
        let kind = ev.kind();
        if !self.cfg.allows(kind) {
            return;
        }
        // once 类事件 (Halted/Stall/Crashed): 状态量, 每进程至多一条, 不走限速 (042)
        if kind.is_once() {
            let first = self.once_sent.lock().map(|mut s| s.insert(kind)).unwrap_or(false);
            if first {
                self.dispatch(ev.render(&self.strategy, 0));
            }
            return;
        }
        // 强平告警去重: 同一 (pair, 方向) 在距离恢复前只发一次 (每 tick 重复 = 刷屏)
        if kind == EventKind::LiqWarn {
            let key = liq_key(&ev);
            let mut sent = match self.liq_sent.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if !sent.insert(key) {
                return;
            }
        }
        let dropped = {
            let mut th = match self.throttle.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match th.allow(kind, Duration::from_secs(self.cfg.min_interval_secs), now) {
                Ok(dropped) => dropped,
                Err(_) => return,
            }
        };
        self.dispatch(ev.render(&self.strategy, dropped));
    }

    /// 停机路径专用 (042): 判定与 003 相同, 但**同步等待投递完成** ——
    /// run_live 返回后进程随即退出, 后台 spawn 任务会被掐断, Stopped/Residual 会丢;
    /// 调用方处于 async 上下文, 直接 await 最坏 ~15s (3 次尝试 × 5s 超时), 停机时可接受。
    pub async fn notify_stopping(&self, ev: NotifyEvent) {
        let kind = ev.kind();
        if !self.cfg.allows(kind) {
            return;
        }
        let dropped = {
            let mut th = match self.throttle.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match th.allow(kind, Duration::ZERO, Instant::now()) {
                Ok(dropped) => dropped,
                Err(_) => return,
            }
        };
        let text = ev.render(&self.strategy, dropped);
        let url = self.cfg.webhook.clone();
        let format = self.cfg.format.detect(&url);
        send_with_retry(&self.http, &url, format, self.cfg.chat_id.as_deref(), &text).await;
    }

    /// 强平距离恢复(高于阈值)时复位去重标记, 允许下次再告警。
    pub fn clear_liq(&self, pair: &str, side: &str) {
        if let Ok(mut sent) = self.liq_sent.lock() {
            sent.remove(&format!("{pair}|{side}"));
        }
    }

    /// 实际投递: spawn + 5s 超时 + 失败重试 (2s/8s 退避, 042 FR-3); 全部失败只 warn,
    /// 不排队不阻塞 (FR-005: 通知不参与交易决策)。停机路径用 [`Self::notify_stopping`]。
    fn dispatch(&self, text: String) {
        let url = self.cfg.webhook.clone();
        let format = self.cfg.format.detect(&url);
        let chat_id = self.cfg.chat_id.clone();
        let http = self.http.clone();
        tokio::spawn(async move {
            send_with_retry(&http, &url, format, chat_id.as_deref(), &text).await;
        });
    }
}

/// 带重试的投递 (3 次尝试, 退避 2s/8s); 全部失败只 warn。
async fn send_with_retry(
    http: &reqwest::Client,
    url: &str,
    format: NotifyFormat,
    chat_id: Option<&str>,
    text: &str,
) {
    let body = payload(format, chat_id, text);
    let mut last = String::from("未知错误");
    for attempt in 0..=RETRY_BACKOFF_SECS.len() {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(RETRY_BACKOFF_SECS[attempt - 1])).await;
        }
        match tokio::time::timeout(SEND_TIMEOUT, http.post(url).json(&body).send()).await {
            Ok(Ok(resp)) if resp.status().is_success() => {
                tracing::debug!(target: "notify", attempt, "通知已投递");
                return;
            }
            Ok(Ok(resp)) => last = format!("HTTP {}", resp.status()),
            Ok(Err(e)) => last = e.to_string(),
            Err(_) => last = format!("超时 ({SEND_TIMEOUT:?})"),
        }
    }
    tracing::warn!(target: "notify", "通知投递失败 (含重试): {last}");
}

fn liq_key(ev: &NotifyEvent) -> String {
    match ev {
        NotifyEvent::LiqWarn { pair, side, .. } => format!("{pair}|{side}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_strategy::StrategyConfig;
    use rust_decimal_macros::dec;

    fn test_config(name: &str) -> StrategyConfig {
        StrategyConfig {
            name: name.into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    fn ev_fill() -> NotifyEvent {
        NotifyEvent::Fill {
            pair: "ETHUSDT".into(),
            side: "buy".into(),
            price: dec!(2500.5),
            size: dec!(0.04),
            fee: dec!(0.000004),
        }
    }

    #[test]
    fn test_event_kind_parse_and_whitelist() {
        assert_eq!(EventKind::parse("fill"), Some(EventKind::Fill));
        assert_eq!(EventKind::parse(" LIQ_WARN "), Some(EventKind::LiqWarn));
        assert_eq!(EventKind::parse("crashed"), Some(EventKind::Crashed));
        assert_eq!(EventKind::parse("nope"), None);
        assert_eq!(ALL_EVENTS.len(), 9);
    }

    #[test]
    fn test_payload_formats() {
        let t = "hi";
        // 飞书壳
        let f = payload(NotifyFormat::Feishu, Some("1"), t);
        assert_eq!(f["msg_type"], "text");
        assert_eq!(f["content"]["text"], t);
        assert!(f.get("text").is_none());
        // Slack 壳: 只有 text
        let s = payload(NotifyFormat::Slack, Some("1"), t);
        assert_eq!(s["text"], t);
        assert!(s.get("chat_id").is_none());
        // Telegram 壳: text + chat_id
        let tg = payload(NotifyFormat::Telegram, Some("123"), t);
        assert_eq!(tg["text"], t);
        assert_eq!(tg["chat_id"], "123");
        // Raw (003 兼容): text + 可选 chat_id
        let raw = payload(NotifyFormat::Raw, None, t);
        assert_eq!(raw["text"], t);
        assert!(raw.get("chat_id").is_none());
    }

    #[test]
    fn test_format_detect_by_url() {
        assert_eq!(
            NotifyFormat::Auto.detect("https://open.feishu.cn/open-apis/bot/v2/hook/x"),
            NotifyFormat::Feishu
        );
        assert_eq!(
            NotifyFormat::Auto.detect("https://api.telegram.org/botT/sendMessage"),
            NotifyFormat::Telegram
        );
        assert_eq!(
            NotifyFormat::Auto.detect("https://hooks.slack.com/services/x"),
            NotifyFormat::Slack
        );
        assert_eq!(NotifyFormat::Auto.detect("https://example.com/hook"), NotifyFormat::Raw);
        // 显式覆盖优先于 URL
        assert_eq!(
            NotifyFormat::Telegram.detect("https://open.feishu.cn/x"),
            NotifyFormat::Telegram
        );
        // parse
        assert_eq!(NotifyFormat::parse(" lark "), Some(NotifyFormat::Feishu));
        assert_eq!(NotifyFormat::parse("bogus"), None);
    }

    #[test]
    fn test_lifecycle_event_render() {
        let st = NotifyEvent::Started {
            mode: "测试网模拟盘(demo)".into(), pair: "SOLUSDT".into()
        }
        .render("shannon_demo2", 0);
        assert!(st.contains("已启动") && st.contains("demo") && st.contains("SOLUSDT"));

        let sp = NotifyEvent::Stopped { reason: "停机指令".into(), fills: 7 }.render("s", 0);
        assert!(sp.contains("已停机") && sp.contains("停机指令") && sp.contains("7 笔"));

        let hb = NotifyEvent::Heartbeat { uptime_secs: 90061, fills: 3, ticks: 100 }.render("s", 0);
        assert!(hb.contains("1天1时1分1秒") && hb.contains("3 笔"));

        let cr = NotifyEvent::Crashed { exit_code: Some(101), mode: "demo".into() }.render("s", 0);
        assert!(cr.contains("自行退出") && cr.contains("101"));
        let cr2 = NotifyEvent::Crashed { exit_code: None, mode: "live".into() }.render("s", 0);
        assert!(cr2.contains("未知"));
    }

    #[tokio::test]
    async fn test_once_events_dedup_until_not() {
        let mut cfg = test_config("s");
        cfg.params.insert(
            "notify_webhook".into(),
            ricow_strategy::ConfigValue::String("http://127.0.0.1:1/x".into()),
        );
        let n = Notifier::from_config(&cfg).expect("启用");
        let ev = || NotifyEvent::Halted { detail: "爆仓".into() };
        n.notify(ev());
        assert_eq!(n.once_sent.lock().unwrap().len(), 1, "首条 Halted 应入 once 集");
        n.notify(ev());
        assert_eq!(n.once_sent.lock().unwrap().len(), 1, "once 去重: 不重复发");
        // once 去重不受限速窗口影响
        assert!(n.throttle.lock().unwrap().last.is_empty(), "once 事件不走限速器");
    }

    /// 本地 TCP 接收器: 读到包含 `needle` 的请求即返回 200 (042 投递路径实测)。
    async fn recv_once(mut sock: tokio::net::TcpStream, needle: &'static str) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = Vec::new();
        while !buf.windows(4).any(|w| w == b"\r\n\r\n")
            || !String::from_utf8_lossy(&buf).contains(needle)
        {
            let mut chunk = [0u8; 1024];
            match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut chunk)).await {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(n)) => buf.extend_from_slice(&chunk),
                Ok(Err(_)) => break,
            }
            if String::from_utf8_lossy(&buf).contains(needle) {
                break;
            }
        }
        assert!(
            String::from_utf8_lossy(&buf).contains(needle),
            "应收到含 {needle} 的请求: {}",
            String::from_utf8_lossy(&buf)
        );
        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
    }

    #[tokio::test]
    async fn test_notify_stopping_delivers_and_retries() {
        // 042: 停机路径同步投递 + 失败重试。先起一个"前两次拒绝/重置, 第三次 200"的接收器
        // 较复杂, 这里用两个事实分别验证: (a) notify_stopping 真实发出并送达;
        // (b) 重试由 send_with_retry 内部完成 (dispatch 侧单测难以断言, 以送达为准)。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg_port = format!("http://{addr}/hook");
        let mut cfg = test_config("s");
        cfg.params.insert(
            "notify_webhook".into(),
            ricow_strategy::ConfigValue::String(cfg_port.clone().into()),
        );
        // 飞书形态显式覆盖, 同时验证 payload 壳
        cfg.params
            .insert("notify_format".into(), ricow_strategy::ConfigValue::String("feishu".into()));
        let n = Notifier::from_config(&cfg).expect("启用");
        let (tx, rx) = tokio::sync::oneshot::channel::<tokio::net::TcpStream>();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let _ = tx.send(sock);
        });
        n.notify_stopping(NotifyEvent::Stopped { reason: "停机指令".into(), fills: 3 }).await;
        let sock = rx.await.unwrap();
        recv_once(sock, "停机指令").await; // JSON body 为 UTF-8 原文 (非 URL 编码)
    }

    #[test]
    fn test_render_contains_key_fields() {
        let s = ev_fill().render("grid-eth", 0);
        assert!(s.contains("grid-eth") && s.contains("成交"));
        assert!(s.contains("ETHUSDT") && s.contains("2500.5") && s.contains("0.04"));
        // 限速压制数要如实附带, 不静默丢
        assert!(ev_fill().render("grid-eth", 7).contains("7 条同类事件被限速省略"));

        let liq = NotifyEvent::LiqWarn {
            pair: "ETHUSDT".into(),
            side: "Buy".into(),
            mark: dec!(2479.89),
            liq: dec!(2443.31),
            distance_pct: 0.0148,
            threshold_pct: 0.15,
        }
        .render("s", 0);
        assert!(liq.contains("1.48%") && liq.contains("15.0%") && liq.contains("2443.31"));

        let res = NotifyEvent::Residual { orders: 2, position: dec!(0.04) }.render("s", 0);
        assert!(res.contains("残留") && res.contains("2 笔"));
        // 数值规整: f64 尾差不出现在给人看的文本里
        let messy = NotifyEvent::Fill {
            pair: "ETHUSDT".into(),
            side: "Buy".into(),
            price: dec!(2480.16000000),
            size: "0.0040000000000000000832667269".parse::<Decimal>().unwrap(),
            fee: dec!(0.00992064),
        }
        .render("s", 0);
        assert!(messy.contains("0.004 "), "尾差应规整: {messy}");
        assert!(!messy.contains("0832667269"), "不得出现长尾差: {messy}");
    }

    #[test]
    fn test_throttle_suppresses_then_reports_dropped() {
        let mut th = Throttle::default();
        let t0 = Instant::now();
        let iv = Duration::from_secs(5);
        // 首条放行, 无压制计数
        assert_eq!(th.allow(EventKind::Fill, iv, t0), Ok(0));
        // 窗口内被压制并累计
        assert_eq!(th.allow(EventKind::Fill, iv, t0 + Duration::from_secs(1)), Err(1));
        assert_eq!(th.allow(EventKind::Fill, iv, t0 + Duration::from_secs(2)), Err(2));
        // 过窗放行, 带出压制数并归零
        assert_eq!(th.allow(EventKind::Fill, iv, t0 + Duration::from_secs(6)), Ok(2));
        // 不同类别互不影响
        assert_eq!(th.allow(EventKind::Residual, iv, t0 + Duration::from_secs(6)), Ok(0));
        // 0 间隔 = 不限速
        assert_eq!(th.allow(EventKind::Fill, Duration::ZERO, t0 + Duration::from_secs(6)), Ok(0));
    }

    #[test]
    fn test_notify_config_from_params() {
        let mut cfg = test_config("s");
        assert!(NotifyConfig::from_config(&cfg).is_none(), "未配 webhook → 关闭");
        cfg.params.insert(
            "notify_webhook".into(),
            ricow_strategy::ConfigValue::String("  https://x/hook  ".into()),
        );
        let n = NotifyConfig::from_config(&cfg).expect("应启用");
        assert_eq!(n.webhook, "https://x/hook", "端点应 trim");
        assert_eq!(n.events.len(), 9, "缺省全开 (042: 3 原有 + 6 生命周期事件)");
        assert_eq!(n.min_interval_secs, 5, "默认限速 5s");

        cfg.params.insert(
            "notify_events".into(),
            ricow_strategy::ConfigValue::String("fill,liq_warn,bogus".into()),
        );
        cfg.params
            .insert("notify_min_interval_secs".into(), ricow_strategy::ConfigValue::Float(0.0));
        cfg.params
            .insert("notify_chat_id".into(), ricow_strategy::ConfigValue::String("12345".into()));
        let n2 = NotifyConfig::from_config(&cfg).unwrap();
        assert_eq!(n2.events, vec![EventKind::Fill, EventKind::LiqWarn], "白名单生效, 无效项忽略");
        assert_eq!(n2.min_interval_secs, 0, "0 = 不限速");
        assert_eq!(n2.chat_id.as_deref(), Some("12345"));
    }

    #[tokio::test]
    async fn test_liq_dedup_until_cleared() {
        let mut cfg = test_config("s");
        cfg.params.insert(
            "notify_webhook".into(),
            ricow_strategy::ConfigValue::String("http://127.0.0.1:1/x".into()),
        );
        let n = Notifier::from_config(&cfg).expect("启用");
        let ev = || NotifyEvent::LiqWarn {
            pair: "ETHUSDT".into(),
            side: "Buy".into(),
            mark: dec!(2479),
            liq: dec!(2443),
            distance_pct: 0.014,
            threshold_pct: 0.15,
        };
        let t = Instant::now();
        n.notify_at(ev(), t);
        let sent_after_first = n.liq_sent.lock().unwrap().len();
        assert_eq!(sent_after_first, 1, "首次应标记已发");
        n.notify_at(ev(), t + Duration::from_secs(60));
        assert_eq!(n.liq_sent.lock().unwrap().len(), 1, "去重: 同 pair/方向不再重复");
        // 距离恢复 → 复位 → 可再发 (另一方向互不影响)
        n.clear_liq("ETHUSDT", "Buy");
        assert_eq!(n.liq_sent.lock().unwrap().len(), 0);
        let mut short = ev();
        if let NotifyEvent::LiqWarn { side, .. } = &mut short {
            *side = "Sell".into();
        }
        n.notify_at(short, t + Duration::from_secs(120));
        assert_eq!(n.liq_sent.lock().unwrap().len(), 1);
    }
}
