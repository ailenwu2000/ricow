//! `ricow exposure` — 跨策略组合敞口只读视图 (037 P0-C / FR-3)。
//!
//! 只做一件事: 把本地库里**所有**策略的持仓与未终结挂单聚合成一张"我到底押了多少"的表。
//! 背景与设计取舍见 `ricow_engine::exposure` 的模块文档 —— 三条硬约束在这里同样成立:
//! **只展示不拦截**(宪法: 平台不做投资判断)、**数据源只有本地库**(不直连交易所、不需要密钥)、
//! **绝不跨模式相加**(dry_run 是模拟的, 与实盘相加会造出看似精确实则错误的数字)。

use std::fmt::Write as _;
use std::path::Path;

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_engine::{aggregate_with, ExposureFilter, ExposureView, PairExposure, StrategyExposure};
use ricow_strategy::{Database, SqlxResultExt};

use crate::commands::{pad_display, pad_display_right};

#[derive(Args)]
pub struct ExposureArgs {
    /// 只看某个交易对 (大小写不敏感; 缺省为全部)
    #[arg(long)]
    pub pair: Option<String>,
    /// 只看某个模式: live | demo | dry_run (缺省为全部)
    #[arg(long)]
    pub mode: Option<String>,
    /// 下钻: 追加"策略 × 标的 × 模式"明细 (含已无敞口的历史行)
    #[arg(long)]
    pub detail: bool,
    /// 扫描最近多少条订单来统计挂单数 (挂单数只覆盖这些订单)
    #[arg(long, default_value_t = 500)]
    pub limit: i64,
}

pub async fn run(args: ExposureArgs) -> CoreResult<()> {
    print!("{}", format_exposure(&crate::commands::project_root(), &args).await?);
    Ok(())
}

/// 读取本地库并渲染视图 (与 CLI 入口分离, 便于按会话 root 复用)。
pub(crate) async fn format_exposure(root: &Path, args: &ExposureArgs) -> CoreResult<String> {
    let filter = build_filter(args)?;
    let limit = args.limit.max(1);
    let db = Database::open(&crate::commands::db_path_in(root)).await.core()?;
    // `None` = 全部策略 —— 这正是本视图与"按策略查看持仓"的区别:
    // 它要回答的是"合起来多少", 不是"某一个多少"。
    let positions = db
        .current_positions(None)
        .await
        .map_err(|e| CoreError::InvalidArgument(format!("查询持仓失败: {e}")))?;
    let orders = db
        .recent_orders(None, limit)
        .await
        .map_err(|e| CoreError::InvalidArgument(format!("查询订单失败: {e}")))?;

    let view = aggregate_with(&positions, &orders, &filter);
    Ok(render(&view, &filter, limit, args.detail))
}

/// `--mode` 校验 (与 `ricow db --market` 同口径: 拼错不静默给空结果, 直接报错)。
///
/// 沉默的空表会被当成"我没有敞口" —— 在一个资金安全视图上, 这个误解比报错危险得多。
fn build_filter(args: &ExposureArgs) -> CoreResult<ExposureFilter> {
    if let Some(m) = args.mode.as_deref() {
        if !matches!(m, "live" | "demo" | "dry_run") {
            return Err(CoreError::InvalidArgument(format!(
                "--mode 仅支持 live|demo|dry_run, 收到 '{m}'"
            )));
        }
    }
    Ok(ExposureFilter { pair: args.pair.clone(), mode: args.mode.clone() })
}

/// 表格里的短模式标签 (完整说法如"测试网模拟盘(demo)"太长, 会把表挤歪)。
fn short_mode(mode: &str) -> String {
    match mode {
        "live" => "实盘".into(),
        "demo" => "测试网".into(),
        "dry_run" => "DryRun".into(),
        "" => "未知".into(),
        other => other.to_string(),
    }
}

/// 列对齐方式。
#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
}

/// 拼一行表格: 单元格按**显示宽度**补位 (CJK 算 2 列), 行尾空白裁掉。
fn row(cells: &[(&str, usize, Align)]) -> String {
    let mut out = String::new();
    for (i, (s, w, a)) in cells.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(
            match a {
                Align::Left => pad_display(s, *w),
                Align::Right => pad_display_right(s, *w),
            }
            .as_str(),
        );
    }
    out.trim_end().to_string()
}

/// 合计表列宽 (显示宽度): 标的 / 模式 / 净头寸 / 多头合计 / 空头合计 / 总名义 / 挂单 / 策略 / 最近变动。
const SUM_W: [usize; 9] = [12, 8, 16, 16, 16, 16, 6, 6, 20];
/// 明细表列宽: 策略 / 标的 / 模式 / 净头寸 / 开仓均价 / 挂单 / 最近变动。
const DET_W: [usize; 7] = [24, 12, 8, 16, 14, 6, 20];

fn table_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len().saturating_sub(1)
}

/// 渲染 (纯函数): 同一份 view + filter 必然输出同一段文本。
pub(crate) fn render(
    view: &ExposureView,
    filter: &ExposureFilter,
    limit: i64,
    detail: bool,
) -> String {
    let mut out = String::new();
    line!(out, "组合敞口 (只读; 数据源 = 本地库 positions/orders, 不直连交易所)");
    line!(
        out,
        "本地库参与聚合: {} 条持仓行 / {} 条订单 (最近 {} 条上限)",
        view.positions_scanned,
        view.orders_scanned,
        limit
    );
    if filter.is_active() {
        line!(
            out,
            "筛选: 标的={} 模式={}",
            filter.pair.as_deref().unwrap_or("全部"),
            filter.mode.as_deref().unwrap_or("全部")
        );
    }

    if view.is_empty() {
        if filter.is_active() {
            line!(out, "该筛选下本地库无匹配记录 (无持仓、无挂单)");
        } else {
            line!(out, "无持仓、无挂单 (本地库 positions/orders 为空)");
        }
        line!(
            out,
            "提示: 本视图只读本地库 —— 策略跑过并落库后才有数据; 交易所侧实时状态请用 `ricow info <策略>`"
        );
        return out;
    }

    // 挂单数被 `--limit` 截断时必须如实说明: 把"我只看了最近 500 条"说成"没有挂单"是误导。
    if view.orders_scanned >= limit as usize {
        line!(
            out,
            "注意: 挂单数只统计了最近 {} 条订单 —— 更早的挂单可能未计入 (可用 --limit 调大)",
            limit
        );
    }

    // 已无敞口的组合 (头寸全 0 且无挂单) 不进表: 它们答不了"我押了多少", 只会把真数字挤散。
    // 但**不静默丢弃** —— 下面如实说明漏了多少, `--detail` 看得到。
    let shown: Vec<&PairExposure> = view.pairs.iter().filter(|e| e.strategies > 0).collect();
    let hidden = view.pairs.len() - shown.len();

    line!(out, "");
    if shown.is_empty() {
        line!(
            out,
            "当前无敞口: 本地库有 {} 个 (标的 × 模式) 组合, 但头寸全为 0 且无未终结挂单",
            view.pairs.len()
        );
        line!(
            out,
            "      (历史记录加 --detail 可查; 这不表示交易所侧一定没有仓位 —— 本视图只读本地库)"
        );
    } else {
        line!(
            out,
            "{}",
            row(&[
                ("标的", SUM_W[0], Align::Left),
                ("模式", SUM_W[1], Align::Left),
                ("净头寸", SUM_W[2], Align::Right),
                ("多头合计", SUM_W[3], Align::Right),
                ("空头合计", SUM_W[4], Align::Right),
                ("总名义(估值)", SUM_W[5], Align::Right),
                ("挂单", SUM_W[6], Align::Right),
                ("策略", SUM_W[7], Align::Right),
                ("最近变动", SUM_W[8], Align::Left),
            ])
        );
        line!(out, "{}", "-".repeat(table_width(&SUM_W)));
        for e in &shown {
            line!(out, "{}", pair_line(e));
        }
    }

    let incomplete = view.pairs.iter().any(|e| e.notional_incomplete);
    line!(out, "");
    line!(
        out,
        "合计 {} 个 (标的 × 模式) 组合 (其中 {} 个当前有敞口), {} 笔未终结挂单",
        view.pairs.len(),
        shown.len(),
        view.open_orders_total
    );
    // shown 为空时上面那段"当前无敞口"已经把话说完了, 不再重复一遍。
    if hidden > 0 && !shown.is_empty() {
        line!(out, "另有 {hidden} 个组合已无敞口 (头寸 0 且无挂单), 未列于上表; 加 --detail 可查");
    }
    line!(
        out,
        "口径: 净头寸 = 多 − 空 (同一标的上不同策略反向持仓会相互抵消, 但**两边的手续费都真实发生了**)"
    );
    line!(
        out,
        "      总名义 = Σ|头寸| × 开仓均价, 是**规模估算**不是实时市值; 也不跨交易对汇总 (报价资产未必一致)"
    );
    line!(
        out,
        "      不同模式 (实盘/测试网/DryRun) **分开成行, 绝不相加** —— 模拟持仓不能混进实盘敞口"
    );
    if incomplete {
        line!(out, "      带 * 的总名义**被低估**: 有头寸但本地未记录开仓均价, 那部分无法计入");
    }

    if detail {
        line!(out, "");
        line!(out, "明细 (策略 × 标的 × 模式; 含已无敞口的历史行):");
        line!(
            out,
            "{}",
            row(&[
                ("策略", DET_W[0], Align::Left),
                ("标的", DET_W[1], Align::Left),
                ("模式", DET_W[2], Align::Left),
                ("净头寸", DET_W[3], Align::Right),
                ("开仓均价", DET_W[4], Align::Right),
                ("挂单", DET_W[5], Align::Right),
                ("最近变动", DET_W[6], Align::Left),
            ])
        );
        line!(out, "{}", "-".repeat(table_width(&DET_W)));
        for r in &view.rows {
            line!(out, "{}", detail_line(r));
        }
    }
    out
}

/// 合计行 (抽出来是为了单测能直接断言 `*` 标记与列口径)。
fn pair_line(e: &PairExposure) -> String {
    let mode = short_mode(&e.mode);
    let net = e.net_size.normalize().to_string();
    let long = e.long_size.normalize().to_string();
    let short = e.short_size.normalize().to_string();
    let notional =
        format!("{}{}", e.gross_notional.normalize(), if e.notional_incomplete { "*" } else { "" });
    let orders = e.open_orders.to_string();
    let strategies = e.strategies.to_string();
    let ts = fmt_ts(e.updated_at);
    row(&[
        (&e.pair, SUM_W[0], Align::Left),
        (&mode, SUM_W[1], Align::Left),
        (&net, SUM_W[2], Align::Right),
        (&long, SUM_W[3], Align::Right),
        (&short, SUM_W[4], Align::Right),
        (&notional, SUM_W[5], Align::Right),
        (&orders, SUM_W[6], Align::Right),
        (&strategies, SUM_W[7], Align::Right),
        (&ts, SUM_W[8], Align::Left),
    ])
}

/// 明细行。
fn detail_line(r: &StrategyExposure) -> String {
    let mode = short_mode(&r.mode);
    let size = r.size.normalize().to_string();
    let entry = r.entry_price.normalize().to_string();
    let orders = r.open_orders.to_string();
    let ts = fmt_ts(r.updated_at);
    row(&[
        (&r.strategy_id, DET_W[0], Align::Left),
        (&r.pair, DET_W[1], Align::Left),
        (&mode, DET_W[2], Align::Left),
        (&size, DET_W[3], Align::Right),
        (&entry, DET_W[4], Align::Right),
        (&orders, DET_W[5], Align::Right),
        (&ts, DET_W[6], Align::Left),
    ])
}

fn fmt_ts(ms: i64) -> String {
    if ms <= 0 {
        return "-".into();
    }
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_engine::exposure::aggregate;
    use ricow_strategy::{OrderRecord, PositionRecord};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn pos(
        strategy: &str,
        pair: &str,
        mode: &str,
        size: Decimal,
        entry: Decimal,
    ) -> PositionRecord {
        PositionRecord {
            strategy_id: strategy.to_string(),
            pair: pair.to_string(),
            size,
            entry_price: entry,
            mode: mode.to_string(),
            updated_at: 1_700_000_000_000,
        }
    }

    fn open_order(strategy: &str, pair: &str, mode: &str, status: &str) -> OrderRecord {
        OrderRecord {
            strategy_id: strategy.to_string(),
            exchange_order_id: format!("EX-{strategy}"),
            client_order_id: format!("{strategy}-1"),
            pair: pair.to_string(),
            side: "buy".to_string(),
            price: dec!(100),
            size: dec!(1),
            filled_size: Decimal::ZERO,
            status: status.to_string(),
            mode: mode.to_string(),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
        }
    }

    /// `--mode` 拼错必须报错, 而不是静默给一张空表 (沉默的空表会被当成"我没有敞口")。
    #[test]
    fn build_filter_rejects_unknown_mode() {
        let ok = ExposureArgs { pair: None, mode: Some("live".into()), detail: false, limit: 500 };
        assert_eq!(build_filter(&ok).unwrap().mode.as_deref(), Some("live"));
        for bad in ["", "Live", "LIVE", "real", "testnet"] {
            let a = ExposureArgs { pair: None, mode: Some(bad.into()), detail: false, limit: 500 };
            let err = build_filter(&a).unwrap_err().to_string();
            assert!(err.contains("--mode 仅支持 live|demo|dry_run"), "{bad} → {err}");
        }
        let none = ExposureArgs { pair: None, mode: None, detail: false, limit: 500 };
        assert!(!build_filter(&none).unwrap().is_active());
    }

    /// 空态: 如实说"本地库为空", 并给出"数据从哪来"的下一步 (与 018/026 的空态纪律一致)。
    #[test]
    fn render_empty_state_is_honest_and_actionable() {
        let view = aggregate(&[], &[]);
        let out = render(&view, &ExposureFilter::default(), 500, false);
        assert!(out.contains("无持仓、无挂单 (本地库 positions/orders 为空)"), "{out}");
        assert!(out.contains("ricow info <策略>"), "空态须给出下一步: {out}");
        assert!(!out.contains("总名义(估值)"), "空态不该打印表头: {out}");
    }

    /// 筛选后无命中: 不能说成"你没有敞口", 而要说"该筛选下无匹配"。
    #[test]
    fn render_filtered_empty_state_distinguishes_from_true_empty() {
        let view = aggregate(&[], &[]);
        let f = ExposureFilter { pair: Some("DOGEUSDT".into()), mode: None };
        let out = render(&view, &f, 500, false);
        assert!(out.contains("该筛选下本地库无匹配记录"), "{out}");
        assert!(out.contains("筛选: 标的=DOGEUSDT 模式=全部"), "{out}");
    }

    /// 对冲组合: 净头寸归零但名义敞口不为零 —— 视图必须把两者**同时**摆出来。
    #[test]
    fn render_shows_netting_and_gross_side_by_side() {
        let ps = [
            pos("grid_a", "ETHUSDT", "live", dec!(1), dec!(2000)),
            pos("grid_b", "ETHUSDT", "live", dec!(-1), dec!(2000)),
        ];
        let view = aggregate(&ps, &[]);
        let out = render(&view, &ExposureFilter::default(), 500, false);
        assert!(out.contains("口径: 净头寸 = 多 − 空"), "{out}");
        assert!(out.contains("手续费都真实发生了"), "{out}");
        assert!(out.contains("合计 1 个 (标的 × 模式) 组合 (其中 1 个当前有敞口)"), "{out}");
        assert!(out.contains("4000"), "名义敞口须可见: {out}");
    }

    /// 名义被低估时必须有 `*` 与脚注 —— 不给"看起来完整"的假数字。
    #[test]
    fn render_flags_understated_notional() {
        let ps = [pos("a", "ETHUSDT", "live", dec!(1), Decimal::ZERO)];
        let view = aggregate(&ps, &[]);
        let out = render(&view, &ExposureFilter::default(), 500, false);
        assert!(out.contains('*'), "{out}");
        assert!(out.contains("被低估"), "{out}");
    }

    /// 订单被 `--limit` 截断时如实说明; 未触顶时不得无端告警 (狼来了会让人忽略真告警)。
    #[test]
    fn render_warns_only_when_order_scan_hit_the_limit() {
        let many: Vec<OrderRecord> =
            (0..5).map(|i| open_order(&format!("s{i}"), "ETHUSDT", "live", "open")).collect();
        let view = aggregate(&[], &many);
        let truncated = render(&view, &ExposureFilter::default(), 5, false);
        assert!(truncated.contains("更早的挂单可能未计入"), "{truncated}");
        let not_truncated = render(&view, &ExposureFilter::default(), 500, false);
        assert!(!not_truncated.contains("更早的挂单可能未计入"), "{not_truncated}");
    }

    /// 明细默认不打印, `--detail` 才下钻 —— 且明细里平仓行 (头寸 0) 也要如实列出。
    #[test]
    fn render_detail_is_opt_in_and_lists_flat_rows() {
        let ps = [
            pos("still_on", "ETHUSDT", "live", dec!(2), dec!(100)),
            pos("flat", "ETHUSDT", "live", Decimal::ZERO, Decimal::ZERO),
        ];
        let view = aggregate(&ps, &[]);
        let summary = render(&view, &ExposureFilter::default(), 500, false);
        assert!(!summary.contains("明细 (策略 × 标的 × 模式"), "{summary}");
        let d = render(&view, &ExposureFilter::default(), 500, true);
        assert!(d.contains("明细 (策略 × 标的 × 模式"), "{d}");
        assert!(d.contains("still_on"), "{d}");
        assert!(d.contains("flat"), "平仓行不该被藏起来: {d}");
    }

    /// 完全无敞口 (只剩已平仓的历史行): 不给空表, 但要如实说明"有记录、只是没敞口"。
    #[test]
    fn render_flat_only_view_explains_itself_and_is_not_mistaken_for_no_data() {
        let ps = [pos("was_long", "ETHUSDT", "live", Decimal::ZERO, Decimal::ZERO)];
        let view = aggregate(&ps, &[]);
        let out = render(&view, &ExposureFilter::default(), 500, false);
        assert!(out.contains("当前无敞口"), "{out}");
        assert!(out.contains("但头寸全为 0 且无未终结挂单"), "{out}");
        assert!(!out.contains("总名义(估值)"), "无敞口时不必打印表头: {out}");
        assert!(out.contains("这不表示交易所侧一定没有仓位"), "不得把本地视图说成权威: {out}");
    }

    /// 有敞口的组合进表, 已无敞口的只计数不占行 —— 且漏掉多少要明说。
    #[test]
    fn render_hides_flat_combos_but_reports_the_count() {
        let ps = [
            pos("live_one", "ETHUSDT", "live", dec!(1), dec!(100)),
            pos("done", "DOGEUSDT", "live", Decimal::ZERO, Decimal::ZERO),
        ];
        let view = aggregate(&ps, &[]);
        let out = render(&view, &ExposureFilter::default(), 500, false);
        assert!(out.contains("ETHUSDT"), "{out}");
        assert!(!out.contains("DOGEUSDT"), "已无敞口的组合不该占表行: {out}");
        assert!(out.contains("另有 1 个组合已无敞口"), "{out}");
    }

    /// 模式短标签: 未知模式不猜 (原样透出), 空串说"未知"。
    #[test]
    fn short_mode_labels_are_explicit() {
        assert_eq!(short_mode("live"), "实盘");
        assert_eq!(short_mode("demo"), "测试网");
        assert_eq!(short_mode("dry_run"), "DryRun");
        assert_eq!(short_mode(""), "未知");
        assert_eq!(short_mode("paper"), "paper");
    }

    /// 单元格按**显示宽度**补位: CJK 表头(2 列)与 ASCII 数据(1 列)拼出的行必须等宽,
    /// 否则表头之后的每一列都会右移 (中文终端里"表头和数据对不上"的根因)。
    #[test]
    fn row_pads_by_display_width() {
        let header = row(&[("标的", 12, Align::Left), ("净头寸", 16, Align::Right)]);
        let data = row(&[("ETHUSDT", 12, Align::Left), ("1", 16, Align::Right)]);
        let w = crate::commands::display_width;
        assert_eq!(w(&header), w(&data), "行宽必须一致: {header:?} vs {data:?}");
        assert_eq!(w(&header), 12 + 1 + 16);
        // 右对齐: 数字贴着列的右边界
        assert!(data.ends_with('1'), "{data:?}");
        // 超出列宽时不得截断 (宁可挤歪也不丢数据)
        let long = row(&[("averyveryverylongstrategy", 12, Align::Left)]);
        assert!(long.starts_with("averyveryverylongstrategy"), "{long:?}");
    }

    /// 端到端: 同一张表里两行数据宽度必须一致 (列宽常量与 `row` 配合正确)。
    #[test]
    fn rendered_rows_have_equal_width() {
        let ps = [
            pos("a", "ETHUSDT", "live", dec!(1), dec!(2000)),
            pos("b", "BTCUSDT", "live", dec!(0.5), dec!(100)),
        ];
        let view = aggregate(&ps, &[]);
        let out = render(&view, &ExposureFilter::default(), 500, false);
        let w = crate::commands::display_width;
        let data: Vec<usize> =
            out.lines().filter(|l| l.contains("USDT") && l.contains("实盘")).map(w).collect();
        assert_eq!(data.len(), 2, "{out}");
        assert_eq!(data[0], data[1], "同表两行宽度须一致: {out}");
    }
}
