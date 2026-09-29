//! rig 适配层 (019) —— **唯一与 rig 耦合的文件**: 换/升级 LLM 框架只改这里。
//!
//! 口径(spec FR-002 / FR-006):
//! - 统一走 **OpenAI 兼容 Chat Completions**: 必须显式 `.completions_api()`,
//!   因为 rig 的 openai 客户端默认走 Responses API, 而兼容端点普遍不实现 `/responses`;
//! - 本机端点(`127.0.0.1` / `localhost`)不要求密钥(如本地 Ollama), 其余端点必须带密钥;
//! - 工具循环上限 = `Resolved::max_turns`(成本护栏), 每轮答复打印 token 用量。

use std::path::Path;

use futures::StreamExt;
use ricow_core::{CoreError, CoreResult};
use rig::agent::MultiTurnStreamItem;
use rig::message::Message;
use rig::prelude::*;
use rig::providers::openai;
use rig::streaming::StreamedAssistantContent;

use super::config::{self, Resolved};
use super::session::SessionSink;

/// 032 US3 (FR-018): Web「AI 改 Lua」一次性问答的系统提示 —— 无工具、单轮,
/// 只做一件事: 在原策略代码基础上按自然语言指令改写, 并只输出完整 Lua。
const AI_EDIT_PREAMBLE: &str = "你是 ricow 的 Lua 交易策略编辑助手。你只负责改写策略代码, \
    不做行情分析、不调用任何工具; 严格使用 ricow 既有 ctx API(不发明接口), \
     与修改指令无关的原有逻辑必须逐字保留; 最终只输出一份完整 Lua 代码, 不加任何解释或 Markdown 围栏。";

/// 构造 OpenAI 兼容客户端(自定义 base_url + 密钥)。
pub fn build_client(resolved: &Resolved, api_key: &str) -> CoreResult<openai::CompletionsClient> {
    let client = openai::Client::builder()
        .api_key(api_key.to_string())
        .base_url(resolved.base_url.clone())
        .build()
        .map_err(|e| {
            CoreError::Auth(format!("构造 LLM 客户端失败(base_url={}): {e}", resolved.base_url))
        })?;
    // Chat Completions 通道(兼容端点普遍无 /responses)
    Ok(client.completions_api())
}

/// 本机端点判定(纯函数, 便于单测): 本机端点不强制密钥。
///
/// 支持的形态: `127.0.0.1` / `localhost` / `0.0.0.0` / IPv6 字面量 `[::1]`(带端口亦可)。
/// 注意: `https://127.0.0.1.evil.com/...` 这类"前缀相似的域名"**不算**本机。
pub fn is_local_endpoint(base_url: &str) -> bool {
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let authority = rest.split('/').next().unwrap_or("");
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 字面量形如 [::1]:11434 —— 取到 ']' 为止(不能按 ':' 切)
        format!("[{}]", stripped.split(']').next().unwrap_or(""))
    } else {
        authority.split(':').next().unwrap_or("").to_string()
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "0.0.0.0" | "[::1]")
}

/// 密钥解析: 本机端点允许缺省(用占位值), 其余端点缺密钥即报错(带解法)。
pub fn resolve_key(base_url: &str, from_config: CoreResult<String>) -> CoreResult<String> {
    match from_config {
        Ok(k) => Ok(k),
        Err(e) => {
            if is_local_endpoint(base_url) {
                Ok("local-endpoint".into())
            } else {
                Err(e)
            }
        }
    }
}

/// 最小连通校验 (019-R4): 发一次极小请求, 验证"端点可达 + 密钥有效 + 模型可用"。
///
/// 与 [`connect`] 的区别: 不挂工具、不带系统提示、`max_tokens` 压到 1 —— 只为打通一次,
/// 不产生实际消耗。失败原因原样回给调用方(向导据此让用户"重输 / 仍然保存 / 退出")。
pub async fn probe(resolved: &Resolved, api_key: &str) -> CoreResult<()> {
    let client = build_client(resolved, api_key)?;
    let model = client.completion_model(resolved.model.as_str());
    model
        .completion_request("ping")
        .max_tokens(1)
        .send()
        .await
        .map_err(|e| CoreError::Exchange(format!("LLM 连通校验失败: {e}")))?;
    Ok(())
}

/// 一轮问答的产出。
pub struct Answer {
    pub text: String,
    /// token 用量文本(rig 返回的 Usage; 供应商不支持时可能缺失)。
    pub usage: Option<String>,
}

/// 会话实例(持有 rig agent 与生效参数)。
pub struct Llm {
    agent: rig::agent::Agent,
    /// 生效的每轮模型调用上限(非流式路径用)。
    max_turns: usize,
}

/// 用最终参数 + 密钥 + 系统提示 + 工具集构造会话。
///
/// `tools` 只能是 [`super::tools::build`] 产出的 L0 只读 + L1 虚拟工具;`ToolGuard` 作为
/// 审批门一并挂上(fail-closed: 白名单之外一律拒绝执行)。
pub fn connect(
    resolved: Resolved,
    api_key: &str,
    preamble: &str,
    tools: Vec<rig::tool::DynamicTool>,
) -> CoreResult<Llm> {
    let client = build_client(&resolved, api_key)?;
    let agent = client
        .agent(resolved.model.as_str())
        .preamble(preamble)
        .default_max_turns(resolved.max_turns)
        .dynamic_tools(tools)
        .add_hook(super::tools::ToolGuard)
        .build();
    Ok(Llm { agent, max_turns: resolved.max_turns })
}

impl Llm {
    /// 非流式一轮: 一次性返回最终答复(内部工具循环仍是同一个 rig agent)。
    ///
    /// 用途: ① `ricow ai --plain`(便于脚本/管道); ② 排查某些端点在**流式**下
    /// 不返回 `tool_calls`、把工具调用当普通文本吐出的情况(实测 ollama + 小模型会这样)。
    pub async fn ask(&self, prompt: &str) -> CoreResult<Answer> {
        let resp = self
            .agent
            .prompt(prompt)
            .max_turns(self.max_turns)
            .extended_details()
            .await
            .map_err(|e| CoreError::Exchange(format!("LLM 调用失败: {e}")))?;
        let usage = format!(
            "输入 {} / 输出 {} / 合计 {} tokens",
            resp.usage.input_tokens, resp.usage.output_tokens, resp.usage.total_tokens
        );
        Ok(Answer { text: resp.output.clone(), usage: Some(usage) })
    }

    /// 流式一轮: 逐段把助手文本增量交给 `sink`(本模块**不打印**), 返回最终答复(整轮结束后)。
    pub async fn ask_stream(
        &self,
        prompt: &str,
        history: &[Message],
        sink: &mut dyn SessionSink,
    ) -> CoreResult<Answer> {
        let mut stream = self.agent.stream_chat(prompt, history.to_vec()).await;
        let mut final_text: Option<String> = None;
        let mut printed = false;
        // token 用量: 每次模型调用都带 CompletionCall(供应商不报用量时全 0)
        let mut usage: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))) => {
                    sink.text(&t.text);
                    printed = true;
                }
                Ok(MultiTurnStreamItem::FinalResponse(r)) => {
                    final_text = Some(r.output().to_string());
                }
                Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                    // rig 的 Usage 字段名(实测 ollama 流式上报): input/output/total tokens
                    let u = &call.usage;
                    usage = Some(format!(
                        "输入 {} / 输出 {} / 合计 {} tokens",
                        u.input_tokens, u.output_tokens, u.total_tokens
                    ));
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(CoreError::Exchange(format!("LLM 流式调用中断: {e}")));
                }
            }
        }
        if printed {
            // 收尾换行: 增量文本不以换行结尾, 由这里补齐(用量行另起一行)
            sink.text("\n");
        }
        let text = final_text.ok_or_else(|| {
            CoreError::Exchange(
                "流式结束但未收到最终答复(模型可能未按 Chat Completions 返回)".into(),
            )
        })?;
        Ok(Answer { text, usage })
    }
}

/// 构造「AI 改 Lua」的用户提示词(032 FR-018, 纯函数便于离线单测)。
///
/// 三段固定结构: ① 策略参数说明摘要(只供模型理解参数语义, 由 manifest 数据拼出, 不含任何配置密钥);
/// ② 原策略的**完整** Lua 代码; ③ 用户的自然语言修改指令。并显式约束输出形态:
/// 只输出一份完整 Lua、无 ``` 围栏、无解释 —— 调用方随后用 [`ricow_engine::extract_code`]
/// 再剥一次围栏做兜底, 然后强制过编译门禁, 全程不落盘。
pub(crate) fn build_edit_prompt(
    instruction: &str,
    lua_code: &str,
    manifest_summary: &str,
) -> String {
    format!(
        "请按用户的修改指令, 在下面这份**原策略完整代码**的基础上改写策略。\n\
         \n\
         ## 策略参数说明(仅供理解, 不要擅自增删与本次指令无关的参数)\n\
         {manifest_summary}\n\
         \n\
         ## 原策略完整 Lua 代码\n\
         {lua_code}\n\
         \n\
         ## 用户的修改指令\n\
         {instruction}\n\
         \n\
         ## 输出要求(必须严格遵守)\n\
         1. 只输出**一份**改写后的完整 Lua 代码(包含完整的 on_tick 以及需要保留的 on_init 等函数);\n\
         2. 不要输出 ```lua 之类的代码围栏, 不要输出任何解释、前后缀、寒暄或 Markdown;\n\
         3. 只能使用 ricow 既有的 ctx API, 不要发明不存在的接口或参数;\n\
         4. 与本次修改指令无关的原有逻辑逐字保留。"
    )
}

/// 032 US3 (FR-018): Web「AI 改 Lua」的一次性非流式问答薄封装。
///
/// 与对话会话([`super::session::ChatSession`])取**同一套**配置: provider/model/base_url
/// 只来自 `ricow.toml` 的 `[ai]` 段 + `RICOW_AI_*` 环境变量, Web 侧不提供任何覆盖入口;
/// 密钥也**绝不**进入提示词(只用于构造客户端)。
///
/// 错误建模(供 Web 层稳定判定, 不新增 CoreError 变体):
/// - 未配密钥(或客户端构造的鉴权前置失败)→ [`CoreError::Auth`] —— Web 映射 403 `need_keys`;
/// - 调用超时 / 上游错误 → [`CoreError::Exchange`](`Llm::ask` 已做中文包装)—— Web 映射 400;
/// - 其余配置/IO 错误原样透传(Auth/InvalidArgument 等);
/// - 本机端点(如本地 Ollama)沿用 [`resolve_key`] 例外: 缺密钥时以占位值放行, 不判 need_keys。
///
/// 单轮不挂工具(`tools` 传空), `max_tokens` 跟随 rig/agent 默认 —— 不单独暴露。
pub(crate) async fn quick_ask(
    root: &Path,
    instruction: &str,
    lua_code: &str,
    manifest_summary: &str,
) -> CoreResult<String> {
    // 唯一配置文件: ricow.toml(缺文件时生成模板; 与 ChatSession::open 逐字同路径)。
    let file = crate::commands::config_file::load(root)?;
    let cfg = config::AiConfig {
        provider: file.ai.provider.clone(),
        model: file.ai.model.clone().unwrap_or_default(),
        base_url: file.ai.base_url.clone(),
        max_turns: file
            .ai
            .max_turns
            .map(config::check_max_turns)
            .transpose()?
            .unwrap_or(config::DEFAULT_MAX_TURNS),
    };
    // 覆盖优先级与对话会话一致: 环境变量 > ricow.toml > 预设(Web 无命令行覆盖层)。
    let env_base_url = std::env::var(config::ENV_BASE_URL).ok();
    let env_model = std::env::var(config::ENV_MODEL).ok();
    let resolved = config::resolve(&cfg, env_base_url, env_model)?;

    // 缺密钥(远程端点)在此即返回 CoreError::Auth, 发生在任何网络请求之前。
    let api_key = resolve_key(&resolved.base_url, config::api_key(root, &resolved.provider))?;

    let prompt = build_edit_prompt(instruction, lua_code, manifest_summary);
    let llm = connect(resolved, &api_key, AI_EDIT_PREAMBLE, Vec::new())?;
    let answer = llm.ask(&prompt).await?;
    Ok(answer.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_endpoint_detection() {
        assert!(is_local_endpoint("http://127.0.0.1:11434/v1"));
        assert!(is_local_endpoint("http://localhost:8080/v1"));
        assert!(is_local_endpoint("https://localhost/v1"));
        assert!(!is_local_endpoint("https://api.deepseek.com/v1"));
        assert!(!is_local_endpoint("https://127.0.0.1.evil.com/v1"), "前缀相似的域名不算本机");
        assert!(is_local_endpoint("http://[::1]:11434/v1"));
    }

    #[test]
    fn test_resolve_key_local_exception_only() {
        let missing = || Err(ricow_core::CoreError::Auth("未找到 LLM API 密钥".into()));
        // 有密钥: 原样返回
        assert_eq!(resolve_key("https://api.deepseek.com/v1", Ok("k".into())).unwrap(), "k");
        // 本机端点无密钥: 允许(占位值)
        assert_eq!(resolve_key("http://127.0.0.1:11434/v1", missing()).unwrap(), "local-endpoint");
        // 远程端点无密钥: 报错
        assert!(resolve_key("https://api.deepseek.com/v1", missing()).is_err());
    }

    #[test]
    fn test_build_edit_prompt_contains_all_sections_and_no_secrets() {
        let instruction = "把网格间距改成 ATR 三倍";
        let lua = "function on_tick(ctx)\n  ctx:log('hi')\nend\n";
        let summary = "- 网格步长 (key=grid_step): f64, 必填";
        let p = build_edit_prompt(instruction, lua, summary);
        // 三段结构关键内容都要在
        assert!(p.contains(summary), "参数摘要必须原样进入 prompt: {p}");
        assert!(p.contains(lua), "原代码必须完整进入 prompt: {p}");
        assert!(p.contains(instruction), "用户指令必须原样进入 prompt: {p}");
        // 输出约束关键段
        assert!(p.contains("on_tick"), "要求输出完整函数: {p}");
        assert!(p.contains("围栏"), "必须要求不带代码围栏: {p}");
        // 密钥类字样绝不进入 prompt(quick_ask 只在构造客户端时用 key)
        assert!(!p.to_ascii_lowercase().contains("api_key"), "prompt 不得含密钥字样: {p}");
        assert!(!p.to_ascii_lowercase().contains("secret"), "prompt 不得含密钥字样: {p}");
    }
}
