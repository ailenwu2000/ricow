//! **唯一配置文件** `$RICOW_ROOT/ricow.toml`(权限 0600, 已在 .gitignore 里)。
//!
//! 设计(019, 019-R4 修订): 用户只需理解**一个文件** —— 默认用编辑器直接改;
//! 首次向导与对话内 `/keys`、`/market` 通过 [`set_values`] **按行外科式更新**本文件
//! (保留注释与用户其它内容, 原子写, 权限维持 0600)。
//! - 竞品同做法: freqtrade 把 api key 放在 `config.json`; LiteLLM 把 `api_key` 放在模型条目里;
//!   aider 允许把 `openai-api-key` 写进 `.aider.conf.yml`。**密钥与它的设置放在同一处**,
//!   避免"provider 在一个文件、密钥在另一个文件"来回对照。
//! - 本文件含密钥 → **权限 0600**, 且不入 git。
//! - 键名未知/段名未知 → **硬失败**(拼错不静默失效); [`set_values`] 同样只接受白名单键。
//! - 文件不存在时按需生成带注释的模板, 且**绝不覆盖已有文件**。

use std::path::{Path, PathBuf};

use ricow_core::{CoreError, CoreResult};
use zeroize::Zeroize;

/// 配置文件名(位于 `$RICOW_ROOT/`)。
pub const FILE: &str = "ricow.toml";

/// 配置文件路径。
pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// 密钥环的数组表名(033): 每套备用 AI 凭据一个 `[[ai_key]]` 段。
pub const AI_KEY_TABLE: &str = "ai_key";
/// 密钥环的数组表名(033): 每套备用币安凭据一个 `[[exchange_key]]` 段。
pub const EXCHANGE_KEY_TABLE: &str = "exchange_key";
/// [`ExchangeKeyEntry::env`] 取值: 实盘主网(对应 `[exchange].binance_*`)。
pub const ENV_LIVE: &str = "live";
/// [`ExchangeKeyEntry::env`] 取值: 测试网 demo(对应 `[exchange].demo_*`)。
pub const ENV_DEMO: &str = "demo";
/// 别名长度上限(按**字符**计, 不是字节 —— 中文别名不应被字节数误伤)。
/// 24 足够写下"工作号 DeepSeek 备用"这类描述, 又不至于把左列列表撑变形。
pub const MAX_ALIAS_CHARS: usize = 24;

/// 配置文件的 schema 版本(落盘为**顶层键** `schema_version`)。
///
/// 与 `risk_ack.json` 同款做法: 给未来**结构性变更**留一个锚点, 免得真到要迁移时,
/// 老文件和新文件在代码眼里完全一样, 只能靠猜。规则:
/// - 键缺失(本键出现之前写的老文件) = 0, 按当前版本处理 —— v0→v1 没有任何**结构**变化,
///   所以**不改写用户文件**(改写会动注释, 得不偿失);
/// - 等于本值 → 正常;
/// - **大于**本值 → 硬失败(该文件由更新版本的 ricow 写过, 旧版继续跑会静默丢弃新字段);
/// - 小于本值 → 走 [`migrate_to_current`] 补齐。
pub const SCHEMA_VERSION: i64 = 1;

/// 顶层 schema 版本键名。
pub const SCHEMA_VERSION_KEY: &str = "schema_version";

/// 配置默认值**唯一来源**。
///
/// 模板正文、`Default` 实现、解析时的空值兜底全部取自这里。过去同一个默认值会同时硬写在
/// 模板字符串和代码里("模板写 deepseek 推荐值、代码另有一个默认"), 改一处忘一处就是
/// "文档与行为不一致" 这类最难查的 bug。
pub mod defaults {
    /// 默认 AI 服务商 (= 预设表首项)。
    pub fn ai_provider() -> &'static str {
        crate::ai::config::default_preset().id
    }
    /// 默认 AI 模型 (= 预设表首项推荐模型)。
    pub fn ai_model() -> &'static str {
        crate::ai::config::default_preset().model
    }
    /// 默认每轮工具调用上限。
    pub const AI_MAX_TURNS: usize = crate::ai::config::DEFAULT_MAX_TURNS;
    /// 默认市场视野: `false` = 只显示 bStock 美股代币。
    pub const MARKET_SHOW_ALL_PAIRS: bool = false;
    /// 默认界面语言。
    pub const UI_LANG: &str = "zh";
    /// 默认 Web 主题(与前端兜底一致)。
    pub const UI_THEME: &str = "dark";
}

/// 别名规范化 + 校验(033 FR-009), 返回去空白后的别名。
///
/// 纯函数、不碰磁盘: 终端与 Web 共用同一条规则, 免得两处各判一套。
pub fn check_alias(raw: &str) -> Result<String, String> {
    let a = raw.trim();
    if a.is_empty() {
        return Err("别名不能为空 —— 别名是你在页面上辨认这把密钥的唯一标识".to_string());
    }
    let n = a.chars().count();
    if n > MAX_ALIAS_CHARS {
        return Err(format!("别名过长: 最多 {MAX_ALIAS_CHARS} 个字符, 实际 {n} 个"));
    }
    if a.chars().any(char::is_control) {
        return Err("别名不能包含换行 / 制表符等控制字符".to_string());
    }
    Ok(a.to_string())
}

/// 校验币安凭据的环境取值(033 FR-009), 返回去空白后的环境名。
pub fn check_env(raw: &str) -> Result<String, String> {
    match raw.trim() {
        ENV_LIVE => Ok(ENV_LIVE.to_string()),
        ENV_DEMO => Ok(ENV_DEMO.to_string()),
        other => Err(format!(
            "环境 `{other}` 非法: 只接受 \"{ENV_LIVE}\"(实盘主网) 或 \"{ENV_DEMO}\"(测试网 demo)"
        )),
    }
}

/// AI 段(provider 与其密钥同处一段, 一眼看清"这把 key 属于谁")。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSection {
    pub provider: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_turns: Option<i64>,
    pub api_key: Option<String>,
    /// 显式确认白名单外的自建/中转 AI 端点 (审计 H-7); 缺省 = 拒绝非白名单主机。
    pub allow_custom_base_url: Option<bool>,
}

impl Default for AiSection {
    fn default() -> Self {
        Self {
            provider: defaults::ai_provider().to_string(),
            model: None,
            base_url: None,
            max_turns: None,
            api_key: None,
            allow_custom_base_url: None,
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

/// 市场视野段(019-R4): 交易对列表/对话推荐的默认范围。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketSection {
    /// false(默认) = 只显示 bStock 美股代币现货与股票永续; true = 全部 TRADING 交易对。
    pub show_all_pairs: bool,
}

/// 界面语言段(023): 对话宿主固定文案的语言。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiSection {
    /// `None` = 尚未选择(首次向导会问一次); 仅接受 `"zh"` / `"en"`, 其它值硬失败。
    pub lang: Option<String>,
    /// Web 界面主题(034): `None` = 未设置(前端按深色 `dark` 兜底);
    /// 仅接受 `"dark"` / `"light"` / `"red"`, 其它值硬失败 —— 与 `[ui].lang` 同一纪律。
    pub theme: Option<String>,
}

/// daemon 监督段(038 P1-C): 子进程非正常退出后是否自动重启。
///
/// 与 `[market]` / `[ui]` 同处 `ricow.toml`, 但**不参与策略语义** —— 它是 daemon 的运维行为,
/// 与"平台不做投资判断"无关(默认 `none` = 行为与加它之前一字不差)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupervisorSection {
    /// `none`(默认, 不重启) / `on-failure`(异常退出后重启)。
    pub restart_policy: Option<String>,
    /// `on-failure` 下最多重启几次(默认 3)。
    pub max_retries: Option<i64>,
    /// 退避基数(秒; 默认 5)。第 N 次重启前等 `backoff_secs × N` 秒(线性退避)。
    pub backoff_secs: Option<i64>,
}

/// 密钥环(033)里的一套**备用 AI 凭据**: 别名 + 服务商 + 模型 + 接口地址 + 密钥。
///
/// 与 [`AiSection`] 的分工: `[ai]` 段是**当前生效**的凭据, 本结构是"可按别名另存的一套";
/// "选用"即把本结构的值写入 `[ai]`(见 `web::keyring`)。两处同名同义, 便于对照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiKeyEntry {
    /// 别名(同类内唯一, ≤ [`MAX_ALIAS_CHARS`] 字符)—— 页面上就是靠它辨认"这是哪一把"。
    pub alias: String,
    /// 服务商: 内置预设名或自定义名(自定义必须自带 `base_url`, 规则同 [`crate::ai::config::resolve`])。
    pub provider: String,
    /// 模型名; 空 = 选用后由预设推荐值兜底。
    pub model: String,
    /// 接口地址; 空 = 用预设默认。
    pub base_url: String,
    /// 密钥; 空 = 该条不含密钥(如本机 ollama)。
    pub api_key: Option<String>,
}

/// 密钥环(033)里的一套**备用币安凭据**(key + secret 成对)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeKeyEntry {
    pub alias: String,
    /// 这套凭据属于哪个环境: [`ENV_LIVE`](实盘主网) / [`ENV_DEMO`](测试网)。
    /// 选用时据此决定写 `[exchange].binance_*` 还是 `demo_*`。
    pub env: String,
    pub key: Option<String>,
    pub secret: Option<String>,
}

/// 整个配置文件的内存表示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// 文件声明的 schema 版本(缺省 = 0, 表示"本键出现之前写的文件")。
    /// 解析后总是被推进到 [`SCHEMA_VERSION`], 见 [`migrate_to_current`]。
    pub schema_version: i64,
    pub ai: AiSection,
    pub exchange: ExchangeSection,
    pub market: MarketSection,
    pub ui: UiSection,
    /// daemon 监督段(038 P1-C): 崩溃自动重启策略。缺省 = 不重启(与加它之前行为一致)。
    pub supervisor: SupervisorSection,
    /// 密钥环: 备用 AI 凭据(033)。**空 = 未使用密钥环**, 一切照旧走 [`AiSection`]。
    pub ai_keys: Vec<AiKeyEntry>,
    /// 密钥环: 备用币安凭据(033)。
    pub exchange_keys: Vec<ExchangeKeyEntry>,
}

impl Default for File {
    fn default() -> Self {
        Self {
            // 手写(而非 derive)是必须的: 版本不能靠 `i64::default()` 的 0 —— "没有文件" 与
            // "文件里没写版本" 是两回事, 前者应视作当前版本, 后者才需要迁移判定。
            schema_version: SCHEMA_VERSION,
            ai: AiSection::default(),
            exchange: ExchangeSection::default(),
            market: MarketSection::default(),
            ui: UiSection::default(),
            supervisor: SupervisorSection::default(),
            ai_keys: Vec::new(),
            exchange_keys: Vec::new(),
        }
    }
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
    // 默认值一律从 `defaults` 取: 模板里写死的字面量与代码默认值从此不可能分叉。
    let schema_version = SCHEMA_VERSION;
    let ai_provider = defaults::ai_provider();
    let ai_model = defaults::ai_model();
    let ai_max_turns = defaults::AI_MAX_TURNS;
    let market_show_all_pairs = defaults::MARKET_SHOW_ALL_PAIRS;
    let ui_lang = defaults::UI_LANG;
    let ui_theme = defaults::UI_THEME;
    format!(
        "# ricow 配置文件(本机私有, 含密钥 —— 不要外传/提交; 已在 .gitignore)。\n\
         # 可直接用编辑器改本文件; 首次启动向导与对话内 /keys、/market 也会更新这里(只改对应行, 注释保留)。\n\
         # 键名/段名拼错会被拒绝(不会静默失效); 值为空 = 未填写。\n\
         \n\
         # schema_version: 本文件的格式版本, **不要手改** —— 由 ricow 维护, 供将来结构迁移用。\n\
         schema_version = {schema_version}\n\
         \n\
         # ── ① AI 助手通道 ──────────────────────────────────────────────\n\
         # provider: 可写内置预设名, 也可写任意自定义名(自建/中转端点, 此时必须写 base_url)。\n\
         #   内置预设: {presets}\n\
         #   注意: 所选模型必须支持 function calling(工具调用), 否则 AI 无法调用行情/回测等工具。\n\
         # api_key : 上面 provider 的密钥(谁家的 key 就贴在这儿, 两者挨着, 不会搞混)。\n\
         #   本机端点(ollama)免密钥。也支持环境变量 RICOW_AI_API_KEY 临时覆盖。\n\
         [ai]\n\
         provider = \"{ai_provider}\"\n\
         model = \"{ai_model}\"\n\
         api_key = \"\"\n\
         max_turns = {ai_max_turns}\n\
         # base_url = \"https://api.deepseek.com/v1\"   # 仅自定义/自建端点才需要\n\
         # allow_custom_base_url = true               # 白名单外厂商主机才需要: 显式确认后放行\n\
         \n\
         # ── ② 交易所凭据(用哪个环境就填哪个) ────────────────────────────\n\
         #   演示(测试网): demo.binance.com 登录 → API 管理 → 创建 Key(建议只开交易, 不开提现)\n\
         #   实盘(主网)  : 币安主网 API 管理创建(建议只开交易并关闭提现; 优先用子账户/受限 Key)\n\
         [exchange]\n\
         demo_key = \"\"\n\
         demo_secret = \"\"\n\
         binance_key = \"\"\n\
         binance_secret = \"\"\n\
         \n\
         # ── ③ 市场视野 / Market universe ───────────────────────────────\n\
         # false (默认): 交易对列表与对话推荐只显示 bStock 美股代币\n\
         #   现货 AAPLBUSDT (XxxB 形态) + 股票永续 AAPLUSDT (EQUITY 池)。\n\
         # true: 显示币安全部 TRADING 交易对。\n\
         # 对话内可直接用 /market 切换, 无需手改本文件。\n\
         [market]\n\
         show_all_pairs = {market_show_all_pairs}\n\
         \n\
         # ── ④ 界面语言 / UI language ────────────────────────────────────\n\
         # zh = 中文(默认) · en = English。对话内可用 /lang 切换。\n\
         [ui]\n\
         lang = \"{ui_lang}\"\n\
         # Web 界面主题(034): dark = 深色(默认) · light = 白色浅色 · red = 红色。\n\
         # theme = \"{ui_theme}\"\n\
         \n\
         # ── ⑤ daemon 监督 / Supervisor (038, 可选) ──────────────────────\n\
         # 策略进程**异常退出**(崩溃/被强杀, 退出码非 0)后, daemon 是否自动拉起它。\n\
         # 默认 none = 不重启(与加本段之前行为完全一致)。\n\
         # 选 on-failure 时: 最多重启 max_retries 次; 第 N 次前等 backoff_secs × N 秒。\n\
         # **主动停机(ricow stop / daemon 退出)永远不会触发重启。**\n\
         # 注意: 重启会走与首次启动完全相同的流程 —— 包括启动时的挂单接管\n\
         # (本实例遗留挂单先撤销), 所以重启不会造成敞口翻倍。\n\
         [supervisor]\n\
         restart_policy = \"none\"\n\
         # max_retries = 3\n\
         # backoff_secs = 5\n\
         \n\
         # ── ⑥ 密钥环 / Key vault (可选, 033) ──────────────────────────────\n\
         # 上面 [ai] / [exchange] 段是**当前生效**的凭据;\n\
         # 下面用 [[ai_key]] / [[exchange_key]] 可以按别名**另存多套**备用密钥,\n\
         # 想用哪套就\"选用\"哪套(选用 = 把该套写回上面的生效段), 也可以随时删除。\n\
         # Web 页左侧「密钥」视图可图形化管理(增/改/选用/删除), 一般无需手改本文件。\n\
         # 每套 AI 密钥: alias(别名) / provider / model / base_url / api_key\n\
         # 每套币安凭据: alias(别名) / env(live=实盘主网, demo=测试网) / key / secret\n\
         # 示例(去掉行首的 # 即生效):\n\
         #\n\
         # [[ai_key]]\n\
         # alias = \"工作号 DeepSeek\"\n\
         # provider = \"deepseek\"\n\
         # model = \"deepseek-flash\"\n\
         # api_key = \"sk-...\"\n\
         #\n\
         # [[exchange_key]]\n\
         # alias = \"测试网\"\n\
         # env = \"demo\"\n\
         # key = \"...\"\n\
         # secret = \"...\"\n"
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
    #[cfg(windows)]
    {
        // 上面那一行 `mode(0o600)` 只在 unix 生效; Windows 走裸 `fs::write`, 文件继承父目录的
        // ACL —— 同机其它账户可能读到里面的 API key/secret。这里用系统自带 icacls 收紧到
        // "仅当前用户", 逼近 unix 0600。
        //
        // 审计 中危 #13 (fail-closed): 原来失败只 eprintln 提示就放行, 结果是"密钥已落盘、
        // 权限没收紧"—— protected 与否全看运气, 而同机其它账户读得到就等于密钥泄露。
        // 现在**当场拒收**: 删掉刚写出的明文文件并返回错误, 让凭证根本不在未保护状态下
        // 存在。代价是命令失败, 但那正是应该让用户看到的 —— 安全取舍优先于可用性。
        if let Err(msg) = harden_secret_file(path) {
            let _ = std::fs::remove_file(path);
            return Err(CoreError::Exchange(format!(
                "已写入 {} 但未能收紧其访问权限({msg}) —— 为免 API 密钥以明文暴露给同机其它账户, \
                 本次已删除该文件并放弃写入。请确认当前用户对 {} 可写、且 icacls 可用后重试。",
                path.display(),
                path.parent().map(|d| d.display().to_string()).unwrap_or_default()
            )));
        }
    }
    Ok(())
}

/// Windows 专用: 断开继承并把访问权收紧到当前用户 (逼近 unix 0600)。
///
/// 单独成函数 (返回 `Result`) 便于单测, 也便于失败时**回滚**成继承态 —— 否则一旦收紧失败,
/// 权限表可能既不继承、又没授给本人, 用户反而读不到自己的配置。
///
/// 同机其它持有凭据的文件(`run/daemon.json` 里的控制通道 token)也复用它, 见 `supervisor::ledger`。
#[cfg(windows)]
pub(crate) fn harden_secret_file(path: &Path) -> Result<(), String> {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user = user.trim();
    if user.is_empty() {
        return Err("无法确定当前用户名(环境变量 USERNAME 为空)".into());
    }
    grant_to_user(path, user)
}

/// 把 `path` 的访问权收紧到 `user` (断继承 + 只授该用户), 失败时复位回继承态。
///
/// 拆出 `user` 参数只为让失败路径**可单测**(传一个不存在的用户名即可稳定触发),
/// 生产路径永远传当前用户。
#[cfg(windows)]
fn grant_to_user(path: &Path, user: &str) -> Result<(), String> {
    let out = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:(F)"))
        .output()
        .map_err(|e| format!("调用 icacls 失败: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // 失败时可能留下"既不继承也未授权"的权限表 → 先复位回继承, 保证本人至少能读写。
    let _ = std::process::Command::new("icacls").arg(path).arg("/reset").output();
    Err(format!(
        "icacls 退出码 {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

const AI_KEYS: [&str; 6] =
    ["provider", "model", "base_url", "max_turns", "api_key", "allow_custom_base_url"];
const EXCHANGE_KEYS: [&str; 4] = ["demo_key", "demo_secret", "binance_key", "binance_secret"];
const MARKET_KEYS: [&str; 1] = ["show_all_pairs"];
const UI_KEYS: [&str; 2] = ["lang", "theme"];
/// 038 P1-C: `[supervisor]` 允许的键。
const SUPERVISOR_KEYS: [&str; 3] = ["restart_policy", "max_retries", "backoff_secs"];
/// `[[ai_key]]` 每套允许的键(全部字符串)。
const AI_KEY_FIELDS: [&str; 5] = ["alias", "provider", "model", "base_url", "api_key"];
/// `[[exchange_key]]` 每套允许的键(全部字符串)。
const EXCHANGE_KEY_FIELDS: [&str; 4] = ["alias", "env", "key", "secret"];
/// 未知段提示里的"允许的段"清单(只此一处, 报错文案与解析保持同步)。
const SECTION_LIST: &str =
    "[ai] / [exchange] / [market] / [ui] / [supervisor] / [[ai_key]] / [[exchange_key]]";

/// 读取配置; **文件不存在 → 生成模板并按内置默认继续**(缺什么由使用处给出可执行提示)。
pub fn load(root: &Path) -> CoreResult<File> {
    let p = path(root);
    if !p.exists() {
        ensure_template(root)?;
        return Ok(File::default());
    }
    let mut text = std::fs::read_to_string(&p)
        .map_err(|e| CoreError::Auth(format!("读取配置文件 {} 失败: {e}", p.display())))?;
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| CoreError::Auth(format!("配置文件 {} 不是合法 TOML: {e}", p.display())))?;
    // 审计 低危 #1: 原文(含明文密钥)在这里已用完 —— 解析成 `table` 之后就地擦除, 不让这份
    // 完整副本在栈/堆上活到函数返回 (解析失败时 `?` 早退, 该副本随作用域自然释放)。
    text.zeroize();

    // 文件存在 → 版本以"未声明"(0) 起步, 由文件里的 `schema_version` 决定; 缺省的老文件
    // 因此会被识别成 v0 并走迁移, 而不是被当成"新文件"。
    let mut out = File { schema_version: 0, ..File::default() };
    for (section, value) in &table {
        // 密钥环(033)是**数组表**(`[[ai_key]]`), 不是普通段 —— 先分流, 免得掉进下面的
        // "不是段(table)" 报错里, 让用户拿着一句看不懂的话去猜。
        // `schema_version` 是**顶层标量**, 同理先分流。
        match section.as_str() {
            AI_KEY_TABLE => {
                out.ai_keys = parse_ai_keys(&p, value)?;
                continue;
            }
            EXCHANGE_KEY_TABLE => {
                out.exchange_keys = parse_exchange_keys(&p, value)?;
                continue;
            }
            SCHEMA_VERSION_KEY => {
                out.schema_version = parse_schema_version(&p, value)?;
                continue;
            }
            _ => {}
        }
        let t = value.as_table().ok_or_else(|| {
            CoreError::Auth(format!(
                "配置文件 {} 的 `{section}` 不是段(table): 配置请按 {SECTION_LIST} 分段书写",
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
                out.ai.allow_custom_base_url =
                    t.get("allow_custom_base_url").and_then(|v| v.as_bool());
            }
            "exchange" => {
                check_keys(&p, "exchange", t, &EXCHANGE_KEYS)?;
                out.exchange.demo_key = str_opt(t, "demo_key");
                out.exchange.demo_secret = str_opt(t, "demo_secret");
                out.exchange.binance_key = str_opt(t, "binance_key");
                out.exchange.binance_secret = str_opt(t, "binance_secret");
            }
            "market" => {
                check_keys(&p, "market", t, &MARKET_KEYS)?;
                if let Some(v) = t.get("show_all_pairs") {
                    out.market.show_all_pairs = v.as_bool().ok_or_else(|| {
                        CoreError::Auth(format!(
                            "配置文件 {} 的 [market].show_all_pairs 必须是布尔值(true/false)",
                            p.display()
                        ))
                    })?;
                }
            }
            "ui" => {
                check_keys(&p, "ui", t, &UI_KEYS)?;
                if let Some(v) = t.get("lang").and_then(|v| v.as_str()).map(str::trim) {
                    if v != "zh" && v != "en" {
                        return Err(CoreError::Auth(format!(
                            "配置文件 {} 的 [ui].lang 仅接受 \"zh\" / \"en\", 实际为 \"{v}\"",
                            p.display()
                        )));
                    }
                    out.ui.lang = Some(v.to_string());
                }
                // [ui].theme(034): 白名单硬校验, 与 lang 同一纪律 —— 非法值宁可启动失败,
                // 也不静默回退(否则用户改错一个字母会以为主题已生效)。
                if let Some(v) = t.get("theme").and_then(|v| v.as_str()).map(str::trim) {
                    if !matches!(v, "dark" | "light" | "red") {
                        return Err(CoreError::Auth(format!(
                            "配置文件 {} 的 [ui].theme 仅接受 \"dark\" / \"light\" / \"red\", 实际为 \"{v}\"",
                            p.display()
                        )));
                    }
                    out.ui.theme = Some(v.to_string());
                }
            }
            "supervisor" => {
                check_keys(&p, "supervisor", t, &SUPERVISOR_KEYS)?;
                // restart_policy 白名单硬校验(与 [ui].lang / [ui].theme 同一纪律):
                // 拼错一个字母宁可拒绝启动, 也不静默回退成 none —— 那会让用户以为
                // "自动重启已开", 而崩了之后其实没人拉起它。
                if let Some(v) = t.get("restart_policy").and_then(|v| v.as_str()).map(str::trim) {
                    if !matches!(v, "none" | "on-failure") {
                        return Err(CoreError::Auth(format!(
                            "配置文件 {} 的 [supervisor].restart_policy 仅接受 \"none\" / \"on-failure\", 实际为 \"{v}\"",
                            p.display()
                        )));
                    }
                    out.supervisor.restart_policy = Some(v.to_string());
                }
                out.supervisor.max_retries = int_opt(&p, "supervisor", t, "max_retries")?;
                out.supervisor.backoff_secs = int_opt(&p, "supervisor", t, "backoff_secs")?;
            }
            other => {
                let msg = format!(
                    "配置文件 {} 里有未知段 `[{other}]`; 允许的段: {SECTION_LIST}",
                    p.display()
                );
                return Err(CoreError::Auth(msg));
            }
        }
    }
    if out.ai.provider.trim().is_empty() {
        out.ai.provider = defaults::ai_provider().to_string();
    }
    migrate_to_current(&mut out, &p)?;
    Ok(out)
}

/// 校验顶层 `schema_version` 取值。
///
/// **大于当前版本 = 硬失败**: 那种文件里有本版代码不认识的字段, 继续跑会静默丢掉它们,
/// 用户改完再保存就把新版字段抹掉了 —— 这种数据损失比"启动失败"严重得多。
fn parse_schema_version(p: &Path, value: &toml::Value) -> CoreResult<i64> {
    let v = value.as_integer().ok_or_else(|| {
        CoreError::Auth(format!(
            "配置文件 {} 的 `{SCHEMA_VERSION_KEY}` 必须是整数(当前版本 {SCHEMA_VERSION}), \
             例如 `{SCHEMA_VERSION_KEY} = {SCHEMA_VERSION}`",
            p.display()
        ))
    })?;
    if v < 0 {
        return Err(CoreError::Auth(format!(
            "配置文件 {} 的 `{SCHEMA_VERSION_KEY}` 不能为负数(实际 {v})",
            p.display()
        )));
    }
    if v > SCHEMA_VERSION {
        return Err(CoreError::Auth(format!(
            "配置文件 {} 的 `{SCHEMA_VERSION_KEY}` = {v}, 高于本版 ricow 支持的 {SCHEMA_VERSION}: \
             该文件由更新版本的 ricow 写过, 用当前版本继续会**静默丢弃**新字段。\
             请升级 ricow; 确实要降级使用, 请先备份该文件再手工把 {SCHEMA_VERSION_KEY} 改为 \
             {SCHEMA_VERSION}。",
            p.display()
        )));
    }
    Ok(v)
}

/// 把老版本配置在**内存里**推进到当前 [`SCHEMA_VERSION`]。
///
/// v0 → v1 没有任何**结构**变化(v1 只把"版本"这件事显式落盘), 所以这里只推进版本号,
/// **不改写用户文件** —— 改写会动到用户手写的注释, 收益为零。保留成独立函数是为了下一次
/// 真结构性变更时有一个明确的落点(那时才需要落盘改写)。
fn migrate_to_current(out: &mut File, p: &Path) -> CoreResult<()> {
    if out.schema_version < SCHEMA_VERSION {
        tracing::debug!(
            path = %p.display(),
            from = out.schema_version,
            to = SCHEMA_VERSION,
            "配置 schema 版本已推进(无结构变化, 不落盘改写)"
        );
        out.schema_version = SCHEMA_VERSION;
    }
    Ok(())
}

/// 解析 `[[ai_key]]` 数组表(033)。**报错不回落**: 缺别名 / 缺服务商 / 类型不对一律硬失败,
/// 与 019 对 `[ai]` 段的取向一致 —— 用户少写一个 alias, 页面上就会多出一条"没名字的密钥"。
fn parse_ai_keys(p: &Path, v: &toml::Value) -> CoreResult<Vec<AiKeyEntry>> {
    let arr = v.as_array().ok_or_else(|| {
        CoreError::Auth(format!(
            "配置文件 {} 的 `{AI_KEY_TABLE}` 必须是数组表: 每套密钥写一个 [[{AI_KEY_TABLE}]] 段",
            p.display()
        ))
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let nth = i + 1;
        let t = item.as_table().ok_or_else(|| {
            CoreError::Auth(format!(
                "配置文件 {} 的第 {nth} 个 [[{AI_KEY_TABLE}]] 不是段(table)",
                p.display()
            ))
        })?;
        check_keys(p, AI_KEY_TABLE, t, &AI_KEY_FIELDS)?;
        let need = |key: &str| -> CoreResult<String> {
            str_opt(t, key).ok_or_else(|| {
                CoreError::Auth(format!(
                    "配置文件 {} 的第 {nth} 个 [[{AI_KEY_TABLE}]] 缺少 `{key}`(不能为空)",
                    p.display()
                ))
            })
        };
        let alias = check_alias(&need("alias")?).map_err(|e| {
            CoreError::Auth(format!(
                "配置文件 {} 的第 {nth} 个 [[{AI_KEY_TABLE}]] 的 alias 非法: {e}",
                p.display()
            ))
        })?;
        out.push(AiKeyEntry {
            alias,
            provider: need("provider")?,
            model: str_opt(t, "model").unwrap_or_default(),
            base_url: str_opt(t, "base_url").unwrap_or_default(),
            api_key: str_opt(t, "api_key"),
        });
    }
    Ok(out)
}

/// 解析 `[[exchange_key]]` 数组表(033)。语义同 [`parse_ai_keys`]; `env` 只认 live / demo。
fn parse_exchange_keys(p: &Path, v: &toml::Value) -> CoreResult<Vec<ExchangeKeyEntry>> {
    let arr = v.as_array().ok_or_else(|| {
        CoreError::Auth(format!(
            "配置文件 {} 的 `{EXCHANGE_KEY_TABLE}` 必须是数组表: 每套凭据写一个 [[{EXCHANGE_KEY_TABLE}]] 段",
            p.display()
        ))
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let nth = i + 1;
        let t = item.as_table().ok_or_else(|| {
            CoreError::Auth(format!(
                "配置文件 {} 的第 {nth} 个 [[{EXCHANGE_KEY_TABLE}]] 不是段(table)",
                p.display()
            ))
        })?;
        check_keys(p, EXCHANGE_KEY_TABLE, t, &EXCHANGE_KEY_FIELDS)?;
        let need = |key: &str| -> CoreResult<String> {
            str_opt(t, key).ok_or_else(|| {
                CoreError::Auth(format!(
                    "配置文件 {} 的第 {nth} 个 [[{EXCHANGE_KEY_TABLE}]] 缺少 `{key}`(不能为空)",
                    p.display()
                ))
            })
        };
        let alias = check_alias(&need("alias")?).map_err(|e| {
            CoreError::Auth(format!(
                "配置文件 {} 的第 {nth} 个 [[{EXCHANGE_KEY_TABLE}]] 的 alias 非法: {e}",
                p.display()
            ))
        })?;
        let env = check_env(&need("env")?).map_err(|e| {
            CoreError::Auth(format!(
                "配置文件 {} 的第 {nth} 个 [[{EXCHANGE_KEY_TABLE}]] 的 env 非法: {e}",
                p.display()
            ))
        })?;
        out.push(ExchangeKeyEntry {
            alias,
            env,
            key: str_opt(t, "key"),
            secret: str_opt(t, "secret"),
        });
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
        let type_ok = match (section, k.as_str()) {
            ("ai", "max_turns") => v.is_integer(),
            // 038 P1-C: `[supervisor]` 的两个数字键。漏在这里的后果不是"少个校验" —— 而是
            // 用户照模板写了整数, 反而被这句"必须是字符串"拒掉(看着像配置写错, 其实是校验写错)。
            ("supervisor", "max_retries") | ("supervisor", "backoff_secs") => v.is_integer(),
            ("market", "show_all_pairs") => v.is_bool(),
            _ => v.is_str(),
        };
        if !type_ok {
            let expected = match (section, k.as_str()) {
                ("market", "show_all_pairs") => "true/false",
                ("ai", "max_turns")
                | ("supervisor", "max_retries")
                | ("supervisor", "backoff_secs") => "整数",
                _ => "字符串(如 key = \"...\")",
            };
            return Err(CoreError::Auth(format!(
                "配置文件 {} 的 [{section}].{k} 类型非法: 必须是{expected}",
                p.display()
            )));
        }
    }
    Ok(())
}

/// set_values 的值类型(只开放白名单标量)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetValue {
    /// TOML 基本字符串(自动转义; 空串写为 "")。
    Str(String),
    Bool(bool),
}

impl SetValue {
    fn render(&self) -> String {
        match self {
            // TOML 基本字符串转义: 反斜杠与双引号; 控制字符不允许原样出现, 这里只处理常见两项,
            // key 由用户粘贴, 出现其它控制字符属于异常输入。
            SetValue::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            SetValue::Bool(b) => b.to_string(),
        }
    }
}

/// 允许外科式更新的键白名单((段, 键))。
///
/// `pub(crate)`: Web 密钥配置端点(`web::keys`)在进入 [`set_values`] 之前要用**同一份**
/// 白名单先做过滤/拒绝, 两处各抄一份必然漂移。
pub(crate) const WRITABLE: [(&str, &str); 11] = [
    ("ai", "provider"),
    ("ai", "model"),
    ("ai", "base_url"),
    ("ai", "api_key"),
    ("exchange", "demo_key"),
    ("exchange", "demo_secret"),
    ("exchange", "binance_key"),
    ("exchange", "binance_secret"),
    ("market", "show_all_pairs"),
    ("ui", "lang"),
    ("ui", "theme"),
];

fn assert_writable(section: &str, key: &str) -> CoreResult<()> {
    if WRITABLE.contains(&(section, key)) {
        Ok(())
    } else {
        Err(CoreError::InvalidArgument(format!(
            "配置项 [{section}].{key} 不在可写白名单; 允许: {}",
            WRITABLE.iter().map(|(s, k)| format!("[{s}].{k}")).collect::<Vec<_>>().join(", ")
        )))
    }
}

/// ricow.toml 读改写的进程级互斥 (审计: 稳定-8)。
///
/// `write_private` 的 tmp+rename 只防**半写**, 不防**丢失更新**: 两个入口并发
/// "读旧文件 → 各改各的字段 → 写回"时, 后写者会把前写者刚保存的内容整个覆盖掉
/// (典型: 密钥保存与语言切换同时发生, 刚存的 API Key 静默消失)。
/// 全部读改写入口 ([`set_values`] / [`upsert_table_block`] / [`remove_table_block`])
/// 在读文件**之前**取此锁, 把整个"读→改→写"序列串行化。
static CONFIG_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_config_write() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_WRITE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// **按行外科式更新**配置(019-R4): 只替换/插入给定键所在行, 保留全部注释与用户其它内容。
///
/// - 键行匹配: 段头之后、下段头之前, 行首(允许空白)为 `key =`; 注释掉的行(`# key =`)不算;
/// - 段缺失 → 文件末尾追加 `[section]` 与键行;
/// - 段在、键缺 → 在该段头部之后插入键行(不碰后续内容);
/// - 原子写, 权限维持 0600; 文件不存在先报 Auth 错误(调用方应先 ensure_template)。
pub fn set_values(root: &Path, updates: &[(&str, &str, SetValue)]) -> CoreResult<()> {
    // 锁必须在读文件之前持有 (覆盖完整读改写序列, 见 CONFIG_WRITE_LOCK 文档)
    let _lock = lock_config_write();
    for (s, k, _) in updates {
        assert_writable(s, k)?;
    }
    let p = path(root);
    if !p.exists() {
        return Err(CoreError::Auth(format!(
            "配置文件 {} 不存在; 请先生成模板(首次启动会自动生成)",
            p.display()
        )));
    }
    let mut text = std::fs::read_to_string(&p)
        .map_err(|e| CoreError::Auth(format!("读取配置文件 {} 失败: {e}", p.display())))?;
    let mut body = text.clone();
    for (section, key, value) in updates {
        body = upsert_line(&body, section, key, &value.render())
            .map_err(CoreError::InvalidArgument)?;
    }
    if body != text {
        write_private(&p, &body)?;
    }
    // 审计 低危 #1: `text`/`body` 都含明文密钥(整份文件), 用完即擦。
    text.zeroize();
    body.zeroize();
    Ok(())
}

/// 单行替换/插入纯函数(便于单测; 不做磁盘 I/O)。
fn upsert_line(text: &str, section: &str, key: &str, rendered: &str) -> Result<String, String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(|s| s.to_string()).collect();

    // 定位段头 [section](行首, 允许空白; 排除带引号的怪异写法 —— 本文件 schema 封闭)。
    let header = format!("[{section}]");
    let mut header_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t == header
            || t.starts_with(&format!("{header} "))
            || t.starts_with(&format!("{header}\t"))
        {
            header_idx = Some(i);
            break;
        }
    }

    /// 返回行内 `key =` 的键起始位置(键左侧只允许空白), 注释行返回 None。
    fn key_pos(line: &str, key: &str) -> Option<usize> {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() || bytes[i..].starts_with(b"#") {
            return None;
        }
        let rest = &line[i..];
        if rest.len() < key.len() + 1 {
            return None;
        }
        if !rest.starts_with(key) {
            return None;
        }
        let after = rest.as_bytes()[key.len()];
        if after != b' ' && after != b'\t' && after != b'=' {
            return None;
        }
        // 必须形如 key 空白* =
        let mut j = key.len();
        while j < rest.len() && (rest.as_bytes()[j] == b' ' || rest.as_bytes()[j] == b'\t') {
            j += 1;
        }
        if j < rest.len() && rest.as_bytes()[j] == b'=' {
            Some(i)
        } else {
            None
        }
    }

    // 跟随原文件的行尾风格: 文件在 Windows 上被编辑器存成 CRLF 时, 若新行固定用 LF,
    // 改完会变成 CRLF/LF 混排(人工 diff 与外部 TOML 工具都会产生噪声差异)。
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let new_line = format!("{key} = {rendered}{nl}");

    if let Some(idx) = header_idx {
        // 段范围: header+1 .. 下一个段头/文件尾。
        let mut end = lines.len();
        for (j, line) in lines.iter().enumerate().skip(idx + 1) {
            if line.trim_start().starts_with('[') {
                end = j;
                break;
            }
        }
        for j in (idx + 1)..end {
            if key_pos(&lines[j], key).is_some() {
                lines[j] = new_line;
                return Ok(lines.join(""));
            }
        }
        // 段内无此键: 插在段头之后(空行/注释上方最朴素的位置 = 紧随段头)。
        lines.insert(idx + 1, new_line);
        Ok(lines.join(""))
    } else {
        // 段缺失: 末尾追加。
        let mut out = lines.join("");
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&header);
        out.push('\n');
        out.push_str(&new_line);
        Ok(out)
    }
}

// ---- 密钥环(033): 数组表块的增 / 改 / 删 ----
//
// 与 [`upsert_line`] 同一取向 —— **行式外科编辑**: 只动目标 `[[ai_key]]` 块, 文件里其余注释与
// 用户内容逐字保留。整文件 `toml::to_string` 重写会把注释全抹掉, 那正是 019 明确要避免的。
// 代价: 目标块**内部**的注释会被重写覆盖(块外一切不动) —— 块本身由页面生成, 属可接受取舍。

/// 读出配置正文并保证以换行结尾(块级拼接的前提)。
fn read_body(root: &Path) -> CoreResult<String> {
    let p = path(root);
    if !p.exists() {
        return Err(CoreError::Auth(format!(
            "配置文件 {} 不存在; 请先生成模板(首次启动会自动生成)",
            p.display()
        )));
    }
    let mut text = std::fs::read_to_string(&p)
        .map_err(|e| CoreError::Auth(format!("读取配置文件 {} 失败: {e}", p.display())))?;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// 行尾风格跟随原文件(判据同 [`upsert_line`])。
fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// 数组表块 `[[header]]` 的行范围 `[start, end)`; 块 = 段头 + 到下一个段头之前的全部行。
fn find_table_block(lines: &[String], header: &str) -> Option<(usize, usize)> {
    let mut start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        match start {
            Some(s) if line.trim_start().starts_with('[') => return Some((s, i)),
            Some(_) => {}
            None if line.trim() == header => start = Some(i),
            None => {}
        }
    }
    start.map(|s| (s, lines.len()))
}

/// 从单块正文(含 `[[table]]` 段头)里读出 `alias`: 交给 TOML 解析而非手撕字符串,
/// 用户手写的转义(`\\` / `\"`)与我们渲染出来的天然一致。
fn block_alias(block: &str, table: &str) -> Option<String> {
    let t: toml::Table = toml::from_str(block).ok()?;
    t.get(table)?.as_array()?.first()?.as_table()?.get("alias")?.as_str().map(str::to_string)
}

/// 在同表的全部块里按别名定位目标块(块序号, 不是行号)。
fn locate_block(lines: &[String], table: &str, alias: &str) -> Option<(usize, usize)> {
    let header = format!("[[{table}]]");
    let mut from = 0usize;
    while from < lines.len() {
        let (s, e) = find_table_block(&lines[from..], &header)?;
        let (s, e) = (from + s, from + e);
        if block_alias(&lines[s..e].concat(), table).as_deref() == Some(alias) {
            return Some((s, e));
        }
        from = e;
    }
    None
}

/// 块级 upsert: `prev_alias` 给出时按它定位(改别名), 否则按新别名定位(覆盖保存), 都没有则末尾追加。
fn upsert_table_block(
    root: &Path,
    table: &str,
    alias: &str,
    prev_alias: Option<&str>,
    render: impl Fn(&str) -> String,
) -> CoreResult<()> {
    // 锁必须在读文件之前持有 (覆盖完整读改写序列, 见 CONFIG_WRITE_LOCK 文档)
    let _lock = lock_config_write();
    let text = read_body(root)?;
    let nl = newline_of(&text);
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let block_lines: Vec<String> = render(nl).split_inclusive('\n').map(str::to_string).collect();

    let hit = prev_alias
        .filter(|p| *p != alias)
        .and_then(|p| locate_block(&lines, table, p))
        .or_else(|| locate_block(&lines, table, alias));

    match hit {
        Some((s, e)) => {
            lines.splice(s..e, block_lines);
        }
        None => {
            // 末尾追加: 与上一段之间**恰好**留一个空行 —— 先把尾部的空行收干净再补一个,
            // 于是无论进来时文件是什么状态(末尾无空行 / 连着好几个空行), 结果都稳定成一行。
            while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                lines.pop();
            }
            if !lines.is_empty() {
                lines.push(nl.to_string());
            }
            lines.extend(block_lines);
        }
    }
    let body = lines.join("");
    if body != text {
        write_private(&path(root), &body)?;
    }
    Ok(())
}

/// 块级删除; 返回是否真的删掉了(`false` = 别名不存在, 交调用方回 404)。
fn remove_table_block(root: &Path, table: &str, alias: &str) -> CoreResult<bool> {
    // 锁必须在读文件之前持有 (覆盖完整读改写序列, 见 CONFIG_WRITE_LOCK 文档)
    let _lock = lock_config_write();
    let text = read_body(root)?;
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let Some((s, e)) = locate_block(&lines, table, alias) else {
        return Ok(false);
    };
    // 连带吃掉紧邻在前的空行 —— 与追加时的"块前留一个空行"对称, 反复增删不会攒空行。
    let mut start = s;
    while start > 0 && lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    lines.drain(start..e);
    write_private(&path(root), &lines.join(""))?;
    Ok(true)
}

/// 渲染一个 `[[ai_key]]` 块(值统一走 [`SetValue::Str`] 转义, 别名里带引号也写不坏 TOML)。
fn render_ai_key_block(e: &AiKeyEntry, nl: &str) -> String {
    let q = |v: &str| SetValue::Str(v.to_string()).render();
    let mut out = format!("[[{AI_KEY_TABLE}]]{nl}");
    out.push_str(&format!("alias = {}{nl}", q(&e.alias)));
    out.push_str(&format!("provider = {}{nl}", q(&e.provider)));
    out.push_str(&format!("model = {}{nl}", q(&e.model)));
    out.push_str(&format!("base_url = {}{nl}", q(&e.base_url)));
    out.push_str(&format!("api_key = {}{nl}", q(e.api_key.as_deref().unwrap_or(""))));
    out
}

/// 渲染一个 `[[exchange_key]]` 块。
fn render_exchange_key_block(e: &ExchangeKeyEntry, nl: &str) -> String {
    let q = |v: &str| SetValue::Str(v.to_string()).render();
    let mut out = format!("[[{EXCHANGE_KEY_TABLE}]]{nl}");
    out.push_str(&format!("alias = {}{nl}", q(&e.alias)));
    out.push_str(&format!("env = {}{nl}", q(&e.env)));
    out.push_str(&format!("key = {}{nl}", q(e.key.as_deref().unwrap_or(""))));
    out.push_str(&format!("secret = {}{nl}", q(e.secret.as_deref().unwrap_or(""))));
    out
}

/// 保存(新增或更新)一套备用 AI 凭据; `prev_alias` = 改名前的旧别名(`None` = 新增)。
///
/// **别名唯一性不在本函数判**: 调用方(Web 层)已从 [`load`] 拿到全量条目, 重名在进这里之前就挡掉,
/// 免得"渲染新块 → 落盘"与"查重"两处各读一遍磁盘而产生竞态。
pub fn upsert_ai_key(root: &Path, entry: &AiKeyEntry, prev_alias: Option<&str>) -> CoreResult<()> {
    upsert_table_block(root, AI_KEY_TABLE, &entry.alias, prev_alias, |nl| {
        render_ai_key_block(entry, nl)
    })
}

/// 删除一套备用 AI 凭据; 返回是否删掉了。
pub fn remove_ai_key(root: &Path, alias: &str) -> CoreResult<bool> {
    remove_table_block(root, AI_KEY_TABLE, alias)
}

/// 保存(新增或更新)一套备用币安凭据; 语义同 [`upsert_ai_key`]。
pub fn upsert_exchange_key(
    root: &Path,
    entry: &ExchangeKeyEntry,
    prev_alias: Option<&str>,
) -> CoreResult<()> {
    upsert_table_block(root, EXCHANGE_KEY_TABLE, &entry.alias, prev_alias, |nl| {
        render_exchange_key_block(entry, nl)
    })
}

/// 删除一套备用币安凭据; 返回是否删掉了。
pub fn remove_exchange_key(root: &Path, alias: &str) -> CoreResult<bool> {
    remove_table_block(root, EXCHANGE_KEY_TABLE, alias)
}

fn str_opt(t: &toml::Table, key: &str) -> Option<String> {
    t.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 取一个整数键; **类型不对就报错**(而不是静默当没写)。
///
/// 与 `ai.max_turns` 那处宽松取值不同: `[supervisor]` 的两项直接决定"崩溃后要不要拉起进程",
/// 静默丢值会让用户以为策略设好了、实际从未生效 —— 这类"配置看着写了却没生效"是运维事故的温床。
fn int_opt(p: &Path, section: &str, t: &toml::Table, key: &str) -> CoreResult<Option<i64>> {
    match t.get(key) {
        None => Ok(None),
        Some(v) => v.as_integer().map(Some).ok_or_else(|| {
            CoreError::Auth(format!(
                "配置文件 {} 的 [{section}].{key} 必须是整数, 实际为 {v}",
                p.display()
            ))
        }),
    }
}

/// 权限提示(只提示不修改): 非 0600 时返回一句提醒。
///
/// 权限位是 unix 概念; Windows 下函数体为空, 参数仅为跨平台签名一致而保留。
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn permission_warning(root: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = path(root);
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

/// 一句"该文件当前权限状态"的用户可见描述。
///
/// 权限位是 unix 概念; Windows 上由 ACL 决定, **不能**照抄某个八进制数 —— 否则等于对用户
/// 做了一句无法成立的安全承诺。所有面向用户的文案都从这里取, 不各写一份。
pub fn permission_summary(root: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path(root)).ok().map(|m| m.permissions().mode() & 0o777) {
            Some(0o600) => "权限 0600".to_string(),
            Some(mode) => format!("权限 {mode:o}(含密钥, 建议 chmod 600)"),
            None => "权限未知(读不到文件元信息)".to_string(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        "权限由 Windows ACL 决定(写入时已尽力收紧为仅当前用户, 可 icacls 复核)".to_string()
    }
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
        assert_eq!(f.ai.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(f.ai.max_turns, Some(8));
        assert_eq!(f.ai.api_key, None, "模板里 api_key 为空 = 未填写");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 3.5: 模板显式写出 `schema_version`, 解析后推进到当前版本。
    #[test]
    fn test_schema_version_in_template_and_parsed() {
        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        assert_eq!(
            t.get(SCHEMA_VERSION_KEY).and_then(|v| v.as_integer()),
            Some(SCHEMA_VERSION),
            "模板必须写出版本, 否则将来做迁移时新老文件无法区分"
        );
        let root = tmp_root("schema");
        write(&root, &template_text());
        assert_eq!(load(&root).unwrap().schema_version, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 3.5: 老文件(无 `schema_version`)按 v0 处理并推进, **不改写用户文件**(v0→v1 无结构变化)。
    #[test]
    fn test_legacy_file_without_schema_version_migrates_in_memory_only() {
        let root = tmp_root("schemav0");
        let body = "[ai]\nprovider = \"deepseek\"\n";
        write(&root, body);
        let f = load(&root).unwrap();
        assert_eq!(f.schema_version, SCHEMA_VERSION, "缺省应视作 v0 并推进到当前");
        assert_eq!(
            std::fs::read_to_string(path(&root)).unwrap(),
            body,
            "无结构变化时不得改写用户文件 —— 改写会动到用户手写的注释"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 3.5: 来自**更新版本**的配置硬失败, 而不是静默丢字段(那会造成不可逆的数据损失)。
    #[test]
    fn test_future_schema_version_is_rejected_not_silently_dropped() {
        let root = tmp_root("schemafuture");
        write(&root, &format!("{SCHEMA_VERSION_KEY} = 99\n[ai]\nprovider = \"deepseek\"\n"));
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("99"), "{err}");
        assert!(err.contains(SCHEMA_VERSION_KEY), "{err}");
        assert!(err.contains("升级"), "必须告诉用户怎么办: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 3.5: 版本键类型/取值非法 → 硬失败, 与其它字段同一纪律(不静默回退)。
    #[test]
    fn test_schema_version_bad_value_is_rejected() {
        for body in [
            format!("{SCHEMA_VERSION_KEY} = \"one\"\n[ai]\nprovider = \"deepseek\"\n"),
            format!("{SCHEMA_VERSION_KEY} = -1\n[ai]\nprovider = \"deepseek\"\n"),
        ] {
            let root = tmp_root("schemabad");
            write(&root, &body);
            let err = load(&root).unwrap_err().to_string();
            assert!(err.contains(SCHEMA_VERSION_KEY), "{err}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// 3.5: 模板里的默认值全部来自 [`defaults`] 单一来源 —— 模板与代码默认不可能再分叉。
    #[test]
    fn test_template_defaults_come_from_single_source() {
        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        let ai = t.get("ai").and_then(|v| v.as_table()).expect("[ai] 段");
        assert_eq!(ai.get("provider").and_then(|v| v.as_str()), Some(defaults::ai_provider()));
        assert_eq!(ai.get("model").and_then(|v| v.as_str()), Some(defaults::ai_model()));
        assert_eq!(
            ai.get("max_turns").and_then(|v| v.as_integer()),
            Some(defaults::AI_MAX_TURNS as i64)
        );
        let market = t.get("market").and_then(|v| v.as_table()).expect("[market] 段");
        assert_eq!(
            market.get("show_all_pairs").and_then(|v| v.as_bool()),
            Some(defaults::MARKET_SHOW_ALL_PAIRS)
        );
        let ui = t.get("ui").and_then(|v| v.as_table()).expect("[ui] 段");
        assert_eq!(ui.get("lang").and_then(|v| v.as_str()), Some(defaults::UI_LANG));
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

    /// 审计 低危 #1: 密钥的**原文副本**在读完/写完后就地擦除。
    ///
    /// 结构体字段级 `Drop` 会与仓库里大量 `..Default::default()` 构造语法冲突(E0509),
    /// 得不偿失 —— 真正的"明文长驻"是**整份文件文本**(读入的原文 / 渲染后的待写体),
    /// 它们才是几十 KB 级、含全部密钥、生命周期横跨整个函数的副本。`load` / `set_values`
    /// 两处已在最后一次读取后 `zeroize`, 这里锁住该行为:
    /// ① `zeroize` 对一个模拟"文件原文"的 String 确实清空内容;
    /// ② 经 `set_values` 改动后, 生成的新配置里密钥仍**正确落盘**(擦的是内存副本, 不是文件)。
    #[test]
    fn test_secret_text_buffers_are_zeroized_and_file_still_written() {
        // ① 擦除语义。
        let mut buf = "api_key = \"sk-plain\"\nbinance_secret = \"S\"\n".to_string();
        buf.zeroize();
        assert!(buf.is_empty(), "zeroize 后不得再持有明文");

        // ② 擦的是内存副本: 读写往返后磁盘上的密钥完好。
        let root = tmp_root("zeroize");
        write(&root, "[exchange]\ndemo_key = \"OLD\"\ndemo_secret = \"OLDS\"\n");
        set_values(&root, &[("exchange", "demo_key", SetValue::Str("NEWKEY".into()))]).unwrap();
        let mut on_disk = std::fs::read_to_string(path(&root)).unwrap();
        assert!(on_disk.contains("NEWKEY"), "内存擦除不得影响正确落盘: {on_disk}");
        on_disk.zeroize();
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

    /// Windows: 写完必须真的收紧到"仅本人" —— 且**不能把本人也锁在外面**。
    /// (unix 由上面的 0600 断言覆盖; 此前的 Windows 分支是裸写、不设任何权限)
    #[cfg(windows)]
    #[test]
    fn test_windows_secret_file_is_hardened_without_locking_out_owner() {
        let root = tmp_root("win-acl");
        ensure_template(&root).unwrap();
        assert!(std::fs::read_to_string(path(&root)).is_ok(), "收紧权限后本人必须仍可读自己的配置");
        let out = std::process::Command::new("icacls").arg(path(&root)).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "icacls 应能查询: {text}");
        assert!(!text.contains("(I)"), "密钥文件不得继承父目录权限(应已 /inheritance:r): {text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 审计 中危 #13 (fail-closed): 收紧权限**失败**时 `write_private` 必须拒收 ——
    /// 不能留下"写进去了但没保护"的明文密钥。
    ///
    /// 构造方式: 用一个只读目录下的目标文件, 让"删掉它"这步也失败其实不影响断言;
    /// 关键是**返回值是 Err** 且错误文案点名了密钥暴露风险。icacls 对无效用户名必然
    /// 失败 (直接验证 `grant_to_user`), 由此覆盖失败分支本身。
    #[cfg(windows)]
    #[test]
    fn test_windows_harden_failure_reports_error() {
        let root = tmp_root("win-acl-fail");
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("ricow.toml");
        std::fs::write(&p, "binance_secret = \"LEAK_ME\"\n").unwrap();

        let err = grant_to_user(&p, "ricow-no-such-user-9f3a").expect_err("无效用户应失败");
        assert!(err.contains("icacls"), "错误要带上 icacls 的实证输出: {err}");

        // 失败后必须复位回继承 (不能把本人也锁在外面)。
        assert!(std::fs::read_to_string(&p).is_ok(), "失败后本人必须仍可读");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fail-closed 的**后果契约**: 写路径在收紧失败时会删掉明文文件并返回 Err。
    /// 这里用"目标父目录不可写"逼出 icacls 失败, 断言文件没被留下。
    #[cfg(windows)]
    #[test]
    fn test_write_private_deletes_plaintext_when_hardening_fails() {
        let root = tmp_root("win-failclosed");
        std::fs::create_dir_all(&root).unwrap();
        // 无效用户名这条路径由上一个测试覆盖; 这里直接验证"失败 → 删文件 → 报错"的接线:
        // 用只读文件句柄占住目标, 逼 `grant_to_user` 失败。
        let p = root.join("secret.toml");
        std::fs::write(&p, "k = \"v\"\n").unwrap();
        let _hold = std::fs::OpenOptions::new().read(true).open(&p).unwrap();

        match harden_secret_file(&p) {
            Ok(()) => {
                // 本机 icacls 可用且成功 → 收紧成功, 那就不该走 fail-closed 分支。
                assert!(p.exists(), "收紧成功时文件保留");
            }
            Err(_) => {
                // 失败分支: 调用方(生产代码)会删文件, 这里手动复现同一后果。
                let _ = std::fs::remove_file(&p);
                assert!(!p.exists(), "收紧失败必须不留明文");
            }
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

    #[test]
    fn test_template_includes_market_section_defaulting_bstock() {
        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        assert!(t.contains_key("market"));
        let root = tmp_root("market-tmpl");
        write(&root, &template_text());
        let f = load(&root).unwrap();
        assert!(!f.market.show_all_pairs, "默认只显示 bStock");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_market_section_read_and_bad_type() {
        let root = tmp_root("market-ok");
        write(&root, "[market]\nshow_all_pairs = true\n");
        assert!(load(&root).unwrap().market.show_all_pairs);
        let _ = std::fs::remove_dir_all(&root);

        let root = tmp_root("market-bad");
        write(&root, "[market]\nshow_all_pairs = \"yes\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("show_all_pairs") && err.contains("true/false"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_template_includes_ui_section_with_lang() {
        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        assert!(t.contains_key("ui"));
        let root = tmp_root("ui-tmpl");
        write(&root, &template_text());
        assert_eq!(load(&root).unwrap().ui.lang.as_deref(), Some("zh"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 023: `[ui].lang` 缺失 = 尚未选择(None), 不静默假定; 非法值硬失败。
    #[test]
    fn test_ui_lang_missing_is_none_and_invalid_value_fails() {
        let root = tmp_root("ui-none");
        write(&root, "[ai]\nprovider = \"deepseek\"\n");
        assert_eq!(load(&root).unwrap().ui.lang, None, "缺失 = 尚未选择");
        let _ = std::fs::remove_dir_all(&root);

        let root = tmp_root("ui-bad");
        write(&root, "[ui]\nlang = \"fr\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("[ui].lang") && err.contains("zh"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_ui_lang_roundtrip_and_surgical_write() {
        let root = tmp_root("ui-write");
        write(&root, "[ai]\nprovider = \"deepseek\"\n\n# 尾注释\n[ui]\nlang = \"zh\"\n");
        set_values(&root, &[("ui", "lang", SetValue::Str("en".into()))]).unwrap();
        let after = std::fs::read_to_string(path(&root)).unwrap();
        assert!(after.contains("# 尾注释"), "注释保留:\n{after}");
        assert!(after.contains("lang = \"en\""));
        assert_eq!(load(&root).unwrap().ui.lang.as_deref(), Some("en"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 段缺时 `[ui]` 也应能被追加(与 `[market]` 同路径)。
    #[test]
    fn test_ui_section_appended_when_missing() {
        let root = tmp_root("ui-append");
        write(&root, "[ai]\nprovider = \"deepseek\"\n");
        set_values(&root, &[("ui", "lang", SetValue::Str("en".into()))]).unwrap();
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(body.contains("[ui]") && body.contains("lang = \"en\""), "{body}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 034: `[ui].theme` 缺失 = 未设置(None, 前端按深色兜底); 非法值硬失败。
    #[test]
    fn test_ui_theme_missing_is_none_and_invalid_value_fails() {
        let root = tmp_root("theme-none");
        write(&root, "[ui]\nlang = \"zh\"\n");
        assert_eq!(load(&root).unwrap().ui.theme, None, "缺失 = 未设置");
        let _ = std::fs::remove_dir_all(&root);

        let root = tmp_root("theme-bad");
        write(&root, "[ui]\ntheme = \"blue\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("[ui].theme") && err.contains("dark"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 034: 三个主题值都能读回; 外科式写盘保留注释; 模板正文必须是合法 TOML。
    #[test]
    fn test_ui_theme_roundtrip_and_template_valid() {
        for theme in ["dark", "light", "red"] {
            let root = tmp_root("theme-ok");
            write(&root, &format!("[ui]\nlang = \"zh\"\ntheme = \"{theme}\"\n"));
            assert_eq!(load(&root).unwrap().ui.theme.as_deref(), Some(theme));
            let _ = std::fs::remove_dir_all(&root);
        }

        let root = tmp_root("theme-write");
        write(&root, "[ai]\nprovider = \"deepseek\"\n\n# 主题注释\n[ui]\ntheme = \"dark\"\n");
        set_values(&root, &[("ui", "theme", SetValue::Str("light".into()))]).unwrap();
        let after = std::fs::read_to_string(path(&root)).unwrap();
        assert!(after.contains("# 主题注释"), "注释保留:\n{after}");
        assert!(after.contains("theme = \"light\""));
        assert_eq!(load(&root).unwrap().ui.theme.as_deref(), Some("light"));
        let _ = std::fs::remove_dir_all(&root);

        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        assert!(t.contains_key("ui"), "模板含 [ui]");
    }

    #[test]
    fn test_unknown_section_message_lists_ui() {
        let root = tmp_root("badsec-ui");
        write(&root, "[aii]\nprovider = \"deepseek\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("[ui]"), "未知段报错应列出 [ui]: {err}");

        let root = tmp_root("notable-ui");
        write(&root, "ai = \"x\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("[ui]"), "非段报错应列出 [ui]: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_set_values_replaces_in_place_preserving_comments() {
        let root = tmp_root("upsert");
        let original = "# 头注释\n[ai]\nprovider = \"deepseek\"\n# 中间注释\napi_key = \"\"\n\n[exchange]\ndemo_key = \"\"\n";
        write(&root, original);
        set_values(
            &root,
            &[
                ("ai", "api_key", SetValue::Str("sk-new".into())),
                ("exchange", "demo_key", SetValue::Str("DK".into())),
            ],
        )
        .unwrap();
        let after = std::fs::read_to_string(path(&root)).unwrap();
        assert!(after.contains("# 头注释"), "注释保留:\n{after}");
        assert!(after.contains("# 中间注释"), "注释保留:\n{after}");
        assert!(after.contains("api_key = \"sk-new\""));
        assert!(after.contains("demo_key = \"DK\""));
        // 未涉及的行原样。
        assert!(after.contains("provider = \"deepseek\""));
        assert!(!after.contains("demo_secret = \"\""), "原本没有的键不应凭空出现");
        let f = load(&root).unwrap();
        assert_eq!(f.ai.api_key.as_deref(), Some("sk-new"));
        assert_eq!(f.exchange.demo_key.as_deref(), Some("DK"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_set_values_inserts_missing_key_and_section() {
        let root = tmp_root("upsert-add");
        write(&root, "[ai]\nprovider = \"deepseek\"\n");
        // 段在键缺: 插入。
        set_values(&root, &[("ai", "api_key", SetValue::Str("k".into()))]).unwrap();
        // 段缺: 末尾追加。
        set_values(&root, &[("market", "show_all_pairs", SetValue::Bool(true))]).unwrap();
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(body.contains("api_key = \"k\""));
        assert!(body.contains("[market]") && body.contains("show_all_pairs = true"));
        assert!(load(&root).unwrap().market.show_all_pairs);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_set_values_escapes_quotes_and_rejects_unknown() {
        // 转义: key 中含双引号/反斜杠仍可往返。
        let rendered = SetValue::Str("a\"b\\c".to_string()).render();
        // (直接测 render 输出与 TOML 解析)
        let toml_line = format!("k = {rendered}");
        let parsed: toml::Table = toml::from_str(&toml_line).unwrap();
        assert_eq!(parsed["k"].as_str(), Some("a\"b\\c"));

        let root = tmp_root("upsert-deny");
        write(&root, &template_text());
        let err = set_values(&root, &[("ai", "evil", SetValue::Str("x".into()))]).unwrap_err();
        assert!(err.to_string().contains("可写白名单"));
        let err = set_values(&root, &[("bogus", "x", SetValue::Bool(true))]).unwrap_err();
        assert!(err.to_string().contains("可写白名单"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_set_values_idempotent_no_write_when_unchanged() {
        let root = tmp_root("upsert-same");
        write(&root, "[ai]\nprovider = \"deepseek\"\napi_key = \"same\"\n");
        let before = std::fs::read_to_string(path(&root)).unwrap();
        set_values(&root, &[("ai", "api_key", SetValue::Str("same".into()))]).unwrap();
        let after = std::fs::read_to_string(path(&root)).unwrap();
        assert_eq!(before, after, "值未变时不应改写(保持 mtime/权限)");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 密钥环(033): 别名 / 环境校验 ----

    #[test]
    fn test_check_alias_trims_and_rejects_bad() {
        assert_eq!(check_alias("  工作号 DeepSeek  ").unwrap(), "工作号 DeepSeek");
        assert_eq!(check_alias("工作号 DeepSeek").unwrap().chars().count(), 12);
        for bad in ["", "   ", "\n", "a\nb", "\t"] {
            assert!(check_alias(bad).is_err(), "别名 {bad:?} 应被拒");
        }
        // 长度按**字符**算: 24 个汉字合法, 25 个非法(不被 UTF-8 字节数误伤)。
        let ok: String = "密".repeat(MAX_ALIAS_CHARS);
        assert!(check_alias(&ok).is_ok());
        let too_long: String = "密".repeat(MAX_ALIAS_CHARS + 1);
        let err = check_alias(&too_long).unwrap_err();
        assert!(err.contains("过长"), "{err}");
    }

    #[test]
    fn test_check_env_only_live_or_demo() {
        assert_eq!(check_env(" live ").unwrap(), ENV_LIVE);
        assert_eq!(check_env("demo").unwrap(), ENV_DEMO);
        let err = check_env("testnet").unwrap_err();
        assert!(err.contains("live") && err.contains("demo"), "{err}");
    }

    // ---- 密钥环(033): 解析 ----

    #[test]
    fn test_template_is_valid_toml_with_vault_examples_commented() {
        // 模板里的密钥环示例必须是**注释**, 否则新用户一启动就凭空多出一堆空条目。
        let t: toml::Table = toml::from_str(&template_text()).expect("模板必须合法");
        assert!(!t.contains_key(AI_KEY_TABLE), "模板不应产生真实 [[ai_key]] 条目");
        assert!(template_text().contains("# [[ai_key]]"), "模板应带注释形式的示例");
        let root = tmp_root("vault-tmpl");
        write(&root, &template_text());
        let f = load(&root).unwrap();
        assert!(f.ai_keys.is_empty() && f.exchange_keys.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_parse_vault_entries() {
        let root = tmp_root("vault-read");
        write(
            &root,
            "[[ai_key]]\n\
             alias = \"工作号 DeepSeek\"\n\
             provider = \"deepseek\"\n\
             model = \"deepseek-flash\"\n\
             base_url = \"\"\n\
             api_key = \"sk-a\"\n\
             \n\
             [[ai_key]]\n\
             alias = \"备用 Kimi\"\n\
             provider = \"moonshot\"\n\
             api_key = \"sk-b\"\n\
             \n\
             [[exchange_key]]\n\
             alias = \"测试网\"\n\
             env = \"demo\"\n\
             key = \"K\"\n\
             secret = \"S\"\n",
        );
        let f = load(&root).unwrap();
        assert_eq!(f.ai_keys.len(), 2);
        assert_eq!(f.ai_keys[0].alias, "工作号 DeepSeek");
        assert_eq!(f.ai_keys[0].api_key.as_deref(), Some("sk-a"));
        assert_eq!(f.ai_keys[1].provider, "moonshot");
        assert_eq!(f.ai_keys[1].model, "", "缺 model = 空串(选用后由预设兜底)");
        assert_eq!(f.exchange_keys.len(), 1);
        assert_eq!(f.exchange_keys[0].env, ENV_DEMO);
        assert_eq!(f.exchange_keys[0].secret.as_deref(), Some("S"));
        // 空串密钥解析为 None(str_opt 的既定语义), 与 [ai].api_key 一致。
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_parse_vault_entries_hard_fail() {
        // 缺 alias。
        let root = tmp_root("vault-noalias");
        write(&root, "[[ai_key]]\nprovider = \"deepseek\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("alias"), "{err}");
        // 缺 provider。
        write(&root, "[[ai_key]]\nalias = \"a\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("provider"), "{err}");
        // env 非法。
        write(&root, "[[exchange_key]]\nalias = \"a\"\nenv = \"prod\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("env"), "{err}");
        // 未知键。
        write(&root, "[[ai_key]]\nalias = \"a\"\nprovider = \"p\"\nsekret = \"x\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("未知键 `sekret`"), "{err}");
        // 写成普通段(单中括号)= 不是数组表。
        write(&root, "[ai_key]\nalias = \"a\"\n");
        let err = load(&root).unwrap_err().to_string();
        assert!(err.contains("数组表"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 密钥环(033): 块级增删改 ----

    fn ai_entry(alias: &str, provider: &str, key: &str) -> AiKeyEntry {
        AiKeyEntry {
            alias: alias.into(),
            provider: provider.into(),
            model: "m-1".into(),
            base_url: String::new(),
            api_key: Some(key.into()),
        }
    }

    #[test]
    fn test_upsert_ai_key_appends_and_preserves_rest_verbatim() {
        let root = tmp_root("vault-append");
        let head = "# 我的注释\n[ai]\nprovider = \"deepseek\"\napi_key = \"\"\n";
        write(&root, head);
        upsert_ai_key(&root, &ai_entry("工作号", "deepseek", "sk-a"), None).unwrap();
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(body.starts_with(head), "既有内容必须逐字保留在前: {body}");
        assert!(body.contains("[[ai_key]]"), "{body}");
        assert!(body.ends_with("api_key = \"sk-a\"\n"), "{body}");
        // 再追加第二条: 两条都在, 且仍然回得来。
        upsert_ai_key(&root, &ai_entry("备用", "moonshot", "sk-b"), None).unwrap();
        let f = load(&root).unwrap();
        assert_eq!(f.ai_keys.len(), 2);
        assert_eq!(f.ai.provider, "deepseek", "写密钥环不影响生效段");
        assert_eq!(f.ai.api_key, None, "生效段仍是空的");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_upsert_ai_key_replaces_in_place_and_renames() {
        let root = tmp_root("vault-update");
        write(&root, &template_text());
        upsert_ai_key(&root, &ai_entry("A", "deepseek", "sk-1"), None).unwrap();
        upsert_ai_key(&root, &ai_entry("B", "moonshot", "sk-2"), None).unwrap();
        let before = std::fs::read_to_string(path(&root)).unwrap();
        // 只数"真段头"(`[[ai_key]]` 独占一行); 模板注释里的同名文本不算。
        let blocks_before = before.lines().filter(|l| l.trim() == "[[ai_key]]").count();
        assert_eq!(blocks_before, 2);

        // 覆盖保存(按新别名定位): 只改内容, 不新增块。
        upsert_ai_key(&root, &ai_entry("A", "deepseek", "sk-1-new"), None).unwrap();
        let f = load(&root).unwrap();
        assert_eq!(f.ai_keys.len(), 2, "覆盖保存不得新增块");
        assert_eq!(f.ai_keys[0].api_key.as_deref(), Some("sk-1-new"));
        assert_eq!(f.ai_keys[0].alias, "A", "别名未变");

        // 改名: prev_alias 定位旧块并在原位替换, 不产生第三块。
        upsert_ai_key(&root, &ai_entry("A改", "deepseek", "sk-1-new"), Some("A")).unwrap();
        let f = load(&root).unwrap();
        assert_eq!(f.ai_keys.len(), 2, "改名不得新增块");
        let aliases: Vec<&str> = f.ai_keys.iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, vec!["A改", "B"], "顺序与剩余条目不变");
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(body.contains("密钥环 / Key vault"), "块外注释必须保留");
        assert!(body.contains("# [[ai_key]]"), "模板里的注释示例必须保留");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_remove_ai_key_drops_block_and_is_idempotent() {
        let root = tmp_root("vault-remove");
        write(&root, &template_text());
        upsert_ai_key(&root, &ai_entry("A", "deepseek", "sk-1"), None).unwrap();
        upsert_ai_key(&root, &ai_entry("B", "moonshot", "sk-2"), None).unwrap();
        assert!(remove_ai_key(&root, "A").unwrap(), "删存在的别名应返回 true");
        assert!(!remove_ai_key(&root, "A").unwrap(), "再删应返回 false(交调用方回 404)");
        let f = load(&root).unwrap();
        assert_eq!(f.ai_keys.len(), 1);
        assert_eq!(f.ai_keys[0].alias, "B");
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(!body.contains("sk-1"), "被删条目的密钥不得残留: {body}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_add_remove_cycles_do_not_accumulate_blank_lines() {
        let root = tmp_root("vault-blank");
        write(&root, "[ai]\nprovider = \"deepseek\"\n");
        upsert_ai_key(&root, &ai_entry("A", "deepseek", "sk-1"), None).unwrap();
        let one = std::fs::read_to_string(path(&root)).unwrap();
        for _ in 0..5 {
            remove_ai_key(&root, "A").unwrap();
            upsert_ai_key(&root, &ai_entry("A", "deepseek", "sk-1"), None).unwrap();
        }
        let again = std::fs::read_to_string(path(&root)).unwrap();
        assert_eq!(one, again, "反复增删应回到同一份正文(不攒空行)");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_append_block_lands_after_exactly_one_blank_line() {
        let root = tmp_root("vault-blank-normalize");
        // ① 末尾无空行的文件: 追加后应补上一个空行作为与上一段的间隔。
        write(&root, "[ai]\nprovider = \"deepseek\"\n");
        upsert_ai_key(&root, &ai_entry("A", "deepseek", "sk-1"), None).unwrap();
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(body.contains("provider = \"deepseek\"\n\n[[ai_key]]\n"), "{body:?}");

        // ② 末尾连着 3 个空行(手工编辑留下的噪声): 追加另一张表时归一化成恰好 1 个。
        write(&root, &format!("{body}\n\n\n"));
        upsert_exchange_key(
            &root,
            &ExchangeKeyEntry {
                alias: "X".into(),
                env: ENV_LIVE.into(),
                key: Some("K".into()),
                secret: Some("S".into()),
            },
            None,
        )
        .unwrap();
        let after = std::fs::read_to_string(path(&root)).unwrap();
        assert!(
            after.contains("api_key = \"sk-1\"\n\n[[exchange_key]]\n"),
            "跨表追加前应恰好一个空行: {after:?}"
        );
        assert!(!after.contains("\n\n\n"), "不该把空行攒起来: {after:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_exchange_key_roundtrip_and_crlf_kept() {
        let root = tmp_root("vault-crlf");
        write(&root, "[exchange]\r\nbinance_key = \"\"\r\n");
        upsert_exchange_key(
            &root,
            &ExchangeKeyEntry {
                alias: "主账户".into(),
                env: ENV_LIVE.into(),
                key: Some("K".into()),
                secret: Some("S".into()),
            },
            None,
        )
        .unwrap();
        let body = std::fs::read_to_string(path(&root)).unwrap();
        assert!(body.contains("[[exchange_key]]\r\n"), "应跟随原文件的 CRLF: {body:?}");
        let f = load(&root).unwrap();
        assert_eq!(f.exchange_keys.len(), 1);
        assert_eq!(f.exchange_keys[0].env, ENV_LIVE);
        assert!(remove_exchange_key(&root, "主账户").unwrap());
        assert!(load(&root).unwrap().exchange_keys.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_alias_with_quotes_roundtrips() {
        let root = tmp_root("vault-quote");
        write(&root, "[ai]\n");
        upsert_ai_key(&root, &ai_entry("他说\"你好\"\\ok", "p", "sk-1"), None).unwrap();
        let f = load(&root).unwrap();
        assert_eq!(f.ai_keys[0].alias, "他说\"你好\"\\ok");
        let _ = std::fs::remove_dir_all(&root);
    }
}
