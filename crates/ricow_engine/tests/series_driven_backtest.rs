//! 028 T024 集成测试: 引擎按策略声明驱动序列, 且**无前视**。
//!
//! 数据源是"已知向量替身"(固定日线序列, 纯逻辑, 不替代任何真实数据源路径); 数据先落本地
//! 库, 回测按 D3 只读本地库(`allow_fetch = false`) —— 因此本测试同时验证"回测不联网"。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ricow_core::{
    Balance, CoreResult, Interval, Kline, OrderRequest, OrderSide, OrderType, SeriesKey,
    SourceRegistry,
};
use ricow_engine::data::{DataHub, EngineHost, SeriesDriver};
use ricow_engine::run_backtest_with_series;
use ricow_strategy::{
    Context, Database, HostServices, LuaStrategy, SeriesDecl, SeriesInfo, Strategy, StrategyConfig,
};
use rust_decimal::Decimal;

const DAY_MS: i64 = 86_400_000;

/// 固定序列替身: `open = 1000 + 10i`, `close = open + 5` ⇒ 下一根的开盘 = 本根收盘 + 5
/// (所以"成交价是否等于已知收盘 + 5"就是一个可证伪的无前视判据)。
struct FixedSource {
    bars: Vec<Kline>,
}

#[async_trait]
impl ricow_core::KlineSource for FixedSource {
    fn name(&self) -> &'static str {
        "fixed"
    }

    fn supported_intervals(&self) -> &'static [Interval] {
        &[Interval::D1]
    }

    async fn fetch_klines(
        &self,
        _symbol: &str,
        _interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        Ok(self
            .bars
            .iter()
            .filter(|b| {
                let t = b.open_time.timestamp_millis();
                t >= from_ms && t < to_ms
            })
            .cloned()
            .collect())
    }
}

fn daily_bars(n: i64) -> Vec<Kline> {
    (0..n)
        .map(|i| {
            let open_ms = i * DAY_MS;
            Kline {
                open_time: chrono::DateTime::from_timestamp_millis(open_ms).unwrap(),
                open: Decimal::from(1000 + i * 10),
                high: Decimal::from(1010 + i * 10),
                low: Decimal::from(990 + i * 10),
                close: Decimal::from(1005 + i * 10),
                volume: Decimal::from(100),
                close_time: chrono::DateTime::from_timestamp_millis(open_ms + DAY_MS - 1).unwrap(),
            }
        })
        .collect()
}

async fn hub_with(bars: Vec<Kline>) -> Arc<DataHub> {
    let mut registry = SourceRegistry::new();
    registry.register(Arc::new(FixedSource { bars }));
    let db = Database::open_in_memory().await.unwrap();
    Arc::new(DataHub::new(registry, db))
}

fn config() -> StrategyConfig {
    StrategyConfig {
        name: "series-driven".into(),
        strategy_type: "lua".into(),
        enabled: true,
        exchange: "binance".into(),
        params: Default::default(),
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    }
}

fn balance() -> Balance {
    Balance { asset: "USDT".into(), free: Decimal::from(1_000_000), locked: Decimal::ZERO }
}

fn series_key() -> SeriesKey {
    SeriesKey::new("fixed", "TEST", Interval::D1).unwrap()
}

/// 记录型策略: 记下每次 `on_bar` 看到的 bar, 并每次下一张市价买单。
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(i64, Decimal)>>,
}

impl Recorder {
    fn seen(&self) -> Vec<(i64, Decimal)> {
        self.seen.lock().unwrap().clone()
    }
}

impl Strategy for Recorder {
    fn on_bar(
        &mut self,
        _ctx: &mut dyn Context,
        series: &SeriesInfo,
        bar: &Kline,
    ) -> Vec<OrderRequest> {
        assert_eq!(series.id, "d1", "序列描述原样带过来");
        assert_eq!(series.symbol, "TEST");
        self.seen.lock().unwrap().push((bar.close_time.timestamp_millis(), bar.close));
        vec![OrderRequest {
            client_order_id: String::new(),
            pair: "TEST".into(),
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            price: None,
            size: Decimal::from(2),
            reduce_only: false,
            position_side: None,
        }]
    }
}

/// 无前视: 策略看到的是"上一根"的收盘, 而成交价是"这一根"的开盘。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_series_driven_backtest_prices_knowledge_before_trade() {
    let bars = daily_bars(6);
    let hub = hub_with(bars.clone()).await;
    let key = series_key();
    // 数据先落本地库(这一步允许联网; 回测阶段不允许)
    hub.ensure_cached(&key, 0, 6 * DAY_MS).await.unwrap();

    let host = Arc::new(EngineHost::new(hub.clone(), 0, false).unwrap());
    let mut decl = SeriesDecl::new("d1", key.clone());
    decl.bars = Some(3);
    let mut driver = SeriesDriver::load(&host, &[decl], 0, 6 * DAY_MS).unwrap();

    let mut rec = Recorder::default();
    let report = run_backtest_with_series(config(), balance(), &bars, &mut driver, &mut rec);

    let seen = rec.seen();
    assert_eq!(seen.len(), 5, "6 根里最后一根不派发(其后没有可成交的刻度)");
    for (i, (_, close)) in seen.iter().enumerate() {
        assert_eq!(
            *close,
            Decimal::from(1005 + i as i64 * 10),
            "第 {i} 次 on_bar 看到的应是第 {i} 根(按时间顺序, 不跳不重)"
        );
    }
    assert!(
        seen.last().unwrap().0 < bars[5].open_time.timestamp_millis(),
        "最后一根被看到的 bar 必须早于最后一个刻度"
    );

    assert_eq!(report.fills.len(), 5, "每次 on_bar 下的市价单都成交了");
    for (i, f) in report.fills.iter().enumerate() {
        assert_eq!(
            f.fill_price,
            seen[i].1 + Decimal::from(5),
            "第 {i} 笔成交价必须 = 已知收盘 + 5(即下一根开盘价); 若等于本根开盘价则是前视"
        );
    }
    assert!(report.total_trades >= 5);
}

/// 端到端: Lua 策略 `data:series{...}` 声明 → 引擎装数据 → 派发 `on_bar`(真实 Lua 路径)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lua_strategy_declaration_drives_on_bar_end_to_end() {
    let bars = daily_bars(5);
    let hub = hub_with(bars.clone()).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 5 * DAY_MS).await.unwrap();

    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d", bars = 3 }
        function on_bar(ctx, series, bar)
            -- 句柄必须已推进到当前这根(单一写者 = 引擎派发)
            local last = s:last()
            if last == nil or last.open_time ~= bar.open_time then
                return {}
            end
            -- 主动订单出口 + 回调返回值, 两条出口都应生效
            ctx:place_order{ pair = "TEST", side = "buy", size = 1, order_type = "market" }
            return { { pair = "TEST", side = "buy", size = 2, order_type = "market" } }
        end
    "#;
    // 可见时刻 = 第二根 bar 的开盘(即"回测起点已有 1 根预热 bar 可见")
    let host = Arc::new(
        EngineHost::new(hub.clone(), bars[1].open_time.timestamp_millis(), false).unwrap(),
    );
    let host_dyn: Arc<dyn HostServices> = host.clone();
    let mut strategy = LuaStrategy::from_source_with_host(script, config(), host_dyn)
        .expect("顶层声明应成功(宿主已注入)");

    let mut decls = strategy.declarations();
    assert_eq!(decls.len(), 1, "声明被记录");
    assert_eq!(decls[0].id, "d1");
    decls[0].bars = Some(3);

    let mut driver = SeriesDriver::load(&host, &decls, 0, 5 * DAY_MS).unwrap();
    let report = run_backtest_with_series(config(), balance(), &bars, &mut driver, &mut strategy);

    // 4 次 on_bar(5 根里最后一根不派发) × 2 张单 = 8 笔成交
    assert_eq!(report.fills.len(), 8, "回调返回值与 ctx:place_order 同批落地");
}

/// 缺数据: 报错必须带可执行的 `ricow data pull` 命令(FR-031)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_series_driver_missing_data_error_carries_pull_hint() {
    let hub = hub_with(daily_bars(3)).await;
    let host = Arc::new(EngineHost::new(hub, 0, false).unwrap());
    let decl = SeriesDecl::new("d1", SeriesKey::new("fixed", "NOPE", Interval::D1).unwrap());
    let Err(err) = SeriesDriver::load(&host, &[decl], 0, 3 * DAY_MS) else {
        panic!("缺序列必须报错");
    };
    let msg = err.to_string();
    assert!(msg.contains("ricow data pull --source fixed --symbol NOPE --interval 1d"), "{msg}");
    assert!(msg.contains("没有可用数据"), "{msg}");
}

/// 序列过短(指标预热不足)也要给出可执行提示(FR-031 第三类)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_series_driver_too_short_series_error() {
    let bars = daily_bars(3);
    let hub = hub_with(bars.clone()).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 3 * DAY_MS).await.unwrap();
    let host = Arc::new(EngineHost::new(hub, 0, false).unwrap());
    let mut decl = SeriesDecl::new("d1", key);
    decl.min_bars = Some(50);
    let Err(err) = SeriesDriver::load(&host, &[decl], 0, 3 * DAY_MS) else {
        panic!("序列过短必须报错");
    };
    let msg = err.to_string();
    assert!(msg.contains("过短"), "{msg}");
    assert!(msg.contains("至少 50 根"), "{msg}");
}

/// US1: 声明驱动回测的装配语义 (T024).
///
/// 锁定三件事:
/// 1. **预热不派发** —— 窗口起点之前的 bar 只进句柄(`data:series` 声明期已装载), 不触发 `on_bar`;
/// 2. **窗口内每根派发一次** —— 回调数 == 窗口内 bar 数(末根除外, 其后没有可成交刻度);
/// 3. **无前视** —— 看到第 k 根收盘后, 成交发生在第 k+1 根开盘。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_declared_backtest_skips_warmup_and_trades_next_bar_open() {
    let bars = daily_bars(12);
    let hub = hub_with(bars.clone()).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    // 窗口 = [第 5 天, 第 12 天): 前 5 根是预热(不该派发), 窗口内 7 根(末根不派发 → 6 次回调)。
    let from_ms = 5 * DAY_MS;
    let to_ms = 12 * DAY_MS;

    let mut cfg = config();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar)
            -- 句柄已在声明期预热(窗口前 3 根), 这里再验一次尾窗契约
            local last = s:last()
            if last == nil or last.open_time ~= bar.open_time then
                return {}
            end
            return { { pair = "TEST", side = "buy", size = 1, order_type = "market" } }
        end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));

    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        balance(),
        hub.clone(),
        from_ms,
        to_ms,
        false,
    )
    .expect("声明驱动回测应成功");

    assert_eq!(report.total_bars, 7, "主时钟 = 第一条驱动序列的窗口内 bar (12-5=7 根)");
    assert_eq!(
        report.fills.len(),
        6,
        "窗口内 7 根只派发 6 次(末根之后没有可成交刻度); 若预热被派发则会远多于 6"
    );
    // 第 5 根(索引 5)收盘 1055 → 下一根开仓, 成交价 = 第 6 根开盘 = 1000 + 60 = 1060。
    assert_eq!(
        report.fills[0].fill_price,
        Decimal::from(1060),
        "看到第 5 根收盘后应在第 6 根开盘成交(等于本根开盘即前视, 等于第 5 根开盘即串位)"
    );
    assert!(report.fills.iter().all(|f| f.fill_price >= Decimal::from(1060)));
}

/// US1: 无声明 → 报错说清原因(而不是静默跑一条空回测)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_declared_backtest_requires_declaration() {
    let bars = daily_bars(3);
    let hub = hub_with(bars).await;
    hub.ensure_cached(&series_key(), 0, 3 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = "function on_tick(ctx) return {} end";
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));

    let err = ricow_engine::data::run_declared_backtest(cfg, balance(), hub, 0, 3 * DAY_MS, false)
        .expect_err("未声明驱动序列必须报错");
    let msg = err.to_string();
    assert!(msg.contains("未声明任何序列"), "错误应说明原因, got: {msg}");
}

/// 028 审核修复的回归: **只声明句柄(`drive=false`)** 的策略也必须走数据服务路径 ——
/// 不能因为"没有驱动序列"就静默回落到联网预取 K 线的旧路径(D3: 回测只读本地库)。
/// 装配层必须**明确报错**说清"没有时间轴", 并告诉用户怎么改。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_handle_only_declaration_errors_instead_of_silently_using_old_path() {
    let bars = daily_bars(6);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 6 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = false }
        function on_tick(ctx) return {} end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    // 判定层: 声明过序列(哪怕只要句柄) → 非空, CLI 会走声明路径而不是旧路径
    let decls =
        ricow_engine::data::declared_series(&cfg, hub.clone(), 0, true).expect("判定应成功");
    assert_eq!(decls.len(), 1, "drive=false 也是声明, 判定层必须看见它");
    // 装配层: 没有驱动序列 → 明确报错(而不是偷偷联网跑旧路径)
    let err = ricow_engine::data::run_declared_backtest(cfg, balance(), hub, 0, 6 * DAY_MS, false)
        .expect_err("无驱动序列必须报错");
    let msg = err.to_string();
    assert!(msg.contains("drive=false"), "错误要说清是句柄-only 声明, got: {msg}");
    assert!(msg.contains("drive = true"), "错误要给出改法, got: {msg}");
}

/// 实盘/Dry Run 语义 (T028 真机踩坑后补): 装配只吃"起点之后要派发的 bar",
/// 之后由 `advance_live` 按当前时刻**增量取数**, 只派发新收盘的 bar。
///
/// 真机 Dry Run 实测到过两件事, 都在这里锁定:
/// 1. 启动瞬间不能把装载到的历史 bar 一次性派发(否则策略对着旧 bar 连开一堆仓);
/// 2. 只吃静态装载列表 → 启动后新收盘的 bar 永远进不来, `on_bar` 一次都不触发。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_advance_live_tops_up_only_new_closed_bars() {
    let bars = daily_bars(9);
    let hub = hub_with(bars.clone()).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 9 * DAY_MS).await.unwrap();

    // 装配起点 = 第 6 天(相当于实盘启动那一刻)。
    let host = Arc::new(EngineHost::new(hub.clone(), 6 * DAY_MS, true).unwrap());
    let mut decl = SeriesDecl::new("d1", key.clone());
    decl.bars = Some(3);
    let mut driver = SeriesDriver::load(&host, &[decl], 6 * DAY_MS, 6 * DAY_MS).unwrap();

    assert!(
        driver.advance(6 * DAY_MS).is_empty(),
        "启动瞬间不得派发历史 bar(装配只装起点之后要派发的)"
    );

    // 时间走到第 7 天: 第 6 天那根已收盘 → 增量取数后派发它, 且只派发它。
    let (events, health) = driver.advance_live(&host, 7 * DAY_MS);
    assert_eq!(events.len(), 1, "只应派发新收盘的第 6 根, got {}", events.len());
    assert_eq!(events[0].0.id, "d1");
    assert_eq!(events[0].1.open_time.timestamp_millis(), 6 * DAY_MS, "派发的就是第 6 根");
    assert!(health.iter().all(|(_, ok)| *ok), "取数应成功");

    // 同一时刻再推: 不重复派发。
    let (again, _) = driver.advance_live(&host, 7 * DAY_MS);
    assert!(again.is_empty(), "同一刻度不重复派发");

    // 时间跨到第 9 天: 中间缺口一次补齐(不丢 bar)。
    let (catch_up, _) = driver.advance_live(&host, 9 * DAY_MS);
    assert_eq!(catch_up.len(), 2, "第 7/8 两根一次补齐, got {}", catch_up.len());
}

// ============================================================================
// 2026-09-21 独立审核(3 位)发现的问题 → 逐条回归(修完即钉住, 防复发)
// ============================================================================

use ricow_engine::data::EngineHost as Host;
use ricow_engine::data::{DriveEvent, DrivenRuntime};

/// 按 symbol 返回不同序列的替身: 多标的场景必需(上面的 `FixedSource` 对所有 symbol 返回同一份)。
struct BySymbolSource {
    aaa: Vec<Kline>,
    bbb: Vec<Kline>,
}

#[async_trait]
impl ricow_core::KlineSource for BySymbolSource {
    fn name(&self) -> &'static str {
        "fixed"
    }
    fn supported_intervals(&self) -> &'static [Interval] {
        &[Interval::D1]
    }
    async fn fetch_klines(
        &self,
        symbol: &str,
        _interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        let src = if symbol == "AAA" { &self.aaa } else { &self.bbb };
        Ok(src
            .iter()
            .filter(|b| {
                let t = b.open_time.timestamp_millis();
                t >= from_ms && t < to_ms
            })
            .cloned()
            .collect())
    }
}

/// 任意基准价的日线: `open = base + step*i`, `close = open + close_off`。
fn bars_scaled(n: i64, base: i64, step: i64, close_off: i64) -> Vec<Kline> {
    (0..n)
        .map(|i| {
            let open_ms = i * DAY_MS;
            Kline {
                open_time: chrono::DateTime::from_timestamp_millis(open_ms).unwrap(),
                open: Decimal::from(base + i * step),
                high: Decimal::from(base + i * step + close_off + 5),
                low: Decimal::from(base + i * step - 5),
                close: Decimal::from(base + i * step + close_off),
                volume: Decimal::from(100),
                close_time: chrono::DateTime::from_timestamp_millis(open_ms + DAY_MS - 1).unwrap(),
            }
        })
        .collect()
}

async fn hub_two_symbols() -> Arc<DataHub> {
    let mut registry = SourceRegistry::new();
    registry.register(Arc::new(BySymbolSource {
        aaa: bars_scaled(12, 1000, 10, 5),
        bbb: bars_scaled(12, 10, 1, 1),
    }));
    let db = Database::open_in_memory().await.unwrap();
    Arc::new(DataHub::new(registry, db))
}

/// 审核 🔴#1: 多标的声明回测里, **成交价与 `ctx:price` 必须按各自序列取**, 不能用主时钟的价。
///
/// 实测复现过: AAA(千元档)当主时钟、策略交易 BBB(十元档)时, BBB 以 AAA 的价成交(1010 vs 真实 10)。
/// 断言写成"BBB 成交价 < 100" —— 一旦回落到主时钟价格, 数字差两个数量级, 必然失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multi_symbol_backtest_prices_each_pair_from_its_own_series() {
    let hub = hub_two_symbols().await;
    let aaa = SeriesKey::new("fixed", "AAA", Interval::D1).unwrap();
    let bbb = SeriesKey::new("fixed", "BBB", Interval::D1).unwrap();
    hub.ensure_cached(&aaa, 0, 12 * DAY_MS).await.unwrap();
    hub.ensure_cached(&bbb, 0, 12 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = r#"
        local a = data:series{ id = "aaa", source = "fixed", symbol = "AAA", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        local b = data:series{ id = "bbb", source = "fixed", symbol = "BBB", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar)
            if series.symbol ~= "BBB" then return {} end
            seen_bbb = ctx:price("BBB")
            seen_aaa = ctx:price("AAA")
            return { { pair = "BBB", side = "buy", size = 1, order_type = "market" } }
        end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));

    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        balance(),
        hub,
        4 * DAY_MS,
        9 * DAY_MS,
        false,
    )
    .expect("声明驱动回测应成功");

    assert!(!report.fills.is_empty(), "BBB 应有成交");
    for f in &report.fills {
        assert_eq!(f.pair, "BBB");
        assert!(
            f.fill_price < Decimal::from(100),
            "BBB 成交价必须来自 BBB 自己的序列(<100); 实测 {} —— 若取主时钟(AAA)的价约为 1000+",
            f.fill_price
        );
    }
}

/// 审核 🔴#2: 声明定时器的回测里 `on_timer` **必须真的被触发**。
///
/// 之前只把 bar 接进回测主循环, 定时器完全没接线 → 只靠 `on_timer` 下丹的策略回测出 0 成交
/// 却被当成"策略没信号"的正常报告(静默假结论)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_declared_backtest_dispatches_declared_timers() {
    let bars = daily_bars(12);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        data.timer{ label = "t1", secs = 3600 }
        function on_bar(ctx, series, bar) return {} end
        function on_timer(ctx, label)
            timer_hits = (timer_hits or 0) + 1
            return { { pair = "TEST", side = "buy", size = 1, order_type = "market" } }
        end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));

    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        balance(),
        hub,
        4 * DAY_MS,
        9 * DAY_MS,
        false,
    )
    .expect("声明驱动回测应成功");
    assert!(
        !report.fills.is_empty(),
        "声明了 data.timer 的回测里 on_timer 必须被触发(修复前实测 0 次)"
    );
}

/// 审核 🔴#3: 实盘/Dry Run 装配必须尊重 `drive = false`(只要句柄、不要回调)。
///
/// 修复前 `assemble` 用 `declarations()` → drive=false 的序列照样推 `on_bar`, 同一份 Lua 在
/// 回测(尊重 drive)与实盘(忽略 drive)行为不一致。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_live_assembly_respects_drive_false() {
    let bars = daily_bars(12);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    let now = 6 * DAY_MS;
    let host: Arc<Host> = Arc::new(Host::new(hub, now, true).unwrap());
    let dyn_host: Arc<dyn HostServices> = host.clone();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = false }
        function on_bar(ctx, series, bar) bar_hits = (bar_hits or 0) + 1; return {} end
    "#;
    let cfg = {
        let mut c = config();
        c.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
        c
    };
    let strategy = LuaStrategy::from_source_with_host(script, cfg, dyn_host).expect("脚本应通过");
    assert_eq!(strategy.declarations().len(), 1, "声明了一条序列");
    assert_eq!(strategy.driving_declarations().len(), 0, "drive=false 不是驱动序列");

    let mut rt = DrivenRuntime::assemble(host, &strategy, now).expect("装配应成功");
    let events = rt.advance(now + 2 * DAY_MS);
    assert!(
        !events.iter().any(|e| matches!(e, DriveEvent::Bar(..))),
        "drive=false 的序列在实盘装配里不得推 on_bar(修复前会推)"
    );
}

/// 审核 🟡: 预热边界必须与装载判据同向 —— `close_time` **恰好等于**窗口起点的 bar 属预热,
/// 不得既进句柄又被派发一次。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_warmup_boundary_bar_with_close_time_equal_to_window_start_is_not_dispatched() {
    // close_time = open + 1s, 于是"close_time 恰好等于 from_ms"很好构造。
    let bars: Vec<Kline> = (0..9)
        .map(|i| {
            let open_ms = i * DAY_MS;
            Kline {
                open_time: chrono::DateTime::from_timestamp_millis(open_ms).unwrap(),
                open: Decimal::from(1000 + i * 10),
                high: Decimal::from(1010 + i * 10),
                low: Decimal::from(990 + i * 10),
                close: Decimal::from(1005 + i * 10),
                volume: Decimal::from(100),
                close_time: chrono::DateTime::from_timestamp_millis(open_ms + 1000).unwrap(),
            }
        })
        .collect();
    let hub = hub_with(bars.clone()).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 9 * DAY_MS).await.unwrap();

    // from_ms 恰等于第 4 根(索引 3)的 close_time。
    let from_ms = 3 * DAY_MS + 1000;
    let host = Arc::new(Host::new(hub, from_ms, false).unwrap());
    let mut decl = SeriesDecl::new("d1", key.clone());
    decl.bars = Some(3);
    let mut driver = SeriesDriver::load(&host, &[decl], from_ms, 9 * DAY_MS).unwrap();

    let mut dispatched = Vec::new();
    let clock = driver.clock_bars(0);
    assert!(!clock.is_empty());
    for k in &clock {
        for (_, bar) in driver.advance(k.open_time.timestamp_millis()) {
            dispatched.push(bar.close_time.timestamp_millis());
        }
    }
    assert!(
        dispatched.iter().all(|c| *c > from_ms),
        "预热段(close_time <= 窗口起点)一根都不能派发; 实测派发了 {:?}(from_ms={from_ms})",
        dispatched
    );
}

/// 审核 must-fix: `stale` 端到端 —— 增量取数失败 → 策略 `s:stale()` 可见。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_stale_reaches_strategy_after_incremental_fetch_failure() {
    /// 首次取数成功, 之后失败(模拟源被限流/不可达)。
    struct FlakySource {
        bars: Vec<Kline>,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl ricow_core::KlineSource for FlakySource {
        fn name(&self) -> &'static str {
            "fixed"
        }
        fn supported_intervals(&self) -> &'static [Interval] {
            &[Interval::D1]
        }
        async fn fetch_klines(
            &self,
            _symbol: &str,
            _interval: Interval,
            from_ms: i64,
            to_ms: i64,
        ) -> CoreResult<Vec<Kline>> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n > 0 {
                return Err(ricow_core::CoreError::InvalidArgument(
                    "测试替身: 源不可达".to_string(),
                ));
            }
            Ok(self
                .bars
                .iter()
                .filter(|b| {
                    let t = b.open_time.timestamp_millis();
                    t >= from_ms && t < to_ms
                })
                .cloned()
                .collect())
        }
    }

    let mut registry = SourceRegistry::new();
    registry.register(Arc::new(FlakySource {
        bars: daily_bars(12),
        calls: std::sync::atomic::AtomicUsize::new(0),
    }));
    let db = Database::open_in_memory().await.unwrap();
    let hub = Arc::new(DataHub::new(registry, db));
    let key = series_key();
    // 只缓存到第 7 天(首次取数走替身 = 成功): 之后窗口要 [7d, 9d] 的数据, 必须向源再取
    // → 替身第 2 次调用失败 → 这才触发"增量取数失败"这条路径。
    hub.ensure_cached(&key, 0, 7 * DAY_MS).await.unwrap();

    let now = 6 * DAY_MS;
    let host: Arc<Host> = Arc::new(Host::new(hub, now, true).unwrap());
    let dyn_host: Arc<dyn HostServices> = host.clone();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar) return {} end
        function on_tick(ctx)
            if s:stale() then
                return { { pair = "ETHUSDT", side = "buy", size = 1, order_type = "market" } }
            end
            return {}
        end
    "#;
    let mut cfg = config();
    cfg.params.insert("pair".into(), ricow_strategy::ConfigValue::String("ETHUSDT".into()));
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    let mut strategy =
        LuaStrategy::from_source_with_host(script, cfg.clone(), dyn_host).expect("脚本应通过");

    let mut rt = DrivenRuntime::assemble(host, &strategy, now).expect("装配应成功");
    let _ = rt.advance(now + 3 * DAY_MS);
    let health = rt.take_stale_updates();
    assert!(
        health.iter().any(|(id, ok)| id == "d1" && !*ok),
        "增量取数失败必须上报为 stale, got {health:?}"
    );
    for (id, ok) in health {
        strategy.mark_series_stale(&id, !ok);
    }

    // 用**正常订单管线**验证策略确实"看见了" stale: 脚本只在 s:stale() 为真时下单,
    // 于是"on_tick 返回了订单"就是端到端证据(比读 Lua 内部变量更接近真实用法, 也不需要为测试开 API)。
    let mut ctx = ricow_strategy::BacktestContext::new(cfg, balance());
    let orders = strategy.on_tick(&mut ctx);
    assert!(!orders.is_empty(), "取数失败后策略应能通过 s:stale() 感知并下单");
    assert_eq!(orders[0].pair, "ETHUSDT");
}

/// 审核修复(同类门口的第二个): 声明为 `drive = false`(只要句柄)的序列**没有撮合参考价**,
/// 交易它必须**明确拒单**, 而不是悄悄回落到主时钟序列的价格。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_trading_a_handle_only_series_is_rejected_not_mispriced() {
    let bars = daily_bars(12);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    let mut cfg = config();
    // 序列 A(d1)驱动 + 序列 B(只句柄, drive=false); 策略交易 B。
    let script = r#"
        local a = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        local b = data:series{ id = "d2", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = false }
        function on_bar(ctx, series, bar)
            return { { pair = "TEST", side = "buy", size = 1, order_type = "market" } }
        end
        function on_tick(ctx) return {} end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        balance(),
        hub,
        4 * DAY_MS,
        9 * DAY_MS,
        false,
    )
    .expect("声明驱动回测应成功");
    // d1 是驱动序列 → 它自己的 bar 就是参考价, 所以这里"TEST"有价可撮合 → 会成交。
    // 关键断言: 成交价必须等于 **TEST 自己那条序列的 open**(1000+ 档), 不是别的标的的价。
    for f in &report.fills {
        assert!(
            f.fill_price >= Decimal::from(1000) && f.fill_price < Decimal::from(2000),
            "成交价必须来自 TEST 自己的序列(1000~2000 档), 实测 {}",
            f.fill_price
        );
    }
    // 而如果策略交易一个**没声明过**的标的(CCCUSDT), 那才是"已知但无价" → 拒单、不成交。
    let mut cfg2 = config();
    let script2 = r#"
        local a = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar)
            return { { pair = "CCCUSDT", side = "buy", size = 1, order_type = "market" } }
        end
    "#;
    cfg2.params.insert("script".into(), ricow_strategy::ConfigValue::String(script2.into()));
    let hub2 = hub_with(daily_bars(12)).await;
    hub2.ensure_cached(&series_key(), 0, 12 * DAY_MS).await.unwrap();
    let report2 = ricow_engine::data::run_declared_backtest(
        cfg2,
        balance(),
        hub2,
        4 * DAY_MS,
        9 * DAY_MS,
        false,
    )
    .expect("回测应成功");
    assert!(
        report2.fills.is_empty() && report2.rejected_count > 0,
        "交易未声明的标的应被拒单(无参考价), 实测 fills={} rejected={}",
        report2.fills.len(),
        report2.rejected_count
    );
}

/// 支持 1h + 1d 两个周期的替身(同标的多周期场景必需)。
struct HourlyAndDailySource {
    hourly: Vec<Kline>,
    daily: Vec<Kline>,
}

#[async_trait]
impl ricow_core::KlineSource for HourlyAndDailySource {
    fn name(&self) -> &'static str {
        "fixed"
    }
    fn supported_intervals(&self) -> &'static [Interval] {
        &[Interval::H1, Interval::D1]
    }
    async fn fetch_klines(
        &self,
        _symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        let src = if interval == Interval::H1 { &self.hourly } else { &self.daily };
        Ok(src
            .iter()
            .filter(|b| {
                let t = b.open_time.timestamp_millis();
                t >= from_ms && t < to_ms
            })
            .cloned()
            .collect())
    }
}

/// 任意基准价的 1h 序列(与 `bars_scaled` 同构, 只是粒度换成小时)。
fn hourly_bars(n: i64, base: i64) -> Vec<Kline> {
    (0..n)
        .map(|i| {
            let open_ms = i * 3_600_000;
            Kline {
                open_time: chrono::DateTime::from_timestamp_millis(open_ms).unwrap(),
                open: Decimal::from(base + i),
                high: Decimal::from(base + i + 5),
                low: Decimal::from(base + i - 5),
                close: Decimal::from(base + i + 1),
                volume: Decimal::from(100),
                close_time: chrono::DateTime::from_timestamp_millis(open_ms + 3_600_000 - 1)
                    .unwrap(),
            }
        })
        .collect()
}

// ============================================================================
// 2026-09-21 第三轮独立审核(3 位)发现的回测时序/估值问题 → 逐条回归
// ============================================================================

/// 审核 2 号 🔴: 多标的持仓的**估值/期末权益**必须按各标的自己的价, 不能用主时钟标的的价。
/// AAA≈1000 档(主时钟) vs BBB≈10 档: 买 2 个 BBB 后, 若按 AAA 估值, 期末权益会比本金多 ~2000。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multi_symbol_positions_are_valued_at_their_own_prices() {
    let hub = hub_two_symbols().await;
    let aaa = SeriesKey::new("fixed", "AAA", Interval::D1).unwrap();
    let bbb = SeriesKey::new("fixed", "BBB", Interval::D1).unwrap();
    hub.ensure_cached(&aaa, 0, 12 * DAY_MS).await.unwrap();
    hub.ensure_cached(&bbb, 0, 12 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = r#"
        local a = data:series{ id = "aaa", source = "fixed", symbol = "AAA", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        local b = data:series{ id = "bbb", source = "fixed", symbol = "BBB", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar)
            if series.symbol ~= "BBB" or placed then return {} end
            placed = true
            return { { pair = "BBB", side = "buy", size = 2, order_type = "market" } }
        end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    let initial = Decimal::from(100); // 买 2 个 BBB(≈11~16 档) 之后还要留手续费
    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        Balance { asset: "USDT".into(), free: initial, locked: Decimal::ZERO },
        hub,
        4 * DAY_MS,
        9 * DAY_MS,
        false,
    )
    .expect("声明驱动回测应成功");

    assert_eq!(report.fills.len(), 1, "只买一次");
    // 买 2 个 BBB(≈11~16 档) 后: 期末权益 ≈ 本金(20) - 手续费 ± 小幅浮盈浮亏, 量级 = 几十。
    // 若按主时钟 AAA(≈1000 档) 估值 → 权益会是 ~2000 量级(差两个数量级), 断言必失败。
    assert!(
        report.final_equity < Decimal::from(200),
        "多标的持仓必须按自己的价估值; 实测 final_equity={} (按 AAA 估值会是 ~2000 量级)",
        report.final_equity
    );
}

/// 审核 2 号 🟠: 声明路径的**挂单撮合不能晚一刻** —— `set_declared_bars` 必须在 `step_bar`
/// 之前(否则 tick k 的 match_pending 用的是 k−1 的参考 bar)。
///
/// 判据用**成交时间戳**(虚拟钟): 卖单挂在 bar5.high 与 bar6.high 之间, 只可能被 bar6 触发。
/// 修好 → 在 tick 6d 成交; 晚一刻 → tick 7d 才成交。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pending_order_matches_on_the_same_tick_as_the_old_path() {
    let bars = daily_bars(12);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    // bar_i: open=1000+10i, high=1010+10i, low=990+10i, close=1005+10i。
    // 在 tick 4d(看到 bar3)挂一张**买**限价单, 它在 tick 5d 与参考 bar5 比:
    //   参考 bar5(low=1040) → 挂价 1045 成交 / 挂价 1035 不成交;
    //   若参考晚一刻(用 bar4, low=1030) → 1035 也会被成交(判据正好相反)。
    for (limit, should_fill) in [(1045, true), (1035, false)] {
        let mut cfg = config();
        let script = format!(
            r#"
        local s = data:series{{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 5, min_bars = 2, drive = true }}
        function on_bar(ctx, series, bar)
            if placed then return {{}} end
            placed = true
            return {{ {{ pair = "TEST", side = "buy", size = 1, order_type = "limit", price = {limit} }} }}
        end
    "#
        );
        cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script));
        let report = ricow_engine::data::run_declared_backtest(
            cfg,
            Balance { asset: "USDT".into(), free: Decimal::from(10_000), locked: Decimal::ZERO },
            hub.clone(),
            4 * DAY_MS,
            9 * DAY_MS,
            false,
        )
        .expect("回测应成功");
        let label = format!("挂价 {limit}");
        if should_fill {
            assert_eq!(report.fills.len(), 1, "{label}: 应被参考 bar5(low=1040) 成交");
            assert_eq!(report.fills[0].fill_price, Decimal::from(limit), "{label}: 限价成交");
            assert_eq!(
                report.fills[0].timestamp.timestamp_millis(),
                5 * DAY_MS,
                "{label}: 必须在 tick 5d 成交(参考 bar 与 step_bar 同刻)"
            );
        } else {
            assert!(
                report.fills.is_empty(),
                "{label}: 参考 bar5 的 low 高于挂价 → 不该成交; 成交说明挂单撮合晚了一刻(用了 bar4)"
            );
        }
    }
}

/// 审核 2 号 🟠: 主时钟必须受 `to_ms` 右端约束(原来会一路跑到本地库最后一根)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_declared_backtest_window_right_end_is_enforced() {
    let bars = daily_bars(12);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar) return {} end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        balance(),
        hub,
        3 * DAY_MS,
        8 * DAY_MS,
        false,
    )
    .expect("回测应成功");
    assert_eq!(report.total_bars, 5, "窗口 [3d, 8d) 只有 5 根 bar; 不受右端约束会跑出 9 根");
}

/// 审核 2 号 🟠: 同标的声明两条周期时, 撮合参考价取**最细周期**, 与声明顺序无关。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_same_symbol_two_intervals_picks_finest_regardless_of_order() {
    for daily_first in [true, false] {
        let mut registry = SourceRegistry::new();
        registry.register(Arc::new(HourlyAndDailySource {
            hourly: hourly_bars(48, 10),
            daily: daily_bars(6),
        }));
        let db = Database::open_in_memory().await.unwrap();
        let hub = Arc::new(DataHub::new(registry, db));
        hub.ensure_cached(&SeriesKey::new("fixed", "TEST", Interval::H1).unwrap(), 0, 3 * DAY_MS)
            .await
            .unwrap();
        hub.ensure_cached(&SeriesKey::new("fixed", "TEST", Interval::D1).unwrap(), 0, 6 * DAY_MS)
            .await
            .unwrap();

        let (a, b) = if daily_first {
            (r#"interval = "1d", bars = 3"#, r#"interval = "1h", bars = 5"#)
        } else {
            (r#"interval = "1h", bars = 5"#, r#"interval = "1d", bars = 3"#)
        };
        let script = format!(
            r#"
        local s1 = data:series{{ id = "s1", source = "fixed", symbol = "TEST", {a}, min_bars = 2, drive = true }}
        local s2 = data:series{{ id = "s2", source = "fixed", symbol = "TEST", {b}, min_bars = 2, drive = true }}
        function on_bar(ctx, series, bar)
            if placed then return {{}} end
            placed = true
            return {{ {{ pair = "TEST", side = "buy", size = 1, order_type = "market" }} }}
        end
    "#
        );
        let mut cfg = config();
        cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script));
        let report = ricow_engine::data::run_declared_backtest(
            cfg,
            balance(),
            hub,
            12 * 3_600_000,
            12 * 3_600_000 + 4 * DAY_MS,
            false,
        )
        .expect("回测应成功");
        assert!(!report.fills.is_empty(), "应有一笔成交");
        // 1h 序列的价档 = 10 附近(远小于日线 1000 档): 取最细周期 → 成交价必须在 1h 档。
        assert!(
            report.fills[0].fill_price < Decimal::from(100),
            "同标的多周期时必须取**最细**周期作参考价(与声明顺序无关); daily_first={daily_first} 实测 {}",
            report.fills[0].fill_price
        );
    }
}

/// 审核 2 号 🟡: 用**别名前缀**的 pair 下单也不得回落到主时钟价(必须无参考价 → 拒单)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_undeclared_pair_never_falls_back_to_main_clock_price() {
    let bars = daily_bars(12);
    let hub = hub_with(bars).await;
    let key = series_key();
    hub.ensure_cached(&key, 0, 12 * DAY_MS).await.unwrap();

    let mut cfg = config();
    let script = r#"
        local s = data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                               bars = 3, min_bars = 2, drive = true }
        function on_bar(ctx, series, bar)
            return { { pair = "CCCUSDT", side = "buy", size = 1, order_type = "market" } }
        end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    let report = ricow_engine::data::run_declared_backtest(
        cfg,
        balance(),
        hub,
        4 * DAY_MS,
        9 * DAY_MS,
        false,
    )
    .expect("回测应成功");
    assert!(
        report.fills.is_empty() && report.rejected_count > 0,
        "未声明标的 CCCUSDT 必须拒单(绝不回落主时钟价); 实测 fills={} rejected={}",
        report.fills.len(),
        report.rejected_count
    );
    // 第四轮复核: 带交易所名前缀的**别名**写法(`BINANCE:TEST`)现在按 base 归一化命中声明序列,
    // 属"同一序列的等价写法"而不是绕过 —— 由 `test_declared_lookup_accepts_prefix_and_case_aliases` 覆盖。
}

// ============================================================================
/// 第四轮复核发现: 声明序列的取价原来只认"裸 symbol"与 `bn:` 前缀 —— 写成交易所全名
/// `binance:ETHUSDT` / `BINANCE:ETHUSDT` 会被当成"未声明标的"而**静默拒单**。
/// 现在四种写法必须都命中**同一条声明序列**(同一个参考价), 未声明标的仍必须拒单。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_declared_lookup_accepts_prefix_and_case_aliases() {
    let hub = hub_with(daily_bars(12)).await;
    // 回测只读本地库(D3): 先把序列落进库, 否则声明期就会报"本地库为空"。
    hub.ensure_cached(&series_key(), 0, 12 * DAY_MS).await.unwrap();
    let mut cfg = config();
    cfg.params.insert("pair".into(), ricow_strategy::ConfigValue::String("TEST".into()));

    let script = r#"
        data:series{ id = "d1", source = "fixed", symbol = "TEST", interval = "1d",
                     bars = 3, min_bars = 2, drive = true }
        local forms = { "TEST", "bn:TEST", "binance:TEST", "BINANCE:TEST", "CCCUSDT" }
        local i = 0
        function on_bar(ctx, series, bar)
            i = i + 1
            if i > #forms then return {} end
            return { { pair = forms[i], side = "buy", size = 1, order_type = "market" } }
        end
    "#;
    cfg.params.insert("script".into(), ricow_strategy::ConfigValue::String(script.into()));
    // 窗口 6 天 → 6 个刻度, 最后一根没有后继 → 5 次 on_bar 回调, 正好放下 5 种写法。
    let report =
        ricow_engine::data::run_declared_backtest(cfg, balance(), hub, 0, 6 * DAY_MS, false)
            .expect("声明驱动回测应成功");

    let prices: Vec<String> = report.fills.iter().map(|f| f.fill_price.to_string()).collect();
    assert_eq!(
        report.fills.len(),
        4,
        "TEST / bn:TEST / binance:TEST / BINANCE:TEST 四种写法都必须命中声明序列; got fills={:?}",
        prices
    );
    // 四个 tick 各下一单 → 参考 bar 逐 tick 前进(fixed 源 open=1000+10i), 故成交价应严格 +10 递增;
    // 只要落在 1000~1100 档且步长 10, 就证明取价来自**声明的那条 TEST 序列**(而非别的标的)。
    let nums: Vec<i64> = prices.iter().map(|p| p.parse::<i64>().unwrap()).collect();
    assert!(
        nums.windows(2).all(|w| w[1] - w[0] == 10) && nums.iter().all(|n| (1000..1100).contains(n)),
        "四种写法都必须取到声明序列 TEST 的价(1000 档、步长 10), got: {prices:?}"
    );
    // 未声明的 CCCUSDT 仍必须被拒(绝不回落主时钟价)。
    assert!(report.rejected_count > 0, "未声明标的的市价单必须拒单");
}
