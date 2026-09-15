//! `ricow approve <preview-id>` — 批准待确认操作 (两步确认的用户侧入口)。
//!
//! 019 T026 / FR-020 / FR-021: 确认由**确认块**(动作 / 目标 / 关键参数 / 后果)+ **明确短语**组成,
//! 裸 `y` / `yes` / 回车一律**不接受**(避免肌肉记忆式确认)。
//! 输入错/空/中断 → **零副作用**: 不改预览状态(仍可在 15 分钟内重试), 也不产生 token。
//! 想放弃时显式输入 `拒绝` / `reject`。

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_engine::{approve as confirm_approve, get_preview, reject};
use ricow_strategy::{Database, StrategyConfig};

use std::io::IsTerminal;

#[derive(Args)]
pub struct ApproveArgs {
    /// preview id (写操作首次调用返回)
    pub preview_id: String,
}

/// 期望的确认短语: `确认部署 <策略名>`(名字从预览载荷解析; 解析不出时退化为 `确认部署`)。
pub fn expected_phrase(payload: &str) -> String {
    match StrategyConfig::from_toml(payload) {
        Ok(c) if !c.name.trim().is_empty() => format!("确认部署 {}", c.name.trim()),
        _ => "确认部署".to_string(),
    }
}

/// 确认块渲染(动作 / 目标 / 关键参数 / 后果)。与 AI 侧给用户的说明同源。
pub fn confirmation_block(preview_kind: &str, payload: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "———— 确认块 ————");
    let cfg = StrategyConfig::from_toml(payload);
    match cfg {
        Ok(c) => {
            let name = c.name.trim();
            let market = if c.market.eq_ignore_ascii_case("futures") { "合约 USDT-M" } else { "现货" };
            let pair = c.get_str("pair").unwrap_or("<未指定>");
            let script_len = c.get_str("script").map(|s| s.len()).unwrap_or(0);
            let _ = writeln!(out, "动作: 落盘部署 [{preview_kind}] —— 写 strategies/{name}.toml + strategies/{name}.lua");
            let _ = writeln!(out, "目标: 策略名 {name} · 交易对 {pair} · 市场 {market} · 脚本 {script_len} 字节");
            let _ = writeln!(out, "参数: {}", summarize_params(&c));
            let _ = writeln!(
                out,
                "后果: 落盘后可用 `ricow run {name}` 起 Dry Run(虚拟撮合); \
                 实盘还需 TOML 声明 live_enabled=true 且命令行 --live --accept-risk; 本步不会自动交易、不会下单"
            );
        }
        Err(e) => {
            let _ = writeln!(out, "动作: 执行 [{preview_kind}]");
            let _ = writeln!(out, "载荷解析失败({e}) —— 请人工核对以下原文后再决定");
        }
    }
    let _ = writeln!(out, "载荷原文:");
    let _ = writeln!(out, "{payload}");
    out
}

/// 参数摘要(排除 script 正文; 最多 6 项, 超出以 `…` 标记)。
fn summarize_params(cfg: &StrategyConfig) -> String {
    let mut items: Vec<String> = cfg
        .params
        .iter()
        .filter(|(k, _)| k.as_str() != "script")
        .map(|(k, v)| {
            let shown = match v {
                ricow_strategy::ConfigValue::String(s) => s.clone(),
                ricow_strategy::ConfigValue::Float(f) => f.to_string(),
                ricow_strategy::ConfigValue::Integer(i) => i.to_string(),
                ricow_strategy::ConfigValue::Boolean(b) => b.to_string(),
            };
            format!("{k}={shown}")
        })
        .collect();
    items.sort();
    let total = items.len();
    items.truncate(6);
    if total > items.len() {
        items.push(format!("… 共 {total} 项"));
    }
    if items.is_empty() {
        "无".to_string()
    } else {
        items.join(" ")
    }
}

pub async fn run(args: ApproveArgs) -> CoreResult<()> {
    // 019 spec §七 R2: 人工批准必须发生在**交互终端** —— 管道/脚本/agent 工具调用喂入短语一律拒绝
    // (否则"逐字短语"只约束格式, 不约束"是否真人当场确认")。
    crate::commands::require_interactive_terminal(std::io::stdin().is_terminal())?;
    let db = Database::open(&crate::commands::default_db_path())
        .await
        .map_err(|e| CoreError::Exchange(e.to_string()))?;

    let preview = get_preview(&db, &args.preview_id).await?;
    print!("{}", confirmation_block(&preview.kind, &preview.payload_json));

    let expected = expected_phrase(&preview.payload_json);
    println!("请输入确认短语(逐字): {expected}");
    println!("(放弃请输入: 拒绝)");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).map_err(|e| CoreError::Parse(e.to_string()))?;

    if crate::commands::is_explicit_confirmation(&input, &expected) {
        let token = confirm_approve(&db, &args.preview_id).await?;
        println!("已批准。一次性 token: {token}, 携 (preview_id, token) 重放写操作即可执行。");
        return Ok(());
    }
    if crate::commands::is_explicit_rejection(&input) {
        reject(&db, &args.preview_id).await?;
        println!("已拒绝(终态)。");
        return Ok(());
    }
    // 输入错/空/中断 → **零副作用**: 不改状态, 预览仍在有效期内可重试 (FR-021)
    Err(CoreError::InvalidArgument(format!(
        "未确认: 输入与确认短语不一致(期望逐字: {expected})。预览保持有效(15 分钟内可重试), 未产生任何副作用。"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{is_explicit_confirmation, is_explicit_rejection};

    const PAYLOAD: &str = r#"
[strategy]
name = "eth-simple-1"
type = "lua"
enabled = true
exchange = "binance"
live_enabled = false
market = "spot"
position_mode = "one-way"

[strategy.params]
pair = "ETHUSDT"
order_size = 0.01
max_base = 0.2
script = "function on_tick(ctx) return {} end"
"#;

    #[test]
    fn test_phrase_derived_from_payload() {
        assert_eq!(expected_phrase(PAYLOAD), "确认部署 eth-simple-1");
        assert_eq!(expected_phrase("不是 TOML"), "确认部署");
    }

    #[test]
    fn test_bare_y_rejected_explicit_phrase_accepted() {
        let exp = "确认部署 eth-simple-1";
        for bad in ["y", "Y", "yes", "OK", "", "   ", "确认部署", "确认部署 eth-simple", "确认部署eth-simple-1"] {
            assert!(!is_explicit_confirmation(bad, exp), "'{bad}' 不应被当作明确确认");
        }
        assert!(is_explicit_confirmation("确认部署 eth-simple-1", exp));
        assert!(is_explicit_confirmation("  确认部署 eth-simple-1  ", exp));
    }

    #[test]
    fn test_explicit_rejection_recognized() {
        assert!(is_explicit_rejection("拒绝"));
        assert!(is_explicit_rejection("reject"));
        assert!(!is_explicit_rejection("n"));
    }

    #[test]
    fn test_confirmation_block_covers_action_target_params_consequence() {
        let b = confirmation_block("strategy", PAYLOAD);
        for must in ["动作:", "目标:", "参数:", "后果:", "确认块", "eth-simple-1", "ETH", "order_size=0.01"] {
            assert!(b.contains(must), "确认块缺少 {must}:\n{b}");
        }
        // 脚本正文不应混进参数摘要(只报字节数)
        assert!(!b.contains("参数: script="), "{b}");
        assert!(b.contains("脚本"), "{b}");
    }

    #[test]
    fn test_confirmation_block_survives_bad_payload() {
        let b = confirmation_block("strategy", "不是 TOML");
        assert!(b.contains("载荷解析失败"), "{b}");
        assert!(b.contains("不是 TOML"), "原文必须原样附上: {b}");
    }
}
