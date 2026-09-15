//! 工具注册表 (019): L0 只读 + L1 虚拟。
//!
//! **结构边界**(spec FR-008 / FR-009): 本模块只注册 **L0 只读**与 **L1 虚拟**工具 ——
//! L1 虚拟可直调但**不落盘、不碰资金**(当前仅 `preview_strategy`: 生成预览, 落盘仍需用户本人 approve/deploy)。写实动作
//! (落盘部署 / Dry Run 或实盘启停 / 平仓 / 改参数 / 改 `live_enabled`) **不作为工具注册给模型**
//! —— 模型连调用面都没有, 只能由用户在聊天里敲确认、由 CLI 自己执行。
//! 审批门 `ToolGuard` 再按本表白名单 fail-closed 放行一次, 是第二道同向保证。
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

use crate::commands;
use crate::supervisor::ledger;

/// 工具运行上下文(只读): 数据目录与 DB 路径。不含任何凭据。
#[derive(Clone)]
pub struct ToolCtx {
    pub root: PathBuf,
}

impl ToolCtx {
    pub fn from_cli() -> Self {
        Self { root: commands::project_root() }
    }
}

/// L0 只读工具白名单 —— 无副作用。
pub const READ_ONLY_TOOLS: [&str; 9] = [
    "list_strategies",
    "strategy_read",
    "read_doc",
    "run_backtest",
    "instance_status",
    "fills",
    "logs_tail",
    "market_ticker",
    "market_orderbook",
];

/// L1 虚拟工具白名单 —— 可直调, 但**不产生真实资金动作、不落盘**(只写预览记录)。
pub const VIRTUAL_TOOLS: [&str; 3] = ["preview_strategy", "start_dry_run", "stop_run"];

/// 工具是否被允许执行(L0 ∪ L1)。未列入一律拒绝(fail-closed)。
pub fn is_allowed(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name) || VIRTUAL_TOOLS.contains(&name)
}

/// 单个工具输出的字符上限(超出即截断并标记)。
pub const MAX_OUTPUT_CHARS: usize = 8_000;

/// 截断输出: 字符级(不切坏多字节), 且必须显式标记被截掉的部分。
pub fn clamp_output(text: impl Into<String>) -> String {
    let text = text.into();
    let total = text.chars().count();
    if total <= MAX_OUTPUT_CHARS {
        return text;
    }
    let kept: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    format!("{kept}\n\n[输出已截断: 共 {total} 字符, 只显示前 {MAX_OUTPUT_CHARS} 字符]")
}

/// 凭据打码: 工具输出(尤其日志原文)可能夹带密钥, 出模型上下文前再过一遍。
pub fn redact(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut masked_prev = false;
        let mut tokens: Vec<String> = Vec::new();
        for token in line.split_whitespace() {
            let masked = if masked_prev {
                "[已打码]".to_string()
            } else if is_secret_like(token) {
                mask_secret(token)
            } else {
                token.to_string()
            };
            masked_prev = token.eq_ignore_ascii_case("bearer") || token.ends_with("Authorization:");
            tokens.push(masked);
        }
        out.push(tokens.join(" "));
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
                READ_ONLY_TOOLS.iter().chain(VIRTUAL_TOOLS.iter()).copied().collect::<Vec<_>>().join(", ")
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
                let names = commands::deployed_strategy_names();
                if names.is_empty() {
                    return Ok(ToolOutput::text(
                        "当前没有任何已部署策略(strategies/ 目录为空)。新建需走三步: \
                         `ricow create` → `ricow approve` → `ricow deploy`。",
                    ));
                }
                let mut out =
                    format!("已部署策略 {} 个(数据目录 {}):\n", names.len(), ctx.root.display());
                for name in &names {
                    match commands::read_strategy_config(name) {
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

fn tool_strategy_read(_ctx: ToolCtx) -> DynamicTool {
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
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                safe_strategy_name(&name)?;
                let dir = commands::strategies_dir();
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
        "读取本产品的权威说明(策略 Lua API / 回测口径 / 风险披露)。写策略或解释指标语义前先读。只读。",
        json!({
            "type": "object",
            "properties": {
                "topic": {
                    "type": "string",
                    "enum": ["lua-api", "backtest", "risk"],
                    "description": "lua-api=策略可用的回调/指标/exec 组件; backtest=回测撮合与口径; risk=风险披露与限额"
                }
            },
            "required": ["topic"],
            "additionalProperties": false
        }),
        move |_c, args| {
            Box::pin(async move {
                let topic = arg_str(&args, "topic")?;
                let text = match topic.as_str() {
                    "lua-api" => crate::ai::prompt::STRATEGY_API_DOC,
                    "backtest" => include_str!("../../../../specs/backtest.md"),
                    "risk" => ricow_engine::RISK_DISCLOSURE,
                    other => {
                        return Err(ToolExecutionError::invalid_args(format!(
                            "未知 topic '{other}'; 可用: lua-api, backtest, risk"
                        )))
                    }
                };
                Ok(ToolOutput::text(clamp_output(text)))
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

/// L1 虚拟工具: 生成策略预览(编译门禁 → 真实 K 线沙箱回测 → preview_id)。
///
/// **不落盘、不碰资金**(只写一条 previews 记录); 落盘必须由用户本人 `approve` + `deploy`。
/// 与 CLI `ricow create` 共用 `commands::create::create_preview`(FR-016 同口径)。
fn tool_preview_strategy(_ctx: ToolCtx) -> DynamicTool {
    DynamicTool::new(
        "preview_strategy",
        "把一段 Lua 策略代码过一遍门禁并生成**预览**: 编译门禁 → 真实历史 K 线沙箱回测 → preview_id(15 分钟一次性)。虚拟操作: 不写任何策略文件、不碰资金(会占用一条预览记录)。落盘**必须由用户本人**执行 `ricow approve <preview_id>` 与 `ricow deploy <preview_id> --token <token>`, 你只能把这两条命令告诉用户, 不得声称已部署。",
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
                    "{}\n编译门禁 ✓  沙箱回测 ✓  —— **尚未部署**(未写入任何策略文件)\npreview_id: {}\n下一步必须由用户本人执行(你没有落盘权限):\n  ricow approve {}\n  ricow deploy {} --token <token>",
                    out.report_text, out.preview_id, out.preview_id, out.preview_id
                );
                Ok(ToolOutput::text(clamp_output(redact(&text))))
            })
        },
    )
}

/// 代停实盘/demo 时给用户的拒绝文案(纯函数, 便于单测): 这两类停机涉及交易所侧清理, 不在 AI 权限内。
fn stop_refusal(mode: &str, name: &str) -> Option<String> {
    match mode {
        "live" | "demo" => Some(format!(
            "该实例当前模式是「{}」: 停机涉及交易所侧的撤单/平仓清理, 不在我的权限内(我只被允许停 Dry Run)。\n请你本人执行: ricow stop {name}{}",
            crate::commands::instances::mode_text(mode),
            if mode == "live" { " (如需平仓加 --close-all)" } else { "" }
        )),
        _ => None,
    }
}

/// L1 虚拟工具: 启动 Dry Run(本地虚拟撮合; 不碰资金、不需要凭据)。
///
/// 必须告知用户"首次启动会写 dry_run_started_at = 实盘时长门禁开始计时"(FR-024)。
fn tool_start_dry_run(_ctx: ToolCtx) -> DynamicTool {
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
            Box::pin(async move {
                let name = arg_str(&args, "name")?;
                let (pid, mode) = commands::ctrl::start_daemon(&name, false, false, false)
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
                let text = commands::ctrl::stop_daemon(&name, false)
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
        tool_start_dry_run(ctx.clone()),
        tool_stop_run(ctx.clone()),
        tool_market_orderbook(ctx),
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
        // L1 虚拟工具可直调(不落盘/不碰资金), 写实动作仍一律拒绝
        assert!(is_allowed("preview_strategy"));
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
    fn test_redact_masks_secrets_in_free_text() {
        let raw = "GET /api key=abc\nAuthorization: Bearer sk-abcdef123456\napi_key=deadbeef\n正常的一行行情: ETHUSDT 3500.5";
        let out = redact(raw);
        assert!(!out.contains("sk-abcdef123456"), "sk- 形态必须打码: {out}");
        assert!(!out.contains("deadbeef"), "api_key 值必须打码: {out}");
        assert!(out.contains("api_key=***"), "{out}");
        assert!(out.contains("正常的一行行情"), "正常内容不应被破坏: {out}");
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
}
