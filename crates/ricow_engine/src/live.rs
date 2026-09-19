//! 实盘运行器支撑 (011): 时钟预检判定 / 订单归属判定 / 停机清理编排。
//!
//! 本模块只放**纯逻辑** (可单测), 交易所调用留在 `Engine::run_live` / CLI 侧 (T007/T008):
//! - 时钟预检: 币安对签名请求"超前 >1000ms 硬拒" (specs/testnet.md), 滞后侧有 recvWindow 容差;
//! - 归属判定: 本实例订单 = `clientOrderId` 带 `<策略名>-` 前缀 (口径拍板 4), 只撤自己的单;
//! - 停机清理: 撤单兜底 → 可选平仓 → 残留复查, 顺序与幂等在此固化。

use chrono::{DateTime, NaiveDateTime, Utc};
use ricow_core::{Market, OrderAction, OrderInfo, OrderRequest, OrderSide, OrderType, Position};
use ricow_strategy::{floor_to_step, is_owned, MAX_CLIENT_ORDER_ID_LEN};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

// ---- 时钟预检 (FR-008 / D8) ----

/// 本机超前交易所的最大容忍 (ms): 币安硬拒"超前 > 1000ms"。
pub const MAX_AHEAD_MS: i64 = 1000;
/// 本机滞后交易所的最大容忍 (ms): 滞后侧有 recvWindow(默认 5s) 容差, 留 1s 余量。
pub const MAX_BEHIND_MS: i64 = 4000;

/// 时钟预检结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockVerdict {
    /// 放行 (附实际偏差, 便于如实回显)
    Ok { skew_ms: i64 },
    /// 拒绝启动 (附原因与可执行的对齐步骤)
    Reject { skew_ms: i64, message: String },
}

impl ClockVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, ClockVerdict::Ok { .. })
    }
}

/// 时钟对齐指引 (取自 specs/testnet.md 的实测结论与命令)。
pub fn clock_align_guidance() -> String {
    "时钟对齐步骤 (specs/testnet.md):\n  \
     1) 查偏差: python3 -c \"import time,json,urllib.request as u;print(int(time.time()*1000)-json.load(u.urlopen('https://demo-api.binance.com/api/v3/time'))['serverTime'],'ms')\"\n  \
     2) 修复 (需 root / Windows 侧执行): sudo hwclock -s 或 Windows 执行 wsl --shutdown 后重开\n  \
     3) 重新启动实盘"
        .to_string()
}

/// 本机与交易所服务器时间偏差 (ms; 正 = 本机超前) —— 纯函数, 便于单测。
pub fn skew_ms(local: DateTime<Utc>, server: DateTime<Utc>) -> i64 {
    local.signed_duration_since(server).num_milliseconds()
}

/// 纯判定: `skew_ms` = 本机时间 - 交易所服务器时间 (正 = 本机超前)。
pub fn check_clock_skew(skew_ms: i64) -> ClockVerdict {
    if skew_ms > MAX_AHEAD_MS {
        return ClockVerdict::Reject {
            skew_ms,
            message: format!(
                "本机时钟比交易所快 {skew_ms} ms (允许上限 {MAX_AHEAD_MS} ms); 币安会以 -1021 硬拒签名请求。\n{}",
                clock_align_guidance()
            ),
        };
    }
    if skew_ms < -MAX_BEHIND_MS {
        return ClockVerdict::Reject {
            skew_ms,
            message: format!(
                "本机时钟比交易所慢 {} ms (允许上限 {MAX_BEHIND_MS} ms; recvWindow 容差有限)。\n{}",
                -skew_ms,
                clock_align_guidance()
            ),
        };
    }
    ClockVerdict::Ok { skew_ms }
}

// ---- 实盘启用门禁 (FR-013 / D2) ----

/// 实盘门禁结论: 双条件 (配置声明 + 命令行显式) 缺一即按 Dry Run 运行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveGate {
    /// 双条件满足 → 进实盘
    Live,
    /// 未满足 → 按 Dry Run 运行, 并如实说明原因
    DryRun { reason: String },
}

/// 纯判定: `declared_live` = TOML `live_enabled`, `cli_live` = 命令行 `--live`。
///
/// 依据 product.md "Dry Run 默认, 确认后切实盘": 只写配置字段不够, 必须命令行再确认一次。
pub fn live_gate(declared_live: bool, cli_live: bool) -> LiveGate {
    match (declared_live, cli_live) {
        (true, true) => LiveGate::Live,
        (false, true) => LiveGate::DryRun {
            reason: "策略配置未声明实盘 (TOML live_enabled 缺省/false), 已按 Dry Run 运行".into(),
        },
        (true, false) => LiveGate::DryRun {
            reason: "命令行未显式要求实盘 (缺 --live), 已按 Dry Run 运行".into(),
        },
        (false, false) => LiveGate::DryRun {
            reason:
                "配置未声明实盘且命令行未要求 (缺 live_enabled=true 与 --live), 已按 Dry Run 运行"
                    .into(),
        },
    }
}

/// 风险披露要点 (018): 拒绝实盘启动时打印; 全文见 `README.md` / `README_zh.md` 免责声明。
///
/// 依据 `specs/product.md` §十(安全与合规): "风险披露 = README 免责声明 + **首次使用风险确认**"。
pub const RISK_DISCLOSURE: &str = "\
 · 本软件仅供学习与研究, 不构成投资建议; 加密货币交易风险极高, 可能损失全部本金。\n\
 · 内置策略(含网格)无止损, 标的长期下跌会持续亏损; 选品与仓位由你自行负责。\n\
 · 请先用 Dry Run 在真实行情下观察, 并用交易所**单独子账号**、只投入亏得起的资金。\n\
 · 软件不代管资金、不托管密钥; 所有下单都发生在你自己的交易所账户上。";

/// 实盘首次使用风险确认的判定 (018)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskGate {
    /// 已确认过 → 直接放行
    Proceed,
    /// 本次带了 `--accept-risk` → 记录确认并放行
    JustAcked,
    /// 未确认且未带开关 → 拒绝 (消息含披露要点与确认方式)
    Refuse { message: String },
}

/// 纯判定: `already_acked` = 数据目录里已有确认记录; `accept_flag` = 命令行 `--accept-risk`。
///
/// 只在**实盘**路径调用(Dry Run / 回测不涉真实资金, 不做打扰); 确认一次即长期有效(记录见
/// `$RICOW_ROOT/risk_ack.json`)。
pub fn risk_gate(already_acked: bool, accept_flag: bool) -> RiskGate {
    if already_acked {
        return RiskGate::Proceed;
    }
    if accept_flag {
        return RiskGate::JustAcked;
    }
    RiskGate::Refuse {
        message: format!(
            "实盘启动前需要一次风险确认 (specs/product.md §十)。\n{RISK_DISCLOSURE}\n\
 确认方式: 读过上述要点与 README 免责声明后, 重跑并加 `--accept-risk` (只需一次, 之后记在数据目录)。\n\
 建议先做无资金预演: `ricow run <name>`(Dry Run, 真实行情/虚拟成交)。"
        ),
    }
}

/// 默认 Dry Run 时长阈值 (小时, 002 FR-007): 24 小时 = 覆盖亚/欧/美三个交易时段的一个完整日周期。
/// 策略节奏不同时可经 `params.min_dry_run_hours` 下调, 设 0 关闭 —— 不做无依据的保守放大。
pub const DEFAULT_MIN_DRY_RUN_HOURS: f64 = 24.0;

/// Dry Run 默认初始虚拟本金 (016): 100,000 quote。
pub const DEFAULT_DRY_RUN_CASH: f64 = 100_000.0;

/// Dry Run 虚拟本金解析 (016): `params.initial_cash` 缺省 [`DEFAULT_DRY_RUN_CASH`]。
///
/// **为什么可配**: 原先硬编码 100k, 使"用小资金预演实盘"不成立 —— 同一份 `[risk]` 限额不可能同时适配
/// 100k 与真实几百 USDT(实测: 小资金限额下 Dry Run 65 单全被拒)。可配后一份配置即可同时预演 Dry Run 与实盘。
///
/// 非法值(≤0 / NaN / inf)直接报错, **不静默回落到默认值** —— 静默回落会让预演口径悄悄变回 100k,
/// 而这正是本变更要消除的坑。
pub fn dry_run_initial_cash(configured: Option<f64>) -> Result<Decimal, String> {
    let v = configured.unwrap_or(DEFAULT_DRY_RUN_CASH);
    if !v.is_finite() || v <= 0.0 {
        return Err(format!(
            "params.initial_cash 非法 (需 > 0 的有限数, 收到 {v}); 该参数是 Dry Run 的虚拟本金"
        ));
    }
    Decimal::from_f64_retain(v)
        .ok_or_else(|| format!("params.initial_cash 无法转换为 Decimal: {v}"))
}

/// Dry Run 时长门禁 (002 FR-007): 声明实盘时, 要求该策略已在 Dry Run 下累计运行足够时长。
///
/// `started_at` = 策略 TOML 的 `dry_run_started_at` (ISO8601 字符串, 见 `StrategyConfig`);
/// `min_hours <= 0` → 关闭 (用户显式选择, 例如高频策略不需要长时间观察);
/// `None` → 从未 Dry Run 过 → 拒绝; 时间戳无法解析 → 拒绝 (不猜, 报出原值让用户修正)。
///
/// 纯函数 (只依赖传入的 `now`); 只在**实盘**启动路径调用 (Dry Run / 回测 / 沙箱不受影响)。
/// 阈值依据: 24 小时覆盖亚/欧/美三个交易时段的一个完整日周期, 足以让策略在真实行情下暴露错误。
pub fn dry_run_gate(
    started_at: Option<&str>,
    now: DateTime<Utc>,
    min_hours: f64,
) -> Result<(), String> {
    if min_hours <= 0.0 {
        return Ok(());
    }
    let Some(raw) = started_at else {
        return Err(
            "该策略从未在 Dry Run 下运行过 (策略 TOML 缺 `dry_run_started_at`); 先 `ricow run <name>` 在真实行情下观察。\n\
             解除方式: 完成 Dry Run 后重启实盘; 或在该策略 TOML 的 `[strategy.params]` 设 `min_dry_run_hours = 0` 关闭本门禁。"
                .to_string(),
        );
    };
    let Some(start) = parse_dry_run_started(raw) else {
        return Err(format!(
            "无法解析 `dry_run_started_at = \"{raw}\"` (需 ISO8601, 如 `2026-07-18T00:00:00Z`); 请修正该字段或设 `min_dry_run_hours = 0`。"
        ));
    };
    let elapsed_hours = (now - start).num_seconds() as f64 / 3600.0;
    if elapsed_hours < min_hours {
        return Err(format!(
            "Dry Run 累计 {elapsed_hours:.1} 小时, 不足阈值 {min_hours:.1} 小时 —— 拒绝进实盘 (product.md: Dry Run 观察后再切实盘)。\n\
             解除方式: 继续 Dry Run 至期满后重启; 或把该策略 TOML 的 `[strategy.params] min_dry_run_hours` 调低/设 0。"
        ));
    }
    Ok(())
}

/// 解析 Dry Run 起点: 优先 RFC3339(带时区), 兼容无时区的 `%Y-%m-%dT%H:%M:%S`(按 UTC 处理)。
fn parse_dry_run_started(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok().map(|n| n.and_utc())
}

// ---- 订单归属判定 (D6 / 口径拍板 4) ----
//
// 归属前缀与匹配规则实现于 `ricow_strategy::align` (下单前注入前缀的地方), 此处直接复用。

// ---- 停机清理编排 (FR-005/FR-007 / D5) ----

/// 停机清理一次性门闩: 重复停机只执行一次清理 (幂等)。
#[derive(Debug, Default)]
pub struct OnceGate {
    done: bool,
}

impl OnceGate {
    /// 首次进入返回 true (应执行清理), 之后恒返回 false。
    pub fn enter(&mut self) -> bool {
        let first = !self.done;
        self.done = true;
        first
    }

    pub fn done(&self) -> bool {
        self.done
    }
}

/// 停机清理计划 (纯函数产出, 不含任何交易所调用)。
#[derive(Debug, Clone, Default)]
pub struct CleanupPlan {
    /// 待撤挂单 (本实例归属)
    pub cancels: Vec<OrderInfo>,
    /// 非本实例归属的挂单: 只上报, **不撤** (可能是用户手工单或其他实例)
    pub foreign: Vec<OrderInfo>,
    /// 平仓单 (现货/one-way 最多一条; 合约 hedge 多空各一条)
    pub closes: Vec<OrderRequest>,
    /// 未平仓/跳过说明 (无持仓、数量不足、方向不明等, 如实输出)
    pub close_notes: Vec<String>,
}

/// 平仓单号: `<前缀><序号>`, 超长时截断前缀尾部 —— 交易所对 `newClientOrderId` 有 **36 字符硬限制**
/// (2026-09-13 实测: 超长直接被拒 `Client order id length should be less than 36 chars`, 平仓失败)。
pub fn close_client_order_id(prefix: &str, index: usize) -> String {
    let suffix = format!("{index}");
    let want = format!("{prefix}{suffix}");
    if want.len() <= MAX_CLIENT_ORDER_ID_LEN {
        return want;
    }
    let keep = MAX_CLIENT_ORDER_ID_LEN.saturating_sub(suffix.len() + 1);
    let head: String = prefix.chars().take(keep).collect();
    format!("{head}-{suffix}")
}

/// 距强平距离 (014 FR-005): 标记价到强平价的相对距离(比率, 0.15 = 15%)。
///
/// - 多头: `(mark − liq) / mark` —— 价格下跌逼近强平
/// - 空头: `(liq − mark) / mark` —— 价格上涨逼近强平
///
/// 返回**负数**表示已穿越(应已被交易所清算, 若仍报出说明数据滞后 —— 如实返回, 不夹到 0);
/// `mark`/`liq` 缺失或非正 → `None`(距离"未知", 由调用方标注, 不猜)。
pub fn liquidation_distance(mark: Decimal, liq: Decimal, side: OrderSide) -> Option<f64> {
    if mark <= Decimal::ZERO || liq <= Decimal::ZERO {
        return None;
    }
    let d = match side {
        OrderSide::Buy => (mark - liq) / mark,
        OrderSide::Sell => (liq - mark) / mark,
    };
    d.to_f64()
}

/// 制定停机清理计划。
///
/// - `close_all`: 是否带平仓开关 (CLI `stop --close-all` 或策略 `on_stop` 后的兜底平仓);
/// - `positions`: **定向**持仓 (现货 = base 余额包装单条; 合约 one-way 一条; hedge 多空各一条);
/// - `hedge`: 合约双向模式 → 平仓带 `positionSide` 且不带 `reduceOnly`(fapi 拒绝两者同带);
/// - 平仓数量按 `step_size` 向下取整, 不足最小数量 → 不平仓并如实说明。
#[allow(clippy::too_many_arguments)] // 参数聚合重构另行立项(021 只清存量告警, 不改结构)
pub fn plan_cleanup(
    orders: &[OrderInfo],
    prefix: &str,
    positions: &[Position],
    pair: &str,
    close_all: bool,
    market: &Market,
    close_client_order_id_prefix: &str,
    hedge: bool,
) -> CleanupPlan {
    let mut plan = CleanupPlan::default();
    for o in orders {
        if is_owned(&o.client_order_id, prefix) {
            plan.cancels.push(o.clone());
        } else {
            plan.foreign.push(o.clone());
        }
    }

    if !close_all {
        return plan;
    }
    if positions.is_empty() {
        plan.close_notes.push("无持仓, 无需平仓".into());
        return plan;
    }

    for (i, pos) in positions.iter().enumerate() {
        let dir = match pos.side {
            OrderSide::Buy => "多",
            OrderSide::Sell => "空",
        };
        if pos.size <= Decimal::ZERO {
            plan.close_notes.push(format!("{dir}向持仓为 0, 跳过"));
            continue;
        }
        let size = floor_to_step(pos.size, market.step_size);
        if size <= Decimal::ZERO || size < market.min_size {
            plan.close_notes.push(format!(
                "{dir}向持仓 {} 对齐到 step_size {:?} 后为 {} , 低于交易所最小数量 {} → 未自动平仓, 请手工处理",
                pos.size, market.step_size, size, market.min_size
            ));
            continue;
        }
        // 平仓方向 = 持仓反向
        let side = match pos.side {
            OrderSide::Buy => OrderSide::Sell,
            OrderSide::Sell => OrderSide::Buy,
        };
        // 合约: one-way 用 reduce_only; hedge 用 positionSide(互斥)
        let position_side = if hedge {
            Some(match pos.side {
                OrderSide::Buy => "long".to_string(),
                OrderSide::Sell => "short".to_string(),
            })
        } else {
            None
        };
        plan.closes.push(OrderRequest {
            client_order_id: close_client_order_id(close_client_order_id_prefix, i),
            pair: pair.to_string(),
            side,
            order_type: OrderType::Market,
            price: None,
            size,
            reduce_only: market.is_perpetual && !hedge,
            position_side,
            action: OrderAction::Place,
        });
    }
    plan
}

/// 停机清理结果 (如实累计, 不掩饰失败)。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupOutcome {
    /// 已撤挂单号
    pub canceled: Vec<String>,
    /// 撤单失败 (单号, 原因) —— 单个失败不中断其余
    pub cancel_failed: Vec<(String, String)>,
    /// 成功下发的平仓单号 (现货/one-way 最多一笔; hedge 多空各一笔)
    pub close_done: Vec<String>,
    /// 平仓失败 (单号, 原因)
    pub close_error: Vec<(String, String)>,
    /// 未平仓说明 (无持仓/数量不足/方向跳过等)
    pub close_notes: Vec<String>,
    /// 复查后仍存在的本实例挂单 (残留, 需用户处理)
    pub residual: Vec<String>,
    /// 复查后仍存在的持仓数量
    pub residual_position: Option<Decimal>,
}

impl CleanupOutcome {
    /// 记录一次撤单结果: 成功/失败都累计, **失败不抛出** (调用方据此继续处理其余挂单)。
    pub fn record_cancel(&mut self, client_order_id: &str, result: Result<(), String>) {
        match result {
            Ok(()) => self.canceled.push(client_order_id.to_string()),
            Err(e) => self.cancel_failed.push((client_order_id.to_string(), e)),
        }
    }

    /// 是否有残留 (挂单或持仓) —— true 时停机输出须明确提示用户手工处理。
    pub fn has_residual(&self) -> bool {
        !self.residual.is_empty()
            || self.residual_position.map(|d| d > Decimal::ZERO).unwrap_or(false)
    }
}

/// 复查残留: 清理后仍属于本实例的挂单。
pub fn residual_owned(after: &[OrderInfo], prefix: &str) -> Vec<String> {
    after
        .iter()
        .filter(|o| is_owned(&o.client_order_id, prefix))
        .map(|o| o.client_order_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_risk_gate_first_use_requires_ack() {
        // 未确认且未带开关 → 拒绝, 消息含披露与确认方式
        match risk_gate(false, false) {
            RiskGate::Refuse { message } => {
                assert!(message.contains("风险"), "应含披露要点: {message}");
                assert!(message.contains("--accept-risk"), "应给出确认方式: {message}");
                assert!(message.contains("README"), "应指向免责声明: {message}");
            }
            other => panic!("应拒绝, 实际 {other:?}"),
        }
    }

    #[test]
    fn test_risk_gate_accept_flag_and_prior_ack() {
        // 本次带开关 → 记录并放行
        assert_eq!(risk_gate(false, true), RiskGate::JustAcked);
        // 已确认过 → 直接放行(不再要求带开关)
        assert_eq!(risk_gate(true, false), RiskGate::Proceed);
        assert_eq!(risk_gate(true, true), RiskGate::Proceed);
    }

    #[test]
    fn test_dry_run_initial_cash_default_and_custom() {
        // 缺省 = 100k (保持既有行为不变)
        assert_eq!(dry_run_initial_cash(None).unwrap(), dec!(100000));
        // 可配: 小资金预演用真实口径
        assert_eq!(dry_run_initial_cash(Some(300.0)).unwrap(), dec!(300));
        assert_eq!(dry_run_initial_cash(Some(0.5)).unwrap(), dec!(0.5));
    }

    #[test]
    fn test_dry_run_initial_cash_rejects_invalid() {
        // 非法值必须报错而不是静默回落 100k (静默回落会让预演口径悄悄变回 100k)
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let e = dry_run_initial_cash(Some(bad)).unwrap_err();
            assert!(e.contains("initial_cash"), "报错应指出参数名: {e}");
        }
    }
    use ricow_core::OrderStatus;
    use ricow_strategy::ownership_prefix;

    fn market(min_size: Decimal, step: Option<Decimal>) -> Market {
        Market {
            symbol: "ETHUSDT".into(),
            base_asset: "ETH".into(),
            quote_asset: "USDT".into(),
            is_perpetual: false,
            min_size,
            tick_size: dec!(0.01),
            step_size: step,
            min_notional: None,
            max_leverage: None,
            margin_mode: None,
            is_delisted: false,
        }
    }

    fn perp_market(min_size: Decimal, step: Option<Decimal>) -> Market {
        Market { is_perpetual: true, ..market(min_size, step) }
    }

    fn order(cid: &str) -> OrderInfo {
        OrderInfo {
            exchange_order_id: format!("ex-{cid}"),
            client_order_id: cid.into(),
            pair: "ETHUSDT".into(),
            side: OrderSide::Buy,
            price: dec!(3000),
            size: dec!(1),
            filled_size: Decimal::ZERO,
            status: OrderStatus::Open,
        }
    }

    // ---- 时钟预检: 超前拒 / 滞后过大拒 / 正常放行 ----

    #[test]
    fn test_clock_skew_rejects_ahead() {
        let v = check_clock_skew(1500);
        assert!(!v.is_ok());
        match v {
            ClockVerdict::Reject { skew_ms, message } => {
                assert_eq!(skew_ms, 1500);
                assert!(message.contains("1500"), "应回显实际偏差: {message}");
                assert!(
                    message.contains("hwclock") && message.contains("wsl --shutdown"),
                    "应含对齐步骤: {message}"
                );
            }
            _ => panic!("应拒绝"),
        }
        // 边界: 恰好 1000ms 不拒 (严格大于才拒)
        assert!(check_clock_skew(1000).is_ok());
        assert!(!check_clock_skew(1001).is_ok());
    }

    #[test]
    fn test_clock_skew_rejects_too_far_behind() {
        assert!(check_clock_skew(-4000).is_ok(), "边界内放行");
        let v = check_clock_skew(-4001);
        match v {
            ClockVerdict::Reject { skew_ms, message } => {
                assert_eq!(skew_ms, -4001);
                assert!(message.contains("4001"), "超限值按绝对值回显: {message}");
            }
            _ => panic!("滞后过大应拒绝"),
        }
    }

    #[test]
    fn test_skew_ms_sign() {
        let t0 = DateTime::from_timestamp_millis(1_760_000_000_000).unwrap();
        assert_eq!(skew_ms(t0 + chrono::Duration::milliseconds(250), t0), 250);
        assert_eq!(skew_ms(t0 - chrono::Duration::milliseconds(1200), t0), -1200);
        assert_eq!(skew_ms(t0, t0), 0);
        // 与预检联动: 本机超前 1500ms → 拒绝启动
        let ahead = skew_ms(t0 + chrono::Duration::milliseconds(1500), t0);
        assert_eq!(ahead, 1500);
        assert!(!check_clock_skew(ahead).is_ok(), "本机超前 1500ms 应被预检拒绝");
    }

    #[test]
    fn test_live_gate_requires_both_conditions() {
        assert_eq!(live_gate(true, true), LiveGate::Live, "双条件 → 实盘");
        for (declared, cli) in [(false, true), (true, false), (false, false)] {
            match live_gate(declared, cli) {
                LiveGate::DryRun { reason } => {
                    assert!(reason.contains("Dry Run"), "原因须说明按 Dry Run 运行: {reason}");
                    assert!(
                        reason.contains("--live") || reason.contains("live_enabled"),
                        "原因须指明缺失条件: {reason}"
                    );
                }
                LiveGate::Live => panic!("({declared},{cli}) 不应进实盘"),
            }
        }
    }

    #[test]
    fn test_clock_skew_ok_echoes_actual_skew() {
        assert_eq!(check_clock_skew(0), ClockVerdict::Ok { skew_ms: 0 });
        assert_eq!(check_clock_skew(250), ClockVerdict::Ok { skew_ms: 250 });
        assert_eq!(check_clock_skew(-2000), ClockVerdict::Ok { skew_ms: -2000 });
    }

    // ---- 归属匹配 ----

    #[test]
    fn test_is_owned_only_matches_prefix() {
        let prefix = ownership_prefix("grid-eth");
        assert_eq!(prefix, "grid-eth-");
        assert!(is_owned("grid-eth-0001", &prefix), "本实例单应匹配");
        assert!(!is_owned("grid-eth", &prefix), "缺分隔符不匹配 (前缀含尾部分隔符)");
        assert!(!is_owned("grid-ethx-0001", &prefix), "同前缀但不同策略名不匹配");
        assert!(!is_owned("manual-0001", &prefix), "他人/手工单不匹配");
    }

    #[test]
    fn test_ownership_prefix_normalizes_and_empty_prefix_never_matches() {
        // 非 [A-Za-z0-9_-] 字符替换为 '-'
        assert_eq!(ownership_prefix("网格 ETH/USDT"), "---ETH-USDT-");
        assert_eq!(ownership_prefix(""), "ricow-", "空策略名兜底");
        // 空前缀一律不匹配 (宁漏撤不误撤)
        assert!(!is_owned("anything", ""));
        // 大小写敏感 (交易所 clientOrderId 区分大小写)
        assert!(!is_owned("GRID-ETH-1", &ownership_prefix("grid-eth")));
    }

    // ---- 清理顺序 / 幂等 / 残留 ----

    #[test]
    fn test_plan_cleanup_splits_owned_and_foreign() {
        let prefix = ownership_prefix("grid");
        let orders = vec![order("grid-1"), order("manual-2"), order("grid-3")];
        let plan = plan_cleanup(
            &orders,
            &prefix,
            &[],
            "ETHUSDT",
            false,
            &market(dec!(0.001), Some(dec!(0.001))),
            "grid-close",
            false,
        );
        assert_eq!(plan.cancels.len(), 2);
        assert_eq!(plan.foreign.len(), 1, "非本实例单只上报不撤");
        assert_eq!(plan.foreign[0].client_order_id, "manual-2");
        assert!(plan.closes.is_empty(), "未开平仓开关 → 无平仓单");
    }

    #[test]
    fn test_plan_cleanup_close_all_spot() {
        let prefix = ownership_prefix("grid");
        let m = market(dec!(0.001), Some(dec!(0.001)));
        let pos = Position {
            pair: "ETHUSDT".into(),
            side: OrderSide::Buy,
            size: dec!(1.5),
            entry_price: dec!(3000),
            mark_price: dec!(3000),
            liquidation_price: None,
            unrealized_pnl: Decimal::ZERO,
            leverage: None,
        };
        let plan = plan_cleanup(
            &[],
            &prefix,
            std::slice::from_ref(&pos),
            "ETHUSDT",
            true,
            &m,
            "grid-close",
            false,
        );
        assert_eq!(plan.closes.len(), 1, "现货单条持仓 → 一笔平仓单");
        let close = &plan.closes[0];
        assert_eq!(close.side, OrderSide::Sell, "现货平仓 = 卖出");
        assert_eq!(close.order_type, OrderType::Market);
        assert_eq!(close.size, dec!(1.5));
        assert_eq!(close.client_order_id, "grid-close0");
        assert!(!close.reduce_only, "现货无 reduceOnly 概念");
        assert!(close.position_side.is_none(), "现货无方向仓");

        // 数量对齐后低于最小数量 → 不平仓 + 如实说明
        let tiny = Position { size: dec!(0.0004), ..pos.clone() };
        let plan2 = plan_cleanup(&[], &prefix, &[tiny], "ETHUSDT", true, &m, "c", false);
        assert!(plan2.closes.is_empty());
        assert!(
            plan2.close_notes.iter().any(|n| n.contains("未自动平仓")),
            "{:?}",
            plan2.close_notes
        );

        // 无持仓
        let plan3 = plan_cleanup(&[], &prefix, &[], "ETHUSDT", true, &m, "c", false);
        assert!(plan3.closes.is_empty());
        assert!(plan3.close_notes.iter().any(|n| n.contains("无持仓")));
    }

    #[test]
    fn test_plan_cleanup_close_all_futures_one_way_and_hedge() {
        let prefix = ownership_prefix("grid");
        let m = perp_market(dec!(0.001), Some(dec!(0.001)));
        let long = Position {
            pair: "ETHUSDT".into(),
            side: OrderSide::Buy,
            size: dec!(0.05),
            entry_price: dec!(2500),
            mark_price: dec!(2500),
            liquidation_price: None,
            unrealized_pnl: Decimal::ZERO,
            leverage: None,
        };
        let short = Position { side: OrderSide::Sell, size: dec!(0.02), ..long.clone() };

        // one-way: 单条净仓 → 一笔 reverse 单 + reduceOnly (不开反向仓)
        let one = plan_cleanup(
            &[],
            &prefix,
            std::slice::from_ref(&long),
            "ETHUSDT",
            true,
            &m,
            "c",
            false,
        );
        assert_eq!(one.closes.len(), 1);
        assert_eq!(one.closes[0].side, OrderSide::Sell);
        assert!(one.closes[0].reduce_only, "one-way 平仓必须 reduceOnly");
        assert!(one.closes[0].position_side.is_none());

        // hedge: 两侧各一笔, 带 positionSide 且**不带** reduceOnly (fapi 拒绝两者同带)
        let both = plan_cleanup(
            &[],
            &prefix,
            &[long.clone(), short.clone()],
            "ETHUSDT",
            true,
            &m,
            "c",
            true,
        );
        assert_eq!(both.closes.len(), 2, "hedge 多空各平一笔");
        assert_eq!(both.closes[0].position_side.as_deref(), Some("long"));
        assert_eq!(both.closes[0].side, OrderSide::Sell, "平多 = 卖");
        assert!(!both.closes[0].reduce_only, "hedge 不带 reduceOnly");
        assert_eq!(both.closes[1].position_side.as_deref(), Some("short"));
        assert_eq!(both.closes[1].side, OrderSide::Buy, "平空 = 买");
        assert_eq!(both.closes[1].client_order_id, "c1", "每笔独立单号");

        // 单号长度硬约束: 超长前缀必须截断到 ≤ 36 (实测被交易所拒过的坑)
        let long_prefix = "probe-fut-hold-012-close-1789290554024-";
        let cid = close_client_order_id(long_prefix, 0);
        assert!(cid.len() <= MAX_CLIENT_ORDER_ID_LEN, "单号超长: {cid} ({})", cid.len());
        assert!(cid.ends_with("0"), "序号须保留: {cid}");
        assert!(is_owned(&cid, "probe-fut-hold-012-"), "截断后仍须可判归属: {cid}");
        assert_eq!(close_client_order_id("grid-close-", 1), "grid-close-1", "短前缀不截断");
        assert!(!both.closes[1].reduce_only);
    }

    #[test]
    fn test_liquidation_distance_directions_and_missing() {
        // 多头: 标记 100, 强平 85 → 15%
        let d = liquidation_distance(dec!(100), dec!(85), OrderSide::Buy).unwrap();
        assert!((d - 0.15).abs() < 1e-9, "多头距离 {d}");
        // 空头: 标记 100, 强平 115 → 15% (上涨方向)
        let s = liquidation_distance(dec!(100), dec!(115), OrderSide::Sell).unwrap();
        assert!((s - 0.15).abs() < 1e-9, "空头距离 {s}");
        // 已穿越 → 负数 (如实报出, 不夹到 0)
        let through = liquidation_distance(dec!(100), dec!(101), OrderSide::Buy).unwrap();
        assert!(through < 0.0, "穿越应报负数: {through}");
        // 缺失/非正 → None (未知)
        assert!(liquidation_distance(dec!(100), Decimal::ZERO, OrderSide::Buy).is_none());
        assert!(liquidation_distance(Decimal::ZERO, dec!(85), OrderSide::Buy).is_none());
    }

    #[test]
    fn test_once_gate_idempotent() {
        let mut gate = OnceGate::default();
        assert!(gate.enter(), "首次应执行清理");
        assert!(!gate.enter(), "重复停机不再清理");
        assert!(!gate.enter());
        assert!(gate.done());
    }

    #[test]
    fn test_record_cancel_continues_on_failure() {
        let mut o = CleanupOutcome::default();
        o.record_cancel("grid-1", Ok(()));
        o.record_cancel("grid-2", Err("BN 400: Unknown order sent".into()));
        o.record_cancel("grid-3", Ok(()));
        assert_eq!(o.canceled, vec!["grid-1", "grid-3"], "失败不中断其余撤单");
        assert_eq!(o.cancel_failed.len(), 1);
        assert!(o.cancel_failed[0].1.contains("Unknown order"));
    }

    #[test]
    fn test_residual_reported_faithfully() {
        let prefix = ownership_prefix("grid");
        let after = vec![order("grid-2"), order("manual-9")];
        let residual = residual_owned(&after, &prefix);
        assert_eq!(residual, vec!["grid-2"], "只把本实例残留算残留");

        let mut o = CleanupOutcome { residual, ..Default::default() };
        assert!(o.has_residual());
        o.residual_position = Some(dec!(0.5));
        assert!(o.has_residual());
        let clean = CleanupOutcome::default();
        assert!(!clean.has_residual());
    }

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc)
    }

    #[test]
    fn test_dry_run_gate_disabled_when_non_positive() {
        // min_hours <= 0 = 用户显式关闭 → 任何状态都放行 (含从未 Dry Run)
        assert!(dry_run_gate(None, at("2026-09-13T00:00:00Z"), 0.0).is_ok());
        assert!(dry_run_gate(None, at("2026-09-13T00:00:00Z"), -1.0).is_ok());
    }

    #[test]
    fn test_dry_run_gate_rejects_never_ran() {
        let e = dry_run_gate(None, at("2026-09-13T00:00:00Z"), 24.0).unwrap_err();
        assert!(e.contains("从未"), "应说明从未 Dry Run 过: {e}");
        assert!(e.contains("min_dry_run_hours = 0"), "应给出解除方式: {e}");
    }

    #[test]
    fn test_dry_run_gate_rejects_insufficient_and_accepts_boundary() {
        let now = at("2026-09-10T12:00:00Z");
        // 起点在同日 12:30 → 已运行 -0.5 小时(未来时间) → 当作不足拒绝
        let e = dry_run_gate(Some("2026-09-10T12:30:00Z"), now, 24.0).unwrap_err();
        assert!(e.contains("不足"), "{e}");
        // 起点 23.5 小时前 → 拒绝, 并报出已运行时长
        let e = dry_run_gate(Some("2026-09-09T12:30:00Z"), now, 24.0).unwrap_err();
        assert!(e.contains("23.5"), "应报出已运行时长: {e}");
        // 恰好 24 小时 → 放行 (边界取 >=)
        assert!(dry_run_gate(Some("2026-09-09T12:00:00Z"), now, 24.0).is_ok());
        // 超过 → 放行; 无时区后缀亦按 UTC 解析
        assert!(dry_run_gate(Some("2026-09-06T12:00:00"), now, 24.0).is_ok());
    }

    #[test]
    fn test_dry_run_gate_custom_threshold_and_bad_timestamp() {
        let now = at("2026-09-10T12:00:00Z");
        assert!(dry_run_gate(Some("2026-09-10T11:00:00Z"), now, 1.0).is_ok(), "1 小时阈值应放行");
        assert!(dry_run_gate(Some("2026-09-10T11:30:00Z"), now, 1.0).is_err(), "半小时不足 1 小时");
        // 时间戳无法解析 → 拒绝且报出原值 (不猜)
        let e = dry_run_gate(Some("昨天"), now, 24.0).unwrap_err();
        assert!(e.contains("昨天"), "应报出原值: {e}");
    }
}
