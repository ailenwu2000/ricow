//! 从 Lua 源码**静态提取**参数骨架 (052 方案 2)。
//!
//! 动机(见 `specs/research/webui-strategy-config-gap-2026-10-11.md`):
//! 策略的「参数有哪些 / 叫什么 / 默认多少」四件事**全在清单里, 不在 Lua 里** —— 复制策略时
//! 清单不跟着走, 用户打开详情页面对一片空白。可**参数读取调用本身是字面量**:
//!
//! ```lua
//! local spacing_pct = num(ctx, "spacing_pct", 0.01)
//! local mode        = cfg_str(ctx, "accumulate_mode", "u")
//! local pair        = ctx:config_str("pair")
//! ```
//!
//! 于是「这个策略有哪些旋钮、默认多少」这件事**纯静态扫描就能拿到**, 既不需要 AI,
//! 也用不着用户去读代码。本模块只做这一件事, 是一个**纯函数**(无 IO、无 await)。
//!
//! ## 只提取"客观事实"
//!
//! 抽出来的是 **键 / 类型 / 字面量兜底默认 / 注释里原样搬来的说明** 四项。
//! **不猜中文名、不编说明** —— 名字是作者意图, 由用户填或 AI 生成后由用户核对(N2-C);
//! 说明只在脚本注释**明写**时原样搬走(见 [`comment_docs`]), 提不出来就留空。
//!
//! ## 边界(如实)
//!
//! - **变量键不提取**: `ctx:config_f64(key)` / `num(ctx, some_var, d)` 里键是变量, 不是事实,
//!   一律跳过(内置脚本的 `num`/`cfg_str` 辅助函数体正是这种形态 —— 它们不会被误当成参数)。
//! - **语义默认拿不到**: `num(ctx,"min_pair_profit",0)` 的字面量兜底是 `0`, 而清单里记的是
//!   语义默认 `0.002`(代码随后有 `if <= 0 then 2*fee_side` 的修正)。静态扫描只认字面量,
//!   两处不一致时**如实保留差异**, 不为了让断言好看去对齐任何一侧。
//! - **说明只搬不切**: 注释行 `-- spacing_pct  间距百分比(小数), 默认 0.01` 里那句说明整段照搬,
//!   但**不从中切一个"中文名"出来** —— 实测首段不一定是名字(`pair` 那行首段是「必填」),
//!   切错比留空更糟。

use std::collections::HashMap;

use ricow_strategy::ConfigValue;
use serde::{Deserialize, Serialize};

use super::catalog::ParamType;

/// 一条静态提取出的参数骨架。
///
/// 注: **不派生 `PartialEq`** —— 内含的 `ConfigValue` 没有实现它(刻意不碰 `ricow_strategy`
/// 的公共接口)。测试里用 [`tests::show`] 归一成可比较的字符串。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamHint {
    /// 参数键名(与 Lua `config_*`/`num`/`cfg_str` 读取键一致)。
    pub key: String,
    /// 推断类型 —— 取 **API 语义**(读它的函数决定了它是什么): `num`/`config_f64` → f64,
    /// `config_i64` → i64, `cfg_str`/`config_str` → string, `config_bool` → bool。
    #[serde(rename = "type")]
    pub ty: ParamType,
    /// 代码里的**字面量**兜底默认(无字面量则缺省)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ConfigValue>,
    /// 脚本注释里写的说明(**原文照搬**, 见 [`comment_docs`])。注释没写就是 `None` ——
    /// 这时界面与 AI 都要按"没有依据"处理, 不许自己补一句。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// 引擎内部键: 由 loader 装载脚本使用, 不是用户参数(与 `strategy_io::RESERVED_PARAM_KEYS` 同口径)。
const ENGINE_KEYS: &[&str] = &["script", "script_path"];

/// 六种被识别的调用标记: (标记文本, 类型, 是否 num/cfg_str 形态 —— 那种形态第 0 个实参须是 `ctx`)。
const MARKERS: &[(&str, ParamType, bool)] = &[
    ("num(", ParamType::Float, true),
    ("cfg_str(", ParamType::Str, true),
    ("ctx:config_f64(", ParamType::Float, false),
    ("ctx:config_i64(", ParamType::Int, false),
    ("ctx:config_str(", ParamType::Str, false),
    ("ctx:config_bool(", ParamType::Bool, false),
];

/// 从 Lua 源码提取参数骨架(按**首次出现顺序**去重)。
///
/// 例: 喂 `strategies/spot/linear_position_grid.lua` → 19 个键, 与 `linear_position_grid.toml` 声明的键集合一致。
pub fn extract_param_hints(code: &str) -> Vec<ParamHint> {
    let orig: Vec<char> = code.chars().collect();
    let mask = mask_comments_and_strings(&orig);

    // 先收集"命中位置 + 骨架", 最后按位置排序。**不能边扫边 push**: 外层遍历的是 MARKERS
    // 标记表, 直接 push 会让输出顺序取决于标记表顺序(所有 `num` 挤在一起), 而不是策略作者
    // 写参数的顺序 —— 清单声明的顺序就是表单顺序, 顺序错了表单也跟着乱。
    let mut hits: Vec<(usize, ParamHint)> = Vec::new();
    // 注释里的说明: 单独扫一遍(与代码扫描互不干扰), 命中同键时原样搬走。
    let docs = comment_docs(code);

    for &(marker, ty, takes_ctx) in MARKERS {
        let mchars: Vec<char> = marker.chars().collect();
        let mut i = 0usize;
        while i + mchars.len() <= mask.len() {
            if mask[i..i + mchars.len()] != mchars[..] {
                i += 1;
                continue;
            }
            // 词边界: 标记前一个字符不能是标识符字符(否则 `my_num(` 也会命中)。
            if i > 0 && is_ident_char(mask[i - 1]) {
                i += 1;
                continue;
            }
            let lparen = i + mchars.len() - 1;
            let args = parse_call_args(&orig, lparen);
            // `num(ctx, "k", d)` 形态: 第 0 个实参必须字面是 `ctx`。
            let key_idx = if takes_ctx {
                if args.first().map(|a| a.trim()) != Some("ctx") {
                    i = lparen + 1;
                    continue;
                }
                1
            } else {
                0
            };
            if let Some(key) = args.get(key_idx).and_then(|a| parse_string_literal(a.trim())) {
                if !key.is_empty() && !ENGINE_KEYS.contains(&key.as_str()) {
                    // 默认值只认字面量; 按 API 语义的类型归一(整数默认值落到 f64 参数上要转成浮点)。
                    let default = args
                        .get(key_idx + 1)
                        .and_then(|a| parse_default_literal(a.trim()))
                        .map(|d| coerce_like(ty, d));
                    let desc = docs.get(&key).cloned();
                    hits.push((i, ParamHint { key, ty, default, desc }));
                }
            }
            i = lparen + 1;
        }
    }

    hits.sort_by_key(|(pos, _)| *pos);
    let mut out: Vec<ParamHint> = Vec::new();
    for (_, h) in hits {
        if !out.iter().any(|x| x.key == h.key) {
            out.push(h);
        }
    }
    out
}

/// 从脚本注释里捡「键 + 说明」形态的说明原文 (052 / N2)。
///
/// 只认**整行注释**(行首空白后即 `--`, 块注释 `--[[` 不算), 且去掉 `--` 后
/// **第一段是合法参数键、后面还有非空说明** 的行。命中就把后半段**原样搬走** ——
/// 不缩写、不润色、不加标点, 因为那都会变成"替作者说话"。同一键出现多次取首次。
///
/// 内置脚本的参数清单块正是这个形态:
/// ```lua
/// --   spacing_pct       间距百分比(小数, 0.01=1%), 默认 0.01
/// ```
///
/// **为什么只搬说明、不切中文名**: 实测首段不一定是名字 ——
/// `--   pair              必填, 交易对(现货)` 的首段是「必填」。按"首段即名字"切会切出一个
/// 叫「必填」的参数, 界面上显示一个错名字**比显示键名更糟**(用户会当真)。名字留给用户或
/// AI(N2-C)。
pub fn comment_docs(code: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for line in code.lines() {
        let Some(rest) = line.trim_start().strip_prefix("--") else {
            continue;
        };
        // 块注释 `--[[` / `--[==[` 不是「键 + 说明」形态。
        if rest.starts_with('[') {
            continue;
        }
        let body = rest.trim();
        let Some((key, doc)) = body.split_once(char::is_whitespace) else {
            continue;
        };
        let doc = doc.trim();
        if doc.is_empty() || !is_param_key(key) || ENGINE_KEYS.contains(&key) {
            continue;
        }
        out.entry(key.to_string()).or_insert_with(|| doc.to_string());
    }
    out
}

/// 参数键规范(与 `web::strategy_io::valid_param_key` 同口径, 但不跨模块依赖 web 层):
/// 字母/下划线开头, 只含字母 / 数字 / 下划线。
fn is_param_key(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '.'
}

/// 造一份**等长**副本, 把注释与字符串字面量的内容换成空格(换行保留)。
///
/// 目的: 在 "源码里找调用标记" 这一步不会命中注释里或字符串里的同名文本
/// (例如注释里写 `-- 这里 num(ctx, "x", 1) 是示例`)。字符数一一对应, 故下标可直接对回原文。
///
/// 覆盖四种形态: `--…` 行注释 / `--[[…]]`(含 `--[==[…]==]`)块注释 /
/// `"…"`·`'…'` 引号串 / `[[…]]`(含 `[==[…]==]`)长括号串。
fn mask_comments_and_strings(orig: &[char]) -> Vec<char> {
    let mut out = vec![' '; orig.len()];
    let mut i = 0usize;
    while i < orig.len() {
        let c = orig[i];
        // 注释: 行注释 `--…` 或块注释 `--[==[…]==]`。
        if c == '-' && orig.get(i + 1) == Some(&'-') {
            if let Some(level) = long_bracket_level(orig, i + 2) {
                i = skip_long_bracket(orig, i + 2, level);
                continue;
            }
            while i < orig.len() && orig[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // 长括号串 `[[…]]` / `[==[…]==]`(先于表下标判断: 只有 `[` 后跟 `=`*`[` 才算)。
        if c == '[' {
            if let Some(level) = long_bracket_level(orig, i) {
                i = skip_long_bracket(orig, i, level);
                continue;
            }
        }
        // 引号串 `"…"` / `'…'`(内容与引号一起遮罩; 实参解析回到原文取字面量)。
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            while i < orig.len() {
                if orig[i] == '\\' {
                    i += 2;
                    continue;
                }
                if orig[i] == quote || orig[i] == '\n' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out[i] = c;
        i += 1;
    }
    // 换行保留: 遮罩区里原本的换行照写回(位置信息不丢, 调试时也能对上原文)。
    for (k, ch) in orig.iter().enumerate() {
        if *ch == '\n' {
            out[k] = '\n';
        }
    }
    out
}

/// `at` 处是不是长括号开括号 `[` `=`* `[`(返回 `=` 的个数); 不是则 `None`。
fn long_bracket_level(orig: &[char], at: usize) -> Option<usize> {
    if orig.get(at) != Some(&'[') {
        return None;
    }
    let mut j = at + 1;
    let mut level = 0usize;
    while orig.get(j) == Some(&'=') {
        level += 1;
        j += 1;
    }
    (orig.get(j) == Some(&'[')).then_some(level)
}

/// 从 `at`(开括号 `[` 的位置)跳到长括号闭合处之后; 返回新下标。
fn skip_long_bracket(orig: &[char], at: usize, level: usize) -> usize {
    // 闭合形态 = `]` + `=`×level + `]`(直接拼串再转字符, 免得 `Vec::push` 在循环里被 lint 盯上)。
    let close: Vec<char> = format!("]{}]", "=".repeat(level)).chars().collect();
    let mut j = at + level + 2; // 越过 `[` + `=`×level + `[`
    while j < orig.len() {
        if orig[j..].starts_with(&close[..]) {
            return j + close.len();
        }
        j += 1;
    }
    orig.len()
}

/// 从 `lparen`(原文里 `(` 的下标)起取出实参表: 顶层逗号切分, 跳过嵌套括号与字符串。
/// 找不到配对右括号 → 返回已收集的部分(调用方按"实参不足"丢弃)。
fn parse_call_args(orig: &[char], lparen: usize) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut i = lparen;
    while i < orig.len() {
        let c = orig[i];
        match c {
            '(' => {
                depth += 1;
                if depth > 1 {
                    cur.push(c);
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    args.push(cur.clone());
                    return args;
                }
                cur.push(c);
            }
            ',' if depth == 1 => {
                args.push(std::mem::take(&mut cur));
            }
            '"' | '\'' => {
                // 整段字符串原样搬进实参(含引号与转义), 供 parse_string_literal 解析。
                let quote = c;
                cur.push(c);
                i += 1;
                while i < orig.len() {
                    let d = orig[i];
                    cur.push(d);
                    if d == '\\' {
                        if let Some(next) = orig.get(i + 1) {
                            cur.push(*next);
                            i += 2;
                            continue;
                        }
                    }
                    if d == quote {
                        break;
                    }
                    i += 1;
                }
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    args
}

/// 解析字符串字面量(去引号 + 最小反转义)。不是字面量(变量 / 数字) → `None`。
fn parse_string_literal(s: &str) -> Option<String> {
    let mut it = s.chars();
    let quote = it.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    if !s.ends_with(quote) || s.len() < 2 {
        return None;
    }
    let body = &s[quote.len_utf8()..s.len() - quote.len_utf8()];
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some(other) => out.push(other),
            None => break,
        }
    }
    Some(out)
}

/// 解析兜底默认的字面量: 布尔 / `nil`(→无) / 字符串 / 数字(带小数点或指数 → 浮点, 否则整数)。
fn parse_default_literal(s: &str) -> Option<ConfigValue> {
    match s {
        "true" => return Some(ConfigValue::Boolean(true)),
        "false" => return Some(ConfigValue::Boolean(false)),
        // `nil` 兜底 = 没有默认(如 `num(ctx, "x", nil)`)。
        "nil" => return None,
        "" => return None,
        _ => {}
    }
    if let Some(text) = parse_string_literal(s) {
        return Some(ConfigValue::String(text));
    }
    if !s.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+') {
        return None;
    }
    if s.contains('.') || s.contains('e') || s.contains('E') {
        return s.parse::<f64>().ok().map(ConfigValue::Float);
    }
    match s.parse::<i64>() {
        Ok(i) => Some(ConfigValue::Integer(i)),
        // 超出 i64 的整数字面量: 退化成浮点也比丢掉好。
        Err(_) => s.parse::<f64>().ok().map(ConfigValue::Float),
    }
}

/// 把字面量默认值按**读它的 API 类型**归一: `num` 读到的一定是浮点,
/// 故 `num(ctx,"order_amount",10)` 的默认应记 `10.0` 而不是 `10`。
fn coerce_like(ty: ParamType, v: ConfigValue) -> ConfigValue {
    match (ty, v) {
        (ParamType::Float, ConfigValue::Integer(i)) => ConfigValue::Float(i as f64),
        (ParamType::Int, ConfigValue::Float(f)) if f.is_finite() && f.fract() == 0.0 => {
            ConfigValue::Integer(f as i64)
        }
        (_, other) => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(code: &str) -> Vec<(String, ParamType, String)> {
        extract_param_hints(code).into_iter().map(|h| (h.key, h.ty, show(&h.default))).collect()
    }

    /// 把默认值归一成可比较的字符串(`ConfigValue` 没有 `PartialEq`, 也不必为测试给它加)。
    fn show(v: &Option<ConfigValue>) -> String {
        match v {
            None => "none".to_string(),
            Some(ConfigValue::Float(f)) => format!("f64:{f}"),
            Some(ConfigValue::Integer(i)) => format!("i64:{i}"),
            Some(ConfigValue::String(s)) => format!("str:{s}"),
            Some(ConfigValue::Boolean(b)) => format!("bool:{b}"),
        }
    }

    /// N2-B (2026-10-11): 说明取自**脚本注释原文**, 一个字都不加工; 注释没写就留空。
    #[test]
    fn comment_docs_are_taken_verbatim_from_comments() {
        let code = r#"
-- ═══ 参数(全部可配) ═══
--   spacing_pct       间距百分比(小数, 0.01=1%), 默认 0.01
--   pair              必填, 交易对(现货)
local s = num(ctx, "spacing_pct", 0.01)
local p = ctx:config_str("pair")
local no_doc = ctx:config_f64("atr_mult")
"#;
        let got = extract_param_hints(code);
        let at = |k: &str| got.iter().find(|h| h.key == k).unwrap();
        assert_eq!(
            at("spacing_pct").desc.as_deref(),
            Some("间距百分比(小数, 0.01=1%), 默认 0.01"),
            "注释里的说明必须原样搬走(不缩写、不润色)"
        );
        // `pair` 那行首段是「必填」而不是名字 —— 正因为注释里首段不一定是名字, 这里只搬说明、
        // 不切名字(切出一个叫「必填」的参数比留空更糟)。
        assert_eq!(at("pair").desc.as_deref(), Some("必填, 交易对(现货)"));
        assert!(at("atr_mult").desc.is_none(), "注释没写说明时必须留空, 不许编");
    }

    /// N2-B 边界: 说明只认「整行注释 + 首段是合法键 + 后面还有内容」, 且**只对真参数键生效**
    /// —— 注释里那些恰好长得像键的词不会污染输出。
    #[test]
    fn comment_docs_only_attach_to_real_params() {
        let code = r#"
-- 这是一句普通的说明文字, 不是参数行
--   spacing_pct
--[[ spacing_pct 块注释里的不算 ]]
local s = num(ctx, "spacing_pct", 0.01)
"#;
        let got = extract_param_hints(code);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "spacing_pct");
        assert!(got[0].desc.is_none(), "注释里只有裸键(无说明)时, 说明必须留空");
    }

    /// N2-B 真机: 内置 `paired_grid_futures_long.lua` 注释块里的说明能被带出来。
    ///
    /// 这条是**跨文件**断言, 价值在于钉住两个互相独立的来源: 键来自代码里的
    /// `num(ctx, "spacing_pct", …)`, 说明来自注释块里的 `--   spacing_pct   网格间距…`。
    /// 两边一旦有一天对不上(改名、改注释), 说明会整体丢光 —— 而那时**页面只是变裸键,
    /// 不会报错**, 属静默劣化, 所以需要一条测试钉住。
    #[test]
    fn builtin_paired_grid_futures_comment_docs_cover_read_params() {
        let code = include_str!("../../../../strategies/futures/paired_grid_futures_long.lua");
        let got = extract_param_hints(code);
        let s = got.iter().find(|h| h.key == "spacing_pct").expect("应提取到 spacing_pct");
        assert_eq!(
            s.desc.as_deref(),
            Some("网格间距(小数, 0.01=1%), 默认 0.01"),
            "内置脚本注释里的说明必须逐字带出(证明是搬走而不是加工)"
        );
        assert!(
            got.iter().any(|h| h.key == "start_price" && h.desc.is_some()),
            "注释块里的参数应带出说明"
        );
    }

    /// 三种形态都要认: 辅助函数 `num`/`cfg_str`、直接 `ctx:config_*`。
    #[test]
    fn extracts_three_call_shapes() {
        let code = r#"
local a = num(ctx, "spacing_pct", 0.01)
local b = cfg_str(ctx, "accumulate_mode", "u")
local c = ctx:config_i64("atr_period")
local d = ctx:config_bool("enable_build")
local e = ctx:config_f64("virtual_mult")
local pair = ctx:config_str("pair")
"#;
        let got = hints(code);
        let keys: Vec<&str> = got.iter().map(|h| h.0.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "spacing_pct",
                "accumulate_mode",
                "atr_period",
                "enable_build",
                "virtual_mult",
                "pair"
            ]
        );
        assert_eq!(got[0].1, ParamType::Float);
        assert_eq!(got[0].2, "f64:0.01");
        assert_eq!(got[1].1, ParamType::Str);
        assert_eq!(got[1].2, "str:u");
        assert_eq!(got[2].1, ParamType::Int);
        assert_eq!(got[2].2, "none", "无兜底默认 → 不填");
        assert_eq!(got[3].1, ParamType::Bool);
        assert_eq!(got[4].2, "none");
    }

    /// `num` 读到的是浮点: 整数字面量默认值要归一成浮点。
    #[test]
    fn num_coerces_integer_literal_default_to_float() {
        let got = hints(r#"local x = num(ctx, "order_amount", 10)"#);
        assert_eq!(got[0].1, ParamType::Float);
        assert_eq!(got[0].2, "f64:10");
    }

    /// 变量键不是事实 —— `num`/`cfg_str` 辅助函数体里的 `ctx:config_f64(key)` 必须被跳过,
    /// 否则每份策略都会多出一个假参数 `key`。
    #[test]
    fn variable_keys_are_not_extracted() {
        let code = r#"
local function num(ctx, key, dflt)
    local v = ctx:config_f64(key)
    if v == nil or v == 0 then return dflt end
    return v
end
local t = num(ctx, "real_param", 1)
"#;
        let got = hints(code);
        assert_eq!(got.len(), 1, "只应提到真实参数: {got:?}");
        assert_eq!(got[0].0, "real_param");
    }

    /// 注释与字符串里的同名文本不得命中。
    #[test]
    fn comments_and_strings_do_not_match() {
        let code = r#"
-- 示例: num(ctx, "commented_out", 1)
local s = "num(ctx, \"in_string\", 2)"
local doc = [[ num(ctx, "in_long_string", 3) ]]
local real = num(ctx, "only_this", 4)
"#;
        let got = hints(code);
        let keys: Vec<&str> = got.iter().map(|h| h.0.as_str()).collect();
        assert_eq!(keys, vec!["only_this"], "注释/字符串里的不得命中: {keys:?}");
    }

    /// 引擎内部键不进骨架(它们不是用户参数)。
    #[test]
    fn engine_keys_are_skipped() {
        let got = hints(r#"local p = ctx:config_str("script_path")"#);
        assert!(got.is_empty(), "script_path 不是用户参数: {got:?}");
    }

    /// 去重: 同一个键被读多次只出现一次(取首次出现顺序)。
    #[test]
    fn duplicate_keys_collapse_in_first_seen_order() {
        let code = r#"
local a = num(ctx, "b_key", 1)
local b = num(ctx, "a_key", 2)
local c = num(ctx, "b_key", 9)
"#;
        let got = hints(code);
        let keys: Vec<&str> = got.iter().map(|h| h.0.as_str()).collect();
        assert_eq!(keys, vec!["b_key", "a_key"]);
        assert_eq!(got[0].2, "f64:1", "保留首次出现的默认值");
    }

    /// 与内置清单的**端到端一致性**: 喂 `linear_position_grid.lua` 抽出的键集合必须与
    /// `linear_position_grid.toml` 声明的键集合**完全相同**。
    ///
    /// 默认值这一层**只比对两侧都有、且代码里是纯字面量兜底**的那些键; 一处**已知不一致**
    /// 的照实点名(见下), 不为了让断言好看去改任何一侧 —— 清单记的是"语义默认",
    /// 静态扫描只拿得到"字面量兜底":
    ///   · `interval`: 代码兜底 `"1h"`, 清单默认 `"1m"` —— 生效默认由
    ///     `resolve_builtin_script` 按清单回填, 代码兜底只在参数完全缺席时生效。
    #[test]
    fn linear_position_grid_keys_match_manifest_exactly() {
        let lua = include_str!("../../../../strategies/spot/linear_position_grid.lua");
        let toml_src = include_str!("../../../../strategies/spot/linear_position_grid.toml");

        let got = extract_param_hints(lua);
        let manifest = crate::strategies::catalog::StrategyManifest::parse(toml_src)
            .expect("内置清单应恒可解析");

        let mut got_keys: Vec<&str> = got.iter().map(|h| h.key.as_str()).collect();
        let mut want_keys: Vec<&str> = manifest.params.iter().map(|p| p.key.as_str()).collect();
        got_keys.sort_unstable();
        want_keys.sort_unstable();
        assert_eq!(got_keys, want_keys, "提取出的键集合必须与清单声明一致");

        // 默认值: 两侧都有且为字面量的逐项相等。
        let known_skew = ["interval"];
        for h in &got {
            if known_skew.contains(&h.key.as_str()) {
                continue;
            }
            let declared =
                manifest.params.iter().find(|p| p.key == h.key).and_then(|p| p.default.clone());
            match (&h.default, &declared) {
                (Some(a), Some(b)) => {
                    let same = match (a, b) {
                        (ConfigValue::String(x), ConfigValue::String(y)) => x == y,
                        _ => a.as_f64() == b.as_f64(),
                    };
                    assert!(same, "参数 {} 的默认值不一致: 提取 {a:?} vs 清单 {b:?}", h.key);
                }
                (None, None) => {}
                // 一侧没有字面量兜底(required / 语义默认) → 不在此断言范围。
                _ => {}
            }
        }

        // 已知偏差照实钉住, 免得以后被"顺手对齐"掉。
        let iv = got.iter().find(|h| h.key == "interval").unwrap();
        assert_eq!(
            show(&iv.default),
            "str:1h",
            "interval 的**字面量**兜底是 1h(清单默认 1m 是生效值, 由 resolve_builtin_script 回填)"
        );
    }

    /// 060 smart_position: 同款端到端一致性。`interval` 两侧都是 `1h`(时间闸才是频率开关,
    /// 清单与代码兜底无偏差), 故无需 known_skew 豁免。
    #[test]
    fn smart_position_keys_match_manifest_exactly() {
        let lua = include_str!("../../../../strategies/spot/smart_position.lua");
        let toml_src = include_str!("../../../../strategies/spot/smart_position.toml");

        let got = extract_param_hints(lua);
        let manifest = crate::strategies::catalog::StrategyManifest::parse(toml_src)
            .expect("内置清单应恒可解析");

        let mut got_keys: Vec<&str> = got.iter().map(|h| h.key.as_str()).collect();
        let mut want_keys: Vec<&str> = manifest.params.iter().map(|p| p.key.as_str()).collect();
        got_keys.sort_unstable();
        want_keys.sort_unstable();
        assert_eq!(got_keys, want_keys, "提取出的键集合必须与清单声明一致");

        for h in &got {
            let declared =
                manifest.params.iter().find(|p| p.key == h.key).and_then(|p| p.default.clone());
            // 一侧没有字面量兜底(required / nil 检查直读) → 不在此断言范围。
            if let (Some(a), Some(b)) = (&h.default, &declared) {
                let same = match (a, b) {
                    (ConfigValue::String(x), ConfigValue::String(y)) => x == y,
                    _ => a.as_f64() == b.as_f64(),
                };
                assert!(same, "参数 {} 的默认值不一致: 提取 {a:?} vs 清单 {b:?}", h.key);
            }
        }
    }
}
