//! 032 US3 策略源码读取与页面保存端点 (FR-015 ~ FR-017 / FR-020 / FR-028):
//!
//! - `GET /api/strategies/{id}/source` — `{ id, market, lua, instance_toml }`;
//!   `lua` 为文件原文, 内置策略 `instance_toml = null`;
//! - `POST /api/strategies` — 页面直控新建/复制/覆盖保存: 命名校验链(名格式 → 前缀冲突 →
//!   内置保留名 → 运行中拒绝 → 同名 overwrite 判定)→ 编译门禁(`ricow_engine::create_strategy`,
//!   内含 mlua 编译)→ 复用引擎同一落盘内核 [`ricow_engine::write_strategy_files`]。
//!
//! FR-028: 用户亲手新建/编辑的策略**免去沙箱回测 + preview + approve 前置链**(用户本人即作者
//! 与确认者), 但编译门禁不豁免; 终端/对话渠道一行不改(仍走 `execute_strategy` 消费一次性 token)。
//!
//! 不碰磁盘的判定全部抽成纯函数(见下), 单测覆盖名字/保留名/前缀/overwrite/参数组装/行号解析。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use axum::extract::{rejection::JsonRejection, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::ConfigValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::strategies::catalog::{self, ManifestParam, ParamType, Source, StrategyManifest};

use super::{WebError, WebState};

// ---- 响应/请求体 (data-model §4/§5, http-api §3) ----

/// `GET .../source` 响应: Lua 文件原文 + 用户实例 TOML 原文(内置为 null)。
#[derive(Debug, Serialize)]
pub(super) struct SourceReply {
    id: String,
    market: String,
    lua: String,
    instance_toml: Option<String>,
}

/// `POST /api/strategies` 请求体(字段严格按 data-model §5 契约: name/overwrite, 不是 id/allow_replace)。
#[derive(Debug, Deserialize)]
pub(super) struct SaveRequest {
    /// 策略名(也是落盘文件名): [A-Za-z0-9_-], 1..=24 字符。
    name: String,
    /// 市场: spot | futures。
    market: String,
    /// 交易对(如 ETHUSDT)。
    pair: String,
    /// Lua 源码(允许带 ```lua 围栏 —— 引擎 `create_strategy` 会剥围栏)。
    code: String,
    /// 策略参数覆盖值: 数字 / 字符串 / 布尔; 键来自清单, 本层不解读参数含义。
    #[serde(default)]
    params: HashMap<String, Value>,
    /// 覆盖已存在的同名用户策略: false(缺省)= 同名 409; true = 受控覆盖(先备份)。
    #[serde(default)]
    overwrite: bool,
    /// 052 方案 1: **复制/模板新建**时的源策略 id(内置或用户)。
    ///
    /// 服务端据此把源清单(中文名/说明/参数 schema)**一并落盘**到
    /// `strategies/{market}/{name}.toml`, 让副本一打开就带完整参数说明 ——
    /// 此前复制只搬 Lua、不搬清单, 打开详情页「参数」卡片只剩一个「交易对」输入框。
    ///
    /// 清单的**唯一真相留在后端**(前端只传 id, 不搬运一份可能失真的副本)。
    /// 与 `overwrite: true` 互斥: 复制是"新建"语义, 覆盖是"保存"语义。
    #[serde(default)]
    copy_from: Option<String>,
}

/// `POST /api/strategies` 成功响应(http-api §3: name/toml_path/lua_path/backup?; 另带 market)。
#[derive(Debug, Serialize)]
pub(super) struct SaveReply {
    name: String,
    market: String,
    toml_path: String,
    lua_path: String,
    /// 受控覆盖时的首个备份路径(http-api §3 的 `backup?`); 全新部署时该字段**缺省**(不出现)。
    #[serde(skip_serializing_if = "Option::is_none")]
    backup: Option<String>,
    /// 052 方案 1: 随复制一并落盘的参数清单路径; 非复制路径缺省(不出现)。
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_path: Option<String>,
    /// 静态检查提示(P1-5): 只提示不拦截, 空则字段缺省不出现。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

// ---- 纯函数: 参数 / 市场 / 行号 / 保存判定(不碰磁盘, 不 .await) ----

/// 市场白名单: 容忍前后空白与大小写(与 `commands::create` / markets 端点同口径)。
pub(super) fn parse_market(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "spot" => Ok("spot"),
        "futures" => Ok("futures"),
        other => Err(format!("market 仅支持 spot|futures, 收到 '{other}'")),
    }
}

/// 把请求里的 JSON 参数值转成引擎 [`ConfigValue`](纯函数, 便于单测):
/// 布尔 / 整数 / 浮点 / 字符串放行; null、对象、数组一律 400(参数键是数据, 但值必须可落 TOML)。
///
/// 保存端点与回测端点(data-model §7 params 覆盖)共用同一份转换, 避免两处口径漂移。
pub(super) fn config_values(
    params: &HashMap<String, Value>,
) -> Result<HashMap<String, ConfigValue>, String> {
    let mut out = HashMap::with_capacity(params.len());
    for (key, value) in params {
        let cv = match value {
            Value::Bool(b) => ConfigValue::Boolean(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    ConfigValue::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    ConfigValue::Float(f)
                } else {
                    return Err(format!("参数 {key} 的数值超出可表示范围: {n}"));
                }
            }
            Value::String(s) => ConfigValue::String(s.clone()),
            Value::Null => {
                return Err(format!("参数 {key} 不能为 null: 请给具体的数值/字符串/布尔值"));
            }
            other => {
                return Err(format!("参数 {key} 的值类型不支持(仅支持数字/字符串/布尔): {other}"));
            }
        };
        out.insert(key.clone(), cv);
    }
    Ok(out)
}

/// 从 mlua 编译错误原文里抠行号(尽量定位, data-model §5): mlua 形如
/// `脚本编译错误:\n[string "chunk"]:12: <原因>` —— 取最后一个 `:数字:` 片段。
fn parse_lua_line(err: &str) -> Option<u32> {
    let bytes = err.as_bytes();
    let mut found = None;
    for i in 0..bytes.len() {
        if bytes[i] != b':' {
            continue;
        }
        let mut j = i + 1;
        let mut n: u32 = 0;
        let mut any = false;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            n = n.checked_mul(10)?.checked_add((bytes[j] - b'0') as u32)?;
            any = true;
            j += 1;
        }
        if any && j < bytes.len() && bytes[j] == b':' {
            found = Some(n);
        }
    }
    found
}

/// 保存校验链的拒绝结论(纯函数载体): HTTP 状态 + 机器码 + 中文文案。
#[derive(Debug, PartialEq, Eq)]
struct Reject {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl Reject {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).expect("拒绝状态码必须合法"),
            code,
            message: message.into(),
        }
    }

    fn into_web_error(self) -> WebError {
        if self.status == StatusCode::CONFLICT {
            WebError::conflict(self.message, self.code)
        } else {
            WebError::bad_request_code(self.message, self.code)
        }
    }
}

/// 保存前置判定(纯函数, data-model §5 顺序):
///
/// 1. 名格式(`validate_strategy_name`, 兼防路径穿越)→ 400 `invalid_name`;
/// 2. **仅新名**查前缀冲突(`validate_new_name`, 与 CLI create 同一实现/文案)→ 409 `prefix`
///    (覆盖保存时名字自身必然与自己"互为前缀", 必须跳过此查);
/// 3. 内置保留名(单一清单 `catalog::is_builtin_id`, 不另造列表)→ 409 `reserved`;
/// 4. 同名实例正在运行 → 409 `running`(FR-020: 先停止再保存);
/// 5. 已存在 + `overwrite=false` → 409 `exists`; 已存在 + `overwrite=true` → 受控覆盖。
///
/// 返回落盘内核要用的 `allow_replace`(新名 false / 受控覆盖 true)。
fn decide_allow_replace(
    name: &str,
    existing: &[String],
    exists: bool,
    overwrite: bool,
    running: bool,
) -> Result<bool, Reject> {
    // ① 名格式。
    ricow_strategy::validate_strategy_name(name)
        .map_err(|e| Reject::new(400, "invalid_name", e))?;

    // ② 前缀冲突(仅新名; 名字格式已由①拦过, 此处 Err 只可能是前缀冲突)。
    if !exists {
        if let Err(e) = crate::commands::create::validate_new_name(name, existing) {
            return Err(Reject::new(409, "prefix", e.to_string()));
        }
    }

    // ③ 内置保留名: 覆盖也不行(内置标识永不允许用户占用/改写)。
    if catalog::is_builtin_id(name) {
        return Err(Reject::new(
            409,
            "reserved",
            format!(
                "策略名 '{name}' 是内置策略保留标识, 用户策略不得占用 (FR-016): 请换一个新名字"
            ),
        ));
    }

    // ④ 运行中实例禁止保存覆盖 (FR-020)。
    if running {
        return Err(Reject::new(
            409,
            "running",
            format!("策略 '{name}' 正在运行: 请先停止再保存, 运行中禁止覆盖 (FR-020)"),
        ));
    }

    // ⑤ 同名 + overwrite 判定。
    if exists {
        if !overwrite {
            return Err(Reject::new(
                409,
                "exists",
                format!(
                    "同名策略 '{name}' 已存在且本次保存未要求覆盖(overwrite=false): 拒绝静默覆盖; \
                     确要保存修改请带 overwrite=true(覆盖前会自动备份旧文件)"
                ),
            ));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// 同名文件是否已存在 —— 与引擎落盘内核的判定逐字一致:
/// 实例 TOML 在 `strategies/<name>.toml`, 脚本在 `strategies/{market}/<name>.lua`。
fn strategy_files_exist(dir: &Path, market: &str, name: &str) -> bool {
    dir.join(format!("{name}.toml")).exists()
        || dir.join(market).join(format!("{name}.lua")).exists()
}

/// 读用户实例 TOML 原文 `strategies/<id>.toml`; 文件不存在 → Ok(None)(如实给 null),
/// 其他 IO 错误 → 500。调用前 id 已过名字规范校验(无路径分隔/`.`), 不会目录穿越。
fn read_instance_toml(root: &Path, id: &str) -> CoreResult<Option<String>> {
    let path = root.join("strategies").join(format!("{id}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::Exchange(format!("读实例 TOML {} 失败: {e}", path.display()))),
    }
}

/// 详情表单回填用的实例当前值(032 T027): 交易对 + 已保存的策略参数。
///
/// 前端不引 TOML 解析库, 由本端点把 `strategies/<id>.toml` 反序列化后以 JSON 给出;
/// `pair` 与引擎内部键(`script`/`script_path`)不进参数表单, 单独/剔除处理。
#[derive(Debug, Serialize)]
pub(super) struct InstanceValues {
    /// 实例当前交易对; TOML 未写 pair(异常旧文件)时为 None。
    pub(super) pair: Option<String>,
    /// 当前参数值(键 → 数字/字符串/布尔; 已剔除 pair/script/script_path 内部键)。
    pub(super) current: HashMap<String, ConfigValue>,
}

/// 读并**轻量**反序列化用户实例 TOML(只用 `StrategyConfig::from_toml`, 不注入 script 文件内容、
/// 不校验 enabled —— 详情只读展示, 不应有读 lua 文件之类的副作用):
/// 内置策略 / 实例文件不存在 → Ok(None)(前端回退清单默认值)。
pub(super) fn load_instance_values(root: &Path, id: &str) -> CoreResult<Option<InstanceValues>> {
    let Some(text) = read_instance_toml(root, id)? else {
        return Ok(None);
    };
    let config = ricow_strategy::StrategyConfig::from_toml(&text)
        .map_err(|e| CoreError::Parse(format!("实例 TOML 解析失败: {e}")))?;
    let mut params = config.params;
    // 内部键不进"当前参数": pair 单独回; script/script_path 是引擎装载细节。
    let pair = match params.remove("pair") {
        Some(ConfigValue::String(p)) => Some(p),
        _ => None,
    };
    params.remove("script");
    params.remove("script_path");
    Ok(Some(InstanceValues { pair, current: params }))
}

/// 把清单的参数 schema 拼成给 LLM 的简短中文说明(纯函数, 032 FR-018):
/// 每项给「中文名 / 键名 / 类型 / 是否必填 / 说明 / 枚举或默认值」。参数键全部来自 manifest
/// 数据(catalog 铁律: 表现层不硬编码参数名), 摘要只帮助模型理解参数语义, 不进引擎。
fn manifest_summary(m: &StrategyManifest) -> String {
    let mut out = format!(
        "策略: {}(id={}, 适用市场 {})\n一句话说明: {}\n参数清单:",
        m.name, m.id, m.market, m.summary
    );
    if m.params.is_empty() {
        out.push_str(" 无声明参数");
        return out;
    }
    for p in &m.params {
        out.push_str(&format!("\n- {}", one_param_line(p)));
    }
    out
}

/// 单个参数的一行说明(便于单测断言)。
fn one_param_line(p: &ManifestParam) -> String {
    let ty = match p.ty {
        ParamType::Float => "f64(数值)",
        ParamType::Int => "i64(整数)",
        ParamType::Str => "string(字符串)",
        ParamType::Bool => "bool(布尔)",
        ParamType::Enum => "enum(枚举)",
    };
    // N2 (2026-10-11): 中文名/说明留空是常态(从脚本识别出来的参数就是这样) —— 名字回退显示
    // 键名, 说明位**如实**写"清单未填说明", 好让模型知道这里没有作者写的语义可依据,
    // 而不是看到一片空白自己去猜(猜出来的东西会混进"风险说明"里, 那是编造作者意图)。
    let name = if p.name.trim().is_empty() { p.key.as_str() } else { p.name.as_str() };
    let desc = if p.desc.trim().is_empty() { "(清单未填说明)" } else { p.desc.as_str() };
    let mut line = format!(
        "{} (键名 {}, 类型 {}{}): {}",
        name,
        p.key,
        ty,
        if p.required { ", 必填" } else { "" },
        desc
    );
    if let Some(opts) = &p.options {
        if !opts.is_empty() {
            line.push_str(&format!("; 可选值: {}", opts.join("/")));
        }
    }
    if let Some(default) = &p.default {
        if let Ok(j) = serde_json::to_string(default) {
            line.push_str(&format!("; 默认: {j}"));
        }
    }
    line
}

// ---- 静态检查 (P1-5): 保存 / AI 产物共用, 只提示不拦截 ----

/// 单词边界包含判定(避免 "cross." 误中 "os." 这类子串假阳性)。
fn has_word(s: &str, pat: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = s[from..].find(pat) {
        let start = from + pos;
        let before_ok = start == 0
            || !s[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if before_ok {
            return true;
        }
        from = start + pat.len().max(1);
    }
    false
}

/// 静态检查(纯函数, P1-5): 编译门禁只保证**语法**可编译, 这里把已知的高频踩坑
/// (specs/lua-api.md 实测记录)在保存/生成时提示给作者 —— **只提示不拦截**:
/// 每条都可能是误报, 拦截权在用户手里。最多回 8 条, 防止刷屏。
pub(super) fn lint_lua(code: &str) -> Vec<String> {
    let mut warns: Vec<String> = Vec::new();
    for (idx, raw) in code.lines().enumerate() {
        if warns.len() >= 8 {
            break;
        }
        let line = raw.trim();
        let n = idx + 1;
        // ① on_fill 成交字段误用(规范 §一: 字段是 fill_price/fill_size, 实测有人打成 price/size)。
        if line.contains("fill.price") {
            warns.push(format!("第 {n} 行: fill.price 不存在(读到 nil), 应为 fill_price"));
        }
        if line.contains("fill.size") && !line.contains("fill_size") {
            warns.push(format!("第 {n} 行: fill.size 不存在(读到 nil), 应为 fill_size"));
        }
        // ② Lua or 陷阱(规范 §二: config_f64/i64 未配置时返回 0 而非 nil, `or 缺省` 永远取不到)。
        if (line.contains("config_f64(") || line.contains("config_i64(")) && line.contains(" or ") {
            warns.push(format!(
                "第 {n} 行: config_f64/config_i64 未配置时返回 0, `... or 缺省` 取不到缺省 —— 请用 num/cfg_str 辅助函数"
            ));
        }
        // ③ 沙箱禁用库/函数(编译只查语法, 这些到运行期才炸)。
        if ["os.", "io.", "require(", "loadstring", "loadfile", "dofile", "pcall("]
            .iter()
            .any(|w| has_word(line, w))
        {
            warns.push(format!(
                "第 {n} 行: 沙箱禁用 os/io/require/pcall 等能力(规范 §七), 运行期会报错"
            ));
        }
        // ④ 订单表字段误用: type = "limit"/"market" 应为 order_type(规范 §三)。
        if (line.contains("\"limit\"") || line.contains("\"market\""))
            && line.contains("type =")
            && !line.contains("order_type =")
        {
            warns.push(format!("第 {n} 行: 订单表字段应为 order_type(不是 type)"));
        }
        // ⑤ 潜在死循环: 沙箱有指令预算, 但撞预算即中断本 tick —— 提示检查退出条件。
        if line.contains("while true") && !line.contains("break") {
            warns.push(format!("第 {n} 行: while true 需保证循环体里有退出条件(指令预算会中断)"));
        }
    }
    warns
}

// ---- 用户策略清单 (manifest) 编辑 (P2-8) ----
//
// 动机: 参数表单/`current` 回填都读清单 (catalog), 而用户自写策略默认没有清单 (合成最小清单,
// params 为空) —— 参数与代码两张皮, 用户无法在 UI 里声明"这个策略有哪些参数、什么类型、默认值"。
// 本端点让用户为自己的策略维护一份清单, 落盘位置与内置清单**同构同目录**
// (`strategies/{market}/{id}.toml`), 从而被 catalog 扫描复用、参数表单随之点亮。

/// 引擎内部参数键: 由 loader 装载脚本使用, 清单不得声明(否则会与装载细节冲突)。
const RESERVED_PARAM_KEYS: &[&str] = &["script", "script_path"];

/// `POST /api/strategies/{id}/manifest` 请求体。`id`/`market` 由服务端从策略本身取, 不接受前端覆盖
/// —— 二者必须与落盘文件名/目录一致 (catalog 扫描期 `check_dir` 强校验)。
#[derive(Debug, Deserialize)]
pub(super) struct ManifestRequest {
    /// 中文名(必填)。
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    suitable: String,
    #[serde(default)]
    unsuitable: String,
    /// 参数 schema(声明顺序即表单顺序)。
    #[serde(default)]
    params: Vec<ManifestParam>,
}

/// `POST .../manifest` 成功响应: 落盘后的清单 + 覆盖时的备份路径(缺省不出现)。
#[derive(Debug, Serialize)]
pub(super) struct ManifestReply {
    #[serde(flatten)]
    manifest: StrategyManifest,
    #[serde(skip_serializing_if = "Option::is_none")]
    backup: Option<String>,
}

/// 参数键规范: 以字母/下划线开头, 其余为字母/数字/下划线 (与 Lua `config_*` 字符串键一致)。
fn valid_param_key(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 参数类型的中文/代码名(错误信息用)。
fn ty_name(ty: ParamType) -> &'static str {
    match ty {
        ParamType::Float => "f64",
        ParamType::Int => "i64",
        ParamType::Str => "string",
        ParamType::Bool => "bool",
        ParamType::Enum => "enum",
    }
}

/// 空串 → None(可选字段归一: 前端留空即"不写该字段")。
fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// 默认值按声明类型归一 (纯函数): JSON 数字是无类型的, `serde` 会把 `14` 解成 `Float(14.0)`,
/// 直接落盘会写成 `default = 14.0`(与内置清单的 `default = 14` 风格不一致)。这里:
/// - f64 → `Float`(整数也接受, 转成浮点);
/// - i64 → `Integer`, **拒绝非整数值**(`1.5` 会被 `as_i64` 静默截断成 1, 必须当场拦);
/// - string/enum → `String`; bool → `Boolean`。
///
/// 类型不符 → 中文原因(带键名)。
fn coerce_default(
    key: &str,
    ty: ParamType,
    d: Option<ConfigValue>,
) -> Result<Option<ConfigValue>, String> {
    let Some(d) = d else { return Ok(None) };
    let bad = |hint: &str| -> String {
        format!("参数 '{key}' 的默认值类型与 type={} 不一致: 需为{hint}", ty_name(ty))
    };
    let out = match ty {
        ParamType::Float => match d {
            ConfigValue::Float(f) => ConfigValue::Float(f),
            ConfigValue::Integer(i) => ConfigValue::Float(i as f64),
            _ => return Err(bad("数值")),
        },
        ParamType::Int => match d {
            ConfigValue::Integer(i) => ConfigValue::Integer(i),
            ConfigValue::Float(f) if f.is_finite() && f.fract() == 0.0 => {
                ConfigValue::Integer(f as i64)
            }
            _ => return Err(bad("整数")),
        },
        ParamType::Str | ParamType::Enum => match d {
            ConfigValue::String(s) => ConfigValue::String(s),
            _ => return Err(bad("字符串")),
        },
        ParamType::Bool => match d {
            ConfigValue::Boolean(b) => ConfigValue::Boolean(b),
            _ => return Err(bad("布尔值")),
        },
    };
    Ok(Some(out))
}

/// 校验并构造清单 (纯函数, 便于单测): id/market 由调用方(策略本身)注入, 前端只能改展示字段与
/// 参数 schema。任一非法 → 中文错误串 (由 handler 转 400, 不落盘)。
pub(super) fn build_manifest(
    id: &str,
    market: &str,
    req: ManifestRequest,
) -> Result<StrategyManifest, String> {
    // N2 (2026-10-11): 策略中文名**允许留空** —— 用户是脚本的使用者不是作者, 不该被逼着起名。
    // 界面在空名时回退显示 id。这里不再拦截, 否则"识别参数后一键保存"永远 400。
    let name = req.name.trim();
    let mut seen: HashSet<String> = HashSet::new();
    let mut params: Vec<ManifestParam> = Vec::with_capacity(req.params.len());
    for p in req.params {
        let key = p.key.trim().to_string();
        if !valid_param_key(&key) {
            return Err(format!(
                "参数键 '{key}' 非法: 需以字母/下划线开头, 只含字母/数字/下划线(与 Lua config_* 读取键一致)"
            ));
        }
        if RESERVED_PARAM_KEYS.contains(&key.as_str()) {
            return Err(format!("参数键 '{key}' 是引擎内部保留键, 不能声明"));
        }
        if !seen.insert(key.clone()) {
            return Err(format!("参数键 '{key}' 重复声明"));
        }
        // N2 (2026-10-11): 参数中文名与说明**都允许留空**。静态提取出来的参数本就没有人给它
        // 起名/写说明, 留空是常态而非错误; 界面回退显示键名。真正需要"有说明"的地方(AI 讲风险)
        // 由调用侧如实标注"依据不足", 而不是靠这里拦着不让保存。
        let pname = p.name.trim();
        let desc = p.desc.trim();
        if p.ty == ParamType::Enum && p.options.as_ref().is_none_or(|o| o.is_empty()) {
            return Err(format!("参数 '{key}' 是 enum, 必须给出 options(可选值列表)"));
        }
        // 默认值按类型归一(JSON 的无类型数字需落到具体 ConfigValue 变体, 才能写出干净的 TOML)。
        let default = coerce_default(&key, p.ty, p.default)?;
        if p.ty == ParamType::Enum {
            if let Some(o) = p.options.as_ref() {
                if let Some(d) = &default {
                    let dv = d.as_str().unwrap_or_default();
                    if !o.iter().any(|x| x == dv) {
                        return Err(format!("参数 '{key}' 的默认值 '{dv}' 不在 options 里"));
                    }
                }
            }
        }
        params.push(ManifestParam {
            key,
            name: pname.to_string(),
            ty: p.ty,
            desc: desc.to_string(),
            default,
            required: p.required,
            options: p.options,
        });
    }
    Ok(StrategyManifest {
        id: id.to_string(),
        name: name.to_string(),
        market: market.to_string(),
        summary: req.summary.trim().to_string(),
        description: non_empty(req.description),
        suitable: non_empty(req.suitable),
        unsuitable: non_empty(req.unsuitable),
        params,
        position_mode: None,
        default_leverage: None,
        backtest: None,
    })
}

/// 覆盖前备份已有清单为同目录 `<id>.toml.<ts>.bak`。失败即中止(不拿用户资产冒险)。
/// 备份文件扩展名是 `.bak`, 不会被 catalog 的 `.toml` 扫描误当清单。
fn backup_manifest(path: &Path) -> Result<std::path::PathBuf, WebError> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("manifest.toml");
    let mut dst = dir.join(format!("{file}.{ts}.bak"));
    let mut n = 1u32;
    while dst.exists() {
        dst = dir.join(format!("{file}.{ts}-{n}.bak"));
        n += 1;
    }
    std::fs::copy(path, &dst).map_err(|e| {
        WebError::from(CoreError::Exchange(format!(
            "备份旧清单失败({} → {}): {e}; 已中止, 旧清单保持原样",
            path.display(),
            dst.display()
        )))
    })?;
    Ok(dst)
}

/// 052 方案 1: 从源清单**派生副本清单**(纯函数, 便于单测)。
///
/// - `id` / `market` 必须改写: 前者要等于落盘文件名(`catalog` 扫描期按 id 注册), 后者要等于
///   所在市场目录(`manifest.check_dir` 会强校验, 不一致整条被跳过);
/// - 其余(中文名 / 说明 / 适用性 / 参数 schema / 持仓模式 / 默认杠杆 / `[backtest]`)**逐字照搬**
///   —— 副本与源**是同一个策略**, 参数说明本就由这份清单承载; 用户想改名可在清单编辑器里改;
/// - `pair` 这类保留键**照搬**: 内置清单本就声明了它(给"交易对"字段配中文名与必填),
///   表单侧自会跳过(`RESERVED_PARAM_KEYS`)。
pub(super) fn derive_manifest(
    src: &StrategyManifest,
    new_id: &str,
    market: &str,
) -> StrategyManifest {
    let mut m = src.clone();
    m.id = new_id.to_string();
    m.market = market.to_string();
    m
}

/// 写清单文件(已存在则先备份)。返回备份路径(原先不存在 → `None`)。
fn write_manifest_file(
    path: &Path,
    toml_str: &str,
) -> Result<Option<std::path::PathBuf>, WebError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            WebError::from(CoreError::Exchange(format!(
                "创建策略目录 {} 失败: {e}",
                parent.display()
            )))
        })?;
    }
    let backup = if path.exists() { Some(backup_manifest(path)?) } else { None };
    std::fs::write(path, toml_str).map_err(|e| {
        WebError::from(CoreError::Exchange(format!("写清单 {} 失败: {e}", path.display())))
    })?;
    Ok(backup)
}

/// 回滚刚写的清单: 有备份则恢复原样, 否则删除 —— 与 `.lua`/`.toml` 的"不留半成品"同口径。
///
/// 刻意**不返回错误**: 调用它的时候主线已经失败了, 这里再抛只会盖掉真正的失败原因;
/// 回滚不成功最坏留下一份"有清单没脚本"的孤儿清单, catalog 扫描会把它记为问题并跳过,
/// 不会污染任何在注册的策略。
fn rollback_manifest(path: &Path, backup: Option<&Path>) {
    match backup {
        Some(b) => {
            let _ = std::fs::copy(b, path);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `POST /api/strategies/{id}/manifest`(P2-8): 保存/更新**用户策略**的清单。
///
/// 状态码: 非法/未知 id → 404(与 source 同口径); 内置策略 → 409 `builtin`(其清单随代码发布,
/// 不可页面编辑); 字段非法 → 400 中文原因。成功 → 回落盘清单(前端可直接刷新表单)。
pub(super) async fn save_strategy_manifest(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<ManifestReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: ManifestRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!("请求体字段非法: 需要 name 字符串与 params 数组: {e}"))
    })?;

    if ricow_strategy::validate_strategy_name(&id).is_err() {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }
    let entry = catalog::find(&id).ok_or_else(|| WebError::not_found(format!("没有策略 {id}")))?;
    if entry.source == Source::Builtin {
        return Err(WebError::conflict(
            format!(
                "'{id}' 是内置策略, 清单随代码发布, 不能通过页面编辑; \
                 请先「复制」为你自己的策略, 再编辑它的清单"
            ),
            "builtin",
        ));
    }
    let market = entry.manifest.market.clone();
    let manifest = build_manifest(&id, &market, req).map_err(WebError::bad_request)?;

    // 落盘: strategies/{market}/{id}.toml (与内置清单同目录同构)。
    let dir = crate::commands::ensure_strategies_dir_in(&state.root)?;
    let path = dir.join(&market).join(format!("{id}.toml"));
    let toml_str = toml::to_string_pretty(&manifest)
        .map_err(|e| WebError::bad_request(format!("清单序列化失败: {e}")))?;
    let backup = write_manifest_file(&path, &toml_str)?;

    Ok(Json(ManifestReply { manifest, backup: backup.map(|p| p.display().to_string()) }))
}

// ---- 052 方案 2: 从 Lua 源码静态提取参数骨架 (无状态端点) ----

/// `POST /api/strategies/extract-params` 请求体: 待分析的 Lua 源码。
#[derive(Debug, Deserialize)]
pub(super) struct ExtractParamsRequest {
    code: String,
}

/// 成功响应: 提取出的参数骨架(键 / 类型 / 字面量兜底默认)。
#[derive(Debug, Serialize)]
pub(super) struct ExtractParamsReply {
    params: Vec<crate::strategies::params::ParamHint>,
}

/// `POST /api/strategies/extract-params`(052 方案 2): **无状态**的静态扫描 ——
/// 只吃请求体里的代码, 不读盘、不落盘、不碰 AI。
///
/// 无状态是刻意设计: 详情页的 Lua 编辑器里可能是**尚未保存的草稿**, 而"这份脚本读了哪些参数"
/// 是代码本身的属性 —— 要求用户先保存再识别是多余的一步。
///
/// 状态码: 代码为空 → 400; 成功 → 200。**提不出参数不是错误**(空数组) ——
/// "这份脚本不读任何参数"本身就是个可读的结论。
pub(super) async fn extract_params(
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<ExtractParamsReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: ExtractParamsRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!("请求体字段非法: 需要 code 字符串(Lua 源码): {e}"))
    })?;
    if req.code.trim().is_empty() {
        return Err(WebError::bad_request("code(Lua 源码)不能为空: 没有代码就无从识别参数"));
    }
    let params = crate::strategies::params::extract_param_hints(&req.code);
    Ok(Json(ExtractParamsReply { params }))
}

// ---- N2-C: 让 AI 给参数起中文名与写说明 ----

/// `POST /api/strategies/ai-name-params` 请求 —— 一段 Lua 源码(通常是编辑器里的草稿)。
#[derive(Debug, Deserialize)]
pub(super) struct AiNamesRequest {
    code: String,
}

/// 一个键的命名结果。
#[derive(Debug, Serialize)]
pub(super) struct NamedParam {
    key: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    desc: Option<String>,
}

/// 成功响应: 与 `hints` **同序**的命名结果(没被命名的键不出现)。
#[derive(Debug, Serialize)]
pub(super) struct AiNamesReply {
    names: Vec<NamedParam>,
}

/// `POST /api/strategies/ai-name-params` (N2-C, 2026-10-12): 让 AI 给脚本里读到的参数键
/// 起中文名并写一句说明。**无状态**: 吃请求体里的代码, 不读盘不落盘(编辑器里可能是草稿)。
///
/// 与 `ai-edit?mode=params` 的分工: 那个改**取值**, 这个补**名字与说明**; 两者都不写盘,
/// 用户在清单编辑器里核对后才保存 —— 所以 AI 起的名**没有**绕过用户确认。
///
/// 键集合由**静态提取**决定(与"从脚本识别参数"同一来源), 不让模型自己列: 模型列的键可能多
/// (编出新键)可能少(漏掉偏僻读法), 而静态提取是代码事实。
pub(super) async fn ai_name_params(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<AiNamesReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: AiNamesRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!("请求体字段非法: 需要 code 字符串(Lua 源码): {e}"))
    })?;
    if req.code.trim().is_empty() {
        return Err(WebError::bad_request("code(Lua 源码)不能为空: 没有代码就无从起名"));
    }
    let hints = crate::strategies::params::extract_param_hints(&req.code);
    if hints.is_empty() {
        return Err(WebError::bad_request_code(
            "这份脚本没有读到任何参数, 没有可命名的键: 先确认它用 num/cfg_str/ctx:config_* \
             读取了参数",
            "no_params",
        ));
    }
    let key_lines: String =
        hints.iter().map(|h| format!("- {}", h.key)).collect::<Vec<_>>().join("\n");
    let replied = crate::ai::provider::quick_names(
        &state.root,
        &crate::ai::provider::build_names_prompt(&key_lines, &req.code),
    )
    .await
    .map_err(map_ai_error)?;

    let names = parse_names(&replied, &hints)?;
    Ok(Json(AiNamesReply { names }))
}

/// 解析 AI 回的命名 JSON, 并**只保留静态提取到的那些键**(与 `keep_manifest_params` 同一纪律:
/// 模型多给的键一律丢弃, 少给的键就是没命名 —— 不用空名占位, 空名等于没命名)。
///
/// 宽容两种回包形状: `{"key": {"name": "…", "desc": "…"}}` 与 `{"key": "中文名"}`(无说明)。
pub(super) fn parse_names(
    raw: &str,
    hints: &[crate::strategies::params::ParamHint],
) -> Result<Vec<NamedParam>, WebError> {
    let json = extract_json_object(raw).ok_or_else(|| {
        WebError::bad_request_code(
            "AI 未返回可识别的 JSON 对象: 应只输出一个 {键: {name, desc}} 对象(无解释、无围栏)。\
             请重试",
            "names_parse",
        )
    })?;
    let map: HashMap<String, Value> = serde_json::from_str(&json).map_err(|e| {
        WebError::bad_request_code(format!("AI 返回的 JSON 无法解析({e}): 请重试"), "names_parse")
    })?;

    let mut out: Vec<NamedParam> = Vec::new();
    for h in hints {
        let Some(v) = map.get(&h.key) else { continue };
        let (name, desc) = match v {
            Value::String(s) => (s.trim().to_string(), String::new()),
            Value::Object(o) => (
                o.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                o.get("desc").and_then(Value::as_str).unwrap_or("").trim().to_string(),
            ),
            _ => continue,
        };
        if name.is_empty() {
            continue;
        }
        out.push(NamedParam {
            key: h.key.clone(),
            name,
            desc: if desc.is_empty() { None } else { Some(desc) },
        });
    }
    if out.is_empty() {
        return Err(WebError::bad_request_code(
            "AI 没有给出任何可用命名: 换一种方式重试, 或自己填中文名(留空也能保存)",
            "names_empty",
        ));
    }
    Ok(out)
}

// ---- N1: 删除用户策略(2026-10-12) ----

/// `DELETE /api/strategies/{id}` 的成功回包。
#[derive(Debug, Serialize)]
pub(super) struct DeleteReply {
    deleted: bool,
    /// 实际删掉的文件(相对 root 的路径)。**如实列出** —— 前端拿它告诉用户究竟删了哪几个。
    removed: Vec<String>,
}

/// `DELETE /api/strategies/{id}` (N1, 2026-10-12): 删除一个**用户**策略。
///
/// 删的是**策略本体**:
/// - 实例 TOML `strategies/{id}.toml`
/// - 脚本     `strategies/{market}/{id}.lua`
/// - 清单     `strategies/{market}/{id}.toml`(可选 —— 裸脚本没有)
/// - 同目录下该 id 的 `*.bak`(覆盖保存留下的备份)
///
/// **不删**: `run/backtest/*-{id}-*.json` 回测历史与 `ricow.db` 台账。
/// 理由: 删一个策略不该顺手抹掉它的历史 —— 那会让总收益这类统计跟着变。
/// 前端确认框会**如实写明**这一点, 不让人以为删干净了。
///
/// 拒绝口径与保存同源: 内置保留名 → 409 `reserved`; 实例正在运行 → 409 `running`
/// (先停再删, 与"先停再保存"同一条 FR-020 精神); 名字非法或查无此策略 → 404。
pub(super) async fn delete_strategy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<DeleteReply>, WebError> {
    // 与 GET source / ai-edit 同口径: 非法名一律 404(不区分"非法"与"不存在", 防路径探测)。
    if ricow_strategy::validate_strategy_name(&id).is_err() {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }
    if catalog::is_builtin_id(&id) {
        return Err(WebError::conflict(
            format!("'{id}' 是内置策略, 不能删除: 它随程序发布, 不是你的文件"),
            "reserved",
        ));
    }
    // 候选位置与 CLI / AI 对话的删除**同一份判据**(见 `commands::strategy_file_candidates`)——
    // 免得两条入口删的文件不一样, 那正是 031 之后"策略删了却仍在列表里"的老问题。
    let targets = crate::commands::strategy_file_candidates(&state.root, &id);
    if !targets.iter().any(|p| p.exists()) {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }

    // 运行中禁止删除(与保存同口径: 先停再动)。
    let running = crate::commands::instances::views(&state.root)
        .await
        .iter()
        .any(|v| v.running && v.name == id);
    if running {
        return Err(WebError::conflict(
            format!("策略 '{id}' 正在运行: 请先停止再删除 (FR-020)"),
            "running",
        ));
    }

    let mut removed: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for p in targets {
        match std::fs::remove_file(&p) {
            Ok(()) => removed.push(rel_path(&state.root, &p)),
            // 不存在 = 本来就没有(裸脚本没有清单), 不是错。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => failures.push(format!("{}: {e}", rel_path(&state.root, &p))),
        }
    }

    if !failures.is_empty() {
        // 部分删除比全不删更糟 —— 必须让用户知道**现在到底什么状态**(已删哪些、哪些没删掉),
        // 不能回一句"删除失败"就完事(那时磁盘已经变了)。
        return Err(WebError::from(CoreError::Exchange(format!(
            "删除策略 '{id}' 时有文件删不掉(已删 {} 个: {}): {}",
            removed.len(),
            if removed.is_empty() { "无".to_string() } else { removed.join(", ") },
            failures.join("; ")
        ))));
    }
    if removed.is_empty() {
        return Err(WebError::not_found(format!(
            "策略 {id} 在目录里已经找不到任何文件(可能已被外部删除)"
        )));
    }
    Ok(Json(DeleteReply { deleted: true, removed }))
}

/// 相对 root 的路径(便于回包与文案; 异常时退回绝对路径)。统一成正斜杠, 免得 Windows 上
/// 回包与测试断言因为 `\` 与 `/` 打架。
fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

// ---- AI 改 Lua (data-model §6, http-api §4) ----

/// AI 改写的两种模式(052 方案 3): 改代码 / 只改参数取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AiEditMode {
    /// 改写 Lua 源码(既有行为, 缺省)。
    Code,
    /// 把自然语言翻译成**已声明参数**的取值, 不碰代码。
    Params,
}

/// 解析 `mode` 字段: 缺省 / `"code"` → [`AiEditMode::Code`]; `"params"` → [`AiEditMode::Params`];
/// 其他 → 中文 400。缺省取 `code` 是为了**向后兼容**(老前端不带这个字段)。
pub(super) fn parse_ai_mode(raw: Option<&str>) -> Result<AiEditMode, String> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None | Some("") | Some("code") => Ok(AiEditMode::Code),
        Some("params") => Ok(AiEditMode::Params),
        Some(other) => Err(format!("mode 仅支持 code|params, 收到 '{other}'")),
    }
}

/// `POST .../ai-edit` 请求: 一句自然语言修改指令(去空白后必须非空) + 模式 + 对话历史。
#[derive(Debug, Deserialize)]
pub(super) struct AiEditRequest {
    instruction: String,
    /// `code`(缺省) = 改写 Lua 源码; `params` = 只改参数取值, 不碰代码。
    #[serde(default)]
    mode: Option<String>,
    /// N3 (2026-10-12): `params` 模式的**多轮对话历史**(前端持有并回传的纯文本流水)。
    ///
    /// 为什么由前端持有: 服务端不引入会话状态 —— 一次 AI 调用就是一次纯函数式的问答
    /// (`quick_params`), 没有可持久化的东西, 也就不该有"这个对话属于谁"的一堆生命周期问题。
    /// 代价如实现: 刷新页面即失, 历史只在当前这次浏览里有效 —— 这一点在界面上如实写明。
    /// 长度由 [`cap_history`] 截断(`code` 模式忽略它, 见 `ai_edit_strategy` 的说明)。
    #[serde(default)]
    history: Option<String>,
}

/// `POST .../ai-edit` 成功响应: 两个模式**二选一**(均**未落盘**, FR-018)。
#[derive(Debug, Serialize)]
pub(super) struct AiEditReply {
    /// `code` 模式: 改写后的完整 Lua(已过编译门禁)。
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    /// `params` 模式: 建议的参数取值(**已过滤**为清单里真实存在的键)。
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<HashMap<String, ConfigValue>>,
    /// N3: `params` 模式: 顾问的**自然语言说明**(打算怎么改 / 逐项理由 / 风险), **未落盘**。
    ///
    /// 这是 N3 的重点: 从前回包只有一串 `{键: 值}`, 用户看到"间距 0.01→0.02"却不知道**为什么**、
    /// 也不知道**代价**是什么。现在先给一段人话, 数字只是它的落点。
    #[serde(skip_serializing_if = "Option::is_none")]
    advice: Option<String>,
    /// N3: 真正**变了**的键(旧 → 新)。`params` 给的是整组值, 这里只列变动项 —— 逐条对照表。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    changed: Vec<ParamChange>,
    /// 静态检查提示(P1-5)/ 被丢弃的建议键(052): 只提示不拦截。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

/// N3: 一个参数在本次改动里的前后取值 —— 前端据此渲染"这次到底动了什么"。
///
/// 与 `params`(整组值)的分工: `params` 是**要给表单的东西**, `changed` 是**要给人看的东西**。
/// 两者不能合并 —— 表单需要整组(否则没被提到的键会被当成"没值"), 而人不需要读 12 行里 10 行没变的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct ParamChange {
    /// 参数键(清单里的键名)。
    #[serde(default)]
    pub(super) key: String,
    /// 中文名(清单没填则**回退键名**)—— 给用户看的那行必须是人能认的。
    #[serde(default)]
    pub(super) name: String,
    /// 改动前的**有效值**: 优先实例 TOML 里保存过的值; 没保存过则清单默认值; 都没有 → 字段不出现。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) before: Option<String>,
    /// 改动后的值(模型给的、已按清单类型归一)。
    #[serde(default)]
    pub(super) after: String,
    /// `before` 是不是**只是清单默认值**(用户从没保存过这个键)。
    ///
    /// 必须如实标出来: "改前 0.01" 与 "清单默认 0.01, 你没保存过" 是两件事 ——
    /// 前者说明这组参数真的在跑, 后者说明用户其实还没确认过它。混在一起就是编造运行事实。
    #[serde(default)]
    pub(super) before_is_default: bool,
}

/// `POST /api/strategies/ai-generate` 请求(P0-3): 结构化的从零生成意图。
#[derive(Debug, Deserialize)]
pub(super) struct AiGenerateRequest {
    /// 策略名(生成成功后由前端调 POST /api/strategies 落盘)。
    name: String,
    /// 市场: spot | futures。
    market: String,
    /// 交易对(如 ETHUSDT)。
    pair: String,
    /// 主时钟周期(缺省 1h; 白名单同回测)。
    #[serde(default)]
    interval: Option<String>,
    /// 策略思路(自然语言, 必填)。
    idea: String,
    /// 风控与约束(可选)。
    #[serde(default)]
    constraints: Option<String>,
}

/// `POST /api/strategies/ai-generate` 成功响应: 生成的完整 Lua(已过编译门禁, **未落盘**)。
#[derive(Debug, Serialize)]
pub(super) struct AiGenerateReply {
    code: String,
    /// 静态检查提示(P1-5): 只提示不拦截。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

// ---- 052 方案 3: AI 改配置(只改参数, 不动代码) ----

/// 把实例当前值排成给模型的「键 = 值」清单(纯函数, 便于单测)。
///
/// 顺序刻意固定(键排序 + `pair` 归在同一条序列里): 同样的输入必须给模型同样的提示词,
/// 否则"哪次调用了什么"不可复现。
fn current_values_text(values: Option<&InstanceValues>) -> String {
    let mut rows: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    if let Some(v) = values {
        if let Some(p) = &v.pair {
            rows.insert("pair".to_string(), format!("{p:?}"));
        }
        for (k, val) in &v.current {
            rows.insert(k.clone(), config_value_text(val));
        }
    }
    if rows.is_empty() {
        return "(这个策略还没有保存过任何取值 —— 全部参数都在用清单里的默认值)".to_string();
    }
    let mut out = String::new();
    for (k, v) in rows {
        out.push_str(&format!("- {k} = {v}\n"));
    }
    out
}

/// 单个参数值的可读文本(提示词与 warning 共用)。
fn config_value_text(v: &ConfigValue) -> String {
    match v {
        ConfigValue::String(s) => format!("{s:?}"),
        ConfigValue::Float(f) => f.to_string(),
        ConfigValue::Integer(i) => i.to_string(),
        ConfigValue::Boolean(b) => b.to_string(),
    }
}

/// 剥掉 ``` 代码围栏(prompt 要求不带, 模型不遵守时兜底; 与 `ricow_engine::extract_code` 同一思路)。
fn strip_fence(t: &str) -> &str {
    let t = t.trim();
    match t.strip_prefix("```") {
        Some(rest) => {
            let inner = rest.split_once('\n').map(|(_, b)| b).unwrap_or(rest);
            inner.trim_end().strip_suffix("```").unwrap_or(inner).trim()
        }
        None => t,
    }
}

/// 从模型回包里抠出 JSON 对象: 剥掉 ```json 围栏, 再取第一个 `{` 到最后一个 `}`。
fn extract_json_object(reply: &str) -> Option<String> {
    let body = strip_fence(reply);
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    (end > start).then(|| body[start..=end].to_string())
}

/// N3: 把回包切成「自然语言说明」与「JSON 对象」两段(纯函数, 便于单测)。
///
/// 提示词要求"讲完理由后**另起一行**给 JSON、JSON 之后不再写任何内容", 所以优先认
/// **最后一个以 `{` 开头的行** —— 说明里出现一个花括号(中文里很少, 但括号里有公式就可能)时,
/// 按"全文第一个 `{`"切会把说明**腰斩**并把公式当地址, 直接 400。先按行找、再退回全文找,
/// 两种都不成 → JSON 为 None(由调用方报中文 400)。
fn split_advice_and_json(reply: &str) -> (String, Option<String>) {
    // 先拆掉**围栏行**: 模型不遵守"不要围栏"时, ```json / ``` 这两行既不是说明也不是 JSON,
    // 留在说明里就是两行噪声(而且它们会被当成"JSON 之前的说明"原样回给用户)。
    let stripped = strip_fence(reply);
    let body = stripped
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.as_str();
    let mut line_starts = vec![0usize];
    for (i, _) in body.match_indices('\n') {
        line_starts.push(i + 1);
    }
    for &s in line_starts.iter().rev() {
        let line = body[s..].trim();
        if !line.starts_with('{') {
            continue;
        }
        let usable = serde_json::from_str::<HashMap<String, Value>>(line).is_ok()
            || extract_json_object(line)
                .is_some_and(|o| serde_json::from_str::<HashMap<String, Value>>(&o).is_ok());
        if usable {
            let obj = if serde_json::from_str::<HashMap<String, Value>>(line).is_ok() {
                line.to_string()
            } else {
                extract_json_object(line).unwrap_or_default()
            };
            return (body[..s].trim().to_string(), Some(obj));
        }
    }
    match extract_json_object(body) {
        Some(obj) => {
            let start = body.find('{').unwrap_or(0);
            (body[..start].trim().to_string(), Some(obj))
        }
        // 一个花括号都没有: 全都算说明 —— 调用方会因"没有 JSON"回中文 400, 但**说明不丢**
        // (它可能正是"我这次不该改任何参数, 因为…"这种有价值的回答)。
        None => (body.trim().to_string(), None),
    }
}

/// N3: 前端回传的对话历史上限(字符数)。
///
/// 为什么设限: 它会被原样塞进提示词, 而提示词预算是有限的 —— 不设限的话, 用户来回几十轮之后
/// 参数清单那一节会被挤到模型的注意力之外, 反而更容易编键。取 4000 字(约 2-3 千汉字),
/// 够十几轮; 超出只保留**最近**的部分, 因为"再保守点"这类跟进话指代的就是最近聊到的东西。
const MAX_HISTORY_CHARS: usize = 4000;

/// N3: 截断对话历史(纯函数, 便于单测)。空/全空白 → 空串(提示词那边会写"这是第一轮")。
fn cap_history(raw: Option<&str>) -> String {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let total = s.chars().count();
    if total <= MAX_HISTORY_CHARS {
        return s.to_string();
    }
    let tail: String = s.chars().skip(total - MAX_HISTORY_CHARS).collect();
    format!("(更早的对话已省略)\n{tail}")
}

/// N3: 把 AI 建议的整组值与**当前有效值**逐个比对, 只留真正变了的键(纯函数, 便于单测)。
///
/// "当前有效值" 的取法(两条来源**分别标注**, 见 [`ParamChange::before_is_default`]):
/// ① 实例 TOML 里保存过的值 → `before_is_default = false`(这组参数真的在跑);
/// ② 没保存过 → 清单默认值 → `before_is_default = true`; ③ 两者都没有 → `before` 为 None。
///
/// 比较用的是**格式化后的文本**而非值本身(`ConfigValue` 没有 `PartialEq`, 且这里要的就是
/// "用户看到的数字变没变"): `1`(整数) 与 `1.0`(浮点) 显示相同 → 不算改动。
fn diff_params(
    kept: &HashMap<String, ConfigValue>,
    values: Option<&InstanceValues>,
    manifest: &StrategyManifest,
) -> Vec<ParamChange> {
    let mut out: Vec<ParamChange> = Vec::new();
    let mut keys: Vec<&String> = kept.keys().collect();
    keys.sort(); // 输出稳定: 对照表的行序不随 HashMap 迭代顺序漂移
    for key in keys {
        let Some(value) = kept.get(key) else { continue };
        let spec = manifest.params.iter().find(|p| &p.key == key);
        let name = spec
            .map(|p| if p.name.trim().is_empty() { p.key.clone() } else { p.name.clone() })
            .unwrap_or_else(|| key.clone());
        let after = config_value_text(value);
        let saved = values.and_then(|v| v.current.get(key));
        let (before, before_is_default) = match saved {
            Some(v) => (Some(config_value_text(v)), false),
            None => match spec.and_then(|p| p.default.as_ref()) {
                Some(d) => (Some(config_value_text(d)), true),
                None => (None, true),
            },
        };
        if before.as_deref() == Some(after.as_str()) {
            continue; // 没变: 不进对照表(整组值仍在 `params` 里回给表单)
        }
        out.push(ParamChange { key: key.clone(), name, before, after, before_is_default });
    }
    out
}

/// 把 AI 建议的参数值**过滤**成清单里真实存在的键(纯函数, 便于单测)。
///
/// 红线(与对话侧 `update_params` 工具描述同源: "只有用户明确的键才写, 不要自行推断"):
/// **不能让模型凭空编键去写实例 TOML** —— 清单里没有的键一律丢弃, 且**如实回一条 warning**,
/// 不静默(用户可能真想要新参数, 那就该走"改代码逻辑")。类型对不上 / 枚举值不在可选集里
/// 同样丢弃并点名原因。
pub(super) fn keep_manifest_params(
    suggested: HashMap<String, ConfigValue>,
    manifest: &StrategyManifest,
) -> (HashMap<String, ConfigValue>, Vec<String>) {
    let mut kept: HashMap<String, ConfigValue> = HashMap::new();
    let mut warns: Vec<String> = Vec::new();
    let mut keys: Vec<&String> = suggested.keys().collect();
    keys.sort(); // 输出稳定: 用户看到的 warning 顺序不随 HashMap 迭代顺序漂移
    for key in keys {
        let Some(value) = suggested.get(key).cloned() else { continue };
        let Some(spec) = manifest.params.iter().find(|p| &p.key == key) else {
            warns.push(format!(
                "AI 建议的参数 '{key}' 不在本策略清单里, 已忽略 —— 新增参数必须同时改 Lua 代码, \
                 它不会生效; 若确实要新增, 请改用「改代码逻辑」模式"
            ));
            continue;
        };
        match coerce_suggested(spec, value) {
            Ok(v) => {
                kept.insert(key.clone(), v);
            }
            Err(why) => warns.push(format!("AI 给的参数 '{key}' 已忽略: {why}")),
        }
    }
    (kept, warns)
}

/// 把建议值按清单声明的类型归一; 类型不符 → 中文原因(由 [`keep_manifest_params`] 转 warning)。
fn coerce_suggested(spec: &ManifestParam, v: ConfigValue) -> Result<ConfigValue, String> {
    let bad = |want: &str, got: &ConfigValue| {
        format!("'{}' 需要{want}, AI 给的是 {}", spec_name(spec), config_value_text(got))
    };
    match spec.ty {
        ParamType::Float => match v {
            ConfigValue::Float(f) => Ok(ConfigValue::Float(f)),
            ConfigValue::Integer(i) => Ok(ConfigValue::Float(i as f64)),
            other => Err(bad("数值", &other)),
        },
        ParamType::Int => match v {
            ConfigValue::Integer(i) => Ok(ConfigValue::Integer(i)),
            ConfigValue::Float(f) if f.is_finite() && f.fract() == 0.0 => {
                Ok(ConfigValue::Integer(f as i64))
            }
            other => Err(bad("整数", &other)),
        },
        ParamType::Str => match v {
            ConfigValue::String(s) => Ok(ConfigValue::String(s)),
            other => Err(bad("字符串", &other)),
        },
        ParamType::Enum => match v {
            ConfigValue::String(s) => {
                let opts = spec.options.clone().unwrap_or_default();
                if opts.contains(&s) {
                    Ok(ConfigValue::String(s))
                } else {
                    Err(format!("'{}' 只接受 {}(AI 给了 '{s}')", spec_name(spec), opts.join("/")))
                }
            }
            other => Err(bad("枚举字符串", &other)),
        },
        ParamType::Bool => match v {
            ConfigValue::Boolean(b) => Ok(ConfigValue::Boolean(b)),
            other => Err(bad("布尔值", &other)),
        },
    }
}

/// 参数的中文名(无则退回键名)—— 报错时点名用户看得懂的那个。
fn spec_name(p: &ManifestParam) -> String {
    if p.name.is_empty() {
        p.key.clone()
    } else {
        format!("{} ({})", p.name, p.key)
    }
}

/// AI 调用错误 → WebError 的统一映射(code / params 两个模式共用, 免得两处口径漂移)。
fn map_ai_error(e: CoreError) -> WebError {
    match e {
        // 缺密钥/鉴权前置失败 → 403 引导用户先去设置页配置 (data-model §6)。
        CoreError::Auth(msg) => WebError::forbidden(
            format!("AI 尚未配置可用密钥, 无法发起 AI 修改: {msg}"),
            "need_keys",
        ),
        // 超时/上游/流式中断(provider 层已包中文)→ 400, 用户可重试或手动改。
        CoreError::Exchange(msg) | CoreError::Network(msg) => WebError::bad_request(format!(
            "AI 调用失败(超时或上游错误), 请稍后重试或改为手动编辑: {msg}"
        )),
        other => WebError::from(other),
    }
}

// ---- handler ----

/// `GET /api/strategies/{id}/source`: 读策略 Lua 原文与用户实例 TOML 原文。
///
/// 防目录穿越: id 必须先过名字规范(无 `/`、`\`、`..`), 且必须在统一目录里注册;
/// 非法形态 / 未知 id 一律 404 中文(不区分"非法"与"不存在", 不回显路径)。
pub(super) async fn get_strategy_source(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<SourceReply>, WebError> {
    if ricow_strategy::validate_strategy_name(&id).is_err() {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }
    let entry = catalog::find(&id).ok_or_else(|| WebError::not_found(format!("没有策略 {id}")))?;

    // 内置策略只有编译期嵌入的清单/脚本, 没有用户实例 TOML → 恒为 null。
    let instance_toml =
        if entry.source == Source::Builtin { None } else { read_instance_toml(&state.root, &id)? };

    Ok(Json(SourceReply {
        id,
        market: entry.manifest.market.clone(),
        // 目录条目的 code: 内置=编译期嵌入的仓库文件原文; 用户=扫描期读盘的 .lua 原文。
        lua: entry.code.clone(),
        instance_toml,
    }))
}

/// `POST /api/strategies`(新建/复制/覆盖保存): 校验链 → 编译门禁 → 引擎落盘内核。
///
/// 编译在任何写盘之前, 编译失败不落盘、不破坏旧文件 (FR-017); 落盘阶段的备份/回滚
/// 语义与终端 `ricow deploy`、对话受控覆盖**完全一致**(同一内核)。
pub(super) async fn save_strategy(
    State(state): State<WebState>,
    // 手动接 Result: JSON 解析失败也回统一中文 WebError(同 keys.rs / markets.rs)。
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<SaveReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: SaveRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!(
            "请求体字段非法: 需要 name/market/pair/code 字符串、params 对象、overwrite 布尔: {e}"
        ))
    })?;

    let name = req.name.trim();
    let pair = req.pair.trim();
    if pair.is_empty() {
        return Err(WebError::bad_request("pair(交易对)不能为空"));
    }
    if req.code.trim().is_empty() {
        return Err(WebError::bad_request("code(Lua 源码)不能为空"));
    }
    let market = parse_market(&req.market).map_err(WebError::bad_request)?;

    // 落盘目录 = 数据目录下 strategies/(与 AI 会话受控覆盖、CLI deploy 同一路径)。
    let dir = crate::commands::ensure_strategies_dir_in(&state.root)?;
    let exists = strategy_files_exist(&dir, market, name);
    let existing = crate::commands::deployed_strategy_names_in(&state.root);
    // 运行台账: daemon 在线取实时视图, 不在线退回历史台账(running 均为 false, 如实)。
    let running = crate::commands::instances::views(&state.root)
        .await
        .iter()
        .any(|v| v.running && v.name == name);
    let allow_replace = decide_allow_replace(name, &existing, exists, req.overwrite, running)
        .map_err(Reject::into_web_error)?;

    // 参数组装(纯函数): 非法值类型在编译/落盘之前 400。
    let params = config_values(&req.params).map_err(WebError::bad_request)?;

    // 编译门禁(gate 1): create_strategy 内含名字复核 + 剥围栏 + mlua 编译, 失败透传引擎原文行号。
    let mut config = ricow_engine::create_strategy(name, &req.code, pair, params)
        .map_err(|e| WebError::compile_failed(e.clone(), parse_lua_line(&e)))?;
    config.market = market.to_string();

    // 052 方案 1: 复制/模板新建 → 先把源清单派生好。任何失败都在**写盘之前**返回(400)。
    let manifest_target: Option<(std::path::PathBuf, String)> =
        match req.copy_from.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(src_id) => {
                if req.overwrite {
                    return Err(WebError::bad_request(
                        "copy_from(复制源)与 overwrite=true 互斥: 复制是「新建一个策略」, \
                     覆盖保存请去掉 copy_from(参数清单另有 /manifest 端点维护)",
                    ));
                }
                let src = catalog::find(src_id).ok_or_else(|| {
                    WebError::bad_request(format!(
                        "copy_from 指向的策略 '{src_id}' 不存在: 无法复制它的参数清单"
                    ))
                })?;
                if src.source == Source::User && !src.declared {
                    // 源自己就没有清单 —— 复制过去也只是空清单, 不如直说, 让前端走「从脚本识别参数」。
                    return Err(WebError::bad_request(format!(
                        "复制源 '{src_id}' 没有声明参数清单(它是裸 .lua), 复制不会带来参数说明: \
                     请直接在副本页点「从脚本识别参数」"
                    )));
                }
                let derived = derive_manifest(&src.manifest, name, market);
                let toml_str = toml::to_string_pretty(&derived).map_err(|e| {
                    WebError::from(CoreError::Exchange(format!("副本清单序列化失败: {e}")))
                })?;
                Some((dir.join(market).join(format!("{name}.toml")), toml_str))
            }
        };

    // 先写清单(若本次是复制): 它失败时**一个字都还没落盘**, 无需回滚脚本;
    // 反过来(先写脚本)失败则要恢复 `.lua`/`.toml` 两个文件, 回滚面大得多。
    let manifest_written = match &manifest_target {
        Some((path, text)) => Some((path.clone(), write_manifest_file(path, text)?)),
        None => None,
    };

    // 落盘内核(与 execute_strategy 同一份): 备份 .bak → 先写 lua 再写 toml → 失败回滚。
    let deployed = match ricow_engine::write_strategy_files(config, &dir, allow_replace) {
        Ok(d) => d,
        Err(e) => {
            // 脚本没落盘 → 把刚写的清单回滚掉, 不留"有清单没脚本"的半成品。
            if let Some((path, backup)) = &manifest_written {
                rollback_manifest(path, backup.as_deref());
            }
            return Err(match &e {
                // 并发竞争下内核仍可能以"拒绝覆盖"兜底 —— 映射成 409 exists 而非 500。
                CoreError::InvalidArgument(msg) if msg.contains("拒绝覆盖") => {
                    WebError::conflict(msg.clone(), "exists")
                }
                _ => WebError::from(e),
            });
        }
    };

    Ok(Json(SaveReply {
        name: name.to_string(),
        market: market.to_string(),
        toml_path: deployed.toml_path.display().to_string(),
        lua_path: deployed.lua_path.display().to_string(),
        backup: deployed.backup.map(|p| p.display().to_string()),
        manifest_path: manifest_written.map(|(p, _)| p.display().to_string()),
        warnings: lint_lua(&req.code),
    }))
}

/// `POST /api/strategies/{id}/ai-edit`(FR-018 + 052 方案 3 + N3): 一句自然语言指令 → AI 改写。
///
/// **两种模式**(`mode`, 缺省 `code`):
/// - `code`: 在**原代码**基础上改写, 产物先剥围栏再过**与保存同一道编译门禁**, 返回 `{code}`;
/// - `params`(052/N3): 把用户**说的意图**(如"保守点")翻译成**已声明参数**的一整组取值,
///   返回 `{advice, changed, params}`, **一行代码都不碰**。
///
/// N3 的两处变化:
/// 1. `params` 模式**支持多轮** —— 请求可带 `history`(前端持有的纯文本流水), 后端据此接住
///    "再保守点"这类跟进话。`code` 模式**仍是单轮**: 它每次产出的是**整份代码**(全量重写),
///    上一轮那版代码已经在编辑器里了, 再叠一层对话历史对"该怎么改这几行"没有增量信息,
///    反而会挤占代码本身的注意力预算。这一点是有意的, 不是漏做。
/// 2. `params` 模式回包多了 `advice`(为什么这么改 / 风险)与 `changed`(旧→新对照) ——
///    从前只回一串数字, 用户看到"间距 0.01→0.02"却不知道原因与代价。
///
/// 两个模式都**不写任何文件**: 用户必须随后显式调 `POST /api/strategies`(overwrite)才落盘。
///
/// 状态码(http-api §4 / data-model §6):
/// - 指令为空 / 非 JSON / `mode` 非法 → 400; 非法/未知 id → 404(与 source 同口径);
/// - 未配 AI 密钥(远程端点缺 key)→ 403 `code:"need_keys"`(provider 在联网前即以 Auth 失败);
/// - AI 超时/上游错误 → 400; `code` 产物无法提取 Lua 或编译失败 → 400 `code:"compile"` 带行号;
/// - `params` 模式: 清单为空 → 400; 回包不是 JSON 对象 → 400 `code:"params_parse"`;
///   过滤后一个键都不剩 → 400 `code:"no_params"`。
pub(super) async fn ai_edit_strategy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<AiEditReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: AiEditRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!("请求体字段非法: 需要非空 instruction 字符串: {e}"))
    })?;
    let mode = parse_ai_mode(req.mode.as_deref()).map_err(WebError::bad_request)?;
    let instruction = req.instruction.trim();
    if instruction.is_empty() {
        return Err(WebError::bad_request(
            "instruction(修改指令)不能为空: 用一句话说明要改什么, 例如「把网格间距改成 ATR 三倍」",
        ));
    }

    // 与 GET source 同口径: 非法/未知一律 404(不区分, 防穿越, 不回显路径)。
    if ricow_strategy::validate_strategy_name(&id).is_err() {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }
    let entry = catalog::find(&id).ok_or_else(|| WebError::not_found(format!("没有策略 {id}")))?;
    let summary = manifest_summary(&entry.manifest);

    if mode == AiEditMode::Params {
        return ai_edit_params(
            &state,
            &id,
            instruction,
            &summary,
            &entry.manifest,
            req.history.as_deref(),
        )
        .await;
    }

    // 一次性非流式问答(密钥只在 quick_ask 内部构造客户端, 不进 prompt)。
    let replied = crate::ai::provider::quick_ask(&state.root, instruction, &entry.code, &summary)
        .await
        .map_err(map_ai_error)?;

    // 剥围栏兜底(prompt 已要求不带围栏, 模型不遵守时由引擎同一提取逻辑兜底)。
    let code = ricow_engine::extract_code(&replied).ok_or_else(|| {
        WebError::bad_request("AI 未返回可识别的 Lua 代码: 应只输出一份完整 Lua(无解释、无围栏)")
    })?;

    // 编译门禁(与保存同一路径 create_strategy, 不落盘): 编译不运行策略, pair 与参数集不参与
    // 编译产物, 传空即可; name 用 id 仅做名字复核。失败透传引擎原文 + chunk 行号。
    ricow_engine::create_strategy(&id, &code, "", HashMap::new())
        .map_err(|e| WebError::compile_failed(e.clone(), parse_lua_line(&e)))?;

    let warnings = lint_lua(&code);
    Ok(Json(AiEditReply {
        code: Some(code),
        params: None,
        advice: None,
        changed: Vec::new(),
        warnings,
    }))
}

/// 052 方案 3 + N3 的 `params` 分支: 用户的**意图**(而非具体键值) → **清单内参数的整组取值**,
/// 外加一段讲清理由与风险的说明。全程不写任何文件。
async fn ai_edit_params(
    state: &WebState,
    id: &str,
    instruction: &str,
    summary: &str,
    manifest: &StrategyManifest,
    history: Option<&str>,
) -> Result<Json<AiEditReply>, WebError> {
    if manifest.params.is_empty() {
        return Err(WebError::bad_request(
            "这个策略还没有声明参数清单, 没有可调参数: 请先在「参数清单」里声明参数\
             (可点「从脚本识别参数」自动识别), 或者改用「改代码逻辑」模式",
        ));
    }
    // 当前取值只是给模型的上下文; 读不到(还没保存过)不是错误。
    let values = load_instance_values(&state.root, id)?;
    let current = current_values_text(values.as_ref());
    let hist = cap_history(history);

    let replied =
        crate::ai::provider::quick_params(&state.root, instruction, summary, &current, &hist)
            .await
            .map_err(map_ai_error)?;

    // 回包 = 人话 + JSON 两段。先切段: "说不出 JSON"与"说不出人话"是两种毛病, 提示语要分开。
    let (advice, json) = split_advice_and_json(&replied);
    let Some(json) = json else {
        let hint = if advice.is_empty() {
            String::new()
        } else {
            format!("(它的说明是: {})", truncate_for_msg(&advice))
        };
        return Err(WebError::bad_request_code(
            format!(
                "AI 只讲了思路, 没有给出可用的 JSON 参数对象{hint}: 请重试, 或改用「改代码逻辑」模式"
            ),
            "params_parse",
        ));
    };
    let raw: HashMap<String, Value> = serde_json::from_str(&json).map_err(|e| {
        WebError::bad_request_code(
            format!("AI 返回的 JSON 无法解析({e}): 请重试, 或改用「改代码逻辑」模式"),
            "params_parse",
        )
    })?;
    // 值类型转换复用保存端点的同一份实现(拒绝 null/数组/对象), 两处口径不会漂移。
    let suggested =
        config_values(&raw).map_err(|msg| WebError::bad_request_code(msg, "params_invalid"))?;

    let (kept, warnings) = keep_manifest_params(suggested, manifest);
    if kept.is_empty() {
        let detail =
            if warnings.is_empty() { String::new() } else { format!("({})", warnings.join("; ")) };
        return Err(WebError::bad_request_code(
            format!(
                "AI 没有给出任何本策略清单里的参数, 无可应用的修改{detail}: \
                 换一种说法再试, 或用「改代码逻辑」模式"
            ),
            "no_params",
        ));
    }
    // 变动的键逐条对照(整组值仍在 `params` 里回给表单)。**空表不是错误** ——
    // "这次一个数都没动"是一个诚实的结论(用户可能只是问了句"现在这组参数怎么看"),
    // 硬报 400 会把它变成失败, 逼用户重问。
    let changed = diff_params(&kept, values.as_ref(), manifest);
    Ok(Json(AiEditReply {
        code: None,
        params: Some(kept),
        advice: (!advice.is_empty()).then_some(advice),
        changed,
        warnings,
    }))
}

/// 把一段可能很长的说明截到一条 message 里放得下的长度(按字符, 不按字节 —— 中文按 3 字节算会腰斩)。
fn truncate_for_msg(s: &str) -> String {
    const MAX: usize = 200;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}

// ---- N3-B: 评估一组参数改动的实际影响 ----

/// `POST /api/strategies/{id}/ai-assess` 请求(N3-B)。
///
/// 三样输入缺一不可: `changed` 说明动了什么, `base_metrics` / `new_metrics` 是**同一窗口下
/// 两次真实回测**的指标 —— 前端从 `/api/backtest` 的作业回包里**原样透传**。
/// 本端点**不跑回测、不写盘**: 回测走既有作业端点(带 busy / 进度 / 5 分钟保留那一整套),
/// 这里只负责把实测数字讲成人话。分成两步的好处是"回测能不能跑"与"AI 能不能问"各自独立失败,
/// 用户不会因为没配密钥就连回测都跑不了。
#[derive(Debug, Deserialize)]
pub(super) struct AiAssessRequest {
    /// 改动的键与前后取值 —— 与 `ai-edit` 回包里的 `changed` **同一个形状**,
    /// 前端原样回传即可(两端共用一个结构, 不会各自漂移)。
    #[serde(default)]
    changed: Vec<ParamChange>,
    /// 改动前的实测指标(`/api/backtest` 的 `metrics` 对象原样)。
    #[serde(default)]
    base_metrics: Value,
    /// 改动后的实测指标。
    #[serde(default)]
    new_metrics: Value,
}

/// `POST .../ai-assess` 成功响应: 一段人话(**未落盘**, 也不回数字 —— 数字本来就在前端手里)。
#[derive(Debug, Serialize)]
pub(super) struct AiAssessReply {
    advice: String,
}

/// 指标白名单(键 → 中文标签)。**只讲表里有的** —— 模型没见过的字段不能进提示词,
/// 否则它会顺手"解释"一个它其实没看到的数字。
///
/// 顺序 = 讲述顺序(收益 → 成本 → 风险 → 质量 → 规模), 与人在看回测报告时的关注顺序一致。
const ASSESS_METRICS: [(&str, &str); 17] = [
    ("equity_change_pct", "区间收益 %"),
    ("benchmark_return_pct", "基准(买入持有)收益 %"),
    ("net_pnl", "净盈亏(计价币)"),
    ("realized_pnl", "已实现盈亏"),
    ("total_fees", "手续费合计"),
    ("max_drawdown_pct", "最大回撤 %"),
    ("annual_return_pct", "年化收益 %"),
    ("annual_volatility_pct", "年化波动率 %"),
    ("sharpe", "夏普"),
    ("sortino", "索提诺"),
    ("calmar", "卡玛"),
    ("win_rate", "胜率 %"),
    ("profit_factor", "盈亏比"),
    ("total_trades", "成交笔数"),
    ("rejected_count", "被拒单数"),
    ("turnover_ratio", "换手率"),
    ("final_equity", "期末权益"),
];

/// 数字转提示词里的文本: 最多 4 位小数并去掉尾零(`0.15000 → 0.15`)。
///
/// 不做四舍五入到 2 位 —— 夏普 1.234 与 1.23 在"变好还是变差"的判断上可能刚好跨过用户的阈值。
/// 只在**尾零**上做减法, 不在有效位上做取舍。
fn fmt_metric_num(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if f.is_finite() => {
            let s = format!("{f:.4}");
            let t = s.trim_end_matches('0').trim_end_matches('.');
            if t.is_empty() || t == "-" {
                "0".to_string()
            } else {
                t.to_string()
            }
        }
        _ => n.to_string(),
    }
}

/// 把一份 `metrics` 对象排成给模型的「标签 = 值」清单(纯函数, 便于单测)。
///
/// 缺字段 / `null` 一律**跳过**: 内核对算不出来的指标回 `null`(如样本太短的夏普),
/// 把它写成"无"会让模型以为那是 0; 不写它, 模型就只会讲它真看到的那些。
fn metrics_brief(v: &Value) -> Result<String, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| "指标必须是 /api/backtest 回包里的 metrics 对象".to_string())?;
    let mut out = String::new();
    // 窗口上下文: 两份指标必须同窗口才可比, 把窗口写进提示词正是为了让模型能发现不一致。
    let mut ctx: Vec<String> = Vec::new();
    for key in ["pair", "interval"] {
        if let Some(s) = obj.get(key).and_then(Value::as_str).map(str::trim) {
            if !s.is_empty() {
                ctx.push(s.to_string());
            }
        }
    }
    if let Some(d) = obj.get("days").and_then(Value::as_u64) {
        ctx.push(format!("{d} 天"));
    }
    if !ctx.is_empty() {
        out.push_str(&format!("窗口: {}\n", ctx.join(" · ")));
    }
    let mut rows = 0usize;
    for (key, label) in ASSESS_METRICS {
        let Some(val) = obj.get(key) else { continue };
        let text = match val {
            Value::Number(n) => fmt_metric_num(n),
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            _ => continue, // null / 数组 / 对象: 如实不写
        };
        out.push_str(&format!("- {label}: {text}\n"));
        rows += 1;
    }
    if rows == 0 {
        return Err("这份指标里没有任何可比较的字段(它不是回测回包里的 metrics 对象)".to_string());
    }
    Ok(out)
}

/// 把改动列表排成给模型的清单(纯函数, 便于单测)。
///
/// 上限 40 行: 提示词要留预算给两份指标。超出的**如实计数**, 不静默截断 ——
/// 少列了 5 个键却不说, 模型会得出"只动了 40 个"的结论。
const MAX_CHANGED_LINES: usize = 40;

fn changed_lines(changed: &[ParamChange]) -> String {
    let mut out = String::new();
    for c in changed.iter().take(MAX_CHANGED_LINES) {
        let before = match (&c.before, c.before_is_default) {
            (Some(v), false) => v.clone(),
            (Some(v), true) => format!("{v}(清单默认, 未保存过)"),
            (None, _) => "未设置".to_string(),
        };
        out.push_str(&format!("- {} ({}): {} → {}\n", c.name, c.key, before, c.after));
    }
    if changed.len() > MAX_CHANGED_LINES {
        out.push_str(&format!("(另有 {} 项改动未列出)\n", changed.len() - MAX_CHANGED_LINES));
    }
    out
}

/// `POST /api/strategies/{id}/ai-assess`(N3-B): 两份**实测**指标 → 一段人话影响评估。
///
/// 为什么不在这里自己跑回测: 回测是**作业**(busy 409 / 进度 / 结果 5 分钟保留), 已有
/// `/api/backtest` 那一整套; 在 AI 端点里再开一条同步回测路径, 等于把并发规则抄第二遍。
/// 由前端串起"跑改前 → 跑改后 → 问影响"三步, 每一步失败都能单独如实告诉用户。
///
/// 状态码: 非法/未知 id → 404; 非 JSON / 字段类型错 → 400;
/// 没有改动 / 指标不是对象 / 指标里没有可比字段 → 400(带 `code`);
/// 未配密钥 → 403 `code:"need_keys"`; AI 超时或上游错误 → 400。
pub(super) async fn ai_assess_params(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<AiAssessReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: AiAssessRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!(
            "请求体字段非法: 需要 changed 数组、base_metrics / new_metrics 对象: {e}"
        ))
    })?;

    if ricow_strategy::validate_strategy_name(&id).is_err() {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }
    if catalog::find(&id).is_none() {
        return Err(WebError::not_found(format!("没有策略 {id}")));
    }
    if req.changed.is_empty() {
        return Err(WebError::bad_request_code(
            "没有可评估的改动: 请先让 AI 改一组参数(或手动改), 再回来评估影响",
            "nothing_changed",
        ));
    }
    let base = metrics_brief(&req.base_metrics)
        .map_err(|m| WebError::bad_request_code(format!("改前指标不可用: {m}"), "bad_metrics"))?;
    let new = metrics_brief(&req.new_metrics)
        .map_err(|m| WebError::bad_request_code(format!("改后指标不可用: {m}"), "bad_metrics"))?;

    let prompt =
        crate::ai::provider::build_assess_prompt(&changed_lines(&req.changed), &base, &new);
    let advice =
        crate::ai::provider::quick_assess(&state.root, &prompt).await.map_err(map_ai_error)?;
    let advice = strip_fence(&advice).trim().to_string();
    if advice.is_empty() {
        return Err(WebError::bad_request_code("AI 没有给出任何评估内容, 请重试", "assess_empty"));
    }
    Ok(Json(AiAssessReply { advice }))
}

/// `POST /api/strategies/ai-generate`(P0-3): 结构化策略意图 → AI 参照 specs/lua-api.md 全文
/// 从零生成一份完整 Lua, 剥围栏后过**与保存同一道编译门禁**, 返回 `{code}` —— **不写任何文件**,
/// 由前端随后调 `POST /api/strategies`(overwrite=false)落盘。
///
/// 状态码: 名非法/意图为空/参数非法 → 400; 内置保留名或同名已存在 → 409;
/// 未配 AI 密钥 → 403 `code:"need_keys"`; AI 超时/上游错误 → 400;
/// AI 产物无法提取 Lua 或编译失败 → 400 `code:"compile"` 带行号。
pub(super) async fn ai_generate_strategy(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Json<AiGenerateReply>, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: AiGenerateRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!(
            "请求体字段非法: 需要 name/market/pair/idea 字符串, interval/constraints 可选: {e}"
        ))
    })?;

    // ---- 同步校验(全部离线) ----
    let name = req.name.trim();
    ricow_strategy::validate_strategy_name(name)
        .map_err(|e| WebError::bad_request_code(e, "invalid_name"))?;
    if catalog::is_builtin_id(name) {
        return Err(WebError::conflict(
            format!("策略名 '{name}' 是内置策略保留标识, 用户策略不得占用: 请换一个新名字"),
            "reserved",
        ));
    }
    let market = parse_market(&req.market).map_err(WebError::bad_request)?;
    let pair = req.pair.trim();
    if pair.is_empty() {
        return Err(WebError::bad_request("pair(交易对)不能为空"));
    }
    let interval = match req.interval.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => "1h",
        Some(s) => super::backtest_jobs::parse_interval(Some(s)).map_err(WebError::bad_request)?,
    };
    let idea = req.idea.trim();
    if idea.is_empty() {
        return Err(WebError::bad_request("idea(策略思路)不能为空: 描述你想要策略做什么"));
    }
    let constraints =
        req.constraints.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("无特别约束");
    // 同名已存在: 生成结果无法用现名落盘, 当场 409(前端提示换名), 不让用户白等一轮 AI。
    let dir = crate::commands::ensure_strategies_dir_in(&state.root)?;
    if strategy_files_exist(&dir, market, name) {
        return Err(WebError::conflict(
            format!("同名策略 '{name}' 已存在: 请换一个新名字再生成"),
            "exists",
        ));
    }

    // ---- AI 生成(密钥只在 one_shot 内部构造客户端, 不进 prompt) ----
    let prompt =
        crate::ai::provider::build_generate_prompt(market, pair, interval, idea, constraints);
    let replied =
        crate::ai::provider::quick_generate(&state.root, &prompt).await.map_err(|e| match e {
            CoreError::Auth(msg) => WebError::forbidden(
                format!("AI 尚未配置可用密钥, 无法生成策略: {msg}"),
                "need_keys",
            ),
            CoreError::Exchange(msg) | CoreError::Network(msg) => {
                WebError::bad_request(format!("AI 调用失败(超时或上游错误), 请稍后重试: {msg}"))
            }
            other => WebError::from(other),
        })?;

    // 剥围栏兜底 + 编译门禁(与保存同一路径, 不落盘)。
    let code = ricow_engine::extract_code(&replied).ok_or_else(|| {
        WebError::bad_request("AI 未返回可识别的 Lua 代码: 应只输出一份完整 Lua(无解释、无围栏)")
    })?;
    ricow_engine::create_strategy(name, &code, pair, HashMap::new())
        .map_err(|e| WebError::compile_failed(e.clone(), parse_lua_line(&e)))?;

    let warnings = lint_lua(&code);
    Ok(Json(AiGenerateReply { code, warnings }))
}

#[cfg(test)]
mod tests;
