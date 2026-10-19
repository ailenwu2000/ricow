//! 下单工程护栏 — 固定 100 单/秒滑动窗口 + 单笔名义上限(2026-09-16, 019-R5 / 2026-10-18, 057-P0-D)。
//!
//! 边界(用户 2026-09-16 决策, 见 specs/changes/019-ai-assistant;
//! 2026-10-18 补充 P0-D, 见 specs/roadmap.md「下一步」第 9 项 ⓑ 方案):
//! - **平台不做投资风控**: 原 RiskEngine 的最大持仓/单日亏损/最小订单/滑点四条静态限额、
//!   `[risk]` TOML 段、`risk_*` 参数全部删除; 盈亏/仓位政策由策略自行用
//!   `ctx:net_pnl()`/`ctx:equity()` 等实现。
//! - 本护栏**不是投资判断**, 是防程序失控(bug 死循环风暴下单 / 数量级笔误)导致账户被交易所
//!   限流/封禁或一次性打穿资金的工程保险丝; **全部固定值、不接受任何配置**, 避免重新长出风控配置面。
//! - **两条规则的边界(别混)**:
//!   ① 频率(100 单/秒): 防 bug 风暴;
//!   ② 单笔名义(权益 × 10, 057-P0-D): **只拦数量级笔误**(把 1000 当 0.1、漏小数点这类)。
//!   它**不拦**"策略本身就要重仓"—— 那是仓位政策, 属投资判断, 由策略自管。
//! - 接入点仍是回测 / Dry Run / 实盘三条 `place_order` 顶部: 超限返回 Rejected ack
//!   (策略循环不中断), 并记 warn 日志(target = "order_guard"), 计入 rejected_count。

use std::collections::VecDeque;
use std::fmt;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use ricow_core::{OrderAck, OrderRequest, OrderStatus};
use rust_decimal::Decimal;

/// 默认下单频率上限 (每秒)。参照 NautilusTrader 默认 100/s; 内置策略单 tick 最大下单数远低于此。
pub const DEFAULT_MAX_ORDERS_PER_SEC: u32 = 100;
/// 频率上限的滑动窗口长度 (毫秒)。
pub const RATE_WINDOW_MS: i64 = 1_000;
/// 单笔名义上限 = 权益 × 本倍数 (057-P0-D, roadmap 第 9 项 ⓑ 方案)。
///
/// **为什么是固定常量、不进配置面**: 一旦可配置就等于重新长出"风险参数面",
/// 宪法 D15/D17 的边界实质失效。固定值 = 拿不到"平台替用户调风控"这把口子的对冲。
///
/// **为什么是 10**: 内置网格/DCA 策略的单笔名义远低于权益; 而一个明显失控的下单
/// (把 1000 当 0.1, 或漏了小数点) 通常是**数量级**错误 —— 10 倍这个"宽但不失守"的量级
/// 既能拦住数量级笔误, 又不会误伤任何正常策略的单笔动作。
/// 它拦不住"策略本身就要重仓"(那是仓位政策, 由策略自管), 也**不该**拦。
pub const NOTIONAL_LIMIT_MULTIPLE: u32 = 10;

/// 单笔名义判定 (057-P0-D, **纯函数**, 便于单测)。
///
/// - `equity` 取不到 → **放行**(见函数文档的诚实性说明);
/// - `price` 缺失(市价单无价格) → **放行**;
///   两者都不是"猜一个值", 而是"拿不到就不拦" —— 护栏拒绝必须是**有依据**的拒绝。
///
/// ## 为什么取不到权益就放行
/// 护栏的作用是拦"明显失控", 不是"必须拦住一切"。在没有权益基准时按某个假定的权益
/// 去拒单, 会把**正常下单**误拒成拒单(比不拦更坏: 用户会以为是 bug)。
/// 宁可漏拦一次数量级笔误, 也不误拒一次正常单 —— 漏拦的后果是用户看到交易所拒单或自己发现,
/// 误拒的后果是策略在合法时凭空停手。
pub fn check_notional(
    notional: Decimal,
    equity: Option<Decimal>,
    multiple: u32,
) -> Result<(), OrderGuardError> {
    let Some(equity) = equity else {
        return Ok(());
    };
    if equity <= Decimal::ZERO {
        return Ok(());
    }
    if notional <= equity * Decimal::from(multiple) {
        return Ok(());
    }
    Err(OrderGuardError::NotionalLimitExceeded { notional, equity, multiple })
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderGuardError {
    /// 下单频率超限: 滑动窗口内请求数超过固定上限。
    OrderRateLimited { limit: u32, count: u32, window_ms: i64 },
    /// 单笔名义超限 (057-P0-D): 本单 `价格 × 数量` 超过 `权益 × 倍数`。
    ///
    /// **只拦这一类明显失控的下单**(如把 1000 当 0.1), 不是仓位政策 ——
    /// 最大持仓 / 单日亏损 / 回撤熔断仍属投资判断, 由策略自管(宪法 D15/D17)。
    NotionalLimitExceeded { notional: Decimal, equity: Decimal, multiple: u32 },
}

impl fmt::Display for OrderGuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderGuardError::OrderRateLimited { limit, count, window_ms } => {
                write!(f, "下单频率超限: {count} 单 / {window_ms}ms > 固定工程护栏 {limit} 单/秒")
            }
            OrderGuardError::NotionalLimitExceeded { notional, equity, multiple } => {
                write!(
                    f,
                    "单笔名义超限: 本单 {notional} > 权益 {equity} × {multiple}(固定工程护栏)\
                     —— 只拦明显失控的下单; 仓位/回撤政策请在策略里用 ctx:equity() 实现"
                )
            }
        }
    }
}

/// 下单频率护栏 (滑动窗口 1 秒)。
///
/// 语义: 窗口内(含本次)请求数 > 上限即拒本次请求(仍计入窗口, 风暴继续被拒)。
/// 窗口按调用方给的时间推进 —— 回测 = 虚拟 tick 时间(可复现),
/// Dry Run / 实盘 = 真实 UTC。计数对全部 pair 共享(防的是策略级风暴), 不区分开仓/平仓。
pub struct OrderGuard {
    max_per_sec: u32,
    window: VecDeque<DateTime<Utc>>,
    /// 累计拒单次数(诊断/报告用)。
    rejections: u64,
    last_reason: Option<String>,
}

impl OrderGuard {
    pub fn new() -> Self {
        Self {
            max_per_sec: DEFAULT_MAX_ORDERS_PER_SEC,
            window: VecDeque::new(),
            rejections: 0,
            last_reason: None,
        }
    }

    /// 测试/护栏单测用: 自定义上限(生产路径固定 100, 无配置入口)。
    #[cfg(test)]
    fn with_limit(max_per_sec: u32) -> Self {
        Self { max_per_sec: max_per_sec.max(1), ..Self::new() }
    }

    /// 检查并推进窗口; `now` = 本次请求时间(None 取系统 UTC)。
    pub fn check(&mut self, now: Option<DateTime<Utc>>) -> Result<(), OrderGuardError> {
        let now = now.unwrap_or_else(Utc::now);
        let cutoff = now - ChronoDuration::milliseconds(RATE_WINDOW_MS);
        while let Some(front) = self.window.front() {
            if *front <= cutoff {
                self.window.pop_front();
            } else {
                break;
            }
        }
        self.window.push_back(now);
        let count = self.window.len() as u32;
        if count > self.max_per_sec {
            let e = OrderGuardError::OrderRateLimited {
                limit: self.max_per_sec,
                count,
                window_ms: RATE_WINDOW_MS,
            };
            self.rejections += 1;
            self.last_reason = Some(e.to_string());
            Err(e)
        } else {
            Ok(())
        }
    }

    /// 当前窗口内已计入的请求数(诊断/测试用)。
    pub fn window_len(&self) -> usize {
        self.window.len()
    }

    /// 累计拒单次数。
    pub fn rejection_count(&self) -> u64 {
        self.rejections
    }

    /// 最近一次拒单原因。
    pub fn last_reason(&self) -> Option<&str> {
        self.last_reason.as_deref()
    }
}

impl Default for OrderGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// 交易对报价资产 (`ETHUSDT` → `USDT`): 只用于取报价余额算权益, **识别不到回 `None`**。
///
/// 与 `commands/run.rs:360` 的同名函数**刻意不同**: 那里是 Dry Run 初始资金设定,
/// 识别不到必须回落到一个默认值(否则策略起不来); 这里算权益, **猜错资产 = 取错余额 =
/// 按错基准拒单**, 所以宁可不拦(`None`)也不能猜。
pub fn quote_asset_of(pair: &str) -> Option<&'static str> {
    let up = pair.to_uppercase();
    // 必须按**后缀**匹配: `BTCUSDT` 与 `BTCFDUSD` 都以 BTC 开头, 前缀匹配会取错资产。
    ["USDT", "USDC", "FDUSD", "TUSD", "BUSD"].into_iter().find(|q| up.ends_with(q))
}

/// 两条 context 路径(Live / Dry Run)共用的护栏判定: 频率 → 名义。
///
/// 顺序刻意: **先频率后名义** —— 频率超限时这一单根本没打算成交, 名义再离谱也无所谓;
/// 反过来先判名义, 一个 bug 风暴会先撞名义上限, 报出"单笔名义超限"这种**误导性**理由。
pub fn guard_check(
    guard: &mut OrderGuard,
    now: Option<DateTime<Utc>>,
    notional: Option<Decimal>,
    equity: Option<Decimal>,
) -> Result<(), OrderGuardError> {
    guard.check(now)?;
    let (Some(notional), Some(equity)) = (notional, equity) else {
        return Ok(());
    };
    check_notional(notional, Some(equity), NOTIONAL_LIMIT_MULTIPLE)
}

/// 构造护栏拒单的 `Rejected` ack —— 三个 `place_order` 顶部共用。
///
/// 语义同交易所拒单: 不抛错 (`place_order` 返回 `Ok`), 策略循环不中断,
/// 策略可按 `ack.status == Rejected` 自行决策。
pub fn rejected_ack(req: &OrderRequest) -> OrderAck {
    OrderAck {
        exchange_order_id: String::new(),
        client_order_id: req.client_order_id.clone(),
        pair: req.pair.clone(),
        side: req.side,
        price: req.price.unwrap_or(Decimal::ZERO),
        size: req.size,
        filled_size: Decimal::ZERO,
        status: OrderStatus::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_past_limit_within_window() {
        let mut g = OrderGuard::with_limit(1);
        assert!(g.check(DateTime::from_timestamp(1_000, 0)).is_ok(), "第 1 单放行");
        let err = g.check(DateTime::from_timestamp(1_000, 0)).unwrap_err();
        assert_eq!(
            err,
            OrderGuardError::OrderRateLimited { limit: 1, count: 2, window_ms: RATE_WINDOW_MS }
        );
        assert!(err.to_string().contains("下单频率超限"));
        assert_eq!(g.rejection_count(), 1);
    }

    #[test]
    fn test_window_expires() {
        let mut g = OrderGuard::with_limit(1);
        assert!(g.check(DateTime::from_timestamp(1_000, 0)).is_ok());
        assert!(g.check(DateTime::from_timestamp(2_100, 0)).is_ok(), "1.1s 后窗口过期应恢复放行");
        assert_eq!(g.window_len(), 1);
    }

    #[test]
    fn test_default_is_generous_and_fixed() {
        let mut g = OrderGuard::new();
        for _ in 0..DEFAULT_MAX_ORDERS_PER_SEC {
            assert!(g.check(DateTime::from_timestamp(1_000, 0)).is_ok());
        }
        assert!(g.check(DateTime::from_timestamp(1_000, 0)).is_err(), "第 101 单应被拒");
        assert_eq!(g.rejection_count(), 1);
        assert!(g.last_reason().unwrap().contains("固定工程护栏"));
    }

    // ———————— 057-P0-D: 单笔名义上限 ————————

    fn eq(v: &str) -> Decimal {
        Decimal::from_str_radix(v, 10).unwrap()
    }

    #[test]
    fn notional_limit_rejects_order_above_multiple() {
        // 权益 1000, 上限 = 1000×10 = 10000
        let err =
            check_notional(eq("10000.01"), Some(eq("1000")), NOTIONAL_LIMIT_MULTIPLE).unwrap_err();
        assert_eq!(
            err,
            OrderGuardError::NotionalLimitExceeded {
                notional: eq("10000.01"),
                equity: eq("1000"),
                multiple: NOTIONAL_LIMIT_MULTIPLE
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("单笔名义超限"), "{msg}");
        // 文案必须自陈边界: 不得让用户以为平台开始做仓位/回撤风控
        assert!(msg.contains("策略"), "{msg}");
    }

    #[test]
    fn notional_limit_boundary_is_inclusive() {
        // 恰好等于上限 → 放行(判据是 `>`, 不是 `>=`: 边界值不该被拒)
        assert!(check_notional(eq("10000"), Some(eq("1000")), NOTIONAL_LIMIT_MULTIPLE).is_ok());
        assert!(check_notional(eq("9999.99"), Some(eq("1000")), NOTIONAL_LIMIT_MULTIPLE).is_ok());
    }

    #[test]
    fn notional_limit_passes_when_equity_unknown() {
        // 拿不到权益 → 放行(见函数文档: 宁可漏拦, 不误拒正常单)
        assert!(check_notional(eq("999999999"), None, NOTIONAL_LIMIT_MULTIPLE).is_ok());
    }

    #[test]
    fn notional_limit_passes_on_non_positive_equity() {
        // 权益为 0 (新账户/数据未就绪) 时按 0 倍算会把一切拒掉 —— 必须放行
        assert!(check_notional(eq("500"), Some(Decimal::ZERO), NOTIONAL_LIMIT_MULTIPLE).is_ok());
    }

    #[test]
    fn notional_limit_multiple_catches_magnitude_errors() {
        // 护栏的**设计意图**: 拦数量级笔误(把 1000 当 0.1 → 10000×), 不拦"策略想重仓"
        let equity = eq("1000");
        assert!(
            check_notional(eq("100000"), Some(equity), NOTIONAL_LIMIT_MULTIPLE).is_err(),
            "漏小数点(1000 → 100000)这类数量级错误必须被拦"
        );
    }

    #[test]
    fn quote_asset_matches_suffix_not_prefix() {
        assert_eq!(quote_asset_of("ETHUSDT"), Some("USDT"));
        assert_eq!(quote_asset_of("BTCFDUSD"), Some("FDUSD"));
        assert_eq!(quote_asset_of("btcusdc"), Some("USDC"), "大小写不敏感");
        // 识别不到必须回 None 而**不是**猜一个(猜错 = 取错余额 = 按错基准拒单)
        assert_eq!(quote_asset_of("ETHBTC"), None);
        assert_eq!(quote_asset_of("ETH"), None);
    }

    #[test]
    fn guard_check_reports_rate_before_notional() {
        // 频率与名义同时超限时, **必须**报频率 —— 否则 bug 风暴会被报成"单笔名义超限",
        // 理由与真实原因不符, 用户会去改参数而不是改 bug。
        let mut g = OrderGuard::with_limit(1);
        let t = DateTime::from_timestamp(1_000, 0);
        // 第 1 单: 名义小(放行), 计入窗口
        assert!(guard_check(&mut g, t, Some(eq("100")), Some(eq("1000"))).is_ok());
        // 第 2 单: 频率(第2>1)与名义(100000>10000)同时超限 → 必须报频率
        let err = guard_check(&mut g, t, Some(eq("100000")), Some(eq("1000"))).unwrap_err();
        assert!(matches!(err, OrderGuardError::OrderRateLimited { .. }), "先报频率, 实际: {err}");
    }

    #[test]
    fn guard_check_passes_when_notional_unknown() {
        // 市价单无价格 → notional 取不到 → 放行(频率判定仍照常)
        let mut g = OrderGuard::with_limit(100);
        assert!(
            guard_check(&mut g, DateTime::from_timestamp(1_000, 0), None, Some(eq("1000"))).is_ok()
        );
    }
}
