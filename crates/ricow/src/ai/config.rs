//! AI 设置 (019): 供应商预设 + 与密钥同处一处的配置读取。
//!
//! 配置唯一来源 = `$RICOW_ROOT/ricow.toml` 的 `[ai]` 段(见 `commands::config_file`):
//! `provider` / `model` / `base_url` / `max_turns` / **`api_key`(密钥与 provider 挨着, 不会搞混)**。
//! - 本模块负责: 预设表(provider → 默认 base_url/model) + 三层取值(env 覆盖 > 配置 > 预设) + 校验。
//! - 环境变量 `RICOW_AI_API_KEY` / `RICOW_AI_BASE_URL` / `RICOW_AI_MODEL` 仅作**显式临时覆盖**。
//! - 非法配置**报错并给解法**, 不静默回落。

use std::path::Path;

use ricow_core::{CoreError, CoreResult};

/// 未显式指定 provider 时使用的默认供应商(与预设表首项一致)。
pub const DEFAULT_PROVIDER: &str = "deepseek";

/// 预设菜单(报错与模板注释用): `deepseek(DeepSeek 推荐), moonshot(Kimi / Moonshot), ...`。
pub fn preset_menu() -> String {
    PRESETS.iter().map(|p| format!("{}({})", p.id, p.label)).collect::<Vec<_>>().join(", ")
}

/// 密钥环境变量 (兜底)。
pub const ENV_API_KEY: &str = "RICOW_AI_API_KEY";
/// 临时覆盖 base_url (冒烟/换端点用; 优先于配置文件)。
pub const ENV_BASE_URL: &str = "RICOW_AI_BASE_URL";
/// 临时覆盖模型名 (优先于配置文件)。
pub const ENV_MODEL: &str = "RICOW_AI_MODEL";
/// 默认单次对话最多模型轮次 (工具循环上限, 成本护栏)。
pub const DEFAULT_MAX_TURNS: usize = 8;
/// `max_turns` 上限 (防误配成天文数字烧钱)。
pub const MAX_MAX_TURNS: usize = 32;

/// 供应商预设。`model` 为推荐模型(取自各官方文档/rig 常量); 所选模型必须支持 function calling。
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
}

/// 预设表(顺序即引导页顺序, 首项为默认推荐)。
/// 模型名来源: rig-core 0.42 provider 常量(deepseek/moonshot/zai)与官方 OpenAI 兼容端点
/// (openai / openrouter / dashscope 兼容模式 / 本地 ollama); 端点连通性已实测(401/403=需鉴权即存活)。
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "deepseek",
        label: "DeepSeek 推荐",
        base_url: "https://api.deepseek.com/v1",
        model: "deepseek-chat",
    },
    Preset {
        id: "moonshot",
        label: "Kimi / Moonshot",
        base_url: "https://api.moonshot.cn/v1",
        model: "kimi-k2",
    },
    Preset {
        id: "zhipu",
        label: "智谱 GLM",
        base_url: "https://api.z.ai/api/paas/v4",
        model: "glm-4.6",
    },
    Preset {
        id: "qwen",
        label: "通义千问 百炼兼容模式",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        model: "qwen-plus",
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter 聚合",
        base_url: "https://openrouter.ai/api/v1",
        model: "",
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-mini",
    },
    Preset {
        id: "ollama",
        label: "本地 Ollama 可离线",
        base_url: "http://127.0.0.1:11434/v1",
        model: "",
    },
];

/// `ricow.toml` 的 `[ai]` 段内容 (不含密钥)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConfig {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub max_turns: usize,
}

/// 解析后的最终参数(base_url/model 已合成, env 覆盖已应用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub max_turns: usize,
}

pub fn preset_by_id(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// 全部预设 id 的逗号串 (错误文案用)。
pub fn preset_ids() -> String {
    PRESETS.iter().map(|p| format!("{}({})", p.id, p.label)).collect::<Vec<_>>().join(", ")
}

pub fn check_max_turns(n: i64) -> CoreResult<usize> {
    if n < 1 || n > MAX_MAX_TURNS as i64 {
        return Err(CoreError::InvalidArgument(format!(
            "max_turns={n} 非法: 取值范围 1..={MAX_MAX_TURNS}(每轮都是一次模型调用, 直接决定成本); \
             删掉该行或改为 1..={MAX_MAX_TURNS}"
        )));
    }
    Ok(n as usize)
}

/// 合成最终参数: 预设给默认 base_url/模型, 配置文件可覆盖, env 覆盖优先(冒烟/换端点)。
///
/// 规则(全部"报错不回落"):
/// - `provider` 必须命中预设或为 `custom`(custom 必须给 base_url);
/// - `model` 必须非空(预设的推荐模型可用于填空, 但 OpenRouter/Ollama 无推荐值);
/// - env `RICOW_AI_BASE_URL` / `RICOW_AI_MODEL` 覆盖前两者(用于真实冒烟)。
pub fn resolve(
    cfg: &AiConfig,
    env_base_url: Option<String>,
    env_model: Option<String>,
) -> CoreResult<Resolved> {
    // provider 名自由: 命中预设 → 用预设的 base_url/model 默认值; 否则必须自带 base_url
    // (对齐竞品: llm 的 --base-url/api_base、aider 的 OPENAI_API_BASE、LiteLLM 的 api_base)。
    let preset = preset_by_id(&cfg.provider);

    let env_b = env_base_url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let env_m = env_model.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    let base_url = env_b
        .or_else(|| cfg.base_url.clone())
        .or_else(|| preset.map(|p| p.base_url.to_string()))
        .ok_or_else(|| {
            CoreError::InvalidArgument(format!(
                "provider '{}' 不是内置预设(可选: {}) —— 自定义/中转端点请在 ricow.toml 的 [ai] 段写 base_url \
                 (或设 {ENV_BASE_URL}); 密钥键名为 {}_key",
                cfg.provider,
                preset_ids(),
                cfg.provider
            ))
        })?;
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(CoreError::InvalidArgument(format!(
            "base_url '{base_url}' 非法: 需以 http:// 或 https:// 开头"
        )));
    }

    let model = env_m
        .or_else(|| Some(cfg.model.clone()).filter(|s| !s.is_empty()))
        .or_else(|| preset.map(|p| p.model.to_string()).filter(|s| !s.is_empty()))
        .ok_or_else(|| {
            CoreError::InvalidArgument(format!(
                "provider={} 需要模型名: 在 ricow.toml 的 [ai] 段写 model 或设 {ENV_MODEL}(该预设无推荐模型)",
                cfg.provider
            ))
        })?;

    Ok(Resolved { provider: cfg.provider.clone(), base_url, model, max_turns: cfg.max_turns })
}

/// 取 AI 密钥: 环境变量(显式覆盖, 适合 CI/临时) → 配置文件 `[ai].api_key`。
/// provider 与密钥同处一段(见 `commands::config_file`), 不会出现"不知道这把 key 属于谁"。
pub fn api_key(root: &Path, provider: &str) -> CoreResult<String> {
    if let Some(v) =
        std::env::var(ENV_API_KEY).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    {
        return Ok(v);
    }
    let f = crate::commands::config_file::load(root)?;
    f.ai.api_key.clone().ok_or_else(|| {
        CoreError::Auth(format!(
            "配置文件 {} 的 [ai].api_key 尚未填写(provider = {provider})。\n\
             自建/中转端点请同时写 [ai].base_url; 也可临时设环境变量 {ENV_API_KEY}",
            crate::commands::config_file::path(root).display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: &str, model: &str) -> AiConfig {
        AiConfig {
            provider: provider.into(),
            model: model.into(),
            base_url: None,
            max_turns: DEFAULT_MAX_TURNS,
        }
    }

    #[test]
    fn test_max_turns_bounds_rejected_not_silently_fixed() {
        for bad in [0i64, 33, -1] {
            let err = check_max_turns(bad).unwrap_err().to_string();
            assert!(err.contains("max_turns"), "{err}");
            assert!(err.contains("0..=32") || err.contains("1..=32"), "{err}");
        }
        assert_eq!(check_max_turns(DEFAULT_MAX_TURNS as i64).unwrap(), DEFAULT_MAX_TURNS);
    }

    #[test]
    fn test_resolve_uses_preset_defaults() {
        let r = resolve(&cfg("deepseek", ""), None, None).unwrap();
        assert_eq!(r.base_url, "https://api.deepseek.com/v1");
        assert_eq!(r.model, "deepseek-chat", "空模型应取预设推荐值");
    }

    #[test]
    fn test_resolve_env_overrides_file_and_preset() {
        let mut c = cfg("deepseek", "deepseek-chat");
        c.base_url = Some("https://my-proxy.local/v1".into());
        let r = resolve(&c, Some("http://127.0.0.1:11434/v1".into()), Some("qwen3:0.6b".into()))
            .unwrap();
        assert_eq!(r.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(r.model, "qwen3:0.6b");
        // 配置文件 base_url 优先于预设
        let r2 = resolve(&c, None, None).unwrap();
        assert_eq!(r2.base_url, "https://my-proxy.local/v1");
    }

    #[test]
    fn test_resolve_custom_provider_needs_base_url() {
        let err = resolve(&cfg("myproxy", "m"), None, None).unwrap_err().to_string();
        assert!(err.contains("base_url"), "{err}");
        let mut c = cfg("myproxy", "m");
        c.base_url = Some("https://x.local/v1".into());
        assert_eq!(resolve(&c, None, None).unwrap().base_url, "https://x.local/v1");
    }

    #[test]
    fn test_resolve_rejects_unknown_provider_and_bad_url_and_empty_model() {
        // 非预设 provider 名不再被拒, 但必须自填 base_url, 且报错要列出可选预设
        let err = resolve(&cfg("mistral", "m"), None, None).unwrap_err().to_string();
        assert!(err.contains("base_url"), "{err}");
        assert!(err.contains("deepseek"), "报错应列出可选预设: {err}");

        let mut c = cfg("myproxy", "m");
        c.base_url = Some("ftp://x/v1".into());
        assert!(resolve(&c, None, None).unwrap_err().to_string().contains("http://"));

        // OpenRouter / Ollama 无推荐模型 → 必须让用户填
        let err = resolve(&cfg("openrouter", ""), None, None).unwrap_err().to_string();
        assert!(err.contains("model") || err.contains("模型"), "{err}");
    }
}
