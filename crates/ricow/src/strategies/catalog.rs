//! 策略目录(catalog): 内置示例 + 用户自写策略的统一注册表 (031)。
//!
//! 策略清单(manifest)是**表现层数据**: 描述策略的中文名/说明/参数 schema, 仅供
//! CLI 帮助 / AI 工具 / Web 端点展示与表单渲染。引擎与绑定层不读清单(架构铁律:
//! 引擎/CLI/绑定层零策略参数名 —— 清单参数键是数据, 不是读值调用)。
//!
//! 本文件生产代码**不得出现策略参数名字面量**(`pair`/`script` 这类通用键除外):
//! 参数键全部来自 TOML 清单数据(内置经 `include_str!` 嵌入, 用户经磁盘扫描)。

use ricow_strategy::ConfigValue;
use serde::{Deserialize, Serialize};

/// 参数类型(驱动 UI 表单渲染)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamType {
    #[serde(rename = "f64")]
    Float,
    #[serde(rename = "i64")]
    Int,
    #[serde(rename = "string")]
    Str,
    #[serde(rename = "bool")]
    Bool,
    #[serde(rename = "enum")]
    Enum,
}

/// 一个参数的展示元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestParam {
    /// 参数键名(与 Lua `config_*`/`num`/`cfg_str` 读取键一致)。
    pub key: String,
    /// 中文名。
    pub name: String,
    /// 类型。
    #[serde(rename = "type")]
    pub ty: ParamType,
    /// 说明。
    pub desc: String,
    /// 默认值(UI 预填; 与 Lua 兜底默认一致, 由一致性单测兜底)。
    /// `skip_serializing_if`: 无默认值时不出现在序列化结果里 (TOML 无 null; JSON 也保持精简)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ConfigValue>,
    /// 是否必填。
    #[serde(default)]
    pub required: bool,
    /// 枚举项(仅 ty == Enum)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// 一份策略清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyManifest {
    /// 英文标识, 全局唯一(`--strategy <id>` 与实例 TOML 的 type 都用它)。
    pub id: String,
    /// 中文名(UI 标题)。
    pub name: String,
    /// 市场: "spot" | "futures"。
    pub market: String,
    /// 一句话说明。
    pub summary: String,
    /// 长说明(可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 适用场景(可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suitable: Option<String>,
    /// 不适用场景(可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsuitable: Option<String>,
    /// 参数列表(声明顺序即表单顺序)。
    #[serde(default)]
    pub params: Vec<ManifestParam>,
    /// 持仓模式 (032, 可选): "one-way" (缺省) | "hedge" —— 直跑路径回填 config 用。
    #[serde(default)]
    pub position_mode: Option<String>,
    /// 默认杠杆 (032, 可选): 直跑路径无 `[backtest]` 段时回填 `leverage` (审核 S1)。
    #[serde(default)]
    pub default_leverage: Option<f64>,
    /// `[backtest]` 段 (045, 可选): 回测引擎参数持久默认 (如 margin_mode), 由
    /// `resolve_builtin_script` 合并进 config —— 直跑/回测路径策略 TOML 的引擎级
    /// 默认值必须生效, 不能只对 `ricow create` 之类读原始 TOML 的路径生效。
    #[serde(default)]
    pub backtest: Option<ricow_strategy::BacktestToml>,
}

impl StrategyManifest {
    /// 解析清单 TOML 并做基础校验(id 非空 / market 合法 / 参数 key 非空)。
    pub fn parse(toml_str: &str) -> Result<Self, String> {
        let m: StrategyManifest =
            toml::from_str(toml_str).map_err(|e| format!("清单解析失败: {e}"))?;
        if m.id.is_empty() {
            return Err("清单缺 id".to_string());
        }
        if m.market != "spot" && m.market != "futures" {
            return Err(format!("清单 {} 的 market 非法: {}", m.id, m.market));
        }
        if m.params.iter().any(|p| p.key.is_empty()) {
            return Err(format!("清单 {} 有参数缺 key", m.id));
        }
        if let Some(p) = m.params.iter().find(|p| p.name.is_empty()) {
            return Err(format!("清单 {} 的参数 {} 缺中文名", m.id, p.key));
        }
        Ok(m)
    }

    /// 校验清单市场与所在目录一致(FR-004)。
    pub fn check_dir(&self, dir: &str) -> Result<(), String> {
        if self.market != dir {
            return Err(format!(
                "策略 {} 的 market={} 与目录 {} 不一致",
                self.id, self.market, dir
            ));
        }
        Ok(())
    }
}

/// 策略来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Builtin,
    User,
}

/// 目录里的一条策略: 清单 + Lua 源码 + 来源 + 诚实性标记。
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub manifest: StrategyManifest,
    pub code: String,
    pub source: Source,
    /// 该条目是否由磁盘上的**清单 TOML** 声明(内置恒 `true`)。
    ///
    /// `false` = 只有一份裸 `.lua` 落盘(`ricow deploy` / Web 保存但未走过 P2-8 声明)——
    /// 参数 schema 为空, 前端表单只画得出交易对; 必填参数(如香农的 `start_price`)不在表单里,
    /// 启动即 `[FATAL] 缺少必填参数 -> 停机`, 0 成交(2026-10-05 事件)。标记出来, 别让用户猜。
    pub declared: bool,
    /// 脚本与某个内置脚本**逐字相同**(归一化换行后)时, 那个内置的 id —— 冗余副本。
    ///
    /// 同一策略逻辑以两个 id 各跑一份、又指向同一交易对时, 两个实例会各自下单(互不知情地
    /// 重复建仓)。2026-10-05 现场: `strategies/spot/shannon_spot_grid-1.lua` 与内置逐字节相同。
    pub duplicate_of: Option<String>,
}

impl CatalogEntry {
    /// 是否应拒绝运行(回测 / 启动): **未声明清单**的裸 `.lua` 且与内置脚本逐字相同。
    ///
    /// 已声明的副本只标记不拦——用户明确为它写过清单, 拦截权还给他。
    pub fn blocks_run(&self) -> bool {
        !self.declared && self.duplicate_of.is_some()
    }
}

/// 内置示例(编译期嵌入): (策略 id, 清单 TOML, Lua 源码)。
const BUILTIN: &[(&str, &str, &str)] = &[
    (
        "paired_grid",
        include_str!("../../../../strategies/spot/paired_grid.toml"),
        include_str!("../../../../strategies/spot/paired_grid.lua"),
    ),
    (
        "paired_grid_futures_long",
        include_str!("../../../../strategies/futures/paired_grid_futures_long.toml"),
        include_str!("../../../../strategies/futures/paired_grid_futures_long.lua"),
    ),
    (
        "shannon_grid",
        include_str!("../../../../strategies/spot/shannon_grid.toml"),
        include_str!("../../../../strategies/spot/shannon_grid.lua"),
    ),
    (
        "shannon_virtual_grid",
        include_str!("../../../../strategies/spot/shannon_virtual_grid.toml"),
        include_str!("../../../../strategies/spot/shannon_virtual_grid.lua"),
    ),
    (
        "shannon_hedge_grid_futures",
        include_str!("../../../../strategies/futures/shannon_hedge_grid_futures.toml"),
        include_str!("../../../../strategies/futures/shannon_hedge_grid_futures.lua"),
    ),
    (
        "shannon_grid_futures",
        include_str!("../../../../strategies/futures/shannon_grid_futures.toml"),
        include_str!("../../../../strategies/futures/shannon_grid_futures.lua"),
    ),
    (
        "shannon_neutral_grid_futures",
        include_str!("../../../../strategies/futures/shannon_neutral_grid_futures.toml"),
        include_str!("../../../../strategies/futures/shannon_neutral_grid_futures.lua"),
    ),
    (
        "shannon_short_grid_futures",
        include_str!("../../../../strategies/futures/shannon_short_grid_futures.toml"),
        include_str!("../../../../strategies/futures/shannon_short_grid_futures.lua"),
    ),
];

/// 内置策略 id 是否为保留名。
///
/// 032 US3 起 Web 保存端点(FR-016)复用同一份清单做保留名拒绝 —— 不新增第二份保留名列表。
pub(crate) fn is_builtin_id(id: &str) -> bool {
    BUILTIN.iter().any(|(bid, _, _)| *bid == id)
}

/// 归一化 Lua 源码, 只用于"是不是同一份脚本"的比对: **去掉所有 `\r`** + 去首尾空白。
///
/// 逐字节比对会漏掉行尾差异 —— 2026-10-05 那份 `strategies/spot/shannon_spot_grid-1.lua`
/// 与内置正是"内容相同, 只差行尾"。去 `\r`(而不是只把 CRLF 折成 LF)才能同时覆盖
/// CRLF / LF / 混排三种形态: 本仓的 `strategies/spot/*.lua` 本身就是 CRLF 落盘,
/// 若只折一次换行, 对已含 CRLF 的原文再加 CRLF 会留下孤立的 `\r` 而判定失配。
fn normalize_lua(src: &str) -> String {
    src.replace('\r', "").trim().to_string()
}

/// 该脚本是否与某个内置脚本是同一份(归一化后相同) → 返回那个内置 id。
fn builtin_duplicate_of(code: &str) -> Option<&'static str> {
    let norm = normalize_lua(code);
    BUILTIN.iter().find(|(_, _, bcode)| normalize_lua(bcode) == norm).map(|(bid, _, _)| *bid)
}

/// "未声明清单的内置副本"的拒绝原因 —— 扫描期问题清单与 [`run_block`] **共用同一段文案**,
/// 免得两处口径漂移(用户从不同入口拿到不同解释)。
fn duplicate_copy_reason(id: &str, builtin: &str) -> String {
    format!(
        "策略 {id} 的脚本与内置 {builtin} 逐字相同, 且未声明参数清单 → 视为冗余副本, 拒绝运行: \
         同一策略以两个 id 各跑一份、又指向同一交易对, 会重复下单(重复建仓)。三个改法任选: \
         ① 直接给实例 TOML 配 type = \"{builtin}\"(不必复制脚本); \
         ② 在策略详情页 →「策略清单」里为它声明参数; ③ 修改脚本使其与内置不同。"
    )
}

/// 统一策略目录 = 内置示例 ∪ 用户自写(磁盘扫描, 按 id 去重, 内置 id 为保留名)。
pub fn all() -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for (id, manifest_toml, code) in BUILTIN {
        let manifest = StrategyManifest::parse(manifest_toml)
            .expect("内置清单解析失败(编译期资产, 应恒可解析)");
        debug_assert_eq!(manifest.id, *id, "内置清单 id 与注册 id 不一致");
        out.push(CatalogEntry {
            manifest,
            code: code.to_string(),
            source: Source::Builtin,
            declared: true,
            duplicate_of: None,
        });
    }
    for e in scan_user() {
        // 按 id 去重(计划 §4.3): 同名后者跳过并警告(内置 id 在前, 用户占用已在扫描期拒)。
        if out.iter().any(|x| x.manifest.id == e.manifest.id) {
            eprintln!("[catalog] 跳过重复策略 id: {}(已有同 id 条目)", e.manifest.id);
            continue;
        }
        out.push(e);
    }
    out
}

/// 按 id 查找策略。
pub fn find(id: &str) -> Option<CatalogEntry> {
    all().into_iter().find(|e| e.manifest.id == id)
}

/// 扫描磁盘 `strategies/{spot,futures}/` 下的用户策略(.toml 清单 + 同名 .lua)。
/// 单个文件的问题(解析失败 / market 与目录不一致 / 占用内置 id / 缺同名 .lua)记录进
/// 问题清单并跳过 —— 加载路径命中该 id 时经 [`problem_for`] 报错, 不静默。
fn scan_user() -> Vec<CatalogEntry> {
    scan_user_in(&crate::commands::strategies_dir()).0
}

/// 同 [`scan_user`], 但同时返回被跳过条目的问题清单(按 id 可查)。
fn scan_user_with_problems() -> (Vec<CatalogEntry>, Vec<(String, String)>) {
    scan_user_in(&crate::commands::strategies_dir())
}

/// 扫描实现体: `strategies_dir` 是 `strategies/` 根(生产 = [`crate::commands::strategies_dir`],
/// 单测注入临时目录 —— 否则测试会读到真实工作区的策略目录, 结果随环境漂移)。
fn scan_user_in(strategies_dir: &std::path::Path) -> (Vec<CatalogEntry>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut problems: Vec<(String, String)> = Vec::new();
    for market in ["spot", "futures"] {
        let dir = strategies_dir.join(market);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        // 第一遍: 有清单(.toml)的策略; 记下其同名 .lua 的 stem, 供第二遍去重。
        let mut with_manifest: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let Ok(toml_text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let manifest = match StrategyManifest::parse(&toml_text) {
                Ok(m) => m,
                Err(e) => {
                    let msg = format!("策略清单 {} 解析失败: {e}", path.display());
                    eprintln!("[catalog] {msg}");
                    problems.push((file_stem, msg));
                    continue;
                }
            };
            if let Err(e) = manifest.check_dir(market) {
                eprintln!("[catalog] {e}");
                problems.push((manifest.id.clone(), e));
                continue;
            }
            if is_builtin_id(&manifest.id) {
                // 内置清单文件本身(stem == id)静默跳过(已由 BUILTIN 处理);
                // 用户策略占用内置保留名(stem != id)记录为问题(FR-010: 报错, 不静默)。
                if file_stem != manifest.id {
                    let msg = format!(
                        "策略 id {} 是内置保留名, 用户策略不得占用: {}",
                        manifest.id,
                        path.display()
                    );
                    eprintln!("[catalog] {msg}");
                    problems.push((manifest.id.clone(), msg));
                }
                continue;
            }
            let code = match std::fs::read_to_string(path.with_extension("lua")) {
                Ok(c) => c,
                Err(e) => {
                    let msg =
                        format!("策略 {} 缺同名 .lua 脚本({}): 跳过, 不注册空代码", manifest.id, e);
                    eprintln!("[catalog] {msg}");
                    problems.push((manifest.id.clone(), msg));
                    continue;
                }
            };
            // 声明过清单 = declared; 脚本若与内置逐字相同仍**注册**(用户明确写过清单, 只标记不拦)。
            let duplicate_of = builtin_duplicate_of(&code).map(str::to_string);
            if let Some(bid) = &duplicate_of {
                eprintln!(
                    "[catalog] 策略 {} 的脚本与内置 {bid} 逐字相同(冗余副本, 标记不拦——已声明清单)",
                    manifest.id
                );
            }
            with_manifest.push(file_stem.clone());
            out.push(CatalogEntry {
                manifest,
                code,
                source: Source::User,
                declared: true,
                duplicate_of,
            });
        }
        // 第二遍(031 §4.2): 无清单的 .lua(如 `ricow deploy` 落盘的策略)→ 合成最小清单,
        // 保证部署后的策略仍出现在统一目录/UI(缺省生成最小清单)。
        for entry in std::fs::read_dir(&dir).ok().into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("lua") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if with_manifest.iter().any(|s| s == &stem) || is_builtin_id(&stem) {
                continue;
            }
            let Ok(code) = std::fs::read_to_string(&path) else { continue };
            let duplicate_of = builtin_duplicate_of(&code).map(str::to_string);
            if let Some(bid) = &duplicate_of {
                // 裸 .lua 且与内置逐字相同 = 与内置并列再来一份, 没有任何声明说明它是谁 ——
                // 记为问题(加载路径也能看到原因), 并由 [`run_block`] 在回测/启动前拒掉。
                let msg = duplicate_copy_reason(&stem, bid);
                eprintln!("[catalog] {msg}");
                problems.push((stem.clone(), msg));
            }
            out.push(CatalogEntry {
                manifest: StrategyManifest {
                    id: stem,
                    name: path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string(),
                    market: market.to_string(),
                    summary: String::new(),
                    description: None,
                    suitable: None,
                    unsuitable: None,
                    params: Vec::new(),
                    position_mode: None,
                    default_leverage: None,
                    backtest: None,
                },
                code,
                source: Source::User,
                declared: false,
                duplicate_of,
            });
        }
    }
    (out, problems)
}

/// 查询某策略 id 在扫描期被跳过/拒绝的原因(若有)。供加载路径把"静默缺失"升级为明确报错。
pub fn problem_for(id: &str) -> Option<String> {
    scan_user_with_problems().1.into_iter().find(|(pid, _)| pid == id).map(|(_, m)| m)
}

/// **运行前门禁**(回测 / 启动共用): 返回 `Some(中文原因)` = 该 id 不可运行。
///
/// 关掉两类"目录里明知有问题, 却仍能被实例 TOML 放行"的漏洞 —— 实例 TOML 的存在性判定
/// 绕过 catalog, 所以只看 `strategies/<id>.toml` 是不是文件是不够的:
/// 1. 扫描期被拒的 id(清单解析失败 / market 与目录不符 / 占用内置保留名 / 缺同名 .lua);
/// 2. **未声明清单**的裸 `.lua` 且与内置脚本逐字相同 —— 冗余副本(与内置并列再跑一份会重复下单)。
///
/// 内置 id 恒放行(内置资产由编译期 `include_str!` 保证, 不存在磁盘漂移)。
pub fn run_block(id: &str) -> Option<String> {
    run_block_in(&crate::commands::strategies_dir(), id)
}

/// [`run_block`] 的实现体(可注入目录, 便于单测)。
fn run_block_in(strategies_dir: &std::path::Path, id: &str) -> Option<String> {
    if is_builtin_id(id) {
        return None;
    }
    // 只扫一次: 条目与问题清单都来自同一次扫描, 避免"查一次扫三遍"。
    let (entries, problems) = scan_user_in(strategies_dir);
    // 标记优先(与问题清单同文案): 未声明清单的裸 .lua 且与内置脚本逐字相同。
    if let Some(e) = entries.iter().find(|e| e.manifest.id == id) {
        if e.blocks_run() {
            return Some(duplicate_copy_reason(
                id,
                e.duplicate_of.as_deref().unwrap_or("内置策略"),
            ));
        }
    }
    problems.into_iter().find(|(pid, _)| pid == id).map(|(_, why)| why)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
id = "paired_grid"
name = "现货动态非对称网格"
market = "spot"
summary = "固定金额配对网格"

[[params]]
key = "start_price"
name = "开始价格"
type = "f64"
desc = "价格低于它才激活"
required = true

[[params]]
key = "spacing_pct"
name = "间距百分比"
type = "f64"
desc = "网格间距"
default = 0.01

[[params]]
key = "atr_period"
name = "ATR 根数"
type = "i64"
desc = "样本根数"
default = 14

[[params]]
key = "accumulate_mode"
name = "成交模式"
type = "enum"
desc = "u 或 coin"
default = "u"
options = ["u", "coin"]
"#;

    #[test]
    fn parse_full_manifest() {
        let m = StrategyManifest::parse(MANIFEST).unwrap();
        assert_eq!(m.id, "paired_grid");
        assert_eq!(m.name, "现货动态非对称网格");
        assert_eq!(m.market, "spot");
        assert_eq!(m.params.len(), 4);

        let start = &m.params[0];
        assert_eq!(start.key, "start_price");
        assert_eq!(start.ty, ParamType::Float);
        assert!(start.required);
        assert!(start.default.is_none());

        let spacing = &m.params[1];
        assert_eq!(spacing.default.as_ref().and_then(|v| v.as_f64()), Some(0.01));

        let atr = &m.params[2];
        assert_eq!(atr.ty, ParamType::Int);
        assert_eq!(atr.default.as_ref().and_then(|v| v.as_i64()), Some(14));

        let mode = &m.params[3];
        assert_eq!(mode.ty, ParamType::Enum);
        assert_eq!(mode.default.as_ref().and_then(|v| v.as_str()), Some("u"));
        assert_eq!(mode.options, Some(vec!["u".to_string(), "coin".to_string()]));
    }

    #[test]
    fn manifest_optional_fields_default_none() {
        // 032: 旧清单(无 position_mode / default_leverage)必须照常解析, 两字段缺省 None。
        let m = StrategyManifest::parse(MANIFEST).unwrap();
        assert_eq!(m.position_mode, None);
        assert_eq!(m.default_leverage, None);
    }

    #[test]
    fn parse_manifest_with_position_mode_and_leverage() {
        // 032: futures 清单声明 position_mode + default_leverage → 正确解析。
        let src = r#"
id = "paired_grid_futures"
name = "合约双向配对网格"
market = "futures"
summary = "多空双向网格"
position_mode = "hedge"
default_leverage = 2.0
"#;
        let m = StrategyManifest::parse(src).unwrap();
        assert_eq!(m.position_mode.as_deref(), Some("hedge"));
        assert_eq!(m.default_leverage, Some(2.0));
    }

    #[test]
    fn parse_rejects_bad_type() {
        let bad = MANIFEST.replace("type = \"f64\"", "type = \"wat\"");
        assert!(StrategyManifest::parse(&bad).is_err());
    }

    #[test]
    fn parse_rejects_empty_id() {
        let bad = MANIFEST.replace("id = \"paired_grid\"", "id = \"\"");
        assert!(StrategyManifest::parse(&bad).unwrap_err().contains("id"));
    }

    #[test]
    fn parse_rejects_bad_market() {
        let bad = MANIFEST.replace("market = \"spot\"", "market = \"options\"");
        assert!(StrategyManifest::parse(&bad).unwrap_err().contains("market"));
    }

    #[test]
    fn parse_rejects_empty_param_key() {
        let bad = MANIFEST.replace("key = \"start_price\"", "key = \"\"");
        assert!(StrategyManifest::parse(&bad).is_err());
    }

    #[test]
    fn check_dir_matches_market() {
        let m = StrategyManifest::parse(MANIFEST).unwrap();
        assert!(m.check_dir("spot").is_ok());
        assert!(m.check_dir("futures").is_err());
    }

    #[test]
    fn builtin_catalog_has_both_strategies() {
        let entries = all();
        assert!(entries
            .iter()
            .any(|e| e.manifest.id == "paired_grid" && e.source == Source::Builtin));
        assert!(entries
            .iter()
            .any(|e| e.manifest.id == "paired_grid" && e.code.contains("on_tick")));
    }

    #[test]
    fn find_works() {
        assert!(find("paired_grid").is_some());
        assert!(find("no-such-strategy").is_none());
    }

    // ---- 诚实性标记: declared / duplicate_of / run_block(2026-10-05) ----

    /// 临时目录: 每个用例一个独占目录, 避免并行互踩。
    fn tmp_strategies_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir()
            .join(format!("ricow-catalog-{tag}-{}-{nanos}", std::process::id()))
            .join("strategies");
        std::fs::create_dir_all(dir.join("spot")).expect("建临时 strategies/spot");
        dir
    }

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        std::fs::write(dir.join(rel), body).expect("写临时文件");
    }

    /// 换行归一化: 逐字节比对会漏掉 Windows 另存为引入的 CRLF(现场那个副本正是这种形态)。
    #[test]
    fn normalize_lua_ignores_line_endings_and_trailing_space() {
        assert_eq!(normalize_lua("a\r\nb\r\n"), normalize_lua("a\nb"));
        assert_eq!(normalize_lua("  x\n"), "x");
    }

    /// 与内置逐字相同(含 CRLF 变体)→ 认出重复; 改一个字符 → 不再算重复。
    #[test]
    fn builtin_duplicate_of_detects_copy_but_not_edit() {
        let (bid, _, bcode) = BUILTIN[0];
        assert_eq!(builtin_duplicate_of(bcode), Some(bid), "原样必须是重复");
        let crlf = bcode.replace('\n', "\r\n");
        assert_eq!(builtin_duplicate_of(&crlf), Some(bid), "CRLF 变体也算重复");
        let edited = format!("{bcode}\n-- 改一行\n");
        assert_eq!(builtin_duplicate_of(&edited), None, "有改动就不算重复");
    }

    /// 内置条目恒为"已声明 + 非副本"。
    #[test]
    fn builtin_entries_are_declared_without_duplicate() {
        for e in all().iter().filter(|e| e.source == Source::Builtin) {
            assert!(e.declared, "内置 {} 必须 declared", e.manifest.id);
            assert!(e.duplicate_of.is_none(), "内置 {} 不应是副本", e.manifest.id);
            assert!(!e.blocks_run(), "内置 {} 不得被运行门禁拦下", e.manifest.id);
        }
    }

    /// 三种落盘形态的标记: 裸 .lua(未声明) / 声明过清单的 / 裸 .lua 且是内置副本。
    #[test]
    fn scan_marks_declared_and_duplicate() {
        let dir = tmp_strategies_dir("marks");
        let builtin_code = BUILTIN[0].2;

        // ① 裸 .lua(自己的代码, 没清单)→ 未声明
        write(&dir, "spot/own.lua", "function on_tick(ctx) return {} end\n");
        // ② 声明过清单 + 自己的代码 → 已声明
        write(
            &dir,
            "spot/decl.toml",
            "id = \"decl\"\nname = \"声明过的\"\nmarket = \"spot\"\nsummary = \"\"\n",
        );
        write(&dir, "spot/decl.lua", "function on_tick(ctx) return {} end\n");
        // ③ 裸 .lua 且与内置逐字相同(CRLF 形态)→ 未声明 + 副本
        write(&dir, "spot/copy.lua", &builtin_code.replace('\n', "\r\n"));

        let (entries, problems) = scan_user_in(&dir);
        let get = |id: &str| entries.iter().find(|e| e.manifest.id == id).expect("条目应在");

        let own = get("own");
        assert!(!own.declared, "裸 .lua 必须是未声明");
        assert!(own.duplicate_of.is_none());
        assert!(!own.blocks_run(), "自己写的代码不该被拦");

        let decl = get("decl");
        assert!(decl.declared, "有同名清单 TOML 即已声明");
        assert!(decl.duplicate_of.is_none());

        let copy = get("copy");
        assert!(!copy.declared);
        assert_eq!(copy.duplicate_of.as_deref(), Some(BUILTIN[0].0));
        assert!(copy.blocks_run(), "未声明的内置副本必须被运行门禁拦下");

        // 副本同时进问题清单(供加载路径把"静默缺失"升级为明确报错)。
        assert!(problems.iter().any(|(pid, _)| pid == "copy"), "副本应记入问题清单: {problems:?}");

        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    /// 运行门禁: 未声明副本 → 拒并给出中文原因(含内置 id 与改法); 已声明副本 → 放行;
    /// 未知 id / 内置 id → 放行(未知交由调用方按 404 处理)。
    #[test]
    fn run_block_rejects_only_undeclared_builtin_copy() {
        let dir = tmp_strategies_dir("runblock");
        let builtin_code = BUILTIN[0].2;
        write(&dir, "spot/copy.lua", builtin_code);
        write(
            &dir,
            "spot/decl.toml",
            "id = \"decl\"\nname = \"声明过的\"\nmarket = \"spot\"\nsummary = \"\"\n",
        );
        write(&dir, "spot/decl.lua", builtin_code);

        let why = run_block_in(&dir, "copy").expect("未声明副本必须被拒");
        assert!(why.contains(BUILTIN[0].0), "原因须点名内置 id: {why}");
        assert!(why.contains("重复下单"), "原因须说清危害: {why}");

        assert!(run_block_in(&dir, "decl").is_none(), "已声明的副本只标记不拦");
        assert!(run_block_in(&dir, "no-such").is_none(), "未知 id 交给调用方按 404 处理");
        assert!(run_block_in(&dir, BUILTIN[0].0).is_none(), "内置 id 恒放行");

        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }
}
