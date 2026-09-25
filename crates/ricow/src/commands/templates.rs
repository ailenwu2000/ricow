//! 内置策略模板目录 (019 G8): 由 `commands/mod.rs` 私有常量迁出并补上元数据。
//!
//! 编译期嵌入, 无运行时依赖, 一份来源供三个出口:
//! ① CLI 回测 `ricow backtest --strategy shannon_spot_grid`(经 [code_of] 把内置名 Lua 化);
//! ② AI 只读工具 `list_templates` / `read_template`;
//! ③ 空状态欢迎文案(告知"可以从模板起步")。
//!
//! 内置策略 2 个: shannon_spot_grid(香农现货网格) / paired_grid(现货动态非对称网格)。

/// 一条内置模板。
pub struct Template {
    /// 模板名 = 内置名(`--strategy <name>` 与 `read_template(name)` 都用它)。
    pub name: &'static str,
    /// 一句话中文说明。
    pub summary: &'static str,
    /// 该模板读取的配置键(每个策略各有各的一套)。
    pub params: &'static [&'static str],
    /// Lua 原文(编译期嵌入)。
    pub code: &'static str,
}

/// 全部内置模板(顺序即展示顺序)。
pub const ALL: &[Template] = &[
    Template {
        name: "shannon_spot_grid",
        summary: "香农现货网格: 虚拟账本(本金 × 杠杆 1~5)决定应持币量, 在平衡价上下 atr_mult×ATR 各挂一张限价单, 成交即以成交价为新平衡价并立即重挂两侧; 趋势门控(默认关, 可开)在上涨趋势暂停卖出、下跌趋势暂停买入; 价格低于 start_price 才激活; 适合震荡市, 不适合单边急涨(会跑输同敞口持有)与单边下跌",
        params: &[
            "pair",
            "interval",
            "atr_interval",
            "atr_period",
            "atr_mult",
            "start_price",
            "initial_buy_amount",
            "leverage_mult",
            "leverage_basis",
            "real_cash",
            "target_ratio",
            "trend_gate",
            "regime_filter",
            "regime_interval",
            "regime_ema_period",
            "regime_band_pct",
            "min_notional",
            "fee_side",
        ],
        code: include_str!("../../../../strategies/builtin/shannon_spot_grid.lua"),
    },
    Template {
        name: "paired_grid",
        summary: "现货动态非对称网格: 价格低于 start_price 激活, 以最近成交价为参考价上下各挂一单(下方固定金额买单、上方配对卖单, 卖价恒>买价); 方向标志(买-1/卖+1)驱动上下间距不对称放大, 抑制单向成交; 可选建仓与积累币/积累U两种成交模式; 适合震荡市, 不适合单边(下跌满仓套牢/上涨跑输持有)",
        params: &[
            "pair",
            "interval",
            "spacing_pct",
            "start_price",
            "order_amount",
            "initial_buy_amount",
            "direction_offset",
            "min_pair_profit",
            "accumulate_mode",
            "min_notional",
            "fee_side",
        ],
        code: include_str!("../../../../strategies/builtin/paired_grid.lua"),
    },
];

/// 按名查模板(不存在 → None)。
pub fn find(name: &str) -> Option<&'static Template> {
    ALL.iter().find(|t| t.name == name)
}

/// 内置名 → Lua 原文(供 `resolve_builtin_script` 保持原行为)。
pub fn code_of(name: &str) -> Option<&'static str> {
    find(name).map(|t| t.code)
}

/// 参数键摘要, 如 `pair, order_size`。
fn params_text(t: &Template) -> String {
    t.params.join(", ")
}

/// 模板清单(不含代码): 供 `list_templates` 工具与 CLI 提示复用。
pub fn list_text() -> String {
    let mut out = format!("内置策略模板 {} 个(编译期内置, 免网络):\n", ALL.len());
    for t in ALL {
        out.push_str(&format!("【完整策略】{} — {}\n", t.name, t.summary));
        out.push_str(&format!("  参数: {}\n", params_text(t)));
    }
    out.push_str(
        "用法: 每个模板原文都可用 read_template 取到, 直接作为 preview_strategy 的 script(参数按用户回答填); \
         完全不用模板也可以: 按用户需求新写一份完整策略即可。\n",
    );
    out
}

/// 单个模板详情(元数据 + 原文代码), 供 `read_template` 工具。
pub fn render_read(t: &Template) -> String {
    let mut out = format!("模板 {} — {}\n参数: {}\n", t.name, t.summary, params_text(t));
    out.push_str("可直接作为 preview_strategy 的 script(把参数按用户回答填好, 不要留空)。\n");
    out.push_str("\nLua 原文:\n");
    out.push_str(t.code);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_templates_have_metadata_and_code() {
        assert!(!ALL.is_empty());
        for t in ALL {
            assert!(!t.name.is_empty(), "模板名不得为空");
            assert!(t.name.is_ascii(), "模板名须为 ASCII: {}", t.name);
            assert!(t.summary.chars().count() > 10, "模板 {} 说明过短", t.name);
            assert!(!t.params.is_empty(), "模板 {} 缺参数摘要", t.name);
            assert!(
                t.code.contains("function on_"),
                "模板 {} 代码不像 Lua 策略: {}",
                t.name,
                t.code.len()
            );
        }
    }

    #[test]
    fn test_names_unique_and_lookup_works() {
        let mut names: Vec<&str> = ALL.iter().map(|t| t.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "模板名重复");
        assert!(find("shannon_spot_grid").is_some());
        assert!(code_of("shannon_spot_grid").is_some_and(|c| c.contains("on_tick")));
        assert!(find("paired_grid").is_some());
        assert!(find("no-such-template").is_none());
        assert!(code_of("no-such-template").is_none());
    }

    #[test]
    fn test_list_text_states_routes() {
        let text = list_text();
        for must in [
            "shannon_spot_grid",
            "paired_grid",
            "现货动态非对称网格",
            "read_template",
            "preview_strategy",
        ] {
            assert!(text.contains(must), "清单缺少: {must}");
        }
    }

    #[test]
    fn test_render_read_includes_code() {
        let grid = render_read(find("shannon_spot_grid").unwrap());
        assert!(grid.contains("pair"), "{grid}");
        assert!(grid.contains("on_tick"), "详情须带原文代码");
    }
}
