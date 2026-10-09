//! 策略模板目录 (019 G8 → 031 薄封装到 `crate::strategies::catalog`)。
//!
//! 031 起,"内置模板"与"用户自写策略"已统一由 [`crate::strategies::catalog`] 管理;
//! 本模块保留 `find` / `list_text` / `render_read` 接口与文案生成, 数据来自 catalog。

use crate::strategies::catalog::{self, CatalogEntry};

/// 按 id 查策略。
pub fn find(name: &str) -> Option<CatalogEntry> {
    catalog::find(name)
}

/// 全部策略 id(供工具错误提示列可用名)。
pub fn names() -> Vec<String> {
    catalog::all().into_iter().map(|e| e.manifest.id).collect()
}

/// 策略清单文本(供 `list_templates` 工具): 内置示例 + 用户自写统一列出。
pub fn list_text() -> String {
    let entries = catalog::all();
    let mut out = format!("策略 {} 个(内置示例 + 用户自写; 内置免网络):\n", entries.len());
    for e in &entries {
        let src = match e.source {
            catalog::Source::Builtin => "内置",
            catalog::Source::User => "用户",
        };
        out.push_str(&format!(
            "【{}·{src}】{} — {}{}\n",
            e.manifest.id,
            e.manifest.name,
            e.manifest.summary,
            entry_marks(e)
        ));
        out.push_str(&format!("  参数: {}\n", params_text(e)));
    }
    out.push_str(
        "用法: 每个策略原文都可用 read_template 取到, 直接作为 preview_strategy 的 script(参数按用户回答填); \
         完全不用模板也可以: 按用户需求新写一份完整策略即可。\n\
         注意: 带 ⚠ 的条目有瑕疵(未声明参数清单 / 与内置脚本逐字相同), 别把「未声明清单」的当模板推荐给\
         用户——它没有参数 schema, 照抄会缺必填参数并静默停机。\n",
    );
    out
}

/// 条目标记(供清单/详情如实提示): 未声明参数清单 / 与内置脚本逐字相同。
fn entry_marks(e: &CatalogEntry) -> String {
    let mut marks: Vec<String> = Vec::new();
    if !e.declared {
        marks.push("未声明参数清单(无参数 schema, 表单只剩交易对, 照抄会缺必填参数)".to_string());
    }
    if let Some(b) = &e.duplicate_of {
        marks.push(format!("脚本与内置 {b} 逐字相同(冗余副本, 别与它跑同一交易对)"));
    }
    if marks.is_empty() {
        String::new()
    } else {
        format!(" ⚠ {}", marks.join("; "))
    }
}

/// 单个策略详情(元数据 + 原文代码), 供 `read_template` 工具。
pub fn render_read(e: &CatalogEntry) -> String {
    let marks = entry_marks(e);
    let mut out = format!(
        "策略 {} — {}\n参数: {}{}\n",
        e.manifest.id,
        e.manifest.name,
        params_text(e),
        marks
    );
    if !marks.is_empty() {
        out.push_str("(⚠ 该策略有瑕疵, 见上; 若用户要的是「可跑通」的起点, 先确认参数再预览。)\n");
    }
    if let Some(d) = &e.manifest.description {
        out.push_str(&format!("说明: {d}\n"));
    }
    out.push_str("可直接作为 preview_strategy 的 script(把参数按用户回答填好, 不要留空)。\n");
    out.push_str("\nLua 原文:\n");
    out.push_str(&e.code);
    out
}

/// 参数键摘要, 如 `pair, order_size`。
fn params_text(e: &CatalogEntry) -> String {
    e.manifest.params.iter().map(|p| p.key.as_str()).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_text_lists_builtin_strategies() {
        let text = list_text();
        for must in ["linear_position_grid", "paired_grid_futures_long", "现货线性仓位网格"] {
            assert!(text.contains(must), "清单缺少: {must}");
        }
    }

    #[test]
    fn find_works() {
        assert!(find("linear_position_grid").is_some());
        assert!(find("no-such-template").is_none());
        assert!(find("linear_position_grid").is_some_and(|e| e.code.contains("on_tick")));
    }

    #[test]
    fn names_contains_builtin() {
        let names = names();
        assert!(names.iter().any(|n| n == "linear_position_grid"));
        assert!(names.iter().any(|n| n == "paired_grid_futures_long"));
    }

    #[test]
    fn render_read_includes_code() {
        let grid = find("linear_position_grid").unwrap();
        let rendered = render_read(&grid);
        assert!(rendered.contains("on_tick"), "详情须带原文代码");
    }

    /// 诚实性标记要出现在 AI 工具文本里: 未声明清单 / 与内置脚本重复(2026-10-05)。
    #[test]
    fn marks_show_undeclared_and_duplicate() {
        let mut e = find("linear_position_grid").expect("内置必须存在");
        assert!(entry_marks(&e).is_empty(), "内置干净条目不该带 ⚠ 标记");

        e.declared = false;
        e.duplicate_of = Some("linear_position_grid".to_string());
        let marks = entry_marks(&e);
        assert!(marks.contains("未声明参数清单"), "须提示未声明清单: {marks}");
        assert!(marks.contains("linear_position_grid"), "须点名重复的内置 id: {marks}");
        assert!(render_read(&e).contains('⚠'), "详情也要带标记");
    }
}
