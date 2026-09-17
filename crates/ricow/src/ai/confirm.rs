//! 对话内确认块的会话状态机 (019 R3, 2026-09-16)。
//!
//! # 安全模型
//! LLM **没有任何写实工具调用面**(工具注册表里没有 deploy/start_demo)。写实流程拆成两半:
//! 1. 模型调 L1 虚拟工具 `request_write_confirmation`: 只校验前提 + 渲染确认块 + 在本状态机
//!    登记一条 [PendingAction], **不落盘、不起进程**;
//! 2. 用户在 `ricow ai` 交互会话里**当场逐字输入**确认短语, 由 REPL **宿主进程**匹配后执行内核。
//!
//! 因此模型输出永远不能触发写实: 它拿不到执行权, 也不持有 pending 的写入端之外的任何能力;
//! 短语匹配只认真实用户输入行(见 [crate::commands::is_explicit_confirmation])。
//!
//! # 开放范围(R4 全对话化)
//! 七类动作全部走同一状态机: 落盘部署 / 启动测试网 demo / 首次实盘风险确认 /
//! 启动实盘 / 停止测试网 / 停止实盘 / 平仓停止实盘。
//!
//! 门禁一条不少: 短语逐字(实盘与终端 `ricow start --live` 同一句)、TTL 15 分钟、
//! 跨动作短语互不放行、tty 门禁(单次模式与管道不登记 pending)。
//! 实盘三判据本身不在此文件 —— 由宿主执行时经 `ctrl::live_preflight` 原样跑一遍。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// pending 有效期 = 引擎 preview TTL, **同一个常量**(不各写一份 15 分钟)。
///
/// 待确认动作里多半带着 preview_id, 批准时引擎会再校验预览是否过期; 两处若各写一个数,
/// 一旦漂移就会出现"短语逐字输对了, 却被告知预览已过期"这种自相矛盾的拒绝。
pub const PENDING_TTL: Duration = Duration::from_secs(ricow_engine::PREVIEW_TTL_SECS as u64);

/// 会话内待确认动作(工具登记 → 用户逐字确认 → 宿主执行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub kind: ActionKind,
    /// 策略名(确认短语与执行都要用)。
    pub name: String,
    /// 落盘动作的 preview_id(start_demo 为 None)。
    pub preview_id: Option<String>,
    /// 落盘动作是否走 **FR-044 受控覆盖**(仅 deploy 有意义; 其余动作恒为 false)。
    ///
    /// 为 true 时宿主会在覆盖前备份旧脚本; 该标记只能由用户逐字确认的 pending 携带,
    /// 模型无法直接把它变成一次落盘 —— 它仍要过短语门禁。
    pub replace: bool,
    created_at: Instant,
}

/// 动作种类(019 R4: 七类, 覆盖策略全生命周期)。
///
/// 所有动作都只是**待确认登记**: 模型无执行权, 短语由用户本人在交互终端逐字输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// 批准 preview 并落盘 strategies/<name>.{toml,lua}。
    Deploy,
    /// 以 --demo 启动已部署策略(币安测试网, 真实下单/撤单但无真实资金)。
    StartDemo,
    /// 首次实盘风险确认(展示 RISK_DISCLOSURE 全文 → 写 risk_ack.json, 一次长期有效)。
    AckRisk,
    /// 以 --live 启动已部署策略(真实资金; 三判据 + 双条件一条不少)。
    StartLive,
    /// 停止测试网 demo 实例(交易所侧撤单清理, 不平仓)。
    StopDemo,
    /// 停止实盘实例(交易所侧撤单清理, 不平仓)。
    StopLive,
    /// 停止实盘实例并市价平掉策略持仓(不可逆)。
    CloseLive,
}

impl ActionKind {
    /// 动作的中文标签(确认块/提示用)。
    pub fn label(self) -> &'static str {
        match self {
            ActionKind::Deploy => "落盘部署",
            ActionKind::StartDemo => "启动测试网 demo",
            ActionKind::AckRisk => "首次实盘风险确认",
            ActionKind::StartLive => "启动实盘",
            ActionKind::StopDemo => "停止测试网 demo",
            ActionKind::StopLive => "停止实盘",
            ActionKind::CloseLive => "平仓停止实盘",
        }
    }
}

impl PendingAction {
    /// 默认路径: 换个新名落盘(同名已存在时由 `prepare` 拒绝)。
    pub fn new_deploy(name: impl Into<String>, preview_id: impl Into<String>) -> Self {
        Self::deploy_inner(name, preview_id, false)
    }

    /// FR-044 受控覆盖: 覆盖同名已部署策略 —— 宿主要在写新脚本前先备份旧脚本。
    pub fn new_deploy_replace(name: impl Into<String>, preview_id: impl Into<String>) -> Self {
        Self::deploy_inner(name, preview_id, true)
    }

    fn deploy_inner(name: impl Into<String>, preview_id: impl Into<String>, replace: bool) -> Self {
        Self {
            kind: ActionKind::Deploy,
            name: name.into(),
            preview_id: Some(preview_id.into()),
            replace,
            created_at: Instant::now(),
        }
    }

    pub fn new_start_demo(name: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::StartDemo,
            name: name.into(),
            preview_id: None,
            replace: false,
            created_at: Instant::now(),
        }
    }

    pub fn new_ack_risk() -> Self {
        Self {
            kind: ActionKind::AckRisk,
            name: String::new(),
            preview_id: None,
            replace: false,
            created_at: Instant::now(),
        }
    }

    pub fn new_start_live(name: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::StartLive,
            name: name.into(),
            preview_id: None,
            replace: false,
            created_at: Instant::now(),
        }
    }

    pub fn new_stop_demo(name: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::StopDemo,
            name: name.into(),
            preview_id: None,
            replace: false,
            created_at: Instant::now(),
        }
    }

    pub fn new_stop_live(name: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::StopLive,
            name: name.into(),
            preview_id: None,
            replace: false,
            created_at: Instant::now(),
        }
    }

    pub fn new_close_live(name: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::CloseLive,
            name: name.into(),
            preview_id: None,
            replace: false,
            created_at: Instant::now(),
        }
    }

    /// 用户需逐字输入的确认短语。
    ///
    /// deploy 与终端 `ricow approve` 的 `确认部署 <name>`、实盘与终端 `ricow start --live`
    /// 的 `确认实盘 <name>` **逐字一致**(同一门禁体验); 其余动作各有独立短语, 互不放行。
    ///
    /// **受控覆盖(FR-044)另给一句 `确认覆盖 <name>`**: 覆盖是破坏性动作, 不能让"确认部署"
    /// 的肌肉记忆顺手把已部署策略换掉 —— 想覆盖就得换一句话输。
    pub fn expected_phrase(&self) -> String {
        match self.kind {
            ActionKind::Deploy if self.replace => format!("确认覆盖 {}", self.name),
            ActionKind::Deploy => format!("确认部署 {}", self.name),
            ActionKind::StartDemo => format!("确认启动测试网 {}", self.name),
            ActionKind::AckRisk => "确认风险".to_string(),
            ActionKind::StartLive => format!("确认实盘 {}", self.name),
            ActionKind::StopDemo => format!("确认停止测试网 {}", self.name),
            ActionKind::StopLive => format!("确认停止实盘 {}", self.name),
            ActionKind::CloseLive => format!("确认平仓停止 {}", self.name),
        }
    }

    /// 是否已过期(now 可注入, 便于单测)。
    pub fn is_expired_at(&self, now: Instant) -> bool {
        now.duration_since(self.created_at) > PENDING_TTL
    }

    pub fn is_expired(&self) -> bool {
        self.is_expired_at(Instant::now())
    }
}

/// REPL 与工具闭包共享的 pending 句柄(同一把锁, 任意时刻最多一条待确认)。
pub type PendingSlot = Arc<Mutex<Option<PendingAction>>>;

pub fn new_slot() -> PendingSlot {
    Arc::new(Mutex::new(None))
}

/// 用户一行输入相对当前 pending 的意图(纯函数, 短语判定复用 commands 同源规则)。
#[derive(Debug, PartialEq, Eq)]
pub enum UserIntent {
    /// 逐字确认短语命中。
    Confirm,
    /// 用户显式放弃(拒绝 / reject)。
    Reject,
    /// 其它输入: pending 保留, 该行按普通提问送模型。
    Other,
}

/// 分类一行输入(不消费状态; 是否过期/清空由调用方决定)。
///
/// 裸 y/yes/ok/回车一律不算确认(与 approve 门禁同一函数), 防止肌肉记忆误触。
pub fn classify_user_line(line: &str, pending: &PendingAction) -> UserIntent {
    let expected = pending.expected_phrase();
    if crate::commands::is_explicit_confirmation(line, &expected) {
        UserIntent::Confirm
    } else if crate::commands::is_explicit_rejection(line) {
        UserIntent::Reject
    } else {
        UserIntent::Other
    }
}

/// REPL 读到一行输入后, 对会话 pending 的处置(锁内完成 take/回填, 避免 TOCTOU)。
#[derive(Debug, PartialEq, Eq)]
pub enum LineDisposition {
    /// 逐字短语命中 → 宿主应执行(动作已从 slot 取出)。
    Confirm(PendingAction),
    /// 用户显式拒绝 → 作废(动作已取出; deploy 由调用方连 preview 一起 reject)。
    Reject(PendingAction),
    /// pending 已过期 → 提示作废; 当前行可继续当普通提问。
    Expired(PendingAction),
    /// 输入与 pending 无关: pending 已**原样留在 slot**, 当前行当普通提问送模型。
    Other,
    /// 没有待确认动作, 当前行直接送模型。
    NoPending,
}

/// 消费一行用户输入, 推进确认状态机(供 REPL 宿主调用; 纯会话逻辑, 不碰引擎/文件)。
pub async fn consume_line(slot: &PendingSlot, line: &str) -> LineDisposition {
    let mut guard = slot.lock().await;
    match guard.take() {
        None => LineDisposition::NoPending,
        Some(action) if action.is_expired() => LineDisposition::Expired(action),
        Some(action) => match classify_user_line(line, &action) {
            UserIntent::Confirm => LineDisposition::Confirm(action),
            UserIntent::Reject => LineDisposition::Reject(action),
            UserIntent::Other => {
                // 普通提问不打断 pending: 原样放回
                *guard = Some(action);
                LineDisposition::Other
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expected_phrases_match_terminal_gates() {
        let deploy = PendingAction::new_deploy("eth-grid-1", "pv-1");
        assert_eq!(deploy.expected_phrase(), "确认部署 eth-grid-1");
        let demo = PendingAction::new_start_demo("eth-grid-1");
        assert_eq!(demo.expected_phrase(), "确认启动测试网 eth-grid-1");
        // 实盘短语与终端 `ricow start --live` 逐字一致(同一门禁体验)
        let live = PendingAction::new_start_live("eth-grid-1");
        assert_eq!(live.expected_phrase(), "确认实盘 eth-grid-1");
        // 两种短语互不混淆
        assert_ne!(deploy.expected_phrase(), demo.expected_phrase());
    }

    /// 七类动作的短语(同名策略)必须两两不同, 否则会出现跨动作误放行。
    fn all_kinds(name: &str) -> Vec<PendingAction> {
        vec![
            PendingAction::new_deploy(name, "pv-1"),
            PendingAction::new_start_demo(name),
            PendingAction::new_ack_risk(),
            PendingAction::new_start_live(name),
            PendingAction::new_stop_demo(name),
            PendingAction::new_stop_live(name),
            PendingAction::new_close_live(name),
        ]
    }

    #[test]
    fn test_all_action_kinds_have_distinct_phrases_and_labels() {
        let actions = all_kinds("g1");
        assert_eq!(actions.len(), 7, "R4 动作全集: 七类");
        let mut phrases: Vec<String> = actions.iter().map(|a| a.expected_phrase()).collect();
        phrases.sort();
        let before = phrases.len();
        phrases.dedup();
        assert_eq!(phrases.len(), before, "确认短语必须两两不同: {phrases:?}");
        // 标签同样唯一(确认块与提示靠它区分动作)
        let mut labels: Vec<&str> = actions.iter().map(|a| a.kind.label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "动作标签必须唯一: {labels:?}");
        // 无名字的动作只有风险确认; 其余都带策略名(短语里必须出现名字)
        assert_eq!(PendingAction::new_ack_risk().expected_phrase(), "确认风险");
        for a in actions.iter().filter(|a| a.kind != ActionKind::AckRisk) {
            assert!(a.expected_phrase().contains("g1"), "短语须含策略名: {}", a.expected_phrase());
        }
    }

    #[test]
    fn test_no_cross_action_phrase_unlocks_another_action() {
        // 结构性保证: 任一动作的短语都不能放行另一个动作(七类全矩阵)
        let actions = all_kinds("g1");
        for pending in &actions {
            for other in &actions {
                if other.kind == pending.kind {
                    continue;
                }
                let intent = classify_user_line(&other.expected_phrase(), pending);
                assert_eq!(
                    intent,
                    UserIntent::Other,
                    "{} 的短语不得放行 {}",
                    other.kind.label(),
                    pending.kind.label()
                );
            }
            // 自己的短语必须命中
            assert_eq!(
                classify_user_line(&pending.expected_phrase(), pending),
                UserIntent::Confirm,
                "{} 的短语应命中自己",
                pending.kind.label()
            );
        }
    }

    #[test]
    fn test_stop_phrases_do_not_collide_with_bare_live_start() {
        // 最容易混的一对: 「确认实盘 g1」/「确认停止实盘 g1」/「确认平仓停止 g1」
        let live = PendingAction::new_start_live("g1");
        let stop = PendingAction::new_stop_live("g1");
        let close = PendingAction::new_close_live("g1");
        assert_eq!(classify_user_line(&live.expected_phrase(), &stop), UserIntent::Other);
        assert_eq!(classify_user_line(&stop.expected_phrase(), &live), UserIntent::Other);
        assert_eq!(classify_user_line(&close.expected_phrase(), &stop), UserIntent::Other);
        assert_eq!(classify_user_line(&stop.expected_phrase(), &close), UserIntent::Other);
        // 裸「确认」/「确认风险」都不算(风险确认是一次性动作, 不能顺带触发停机)
        assert_eq!(classify_user_line("确认", &close), UserIntent::Other);
        assert_eq!(
            classify_user_line(&PendingAction::new_ack_risk().expected_phrase(), &close),
            UserIntent::Other
        );
    }

    #[test]
    fn test_classify_requires_verbatim_phrase() {
        let p = PendingAction::new_deploy("g1", "pv");
        // 命中
        assert_eq!(classify_user_line("确认部署 g1", &p), UserIntent::Confirm);
        assert_eq!(classify_user_line("  确认部署 g1  \n", &p), UserIntent::Confirm);
        // 裸确认 / 错字 / 缺名字 / 无空格 → 一律 Other(零副作用, 保留 pending)
        for bad in ["y", "yes", "ok", "确认部署", "确认部署 g", "确认部署g1", "确认部署 G1", ""]
        {
            assert_eq!(classify_user_line(bad, &p), UserIntent::Other, "'{bad}' 不应命中");
        }
        // 显式拒绝
        assert_eq!(classify_user_line("拒绝", &p), UserIntent::Reject);
        assert_eq!(classify_user_line("reject", &p), UserIntent::Reject);
        // demo 短语不命中 deploy pending
        assert_eq!(
            classify_user_line("确认启动测试网 g1", &p),
            UserIntent::Other,
            "不同动作的短语不得互相放行"
        );
        // 普通提问不打断 pending
        assert_eq!(classify_user_line("帮我看看现在 ETH 多少钱", &p), UserIntent::Other);
    }

    #[test]
    fn test_ttl_expiry_with_injected_clock() {
        let p = PendingAction::new_deploy("g", "pv");
        assert!(!p.is_expired());
        assert!(!p.is_expired_at(Instant::now()));
        // 超过 15 分钟即过期
        assert!(p.is_expired_at(Instant::now() + PENDING_TTL + Duration::from_secs(1)));
        // 临界点(恰好 TTL)不算过期(用 > 而非 >=)
        assert!(!p.is_expired_at(p.created_at + PENDING_TTL));
    }

    #[test]
    fn test_demo_pending_phrase_does_not_unlock_deploy() {
        // 结构性保证: 即便用户口误, demo 短语也不能执行落盘
        let deploy = PendingAction::new_deploy("eth", "pv");
        let demo_phrase = PendingAction::new_start_demo("eth").expected_phrase();
        assert_eq!(classify_user_line(&demo_phrase, &deploy), UserIntent::Other);
    }

    /// FR-044 受控覆盖: 破坏性动作另用一句短语, 且与普通部署互不放行。
    #[test]
    fn test_replace_phrase_differs_from_plain_deploy() {
        let plain = PendingAction::new_deploy("g1", "pv");
        let over = PendingAction::new_deploy_replace("g1", "pv");
        assert_eq!(over.expected_phrase(), "确认覆盖 g1");
        assert_ne!(plain.expected_phrase(), over.expected_phrase());
        // 互不放行: 覆盖 pending 不认"确认部署", 普通 pending 也不认"确认覆盖"
        assert_eq!(classify_user_line(&plain.expected_phrase(), &over), UserIntent::Other);
        assert_eq!(classify_user_line(&over.expected_phrase(), &plain), UserIntent::Other);
        assert_eq!(classify_user_line("确认覆盖 g1", &over), UserIntent::Confirm);
        // 覆盖标记只随构造器进入, 且只有 deploy 有覆盖语义
        assert!(over.replace);
        assert!(!plain.replace);
        assert!(!PendingAction::new_start_live("g1").replace);
        assert!(!PendingAction::new_start_demo("g1").replace);
    }

    #[tokio::test]
    async fn test_consume_line_state_machine() {
        let slot = new_slot();

        // 无 pending: 任意输入直接放行给模型
        assert_eq!(consume_line(&slot, "ETH 多少钱").await, LineDisposition::NoPending);

        let action = PendingAction::new_deploy("g1", "pv-123");
        *slot.lock().await = Some(action.clone());

        // 错误短语: pending 必须原样保留(零副作用), 输入仍送模型
        assert_eq!(consume_line(&slot, "y").await, LineDisposition::Other);
        assert_eq!(slot.lock().await.clone(), Some(action.clone()), "错短语不得消费 pending");
        assert_eq!(consume_line(&slot, "确认部署 g").await, LineDisposition::Other);
        assert!(slot.lock().await.is_some(), "错字短语后 pending 仍在");

        // 普通提问同样不打断 pending
        assert_eq!(consume_line(&slot, "先帮我查下状态").await, LineDisposition::Other);
        assert!(slot.lock().await.is_some());

        // 逐字确认: pending 被取出交宿主执行, slot 清空
        match consume_line(&slot, "确认部署 g1").await {
            LineDisposition::Confirm(a) => assert_eq!(a, action),
            other => panic!("应为 Confirm, 实际 {other:?}"),
        }
        assert!(slot.lock().await.is_none(), "确认后 pending 必须清空");
    }

    #[tokio::test]
    async fn test_consume_line_reject_clears_pending() {
        let slot = new_slot();
        *slot.lock().await = Some(PendingAction::new_start_demo("g2"));
        match consume_line(&slot, "拒绝").await {
            LineDisposition::Reject(a) => {
                assert_eq!(a.kind, ActionKind::StartDemo);
                assert_eq!(a.name, "g2");
            }
            other => panic!("应为 Reject, 实际 {other:?}"),
        }
        assert!(slot.lock().await.is_none());
    }

    #[tokio::test]
    async fn test_consume_line_expired_is_surfaced_and_cleared() {
        let slot = new_slot();
        // 手工构造一条已过期 pending(created_at 私有, 同模块测试可直填)
        *slot.lock().await = Some(PendingAction {
            kind: ActionKind::Deploy,
            name: "old".into(),
            preview_id: Some("pv-old".into()),
            replace: false,
            created_at: Instant::now() - PENDING_TTL - Duration::from_secs(1),
        });
        // 即使输入恰好是确认短语, 过期也优先 → Expired(不得执行)
        match consume_line(&slot, "确认部署 old").await {
            LineDisposition::Expired(a) => assert_eq!(a.preview_id.as_deref(), Some("pv-old")),
            other => panic!("过期 pending 必须拦在确认之前, 实际 {other:?}"),
        }
        assert!(slot.lock().await.is_none());
    }
}
