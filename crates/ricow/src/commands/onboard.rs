//! 首次启动向导 (019-R4) —— 缺密钥时问清楚、静默录入、写回 `ricow.toml`, 然后放用户进对话。
//!
//! 用户视角: 在当前目录运行 `ricow`, 缺什么问什么, 录完就能用自然语言指挥策略。
//! 工程口径:
//! - **只写白名单键**: 落盘走 [`config_file::set_values`](按行外科替换, 保留注释, 原子写, 0600);
//! - **密钥静默输入**: `rpassword`(Windows/Linux/macOS 同一实现), 不回显、不进日志、不进模型上下文;
//! - **有缺口才问**: 密钥已配置(或本机端点/环境变量提供)→ 直接返回, 不打扰;
//! - **非交互式 stdin**(管道/重定向/CI): 裸入口如实报错并给出文件路径, 不静默降级;
//! - **可跳过**: 币安凭据可跳过(用到时在对话里补录); 主网(实盘)凭据向导**不问**。
//!
//! 本模块只做"问 + 写文件", 不建立会话、不联网(除可选的一次最小连通校验)。

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use ricow_core::{CoreError, CoreResult};

use super::config_file::{self, File, SetValue};
use crate::ai::{config as ai_config, provider};

/// 连通校验的等待上限: 超时按"校验失败"处理(用户可选择仍然保存), 不无限挂住向导。
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// 一次向导运行的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 配置齐全, 未打扰用户。
    NotNeeded,
    /// 用户完成/部分完成了录入并落盘。
    Saved,
    /// 用户什么都没写(跳过 / EOF / 非交互式降级)。
    Skipped,
}

/// 需要补齐的缺口(纯数据, 便于单测与打印)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gaps {
    /// 缺 AI 密钥(本机端点或环境变量已提供 → 不算缺口)。
    pub ai_key: bool,
    /// demo(测试网)凭据不完整。
    pub demo_creds: bool,
}

impl Gaps {
    pub fn any(&self) -> bool {
        self.ai_key || self.demo_creds
    }
}

/// 生效的 `[ai].base_url`: 配置文件优先, 其次预设默认值。
pub fn effective_base_url(f: &File) -> String {
    f.ai.base_url
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| ai_config::preset_by_id(&f.ai.provider).map(|p| p.base_url.to_string()))
        .unwrap_or_default()
}

fn has_text(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.trim().is_empty())
}

/// 缺口判定(纯函数)。`env_key_present` = 环境变量 `RICOW_AI_API_KEY` 已提供非空值。
pub fn detect_gaps(f: &File, env_key_present: bool) -> Gaps {
    let base_url = effective_base_url(f);
    let local = !base_url.is_empty() && provider::is_local_endpoint(&base_url);
    Gaps {
        ai_key: !has_text(&f.ai.api_key) && !env_key_present && !local,
        demo_creds: !(has_text(&f.exchange.demo_key) && has_text(&f.exchange.demo_secret)),
    }
}

/// 供应商选择解析(纯函数): 回车 = 保持当前(空缺则用默认); 数字 = 第 n 个预设; 预设名 = 该预设;
/// 其它 = `None`(调用方提示重输 —— 自定义中转端点请在配置文件里写 `base_url`)。
pub fn provider_choice(input: &str, current: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        let c = current.trim();
        return Some(if c.is_empty() {
            ai_config::DEFAULT_PROVIDER.to_string()
        } else {
            c.to_string()
        });
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        let n: usize = t.parse().ok()?;
        return ai_config::PRESETS.get(n.checked_sub(1)?).map(|p| p.id.to_string());
    }
    ai_config::preset_by_id(t).map(|p| p.id.to_string())
}

/// 欢迎语(中英对照; 安全与隐私口径一次说清)。
fn welcome_text() -> String {
    let presets = ai_config::PRESETS
        .iter()
        .enumerate()
        .map(|(i, p)| format!("  {}. {} ({})", i + 1, p.id, p.label))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\
================================================================
 ricow 首次启动向导 / First-run setup
 只问缺的东西, 一分钟录完; 之后直接用中文或英文对话即可。
 Only missing items are asked. Afterwards just chat in 中文 / English.

 密钥只写进本机配置文件(不入 git),
 不会发给模型、不写日志。向导里也请不要把密钥粘到对话提问里。
 Keys are stored in a local config file, never sent to the model.

 供应商预设 / provider presets:
{presets}
================================================================"
    )
}

/// 供应商选择提示语。
fn provider_prompt(current: &str) -> String {
    let def = if current.trim().is_empty() { ai_config::DEFAULT_PROVIDER } else { current.trim() };
    format!("供应商 / provider [回车={def}]: ")
}

/// 运行向导(仅在缺东西时打扰用户)。
///
/// `strict` = true(裸入口): 缺东西且 stdin 非终端 → 报错退出;
/// false(现有子命令): 非终端 → 只提示一句, 放行(不阻断脚本与冒烟)。
pub async fn run_if_needed(root: &Path, strict: bool) -> CoreResult<Outcome> {
    let file = config_file::load(root)?;
    let env_key_present =
        std::env::var(ai_config::ENV_API_KEY).ok().is_some_and(|v| !v.trim().is_empty());
    let gaps = detect_gaps(&file, env_key_present);
    if !gaps.any() {
        return Ok(Outcome::NotNeeded);
    }

    if !std::io::stdin().is_terminal() {
        let hint = format!(
            "首次运行需要在交互式终端里录入密钥(当前输入不是终端, 无法安全录入)。\n\
             First-run setup needs an interactive terminal (stdin is not a TTY).\n\
             请直接运行 / please run: ricow\n\
             配置文件 / config: {}",
            config_file::path(root).display()
        );
        if strict {
            return Err(CoreError::Auth(hint));
        }
        eprintln!("提示: {hint}");
        return Ok(Outcome::Skipped);
    }

    println!("{}", welcome_text());
    println!("配置文件 / config: {}", config_file::path(root).display());

    let mut updates: Vec<(&'static str, &'static str, SetValue)> = Vec::new();

    // ── ②③ AI 通道: 供应商 + 密钥(+ 可选最小连通校验) ────────────────────────
    if gaps.ai_key {
        let provider_id = loop {
            let Some(line) = ask_line(&provider_prompt(&file.ai.provider)).await else {
                return Ok(Outcome::Skipped);
            };
            match provider_choice(&line, &file.ai.provider) {
                Some(id) => break id,
                None => println!(
                    "没看懂 `{}`: 请输入序号(1..={})或预设名(如 deepseek)。\n\
                     自定义/中转端点请直接编辑 ricow.toml 的 [ai] 段写 base_url。",
                    line.trim(),
                    ai_config::PRESETS.len()
                ),
            }
        };
        updates.push(("ai", "provider", SetValue::Str(provider_id.clone())));

        // 模型: 预设带推荐值则直接用; 无推荐值(OpenRouter / Ollama)或沿用自定义 provider → 必须问。
        let preset_model =
            ai_config::preset_by_id(&provider_id).map(|p| p.model.to_string()).unwrap_or_default();
        let model = if !preset_model.trim().is_empty() {
            preset_model
        } else {
            let cur = file.ai.model.clone().unwrap_or_default();
            loop {
                let Some(line) = ask_line(&format!(
                    "模型名 / model [回车={}]: ",
                    if cur.is_empty() { "必填" } else { &cur }
                ))
                .await
                else {
                    return Ok(Outcome::Skipped);
                };
                let m = if line.trim().is_empty() { cur.clone() } else { line.trim().to_string() };
                if !m.is_empty() {
                    break m;
                }
                println!("该供应商没有推荐模型, 必须填写模型名(如 gpt-4o-mini)。");
            }
        };
        if !model.trim().is_empty() {
            updates.push(("ai", "model", SetValue::Str(model.clone())));
        }

        let base_url = file
            .ai
            .base_url
            .clone()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| ai_config::preset_by_id(&provider_id).map(|p| p.base_url.to_string()))
            .unwrap_or_default();

        let key = match ask_secret("粘贴 API Key / paste API key (输入不回显, 回车=暂不填): ").await
        {
            Some(k) if !k.trim().is_empty() => Some(k.trim().to_string()),
            Some(_) => {
                println!(
                    "已跳过密钥: 稍后可在对话里用 /keys ai 补录, 或编辑 ricow.toml 的 [ai].api_key, \
                     或临时设环境变量 {}。",
                    ai_config::ENV_API_KEY
                );
                None
            }
            None => return Ok(Outcome::Skipped),
        };

        if let Some(k) = key {
            // 可选的连通校验: 只发一次极小请求; 失败不阻塞(重输 / 仍保存 / 退出)
            let mut current_key = k;
            if base_url.is_empty() {
                println!(
                    "provider `{provider_id}` 不是内置预设且未配 base_url: 已保存, 但对话前请在 ricow.toml 的 [ai] 段补 base_url。"
                );
            } else if ask_yes_no("现在验证连通 / verify now (会发一次最小请求)? [Y/n]: ").await
            {
                loop {
                    match probe(&provider_id, &base_url, &model, &current_key).await {
                        Ok(()) => {
                            println!("连通正常 / connection OK(端点可达, 密钥有效, 模型可用)。");
                            break;
                        }
                        Err(e) => {
                            println!("连通校验未通过 / verification failed: {e}");
                            match ask_choice(
                                "怎么处理? / what next?  [1] 重新输入密钥  [2] 仍然保存  [3] 退出: ",
                            )
                            .await
                            {
                                Some(1) => match ask_secret("重新粘贴 API Key / retry: ").await {
                                    Some(k) if !k.trim().is_empty() => {
                                        current_key = k.trim().to_string();
                                    }
                                    _ => println!("未重新输入, 保留原密钥继续。"),
                                },
                                Some(2) => {
                                    println!("已按你的选择保存(连通性待运行时确认)。");
                                    break;
                                }
                                _ => return Ok(Outcome::Skipped),
                            }
                        }
                    }
                }
            }
            updates.push(("ai", "api_key", SetValue::Str(current_key)));
        }
    }

    // ── ④ 币安测试网(demo)凭据: 可跳过 ────────────────────────────────────────
    if gaps.demo_creds {
        println!();
        println!("币安测试网(demo)凭据 —— 可跳过, 之后在对话里用 /keys demo 补录即可。");
        println!("Binance demo credentials — optional; add them later in chat with /keys demo.");
        println!("获取方式: demo.binance.com 登录 → API 管理 → 创建 Key(只勾选交易, 不要提现)");
        if let Some(k) = ask_line("demo_key [回车=跳过 / skip]: ").await {
            let k = k.trim().to_string();
            if !k.is_empty() {
                if let Some(s) = ask_secret("demo_secret(输入不回显): ").await {
                    let s = s.trim().to_string();
                    if !s.is_empty() {
                        updates.push(("exchange", "demo_key", SetValue::Str(k)));
                        updates.push(("exchange", "demo_secret", SetValue::Str(s)));
                    } else {
                        println!("secret 为空 → 本次不保存(两者必须成对)。");
                    }
                }
            }
        }
    }

    // ── ⑤ 落盘 ──────────────────────────────────────────────────────────────
    if updates.is_empty() {
        println!("\n本次未写入任何配置 / nothing saved。");
        return Ok(Outcome::Skipped);
    }
    config_file::set_values(root, &updates)?;
    println!(
        "\n已保存 / saved → {}({}, 不入 git)。",
        config_file::path(root).display(),
        config_file::permission_summary(root)
    );
    if let Some(w) = config_file::permission_warning(root) {
        eprintln!("提示: {w}");
    }
    println!(
        "下一步 / next: 直接对话即可, 例如\n  \
         - 帮我用香农网格模板建一个 AAPL 网格策略 / build an AAPL grid from the shannon template\n  \
         - 完全新写一个 TWAP 策略 / write a brand-new TWAP strategy\n  \
         - 回测我刚部署的策略 / backtest the strategy I just deployed\n\
         随时输入 /help 看可用命令与边界。"
    );
    Ok(Outcome::Saved)
}

/// 最小连通校验: 用所选 provider/model/key 发一次极小请求。
async fn probe(provider_id: &str, base_url: &str, model: &str, key: &str) -> CoreResult<()> {
    if model.trim().is_empty() {
        return Err(CoreError::InvalidArgument("模型名为空, 无法校验".into()));
    }
    let resolved = ai_config::Resolved {
        provider: provider_id.to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        max_turns: 1,
    };
    match tokio::time::timeout(PROBE_TIMEOUT, provider::probe(&resolved, key)).await {
        Ok(r) => r,
        Err(_) => Err(CoreError::Exchange(format!(
            "连通校验超时({}s); 端点可能被网络/代理阻断",
            PROBE_TIMEOUT.as_secs()
        ))),
    }
}

/// 读一行可见输入(阻塞读放 spawn_blocking; EOF/读失败 → None)。
async fn ask_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        match std::io::stdin().read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s),
        }
    })
    .await
    .ok()
    .flatten()
    .map(|s| s.trim_end().to_string())
}

/// 读一行**不回显**输入(`rpassword`; 提示语由 rpassword 打到 TTY)。
async fn ask_secret(prompt: &str) -> Option<String> {
    let p = prompt.to_string();
    tokio::task::spawn_blocking(move || rpassword::prompt_password(p).ok()).await.ok().flatten()
}

/// 是/否(回车 = 是)。
async fn ask_yes_no(prompt: &str) -> bool {
    match ask_line(prompt).await {
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            t.is_empty() || t == "y" || t == "yes"
        }
        None => false,
    }
}

/// 从 `1..=n` 里选一个; 其它输入 → None。
async fn ask_choice(prompt: &str) -> Option<u32> {
    let s = ask_line(prompt).await?;
    s.trim().parse::<u32>().ok().filter(|n| (1..=9).contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config_file::{AiSection, ExchangeSection, MarketSection};

    fn file_with(api_key: Option<&str>, provider: &str, base_url: Option<&str>) -> File {
        File {
            ai: AiSection {
                provider: provider.into(),
                model: Some("deepseek-flash".into()),
                base_url: base_url.map(|s| s.to_string()),
                max_turns: None,
                api_key: api_key.map(|s| s.to_string()),
            },
            exchange: ExchangeSection::default(),
            market: MarketSection::default(),
        }
    }

    #[test]
    fn test_detect_gaps_missing_key_and_demo() {
        let f = file_with(None, "deepseek", None);
        let g = detect_gaps(&f, false);
        assert!(g.ai_key, "无密钥 → 缺");
        assert!(g.demo_creds, "无 demo 凭据 → 缺");
        assert!(g.any());
    }

    #[test]
    fn test_detect_gaps_no_gap_when_key_and_demo_present() {
        let mut f = file_with(Some("sk-x"), "deepseek", None);
        f.exchange.demo_key = Some("DK".into());
        f.exchange.demo_secret = Some("DS".into());
        let g = detect_gaps(&f, false);
        assert!(!g.any(), "齐全时不应打扰用户");
    }

    #[test]
    fn test_detect_gaps_env_key_or_local_endpoint_counts_as_present() {
        // 环境变量提供密钥 → 不算缺口
        let f = file_with(None, "deepseek", None);
        assert!(!detect_gaps(&f, true).ai_key);
        // 本机端点(ollama)不需要密钥
        let f = file_with(None, "ollama", Some("http://127.0.0.1:11434/v1"));
        assert!(!detect_gaps(&f, false).ai_key);
        // 预设自带的 base_url 也参与本机判定
        let f = file_with(None, "ollama", None);
        assert!(!detect_gaps(&f, false).ai_key, "ollama 预设 base_url 是本机");
    }

    #[test]
    fn test_effective_base_url_file_first_then_preset() {
        assert_eq!(
            effective_base_url(&file_with(None, "deepseek", None)),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            effective_base_url(&file_with(None, "deepseek", Some("https://my.proxy/v1"))),
            "https://my.proxy/v1"
        );
        assert_eq!(effective_base_url(&file_with(None, "myproxy", None)), "");
    }

    #[test]
    fn test_provider_choice_empty_keeps_current_or_default() {
        assert_eq!(provider_choice("", "moonshot").as_deref(), Some("moonshot"));
        assert_eq!(provider_choice("   ", "").as_deref(), Some(ai_config::DEFAULT_PROVIDER));
    }

    #[test]
    fn test_provider_choice_number_and_id() {
        assert_eq!(provider_choice("1", "").as_deref(), Some("deepseek"));
        assert_eq!(provider_choice("5", "").as_deref(), Some("openrouter"));
        assert_eq!(provider_choice("ollama", "").as_deref(), Some("ollama"));
        assert_eq!(provider_choice("qwen", "deepseek").as_deref(), Some("qwen"));
    }

    #[test]
    fn test_provider_choice_rejects_unknown_and_out_of_range() {
        assert_eq!(provider_choice("99", ""), None);
        assert_eq!(provider_choice("0", ""), None);
        assert_eq!(provider_choice("myproxy", ""), None, "自定义名走配置文件, 向导不猜");
    }

    #[test]
    fn test_welcome_text_is_bilingual_and_lists_presets() {
        let w = welcome_text();
        assert!(w.contains("首次启动向导") && w.contains("First-run setup"), "{w}");
        assert!(w.contains("deepseek") && w.contains("ollama"), "{w}");
    }

    #[test]
    fn test_provider_prompt_has_default() {
        assert!(provider_prompt("deepseek").contains("回车=deepseek"));
        assert!(provider_prompt("").contains(ai_config::DEFAULT_PROVIDER));
    }
}
