//! 033 密钥管理端点: 密钥环总览 + 各套密钥的保存 / 选用 / 删除 + 清除生效凭据。
//!
//! 模型(见 `specs/changes/033-key-manager/plan.md` D2 / D3):
//! - **密钥环** = `ricow.toml` 的 `[[ai_key]]` / `[[exchange_key]]`(可存多套, 每套带别名);
//! - `[ai]` / `[exchange]` 段仍是**唯一**的"当前生效凭据", 字段一个不加 —— 于是运行期读凭据的
//!   代码(`ai::config` / `supervisor` / 会话)一行都不用改;
//! - **选用** = 把某套的值写进生效段(复制)。代价是同一把密钥在文件里出现两次, 换来零运行期风险。
//!
//! 安全(与 032 FR-007 同一条契约): 读接口只回 `{configured, hint}`(末 4 位), 密钥全文
//! 从不出现在任何响应里; 密钥框留空 = 本次不修改(FR-011)。

use axum::extract::{rejection::JsonRejection, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::config::{DEFAULT_PROVIDER, PRESETS};
use crate::commands::config_file::{
    self, AiKeyEntry, ExchangeKeyEntry, SetValue, ENV_DEMO, ENV_LIVE,
};

use super::keys::{hint_of, SecretView};
use super::{WebError, WebState};

// ---- 读响应 ----

/// 一套 AI 密钥的脱敏视图。
#[derive(Debug, Serialize, PartialEq, Eq)]
struct AiEntryView {
    alias: String,
    provider: String,
    model: String,
    base_url: String,
    api_key: SecretView,
    /// 该套正是当前生效的凭据(判定见 [`ai_entry_is_active`])—— 页面据此打"使用中"徽标。
    active: bool,
}

/// 一套币安密钥的脱敏视图。
#[derive(Debug, Serialize, PartialEq, Eq)]
struct ExchangeEntryView {
    alias: String,
    /// `live`(实盘主网) / `demo`(测试网)。
    env: String,
    key: SecretView,
    secret: SecretView,
    active: bool,
}

/// `[ai]` 生效段的脱敏视图。
#[derive(Debug, Serialize, PartialEq, Eq)]
struct AiCurrentView {
    provider: String,
    model: String,
    base_url: String,
    api_key: SecretView,
    /// 生效凭据来自密钥环的哪一套(别名); `None` = 不来自任何条目(手工填写或未配置)。
    from_entry: Option<String>,
}

/// `[exchange]` 生效段的脱敏视图。
#[derive(Debug, Serialize, PartialEq, Eq)]
struct ExchangeCurrentView {
    binance_key: SecretView,
    binance_secret: SecretView,
    demo_key: SecretView,
    demo_secret: SecretView,
    live_from: Option<String>,
    demo_from: Option<String>,
}

/// 服务商预设(只读下发): 前端下拉直接用它, 不在 JS 里再抄一份预设表。
#[derive(Debug, Serialize, PartialEq, Eq)]
struct PresetView {
    id: &'static str,
    label: &'static str,
    base_url: &'static str,
    model: &'static str,
}

#[derive(Debug, Serialize)]
struct AiGroup {
    entries: Vec<AiEntryView>,
    current: AiCurrentView,
}

#[derive(Debug, Serialize)]
struct ExchangeGroup {
    entries: Vec<ExchangeEntryView>,
    current: ExchangeCurrentView,
}

/// `GET /api/keys` 响应。
#[derive(Debug, Serialize)]
pub(super) struct VaultReply {
    ai: AiGroup,
    exchange: ExchangeGroup,
    presets: Vec<PresetView>,
}

/// 归一化比较: `None` 与空串对用户是同一件事("没写" == "写了空串"), 比较前统一成空串再比。
fn same_opt(a: Option<&str>, b: Option<&str>) -> bool {
    a.unwrap_or("").trim() == b.unwrap_or("").trim()
}

/// 生效段是否"确实配了东西"。用于挡掉一个边界: 全新安装时模板写的就是
/// `provider = "deepseek"`, 若某套条目恰好也是"deepseek + 全空", 两者会撞成"使用中"的假象。
fn ai_has_credential(ai: &config_file::AiSection) -> bool {
    ai.api_key.is_some() || ai.base_url.is_some() || ai.provider != DEFAULT_PROVIDER
}

/// AI 条目是否"使用中": provider/model/base_url/api_key 四元组与生效段全等(D4 值比对, 不落标志位)。
fn ai_entry_is_active(e: &AiKeyEntry, ai: &config_file::AiSection) -> bool {
    ai_has_credential(ai)
        && e.provider == ai.provider
        && same_opt(Some(e.model.as_str()), ai.model.as_deref())
        && same_opt(Some(e.base_url.as_str()), ai.base_url.as_deref())
        && same_opt(e.api_key.as_deref(), ai.api_key.as_deref())
}

/// 币安条目是否"使用中": 按该条声明的环境, 与对应生效字段的 key + secret 同时相等。
fn exchange_entry_is_active(e: &ExchangeKeyEntry, ex: &config_file::ExchangeSection) -> bool {
    if e.key.is_none() {
        return false;
    }
    let (k, s) = if e.env == ENV_DEMO {
        (&ex.demo_key, &ex.demo_secret)
    } else {
        (&ex.binance_key, &ex.binance_secret)
    };
    same_opt(e.key.as_deref(), k.as_deref()) && same_opt(e.secret.as_deref(), s.as_deref())
}

/// 把配置文件翻译成脱敏总览(纯函数, 便于单测)。
fn vault_reply(f: &config_file::File) -> VaultReply {
    let entries: Vec<AiEntryView> = f
        .ai_keys
        .iter()
        .map(|e| AiEntryView {
            alias: e.alias.clone(),
            provider: e.provider.clone(),
            model: e.model.clone(),
            base_url: e.base_url.clone(),
            api_key: hint_of(e.api_key.as_deref()),
            active: ai_entry_is_active(e, &f.ai),
        })
        .collect();
    let from_entry = entries.iter().find(|v| v.active).map(|v| v.alias.clone());

    let ex_entries: Vec<ExchangeEntryView> = f
        .exchange_keys
        .iter()
        .map(|e| ExchangeEntryView {
            alias: e.alias.clone(),
            env: e.env.clone(),
            key: hint_of(e.key.as_deref()),
            secret: hint_of(e.secret.as_deref()),
            active: exchange_entry_is_active(e, &f.exchange),
        })
        .collect();
    // "生效来自哪一套"按环境各自找, 与"使用中"徽标的判定完全同一份逻辑。
    let from_of =
        |env: &str| ex_entries.iter().find(|v| v.active && v.env == env).map(|v| v.alias.clone());
    let (live_from, demo_from) = (from_of(ENV_LIVE), from_of(ENV_DEMO));

    VaultReply {
        ai: AiGroup {
            entries,
            current: AiCurrentView {
                provider: f.ai.provider.clone(),
                model: f.ai.model.clone().unwrap_or_default(),
                base_url: f.ai.base_url.clone().unwrap_or_default(),
                api_key: hint_of(f.ai.api_key.as_deref()),
                from_entry,
            },
        },
        exchange: ExchangeGroup {
            entries: ex_entries,
            current: ExchangeCurrentView {
                binance_key: hint_of(f.exchange.binance_key.as_deref()),
                binance_secret: hint_of(f.exchange.binance_secret.as_deref()),
                demo_key: hint_of(f.exchange.demo_key.as_deref()),
                demo_secret: hint_of(f.exchange.demo_secret.as_deref()),
                live_from,
                demo_from,
            },
        },
        presets: PRESETS
            .iter()
            .map(|p| PresetView { id: p.id, label: p.label, base_url: p.base_url, model: p.model })
            .collect(),
    }
}

/// `GET /api/keys`: 密钥环 + 生效状态 + 预设表, 全部脱敏。
pub(super) async fn get_keys(State(state): State<WebState>) -> Result<Json<VaultReply>, WebError> {
    let f = config_file::load(&state.root)?;
    Ok(Json(vault_reply(&f)))
}

// ---- 写请求 ----

/// 请求体解析: JSON 语法错也给统一的中文 `WebError`, 而不是 axum 默认的英文拒绝体。
fn body(payload: Result<Json<Value>, JsonRejection>) -> Result<Value, WebError> {
    payload.map(|Json(v)| v).map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))
}

/// 把 `Value` 反序列化成请求结构, 失败 → 400 + 中文一句。
fn parse<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, WebError> {
    serde_json::from_value(v)
        .map_err(|e| WebError::bad_request(format!("请求体字段非法(类型或缺失): {e}")))
}

/// 别名规范化 + 400(带机器码, 前端据此定位到别名输入框)。
fn alias_or_400(raw: &str) -> Result<String, WebError> {
    config_file::check_alias(raw).map_err(|e| WebError::bad_request_code(e, "invalid_alias"))
}

/// 密钥类字段的取值: 缺省 / null / 空串 = **本次不修改**(FR-011), 非空才作为新值。
fn new_secret(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 在密钥环里按别名找一套 AI 密钥。
fn find_ai<'a>(f: &'a config_file::File, alias: &str) -> Option<&'a AiKeyEntry> {
    f.ai_keys.iter().find(|e| e.alias == alias)
}

/// 在密钥环里按别名找一套币安密钥。
fn find_exchange<'a>(f: &'a config_file::File, alias: &str) -> Option<&'a ExchangeKeyEntry> {
    f.exchange_keys.iter().find(|e| e.alias == alias)
}

/// 重名检查: 除 `keep_alias`(正在改的那条)以外, 是否已存在同名条目。
fn check_dup(
    aliases: impl Iterator<Item = String>,
    alias: &str,
    keep: Option<&str>,
) -> Result<(), WebError> {
    for a in aliases {
        if a == alias && keep != Some(a.as_str()) {
            return Err(WebError::bad_request_code(
                format!("别名「{alias}」已存在 —— 别名是辨认密钥的唯一标识, 请换一个"),
                "dup_alias",
            ));
        }
    }
    Ok(())
}

/// 把一套 AI 密钥写进生效段 `[ai]`(033 FR-005)。`max_turns` 等非本条目字段一律不碰。
fn apply_ai(root: &std::path::Path, e: &AiKeyEntry) -> Result<(), WebError> {
    let key = e.api_key.clone().unwrap_or_default();
    config_file::set_values(
        root,
        &[
            ("ai", "provider", SetValue::Str(e.provider.clone())),
            ("ai", "model", SetValue::Str(e.model.clone())),
            ("ai", "base_url", SetValue::Str(e.base_url.clone())),
            ("ai", "api_key", SetValue::Str(key)),
        ],
    )?;
    Ok(())
}

/// 把一套币安密钥写进生效段 `[exchange]` 的对应环境(033 FR-005)。
fn apply_exchange(root: &std::path::Path, e: &ExchangeKeyEntry) -> Result<(), WebError> {
    let (k, s) = match e.env.as_str() {
        ENV_DEMO => ("demo_key", "demo_secret"),
        _ => ("binance_key", "binance_secret"),
    };
    config_file::set_values(
        root,
        &[
            ("exchange", k, SetValue::Str(e.key.clone().unwrap_or_default())),
            ("exchange", s, SetValue::Str(e.secret.clone().unwrap_or_default())),
        ],
    )?;
    Ok(())
}

// ---- AI 密钥条目 ----

#[derive(Debug, Deserialize)]
struct AiSaveRequest {
    alias: String,
    /// 改名前的旧别名(不传 = 新增或按新别名覆盖)。
    #[serde(default)]
    prev_alias: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    /// 缺省 / 空串 = 保留原密钥。
    #[serde(default)]
    api_key: Option<String>,
}

/// `POST /api/keys/ai/save`: 新增或更新一套 AI 密钥(FR-003 / FR-008 / FR-009)。
pub(super) async fn save_ai(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: AiSaveRequest = parse(body(payload)?)?;
    let alias = alias_or_400(&req.alias)?;
    let prev = match req.prev_alias.as_deref() {
        Some(p) => Some(alias_or_400(p)?),
        None => None,
    };

    let f = config_file::load(&state.root)?;
    let existing = prev.as_deref().and_then(|p| find_ai(&f, p)).or_else(|| find_ai(&f, &alias));
    if let Some(p) = prev.as_deref() {
        if find_ai(&f, p).is_none() {
            return Err(WebError::not_found(format!("密钥列表里没有别名为「{p}」的条目")));
        }
    }
    check_dup(f.ai_keys.iter().map(|e| e.alias.clone()), &alias, prev.as_deref())?;

    // 服务商: 更新时缺省 = 沿用原值; 新增时必填(否则这一套无从解析)。
    let provider = match req.provider.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => match existing {
            Some(e) => e.provider.clone(),
            None => {
                return Err(WebError::bad_request(
                    "新增一套 AI 密钥必须给出服务商(provider): 内置预设名(如 deepseek)或自定义名 + 接口地址",
                ));
            }
        },
    };

    let entry = AiKeyEntry {
        alias: alias.clone(),
        provider,
        model: req
            .model
            .as_deref()
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| existing.map(|e| e.model.clone()))
            .unwrap_or_default(),
        base_url: req
            .base_url
            .as_deref()
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| existing.map(|e| e.base_url.clone()))
            .unwrap_or_default(),
        // 留空 = 不修改: 更新时沿用原密钥; 新增时为空(如本机 ollama 免密钥)。
        api_key: new_secret(req.api_key.as_ref())
            .or_else(|| existing.and_then(|e| e.api_key.clone())),
    };

    // FR-008: 改的正是"使用中"的那一套 → 生效段跟着走, 不留旧值。
    let was_active = existing.map(|e| ai_entry_is_active(e, &f.ai)).unwrap_or(false);

    config_file::upsert_ai_key(&state.root, &entry, prev.as_deref())?;
    if was_active {
        apply_ai(&state.root, &entry)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/keys/ai/use`: 选用某一套为当前生效凭据。
pub(super) async fn use_ai(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: AliasOnly = parse(body(payload)?)?;
    let alias = alias_or_400(&req.alias)?;
    let f = config_file::load(&state.root)?;
    let entry = find_ai(&f, &alias)
        .ok_or_else(|| WebError::not_found(format!("密钥列表里没有别名为「{alias}」的条目")))?;
    apply_ai(&state.root, entry)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/keys/ai/delete`: 删除一套; 若它正在使用中, 一并清空生效密钥(FR-007 幽灵凭据防护)。
pub(super) async fn delete_ai(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: AliasOnly = parse(body(payload)?)?;
    let alias = alias_or_400(&req.alias)?;
    let f = config_file::load(&state.root)?;
    let was_active = find_ai(&f, &alias).map(|e| ai_entry_is_active(e, &f.ai)).unwrap_or(false);

    if !config_file::remove_ai_key(&state.root, &alias)? {
        return Err(WebError::not_found(format!("密钥列表里没有别名为「{alias}」的条目")));
    }
    if was_active {
        // 只清凭据本身: provider/model/base_url 不是密钥, 用户可能还想留着。
        config_file::set_values(&state.root, &[("ai", "api_key", SetValue::Str(String::new()))])?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- 币安密钥条目 ----

#[derive(Debug, Deserialize)]
struct ExchangeSaveRequest {
    alias: String,
    #[serde(default)]
    prev_alias: Option<String>,
    env: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    secret: Option<String>,
}

/// `POST /api/keys/exchange/save`: 新增或更新一套币安凭据(FR-004 / FR-008 / FR-009)。
pub(super) async fn save_exchange(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: ExchangeSaveRequest = parse(body(payload)?)?;
    let alias = alias_or_400(&req.alias)?;
    let prev = match req.prev_alias.as_deref() {
        Some(p) => Some(alias_or_400(p)?),
        None => None,
    };
    let env = config_file::check_env(&req.env)
        .map_err(|e| WebError::bad_request(format!("env 非法: {e}")))?;

    let f = config_file::load(&state.root)?;
    if let Some(p) = prev.as_deref() {
        if find_exchange(&f, p).is_none() {
            return Err(WebError::not_found(format!("密钥列表里没有别名为「{p}」的条目")));
        }
    }
    let existing =
        prev.as_deref().and_then(|p| find_exchange(&f, p)).or_else(|| find_exchange(&f, &alias));
    check_dup(f.exchange_keys.iter().map(|e| e.alias.clone()), &alias, prev.as_deref())?;

    let entry = ExchangeKeyEntry {
        alias: alias.clone(),
        env,
        key: new_secret(req.key.as_ref()).or_else(|| existing.and_then(|e| e.key.clone())),
        secret: new_secret(req.secret.as_ref()).or_else(|| existing.and_then(|e| e.secret.clone())),
    };
    // 币安下单必须 key + secret 成对; 半截凭据不许静默落盘(用户故事 2 场景 6)。
    if entry.key.is_none() || entry.secret.is_none() {
        return Err(WebError::bad_request(
            "币安凭据需要同时提供 API Key 与 API Secret(二者缺一不可)",
        ));
    }

    let was_active = existing.map(|e| exchange_entry_is_active(e, &f.exchange)).unwrap_or(false);

    config_file::upsert_exchange_key(&state.root, &entry, prev.as_deref())?;
    if was_active {
        apply_exchange(&state.root, &entry)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/keys/exchange/use`: 选用某一套为该环境的当前生效凭据。
pub(super) async fn use_exchange(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: AliasOnly = parse(body(payload)?)?;
    let alias = alias_or_400(&req.alias)?;
    let f = config_file::load(&state.root)?;
    let entry = find_exchange(&f, &alias)
        .ok_or_else(|| WebError::not_found(format!("密钥列表里没有别名为「{alias}」的条目")))?;
    apply_exchange(&state.root, entry)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/keys/exchange/delete`: 删除一套; 若它正在使用中, 一并清空该环境的生效凭据。
pub(super) async fn delete_exchange(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: AliasOnly = parse(body(payload)?)?;
    let alias = alias_or_400(&req.alias)?;
    let f = config_file::load(&state.root)?;
    let hit = find_exchange(&f, &alias);
    let (was_active, env) = (
        hit.map(|e| exchange_entry_is_active(e, &f.exchange)).unwrap_or(false),
        hit.map(|e| e.env.clone()),
    );

    if !config_file::remove_exchange_key(&state.root, &alias)? {
        return Err(WebError::not_found(format!("密钥列表里没有别名为「{alias}」的条目")));
    }
    if was_active {
        let (k, s) = if env.as_deref() == Some(ENV_DEMO) {
            ("demo_key", "demo_secret")
        } else {
            ("binance_key", "binance_secret")
        };
        config_file::set_values(
            &state.root,
            &[
                ("exchange", k, SetValue::Str(String::new())),
                ("exchange", s, SetValue::Str(String::new())),
            ],
        )?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- 清除生效凭据 ----

#[derive(Debug, Deserialize)]
struct AliasOnly {
    alias: String,
}

#[derive(Debug, Deserialize)]
struct ClearRequest {
    /// `ai` / `live` / `demo`。
    target: String,
}

/// `POST /api/keys/clear`: 清空**生效凭据**(不动密钥环)。
///
/// 补的是 032 留下的洞: 密钥框"留空 = 不修改"是有意设计, 于是手工填的凭据一直没有清除入口;
/// 用户明确要求"可以删除自己的币安 key", 该动作就在这里。
pub(super) async fn clear(
    State(state): State<WebState>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let req: ClearRequest = parse(body(payload)?)?;
    let pairs: &[(&str, &str)] = match req.target.trim() {
        "ai" => &[("ai", "api_key")],
        ENV_LIVE => &[("exchange", "binance_key"), ("exchange", "binance_secret")],
        ENV_DEMO => &[("exchange", "demo_key"), ("exchange", "demo_secret")],
        other => {
            return Err(WebError::bad_request(format!(
                "target `{other}` 非法: 只接受 \"ai\"(AI 密钥) / \"live\"(币安主网) / \"demo\"(币安测试网)"
            )));
        }
    };
    let updates: Vec<(&str, &str, SetValue)> =
        pairs.iter().map(|(s, k)| (*s, *k, SetValue::Str(String::new()))).collect();
    config_file::set_values(&state.root, &updates)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config_file::{AiSection, ExchangeSection};

    /// 取错误分支: `WebError` 不带 `Debug`(它是 handler 的内部出口), 用不上 `unwrap_err()`。
    fn err_of<T>(r: Result<T, WebError>) -> WebError {
        match r {
            Ok(_) => panic!("本应失败, 却返回成功"),
            Err(e) => e,
        }
    }

    fn ai_entry(alias: &str, key: Option<&str>) -> AiKeyEntry {
        AiKeyEntry {
            alias: alias.into(),
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: String::new(),
            api_key: key.map(str::to_string),
        }
    }

    fn ex_entry(alias: &str, env: &str, key: &str, secret: &str) -> ExchangeKeyEntry {
        ExchangeKeyEntry {
            alias: alias.into(),
            env: env.into(),
            key: Some(key.into()),
            secret: Some(secret.into()),
        }
    }

    // ---- "使用中"判定 ----

    #[test]
    fn test_ai_active_requires_full_tuple_match() {
        let mut ai = AiSection {
            provider: "deepseek".into(),
            api_key: Some("sk-a".into()),
            ..Default::default()
        };
        ai.model = Some("deepseek-flash".into());
        assert!(ai_entry_is_active(&ai_entry("A", Some("sk-a")), &ai));
        // 密钥不同 → 不是使用中。
        assert!(!ai_entry_is_active(&ai_entry("B", Some("sk-b")), &ai));
        // 密钥相同但 provider 不同 → 不是使用中(避免两把同值 key 互相冒充)。
        let mut other = ai.clone();
        other.provider = "moonshot".into();
        assert!(!ai_entry_is_active(&ai_entry("A", Some("sk-a")), &other));
    }

    #[test]
    fn test_ai_active_false_on_pristine_config() {
        // 全新安装: 模板写的 provider=deepseek 且没有密钥 → 任何条目都不该被标成"使用中"。
        let ai = AiSection { provider: DEFAULT_PROVIDER.into(), ..Default::default() };
        assert!(!ai_entry_is_active(&ai_entry("空条目", None), &ai));
    }

    #[test]
    fn test_ai_active_when_keyless_local_endpoint_selected() {
        // 本机 ollama: 无密钥, 但 base_url 已写入 → 仍应认得出"使用中"。
        let mut ai = AiSection { provider: "ollama".into(), ..Default::default() };
        ai.base_url = Some("http://127.0.0.1:11434/v1".into());
        let e = AiKeyEntry {
            alias: "本机".into(),
            provider: "ollama".into(),
            model: String::new(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            api_key: None,
        };
        assert!(ai_entry_is_active(&e, &ai));
    }

    #[test]
    fn test_exchange_active_is_per_env() {
        let ex = ExchangeSection {
            binance_key: Some("LIVE-K".into()),
            binance_secret: Some("LIVE-S".into()),
            demo_key: Some("DEMO-K".into()),
            demo_secret: Some("DEMO-S".into()),
        };
        assert!(exchange_entry_is_active(&ex_entry("主账户", ENV_LIVE, "LIVE-K", "LIVE-S"), &ex));
        assert!(exchange_entry_is_active(&ex_entry("测试网", ENV_DEMO, "DEMO-K", "DEMO-S"), &ex));
        // 环境错位: 把主网 key 写成 demo 条目 → 不算使用中。
        assert!(!exchange_entry_is_active(&ex_entry("错位", ENV_DEMO, "LIVE-K", "LIVE-S"), &ex));
        // secret 不同 → 不算。
        assert!(!exchange_entry_is_active(&ex_entry("半截", ENV_LIVE, "LIVE-K", "X"), &ex));
        // 没 key 的条目 → 不算。
        assert!(!exchange_entry_is_active(
            &ExchangeKeyEntry {
                alias: "空".into(), env: ENV_LIVE.into(), key: None, secret: None
            },
            &ex
        ));
    }

    // ---- 脱敏 ----

    #[test]
    fn test_reply_never_contains_full_secret() {
        const AI_FULL: &str = "sk-live-ai-9f0c24";
        const BN_KEY: &str = "bn-live-key-0001";
        const BN_SECRET: &str = "bn-live-secret-0002";
        let f = config_file::File {
            ai: AiSection {
                provider: "deepseek".into(),
                model: Some("deepseek-flash".into()),
                api_key: Some(AI_FULL.into()),
                ..Default::default()
            },
            ai_keys: vec![
                ai_entry("工作号", Some(AI_FULL)),
                ai_entry("备用", Some("sk-other-1234")),
            ],
            exchange_keys: vec![ex_entry("主账户", ENV_LIVE, BN_KEY, BN_SECRET)],
            exchange: ExchangeSection {
                binance_key: Some(BN_KEY.into()),
                binance_secret: Some(BN_SECRET.into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let json = serde_json::to_string(&vault_reply(&f)).unwrap();
        for secret in [AI_FULL, BN_KEY, BN_SECRET, "sk-other-1234"] {
            assert!(!json.contains(secret), "响应不得包含密钥全文 {secret}: {json}");
        }
        // 只给"已配置 + 末 4 位"。
        assert!(json.contains(r#""hint":"***0c24""#), "{json}");
        assert!(json.contains(r#""hint":"***0001""#), "{json}");
        assert!(json.contains(r#""active":true"#), "{json}");
        // 生效来源与预设表都在。
        assert!(json.contains(r#""from_entry":"工作号""#), "{json}");
        assert!(json.contains(r#""live_from":"主账户""#), "{json}");
        assert!(json.contains(r#""id":"deepseek""#), "{json}");
    }

    #[test]
    fn test_reply_marks_no_entry_when_handwritten() {
        // 手工填的凭据(不来自密钥环) → from_entry 为 null, 页面据此显示"当前生效(未来自密钥列表)"。
        let f = config_file::File {
            ai: AiSection {
                provider: "deepseek".into(),
                api_key: Some("sk-hand".into()),
                ..Default::default()
            },
            ai_keys: vec![ai_entry("别的", Some("sk-x"))],
            ..Default::default()
        };
        let r = vault_reply(&f);
        assert_eq!(r.ai.current.from_entry, None);
        assert!(r.ai.entries.iter().all(|e| !e.active));
    }

    // ---- 字段清洗 ----

    #[test]
    fn test_new_secret_empty_means_keep() {
        assert_eq!(new_secret(None), None);
        assert_eq!(new_secret(Some(&String::new())), None);
        assert_eq!(new_secret(Some(&"  ".to_string())), None);
        assert_eq!(new_secret(Some(&"sk-a".to_string())).as_deref(), Some("sk-a"));
    }

    #[test]
    fn test_check_dup_allows_self_rename() {
        let aliases = || vec!["A".to_string(), "B".to_string()].into_iter();
        assert!(check_dup(aliases(), "C", None).is_ok());
        // 改名保留自己: 目标别名 == 旧别名时不报重复。
        assert!(check_dup(aliases(), "A", Some("A")).is_ok());
        // 撞到别人 → 400 + dup_alias。
        let err = err_of(check_dup(aliases(), "B", Some("A")));
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.body.code.as_deref(), Some("dup_alias"));
    }

    #[test]
    fn test_alias_or_400_carries_machine_code() {
        let err = err_of(alias_or_400("   "));
        assert_eq!(err.body.code.as_deref(), Some("invalid_alias"));
        // 首尾空白被裁掉(用户从聊天窗口粘贴时常带空格)。
        assert!(alias_or_400(" 工作号 ").is_ok());
    }
}
