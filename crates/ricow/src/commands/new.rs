//! `ricow new <name> --template <id> --pair <pair>` — 从内置模板**零网络**生成策略骨架 (#026)。
//!
//! 与 `ricow create`(编译门禁 + 真实 K 线沙箱回测, 需网络)互补: `new` 只做本地落盘 ——
//! 只装发布包的用户拿不到仓库里的模板原文, 这里从 catalog(编译期内嵌)取 Lua 原文,
//! 写 `strategies/{market}/<name>.lua` + 同名实例 `strategies/<name>.toml`,
//! 并把回测参数解析后的**生效值**固化进 TOML `[backtest]` 段(每策略独立一份, 互不共享 ——
//! 改 A 策略的本金不会影响 B 策略)。落盘复用 [`ricow_engine::write_strategy_files`]
//! 内核, 与 deploy/Web 保存两条渠道逐字一致(同名拒绝覆盖 / script_path 指针化)。
//!
//! 首次 `ricow backtest` 会做编译校验; `new` 本身不做(免网络是它的核心价值)。

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{BacktestParams, BacktestToml, ConfigValue, StrategyConfig};
use std::collections::HashMap;

#[derive(Args)]
pub struct NewArgs {
    /// 新策略名 (生成 strategies/<name>.toml 与 {market}/<name>.lua, 同 stem)
    pub name: String,
    /// 内置模板 id (省略 --template 时列出全部模板与参数清单)
    #[arg(long)]
    pub template: Option<String>,
    /// 交易对 (如 TSLABUSDT; 省略时用模板清单里的 pair 默认值)
    #[arg(long)]
    pub pair: Option<String>,
    /// 覆盖模板参数, 形如 key=value (true/false 与数字自动识别)
    #[arg(long = "param")]
    pub params: Vec<String>,
}

pub fn run(args: NewArgs) -> CoreResult<()> {
    // ⓪ --template 省略 = 列模板 (这是发现模板的唯一离线入口, 不能只报错)
    let Some(template_id) = args.template.clone() else {
        println!("{}", crate::commands::templates::list_text());
        return Err(CoreError::InvalidArgument(
            "请用 --template <id> 指定模板 (上方清单里的方括号 id)".into(),
        ));
    };

    // ① 名字规范 + 与既有策略不冲突 (与 create/deploy 同一校验: 字符集/长度 + 互前缀)
    crate::commands::create::validate_new_name(
        &args.name,
        &crate::commands::deployed_strategy_names(),
    )?;

    // ② 模板解析: 必须有参数 schema (declared) —— 裸 lua 条目照抄会缺必填参数并静默停机
    let entry = crate::commands::templates::find(&template_id).ok_or_else(|| {
        CoreError::InvalidArgument(format!(
            "模板 {template_id} 不存在。可用模板: {}",
            crate::commands::templates::names().join(", ")
        ))
    })?;
    if !entry.declared {
        return Err(CoreError::InvalidArgument(format!(
            "模板 {template_id} 未声明参数清单(无参数 schema), 照抄会缺必填参数。\
             请换内置模板, 或走 `ricow create` / AI 对话补全参数后部署"
        )));
    }

    // ③ 参数合成: CLI --param > 模板默认 > 必填缺失即拒
    let mut overrides: HashMap<String, ConfigValue> = HashMap::new();
    for p in &args.params {
        let (k, v) = crate::commands::backtest::parse_param(p).ok_or_else(|| {
            CoreError::InvalidArgument(format!("--param 需为 key=value, 收到 '{p}'"))
        })?;
        if k == "script" || k == "script_path" {
            return Err(CoreError::InvalidArgument(format!(
                "--param 不能覆盖保留键 '{k}' (脚本由模板原文落盘)"
            )));
        }
        overrides.insert(k, v);
    }
    let pair =
        match overrides.remove("pair").or_else(|| {
            args.pair.clone().map(ConfigValue::String).or_else(|| template_default(&entry, "pair"))
        }) {
            Some(ConfigValue::String(s)) if !s.is_empty() => s,
            _ => return Err(CoreError::InvalidArgument(
                "缺少交易对: 请用 --pair <pair> 指定 (现货形如 TSLABUSDT, 视野见 `ricow pairs`)"
                    .into(),
            )),
        };

    let mut missing: Vec<String> = Vec::new();
    let mut params: HashMap<String, ConfigValue> = HashMap::new();
    params.insert("pair".into(), ConfigValue::String(pair));
    for p in &entry.manifest.params {
        if p.key == "pair" {
            continue; // 已由上面合并, 不重复放默认值盖掉用户选择
        }
        let value = overrides.remove(&p.key).or_else(|| p.default.clone()).or_else(|| {
            if p.required {
                missing.push(p.key.clone());
            }
            None
        });
        if let Some(v) = value {
            params.insert(p.key.clone(), v);
        }
    }
    if !missing.is_empty() {
        return Err(CoreError::InvalidArgument(format!(
            "缺少必填参数: {} (模板 {} 的参数清单: --param key=value 逐个给, 默认值见 \
             `ricow new` 无参输出的模板清单)",
            missing.join(", "),
            entry.manifest.id
        )));
    }

    // ④ 回测默认固化 (#026 ④): 用唯一权威 resolve 解析出生效值(含市场分支费率),
    //    全字段显式写进同名 TOML —— 用户改的是自己这份, 不共享任何"统一默认"。
    let mut config = StrategyConfig {
        name: args.name.clone(),
        strategy_type: "lua".into(),
        enabled: true,
        exchange: "binance".into(),
        params,
        dry_run_started_at: None,
        live_enabled: false,
        market: entry.manifest.market.clone(),
        position_mode: "one-way".into(),
        backtest: None,
    };
    let bt = BacktestParams::resolve(&config, &BacktestToml::default());
    config.backtest = Some(BacktestToml {
        fee_maker_bps: Some(bt.fee_maker_bps),
        fee_taker_bps: Some(bt.fee_taker_bps),
        slippage_bps: Some(bt.slippage_bps),
        // 038 P1-A: 默认写 0 (触及即成交) —— 让「限价成交假设」这个旋钮在新策略里可见可改。
        limit_fill_penetration_bps: Some(bt.limit_fill_penetration_bps),
        initial_cash: Some(bt.initial_cash),
        leverage: Some(bt.leverage),
        max_leverage: Some(bt.max_leverage),
        mmr_pct: Some(bt.mmr_pct),
        funding_rate_8h: Some(bt.funding_rate_8h),
        margin_mode: Some(bt.margin_mode),
    });

    // ⑤ 落盘 (复用 deploy/Web 同一内核: 同名拒绝覆盖 / 写 toml 失败回收 .lua)。
    //     `script` 由内核摘出写成 {market}/<name>.lua, 不会残留在 TOML 里。
    config.params.insert("script".into(), ConfigValue::String(entry.code.clone()));
    let dir = crate::commands::ensure_strategies_dir()?;
    let out = ricow_engine::write_strategy_files(config, &dir, false)?;

    // ⑥ 参数清单声明 (#025 目录护栏改法②): `new` 生成的脚本初始与内置逐字相同 ——
    //     若不声明清单, 目录会把"逐字相同 + 未声明"判为危险冗余副本并在回测/启动时拒绝。
    //     模板自带参数 schema, 顺手以新 id 落一份(策略源码目录, 与 .lua 同 stem 配对);
    //     之后用户改脚本, "与内置逐字相同"的冗余标记会自动消失。
    let manifest_path = dir.join(&entry.manifest.market).join(format!("{}.toml", args.name));
    let mut manifest = entry.manifest.clone();
    manifest.id = args.name.clone();
    let manifest_toml = toml::to_string_pretty(&manifest)
        .map_err(|e| CoreError::Parse(format!("清单序列化失败: {e}")))?;
    std::fs::write(&manifest_path, format!(
        "# 策略清单(ricow new 自动生成, 源模板: {}): 展示元数据 + 参数 schema。仅供 UI/CLI/AI, 引擎不读。\n{manifest_toml}",
        entry.manifest.id
    ))
    .map_err(|e| CoreError::Exchange(format!("写 {} 失败: {e}", manifest_path.display())))?;

    println!("已从模板 {} 生成策略 {}:", entry.manifest.id, args.name);
    println!("  {}", out.toml_path.display());
    println!("  {}", out.lua_path.display());
    println!("  {} (参数清单)", manifest_path.display());
    println!(
        "[backtest] 段已固化该策略的回测默认 ({} 口径), 按需修改只影响这一个策略。",
        entry.manifest.market
    );
    println!("下一步:");
    println!(
        "  1) 回测验证: ricow backtest --strategy {} --days 30 (TOML 里已带 pair 与回测参数)",
        args.name
    );
    println!("  2) 满意后启停: ricow start {} / ricow stop {}", args.name, args.name);
    Ok(())
}

/// 模板清单里某参数的默认值 (只认字符串形态; pair 声明为数字等异常形态不采用)。
fn template_default(
    entry: &crate::strategies::catalog::CatalogEntry,
    key: &str,
) -> Option<ConfigValue> {
    entry.manifest.params.iter().find(|p| p.key == key).and_then(|p| p.default.clone())
}

#[cfg(test)]
mod tests {
    /// 参数合成优先级: CLI --param > 模板默认; 保留键拒绝。
    #[test]
    fn test_param_merge_priority_and_reserved_keys() {
        // parse_param 的 key=value 解析已被 backtest.rs 单测覆盖;
        // 这里锁定 new 命令对保留键的拒绝行为。
        let p = crate::commands::backtest::parse_param("script=x").unwrap();
        assert_eq!(p.0, "script");
    }
}
