//! 038 P1-E: `ricow align` —— 实盘 / 回测**对齐工具**(只读)。
//!
//! 为什么需要它: 回测与实盘的可比性靠"同窗口 + 同参数"才能成立, 而此前拿不到这个对照 ——
//! 于是 P1-A 那类**撮合模型误差**(限价"触及即全成")只能靠猜。本工具把两侧放在一张表里,
//! 让偏差自己暴露出来。
//!
//! 三条**诚实性硬约束**(FR-5.3), 违反其一即等于给出误导结论:
//! 1. 口径差异必须**显式印出来**(回测是虚拟撮合; 实盘只覆盖本进程落库的成交);
//! 2. 实盘样本不足(0 笔)时**直说"无从对比"**, 不给看起来完整的空表;
//! 3. 期货下净现金流**不是盈亏**(未计持仓市值与资金费) → 一律叫"现金净流入", 不叫"盈亏"。
//!
//! 本命令**只读**: 不写库、不落盘、不连交易所、不新增确认面。

use std::path::{Path, PathBuf};

use clap::Args;
use ricow_core::CoreResult;
use ricow_strategy::FillWithMode;
use rust_decimal::Decimal;

use crate::commands::backtest::{read_run_cards, RunCard};

#[derive(Args)]
pub struct AlignArgs {
    /// 策略名(与 run card 里记录的名字一致, 即 `ricow backtest --strategy` 用的那个)
    pub strategy: String,
    /// 指定 run card 文件路径; 缺省 = 该策略**最新**一张
    #[arg(long)]
    pub card: Option<String>,
    /// 对比哪一侧的成交: live(默认, 实盘主网) / demo(测试网) / dry_run(模拟)
    #[arg(long)]
    pub mode: Option<String>,
    /// 最多读取多少条成交记录(默认 20000)
    #[arg(long)]
    pub limit: Option<i64>,
}

/// 对照窗口(毫秒; 左闭右开 —— 与回测 `--end` 的文档语义一致)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// 实盘侧事实(从本地库 `fills` 汇总出来的**可直接比的量**)。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LiveFacts {
    pub fills: u64,
    /// Σ|成交价 × 数量|。
    pub notional: Decimal,
    pub fees: Decimal,
    /// Σ卖出金额 − Σ买入金额。**不是盈亏**(见模块文档约束 3)。
    pub net_cash_in: Decimal,
    pub first_ms: Option<i64>,
    pub last_ms: Option<i64>,
}

/// 从成交记录汇总实盘事实(纯函数; 只吃已过滤的行)。
pub(crate) fn summarize_live(rows: &[&FillWithMode]) -> LiveFacts {
    let mut f = LiveFacts::default();
    for r in rows {
        f.fills += 1;
        let notional = r.fill_price * r.fill_size;
        f.notional += notional.abs();
        f.fees += r.fee;
        // side 由 `OrderSide::to_string()` 写入 (`buy` / `sell`)。
        if r.side.eq_ignore_ascii_case("sell") {
            f.net_cash_in += notional;
        } else {
            f.net_cash_in -= notional;
        }
        f.first_ms = Some(f.first_ms.map_or(r.timestamp, |v: i64| v.min(r.timestamp)));
        f.last_ms = Some(f.last_ms.map_or(r.timestamp, |v: i64| v.max(r.timestamp)));
    }
    f
}

/// 是否落在对照窗口内(左闭右开)。
pub(crate) fn in_window(ts_ms: i64, w: Window) -> bool {
    ts_ms >= w.start_ms && ts_ms < w.end_ms
}

/// 实盘侧输入打包 —— 单独成结构而非继续摊平参数, 是因为"扫描了多少条 / 命中多少条"
/// 与"命中行的汇总"必须**同源**, 分开传容易在调用点串错。
pub(crate) struct LiveSide<'a> {
    pub mode: &'a str,
    pub scanned: usize,
    pub matched: usize,
    pub facts: &'a LiveFacts,
}

/// 渲染对照表(纯函数)。`None` 表示该侧算不出这个量 —— 印 `—`, 不编造 0。
pub(crate) fn render_report(
    strategy: &str,
    card_path: &Path,
    card: &RunCard,
    window: Window,
    live: &LiveSide<'_>,
) -> String {
    let mode = live.mode;
    let scanned = live.scanned;
    let matched = live.matched;
    let facts = live.facts;
    let mut out = String::new();
    let is_futures = card.window.market == "futures";
    // 局部宏: 往 `out` 追加一行。刻意**不叫 `line!`** —— 那是 std 内建宏, 同名会造成阅读歧义。
    macro_rules! ln {
        ($($arg:tt)*) => {{ out.push_str(&format!($($arg)*)); out.push('\n'); }};
    }

    ln!("实盘 / 回测对齐: {strategy}");
    ln!(
        "对照窗口: {} → {}  (左闭右开; 由 run card 窗口还原)",
        fmt_ms(window.start_ms),
        fmt_ms(window.end_ms)
    );
    ln!(
        "回测侧: {} | {} 根 | 终点 {} | 数据源 {}",
        card.window.interval,
        card.window.bars,
        card.window.end_ms.map(fmt_ms).unwrap_or_else(|| "到生成时刻".into()),
        card.window.data_source
    );
    ln!("回测卡: {} (生成于 {})", card_path.display(), card.generated_at);
    ln!(
        "实盘侧: mode={mode}, 交易对={}, 库内扫描 {scanned} 条 → 命中窗口 {matched} 条",
        card.window.pair
    );
    out.push('\n');

    if matched == 0 {
        // 约束 2: 样本不足就直说, 不给一张全是 — 的"看起来正常"的表。
        ln!("**样本不足, 无从对比**: 该窗口内没有 mode={mode} 的实盘成交记录。");
        ln!("  可能原因: ① 实盘还没跑到这个窗口; ② --mode 选错了(试试 --mode demo);");
        ln!("           ③ 该策略尚未上过实盘(此时回测与实盘本就无可比)。");
        ln!("  本工具不会用「没有数据」冒充「没有偏差」。");
        return out;
    }

    // 逐项对照: 只有**可直接比**的量才进表(收益率需要持仓与资金费口径, 拿不到干净值)。
    let bt_trades = card.metrics.total_trades;
    let bt_fees: Decimal = card.metrics.total_fees.parse().unwrap_or(Decimal::ZERO);

    ln!("指标            {:<18}{:<18}偏差", "回测", "实盘");
    ln!(
        "成交笔数        {:<18}{:<18}{}",
        bt_trades,
        facts.fills,
        fmt_count_delta(bt_trades, facts.fills)
    );
    ln!(
        "手续费          {:<18}{:<18}{}",
        bt_fees.normalize(),
        facts.fees.normalize(),
        fmt_pct_delta(bt_fees, facts.fees)
    );
    ln!("现金净流入      {:<18}{:<18}—", "—", facts.net_cash_in.normalize());
    ln!("净盈亏(回测)    {:<18}{:<18}—", card.metrics.net_pnl, "不可直接从成交推出");
    out.push('\n');

    // 约束 1 + 3: 口径差异与"现金流 ≠ 盈亏"必须写出来。
    ln!("口径说明 (必读, 否则这张表会被误读):");
    ln!("  · 回测是**虚拟撮合**: 成交价由 K 线 + 滑点/费率假设推出, 不含真实排队、部分成交与撤单;");
    ln!("  · 实盘侧只覆盖**本进程落库**的成交 —— 手工下单、其它实例、以及未落库的成交都不在内;");
    ln!(
        "  · 「现金净流入」= Σ卖出金额 − Σ买入金额, {}",
        if is_futures {
            "**不是盈亏**: 未计持仓市值与资金费, 期货下尤其不可当盈亏读。"
        } else {
            "也不等于盈亏: 未计未平仓的持仓市值。"
        }
    );
    ln!("  · 笔数差得多不等于策略失效: 实盘可能少跑、多跑或中途停过; 先看两侧是否真在同一段行情里。");
    out
}

/// 笔数偏差(实盘 − 回测)。
fn fmt_count_delta(backtest: u64, live: u64) -> String {
    format!("{:+}", live as i64 - backtest as i64)
}

/// 百分比偏差 `(实盘 − 回测) / 回测`; 回测为 0 时不硬算(避免除零给出"无穷大偏差")。
fn fmt_pct_delta(backtest: Decimal, live: Decimal) -> String {
    if backtest.is_zero() {
        return if live.is_zero() { "—".into() } else { "回测为 0, 无法算偏差".into() };
    }
    let pct = (live - backtest) / backtest * Decimal::from(100);
    format!("{:+.1}%", pct)
}

/// Unix 毫秒 → `YYYY-MM-DD HH:MM:SS UTC`。
fn fmt_ms(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => format!("{ms}ms"),
    }
}

/// 由 run card 还原对照窗口: 终点 = 卡的 `end_ms`(缺省 = 卡生成时刻), 起点 = 终点 − 根数 × 步长。
pub(crate) fn window_of(card: &RunCard) -> CoreResult<Window> {
    let end_ms = match card.window.end_ms {
        Some(v) => v,
        None => chrono::DateTime::parse_from_rfc3339(&card.generated_at)
            .map_err(|e| {
                ricow_core::CoreError::InvalidArgument(format!(
                    "run card 的 generated_at 不是合法 RFC3339 时刻({e}), 且未记窗口终点 —— 无法还原对照窗口"
                ))
            })?
            .timestamp_millis(),
    };
    let step_ms = ricow_strategy::tf_ms_of(&card.window.interval).ok_or_else(|| {
        ricow_core::CoreError::InvalidArgument(format!(
            "run card 的 interval `{}` 不在已知周期表内, 无法还原窗口",
            card.window.interval
        ))
    })?;
    Ok(Window { start_ms: end_ms - (card.window.bars as i64) * step_ms, end_ms })
}

/// `ricow align` 入口。
pub(crate) async fn run(args: AlignArgs) -> CoreResult<()> {
    let root = crate::commands::project_root();
    let mode = args.mode.clone().unwrap_or_else(|| "live".to_string());
    if !matches!(mode.as_str(), "live" | "demo" | "dry_run") {
        return Err(ricow_core::CoreError::InvalidArgument(format!(
            "--mode 仅接受 live / demo / dry_run, 实际为 \"{mode}\""
        )));
    }

    // ① 选卡: 显式路径优先; 否则取该策略最新一张(卡按写入时间倒序读回, 首个命中即最新)。
    let (card_path, card) = match &args.card {
        Some(p) => {
            let path = PathBuf::from(p);
            let text = std::fs::read_to_string(&path).map_err(|e| {
                ricow_core::CoreError::InvalidArgument(format!(
                    "读取 run card {} 失败: {e}",
                    path.display()
                ))
            })?;
            let card: RunCard = serde_json::from_str(&text).map_err(|e| {
                ricow_core::CoreError::InvalidArgument(format!(
                    "解析 run card {} 失败: {e}",
                    path.display()
                ))
            })?;
            (path, card)
        }
        None => {
            let dir = root.join("run").join("backtest");
            let found =
                read_run_cards(&root).into_iter().find(|(_, c)| c.strategy.name == args.strategy);
            match found {
                Some(v) => v,
                None => {
                    return Err(ricow_core::CoreError::InvalidArgument(format!(
                        "没有找到策略 `{}` 的 run card。\n\
                         下一步: 先跑一次回测(会落卡): `ricow backtest --strategy {} ...`;\n\
                         或用 --card <路径> 显式指定一张卡。\n\
                         已查看目录: {}",
                        args.strategy,
                        args.strategy,
                        dir.display()
                    )));
                }
            }
        }
    };

    let window = window_of(&card)?;

    // ② 读实盘成交并过滤到对照窗口(策略 + 交易对 + mode + 时间)。
    let db = crate::commands::open_cache_db(&root).await.ok_or_else(|| {
        ricow_core::CoreError::InvalidArgument("打开本地库失败(没有可读的 ricow.db?)".into())
    })?;
    let limit = args.limit.unwrap_or(20_000);
    let rows = db
        .recent_fills_with_mode(Some(&args.strategy), limit)
        .await
        .map_err(|e| ricow_core::CoreError::Exchange(format!("读取成交记录失败: {e}")))?;
    let scanned = rows.len();
    let hits: Vec<&FillWithMode> = rows
        .iter()
        .filter(|r| {
            r.mode.as_deref() == Some(mode.as_str())
                && r.pair == card.window.pair
                && in_window(r.timestamp, window)
        })
        .collect();
    let facts = summarize_live(&hits);

    if scanned as i64 >= limit {
        // 如实提示: 只看过最近 N 条 —— 别让人以为"库里的成交全在这张表里"。
        println!("提示: 成交记录按最近 {limit} 条截断读取, 更早的成交可能未计入;\n      如需完整对照请调大 --limit。\n");
    }
    print!(
        "{}",
        render_report(
            &args.strategy,
            &card_path,
            &card,
            window,
            &LiveSide { mode: &mode, scanned, matched: hits.len(), facts: &facts }
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn fill(ts: i64, side: &str, price: Decimal, size: Decimal, fee: Decimal) -> FillWithMode {
        FillWithMode {
            strategy_id: "s1".into(),
            pair: "ETHUSDT".into(),
            side: side.into(),
            fill_price: price,
            fill_size: size,
            fee,
            timestamp: ts,
            mode: Some("live".into()),
        }
    }

    #[test]
    fn summarize_counts_notional_fees_and_cash_flow() {
        let a = fill(1_000, "buy", dec!(100), dec!(2), dec!(0.1));
        let b = fill(2_000, "sell", dec!(110), dec!(1), dec!(0.2));
        let c = fill(3_000, "sell", dec!(120), dec!(1), dec!(0.3));
        let f = summarize_live(&[&a, &b, &c]);
        assert_eq!(f.fills, 3);
        assert_eq!(f.notional, dec!(200) + dec!(110) + dec!(120));
        assert_eq!(f.fees, dec!(0.6));
        // 卖出 230 − 买入 200 = 30。
        assert_eq!(f.net_cash_in, dec!(30));
        assert_eq!((f.first_ms, f.last_ms), (Some(1_000), Some(3_000)));
    }

    #[test]
    fn summarize_on_empty_is_all_zero_and_none() {
        let f = summarize_live(&[]);
        assert_eq!(f.fills, 0);
        assert_eq!(f.notional, Decimal::ZERO);
        assert_eq!((f.first_ms, f.last_ms), (None, None));
    }

    #[test]
    fn window_is_left_closed_right_open() {
        let w = Window { start_ms: 100, end_ms: 200 };
        assert!(!in_window(99, w));
        assert!(in_window(100, w), "起点闭");
        assert!(in_window(199, w));
        assert!(!in_window(200, w), "终点开 —— 与回测 --end 的文档语义一致");
    }

    fn card(end_ms: Option<i64>, bars: usize, market: &str) -> RunCard {
        RunCard {
            schema_version: 1,
            generated_at: "2026-10-06T00:00:00+00:00".into(),
            engine_version: "0.7.0".into(),
            strategy: crate::commands::backtest::RunCardStrategy {
                name: "s1".into(),
                kind: "lua".into(),
                source_sha256: "ab".into(),
                source_bytes: 1,
            },
            params: serde_json::Map::new(),
            window: crate::commands::backtest::RunCardWindow {
                pair: "ETHUSDT".into(),
                interval: "1h".into(),
                market: market.into(),
                position_mode: "one-way".into(),
                requested_bars: 100,
                warmup_bars: 0,
                bars,
                first_open_time_ms: None,
                last_open_time_ms: None,
                end_ms,
                data_source: "binance-rest".into(),
            },
            metrics: crate::commands::backtest::RunCardMetrics {
                total_trades: 120,
                rejected_count: 0,
                net_pnl: "12.5".into(),
                realized_pnl: "13".into(),
                total_fees: "1.5".into(),
                win_rate: 0.5,
                max_drawdown: "0.01".into(),
                annual_return: None,
                annual_volatility: None,
                sharpe: None,
                sortino: None,
                calmar: None,
                profit_factor: None,
                turnover_ratio: 1.0,
                equity_change_pct: 1.0,
                benchmark_return_pct: None,
            },
        }
    }

    #[test]
    fn window_of_uses_recorded_end_and_bar_count() {
        let c = card(Some(1_000_000_000_000), 24, "spot");
        let w = window_of(&c).expect("应能还原");
        // 24 根 1h bar → 24 * 3_600_000 ms。
        assert_eq!(w.end_ms, 1_000_000_000_000);
        assert_eq!(w.end_ms - w.start_ms, 24 * 3_600_000);
    }

    #[test]
    fn window_of_falls_back_to_generated_at_when_end_missing() {
        let c = card(None, 1, "spot");
        let w = window_of(&c).expect("应能用生成时刻兜底");
        assert_eq!(w.end_ms, 1_791_244_800_000, "2026-10-06T00:00:00Z 的毫秒值");
    }

    #[test]
    fn window_of_rejects_unknown_interval() {
        let mut c = card(Some(1), 1, "spot");
        c.window.interval = "3h".into();
        assert!(window_of(&c).is_err(), "未知周期必须报错, 不得猜一个步长出来");
    }

    #[test]
    fn report_says_no_sample_instead_of_showing_empty_table() {
        let c = card(Some(1_000_000_000_000), 24, "spot");
        let out = render_report(
            "s1",
            Path::new("run/backtest/x.json"),
            &c,
            Window { start_ms: 0, end_ms: 1 },
            &LiveSide { mode: "live", scanned: 50, matched: 0, facts: &LiveFacts::default() },
        );
        assert!(out.contains("样本不足, 无从对比"), "{out}");
        assert!(!out.contains("成交笔数"), "样本不足时不该给一张空表: {out}");
        // 反过来的诚实性也要有: 明说不会拿"没有数据"冒充"没有偏差"。
        assert!(out.contains("冒充"), "{out}");
    }

    #[test]
    fn report_states_caveats_and_never_calls_cash_flow_profit() {
        let c = card(Some(1_000_000_000_000), 24, "futures");
        let facts = LiveFacts {
            fills: 7,
            notional: dec!(1000),
            fees: dec!(1),
            net_cash_in: dec!(-50),
            first_ms: Some(1),
            last_ms: Some(2),
        };
        let out = render_report(
            "s1",
            Path::new("run/backtest/x.json"),
            &c,
            Window { start_ms: 0, end_ms: 1_000_000_000_001 },
            &LiveSide { mode: "live", scanned: 7, matched: 7, facts: &facts },
        );
        assert!(out.contains("成交笔数"), "{out}");
        assert!(out.contains("现金净流入"), "{out}");
        // 期货下必须点明"不是盈亏"。
        assert!(out.contains("不是盈亏"), "期货侧必须写明现金流不是盈亏: {out}");
        assert!(out.contains("虚拟撮合"), "必须说明回测口径: {out}");
        assert!(out.contains("本进程落库"), "必须说明实盘数据覆盖面: {out}");
        assert!(out.contains("-50"), "现金净流入要如实印出来: {out}");
    }

    #[test]
    fn pct_delta_never_divides_by_zero() {
        assert_eq!(fmt_pct_delta(dec!(0), dec!(0)), "—");
        assert!(fmt_pct_delta(dec!(0), dec!(5)).contains("无法算"), "除零不得给出'无穷大偏差'");
        assert_eq!(fmt_pct_delta(dec!(100), dec!(50)), "-50.0%");
    }
}
