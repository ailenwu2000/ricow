//! 出站通知 (003): 成交 / 接近强平 / 停机残留 → 用户自选 webhook。
//! (020 起删除"熔断"事件: 平台不再有亏损熔断。)
//!
//! **为什么是 webhook**: `specs/product.md` §四把"本地私钥 + 数据不出本机"作为支点, 因此通知不引入平台账号、
//! 不写死第三方 SDK —— 一个 `POST` JSON 就能覆盖 Telegram(`sendMessage`)/飞书/Slack/自建端点。
//!
//! **分层**: 本模块把"要不要发、发什么文本"([[`NotifyEvent::render`]] / [`Throttle`] / 去重, 全可单测)
//! 与"怎么发出去"([`Notifier::dispatch`], 一次性 HTTP)分开; 投递失败**只 warn**, 不重试、不排队 ——
//! 通知不参与交易决策, 绝不能反压策略循环 (003 FR-005 / A2)。
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
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Fill => "fill",
            EventKind::LiqWarn => "liq_warn",
            EventKind::Residual => "residual",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fill" => Some(EventKind::Fill),
            "liq_warn" => Some(EventKind::LiqWarn),
            "residual" => Some(EventKind::Residual),
            _ => None,
        }
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
}

impl NotifyEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            NotifyEvent::Fill { .. } => EventKind::Fill,
            NotifyEvent::LiqWarn { .. } => EventKind::LiqWarn,
            NotifyEvent::Residual { .. } => EventKind::Residual,
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
        }
    }
}

/// 通知配置 (从策略 params 读取, 与 014 的 `liq_warn_pct` 同一通道)。
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyConfig {
    /// 接收 `POST` JSON 的端点 (Telegram Bot API / 飞书 / Slack / 自建)。
    pub webhook: String,
    /// 可选: Telegram 形态需要的 chat_id (通用 webhook 会忽略该字段)。
    pub chat_id: Option<String>,
    /// 事件白名单 (缺省 = 四类全开)。
    pub events: Vec<EventKind>,
    /// 同类事件最小间隔秒数 (0 = 不限速)。
    pub min_interval_secs: u64,
}

impl NotifyConfig {
    /// 从策略配置读取; **未配 `notify_webhook` → None**(默认关闭)。
    ///
    /// - `notify_webhook`: 端点 (必填才启用)
    /// - `notify_chat_id`: 可选
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

/// 四类事件 (白名单缺省值)。
pub const ALL_EVENTS: [EventKind; 3] =
    [EventKind::Fill, EventKind::LiqWarn, EventKind::Residual];

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

    /// 强平距离恢复(高于阈值)时复位去重标记, 允许下次再告警。
    pub fn clear_liq(&self, pair: &str, side: &str) {
        if let Ok(mut sent) = self.liq_sent.lock() {
            sent.remove(&format!("{pair}|{side}"));
        }
    }

    /// 实际投递: spawn + 5s 超时; 失败/非 2xx 只 warn (FR-005, 不重试不排队)。
    fn dispatch(&self, text: String) {
        let url = self.cfg.webhook.clone();
        let chat_id = self.cfg.chat_id.clone();
        let http = self.http.clone();
        tokio::spawn(async move {
            let mut body = json!({ "text": text });
            if let Some(c) = chat_id {
                body["chat_id"] = json!(c);
            }
            match tokio::time::timeout(SEND_TIMEOUT, http.post(&url).json(&body).send()).await {
                Ok(Ok(resp)) if resp.status().is_success() => {
                    tracing::debug!(target: "notify", "通知已投递");
                }
                Ok(Ok(resp)) => {
                    tracing::warn!(target: "notify", "通知投递失败: HTTP {}", resp.status())
                }
                Ok(Err(e)) => tracing::warn!(target: "notify", "通知投递失败: {e}"),
                Err(_) => tracing::warn!(target: "notify", "通知投递超时 ({SEND_TIMEOUT:?})"),
            }
        });
    }
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
            risk: None,
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
        assert_eq!(EventKind::parse("nope"), None);
        assert_eq!(ALL_EVENTS.len(), 3);
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
        assert_eq!(n.events.len(), 3, "缺省全开 (成交/接近强平/停机残留; 熔断事件已于 020 删除)");
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
