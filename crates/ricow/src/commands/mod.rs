//! CLI 子命令模块。

/// 回测报告行写入(写 `String` 不会失败; 忽略 `fmt::Error`, 免得每行都写 `let _ =`)。
macro_rules! line {
    ($out:expr, $($arg:tt)*) => {{
        let _ = writeln!($out, $($arg)*);
    }};
}

pub mod agentkit;
pub mod ai;
pub mod approve;
pub mod backtest;
pub mod chat;
pub mod config_file;
pub mod create;
pub mod ctrl;
pub mod daemon;
pub mod db;
pub mod deploy;
pub mod instances;
pub mod logs;
pub mod market;
pub mod onboard;
pub mod pairs;
pub mod run;
pub mod templates;

use std::fmt::Write as _;
use std::io::IsTerminal;

use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{ConfigValue, StrategyConfig};
use rust_decimal::Decimal;

/// 创建 Binance 现货交易所 (公开行情 API, 免 key)。
pub(crate) fn bn_exchange() -> ricow_core::CoreResult<std::sync::Arc<dyn ricow_core::Exchange>> {
    let client = ricow_binance::BinanceClient::new()?;
    Ok(std::sync::Arc::new(ricow_binance::BnSpotExchange::new(client)))
}

/// 创建带凭据的 Binance 现货交易所 (签名端点: 实盘下单 / 账户查询)。
/// 创建带凭据的 Binance USDT-M 合约交易所 (012; 返回具体类型以便启动预配置)。
/// 按策略市场创建带凭据的交易所 (spot / futures)。
/// 交易所对接模式: `Live` = 主网(真实资金); `Demo` = 币安测试网 demo(真实调用、无真实资金)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Live,
    Demo,
}

/// 币安测试网(demo)端点。现货与合约是两套域名, 与主网同样分离。
pub(crate) const DEMO_SPOT_URL: &str = "https://demo-api.binance.com";
pub(crate) const DEMO_FAPI_URL: &str = "https://demo-fapi.binance.com";

/// 测试网凭据的环境变量覆盖名(成对出现时优先于 ricow.toml [exchange])。
pub(crate) const ENV_DEMO_KEY: &str = "RICOW_DEMO_KEY";
pub(crate) const ENV_DEMO_SECRET: &str = "RICOW_DEMO_SECRET";

impl Mode {
    /// 面向用户的模式名(打印时必须如实, 绝不把 demo 说成实盘)。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Live => "实盘",
            Mode::Demo => "测试网模拟盘(demo)",
        }
    }
}

/// 按模式取凭据。**两套凭据不互相回落**: demo 的 key 永远不被当作实盘 key 使用(反之亦然)。
pub(crate) fn load_credentials(mode: Mode) -> ricow_core::CoreResult<(String, String)> {
    match mode {
        Mode::Live => load_live_credentials(),
        Mode::Demo => load_demo_credentials(&project_root()),
    }
}

/// demo 凭据(env 成对覆盖 > 指定 root 下 ricow.toml [exchange])。拆出 root 参数便于 AI 会话按数据目录校验。
pub(crate) fn load_demo_credentials(
    root: &std::path::Path,
) -> ricow_core::CoreResult<(String, String)> {
    // 覆盖优先级: 环境变量 RICOW_DEMO_KEY/RICOW_DEMO_SECRET > ricow.toml [exchange]。
    // env 供 CI/临时测试注入测试网凭据, 避免把 key 写进仓库配置; 两个必须成对出现, 否则回落文件。
    let env_pair = (
        std::env::var(ENV_DEMO_KEY).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        std::env::var(ENV_DEMO_SECRET).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
    );
    let cfg = config_file::load(root)?;
    let path = config_file::path(root);
    let pair = match env_pair {
        (Some(k), Some(s)) => (Some(k), Some(s)),
        _ => (cfg.exchange.demo_key, cfg.exchange.demo_secret),
    };
    match pair {
        (Some(k), Some(s)) => Ok((k, s)),
        _ => Err(ricow_core::CoreError::Auth(format!(
            "测试网(demo)凭据未填写: 请在 {} 的 [exchange] 段填入 demo_key / demo_secret\n             \
             (或设置环境变量 {ENV_DEMO_KEY} / {ENV_DEMO_SECRET}; 币安 demo: demo.binance.com → API 管理 → 创建; 与实盘凭据分开存放)",
            path.display()
        ))),
    }
}

/// 带凭据构造现货交易所(可指定 demo 域名)。
pub(crate) fn bn_spot_signed_mode(
    mode: Mode,
) -> ricow_core::CoreResult<std::sync::Arc<dyn ricow_core::Exchange>> {
    let (key, secret) = load_credentials(mode)?;
    let mut client = ricow_binance::BinanceClient::new()?.with_credentials(key, secret);
    if mode == Mode::Demo {
        client = client.with_base_url(DEMO_SPOT_URL);
    }
    Ok(std::sync::Arc::new(ricow_binance::BnSpotExchange::new(client)))
}

/// 带凭据构造合约交易所(可指定 demo 域名)。
pub(crate) fn bn_futures_signed_mode(
    mode: Mode,
) -> ricow_core::CoreResult<ricow_binance::BnFuturesExchange> {
    let (key, secret) = load_credentials(mode)?;
    let mut client = ricow_binance::FuturesClient::with_credentials(key, secret, None)?;
    if mode == Mode::Demo {
        client = client.with_base_url(DEMO_FAPI_URL);
    }
    Ok(ricow_binance::BnFuturesExchange::new(client))
}

/// 按策略市场 + 模式创建交易所。
pub(crate) fn bn_signed_exchange_mode(
    market: &str,
    mode: Mode,
) -> ricow_core::CoreResult<std::sync::Arc<dyn ricow_core::Exchange>> {
    if market.eq_ignore_ascii_case("futures") {
        Ok(std::sync::Arc::new(bn_futures_signed_mode(mode)?))
    } else {
        bn_spot_signed_mode(mode)
    }
}

/// 时钟预检取数: 公共端点读**该市场自己的**服务器时间 (免 key), 返回"本机 - 交易所"偏差 (ms)。
///
/// 现货与合约的 demo 服务器时间不同步(实测差 1.5~1.9s), 必须按市场取数, 否则校准白做。
/// 前台 `run` 与 `start` 的实盘预检 (019 R4) 共用这一份实现。
pub(crate) async fn fetch_clock_skew(market: &str, mode: Mode) -> CoreResult<i64> {
    let demo = mode == Mode::Demo;
    let server = if market.eq_ignore_ascii_case("futures") {
        // 公开端点: 无需凭据
        let c = ricow_binance::FuturesClient::new()?;
        let c = if demo { c.with_base_url(DEMO_FAPI_URL) } else { c };
        c.server_time().await
    } else {
        let c = ricow_binance::BinanceClient::new()?;
        let c = if demo { c.with_base_url(DEMO_SPOT_URL) } else { c };
        c.server_time().await
    }
    .map_err(|e| {
        CoreError::Network(format!(
            "时钟预检失败: 读取交易所服务器时间失败 ({e}); 网络不通时无法安全启动实盘"
        ))
    })?;
    Ok(ricow_engine::skew_ms(chrono::Utc::now(), server))
}

/// API 凭据 (签名请求必需)。
///
/// 来源优先级: env `RICOW_BN_API_KEY` / `RICOW_BN_SECRET_KEY` (specs/testnet.md 的联调约定,
/// demo 与主网同接口) → 唯一配置文件 `ricow.toml` 的 `[exchange]` 段。
/// 两处都没有时明确报错, 不静默用空凭据发起请求。
pub(crate) fn load_live_credentials() -> ricow_core::CoreResult<(String, String)> {
    // 环境变量是**显式覆盖**(CI/临时/密钥管理器注入), 不是回落; 值不落盘。
    let env_pair =
        (std::env::var("RICOW_BN_API_KEY").ok(), std::env::var("RICOW_BN_SECRET_KEY").ok());
    if let (Some(k), Some(s)) = env_pair {
        if !k.trim().is_empty() && !s.trim().is_empty() {
            return Ok((k.trim().to_string(), s.trim().to_string()));
        }
    }
    let root = project_root();
    let cfg = config_file::load(&root)?;
    let path = config_file::path(&root);
    match (cfg.exchange.binance_key, cfg.exchange.binance_secret) {
        (Some(k), Some(s)) => Ok((k, s)),
        _ => Err(ricow_core::CoreError::Auth(format!(
            "实盘凭据未填写: 请在 {} 的 [exchange] 段填入 binance_key / binance_secret\n             (或临时设环境变量 RICOW_BN_API_KEY / RICOW_BN_SECRET_KEY)",
            path.display()
        ))),
    }
}

/// 项目根 (数据目录, 决策 D4):
/// 1) `RICOW_ROOT` 显式覆盖优先
/// 2) 当前目录已有 `ricow.db` 或 `strategies/` → 沿用现状 (不静默迁移用户数据)
/// 3) 否则用平台标准数据目录:
///    Windows `%APPDATA%\ricow` / macOS `~/Library/Application Support/ricow` / Linux `$XDG_DATA_HOME|~/.local/share` `/ricow`
pub(crate) fn project_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let explicit = std::env::var("RICOW_ROOT").ok().map(std::path::PathBuf::from);
    resolve_root(explicit, cwd, platform_data_dir())
}

/// 数据目录决策 (纯函数, 便于单测): 显式覆盖 > 现状回退 > 平台标准目录。
fn resolve_root(
    explicit: Option<std::path::PathBuf>,
    cwd: std::path::PathBuf,
    platform: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if cwd.join("ricow.db").exists() || cwd.join("strategies").exists() {
        return cwd;
    }
    platform.unwrap_or(cwd)
}

/// 平台标准数据目录 (不引入额外依赖: 按平台约定取环境变量)。
pub(crate) fn platform_data_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|p| std::path::PathBuf::from(p).join("ricow")).or_else(
            || std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p).join(".ricow")),
        )
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|p| std::path::PathBuf::from(p).join("Library/Application Support/ricow"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
            return Some(std::path::PathBuf::from(x).join("ricow"));
        }
        std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".local/share/ricow"))
    }
}

/// 本地数据库默认路径 (<项目根>/ricow.db, 可用 RICOW_DB 覆盖)。
pub(crate) fn default_db_path() -> std::path::PathBuf {
    db_path_in(&project_root())
}

/// 数据库路径 (以**调用方给定的 root** 为基准, `RICOW_DB` 全局覆盖仍最优先)。
///
/// 与 [`default_db_path`] 的区别只在基准目录: 会话/daemon 手里已经有 root, 直接用它的
/// 即可, 且必须与 `create`/`approve`/`deploy` 取同一个库 —— 否则设了 `RICOW_DB` 时
/// 会出现"预览写在一个库里、确认去另一个库里找"的静默不一致。
pub(crate) fn db_path_in(root: &std::path::Path) -> std::path::PathBuf {
    if let Ok(p) = std::env::var("RICOW_DB") {
        return std::path::PathBuf::from(p);
    }
    root.join("ricow.db")
}

/// 策略目录 (<项目根>/strategies/)。
pub(crate) fn strategies_dir() -> std::path::PathBuf {
    project_root().join("strategies")
}

/// 读取策略 TOML 配置 (仅解析; 不做 enabled/脚本门禁 —— 供展示类命令使用)。
pub(crate) fn read_strategy_config(name: &str) -> Option<StrategyConfig> {
    read_strategy_config_in(&project_root(), name)
}

/// 同上, 但显式指定数据目录 (019 R4: 会话 root 与进程全局 root 未必同一份时按前者取数)。
pub(crate) fn read_strategy_config_in(
    root: &std::path::Path,
    name: &str,
) -> Option<StrategyConfig> {
    let path = root.join("strategies").join(format!("{name}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    StrategyConfig::from_toml(&text).ok()
}

/// 写文件类动作的策略名门禁(防目录穿越): 只接受单段名, 不得含路径分隔/上跳/隐藏前缀。
///
/// 与 `ai::tools::safe_strategy_name` 同一口径 —— 那一个在工具入口校验(模型入参),
/// 这一个在**宿主内核**再校验一次(写盘/删除路径的最后一道, 不依赖上游是否校验过)。
fn safe_strategy_file_stem(name: &str) -> CoreResult<()> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        return Err(CoreError::InvalidArgument(format!(
            "策略名非法: \"{name}\" (不得为空、不得含路径分隔符/..)"
        )));
    }
    Ok(())
}

/// 参数值渲染(回执用): 字符串原样, 数值/布尔按字面。
fn render_config_value(v: &ConfigValue) -> String {
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

/// 改策略参数并落盘(023 FR-024/FR-025): 读 TOML → 合并 `[strategy.params]` → 时间戳备份 → 写回。
///
/// - **只动 `params`**: 顶层 `enabled` / `live_enabled` / `market` / `position_mode` 等原样保留 ——
///   实盘开关由实盘门禁管辖, 不让"改参数"顺手把它打开;
/// - 写前留同目录 `{name}.toml.<时间戳>.bak`(重写会丢注释, 留一份可回溯);
/// - 返回逐键差异 `(键, "改前 → 改后")`, 找不到旧值记 `<未设置>`;
/// - 前置校验全在写之前完成(名字/非空/可读/可解析), 任一失败即返回 `Err` 且**不落盘**。
pub(crate) fn update_strategy_params_in(
    root: &std::path::Path,
    name: &str,
    updates: &[(String, ConfigValue)],
) -> CoreResult<Vec<(String, String)>> {
    safe_strategy_file_stem(name)?;
    if updates.is_empty() {
        return Err(CoreError::InvalidArgument("没有需要修改的参数".into()));
    }
    let dir = root.join("strategies");
    let path = dir.join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::InvalidArgument(format!("读取策略 {name} 失败: {e}")))?;
    let mut config = StrategyConfig::from_toml(&text)
        .map_err(|e| CoreError::InvalidArgument(format!("策略 {name} TOML 解析失败: {e}")))?;

    let mut diffs = Vec::with_capacity(updates.len());
    for (k, v) in updates {
        let before =
            config.params.get(k).map(render_config_value).unwrap_or_else(|| "<未设置>".to_string());
        config.params.insert(k.clone(), v.clone());
        diffs.push((k.clone(), format!("{before} → {}", render_config_value(v))));
    }

    let backup = dir.join(format!("{name}.toml.{}.bak", chrono::Utc::now().format("%Y%m%d%H%M%S")));
    std::fs::copy(&path, &backup)
        .map_err(|e| CoreError::Exchange(format!("备份 {} 失败: {e}", backup.display())))?;

    let toml_str =
        config.to_toml().map_err(|e| CoreError::Parse(format!("策略 TOML 序列化失败: {e}")))?;
    std::fs::write(&path, toml_str)
        .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", path.display())))?;
    Ok(diffs)
}

/// 删除策略文件(023 FR-024): 只删 `strategies/{name}.toml` 与 `strategies/{name}.lua`(存在才删)。
///
/// **不动 `logs/`**: 停机与成交留痕要保留, 供事后追溯。返回被删文件的文件名列表。
/// 前置校验全在删除之前完成(名字/至少命中一个文件), 避免"删了一半才发现名字非法"。
pub(crate) fn delete_strategy_files_in(
    root: &std::path::Path,
    name: &str,
) -> CoreResult<Vec<String>> {
    safe_strategy_file_stem(name)?;
    let dir = root.join("strategies");
    let candidates = [dir.join(format!("{name}.toml")), dir.join(format!("{name}.lua"))];
    let existing: Vec<&std::path::PathBuf> = candidates.iter().filter(|p| p.is_file()).collect();
    if existing.is_empty() {
        return Err(CoreError::InvalidArgument(format!("策略 {name} 不存在(没有找到 .toml/.lua)")));
    }
    let mut removed = Vec::with_capacity(existing.len());
    for p in &existing {
        std::fs::remove_file(p)
            .map_err(|e| CoreError::Exchange(format!("删除 {} 失败: {e}", p.display())))?;
        removed.push(p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    }
    removed.sort();
    Ok(removed)
}

/// 是否为**明确确认**(裸 y/yes/ok/n/no/空 一律不算; 必须逐字等于 expected)。019 T026/T030 共用。
///
/// **仅终端渠道**(`approve.rs` / `run.rs` / `ctrl.rs`)在用; 对话渠道用当前语言的口语词,
/// 见 [`crate::ai::confirm::is_simple_confirmation`](023 决策 D1/D2)。
pub(crate) fn is_explicit_confirmation(input: &str, expected: &str) -> bool {
    let t = input.trim();
    if t.is_empty() {
        return false;
    }
    if matches!(t.to_ascii_lowercase().as_str(), "y" | "yes" | "ok" | "n" | "no") {
        return false;
    }
    t == expected
}

/// 是否显式放弃(标记拒绝 / 中止操作)。
pub(crate) fn is_explicit_rejection(input: &str) -> bool {
    matches!(input.trim().to_ascii_lowercase().as_str(), "拒绝" | "reject")
}

/// 交互式**明确短语**确认: 打印上下文 → 读一行 → 逐字匹配。
///
/// 不匹配(空输入 / EOF / 裸 y / 错字) → `Err`, 且**零副作用**(调用方不得在此之前产生任何动作)。
/// 确认动作必须来自**交互终端** (019 spec §七 R2): stdin 不是终端即拒绝。
///
/// 为什么必须是**结构性**门禁: 逐字短语的语义是"用户本人在自己的终端知情确认"。
/// 若 stdin 可以是管道/重定向(脚本、`echo … | ricow approve`、AI agent 的工具调用),
/// 任何程序都能把短语喂进来 —— 于是"人工确认"退化成"格式正确的输入", 门禁形同虚设。
/// 拆成接受 bool 的纯函数, 便于单测两条分支。
pub(crate) fn require_interactive_terminal(is_terminal: bool) -> ricow_core::CoreResult<()> {
    if is_terminal {
        return Ok(());
    }
    Err(ricow_core::CoreError::InvalidArgument(
        "确认必须在**交互终端**输入(检测到标准输入不是终端): 本命令不接受管道/脚本/工具调用喂入的确认短语。\n\
         请在你自己终端的提示符下直接执行本命令, 手动逐字输入短语。"
            .to_string(),
    ))
}

pub(crate) fn require_explicit_phrase(context: &str, expected: &str) -> ricow_core::CoreResult<()> {
    require_interactive_terminal(std::io::stdin().is_terminal())?;
    println!("{context}");
    println!("请输入确认短语(逐字): {expected}");
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| ricow_core::CoreError::Parse(e.to_string()))?;
    if is_explicit_confirmation(&input, expected) {
        println!("已确认: {expected}");
        return Ok(());
    }
    Err(ricow_core::CoreError::InvalidArgument(format!(
        "未确认: 输入与确认短语不一致(期望逐字: {expected}); 未执行任何动作。"
    )))
}

/// 已部署策略名 (`strategies/*.toml` 文件名, 排序)。
pub(crate) fn deployed_strategy_names() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(strategies_dir()) else {
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

/// 确保策略目录存在 (运行时生成), 返回目录路径。
pub(crate) fn ensure_strategies_dir() -> CoreResult<std::path::PathBuf> {
    ensure_strategies_dir_in(&project_root())
}

/// 同 [ensure_strategies_dir], 但显式指定数据目录(AI 会话/测试用)。
pub(crate) fn ensure_strategies_dir_in(root: &std::path::Path) -> CoreResult<std::path::PathBuf> {
    let dir = root.join("strategies");
    std::fs::create_dir_all(&dir).map_err(|e| {
        CoreError::InvalidArgument(format!("创建策略目录 {} 失败: {e}", dir.display()))
    })?;
    Ok(dir)
}

/// 内置脚本 Lua 化: strategy_type 命中内置名且无 script 参数时, 注入
/// `strategies/builtin/` 对应脚本内容 (编译期嵌入), type 改 "lua"。
/// 非内置名 / 已有 script(用户 lua 策略)原样返回。
///
/// 内置清单本身在 [`templates`](crate::commands::templates)(带类别/说明/参数摘要, 供 AI 工具复用)。
pub(crate) fn resolve_builtin_script(mut config: StrategyConfig) -> CoreResult<StrategyConfig> {
    let Some(code) = templates::code_of(&config.strategy_type) else {
        return Ok(config); // 非内置名
    };
    if config.get_str("script").is_some() {
        return Ok(config); // 已有 script(用户 lua 策略)
    }
    config.params.insert("script".into(), ConfigValue::String(code.to_string()));
    config.strategy_type = "lua".into();
    Ok(config)
}

/// 从策略目录加载已部署策略 TOML。
///
/// lua 策略: `params.script_path` 存在则读文件内容填入 `script` (相对 dir 或绝对路径);
/// 无 `script_path` 才用内嵌 `script` (旧 create_strategy 部署兼容); 两者皆无报错。
/// 内置名 type (如 shannon_grid) 经 `resolve_builtin_script` Lua 化。
pub(crate) fn load_strategy_toml(dir: &std::path::Path, name: &str) -> CoreResult<StrategyConfig> {
    let path = dir.join(format!("{name}.toml"));
    let content = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::InvalidArgument(format!("读取策略 {name} 失败: {e}")))?;
    let mut config = StrategyConfig::from_toml(&content)
        .map_err(|e| CoreError::InvalidArgument(format!("策略 {name} TOML 解析失败: {e}")))?;
    if !config.enabled {
        return Err(CoreError::InvalidArgument(format!("策略 {name} 未启用 (enabled=false)")));
    }
    if config.strategy_type == "lua" && config.get_str("script").is_none() {
        match config.get_str("script_path") {
            Some(p) => {
                let script_path = if std::path::Path::new(p).is_absolute() {
                    std::path::PathBuf::from(p)
                } else {
                    dir.join(p)
                };
                let code = std::fs::read_to_string(&script_path).map_err(|e| {
                    CoreError::InvalidArgument(format!(
                        "读取策略 {name} 脚本 {} 失败: {e}",
                        script_path.display()
                    ))
                })?;
                config.params.insert("script".into(), ConfigValue::String(code));
            }
            None => {
                return Err(CoreError::InvalidArgument(format!(
                    "lua 策略 {name} 需要 script 或 script_path 参数"
                )))
            }
        }
    }
    resolve_builtin_script(config)
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::Mutex;

    /// 串行化 RICOW_ROOT/RICOW_DB 环境变量操作 (并行测试会互相污染进程 env)。
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn with_ricow_root(root: &str, f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", root);
        f();
        std::env::remove_var("RICOW_ROOT");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_util::with_ricow_root;
    use std::collections::HashMap;

    // ---- 019 spec §七 R2: 人工确认必须来自交互终端 ----

    #[test]
    fn test_require_interactive_terminal_rejects_pipe() {
        // 非终端(管道/重定向/agent 工具调用) → 拒绝, 且错误要可执行
        let err = require_interactive_terminal(false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("交互终端"), "{msg}");
        assert!(msg.contains("管道"), "要写明管道无效: {msg}");
    }

    #[test]
    fn test_require_interactive_terminal_accepts_tty() {
        assert!(require_interactive_terminal(true).is_ok());
    }

    #[test]
    fn test_paths_follow_ricow_root() {
        with_ricow_root("/tmp/ricow-root-test", || {
            assert_eq!(
                strategies_dir(),
                std::path::PathBuf::from("/tmp/ricow-root-test/strategies")
            );
            assert_eq!(
                default_db_path(),
                std::path::PathBuf::from("/tmp/ricow-root-test/ricow.db")
            );
            // RICOW_DB 单独覆盖优先。
            std::env::set_var("RICOW_DB", "/tmp/custom.db");
            assert_eq!(default_db_path(), std::path::PathBuf::from("/tmp/custom.db"));
            std::env::remove_var("RICOW_DB");
        });
    }

    #[test]
    fn test_data_dir_resolution_rules() {
        let base = std::env::temp_dir().join(format!("ricow-root-rule-{}", std::process::id()));
        let empty = base.join("empty");
        let legacy = base.join("legacy");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        let platform = base.join("platform/ricow");

        // 1) 显式 RICOW_ROOT 优先于一切
        assert_eq!(
            resolve_root(Some(base.join("explicit")), legacy.clone(), Some(platform.clone())),
            base.join("explicit")
        );
        // 2) 空目录 → 平台标准数据目录
        assert_eq!(
            resolve_root(None, empty.clone(), Some(platform.clone())),
            platform,
            "空目录应用平台标准目录"
        );
        // 3) 现状回退: 目录里已有 strategies/ (或 ricow.db)
        std::fs::create_dir_all(legacy.join("strategies")).unwrap();
        assert_eq!(
            resolve_root(None, legacy.clone(), Some(platform.clone())),
            legacy,
            "已有 strategies/ 时应沿用当前目录 (不静默迁移)"
        );
        // 4) 无平台目录可取时回退 cwd
        assert_eq!(resolve_root(None, empty.clone(), None), empty);

        // 平台目录按各自约定拼装 (本机至少应返回一个含 ricow 的路径)
        if let Some(p) = platform_data_dir() {
            assert!(p.to_string_lossy().contains("ricow"), "实际: {p:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_paths_follow_resolved_root() {
        // 数据目录三条规则已由 test_data_dir_resolution_rules 覆盖; 此处校验派生路径与 root 一致。
        // 必须持 ENV_LOCK: 并行测试会临时设置 RICOW_ROOT, 不加锁会读到别家的值 (实测全量跑偶发失败)。
        let _guard = crate::commands::test_util::ENV_LOCK.lock().unwrap();
        std::env::remove_var("RICOW_ROOT");
        std::env::remove_var("RICOW_DB");
        let root = project_root();
        assert_eq!(strategies_dir(), root.join("strategies"));
        assert_eq!(default_db_path(), root.join("ricow.db"));
    }

    #[test]
    fn test_ensure_strategies_dir_creates() {
        with_ricow_root("/tmp/ricow-root-ensure-test", || {
            let dir = ensure_strategies_dir().expect("should create");
            assert!(dir.is_dir());
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    #[test]
    fn test_resolve_builtin_script_injects_lua() {
        // 内置名无 script → 注入脚本内容, type 改 lua。
        let mut params = HashMap::new();
        params.insert("pair".into(), ConfigValue::String("ETH".into()));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let resolved = resolve_builtin_script(config).unwrap();
        assert_eq!(resolved.strategy_type, "lua");
        assert!(resolved.get_str("script").unwrap().contains("on_tick"));
    }

    #[test]
    fn test_resolve_builtin_script_skips_non_builtin_and_scripted() {
        // 非内置名原样; 已有 script 的 lua 策略不动。
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: HashMap::new(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        assert_eq!(resolve_builtin_script(config).unwrap().strategy_type, "custom");

        let mut params = HashMap::new();
        params.insert(
            "script".into(),
            ConfigValue::String("function on_tick(ctx) return {} end".into()),
        );
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let resolved = resolve_builtin_script(config).unwrap();
        assert_eq!(resolved.strategy_type, "lua");
        assert!(resolved.get_str("script").unwrap().contains("on_tick"));
    }

    #[test]
    fn test_load_toml_with_script_path() {
        let dir = std::env::temp_dir().join(format!("ricow-mod-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("demo.lua"), "function on_tick(ctx) return {} end").unwrap();
        std::fs::write(
            dir.join("demo.toml"),
            r#"
[strategy]
name = "demo"
type = "lua"
enabled = true
exchange = "binance"

[strategy.params]
pair = "ETH"
script_path = "demo.lua"
"#,
        )
        .unwrap();
        let cfg = load_strategy_toml(&dir, "demo").expect("should load");
        assert_eq!(cfg.strategy_type, "lua");
        assert_eq!(cfg.get_str("script"), Some("function on_tick(ctx) return {} end"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_load_toml_lua_missing_script_errors() {
        let dir =
            std::env::temp_dir().join(format!("ricow-mod-test-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("bad.toml"),
            r#"
[strategy]
name = "bad"
type = "lua"
enabled = true
exchange = "binance"

[strategy.params]
pair = "ETH"
"#,
        )
        .unwrap();
        let err = load_strategy_toml(&dir, "bad").expect_err("lua 无 script/script_path 应报错");
        assert!(err.to_string().contains("script"), "err = {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_load_toml_inline_script_compat() {
        // 旧 create_strategy 部署: TOML 内嵌 script 字符串, 无需 script_path。
        let dir =
            std::env::temp_dir().join(format!("ricow-mod-test-inline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("old.toml"),
            r#"
[strategy]
name = "old"
type = "lua"
enabled = true
exchange = "binance"

[strategy.params]
pair = "ETH"
script = "function on_tick(ctx) return {} end"
"#,
        )
        .unwrap();
        let cfg = load_strategy_toml(&dir, "old").expect("should load");
        assert!(cfg.get_str("script").unwrap().contains("on_tick"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- 023: 改参数 / 删策略 宿主内核(写文件路径) ----

    /// 临时数据目录(带 strategies/), 返回 (root, strategies_dir)。
    fn temp_root(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("ricow-mod-023-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("strategies");
        std::fs::create_dir_all(&dir).unwrap();
        (root, dir)
    }

    fn write_strategy(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(format!("{name}.toml"));
        std::fs::write(&p, body).unwrap();
        p
    }

    const SAMPLE: &str = r#"
[strategy]
name = "g1"
type = "lua"
enabled = true
exchange = "binance"
market = "futures"
position_mode = "hedge"
live_enabled = true

[strategy.params]
pair = "ETH"
upper_price = 100.0
script_path = "g1.lua"
"#;

    #[test]
    fn test_update_strategy_params_keeps_other_fields_and_backs_up() {
        let (root, dir) = temp_root("update");
        write_strategy(&dir, "g1", SAMPLE);

        let diffs = update_strategy_params_in(
            &root,
            "g1",
            &[("upper_price".to_string(), ConfigValue::Float(120.0))],
        )
        .expect("应改成功");
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].0, "upper_price");
        assert!(diffs[0].1.contains("100"), "改前值应回显: {:?}", diffs[0].1);
        assert!(diffs[0].1.contains("120"), "改后值应回显: {:?}", diffs[0].1);

        // 顶层字段一个不动(尤其 live_enabled: 实盘开关不归"改参数"管)
        let after = std::fs::read_to_string(dir.join("g1.toml")).unwrap();
        let cfg = StrategyConfig::from_toml(&after).unwrap();
        assert!(cfg.live_enabled, "live_enabled 不得被改参数顺手打开/关掉");
        assert!(cfg.enabled);
        assert_eq!(cfg.market, "futures");
        assert_eq!(cfg.position_mode, "hedge");
        assert_eq!(cfg.get_str("pair"), Some("ETH"), "未提到的参数保持原值");
        assert_eq!(cfg.get_str("script_path"), Some("g1.lua"), "script_path 不得丢");
        assert_eq!(cfg.params.get("upper_price").and_then(|v| v.as_f64()), Some(120.0));

        // 时间戳备份留在同目录
        let baks: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with("g1.toml.") && n.ends_with(".bak"))
            .collect();
        assert_eq!(baks.len(), 1, "应留一份 .bak: {baks:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_update_strategy_params_rejects_bad_input_without_writing() {
        let (root, dir) = temp_root("update-reject");
        let path = write_strategy(&dir, "g1", SAMPLE);
        let before = std::fs::read_to_string(&path).unwrap();

        // 空更新
        assert!(update_strategy_params_in(&root, "g1", &[]).is_err());
        // 路径穿越名
        assert!(update_strategy_params_in(
            &root,
            "../evil",
            &[("a".to_string(), ConfigValue::Float(1.0))]
        )
        .is_err());
        // 不存在的策略
        assert!(update_strategy_params_in(
            &root,
            "nope",
            &[("a".to_string(), ConfigValue::Float(1.0))]
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "拒绝路径不得落盘");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_delete_strategy_files_removes_toml_and_lua_only() {
        let (root, dir) = temp_root("delete");
        write_strategy(&dir, "g1", SAMPLE);
        std::fs::write(dir.join("g1.lua"), "function on_tick(ctx) return {} end").unwrap();
        // logs/ 不得被碰
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let log = logs.join("g1.log");
        std::fs::write(&log, "trace").unwrap();

        let removed = delete_strategy_files_in(&root, "g1").expect("应删成功");
        assert_eq!(removed, vec!["g1.lua".to_string(), "g1.toml".to_string()]);
        assert!(!dir.join("g1.toml").exists());
        assert!(!dir.join("g1.lua").exists());
        assert!(log.exists(), "logs/ 必须保留(追溯用)");

        // 已删光后再删 → 明确报错, 不静默成功
        assert!(delete_strategy_files_in(&root, "g1").is_err());
        // 非法名 → 拒绝
        assert!(delete_strategy_files_in(&root, "../x").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ---- 回测报告打印 (002: backtest 与 create 共用同一口径, 避免两处格式漂移) ----

/// `Option<f64>` 展示: 无值打 `n/a`。
pub(crate) fn fmt_opt(v: Option<f64>, digits: usize) -> String {
    match v {
        Some(x) => format!("{x:.digits$}"),
        None => "n/a".into(),
    }
}

/// 回测报告正文 (表头由调用方给出, 便于区分 `backtest` 与 `create` 两条入口)。
///
/// **单一口径**(019 FR-014): CLI `ricow backtest`/`create` 的打印与 AI 工具 `run_backtest`
/// 都走本函数, 不各写一份格式化逻辑(避免数字与定义漂移)。返回 String 而非直接打印 ——
/// 由调用方决定是打印还是喂给模型。
pub(crate) fn format_backtest_report(
    report: &ricow_strategy::BacktestReport,
    header: &str,
    initial_cash: rust_decimal::Decimal,
    is_futures: bool,
) -> String {
    let mut out = String::new();
    line!(out, "{header}");
    line!(out, "  K 线数: {}", report.total_bars);
    line!(out, "  成交笔数: {}", report.total_trades);
    line!(out, "  已实现盈亏: {}", report.realized_pnl);
    line!(out, "  手续费: {}", report.total_fees);
    line!(out, "  手续费占比: {:.4}%", report.fee_ratio * Decimal::from(100));
    line!(out, "  最大回撤: {:.2}%", report.max_drawdown * Decimal::from(100));
    line!(out, "  净盈亏: {}", report.net_pnl);
    line!(out, "  胜率: {:.2}%", report.win_rate * 100.0);
    line!(out, "  拒单次数: {} (资金不足/无仓可平; Bug A 修复后如实显示)", report.rejected_count);
    if is_futures {
        match report.funding_net {
            Some(n) => line!(out, "  资金费净额: {n} (正值 = 空头净收; 多头净付为负)"),
            None => line!(out, "  资金费净额: 0 (未触发 8h 结算)"),
        }
        line!(out, "  强平次数: {}", report.liquidation_count.unwrap_or(0));
        // 013 FR-006: hedge 按侧明细 (与交易所账单对照 —— 真实清算只影响单侧)
        if let Some(h) = &report.hedge_sides {
            line!(
                out,
                "  按侧明细 (hedge): LONG 强平 {} 次 / 钱包 {} ; SHORT 强平 {} 次 / 钱包 {}",
                h.liquidation_count_long,
                h.wallet_long,
                h.liquidation_count_short,
                h.wallet_short
            );
        }
    }
    line!(out, "  --- 风险/收益指标 (无风险利率 rf = {:.2}%/年) ---", report.risk_free_rate);
    line!(out, "  年化收益率 (几何): {}", fmt_opt(report.annual_return.map(|v| v * 100.0), 2));
    line!(out, "  年化波动率: {}", fmt_opt(report.annual_volatility.map(|v| v * 100.0), 2));
    line!(out, "  夏普比率: {}", fmt_opt(report.sharpe, 2));
    line!(out, "  索提诺比率: {}", fmt_opt(report.sortino, 2));
    line!(out, "  Calmar 比率: {}", fmt_opt(report.calmar, 2));
    line!(out, "  盈亏比: {}", fmt_opt(report.profit_factor, 2));
    match (report.avg_win, report.avg_loss) {
        (Some(w), Some(l)) => line!(out, "  平均盈利/亏损: {w:.2} / {l:.2} USDT"),
        _ => line!(out, "  平均盈利/亏损: n/a"),
    }
    line!(out, "  --- 资产变化 (基准: 初始余额 {initial_cash} USDT) ---");
    line!(out, "  标的涨跌: {:+.2}% (首根open → {})", report.price_change_pct, report.final_price);
    if is_futures {
        // 合约: 持仓币数无意义 (K8), 以名义敞口替代。
        match report.final_notional {
            Some(n) => line!(
                out,
                "  名义敞口: {n} USDT ({:+.2}% vs 建仓后)",
                report.nominal_exposure_pct.unwrap_or(0.0)
            ),
            None => line!(out, "  名义敞口: 0 (期末无持仓)"),
        }
    } else {
        match report.coin_change_pct {
            Some(p) => line!(
                out,
                "  持仓币数: {} → {} ({:+.2}%)",
                report.base_coin_size,
                report.final_pos_size,
                p
            ),
            None => line!(out, "  持仓币数: 0 → {} (期初无仓, 无基准%)", report.final_pos_size),
        }
    }
    line!(
        out,
        "  现金 USDT: 建仓后 {:.2} → {:.2} ({:+.2}%)  [期初投入 {initial_cash}]",
        report.base_cash,
        report.final_cash,
        report.cash_change_pct
    );
    if is_futures {
        line!(
            out,
            "  总权益 (现金+逐仓钱包+未实现): {} USDT ({:+.2}%)",
            report.final_equity,
            report.equity_change_pct
        );
    } else {
        line!(
            out,
            "  总价值 (币+现金): {} USDT ({:+.2}%, 含未实现盈亏)",
            report.final_equity,
            report.equity_change_pct
        );
    }
    out
}

/// Dry Run 起点留痕 (002 FR-006): 首次以 Dry Run 运行时, 把起点写进策略 TOML 的 `dry_run_started_at`。
///
/// - 只在 `strategies/<name>.toml` **已存在**时写回 (直跑/内置策略无文件 → 返回 None, 不凭空建文件);
/// - 写盘用副本, 并摘掉内存中由 loader/门禁注入的 `script` 内容 —— 否则整段 Lua 会被固化进 TOML;
/// - 已记录过(`dry_run_started_at` 非空)由调用方判断, 本函数不做覆盖。
pub(crate) fn record_dry_run_start(
    config: &StrategyConfig,
) -> CoreResult<Option<std::path::PathBuf>> {
    let path = strategies_dir().join(format!("{}.toml", config.name));
    if !path.exists() {
        return Ok(None);
    }
    let mut out = config.clone();
    if out.get_str("script_path").is_some() {
        out.params.remove("script");
    }
    // 字段类型是 ISO8601 字符串 (见 StrategyConfig), 与既有 TOML 写法 `2026-07-18T00:00:00Z` 一致
    out.dry_run_started_at = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
    let toml_str =
        out.to_toml().map_err(|e| CoreError::Parse(format!("策略 TOML 序列化失败: {e}")))?;
    std::fs::write(&path, toml_str)
        .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", path.display())))?;
    Ok(Some(path))
}

/// 风险确认记录路径 (018): `$RICOW_ROOT/risk_ack.json`(数据目录内的自有文件, 不新增 DB 表)。
pub(crate) fn risk_ack_path() -> std::path::PathBuf {
    project_root().join("risk_ack.json")
}

/// 是否已完成首次使用风险确认 (018): 记录存在且 schema 版本匹配。
pub(crate) fn risk_acked() -> bool {
    let Ok(text) = std::fs::read_to_string(risk_ack_path()) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    v.get("version").and_then(|x| x.as_u64()) == Some(RISK_ACK_VERSION)
}

/// 风险确认 schema 版本: 披露内容有实质变化时递增 → 触发重新确认。
const RISK_ACK_VERSION: u64 = 1;

/// 写入风险确认记录 (018), 返回路径。
pub(crate) fn write_risk_ack() -> CoreResult<std::path::PathBuf> {
    let path = risk_ack_path();
    let body = serde_json::json!({
        "version": RISK_ACK_VERSION,
        "acked_at": chrono::Utc::now().to_rfc3339(),
        "disclosure": ricow_engine::RISK_DISCLOSURE,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap_or_default())
        .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", path.display())))?;
    Ok(path)
}
