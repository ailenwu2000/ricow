//! 界面语言(023): **宿主固定文案的唯一来源**。
//!
//! 关于"确认词"的语言侧依据(两套语义, 见 [`crate::ai::confirm`]):
//! - **对话渠道**用当前语言的**口语确认词**(`确认` / `确定` / `同意` | `confirm` / `confirmed`),
//!   由 [`crate::ai::confirm::is_simple_confirmation`] 判定, `lang` 即本模块解析出的语言;
//! - **终端渠道**(`ricow approve` / `ricow start --live` / `ricow stop --live`)保持**逐字中文长短语**,
//!   `approve.rs` / `ctrl.rs` 一行不改 —— 因此终端文案不经过本模块。
//!
//! 设计取舍(023 决策 D13): **不引入 i18n 框架**(无 Fluent/gettext、无资源文件、无构建期生成)。
//! 需要占位符的文案由调用点 `format!` 组装, [`t`] 只负责在两条字面量之间选词。

use crate::commands::config_file;

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 中文(默认)。
    Zh,
    /// English。
    En,
}

impl Lang {
    /// 解析语言代号(大小写与首尾空白容错); 非 `zh` / `en` → `None`。
    pub fn parse(s: &str) -> Option<Lang> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("zh") {
            Some(Lang::Zh)
        } else if s.eq_ignore_ascii_case("en") {
            Some(Lang::En)
        } else {
            None
        }
    }

    /// 配置里存的语言代号。
    pub fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }
}

/// 从配置解析语言; **未选择**(`[ui].lang` 缺失)或值无法识别 → 默认中文。
///
/// 注: 非法值在 [`config_file::load`] 处已硬失败, 走不到这里; 此处的兜底只为
/// 不 panic(与 `load` 同一口径: 宁可退回默认, 也不让对话入口崩掉)。
pub fn resolve(f: &config_file::File) -> Lang {
    f.ui.lang.as_deref().and_then(Lang::parse).unwrap_or(Lang::Zh)
}

/// 双语取词: 就地写两条文案, 按语言选一条。
pub fn t(lang: Lang, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lang_parse_accepts_zh_en_and_rejects_others() {
        assert_eq!(Lang::parse("zh"), Some(Lang::Zh));
        assert_eq!(Lang::parse("en"), Some(Lang::En));
        assert_eq!(Lang::parse("  EN "), Some(Lang::En), "大小写与空白容错");
        assert_eq!(Lang::parse("Zh"), Some(Lang::Zh));
        assert_eq!(Lang::parse(""), None);
        assert_eq!(Lang::parse("fr"), None);
        assert_eq!(Lang::parse("english"), None);
        assert_eq!(Lang::parse("中"), None);
    }

    #[test]
    fn test_lang_code_round_trips() {
        assert_eq!(Lang::parse(Lang::Zh.code()), Some(Lang::Zh));
        assert_eq!(Lang::parse(Lang::En.code()), Some(Lang::En));
    }

    #[test]
    fn test_resolve_defaults_to_zh_when_unset() {
        let mut f = config_file::File::default();
        assert_eq!(resolve(&f), Lang::Zh, "未选择 → 默认中文");
        f.ui.lang = Some("en".into());
        assert_eq!(resolve(&f), Lang::En);
        f.ui.lang = Some("ZH".into());
        assert_eq!(resolve(&f), Lang::Zh);
    }

    #[test]
    fn test_t_picks_by_language() {
        assert_eq!(t(Lang::Zh, "你好", "Hi"), "你好");
        assert_eq!(t(Lang::En, "你好", "Hi"), "Hi");
    }
}
