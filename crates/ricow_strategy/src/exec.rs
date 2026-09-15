//! exec 执行组件库 (引擎内置 Rust 实现)。
//!
//! 引擎能力与 ctx.* 同层: 档位生成 / 回调判断 / 间隔换算 / 切片到期 / 报价推导 / 对手价订单。
//! 注册为 Lua 全局表 `exec`(见 [`register`]), 策略脚本直接调用 `exec.*`,
//! 也可在脚本内覆盖同名函数 (复制即自定义)。
//!
//! API 规范见 `specs/lua-api.md` "exec 执行组件" 章节。

use mlua::{AnyUserData, Lua};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::lua::LuaCtxData;

/// 档位生成 (ladder): lower~upper 间 n 档, geo=true 等比 (对数刻度), 否则等差。
///
/// equal 用 Decimal 精确等差 (对齐旧 Rust generate_levels), geo 用 f64 对数刻度。
/// n < 2 返回空 —— 行为修正: Lua 版 n=1 时 i/(n-1) 除零得 NaN 档位。
pub fn levels(lower: f64, upper: f64, n: i64, geo: bool) -> Vec<f64> {
    if n < 2 {
        return Vec::new();
    }
    let n = n as usize;
    if geo {
        (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                lower * (upper / lower).powf(t)
            })
            .collect()
    } else {
        let lower_d = Decimal::from_f64_retain(lower).unwrap_or(Decimal::ZERO);
        let step = (Decimal::from_f64_retain(upper).unwrap_or(Decimal::ZERO) - lower_d)
            / Decimal::from((n - 1) as u64);
        (0..n)
            .map(|i| (lower_d + step * Decimal::from(i as u64)).to_f64().unwrap_or(lower))
            .collect()
    }
}

/// 回调触发判断 (pullback): side="buy" 回撤触发 (跌 pct/abs), "sell" 对称 (涨 pct/abs)。
/// pct/abs 为 None 表示不启用该模式。
pub fn pullback_triggered(
    side: &str,
    px: f64,
    extreme: f64,
    pct: Option<f64>,
    abs: Option<f64>,
) -> bool {
    if side == "buy" {
        if let Some(pct) = pct {
            if px <= extreme * (1.0 - pct) {
                return true;
            }
        }
        if let Some(abs) = abs {
            if px <= extreme - abs {
                return true;
            }
        }
    } else {
        if let Some(pct) = pct {
            if px >= extreme * (1.0 + pct) {
                return true;
            }
        }
        if let Some(abs) = abs {
            if px >= extreme + abs {
                return true;
            }
        }
    }
    false
}

/// 报价资产: 从 pair 尾部推导 (BNBUSDT → USDT), 兜底 "USDT"。
pub fn detect_quote(pair: &str) -> String {
    const QUOTES: [&str; 8] = ["USDT", "USDC", "BUSD", "FDUSD", "TUSD", "DAI", "EUR", "USD"];
    for q in QUOTES {
        if pair.len() > q.len() && pair.ends_with(q) {
            return q.to_string();
        }
    }
    "USDT".to_string()
}

/// tick 间隔换算: interval_secs / bar_secs 个 tick, 至少 1。
///
/// ⚠️ 守卫: interval<=0 或 bar_secs<=0 时返回 1 — CLI 直跑不注入 bar_seconds (缺省 0),
/// 无守卫会除零得 inf → 策略永不触发。
pub fn ticks_per(interval_secs: i64, bar_secs: i64) -> i64 {
    if interval_secs <= 0 || bar_secs <= 0 {
        return 1;
    }
    (interval_secs as f64 / bar_secs as f64 + 0.5).floor().max(1.0) as i64
}

/// TWAP 切片到期判断 (纯函数, 状态由策略持有; done 由策略先判再调用)。
/// 返回 true = 本片到期: 首片立即 (slices_sent==0), 之后每 ticks_per_slice 一片。
pub fn slice_due(tick_count: i64, start_tick: i64, slices_sent: i64, ticks_per_slice: i64) -> bool {
    if slices_sent == 0 {
        return true;
    }
    (tick_count - start_tick) >= ticks_per_slice * slices_sent
}

/// 注册 exec 全局表到沙箱引擎 (from_source 加载策略时调用)。
///
/// 纯函数经闭包包装 (mlua create_function 要求返回 mlua::Result, 纯函数保持纯签名便于单测);
/// `exec.side_order` 直接注册 (自身返回 Result)。用户脚本 `exec.xxx = ...` 覆盖仍生效 (全局表)。
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let exec = lua.create_table()?;
    exec.set(
        "levels",
        lua.create_function(|_, (lower, upper, n, geo): (f64, f64, i64, bool)| {
            Ok(levels(lower, upper, n, geo))
        })?,
    )?;
    exec.set(
        "pullback_triggered",
        lua.create_function(
            |_, (side, px, extreme, pct, abs): (String, f64, f64, Option<f64>, Option<f64>)| {
                Ok(pullback_triggered(&side, px, extreme, pct, abs))
            },
        )?,
    )?;
    exec.set("detect_quote", lua.create_function(|_, pair: String| Ok(detect_quote(&pair)))?)?;
    exec.set(
        "ticks_per",
        lua.create_function(|_, (interval_secs, bar_secs): (i64, i64)| {
            Ok(ticks_per(interval_secs, bar_secs))
        })?,
    )?;
    exec.set(
        "slice_due",
        lua.create_function(
            |_, (tick_count, start_tick, slices_sent, ticks_per_slice): (i64, i64, i64, i64)| {
                Ok(slice_due(tick_count, start_tick, slices_sent, ticks_per_slice))
            },
        )?,
    )?;
    exec.set(
        "side_order",
        lua.create_function(|lua, (ctx, pair, side, size): (AnyUserData, String, String, f64)| {
            let data = ctx.borrow::<LuaCtxData>().map_err(|e| {
                mlua::Error::RuntimeError(format!("exec.side_order: ctx downcast 失败: {e}"))
            })?;
            let price = if side == "buy" { data.best_ask(&pair) } else { data.best_bid(&pair) };
            let order = lua.create_table()?;
            order.set("pair", pair)?;
            order.set("side", side)?;
            order.set("size", size)?;
            if let Some(p) = price {
                order.set("price", p)?;
                order.set("order_type", "limit")?;
            } else {
                order.set("order_type", "market")?;
            }
            Ok(order)
        })?,
    )?;
    lua.globals().set("exec", exec)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levels_equal() {
        // 已知向量: lower 90 / upper 110 / n 3 → [90, 100, 110] (精确)。
        let out = levels(90.0, 110.0, 3, false);
        assert_eq!(out, vec![90.0, 100.0, 110.0]);
    }

    #[test]
    fn test_levels_geometric() {
        // 已知向量: lower 100 / upper 400 / n 3 → [100, ~200, 400]。
        let out = levels(100.0, 400.0, 3, true);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 100.0).abs() < 0.01, "首档 = {}", out[0]);
        assert!((out[1] - 200.0).abs() < 1.0, "等比中项 ≈ 200, got {}", out[1]);
        assert!((out[2] - 400.0).abs() < 0.01, "末档 = {}", out[2]);
    }

    #[test]
    fn test_levels_n_below_2_empty() {
        // 行为修正: n<2 返回空 (Lua 版 n=1 除零得 NaN 档位)。
        assert!(levels(100.0, 200.0, 1, false).is_empty());
        assert!(levels(100.0, 200.0, 1, true).is_empty());
        assert!(levels(100.0, 200.0, 0, false).is_empty());
    }

    #[test]
    fn test_pullback_triggered() {
        // 95 vs extreme 100 回撤 5% → 触发; 99.9 回撤不足 → 不触发。
        assert!(pullback_triggered("buy", 95.0, 100.0, Some(0.05), None));
        assert!(!pullback_triggered("buy", 99.9, 100.0, Some(0.05), None));
        // sell 对称: 反弹 5% 触发。
        assert!(pullback_triggered("sell", 105.0, 100.0, Some(0.05), None));
        assert!(!pullback_triggered("sell", 100.1, 100.0, Some(0.05), None));
        // abs 模式。
        assert!(pullback_triggered("buy", 94.0, 100.0, None, Some(6.0)));
        assert!(!pullback_triggered("buy", 94.5, 100.0, None, Some(6.0)));
        // 均未启用 → 恒 false。
        assert!(!pullback_triggered("buy", 50.0, 100.0, None, None));
    }

    #[test]
    fn test_detect_quote() {
        assert_eq!(detect_quote("BNBUSDT"), "USDT");
        assert_eq!(detect_quote("ETHUSDC"), "USDC");
        assert_eq!(detect_quote("BTCUSDT"), "USDT");
        assert_eq!(detect_quote("ETH"), "USDT"); // 无报价后缀 → 兜底
    }

    #[test]
    fn test_ticks_per() {
        assert_eq!(ticks_per(7200, 3600), 2, "7200/3600 → 2 tick");
        assert_eq!(ticks_per(3600, 0), 1, "bar_secs=0 守卫 → 1 tick");
        assert_eq!(ticks_per(0, 3600), 1, "interval=0 守卫 → 1 tick");
        assert_eq!(ticks_per(3600, 7200), 1, "不足一片 → 至少 1");
    }

    #[test]
    fn test_slice_due() {
        assert!(slice_due(5, 1, 2, 2), "(5-1) >= 2*2 → 第 3 片到期");
        assert!(!slice_due(4, 1, 2, 2), "3 < 4 → 间隔未到");
        assert!(slice_due(10, 5, 0, 2), "slices_sent==0 → 首片立即");
    }
}
