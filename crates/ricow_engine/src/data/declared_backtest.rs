//! 声明驱动回测的装配 (028 T024 / US1): 宿主 → 策略(顶层声明即装载) → 序列驱动 → 主时钟。
//!
//! 为什么单独一层: 回测有**虚拟时钟**, 与 Dry Run/实盘的墙钟装配([[`DrivenRuntime`]])不能共用一个
//! `assemble`; 但两侧的"可见性判据 / bar 去重 / 预热不派发"完全是同一份 [`SeriesDriver`] 实现,
//! 所以"回测里看到什么"与"实盘里看到什么"不会因为两套代码而分歧。
//!
//! **单时间轴**(审核补充的硬约束): 声明驱动的回测只有**一个**时间轴(第一条驱动序列),
//! 每个刻度内所有序列的成交都发生在这一刻; 各标的的成交参考价 = **该标的自己那条序列**
//! 此刻正在形成 bar 的 `open`(见 `BacktestContext::set_declared_bars`)。交易一个**没有
//! 声明序列**的标的时无自己的 bar → 市价单按既有语义拒单, 不会悄悄用别的标的的价成交。
//!
//! 无前视在这里由两件事共同保证:
//! 1. 宿主 now 固定为**窗口起点** —— 策略声明期(`data:series`)拿到的句柄只有窗口前的历史(预热);
//! 2. 驱动游标从窗口内第一根起 —— 窗口外的 bar 永不派发(见 [`SeriesDriver::load`])。

use ricow_strategy::Strategy;
use std::sync::Arc;

use ricow_core::{Balance, CoreError, CoreResult, Kline};
use ricow_strategy::{BacktestReport, SeriesDecl, StrategyConfig};

use super::host_impl::EngineHost;
use super::series_driver::SeriesDriver;
use super::DataHub;

/// 只读声明: CLI 用它判断"这个策略走声明路径还是旧 `ctx:klines` 路径"。
///
/// 会**实例化一次策略**(脚本顶层要执行才拿得到声明) —— 脚本很小, 代价可忽略。
///
/// `allow_fetch`: 声明期取数是否允许回源补缺口。
/// - `false` = 回测路径(D3: 只读本地库, 可复现; 缺数据直接报错并给 `ricow data pull` 命令);
/// - `true` = `ricow create` 的沙箱门禁(创建流程不该要求用户先手工拉数)。
pub fn declared_series(
    config: &StrategyConfig,
    hub: Arc<DataHub>,
    now_ms: i64,
    allow_fetch: bool,
) -> CoreResult<Vec<SeriesDecl>> {
    let host = Arc::new(EngineHost::new(hub, now_ms, allow_fetch)?);
    let strategy = crate::loader::load_strategy_with_host(config, host)?;
    // ⚠️ 返回**全部**声明(不是只返回驱动声明): 只要策略声明过序列就走数据服务路径。
    // 若这里只认驱动声明, "只声明句柄(`drive=false`)"的策略会被判成"没声明", CLI 静默回落到
    // **联网预取 K 线**的旧路径 —— 既违反 D3(回测只读本地库), 也与"策略自己声明数据"的前提相反。
    Ok(strategy.declarations())
}

/// 声明驱动的回测: 主时钟 = 第一条驱动序列的窗口内 bar。
///
/// 策略未声明任何驱动序列 → 报错(调用方据此回落到旧路径): 没有驱动序列就没有时间轴,
/// 回测不知道该按什么节奏推进。
/// 回测不产生盘口事件 —— 只写 `on_quote` 的策略在这里一次都不会触发(实盘/Dry Run 才有盘口)。
/// 这属于"静默 0 成交"的坑, 装配期必须明说(第四轮复核发现: 上轮汇报声称已有此告警, 实际没有)。
#[doc(hidden)]
pub fn warn_if_quote_only(strategy: &dyn Strategy) {
    let quote_only = strategy.has_callback("on_quote") && !strategy.has_callback("on_tick");
    if quote_only {
        tracing::warn!(
            target: "backtest",
            "该策略只定义了 on_quote, 而**回测不产生盘口事件**(只有 Dry Run / 实盘 / demo 有行情流): \
             这次回测里它不会被触发, 不要据此判断策略无效; 请用 ricow run (Dry Run) 或 --demo 验证"
        );
    }
}

pub fn run_declared_backtest(
    config: StrategyConfig,
    initial_balance: Balance,
    hub: Arc<DataHub>,
    from_ms: i64,
    to_ms: i64,
    allow_fetch: bool,
) -> CoreResult<BacktestReport> {
    // 宿主 now = 窗口起点: 声明期装载只能看到 from_ms 之前的历史(预热), 无前视。
    let host = Arc::new(EngineHost::new(hub, from_ms, allow_fetch)?);
    let mut strategy = crate::loader::load_strategy_with_host(&config, host.clone())?;
    warn_if_quote_only(&*strategy);
    let decls = strategy.driving_declarations();
    if decls.is_empty() {
        let all = strategy.declarations();
        let detail = if all.is_empty() {
            "策略未声明任何序列".to_string()
        } else {
            format!("策略声明了 {} 条序列但全部是 drive=false(只有句柄, 不进时间轴)", all.len())
        };
        return Err(CoreError::InvalidArgument(format!(
            "{detail}: 无法确定回测时间轴 —— 请把主序列改成 data:series{{..., drive = true}} \
             (不需要 on_bar 回调也可以驱动, 句柄会随派发更新)"
        )));
    }
    // 提示(不是错误): `--pair` / 配置里的交易标的**不在声明序列里** → 它没有撮合参考价,
    // 它的市价单会被拒单。声明驱动回测的取价规则是"只认声明序列"(防跨标的错价), 用户看到
    // 这行提示就知道该给交易标的补一条 `data:series{...}`。
    if let Some(pair) = config.get_str("pair") {
        let symbols: Vec<&str> = decls.iter().map(|d| d.key.symbol.as_str()).collect();
        if !symbols.contains(&pair) {
            tracing::warn!(
                target: "backtest",
                "交易标的 {} 不在声明序列里({}); 未声明的标的没有撮合参考价, 其市价单会被拒单 —— \
                 要交易它请补一条 data:series{{ source=…, symbol=\"{}\" , … }}",
                pair, symbols.join(", "), pair
            );
        }
    }
    let mut driver = SeriesDriver::load(&host, &decls, from_ms, to_ms)?;
    // 主时钟 = 第一条驱动序列(声明序): 与 advance 同一份内存数据, 不重复读库。
    let clock: Vec<Kline> = driver.clock_bars(0);
    if clock.is_empty() {
        return Err(CoreError::InvalidArgument(format!(
            "序列 {} 在窗口 [{} → {}] 内没有 bar; 先用 ricow data pull 补数据",
            decls[0].key, from_ms, to_ms
        )));
    }
    Ok(crate::backtest_runner::run_backtest_with_series(
        config,
        initial_balance,
        &clock,
        &mut driver,
        &mut *strategy,
    ))
}
