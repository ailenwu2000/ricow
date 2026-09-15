//! 策略创建通道: 外部提交的 Lua 代码 → 编译门禁。
//!
//! P3 架构修正 (2026-08-18): ricow 不内置 LLM。AI 客户端(自带 key)读取
//! `specs/lua-api.md` 规范后编写 Lua 代码, 经 `create_strategy` 提交(入口后置:
//! 当前仅 CLI, 其他入口远期可选), 本模块负责: 提取代码块 + 编译校验 (gate 1)。
//!
//! 三关 (AST 编译校验 → 沙箱回测 → 用户确认) 中, 本模块负责第一关与**最后一关的落盘**
//! (`execute_strategy`, 002: 携一次性 token 把已批准的策略写进 `strategies/`)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{validate_lua, ConfigValue, Database, StrategyConfig};

/// 从 AI 响应中提取 Lua 代码块 (```lua / ``` / 无围栏纯代码)。
pub fn extract_code(response: &str) -> Option<String> {
    let patterns = ["```lua", "```"];

    for pattern in &patterns {
        if let Some(start) = response.find(pattern) {
            let code_start = start + pattern.len();
            let after_tag = &response[code_start..];
            let trimmed_after = after_tag.trim_start_matches(['\n', '\r']);
            let code_start_idx =
                response.len() - after_tag.len() + (after_tag.len() - trimmed_after.len());

            if let Some(end) = response[code_start_idx..].find("\n```") {
                return Some(response[code_start_idx..code_start_idx + end].trim().to_string());
            }
            if let Some(end) = response[code_start_idx..].find("```") {
                let code = response[code_start_idx..code_start_idx + end].trim();
                if !code.is_empty() {
                    return Some(code.to_string());
                }
            }
        }
    }

    // 无代码围栏, 检查整个响应是否就是代码。
    let trimmed = response.trim();
    if trimmed.contains("function on_tick") || trimmed.contains("function on_init") {
        return Some(trimmed.to_string());
    }

    None
}

/// 创建策略: 外部 AI 代码 → 提取 → 编译门禁 (gate 1) → StrategyConfig。
///
/// `name` 为策略名 (建议唯一, 部署文件按此命名); `params` 为策略配置参数
/// (config_xxx 读取), 与 code/pair 合并进 `params` 字段; 同名键以 code/pair
/// 为准 (提交参数优先, 防冲突)。
pub fn create_strategy(
    name: &str,
    code: &str,
    pair: &str,
    params: HashMap<String, ConfigValue>,
) -> Result<StrategyConfig, String> {
    // 策略名规范 (019 D18 / FR-043): 名字派生订单归属前缀, 非法字符会塌缩成 '-' 导致互相误撤挂单
    ricow_strategy::validate_strategy_name(name)?;

    ricow_strategy::lua::validate_script_source(code)?;

    let code = extract_code(code).ok_or_else(|| "未提取到 Lua 代码".to_string())?;

    validate_lua(&code).map_err(|e| format!("生成代码未通过编译门禁:\n{e}"))?;

    let mut merged: HashMap<String, ConfigValue> = HashMap::new();
    merged.insert("script".into(), ConfigValue::String(code));
    merged.insert("pair".into(), ConfigValue::String(pair.to_string()));
    for (k, v) in params {
        merged.entry(k).or_insert(v);
    }

    Ok(StrategyConfig {
        name: name.to_string(),
        strategy_type: "lua".into(),
        enabled: true,
        exchange: "binance".into(),
        params: merged,
        risk: None,
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    })
}

/// 部署已批准的建策略 preview (002 FR-003/FR-004/FR-005): 消费一次性 token → 落盘。
///
/// - `(preview_id, token)` 由 `confirm::consume` **原子**校验: 未批准 / token 不匹配 / 已消费 / 过期
///   都在此步失败, 因此部署无法绕过两步确认;
/// - **同名策略已存在即拒绝**(不覆盖用户已部署的策略, 也不静默改名);
/// - 落盘形态: 代码进 `<name>.lua`, TOML 只留 `params.script_path` —— 避免把整段 Lua 内嵌回 TOML
///   (loader 会把 `script_path` 的内容注入内存 `script`, 若不摘除就会在写回时被固化);
/// - 写 TOML 失败时回收已写的 `.lua`, 不留半成品。
///
/// 返回 `(toml_path, lua_path)`。
pub async fn execute_strategy(
    db: &Database,
    preview_id: &str,
    token: &str,
    dir: &Path,
) -> CoreResult<(PathBuf, PathBuf)> {
    let payload = crate::confirm::consume(db, preview_id, token).await?;
    let mut config = StrategyConfig::from_toml(&payload)
        .map_err(|e| CoreError::Parse(format!("preview 载荷解析失败: {e}")))?;

    let name = config.name.trim().to_string();
    if name.is_empty() {
        return Err(CoreError::InvalidArgument("preview 载荷缺策略名".into()));
    }
    // 策略名直接决定文件名 → 必须过同一套规范(字符集/长度), 兼防路径穿越与归属前缀塌缩。
    if let Err(e) = ricow_strategy::validate_strategy_name(&name) {
        return Err(CoreError::InvalidArgument(format!("策略名非法: {e}")));
    }

    let toml_path = dir.join(format!("{name}.toml"));
    let lua_path = dir.join(format!("{name}.lua"));
    if toml_path.exists() || lua_path.exists() {
        return Err(CoreError::InvalidArgument(format!(
            "同名策略已存在, 拒绝覆盖: {} (如需替换请先自行移除)",
            toml_path.display()
        )));
    }

    let code = match config.params.remove("script") {
        Some(ConfigValue::String(c)) => c,
        _ => {
            return Err(CoreError::InvalidArgument(
                "preview 载荷缺 `script` (Lua 代码), 无法部署".into(),
            ))
        }
    };
    config.params.insert("script_path".into(), ConfigValue::String(format!("{name}.lua")));
    let toml_str =
        config.to_toml().map_err(|e| CoreError::Parse(format!("策略 TOML 序列化失败: {e}")))?;

    std::fs::create_dir_all(dir)
        .map_err(|e| CoreError::Exchange(format!("创建策略目录 {} 失败: {e}", dir.display())))?;
    std::fs::write(&lua_path, &code)
        .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", lua_path.display())))?;
    if let Err(e) = std::fs::write(&toml_path, &toml_str) {
        let _ = std::fs::remove_file(&lua_path); // 不留半成品
        return Err(CoreError::Exchange(format!("写 {} 失败: {e}", toml_path.display())));
    }
    Ok((toml_path, lua_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AI_CODE: &str = "function on_tick(ctx)\n    return {}\nend\n";

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-002-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("建临时目录");
        d
    }

    async fn preview_of(db: &Database, name: &str) -> String {
        let cfg = create_strategy(name, AI_CODE, "ETHUSDT", HashMap::new()).expect("门禁应通过");
        let toml_str = cfg.to_toml().expect("序列化");
        crate::confirm::create_preview(db, "strategy", &toml_str).await.expect("建 preview")
    }

    /// 绕过创建期名字门禁的夹具(直接改 config 再序列化): 用于验证**部署期**的第二道防线。
    async fn preview_of_raw_name(db: &Database, name: &str) -> String {
        let mut cfg =
            create_strategy("ok-name", AI_CODE, "ETHUSDT", HashMap::new()).expect("门禁应通过");
        cfg.name = name.to_string();
        let toml_str = cfg.to_toml().expect("序列化");
        crate::confirm::create_preview(db, "strategy", &toml_str).await.expect("建 preview")
    }

    #[tokio::test]
    async fn test_create_strategy_rejects_invalid_name() {
        // 创建期第一道防线: 中文/空格/路径穿越名一律拒绝 (019 D18 / FR-043)
        for bad in ["网格A", "my grid", "../evil", "a/b", &"x".repeat(25)] {
            let err = create_strategy(bad, AI_CODE, "ETHUSDT", HashMap::new()).unwrap_err();
            assert!(
                err.contains("策略名") || err.contains("非法字符") || err.contains("超过上限"),
                "{bad} → {err}"
            );
        }
    }

    #[tokio::test]
    async fn test_execute_strategy_requires_approved_token() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = tmp_dir("t1");
        let pid = preview_of(&db, "ai-grid").await;

        // 未批准 → 拒绝, 且不落任何文件
        assert!(execute_strategy(&db, &pid, "forged-token", &dir).await.is_err());
        assert!(!dir.join("ai-grid.toml").exists(), "未批准不得落盘");
        assert!(!dir.join("ai-grid.lua").exists());

        // 批准后成功
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let (toml_path, lua_path) = execute_strategy(&db, &pid, &token, &dir).await.unwrap();
        assert_eq!(toml_path, dir.join("ai-grid.toml"));
        assert_eq!(lua_path, dir.join("ai-grid.lua"));

        let body = std::fs::read_to_string(&toml_path).unwrap();
        assert!(body.contains("script_path"), "TOML 应引用脚本文件: {body}");
        assert!(!body.contains("function on_tick"), "R1: 代码不得内嵌进 TOML: {body}");
        let lua = std::fs::read_to_string(&lua_path).unwrap();
        assert!(lua.contains("function on_tick"), "脚本内容应与提交一致");

        // 一次性 token: 二次部署失败
        assert!(execute_strategy(&db, &pid, &token, &dir).await.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_execute_strategy_refuses_overwrite() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = tmp_dir("t2");
        // 用户已有一个同名策略文件
        std::fs::write(dir.join("ai-grid.toml"), "[strategy]\nname = \"ai-grid\"\n").unwrap();

        let pid = preview_of(&db, "ai-grid").await;
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let err = execute_strategy(&db, &pid, &token, &dir).await.unwrap_err();
        assert!(err.to_string().contains("拒绝覆盖"), "{err}");
        // 不产生半成品: .lua 不应被写出来
        assert!(!dir.join("ai-grid.lua").exists(), "拒绝覆盖时不得留下脚本文件");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_execute_strategy_rejects_path_traversal_name() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = tmp_dir("t3");
        let pid = preview_of_raw_name(&db, "../evil").await;
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let err = execute_strategy(&db, &pid, &token, &dir).await.unwrap_err();
        assert!(err.to_string().contains("策略名非法"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_extract_code_with_tag() {
        let response = "Here is the strategy:\n\n```lua\nfunction on_tick(ctx)\n    return {}\nend\n```\n\nCheck it.";
        let code = extract_code(response);
        assert!(code.is_some());
        assert!(code.unwrap().contains("function on_tick"));
    }

    #[test]
    fn test_extract_code_plain_fence() {
        let response = "```\nfunction on_tick(ctx) return {} end\n```";
        let code = extract_code(response);
        assert!(code.is_some());
        assert!(code.unwrap().contains("function on_tick"));
    }

    #[test]
    fn test_extract_code_no_fence() {
        let response =
            "function on_tick(ctx)\n    local p = ctx:price(\"ETH\")\n    return {}\nend";
        let code = extract_code(response);
        assert!(code.is_some());
    }

    #[test]
    fn test_extract_code_none() {
        let response = "Sorry, I cannot generate this strategy.";
        assert!(extract_code(response).is_none());
    }

    #[test]
    fn test_create_strategy_valid_code() {
        let code = "function on_tick(ctx) local p = ctx:price(\"ETH\") return {} end";
        let mut params = HashMap::new();
        params.insert("order_size".into(), ConfigValue::Float(0.01));

        let config = create_strategy("my-grid", code, "ETH", params).expect("合法代码应通过门禁");
        assert_eq!(config.strategy_type, "lua");
        assert_eq!(config.name, "my-grid");
        assert!(config.get_str("script").unwrap().contains("function on_tick"));
        assert_eq!(config.get_str("pair").unwrap(), "ETH");
        assert_eq!(config.get_f64("order_size"), Some(0.01));
    }

    #[test]
    fn test_create_strategy_rejects_bad_code() {
        let err = create_strategy(
            "x",
            "function on_tick(ctx) invalid syntax!!! end",
            "ETH",
            HashMap::new(),
        )
        .expect_err("语法错误应被编译门禁拦截");
        assert!(err.contains("编译门禁"));
    }

    #[test]
    fn test_create_strategy_params_cannot_override_pair() {
        let mut params = HashMap::new();
        params.insert("pair".into(), ConfigValue::String("BTC".into()));

        let config =
            create_strategy("x", "function on_tick(ctx) return {} end", "ETH", params).unwrap();
        // 提交参数优先: pair 以 create_strategy 参数为准。
        assert_eq!(config.get_str("pair").unwrap(), "ETH");
    }
}
