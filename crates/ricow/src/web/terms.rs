//! 术语表 (025 FR-024 / FR-026 / D13): **静态内置**数据, 中英各一份 —— 不联网、不由 AI 生成,
//! 保证离线可用与解释一致。
//!
//! 词条来源是[回测报告]的指标文案: key 与 `crate::commands::format_backtest_report`
//! 打印的指标名**逐字一致**(SC-009)。报告改了名字而术语表没跟上, 单测会直接失败 ——
//! 这也是"口径同源"的落点: 解释里的数字含义与报告里的数字含义只有一份定义。
//!
//! [回测报告]: crate::commands::format_backtest_report

/// 一条术语解释: `key` = 报告/对话里出现的那串名字, `zh` / `en` = 通俗说明(一句话 + 必要口径)。
#[derive(serde::Serialize)]
pub struct Term {
    pub key: &'static str,
    pub zh: &'static str,
    pub en: &'static str,
}

/// 回测报告指标名(顺序即报告里的出现顺序), 与 `format_backtest_report` 一一对应。
pub const REPORT_TERMS: &[Term] = &[
    Term {
        key: "K 线数",
        zh: "本次回测一共走过多少根 K 线(每根代表一个时间周期, 如 1 小时)。",
        en: "How many candles the backtest walked through; one candle is one time interval, e.g. 1 hour.",
    },
    Term {
        key: "成交笔数",
        zh: "回测期间真正成交的订单笔数; 被拒的订单不计入。",
        en: "Number of orders that actually filled; rejected orders are not counted.",
    },
    Term {
        key: "已实现盈亏",
        zh: "已平仓部分结算出的盈亏, 不含手续费, 也不算未平仓的浮动盈亏。",
        en: "P&L settled on closed positions; excludes fees and unrealized P&L on open positions.",
    },
    Term {
        key: "手续费",
        zh: "累计支付的交易手续费 = 每笔成交额 × 费率。",
        en: "Total trading fees paid = each fill notional × the fee rate.",
    },
    Term {
        key: "手续费占比",
        zh: "手续费 ÷ 成交额; 越高说明交易越频繁, 成本吃掉收益越多。",
        en: "Fees ÷ turnover; the higher it is, the more often the strategy trades and the more costs eat into returns.",
    },
    Term {
        key: "最大回撤",
        zh: "净值从最高点跌到最低点的最大跌幅, 衡量最难熬的一段亏损。",
        en: "Largest peak-to-trough drop of equity; how painful the worst losing stretch was.",
    },
    Term {
        key: "净盈亏",
        zh: "已实现盈亏扣掉手续费后的净额(计价币, 通常是 USDT)。",
        en: "Realized P&L minus fees, in quote currency (usually USDT).",
    },
    Term {
        key: "胜率",
        zh: "盈利的平仓笔数 ÷ 总平仓笔数; 高胜率不等于赚钱, 还要看盈亏比。",
        en: "Profitable closed trades ÷ all closed trades; a high win rate alone does not mean profit — check the profit factor too.",
    },
    Term {
        key: "拒单次数",
        zh: "因资金不足或无仓可平被拒的订单数; 被拒不等于亏损。",
        en: "Orders rejected for insufficient funds or nothing to close; a rejection is not a loss.",
    },
    Term {
        key: "资金费净额",
        zh: "永续合约资金费收付的净额: 正值 = 净收(通常空头), 负值 = 净付(通常多头)。",
        en: "Net funding payments on perpetuals: positive = net received (usually shorts), negative = net paid (usually longs).",
    },
    Term {
        key: "强平次数",
        zh: "保证金不足被交易所强制平仓的次数。",
        en: "Times a position was force-closed by the exchange because margin ran out.",
    },
    Term {
        key: "按侧明细",
        zh: "对冲模式下 LONG / SHORT 两侧各自的强平次数与逐仓钱包余额, 便于与交易所账单对照。",
        en: "In hedge mode, liquidations and isolated-wallet balance per side (LONG / SHORT), for reconciling against exchange statements.",
    },
    Term {
        key: "年化收益率",
        zh: "把整段回测的收益折算成一年口径(复利几何平均), 便于和别的策略横向比较。",
        en: "The backtest return scaled to a one-year basis (compounded, geometric), so strategies are comparable.",
    },
    Term {
        key: "年化波动率",
        zh: "收益率的年度化波动幅度, 越大说明净值起伏越剧烈。",
        en: "Annualized volatility of returns; bigger means the equity curve swings harder.",
    },
    Term {
        key: "夏普比率",
        zh: "每承担一单位波动换来多少超额收益(已扣无风险利率); 一般越高越好。",
        en: "Excess return per unit of volatility, above the risk-free rate; generally the higher the better.",
    },
    Term {
        key: "索提诺比率",
        zh: "与夏普类似, 但只把下跌波动当风险, 更贴近怕亏的实际感受。",
        en: "Like Sharpe, but only downside volatility counts as risk — closer to how losses actually feel.",
    },
    Term {
        key: "Calmar 比率",
        zh: "年化收益 ÷ 最大回撤; 衡量要赚这份收益, 得忍受多大的回撤。",
        en: "Annualized return ÷ max drawdown; how much drawdown you must endure for that return.",
    },
    Term {
        key: "盈亏比",
        zh: "总盈利 ÷ 总亏损; 大于 1 表示赚的比亏的多, 与胜率是两回事。",
        en: "Gross profit ÷ gross loss; above 1 means wins outweigh losses, which is not the same as win rate.",
    },
    Term {
        key: "平均盈利/亏损",
        zh: "每笔盈利交易的平均盈利 / 每笔亏损交易的平均亏损(计价币)。",
        en: "Average profit per winning trade / average loss per losing trade, in quote currency.",
    },
    Term {
        key: "标的涨跌",
        zh: "标的自身从首根开盘价到期末价的涨跌幅; 用来对照策略有没有跑赢单纯持有。",
        en: "The instrument's own move from the first open to the final price; compare with it to see whether the strategy beat buy and hold.",
    },
    Term {
        key: "名义敞口",
        zh: "期末持仓折算成计价币的价值(合约含杠杆放大); 0 表示期末没有持仓。",
        en: "End-of-backtest position value in quote currency (leverage included for futures); 0 means flat at the end.",
    },
    Term {
        key: "持仓币数",
        zh: "现货回测结束时的持币数量, 以及相对建仓后的变化幅度。",
        en: "Coin holdings at the end of a spot backtest, and how much they changed since entry.",
    },
    Term {
        key: "现金 USDT",
        zh: "账户现金余额从建仓后到期末的变化, 与持仓分开看。",
        en: "Change in cash balance from right after entry to the end, shown apart from holdings.",
    },
    Term {
        key: "总权益",
        zh: "合约账户总资产 = 现金 + 逐仓钱包 + 未实现盈亏, 即账户真正值多少钱。",
        en: "Futures account equity = cash + isolated wallets + unrealized P&L; what the account is really worth.",
    },
    Term {
        key: "总价值",
        zh: "现货账户总资产 = 持币市值 + 现金, 含未实现盈亏。",
        en: "Spot account value = holdings at market + cash, including unrealized P&L.",
    },
    Term {
        key: "基准对照",
        zh: "把策略与「同本金满仓买入持有」并列对照, 从首次成交价同时点同本金起算(建仓前空仓不计)。",
        en: "Places the strategy next to buy-and-hold at the same capital, measured from the first fill price, same moment and same principal (no position before entry).",
    },
];

/// 报告里没有打印、但对话与配置里常出现的术语 (FR-024 点名的那几个)。
///
/// 与 [`REPORT_TERMS`] 分开摆是为了保住"报告指标名 ↔ 词条"的双向核对(SC-009) ——
/// 混在一起就无法区分"表多出来的词条"与"报告漏配的词条"。
pub const GENERAL_TERMS: &[Term] = &[
    Term {
        key: "资金费率",
        zh: "永续合约按固定周期(通常 8 小时)在多空之间收付的费用比率, 用来把合约价格拉回现货价。",
        en: "The rate perpetual futures charge between longs and shorts on a fixed cycle (usually every 8 hours) to keep the contract price near spot.",
    },
    Term {
        key: "滑点",
        zh: "下单价格与最终成交价的偏差; 市价单越大、行情越快, 滑点越高。",
        en: "The gap between the price you asked for and the price you got; bigger market orders and faster markets mean more slippage.",
    },
    Term {
        key: "杠杆",
        zh: "用保证金放大持仓的倍数; 放大收益也放大亏损, 倍数越高越容易触发强平。",
        en: "How much a margin deposit is multiplied into position size; it magnifies gains and losses alike, and higher leverage liquidates sooner.",
    },
];

/// 全部词条 (报告指标在前, 通用术语在后): 前端一次取全, 切换语言无需重取。
pub fn all() -> Vec<&'static Term> {
    REPORT_TERMS.iter().chain(GENERAL_TERMS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::format_backtest_report;
    use ricow_strategy::{BacktestReport, HedgeSides};
    use rust_decimal::Decimal;

    /// 造一份样本报告: 字段全给非零值, 保证每一行都真的被打印出来。
    ///
    /// 期货与现货走的是不同分支(期货打 `名义敞口` / `总权益` / 资金费与强平, 现货打
    /// `持仓币数` / `总价值`) —— 两边都造, 才能覆盖报告的全部指标名。
    fn sample(is_futures: bool) -> BacktestReport {
        let mut report = BacktestReport {
            total_bars: 24,
            total_trades: 3,
            realized_pnl: Decimal::from(12),
            total_fees: Decimal::from(1),
            fee_ratio: Decimal::new(5, 4),
            max_drawdown: Decimal::new(7, 2),
            net_pnl: Decimal::from(11),
            win_rate: 0.66,
            rejected_count: 1,
            risk_free_rate: 3.8,
            price_change_pct: 2.5,
            cash_change_pct: 1.0,
            equity_change_pct: 1.2,
            final_price: Decimal::from(100),
            base_cash: Decimal::from(1000),
            final_cash: Decimal::from(1010),
            final_equity: Decimal::from(1050),
            base_coin_size: Decimal::from(1),
            final_pos_size: Decimal::from(1),
            annual_return: Some(0.5),
            annual_volatility: Some(0.2),
            sharpe: Some(1.5),
            sortino: Some(2.0),
            calmar: Some(3.0),
            profit_factor: Some(1.8),
            avg_win: Some(10.0),
            avg_loss: Some(5.0),
            ..Default::default()
        };
        if is_futures {
            report.funding_net = Some(Decimal::from(3));
            report.liquidation_count = Some(1);
            report.hedge_sides = Some(HedgeSides::default());
            report.final_notional = Some(Decimal::from(500));
            report.nominal_exposure_pct = Some(-4.0);
        } else {
            report.coin_change_pct = Some(1.5);
        }
        report
    }

    fn printed_report(is_futures: bool) -> String {
        format_backtest_report(&sample(is_futures), "回测报告", Decimal::from(10_000), is_futures)
    }

    /// 抠出报告正文里的指标名: 只认 `指标名: 值` 这种正文行 —— 表头无冒号、段标题以 `---` 开头,
    /// 两类都自然落选; 指标名后面的括注(`年化收益率 (几何)` / `按侧明细 (hedge)`) 不算名字本身。
    fn labels(report: &str) -> Vec<String> {
        report
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.starts_with("---") {
                    return None;
                }
                let (name, _) = line.split_once(':')?;
                Some(name.split('(').next().unwrap_or(name).trim().to_string())
            })
            .collect()
    }

    /// SC-009: 报告里出现的每一个指标名, 术语表都得有对应条目(无遗漏)。
    #[test]
    fn test_every_report_metric_has_a_term() {
        for is_futures in [true, false] {
            let report = printed_report(is_futures);
            for name in labels(&report) {
                assert!(
                    REPORT_TERMS.iter().any(|t| t.key == name),
                    "报告指标「{name}」缺术语条目:\n{report}"
                );
            }
        }
    }

    /// R7 反向: 术语表里的报告词条必须真的在报告里打印(防止报告改了指标名而表没跟上)。
    #[test]
    fn test_every_report_term_is_actually_printed() {
        let printed: Vec<String> =
            [true, false].iter().flat_map(|f| labels(&printed_report(*f))).collect();
        for term in REPORT_TERMS {
            assert!(
                printed.iter().any(|name| name == term.key),
                "术语「{}」在回测报告里找不到, 指标名可能已改",
                term.key
            );
        }
    }

    /// SC-009 后半: 每条词条中英各一份(都非空、且不是同一串), 且 key 不重复。
    #[test]
    fn test_every_term_is_bilingual_and_unique() {
        let terms = all();
        assert!(terms.len() >= REPORT_TERMS.len() + GENERAL_TERMS.len());
        for term in &terms {
            assert!(!term.key.trim().is_empty(), "词条 key 不能为空");
            assert!(!term.zh.trim().is_empty(), "「{}」缺中文解释", term.key);
            assert!(!term.en.trim().is_empty(), "「{}」缺英文解释", term.key);
            assert_ne!(term.zh, term.en, "「{}」的中英解释不该是同一串", term.key);
        }
        for (i, a) in terms.iter().enumerate() {
            for b in &terms[i + 1..] {
                assert_ne!(a.key, b.key, "重复词条: {}", a.key);
            }
        }
    }
}
