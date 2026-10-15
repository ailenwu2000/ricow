//! 054 P2: 风险档位 → **具体数字**的映射表(单点定义)。
//!
//! 存在理由(审计 #2): 用户只会说"我想稳一点", 而此前这条自然语言被**原样**塞进提示词 ——
//! "稳一点"对模型没有任何约束力, 保守的人和激进的人说同一句话会得到同一份策略。
//! 所以档位必须落成**数字**, 提示词里出现的是"单笔仓位 ≤ 10%"而不是"保守一点"。
//!
//! **单点定义**是硬要求: 前端"这档意味着什么"的文案、给模型的提示词、落进 TOML 的杠杆值,
//! 三处都从本文件的常量派生。任何一处手抄数字, 改表时就会静默分叉(spec §四-1)。
//!
//! 数值依据见 `specs/changes/054-ai-strategy-flow/spec.md §二` —— 全部取自内置策略的实测值
//! 或引擎硬上限, 不是拍脑袋:
//! - 杠杆 2/3/5x: 内置合约实体策略实测用 2x / 3x; 引擎硬上限 10x。
//! - 网格最小间距 0.4%(现货)/0.2%(合约): 内置 `shannon_grid.min_spacing_pct = 0.004`、
//!   `shannon_grid_futures.min_spacing_pct = 0.002`(都注明"4×费率")。激进档取这两个实证值当底线。
//! - 止损/保护要求**按策略类型分支**, 类型由 AI 自己按 `idea` 判断(spec §三 D2)——
//!   后端**不猜类型**, 只是把两类要求都写进提示词让它自己挑。

use serde::{Deserialize, Serialize};

/// 风险档位(四档)。
///
/// 自定义档(`Custom`)不是"第四列固定数值": 它**没有默认值**, 四个数值字段全必填。
/// 给了默认值就等于偷偷帮用户选了档(spec §二 D3)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RiskTier {
    /// 保守。
    Conservative,
    /// 稳健。
    Balanced,
    /// 激进。
    Aggressive,
    /// 自定义: 数值全部由用户填, **无默认值**。
    Custom,
}

impl RiskTier {
    /// 请求里的字符串 → 档位(非法 → None, 由调用方报 400 并列出合法值)。
    pub(super) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "conservative" | "保守" => Some(Self::Conservative),
            "balanced" | "稳健" => Some(Self::Balanced),
            "aggressive" | "激进" => Some(Self::Aggressive),
            "custom" | "自定义" => Some(Self::Custom),
            _ => None,
        }
    }

    /// 合法取值(400 的提示文案用, 与 parse 同源 —— 免得文案里手抄一份又漂移)。
    pub(super) fn accepted() -> &'static [&'static str] {
        &["conservative", "balanced", "aggressive", "custom"]
    }

    /// 中文名(UI 展示; 前端也可从 `/api/strategies/risk-profiles` 整表取)。
    pub(super) fn label_zh(&self) -> &'static str {
        match self {
            Self::Conservative => "保守",
            Self::Balanced => "稳健",
            Self::Aggressive => "激进",
            Self::Custom => "自定义",
        }
    }

    pub(super) fn label_en(&self) -> &'static str {
        match self {
            Self::Conservative => "Conservative",
            Self::Balanced => "Balanced",
            Self::Aggressive => "Aggressive",
            Self::Custom => "Custom",
        }
    }
}

/// 一档的量化约束(自定义档的字段恒为 `None` —— 等用户填)。
///
/// `None` 的语义是"**这一档不预设这个数**", 不是"不限"。渲染提示词时 `None` 字段**整条不出现**,
/// 绝不能退化成"没有限制"—— 那会让自定义档变成"最激进档"。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(super) struct RiskProfile {
    /// 档位。
    pub(super) tier: RiskTier,
    /// 最大回撤目标(%)。目标是目标 —— 措辞用"目标是 ≤ N%", 不做硬断言(D1)。
    pub(super) max_drawdown_pct: Option<f64>,
    /// 单笔最大仓位(占权益, %)。
    pub(super) max_position_pct: Option<f64>,
    /// 杠杆上限(仅合约; 现货恒为 None)。**这一项可静态验证** —— 会真的写进 TOML。
    pub(super) leverage_cap: Option<f64>,
    /// 网格最小间距: `(现货 %, 合约 %)`。
    pub(super) min_spacing_pct: Option<(f64, f64)>,
    /// 单边加仓: 是否**禁止**(true = 禁止; 激进档为 false = 允许但须有上限)。
    pub(super) forbid_one_side_add: bool,
    /// 换手倾向(定性词, 直接进提示词)。
    pub(super) turnover: &'static str,
}

/// 三档映射表(自定义档不在此列 —— 它由用户填的数拼出来, 见 [`profile_for`])。
pub(super) const RISK_PROFILES: [RiskProfile; 3] = [
    RiskProfile {
        tier: RiskTier::Conservative,
        max_drawdown_pct: Some(10.0),
        max_position_pct: Some(10.0),
        leverage_cap: Some(2.0),
        min_spacing_pct: Some((0.8, 0.4)),
        forbid_one_side_add: true,
        turnover: "低",
    },
    RiskProfile {
        tier: RiskTier::Balanced,
        max_drawdown_pct: Some(20.0),
        max_position_pct: Some(25.0),
        leverage_cap: Some(3.0),
        min_spacing_pct: Some((0.5, 0.25)),
        forbid_one_side_add: true,
        turnover: "中",
    },
    RiskProfile {
        tier: RiskTier::Aggressive,
        max_drawdown_pct: Some(40.0),
        max_position_pct: Some(50.0),
        leverage_cap: Some(5.0),
        min_spacing_pct: Some((0.4, 0.2)),
        // 激进档允许单边加仓, 但**必须有上限** —— 无上限的加仓是爆仓最常见的一条路。
        forbid_one_side_add: false,
        turnover: "高(仍须覆盖成本)",
    },
];

impl RiskProfile {
    /// 按档位取预设档。自定义档落在 [`Self::custom_template`]。
    pub(super) fn of(tier: RiskTier) -> Self {
        RISK_PROFILES.iter().find(|p| p.tier == tier).copied().unwrap_or_else(Self::custom_template)
    }

    /// 自定义档的空壳: 所有数值字段 `None`, 等调用方按用户填的值覆盖。
    ///
    /// `forbid_one_side_add = true` 是**唯一的安全默认** —— 用户没表态时不替他放开加仓。
    pub(super) fn custom_template() -> Self {
        Self {
            tier: RiskTier::Custom,
            max_drawdown_pct: None,
            max_position_pct: None,
            leverage_cap: None,
            min_spacing_pct: None,
            forbid_one_side_add: true,
            turnover: "由你按下面的数字自行判断",
        }
    }
}

// ---------------------------------------------------------------------------
// 055 B3: 档位目标的**事后核对**
// ---------------------------------------------------------------------------

/// 一项目标与实测的对照。
#[derive(Debug, Clone, Serialize)]
pub(super) struct TierCheckItem {
    /// 目标键(前端只用于去重, 不显示)。
    pub(super) key: &'static str,
    pub(super) label_zh: &'static str,
    pub(super) label_en: &'static str,
    /// 档位表里的目标值; 已是百分数(与回测指标 `*_pct` 同口径)。
    pub(super) target: f64,
    /// 实测值; `None` = 这次没测出来(如实说"未知", 不猜)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) actual: Option<f64>,
    /// `"met"` 达标 / `"unmet"` 未达标 / `"unknown"` 测不出来。
    pub(super) verdict: &'static str,
    /// 单位(展示用)。
    pub(super) unit: &'static str,
}

/// 整档的核对结果。
#[derive(Debug, Clone, Serialize)]
pub(super) struct TierCheck {
    /// 档位中文名(如"保守") —— 让用户确认"这就是我选的那档"。
    pub(super) tier_zh: String,
    pub(super) tier_en: String,
    pub(super) items: Vec<TierCheckItem>,
    /// 未达标项数(0 = 全部达标)。
    pub(super) unmet: usize,
}

/// 把档位目标与**回测实测**逐项对照(纯函数, 便于离线单测)。
///
/// 存在理由(055 B3): 档位此前**只对模型有约束力** —— 提示词说"回撤目标 ≤10%", 但生成完
/// 界面只把实测的 25% 显示成一个红数字, 从不说"你选的是 10%, 没达到"。于是"我选了保守档"
/// 这句话在用户侧没有任何反馈闭环: 他得自己记住选了什么、再去比对。
///
/// **只核回撤一项**, 这是刻意的: 其余档位项(单笔仓位/网格间距/杠杆上限)是**代码属性**,
/// 回测指标里根本没有对应数字 —— 凑一个"核对"出来就是编造实测值, 比不核对更糟。
/// 回撤是唯一一个"档位里有数、回测里也有数"的项。
///
/// 措辞纪律(D1 同源): 档位写的是**目标**不是断言, 所以判语是"未达到目标"而非"违规/失败";
/// 测不出来就标 `unknown`, 不猜也不省略。
pub(super) fn tier_check(
    p: &RiskProfile,
    metrics: Option<&serde_json::Value>,
) -> Option<TierCheck> {
    // 没跑回测(用户跳过 / 跑失败)就没有实测值 → **不下发**核对框, 而不是摆一个
    // "目标 10% / 实测 —" 的空壳: 那会用界面暗示"已经核过了"。
    let m = metrics?;
    // 自定义档: 数字由用户填, 而那几个数不在回测指标里, 所以**不核**。
    let target = p.max_drawdown_pct?;
    let actual = m.get("max_drawdown_pct").and_then(|v| v.as_f64()).map(|v| v.abs());
    let verdict = match actual {
        None => "unknown",
        // 回撤是"越小越好", 实测超过目标即未达到(相等算达标)。
        Some(a) if a > target => "unmet",
        Some(_) => "met",
    };
    let item = TierCheckItem {
        key: "max_drawdown_pct",
        label_zh: "最大回撤",
        label_en: "Max drawdown",
        target,
        actual,
        verdict,
        unit: "%",
    };
    Some(TierCheck {
        tier_zh: p.tier.label_zh().to_string(),
        tier_en: p.tier.label_en().to_string(),
        unmet: usize::from(verdict == "unmet"),
        items: vec![item],
    })
}

/// 一档的含义(给前端的一行话): 从常量派生, 不手抄。
///
/// 前端**必须**展示这一句 —— 用户选"保守"时得知道这档到底意味着 10% 还是 40%,
/// 否则档位只是三个没有信息的按钮。
pub(super) fn tier_meaning_zh(p: &RiskProfile) -> String {
    match p.tier {
        RiskTier::Custom => "按你自己填的数字生成 —— 界面不代你解读, 也不给默认值。".to_string(),
        _ => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(d) = p.max_drawdown_pct {
                parts.push(format!("回撤目标 ≤ {d}%"));
            }
            if let Some(pos) = p.max_position_pct {
                parts.push(format!("单笔仓位 ≤ {pos}%"));
            }
            if let Some(lv) = p.leverage_cap {
                parts.push(format!("合约杠杆上限 {lv}x(会真的写进配置)"));
            }
            if let Some((spot, fut)) = p.min_spacing_pct {
                parts.push(format!("网格最小间距 现货 {spot}% / 合约 {fut}%"));
            }
            parts.push(if p.forbid_one_side_add {
                "禁止单边加仓".to_string()
            } else {
                "允许单边加仓(但必须有上限)".to_string()
            });
            parts.push(format!("换手倾向 {}", p.turnover));
            format!("{}: {}", p.tier.label_zh(), parts.join("; "))
        }
    }
}

/// 英文版含义(与 [`tier_meaning_zh`] 同源派生, 不是另写一份)。
pub(super) fn tier_meaning_en(p: &RiskProfile) -> String {
    match p.tier {
        RiskTier::Custom => {
            "Generated from the numbers you enter — the UI does not interpret them for you, and provides no defaults.".to_string()
        }
        _ => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(d) = p.max_drawdown_pct {
                parts.push(format!("drawdown target ≤ {d}%"));
            }
            if let Some(pos) = p.max_position_pct {
                parts.push(format!("position per trade ≤ {pos}%"));
            }
            if let Some(lv) = p.leverage_cap {
                parts.push(format!("futures leverage cap {lv}x (really written into the config)"));
            }
            if let Some((spot, fut)) = p.min_spacing_pct {
                parts.push(format!("grid min spacing spot {spot}% / futures {fut}%"));
            }
            parts.push(if p.forbid_one_side_add {
                "no one-sided averaging down".to_string()
            } else {
                "one-sided averaging allowed (but must be capped)".to_string()
            });
            format!("{}: {}", p.tier.label_en(), parts.join("; "))
        }
    }
}

/// 给前端的一档说明(JSON)—— **数字全部从常量派生**, 前端不手抄一份。
///
/// 存在理由(spec §四-1): 若前端自己写死"保守 = 回撤 10%", 改表时两处就会静默分叉,
/// 用户看到的档位说明和真正生效的约束不是一回事 —— 这比没有说明更糟。
#[derive(Debug, Clone, Serialize)]
pub(super) struct RiskProfileDoc {
    /// 档位键(与 `risk_tolerance` 请求字段同源)。
    pub(super) key: &'static str,
    pub(super) label_zh: &'static str,
    pub(super) label_en: &'static str,
    /// 「选了这档意味着什么」—— 含全部数字。
    pub(super) meaning_zh: String,
    pub(super) meaning_en: String,
    pub(super) max_drawdown_pct: Option<f64>,
    pub(super) max_position_pct: Option<f64>,
    /// 杠杆上限(`null` = 该档不预设 / 现货不使用杠杆)。
    pub(super) leverage_cap: Option<f64>,
    pub(super) min_spacing_spot_pct: Option<f64>,
    pub(super) min_spacing_futures_pct: Option<f64>,
    pub(super) forbid_one_side_add: bool,
    /// 自定义档为 true: 前端据此展开 4 个数值字段且**不给默认值**。
    pub(super) needs_custom_fields: bool,
}

/// 四档的完整说明(顺序: 保守 / 稳健 / 激进 / 自定义)。
pub(super) fn docs() -> Vec<RiskProfileDoc> {
    let mut out: Vec<RiskProfileDoc> = RISK_PROFILES
        .iter()
        .map(|p| RiskProfileDoc {
            key: tier_key(p.tier),
            label_zh: p.tier.label_zh(),
            label_en: p.tier.label_en(),
            meaning_zh: tier_meaning_zh(p),
            meaning_en: tier_meaning_en(p),
            max_drawdown_pct: p.max_drawdown_pct,
            max_position_pct: p.max_position_pct,
            leverage_cap: p.leverage_cap,
            min_spacing_spot_pct: p.min_spacing_pct.map(|(s, _)| s),
            min_spacing_futures_pct: p.min_spacing_pct.map(|(_, f)| f),
            forbid_one_side_add: p.forbid_one_side_add,
            needs_custom_fields: false,
        })
        .collect();
    let c = RiskProfile::custom_template();
    out.push(RiskProfileDoc {
        key: tier_key(c.tier),
        label_zh: c.tier.label_zh(),
        label_en: c.tier.label_en(),
        meaning_zh: tier_meaning_zh(&c),
        meaning_en: tier_meaning_en(&c),
        max_drawdown_pct: None,
        max_position_pct: None,
        leverage_cap: None,
        min_spacing_spot_pct: None,
        min_spacing_futures_pct: None,
        forbid_one_side_add: c.forbid_one_side_add,
        needs_custom_fields: true,
    });
    out
}

/// 档位 → 请求里的字符串(与 `RiskTier::parse` 互为逆运算, 且取自 `accepted()` 同一族值)。
fn tier_key(t: RiskTier) -> &'static str {
    match t {
        RiskTier::Conservative => "conservative",
        RiskTier::Balanced => "balanced",
        RiskTier::Aggressive => "aggressive",
        RiskTier::Custom => "custom",
    }
}

/// 渲染成给模型的约束文字(提示词用)。
///
/// 两条措辞纪律(spec §四-4):
/// - 除"杠杆上限"外一律用"**目标是**…" —— 那些项**不可静态验证**, 写"必须"是在撒谎;
/// - 杠杆上限用"必须"并真的写进 TOML, 由 `validate_leverage` 执行 —— 这是唯一可验证的一项。
pub(super) fn constraints_text(p: &RiskProfile, market: &str, initial_cash: f64) -> String {
    let is_futures = market == "futures";
    let mut out = String::new();
    out.push_str(&format!("【资金】用户这个策略准备投入 {initial_cash} USDT。\n"));
    out.push_str("【风险档位】");
    out.push_str(p.tier.label_zh());
    out.push('\n');
    if let Some(d) = p.max_drawdown_pct {
        out.push_str(&format!("- 最大回撤: 目标是 ≤ {d}%(这是目标, 历史回测达不到不必硬凑)\n"));
    }
    if let Some(pos) = p.max_position_pct {
        out.push_str(&format!("- 单笔最大仓位: 目标是 ≤ 权益的 {pos}%\n"));
    }
    if is_futures {
        if let Some(lv) = p.leverage_cap {
            // 唯一可静态验证项: 用"必须", 且真的写进 TOML。
            out.push_str(&format!(
                "- 杠杆: **必须** ≤ {lv}x(这一项会被写进策略配置并真正校验, 超限会被拒绝)\n"
            ));
        }
    } else {
        out.push_str("- 杠杆: 现货不使用杠杆(不要写出依赖杠杆的逻辑)\n");
    }
    if let Some((spot, fut)) = p.min_spacing_pct {
        let v = if is_futures { fut } else { spot };
        out.push_str(&format!(
            "- 网格最小间距: 若写成网格/分批挂单类策略, 目标是 ≥ {v}%(低于它会被手续费磨损吃掉)\n"
        ));
    }
    out.push_str(if p.forbid_one_side_add {
        "- 单边加仓: **禁止**(亏损后同方向继续加仓是最常见的爆仓路径)\n"
    } else {
        "- 单边加仓: 允许, 但**必须有明确上限**(次数或金额), 不允许无上限摊平\n"
    });
    out.push_str(&format!("- 换手倾向: {}\n", p.turnover));
    out.push_str(protection_text());
    out
}

/// D2: 保护要求按**策略类型**分支 —— 类型由 AI 按 `idea` 自己判断, 后端不猜。
///
/// 两类都写在这里, 让它自己挑: 只写"必须有止损"会把网格类策略逼成四不像
/// (网格的自我保护机制是再平衡本身, 硬塞止损反而让它永远跑不出信号)。
pub(super) fn protection_text() -> &'static str {
    "- 保护要求(按你判断的策略类型二选一, 并在代码里真的实现):\n\
     · 趋势 / 突破 / 择时类: 必须有**明确止损** —— 价格或回撤触线即平仓; 禁止无止损的逆势加仓。\n\
     · 网格 / 再平衡类: 等价保护三件套 —— ① 仓位上限(见上); ② 最小间距下限(见上); \n\
     \x20 ③ 合约须有爆仓/深亏监控(参照内置 `liq_warn_ratio = 0.1` 的量级在 `on_tick` 里告警或降仓)。\n\
     \x20 这类不强行要求止损, 但上面三件必须齐。"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三处一致(spec §四-1): 常量 → 中文含义 → 英文含义, 数字必须**同源**。
    /// 最稳的断言方式不是比对两串文字, 而是断言"含义串里出现了常量里的那个数"。
    #[test]
    fn test_tier_meaning_derives_from_constants() {
        for p in RISK_PROFILES {
            let zh = tier_meaning_zh(&p);
            let en = tier_meaning_en(&p);
            if let Some(d) = p.max_drawdown_pct {
                assert!(zh.contains(&trim_zero(d)), "中文含义缺回撤数: {zh}");
            }
            if let Some(pos) = p.max_position_pct {
                assert!(zh.contains(&trim_zero(pos)), "中文含义缺仓位数: {zh}");
            }
            if let Some(lv) = p.leverage_cap {
                assert!(zh.contains(&trim_zero(lv)), "中文含义缺杠杆数: {zh}");
                assert!(en.contains(&trim_zero(lv)), "英文含义缺杠杆数: {en}");
            }
            assert!(!zh.is_empty() && !en.is_empty());
        }
    }

    fn trim_zero(v: f64) -> String {
        if (v - v.round()).abs() < 1e-9 {
            format!("{}", v.round() as i64)
        } else {
            format!("{v}")
        }
    }

    /// 数值依据回归: 激进档的网格最小间距必须等于内置策略的实证值(0.4% 现货 / 0.2% 合约)。
    /// 改这两个数等于推翻"数值来自实测"这条依据, 必须是有意的。
    #[test]
    fn test_aggressive_grid_spacing_matches_builtin_evidence() {
        let a = RiskProfile::of(RiskTier::Aggressive);
        assert_eq!(a.min_spacing_pct, Some((0.4, 0.2)));
        // 保守档 ×2、稳健档 ×1.25(相对激进档)。
        let c = RiskProfile::of(RiskTier::Conservative);
        let b = RiskProfile::of(RiskTier::Balanced);
        assert_eq!(c.min_spacing_pct, Some((0.8, 0.4)));
        assert_eq!(b.min_spacing_pct, Some((0.5, 0.25)));
    }

    /// 自定义档**不能有默认值** —— 给了默认值就等于偷偷帮用户选了档(D3)。
    #[test]
    fn test_custom_tier_has_no_defaults() {
        let p = RiskProfile::custom_template();
        assert!(p.max_drawdown_pct.is_none(), "自定义档不得预设回撤");
        assert!(p.max_position_pct.is_none(), "自定义档不得预设仓位");
        assert!(p.leverage_cap.is_none(), "自定义档不得预设杠杆");
        assert!(p.min_spacing_pct.is_none(), "自定义档不得预设间距");
        // 唯一的安全默认: 没表态时不替用户放开加仓。
        assert!(p.forbid_one_side_add, "自定义档默认禁止单边加仓");
    }

    /// 提示词必须含**每个数字**(spec §四-3)—— 否则字段就是装饰。
    /// 且"目标"与"必须"的措辞要分家: 只有杠杆是"必须"。
    #[test]
    fn test_constraints_text_carries_every_number_and_split_wording() {
        let p = RiskProfile::of(RiskTier::Balanced);
        let s = constraints_text(&p, "futures", 50_000.0);
        assert!(s.contains("50,000") || s.contains("50000"), "本金必须出现: {s}");
        for n in ["20", "25", "3", "0.25"] {
            assert!(s.contains(n), "提示词缺数字 {n}: {s}");
        }
        assert!(s.contains("**必须** ≤ 3x"), "杠杆是唯一可静态验证项, 必须用「必须」: {s}");
        assert!(s.contains("目标是"), "其余项必须是「目标」措辞: {s}");
        // 现货: 杠杆那段必须改成"不使用杠杆", 不能还写着 3x。
        let spot = constraints_text(&p, "spot", 10_000.0);
        assert!(spot.contains("不使用杠杆"), "现货不该出现杠杆上限: {spot}");
        assert!(!spot.contains("**必须** ≤ 3x"), "现货不得下达杠杆约束: {spot}");
    }

    /// D2: 保护要求必须**两类都在**(网格/趋势), 由 AI 自己挑 —— 只写一类会误伤另一种形态。
    #[test]
    fn test_protection_text_covers_both_strategy_shapes() {
        let s = protection_text();
        assert!(s.contains("趋势"), "缺趋势类保护要求: {s}");
        assert!(s.contains("网格"), "缺网格类保护要求: {s}");
        assert!(s.contains("止损"), "趋势类必须点名止损: {s}");
        assert!(s.contains("爆仓"), "网格类必须点名爆仓监控: {s}");
    }

    /// 下发给前端的 JSON 必须**自洽**: ① key 能被 parse 回来(否则用户选了却提交不上去);
    /// ② 每一档的每个数字都出现在它的"意味着什么"里(否则说明文案在骗人);
    /// ③ 自定义档标记 needs_custom_fields 且数字全为 None(不给默认值)。
    #[test]
    fn test_docs_round_trip_and_carry_their_own_numbers() {
        let docs = docs();
        assert_eq!(docs.len(), 4, "四档: 保守/稳健/激进/自定义");
        for d in &docs {
            assert!(
                RiskTier::parse(d.key).is_some(),
                "下发的档位键 '{}' 必须能被 parse 接受",
                d.key
            );
            if d.needs_custom_fields {
                assert!(d.max_drawdown_pct.is_none(), "自定义档不得带默认回撤");
                assert!(d.max_position_pct.is_none(), "自定义档不得带默认仓位");
                assert!(d.leverage_cap.is_none(), "自定义档不得带默认杠杆");
                continue;
            }
            for n in [d.max_drawdown_pct, d.max_position_pct, d.leverage_cap].into_iter().flatten()
            {
                assert!(
                    d.meaning_zh.contains(&trim_zero(n)),
                    "档位 '{}' 的说明缺数字 {}: {}",
                    d.key,
                    n,
                    d.meaning_zh
                );
            }
            for n in [d.min_spacing_spot_pct, d.min_spacing_futures_pct].into_iter().flatten() {
                assert!(
                    d.meaning_zh.contains(&format!("{n}")),
                    "档位 '{}' 的说明缺间距 {}: {}",
                    d.key,
                    n,
                    d.meaning_zh
                );
            }
        }
    }

    #[test]
    fn test_parse_tier_accepts_both_zh_and_en() {
        assert_eq!(RiskTier::parse("conservative"), Some(RiskTier::Conservative));
        assert_eq!(RiskTier::parse("保守"), Some(RiskTier::Conservative));
        assert_eq!(RiskTier::parse(" AGGRESSIVE "), Some(RiskTier::Aggressive));
        assert_eq!(RiskTier::parse("自定义"), Some(RiskTier::Custom));
        assert_eq!(RiskTier::parse("随便"), None);
        // 合法取值列表与 parse 同源。
        for v in RiskTier::accepted() {
            assert!(RiskTier::parse(v).is_some(), "accepted 里的 {v} 必须能被 parse");
        }
    }

    // ---- 055 B3: 档位目标的事后核对 ----

    fn metrics_dd(v: f64) -> serde_json::Value {
        serde_json::json!({ "max_drawdown_pct": v, "net_pnl": 1.0 })
    }

    #[test]
    fn test_tier_check_says_unmet_when_drawdown_exceeds_the_target() {
        let p = RiskProfile::of(RiskTier::Conservative); // 回撤目标 10%
                                                         // 实测 5.06% → 达到目标。
        let ok = tier_check(&p, Some(&metrics_dd(5.06))).expect("应产出核对");
        assert_eq!(ok.items[0].verdict, "met");
        assert_eq!(ok.unmet, 0);
        assert_eq!(ok.tier_zh, "保守");
        assert!((ok.items[0].actual.unwrap() - 5.06).abs() < 1e-9);

        // 实测 25.3% → 未达到(这是 B3 存在的理由: 界面此前只把 25.3% 显示成红数字,
        // 从不说"你选的是 10%")。
        let bad = tier_check(&p, Some(&metrics_dd(25.3))).expect("应产出核对");
        assert_eq!(bad.items[0].verdict, "unmet");
        assert_eq!(bad.unmet, 1);
        assert_eq!(bad.items[0].target, 10.0, "目标必须来自档位表, 不是硬编码");

        // 恰好等于目标算达标(边界: 不要因为浮点差把"刚好做到"判成没做到)。
        assert_eq!(tier_check(&p, Some(&metrics_dd(10.0))).unwrap().items[0].verdict, "met");
    }

    #[test]
    fn test_tier_check_refuses_to_invent_a_number() {
        let p = RiskProfile::of(RiskTier::Conservative);
        // ① 没跑回测(用户跳过 / 跑失败) → 不产出核对框, 而不是摆一个"目标 10% / 实测 —"。
        assert!(tier_check(&p, None).is_none(), "没有实测值就不该下发核对");
        // ② 指标里没有这一项 → 如实标 unknown, 不填 0 冒充实测。
        let empty = serde_json::json!({ "net_pnl": 1.0 });
        let u = tier_check(&p, Some(&empty)).expect("档位有目标就应产出核对");
        assert_eq!(u.items[0].verdict, "unknown");
        assert!(u.items[0].actual.is_none());
        assert_eq!(u.unmet, 0, "测不出来不算未达标");
        // ③ 自定义档没有档位目标 → 不核(数字是他自己填的, 不在回测指标里)。
        assert!(
            tier_check(&RiskProfile::custom_template(), Some(&metrics_dd(3.0))).is_none(),
            "自定义档不该被核"
        );
    }

    /// 只核回撤: 其余档位项(仓位/间距/杠杆)在回测指标里**没有对应数字**,
    /// 凑一个"核对"出来就是编造实测值。这条把范围钉住。
    #[test]
    fn test_tier_check_only_covers_what_the_backtest_can_measure() {
        let p = RiskProfile::of(RiskTier::Aggressive);
        let tc = tier_check(&p, Some(&metrics_dd(50.0))).unwrap();
        assert_eq!(tc.items.len(), 1, "一次只核回撤一项: {tc:?}");
        assert_eq!(tc.items[0].key, "max_drawdown_pct");
        // 激进档目标 40% → 实测 50% 未达到(目标随档位走, 不是写死的 10)。
        assert_eq!(tc.items[0].target, 40.0);
        assert_eq!(tc.items[0].verdict, "unmet");
    }
}
