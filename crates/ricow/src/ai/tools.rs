//! 工具注册表 (019): L0 只读 + L1 虚拟。
//!
//! **结构边界**(spec FR-008 / FR-009, 2026-09-16 R3 修订): 本模块只注册 **L0 只读**与 **L1 虚拟**工具 ——
//! L1 虚拟可直调但**自身不产生写实结果**。写实动作分两类:
//! - `preview_strategy`: 生成预览(写一条预览记录), 落盘需确认;
//! - `request_write_confirmation`(R3): 只校验前提 + 渲染确认块 + 在会话内**登记**一条待确认动作,
//!   **不落盘、不起进程**; 真正执行由 REPL 宿主在用户当场逐字输入短语后调用引擎内核(见 `ai::confirm`)。
//!
//! 模型因此**始终没有写实工具调用面**(注册表里没有 deploy/start_demo/stop_live/close_all),
//! 连"直接调用"的入口都不存在。审批门 `ToolGuard` 再按本表白名单 fail-closed 放行一次, 是第二道同向保证。
//!
//! **与框架解耦**: 工具的 `name` / `description` / 参数 schema 与本模块的实现函数是唯一事实;
//! `dynamic_tools()` 只做 rig 构造。Phase 7 的 `ricow mcp`(rmcp) 复用同一份定义,
//! 避免"内置 AI 会做的事、外部 agent 做不了"的能力漂移。
//!
//! 输出纪律(FR-015 / FR-016): 所有工具输出过 `clamp_output` 截断 + `redact` 打码,
//! 截断必须显式标记(不静默丢内容)。

use std::path::PathBuf;

use rig::agent::{AgentHook, HookContext, ToolCall as ToolCallEvent, ToolCallAction};
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use crate::ai::confirm::{ActionKind, PendingAction, PendingSlot};
use crate::commands;
use crate::supervisor::ledger;

/// 工具运行上下文(只读事实 + 会话 pending 句柄)。不含任何凭据。
#[derive(Clone)]
pub struct ToolCtx {
    pub root: PathBuf,
    /// 是否处于可对话内确认的交互会话(stdin 是 tty 且为 REPL)。
    pub interactive: bool,
    /// 与 REPL 共享的待确认动作句柄(非交互/单次模式下永不被登记)。
    pub pending: PendingSlot,
}

impl ToolCtx {
    pub fn new(root: PathBuf, interactive: bool, pending: PendingSlot) -> Self {
        Self { root, interactive, pending }
    }
}

/// L0 只读工具白名单 —— 无副作用。
pub const READ_ONLY_TOOLS: [&str; 12] = [
    "list_strategies",
    "strategy_read",
    "read_doc",
    "run_backtest",
    "instance_status",
    "fills",
    "logs_tail",
    "market_ticker",
    "market_orderbook",
    "list_pairs",
    "list_templates",
    "read_template",
];

/// L1 虚拟工具白名单 —— 可直调, 但**自身不产生写实结果**。
///
/// - `preview_strategy` 写一条预览记录(不落盘);
/// - `request_write_confirmation` 只登记会话待确认(执行权在 REPL 宿主, 见 `ai::confirm`);
/// - `start_dry_run`/`stop_run` 仅作用于本地虚拟撮合档。
pub const VIRTUAL_TOOLS: [&str; 4] =
    ["preview_strategy", "request_write_confirmation", "start_dry_run", "stop_run"];

/// 工具是否被允许执行(L0 ∪ L1)。未列入一律拒绝(fail-closed)。
pub fn is_allowed(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name) || VIRTUAL_TOOLS.contains(&name)
}

/// 单个工具输出的字符上限(超出即截断并标记)。
pub const MAX_OUTPUT_CHARS: usize = 8_000;

/// 文档类输出(read_doc)的字符上限: 权威文档(lua-api.md≈14k / backtest.md≈13k)必须能整篇读到,
/// 否则模型只能拿到前 60%, exec 组件与完整示例丢失(019 G1)。仍有界 + 显式截断标记。
pub const DOC_MAX_OUTPUT_CHARS: usize = 20_000;

/// 截断输出: 字符级(不切坏多字节), 且必须显式标记被截掉的部分。
pub fn clamp_output(text: impl Into<String>) -> String {
    clamp_output_limited(text, MAX_OUTPUT_CHARS)
}

/// 按指定上限截断(read_doc 用 [DOC_MAX_OUTPUT_CHARS], 其余用 [MAX_OUTPUT_CHARS])。
pub fn clamp_output_limited(text: impl Into<String>, limit: usize) -> String {
    let text = text.into();
    let total = text.chars().count();
    if total <= limit {
        return text;
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}\n\n[输出已截断: 共 {total} 字符, 只显示前 {limit} 字符]")
}

/// 凭据打码: 工具输出(尤其日志原文)可能夹带密钥, 出模型上下文前再过一遍。
///
/// 只替换 token 本身, **原样保留空白**(缩进/多空格/制表符): 打码是安全动作, 不该顺手把
/// 日志排版压平 —— 压平后缩进层级丢失, 模型读到的日志结构与会话里其它输出也对不上。
pub fn redact(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut masked_prev = false;
        let mut rebuilt = String::with_capacity(line.len());
        let mut rest = line;
        while !rest.is_empty() {
            let ws_len = rest.len() - rest.trim_start().len();
            if ws_len > 0 {
                rebuilt.push_str(&rest[..ws_len]);
                rest = &rest[ws_len..];
                continue;
            }
            let tok_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let token = &rest[..tok_len];
            if masked_prev {
                rebuilt.push_str("[已打码]");
            } else if is_secret_like(token) {
                rebuilt.push_str(&mask_secret(token));
            } else {
                rebuilt.push_str(token);
            }
            masked_prev = token.eq_ignore_ascii_case("bearer") || token.ends_with("Authorization:");
            rest = &rest[tok_len..];
        }
        out.push(rebuilt);
    }
    out.join("\n")
}

fn is_secret_like(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    const SECRET_KEYS: [&str; 6] = ["api_key", "apikey", "secret", "password", "passwd", "token"];
    if let Some((k, v)) = lower.split_once('=') {
        if SECRET_KEYS.contains(&k) && !v.is_empty() {
            return true;
        }
    }
    lower.starts_with("sk-") && lower.len() >= 12
}

fn mask_secret(token: &str) -> String {
    match token.split_once('=') {
        Some((k, _)) => format!("{k}=***"),
        None => "[已打码]".to_string(),
    }
}

/// 审批门: 只放行白名单工具(L0 只读 ∪ L1 虚拟), 其余一律 skip(fail-closed)。
///
/// 由于注册表内**根本没有写实工具**, 这道门的作用是防"模型自己编一个工具名"或
/// 将来有人误加工具时保持默认拒绝。
pub struct ToolGuard;

impl AgentHook for ToolGuard {
    async fn on_tool_call(&self, _ctx: &HookContext, event: ToolCallEvent<'_>) -> ToolCallAction {
        if is_allowed(event.tool_name) {
            ToolCallAction::run()
        } else {
            ToolCallAction::skip(format!(
                "工具 '{}' 不在 019 工具白名单内(L0 只读 + L1 虚拟), 已拒绝执行。可用工具: {}",
                event.tool_name,
                READ_ONLY_TOOLS
                    .iter()
                    .chain(VIRTUAL_TOOLS.iter())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// 工具实现
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> Result<String, ToolExecutionError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolExecutionError::invalid_args(format!("缺少参数 {key}(字符串)")))
}

/// 策略名安全校验: 只接受单段名字, 不允许路径分隔/上跳(防目录穿越)。
fn safe_strategy_name(name: &str) -> Result<(), ToolExecutionError> {
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.starts_with('.') {
        return Err(ToolExecutionError::invalid_args("策略名不能包含路径分隔符或 .."));
    }
    Ok(())
}

/// 列出某 strategies 目录下全部 *.toml 策略名(不存在 → 空)。会话数据目录用, 不走全局 ROOT。
pub(crate) fn list_toml_stems(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("toml") {
                p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

fn render_param(v: &ricow_strategy::ConfigValue) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(f) = v.as_f64() {
        return f.to_string();
    }
    if let Some(i) = v.as_i64() {
        return i.to_string();
    }
    if let Some(b) = v.as_bool() {
        return b.to_string();
    }
    "(未知类型)".into()
}

fn tool_list_strategies(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "list_strategies",
        "列出本机已部署的策略(名称/交易对/市场/是否开启实盘/Dry Run 起点等)。只读, 无参数。",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        move |_c, _args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // 一律按**会话 root** 取数(不用进程全局 root: 两者未必同一份数据目录)
                let names = list_toml_stems(&ctx.root.join("strategies"));
                if names.is_empty() {
                    return Ok(ToolOutput::text(
                        "当前没有任何已部署策略(strategies/ 目录为空)。新建需走三步: \
                         `ricow create` → `ricow approve` → `ricow deploy`。",
                    ));
                }
                let mut out =
                    format!("已部署策略 {} 个(数据目录 {}):\n", names.len(), ctx.root.display());
                for name in &names {
                    match commands::read_strategy_config_in(&ctx.root, name) {
                        Some(c) => {
                            let pair = c
                                .params
                                .get("pair")
                                .map(render_param)
                                .unwrap_or_else(|| "(未设置)".into());
                            let script = c
                                .params
                                .get("script_path")
                                .and_then(|v| v.as_str())
                                .unwrap_or("(内嵌 Lua)");
                            out.push_str(&format!(
                                "- {name}: 交易对={pair}, 市场={}, 持仓模式={}, 已启用={}, 允许实盘={}",
                                c.market, c.position_mode, c.enabled, c.live_enabled
                            ));
                            match &c.dry_run_started_at {
                                Some(t) => out.push_str(&format!(", Dry Run 起算={t}")),
                                None => out.push_str(", 未开始 Dry Run"),
                            }
                            out.push_str(&format!(", 脚本={script}\n"));
                        }
                        None => out.push_str(&format!("- {name}: (TOML 无法解析)\n")),
                    }
                }
                Ok(ToolOutput::text(clamp_output(redact(&out))))
            })
        },
    )
}

fn tool_strategy_read(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "strategy_read",
        "读取某个已部署策略的 Lua 源码与参数(用于解释、诊断、改写前的现状核对)。只读。",
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "策略名, 对应 strategies/<name>.toml" } },
            "required": ["name"],
            "additionalProperties": false
        }),
        move |_c, args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                safe_strategy_name(&name)?;
                // 同样以**会话 root** 为准(与 list_strategies/prepare_deploy 同源)
                let dir = ctx.root.join("strategies");
                let cfg = commands::load_strategy_toml(&dir, &name)
                    .map_err(|e| ToolExecutionError::other(format!("读取策略 {name} 失败: {e}")))?;
                let mut out = format!("策略 {name}:\n交易对/参数:\n");
                let mut keys: Vec<&String> = cfg.params.keys().collect();
                keys.sort();
                for k in keys {
                    if let Some(v) = cfg.params.get(k) {
                        out.push_str(&format!("  {k} = {}\n", render_param(v)));
                    }
                }
                out.push_str(&format!(
                    "市场={}, 持仓模式={}, 已启用={}, 允许实盘={}, Dry Run 起算={}\n",
                    cfg.market,
                    cfg.position_mode,
                    cfg.enabled,
                    cfg.live_enabled,
                    cfg.dry_run_started_at.as_deref().unwrap_or("(未开始)")
                ));
                if let Some(lua) = cfg.params.get("script").and_then(|v| v.as_str()) {
                    out.push_str("\nLua 源码:\n");
                    out.push_str(lua);
                }
                Ok(ToolOutput::text(clamp_output(redact(&out))))
            })
        },
    )
}

fn tool_read_doc(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "read_doc",
        "读取本产品的权威说明(策略 Lua API / 回测口径 / 风险披露 / 命令与门禁)。写策略或谈部署运行前先读。只读。",
        json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "enum": ["lua-api", "backtest", "risk", "commands"],
                    "description": "lua-api=策略可用的回调/指标/exec 组件; backtest=回测撮合与口径; risk=实盘风险披露与免责确认; commands=四档运行/落盘与实盘门禁/命令速查/易跑偏点"
                }
            },
            "required": ["topic"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let topic = arg_str(&args, "topic")?;
                let text = match topic.as_str() {
                    "lua-api" => crate::ai::prompt::STRATEGY_API_DOC.to_string(),
                    "backtest" => include_str!("../../../../specs/backtest.md").to_string(),
                    "risk" => ricow_engine::RISK_DISCLOSURE.to_string(),
                    // G2: 命令与门禁/易跑偏点与 `ricow agent-kit` 手册同源(同一份编译期常量)
                    "commands" => format!(
                        "{}\n{}",
                        crate::ai::prompt::GATES_GUIDE,
                        crate::ai::prompt::TRAPS_GUIDE
                    ),
                    other => {
                        return Err(ToolExecutionError::invalid_args(format!(
                            "未知 topic '{other}'; 可用: lua-api, backtest, risk, commands"
                        )))
                    }
                };
                // G1: 文档给专用上限(20k), 保证 lua-api/backtest 整篇可取; 仍有界并标记截断
                Ok(ToolOutput::text(clamp_output_limited(text, DOC_MAX_OUTPUT_CHARS)))
            })
        },
    )
}

fn tool_run_backtest(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "run_backtest",
        "对某个策略在真实历史 K 线上跑一次回测并返回报告(与 `ricow backtest` 同一条代码路径、同一份格式化)。\
只读: 不动资金、不落盘、不改配置。策略名可以是已部署策略名, 或内置名 shannon_grid/dca/twap/vwap/pullback/ladder。",
        json!({
            "type": "object",
            "properties": {
                "strategy": { "type": "string", "description": "已部署策略名或内置策略名" },
                "pair": { "type": "string", "description": "交易对, 如 ETHUSDT(策略 TOML 已含 pair 时可省略)" },
                "days": { "type": "integer", "description": "回测天数, 默认 90" },
                "interval": { "type": "string", "enum": ["1m", "5m", "15m", "1h", "4h", "1d"], "description": "K 线间隔, 默认 1h" },
                "market": { "type": "string", "enum": ["spot", "futures"], "description": "市场类型, 默认随策略配置" }
            },
            "required": ["strategy"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let strategy = arg_str(&args, "strategy")?;
                let mut a = commands::backtest::BacktestArgs { strategy, ..Default::default() };
                if let Some(p) = args.get("pair").and_then(|v| v.as_str()) {
                    a.pair = Some(p.trim().to_ascii_uppercase());
                }
                if let Some(d) = args.get("days").and_then(|v| v.as_i64()) {
                    a.days = Some(d.clamp(1, 3650) as u32);
                }
                if let Some(i) = args.get("interval").and_then(|v| v.as_str()) {
                    a.interval = Some(i.to_string());
                }
                if let Some(m) = args.get("market").and_then(|v| v.as_str()) {
                    a.market = Some(m.to_string());
                }
                let (text, _report) = commands::backtest::run_backtest(a)
                    .await
                    .map_err(|e| ToolExecutionError::other(format!("回测失败: {e}")))?;
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

fn tool_instance_status(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "instance_status",
        "查看策略实例状态。不带 name: 列出全部实例(是否运行/PID/运行时长/备注); \
带 name: 该策略的详细状态(成交笔数、资金费、日志路径, 运行中时附交易所账户快照)。只读。",
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "策略名(省略则列出全部)" } },
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let text = match name {
                    Some(n) => {
                        safe_strategy_name(&n)?;
                        commands::instances::format_info(commands::instances::InfoArgs { name: n })
                            .await
                    }
                    None => commands::instances::format_table().await,
                }
                .map_err(|e| ToolExecutionError::other(format!("查询实例状态失败: {e}")))?;
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

fn tool_fills(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "fills",
        "查看成交明细(落库记录)。可按策略名过滤, 默认最近 20 条。只读。",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "策略名(省略则全部策略)" },
                "limit": { "type": "integer", "description": "最多条数, 默认 20, 上限 200" }
            },
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if let Some(n) = &name {
                    safe_strategy_name(n)?;
                }
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20).clamp(1, 200);
                let text = commands::instances::format_fills(commands::instances::FillsArgs {
                    name,
                    limit,
                })
                .await
                .map_err(|e| ToolExecutionError::other(format!("查询成交失败: {e}")))?;
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

fn tool_logs_tail(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "logs_tail",
        "读取某个策略的日志尾部(默认 50 行, 上限 200)。日志是策略进程原样输出, 可能含余额/持仓等账户信息; \
敏感串会被打码。只读。",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "策略名" },
                "lines": { "type": "integer", "description": "最多行数, 默认 50, 上限 200" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        move |_c, args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                safe_strategy_name(&name)?;
                let want = args
                    .get("lines")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(50)
                    .clamp(1, 200) as usize;
                let path = ledger::log_path(&ctx.root, &name);
                let raw = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(_) => {
                        return Ok(ToolOutput::text(format!(
                            "无日志文件: {} (该策略从未启动过?)",
                            path.display()
                        )))
                    }
                };
                let all: Vec<&str> = raw.lines().collect();
                let start = all.len().saturating_sub(want);
                let mut out = format!(
                    "{} 尾部 {} 行(共 {} 行):\n",
                    path.display(),
                    all.len() - start,
                    all.len()
                );
                for l in &all[start..] {
                    out.push_str(l);
                    out.push('\n');
                }
                Ok(ToolOutput::text(clamp_output(redact(&out))))
            })
        },
    )
}

fn tool_market_ticker(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "market_ticker",
        "查询某交易对的实时中间价(公开行情, 只读)。交易对必须带计价币, 例如 ETHUSDT。",
        json!({
            "type": "object",
            "properties": { "pair": { "type": "string", "description": "交易对, 如 ETHUSDT" } },
            "required": ["pair"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let pair = arg_str(&args, "pair")?.to_ascii_uppercase();
                let exchange = commands::bn_exchange()
                    .map_err(|e| ToolExecutionError::other(format!("构造行情客户端失败: {e}")))?;
                let ob = exchange
                    .get_orderbook(&pair, 1)
                    .await
                    .map_err(|e| ToolExecutionError::other(format!("获取 {pair} 盘口失败: {e}")))?;
                match ob.mid_price() {
                    Some(mid) => Ok(ToolOutput::text(format!("{pair} 中间价: {mid}"))),
                    None => Ok(ToolOutput::text(format!("{pair} 无盘口数据(检查交易对拼写)"))),
                }
            })
        },
    )
}

fn tool_market_orderbook(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "market_orderbook",
        "查询某交易对的盘口前 N 档买卖价(公开行情, 只读)。",
        json!({
            "type": "object",
            "properties": {
                "pair": { "type": "string", "description": "交易对, 如 ETHUSDT" },
                "depth": { "type": "integer", "description": "档数, 默认 5, 上限 20" }
            },
            "required": ["pair"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let pair = arg_str(&args, "pair")?.to_ascii_uppercase();
                let depth =
                    args.get("depth").and_then(|v| v.as_i64()).unwrap_or(5).clamp(1, 20) as usize;
                let exchange = commands::bn_exchange()
                    .map_err(|e| ToolExecutionError::other(format!("构造行情客户端失败: {e}")))?;
                let ob = exchange
                    .get_orderbook(&pair, depth as u32)
                    .await
                    .map_err(|e| ToolExecutionError::other(format!("获取 {pair} 盘口失败: {e}")))?;
                let mut out = format!("{pair} 盘口(前 {depth} 档):\n");
                for b in ob.bids.iter().take(depth) {
                    out.push_str(&format!("  bid {} x {}\n", b.price, b.size));
                }
                for a in ob.asks.iter().take(depth) {
                    out.push_str(&format!("  ask {} x {}\n", a.price, a.size));
                }
                if let Some(mid) = ob.mid_price() {
                    out.push_str(&format!("  中间价 {mid}\n"));
                }
                Ok(ToolOutput::text(clamp_output(redact(&out))))
            })
        },
    )
}

/// L0 只读工具: 列出当前视野内的交易对(默认只股票类), 支持市场限定与子串检索。
fn tool_list_pairs(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "list_pairs",
        "列出**可交易的交易对清单**(免 key 公开数据, 只读)。默认只列股票类(bStock 美股代币现货 + 股票永续); 用户可用对话内 /market 切换为全部交易对。给策略挑交易对前先调用本工具, 不要凭记忆写交易对。",
        json!({
            "type": "object",
            "properties": {
                "market": { "type": "string", "enum": ["spot", "futures"], "description": "只看现货或只看合约; 省略 = 两组都列" },
                "q": { "type": "string", "description": "交易对子串检索(大小写不敏感), 如 AAPL" }
            },
            "additionalProperties": false
        }),
        move |_c, args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let opt = |k: &str| {
                    args.get(k)
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                };
                let (market, q) = (opt("market"), opt("q"));
                let view =
                    commands::pairs::lookup(&ctx.root, market.as_deref(), q.as_deref())
                        .await
                        .map_err(|e| {
                            ToolExecutionError::other(format!("获取交易对视野失败(需联网): {e}"))
                        })?;
                Ok(ToolOutput::text(clamp_output(commands::pairs::render(&view, q.as_deref()))))
            })
        },
    )
}

/// L0 只读工具: 列出内置模板(纯本地, 免网络)。
fn tool_list_templates(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "list_templates",
        "列出**内置策略模板**(编译期内置, 不用网络): 完整策略 1 个(shannon_grid) + 执行组件 5 个(dca/twap/vwap/pullback/ladder)。\
         用户说\"从模板建\"或不知道从哪开始时先调用本工具, 再让用户挑一个; 取原文用 read_template。",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        move |_c, _args| {
            Box::pin(async move {
                Ok(ToolOutput::text(clamp_output(commands::templates::list_text())))
            })
        },
    )
}

/// L0 只读工具: 读单个内置模板的原文与元数据(纯本地, 免网络)。
fn tool_read_template(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "read_template",
        "读取某个内置模板的**元数据 + Lua 原文**。\
         完整策略(shannon_grid)的原文可直接作为 preview_strategy 的 script 提交(参数按用户回答填); \
         执行组件(dca/twap/vwap/pullback/ladder)只是执行片段, 不是完整策略 —— 要落盘须先嵌入 on_tick 框架并补齐信号。",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "模板名, 取值见 list_templates(如 shannon_grid / dca)" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                let t = commands::templates::find(&name).ok_or_else(|| {
                    let mut known: Vec<&str> =
                        commands::templates::ALL.iter().map(|t| t.name).collect();
                    known.sort_unstable();
                    ToolExecutionError::invalid_args(format!(
                        "没有模板 '{name}'; 可用: {}",
                        known.join(" / ")
                    ))
                })?;
                Ok(ToolOutput::text(clamp_output(commands::templates::render_read(t))))
            })
        },
    )
}

/// L1 虚拟工具: 生成策略预览(编译门禁 → 真实 K 线沙箱回测 → preview_id)。
///
/// **不落盘、不碰资金**(只写一条 previews 记录); 落盘必须由用户本人 `approve` + `deploy`。
/// 与 CLI `ricow create` 共用 `commands::create::create_preview`(FR-016 同口径)。
fn tool_preview_strategy(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "preview_strategy",
        format!(
            "把一段 Lua 策略代码过一遍门禁并生成**预览**: 编译门禁 → 真实历史 K 线沙箱回测 → \
             preview_id({} 分钟一次性)。虚拟操作: 不写任何策略文件、不碰资金(会占用一条预览记录)。\
             落盘**必须由用户本人**执行 `ricow approve <preview_id>` 与 \
             `ricow deploy <preview_id> --token <token>`, 你只能把这两条命令告诉用户, 不得声称已部署。",
            // 与引擎 TTL 常量同源, 不手抄 15
            ricow_engine::PREVIEW_TTL_SECS / 60
        ),
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "策略名: 仅字母/数字/短横线/下划线, 长度≤24(如 eth-grid-300); 中文或空格会被拒绝" },
                "pair": { "type": "string", "description": "交易对, 如 ETHUSDT" },
                "script": { "type": "string", "description": "完整 Lua 策略代码(可带围栏); 写之前先 read_doc 取 lua-api" },
                "market": { "type": "string", "enum": ["spot", "futures"], "description": "市场, 默认 spot" },
                "days": { "type": "integer", "description": "沙箱回测天数, 默认 90" },
                "interval": { "type": "string", "enum": ["1m", "5m", "15m", "1h", "4h", "1d"], "description": "K 线间隔, 默认 1h" },
                "params": { "type": "object", "description": "策略参数(字符串/数字/布尔), 如 {\"order_size\": 0.01}" }
            },
            "required": ["name", "pair", "script"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                let pair = arg_str(&args, "pair")?.trim().to_ascii_uppercase();
                let script = arg_str(&args, "script")?;
                if script.trim().is_empty() {
                    return Err(ToolExecutionError::other("script 为空"));
                }
                let market =
                    args.get("market").and_then(|v| v.as_str()).unwrap_or("spot").to_string();
                let days = args.get("days").and_then(|v| v.as_i64()).unwrap_or(90).clamp(1, 3650) as u32;
                let interval =
                    args.get("interval").and_then(|v| v.as_str()).unwrap_or("1h").to_string();
                let mut params: Vec<(String, ricow_strategy::ConfigValue)> = Vec::new();
                if let Some(obj) = args.get("params").and_then(|v| v.as_object()) {
                    for (k, v) in obj {
                        let cv = if let Some(s) = v.as_str() {
                            ricow_strategy::ConfigValue::String(s.to_string())
                        } else if let Some(b) = v.as_bool() {
                            ricow_strategy::ConfigValue::Boolean(b)
                        } else if let Some(i) = v.as_i64() {
                            ricow_strategy::ConfigValue::Integer(i)
                        } else if let Some(f) = v.as_f64() {
                            ricow_strategy::ConfigValue::Float(f)
                        } else {
                            return Err(ToolExecutionError::other(format!(
                                "参数 {k} 类型不支持(仅字符串/数字/布尔)"
                            )));
                        };
                        params.push((k.clone(), cv));
                    }
                }
                let out = commands::create::create_preview(
                    &name, &script, &pair, &market, params, days, &interval,
                )
                .await
                .map_err(|e| ToolExecutionError::other(format!("生成预览失败: {e}")))?;
                let text = format!(
                    "{}\n编译门禁 ✓  沙箱回测 ✓  —— **尚未部署**(未写入任何策略文件)\npreview_id: {}\n下一步: 若用户要落盘, 调 request_write_confirmation(action=\"deploy\", preview_id=\"{}\") 生成确认块, 由用户本人逐字确认; 你没有落盘权限, 不要替用户执行 approve/deploy。",
                    out.report_text, out.preview_id, out.preview_id
                );
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

/// L1 虚拟工具(019 R3/R4): 发起对话内确认块 —— **只校验前提 + 渲染确认块 + 登记会话待办, 不落盘、不起进程**。
///
/// 真正执行由 REPL 宿主在用户当场逐字输入短语后调用引擎内核(`ai::confirm` 状态机)。
/// 单次模式与管道(stdin 非 tty)回退为终端命令指引。
fn tool_request_write_confirmation(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "request_write_confirmation",
        "当用户要求执行任一**写实动作**时调用: 校验前提 → 生成确认块 → 在本对话登记一条待确认动作(虚拟操作, 不执行任何写实动作)。\
七个 action: deploy=落盘部署某个 preview(需 preview_id); start_demo=以币安测试网启动已部署策略; \
ack_risk=首次实盘风险确认(无 name); start_live=以真实资金启动已部署策略; stop_demo=停止测试网实例; \
stop_live=停止实盘实例(不平仓); close_live=停止实盘并市价平仓(不可逆)。\
改已部署策略的脚本(deploy 撞名): 默认被拒绝, 默认路径是**换个新名**部署; 只有用户明确要求就改这个策略, \
才带 replace=true 走受控覆盖(覆盖前自动备份旧脚本, 用户须逐字输入 `确认覆盖 <name>`)。\
成功后必须把确认块原文与期望短语转告用户, 请其**本人逐字输入**; 不要替用户输入短语, 也不要替用户回车。",
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["deploy", "start_demo", "ack_risk", "start_live", "stop_demo", "stop_live", "close_live"],
                    "description": "要登记的动作种类(七选一); 实盘相关动作会跑与终端完全相同的三判据"
                },
                "preview_id": { "type": "string", "description": "action=deploy 时必填: preview_strategy 返回的 preview_id" },
                "name": { "type": "string", "description": "除 ack_risk 外均必填: 已部署策略名" },
                "replace": {
                    "type": "boolean",
                    "description": "仅 action=deploy 有效, 默认 false。false=同名已存在则拒绝(默认路径是换个新名部署)。true=受控覆盖同名已部署策略: 会先把旧脚本备份为新名 .bak, 用户须逐字输入 `确认覆盖 <name>`(与普通部署短语不同)。只有在用户明确要求改某个已存在策略的脚本时才置 true。"
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
        move |_c, args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let action = arg_str(&args, "action")?;
                let kind = match action.as_str() {
                    "deploy" => ActionKind::Deploy,
                    "start_demo" => ActionKind::StartDemo,
                    "ack_risk" => ActionKind::AckRisk,
                    "start_live" => ActionKind::StartLive,
                    "stop_demo" => ActionKind::StopDemo,
                    "stop_live" => ActionKind::StopLive,
                    "close_live" => ActionKind::CloseLive,
                    other => {
                        return Err(ToolExecutionError::invalid_args(format!(
                            "未知 action '{other}'; 仅支持 deploy / start_demo / ack_risk / start_live / stop_demo / stop_live / close_live"
                        )))
                    }
                };

                // D5: 非交互(单次提问/管道)不登记 pending, 直接给终端命令
                if !ctx.interactive {
                    return Ok(ToolOutput::text(non_interactive_hint(kind, &args)));
                }

                let action_entry = match kind {
                    ActionKind::Deploy => prepare_deploy(&ctx, &args).await?,
                    ActionKind::StartDemo => prepare_start_demo(&ctx, &args).await?,
                    ActionKind::AckRisk => prepare_ack_risk()?,
                    ActionKind::StartLive => prepare_start_live(&ctx, &args).await?,
                    ActionKind::StopDemo | ActionKind::StopLive | ActionKind::CloseLive => {
                        prepare_stop(&ctx, kind, &args).await?
                    }
                };

                let phrase = action_entry.action.expected_phrase();
                let mut slot = ctx.pending.lock().await;
                // 新请求覆盖旧 pending(旧的 deploy preview 未批准, 过了 PENDING_TTL 就自然过期)
                *slot = Some(action_entry.action.clone());
                drop(slot);

                Ok(ToolOutput::text(format!(
                    "{block}\n\
                     ———— 请在本对话逐字输入(复制即可): {phrase}\n\
                     放弃请输入: 拒绝\n\
                     注意: 确认短语只能由你本人输入, 我不会代填; 该待确认 {ttl} 分钟内有效。",
                    block = action_entry.block,
                    // 与 `ai::confirm::PENDING_TTL` 同源, 不手抄 15
                    ttl = crate::ai::confirm::PENDING_TTL.as_secs() / 60
                )))
            })
        },
    )
}

/// 工具校验后生成的待确认动作 + 要展示给用户的确认块。
#[derive(Debug)]
struct PreparedAction {
    action: PendingAction,
    block: String,
}

/// deploy 前提校验: preview 存在 / pending / 未过期 / 同名未部署。
///
/// **FR-044 受控覆盖**: 同名已部署时默认拒绝(默认路径 = 换个新名); 只有 `replace = true`
/// (且用户在确认块里逐字输入 `确认覆盖 <name>`)才放行, 由宿主编排"先备份旧脚本再覆盖"。
async fn prepare_deploy(ctx: &ToolCtx, args: &Value) -> Result<PreparedAction, ToolExecutionError> {
    let preview_id = arg_str(args, "preview_id")?;
    let replace = args.get("replace").and_then(|v| v.as_bool()).unwrap_or(false);
    let db = ricow_strategy::Database::open(&commands::db_path_in(&ctx.root))
        .await
        .map_err(|e| ToolExecutionError::other(format!("打开本地库失败: {e}")))?;
    let preview = ricow_engine::get_preview(&db, &preview_id)
        .await
        .map_err(|e| ToolExecutionError::other(format!("取预览失败: {e}")))?;
    if preview.kind != "strategy" {
        return Err(ToolExecutionError::other(format!(
            "preview {preview_id} 的类型是 {} 不是 strategy, 不能用于部署",
            preview.kind
        )));
    }
    if preview.status != "pending" {
        return Err(ToolExecutionError::other(format!(
            "preview {preview_id} 当前状态是 {}, 不是 pending(已批准/已消费/已拒绝均不能再次确认)",
            preview.status
        )));
    }
    if chrono::Utc::now().timestamp() > preview.expires_at {
        return Err(ToolExecutionError::other(format!(
            "preview {preview_id} 已过期({} 分钟一次性), 请重新生成预览",
            // 与引擎 TTL 常量同源, 不手抄 15
            ricow_engine::PREVIEW_TTL_SECS / 60
        )));
    }
    let cfg = ricow_strategy::StrategyConfig::from_toml(&preview.payload_json)
        .map_err(|e| ToolExecutionError::other(format!("预览载荷解析失败: {e}")))?;
    let name = cfg.name.trim().to_string();
    if name.is_empty() {
        return Err(ToolExecutionError::other("预览载荷缺少策略名".to_string()));
    }
    let exists = list_toml_stems(&ctx.root.join("strategies")).iter().any(|n| n == &name);
    if exists && !replace {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 已存在: 同名部署默认被拒绝(不覆盖、不静默改名)。\
             **默认路径是换个新名**: 例如 {suggested}。\
             若用户确实要改这个策略的脚本, 请先把\"会覆盖 {name}, 且旧脚本会先备份为 \
             {name}.lua.<时间戳>.bak\"讲清楚并征得明确同意, 再带 replace=true 重新调用本工具 \
             (用户届时需逐字输入 `确认覆盖 {name}`, 与普通部署的短语不同)。",
            suggested = ricow_strategy::suggest_strategy_name(&name)
        )));
    }
    let base = crate::commands::approve::confirmation_block(&preview.kind, &preview.payload_json);
    if !exists {
        return Ok(PreparedAction {
            action: PendingAction::new_deploy(name, preview_id),
            block: base,
        });
    }
    // FR-044: 覆盖是破坏性动作 —— 确认块必须写清"覆盖谁 / 旧脚本去哪 / 何时生效"。
    let running = crate::commands::instances::views(&ctx.root)
        .await
        .iter()
        .any(|v| v.name == name && v.running);
    let running_note = if running {
        format!(
            "注意: {name} **正在运行** —— 运行中的实例执行的是启动时载入内存的**旧**代码, \
             本次覆盖不会热更新; 要让新脚本生效须 `ricow restart {name}`(重启按原模式重过门禁)。\n"
        )
    } else {
        String::new()
    };
    let block = format!(
        "———— 确认块(受控覆盖 FR-044)————\n\
         动作: **覆盖**已部署策略 [{name}] 的脚本与参数\n\
         备份: 覆盖前先把 strategies/{name}.lua → strategies/{name}.lua.<时间戳>.bak, \
         strategies/{name}.toml → strategies/{name}.toml.<时间戳>.bak(**备份失败即中止, 不覆盖**)\n\
         后果: strategies/{name}.toml 与 strategies/{name}.lua 被替换为新脚本; 旧脚本可从 .bak 恢复\n\
         {running_note}\
         建议: 若只是想保留旧策略对比, 请改用新名部署(默认路径)。\n\
         {base}"
    );
    Ok(PreparedAction { action: PendingAction::new_deploy_replace(name, preview_id), block })
}

/// start_demo 前提校验: 已部署 / 当前未运行 / demo 凭据已配置(缺则如实点名)。
async fn prepare_start_demo(
    ctx: &ToolCtx,
    args: &Value,
) -> Result<PreparedAction, ToolExecutionError> {
    let name = arg_str(args, "name")?;
    safe_strategy_name(&name)?;
    if !list_toml_stems(&ctx.root.join("strategies")).iter().any(|n| n == &name) {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 尚未部署(strategies/ 下找不到 {name}.toml); 请先完成生成预览与落盘部署"
        )));
    }
    if let Some(v) = crate::commands::instances::views(&ctx.root)
        .await
        .iter()
        .find(|v| v.name == name && v.running)
    {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 已在运行(模式={}, pid={:?}); 请先停机再启动, 不要重复拉起",
            v.mode.as_deref().unwrap_or("?"),
            v.pid
        )));
    }
    // 凭据前置: 缺 demo_key/demo_secret 时, 确认块都不应发出(避免用户白输短语)
    if let Err(e) = crate::commands::load_demo_credentials(&ctx.root) {
        return Err(ToolExecutionError::other(format!("测试网凭据未就绪, 无法启动 demo: {e}")));
    }
    let block = format!(
        "———— 确认块 ————\n\
         动作: 启动测试网 demo [start_demo]\n\
         目标: 策略 {name}\n\
         端点: 现货 {spot} / 合约 {fapi}(币安测试网, 与主网完全隔离)\n\
         后果: 会**真实向测试网下单/撤单**(用于验证下单链路), **不涉及真实资金**; \
         不需要 live_enabled, 不适用实盘三判据。停机仍须你本人执行 `ricow stop {name}`(涉及撤单清理)。",
        spot = crate::commands::DEMO_SPOT_URL,
        fapi = crate::commands::DEMO_FAPI_URL
    );
    Ok(PreparedAction { action: PendingAction::new_start_demo(name), block })
}

/// ack_risk 前提校验 (019 R4): 018 风险确认**一次长期有效**, 已确认过就明确拒绝重复登记。
///
/// 无参数, 也不读 ctx: 确认记录固定落在 [`crate::commands::risk_ack_path`](= project_root),
/// 那是子进程 `run` 唯一会读的位置。
fn prepare_ack_risk() -> Result<PreparedAction, ToolExecutionError> {
    if crate::commands::risk_acked() {
        return Err(ToolExecutionError::other(format!(
            "已经完成过风险确认({}), 无需重复; 直接登记 start_live 即可。",
            crate::commands::risk_ack_path().display()
        )));
    }
    let block = format!(
        "———— 确认块 ————\n\
         动作: 首次实盘风险确认 [ack_risk]\n\
         风险披露全文(请用户读完再确认):\n{disclosure}\n\
         后果: 确认后写入 {} —— 这是一次性长期记录, 之后启动实盘不再要求这一步。\n\
         提示: 建议先用 Dry Run(真实行情/虚拟成交)与测试网 demo 验证策略, 再动真实资金。",
        crate::commands::risk_ack_path().display(),
        disclosure = ricow_engine::RISK_DISCLOSURE
    );
    Ok(PreparedAction { action: PendingAction::new_ack_risk(), block })
}

/// start_live 前提校验 (019 R4): 已部署 / 未运行 / TOML `live_enabled` / 002 时长门禁 /
/// 018 风险确认 / 实盘凭据。
///
/// 这里**只做本地确定性检查**(不联网), 目的是"门禁不过就不发确认块", 避免用户白输一遍逐字短语。
/// 权威判定仍在宿主执行时经 [`crate::commands::ctrl::live_preflight`] 原样再跑一遍三判据
/// (含唯一需要联网的 FR-008 时钟预检)。
async fn prepare_start_live(
    ctx: &ToolCtx,
    args: &Value,
) -> Result<PreparedAction, ToolExecutionError> {
    let name = arg_str(args, "name")?;
    safe_strategy_name(&name)?;
    if !list_toml_stems(&ctx.root.join("strategies")).iter().any(|n| n == &name) {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 尚未部署(strategies/ 下找不到 {name}.toml); 实盘只能启动已部署策略"
        )));
    }
    if let Some(v) = crate::commands::instances::views(&ctx.root)
        .await
        .iter()
        .find(|v| v.name == name && v.running)
    {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 已在运行(模式={}, pid={:?}); 请先停机再启动, 不要重复拉起",
            v.mode.as_deref().unwrap_or("?"),
            v.pid
        )));
    }
    let Some(config) = crate::commands::read_strategy_config_in(&ctx.root, &name) else {
        return Err(ToolExecutionError::other(format!(
            "读取 strategies/{name}.toml 失败(文件缺失或损坏); 无法确认市场与实盘声明"
        )));
    };
    if !config.live_enabled {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 的配置里 live_enabled = false: 按设计不会进实盘(会降级为 Dry Run)。\n\
             实盘是**双条件**(live_enabled = true 且 显式要求实盘), 缺一不可。\n\
             请让用户在 strategies/{name}.toml 中把它改为 true(改后 `ricow restart {name}` 生效), \
             或先用测试网 demo 验证策略。"
        )));
    }
    // 002 时长门禁与 018 风险确认都走**共享内核**(019 R4): 与命令行/子进程同一份取参默认值
    // 与判定, 只是这里必须"不过就不发确认块", 所以把拒绝说明转成工具错误。
    let min_hours = match crate::commands::ctrl::dry_run_gate_shared(&config) {
        Ok(h) => h,
        Err(msg) => {
            return Err(ToolExecutionError::other(format!(
                "实盘三判据之「Dry Run 时长门禁」尚未通过, 不发确认块(免得你白输短语):\n{msg}"
            )))
        }
    };
    // 018: 本内核在 accept_risk=false 时等价于"是否已确认过"; 话术补上对话内该调哪个动作。
    if let Err(e) = crate::commands::ctrl::risk_gate_shared(false) {
        return Err(ToolExecutionError::other(format!(
            "实盘三判据之「首次风险确认」未完成: {e}\n\
             对话内做法: 先调 request_write_confirmation(action=\"ack_risk\") \
             让用户过一遍风险披露并逐字确认, 然后再来登记 start_live。"
        )));
    }
    if let Err(e) = crate::commands::load_live_credentials() {
        return Err(ToolExecutionError::other(format!("实盘凭据未就绪, 无法启动实盘: {e}")));
    }
    let block = format!(
        "———— 确认块 ————\n\
         动作: 启动实盘 [start_live]\n\
         目标: 策略 {name}(市场: {market})\n\
         后果(**真实资金**): 启动后按你账户里的真实资金下单/撤单; 停机只做撤单兜底, 持仓保留。\n\
         门禁: 018 风险确认 ✓ · 002 Dry Run 时长门禁 ✓({min} 小时)· 执行时再跑 FR-008 时钟预检(不过即拒绝)\n\
         注意: 平台不做风控 —— 仓位与止损由策略自己负责。",
        market = if config.market.eq_ignore_ascii_case("futures") { "合约 USDT-M" } else { "现货" },
        min = min_hours
    );
    Ok(PreparedAction { action: PendingAction::new_start_live(name), block })
}

/// 停机动作与实例当前模式是否匹配(纯函数, 便于单测): `None` = 匹配。
///
/// 误配的代价不对称: 拿 stop_demo 去停实盘会静默放过(反之亦然), 所以宁可直接拒绝并点名正确动作。
fn stop_mode_mismatch(kind: ActionKind, mode: &str) -> Option<String> {
    let want = match kind {
        ActionKind::StopDemo => "demo",
        ActionKind::StopLive | ActionKind::CloseLive => "live",
        _ => return None,
    };
    if mode == want {
        return None;
    }
    Some(format!(
        "该实例当前模式是「{}」, 与动作「{}」不匹配: 测试网实例用 stop_demo, 实盘实例用 stop_live(需一并平仓用 close_live)。",
        crate::commands::instances::mode_text(mode),
        kind.label()
    ))
}

/// stop_demo / stop_live / close_live 前提校验: 实例存在 / 正在运行 / 模式与动作匹配。
///
/// 停机走的是与终端 `ricow stop` 完全相同的内核([`crate::commands::ctrl::stop_daemon`]),
/// 区别只在"谁来敲这一下"。
async fn prepare_stop(
    ctx: &ToolCtx,
    kind: ActionKind,
    args: &Value,
) -> Result<PreparedAction, ToolExecutionError> {
    let name = arg_str(args, "name")?;
    safe_strategy_name(&name)?;
    let views = crate::commands::instances::views(&ctx.root).await;
    let Some(v) = views.iter().find(|v| v.name == name) else {
        return Err(ToolExecutionError::other(format!(
            "没有名为 {name} 的实例(既未在运行, 也没有退出台账); 可用 instance_status 确认现状"
        )));
    };
    if !v.running {
        return Err(ToolExecutionError::other(format!(
            "策略 {name} 当前未在运行(上次模式={}); 无需停机",
            v.mode.as_deref().unwrap_or("?")
        )));
    }
    if let Some(msg) = stop_mode_mismatch(kind, v.mode.as_deref().unwrap_or_default()) {
        return Err(ToolExecutionError::other(msg));
    }
    let pid = v.pid;
    let block = match kind {
        ActionKind::StopDemo => format!(
            "———— 确认块 ————\n\
             动作: 停止测试网 demo [stop_demo]\n\
             目标: 策略 {name}(pid={pid:?})\n\
             后果: 子进程优雅退出 + 交易所侧撤单兜底; **不平仓**(测试网持仓保留), 与真实资金无关。",
        ),
        ActionKind::StopLive => format!(
            "———— 确认块 ————\n\
             动作: 停止实盘 [stop_live]\n\
             目标: 策略 {name}(pid={pid:?})\n\
             后果(**真实资金**): 子进程优雅退出 + 交易所侧撤单兜底; **持仓保留不自动平仓** —— \
             如需一并市价平仓请改用 close_live。",
        ),
        ActionKind::CloseLive => format!(
            "———— 确认块 ————\n\
             动作: 平仓停止实盘 [close_live]\n\
             目标: 策略 {name}(pid={pid:?})\n\
             后果(**不可逆**): 先市价平掉该策略名下持仓, 再停机退出; 成交价由市场决定, 可能产生滑点与亏损。",
        ),
        _ => unreachable!("prepare_stop 只服务 stop_demo / stop_live / close_live"),
    };
    let action = match kind {
        ActionKind::StopDemo => PendingAction::new_stop_demo(name),
        ActionKind::StopLive => PendingAction::new_stop_live(name),
        _ => PendingAction::new_close_live(name),
    };
    Ok(PreparedAction { action, block })
}

/// 非交互环境(单次 `ricow ai "..."` / 管道)的回退指引: 不登记、不执行, 只给终端命令。
fn non_interactive_hint(kind: ActionKind, args: &Value) -> String {
    let head = "当前是非交互环境(单次提问或管道), 对话内确认不开放。请在你自己的终端执行:\n  ";
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("<策略名>");
    match kind {
        ActionKind::Deploy => {
            let id = args.get("preview_id").and_then(|v| v.as_str()).unwrap_or("<preview_id>");
            format!(
                "{head}ricow approve {id}\n  ricow deploy {id} --token <approve 返回的一次性 token>"
            )
        }
        ActionKind::StartDemo => format!("{head}ricow start {name} --demo"),
        ActionKind::AckRisk => format!(
            "{head}ricow start {name} --live --accept-risk\n  \
             (--accept-risk 只需一次: 读过风险披露即完成 018 确认, 之后记在数据目录)"
        ),
        ActionKind::StartLive => format!(
            "{head}ricow start {name} --live --accept-risk\n  \
             实盘是双条件: 还需 strategies/{name}.toml 里 live_enabled = true"
        ),
        ActionKind::StopDemo | ActionKind::StopLive => format!("{head}ricow stop {name}"),
        ActionKind::CloseLive => format!("{head}ricow stop {name} --close-all"),
    }
}

/// 代停实盘/demo 时的转介文案(纯函数, 便于单测): `stop_run` 只直停 Dry Run,
/// 其余两档涉及交易所侧撤单/平仓清理, 必须改走对话内确认(stop_demo / stop_live / close_live)。
fn stop_refusal(mode: &str, name: &str) -> Option<String> {
    match mode {
        "live" | "demo" => Some(format!(
            "该实例当前模式是「{}」: 本工具只能直接停 Dry Run。\n\
             这类停机涉及交易所侧的撤单/平仓清理, 必须由用户本人确认 —— 请改调 \
             request_write_confirmation(action=\"{}\", name=\"{name}\") 生成确认块, 等用户逐字输入短语后由宿主执行。",
            crate::commands::instances::mode_text(mode),
            if mode == "live" { "stop_live" } else { "stop_demo" }
        )),
        _ => None,
    }
}

/// L1 虚拟工具: 启动 Dry Run(本地虚拟撮合; 不碰资金、不需要凭据)。
///
/// 必须告知用户"首次启动会写 dry_run_started_at = 实盘时长门禁开始计时"(FR-024)。
fn tool_start_dry_run(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "start_dry_run",
        "把某个已部署策略以 **Dry Run**(本地虚拟撮合, 用真实行情但不动真钱)后台跑起来。需要该策略已在 strategies/ 下部署。返回 pid 与模式。",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "已部署策略名(不含 .toml)" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        move |_c, args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                let (pid, mode) = commands::ctrl::start_daemon(&ctx.root, &name, false, false, false)
                    .await
                    .map_err(|e| ToolExecutionError::other(format!("启动 Dry Run 失败: {e}")))?;
                let text = format!(
                    "已启动 {name}: pid={pid}, 模式={}\n注意(必须转告用户): Dry Run 首次启动会写入 dry_run_started_at —— 实盘「时长门禁」从这一刻开始计时。\n查看状态: ricow status {name} / 停机: ricow stop {name}",
                    crate::commands::instances::mode_text(&mode)
                );
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

/// L1 虚拟工具: 停止**Dry Run**实例(实盘/demo 一律拒绝, 让用户自己敲命令)。
fn tool_stop_run(ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "stop_run",
        "停止一个 **Dry Run** 实例(不下发 --close-all, 不做平仓)。实盘/测试网(demo)实例会被拒绝: 那类停机涉及交易所侧清理, 必须由用户自己执行。",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "运行中的策略名" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        move |_c, args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                let views = commands::instances::views(&ctx.root).await;
                let mode = views
                    .iter()
                    .find(|v| v.name == name)
                    .and_then(|v| v.mode.clone())
                    .unwrap_or_default();
                if let Some(msg) = stop_refusal(&mode, &name) {
                    return Ok(ToolOutput::text(msg));
                }
                let text = commands::ctrl::stop_daemon(&ctx.root, &name, false)
                    .await
                    .map_err(|e| ToolExecutionError::other(format!("停机失败: {e}")))?;
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

/// 构造全部工具(L0 只读 + L1 虚拟; 顺序即展示顺序)。
pub fn build(ctx: ToolCtx) -> Vec<DynamicTool> {
    vec![
        tool_list_strategies(ctx.clone()),
        tool_strategy_read(ctx.clone()),
        tool_read_doc(ctx.clone()),
        tool_run_backtest(ctx.clone()),
        tool_instance_status(ctx.clone()),
        tool_fills(ctx.clone()),
        tool_logs_tail(ctx.clone()),
        tool_market_ticker(ctx.clone()),
        tool_preview_strategy(ctx.clone()),
        tool_request_write_confirmation(ctx.clone()),
        tool_start_dry_run(ctx.clone()),
        tool_stop_run(ctx.clone()),
        tool_market_orderbook(ctx.clone()),
        tool_list_pairs(ctx.clone()),
        tool_list_templates(ctx.clone()),
        tool_read_template(ctx),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_is_exactly_the_registry() {
        for name in READ_ONLY_TOOLS {
            assert!(is_allowed(name), "{name} 应在白名单内");
        }
        // L1 虚拟工具可直调(自身不产生写实结果), 写实动作仍一律拒绝
        assert!(is_allowed("preview_strategy"));
        assert!(is_allowed("request_write_confirmation"));
        assert!(is_allowed("start_dry_run"));
        assert!(is_allowed("stop_run"));
        // 代停实盘/demo 必须被拒绝(给用户命令, 不代劳)
        assert!(stop_refusal("live", "g").is_some());
        assert!(stop_refusal("demo", "g").is_some());
        assert!(stop_refusal("dry_run", "g").is_none());
        assert!(!VIRTUAL_TOOLS.is_empty());
        for v in VIRTUAL_TOOLS {
            assert!(!READ_ONLY_TOOLS.contains(&v), "L1 不应混进 L0 列表: {v}");
        }
        assert!(!is_allowed("deploy_strategy"));
        assert!(!is_allowed(""));
        assert!(!is_allowed("list_strategies "));
    }

    #[test]
    fn test_no_write_tool_names_are_allowed() {
        // 结构性安全断言: 任何写实动作的名字都不得出现在白名单里(FR-009)
        let forbidden = [
            "deploy_strategy",
            "approve_strategy",
            "start_strategy",
            "stop_strategy",
            "start_live",
            "start_demo",
            "stop_demo",
            "execute_deploy",
            "close_all",
            "set_params",
            "set_live_enabled",
            "run_shell",
            "read_file",
            "write_file",
        ];
        for name in forbidden {
            assert!(!is_allowed(name), "{name} 是写实/越权动作, 绝不能被放行");
        }
    }

    #[test]
    fn test_clamp_output_keeps_short_and_marks_truncation() {
        let short = "短文本".to_string();
        assert_eq!(clamp_output(short.clone()), short);

        // 超限: 必须显式标记, 且不切坏多字节字符
        let long: String = "汉".repeat(MAX_OUTPUT_CHARS + 500);
        let out = clamp_output(long);
        assert!(out.contains("输出已截断"));
        assert!(out.contains(&format!("共 {}", MAX_OUTPUT_CHARS + 500)));
        // 首个片段字符数恰为上限(中文字符按字符计, 不按字节)
        let first = out.split("\n\n[").next().unwrap();
        assert_eq!(first.chars().count(), MAX_OUTPUT_CHARS);
    }

    #[test]
    fn test_doc_limit_covers_full_embedded_docs() {
        // G1 回归: read_doc 专用上限必须容得下整篇 lua-api / backtest, 否则后半(exec 组件/示例)丢失
        let lua_chars = crate::ai::prompt::STRATEGY_API_DOC.chars().count();
        let backtest_chars = include_str!("../../../../specs/backtest.md").chars().count();
        assert!(
            lua_chars < DOC_MAX_OUTPUT_CHARS,
            "lua-api {lua_chars} 字符超出文档上限 {DOC_MAX_OUTPUT_CHARS} —— 模型读不到全文"
        );
        assert!(
            backtest_chars < DOC_MAX_OUTPUT_CHARS,
            "backtest {backtest_chars} 字符超出文档上限 {DOC_MAX_OUTPUT_CHARS}"
        );
        // 通用上限仍会截断 14k 文档(证明两个上限确实不同, 而非误调)
        assert!(lua_chars > MAX_OUTPUT_CHARS);
        // 自定义上限截断行为正确
        let out = clamp_output_limited("x".repeat(100), 10);
        assert_eq!(out.split("\n\n[").next().unwrap().chars().count(), 10);
        assert!(out.contains("共 100 字符"));
    }

    #[test]
    fn test_redact_masks_secrets_in_free_text() {
        let raw = "GET /api key=abc\nAuthorization: Bearer sk-abcdef123456\napi_key=deadbeef\n正常的一行行情: ETHUSDT 3500.5";
        let out = redact(raw);
        assert!(!out.contains("sk-abcdef123456"), "sk- 形态必须打码: {out}");
        assert!(!out.contains("deadbeef"), "api_key 值必须打码: {out}");
        assert!(out.contains("api_key=***"), "{out}");
        assert!(out.contains("正常的一行行情"), "正常内容不应被破坏: {out}");
    }

    #[test]
    fn test_redact_preserves_whitespace_layout() {
        // 日志缩进与多空格是结构信息, 打码不该把它压平成单空格。
        let raw = "  2026-01-01  ERROR   api_key=deadbeef\n\ttab\t分隔";
        let out = redact(raw);
        assert!(out.contains("  2026-01-01  ERROR   api_key=***"), "{out}");
        assert!(out.contains("\ttab\t分隔"), "{out}");
    }

    #[test]
    fn test_safe_strategy_name_blocks_traversal() {
        assert!(safe_strategy_name("grid_v1").is_ok());
        assert!(safe_strategy_name("grid-v1").is_ok());
        for bad in ["../secret", "a/b", "a\\b", ".hidden"] {
            assert!(safe_strategy_name(bad).is_err(), "{bad} 应被拒绝");
        }
    }

    #[test]
    fn test_arg_str_requires_non_empty_string() {
        let args = json!({ "name": "  " });
        assert!(arg_str(&args, "name").is_err());
        let args = json!({ "name": "grid_v1" });
        assert_eq!(arg_str(&args, "name").unwrap(), "grid_v1");
        assert!(arg_str(&json!({}), "name").is_err());
    }

    #[test]
    fn test_read_doc_embedded_docs_are_present() {
        // 内嵌文档非空且长度合理(编译期嵌入, 不依赖运行时文件)
        let lua = crate::ai::prompt::STRATEGY_API_DOC;
        assert!(lua.contains("on_tick"));
        assert!(lua.len() > 10_000);
    }

    #[test]
    fn test_non_interactive_hint_gives_terminal_commands_only() {
        // D5: 非交互环境只能拿到终端命令, 文案里不得出现"已登记/请输入短语后我会执行"等暗示
        let deploy = non_interactive_hint(
            ActionKind::Deploy,
            &json!({ "action": "deploy", "preview_id": "pv-9" }),
        );
        assert!(deploy.contains("ricow approve pv-9"), "{deploy}");
        assert!(deploy.contains("ricow deploy pv-9"), "{deploy}");
        assert!(deploy.contains("非交互环境"), "{deploy}");

        let demo = non_interactive_hint(
            ActionKind::StartDemo,
            &json!({ "action": "start_demo", "name": "g1" }),
        );
        assert!(demo.contains("ricow start g1 --demo"), "{demo}");

        // 缺参数时给占位符而非 panic
        let deploy_blank = non_interactive_hint(ActionKind::Deploy, &json!({ "action": "deploy" }));
        assert!(deploy_blank.contains("<preview_id>"), "{deploy_blank}");
    }

    #[test]
    fn test_registry_count_and_write_tool_boundary() {
        // 注册总数 = 12 只读 + 4 虚拟; 对话内确认工具登记的是"请求确认"而非写实本身
        assert_eq!(READ_ONLY_TOOLS.len(), 12);
        assert!(is_allowed("list_pairs"), "交易对视野是 L0 只读工具");
        assert!(is_allowed("list_templates") && is_allowed("read_template"), "模板面是 L0 只读");
        assert_eq!(VIRTUAL_TOOLS.len(), 4);
        assert!(is_allowed("request_write_confirmation"));
        // 真正的写实名仍然一个都不在
        for forbidden in
            ["deploy", "execute_strategy", "start_demo", "stop_live", "stop_demo", "approve"]
        {
            assert!(!is_allowed(forbidden), "{forbidden} 不得是可调用工具");
        }
    }

    #[test]
    fn test_registry_matches_whitelist_exactly() {
        // 漂移闸: build() 注册的工具名必须与白名单逐字一致(增删工具必须同时改两处)
        let ctx = ToolCtx::new(std::env::temp_dir(), true, crate::ai::confirm::new_slot());
        let mut registered: Vec<String> = build(ctx).iter().map(|t| t.name().to_string()).collect();
        registered.sort();
        let mut expected: Vec<String> =
            READ_ONLY_TOOLS.iter().chain(VIRTUAL_TOOLS.iter()).map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(registered, expected, "注册表与白名单漂移");
    }

    // ── R3 对话内确认全链路(019 S5/S6 的确定性部分, 不依赖 LLM/网络)─────────────
    //
    // 端到端真机(tty REPL)无法在自动化里驱动: 管道喂入的 stdin 不是终端, 会被交互门禁拒(D5)。
    // 这里用进程内直调覆盖"请求确认 → pending 状态机 → 宿主执行"的全部逻辑, 真机部分见
    // tests/ai_live_smoke.rs(需 env key, #[ignore])与人工手测清单。

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static TMP_SEQ: AtomicU32 = AtomicU32::new(0);

    fn r3_temp_root(tag: &str) -> PathBuf {
        let seq = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ricow-r3-{tag}-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| panic!("建临时目录 {}: {e}", dir.display()));
        dir
    }

    const LUA_STUB: &str = "function on_tick(ctx) return {} end";

    /// 在临时 root 里直接造一条 pending 的 strategy preview(不经网络/回测; 与正式载荷同构)。
    async fn seed_pending_preview(root: &Path, name: &str) -> String {
        seed_pending_preview_with_code(root, name, LUA_STUB).await
    }

    /// 同上, 但可指定脚本内容(FR-044 覆盖测试要能看出新旧脚本的区别)。
    async fn seed_pending_preview_with_code(root: &Path, name: &str, code: &str) -> String {
        let db =
            ricow_strategy::Database::open(&root.join("ricow.db")).await.expect("open temp db");
        let config = ricow_engine::create_strategy(name, code, "BTCUSDT", HashMap::new())
            .expect("造 StrategyConfig(与 create 同校验)");
        let payload = config.to_toml().expect("config → TOML");
        ricow_engine::create_preview(&db, "strategy", &payload).await.expect("seed preview")
    }

    /// 模拟 REPL 确认后宿主执行 deploy, 返回执行回执。
    async fn deploy_via_confirmation(root: &Path, name: &str, preview_id: &str) -> String {
        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.to_path_buf(), true, slot.clone());
        let prepared = prepare_deploy(&ctx, &json!({ "preview_id": preview_id }))
            .await
            .expect("prepare_deploy 应通过");
        *slot.lock().await = Some(prepared.action);
        let disposition =
            crate::ai::confirm::consume_line(&slot, &format!("确认部署 {name}")).await;
        let action = match disposition {
            crate::ai::confirm::LineDisposition::Confirm(a) => a,
            other => panic!("逐字短语应判 Confirm, 实际 {other:?}"),
        };
        crate::ai::session::execute_confirmed(&action, root).await.expect("宿主执行落盘")
    }

    #[tokio::test]
    async fn r3s5_confirm_deploy_writes_files_and_consumes_preview() {
        let root = r3_temp_root("deploy");
        let name = "aidep01";
        let preview_id = seed_pending_preview(&root, name).await;
        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.clone(), true, slot.clone());

        let prepared = prepare_deploy(&ctx, &json!({ "preview_id": preview_id })).await.unwrap();
        assert!(prepared.block.contains("落盘部署"), "{block}", block = prepared.block);
        assert_eq!(prepared.action.kind, ActionKind::Deploy);
        assert_eq!(prepared.action.expected_phrase(), format!("确认部署 {name}"));

        // 登记 pending
        *slot.lock().await = Some(prepared.action);

        // 错误短语/普通提问: pending 原样保留, 零副作用
        for line in ["好的部署吧", "确认部署", "y", "yes", "再解释一下风险?"] {
            let d = crate::ai::confirm::consume_line(&slot, line).await;
            assert!(
                matches!(d, crate::ai::confirm::LineDisposition::Other),
                "{line} 不应触发 Confirm: {d:?}"
            );
            assert!(slot.lock().await.is_some(), "错短语后 pending 必须保留");
        }
        // 落盘前零文件
        assert!(!root.join("strategies").exists());

        // 用外层同一个 slot 走完确认 → 宿主执行
        let disposition =
            crate::ai::confirm::consume_line(&slot, &format!("确认部署 {name}")).await;
        let action = match disposition {
            crate::ai::confirm::LineDisposition::Confirm(a) => a,
            other => panic!("逐字短语应判 Confirm, 实际 {other:?}"),
        };
        let msg =
            crate::ai::session::execute_confirmed(&action, &root).await.expect("宿主执行落盘");
        assert!(msg.contains("已确认并完成落盘"), "{msg}");

        // 双证据①: 真实落盘 toml + lua
        assert!(root.join("strategies").join(format!("{name}.toml")).is_file());
        assert!(root.join("strategies").join(format!("{name}.lua")).is_file());
        // 双证据②: preview 已 consumed(一次性 token, 不可重放)
        let db = ricow_strategy::Database::open(&root.join("ricow.db")).await.unwrap();
        let rec = ricow_engine::get_preview(&db, &preview_id).await.unwrap();
        assert_eq!(rec.status, "consumed");

        // pending 已被消费清空
        assert!(slot.lock().await.is_none());
    }

    #[tokio::test]
    async fn r3s5_same_name_deploy_is_refused_after_files_exist() {
        let root = r3_temp_root("dup");
        let name = "aidep02";
        let id1 = seed_pending_preview(&root, name).await;
        deploy_via_confirmation(&root, name, &id1).await;

        // 同名再来一条 preview: prepare 必须拦下(不覆盖、不静默改名)
        let id2 = seed_pending_preview(&root, name).await;
        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.clone(), true, slot);
        let err = prepare_deploy(&ctx, &json!({ "preview_id": id2 })).await.unwrap_err();
        assert!(err.to_string().contains("已存在"), "{err}");
        // 第二条 preview 仍 pending, 未被触碰
        let db = ricow_strategy::Database::open(&root.join("ricow.db")).await.unwrap();
        assert_eq!(ricow_engine::get_preview(&db, &id2).await.unwrap().status, "pending");
    }

    /// FR-044: 撞名默认拒绝, 且拒绝文案必须同时指出"换个新名"(默认路径)与"受控覆盖"(replace=true)。
    #[tokio::test]
    async fn fr044_same_name_refusal_points_to_both_paths() {
        let root = r3_temp_root("fr044-deny");
        let name = "aidp044a";
        let id1 = seed_pending_preview(&root, name).await;
        deploy_via_confirmation(&root, name, &id1).await;

        let id2 = seed_pending_preview(&root, name).await;
        let ctx = ToolCtx::new(root.clone(), true, crate::ai::confirm::new_slot());
        let msg =
            prepare_deploy(&ctx, &json!({ "preview_id": id2 })).await.unwrap_err().to_string();
        assert!(msg.contains("已存在"), "{msg}");
        assert!(msg.contains("replace=true"), "必须告诉模型受控覆盖这条路: {msg}");
        assert!(msg.contains("确认覆盖"), "必须给出覆盖短语: {msg}");
        assert!(msg.contains("新名"), "默认路径(换新名)也要讲明: {msg}");
    }

    /// FR-044 受控覆盖全链路: replace=true → 短语换成「确认覆盖」→ 宿主先备份旧脚本再覆盖。
    #[tokio::test]
    async fn fr044_replace_flow_backs_up_then_overwrites() {
        let root = r3_temp_root("fr044-over");
        let name = "aidp044b";
        let lua_path = root.join("strategies").join(format!("{name}.lua"));

        // 先正常部署一版, 记下原文
        let id1 = seed_pending_preview(&root, name).await;
        deploy_via_confirmation(&root, name, &id1).await;
        let old_lua = std::fs::read_to_string(&lua_path).unwrap();

        // 再生成一版不同脚本的预览, 走受控覆盖
        let new_code = "function on_tick(ctx)\n    return { v = 2 }\nend\n";
        let id2 = seed_pending_preview_with_code(&root, name, new_code).await;
        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.clone(), true, slot.clone());
        let prepared =
            prepare_deploy(&ctx, &json!({ "preview_id": id2, "replace": true })).await.unwrap();
        assert!(prepared.action.replace, "受控覆盖必须带 replace 标记");
        assert_eq!(prepared.action.expected_phrase(), format!("确认覆盖 {name}"));
        assert!(prepared.block.contains("受控覆盖"), "{}", prepared.block);
        assert!(prepared.block.contains(".bak"), "确认块必须写明备份: {}", prepared.block);

        // 普通部署短语不得放行覆盖(破坏性动作另用一句)
        *slot.lock().await = Some(prepared.action);
        let d = crate::ai::confirm::consume_line(&slot, &format!("确认部署 {name}")).await;
        assert!(
            matches!(d, crate::ai::confirm::LineDisposition::Other),
            "旧短语不得放行覆盖: {d:?}"
        );
        assert!(slot.lock().await.is_some(), "错短语后 pending 必须保留");

        // 覆盖短语 → 宿主执行
        let d = crate::ai::confirm::consume_line(&slot, &format!("确认覆盖 {name}")).await;
        let action = match d {
            crate::ai::confirm::LineDisposition::Confirm(a) => a,
            other => panic!("应判 Confirm: {other:?}"),
        };
        let msg =
            crate::ai::session::execute_confirmed(&action, &root).await.expect("宿主执行覆盖");
        assert!(msg.contains("备份"), "回执必须如实给出备份路径: {msg}");

        // 新脚本已生效
        let now = std::fs::read_to_string(&lua_path).unwrap();
        assert!(now.contains("v = 2"), "新脚本应已落盘: {now}");
        assert_ne!(now, old_lua);
        // 旧脚本仍在备份里(且逐字一致)
        let backups: Vec<String> = std::fs::read_dir(root.join("strategies"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(&format!("{name}.lua.")) && n.ends_with(".bak"))
            .collect();
        assert_eq!(backups.len(), 1, "应恰好一个脚本备份: {backups:?}");
        let saved = std::fs::read_to_string(root.join("strategies").join(&backups[0])).unwrap();
        assert_eq!(saved, old_lua, "备份必须是旧脚本原文");
    }

    #[tokio::test]
    async fn r3s5_reject_sets_preview_terminal_and_writes_nothing() {
        let root = r3_temp_root("reject");
        let name = "aidep03";
        let id = seed_pending_preview(&root, name).await;
        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.clone(), true, slot.clone());
        let prepared = prepare_deploy(&ctx, &json!({ "preview_id": id })).await.unwrap();
        *slot.lock().await = Some(prepared.action);

        let disposition = crate::ai::confirm::consume_line(&slot, "拒绝").await;
        let action = match disposition {
            crate::ai::confirm::LineDisposition::Reject(a) => a,
            other => panic!("应判 Reject: {other:?}"),
        };
        assert_eq!(action.kind, ActionKind::Deploy);
        // REPL 在 Reject 分支对 deploy 做的终态化(与 commands/ai.rs 同口径)
        let db = ricow_strategy::Database::open(&root.join("ricow.db")).await.unwrap();
        ricow_engine::reject(&db, &id).await.unwrap();
        assert_eq!(ricow_engine::get_preview(&db, &id).await.unwrap().status, "rejected");
        assert!(!root.join("strategies").exists(), "拒绝后零落盘");
        assert!(slot.lock().await.is_none());
    }

    #[tokio::test]
    async fn r3s6_start_demo_gates_in_order() {
        let root = r3_temp_root("demo");
        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.clone(), true, slot);

        // ① 未部署: 先于凭据检查报错
        let err = prepare_start_demo(&ctx, &json!({ "name": "ghost99" })).await.unwrap_err();
        assert!(err.to_string().contains("尚未部署"), "{err}");

        // 部署一个策略到该 root
        let name = "aidemo01";
        let id = seed_pending_preview(&root, name).await;
        deploy_via_confirmation(&root, name, &id).await;

        // ② 已部署但缺 demo 凭据: 不发确认块(避免用户白输短语)
        let err = prepare_start_demo(&ctx, &json!({ "name": name })).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("demo_key") && msg.contains("demo_secret"), "{msg}");

        // ③ 在该 root 配置占位测试网凭据(仅校验存在性, 不发任何网络请求)
        std::fs::write(
            crate::commands::config_file::path(&root),
            "[exchange]\ndemo_key = \"placeholder-key\"\ndemo_secret = \"placeholder-secret\"\n",
        )
        .unwrap();
        let prepared = prepare_start_demo(&ctx, &json!({ "name": name })).await.unwrap();
        assert_eq!(prepared.action.kind, ActionKind::StartDemo);
        assert_eq!(prepared.action.expected_phrase(), format!("确认启动测试网 {name}"));
        // 确认块必须点明 demo 端点与"真实下单"事实
        assert!(prepared.block.contains("demo-api.binance.com"), "{b}", b = prepared.block);
        assert!(prepared.block.contains("真实"), "{}", prepared.block);
    }

    #[tokio::test]
    async fn r3s5_consumed_preview_cannot_be_prepared_again() {
        let root = r3_temp_root("twice");
        let name = "aidep04";
        let id = seed_pending_preview(&root, name).await;
        deploy_via_confirmation(&root, name, &id).await;

        let slot = crate::ai::confirm::new_slot();
        let ctx = ToolCtx::new(root.clone(), true, slot);
        // 同一 preview 二次请求确认: 状态不是 pending, 必须失败(防 token 重放路径)
        let err = prepare_deploy(&ctx, &json!({ "preview_id": id })).await.unwrap_err();
        assert!(err.to_string().contains("pending"), "{err}");
    }
}
