//! 架构守卫 (030 策略/引擎分层收敛): 机械锁死「引擎/CLI 不读/写策略参数名、策略逻辑全在 Lua」。
//!
//! 规则权威定义见 `specs/architecture.md` 分层纪律。本文件用字符串扫描源码,
//! 宁可误报不可漏报 —— 误报时修正并在此说明理由, 漏报 = 架构失守(引擎重新染指策略知识)。
//!
//! 规则:
//!   A. 引擎层 (`ricow_engine/src`) 与绑定层 (`ricow_strategy/src/{context,lua}.rs`)
//!      生产代码零策略参数名 —— 策略知识只能活在 Lua 策略里(测试代码豁免)。
//!   B. CLI 层 (`ricow/src/commands`) 不得读/写策略参数名。
//!
//! 匹配方式 = 精确匹配「读/写配置的方法调用」(get_str/config_f64/params.entry/... ),
//! 不匹配裸字面量 —— 避免误伤同名 struct/订单字段 (如 OrderRequest 的 `side`、SymbolInfo
//! 的 `min_notional` 都是字段名, 不是策略参数)。
//!
//! 黑名单 = 全部内置策略参数名的全集, 来源 = `strategies/builtin/**` 的 config_xxx/num/cfg_str
//! 调用键(实测提取) + templates.rs 的 params 数组。新增内置策略参数须同步加入(否则漏拦)。
//! 注意 `pair` 是通用配置(交易对, CLI/引擎本来就该知道), 不在此黑名单。

use std::path::Path;

/// workspace 根 (相对 crate 目录 `crates/ricow_strategy` 上溯两级)。
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// 策略参数名全集(策略逻辑专属, 只允许出现在 Lua 策略与测试代码里)。
const STRATEGY_PARAMS: &[&str] = &[
    "activation_price",
    "atr_interval",
    "atr_mult",
    "atr_period",
    "bar_seconds",
    "dd_stop_pct",
    "distribution",
    "fee_side",
    "initial_buy_amount",
    "interval_secs",
    "leverage_basis",
    "leverage_mult",
    "lookback_bars",
    "lower_price",
    "max_buys",
    "min_notional",
    "num_levels",
    "num_slices",
    "order_size",
    "pause_bars",
    "pause_pct",
    "pullback_abs",
    "pullback_pct",
    "real_cash",
    "rebalance_band",
    "regime_band_pct",
    "regime_ema_period",
    "regime_filter",
    "regime_interval",
    "side",
    "slice_interval_secs",
    "start_price",
    "target_ratio",
    "total_size",
    "trend_gate",
    "upper_price",
];

/// 读配置的方法 (策略参数只能通过这些方法从配置池读出)。
const READ_METHODS: &[&str] = &[
    "get_str", "get_f64", "get_i64", "get_bool", "config_str", "config_f64", "config_i64",
    "config_bool",
];

/// 写配置的方法 (直跑模式默认参数表 / 透传)。
const WRITE_METHODS: &[&str] = &["params.entry", "params.insert"];

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

/// 计算一行的大括号净增量 (`{` 数 − `}` 数), 用于定位块边界。
fn brace_delta(line: &str) -> i32 {
    line.matches('{').count() as i32 - line.matches('}').count() as i32
}

/// 生产代码行 = 跳过 `#[cfg(test)]` 标记的项(测试模块/测试辅助方法, 测试用策略参数名合法)。
/// 用 `{}` 深度匹配定位块边界 —— 不能简单截断: context.rs 的 `mod tests` 在文件中间,
/// 其后还有生产 impl。
fn production_lines(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip_depth = 0i32;
    let mut pending_cfg_test = false;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if pending_cfg_test {
            // 本行是 `#[cfg(test)]` 标记项的开头(mod tests { / fn xxx { / 单行项)。
            skip_depth = brace_delta(line);
            pending_cfg_test = false;
            continue;
        }
        if skip_depth > 0 {
            skip_depth += brace_delta(line);
            continue;
        }
        if trimmed == "#[cfg(test)]" {
            pending_cfg_test = true;
            continue;
        }
        out.push(line);
    }
    out
}

/// 提取 `method("key")` 里的 key (method 形如 `get_str` / `params.entry`)。
fn extract_str_arg<'a>(line: &'a str, method: &str) -> Option<&'a str> {
    let rest = line.split(method).nth(1)?;
    let q = rest.find('"')?;
    let after = &rest[q + 1..];
    let e = after.find('"')?;
    Some(&after[..e])
}

/// 检查一行是否出现「读/写策略参数」, 命中则 panic。
fn assert_no_strategy_param(f: &str, line: &str, rule: &str) {
    for m in READ_METHODS.iter().chain(WRITE_METHODS) {
        if let Some(key) = extract_str_arg(line, m) {
            assert!(
                !STRATEGY_PARAMS.contains(&key),
                "架构违规({rule}): {f} 生产代码读/写策略参数 `{key}` (行: {line})"
            );
        }
    }
}

/// 规则 A: 引擎层 + 绑定层生产代码零策略参数名。
#[test]
fn engine_and_binding_layer_zero_strategy_params() {
    let mut files = Vec::new();
    collect_rs(&format!("{ROOT}/crates/ricow_engine/src"), &mut files);
    files.push(format!("{ROOT}/crates/ricow_strategy/src/context.rs"));
    files.push(format!("{ROOT}/crates/ricow_strategy/src/lua.rs"));
    for f in &files {
        let src = read(f);
        let prod = production_lines(&src);
        for line in prod {
            // 跳过纯注释行 (说明性注释提策略参数名不构成"持有策略知识")。
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert_no_strategy_param(f, line, "规则 A");
        }
    }
}

/// 规则 B: CLI 层不得读/写策略参数名。
#[test]
fn cli_must_not_read_or_write_strategy_params() {
    let mut files = Vec::new();
    collect_rs(&format!("{ROOT}/crates/ricow/src/commands"), &mut files);
    for f in &files {
        let src = read(f);
        let prod = production_lines(&src);
        for line in prod {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert_no_strategy_param(f, line, "规则 B");
        }
    }
}
