//! `ricow optimize` (044) — 参数优化: 网格/随机搜索包 `ricow backtest`, 汇总打分表, 可选写回 TOML。
//!
//! 纯 CLI 层: 每组组合构造一份 [`BacktestArgs`] 复用 `run_backtest` 全链路 (参数三层覆盖/导出
//! 语义零改动); 无引擎改动。竞品对标 (Freqtrade hyperopt 简化版) —— 不做智能搜索与过拟合检测。

use std::collections::{HashMap, HashSet};

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::ConfigValue;

use crate::commands::backtest::{parse_param, BacktestArgs, BacktestRunSpec};

/// 单组合组合数护栏: 每组 = 一次全窗口回测, 超 100 组拒绝 (提示收窄网格)。
const MAX_COMBOS: usize = 100;

#[derive(Args)]
pub struct OptimizeArgs {
    /// 已部署策略名 (strategies/<name>.toml; 直跑模式不支持 —— 无参数基线可回填)
    #[arg(long)]
    pub strategy: String,
    /// 参数候选, 格式 key=v1,v2,v3 (可重复; 多键 = 网格笛卡尔积)
    #[arg(long = "param")]
    pub params: Vec<String>,
    /// 回测天数 (默认 90; 与 backtest 同语义)
    #[arg(long)]
    pub days: Option<u32>,
    /// 窗口起点 (YYYY-MM-DD, UTC), 与 --end 配套
    #[arg(long)]
    pub start: Option<String>,
    /// 窗口终点 (YYYY-MM-DD, UTC, 不含); 缺省 = 现在
    #[arg(long)]
    pub end: Option<String>,
    /// K 线间隔 (1m/5m/15m/1h/4h/1d, 默认 1h)
    #[arg(long)]
    pub interval: Option<String>,
    /// 搜索方式: grid(默认, 全笛卡尔积) | random(每维在候选中随机抽)
    #[arg(long = "search", default_value = "grid")]
    pub search: String,
    /// random 模式抽样组数 (默认 20)
    #[arg(long, default_value_t = 20)]
    pub samples: usize,
    /// 排序指标: net_pnl(默认) | sharpe | annual_return
    #[arg(long, default_value = "net_pnl")]
    pub metric: String,
    /// 把最优组合写回 strategies/<name>.toml (外科式, 只动命中键)
    #[arg(long = "write-back")]
    pub write_back: bool,
}

/// 一维候选: 键 + 候选值列表。
fn parse_candidates(spec: &str) -> CoreResult<(String, Vec<ConfigValue>)> {
    let (key, values) = spec.split_once('=').ok_or_else(|| {
        CoreError::InvalidArgument(format!("--param 需 key=v1,v2 形态: {spec:?}"))
    })?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(CoreError::InvalidArgument(format!("--param 键为空: {spec:?}")));
    }
    let cands: Vec<ConfigValue> = values
        .split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| parse_param(&format!("{key}={v}")).map(|(_, cv)| cv))
        .collect::<Option<_>>()
        .ok_or_else(|| CoreError::InvalidArgument(format!("--param 值解析失败: {spec:?}")))?;
    if cands.is_empty() {
        return Err(CoreError::InvalidArgument(format!("--param 无候选值: {spec:?}")));
    }
    Ok((key, cands))
}

/// 网格展开: 各键候选值笛卡尔积 (保持键顺序; 每组 = 有序键值对)。
pub(crate) fn expand_grid(dims: &[(String, Vec<ConfigValue>)]) -> Vec<Vec<(String, ConfigValue)>> {
    let mut acc: Vec<Vec<(String, ConfigValue)>> = vec![Vec::new()];
    for (key, cands) in dims {
        let mut next = Vec::new();
        for base in &acc {
            for v in cands {
                let mut combo = base.clone();
                combo.push((key.clone(), v.clone()));
                next.push(combo);
            }
        }
        acc = next;
    }
    acc
}

/// 组合去重键 (参数渲染, 同键序)。
fn combo_key(combo: &[(String, ConfigValue)]) -> String {
    combo
        .iter()
        .map(|(k, v)| match v {
            ConfigValue::String(s) => format!("{k}={s}"),
            ConfigValue::Float(f) => format!("{k}={f}"),
            ConfigValue::Integer(i) => format!("{k}={i}"),
            ConfigValue::Boolean(b) => format!("{k}={b}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// random 搜索: 每维在候选中随机抽, 去重后凑 `samples` 组 (候选耗尽即止)。
/// 用简单 LCG (无 rand 依赖; 优化不需要密码学随机)。
pub(crate) fn sample_random(
    dims: &[(String, Vec<ConfigValue>)],
    samples: usize,
    seed: u64,
) -> Vec<Vec<(String, ConfigValue)>> {
    let total: usize = dims.iter().map(|(_, c)| c.len()).product();
    let want = samples.min(total);
    let mut state = seed | 1;
    let mut next = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut guard = 0usize;
    while out.len() < want && guard < want * 100 {
        guard += 1;
        let combo: Vec<(String, ConfigValue)> = dims
            .iter()
            .map(|(k, c)| {
                let i = next() % c.len();
                (k.clone(), c[i].clone())
            })
            .collect();
        if seen.insert(combo_key(&combo)) {
            out.push(combo);
        }
    }
    out
}

/// 打分行: 汇总表一行。
struct ScoreRow {
    combo: Vec<(String, ConfigValue)>,
    net_pnl: rust_decimal::Decimal,
    trades: u64,
    max_drawdown: rust_decimal::Decimal,
    sharpe: Option<f64>,
    win_rate: f64,
    error: Option<String>,
}

fn metric_of(r: &ScoreRow, metric: &str) -> f64 {
    match metric {
        "sharpe" => r.sharpe.unwrap_or(f64::NEG_INFINITY),
        // net_pnl 也接受 annual_return 名义 (报告字段省略, 用净盈亏等价排序场景不存在 ——
        // annual_return 未持久化在 BacktestReport 中, 按 net_pnl 排序并提示)。
        "annual_return" | "net_pnl" => {
            r.net_pnl.to_string().parse::<f64>().unwrap_or(f64::NEG_INFINITY)
        }
        _ => f64::NEG_INFINITY,
    }
}

pub async fn run(args: OptimizeArgs) -> CoreResult<()> {
    let metric = args.metric.as_str();
    if !matches!(metric, "net_pnl" | "sharpe" | "annual_return") {
        return Err(CoreError::InvalidArgument(format!(
            "未知指标 {metric:?} (可选 net_pnl/sharpe/annual_return)"
        )));
    }
    if args.search != "grid" && args.search != "random" {
        return Err(CoreError::InvalidArgument(format!(
            "--search 仅支持 grid|random, 收到 {:?}",
            args.search
        )));
    }

    // 基线策略必须已部署 (有 TOML)。
    let toml_path =
        crate::commands::ensure_strategies_dir()?.join(format!("{}.toml", args.strategy));
    if !toml_path.exists() {
        return Err(CoreError::InvalidArgument(format!(
            "strategies/{}.toml 不存在 —— optimize 仅支持已部署策略 (参数要写回 TOML)",
            args.strategy
        )));
    }

    // 展开维度 → 组合。
    let mut dims: Vec<(String, Vec<ConfigValue>)> = Vec::new();
    for spec in &args.params {
        let (k, c) = parse_candidates(spec)?;
        dims.push((k, c));
    }
    if dims.is_empty() {
        return Err(CoreError::InvalidArgument("至少给一个 --param key=v1,v2".into()));
    }
    let combos = if args.search == "random" {
        sample_random(
            &dims,
            args.samples,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(42),
        )
    } else {
        expand_grid(&dims)
    };
    if combos.len() > MAX_COMBOS {
        return Err(CoreError::InvalidArgument(format!(
            "组合数 {} 超过护栏 {MAX_COMBOS} (每组 = 一次全窗口回测) —— 请收窄候选或用 --search random --samples N",
            combos.len()
        )));
    }
    println!("优化: {} — {} 组组合 ({}, 指标 {metric})", args.strategy, combos.len(), args.search);

    // 逐组回测 (同一窗口; 失败组合记录不中断)。
    let mut rows: Vec<ScoreRow> = Vec::new();
    for (i, combo) in combos.iter().enumerate() {
        tracing::info!(target: "optimize", "第 {}/{} 组: {}", i + 1, combos.len(), combo_key(combo));
        let bt_args = BacktestArgs {
            strategy: args.strategy.clone(),
            pair: None,
            days: args.days,
            start: args.start.clone(),
            end: args.end.clone(),
            interval: args.interval.clone(),
            klines_market: None,
            script: None,
            params: combo.iter().map(|(k, v)| format!("{k}={}", config_value_str(v))).collect(),
            fee: None,
            fee_maker: None,
            fee_taker: None,
            slippage_bps: None,
            limit_fill_penetration_bps: None,
            cash: None,
            leverage: None,
            max_leverage: None,
            mmr_pct: None,
            funding_rate: None,
            market: None,
            position_mode: None,
            sensitivity: None,
            sensitivity_fee: None,
            close_at_end: false,
            export_dir: None,
        };
        // 走与 CLI 同一内核 (不写 run card —— 逐组寻优不产 100 张证据卡)。
        let spec = BacktestRunSpec::from_cli_args(crate::commands::project_root(), bt_args);
        let row = match crate::commands::backtest::run_backtest_inner(&spec).await {
            Ok(out) => {
                let r = &out.report;
                ScoreRow {
                    combo: combo.clone(),
                    net_pnl: r.net_pnl,
                    trades: r.total_trades,
                    max_drawdown: r.max_drawdown,
                    sharpe: r.sharpe,
                    win_rate: r.win_rate,
                    error: None,
                }
            }
            Err(e) => ScoreRow {
                combo: combo.clone(),
                net_pnl: Default::default(),
                trades: 0,
                max_drawdown: Default::default(),
                sharpe: None,
                win_rate: 0.0,
                error: Some(e.to_string()),
            },
        };
        println!(
            "  [{}/{}] {} → {}",
            i + 1,
            combos.len(),
            combo_key(combo),
            row.error.as_deref().map(|e| format!("失败: {e}")).unwrap_or_else(|| {
                format!("net_pnl={} trades={} dd={}", row.net_pnl, row.trades, row.max_drawdown)
            })
        );
        rows.push(row);
    }

    // 打分表 (失败行沉底)。
    rows.sort_by(|a, b| {
        let fa = a.error.is_some() as u8;
        let fb = b.error.is_some() as u8;
        fa.cmp(&fb).then(metric_of(b, metric).total_cmp(&metric_of(a, metric)))
    });
    println!(
        "\n{:<40} {:>14} {:>7} {:>12} {:>8} {:>7}",
        "参数组合", "净盈亏", "成交数", "最大回撤", "夏普", "胜率%"
    );
    println!("{}", "-".repeat(92));
    for r in &rows {
        let combo = combo_key(&r.combo);
        match &r.error {
            Some(e) => println!("{:<40} 失败: {e}", truncate(&combo, 38)),
            None => println!(
                "{:<40} {:>14} {:>7} {:>12} {:>8} {:>7.1}",
                truncate(&combo, 38),
                r.net_pnl,
                r.trades,
                r.max_drawdown,
                r.sharpe.map(|s| format!("{s:.2}")).unwrap_or_else(|| "-".into()),
                r.win_rate * 100.0
            ),
        }
    }

    // 最优行 + 写回。
    let best = rows.iter().find(|r| r.error.is_none());
    match best {
        None => println!("\n全部组合失败, 无最优可写回"),
        Some(b) => {
            println!("\n最优 (按 {metric}): {}", combo_key(&b.combo));
            if args.write_back {
                let pairs: HashMap<String, String> =
                    b.combo.iter().map(|(k, v)| (k.clone(), config_value_toml(v))).collect();
                let (new_text, changes) = write_back_toml(
                    &std::fs::read_to_string(&toml_path)
                        .map_err(|e| CoreError::InvalidArgument(format!("读 TOML 失败: {e}")))?,
                    &pairs,
                );
                std::fs::write(&toml_path, new_text)
                    .map_err(|e| CoreError::InvalidArgument(format!("写 TOML 失败: {e}")))?;
                for (k, (old, new)) in &changes {
                    println!("  写回 {k}: {old} → {new}");
                }
            } else {
                println!("提示: 加 --write-back 把最优参数写回 strategies/{}.toml", args.strategy);
            }
        }
    }
    Ok(())
}

fn config_value_str(v: &ConfigValue) -> String {
    match v {
        ConfigValue::String(s) => s.clone(),
        ConfigValue::Float(f) => f.to_string(),
        ConfigValue::Integer(i) => i.to_string(),
        ConfigValue::Boolean(b) => b.to_string(),
    }
}

/// TOML 字面量: 字符串加引号, 其余原样。
fn config_value_toml(v: &ConfigValue) -> String {
    match v {
        ConfigValue::String(s) => format!("\"{s}\""),
        other => config_value_str(other),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

/// 外科式写回 (044): 只动 `[strategy.params]` 段内命中的键行, 其余字节原样保留。
/// 返回 (新文本, [(键, (旧值行, 新值行))]); 键不存在时插在段内最后, 无段则在文件尾追加段。
type TomlChanges = Vec<(String, (String, String))>;

pub(crate) fn write_back_toml(
    text: &str,
    pairs: &HashMap<String, String>,
) -> (String, TomlChanges) {
    let mut changes = Vec::new();
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

    // 定位 [strategy.params] 段边界 (段头行到下一个段头之间)。
    let header = lines.iter().position(|l| l.trim() == "[strategy.params]");
    match header {
        Some(start) => {
            let mut end = start + 1;
            while end < lines.len() && !lines[end].trim_start().starts_with('[') {
                end += 1;
            }
            let mut pending: Vec<(String, String)> = Vec::new();
            for (k, new_v) in pairs {
                let mut hit = false;
                for line in lines[start + 1..end].iter_mut() {
                    let t = line.trim();
                    if let Some(eq) = t.find('=') {
                        if t[..eq].trim() == k {
                            let old = line.clone();
                            *line = format!("{k} = {new_v}");
                            changes.push((
                                k.clone(),
                                (old.trim().to_string(), format!("{k} = {new_v}")),
                            ));
                            hit = true;
                            break;
                        }
                    }
                }
                if !hit {
                    pending.push((k.clone(), new_v.clone()));
                }
            }
            for (k, v) in pending {
                let line = format!("{k} = {v}");
                lines.insert(end, line.clone());
                end += 1;
                changes.push((k, ("(新增)".into(), line)));
            }
        }
        None => {
            // 无段: 文件尾追加 (中间补一个空行)。
            if !pairs.is_empty() && lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            lines.push("[strategy.params]".into());
            for (k, v) in pairs {
                let line = format!("{k} = {v}");
                lines.push(line.clone());
                changes.push((k.to_string(), ("(新增)".into(), line)));
            }
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    (out, changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims() -> Vec<(String, Vec<ConfigValue>)> {
        vec![
            ("a".into(), vec![ConfigValue::Float(1.0), ConfigValue::Float(2.0)]),
            (
                "b".into(),
                vec![ConfigValue::Float(10.0), ConfigValue::Float(20.0), ConfigValue::Float(30.0)],
            ),
        ]
    }

    #[test]
    fn grid_expands_cartesian_product() {
        let g = expand_grid(&dims());
        assert_eq!(g.len(), 6, "2×3 = 6 组");
        assert_eq!(combo_key(&g[0]), "a=1,b=10");
        assert_eq!(combo_key(&g[5]), "a=2,b=30");
    }

    #[test]
    fn random_samples_dedupe_and_respect_candidates() {
        let d = dims();
        let s = sample_random(&d, 20, 7);
        assert!(s.len() <= 6, "候选总量 6, 抽样不得超出");
        let uniq: HashSet<_> = s.iter().map(|c| combo_key(c)).collect();
        assert_eq!(uniq.len(), s.len(), "组合须去重");
        for c in &s {
            for (k, v) in c {
                let cand = d.iter().find(|(ck, _)| ck == k).unwrap();
                assert!(
                    cand.1.iter().any(|cv| config_value_str(cv) == config_value_str(v)),
                    "抽样值必须来自候选: {k}={}",
                    config_value_str(v)
                );
            }
        }
    }

    #[test]
    fn parse_candidates_types_and_errors() {
        let (k, c) = parse_candidates("spacing=0.004,0.006").unwrap();
        assert_eq!(k, "spacing");
        assert!(matches!(c[0], ConfigValue::Float(_)));
        let (_, c2) = parse_candidates("flag=true,false").unwrap();
        assert!(matches!(c2[0], ConfigValue::Boolean(true)));
        assert!(parse_candidates("novalue").is_err());
        assert!(parse_candidates("=1,2").is_err());
    }

    #[test]
    fn write_back_hits_inserts_and_appends_section() {
        let pairs: HashMap<String, String> = [
            ("spacing".to_string(), "0.006".to_string()),
            ("new_key".to_string(), "42".to_string()),
        ]
        .into_iter()
        .collect();
        let toml =
            "[strategy]\nname = \"s\"\n\n[strategy.params]\npair = \"SOLUSDT\"\nspacing = 0.004\n";
        let (out, changes) = write_back_toml(toml, &pairs);
        assert!(out.contains("spacing = 0.006"), "命中键应改值");
        assert!(out.contains("new_key = 42"), "缺失键应插入段内");
        assert!(out.contains("pair = \"SOLUSDT\""), "未命中键原样");
        assert!(changes.iter().any(|(k, (o, _))| k == "spacing" && o.contains("0.004")));

        // 无段: 文件尾追加段。
        let (out2, ch2) = write_back_toml("[strategy]\nname = \"s\"\n", &pairs);
        assert!(out2.contains("[strategy.params]"));
        assert!(out2.contains("spacing = 0.006"));
        assert_eq!(ch2.len(), 2);
    }
}
