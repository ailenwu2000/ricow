//! 下单工程护栏 — 固定 100 单/秒滑动窗口(2026-09-16, 019-R5)。
//!
//! 边界(用户 2026-09-16 决策, 见 specs/changes/019-ai-assistant):
//! - **平台不做投资风控**: 原 RiskEngine 的最大持仓/单日亏损/最小订单/滑点四条静态限额、
//!   `[risk]` TOML 段、`risk_*` 参数全部删除; 盈亏/仓位政策由策略自行用
//!   `ctx:net_pnl()`/`ctx:equity()` 等实现。
//! - 本护栏**不是投资判断**, 是防程序失控(bug 死循环风暴下单)导致账户被交易所限流/封禁
//!   的工程保险丝; 固定值、**不接受任何配置**, 避免重新长出风控配置面。
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

#[derive(Debug, Clone, PartialEq)]
pub enum OrderGuardError {
    /// 下单频率超限: 滑动窗口内请求数超过固定上限。
    OrderRateLimited { limit: u32, count: u32, window_ms: i64 },
}

impl fmt::Display for OrderGuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderGuardError::OrderRateLimited { limit, count, window_ms } => {
                write!(f, "下单频率超限: {count} 单 / {window_ms}ms > 固定工程护栏 {limit} 单/秒")
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
}
