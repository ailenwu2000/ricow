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
//! 黑名单(031 起) = 派生自策略清单 `strategies/{spot,futures}/*.toml` 的 `[[params]]` 键
//! (单一来源, 新增内置策略参数无需手动同步本文件)。通用配置键(`pair` 交易对 / `script` 脚本 /
//! `interval` 主时钟)不在此黑名单 —— 它们本就是 CLI/引擎要透传的通用项。

use std::path::Path;

/// workspace 根 (相对 crate 目录 `crates/ricow_strategy` 上溯两级)。
const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// 策略参数名全集(派生自清单, 非手抄): 扫描 `strategies/{spot,futures}/*.toml` 的 `[[params]]` 键。
/// 排除 `pair`(通用交易对配置)与 `script`(通用脚本注入)。
fn strategy_params() -> Vec<String> {
    let mut keys = Vec::new();
    for market in ["spot", "futures"] {
        let dir = Path::new(ROOT).join("strategies").join(market);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("key = \"") {
                    if let Some(end) = rest.find('"') {
                        let k = &rest[..end];
                        if !is_engine_channel_key(k) {
                            keys.push(k.to_string());
                        }
                    }
                }
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// 引擎通道键: 通用配置(`pair`/`script`/`interval`) + `[backtest]` resolve 通道的引擎级参数
/// (`leverage` 等)。它们由 CLI 按名写回 params 池(apply_backtest_cli)、引擎按名读取, 策略也可读
/// (如展示引擎实际杠杆) —— 语义是**引擎/CLI 配置**, 不是策略参数, 故不进黑名单, 也不要求清单
/// 声明(声明反而与 CLI flag 重复)。032 审核口径。
fn is_engine_channel_key(k: &str) -> bool {
    matches!(
        k,
        "pair"
            | "script"
            | "interval"
            | "leverage"
            | "max_leverage"
            | "mmr_pct"
            | "funding_rate_8h"
            | "fee_maker_bps"
            | "fee_taker_bps"
            | "slippage_bps"
            | "initial_cash"
    )
}

/// 读配置的方法 (策略参数只能通过这些方法从配置池读出)。
const READ_METHODS: &[&str] = &[
    "get_str",
    "get_f64",
    "get_i64",
    "get_bool",
    "config_str",
    "config_f64",
    "config_i64",
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
fn assert_no_strategy_param(params: &[String], f: &str, line: &str, rule: &str) {
    for m in READ_METHODS.iter().chain(WRITE_METHODS) {
        if let Some(key) = extract_str_arg(line, m) {
            assert!(
                !params.iter().any(|k| k == key),
                "架构违规({rule}): {f} 生产代码读/写策略参数 `{key}` (行: {line})"
            );
        }
    }
}

/// 规则 A: 引擎层 + 绑定层生产代码零策略参数名。
#[test]
fn engine_and_binding_layer_zero_strategy_params() {
    let params = strategy_params();
    assert!(!params.is_empty(), "策略清单未派生到任何参数名(清单缺失?)");
    let mut files = Vec::new();
    collect_rs(&format!("{ROOT}/crates/ricow_engine/src"), &mut files);
    files.push(format!("{ROOT}/crates/ricow_strategy/src/context.rs"));
    files.push(format!("{ROOT}/crates/ricow_strategy/src/lua.rs"));
    files.push(format!("{ROOT}/crates/ricow_strategy/src/backtest.rs"));
    for f in &files {
        let src = read(f);
        let prod = production_lines(&src);
        for line in prod {
            // 跳过纯注释行 (说明性注释提策略参数名不构成"持有策略知识")。
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert_no_strategy_param(&params, f, line, "规则 A");
        }
    }
}

/// 规则 B: CLI 层不得读/写策略参数名。
#[test]
fn cli_must_not_read_or_write_strategy_params() {
    let params = strategy_params();
    let mut files = Vec::new();
    collect_rs(&format!("{ROOT}/crates/ricow/src/commands"), &mut files);
    for f in &files {
        let src = read(f);
        let prod = production_lines(&src);
        for line in prod {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert_no_strategy_param(&params, f, line, "规则 B");
        }
    }
}

/// 清单参数键 == 同名 Lua 读取键(防清单与 Lua 漂移, FR-018)。
///
/// 按**同名配对**逐策略比较(.toml ↔ 同目录同 stem 的 .lua), 而非全目录并集 ——
/// 031 起 `ricow deploy` 会把无清单的用户策略 .lua 落进 `spot/`, 并集比较会被其读取键污染。
/// 有清单无脚本 / 有脚本无清单 的半边缺失不在本校验范围(前者由 catalog 扫描报错, 后者合成最小清单)。
#[test]
fn manifest_keys_match_lua_read_keys() {
    let mut checked = 0;
    for market in ["spot", "futures"] {
        let dir = Path::new(ROOT).join("strategies").join(market);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let lua_path = path.with_extension("lua");
            if !lua_path.exists() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Ok(src) = std::fs::read_to_string(&lua_path) else { continue };

            let mut manifest: Vec<String> = text
                .lines()
                .filter_map(|l| {
                    let rest = l.trim().strip_prefix("key = \"")?;
                    rest.find('"').map(|end| rest[..end].to_string())
                })
                .filter(|k| !is_engine_channel_key(k))
                .collect();
            manifest.sort();
            manifest.dedup();
            let mut lua: Vec<String> =
                lua_read_keys(&src).into_iter().filter(|k| !is_engine_channel_key(k)).collect();
            lua.sort();
            lua.dedup();

            assert_eq!(
                manifest,
                lua,
                "清单 {} 的参数键必须与同名 Lua 读取键(config_*/num/cfg_str)一致",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 2, "至少应校验两个内置策略的清单↔Lua 一致性, 实际 {checked}");
}

/// 提取 Lua 源码里读取的策略参数键: `config_*(` / `num(ctx, ` / `cfg_str(ctx, ` 后的第一个引号串。
fn lua_read_keys(src: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for m in ["config_f64(", "config_i64(", "config_str(", "config_bool("] {
        for (i, _) in src.match_indices(m) {
            if let Some(k) = quoted_arg(&src[i + m.len()..]) {
                keys.push(k);
            }
        }
    }
    for m in ["num(ctx, ", "cfg_str(ctx, "] {
        for (i, _) in src.match_indices(m) {
            if let Some(k) = quoted_arg(&src[i + m.len()..]) {
                keys.push(k);
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// 从 `"key"...` 提取第一个引号串。
fn quoted_arg(s: &str) -> Option<String> {
    let s = s.trim_start();
    let s = s.strip_prefix('"')?;
    let end = s.find('"')?;
    Some(s[..end].to_string())
}
