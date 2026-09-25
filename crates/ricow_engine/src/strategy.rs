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
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    })
}

/// 一次落盘的结果(路径 + 受控覆盖时的旧脚本备份)。
#[derive(Debug, Clone)]
pub struct DeployedStrategy {
    pub toml_path: PathBuf,
    pub lua_path: PathBuf,
    /// 受控覆盖(FR-044)时旧脚本的备份路径; 全新部署为 `None`。
    pub backup: Option<PathBuf>,
}

/// 部署已批准的建策略 preview (002 FR-003/FR-004/FR-005): 消费一次性 token → 落盘。
///
/// - `(preview_id, token)` 由 `confirm::consume` **原子**校验: 未批准 / token 不匹配 / 已消费 / 过期
///   都在此步失败, 因此部署无法绕过两步确认;
/// - **同名策略默认拒绝覆盖**(不覆盖用户已部署的策略, 也不静默改名) —— 默认路径是**换个新名**部署;
/// - `allow_replace = true` 走 **FR-044 受控覆盖**: 必须由"已逐字确认"的调用方传入(对话内确认块),
///   覆盖前**先把旧 `.lua`/`.toml` 备份**成 `<name>.<ext>.<ts>.bak`, 备份失败即中止(不拿用户资产冒险);
/// - 落盘形态 (031): 代码进 `strategies/{market}/<name>.lua`(市场子目录, 与策略源码目录统一),
///   实例 TOML 留在 `strategies/<name>.toml`(向后兼容), 只留 `params.script_path = "{market}/{name}.lua"`
///   —— 避免把整段 Lua 内嵌回 TOML (loader 会把 `script_path` 的内容注入内存 `script`, 若不摘除就会在写回时被固化);
/// - `market` 只允许 `spot`/`futures`(031 FR-006: 不存在第三个市场目录), 非法即拒、不落盘;
/// - 写 TOML 失败时回收已写的 `.lua`, 不留半成品。
pub async fn execute_strategy(
    db: &Database,
    preview_id: &str,
    token: &str,
    dir: &Path,
    allow_replace: bool,
) -> CoreResult<DeployedStrategy> {
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

    // 031 FR-006: 策略源码落市场子目录, 实例 TOML 留根(向后兼容)。
    let market = config.market.as_str();
    if market != "spot" && market != "futures" {
        return Err(CoreError::InvalidArgument(format!(
            "market 仅支持 spot|futures, 收到 '{market}'"
        )));
    }
    let src_dir = dir.join(market);
    let toml_path = dir.join(format!("{name}.toml"));
    let lua_path = src_dir.join(format!("{name}.lua"));
    let existed = toml_path.exists() || lua_path.exists();
    if existed && !allow_replace {
        return Err(CoreError::InvalidArgument(format!(
            "同名策略已存在, 拒绝覆盖: {} (如需替换: 换个新名部署, 或在 AI 对话里走 \"改脚本\" \
             受控覆盖流程 —— 会先备份旧脚本)",
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
    config.params.insert("script_path".into(), ConfigValue::String(format!("{market}/{name}.lua")));
    let toml_str =
        config.to_toml().map_err(|e| CoreError::Parse(format!("策略 TOML 序列化失败: {e}")))?;

    std::fs::create_dir_all(&src_dir).map_err(|e| {
        CoreError::Exchange(format!("创建策略目录 {} 失败: {e}", src_dir.display()))
    })?;
    // FR-044: 受控覆盖前先备份。放在写文件之前, 且备份失败即中止 —— 旧脚本是用户资产,
    // 一旦被覆盖就无从恢复(预览 TTL 过期后也拿不回原文)。
    let backup = if existed { Some(backup_existing(&name, &toml_path, &lua_path)?) } else { None };
    std::fs::write(&lua_path, &code)
        .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", lua_path.display())))?;
    if let Err(e) = std::fs::write(&toml_path, &toml_str) {
        let _ = std::fs::remove_file(&lua_path); // 不留半成品
        return Err(CoreError::Exchange(format!("写 {} 失败: {e}", toml_path.display())));
    }
    Ok(DeployedStrategy { toml_path, lua_path, backup })
}

/// FR-044: 覆盖前把已有的 `<name>.{lua,toml}` 备份成同目录下 `<name>.<ext>.<ts>.bak`。
///
/// 返回首个备份路径(供如实回报给用户); 存在即备份、不存在就跳过。**任一步失败都要中止落盘**:
/// 宁可不覆盖, 也不能在没有备份的情况下抹掉旧脚本。备份落在各文件自己的父目录
/// (031 起 `.lua` 在 `strategies/{market}/`、`.toml` 在 `strategies/` 根)。
fn backup_existing(name: &str, toml_path: &Path, lua_path: &Path) -> CoreResult<PathBuf> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let mut first: Option<PathBuf> = None;
    for (src, ext) in [(lua_path, "lua"), (toml_path, "toml")] {
        if !src.exists() {
            continue;
        }
        let dir = src.parent().unwrap_or_else(|| Path::new("."));
        let mut dst = dir.join(format!("{name}.{ext}.{ts}.bak"));
        // 时间戳只到秒: 同一秒内第二次覆盖会撞名, `copy` 会直接抹掉上一份备份 ——
        // 用户以为有两个回滚点, 实际只剩一个。依次加序号直到空位。
        let mut n = 1u32;
        while dst.exists() {
            dst = dir.join(format!("{name}.{ext}.{ts}-{n}.bak"));
            n += 1;
        }
        std::fs::copy(src, &dst).map_err(|e| {
            CoreError::Exchange(format!(
                "备份旧脚本失败({} → {}): {e}; 已中止覆盖, 旧策略保持原样",
                src.display(),
                dst.display()
            ))
        })?;
        first.get_or_insert(dst);
    }
    Ok(first.unwrap_or_else(|| lua_path.with_extension("lua.bak")))
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
        assert!(execute_strategy(&db, &pid, "forged-token", &dir, false).await.is_err());
        assert!(!dir.join("ai-grid.toml").exists(), "未批准不得落盘");
        assert!(!dir.join("spot").join("ai-grid.lua").exists());

        // 批准后成功 (031: 代码落市场子目录, 实例 TOML 留根)
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let out = execute_strategy(&db, &pid, &token, &dir, false).await.unwrap();
        let (toml_path, lua_path) = (out.toml_path, out.lua_path);
        assert_eq!(toml_path, dir.join("ai-grid.toml"));
        assert_eq!(lua_path, dir.join("spot").join("ai-grid.lua"));
        assert!(out.backup.is_none(), "全新部署不应产生备份");

        let body = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            body.contains("script_path") && body.contains("spot/ai-grid.lua"),
            "TOML 应以相对根的路径引用市场子目录脚本: {body}"
        );
        assert!(!body.contains("function on_tick"), "R1: 代码不得内嵌进 TOML: {body}");
        let lua = std::fs::read_to_string(&lua_path).unwrap();
        assert!(lua.contains("function on_tick"), "脚本内容应与提交一致");

        // 一次性 token: 二次部署失败
        assert!(execute_strategy(&db, &pid, &token, &dir, false).await.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_execute_strategy_refuses_overwrite() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = tmp_dir("t2");
        // 用户已有一个同名实例 TOML (根目录)
        std::fs::write(dir.join("ai-grid.toml"), "[strategy]\nname = \"ai-grid\"\n").unwrap();

        let pid = preview_of(&db, "ai-grid").await;
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let err = execute_strategy(&db, &pid, &token, &dir, false).await.unwrap_err();
        assert!(err.to_string().contains("拒绝覆盖"), "{err}");
        // 不产生半成品: 市场子目录下的 .lua 不应被写出来
        assert!(!dir.join("spot").join("ai-grid.lua").exists(), "拒绝覆盖时不得留下脚本文件");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// FR-044 受控覆盖: 经确认块放行时必须**先备份旧脚本**, 再写新脚本。
    #[tokio::test]
    async fn test_execute_strategy_replace_backs_up_old_script_first() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = tmp_dir("t4");
        let spot = dir.join("spot");
        std::fs::create_dir_all(&spot).unwrap();
        let old_lua = "function on_tick(ctx)\n    return { old = true }\nend\n";
        std::fs::write(spot.join("ai-grid.lua"), old_lua).unwrap();
        std::fs::write(dir.join("ai-grid.toml"), "[strategy]\nname = \"ai-grid\"\n").unwrap();

        let pid = preview_of(&db, "ai-grid").await;
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let out = execute_strategy(&db, &pid, &token, &dir, true).await.unwrap();

        let backup = out.backup.expect("受控覆盖必须产生备份");
        assert!(backup.exists(), "备份文件必须落盘: {}", backup.display());
        assert!(
            backup.to_string_lossy().contains("ai-grid.lua."),
            "备份名应含原名与扩展名: {}",
            backup.display()
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), old_lua, "备份必须是旧脚本原文");
        // 新脚本已就位(与旧脚本不同)
        let now = std::fs::read_to_string(&out.lua_path).unwrap();
        assert!(now.contains("function on_tick"), "{now}");
        assert_ne!(now, old_lua, "覆盖后应是新脚本");
        // TOML 也一并备份(参数同样不可丢)
        assert!(
            dir.read_dir().unwrap().flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with("ai-grid.toml.") && n.ends_with(".bak")
            }),
            "旧 TOML 也应备份"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_execute_strategy_rejects_path_traversal_name() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = tmp_dir("t3");
        let pid = preview_of_raw_name(&db, "../evil").await;
        let token = crate::confirm::approve(&db, &pid).await.unwrap();
        let err = execute_strategy(&db, &pid, &token, &dir, false).await.unwrap_err();
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
