//! **唯一配置文件** `$RICOW_ROOT/ricow.toml`(权限 0600, 已在 .gitignore 里)。
//!
//! 设计(019): 用户只需理解**一个文件** —— 用编辑器直接改, 没有任何"写配置"的命令。
//! - 竞品同做法: freqtrade 把 api key 放在 `config.json`; LiteLLM 把 `api_key` 放在模型条目里;
//!   aider 允许把 `openai-api-key` 写进 `.aider.conf.yml`。**密钥与它的设置放在同一处**,
//!   避免"provider 在一个文件、密钥在另一个文件"来回对照。
//! - 本文件含密钥 → **权限 0600**, 且不入 git。
//! - 键名未知/段名未知 → **硬失败**(拼错不静默失效)。
//! - 文件不存在时按需生成带注释的模板, 且**绝不覆盖已有文件**。

use std::path::{Path, PathBuf};

use ricow_core::{CoreError, CoreResult};

/// 配置文件名(位于 `$RICOW_ROOT/`)。
pub const FILE: &str = "ricow.toml";

/// 配置文件路径。
pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// AI 段(provider 与其密钥同处一段, 一眼看清"这把 key 属于谁")。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSection {
    pub provider: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_turns: Option<i64>,
    pub api_key: Option<String>,
}

impl Default for AiSection {
    fn default() -> Self {
        Self {
            provider: crate::ai::config::DEFAULT_PROVIDER.to_string(),
            model: None,
            base_url: None,
            max_turns: None,
            api_key: None,
        }
    }
}

/// 交易所段: 演示(测试网) 与 实盘(主网) 两套凭据, 用哪个环境填哪个。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExchangeSection {
    pub demo_key: Option<String>,
    pub demo_secret: Option<String>,
    pub binance_key: Option<String>,
    pub binance_secret: Option<String>,
}

/// 整个配置文件的内存表示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct File {
    pub ai: AiSection,
    pub exchange: ExchangeSection,
}

/// `ensure_template` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateOutcome {
    Created(PathBuf),
    Existed(PathBuf),
}

/// 模板正文(带注释; 写清每个字段去哪拿)。合法 TOML, 值为空 = 未填写。
pub fn template_text() -> String {
    let presets = crate::ai::config::preset_menu();
    format!(
        "# ricow 配置文件(本机私有, 权限 0600; 已在 .gitignore —— 含密钥, 不要外传/提交)。\n\
         # 直接用编辑器改本文件即可 —— 没有任何命令用来写配置, 产品不代持你的密钥。\n\
         # 键名/段名拼错会被拒绝(不会静默失效); 值为空 = 未填写。\n\
         \n\
         # ── ① AI 助手通道 ──────────────────────────────────────────────\n\
         # provider: 可写内置预设名, 也可写任意自定义名(自建/中转端点, 此时必须写 base_url)。\n\
         #   内置预设: {presets}\n\
         #   注意: 所选模型必须支持 function calling(工具调用), 否则 AI 无法调用行情/回测等工具。\n\
         # api_key : 上面 provider 的密钥(谁家的 key 就贴在这儿, 两者挨着, 不会搞混)。\n\
         #   本机端点(ollama)免密钥。也支持环境变量 RICOW_AI_API_KEY 临时覆盖。\n\
         [ai]\n\
         provider = \"deepseek\"\n\
         model = \"deepseek-chat\"\n\
         api_key = \"\"\n\
         max_turns = 8\n\
         # base_url = \"https://api.deepseek.com/v1\"   # 仅自定义/自建端点才需要\n\
         \n\
         # ── ② 交易所凭据(用哪个环境就填哪个) ────────────────────────────\n\
         #   演示(测试网): demo.binance.com 登录 → API 管理 → 创建 Key(建议只开交易, 不开提现)\n\
         #   实盘(主网)  : 币安主网 API 管理创建(建议只开交易并关闭提现; 优先用子账户/受限 Key)\n\
         [exchange]\n\
         demo_key = \"\"\n\
         demo_secret = \"\"\n\
         binance_key = \"\"\n\
         binance_secret = \"\"\n"
    )
}

/// 文件不存在时生成模板(带注释), **绝不覆盖已有文件**。
pub fn ensure_template(root: &Path) -> CoreResult<TemplateOutcome> {
    let p = path(root);
    if p.exists() {
        return Ok(TemplateOutcome::Existed(p));
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CoreError::Exchange(format!("创建 {} 失败: {e}", dir.display())))?;
    }
    write_private(&p, &template_text())?;
    Ok(TemplateOutcome::Created(p))
}

/// 原子写 + 0600(unix)。
fn write_private(path: &Path, body: &str) -> CoreResult<()> {
    let tmp = path.with_extension("toml.tmp");
    {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", tmp.display())))?;
            f.write_all(body.as_bytes())
                .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", tmp.display())))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp, body)
                .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", tmp.display())))?;
        }
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| CoreError::Exchange(format!("落盘 {} 失败: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

const AI_KEYS: [&str; 5] = ["provider", "model", "base_url", "max_turns", "api_key"];
const EXCHANGE_KEYS: [&str; 4] = ["demo_key", "demo_secret", "binance_key", "binance_secret"];

/// 读取配置; **文件不存在 → 生成模板并按内置默认继续**(缺什么由使用处给出可执行提示)。
pub fn load(root: &Path) -> CoreResult<File> {
    let p = path(root);
    if !p.exists() {
        ensure_template(root)?;
        return Ok(File::default());
    }
    let text = std::fs::read_to_string(&p)
        .map_err(|e| CoreError::Auth(format!("读取配置文件 {} 失败: {e}", p.display())))?;
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| CoreError::Auth(format!("配置文件 {} 不是合法 TOML: {e}", p.display())))?;

    let mut out = File::default();
    for (section, value) in &table {
        let t = value.as_table().ok_or_else(|| {
            CoreError::Auth(format!(
                "配置文件 {} 的 `{section}` 不是段(table): 配置请按 [ai] / [exchange] 分段书写",
                p.display()
            ))
        })?;
        match section.as_str() {
            "ai" => {
                check_keys(&p, "ai", t, &AI_KEYS)?;
                if let Some(v) = t.get("provider").and_then(|v| v.as_str()) {
                    out.ai.provider = v.trim().to_string();
                }
                out.ai.model = str_opt(t, "model");
                out.ai.base_url = str_opt(t, "base_url");
                out.ai.api_key = str_opt(t, "api_key");
                out.ai.max_turns = t.get("max_turns").and_then(|v| v.as_integer());
            }
            "exchange" => {
                check_keys(&p, "exchange", t, &EXCHANGE_KEYS)?;
                out.exchange.demo_key = str_opt(t, "demo_key");
                out.exchange.demo_secret = str_opt(t, "demo_secret");
                out.exchange.binance_key = str_opt(t, "binance_key");
                out.exchange.binance_secret = str_opt(t, "binance_secret");
            }
            other => {
                return Err(CoreError::Auth(format!(
                    "配置文件 {} 里有未知段 `[{other}]`; 允许的段: [ai] / [exchange]",
                    p.display()
                )))
            }
        }
    }
    if out.ai.provider.trim().is_empty() {
        out.ai.provider = crate::ai::config::DEFAULT_PROVIDER.to_string();
    }
    Ok(out)
}

fn check_keys(p: &Path, section: &str, t: &toml::Table, allowed: &[&str]) -> CoreResult<()> {
    for (k, v) in t {
        if !allowed.contains(&k.as_str()) {
            return Err(CoreError::Auth(format!(
                "配置文件 {} 的 [{section}] 段里有未知键 `{k}`; 允许的键: {}",
                p.display(),
                allowed.join(", ")
            )));
        }
        if !v.is_str() && k != "max_turns" {
            return Err(CoreError::Auth(format!(
                "配置文件 {} 的 [{section}].{k} 必须是字符串(如 {k} = \"...\")",
                p.display()
            )));
        }
    }
    Ok(())
}

fn str_opt(t: &toml::Table, key: &str) -> Option<String> {
    t.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 权限提示(只提示不修改): 非 0600 时返回一句提醒。
pub fn permission_warning(root: &Path) -> Option<String> {
    let p = path(root);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p).ok()?.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Some(format!(
                "配置文件 {} 权限是 {:o}(含密钥, 建议 0600); 可执行: chmod 600 {}",
                p.display(),
                mode,
                p.display()
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(root: &Path, body: &str) {
        std::fs::write(path(root), body).unwrap();
    }

    #[test]
    fn test_path_is_single_file_under_root() {
        assert_eq!(path(Path::new("/tmp/x")), PathBuf::from("/tmp/x/ricow.toml"));
    }

    #[test]
    fn test_missing_file_generates_template_and_uses_defaults() {
        let root = tmp_root("gen");
        let f = load(&root).unwrap();
        assert!(path(&root).exists(), "缺文件时应生成模板");
        assert_eq!(f.ai.provider, crate::ai::config::DEFAULT_PROVIDER);
        assert_eq!(f.ai.api_key, None);
        assert!(f.exchange.binance_key.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_template_is_valid_toml_and_round_trips() {
        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        assert!(t.contains_key("ai") && t.contains_key("exchange"));
        let root = tmp_root("tmpl");
        write(&root, &template_text());
        let f = load(&root).unwrap();
        assert_eq!(f.ai.provider, "deepseek");
        assert_eq!(f.ai.model.as_deref(), Some("deepseek-chat"));
        assert_eq!(f.ai.max_turns, Some(8));
        assert_eq!(f.ai.api_key, None, "模板里 api_key 为空 = 未填写");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_provider_and_key_live_in_same_section() {
        let root = tmp_root("same");
        write(
            &root,
            "[ai]\nprovider = \"myproxy\"\nmodel = \"gpt-4o-mini\"\nbase_url = \"https://x.local/v1\"\napi_key = \"sk-abc\"\n",
        );
        let f = load(&root).unwrap();
        assert_eq!(f.ai.provider, "myproxy");
        assert_eq!(f.ai.api_key.as_deref(), Some("sk-abc"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_exchange_section_read() {
        let root = tmp_root("exch");
        write(&root, "[exchange]\ndemo_key = \"D\"\nbinance_key = \"B\"\n");
        let f = load(&root).unwrap();
        assert_eq!(f.exchange.demo_key.as_deref(), Some("D"));
        assert_eq!(f.exchange.binance_key.as_deref(), Some("B"));
        assert_eq!(f.exchange.binance_secret, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_unknown_key_rejected() {
        let root = tmp_root("badkey");
        write(&root, "[ai]\nprovder = \"deepseek\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("未知键 `provder`"), "{err}");
        assert!(err.contains("provider"), "应列出允许的键: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_unknown_section_rejected() {
        let root = tmp_root("badsec");
        write(&root, "[aii]\nprovider = \"deepseek\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("未知段 `[aii]`"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_ensure_template_never_overwrites() {
        let root = tmp_root("keep");
        write(&root, "[ai]\nprovider = \"mine\"\n");
        let out = ensure_template(&root).unwrap();
        assert!(matches!(out, TemplateOutcome::Existed(_)));
        assert_eq!(load(&root).unwrap().ai.provider, "mine");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_file_is_0600_and_warns_otherwise() {
        let root = tmp_root("perm");
        ensure_template(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&root)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            std::fs::set_permissions(path(&root), std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(permission_warning(&root).unwrap().contains("0600"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_corrupt_toml_error_includes_path() {
        let root = tmp_root("corrupt");
        write(&root, "[ai\nprovider = \n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("ricow.toml"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
