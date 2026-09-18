//! 对话内确认块的会话状态机 (019 R3; 023 扩为 13 类写操作 + 双语口语确认)。
//!
//! # 安全模型
//! LLM **没有任何写实工具调用面**(工具注册表里没有 deploy/start_demo/stop_run)。写实流程拆成两半:
//! 1. 模型调 L1 虚拟工具 `request_write_confirmation`: 只校验前提 + 渲染确认块 + 在本状态机
//!    登记一条 [PendingAction], **不落盘、不起进程**;
//! 2. 用户在 `ricow ai` 交互会话里**当场回一句确认词**, 由 REPL **宿主进程**匹配后执行内核。
//!
//! 因此模型输出永远不能触发写实: 它拿不到执行权, 也不持有 pending 的写入端之外的任何能力;
//! 确认词匹配只认真实用户输入行。
//!
//! # 两套确认语义 (023 F4 / FR-022)
//!
//! | 渠道 | 确认方式 | 实现 |
//! |---|---|---|
//! | **对话**(`ricow ai` REPL) | 当前语言的**口语单词** `确认`/`确定`/`同意` · `confirm`/`confirmed` | [`is_simple_confirmation`] |
//! | **终端 CLI**(`ricow approve` / `ricow start --live` / `ricow stop --live`) | **逐字中文长短语** `确认部署 <名>` … | `PendingAction::expected_phrase` |
//!
//! `PendingAction::expected_phrase` **仅供终端渠道**(且仅测试编译): `commands/approve.rs`、`commands/ctrl.rs`
//! 的短语与门禁一行不改(用户明确要求"终端保留长短语")。对话渠道**不再使用**逐字短语。
//!
//! # 写操作全集(13 类)
//! 写操作 = 会改变磁盘内容或进程运行态的动作。因此不仅落盘/启停要确认,
//! 改参数 / 删除策略 / 实盘重启也一并纳入; 只读动作(回测、查询、预览)不经本状态机。
//!
//! 门禁一条不少: TTL 15 分钟、过期优先、单槽 pending、跨动作互不放行、
//! tty 门禁(单次模式与管道不登记 pending)。
//! 实盘三判据本身不在此文件 —— 由宿主执行时经 `ctrl::live_preflight` 原样跑一遍。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::i18n::{t, Lang};

/// pending 有效期 = 引擎 preview TTL, **同一个常量**(不各写一份 15 分钟)。
///
/// 待确认动作里多半带着 preview_id, 批准时引擎会再校验预览是否过期; 两处若各写一个数,
/// 一旦漂移就会出现"确认词输对了, 却被告知预览已过期"这种自相矛盾的拒绝。
pub const PENDING_TTL: Duration = Duration::from_secs(ricow_engine::PREVIEW_TTL_SECS as u64);

/// 会话内待确认动作(工具登记 → 用户回确认词 → 宿主执行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub kind: ActionKind,
    /// 策略名(确认块与执行都要用); 仅 [`ActionKind::AckRisk`] 为空。
    pub name: String,
    /// 落盘动作的 preview_id(其余动作恒为 None)。
    pub preview_id: Option<String>,
    /// [`ActionKind::UpdateParams`] 专用: 待写入的 `key=value` 列表(其余动作恒为空)。
    pub params: Vec<String>,
    created_at: Instant,
}

/// 写操作动作全集(023 FR-019: **13 类**)。
///
/// 所有动作都只是**待确认登记**: 模型无执行权, 由用户本人在交互终端回确认词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    // ── 写文件 ──
    /// 批准 preview 并落盘 strategies/<name>.{toml,lua}。
    Deploy,
    /// FR-044 受控覆盖: 覆盖同名已部署策略(写前先备份旧脚本, **不可逆**)。
    DeployReplace,
    /// 改 `[strategy.params]` 并落盘(写前留 `.toml.<时间戳>.bak`)。
    UpdateParams,
    /// 删除策略文件 `strategies/<name>.{toml,lua}`(**不可逆**; 保留 `logs/`)。
    DeleteStrategy,
    // ── 运行态 ──
    /// 以 Dry Run 启动已部署策略(实时行情 + 虚拟下单, 不涉资金)。
    StartDryRun,
    /// 停止 Dry Run 实例(交易所侧撤单清理, 不平仓)。
    StopDryRun,
    /// 以 --demo 启动已部署策略(币安测试网, 真实下单/撤单但无真实资金)。
    StartDemo,
    /// 停止测试网 demo 实例(交易所侧撤单清理, 不平仓)。
    StopDemo,
    // ── 实盘(真实资金) ──
    /// 首次实盘风险确认(展示 RISK_DISCLOSURE 全文 → 写 risk_ack.json, 一次长期有效)。
    AckRisk,
    /// 以 --live 启动已部署策略(真实资金; 三判据 + 双条件一条不少)。
    StartLive,
    /// 停止实盘实例(交易所侧撤单清理, 不平仓)。
    StopLive,
    /// 停止实盘实例并市价平掉策略持仓(**不可逆**)。
    CloseLive,
    /// 重启实盘实例(改参数后生效路径; 重过三判据, 真实资金)。
    RestartLive,
}

impl ActionKind {
    /// 动作标签(确认块/提示用), 随界面语言。
    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            ActionKind::Deploy => t(lang, "落盘部署", "Deploy strategy"),
            ActionKind::DeployReplace => t(lang, "覆盖部署", "Replace & deploy"),
            ActionKind::UpdateParams => t(lang, "修改参数", "Update parameters"),
            ActionKind::DeleteStrategy => t(lang, "删除策略", "Delete strategy"),
            ActionKind::StartDryRun => t(lang, "启动试跑", "Start dry run"),
            ActionKind::StopDryRun => t(lang, "停止试跑", "Stop dry run"),
            ActionKind::StartDemo => t(lang, "启动测试网", "Start on testnet"),
            ActionKind::StopDemo => t(lang, "停止测试网", "Stop on testnet"),
            ActionKind::AckRisk => t(lang, "实盘风险确认", "Accept live-trading risk"),
            ActionKind::StartLive => t(lang, "启动实盘", "Start live trading"),
            ActionKind::StopLive => t(lang, "停止实盘", "Stop live trading"),
            ActionKind::CloseLive => t(lang, "平仓停止实盘", "Close positions & stop"),
            ActionKind::RestartLive => t(lang, "重启实盘", "Restart live trading"),
        }
    }
}

impl PendingAction {
    /// 只带名字的动作(11 类)共用构造。
    fn simple(kind: ActionKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            preview_id: None,
            params: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// 默认路径: 换个新名落盘(同名已存在时由 `prepare` 拒绝并指向覆盖路径)。
    pub fn new_deploy(name: impl Into<String>, preview_id: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::Deploy,
            name: name.into(),
            preview_id: Some(preview_id.into()),
            params: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// FR-044 受控覆盖: 覆盖同名已部署策略 —— 宿主要在写新脚本前先备份旧脚本。
    pub fn new_deploy_replace(name: impl Into<String>, preview_id: impl Into<String>) -> Self {
        Self {
            kind: ActionKind::DeployReplace,
            name: name.into(),
            preview_id: Some(preview_id.into()),
            params: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// 改参数: `params` 为 `key=value` 形式, 由 `backtest::parse_param` 解析后写入。
    pub fn new_update_params(name: impl Into<String>, params: Vec<String>) -> Self {
        Self {
            kind: ActionKind::UpdateParams,
            name: name.into(),
            preview_id: None,
            params,
            created_at: Instant::now(),
        }
    }

    pub fn new_delete_strategy(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::DeleteStrategy, name)
    }

    pub fn new_start_dry_run(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::StartDryRun, name)
    }

    pub fn new_stop_dry_run(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::StopDryRun, name)
    }

    pub fn new_start_demo(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::StartDemo, name)
    }

    pub fn new_stop_demo(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::StopDemo, name)
    }

    pub fn new_start_live(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::StartLive, name)
    }

    pub fn new_stop_live(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::StopLive, name)
    }

    pub fn new_close_live(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::CloseLive, name)
    }

    pub fn new_restart_live(name: impl Into<String>) -> Self {
        Self::simple(ActionKind::RestartLive, name)
    }

    /// 首次实盘风险确认(无策略名, 与具体策略无关)。
    pub fn new_ack_risk() -> Self {
        Self::simple(ActionKind::AckRisk, "")
    }

    /// 是否 FR-044 受控覆盖(仅 [`ActionKind::DeployReplace`] 为真)。
    pub fn is_replace(&self) -> bool {
        self.kind == ActionKind::DeployReplace
    }

    /// **终端渠道**需逐字输入的确认短语(对话渠道用 [`is_simple_confirmation`])。
    ///
    /// 前七句与终端门禁**逐字一致**: deploy ↔ `ricow approve` 的 `确认部署 <name>`,
    /// deploy-replace ↔ `确认覆盖 <name>`, live ↔ `ricow start --live` 的 `确认实盘 <name>`, 等等。
    ///
    /// 后六句(试跑/改参数/删除/重启实盘)在终端**没有对应命令** —— 它们只有对话入口,
    /// 这里的短语仅用于确认块展示与结构性测试(唯一性/互不放行), 不构成终端门禁。
    ///
    /// 因此生产代码**不再调用**本函数: 对话渠道走口语词, 终端渠道用 `approve.rs` / `ctrl.rs`
    /// 自己的字面量。保留它是为了让漂移闸单测能逐字比对两套短语, 故标注为测试专用。
    #[cfg(test)]
    pub fn expected_phrase(&self) -> String {
        match self.kind {
            ActionKind::Deploy => format!("确认部署 {}", self.name),
            ActionKind::DeployReplace => format!("确认覆盖 {}", self.name),
            ActionKind::UpdateParams => format!("确认改参数 {}", self.name),
            ActionKind::DeleteStrategy => format!("确认删除策略 {}", self.name),
            ActionKind::StartDryRun => format!("确认试跑 {}", self.name),
            ActionKind::StopDryRun => format!("确认停止试跑 {}", self.name),
            ActionKind::StartDemo => format!("确认启动测试网 {}", self.name),
            ActionKind::StopDemo => format!("确认停止测试网 {}", self.name),
            ActionKind::AckRisk => "确认风险".to_string(),
            ActionKind::StartLive => format!("确认实盘 {}", self.name),
            ActionKind::StopLive => format!("确认停止实盘 {}", self.name),
            ActionKind::CloseLive => format!("确认平仓停止 {}", self.name),
            ActionKind::RestartLive => format!("确认重启实盘 {}", self.name),
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

/// 输入是否命中该语言的**口语确认词**(023 FR-022)。
///
/// - `zh` → `确认` / `确定` / `同意`
/// - `en` → `confirm` / `confirmed`(忽略大小写)
///
/// **只认当前语言**: `en` 模式下「确认」不放行, 反之亦然 —— 避免双语混杂削弱门槛。
/// 刻意**不含** `y` / `yes` / `ok` / 空行(保留防肌肉记忆误触的最低门槛)。
pub fn is_simple_confirmation(input: &str, lang: Lang) -> bool {
    match lang {
        Lang::Zh => is_one_of(input, &["确认", "确定", "同意"]),
        Lang::En => is_one_of(input, &["confirm", "confirmed"]),
    }
}

/// 输入是否命中该语言的**口语拒绝词**(023 FR-022/FR-023)。
///
/// - `zh` → `拒绝` / `取消` / `放弃`
/// - `en` → `reject` / `cancel` / `abort`(忽略大小写)
pub fn is_simple_rejection(input: &str, lang: Lang) -> bool {
    match lang {
        Lang::Zh => is_one_of(input, &["拒绝", "取消", "放弃"]),
        Lang::En => is_one_of(input, &["reject", "cancel", "abort"]),
    }
}

/// 全行匹配(trim 后), 忽略 ASCII 大小写 —— 中文词不含 ASCII 字母, 该比较等价于全等。
fn is_one_of(input: &str, words: &[&str]) -> bool {
    let s = input.trim();
    !s.is_empty() && words.iter().any(|w| s.eq_ignore_ascii_case(w))
}

/// 用户一行输入相对当前 pending 的意图。
///
/// 对话渠道的确认是**口语词**, 与 pending 携带的动作种类无关(单槽在握时只有"确认/拒绝/其它"三态);
/// 因此本函数不需要 pending 参数 —— 跨动作短语互不放行由终端渠道的
/// [`PendingAction::expected_phrase`] 承担([`crate::commands::is_explicit_confirmation`])。
#[derive(Debug, PartialEq, Eq)]
pub enum UserIntent {
    /// 命中当前语言的口语确认词。
    Confirm,
    /// 用户显式放弃(拒绝/取消/放弃 · reject/cancel/abort)。
    Reject,
    /// 其它输入: pending 保留, 该行按普通提问送模型。
    Other,
}

/// 分类一行输入(不消费状态; 是否过期/清空由调用方决定)。
///
/// 裸 y/yes/ok/回车一律不算确认(与终端门禁同一纪律)。
pub fn classify_user_line(line: &str, lang: Lang) -> UserIntent {
    if is_simple_confirmation(line, lang) {
        UserIntent::Confirm
    } else if is_simple_rejection(line, lang) {
        UserIntent::Reject
    } else {
        UserIntent::Other
    }
}

/// REPL 读到一行输入后, 对会话 pending 的处置(锁内完成 take/回填, 避免 TOCTOU)。
#[derive(Debug, PartialEq, Eq)]
pub enum LineDisposition {
    /// 确认词命中 → 宿主应执行(动作已从 slot 取出)。
    Confirm(PendingAction),
    /// 用户显式拒绝 → 作废(动作已取出; 落盘类由调用方连 preview 一起 reject)。
    Reject(PendingAction),
    /// pending 已过期 → 提示作废; 当前行可继续当普通提问。
    Expired(PendingAction),
    /// 输入与 pending 无关: pending 已**原样留在 slot**, 当前行当普通提问送模型。
    Other,
    /// 没有待确认动作, 当前行直接送模型。
    NoPending,
}

/// 消费一行用户输入, 推进确认状态机(供 REPL 宿主调用; 纯会话逻辑, 不碰引擎/文件)。
pub async fn consume_line(slot: &PendingSlot, line: &str, lang: Lang) -> LineDisposition {
    let mut guard = slot.lock().await;
    match guard.take() {
        None => LineDisposition::NoPending,
        Some(action) if action.is_expired() => LineDisposition::Expired(action),
        Some(action) => match classify_user_line(line, lang) {
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
    use crate::commands::is_explicit_confirmation;

    /// 13 类动作全集(同名策略), 供唯一性/互斥性结构测试。
    fn all_kinds(name: &str) -> Vec<PendingAction> {
        vec![
            PendingAction::new_deploy(name, "pv-1"),
            PendingAction::new_deploy_replace(name, "pv-1"),
            PendingAction::new_update_params(name, vec!["fast=9".into()]),
            PendingAction::new_delete_strategy(name),
            PendingAction::new_start_dry_run(name),
            PendingAction::new_stop_dry_run(name),
            PendingAction::new_start_demo(name),
            PendingAction::new_stop_demo(name),
            PendingAction::new_ack_risk(),
            PendingAction::new_start_live(name),
            PendingAction::new_stop_live(name),
            PendingAction::new_close_live(name),
            PendingAction::new_restart_live(name),
        ]
    }

    #[test]
    fn test_action_kind_count_is_thirteen() {
        assert_eq!(all_kinds("g1").len(), 13, "023 FR-019: 写操作全集 13 类");
    }

    /// 终端短语闸: 前七句必须与 `approve.rs` / `ctrl.rs` 的字面量逐字一致(零改动约束)。
    #[test]
    fn test_expected_phrases_match_terminal_gates() {
        let deploy = PendingAction::new_deploy("eth-grid-1", "pv-1");
        assert_eq!(deploy.expected_phrase(), "确认部署 eth-grid-1");
        let over = PendingAction::new_deploy_replace("eth-grid-1", "pv-1");
        assert_eq!(over.expected_phrase(), "确认覆盖 eth-grid-1");
        let demo = PendingAction::new_start_demo("eth-grid-1");
        assert_eq!(demo.expected_phrase(), "确认启动测试网 eth-grid-1");
        let stop_demo = PendingAction::new_stop_demo("eth-grid-1");
        assert_eq!(stop_demo.expected_phrase(), "确认停止测试网 eth-grid-1");
        // 实盘短语与终端 `ricow start --live` 逐字一致(同一门禁体验)
        let live = PendingAction::new_start_live("eth-grid-1");
        assert_eq!(live.expected_phrase(), "确认实盘 eth-grid-1");
        let stop_live = PendingAction::new_stop_live("eth-grid-1");
        assert_eq!(stop_live.expected_phrase(), "确认停止实盘 eth-grid-1");
        let close = PendingAction::new_close_live("eth-grid-1");
        assert_eq!(close.expected_phrase(), "确认平仓停止 eth-grid-1");
        assert_eq!(PendingAction::new_ack_risk().expected_phrase(), "确认风险");
    }

    /// 13 类动作的短语(同名策略)必须两两不同, 否则会出现跨动作误放行。
    #[test]
    fn test_terminal_phrases_are_unique_across_all_kinds() {
        let actions = all_kinds("g1");
        let mut phrases: Vec<String> = actions.iter().map(|a| a.expected_phrase()).collect();
        phrases.sort();
        let before = phrases.len();
        phrases.dedup();
        assert_eq!(phrases.len(), before, "确认短语必须两两不同: {phrases:?}");
        // 无名字的动作只有风险确认; 其余都带策略名(短语里必须出现名字)
        for a in actions.iter().filter(|a| a.kind != ActionKind::AckRisk) {
            assert!(a.expected_phrase().contains("g1"), "短语须含策略名: {}", a.expected_phrase());
        }
    }

    /// 标签在两种语言下都必须唯一(确认块与提示靠它区分动作)。
    #[test]
    fn test_labels_are_unique_in_both_languages() {
        for lang in [Lang::Zh, Lang::En] {
            let mut labels: Vec<&str> =
                all_kinds("g1").iter().map(|a| a.kind.label(lang)).collect();
            labels.sort_unstable();
            let before = labels.len();
            labels.dedup();
            assert_eq!(labels.len(), before, "{lang:?} 下动作标签必须唯一: {labels:?}");
        }
    }

    /// 标签确实随语言变化(不是两套相同字面量)。
    #[test]
    fn test_labels_follow_language() {
        assert_eq!(ActionKind::Deploy.label(Lang::Zh), "落盘部署");
        assert_eq!(ActionKind::Deploy.label(Lang::En), "Deploy strategy");
        assert_ne!(ActionKind::StartLive.label(Lang::Zh), ActionKind::StartLive.label(Lang::En));
    }

    /// 结构性保证(终端语义): 任一动作的短语都不能放行另一个动作。
    #[test]
    fn test_no_cross_action_phrase_unlocks_another_action() {
        let actions = all_kinds("g1");
        for pending in &actions {
            for other in &actions {
                let intent =
                    is_explicit_confirmation(&other.expected_phrase(), &pending.expected_phrase());
                if other.kind == pending.kind {
                    assert!(intent, "{} 的短语应命中自己", pending.kind.label(Lang::Zh));
                } else {
                    assert!(
                        !intent,
                        "{} 的短语不得放行 {}",
                        other.kind.label(Lang::Zh),
                        pending.kind.label(Lang::Zh)
                    );
                }
            }
        }
    }

    /// 终端语义: 裸「确认」/「确认风险」都不算(风险确认是一次性动作, 不能顺带触发停机)。
    #[test]
    fn test_stop_phrases_do_not_collide_with_bare_live_start() {
        let live = PendingAction::new_start_live("g1");
        let stop = PendingAction::new_stop_live("g1");
        let close = PendingAction::new_close_live("g1");
        assert!(!is_explicit_confirmation(&live.expected_phrase(), &stop.expected_phrase()));
        assert!(!is_explicit_confirmation(&stop.expected_phrase(), &live.expected_phrase()));
        assert!(!is_explicit_confirmation(&close.expected_phrase(), &stop.expected_phrase()));
        assert!(!is_explicit_confirmation(&stop.expected_phrase(), &close.expected_phrase()));
        assert!(!is_explicit_confirmation("确认", &close.expected_phrase()));
        assert!(!is_explicit_confirmation(
            &PendingAction::new_ack_risk().expected_phrase(),
            &close.expected_phrase()
        ));
    }

    /// FR-044 受控覆盖: 破坏性动作另用一句短语, 且与普通部署互不放行。
    #[test]
    fn test_replace_phrase_differs_from_plain_deploy() {
        let plain = PendingAction::new_deploy("g1", "pv");
        let over = PendingAction::new_deploy_replace("g1", "pv");
        assert_eq!(over.expected_phrase(), "确认覆盖 g1");
        assert_ne!(plain.expected_phrase(), over.expected_phrase());
        assert!(!is_explicit_confirmation(&plain.expected_phrase(), &over.expected_phrase()));
        assert!(!is_explicit_confirmation(&over.expected_phrase(), &plain.expected_phrase()));
        // 覆盖语义只随构造器进入
        assert!(over.is_replace());
        assert!(!plain.is_replace());
        assert!(!PendingAction::new_start_live("g1").is_replace());
    }

    /// 对话语义核心(FR-022): 只认当前语言的口语词, 且刻意不含 y/yes/ok/空。
    #[test]
    fn test_classify_accepts_current_language_words_only() {
        // zh: 三个词放行, 英文词不放行
        for good in ["确认", "确定", "同意", "  确认  \n"] {
            assert_eq!(
                classify_user_line(good, Lang::Zh),
                UserIntent::Confirm,
                "'{good}' 应为 zh 确认"
            );
        }
        for bad in ["confirm", "confirmed", "OK", "确认一下", "确认！"] {
            assert_eq!(
                classify_user_line(bad, Lang::Zh),
                UserIntent::Other,
                "'{bad}' 不应在 zh 下放行"
            );
        }
        // en: 两个词放行(忽略大小写), 中文词不放行
        for good in ["confirm", "Confirm", "CONFIRMED", " confirmed "] {
            assert_eq!(
                classify_user_line(good, Lang::En),
                UserIntent::Confirm,
                "'{good}' 应为 en 确认"
            );
        }
        for bad in ["确认", "确定", "同意", "confirmation", "confirmed!"] {
            assert_eq!(
                classify_user_line(bad, Lang::En),
                UserIntent::Other,
                "'{bad}' 不应在 en 下放行"
            );
        }
        // 两个语言下都不放行: 防肌肉记忆误触的最低门槛
        for bad in ["y", "yes", "ok", "n", "no", "", "   ", "随便", "好"] {
            for lang in [Lang::Zh, Lang::En] {
                assert_eq!(
                    classify_user_line(bad, lang),
                    UserIntent::Other,
                    "'{bad}' 不得放行({lang:?})"
                );
            }
        }
    }

    /// 拒绝词同样只认当前语言。
    #[test]
    fn test_rejection_words_per_language() {
        for good in ["拒绝", "取消", "放弃"] {
            assert_eq!(classify_user_line(good, Lang::Zh), UserIntent::Reject, "'{good}'");
        }
        for good in ["reject", "Cancel", "ABORT", " abort "] {
            assert_eq!(classify_user_line(good, Lang::En), UserIntent::Reject, "'{good}'");
        }
        // 跨语言不放行
        assert_eq!(classify_user_line("取消", Lang::En), UserIntent::Other);
        assert_eq!(classify_user_line("cancel", Lang::Zh), UserIntent::Other);
        // 拒绝词不会误判成确认
        assert_ne!(classify_user_line("拒绝", Lang::Zh), UserIntent::Confirm);
        assert_ne!(classify_user_line("cancel", Lang::En), UserIntent::Confirm);
    }

    /// 对话语义: 单槽在握时, **只有**确认词放行 —— 任何长短语/错字/其它动作的短语都不放行。
    #[test]
    fn test_conversational_bare_confirm_is_the_only_confirmation() {
        for pending in all_kinds("g1") {
            assert_eq!(
                classify_user_line("确认", Lang::Zh),
                UserIntent::Confirm,
                "{} 应认口语确认词",
                pending.kind.label(Lang::Zh)
            );
            // 终端长短语在对话渠道**不再是确认词**(两套语义)
            assert_eq!(
                classify_user_line(&pending.expected_phrase(), Lang::Zh),
                UserIntent::Other,
                "{} 的终端长短语不应在对话渠道放行",
                pending.kind.label(Lang::Zh)
            );
        }
    }

    /// `params` 只随 UpdateParams 构造器进入, 其余动作为空。
    #[test]
    fn test_only_update_params_carries_params() {
        let up = PendingAction::new_update_params("g1", vec!["fast=9".into(), "slow=21".into()]);
        assert_eq!(up.params, vec!["fast=9".to_string(), "slow=21".to_string()]);
        for a in all_kinds("g1") {
            if a.kind != ActionKind::UpdateParams {
                assert!(a.params.is_empty(), "只有改参数带 params: {:?}", a.kind);
            }
        }
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

    #[tokio::test]
    async fn test_consume_line_state_machine() {
        let slot = new_slot();

        // 无 pending: 任意输入直接放行给模型
        assert_eq!(consume_line(&slot, "ETH 多少钱", Lang::Zh).await, LineDisposition::NoPending);

        let action = PendingAction::new_deploy("g1", "pv-123");
        *slot.lock().await = Some(action.clone());

        // 非确认词: pending 必须原样保留(零副作用), 输入仍送模型
        assert_eq!(consume_line(&slot, "y", Lang::Zh).await, LineDisposition::Other);
        assert_eq!(slot.lock().await.clone(), Some(action.clone()), "非确认词不得消费 pending");
        // 终端长短语在对话渠道也不算(两套语义)
        assert_eq!(consume_line(&slot, "确认部署 g1", Lang::Zh).await, LineDisposition::Other);
        assert!(slot.lock().await.is_some(), "长短语后 pending 仍在");

        // 普通提问同样不打断 pending
        assert_eq!(consume_line(&slot, "先帮我查下状态", Lang::Zh).await, LineDisposition::Other);
        assert!(slot.lock().await.is_some());

        // 口语确认词: pending 被取出交宿主执行, slot 清空
        match consume_line(&slot, "确认", Lang::Zh).await {
            LineDisposition::Confirm(a) => assert_eq!(a, action),
            other => panic!("应为 Confirm, 实际 {other:?}"),
        }
        assert!(slot.lock().await.is_none(), "确认后 pending 必须清空");
    }

    /// en 会话下中文「确认」不放行, `confirm` 放行。
    #[tokio::test]
    async fn test_consume_line_follows_session_language() {
        let slot = new_slot();
        *slot.lock().await = Some(PendingAction::new_delete_strategy("g2"));
        assert_eq!(consume_line(&slot, "确认", Lang::En).await, LineDisposition::Other);
        assert!(slot.lock().await.is_some());
        match consume_line(&slot, "confirm", Lang::En).await {
            LineDisposition::Confirm(a) => assert_eq!(a.kind, ActionKind::DeleteStrategy),
            other => panic!("应为 Confirm, 实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_consume_line_reject_clears_pending() {
        let slot = new_slot();
        *slot.lock().await = Some(PendingAction::new_start_demo("g2"));
        match consume_line(&slot, "拒绝", Lang::Zh).await {
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
            params: Vec::new(),
            created_at: Instant::now() - PENDING_TTL - Duration::from_secs(1),
        });
        // 即使输入恰好是确认词, 过期也优先 → Expired(不得执行)
        match consume_line(&slot, "确认", Lang::Zh).await {
            LineDisposition::Expired(a) => assert_eq!(a.preview_id.as_deref(), Some("pv-old")),
            other => panic!("过期 pending 必须拦在确认之前, 实际 {other:?}"),
        }
        assert!(slot.lock().await.is_none());
    }
}
