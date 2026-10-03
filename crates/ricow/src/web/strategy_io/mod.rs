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
mod tests;
