//! `ricow list/status/info/fills` — 实例与成交查看 (008)。
//!
//! 数据来源: daemon 控制通道 (运行中实例) ∪ 实例台账 `run/<name>.json` (历史退出)
//! ∪ 已部署清单 `strategies/*.toml` (从未启动的策略)。三者并集才是"有哪些策略"的完整答案。

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_engine::{is_owned, ownership_prefix};
use ricow_strategy::Database;
use std::fmt::Write as _;

use crate::supervisor::client::Client;
use crate::supervisor::ledger::{self, InstanceRecord};
use crate::supervisor::proto::{InstanceView, Request};

#[derive(Args)]
pub struct ListArgs {}

#[derive(Args)]
pub struct StatusArgs {
    /// 策略名 (缺省列出全部)
    pub name: Option<String>,
}

#[derive(Args)]
pub struct InfoArgs {
    /// 策略名
    pub name: String,
}

#[derive(Args)]
pub struct FillsArgs {
    /// 策略名 (缺省为全部策略)
    pub name: Option<String>,
    /// 最多显示条数
    #[arg(long, default_value_t = 20)]
    pub limit: i64,
}

/// 实例视图: 优先问 daemon (含运行中); daemon 不在时退回台账 (仅历史)。
pub(crate) async fn views(root: &std::path::Path) -> Vec<InstanceView> {
    if let Ok(client) = Client::connect(root).await {
        if let Ok(data) = client.call_ok(Request::List).await {
            let raw = data.get("instances").cloned().unwrap_or(serde_json::json!([]));
            if let Ok(v) = serde_json::from_value::<Vec<InstanceView>>(raw) {
                return v;
            }
        }
    }
    ledger::list_instances(root).into_iter().map(view_from_record).collect()
}

fn view_from_record(rec: InstanceRecord) -> InstanceView {
    InstanceView {
        name: rec.name,
        running: false,
        pid: rec.pid,
        started_at: rec.started_at,
        uptime_secs: None,
        mode: rec.mode,
        pair: rec.pair,
        market: rec.market,
        last_exit: rec.last_exit,
        last_exit_at: rec.last_exit_at,
        last_reason: rec.last_reason,
    }
}

pub async fn list(_args: ListArgs) -> CoreResult<()> {
    print_table().await
}

pub async fn status(args: StatusArgs) -> CoreResult<()> {
    match args.name {
        None => print_table().await,
        Some(name) => info(InfoArgs { name }).await,
    }
}

pub(crate) async fn format_table() -> CoreResult<String> {
    let mut out = String::new();
    let root = crate::commands::project_root();
    let _ = ledger::ensure_dirs(&root);
    let live = views(&root).await;
    let deployed = crate::commands::deployed_strategy_names();

    let mut names: Vec<String> = live.iter().map(|v| v.name.clone()).collect();
    for n in &deployed {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    names.sort();
    names.dedup();

    if names.is_empty() {
        line!(out, "无策略: strategies/ 下无 TOML, 也无实例台账");
        line!(out, "提示: 部署策略后执行 ricow start <name>, 或先 ricow daemon start");
        return Ok(out);
    }

    line!(
        out,
        "{:<22} {:<20} {:<9} {:<10} {:>7} {:<9} {}",
        "策略",
        "状态",
        "模式",
        "交易对",
        "PID",
        "运行时长",
        "备注"
    );
    line!(out, "{}", "-".repeat(96));

    let (mut running, mut exited, mut idle) = (0usize, 0usize, 0usize);
    for name in &names {
        let view = live.iter().find(|v| &v.name == name);
        let cfg = crate::commands::read_strategy_config(name);
        let is_deployed = deployed.contains(name);

        let (status, mode, pair, pid, uptime) = match view {
            Some(v) if v.running => (
                "运行中".to_string(),
                v.mode.as_deref().map(mode_text).unwrap_or_else(|| "未知".into()),
                v.pair.clone().unwrap_or_else(|| "-".into()),
                v.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                v.uptime_secs.map(fmt_uptime).unwrap_or_else(|| "-".into()),
            ),
            Some(v) => {
                let st = match v.last_exit {
                    Some(code) => format!("已中止 (exit={code})"),
                    None => "未运行".to_string(),
                };
                (
                    st,
                    v.mode.as_deref().map(mode_text).unwrap_or_else(|| "未知".into()),
                    v.pair.clone().unwrap_or_else(|| "-".into()),
                    v.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                    "-".to_string(),
                )
            }
            None => (
                "未运行".to_string(),
                cfg.as_ref().map(|_| "Dry Run".to_string()).unwrap_or_else(|| "未知".into()),
                cfg.as_ref().and_then(|c| c.get_str("pair")).unwrap_or("-").to_string(),
                "-".to_string(),
                "-".to_string(),
            ),
        };

        match (view.map(|v| v.running).unwrap_or(false), view.and_then(|v| v.last_exit)) {
            (true, _) => running += 1,
            (false, Some(_)) => exited += 1,
            (false, None) => idle += 1,
        }

        let mut notes: Vec<String> = Vec::new();
        if let Some(v) = view {
            if !v.running {
                if let Some(r) = &v.last_reason {
                    notes.push(r.clone());
                }
            }
        }
        if !is_deployed {
            notes.push("无 TOML (直跑实例)".into());
        }
        if let Some(c) = &cfg {
            if c.live_enabled {
                // 实盘已可运行 (011): 声明只表示允许, 真正进实盘还需命令行 --live
                notes.push("配置声明实盘 (启动需 --live)".into());
            }
        }

        line!(
            out,
            "{:<22} {:<20} {:<9} {:<10} {:>7} {:<9} {}",
            name,
            status,
            mode,
            pair,
            pid,
            uptime,
            notes.join("; ")
        );
    }

    line!(
        out,
        "\n合计 {} 个: {} 运行中 / {} 已中止 / {} 未运行",
        names.len(),
        running,
        exited,
        idle
    );
    Ok(out)
}

/// 打印版(CLI 入口; 与 AI 工具共用 `format_table`, 口径一致)。
async fn print_table() -> CoreResult<()> {
    print!("{}", format_table().await?);
    Ok(())
}

pub async fn format_info(args: InfoArgs) -> CoreResult<String> {
    let mut out = String::new();
    let root = crate::commands::project_root();
    let _ = ledger::ensure_dirs(&root);
    let live = views(&root).await;
    let view = live.iter().find(|v| v.name == args.name);
    let cfg = crate::commands::read_strategy_config(&args.name);

    if view.is_none() && cfg.is_none() {
        return Err(CoreError::InvalidArgument(format!(
            "未找到策略或实例 {} (strategies/{}.toml 不存在, 也无实例台账)",
            args.name, args.name
        )));
    }

    line!(out, "策略: {}", args.name);
    match view {
        Some(v) if v.running => {
            line!(out, "状态: 运行中");
            line!(
                out,
                "PID: {}  启动: {}  运行时长: {}",
                v.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                v.started_at.clone().unwrap_or_else(|| "-".into()),
                v.uptime_secs.map(fmt_uptime).unwrap_or_else(|| "-".into())
            );
            line!(
                out,
                "模式: {}",
                v.mode.as_deref().map(mode_text).unwrap_or_else(|| "未知".into())
            );
            line!(
                out,
                "交易对: {}  市场: {}",
                v.pair.clone().unwrap_or_else(|| "-".into()),
                v.market.clone().unwrap_or_else(|| "-".into())
            );
        }
        Some(v) => {
            line!(out, "状态: 未运行");
            line!(
                out,
                "上次退出: exit={} 时间={} 原因={}",
                v.last_exit.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
                v.last_exit_at.clone().unwrap_or_else(|| "-".into()),
                v.last_reason.clone().unwrap_or_else(|| "-".into())
            );
        }
        None => line!(out, "状态: 未运行 (无实例台账, 从未经 ricow start 启动)"),
    }

    match &cfg {
        Some(c) => {
            line!(
                out,
                "配置: type={} enabled={} market={} position_mode={} live_enabled={}",
                c.strategy_type,
                c.enabled,
                c.market,
                c.position_mode,
                c.live_enabled
            );
            if c.live_enabled {
                line!(out, 
                    "实盘声明: live_enabled=true (实际进实盘还需命令行开关: ricow run --live / ricow start --live)"
                );
            }
        }
        None => line!(out, "配置: 无 TOML (直跑实例, 参数来自命令行)"),
    }

    let db = Database::open(&crate::commands::default_db_path())
        .await
        .map_err(|e| CoreError::InvalidArgument(format!("打开数据库失败: {e}")))?;
    let sid = cfg.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| args.name.clone());
    match db.fill_stats(&sid).await {
        Ok((n, fees, last)) => line!(
            out,
            "成交: {} 笔, 手续费合计 {}, 最近成交 {}",
            n,
            fees,
            last.map(fmt_ts).unwrap_or_else(|| "无".into())
        ),
        Err(e) => line!(out, "成交: 查询失败 ({e})"),
    }
    // 资金费 (014 FR-004): 账户级流水, 与成交口径分开显示 (只有合约有资金费)
    if cfg.as_ref().map(|c| c.market.eq_ignore_ascii_case("futures")).unwrap_or(false) {
        match db.funding_total(None).await {
            Ok((0, _)) => line!(out, "资金费: 0 笔 (账户无资金费流水)"),
            Ok((n, sum)) => line!(out, "资金费: {n} 笔, 合计 {sum} (账户口径; 负 = 净支付)"),
            Err(e) => line!(out, "资金费: 查询失败 ({e})"),
        }
    }
    line!(out, "日志: {}", ledger::log_path(&root, &args.name).display());

    // 账户快照 (FR-011): 声明实盘的策略(主网) 或 正在 demo 运行的实例(测试网) 从交易所实时查询;
    // 端点与凭据按**模式**取 —— 绝不把 demo key 用到主网、反之亦然; 查询失败如实标注, 不显示陈旧值。
    let snap_mode = match view.and_then(|v| v.mode.as_deref()) {
        Some("demo") => Some(crate::commands::Mode::Demo),
        _ if cfg.as_ref().map(|c| c.live_enabled).unwrap_or(false) => Some(crate::commands::Mode::Live),
        _ => None,
    };
    if let Some(mode) = snap_mode {
        line!(out, "交易所账户快照 (实时查询, {}):", mode.label());
        match live_account_snapshot(&args.name, mode).await {
            Ok(lines) => {
                for l in lines {
                    line!(out, "  {l}");
                }
            }
            Err(e) => {
                line!(out, "  查询失败: {e}");
                line!(out, "  注意: 上行失败表示当前状态未知, 请勿据此判断挂单/持仓已清理");
            }
        }
    }
    if !view.map(|v| v.running).unwrap_or(false)
        && cfg.as_ref().map(|c| c.live_enabled).unwrap_or(false)
    {
        line!(out, "提示: 实例未运行; 停机清理的真实结果见 logs/{}.log 与交易所账户", args.name);
    }
    Ok(out)
}

/// 打印版(CLI 入口; 与 AI 工具共用 `format_info`, 口径一致)。
pub async fn info(args: InfoArgs) -> CoreResult<()> {
    print!("{}", format_info(args).await?);
    Ok(())
}

/// 交易所实时账户快照 (实盘 `info` 用): 余额 + 持仓(现货口径) + 挂单(标注是否本策略归属)。
///
/// 任一查询失败即返回 `Err` (调用方标注"查询失败"), 不拼接部分陈旧数据。
async fn live_account_snapshot(name: &str, mode: crate::commands::Mode) -> CoreResult<Vec<String>> {
    let cfg = crate::commands::read_strategy_config(name).ok_or_else(|| {
        CoreError::InvalidArgument(format!("策略 {name} 无 TOML, 无法确定交易对"))
    })?;
    let pair = cfg.get_str("pair").unwrap_or("ETHUSDT").to_string();
    let is_futures = cfg.market.eq_ignore_ascii_case("futures");
    let exchange = crate::commands::bn_signed_exchange_mode(&cfg.market, mode)?;
    let markets = exchange.get_markets().await?;
    let m = markets
        .iter()
        .find(|m| m.symbol.eq_ignore_ascii_case(&pair))
        .ok_or_else(|| CoreError::InvalidArgument(format!("交易所列表未含 {pair}")))?;

    let quote = exchange.get_balance(&m.quote_asset).await?;
    let mut out = Vec::new();
    if is_futures {
        out.push(format!(
            "合约账户: {} 可用 {} (余额 {})",
            m.quote_asset, quote.free, quote.locked
        ));
        let positions = exchange.get_positions_directional(&pair).await?;
        if positions.is_empty() {
            out.push(format!("持仓: 无 ({pair})"));
        } else {
            for p in &positions {
                // 距强平距离 (014 FR-005): 交易所未给强平价 → 标"未知", 不猜
                let dist = p.liquidation_price.and_then(|liq| {
                    let mark = if p.mark_price > rust_decimal::Decimal::ZERO {
                        p.mark_price
                    } else {
                        p.entry_price
                    };
                    ricow_engine::liquidation_distance(mark, liq, p.side)
                });
                out.push(format!(
                    "持仓: {:?} {} {} 开仓价 {} 标记价 {} 未实现盈亏 {} 距强平 {}",
                    p.side,
                    p.size,
                    m.base_asset,
                    p.entry_price,
                    p.mark_price,
                    p.unrealized_pnl,
                    dist.map(|d| format!("{:.2}%", d * 100.0))
                        .unwrap_or_else(|| "未知".to_string())
                ));
            }
        }
    } else {
        let base = exchange.get_balance(&m.base_asset).await?;
        out.push(format!(
            "余额: {} 可用 {} (锁定 {}) / {} 可用 {} (锁定 {})",
            m.base_asset, base.free, base.locked, m.quote_asset, quote.free, quote.locked
        ));
        out.push(format!("持仓: {} {} (现货口径 = {})", base.free, m.base_asset, m.base_asset));
    }

    let orders = exchange.get_open_orders(&pair).await?;
    if orders.is_empty() {
        out.push("挂单: 无".into());
    } else {
        let prefix = ownership_prefix(&cfg.name);
        out.push(format!("挂单: {} 笔 (交易对 {pair})", orders.len()));
        for o in &orders {
            out.push(format!(
                "  {} {:?} {} @ {} 数量 {} 已成交 {} [本策略归属: {}]",
                o.client_order_id,
                o.side,
                o.pair,
                o.price,
                o.size,
                o.filled_size,
                if is_owned(&o.client_order_id, &prefix) { "是" } else { "否 (不撤)" }
            ));
        }
    }
    Ok(out)
}

pub async fn format_fills(args: FillsArgs) -> CoreResult<String> {
    let mut out = String::new();
    let db = Database::open(&crate::commands::default_db_path())
        .await
        .map_err(|e| CoreError::InvalidArgument(format!("打开数据库失败: {e}")))?;
    let sid = args.name.as_deref().map(|n| {
        crate::commands::read_strategy_config(n).map(|c| c.name).unwrap_or_else(|| n.to_string())
    });
    let rows = db
        .recent_fills(sid.as_deref(), args.limit)
        .await
        .map_err(|e| CoreError::InvalidArgument(format!("查询成交失败: {e}")))?;

    if rows.is_empty() {
        match &sid {
            Some(id) => line!(out, "无成交记录 (strategy_id={id})"),
            None => line!(out, "无成交记录"),
        }
        return Ok(out);
    }

    line!(
        out,
        "{:<20} {:<22} {:<10} {:<5} {:>14} {:>16} {:>12}",
        "时间",
        "策略",
        "交易对",
        "方向",
        "价格",
        "数量",
        "手续费"
    );
    line!(out, "{}", "-".repeat(104));
    for r in rows {
        line!(
            out,
            "{:<20} {:<22} {:<10} {:<5} {:>14} {:>16} {:>12}",
            fmt_ts(r.timestamp),
            r.strategy_id,
            r.pair,
            r.side,
            r.fill_price,
            r.fill_size,
            r.fee
        );
    }
    Ok(out)
}

/// 打印版(CLI 入口; 与 AI 工具共用 `format_fills`, 口径一致)。
pub async fn fills(args: FillsArgs) -> CoreResult<()> {
    print!("{}", format_fills(args).await?);
    Ok(())
}

/// 模式显示: 只反映实际运行器 (当前 run 仅 Dry Run)。
pub(crate) fn mode_text(mode: &str) -> String {
    match mode {
        "dry_run" => "Dry Run".into(),
        "live" => "实盘".into(),
        "demo" => "测试网模拟盘(demo)".into(),
        other => other.to_string(),
    }
}

fn fmt_uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

fn fmt_ts(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".into())
}
