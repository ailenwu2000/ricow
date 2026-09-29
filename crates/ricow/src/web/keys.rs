//! 032 US1 密钥配置端点: `GET/POST /api/config/keys`(FR-006 ~ FR-009)。
//!
//! 与终端向导 / 对话内 `/keys` 写**同一份** `ricow.toml`(经 [`config_file::set_values`],
//! 原子写、权限 0600);读接口**永不回显密钥全文**, 只给"是否已配置 + 脱敏尾号"。

use axum::extract::{rejection::JsonRejection, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::config_file::{self, SetValue};

use super::{WebError, WebState};

// ---- 读响应 (data-model §1) ----

/// 单个密钥字段的脱敏视图:**绝不**带原文。
#[derive(Debug, Serialize, PartialEq, Eq)]
struct SecretView {
    configured: bool,
    hint: Option<String>,
}

#[derive(Debug, Serialize)]
struct AiView {
    /// provider/model/base_url 不是密钥, 原值可回; 未配置给空串。
    provider: String,
    model: String,
    base_url: String,
    api_key: SecretView,
}

#[derive(Debug, Serialize)]
struct MarketView {
    show_all_pairs: bool,
}

/// `GET /api/config/keys` 响应(字段顺序即契约顺序)。
#[derive(Debug, Serialize)]
pub(super) struct KeysReply {
    binance_key: SecretView,
    binance_secret: SecretView,
    demo_key: SecretView,
    demo_secret: SecretView,
    ai: AiView,
    market: MarketView,
}

/// 脱敏提示(FR-007): `***` + 末 4 个字符;原值不足 4 字符时一律固定 `"****"`,
/// 不把"这是个短密钥"这一信息也泄露出去;`None`(未配置)= `configured:false, hint:null`。
fn hint_of(v: Option<&str>) -> SecretView {
    match v {
        None => SecretView { configured: false, hint: None },
        Some(s) if s.chars().count() < 4 => {
            SecretView { configured: true, hint: Some("****".to_string()) }
        }
        Some(s) => {
            // 按字符(而非字节)取末 4 位, 尾部再翻回正序。
            let tail: String =
                s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            SecretView { configured: true, hint: Some(format!("***{tail}")) }
        }
    }
}

/// 把配置文件内存模型翻译成脱敏响应(纯函数, 便于单测)。
fn keys_reply(f: &config_file::File) -> KeysReply {
    let secret = |v: &Option<String>| hint_of(v.as_deref());
    KeysReply {
        binance_key: secret(&f.exchange.binance_key),
        binance_secret: secret(&f.exchange.binance_secret),
        demo_key: secret(&f.exchange.demo_key),
        demo_secret: secret(&f.exchange.demo_secret),
        ai: AiView {
            provider: f.ai.provider.clone(),
            model: f.ai.model.clone().unwrap_or_default(),
            base_url: f.ai.base_url.clone().unwrap_or_default(),
            api_key: secret(&f.ai.api_key),
        },
        market: MarketView { show_all_pairs: f.market.show_all_pairs },
    }
}

/// `GET /api/config/keys`: 只回脱敏状态。文件不存在时 [`config_file::load`] 会先生成模板,
/// 行为与 `GET /api/lang` 同路径。
pub(super) async fn get_keys(State(state): State<WebState>) -> Result<Json<KeysReply>, WebError> {
    let f = config_file::load(&state.root)?;
    Ok(Json(keys_reply(&f)))
}

// ---- 写请求 ----

/// 单条更新的**原始**形态(前端 JSON 直映): `section`/`key` 必须是字符串(serde 强制),
/// `value` 可为字符串 / 布尔 / null, `value_bool` 仅 market 开关使用。
#[derive(Debug, Deserialize)]
struct RawUpdate {
    section: String,
    key: String,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    value_bool: Option<bool>,
}

/// 五个密钥键: 空值(缺省/null/空串)语义 = **本次不修改**, 从更新列表里剔除;
/// 非空才落盘 —— 浏览器回填时密钥框永远是空的, 空框保存不能把已有密钥抹掉。
fn is_secret_key(section: &str, key: &str) -> bool {
    matches!(
        (section, key),
        ("exchange", "binance_key")
            | ("exchange", "binance_secret")
            | ("exchange", "demo_key")
            | ("exchange", "demo_secret")
            | ("ai", "api_key")
    )
}

/// 把原始更新**过滤 + 转换**成 [`SetValue`] 列表(纯函数, 不碰磁盘, 便于单测):
///
/// - 不在 [`config_file::WRITABLE`] 白名单的 (section,key) 一律拒绝(400);
/// - 密钥键空值剔除, 非字符串值拒绝;
/// - `market.show_all_pairs` 走布尔(`value_bool` 或 `value` 布尔), 容错字符串 "true"/"false";
/// - 其余字符串键(ai.provider/model/base_url、ui.lang): 缺省/null/空串 = 清空(写 `""`)。
fn normalize(raw: &[RawUpdate]) -> Result<Vec<(String, String, SetValue)>, String> {
    let mut out = Vec::with_capacity(raw.len());
    for u in raw {
        let (section, key) = (u.section.as_str(), u.key.as_str());
        if !config_file::WRITABLE.contains(&(section, key)) {
            return Err(format!("配置项 [{section}].{key} 不在可写白名单"));
        }

        if (section, key) == ("market", "show_all_pairs") {
            let b = match (u.value_bool, &u.value) {
                (Some(b), _) => b,
                (None, Some(Value::Bool(b))) => *b,
                (None, Some(Value::String(s))) => match s.as_str() {
                    "true" => true,
                    "false" => false,
                    other => {
                        return Err(format!(
                            "[market].show_all_pairs 仅接受 true/false, 实际为 {other:?}"
                        ));
                    }
                },
                (None, Some(other)) => {
                    return Err(format!("[market].show_all_pairs 必须是布尔值, 实际为 {other}"));
                }
                (None, None) => {
                    return Err("[market].show_all_pairs 缺少 value_bool/value".to_string());
                }
            };
            out.push((section.into(), key.into(), SetValue::Bool(b)));
        } else if is_secret_key(section, key) {
            match &u.value {
                // 缺省 / null / 空串: 本次不修改。
                None | Some(Value::Null) | Some(Value::String(_)) => {
                    if let Some(Value::String(s)) = &u.value {
                        if !s.is_empty() {
                            out.push((section.into(), key.into(), SetValue::Str(s.clone())));
                        }
                    }
                }
                Some(other) => {
                    return Err(format!(
                        "[{section}].{key} 是密钥字段, value 必须是字符串, 实际为 {other}"
                    ));
                }
            }
        } else {
            let s = match &u.value {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => {
                    return Err(format!("[{section}].{key} 的 value 必须是字符串, 实际为 {other}"));
                }
            };
            out.push((section.into(), key.into(), SetValue::Str(s)));
        }
    }
    Ok(out)
}

/// `POST /api/config/keys`: 一次性经 [`config_file::set_values`] 落盘(它先全量校验白名单,
/// 再原子写, 不会留下半截内容)。成功 204;密钥空值不修改, 非密钥空值清空。
pub(super) async fn post_keys(
    State(state): State<WebState>,
    // 手动接 Result: JSON 解析失败也要回统一的中文 WebError, 而不是 axum 默认英文拒绝体。
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let items = body.get("updates").and_then(Value::as_array).ok_or_else(|| {
        WebError::bad_request("请求体必须是 {\"updates\":[{section,key,value}...]} 形式")
    })?;

    let mut raw = Vec::with_capacity(items.len());
    for item in items {
        let u: RawUpdate = serde_json::from_value(item.clone()).map_err(|e| {
            WebError::bad_request(format!(
                "updates 条目非法(section/key 必须是字符串, value 类型须正确): {e}"
            ))
        })?;
        raw.push(u);
    }

    // 先过滤转换(非白名单 / 类型错 → 400, 不落任何盘)。
    let updates = normalize(&raw).map_err(WebError::bad_request)?;

    // 与 /api/lang 写路径一致: 先确保配置文件存在(首次运行生成模板)。
    config_file::load(&state.root)?;
    if !updates.is_empty() {
        let refs: Vec<(&str, &str, SetValue)> =
            updates.iter().map(|(s, k, v)| (s.as_str(), k.as_str(), v.clone())).collect();
        // 白名单已在 normalize 全量校验; 此处 Err 只可能是文件 IO(Auth)→ 500 + 中文原文。
        config_file::set_values(&state.root, &refs)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时数据目录(写法照 config_file.rs 既有测试, 本 crate 无 tempfile 依赖)。
    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-web-keys-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn raw(section: &str, key: &str, value: Option<Value>, value_bool: Option<bool>) -> RawUpdate {
        RawUpdate { section: section.into(), key: key.into(), value, value_bool }
    }

    // ---- hint 三态 ----

    #[test]
    fn test_hint_none_short_and_long() {
        assert_eq!(hint_of(None), SecretView { configured: false, hint: None });
        // 不足 4 字符: 一律固定 "****"(不泄露短长度)。
        for short in ["", "a", "ab", "abc"] {
            assert_eq!(
                hint_of(Some(short)),
                SecretView { configured: true, hint: Some("****".into()) },
                "短值 {short:?} 不得透出任何原字符"
            );
        }
        // 恰好 4 位: 给出末 4 位(即全部), 前缀 ***(与契约同形态)。
        assert_eq!(
            hint_of(Some("abcd")),
            SecretView { configured: true, hint: Some("***abcd".into()) }
        );
        // 长值: 只露末 4 位; 按字符取, 不被多字节字符切坏。
        let v = hint_of(Some("live-main-secret-a1b2"));
        assert_eq!(v, SecretView { configured: true, hint: Some("***a1b2".into()) });
        let v = hint_of(Some("密钥-尾号-甲乙丙丁"));
        assert_eq!(v.hint.as_deref(), Some("***甲乙丙丁"));
    }

    // ---- 过滤 + 转换纯函数 ----

    #[test]
    fn test_normalize_drops_empty_secrets_but_keeps_nonempty() {
        let out = normalize(&[
            raw("exchange", "binance_key", None, None),
            raw("exchange", "binance_secret", Some(Value::Null), None),
            raw("exchange", "demo_key", Some(Value::String(String::new())), None),
            raw("ai", "api_key", Some(Value::String("sk-new".into())), None),
        ])
        .unwrap();
        // 前三个空密钥全部剔除, 只剩非空的 ai.api_key。
        assert_eq!(
            out,
            vec![("ai".to_string(), "api_key".to_string(), SetValue::Str("sk-new".into()))]
        );
    }

    #[test]
    fn test_normalize_bool_for_market_switch() {
        let out = normalize(&[raw("market", "show_all_pairs", None, Some(true))]).unwrap();
        assert_eq!(out, vec![("market".into(), "show_all_pairs".into(), SetValue::Bool(true))]);

        // value 直接给布尔也行。
        let out =
            normalize(&[raw("market", "show_all_pairs", Some(Value::Bool(false)), None)]).unwrap();
        assert_eq!(out[0].2, SetValue::Bool(false));

        // 容错字符串 "true"/"false";其他字符串拒绝。
        let out =
            normalize(&[raw("market", "show_all_pairs", Some(Value::String("true".into())), None)])
                .unwrap();
        assert_eq!(out[0].2, SetValue::Bool(true));
        assert!(normalize(&[raw(
            "market",
            "show_all_pairs",
            Some(Value::String("yes".into())),
            None
        )])
        .is_err());
        assert!(normalize(&[raw("market", "show_all_pairs", None, None)]).is_err());
    }

    #[test]
    fn test_normalize_rejects_non_whitelist_and_bad_types() {
        // ai.max_turns 是合法配置但不在 Web 可写白名单 → 拒绝。
        assert!(
            normalize(&[raw("ai", "max_turns", Some(Value::String("8".into())), None)]).is_err()
        );
        // 未知段 / 未知键。
        assert!(normalize(&[raw("bogus", "x", Some(Value::String("v".into())), None)]).is_err());
        // 密钥给非字符串 → 拒绝。
        assert!(
            normalize(&[raw("exchange", "binance_key", Some(Value::Bool(true)), None)]).is_err()
        );
        // 非密钥普通键: null/缺省 = 清空(Str(""))。
        let out = normalize(&[raw("ai", "model", None, None)]).unwrap();
        assert_eq!(out[0].2, SetValue::Str(String::new()));
    }

    // ---- 端到端(临时 ricow.toml) ----

    #[test]
    fn test_get_reply_never_contains_full_secret() {
        let root = tmp_root("mask");
        config_file::ensure_template(&root).unwrap();
        const FULL: &str = "live-main-secret-a1b2";
        config_file::set_values(
            &root,
            &[
                ("exchange", "binance_key", SetValue::Str(FULL.into())),
                ("ai", "provider", SetValue::Str("myproxy".into())),
                ("ai", "base_url", SetValue::Str("https://x.local/v1".into())),
            ],
        )
        .unwrap();

        let f = config_file::load(&root).unwrap();
        let reply = keys_reply(&f);
        let json = serde_json::to_string(&reply).unwrap();
        // 明文全文不得出现在响应里;只给 configured + 尾号。
        assert!(!json.contains(FULL), "GET 响应不得包含密钥全文: {json}");
        assert!(json.contains(r#""configured":true"#), "{json}");
        assert!(json.contains(r#""hint":"***a1b2""#), "尾号提示应正确: {json}");
        // 非密钥字段原值回显。
        assert!(json.contains(r#""provider":"myproxy""#), "{json}");
        assert!(json.contains(r#""base_url":"https://x.local/v1""#), "{json}");
        // 未配置的 demo 凭据: false + null。
        assert!(json.contains(r#""demo_key":{"configured":false,"hint":null}"#), "{json}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_empty_secret_update_does_not_overwrite_existing() {
        let root = tmp_root("keep-secret");
        config_file::ensure_template(&root).unwrap();
        // 先配好一把 AI key 与一把 binance key。
        config_file::set_values(
            &root,
            &[
                ("ai", "api_key", SetValue::Str("sk-original-9f0c".into())),
                ("exchange", "binance_key", SetValue::Str("bk-original-0001".into())),
            ],
        )
        .unwrap();

        // 模拟前端"密钥框留空 = 不改", 同时改另一个非密钥字段。
        let updates = normalize(&[
            raw("ai", "api_key", Some(Value::String(String::new())), None),
            raw("exchange", "binance_key", None, None),
            raw("ai", "model", Some(Value::String("m-1".into())), None),
        ])
        .unwrap();
        let refs: Vec<(&str, &str, SetValue)> =
            updates.iter().map(|(s, k, v)| (s.as_str(), k.as_str(), v.clone())).collect();
        config_file::set_values(&root, &refs).unwrap();

        let f = config_file::load(&root).unwrap();
        assert_eq!(f.ai.api_key.as_deref(), Some("sk-original-9f0c"), "空密钥提交不得抹掉原值");
        assert_eq!(
            f.exchange.binance_key.as_deref(),
            Some("bk-original-0001"),
            "缺省密钥提交不得抹掉原值"
        );
        assert_eq!(f.ai.model.as_deref(), Some("m-1"), "同批的非密钥更新应正常落盘");
        let _ = std::fs::remove_dir_all(&root);
    }
}
