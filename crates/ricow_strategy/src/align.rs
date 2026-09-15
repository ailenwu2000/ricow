//! 实盘下单参数对齐 (011 FR-010 / 口径拍板 3): 把策略意图对齐到交易所过滤器合法值。
//!
//! 规则:
//! - 数量按 `step_size` **向下**取整 (不超策略预算);
//! - 限价单价格按 `tick_size` 取到**不劣于原始意图**的一侧 (买单向下、卖单向上), 偏差至多 1 tick;
//! - 对齐后不足 `min_size` 或 `min_notional` → 拒单并如实报错 (错误信息带交易所数值);
//! - 本已合规时**逐字段不变** (零扰动, 避免无意义日志与签名差异)。
//!
//! 边界: 市价单无价格 → 无法计算名义, `min_notional` 不拦截, 交由交易所判定 (plan P8)。

use ricow_core::{CoreError, CoreResult, Market, OrderRequest, OrderSide, OrderType};
use rust_decimal::Decimal;

/// 对齐结果: 对齐后的请求 + 调整明细 (供日志输出"原值→对齐值")。
#[derive(Debug, Clone)]
pub struct AlignedOrder {
    pub req: OrderRequest,
    /// 调整明细 (空 = 本已合规)
    pub adjustments: Vec<String>,
}

impl AlignedOrder {
    /// 是否发生了任何调整。
    pub fn adjusted(&self) -> bool {
        !self.adjustments.is_empty()
    }
}

/// 按 `step_size` 向下取整; `None` 或 0 步进时原样返回 (无约束)。
pub fn floor_to_step(value: Decimal, step: Option<Decimal>) -> Decimal {
    match step {
        Some(s) if !s.is_zero() => {
            let units = (value / s).trunc();
            let mut v = units * s;
            if v > value {
                // 除法舍入导致略超原值时回退一档: 取整方向必须不超预算。
                v -= s;
            }
            v
        }
        _ => value,
    }
}

/// 价格按 tick 对齐: 买单向下 (买得更便宜), 卖单向上 (卖得更贵) —— 不劣于原始意图。
pub fn align_price_to_tick(price: Decimal, tick: Decimal, side: OrderSide) -> Decimal {
    if tick.is_zero() {
        return price;
    }
    match side {
        OrderSide::Buy => (price / tick).trunc() * tick,
        OrderSide::Sell => {
            let units = (price / tick).trunc();
            let down = units * tick;
            if down < price {
                down + tick
            } else {
                down
            }
        }
    }
}

/// 对齐下单请求: 数量向下取整 + 价格不吃亏侧对齐 + 最小数量/名义校验。
///
/// 失败即为**拒单** (数量/名义不足); 调用方应把错误如实上报 (不静默改小、不重试放大)。
pub fn align_order(mut req: OrderRequest, market: &Market) -> CoreResult<AlignedOrder> {
    let mut adjustments: Vec<String> = Vec::new();

    // ① 数量: 向下取整到 step_size 整数倍
    let size = floor_to_step(req.size, market.step_size);
    if size != req.size {
        adjustments.push(format!(
            "数量 {} → {} (step_size {})",
            req.size,
            size,
            market.step_size.map(|s| s.to_string()).unwrap_or_else(|| "-".into())
        ));
    }
    if size <= Decimal::ZERO {
        return Err(CoreError::InvalidArgument(format!(
            "对齐后数量为 {} (原值 {}, step_size {:?}): 不足以构成有效订单",
            size, req.size, market.step_size
        )));
    }
    if size < market.min_size {
        return Err(CoreError::InvalidArgument(format!(
            "对齐后数量 {} 低于交易所最小数量 {} (原值 {}, step_size {:?})",
            size, market.min_size, req.size, market.step_size
        )));
    }

    // ② 价格: 仅限价单 (市价单无价可对齐)
    let mut price = req.price;
    if req.order_type == OrderType::Limit {
        if let Some(p) = price {
            let aligned = align_price_to_tick(p, market.tick_size, req.side);
            if aligned != p {
                adjustments.push(format!(
                    "价格 {} → {} (tick_size {}, {}方对齐)",
                    p,
                    aligned,
                    market.tick_size,
                    match req.side {
                        OrderSide::Buy => "买",
                        OrderSide::Sell => "卖",
                    }
                ));
            }
            price = Some(aligned);
        }
    }

    // ③ 最小名义: 仅在价格可知 (限价单) 时判定; 市价单交由交易所
    if let (Some(min_notional), Some(p)) = (market.min_notional, price) {
        let notional = p * size;
        if notional < min_notional {
            return Err(CoreError::InvalidArgument(format!(
                "对齐后名义 {notional} 低于交易所最小名义 {min_notional} (价格 {p} × 数量 {size})"
            )));
        }
    }

    req.size = size;
    req.price = price;
    Ok(AlignedOrder { req, adjustments })
}

/// 币安 `clientOrderId` 长度上限 (官方限制 36 字符)。
pub const MAX_CLIENT_ORDER_ID_LEN: usize = 36;

/// 由策略名派生订单号前缀: `<策略名>-`; 非 [A-Za-z0-9_-] 的字符替换为 `-`
/// (币安 clientOrderId 只允许字母数字与 `-`/`_`, 且总长受限)。
pub fn ownership_prefix(strategy_name: &str) -> String {
    let mut s: String = strategy_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if s.is_empty() {
        s = "ricow".into();
    }
    format!("{s}-")
}

/// 判定挂单是否属于本实例: `client_order_id` 以 `prefix` 开头 (大小写敏感, 与交易所一致)。
///
/// 空 prefix 一律不匹配 —— 宁可漏撤并上报, 也不误撤用户手工挂的单。
pub fn is_owned(client_order_id: &str, prefix: &str) -> bool {
    !prefix.is_empty() && client_order_id.starts_with(prefix)
}

/// 给订单号注入归属前缀 (011 D6): 前缀 + id 不超长时拼接; 超长或 id 为空时保留 id 尾部。
fn inject_prefix(cid: &str, prefix: &str) -> String {
    if prefix.is_empty() || cid.starts_with(prefix) {
        return cid.to_string();
    }
    if !cid.is_empty() && prefix.len() + cid.len() <= MAX_CLIENT_ORDER_ID_LEN {
        return format!("{prefix}{cid}");
    }
    let room = MAX_CLIENT_ORDER_ID_LEN.saturating_sub(prefix.len());
    let tail: String = {
        let chars: Vec<char> = cid.chars().collect();
        let start = chars.len().saturating_sub(room);
        chars[start..].iter().collect()
    };
    format!("{prefix}{tail}")
}

/// 实盘下单前处理 (011 D6/D7): ① 订单号注入归属前缀 (停机撤单只撤本实例的单);
/// ② 该 pair 过滤器已在缓存中时按规则对齐 (数量/价格/最小数量/最小名义)。
///
/// `market` = None (过滤器未缓存, 例如 get_markets 失败的降级路径) 时**不干预**参数, 交交易所判定。
/// 返回 (可下发的请求, 调整明细); 对齐后不足最小值 → Err (调用方按拒单如实上报, 不静默改小)。
pub fn prepare_live_order(
    mut req: OrderRequest,
    market: Option<&Market>,
    order_prefix: &str,
) -> CoreResult<(OrderRequest, Vec<String>)> {
    req.client_order_id = inject_prefix(&req.client_order_id, order_prefix);
    match market {
        None => Ok((req, Vec::new())),
        Some(m) => {
            let aligned = align_order(req, m)?;
            Ok((aligned.req, aligned.adjustments))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_core::{OrderRequest, OrderSide};
    use rust_decimal_macros::dec;

    fn market(min_size: Decimal, tick_size: Decimal, step_size: Option<Decimal>) -> Market {
        Market {
            symbol: "ETHUSDT".into(),
            base_asset: "ETH".into(),
            quote_asset: "USDT".into(),
            is_perpetual: false,
            min_size,
            tick_size,
            step_size,
            min_notional: None,
            max_leverage: None,
            margin_mode: None,
            is_delisted: false,
        }
    }

    #[test]
    fn test_size_floor_to_step() {
        let m = market(dec!(0.001), dec!(0.01), Some(dec!(0.001)));
        let req = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000.00), dec!(0.1234));
        let out = align_order(req, &m).expect("应对齐而非拒单");
        assert_eq!(out.req.size, dec!(0.123), "数量须向下取整");
        assert!(out.adjusted());
        assert!(out.adjustments[0].contains("数量"), "调整明细: {:?}", out.adjustments);

        // 除法尾差不得导致超预算
        let m2 = market(dec!(0.1), dec!(1), Some(dec!(0.1)));
        let req2 = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(10), dec!(0.3));
        let out2 = align_order(req2, &m2).unwrap();
        assert_eq!(out2.req.size, dec!(0.3), "0.3 为 step 整数倍, 不应被改小");
        assert!(!out2.adjusted(), "本已合规不应有调整记录");
    }

    #[test]
    fn test_price_aligns_to_non_disadvantageous_side() {
        let m = market(dec!(0.0001), dec!(0.01), Some(dec!(0.0001)));
        // 买单: 向下 (更便宜)
        let buy = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000.567), dec!(1)),
            &m,
        )
        .unwrap();
        assert_eq!(buy.req.price, Some(dec!(3000.56)));
        // 卖单: 向上 (更贵)
        let sell = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Sell, dec!(3000.567), dec!(1)),
            &m,
        )
        .unwrap();
        assert_eq!(sell.req.price, Some(dec!(3000.57)));
        // 恰好落在 tick 上: 两侧都不动
        let on_tick = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Sell, dec!(3000.56), dec!(1)),
            &m,
        )
        .unwrap();
        assert_eq!(on_tick.req.price, Some(dec!(3000.56)));
        assert!(!on_tick.adjusted());
    }

    #[test]
    fn test_reject_below_min_size_and_min_notional() {
        // 数量向下取整后低于 min_size → 拒单, 错误含交易所数值
        let m = market(dec!(0.5), dec!(0.01), Some(dec!(0.1)));
        let err = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(0.45)),
            &m,
        )
        .expect_err("应拒单");
        assert!(err.to_string().contains("最小数量"), "err={err}");

        // 取整后归零 → 拒单 (不足以构成有效订单)
        let err0 = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(0.09)),
            &m,
        )
        .expect_err("取整归零应拒单");
        assert!(err0.to_string().contains("不足"), "err={err0}");

        // 名义不足 min_notional → 拒单, 错误含价格/数量/名义
        let mut m2 = market(dec!(0.001), dec!(0.01), Some(dec!(0.001)));
        m2.min_notional = Some(dec!(50));
        let err2 = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(0.01)),
            &m2,
        )
        .expect_err("名义 30 < 50 应拒单");
        let msg = err2.to_string();
        assert!(msg.contains("最小名义") && msg.contains("50"), "err={msg}");

        // 足够名义 → 通过
        let ok = align_order(
            OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(0.02)),
            &m2,
        )
        .expect("名义 60 ≥ 50 应通过");
        assert_eq!(ok.req.size, dec!(0.02));
    }

    #[test]
    fn test_compliant_order_untouched() {
        let m = market(dec!(0.001), dec!(0.01), Some(dec!(0.001)));
        let req = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000.12), dec!(0.25));
        let before = req.clone();
        let out = align_order(req, &m).unwrap();
        assert!(!out.adjusted(), "本已合规: {:?}", out.adjustments);
        assert_eq!(out.req.size, before.size);
        assert_eq!(out.req.price, before.price);
        // 市价单 (无价): 只有数量被校验, 价格保持 None
        let mkt = OrderRequest::new_market("ETHUSDT", OrderSide::Sell, dec!(0.5));
        let out2 = align_order(mkt, &m).unwrap();
        assert_eq!(out2.req.price, None);
        assert!(!out2.adjusted());
    }

    // ---- 实盘下单前处理 (T006 接线逻辑) ----

    #[test]
    fn test_prepare_live_order_rejects_below_minimum() {
        let m = market(dec!(0.5), dec!(0.01), Some(dec!(0.1)));
        // 数量取整后 < min_size → 拒单 (调用方据此如实上报)
        let req = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(0.45));
        let err = prepare_live_order(req, Some(&m), "grid-").expect_err("不合规模单应被拒");
        assert!(err.to_string().contains("最小数量"), "err={err}");

        // 名义不足 → 拒单
        let mut m2 = market(dec!(0.001), dec!(0.01), Some(dec!(0.001)));
        m2.min_notional = Some(dec!(100));
        let req2 = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(0.01));
        assert!(prepare_live_order(req2, Some(&m2), "grid-").is_err());
    }

    #[test]
    fn test_prepare_live_order_compliant_untouched_with_prefix() {
        let m = market(dec!(0.001), dec!(0.01), Some(dec!(0.001)));
        let req = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000.12), dec!(0.25));
        let (out, adjustments) = prepare_live_order(req, Some(&m), "grid-eth-").unwrap();
        assert!(adjustments.is_empty(), "合规单不应被改动: {adjustments:?}");
        assert_eq!(out.size, dec!(0.25));
        assert_eq!(out.price, Some(dec!(3000.12)));
        assert!(
            out.client_order_id.starts_with("grid-eth-"),
            "须注入归属前缀: {}",
            out.client_order_id
        );

        // 未缓存过滤器 (None) → 不干预参数, 仍注入前缀
        let req2 = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000.123456), dec!(0.9));
        let (out2, adj2) = prepare_live_order(req2, None, "grid-eth-").unwrap();
        assert!(adj2.is_empty(), "无过滤器缓存时不得擅自改动参数");
        assert_eq!(out2.size, dec!(0.9));
        assert_eq!(out2.price, Some(dec!(3000.123456)));
    }

    #[test]
    fn test_prepare_live_order_client_id_length_limit() {
        // UUID(36) + 前缀会超 36 字符上限 → 保留 id 尾部, 总长仍 ≤ 36 且带前缀
        let uuid_like = "12345678-1234-1234-1234-1234567890ab";
        let req = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(1));
        let mut req = req;
        req.client_order_id = uuid_like.to_string();
        let prefix = "grid-eth-"; // 9 字符
        let (out, _) = prepare_live_order(req, None, prefix).unwrap();
        assert!(out.client_order_id.starts_with(prefix));
        assert_eq!(out.client_order_id.len(), MAX_CLIENT_ORDER_ID_LEN);
        assert!(
            out.client_order_id.ends_with("4567890ab"),
            "应保留原 id 尾部: {}",
            out.client_order_id
        );

        // 短 id → 直接拼接
        let mut req2 = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(1));
        req2.client_order_id = "abc".into();
        let (out2, _) = prepare_live_order(req2, None, "g-").unwrap();
        assert_eq!(out2.client_order_id, "g-abc");

        // 已带前缀 → 不重复注入
        let mut req3 = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, dec!(3000), dec!(1));
        req3.client_order_id = "g-xyz".into();
        let (out3, _) = prepare_live_order(req3, None, "g-").unwrap();
        assert_eq!(out3.client_order_id, "g-xyz");
    }
}
