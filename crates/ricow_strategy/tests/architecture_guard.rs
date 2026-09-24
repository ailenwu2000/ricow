//! 架构守卫 (030 策略/引擎分层收敛): 机械锁死「引擎/CLI 不读策略参数名、策略逻辑全在 Lua」。
//!
//! 规则权威定义见 `specs/architecture.md` 分层纪律。本文件用字符串扫描源码,
//! 宁可误报不可漏报 —— 误报时修正并在此说明理由, 漏报 = 架构失守(引擎重新染指策略知识)。
//!
//! 规则:
//!   A. 引擎层 (`ricow_engine/src`) 与绑定层 (`ricow_strategy/src/{context,lua}.rs`)
//!      零策略参数名 —— 策略知识只能活在 Lua 策略与测试里。
//!   B. CLI 层 (`ricow/src/commands`) 不得 `get_str/get_f64/get_i64` 读取策略参数名;
//!      通用配置键 (pair/leverage/... ) 天然不在黑名单内, 放行。
//!
//! 注记 (次级残留, Task 10 收口, 本守卫暂不拦「写入」):
//!   `inline_config` 与 `templates.rs` 的 `params.entry("...")` 是直跑模式默认参数表,
//!   属「CLI 知道内置策略默认参数」的残留 —— 违反目标 3 但不在本守卫三条硬规则的拦截面。

use std::path::Path;

/// workspace 根 (相对 crate 目录 `crates/ricow_strategy` 上溯两级)。
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// 策略参数名 (策略逻辑专属, 只允许出现在 Lua 策略、builtin_tests 与直跑模式默认参数表)。
/// 新增内置策略参数时须同步加入本清单, 否则黑名单漏拦。
const STRATEGY_PARAMS: &[&str] = &[
    "atr_interval",
    "regime_interval",
    "ema_interval",
    "atr_period",
    "regime_ema_period",
    "ema_fast",
    "ema_slow",
    "regime_filter",
    "regime_band_pct",
    "atr_mult",
    "trend_gate",
    "target_ratio",
    "leverage_mult",
    "real_cash",
    "initial_buy_amount",
    "start_price",
    "min_notional",
    "fee_side",
    "warmup_bars",
];

fn read(p: &str) -> String {
    std::fs::read_to_string(Path::new(p)).unwrap_or_else(|e| panic!("架构守卫读取 {p} 失败: {e}"))
}

/// 递归收集目录下所有 `.rs` 文件的绝对路径。
fn collect_rs(dir: &str, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("读目录 {dir} 失败: {e}")) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_rs(path.to_str().unwrap(), out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path.to_str().unwrap().to_string());
        }
    }
}

/// 规则 A: 引擎层 + 绑定层零策略参数名。
#[test]
fn engine_and_binding_layer_zero_strategy_params() {
    let mut files = Vec::new();
    collect_rs(&format!("{ROOT}/crates/ricow_engine/src"), &mut files);
    files.push(format!("{ROOT}/crates/ricow_strategy/src/context.rs"));
    files.push(format!("{ROOT}/crates/ricow_strategy/src/lua.rs"));
    for f in &files {
        let src = read(f);
        for p in STRATEGY_PARAMS {
            // 匹配带引号的字符串字面量 `"atr_interval"` —— 拦截实际读取;
            // 不拦同名 struct 字段 (如 SymbolInfo 的 `min_notional:` 元数据字段)。
            let needle = format!("\"{p}\"");
            assert!(
                !src.contains(&needle),
                "架构违规(规则 A): {f} 出现策略参数名 `{p}` —— 引擎/绑定层不得持有策略知识"
            );
        }
    }
}

/// 规则 B: CLI 层不得读取策略参数名。
#[test]
fn cli_must_not_read_strategy_params() {
    let mut files = Vec::new();
    collect_rs(&format!("{ROOT}/crates/ricow/src/commands"), &mut files);
    for f in &files {
        let src = read(f);
        for line in src.lines() {
            // 跳过纯注释行 (避免"旧代码 get_str(\"...\")"这类说明性注释误伤)。
            if line.trim_start().starts_with("//") {
                continue;
            }
            for method in ["get_str", "get_f64", "get_i64"] {
                let Some(rest) = line.split(method).nth(1) else { continue };
                let Some(q) = rest.find('"') else { continue };
                let after = &rest[q + 1..];
                let Some(e) = after.find('"') else { continue };
                let key = &after[..e];
                if STRATEGY_PARAMS.contains(&key) {
                    panic!("架构违规(规则 B): {f} 读取策略参数 `{key}` (行: {line})");
                }
            }
        }
    }
}
