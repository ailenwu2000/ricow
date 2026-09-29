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

use std::collections::HashMap;
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

    Ok(Json(AiEditReply { code }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use ricow_strategy::Database;

    // ---- 纯函数 ----

    #[test]
    fn test_parse_market_whitelist() {
        assert_eq!(parse_market("spot").unwrap(), "spot");
        assert_eq!(parse_market(" FUTURES ").unwrap(), "futures");
        for bad in ["", "  ", "fx", "spots", "现货"] {
            assert!(parse_market(bad).is_err(), "market={bad:?} 应拒绝");
        }
        let err = parse_market("options").unwrap_err();
        assert!(err.contains("spot|futures") && err.contains("options"), "{err}");
    }

    #[test]
    fn test_config_values_converts_json_types() {
        let mut params = HashMap::new();
        params.insert("a_f".to_string(), serde_json::json!(0.01));
        params.insert("a_i".to_string(), serde_json::json!(14));
        params.insert("a_s".to_string(), serde_json::json!("u"));
        params.insert("a_b".to_string(), serde_json::json!(true));
        let cv = config_values(&params).unwrap();
        assert!(matches!(cv.get("a_f"), Some(ConfigValue::Float(f)) if (*f - 0.01).abs() < 1e-9));
        assert!(matches!(cv.get("a_i"), Some(ConfigValue::Integer(14))));
        assert!(matches!(cv.get("a_s"), Some(ConfigValue::String(s)) if s == "u"));
        assert!(matches!(cv.get("a_b"), Some(ConfigValue::Boolean(true))));

        // null / 对象 / 数组拒绝, 且错误带参数键。
        let mut bad = HashMap::new();
        bad.insert("k".to_string(), Value::Null);
        assert!(config_values(&bad).unwrap_err().contains('k'));
        bad.insert("k".to_string(), serde_json::json!({"x": 1}));
        assert!(config_values(&bad).unwrap_err().contains('k'));
        bad.insert("k".to_string(), serde_json::json!([1]));
        assert!(config_values(&bad).unwrap_err().contains('k'));
    }

    #[test]
    fn test_parse_lua_line_finds_chunk_line() {
        let err = "生成代码未通过编译门禁:\n脚本编译错误:\n[string \"chunk\"]:12: '=' expected near 'end'";
        assert_eq!(parse_lua_line(err), Some(12));
        assert_eq!(parse_lua_line("无行号信息"), None);
        // 多个命中取最后一个(消息正文里偶发的 :2: 不影响定位)。
        assert_eq!(parse_lua_line("x:2: boom [string \"c\"]:7: near"), Some(7));
    }

    #[test]
    fn test_decide_allow_replace_matrix() {
        // 新名 + 无冲突 + 未运行 → 全新部署(allow_replace=false)。
        assert!(!decide_allow_replace("new-grid", &[], false, false, false).unwrap());

        // ① 名格式非法 → 400 invalid_name。
        let r = decide_allow_replace("网格 A", &[], false, false, false).unwrap_err();
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert_eq!(r.code, "invalid_name");

        // ② 新名与既有策略互为前缀(两个方向)→ 409 prefix, 文案复用 CLI 原文。
        let existing = vec!["eth-grid".to_string()];
        let r = decide_allow_replace("eth-grid-300", &existing, false, false, false).unwrap_err();
        assert_eq!(r.status, StatusCode::CONFLICT);
        assert_eq!(r.code, "prefix");
        assert!(r.message.contains("互为前缀"), "{r:?}");
        assert!(decide_allow_replace("eth", &existing, false, false, false).is_err());

        // ③ 内置保留名 → 409 reserved(即使 overwrite 也拒)。
        let r = decide_allow_replace("paired_grid", &[], false, true, false).unwrap_err();
        assert_eq!(r.code, "reserved");

        // ④ 运行中实例 → 409 running(优先级高于 overwrite)。
        let r = decide_allow_replace("live-one", &["x".to_string()], true, true, true).unwrap_err();
        assert_eq!(r.code, "running");

        // ⑤ 已存在 + overwrite=false → 409 exists; true → 受控覆盖。
        let r = decide_allow_replace("old-one", &[], true, false, false).unwrap_err();
        assert_eq!(r.code, "exists");
        assert!(decide_allow_replace("old-one", &[], true, true, false).unwrap());

        // 覆盖保存跳过前缀自冲突: 自己在 existing 里也必须放行(只要未运行)。
        assert!(
            decide_allow_replace("eth-grid", &["eth-grid".to_string()], true, true, false).unwrap()
        );
    }

    // ---- 端到端(真实回环套接字, 全程离线: 编译门禁是本地 mlua, 不触网) ----

    /// 临时数据目录。
    fn tmp_root(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ricow-web-stratio-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建临时数据目录");
        d
    }

    /// 起真实监听服务(中间件 + 路由一体), 返回端口。
    async fn boot(root: PathBuf) -> u16 {
        let db = Database::open_in_memory().await.expect("开内存库");
        let store = super::super::SessionStore::new(db.clone());
        let starter: super::super::Starter =
            Arc::new(|_id: &str, _sink: &mut super::super::WebSink| Ok(()));
        let state = super::super::WebState::new("tok-ok".to_string(), root, db, store, starter);
        let (listener, port) = super::super::bind(0).await.expect("绑定回环端口");
        tokio::spawn(super::super::serve(listener, state));
        port
    }

    /// 裸 HTTP/1.1 请求(本 crate 无 HTTP 客户端依赖), 回完整响应文本。
    async fn raw(port: u16, method: &str, target: &str, body: Option<&str>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect((super::super::BIND_ADDR, port))
            .await
            .expect("连上服务");
        let head = match body {
            Some(b) => format!(
                "{method} {target} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{b}",
                super::super::BIND_ADDR,
                b.len()
            ),
            None => format!(
                "{method} {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                super::super::BIND_ADDR
            ),
        };
        stream.write_all(head.as_bytes()).await.expect("发请求");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("读响应");
        String::from_utf8_lossy(&buf).to_string()
    }

    fn status_of(res: &str) -> u16 {
        res.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn body_of(res: &str) -> &str {
        res.split_once("\r\n\r\n").map_or("", |(_, b)| b)
    }

    /// 401 矩阵: 源码读 + 保存写都在 token 门禁之后 —— 无/错 token 一律 401 空体,
    /// 且不回显请求里的 Lua 源码(照 markets.rs 离线风格: 401 在中间件短路)。
    #[tokio::test]
    async fn test_strategy_endpoints_require_token_offline() {
        let root = tmp_root("auth");
        let port = boot(root).await;

        const MARK: &str = "LUA-SECRET-MARK-on_tick";
        let post_body = serde_json::json!({
            "name": "auth-probe", "market": "spot", "pair": "ETHUSDT", "code": MARK
        })
        .to_string();

        for probe in [
            "/api/strategies/paired_grid/source".to_string(),
            "/api/strategies/paired_grid/source?token=wrong".to_string(),
            "/api/strategies".to_string(),
            "/api/strategies?token=wrong".to_string(),
        ] {
            let res = if probe.starts_with("/api/strategies?") || probe == "/api/strategies" {
                raw(port, "POST", &probe, Some(&post_body)).await
            } else {
                raw(port, "GET", &probe, None).await
            };
            assert!(res.starts_with("HTTP/1.1 401"), "{probe} 应 401, 实际: {res}");
            assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
            assert!(!res.contains(MARK), "401 不得回显请求源码: {res}");
        }

        // AI 改 Lua 同样在 token 门禁之后: 无/错 token 401 空体, 不回显指令。
        let ai_body = format!(r#"{{"instruction":"{MARK}"}}"#);
        for probe in [
            "/api/strategies/paired_grid/ai-edit".to_string(),
            "/api/strategies/paired_grid/ai-edit?token=wrong".to_string(),
        ] {
            let res = raw(port, "POST", &probe, Some(&ai_body)).await;
            assert!(res.starts_with("HTTP/1.1 401"), "{probe} 应 401, 实际: {res}");
            assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
            assert!(!res.contains(MARK), "401 不得回显修改指令: {res}");
        }

        // 对 token 取得到内置源码(证明拦的不是"路由不存在")。
        let res = raw(port, "GET", "/api/strategies/paired_grid/source?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200);
        assert!(body_of(&res).contains("on_tick"));
    }

    /// GET source: 内置原文 + instance_toml=null; 未知 id / 目录穿越形态一律 404 中文。
    #[tokio::test]
    async fn test_get_source_builtin_unknown_and_traversal_404() {
        let root = tmp_root("source");
        let port = boot(root).await;

        let res =
            raw(port, "GET", "/api/strategies/shannon_spot_grid/source?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""id":"shannon_spot_grid""#), "{body}");
        assert!(body.contains(r#""market":"spot""#), "{body}");
        assert!(body.contains("on_tick"), "lua 为文件原文: {body}");
        assert!(body.contains(r#""instance_toml":null"#), "内置策略实例 TOML 必须为 null: {body}");

        // 未知 id → 404 中文。
        let res = raw(port, "GET", "/api/strategies/no-such-xyz/source?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 404);
        assert!(body_of(&res).contains("没有策略"), "404 要中文可读: {res}");

        // 目录穿越 / 路径分隔 → 一律 404(不接受 ../、斜杠), 不回显路径。
        for bad in ["..%2Fevil", "a%2Fb", "..%5Cevil"] {
            let target = format!("/api/strategies/{bad}/source?token=tok-ok");
            let res = raw(port, "GET", &target, None).await;
            assert_eq!(status_of(&res), 404, "{} 应 404: {res}", bad);
            assert!(!body_of(&res).contains("strategies"), "不得回显路径: {res}");
        }
    }

    /// POST 保存全链路: 新建 200 落盘 → GET 源码可回读 → 同名 409 → 编译失败 400 且旧文件不坏
    /// → 受控覆盖 200 且有 .bak → 保留名/前缀/坏市场/坏名各自的拒绝码。
    // ② 处有意持 ENV_LOCK 跨 await: catalog 扫描读进程级 RICOW_ROOT, 必须串行化,
    // 否则并行测试会读到彼此的环境变量(同 commands/backtest.rs 的 ENV_LOCK 范式)。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_post_save_roundtrip_compile_guard_and_conflicts() {
        let root = tmp_root("save");
        let port = boot(root.clone()).await;
        let code = "function on_tick(ctx)\n    return {}\nend\n";
        let post = |name: &str, market: &str, the_code: &str, overwrite: bool| {
            serde_json::json!({
                "name": name,
                "market": market,
                "pair": "ETHUSDT",
                "code": the_code,
                "params": {"order_size": 0.01, "grid_count": 14, "mode": "u", "dry": true},
                "overwrite": overwrite,
            })
            .to_string()
        };

        // ① 新名保存 → 200, 文件落位; TOML 只引用 script_path, 不内嵌 Lua。
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post("rt-save", "spot", code, false)),
        )
        .await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(
            body.contains(r#""name":"rt-save""#) && body.contains(r#""market":"spot""#),
            "{body}"
        );
        assert!(body.contains("rt-save.toml") && body.contains("rt-save.lua"), "{body}");
        assert!(!body.contains(r#""backup""#), "全新部署按契约 backup? 字段缺省: {body}");
        let toml_path = root.join("strategies").join("rt-save.toml");
        let lua_path = root.join("strategies").join("spot").join("rt-save.lua");
        assert!(toml_path.is_file() && lua_path.is_file());
        let toml_text = std::fs::read_to_string(&toml_path).unwrap();
        assert!(toml_text.contains("script_path") && toml_text.contains("spot/rt-save.lua"));
        assert!(!toml_text.contains("function on_tick"), "Lua 不得内嵌进 TOML");

        // ② GET source 能回读用户策略的实例 TOML 原文(目录扫描走 project_root → 临时设 RICOW_ROOT)。
        {
            let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
            std::env::set_var("RICOW_ROOT", &root);
            let res = raw(port, "GET", "/api/strategies/rt-save/source?token=tok-ok", None).await;
            assert_eq!(status_of(&res), 200, "{res}");
            let body = body_of(&res);
            assert!(
                body.contains(r#""instance_toml":"#) && !body.contains(r#""instance_toml":null"#)
            );
            assert!(body.contains("script_path"), "实例 TOML 原文应在响应里: {body}");
            assert!(body.contains("return {}"), "lua 原文应在响应里: {body}");
            std::env::remove_var("RICOW_ROOT");
        }

        // ③ 同名再存不带 overwrite → 409 exists。
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post("rt-save", "spot", code, false)),
        )
        .await;
        assert_eq!(status_of(&res), 409, "{res}");
        assert!(body_of(&res).contains(r#""code":"exists""#), "{res}");

        // ④ 编译失败: overwrite=true 也必须在落盘前 400 compile, 带行号, 旧文件一字节不变、无备份。
        let old_lua = std::fs::read_to_string(&lua_path).unwrap();
        let bad_code = "function on_tick(ctx)\n    local x = =\nend\n";
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post("rt-save", "spot", bad_code, true)),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""code":"compile""#), "{body}");
        assert!(body.contains(r#""line":2"#), "应透传 mlua 行号: {body}");
        assert_eq!(std::fs::read_to_string(&lua_path).unwrap(), old_lua, "编译失败不得破坏旧 Lua");
        for ent in walkdir(&root.join("strategies")) {
            assert!(
                ent.extension().and_then(|s| s.to_str()) != Some("bak"),
                "编译失败不得产生备份: {}",
                ent.display()
            );
        }

        // ⑤ 受控覆盖(合法新代码)→ 200, backup 非空; 磁盘有 .bak 且内容是旧脚本, 新脚本已替换。
        let code2 = "function on_tick(ctx)\n    return { v = 2 }\nend\n";
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post("rt-save", "spot", code2, true)),
        )
        .await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""backup":"#), "受控覆盖应给 backup 字段: {body}");
        assert!(body.contains(".bak"), "backup 应指向 .bak: {body}");
        assert!(std::fs::read_to_string(&lua_path).unwrap().contains("v = 2"));
        let has_old_backup = walkdir(&root.join("strategies")).iter().any(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("bak")
                && p.to_string_lossy().contains("rt-save.lua.")
                && std::fs::read_to_string(p).map(|t| t.contains("return {}")).unwrap_or(false)
        });
        assert!(has_old_backup, "旧 Lua 必须备份成 .bak 且内容为旧文");

        // ⑥ 保留名 → 409 reserved。
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post("paired_grid", "spot", code, false)),
        )
        .await;
        assert_eq!(status_of(&res), 409, "{res}");
        assert!(body_of(&res).contains(r#""code":"reserved""#), "{res}");

        // ⑦ 与既有 rt-save 互为前缀的新名 → 409 prefix(两个方向都拒)。
        for candidate in ["rt-save-2", "rt"] {
            let res = raw(
                port,
                "POST",
                "/api/strategies?token=tok-ok",
                Some(&post(candidate, "spot", code, false)),
            )
            .await;
            assert_eq!(status_of(&res), 409, "{candidate} 应 409: {res}");
            assert!(body_of(&res).contains(r#""code":"prefix""#), "{candidate}: {res}");
        }

        // ⑧ market 非法 → 400; 名格式非法 → 400 invalid_name; 参数值非法 → 400 带键名。
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post("ok-other", "options", code, false)),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("spot|futures"), "{res}");

        let mut bad_name = serde_json::json!({"name": "网格 A", "market": "spot", "pair": "ETHUSDT", "code": code});
        bad_name["params"] = serde_json::json!({"weird": null});
        let res =
            raw(port, "POST", "/api/strategies?token=tok-ok", Some(&bad_name.to_string())).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(
            body_of(&res).contains(r#""code":"invalid_name""#),
            "名格式校验先于参数校验: {res}"
        );

        let mut bad_param = serde_json::json!({"name": "ok-param", "market": "spot", "pair": "ETHUSDT", "code": code});
        bad_param["params"] = serde_json::json!({"weird": null});
        let res =
            raw(port, "POST", "/api/strategies?token=tok-ok", Some(&bad_param.to_string())).await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("weird"), "参数错误要带键名: {res}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// manifest 参数摘要: 键名/类型/必填/枚举/默认都要来自清单数据(不硬编码参数名)。
    #[test]
    fn test_manifest_summary_renders_schema() {
        let entry =
            catalog::find("shannon_spot_grid").expect("内置清单 shannon_spot_grid 必须存在");
        let s = manifest_summary(&entry.manifest);
        assert!(s.contains(&entry.manifest.id) && s.contains(&entry.manifest.name), "{s}");
        assert!(s.contains("键名") && s.contains("类型"), "{s}");
        // 内置清单至少有一个参数, 每个参数行都要给键名; 且摘要里找得到 f64/string/bool 之一。
        assert!(!entry.manifest.params.is_empty());
        assert!(
            s.contains("f64") || s.contains("i64") || s.contains("string") || s.contains("bool"),
            "类型标注缺失: {s}"
        );
        assert!(
            entry.manifest.params.iter().any(|p| s.contains(&p.key) && s.contains(&p.name)),
            "每个参数的键名/中文名至少命中一个: {s}"
        );
    }

    /// AI 改 Lua 离线路径: 空指令 400 / 未知 id 404 / 未配 AI 密钥 403 need_keys(在联网前失败)。
    ///
    /// 成功的 LLM 往返与编译门禁的"AI 产物编译失败"分支需要真实上游, 按任务约定不进单测;
    /// 编译门禁本身已由保存端到端用例(真实 mlua)覆盖。
    // 有意持 ENV_LOCK 跨 await: 临时清掉 RICOW_AI_* 防止本机环境变量把缺密钥路径变成真实联网。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_ai_edit_validation_and_need_keys_offline() {
        let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
        // 保存并清空三个 AI 覆盖变量(测完还原), 确保走到"远程端点缺密钥"分支。
        let saved = ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"]
            .map(std::env::var)
            .map(|r| r.ok());
        for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
            std::env::remove_var(k);
        }

        let root = tmp_root("aiedit");
        let port = boot(root.clone()).await;
        let post = |ins: &str| format!(r#"{{"instruction":"{ins}"}}"#);

        // ① 空 / 纯空白指令 → 400(不触网、不查策略)。
        let res = raw(
            port,
            "POST",
            "/api/strategies/shannon_spot_grid/ai-edit?token=tok-ok",
            Some(&post("   ")),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");
        assert!(body_of(&res).contains("instruction"), "{res}");

        // ② 非 JSON → 400。
        let res = raw(
            port,
            "POST",
            "/api/strategies/shannon_spot_grid/ai-edit?token=tok-ok",
            Some("{ not json"),
        )
        .await;
        assert_eq!(status_of(&res), 400, "{res}");

        // ③ 未知 id → 404(与 source 同口径, 先于任何 AI 调用)。
        let res = raw(
            port,
            "POST",
            "/api/strategies/no-such-xyz/ai-edit?token=tok-ok",
            Some(&post("随便改")),
        )
        .await;
        assert_eq!(status_of(&res), 404, "{res}");
        assert!(body_of(&res).contains("没有策略"), "{res}");

        // ④ 合法指令 + 内置策略 + 空配置(临时 root, 模板无 api_key, 默认 deepseek 远程端点)
        //    → 403 code=need_keys, 发生在网络请求之前; 且不回显指令原文。
        let instruction = "UNIQUE-INSTRUCTION-把网格间距改成 ATR 三倍";
        let res = raw(
            port,
            "POST",
            "/api/strategies/shannon_spot_grid/ai-edit?token=tok-ok",
            Some(&post(instruction)),
        )
        .await;
        assert_eq!(status_of(&res), 403, "缺密钥应 403: {res}");
        let body = body_of(&res);
        assert!(body.contains(r#""code":"need_keys""#), "{body}");
        assert!(body.contains("密钥"), "403 要中文可读并引导配置: {body}");
        assert!(!body.contains("UNIQUE-INSTRUCTION"), "错误体不得回显指令/prompt: {body}");

        // 还原环境变量。
        for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved)
        {
            if let Some(val) = v {
                std::env::set_var(k, val);
            } else {
                std::env::remove_var(k);
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GET 详情: 内置 → pair/current 皆 null(清单字段扁平保留); 用户实例 → pair 与已保存参数
    /// 以 JSON 给出, 且内部键 script/script_path 不泄漏进 current(032 T027, 前端免解析 TOML)。
    // 有意持 ENV_LOCK 跨 await: catalog 用户策略扫描读进程级 RICOW_ROOT, 与保存用例同款串行化。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_get_detail_carries_instance_current_values() {
        let root = tmp_root("detail");
        let port = boot(root.clone()).await;

        // ① 内置: 清单字段照常(对话视图依赖扁平的 name/params), pair/current 必须为 null。
        let res = raw(port, "GET", "/api/strategies/shannon_spot_grid?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""id":"shannon_spot_grid""#), "{body}");
        assert!(
            body.contains(r#""name":"#) && body.contains(r#""params":"#),
            "清单字段须扁平保留: {body}"
        );
        assert!(body.contains(r#""pair":null"#), "内置无实例 pair 必须为 null: {body}");
        assert!(body.contains(r#""current":null"#), "内置无实例 current 必须为 null: {body}");

        // ② 保存一个用户策略(带参数), 再以同一 RICOW_ROOT 取详情。
        let code = "function on_tick(ctx)\n    return {}\nend\n";
        let post_body = serde_json::json!({
            "name": "rt-detail",
            "market": "spot",
            "pair": "ETHUSDT",
            "code": code,
            "params": {"order_size": 0.01, "grid_count": 14, "mode": "u", "dry": true},
            "overwrite": false,
        })
        .to_string();
        let res = raw(port, "POST", "/api/strategies?token=tok-ok", Some(&post_body)).await;
        assert_eq!(status_of(&res), 200, "{res}");

        let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", &root);
        let res = raw(port, "GET", "/api/strategies/rt-detail?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""pair":"ETHUSDT""#), "pair 应来自实例 TOML: {body}");
        // 当前参数以 JSON 基本类型给出(ConfigValue 无标签序列化)。
        assert!(body.contains(r#""order_size":0.01"#), "{body}");
        assert!(body.contains(r#""grid_count":14"#), "{body}");
        assert!(body.contains(r#""mode":"u""#), "{body}");
        assert!(body.contains(r#""dry":true"#), "{body}");
        // 内部键不得出现在 current(会被前端误当策略参数回存)。
        assert!(!body.contains(r#""script_path""#), "current 须剔除 script_path: {body}");
        std::env::remove_var("RICOW_ROOT");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 递归列出目录下全部文件(测试小工具)。
    fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return out };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
