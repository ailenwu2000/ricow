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
    let mut line = format!(
        "{} (键名 {}, 类型 {}{}): {}",
        p.name,
        p.key,
        ty,
        if p.required { ", 必填" } else { "" },
        p.desc
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
    let name = req.name.trim();
    if name.is_empty() {
        return Err("name(策略中文名)不能为空".into());
    }
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
        let pname = p.name.trim();
        if pname.is_empty() {
            return Err(format!("参数 '{key}' 缺中文名(name)"));
        }
        let desc = p.desc.trim();
        if desc.is_empty() {
            return Err(format!("参数 '{key}' 缺说明(desc)"));
        }
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
    let market_dir = dir.join(&market);
    std::fs::create_dir_all(&market_dir).map_err(|e| {
        WebError::from(CoreError::Exchange(format!(
            "创建策略目录 {} 失败: {e}",
            market_dir.display()
        )))
    })?;
    let path = market_dir.join(format!("{id}.toml"));
    let backup = if path.exists() { Some(backup_manifest(&path)?) } else { None };
    let toml_str = toml::to_string_pretty(&manifest)
        .map_err(|e| WebError::bad_request(format!("清单序列化失败: {e}")))?;
    std::fs::write(&path, &toml_str).map_err(|e| {
        WebError::from(CoreError::Exchange(format!("写清单 {} 失败: {e}", path.display())))
    })?;

    Ok(Json(ManifestReply { manifest, backup: backup.map(|p| p.display().to_string()) }))
}

// ---- AI 改 Lua (data-model §6, http-api §4) ----

/// `POST .../ai-edit` 请求: 一句自然语言修改指令(去空白后必须非空)。
#[derive(Debug, Deserialize)]
pub(super) struct AiEditRequest {
    instruction: String,
}

/// `POST .../ai-edit` 成功响应: 只回改写后的完整 Lua(已过编译门禁, **未落盘** FR-018)。
#[derive(Debug, Serialize)]
pub(super) struct AiEditReply {
    code: String,
    /// 静态检查提示(P1-5): 只提示不拦截。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
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

    // 落盘内核(与 execute_strategy 同一份): 备份 .bak → 先写 lua 再写 toml → 失败回滚。
    let deployed =
        ricow_engine::write_strategy_files(config, &dir, allow_replace).map_err(|e| {
            match &e {
                // 并发竞争下内核仍可能以"拒绝覆盖"兜底 —— 映射成 409 exists 而非 500。
                CoreError::InvalidArgument(msg) if msg.contains("拒绝覆盖") => {
                    WebError::conflict(msg.clone(), "exists")
                }
                _ => WebError::from(e),
            }
        })?;

    Ok(Json(SaveReply {
        name: name.to_string(),
        market: market.to_string(),
        toml_path: deployed.toml_path.display().to_string(),
        lua_path: deployed.lua_path.display().to_string(),
        backup: deployed.backup.map(|p| p.display().to_string()),
        warnings: lint_lua(&req.code),
    }))
}

/// `POST /api/strategies/{id}/ai-edit`(FR-018): 一句自然语言指令 → AI 在原代码基础上改写,
/// 产物先剥围栏再过**与保存同一道编译门禁**, 才把 `{code}` 返回给前端;全程**不写任何文件** ——
/// 用户必须随后显式调 `POST /api/strategies`(overwrite)才落盘。
///
/// 状态码(http-api §4 / data-model §6):
/// - 指令为空 / 非 JSON → 400; 非法/未知 id → 404(与 source 同口径);
/// - 未配 AI 密钥(远程端点缺 key)→ 403 `code:"need_keys"`(quick_ask 在联网前即以 Auth 失败);
/// - AI 超时/上游错误 → 400; AI 产物无法提取 Lua 或编译失败 → 400 `code:"compile"` 带行号。
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

    // 一次性非流式问答(密钥只在 quick_ask 内部构造客户端, 不进 prompt)。
    let replied = crate::ai::provider::quick_ask(&state.root, instruction, &entry.code, &summary)
        .await
        .map_err(|e| match e {
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
        })?;

    // 剥围栏兜底(prompt 已要求不带围栏, 模型不遵守时由引擎同一提取逻辑兜底)。
    let code = ricow_engine::extract_code(&replied).ok_or_else(|| {
        WebError::bad_request("AI 未返回可识别的 Lua 代码: 应只输出一份完整 Lua(无解释、无围栏)")
    })?;

    // 编译门禁(与保存同一路径 create_strategy, 不落盘): 编译不运行策略, pair 与参数集不参与
    // 编译产物, 传空即可; name 用 id 仅做名字复核。失败透传引擎原文 + chunk 行号。
    ricow_engine::create_strategy(&id, &code, "", HashMap::new())
        .map_err(|e| WebError::compile_failed(e.clone(), parse_lua_line(&e)))?;

    let warnings = lint_lua(&code);
    Ok(Json(AiEditReply { code, warnings }))
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
