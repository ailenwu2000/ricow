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

/// 配置文件名(位于 `$RICOW_ROOT/`)。
pub const FILE: &str = "ricow.toml";

/// 配置文件路径。
pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// AI 段(provider 与其密钥同处一段, 一眼看清"这把 key 属于谁")。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSection {
    pub provider: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_turns: Option<i64>,
    pub api_key: Option<String>,
}

impl Default for AiSection {
    fn default() -> Self {
        Self {
            provider: crate::ai::config::DEFAULT_PROVIDER.to_string(),
            model: None,
            base_url: None,
            max_turns: None,
            api_key: None,
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

/// 整个配置文件的内存表示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct File {
    pub ai: AiSection,
    pub exchange: ExchangeSection,
    pub market: MarketSection,
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
    format!(
        "# ricow 配置文件(本机私有, 含密钥 —— 不要外传/提交; 已在 .gitignore)。\n\
         # 可直接用编辑器改本文件; 首次启动向导与对话内 /keys、/market 也会更新这里(只改对应行, 注释保留)。\n\
         # 键名/段名拼错会被拒绝(不会静默失效); 值为空 = 未填写。\n\
         \n\
         # ── ① AI 助手通道 ──────────────────────────────────────────────\n\
         # provider: 可写内置预设名, 也可写任意自定义名(自建/中转端点, 此时必须写 base_url)。\n\
         #   内置预设: {presets}\n\
         #   注意: 所选模型必须支持 function calling(工具调用), 否则 AI 无法调用行情/回测等工具。\n\
         # api_key : 上面 provider 的密钥(谁家的 key 就贴在这儿, 两者挨着, 不会搞混)。\n\
         #   本机端点(ollama)免密钥。也支持环境变量 RICOW_AI_API_KEY 临时覆盖。\n\
         [ai]\n\
         provider = \"deepseek\"\n\
         model = \"deepseek-flash\"\n\
         api_key = \"\"\n\
         max_turns = 8\n\
         # base_url = \"https://api.deepseek.com/v1\"   # 仅自定义/自建端点才需要\n\
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
         show_all_pairs = false\n"
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
        // "仅当前用户", 逼近 unix 0600。**尽力而为**: 失败不阻断写配置(配置本身要能写进去),
        // 但**如实提示**(stderr), 不静默假装已保护。
        if let Err(msg) = harden_secret_file(path) {
            eprintln!("提示: 未能收紧 {} 的访问权限: {msg}。该文件含 API 密钥, 请确认其所在目录非共享目录。", path.display());
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

const AI_KEYS: [&str; 5] = ["provider", "model", "base_url", "max_turns", "api_key"];
const EXCHANGE_KEYS: [&str; 4] = ["demo_key", "demo_secret", "binance_key", "binance_secret"];
const MARKET_KEYS: [&str; 1] = ["show_all_pairs"];

/// 读取配置; **文件不存在 → 生成模板并按内置默认继续**(缺什么由使用处给出可执行提示)。
pub fn load(root: &Path) -> CoreResult<File> {
    let p = path(root);
    if !p.exists() {
        ensure_template(root)?;
        return Ok(File::default());
    }
    let text = std::fs::read_to_string(&p)
        .map_err(|e| CoreError::Auth(format!("读取配置文件 {} 失败: {e}", p.display())))?;
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| CoreError::Auth(format!("配置文件 {} 不是合法 TOML: {e}", p.display())))?;

    let mut out = File::default();
    for (section, value) in &table {
        let t = value.as_table().ok_or_else(|| {
            CoreError::Auth(format!(
                "配置文件 {} 的 `{section}` 不是段(table): 配置请按 [ai] / [exchange] / [market] 分段书写",
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
            other => {
                return Err(CoreError::Auth(format!(
                    "配置文件 {} 里有未知段 `[{other}]`; 允许的段: [ai] / [exchange] / [market]",
                    p.display()
                )))
            }
        }
    }
    if out.ai.provider.trim().is_empty() {
        out.ai.provider = crate::ai::config::DEFAULT_PROVIDER.to_string();
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
            ("market", "show_all_pairs") => v.is_bool(),
            _ => v.is_str(),
        };
        if !type_ok {
            let expected = if (section, k.as_str()) == ("market", "show_all_pairs") {
                "true/false"
            } else {
                "字符串(如 key = \"...\")"
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
const WRITABLE: [(&str, &str); 9] = [
    ("ai", "provider"),
    ("ai", "model"),
    ("ai", "base_url"),
    ("ai", "api_key"),
    ("exchange", "demo_key"),
    ("exchange", "demo_secret"),
    ("exchange", "binance_key"),
    ("exchange", "binance_secret"),
    ("market", "show_all_pairs"),
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

/// **按行外科式更新**配置(019-R4): 只替换/插入给定键所在行, 保留全部注释与用户其它内容。
///
/// - 键行匹配: 段头之后、下段头之前, 行首(允许空白)为 `key =`; 注释掉的行(`# key =`)不算;
/// - 段缺失 → 文件末尾追加 `[section]` 与键行;
/// - 段在、键缺 → 在该段头部之后插入键行(不碰后续内容);
/// - 原子写, 权限维持 0600; 文件不存在先报 Auth 错误(调用方应先 ensure_template)。
pub fn set_values(root: &Path, updates: &[(&str, &str, SetValue)]) -> CoreResult<()> {
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
    let text = std::fs::read_to_string(&p)
        .map_err(|e| CoreError::Auth(format!("读取配置文件 {} 失败: {e}", p.display())))?;
    let mut body = text.clone();
    for (section, key, value) in updates {
        body = upsert_line(&body, section, key, &value.render())
            .map_err(CoreError::InvalidArgument)?;
    }
    if body != text {
        write_private(&p, &body)?;
    }
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

fn str_opt(t: &toml::Table, key: &str) -> Option<String> {
    t.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
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
}
