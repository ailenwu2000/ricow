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

// 054 P2: 风险档位 → 数字的**单点定义**(提示词 / 前端文案 / TOML 都从它派生)。
pub(super) mod risk_profile;

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
    /// 054 D7: 来源标记(`"ai"` = 这份策略由 AI 生成)。
    ///
    /// **只影响 TOML 顶部一行注释**(`# ricow:origin = ai`), 不新增 schema 字段 ——
    /// 与 `declared`/`duplicate_of` 同一哲学: 来源能**推导**就不额外存一列, 多一列就会
    /// 出现"字段说 AI、文件其实是人写的"这类对不上的状态。
    #[serde(default)]
    origin: Option<String>,
    /// 054 P2: 随落盘写进 `[backtest].initial_cash`(准备投入的本金)。
    ///
    /// 它是结构化输入里**唯一真下场**的数字: 写进 TOML 后, 以后每次回测都按这个本金算,
    /// 与用户在生成时填的那个数是同一个 —— 否则"单笔不超过 10%"在生成时和回测时会是两个口径。
    #[serde(default)]
    initial_cash: Option<f64>,
    /// 随落盘写进 `[backtest].max_leverage`(合约**杠杆上限**, 现货不应传)。
    ///
    /// 提示词里对模型说"杠杆必须 ≤ Nx"却在配置里不设上限, 那句话就是空的 ——
    /// 这一项是唯一可静态验证的约束, 必须**说与做一致**。
    ///
    /// 055 B1 修: 此前写的是 `[backtest].leverage`(**实际杠杆**), 于是"选保守档"被落盘成
    /// **实打实 2 倍杠杆** —— 而 `commands/run.rs` 启动实盘时读的正是 `[backtest].leverage`,
    /// 会真的在交易所把杠杆设成 2x / 3x / 5x。平台替用户开杠杆, 而且是在他以为自己选了
    /// "更稳"的时候。上限就该写 `max_leverage`; 实际用多少杠杆留给**用户自己**去设参数
    /// (`leverage` 保持引擎默认 1x = 不加杠杆)。
    #[serde(default)]
    max_leverage: Option<f64>,
    /// 随落盘写进 `[backtest].leverage`(合约**实际**杠杆; 现货不应传)。
    ///
    /// 055 B1 起 AI 生成流程**不再传这个字段**: 风险档位表达的是"最多允许多少",
    /// 不是"就用多少"。保留字段是因为手动新建/编辑策略仍需要能指定实际杠杆。
    #[serde(default)]
    leverage: Option<f64>,
}

/// 054 D7: 实例 TOML 里的来源标记(整行, 写在文件最顶部)。
///
/// 解析端的安全默认(见 [`origin_is_ai`]): **找不到这行注释 = 用户策略**。
/// 绝不反过来默认成 AI —— 那会把每个老策略都标成"AI 生成", 是明目张胆的编造。
pub(super) const ORIGIN_AI_MARKER: &str = "# ricow:origin = ai";

/// 054 D7: 一段实例 TOML 原文是否带 AI 来源标记(**纯函数**)。
///
/// 只认**独立一行**的注释(前缀匹配后必须到行尾), 免得正文里出现同形字符串被误判。
pub(super) fn origin_is_ai(toml_text: &str) -> bool {
    toml_text.lines().any(|l| l.trim() == ORIGIN_AI_MARKER)
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

/// 054 D7/T24: 这份用户策略是否由 AI 生成(**读盘判定**, 不另存一列)。
///
/// 判据是实例 TOML 顶部那行 `# ricow:origin = ai` 注释; 读不到文件/没有标记一律 `false`
/// —— 安全默认永远指向"用户自己写的", 绝不反过来把老策略标成 AI 产物。
pub(super) fn strategy_origin_ai(root: &Path, id: &str) -> bool {
    let path = root.join("strategies").join(format!("{id}.toml"));
    std::fs::read_to_string(&path).map(|s| origin_is_ai(&s)).unwrap_or(false)
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

/// 逐行类警告的条数上限(防刷屏)。
///
/// 055 B4: 这个上限**只管逐行类**, 管不到整份级结论(第 ⑥ 类) —— 否则一段 10 行的
/// `config_f64(...) or ...` 就能把"这份脚本会下单但没有任何止损线索"挤掉, 而那恰恰是
/// 最该让用户看见的一条。
const LINE_WARN_CAP: usize = 8;

/// 静态检查(纯函数, P1-5): 编译门禁只保证**语法**可编译, 这里把已知的高频踩坑
/// (specs/lua-api.md 实测记录)在保存/生成时提示给作者 —— **只提示不拦截**:
/// 每条都可能是误报, 拦截权在用户手里。
///
/// 两类警告的配额**分开算**(055 B4):
/// - 逐行类(①~⑤): 最多 [`LINE_WARN_CAP`] 条, 防止一份长脚本刷屏;
/// - 整份级(⑥): **独立且恒出现**, 排在最前 —— 它是"没人会替你止损"这句最要紧的话,
///   被逐行噪音挤掉等于没说。
pub(super) fn lint_lua(code: &str) -> Vec<String> {
    let mut warns: Vec<String> = Vec::new();
    // ⑥ 055 T35 + B4: **整份代码级** —— 会下单, 却既没有止损线索、也没有仓位上限/爆仓监控线索。
    //
    // 存在理由: 平台**不代做盈亏风控**(宪法级), 亏损熔断与峰值回撤都要策略自己实现
    // (见 RULES 第 9 条)。一份一路买下去、什么都不管的脚本在回测里可能很好看,
    // 但用户未必意识到"没人会替我止损"。
    //
    // **只警告不拦截**: 命中关键字不代表风控逻辑真的对, 没命中也不代表一定没有
    // (可能用了别的名字或别的实现方式) —— 硬拦会误杀合法策略。
    //
    // **先算先放**: 它排在第一位, 且不占逐行配额(那里很容易被 8 条行号细节填满)。
    if lint_trades(code) && !has_any(code, STOP_CLUES) && !has_any(code, SIZE_CLUES) {
        warns.push(
            "这份脚本会下单, 但没看到止损、仓位上限或爆仓监控的任何线索 —— \
             平台不代做盈亏风控(亏损熔断与回撤止损要策略自己实现, 否则会一直亏下去)。\
             只提示不拦截: 静态判断看不出风控逻辑是否真的对。"
                .to_string(),
        );
    }
    for (idx, raw) in code.lines().enumerate() {
        // 逐行部分自己守住上限: 整份级那条已占 1 个位置, 总长 = 1 + LINE_WARN_CAP。
        if warns.len() > LINE_WARN_CAP {
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

/// ⑥ 的止损线索(小写比对): 命中任一个即认为"作者可能做了止损"。
const STOP_CLUES: &[&str] = &[
    "stop_loss",
    "stoploss",
    "stop_pct",
    "止损",
    "max_drawdown",
    "drawdown_pct",
    "dd_limit",
    "stop_out",
];

/// ⑥ 的仓位上限 / 爆仓监控线索。
const SIZE_CLUES: &[&str] = &[
    "max_position",
    "max_pos",
    "position_limit",
    "max_size",
    "max_notional",
    "仓位",
    "爆仓",
    "liquidation",
    "mmr",
    "margin_ratio",
];

/// ⑥ 的前置: 确实会下单才提示。纯观察型脚本(只 log 不下单)没有风控可言,
/// 对它提示"没有止损"是噪音。
fn lint_trades(code: &str) -> bool {
    code.contains("place_order")
        || code.contains("side_order")
        || code.contains("submit_order")
        || code.contains("exec.")
}

/// ⑥ 的线索命中(纯函数): 整份代码小写化后子串匹配。
fn has_any(code: &str, clues: &[&str]) -> bool {
    let lower = code.to_lowercase();
    clues.iter().any(|c| lower.contains(c))
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
    let map = parse_names_json(raw)?;
    let out = parse_names_map(&map, hints);
    if out.is_empty() {
        return Err(WebError::bad_request_code(
            "AI 没有给出任何可用命名: 换一种方式重试, 或自己填中文名(留空也能保存)",
            "names_empty",
        ));
    }
    Ok(out)
}

/// 把回包里的 JSON 对象抠出来并解析成 map(纯解析, 不含"哪些键算数"的业务判断)。
///
/// 054 T37: 从 `parse_names` 里分出来, 让"生成后补摘要"复用同一套解析 —— 它要在**同一个**
/// JSON 里同时读 `_summary` 与参数命名, 重新抠一遍 JSON 就会有两套围栏处理, 迟早对不上。
fn parse_names_json(raw: &str) -> Result<HashMap<String, Value>, WebError> {
    let json = extract_json_object(raw).ok_or_else(|| {
        WebError::bad_request_code(
            "AI 未返回可识别的 JSON 对象: 应只输出一个 {键: {name, desc}} 对象(无解释、无围栏)。\
             请重试",
            "names_parse",
        )
    })?;
    serde_json::from_str(&json).map_err(|e| {
        WebError::bad_request_code(format!("AI 返回的 JSON 无法解析({e}): 请重试"), "names_parse")
    })
}

/// 按**静态提取到的键**过滤命名结果(纯函数)。键集合来自代码事实, 模型多给的键到此为止。
fn parse_names_map(
    map: &HashMap<String, Value>,
    hints: &[crate::strategies::params::ParamHint],
) -> Vec<NamedParam> {
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
    out
}

// ---- 054 T37: 生成后补「一句话摘要 + 参数中文名/说明」(2026-10-13) ----

/// 摘要在 JSON 里的固定字段名。
///
/// 下划线开头: 与 Lua 参数键的命名规范(字母/下划线开头, 但参数键不会这么写)天然不撞,
/// 且 `valid_param_key` 也认它 —— 靠"约定 + 解析端先 remove"两道保证它不会变成一行参数。
pub(super) const SUMMARY_KEY: &str = "_summary";

/// 摘要在界面上是标题下的**一行**副标题(详情页适用性卡、列表行), 不是长文。
/// 提示词已要求 ≤ 40 字, 这里是兜底: 模型超了也不能把整张卡片撑破。
const SUMMARY_MAX_CHARS: usize = 80;

/// 把 AI 给的一句话摘要收拾成能进清单的样子(纯函数): 取第一行、去空白、超长截断。
///
/// 空串 → `None`(没摘要就是没摘要, **不**拿别的东西顶上 —— 拿思路原文顶上等于在详情页
/// 复述用户自己刚写的话, 还让它看起来像系统的结论)。
pub(super) fn clamp_summary(s: &str) -> Option<String> {
    let first = s.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return None;
    }
    let chars: Vec<char> = first.chars().collect();
    if chars.len() <= SUMMARY_MAX_CHARS {
        return Some(first.to_string());
    }
    Some(format!("{}…", chars[..SUMMARY_MAX_CHARS].iter().collect::<String>()))
}

/// 054 T37: 把**静态提取**到的参数(键/类型/默认值 = 代码事实)与 AI 起的中文名/说明合成为
/// 一份可直接 `POST /manifest` 的参数 schema(纯函数, 便于单测)。
///
/// 三条归属纪律(谁的事实谁说话):
/// - 键 / 类型 / 默认值**只认静态提取**: 模型说这个键是 f64 不算数, 代码怎么读才是事实;
/// - 中文名**只认 AI**: 代码里没有"名字"这回事;
/// - 说明**注释优先于 AI**: 脚本注释里作者自己写的那句, 比模型照着代码现编的更可信。
///   两者都没有就留空 —— 留空是常态, 界面回退显示键名(见 N2 的"不逼用户起名")。
pub(super) fn merge_manifest_params(
    hints: Vec<crate::strategies::params::ParamHint>,
    named: &[NamedParam],
) -> Vec<ManifestParam> {
    let by_key: HashMap<&str, &NamedParam> = named.iter().map(|n| (n.key.as_str(), n)).collect();
    let mut out: Vec<ManifestParam> = Vec::with_capacity(hints.len());
    for h in hints {
        // 引擎内部键由 loader 装载使用, 清单不得声明(与 `build_manifest` 同一条)。
        if RESERVED_PARAM_KEYS.contains(&h.key.as_str()) {
            continue;
        }
        let n = by_key.get(h.key.as_str()).copied();
        let name = n.map(|x| x.name.trim()).unwrap_or("").to_string();
        let from_comment = h.desc.as_deref().map(str::trim).filter(|d| !d.is_empty());
        let from_ai = n.and_then(|x| x.desc.as_deref()).map(str::trim).filter(|d| !d.is_empty());
        let desc = from_comment.or(from_ai).unwrap_or("").to_string();
        out.push(ManifestParam {
            key: h.key,
            name,
            ty: h.ty,
            desc,
            default: h.default,
            required: false,
            options: None,
        });
    }
    out
}

/// 从回包里同时读出摘要与参数命名(纯函数)。`_summary` **先摘出来再从 map 里删掉** ——
/// 否则它会作为"模型多给的键"被静默丢弃, 摘要就没了。
pub(super) fn parse_generated_meta(
    raw: &str,
    hints: &[crate::strategies::params::ParamHint],
) -> Result<(Option<String>, Vec<NamedParam>), WebError> {
    let mut map = parse_names_json(raw)?;
    let summary = map
        .remove(SUMMARY_KEY)
        .and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
        .and_then(|s| clamp_summary(&s));
    let names = parse_names_map(&map, hints);
    Ok((summary, names))
}

/// 一次"补摘要 + 补命名"的结果。
#[derive(Debug, Default)]
struct GeneratedMeta {
    summary: Option<String>,
    params: Vec<ManifestParam>,
    /// 没生成出来时的**如实说明**(绝不静默降级)。
    note: Option<String>,
}

/// 054 T37: 代码过编译门禁之后, 再问一轮 AI 要"这份策略在做什么"与参数中文名。
///
/// **复用 N2-C 的命名能力**(同一套 `extract_param_hints` → 提示词 → JSON 解析), 不另起一套:
/// 两处对"键从哪来、模型多给的键怎么办"的判断必须一致, 否则同一个键在详情页与生成弹窗里
/// 会是两个名字。
///
/// 刻意**不让失败影响生成**: 代码已经能落盘, 少了摘要只是"详情页少一行说明", 不该让整轮
/// 生成报错 —— 但也不静默: 原因进 `note`, 由界面如实说出, 用户知道该自己补。
async fn generate_meta(
    root: &Path,
    market: &str,
    pair: &str,
    interval: &str,
    idea: &str,
    code: &str,
) -> GeneratedMeta {
    let hints = crate::strategies::params::extract_param_hints(code);
    let key_lines = if hints.is_empty() {
        "(这份脚本没有读到任何参数, 只要给 _summary)".to_string()
    } else {
        hints.iter().map(|h| format!("- {}", h.key)).collect::<Vec<_>>().join("\n")
    };
    let prompt = crate::ai::provider::build_generate_meta_prompt(
        market, pair, interval, idea, &key_lines, code,
    );
    match crate::ai::provider::quick_meta(root, &prompt).await {
        Err(e) => GeneratedMeta {
            summary: None,
            params: Vec::new(),
            note: Some(format!(
                "摘要与参数说明没能自动生成(AI 调用失败: {e}); 策略本身已生成, 这两项可以在详情页的清单里自己填"
            )),
        },
        Ok(raw) => match parse_generated_meta(&raw, &hints) {
            Ok((summary, named)) => GeneratedMeta {
                summary,
                params: merge_manifest_params(hints, &named),
                note: None,
            },
            Err(e) => GeneratedMeta {
                summary: None,
                params: Vec::new(),
                // 取错误体里那句中文(`WebError` 本身不实现 Display, 免得把状态码一起拼进去)。
                note: Some(format!(
                    "摘要与参数说明没能自动生成({}); 策略本身已生成, 这两项可以在详情页的清单里自己填",
                    e.body.error
                )),
            },
        },
    }
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
    /// 054 P1: 生成后**立刻**跑一次真实 K 线沙箱回测(默认 **true**)。
    ///
    /// 刻意默认开: 审计 #1 —— 此前 Web 生成的策略只有编译门禁, 用户拿到的是一段**从未在任何
    /// 历史数据上跑过**的代码, 却和"跑过回测的策略"在同一个列表里。
    /// 关掉它意味着"我不知道它历史上表现如何", 所以明码标价(UI 上写清楚)。
    #[serde(default)]
    verify: Option<bool>,
    /// 054 P2: 准备投入的本金(USDT, **必填且必须 > 0**)。
    ///
    /// 这是整个结构化输入里**唯一真下场**的数字 —— 它会写进策略 TOML 的 `[backtest].initial_cash`,
    /// 让回测口径与用户真实资金一致(spec §四-5)。其余字段都是"生成时的约束语义"。
    ///
    /// 必填是刻意的: 不知道投多少钱就谈不上"单笔不超过 10%", 整个档位表会失去锚点。
    initial_cash: Option<f64>,
    /// 054 P2: 风险档位(四档): conservative / balanced / aggressive / custom。
    ///
    /// **必填** —— 不填就等于让平台替用户选档, 而"替用户选"正是 D3 禁止的那件事
    /// (自定义档连默认值都不给, 默认档位更是替他选了)。
    risk_tolerance: Option<String>,
    /// 054 P2: 自定义档的**最大回撤目标**(%)。仅 custom 档必填。
    max_drawdown_pct: Option<f64>,
    /// 054 P2: 自定义档的**单笔最大仓位**(%)。仅 custom 档必填。
    max_position_pct: Option<f64>,
    /// 054 P2: 自定义档的**杠杆上限**(仅合约; 现货给了 → 400)。仅 custom 档必填。
    leverage_cap: Option<f64>,
    /// 054 P2: 自定义档的**网格最小间距**(%, 按当前市场解释)。可选 —— 非网格策略不需要。
    min_spacing_pct: Option<f64>,
    /// 054 P1: 验证回测的窗口天数(默认 **90**, 合法 1–3650)。
    ///
    /// 默认 90 不是挑出来的"更好的数": 对话 `preview_strategy`、`ricow backtest`、
    /// 影响评估、Web 回测**四处的默认都是 90**, 取同一个值才能让几处数字互相可比。
    #[serde(default)]
    verify_days: Option<u32>,
}

/// 054 P1: 验证回测窗口天数的合法上界(10 年)。
pub(super) const MAX_VERIFY_DAYS: u32 = 3_650;

/// 054 P2: `GET /api/strategies/risk-profiles` —— 风险档位表(**纯常量派生**, 无状态)。
///
/// 前端「这档意味着什么」的文案与数字一律从这里取, **不得**在 JS 里手抄一份 ——
/// 两处手抄必然在改表时静默分叉, 用户看到的档位说明就不是真正生效的那个数(spec §四-1)。
pub(super) async fn list_risk_profiles() -> Json<Value> {
    Json(serde_json::json!({ "tiers": risk_profile::docs() }))
}

/// 054 P1: 生成后那次验证回测的结果。
///
/// **失败不阻断生成** —— 网络不可达 / 交易对无数据时, 代码照常回给用户,
/// 只是 `ok=false` 且带中文原因。宁可回一份"未验证但能用"的代码, 也不让一次网络抖动
/// 吃掉用户已经等了几十秒的 AI 产物(生成本身与验证是两件事, 不该互相绑架)。
#[derive(Debug, Serialize)]
pub(super) struct VerifyOutcome {
    /// 回测是否真的跑出来了(指标/图表都有值)。
    ok: bool,
    /// 实际回测天数(回给前端, 免得界面另猜一份)。
    days: u32,
    /// K 线周期。
    interval: String,
    /// 文本报告(与 CLI `ricow backtest` 逐字同源; 失败为 None)。
    #[serde(skip_serializing_if = "Option::is_none")]
    report_text: Option<String>,
    /// 结构化指标(前端指标卡直接吃; 失败为 None)。
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<Value>,
    /// 结构化图表(权益曲线/基准/成交点; 失败为 None)。
    #[serde(skip_serializing_if = "Option::is_none")]
    chart: Option<Value>,
    /// 055 B3: 档位目标的**事后核对**("你选的回撤目标 ≤10%, 实测 25%, 未达到")。
    /// 只核回测真能测出来的项(目前只有最大回撤), 测不出/不适用则不下发。
    #[serde(skip_serializing_if = "Option::is_none")]
    tier_check: Option<risk_profile::TierCheck>,
    /// 054 P3-4: 本次回测**实际生效**的成本假设 —— 必须与指标并排展示(审计 #4)。
    /// 与回测作业同一类型、同一取值函数, 免得两处各写一份口径。
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<super::backtest_jobs::CostAssumption>,
    /// 失败原因(ok=false 时**必填**; ok=true 时为 None)。
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// `POST /api/strategies/ai-generate` 成功响应: 生成的完整 Lua(已过编译门禁, **未落盘**)。
#[derive(Debug, Serialize)]
pub(super) struct AiGenerateReply {
    code: String,
    /// 静态检查提示(P1-5): 只提示不拦截。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// 054 P1: 生成后那次验证回测的结果。`verify=false` 时字段不出现(用户明确跳过了)。
    ///
    /// 刻意**不**把 ok 与否并进 `warnings`: 那是个静态检查清单, 混进"网络不通"会让人
    /// 以为是代码有问题。
    #[serde(skip_serializing_if = "Option::is_none")]
    verify: Option<VerifyOutcome>,
    /// 054 P2: 解析后的本金(回给前端, 让它在落盘时写进 `[backtest].initial_cash`)。
    initial_cash: f64,
    /// 054 T37: AI 写的一句话摘要("这份策略在做什么")。**依据只有代码** —— 提示词里明令
    /// 不许把用户想法里没落地的部分写进来。前端保存时一并写进清单 `summary`。
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    /// 054 T37: 可直接交给 `POST /api/strategies/{id}/manifest` 的参数 schema ——
    /// 键/类型/默认值来自**静态提取**(代码事实), 中文名/说明来自 AI。
    ///
    /// 刻意**在服务端合并**而不是让前端自己拼: 拼错一次就会写出"类型是模型说的"这种清单,
    /// 而清单是参数表单的唯一来源。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    manifest_params: Vec<ManifestParam>,
    /// 054 T37: 上面两样没生成出来时的**如实说明**(失败原因)。
    ///
    /// 不并进 `warnings`(那是静态代码检查清单), 也不静默: 用户有权知道"这份策略缺一行说明,
    /// 得自己补" —— 否则详情页空着一块, 他不知道是没做还是坏了。
    #[serde(skip_serializing_if = "Option::is_none")]
    meta_note: Option<String>,
    /// 054 P2: 生效的合约**杠杆上限**(现货为 null)。**前端必须带着它落盘** ——
    /// 提示词里对模型说"必须 ≤ Nx"却在配置里不设上限, 那句话就是空的。
    ///
    /// 055 B1: 落盘位置是 `[backtest].max_leverage`(**上限**), 不是 `[backtest].leverage`
    /// (**实际**)。后者会被实盘启动读走并在交易所真的设上 —— 那是替用户开杠杆。
    #[serde(skip_serializing_if = "Option::is_none")]
    leverage_cap: Option<f64>,
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
    // 054 P2: 本金与杠杆上限落进 `[backtest]` —— 让"生成时的约束"与"以后每次回测的口径"是同一个数。
    // 校验已在 ai-generate 那一步做过(> 0 / ∈[1,10]), 这里只做搬运; 后端不替用户拍一个数。
    //
    // 055 B1: 档位给的是**上限** → 只写 `max_leverage`; `leverage`(实际杠杆)保持引擎默认 1x。
    // 写实际杠杆等于替用户开杠杆 —— 而实盘启动读的就是 `[backtest].leverage`(run.rs)。
    if req.initial_cash.is_some() || req.max_leverage.is_some() || req.leverage.is_some() {
        let mut bt = config.backtest.clone().unwrap_or_default();
        if let Some(v) = req.initial_cash {
            bt.initial_cash = Some(v);
        }
        if let Some(v) = req.max_leverage {
            bt.max_leverage = Some(v);
        }
        if let Some(v) = req.leverage {
            bt.leverage = Some(v);
        }
        config.backtest = Some(bt);
    }

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

    // 054 D7: 来源标记 —— 落盘**前**先读旧文件, 判断它是否已是 AI 产物。
    // 顺序上必须在 write_strategy_files 之前: 一旦写下去, 旧内容就只剩 .bak 了。
    let instance_toml = dir.join(format!("{name}.toml"));
    let prev_origin_ai =
        std::fs::read_to_string(&instance_toml).map(|s| origin_is_ai(&s)).unwrap_or(false);
    let origin_ai = req.origin.as_deref().map(str::trim) == Some("ai") || prev_origin_ai;

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

    // 054 D7: 标记写在**落盘之后**(引擎不认识这个注释, 由本层补)。
    // 失败不回滚: 脚本与配置都已正确落盘, 为一行注释推翻整次保存得不偿失 ——
    // 但也不静默吞掉, 进服务端日志 + 回包 warnings, 让用户看得见。
    let mut warnings = lint_lua(&req.code);
    if origin_ai {
        if let Err(e) = mark_origin_ai(&deployed.toml_path) {
            tracing::warn!(target: "web", strategy = %name, "来源标记写入失败: {e}");
            warnings.push(format!(
                "这份策略的来源标记没能写入({e}) —— 它会被当作普通策略显示。\
                 代码与配置都已保存成功, 不影响使用"
            ));
        }
    }

    Ok(Json(SaveReply {
        name: name.to_string(),
        market: market.to_string(),
        toml_path: deployed.toml_path.display().to_string(),
        lua_path: deployed.lua_path.display().to_string(),
        backup: deployed.backup.map(|p| p.display().to_string()),
        manifest_path: manifest_written.map(|(p, _)| p.display().to_string()),
        warnings,
    }))
}

/// 054 D7: 给已落盘的实例 TOML 顶部补一行来源注释(**幂等** —— 已有则不重复写)。
fn mark_origin_ai(toml_path: &std::path::Path) -> Result<(), String> {
    let text = std::fs::read_to_string(toml_path)
        .map_err(|e| format!("读 {} 失败: {e}", toml_path.display()))?;
    if origin_is_ai(&text) {
        return Ok(());
    }
    let out = format!("{ORIGIN_AI_MARKER}\n{text}");
    std::fs::write(toml_path, out).map_err(|e| format!("写 {} 失败: {e}", toml_path.display()))
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

/// 054 P2: 结构化投资输入 → (档位约束, 本金)(**纯函数**, 便于单测)。
///
/// 校验口径见 spec §四-2。三条值得单独说明的取舍:
/// 1. `initial_cash` 与 `risk_tolerance` **必填**: 不知道投多少钱、不知道能承受多大波动,
///    档位表就没有锚点; 而给默认值等于替用户选档(D3 明确禁止)。
/// 2. 非自定义档**拒绝**携带自定义数值字段: 静默忽略会造成"我填了 5% 却没生效" ——
///    这是最坏的一种结果, 用户会以为自己被保护着。宁可 400 让他二选一。
/// 3. `min_spacing_pct` 在自定义档下**可选**: 它只对网格/分批挂单类有意义, 逼一个趋势策略
///    的用户编一个网格间距是没有任何信息量的输入(spec §二原写"全部必填", 本处有意收窄)。
#[allow(clippy::too_many_arguments)]
fn resolve_risk_input(
    market: &str,
    initial_cash: Option<f64>,
    risk_tolerance: Option<&str>,
    max_drawdown_pct: Option<f64>,
    max_position_pct: Option<f64>,
    leverage_cap: Option<f64>,
    min_spacing_pct: Option<f64>,
) -> Result<(risk_profile::RiskProfile, f64), String> {
    use risk_profile::{RiskProfile, RiskTier};

    let cash = initial_cash.ok_or_else(|| {
        "缺少 initial_cash(准备投入的本金): 不知道投多少钱, 就无法换算「单笔不超过百分之几」\
         —— 请填一个大于 0 的数字"
            .to_string()
    })?;
    if !cash.is_finite() || cash <= 0.0 {
        return Err(format!("initial_cash(投入本金)必须是大于 0 的有限数, 收到: {cash}"));
    }
    let raw_tier = risk_tolerance.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| {
        format!("缺少 risk_tolerance(风险档位): 可选 {}", RiskTier::accepted().join(" / "))
    })?;
    let tier = RiskTier::parse(raw_tier).ok_or_else(|| {
        format!(
            "risk_tolerance(风险档位)只能是 {}, 收到 '{raw_tier}'",
            RiskTier::accepted().join(" / ")
        )
    })?;

    let has_custom_field = max_drawdown_pct.is_some()
        || max_position_pct.is_some()
        || leverage_cap.is_some()
        || min_spacing_pct.is_some();
    if tier != RiskTier::Custom && has_custom_field {
        return Err(format!(
            "max_drawdown_pct / max_position_pct / leverage_cap / min_spacing_pct 只在 \
             risk_tolerance=custom 下有意义: 当前档位 '{raw_tier}' 的数字由档位表给出, \
             同时传会让人以为填的值生效了(实际不会) —— 请去掉这些字段, 或改用 custom 档"
        ));
    }

    let pct = |name: &str, v: f64| -> Result<f64, String> {
        if !v.is_finite() || v <= 0.0 || v > 100.0 {
            return Err(format!("{name} 需在 (0, 100] 之间, 收到: {v}"));
        }
        Ok(v)
    };

    if tier != RiskTier::Custom {
        // 现货档位表里的 leverage_cap 不适用 —— 渲染时会改成"现货不使用杠杆"。
        return Ok((RiskProfile::of(tier), cash));
    }

    let dd = max_drawdown_pct
        .ok_or_else(|| "custom(自定义)档必须填 max_drawdown_pct(最大回撤目标 %)".to_string())?;
    let dd = pct("max_drawdown_pct", dd)?;
    let pos = max_position_pct
        .ok_or_else(|| "custom(自定义)档必须填 max_position_pct(单笔最大仓位 %)".to_string())?;
    let pos = pct("max_position_pct", pos)?;
    // 现货无杠杆: 给了就是无效输入, 400(spec §四-2 结构性互斥)。
    let lev = if market == "futures" {
        let lv = leverage_cap
            .ok_or_else(|| "custom(自定义)档在合约市场必须填 leverage_cap(杠杆上限)".to_string())?;
        if !lv.is_finite() || !(1.0..=10.0).contains(&lv) {
            return Err(format!(
                "leverage_cap(杠杆上限)需在 [1, 10] 之间(与引擎 max_leverage 同上限), 收到: {lv}"
            ));
        }
        Some(lv)
    } else if leverage_cap.is_some() {
        return Err("现货不使用杠杆: 请不要传 leverage_cap(它只对合约有意义)".to_string());
    } else {
        None
    };
    // 网格间距只在这一个数上做下界校验; 它是可选的(趋势类策略没有"间距"这回事)。
    let spacing = match min_spacing_pct {
        None => None,
        Some(v) => {
            if !v.is_finite() || v <= 0.0 {
                return Err(format!("min_spacing_pct(网格最小间距 %)必须大于 0, 收到: {v}"));
            }
            Some((v, v))
        }
    };
    Ok((
        RiskProfile {
            tier,
            max_drawdown_pct: Some(dd),
            max_position_pct: Some(pos),
            leverage_cap: lev,
            min_spacing_pct: spacing,
            // 用户没表态就不替他放开加仓(与 custom_template 同一安全默认)。
            forbid_one_side_add: true,
            turnover: "由你按下面的数字自行判断",
        },
        cash,
    ))
}

/// 验证回测的入参组装(**纯函数**, 便于离线单测)。
///
/// 抽出来的唯一目的: "本金真的带进回测了"这条必须能被单测钉住 —— 跑回测本身要联网,
/// 只能靠真跑发现(054 T41 就是这样发现的), 而真跑不会天天跑。
///
/// 055 B2 修: `max_leverage` 此前不传(None → 引擎默认 10x), 而保存时档位写的是 2/3/5x ——
/// 于是**合约策略生成时看到的成绩(默认上限)与保存后看到的(档位上限)不是一回事**。
/// 与本金同一条纪律: 用户在生成弹窗里做的每一个结构化选择, 验证回测都必须照同一个口径跑。
#[allow(clippy::too_many_arguments)] // 9 个入参就是 BacktestRunSpec 的字段, 拆成结构体反而多一层无意义的间接
fn verify_spec(
    root: &Path,
    name: &str,
    code: &str,
    pair: &str,
    market: &str,
    days: u32,
    interval: &str,
    initial_cash: f64,
    max_leverage: Option<f64>,
) -> crate::commands::backtest::BacktestRunSpec {
    crate::commands::backtest::BacktestRunSpec {
        root: root.to_path_buf(),
        strategy: name.to_string(),
        pair: Some(pair.to_string()),
        days,
        interval: interval.to_string(),
        start: None,
        end: None,
        market: Some(market.to_string()),
        position_mode: None,
        params: HashMap::new(),
        script_path: None,
        raw_code: Some(code.to_string()),
        fee: None,
        fee_maker: None,
        fee_taker: None,
        slippage_bps: None,
        limit_fill_penetration_bps: None,
        // 054 T41: 用**用户填的本金**跑, 不是默认 10 万 —— 见 [`verify_generated`] 的说明。
        cash: Some(initial_cash),
        // 055 B2: 档位上限照实带进验证回测(现货恒为 None —— 现货没有杠杆这件事要说清楚)。
        // 实际杠杆仍是默认 1x: 与保存后的 TOML 口径完全一致(那边也只写 max_leverage)。
        max_leverage,
        leverage: None,
        mmr_pct: None,
        funding_rate: None,
        klines_market: None,
        // 生成弹窗里选的周期必须压过 Lua 自己的 fallback, 否则验证的粒度不是用户以为的粒度。
        interval_explicit: true,
        close_at_end: false,
        export_dir: None,
    }
}

/// 054 P1: 给**尚未落盘**的 AI 产物跑一次真实 K 线沙箱回测。
///
/// 走的是与 `POST /api/backtest` **同一个** [`crate::commands::backtest::run_backtest_full`]
/// (`BacktestRunSpec.raw_code` 分支), 于是 K 线拉取 / 预热 / 撮合 / 指标 / 图表口径零分叉。
/// 刻意不去落盘再回测 —— 那样用户会先看到一个从未在任何历史数据上跑过的策略文件。
///
/// **任何失败都返回 `ok:false` 而不 `Err`**: 生成本身已经成功, 网络抖动不该吃掉用户等了
/// 几十秒的产物。调用方把 `verify` 原样塞进回包即可。
///
/// 054 T41 真跑发现(并修): 此前 `cash` 传 `None`, 验证回测一律按**默认 10 万**跑; 而给模型的
/// 提示词说的是"用户准备投入 {initial_cash} USDT"。于是用户填 1 万、看到的是 10 万的成交与盈亏
/// —— 仓位是否拒单、手续费占比、网格能不能铺满, 在小资金下完全是另一回事。这与 spec §四-2
/// 「`initial_cash` 是唯一真下场的数字…让回测口径与用户真实资金一致」直接冲突。现在按用户本金跑:
/// 小资金铺不开网格会**如实**出现拒单, 这正是用户该在生成阶段就知道的事。
///
/// 参数组装抽成 [`verify_spec`] 纯函数, 让"本金真的带进去了"可以被离线单测锁住
/// (跑回测本身要联网, 测不了)。
///
/// 055 B2: 同一条纪律延伸到杠杆上限 —— `max_leverage` 现在也带进来了, 否则合约策略
/// 生成时看到的是"默认 10x 上限下的成绩", 保存后跑的却是"档位 2/3/5x 上限下的成绩"。
#[allow(clippy::too_many_arguments)] // 同上: 与它所组装的 spec 一一对应
async fn verify_generated(
    root: &Path,
    name: &str,
    code: &str,
    pair: &str,
    market: &str,
    days: u32,
    interval: &str,
    initial_cash: f64,
    max_leverage: Option<f64>,
) -> VerifyOutcome {
    let spec =
        verify_spec(root, name, code, pair, market, days, interval, initial_cash, max_leverage);
    // 与三个回测端点同一道保险丝: 内核或后处理 panic 一律变中文错误文本, 不逃逸出请求。
    let result =
        super::backtest_jobs::guarded(crate::commands::backtest::run_backtest_full(spec)).await;
    let out = match result {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return VerifyOutcome {
                ok: false,
                days,
                interval: interval.to_string(),
                report_text: None,
                metrics: None,
                chart: None,
                cost: None,
                tier_check: None,
                reason: Some(super::backtest_jobs::redact_root(&e.to_string(), root)),
            };
        }
        Err(panic_text) => {
            return VerifyOutcome {
                ok: false,
                days,
                interval: interval.to_string(),
                report_text: None,
                metrics: None,
                chart: None,
                cost: None,
                tier_check: None,
                // panic 文本里可能有路径与内部细节, 只回一句可行动的中文 + 简要原因。
                reason: Some(format!("回测内核异常, 本次未能验证: {panic_text}")),
            };
        }
    };
    let text = crate::commands::format_backtest_report(
        &out.report,
        &out.header(name),
        out.initial_cash,
        out.is_futures,
        &out.test_params(),
    );
    let metrics = serde_json::to_value(out.metrics()).ok();
    let chart = serde_json::to_value(out.chart()).ok();
    let cost = super::backtest_jobs::cost_assumption(&out);
    VerifyOutcome {
        ok: true,
        days,
        interval: interval.to_string(),
        report_text: Some(text),
        metrics,
        chart,
        cost: Some(cost),
        tier_check: None,
        reason: None,
    }
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
    // 054 P1: 验证开关 —— 默认开; 天数默认 90(与其余四处回测入口同默认, 换取数字可比)。
    let want_verify = req.verify.unwrap_or(true);
    let verify_days = req.verify_days.unwrap_or(90);
    // 只要**显式给了** verify_days 就校验, 哪怕 verify=false: 传了垃圾说明前端状态不对,
    // 放过去只会让"这个数到底有没有生效"变成悬案。
    if req.verify_days.is_some() && !(1..=MAX_VERIFY_DAYS).contains(&verify_days) {
        return Err(WebError::bad_request(format!(
            "verify_days(验证回测天数)需在 1–{MAX_VERIFY_DAYS} 之间, 收到 {verify_days}"
        )));
    }
    // 054 P2: 结构化投资输入 —— 校验全部在 AI 调用之前(缺一个数字都不该浪费一轮生成)。
    // 解析出的档位约束稍后渲染进提示词; 本金还要带回给前端, 让它落盘时写进 TOML。
    let (risk, initial_cash) = resolve_risk_input(
        market,
        req.initial_cash,
        req.risk_tolerance.as_deref(),
        req.max_drawdown_pct,
        req.max_position_pct,
        req.leverage_cap,
        req.min_spacing_pct,
    )
    .map_err(WebError::bad_request)?;
    let constraints =
        req.constraints.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("无特别约束");
    // 杠杆上限是**唯一可静态验证**的约束 → 回给前端, 由它落盘写进 `[backtest].max_leverage`,
    // 之后真跑回测时由 `validate_leverage` 执行(spec §四-4: 措辞与执行必须一致)。
    //
    // 055 B1: 是 `max_leverage`(上限)不是 `leverage`(实际)。写实际杠杆 = 替用户开杠杆,
    // 而 `commands/run.rs` 启动实盘时读的正是 `[backtest].leverage`, 会真的在交易所设上。
    // 055 B2: 同一个数也要进验证回测, 否则"生成时看到的"与"保存后看到的"是两个口径。
    let leverage_cap = if market == "futures" { risk.leverage_cap } else { None };
    // 同名已存在: 生成结果无法用现名落盘, 当场 409(前端提示换名), 不让用户白等一轮 AI。
    let dir = crate::commands::ensure_strategies_dir_in(&state.root)?;
    if strategy_files_exist(&dir, market, name) {
        return Err(WebError::conflict(
            format!("同名策略 '{name}' 已存在: 请换一个新名字再生成"),
            "exists",
        ));
    }

    // ---- AI 生成(密钥只在 one_shot 内部构造客户端, 不进 prompt) ----
    let risk_text = risk_profile::constraints_text(&risk, market, initial_cash);
    let prompt = crate::ai::provider::build_generate_prompt(
        market,
        pair,
        interval,
        idea,
        constraints,
        &risk_text,
    );
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

    // 054 P1: 生成即验证 —— 同一份**未落盘**的代码跑一次真实 K 线回测。
    // 放在编译门禁之后: 编译都不过的代码没必要浪费一轮网络与撮合。
    //
    // 054 T37: 补摘要/命名那一轮 AI 与验证回测**并行**(`join!`): 两者互不依赖(一个问模型,
    // 一个拉行情), 串行会让用户多等一轮模型推理的时间。
    let (meta, verify) =
        tokio::join!(generate_meta(&state.root, market, pair, interval, idea, &code), async {
            if want_verify {
                Some(
                    verify_generated(
                        &state.root,
                        name,
                        &code,
                        pair,
                        market,
                        verify_days,
                        interval,
                        initial_cash,
                        // 055 B2: 验证回测按**档位上限**跑, 与保存后 TOML 的口径一致。
                        leverage_cap,
                    )
                    .await,
                )
            } else {
                None
            }
        });

    // 055 B3: 档位目标 vs 实测 —— 拿到回测数字之后才核得出来, 所以放在 join 之后。
    // 没有回测(用户跳过验证 / 跑失败)就没有实测值, 此时**不下发**核对框 ——
    // 摆一个"目标 10%、实测 —"的空框等于用界面暗示已经核过。
    let verify = verify.map(|mut v| {
        v.tier_check = risk_profile::tier_check(&risk, v.metrics.as_ref());
        v
    });

    Ok(Json(AiGenerateReply {
        code,
        warnings,
        verify,
        initial_cash,
        leverage_cap,
        summary: meta.summary,
        manifest_params: meta.params,
        meta_note: meta.note,
    }))
}

#[cfg(test)]
mod tests;
