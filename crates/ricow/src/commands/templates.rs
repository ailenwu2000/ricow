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
            "【{}·{src}】{} — {}\n",
            e.manifest.id, e.manifest.name, e.manifest.summary
        ));
        out.push_str(&format!("  参数: {}\n", params_text(e)));
    }
    out.push_str(
        "用法: 每个策略原文都可用 read_template 取到, 直接作为 preview_strategy 的 script(参数按用户回答填); \
         完全不用模板也可以: 按用户需求新写一份完整策略即可。\n",
    );
    out
}

/// 单个策略详情(元数据 + 原文代码), 供 `read_template` 工具。
pub fn render_read(e: &CatalogEntry) -> String {
    let mut out =
        format!("策略 {} — {}\n参数: {}\n", e.manifest.id, e.manifest.name, params_text(e));
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
        for must in ["shannon_spot_grid", "paired_grid", "香农现货网格", "现货动态非对称网格"]
        {
            assert!(text.contains(must), "清单缺少: {must}");
        }
    }

    #[test]
    fn find_works() {
        assert!(find("shannon_spot_grid").is_some());
        assert!(find("paired_grid").is_some());
        assert!(find("no-such-template").is_none());
        assert!(find("shannon_spot_grid").is_some_and(|e| e.code.contains("on_tick")));
    }

    #[test]
    fn names_contains_builtin() {
        let names = names();
        assert!(names.iter().any(|n| n == "shannon_spot_grid"));
        assert!(names.iter().any(|n| n == "paired_grid"));
    }

    #[test]
    fn render_read_includes_code() {
        let grid = find("shannon_spot_grid").unwrap();
        let rendered = render_read(&grid);
        assert!(rendered.contains("on_tick"), "详情须带原文代码");
    }
}
