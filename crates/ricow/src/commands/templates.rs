//! 内置策略模板目录 (019 G8): 由 `commands/mod.rs` 私有常量迁出并补上元数据。
//!
//! 编译期嵌入, 无运行时依赖, 一份来源供三个出口:
//! ① CLI 回测 `ricow backtest --strategy shannon_rebalance`(经 [code_of] 把内置名 Lua 化);
//! ② AI 只读工具 `list_templates` / `read_template`;
//! ③ 空状态欢迎文案(告知"可以从模板起步")。
//!
//! 类别必须分清: `strategy` 是**完整策略**, 代码可直接进 `preview_strategy`;
//! `executor_component` 只是执行片段(下单/分片), 需嵌入 `on_tick` 框架后再作为策略使用。

/// 模板类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// 完整策略: 有 on_init/on_tick, 代码可直接生成预览。
    Strategy,
    /// 执行组件示例: 只有执行片段, 需嵌入 on_tick 框架。
    ExecutorComponent,
}

impl TemplateKind {
    /// 展示用短标签。
    pub fn text(self) -> &'static str {
        match self {
            TemplateKind::Strategy => "完整策略",
            TemplateKind::ExecutorComponent => "执行组件",
        }
    }

    /// 能否作为完整策略直接进 `preview_strategy`。
    pub fn is_direct_strategy(self) -> bool {
        self == TemplateKind::Strategy
    }
}

/// 一条内置模板。
pub struct Template {
    /// 模板名 = 内置名(`--strategy <name>` 与 `read_template(name)` 都用它)。
    pub name: &'static str,
    pub kind: TemplateKind,
    /// 一句话中文说明。
    pub summary: &'static str,
    /// 该模板读取的配置键(每个策略/组件各有各的一套)。
    pub params: &'static [&'static str],
    /// Lua 原文(编译期嵌入)。
    pub code: &'static str,
}

/// 全部内置模板(顺序即展示顺序)。
pub const ALL: &[Template] = &[
    Template {
        name: "shannon_rebalance",
        kind: TemplateKind::Strategy,
        summary: "香农 50:50 中轴再平衡: 首 tick 按 target_ratio 建仓, 持仓占比偏离 target_ratio±band 即回平衡; 可选 ATR 自适应带宽与策略自管回撤止损",
        params: &[
            "pair",
            "order_size",
            "target_ratio",
            "rebalance_band",
            "atr_period",
            "atr_mult",
            "pause_pct",
            "pause_bars",
            "dd_stop_pct",
        ],
        code: include_str!("../../../../strategies/builtin/shannon_rebalance.lua"),
    },
    Template {
        name: "dca",
        kind: TemplateKind::ExecutorComponent,
        summary: "定时定投执行片段: 每 interval_secs 买入 order_size, 可选 max_buys 上限(改信号即自定义策略)",
        params: &["pair", "order_size", "interval_secs", "max_buys"],
        code: include_str!("../../../../strategies/builtin/executors/dca.lua"),
    },
    Template {
        name: "twap",
        kind: TemplateKind::ExecutorComponent,
        summary: "TWAP 时间加权分批执行片段: 把 total_size 均分 num_slices 片, 每 slice_interval_secs 下一片",
        params: &["pair", "total_size", "num_slices", "slice_interval_secs", "side"],
        code: include_str!("../../../../strategies/builtin/executors/twap.lua"),
    },
    Template {
        name: "vwap",
        kind: TemplateKind::ExecutorComponent,
        summary: "VWAP 成交量加权分批执行片段: 按最近 lookback_bars 根已收盘 K 线的成交量加权均价挂限价, 无数据回退市价",
        params: &[
            "pair",
            "total_size",
            "num_slices",
            "slice_interval_secs",
            "side",
            "lookback_bars",
        ],
        code: include_str!("../../../../strategies/builtin/executors/vwap.lua"),
    },
    Template {
        name: "pullback",
        kind: TemplateKind::ExecutorComponent,
        summary: "限价回调执行片段: 价格创新高(HWM)后回撤 pullback_pct 或 pullback_abs 时买入, 做空对称",
        params: &[
            "pair",
            "side",
            "order_size",
            "activation_price",
            "pullback_pct",
            "pullback_abs",
        ],
        code: include_str!("../../../../strategies/builtin/executors/pullback.lua"),
    },
    Template {
        name: "ladder",
        kind: TemplateKind::ExecutorComponent,
        summary: "阶梯挂单执行片段: 在 lower_price~upper_price 间按 num_levels 档铺限价单(equal 等差 / geometric 等比)",
        params: &[
            "pair",
            "side",
            "total_size",
            "num_levels",
            "lower_price",
            "upper_price",
            "distribution",
        ],
        code: include_str!("../../../../strategies/builtin/executors/ladder.lua"),
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
    let mut out = format!("内置模板 {} 个(编译期内置, 免网络):\n", ALL.len());
    for t in ALL {
        out.push_str(&format!("【{}】{} — {}\n", t.kind.text(), t.name, t.summary));
        out.push_str(&format!("  参数: {}\n", params_text(t)));
    }
    let direct: Vec<&str> =
        ALL.iter().filter(|t| t.kind.is_direct_strategy()).map(|t| t.name).collect();
    out.push_str(&format!(
        "用法: 【完整策略】的原文可用 read_template 取到, 直接作为 preview_strategy 的 script(参数按用户回答填); \
         只有 {} 这一类可直接生成预览。\n\
         【执行组件】只有下单/执行片段: 需嵌入 on_tick 框架(见 read_doc lua-api)并补齐信号后再作为完整策略, \
         或作为新策略里的可复制写法。\n\
         完全不用模板也可以: 按用户需求新写一份完整策略即可。",
        direct.join(" / ")
    ));
    out
}

/// 单个模板详情(元数据 + 原文代码), 供 `read_template` 工具。
pub fn render_read(t: &Template) -> String {
    let mut out =
        format!("模板 {} ({}) — {}\n参数: {}\n", t.name, t.kind.text(), t.summary, params_text(t));
    if t.kind.is_direct_strategy() {
        out.push_str("可直接作为 preview_strategy 的 script(把参数按用户回答填好, 不要留空)。\n");
    } else {
        out.push_str(
            "这是**执行组件示例, 不是完整策略**: 需嵌入 on_tick 框架并补齐信号后才能进 preview_strategy。\n",
        );
    }
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
        assert!(find("shannon_rebalance").is_some());
        assert!(code_of("shannon_rebalance").is_some_and(|c| c.contains("on_tick")));
        assert!(find("no-such-template").is_none());
        assert!(code_of("no-such-template").is_none());
    }

    #[test]
    fn test_executor_components_are_not_direct_strategies() {
        // 类别纪律: 只有 shannon_rebalance 是完整策略; 执行组件必须被标成组件(免得被直接部署)
        assert!(find("shannon_rebalance").is_some_and(|t| t.kind.is_direct_strategy()));
        for name in ["dca", "twap", "vwap", "pullback", "ladder"] {
            let t = find(name).unwrap_or_else(|| panic!("缺模板 {name}"));
            assert_eq!(t.kind, TemplateKind::ExecutorComponent, "{name} 应是执行组件");
            assert!(!t.kind.is_direct_strategy());
        }
    }

    #[test]
    fn test_list_text_states_both_routes() {
        let text = list_text();
        for must in ["shannon_rebalance", "完整策略", "执行组件", "read_template", "preview_strategy"]
        {
            assert!(text.contains(must), "清单缺少: {must}");
        }
        assert!(text.contains("shannon_rebalance"), "{text}");
    }

    #[test]
    fn test_render_read_includes_code_and_kind_warning() {
        let grid = render_read(find("shannon_rebalance").unwrap());
        assert!(grid.contains("完整策略"), "{grid}");
        assert!(grid.contains("pair"), "{grid}");
        assert!(grid.contains("on_tick"), "详情须带原文代码");

        let dca = render_read(find("dca").unwrap());
        assert!(dca.contains("不是完整策略"), "执行组件须显式告警: {dca}");
        assert!(dca.contains("interval_secs"), "{dca}");
    }
}
