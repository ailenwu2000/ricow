//! rig 适配层 (019) —— **唯一与 rig 耦合的文件**: 换/升级 LLM 框架只改这里。
//!
//! 口径(spec FR-002 / FR-006):
//! - 统一走 **OpenAI 兼容 Chat Completions**: 必须显式 `.completions_api()`,
//!   因为 rig 的 openai 客户端默认走 Responses API, 而兼容端点普遍不实现 `/responses`;
//! - 本机端点(`127.0.0.1` / `localhost`)不要求密钥(如本地 Ollama), 其余端点必须带密钥;
//! - 工具循环上限 = `Resolved::max_turns`(成本护栏), 每轮答复打印 token 用量。

use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use ricow_core::{CoreError, CoreResult};
use rig::agent::MultiTurnStreamItem;
use rig::message::{AssistantContent, Message, UserContent};
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

/// Lua 策略 API 规范全文(编译期嵌入, P0-3): AI 从零生成策略的唯一接口依据 ——
/// 与 specs/lua-api.md 同一来源, 不另造第二份 API 清单。
const LUA_API_DOC: &str = include_str!("../../../../specs/lua-api.md");

/// P0-3: Web「AI 生成新策略」的系统提示 —— 与 AI_EDIT_PREAMBLE 同纪律(只输出完整 Lua),
/// 但任务是从零编写, 且必须先读规范再写代码。
const AI_GEN_PREAMBLE: &str =
    "你是 ricow 的 Lua 交易策略编写助手。你只负责编写一份全新的策略代码, \
    不做行情分析、不调用任何工具; 编写前先通读用户消息里给出的 ricow Lua API 规范, \
    严格只使用规范列出的 ctx/exec API(不发明接口); 最终只输出一份完整 Lua 代码, \
    不加任何解释或 Markdown 围栏。";

/// 052 方案 3: Web「AI 调参数」的系统提示 —— 与上面两条**任务不同**: 不改代码, 只把用户的
/// 自然语言翻译成**已声明参数**的取值, 且**只能**用给定的键。这与 `ai/tools.rs` 里
/// `update_params` 工具的纪律同源("只有用户明确的键才写, 不要自行推断")。
/// N3 (2026-10-12) 起这条的**任务变了**: 从前是"把用户的话翻译成键值", 现在是"当参数顾问" ——
/// 用户只说**意图**(不说改哪个键、改成多少), 助手自己判断该动哪些参数、动到什么程度,
/// 并讲清理由与风险。
///
/// 为什么改: "把 spacing_pct 改成 0.02"这件事用户自己点两下表单就能做, 根本不需要 AI;
/// AI 该干的是**用户干不了的那部分** —— 把"保守点"这种意图翻译成一整套参数, 并说清代价。
const AI_PARAMS_PREAMBLE: &str =
    "你是 ricow 的策略参数顾问。用户只会说他想要的效果(如\"保守一点\"、\"更看好震荡行情\"), \
    不会指定改哪个参数或改成多少 —— 你要自己判断该动哪些参数、动到什么程度, 并讲清理由与风险。 \
    不写策略代码、不做行情分析、不调用任何工具; 参数键只能用用户消息里给出的清单里出现过的键, \
    绝不发明新键; 讲完理由后再给一个 JSON 对象, 不加任何 Markdown 围栏。";

/// N2-C (2026-10-12): Web「给参数起中文名」的系统提示 —— 任务是**补名字与说明**,
/// 既不改代码也不改取值。名字只能从"这个键在脚本里怎么被用"推出来, 所以提示词必须带脚本全文;
/// 代码里看不出用途时**如实说看不出**, 不许编(用户会当真)。
const AI_NAMES_PREAMBLE: &str =
    "你是 ricow 的策略参数命名助手。你只负责根据**脚本代码**为给定的参数键起中文名并写一句说明, \
    不改代码、不改取值、不做行情分析、不调用任何工具; 键只能用用户消息里列出的那些, 绝不发明新键; \
    代码里看不出用途的键要如实说看不出, 不得编一个听起来合理的解释; \
    最终只输出一个 JSON 对象, 不加任何解释或 Markdown 围栏。";

/// 构造 OpenAI 兼容客户端(自定义 base_url + 密钥)。
///
/// 055 B5: 超时**不在这里**设, 见 [`Llm::ask_once`] 外层那道 `tokio::time::timeout`。
/// 原因是依赖树里有两个 reqwest: 本仓用 0.12, 而 rig 0.42 用 **0.13** —— 想通过
/// `ClientBuilder::http_client` 传一个自带 `.timeout()` 的客户端, 类型根本对不上
/// (`reqwest::Client: HttpClientExt` 是为 0.13 实现的)。为它把本仓 reqwest 升到 0.13
/// 会牵动交易所那条线, 不值得 —— 外层超时效果等价(取消 future 即中止请求),
/// 而且对**所有** AI 调用(非流式/流式/连通校验)是同一道, 不会漏。
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
    // 055 B5: 连通校验同样有上限 —— 端点不可达时它会是第一个被卡住的请求。
    // 调用方(onboard)外面还有一道更短的 20s, 两道取小的那个生效, 不会互相打架。
    let fut = model.completion_request("ping").max_tokens(1).send();
    match tokio::time::timeout(std::time::Duration::from_secs(resolved.timeout_secs), fut).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(CoreError::Exchange(format!("LLM 连通校验失败: {e}"))),
        Err(_) => Err(CoreError::Exchange(timeout_message(resolved.timeout_secs))),
    }
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
    /// 055 B5: 生效的总超时(秒) —— 只为了把超时报错说清楚(报多少秒, 怎么调大)。
    timeout_secs: u64,
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
    Ok(Llm { agent, max_turns: resolved.max_turns, timeout_secs: resolved.timeout_secs })
}

/// LLM 调用最多尝试几次(1 次原始 + `LLM_MAX_ATTEMPTS - 1` 次重试)。
const LLM_MAX_ATTEMPTS: u32 = 3;

/// LLM 重试退避: 400ms → 1.2s。
///
/// 比币安 REST 那套保守得多 —— 那里可以静默重试(用户看不见), 而这里是**人机交互路径**:
/// 让用户对着空屏等 8 秒, 不如早点如实报错让他自己重来一次。
fn llm_backoff(attempt: u32) -> Duration {
    Duration::from_millis(400 * 3u64.pow(attempt.min(4)))
}

/// 判断错误里是否出现了某个 HTTP 状态码 token(按数字边界切分, 避免 "1500" 命中 "500")。
fn has_status_token(lowered_msg: &str, code: &str) -> bool {
    lowered_msg.split(|c: char| !c.is_ascii_digit()).any(|tok| tok == code)
}

/// 055 B5: 是否命中**我们自己设的总超时**(纯函数, 便于单测)。
///
/// 判据只能落在错误文本上(rig 收敛掉了结构化状态码)。因为传输层只设了 `timeout`、
/// 没设更细的 connect timeout, 所以文本里出现 timeout/timed out 就意味着
/// "整整等了 N 秒, 上游一个字节都没回"。
///
/// 这与"网络抖一下"不是一回事 —— 见 [`is_retryable_llm_error`] 里那条前置否决。
fn is_ai_timeout(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("timed out") || m.contains("timeout")
}

/// 超时时给用户的说法(把"等了多久"和"怎么调大"一并说清)。
fn timeout_message(secs: u64) -> String {
    format!(
        "模型服务商 {secs} 秒内没有返回任何内容(上游繁忙或网络中断)。\n\
         这不是本机故障, 也不代表策略有问题 —— 稍后重试即可; \
         若你的模型确实需要更久(例如本机小模型), 在 ricow.toml 的 [ai] 段加一行 \
         timeout_secs = 900(取值 {MIN}..={MAX})。",
        MIN = config::MIN_TIMEOUT_SECS,
        MAX = config::MAX_TIMEOUT_SECS,
    )
}

/// LLM 故障是否**值得重试**(纯函数, 便于单测)。
///
/// 只能在**错误文本**上判: rig 把上游失败统一收敛成带 Display 的错误, 我们手里没有结构化的
/// 状态码。因此只认"必然瞬时"的强特征, 并且**先排除**一票否决类 —— 鉴权失败 / 请求非法
/// 重试一万次也是同样结果, 把它们卷进重试只会让用户白等。
fn is_retryable_llm_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    // 一票否决: 这类错误重试无意义(密钥错、请求格式错、余额不足、模型名写错)。
    const NON_RETRYABLE_STATUS: [&str; 7] = ["400", "401", "402", "403", "404", "422", "501"];
    if NON_RETRYABLE_STATUS.iter().any(|c| has_status_token(&m, c)) {
        return false;
    }
    // 明确瞬时: 限流 / 服务端故障。
    const RETRYABLE_STATUS: [&str; 7] = ["408", "425", "429", "500", "502", "503", "504"];
    if RETRYABLE_STATUS.iter().any(|c| has_status_token(&m, c)) {
        return true;
    }
    // 055 B5: **先否决总超时** —— 一次就等满 N 秒, 再退避重试两次就是十几分钟,
    // 用户对着转圈的页面根本等不起, 而且上游多半仍无响应。宁可早点如实报错,
    // 让他自己决定要不要再来一次(必须判在下面 SOFT 之前: 那里把 timeout 当瞬时抖动)。
    if is_ai_timeout(&m) {
        return false;
    }
    // 没有状态码的传输层抖动。
    const SOFT: &[&str] = &[
        "too many requests",
        "rate limit",
        "connection reset",
        "connection refused",
        "broken pipe",
        "temporarily unavailable",
        "overloaded",
        "service unavailable",
        "bad gateway",
    ];
    SOFT.iter().any(|s| m.contains(s))
}

impl Llm {
    /// 非流式一轮: 一次性返回最终答复(内部工具循环仍是同一个 rig agent)。
    ///
    /// 用途: ① `ricow ai --plain`(便于脚本/管道); ② 排查某些端点在**流式**下
    /// 不返回 `tool_calls`、把工具调用当普通文本吐出的情况(实测 ollama + 小模型会这样)。
    ///
    /// 瞬时故障([`is_retryable_llm_error`])自动退避重试 —— 网络抖一下不该让用户重敲一遍问题。
    pub async fn ask(&self, prompt: &str) -> CoreResult<Answer> {
        let mut attempt = 0u32;
        loop {
            match self.ask_once(prompt).await {
                Ok(a) => return Ok(a),
                Err(e) => {
                    attempt += 1;
                    if attempt >= LLM_MAX_ATTEMPTS || !is_retryable_llm_error(&e.to_string()) {
                        return Err(e);
                    }
                    tracing::debug!(attempt, error = %e, "LLM 调用失败, 退避后重试");
                    tokio::time::sleep(llm_backoff(attempt - 1)).await;
                }
            }
        }
    }

    /// 单次非流式尝试(被 [`Llm::ask`] 的重试循环包裹)。
    ///
    /// 055 B5: 外面套一道 `tokio::time::timeout` —— 上游挂起时(真跑实测 900s / 1500s 无响应,
    /// 而同一端点 `/models` 秒回)这里就是唯一的兜底。取消了 future, 底层请求随之中止,
    /// 不会留一条没人管的连接在后台跑。
    ///
    /// 超时**只算单次**: 它已被 [`is_retryable_llm_error`] 判为不可重试, 所以最坏情况就是
    /// 等 `timeout_secs` 一次, 不会变成"每次都等满、连等三遍"。
    async fn ask_once(&self, prompt: &str) -> CoreResult<Answer> {
        let fut = self.agent.prompt(prompt).max_turns(self.max_turns).extended_details();
        let resp = match tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            fut,
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let msg = e.to_string();
                if is_ai_timeout(&msg) {
                    return Err(CoreError::Exchange(timeout_message(self.timeout_secs)));
                }
                return Err(CoreError::Exchange(format!("LLM 调用失败: {msg}")));
            }
            Err(_) => return Err(CoreError::Exchange(timeout_message(self.timeout_secs))),
        };
        let usage = format!(
            "输入 {} / 输出 {} / 合计 {} tokens",
            resp.usage.input_tokens, resp.usage.output_tokens, resp.usage.total_tokens
        );
        Ok(Answer { text: resp.output.clone(), usage: Some(usage) })
    }

    /// 流式一轮: 逐段把助手文本增量交给 `sink`(本模块**不打印**), 返回最终答复(整轮结束后)。
    ///
    /// 重试有一条硬边界: **已经往 `sink` 吐过字就不再重试** —— 重来一遍会把同一段话显示
    /// 第二次, 对用户来说比直接报错更糟。所以重试只覆盖"连接都没建立起来 / 首包之前就断了"
    /// 这一段。
    pub async fn ask_stream(
        &self,
        prompt: &str,
        history: &[Message],
        sink: &mut dyn SessionSink,
    ) -> CoreResult<Answer> {
        let mut attempt = 0u32;
        loop {
            let mut emitted = false;
            match self.ask_stream_once(prompt, history, sink, &mut emitted).await {
                Ok(a) => return Ok(a),
                Err(e) => {
                    attempt += 1;
                    if emitted
                        || attempt >= LLM_MAX_ATTEMPTS
                        || !is_retryable_llm_error(&e.to_string())
                    {
                        return Err(e);
                    }
                    tracing::debug!(attempt, error = %e, "LLM 流式调用失败(尚未输出), 退避后重试");
                    tokio::time::sleep(llm_backoff(attempt - 1)).await;
                }
            }
        }
    }

    /// 单次流式尝试; `emitted` 在**首次向 sink 吐出文本**时置位(供重试判定)。
    ///
    /// 055 B5: 与非流式同一道超时(理由见 [`Llm::ask_once`])。这里套的是**整段流**
    /// 而不只是首包 —— 上游卡在半截时, 用户已经看到前面几个字却永远等不到结尾,
    /// 比一开始就失败还难判断。
    async fn ask_stream_once(
        &self,
        prompt: &str,
        history: &[Message],
        sink: &mut dyn SessionSink,
        emitted: &mut bool,
    ) -> CoreResult<Answer> {
        let fut = self.stream_once(prompt, history, sink, emitted);
        match tokio::time::timeout(std::time::Duration::from_secs(self.timeout_secs), fut).await {
            Ok(r) => r,
            Err(_) => Err(CoreError::Exchange(timeout_message(self.timeout_secs))),
        }
    }

    /// 被 [`Llm::ask_stream_once`] 的超时外壳包裹的实际读流过程。
    async fn stream_once(
        &self,
        prompt: &str,
        history: &[Message],
        sink: &mut dyn SessionSink,
        emitted: &mut bool,
    ) -> CoreResult<Answer> {
        let mut stream = self.agent.stream_chat(prompt, history.to_vec()).await;
        let mut final_text: Option<String> = None;
        // token 用量: 每次模型调用都带 CompletionCall(供应商不报用量时全 0)
        let mut usage: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))) => {
                    sink.text(&t.text);
                    *emitted = true;
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
                    let msg = e.to_string();
                    // 055 B5: 同 ask_once —— 超时讲人话(等了多久 / 怎么调大)。
                    if is_ai_timeout(&msg) {
                        return Err(CoreError::Exchange(timeout_message(self.timeout_secs)));
                    }
                    return Err(CoreError::Exchange(format!("LLM 流式调用中断: {msg}")));
                }
            }
        }
        if *emitted {
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

// ---------------------------------------------------------------------------
// 对话历史预算 (3.4)
// ---------------------------------------------------------------------------

/// 对话历史的字符预算。超出即从**最前面**按整轮折叠, 直到进预算为止。
///
/// 为什么需要预算: 每轮提问都会把**全量** history 送进模型, 而它此前只增不减 —— 代价是双向的:
/// 上下文越长越贵越慢, 且终有一轮会直接顶穿模型的上下文窗口, 让整轮彻底失败。
///
/// 用**字符数**当代理而不引入 tokenizer: 中文约 1 字 ≈ 1 token、英文约 4 字符 ≈ 1 token,
/// 这点误差在"预算留多少余量"的尺度上无所谓, 却省掉一个重量级依赖。
pub(crate) const HISTORY_BUDGET_CHARS: usize = 24_000;

/// 折叠摘记里最多回列几段更早的提问。
const HISTORY_DIGEST_MAX_ITEMS: usize = 3;

/// 折叠摘记里单段提问的字符上限。
const HISTORY_DIGEST_ITEM_CHARS: usize = 60;

/// 一条消息占的字符数(只算文本块, 非文本内容按 0 计)。
fn message_chars(m: &Message) -> usize {
    match m {
        Message::System { content } => content.chars().count(),
        Message::User { content } => content
            .iter()
            .map(|c| match c {
                UserContent::Text(t) => t.text.chars().count(),
                _ => 0,
            })
            .sum(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|c| match c {
                AssistantContent::Text(t) => t.text.chars().count(),
                _ => 0,
            })
            .sum(),
    }
}

/// 取一条消息的首段纯文本(供折叠摘记用)。
fn message_text(m: &Message) -> Option<&str> {
    match m {
        Message::System { content } => Some(content.as_str()),
        Message::User { content } => content.iter().find_map(|c| match c {
            UserContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        }),
        Message::Assistant { content, .. } => content.iter().find_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        }),
    }
}

/// 生成被折叠部分的一句话摘记。
fn history_digest(dropped: &[Message], rounds: usize) -> String {
    let mut items: Vec<String> = Vec::new();
    for m in dropped {
        if items.len() >= HISTORY_DIGEST_MAX_ITEMS {
            break;
        }
        if !matches!(m, Message::User { .. }) {
            continue;
        }
        let Some(t) = message_text(m) else { continue };
        let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
        let head: String = flat.chars().take(HISTORY_DIGEST_ITEM_CHARS).collect();
        items.push(format!("\"{head}\""));
    }
    let more = rounds.saturating_sub(items.len());
    let more_note = if more > 0 { format!("(另有 {more} 轮从略)") } else { String::new() };
    let listed = if items.is_empty() {
        String::new()
    } else {
        format!(", 其提问依次为: {}", items.join("; "))
    };
    format!(
        "[对话历史摘要] 更早的 {rounds} 轮往返已按上下文预算省略{more_note}{listed}。\
         若当前问题依赖这些细节, 请让用户重述, 或重新查一次数据。"
    )
}

/// 把历史折叠进字符预算, 返回**被折叠掉的轮数**(0 = 原样未动)。
///
/// 折叠粒度是**整轮**(user + assistant 成对丢弃): 留下半个来回会让模型看到"答非所问",
/// 比丢一整轮更糟。折叠后在最前面插一行摘记, 让它知道"上面还有更早的对话, 细节已省略",
/// 而不是以为这就是对话的开头。
///
/// 这里刻意**不调 LLM 做摘要**(Vibe-Trading 的 L3 做法): 那要为纯运维目的多付一次调用和一次
/// 等待, 换来"更顺的摘要"。ricow 这个规模不值 —— 先做免费的确定性折叠, 真不够用再说。
pub(crate) fn compact_history(history: &mut Vec<Message>) -> usize {
    // 轮首下标(User 消息的位置)。history 由本程序成对写入, 但按内容判定比按下标假设稳。
    let starts: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, Message::User { .. }))
        .map(|(i, _)| i)
        .collect();
    // 0 轮 / 1 轮都没有"更早"可折 —— 至少留下当前这一轮, 否则模型会答非所问。
    if starts.len() <= 1 {
        return 0;
    }

    let mut total: usize = history.iter().map(message_chars).sum();
    if total <= HISTORY_BUDGET_CHARS {
        return 0;
    }

    let mut cut_round = 0usize;
    while total > HISTORY_BUDGET_CHARS && starts.len() - cut_round > 1 {
        let from = starts[cut_round];
        let to = starts[cut_round + 1];
        total = total.saturating_sub(history[from..to].iter().map(message_chars).sum());
        cut_round += 1;
    }
    if cut_round == 0 {
        return 0;
    }

    let cut = starts[cut_round];
    let digest = history_digest(&history[..cut], cut_round);
    history.drain(..cut);
    history.insert(0, Message::user(digest));
    cut_round
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

/// 一次性非流式问答的公共底座(P0-3 抽出): 配置解析 → 密钥 → 客户端 → 单轮问答。
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
async fn one_shot(root: &Path, preamble: &str, prompt: &str) -> CoreResult<String> {
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
        allow_custom_base_url: file.ai.allow_custom_base_url.unwrap_or(false),
        // 055 B5: 超时 —— 配了就校验(非法报错误解法), 没配回填默认 300 秒。
        timeout_secs: file.ai.timeout_secs.map(config::check_timeout_secs).transpose()?,
    };
    // 覆盖优先级与对话会话一致: 环境变量 > ricow.toml > 预设(Web 无命令行覆盖层)。
    let env_base_url = std::env::var(config::ENV_BASE_URL).ok();
    let env_model = std::env::var(config::ENV_MODEL).ok();
    let resolved = config::resolve(&cfg, env_base_url, env_model)?;

    // 缺密钥(远程端点)在此即返回 CoreError::Auth, 发生在任何网络请求之前。
    let api_key = resolve_key(&resolved.base_url, config::api_key(root, &resolved.provider))?;

    let llm = connect(resolved, &api_key, preamble, Vec::new())?;
    let answer = llm.ask(prompt).await?;
    Ok(answer.text)
}

/// 032 US3 (FR-018): Web「AI 改 Lua」的一次性非流式问答薄封装。
pub(crate) async fn quick_ask(
    root: &Path,
    instruction: &str,
    lua_code: &str,
    manifest_summary: &str,
) -> CoreResult<String> {
    one_shot(root, AI_EDIT_PREAMBLE, &build_edit_prompt(instruction, lua_code, manifest_summary))
        .await
}

/// P0-3: Web「AI 生成新策略」的一次性非流式问答薄封装 —— 提示词里带**全文** Lua API 规范。
pub(crate) async fn quick_generate(root: &Path, prompt: &str) -> CoreResult<String> {
    one_shot(root, AI_GEN_PREAMBLE, prompt).await
}

/// 052 方案 3: 构造「AI 调参数」的用户提示词(纯函数便于离线单测)。
///
/// 与 [`build_edit_prompt`] **任务不同**: 这里不产出代码, 只产出 `{键: 值}`。提示词给出
/// 清单摘要(可用键的**唯一来源**)与实例当前值(让模型只动用户点到的那几个), 并硬性要求
/// "键只能取清单里的" —— 回包那一侧还有一道键过滤做双保险(见 `strategy_io`)。
pub(crate) fn build_params_prompt(
    instruction: &str,
    manifest_summary: &str,
    current_values: &str,
    history: &str,
    initial_cash: Option<f64>,
) -> String {
    let history_block = if history.trim().is_empty() {
        "(这是第一轮)\n".to_string()
    } else {
        format!("{history}\n")
    };
    let body = format!(
        "用户想要的效果在下面最末一行。他**不会**说改哪个参数、改成多少 —— 你自己判断。\n\
         \n\
         ## 可用的参数(键名 / 类型 / 说明 —— 只能用这里出现过的键)\n\
         {manifest_summary}\n\
         \n\
         ## 当前取值\n\
         {current_values}\n\
         \n\
         ## 之前的对话(帮你接住\"再保守点\"这类跟进的话)\n\
         {history_block}\n\
         ## 用户这次的话\n\
         {instruction}\n\
         \n\
         ## 输出要求(必须严格遵守)\n\
         1. 先用中文讲清三件事(**每件一行**, 不要寒暄、不要复述用户的话):\n\
            - 打算怎么改: 一句话说清整体方向(如\"整体调保守: 放宽间距、压低单笔资金\");\n\
            - 逐项理由: 每个被改的参数为什么这么改 —— **只能依据**上面的参数说明与参数含义;\n\
            - 风险: 这么改要付出什么代价(如成交变少、单边行情更容易踏空、回撤未必真的变小);\n\
         2. 上面清单里**没写说明**的参数, 如果你仍要动它, 必须如实标注\"该参数清单里没有说明, \
         下面是按参数名与代码推断的\" —— 不许把推断说得像有出处;\n\
         3. 讲完之后**另起一行**给出一个 JSON 对象, 形如 {{\"spacing_pct\": 0.02, ...}};\n\
         4. JSON 里给的是**整组**参数(含你没改的, 保持当前值), 不是只给改动的几个;\n\
         5. 键**只能**取上面清单里出现过的键名, 绝不发明新键; 值必须与类型一致\
         (数值 / 字符串 / 布尔), 枚举只能取它列出的可选值;\n\
         6. JSON 之后不要再写任何内容, 也不要 Markdown 围栏。"
    );
    // 058: 资金量段插在输出要求之前 —— 参数顾问必须先判「这个钱数跑不跑得动」再谈调参。
    append_cash_section(&body, initial_cash)
}

/// 052 方案 3: Web「AI 调参数」的一次性非流式问答薄封装 —— 回包是 JSON, 全程不落盘。
pub(crate) async fn quick_params(
    root: &Path,
    instruction: &str,
    manifest_summary: &str,
    current_values: &str,
    history: &str,
    initial_cash: Option<f64>,
) -> CoreResult<String> {
    one_shot(
        root,
        AI_PARAMS_PREAMBLE,
        &build_params_prompt(instruction, manifest_summary, current_values, history, initial_cash),
    )
    .await
}

/// 058: 资金量级提醒的**知识段**(编译期常量, 供 `build_cash_advice` 拼进提示词)。
///
/// 为什么要有这一段(真跑出来的, 不是推演): 2026-10-18 在 demo 测试网真跑阶段 0,
/// 同一个内置策略 `shannon_grid` 换资金量级跑出两种结果:
/// - `--cash 100000`(默认) → **21 笔成交**, 网格完整铺开, 净 +6.07;
/// - `--cash 300`(手册建议的小资金下限) → **只成交 1 笔**(建仓那一笔), 之后一单挂不出 ——
///   建仓吃掉一半资金后只剩 149.85, 达不到下一档的最小名义。
///   结论: **同样一套参数, 资金量不够时策略是"跑不起来"的, 不是"收益差一点"**。不把这件事
///   写进提示词, AI 就会照着默认 10 万的直觉给建议, 用户小额跟单时才发现只成交 1 笔。
///
/// 边界(D15/D17: 平台不做投资判断): 这里判的是**可行性**(这个资金量能不能让策略正常运转),
/// **不是**"该投多少钱"、也不是该不该用这个策略。绝不改成收益承诺或仓位推荐。
const CASH_TIER_GUIDE: &str = "\
1. 本金低于 1000 USDT: 网格/分批挂单类策略很可能**跑不起来** —— 建仓就吃掉大半资金, 剩下的钱挂不出下一档, \
表现为\"只有 1 笔成交\"。此时应把单笔金额压到本金的 1%~2%, 或直接建议改用小资金友好的策略形态 \
(如 paired_grid 这类按固定间距挂单、单笔金额小的), 并如实说明原形态在这个资金量下为什么不行。\
2. 本金在 1000~10000 USDT: 多数网格策略可运行, 但单笔金额必须控制在**本金的一两个百分点**以内, \
否则一档就把可用资金吃掉大半; 手续费占比也会明显高于大本金, 要提醒用户这一点。\
3. 本金在 10000~100000 USDT: 常规网格参数基本可用, 重点看单笔金额是否与本金匹配。\
4. 本金高于 100000 USDT: 单笔金额不构成约束, 可以谈间距与档位密度。";

/// 058: 按资金量拼一段给模型看的**可行性提醒**(纯函数, 便于离线单测)。
///
/// 只回答「这个资金量能不能让策略正常跑起来」, 不回答「该投多少」——后者是投资判断,
/// 平台刻意不做(宪法 D15/D17)。`initial_cash` 为 None(策略 TOML 没写过本金)时返回固定文案,
/// **明确告诉模型钱数未知**, 而不是替用户编一个 —— 编一个等于替他做投资决定
/// (与 `tools.rs` 里 `initial_cash`「必问项, 绝不替他填」同一条纪律)。
pub(crate) fn build_cash_advice(initial_cash: Option<f64>) -> String {
    let known = match initial_cash {
        Some(c) if c.is_finite() && c > 0.0 => {
            format!("这个策略准备的本金是 **{c:.2} USDT**。")
        }
        // 拿不到就是拿不到: 如实说不知道, 不猜一个数填进去。
        _ => "**不知道这个策略准备投入多少资金**(策略配置里没写)。\
如果接下来的建议依赖资金量(网格类、仓位类策略尤其依赖), 你必须先在说明里请用户填一下本金, \
不要按任何假设值给参数 —— 按猜的数给的参数会让他在小额跟单时才发现根本跑不起来。"
            .to_string(),
    };
    format!(
        "{known}\n\n\
         **必须先判可行性, 再谈调参**: 本金不够让策略正常运转时(典型症状: 回测只成交一两笔), \
         如实直说\"这个资金量跑不动这种形态\", 并给出可行的替代(压小单笔金额 / 换小资金友好的形态), \
         不要只微调参数就交差。\n\n\
         资金量级怎么影响可行性:\n{CASH_TIER_GUIDE}"
    )
}

/// 058: 把资金量段插进已有提示词(纯函数)。
///
/// 插在**输出要求之前**、上下文之后: 让模型先读到钱数与档位知识, 再按输出要求作答;
/// 放末尾会被当成"补充说明"而权重降低。
pub(crate) fn append_cash_section(prompt: &str, initial_cash: Option<f64>) -> String {
    let marker = "## 输出要求";
    let advice = build_cash_advice(initial_cash);
    match prompt.find(marker) {
        Some(idx) => {
            let (head, tail) = prompt.split_at(idx);
            format!("{head}## 资金量与可行性\n{advice}\n\n{tail}")
        }
        // 没有输出要求段(理论上不会发生): 追加在末尾, 不丢原文。
        None => format!("{prompt}\n\n## 资金量与可行性\n{advice}\n"),
    }
}

/// N3 (2026-10-12): Web「评估这组参数的影响」的系统提示 —— 输入是**实测回测指标**, 不是用户的话。
/// 与 [`AI_PARAMS_PREAMBLE`] 的分工: 那条负责"打算怎么改", 这条负责"实际改成了什么样"。
///
/// 末尾那两条口径知识是**真跑出来的教训**(2026-10-12 用 DeepSeek 实测): 模型第一次看到
/// 「改前 108 笔 / 改后 3 笔」时, 把两次基准收益不同(−0.16% vs +1.65%)当成"数据口径不一致",
/// 还把夏普 −9 → −737 当成重大恶化 —— 前者是**设计**(基准从首次成交起算), 后者是**比率指标
/// 在成交极少时失真**。不写进去, 模型就会拿自己的错误理解去质疑一份其实正确的实测结果。
const AI_ASSESS_PREAMBLE: &str =
    "你是 ricow 的策略参数评估助手。用户刚改了一组策略参数, 并且**真的跑了回测**; 你要基于给出的\
     两份实测指标, 用大白话讲清这次改动的实际影响。不写代码、不做行情预测、不调用任何工具; \
     **只能引用给出的指标数字**, 绝不编造没测过的数字; 拿不准的地方如实说拿不准。\n\
     \n\
     ## 两条必须知道的口径(不知道就会把设计误判成数据错误)\n\
     1. 基准收益是**从首次成交那一刻起算**的买入持有对照, 不是从回测窗口起点起算。改前与改后的\
     首次成交时点不同, 两次基准收益本就不同 —— 这是设计, **不要**据此怀疑「两次数据口径不一致」。\n\
     2. 成交笔数很少(个位数)或年化波动率接近 0 时, 夏普/索提诺这类比率指标会被极小的分母放大到\
     难以解释的量级。此时以**成交笔数与净盈亏的绝对额**为主判据; 比率指标照实报出, 但不拿它下\
     结论, 解释不了就明说解释不了, 不要编一个原因。";

/// N3 (2026-10-12): 构造「评估改动影响」的用户提示词(纯函数便于离线单测)。
///
/// 三样输入缺一不可: 改了什么(键与前后取值)、改前实测、改后实测。少了任何一样, 结论就只能靠猜 ——
/// 而"猜出来的影响"正是这次要消灭的东西。
pub(crate) fn build_assess_prompt(
    changed_lines: &str,
    base_metrics: &str,
    new_metrics: &str,
    initial_cash: Option<f64>,
) -> String {
    let body = format!(
        "同一个策略、同一个回测窗口下, 改参数前后的实测结果如下。请讲清这次改动的**实际**影响。\n\
         \n\
         ## 这次动了哪些参数\n\
         {changed_lines}\n\
         \n\
         ## 改前的实测指标\n\
         {base_metrics}\n\
         \n\
         ## 改后的实测指标\n\
         {new_metrics}\n\
         \n\
         ## 输出要求(必须严格遵守)\n\
         1. 先用一句话给结论: 这组改动**实际**让策略变得更保守了还是更激进了, 依据是哪个指标;\n\
         2. 再逐项对比**关键**指标(如最大回撤、年化波动、夏普、成交笔数、净盈亏), 每一句都要带上\
         改前→改后的数字, 形如\"最大回撤 −18% → −9%\";\n\
         3. 如果有指标变差了(如收益下降、夏普下降), **必须直说** —— 不要只挑好看的说;\n\
         4. 只讲上面两份指标里有的数字, **绝不**推算或编造没给出的数字;\n\
         5. 最后提醒一句: 这只是**一段历史窗口**的回测结果, 不代表未来。"
    );
    // 058: 资金量段插在输出要求之前 —— 评估里最容易漏判的正是「成交笔数极少」背后的资金原因
    // (实测: 同一策略 10 万跑出 21 笔, 300 USDT 只成交 1 笔, 只看指标会误判成"参数改坏了")。
    append_cash_section(&body, initial_cash)
}

/// N3 (2026-10-12): Web「评估改动影响」的一次性非流式问答薄封装 —— 回包是自然语言, 不落盘。
pub(crate) async fn quick_assess(root: &Path, prompt: &str) -> CoreResult<String> {
    one_shot(root, AI_ASSESS_PREAMBLE, prompt).await
}

/// N2-C (2026-10-12): 构造「给参数起中文名 + 写一句说明」的用户提示词(纯函数便于离线单测)。
///
/// 与 [`build_params_prompt`] **任务不同**: 那个是改**取值**, 这个是补**名字与说明**。
/// 必须把脚本全文一并给它 —— 名字只能从"这个键在代码里怎么被读、被拿去算什么"推出来;
/// 只给一串键名就让它起名, 等于让它按词形猜, 那样出来的名字是编的。
pub(crate) fn build_names_prompt(key_lines: &str, code: &str) -> String {
    format!(
        "下面是一份 ricow Lua 策略脚本, 以及它读取的参数键。请为每个键给出**中文名**与**一句说明**。\n\
         \n\
         ## 待命名的参数键\n\
         {key_lines}\n\
         \n\
         ## 脚本全文(命名与说明的**唯一依据**)\n\
         ```lua\n\
         {code}\n\
         ```\n\
         \n\
         ## 输出要求(必须严格遵守)\n\
         1. 只输出**一个 JSON 对象**, 形如 \
         {{\"spacing_pct\": {{\"name\": \"间距百分比\", \"desc\": \"上下挂单的间距\"}}}}, \
         不要输出任何解释、前后缀或 Markdown 围栏;\n\
         2. 键**只能**用上面列出的那些, 一个不多一个不少, 绝不发明新键;\n\
         3. 中文名要短(不超过 10 个字); 说明一句话(不超过 40 个字), 说清这个参数控制什么;\n\
         4. **只能依据脚本代码**: 代码里看不出用途的键, 说明就写「代码中未见其用途」, \
         不要编一个听起来合理的解释 —— 用户会当真。"
    )
}

/// N2-C (2026-10-12): Web「给参数起中文名」的一次性非流式问答薄封装 —— 回包是 JSON, 不落盘。
pub(crate) async fn quick_names(root: &Path, prompt: &str) -> CoreResult<String> {
    one_shot(root, AI_NAMES_PREAMBLE, prompt).await
}

// ---- 054 T37: 生成后补「一句话摘要 + 参数中文名」(2026-10-13) ----

/// 054 T37: 这条任务**单开一个**系统提示, 不用 [`AI_NAMES_PREAMBLE`]。
///
/// 差别只有一条: 那条契约是「只能输出参数键」—— 前端直接拿回包去覆盖清单行, 多一个键都不许有;
/// 这里除了参数键还要一个 `_summary`。混用会破坏前者的契约(前端会以为冒出一个参数), 所以另起一条。
/// 其余纪律与那条完全一致: 只依据代码、不许编、只输出一个 JSON 对象。
const AI_META_PREAMBLE: &str =
    "你是 ricow 的策略说明助手。你只负责根据**脚本代码**做两件事: 写一句「这份策略在做什么」的\
     摘要, 以及为给定的参数键起中文名并写一句说明。不改代码、不改取值、不做行情分析、不调用任何\
     工具; 参数键只能用用户消息里列出的那些(外加固定字段 _summary), 绝不发明新键; \
     代码里没有实现的东西不许写进摘要; 代码里看不出用途的键要如实说看不出, 不得编一个听起来\
     合理的解释; 最终只输出一个 JSON 对象, 不加任何解释或 Markdown 围栏。";

/// 054 T37: 构造「生成后补摘要 + 参数中文名」的用户提示词(纯函数便于离线单测)。
///
/// 与 [`build_names_prompt`] 的关键差别是**摘要的措辞纪律**: 摘要会被当作"这份策略在做什么"
/// 展示给用户, 而模型刚写完代码、最容易照着用户的原始想法复述一遍 —— 想法里有、代码里没落
/// 地的部分一旦写进摘要, 详情页就在骗人。所以提示词把"想法"降级成背景, 把"代码"立成唯一依据。
pub(crate) fn build_generate_meta_prompt(
    market: &str,
    pair: &str,
    interval: &str,
    idea: &str,
    key_lines: &str,
    code: &str,
) -> String {
    format!(
        "下面是一份刚生成的 ricow Lua 策略脚本, 以及它读取的参数键。请做两件事: \
         写一句摘要, 并为每个参数键起中文名、写一句说明。\n\
         \n\
         ## 背景: 用户原本想要什么(**只作背景, 不代表代码一定做到了**)\n\
         市场 {market} · 标的 {pair} · 主时钟 {interval}\n\
         {idea}\n\
         \n\
         ## 待命名的参数键\n\
         {key_lines}\n\
         \n\
         ## 脚本全文(摘要与命名的**唯一依据**)\n\
         ```lua\n\
         {code}\n\
         ```\n\
         \n\
         ## 输出要求(必须严格遵守)\n\
         1. 只输出**一个 JSON 对象**, 不要输出任何解释、前后缀或 Markdown 围栏;\n\
         2. 固定带一个 \"_summary\" 字段: 一句话(不超过 40 个字)说清**这份代码实际在做什么** —— \
         什么条件下进、什么条件下出、靠什么赚钱;\n\
         3. 其余字段是上面列出的参数键, 形如 \
         {{\"spacing_pct\": {{\"name\": \"间距百分比\", \"desc\": \"上下挂单的间距\"}}}};\n\
         4. 参数键**只能**用列出的那些, 一个不多一个不少; 中文名不超过 10 个字, 说明不超过 40 个字;\n\
         5. **代码没写的东西不许写进摘要**: 上面「用户原本想要什么」里有、代码里没落地的, 就当它\
         不存在 —— 这句摘要会被直接展示给用户当成「这份策略在做什么」, 写多了就是骗人;\n\
         6. **只能依据脚本代码**: 代码里看不出用途的键, 说明就写「代码中未见其用途」, \
         不要编一个听起来合理的解释 —— 用户会当真。"
    )
}

/// 054 T37: 同上, 一次性非流式问答薄封装 —— 回包是 JSON, 不落盘。
pub(crate) async fn quick_meta(root: &Path, prompt: &str) -> CoreResult<String> {
    one_shot(root, AI_META_PREAMBLE, prompt).await
}

/// P0-3 构造「AI 生成新策略」的用户提示词(纯函数便于离线单测)。
///
/// 结构: ① 规范全文(LUA_API_DOC, 唯一 API 依据); ② 结构化的策略意图(市场/标的/主时钟/
/// 思路/风控约束); ③ 输出要求(回调结构、need_klines 声明、参数读取辅助、订单字段)。
/// `risk_text`: 054 P2 —— 由风险档位映射出的**具体数字**约束(见
/// `web::strategy_io::risk_profile::constraints_text`)。
///
/// 必须放在"策略意图"里、且排在自由文本 `constraints` **之前**: 用户随口写的一句"稳一点"
/// 不该盖掉换算出来的数字; 反过来, 数字也不删掉用户原话(两者可能互补)。
/// 传空串表示调用方没有结构化输入(对话链路), 此时该小节不出现。
pub(crate) fn build_generate_prompt(
    market: &str,
    pair: &str,
    interval: &str,
    idea: &str,
    constraints: &str,
    risk_text: &str,
) -> String {
    let risk_section = if risk_text.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n         ## 投资约束(上面的自然语言已换算成具体数字; 两者冲突时**以本节为准**)\n\
             {risk_text}\n\
         ## 本节数字必须落到代码里\n\
         - 单笔仓位 / 回撤 / 间距这类「目标」要写成**可执行的代码**(下单前先算额度、超了就不下), \
         不能只写在注释里;\n\
         - 杠杆上限由配置保证(平台会真的校验), 你只需**不写出依赖更高杠杆的逻辑**。\n"
        )
    };
    format!(
        "请根据下面的策略意图, 从零编写一份 ricow Lua 交易策略。\n\
         \n\
         ## ricow Lua API 规范(唯一接口依据, 必须先通读)\n\
         {LUA_API_DOC}\n\
         \n\
         ## 策略意图\n\
         - 市场: {market}\n\
         - 交易对: {pair}\n\
         - 主时钟周期: {interval}\n\
         - 策略思路: {idea}\n\
         - 风控与约束: {constraints}\n\
         {risk_section}\
         ## 输出要求(必须严格遵守)\n\
         1. 只输出**一份**完整 Lua 代码(包含 on_tick; 需要 on_init/on_fill/on_stop 就一并给出);\n\
         2. 不要输出 ```lua 之类的代码围栏, 不要输出任何解释、前后缀、寒暄或 Markdown;\n\
         3. 在 on_init 里用 ctx:need_klines 声明数据需求(primary 为主时钟, aux 为高周期指标);\n\
         4. 读参数一律用 num(ctx, key, 缺省) / cfg_str(ctx, key, 缺省) 辅助函数(先定义后用), \
         不要直接 ctx:config_f64(...) or 缺省(Lua or 陷阱: 未配置时读到 0 而非缺省);\n\
         5. 订单表字段为 pair/side/size/price/order_type/reduce_only/position_side, \
         price 省略即市价单; on_fill 里用 fill_price/fill_size(不是 price/size);\n\
         6. 交易对用参数 pair(不要写死), 数量/价格要做有限性与正数校验;\n\
         7. 代码要能在震荡与趋势两种行情下都安全: 单边行情不许无限加仓, \
         需要有止损或仓位上限等保护(依据上面的风控约束)。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 054 P2/T19: 结构化约束段必须**原样**进提示词, 且空串时**整段不出现**
    /// (对话链路没有结构化输入, 不该冒出一节空标题让模型去猜)。
    #[test]
    fn test_generate_prompt_carries_risk_section_only_when_present() {
        let with =
            build_generate_prompt("futures", "ETHUSDT", "1h", "网格", "无", "【风险档位】激进");
        assert!(with.contains("投资约束"), "结构化约束段必须出现: {with}");
        assert!(with.contains("【风险档位】激进"), "约束原文必须逐字进提示词: {with}");
        // 冲突时以本节为准 —— 否则模型会挑一句好听的照做。
        assert!(with.contains("以本节为准"), "必须写明优先级: {with}");
        // 光给数字不够: 还得要求它落到代码里(否则只是提示词装饰)。
        assert!(with.contains("必须落到代码里"), "必须要求约束可执行: {with}");

        let without = build_generate_prompt("spot", "ETHUSDT", "1h", "网格", "无", "");
        assert!(!without.contains("投资约束"), "空约束不该留空节: {without}");
        assert!(without.contains("风控与约束"), "自然语言约束段仍在: {without}");
    }

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

    // ---- N3: 参数顾问 / 影响评估的提示词 ----

    /// 参数顾问提示词: 三块输入(清单 / 当前值 / 历史)都要在, 并且**必须**要求模型
    /// 先讲理由与风险、说明为空的参数要标注 —— 这几条是 N3 的核心要求, 掉了就退回"只给数字"。
    #[test]
    fn test_build_params_prompt_requires_reasoning_and_risk() {
        let manifest = "- 间距百分比 (键名 spacing_pct, 类型 f64): 网格间距(小数)";
        let current = "- spacing_pct = 0.01\n";
        let p = build_params_prompt("保守一点", manifest, current, "", Some(10000.0));
        assert!(p.contains("保守一点"), "用户的话必须原样进入: {p}");
        assert!(p.contains(manifest) && p.contains(current), "清单与当前值都要给模型: {p}");
        assert!(p.contains("(这是第一轮)"), "没有历史时要明说这是第一轮: {p}");
        assert!(p.contains("风险"), "必须要求讲风险: {p}");
        assert!(p.contains("逐项理由"), "必须要求逐项理由: {p}");
        assert!(p.contains("清单里没有说明"), "说明为空的参数必须要求如实标注: {p}");
        assert!(p.contains("整组"), "JSON 要给整组, 不是只给改动的几个: {p}");
        assert!(p.contains("绝不发明新键"), "禁止编键: {p}");

        // 有历史时: 历史原样进提示词, 且不再出现"第一轮"。
        let p = build_params_prompt(
            "再保守点",
            manifest,
            current,
            "用户: 保守点\nAI: 我把间距放宽了。",
            Some(10000.0),
        );
        assert!(p.contains("用户: 保守点") && p.contains("我把间距放宽了"), "{p}");
        assert!(!p.contains("(这是第一轮)"), "{p}");
    }

    /// 影响评估提示词: 改动清单 + 两份实测指标; 必须要求带上数字对比、直说变差的项、
    /// 不许编造没给出的数字。
    #[test]
    fn test_build_assess_prompt_demands_measured_numbers_only() {
        let changed = "- 间距百分比 (spacing_pct): 0.01 → 0.02\n";
        let base = "- 最大回撤 %: 18\n- 成交笔数: 40\n";
        let new = "- 最大回撤 %: 9\n- 成交笔数: 12\n";
        let p = build_assess_prompt(changed, base, new, Some(300.0));
        assert!(p.contains(changed) && p.contains(base) && p.contains(new), "{p}");
        assert!(p.contains("改前→改后"), "必须要求带前后数字: {p}");
        assert!(p.contains("必须直说"), "变差的地方必须直说: {p}");
        assert!(p.contains("绝不") && p.contains("编造"), "不许编造没给出的数字: {p}");
        assert!(p.contains("一段历史窗口"), "必须提醒窗口局限: {p}");
    }

    /// 054 T37: 「补摘要 + 命名」的提示词必须把**代码**立成唯一依据、把用户想法降级成背景 ——
    /// 否则模型会照着用户原话复述一遍, 把"想要但没做到"的部分写成摘要, 详情页就在骗人。
    #[test]
    fn test_build_generate_meta_prompt_binds_summary_to_code_not_to_idea() {
        let p = build_generate_meta_prompt(
            "spot",
            "ETHUSDT",
            "1h",
            "跌了分批买, 涨了分批卖, 还要能自动止损",
            "- spacing_pct",
            "local a = num(ctx, \"spacing_pct\", 0.01)\n",
        );
        assert!(p.contains("_summary"), "必须点名要 _summary 字段: {p}");
        assert!(p.contains("ETHUSDT") && p.contains("自动止损"), "原始想法要作为背景带进去: {p}");
        assert!(p.contains("只作背景"), "想法必须被降级成背景: {p}");
        assert!(p.contains("唯一依据"), "代码必须是唯一依据: {p}");
        assert!(p.contains("不许写进摘要"), "没落地的不能写进摘要: {p}");
        assert!(p.contains("spacing_pct") && p.contains("num(ctx"), "参数键与全文都要给: {p}");
    }

    /// 053: 评估的系统提示必须自带两条口径 —— 实测中模型正是缺了它们才把**设计**误判成
    /// **数据错误**(两次基准收益不同 → 怀疑口径不一致; 成交 3 笔 → 夏普 −737 当成重大恶化)。
    /// 这两条锁在提示词里, 不是模型该自己推的东西。
    #[test]
    fn test_assess_preamble_states_benchmark_and_ratio_caveats() {
        assert!(
            AI_ASSESS_PREAMBLE.contains("首次成交") && AI_ASSESS_PREAMBLE.contains("基准收益"),
            "必须说清基准从首次成交起算: {AI_ASSESS_PREAMBLE}"
        );
        assert!(
            AI_ASSESS_PREAMBLE.contains("不是从回测窗口起点起算"),
            "必须明说它不是窗口起点起算, 否则模型会当成数据不一致: {AI_ASSESS_PREAMBLE}"
        );
        assert!(
            AI_ASSESS_PREAMBLE.contains("成交笔数很少") && AI_ASSESS_PREAMBLE.contains("夏普"),
            "必须说清成交极少时比率指标失真: {AI_ASSESS_PREAMBLE}"
        );
        assert!(
            AI_ASSESS_PREAMBLE.contains("不要编一个原因"),
            "解释不了时只准如实说, 不许编原因: {AI_ASSESS_PREAMBLE}"
        );
    }

    // ---- 058: 资金量级提醒 ----

    /// 058 的核心: 提示词里**必须出现用户真实的本金数字**。
    /// 不带的话 AI 会按内置默认 10 万的直觉给建议, 用户小额跟单才发现只成交 1 笔
    /// (实测: 同一策略 10 万成交 21 笔 / 300 USDT 成交 1 笔)。
    #[test]
    fn test_cash_advice_carries_the_real_initial_cash() {
        let p = build_cash_advice(Some(300.0));
        assert!(p.contains("300.00"), "本金数字必须原样进入提示词: {p}");
        assert!(p.contains("USDT"), "必须带计价单位: {p}");
        // 必须落在"输出要求"之前: 放末尾会被模型当成补充说明而降低权重。
        let full =
            build_assess_prompt("改了什么", "- 成交笔数: 1\n", "- 成交笔数: 20\n", Some(300.0));
        let cash_at = full.find("## 资金量与可行性").expect("必须插入资金量段");
        let req_at = full.find("## 输出要求").expect("必须有输出要求段");
        assert!(cash_at < req_at, "资金量段必须在输出要求之前: {full}");
    }

    /// 拿不到本金时**如实说不知道**, 不许编一个数 —— 编一个等于替用户做投资决定
    /// (与 `tools.rs` 的 `initial_cash` 必问项同纪律)。也不能回退成内置默认 10 万。
    #[test]
    fn test_cash_advice_never_invents_a_number_when_unknown() {
        for unknown in [None, Some(0.0), Some(-100.0), Some(f64::NAN)] {
            let p = build_cash_advice(unknown);
            assert!(p.contains("不知道"), "必须明说不知道本金: {unknown:?} → {p}");
            assert!(p.contains("先"), "必须要求先问用户: {unknown:?} → {p}");
            assert!(
                !p.contains("100000.00"),
                "绝不能拿内置默认 10 万当用户的本金: {unknown:?} → {p}"
            );
        }
    }

    /// 档位知识必须真的写进去, 且守住 D15/D17 边界: 只判**可行性**, 不做投资建议。
    /// 这段一旦被"优化"成收益承诺或仓位推荐, 就是把平台拉回做投资判断。
    #[test]
    fn test_cash_tier_guide_judges_feasibility_not_investment() {
        assert!(CASH_TIER_GUIDE.contains("1000"), "必须有金额门槛: {CASH_TIER_GUIDE}");
        assert!(
            CASH_TIER_GUIDE.contains("paired_grid"),
            "小资金应指向小资金友好的形态: {CASH_TIER_GUIDE}"
        );
        // 禁止的话术: 承诺收益 / 劝用户加钱 / 替用户选策略
        for banned in ["建议投入", "推荐买入", "预期收益", "稳赚", "加仓到"] {
            assert!(
                !CASH_TIER_GUIDE.contains(banned),
                "不得出现投资建议话术「{banned}」: {CASH_TIER_GUIDE}"
            );
        }
        assert!(
            CASH_TIER_GUIDE.contains("跑不起来"),
            "必须点明本金不足是跑不动而非收益差: {CASH_TIER_GUIDE}"
        );
    }

    // ---- 3.4: LLM 重试判定 ----

    #[test]
    fn test_retryable_llm_errors_are_transient_only() {
        // 瞬时: 限流 / 服务端故障 / 传输层抖动
        for m in [
            "LLM 调用失败: 429 Too Many Requests",
            "LLM 流式调用中断: 503 Service Unavailable",
            "connection reset by peer",
        ] {
            assert!(is_retryable_llm_error(m), "应判可重试: {m}");
        }
        // 一票否决: 重试一万次也一样 —— 卷进重试只会让用户白等
        for m in [
            "LLM 调用失败: 401 Unauthorized",
            "invalid api key",
            "LLM 调用失败: 400 Bad Request",
            "model not found (403)",
            "LLM 调用失败: 422 Unprocessable Entity",
        ] {
            assert!(!is_retryable_llm_error(m), "不应重试: {m}");
        }
        // 无状态码的普通报错不重试(宁可漏判, 不要把无关错误卷进来)
        assert!(!is_retryable_llm_error("模型未按 Chat Completions 返回"));
    }

    /// 055 B5: **总超时不重试** —— 一次就等满 N 秒, 退避重试两次是十几分钟没人等得起。
    #[test]
    fn test_ai_timeout_is_never_retried_but_is_explained() {
        for m in [
            "LLM 调用失败: error sending request for url: operation timed out",
            "request timeout",
            "TIMED OUT",
        ] {
            assert!(is_ai_timeout(m), "应识别为超时: {m}");
            assert!(!is_retryable_llm_error(m), "超时不该重试: {m}");
        }
        // 非超时的传输层抖动照旧可重试(别把整类软错误一起否掉)。
        assert!(is_retryable_llm_error("connection reset by peer"));
        assert!(!is_ai_timeout("connection reset by peer"));

        // 报错要说清三件事: 等了多久 / 不是本机的错 / 怎么调大。
        let msg = timeout_message(300);
        assert!(msg.contains("300"), "{msg}");
        assert!(msg.contains("timeout_secs"), "应给出怎么调: {msg}");
        assert!(msg.contains("900"), "应给出可填的样例值: {msg}");
    }

    /// 状态码必须按数字边界匹配: "1500 tokens" 里的 500 不算 HTTP 500。
    #[test]
    fn test_status_token_matches_on_digit_boundaries_only() {
        assert!(has_status_token("http 429", "429"));
        assert!(!has_status_token("输入 1429 tokens", "429"));
        assert!(!has_status_token("输入 1500 输出 20", "500"));
    }

    // ---- 3.4: 对话历史预算 ----

    fn rounds(n: usize, chars_per_msg: usize) -> Vec<Message> {
        let mut v = Vec::new();
        for i in 0..n {
            v.push(Message::user(format!("问{i} {}", "甲".repeat(chars_per_msg))));
            v.push(Message::assistant(format!("答{i} {}", "乙".repeat(chars_per_msg))));
        }
        v
    }

    #[test]
    fn test_compact_history_noop_when_within_budget() {
        let mut h = rounds(3, 10);
        let before = h.len();
        assert_eq!(compact_history(&mut h), 0, "远低于预算时不得动历史");
        assert_eq!(h.len(), before);
    }

    #[test]
    fn test_compact_history_folds_whole_rounds_and_leaves_digest() {
        // 每轮 ≈ 2*(cost)，构造出远超预算的历史。
        let per_msg = HISTORY_BUDGET_CHARS / 8;
        let mut h = rounds(20, per_msg);
        let folded = compact_history(&mut h);
        assert!(folded > 0, "超预算必须折叠");
        assert!(folded < 20, "至少要留下当前这一轮");

        let total: usize = h.iter().map(message_chars).sum();
        assert!(total <= HISTORY_BUDGET_CHARS, "折叠后必须在预算内: {total}");

        // 最前面是摘记(它自己是一条 user 消息), 所以"去掉摘记后"必须整轮成对 ——
        // 出现孤立 assistant 就是切错了边界。
        let first = h.first().unwrap();
        let digest = message_text(first).expect("摘记应是文本");
        assert!(digest.contains("对话历史摘要"), "{digest}");
        assert!(digest.contains("更早的"), "{digest}");
        let tail = &h[1..];
        let users = tail.iter().filter(|m| matches!(m, Message::User { .. })).count();
        let assistants = tail.iter().filter(|m| matches!(m, Message::Assistant { .. })).count();
        assert_eq!(users, assistants, "折叠后不得留下半个来回: {users} vs {assistants}");
        // 最近一轮必须原样保留(用户当下关心的就是这轮)。
        assert!(message_text(h.last().unwrap()).unwrap().contains("答19"), "最近一轮不得被折");
    }

    #[test]
    fn test_compact_history_keeps_single_round_even_if_oversized() {
        let mut h = rounds(1, HISTORY_BUDGET_CHARS);
        assert_eq!(compact_history(&mut h), 0, "只有一轮时没有\"更早\"可折, 不得把它折掉");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn test_compact_history_digest_lists_earlier_questions() {
        let per_msg = HISTORY_BUDGET_CHARS / 4;
        let mut h = rounds(12, per_msg);
        let folded = compact_history(&mut h);
        assert!(folded > 0);
        let digest = message_text(h.first().unwrap()).unwrap().to_string();
        assert!(digest.contains("问0"), "摘记应回列最早的提问: {digest}");
        assert!(digest.contains("从略"), "超出回列条数要给个交代: {digest}");
    }
}
