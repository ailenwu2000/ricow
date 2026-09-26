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
    #[serde(default)]
    pub default: Option<ConfigValue>,
    /// 是否必填。
    #[serde(default)]
    pub required: bool,
    /// 枚举项(仅 ty == Enum)。
    #[serde(default)]
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
    #[serde(default)]
    pub description: Option<String>,
    /// 适用场景(可选)。
    #[serde(default)]
    pub suitable: Option<String>,
    /// 不适用场景(可选)。
    #[serde(default)]
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

/// 目录里的一条策略: 清单 + Lua 源码 + 来源。
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub manifest: StrategyManifest,
    pub code: String,
    pub source: Source,
}

/// 内置示例(编译期嵌入): (策略 id, 清单 TOML, Lua 源码)。
const BUILTIN: &[(&str, &str, &str)] = &[
    (
        "shannon_spot_grid",
        include_str!("../../../../strategies/spot/shannon_spot_grid.toml"),
        include_str!("../../../../strategies/spot/shannon_spot_grid.lua"),
    ),
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
];

/// 内置策略 id 是否为保留名。
fn is_builtin_id(id: &str) -> bool {
    BUILTIN.iter().any(|(bid, _, _)| *bid == id)
}

/// 统一策略目录 = 内置示例 ∪ 用户自写(磁盘扫描, 按 id 去重, 内置 id 为保留名)。
pub fn all() -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for (id, manifest_toml, code) in BUILTIN {
        let manifest = StrategyManifest::parse(manifest_toml)
            .expect("内置清单解析失败(编译期资产, 应恒可解析)");
        debug_assert_eq!(manifest.id, *id, "内置清单 id 与注册 id 不一致");
        out.push(CatalogEntry { manifest, code: code.to_string(), source: Source::Builtin });
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
    let (entries, _) = scan_user_with_problems();
    entries
}

/// 同 [`scan_user`], 但同时返回被跳过条目的问题清单(按 id 可查)。
fn scan_user_with_problems() -> (Vec<CatalogEntry>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut problems: Vec<(String, String)> = Vec::new();
    for market in ["spot", "futures"] {
        let dir = crate::commands::strategies_dir().join(market);
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
            with_manifest.push(file_stem.clone());
            out.push(CatalogEntry { manifest, code, source: Source::User });
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
                },
                code,
                source: Source::User,
            });
        }
    }
    (out, problems)
}

/// 查询某策略 id 在扫描期被跳过/拒绝的原因(若有)。供加载路径把"静默缺失"升级为明确报错。
pub fn problem_for(id: &str) -> Option<String> {
    scan_user_with_problems().1.into_iter().find(|(pid, _)| pid == id).map(|(_, m)| m)
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
            .any(|e| e.manifest.id == "shannon_spot_grid" && e.source == Source::Builtin));
        assert!(entries
            .iter()
            .any(|e| e.manifest.id == "paired_grid" && e.source == Source::Builtin));
        assert!(entries
            .iter()
            .any(|e| e.manifest.id == "shannon_spot_grid" && e.code.contains("on_tick")));
    }

    #[test]
    fn find_works() {
        assert!(find("shannon_spot_grid").is_some());
        assert!(find("paired_grid").is_some());
        assert!(find("no-such-strategy").is_none());
    }
}
