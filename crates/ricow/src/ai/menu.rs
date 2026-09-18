//! 对话菜单 (023 F3 / FR-013~FR-018): **编号与文案由宿主单一来源产生**。
//!
//! 模型只能经 `show_menu(kind, name)` **请求**渲染菜单, 不能自己编一份编号清单 ——
//! 否则"模型自造编号 + 宿主序号映射"一错位, 用户选的 3 会被执行成第 4 项。
//!
//! # 误触面(刻意的)
//! 菜单内**没有一项直接产生写操作**: 序号只被翻译成 `request` 里那句自然语言请求,
//! 再走正常对话 + F4 确认。因此误触最坏后果 = "多走一步确认", 而不是"误删策略"。
//!
//! 菜单生命周期仅由"被选中"或"被新菜单替换"结束, **不设 TTL**(决策 D9):
//! 陈旧菜单点下去也只是多问一句, 没必要为此加过期逻辑。

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::i18n::Lang;

/// 菜单项: `label` 是给用户看的短标签, `request` 是选中后**转成的自然语言请求**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuOption {
    pub label: String,
    pub request: String,
}

/// 一屏待选菜单(标题 + 选项)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub options: Vec<MenuOption>,
}

/// 与 `confirm::PendingSlot` 同构: 工具闭包与 `ChatSession` 共享的菜单句柄。
pub type MenuSlot = Arc<Mutex<Option<Menu>>>;

pub fn new_slot() -> MenuSlot {
    Arc::new(Mutex::new(None))
}

/// `show_menu` 的 kind 取值: 策略刚落盘, 接下来能做什么。
pub const KIND_STRATEGY_READY: &str = "strategy_ready";
/// `show_menu` 的 kind 取值: 管理一个已有策略。
pub const KIND_MANAGE: &str = "manage";

/// 带占位符的文案: [`crate::i18n::t`] 只选字面量, 这里选两条已 `format!` 好的串。
fn tf(lang: Lang, zh: String, en: String) -> String {
    match lang {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

/// 按 kind 构造菜单; 未收录的 kind 或空名返回 `None`(工具据此报参数错误)。
pub fn build(kind: &str, name: &str, lang: Lang) -> Option<Menu> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    match kind {
        KIND_STRATEGY_READY => Some(strategy_ready(name, lang)),
        KIND_MANAGE => Some(manage(name, lang)),
        _ => None,
    }
}

/// 菜单 A「策略就绪」(FR-014): 回测 / 试跑 / 测试网 / 实盘 / 管理策略。
fn strategy_ready(name: &str, lang: Lang) -> Menu {
    Menu {
        title: tf(
            lang,
            format!("策略 {name} 准备好了, 接下来想做什么? 说序号或原话都行:"),
            format!("{name} is ready — what would you like to do? Reply with a number or in your own words:"),
        ),
        options: vec![
            MenuOption {
                label: tf(
                    lang,
                    "回测 —— 用历史数据看表现(不花钱, 直接跑)".into(),
                    "Backtest — see how it would have done on past data (no cost, runs right away)"
                        .into(),
                ),
                request: tf(
                    lang,
                    format!("用历史数据回测一下 {name}"),
                    format!("Backtest {name} on historical data"),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "试跑 —— 实时行情 + 虚拟下单(不花钱, 会先请你确认)".into(),
                    "Dry run — live prices with simulated orders (no cost; I'll ask you to confirm)"
                        .into(),
                ),
                request: tf(
                    lang,
                    format!("把 {name} 跑成 Dry Run(虚拟试跑)"),
                    format!("Run {name} as a dry run"),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "测试网 —— 连币安测试网真实下单(不是真钱, 会先请你确认)".into(),
                    "Testnet — real orders on the Binance testnet (not real money; I'll ask you to confirm)"
                        .into(),
                ),
                request: tf(
                    lang,
                    format!("把 {name} 连上币安测试网试跑"),
                    format!("Run {name} on the Binance testnet"),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "实盘 —— 真实资金(我会先讲清楚, 你说\"确认\"才执行)".into(),
                    "Live — real money (I'll explain first; nothing runs until you confirm)".into(),
                ),
                request: tf(
                    lang,
                    format!("我要把 {name} 上实盘运行"),
                    format!("Take {name} live"),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "管理策略 —— 看状态 / 停 / 改参数 / 删".into(),
                    "Manage — status / stop / change settings / delete".into(),
                ),
                request: tf(
                    lang,
                    format!("我要管理策略 {name}"),
                    format!("Manage strategy {name}"),
                ),
            },
        ],
    }
}

/// 菜单 B「管理策略」(FR-014): 看状态 / 停止 / 改参数 / 删除。
fn manage(name: &str, lang: Lang) -> Menu {
    Menu {
        title: tf(
            lang,
            format!("策略 {name} 的管理, 说序号或原话:"),
            format!("Managing {name} — reply with a number or in your own words:"),
        ),
        options: vec![
            MenuOption {
                label: tf(
                    lang,
                    "看状态 —— 现在跑得怎么样 / 有没有持仓(直接看)".into(),
                    "Status — how it's doing and any open positions (read-only)".into(),
                ),
                request: tf(
                    lang,
                    format!("看一下 {name} 现在的状态"),
                    format!("Show me the current status of {name}"),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "停止 —— 会先请你确认".into(),
                    "Stop — I'll ask you to confirm first".into(),
                ),
                request: tf(
                    lang,
                    format!("停止策略 {name}; 如果是实盘, 先问我是保留持仓还是平仓"),
                    format!(
                        "Stop strategy {name}; if it is live, first ask me whether to keep positions or close them"
                    ),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "改参数 —— 改完自动重启生效; 实盘实例重启也会先请你确认".into(),
                    "Change settings — applies by restarting; live restarts also need your confirmation"
                        .into(),
                ),
                request: tf(
                    lang,
                    format!("我要改 {name} 的参数"),
                    format!("I want to change parameters of {name}"),
                ),
            },
            MenuOption {
                label: tf(
                    lang,
                    "删除策略 —— 不可逆, 会先请你确认".into(),
                    "Delete — cannot be undone; I'll ask you to confirm first".into(),
                ),
                request: tf(
                    lang,
                    format!("删除策略 {name}"),
                    format!("Delete strategy {name}"),
                ),
            },
        ],
    }
}

/// 渲染菜单(标题 + 逐行编号)。
pub fn render(menu: &Menu) -> String {
    let mut out = String::new();
    out.push_str(&menu.title);
    for (i, opt) in menu.options.iter().enumerate() {
        out.push_str(&format!("\n  {}. {}", i + 1, opt.label));
    }
    out
}

/// 解析纯序号行(FR-016): `1` / `2.` / `3)` / `4、` 命中; `0` / `123` / `1a` / 空 不命中。
///
/// 按 FR-016 的正则 `^\s*([1-9][0-9]?)[).、]?\s*$`, **最多两位**(首位非 0), 故 `10` / `12`
/// 都算"形式合法"; 菜单最多 5 项, 两位序号由调用方越界拦下并提示 `请选 1..N`。
///
/// 返回 **1 基**序号(直接就是 `options` 的下标 + 1), 由调用方做越界判断。
pub fn parse_menu_choice(s: &str) -> Option<usize> {
    let s = s.trim();
    let digits = s.strip_suffix(['.', ')', '、']).unwrap_or(s);
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    if n == 0 {
        None
    } else {
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_menu_choice_accepts_number_with_optional_suffix() {
        for (input, want) in
            [("1", 1), ("2.", 2), ("3)", 3), ("4、", 4), (" 5 ", 5), ("10", 10), ("10.", 10)]
        {
            assert_eq!(parse_menu_choice(input), Some(want), "{input:?} 应命中");
        }
    }

    #[test]
    fn test_parse_menu_choice_rejects_non_numbers_and_zero() {
        for bad in ["", "  ", "0", "123", "1a", "a1", "-1", "1 2", "①"] {
            assert_eq!(parse_menu_choice(bad), None, "{bad:?} 不应命中");
        }
    }

    #[test]
    fn test_strategy_ready_has_five_options_named_after_strategy() {
        let m = build(KIND_STRATEGY_READY, "grid01", Lang::Zh).expect("菜单 A");
        assert_eq!(m.options.len(), 5);
        assert!(m.title.contains("grid01"), "{}", m.title);
        for opt in &m.options {
            assert!(opt.request.contains("grid01"), "request 必须带策略名: {}", opt.request);
            assert!(!opt.label.is_empty());
        }
    }

    #[test]
    fn test_manage_has_four_options_named_after_strategy() {
        let m = build(KIND_MANAGE, "grid01", Lang::Zh).expect("菜单 B");
        assert_eq!(m.options.len(), 4);
        assert!(m.title.contains("grid01"), "{}", m.title);
        for opt in &m.options {
            assert!(opt.request.contains("grid01"), "request 必须带策略名: {}", opt.request);
        }
    }

    #[test]
    fn test_menu_follows_language_and_has_no_terminal_commands() {
        for lang in [Lang::Zh, Lang::En] {
            for kind in [KIND_STRATEGY_READY, KIND_MANAGE] {
                let m = build(kind, "grid01", lang).expect("菜单");
                let text = render(&m);
                assert!(!text.contains("ricow "), "菜单不得出现终端命令: {text}");
            }
        }
        let zh = render(&build(KIND_MANAGE, "g", Lang::Zh).unwrap());
        let en = render(&build(KIND_MANAGE, "g", Lang::En).unwrap());
        assert!(zh.contains("看状态"), "{zh}");
        assert!(en.contains("Status"), "{en}");
    }

    #[test]
    fn test_unknown_kind_or_blank_name_is_none() {
        assert!(build("whatever", "g", Lang::Zh).is_none());
        assert!(build(KIND_MANAGE, "   ", Lang::Zh).is_none());
    }

    #[test]
    fn test_render_numbers_options_from_one() {
        let m = build(KIND_MANAGE, "g", Lang::Zh).unwrap();
        let text = render(&m);
        assert!(text.contains("\n  1. "), "{text}");
        assert!(text.contains("\n  4. "), "{text}");
        assert!(!text.contains("\n  5. "), "{text}");
    }

    #[test]
    fn test_menu_choice_maps_to_the_same_option_the_user_saw() {
        // 序号 → request 的映射必须是同一份数据(FR-013: 编号与文案单一来源)
        let m = build(KIND_STRATEGY_READY, "grid01", Lang::Zh).unwrap();
        for (i, opt) in m.options.iter().enumerate() {
            let n = parse_menu_choice(&format!("{}", i + 1)).unwrap();
            assert_eq!(m.options[n - 1].request, opt.request);
        }
    }
}
