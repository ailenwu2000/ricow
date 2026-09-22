//! 策略名规范 (019 D18 / FR-043)。
//!
//! 为什么必须限定字符合集: 策略名会派生**订单归属前缀** `ownership_prefix(name)` = `<名字>-`,
//! 该函数把非 `[A-Za-z0-9_-]` 的字符一律替换成 `-` (币安 `clientOrderId` 只允许这些字符)。
//! 后果(实测根因, `align.rs:134`):
//!   - `网格A` 与 `网格B` 都塌缩成前缀 `-` → `is_owned()` 认为**对方的挂单是自己的**,
//!     停机清理会去撤别人的单(甚至用户手工挂的单);
//!   - 名字过长还会挤占 `clientOrderId` 的 36 字符预算(`inject_prefix` 会把订单号尾部截掉)。
//!
//! 因此: 名字限定 `[A-Za-z0-9_-]`, 长度 ≤ 24, 且与既有策略名**不得互为前缀**
//! (例如 `abc` 与 `abc-x`: 前者前缀 `abc-` 是后者前缀 `abc-x-` 的前缀 → 仍会互相误撤)。

/// 策略名长度上限(与 D18 一致)。名字 + `-` 后需给 `clientOrderId` 留足空间。
pub const MAX_STRATEGY_NAME_LEN: usize = 24;

/// 合法字符: ASCII 字母 / 数字 / `-` / `_`。
pub fn is_valid_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// 校验策略名。`Err` 里的文案是给用户看的**可执行**说明(含替代名建议)。
pub fn validate_strategy_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("策略名不能为空 (建议用交易对+思路的英文短名, 如 eth-grid-300)".into());
    }
    if let Some(bad) = name.chars().find(|c| !is_valid_char(*c)) {
        return Err(format!(
            "策略名 '{name}' 含非法字符 '{bad}'(U+{:04X}): 会派生订单归属前缀, 非 [A-Za-z0-9_-] 的字符会被替换成 '-', \
             导致不同策略的前缀相同并互相撤掉对方的挂单。请改用英文/数字/短横线, 例如: {}",
            bad as u32,
            suggest_strategy_name(name)
        ));
    }
    let len = name.chars().count();
    if len > MAX_STRATEGY_NAME_LEN {
        return Err(format!(
            "策略名 '{name}' 长度 {len} 超过上限 {MAX_STRATEGY_NAME_LEN}: 名字会进 clientOrderId(限 36 字符), \
             过长会挤掉订单号本身。建议: {}",
            suggest_strategy_name(name)
        ));
    }
    Ok(())
}

/// 生成一个可用的替代名: 非法字符 → `-`, 折叠连续 `-`, 去掉首尾 `-`, 截断到上限; 空则 `strategy`。
pub fn suggest_strategy_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        let c = if is_valid_char(c) { c } else { '-' };
        if c == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        out.push(c);
    }
    let out = out.trim_matches('-').to_string();
    let out: String = out.chars().take(MAX_STRATEGY_NAME_LEN).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "strategy".into()
    } else {
        out
    }
}

/// 与既有策略名是否**互为前缀**(D18 "同前缀冲突拒绝")。
///
/// 判定: 各自加 `-` 后的前缀, 其中一个以另一个开头 → 两个策略的 `is_owned` 会互相命中 → 误撤挂单。
/// 返回冲突的既有名字(若有)。
pub fn prefix_conflict<'a>(candidate: &str, existing: &'a [String]) -> Option<&'a str> {
    let cp = format!("{candidate}-");
    existing
        .iter()
        .find(|e| {
            let ep = format!("{e}-");
            cp.starts_with(&ep) || ep.starts_with(&cp)
        })
        .map(|s| s.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_names_accepted() {
        for n in ["eth-grid-300", "grid_demo", "shannon_rebalance", "a", "ABC123", "x-1_2"] {
            assert!(validate_strategy_name(n).is_ok(), "{n} 应合法");
        }
    }

    #[test]
    fn test_chinese_space_and_other_chars_rejected_with_suggestion() {
        // 中文名: 塌缩成 '-' 的根因场景, 必须拒绝并给英文替代名
        let e = validate_strategy_name("网格A").unwrap_err();
        assert!(e.contains("非法字符"), "{e}");
        assert!(e.contains("例如"), "必须给出替代名建议: {e}");
        // 替代名本身必须能通过校验(可直接采用)
        let sug = e.rsplit("例如: ").next().unwrap().trim().to_string();
        assert!(validate_strategy_name(&sug).is_ok(), "建议名不合法: {sug} (原始错误: {e})");
        // 空格
        let e = validate_strategy_name("my grid").unwrap_err();
        assert!(e.contains("非法字符"), "{e}");
        // 点号/斜杠(常见误用)
        for n in ["v1.2", "grid/eth", "网格"] {
            assert!(validate_strategy_name(n).is_err(), "{n} 应被拒绝");
        }
    }

    #[test]
    fn test_length_limit_24() {
        let ok = "a".repeat(24);
        assert!(validate_strategy_name(&ok).is_ok(), "24 字符应合法");
        let too_long = "a".repeat(25);
        let e = validate_strategy_name(&too_long).unwrap_err();
        assert!(e.contains("超过上限 24"), "{e}");
        // 替代名同样受上限约束
        let s = suggest_strategy_name(&"中文".repeat(20));
        assert!(s.chars().count() <= MAX_STRATEGY_NAME_LEN, "{s}");
    }

    #[test]
    fn test_suggestion_is_usable_and_ascii() {
        assert_eq!(suggest_strategy_name("网格A"), "A");
        assert_eq!(suggest_strategy_name("my grid"), "my-grid");
        assert_eq!(suggest_strategy_name("v1.2"), "v1-2");
        assert_eq!(suggest_strategy_name("---"), "strategy");
        assert_eq!(suggest_strategy_name(""), "strategy");
        // 连续非法字符折叠为一个 '-', 且不留首尾 '-' ; 全是非法字符时退回 "strategy"
        assert_eq!(suggest_strategy_name("  中文 网格  "), "strategy");
        assert_eq!(suggest_strategy_name("eth 网格 300"), "eth-300");
        for s in ["网格A", "my grid", "v1.2", "eth 网格 300"] {
            let sug = suggest_strategy_name(s);
            assert!(validate_strategy_name(&sug).is_ok(), "替代名本身必须合法: {sug}");
        }
    }

    #[test]
    fn test_prefix_conflict_detected() {
        let existing = vec!["abc".to_string(), "grid_demo".to_string()];
        // 互为前缀: abc 与 abc-x / abc_1(名字加 '-' 后互相命中)
        assert_eq!(prefix_conflict("abc-x", &existing), Some("abc"));
        assert_eq!(prefix_conflict("abc", &existing), Some("abc")); // 同名也算冲突(部署会覆盖)
        assert_eq!(prefix_conflict("abc-x-y", &existing), Some("abc"));
        // 安全: abc1 与 abc 不互为前缀('abc1-' 不以 'abc-' 开头)
        assert_eq!(prefix_conflict("abc1", &existing), None);
        assert_eq!(prefix_conflict("eth-grid-300", &existing), None);
    }
}
