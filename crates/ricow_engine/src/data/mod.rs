//! 数据服务 DataHub (028 T009): 键解析 / 缓存命中 / 按需回补 / 同键去重 / 源级限速 / 未知源报错。
//!
//! 职责边界(plan d10/d11):
//! - 取数(网络)与缓存(`data_klines`)都在**引擎侧**; 策略侧只经 `HostServices` 接缝查询(无网络依赖);
//! - 数据源以注册名接入([`SourceRegistry`]), 本模块不认识任何具体源 —— 加了新源不用改这里。
//!
//! 缓存命中判定(如实说明取舍):
//! - **尾部**: 本地最大 `open_time + 周期 >= to` 即视为已覆盖;
//! - **头部**: 需要知道"该源在 `from` 之前是否还有数据", 这点本地库无法自证。做法 = 首次为某序列
//!   补头部区间后, 在**进程内**记下"头部已确认"(`head_checked`); 因此进程重启后每个序列会多一次
//!   头部探测请求(可接受, 换来"不重复补已知为空的区间");
//! - 命中时不发起任何网络请求(可复现性: 回测只读本地库靠的是调用方不调 `ensure_cached`)。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ricow_core::{
    resample_to_interval, Bar, CoreError, CoreResult, Exchange, Kline, PriceMode, SeriesKey,
    SeriesWindow, SourceRegistry,
};
use rust_decimal::Decimal;

use ricow_strategy::Database;

/// 默认数据源装配 (028 T013): `binance_spot` / `binance_futures`(可选) / `nasdaq` / `yahoo`。
///
/// 装配层(CLI、Dry Run、回测)统一调这里; 新增一个来源 = 多一行注册 —— 策略与主流程零改动(SC-002)。
pub fn default_registry(
    spot: Arc<dyn Exchange>,
    futures: Option<Arc<dyn Exchange>>,
) -> SourceRegistry {
    let mut registry = SourceRegistry::new();
    source_binance::register(&mut registry, spot, futures);
    source_nasdaq::register(&mut registry);
    source_yahoo::register(&mut registry);
    registry
}

/// 数据源适配: 每个来源一个文件, 新增来源不改本模块(验收 SC-002)。
pub mod source_binance;
pub mod source_nasdaq;
pub mod source_yahoo;

/// 声明驱动的回测装配 (028 T024/US1): 宿主 + 驱动 + 主时钟。
pub mod declared_backtest;
/// 声明驱动的运行期装配 (028 T023): Dry Run / 实盘共用。
pub mod driven;
/// 宿主服务实现 (028 T016): 策略侧 `HostServices` → DataHub + HTTP。
pub mod host_impl;
/// 序列驱动 (028 T024): 主时钟 → "此刻哪些 bar 刚收盘"。
pub mod series_driver;

pub use declared_backtest::{declared_series, run_declared_backtest, warn_if_quote_only};
pub use driven::{DriveEvent, DrivenRuntime};
pub use host_impl::{EngineHost, HttpPolicy};
pub use series_driver::SeriesDriver;

/// 区间结果的统一点: 升序 + 按 `open_time` 去重 + 半开区间裁剪(带复权列的版本)。
///
/// `KlineSource` 的契约由数据服务依赖: 任何来源实现都必须交回"升序、无重复、落在
/// `[from, to)`"的结果, 因此在适配层统一收口(各源实现只管取数, 不各写一份清洗)。
pub(crate) fn normalize_bars(bars: Vec<Bar>, from_ms: i64, to_ms: i64) -> Vec<Bar> {
    let mut out = bars;
    out.sort_by_key(|b| b.kline.open_time);
    out.dedup_by_key(|b| b.kline.open_time);
    out.retain(|b| {
        let t = b.kline.open_time.timestamp_millis();
        t >= from_ms && t < to_ms
    });
    out
}

/// [`normalize_bars`] 的无复权列版本。
pub(crate) fn normalize_range(bars: Vec<Kline>, from_ms: i64, to_ms: i64) -> Vec<Kline> {
    normalize_bars(bars.into_iter().map(Bar::new).collect(), from_ms, to_ms)
        .into_iter()
        .map(|b| b.kline)
        .collect()
}

/// 按序列声明的价格口径, 把缓存 bar 转成**策略可见**的 K 线 (FR-015)。
///
/// - `PriceMode::Close` = 原样返回;
/// - `PriceMode::AdjClose` = 用 `adj_close / close` 比例**同步缩放 OHLC**, 并把 `close`
///   置为复权收盘(复权序列的口径: 比例缩放可保持 ATR/涨跌幅等价的相对关系);
/// - 源不提供复权列时**报错**, 不静默回落到原始价(避免同一回测里两种口径混用)。
///
/// 缩放用十进制精确乘法(不做四舍五入), 保证逐位可复现。
pub(crate) fn apply_price_mode(
    bars: Vec<Bar>,
    mode: PriceMode,
    key: &SeriesKey,
) -> CoreResult<Vec<Kline>> {
    match mode {
        PriceMode::Close => Ok(bars.into_iter().map(|b| b.kline).collect()),
        PriceMode::AdjClose => {
            let mut out = Vec::with_capacity(bars.len());
            for b in bars {
                let Some(adj) = b.adj_close else {
                    return Err(CoreError::InvalidArgument(format!(
                        "序列 {} 请求复权价, 但来源 '{}' 不提供复权收盘 (adj_close)",
                        key, key.source
                    )));
                };
                if b.kline.close.is_zero() {
                    return Err(CoreError::InvalidArgument(format!(
                        "序列 {key} 出现收盘价为 0 的 bar, 无法计算复权比例"
                    )));
                }
                let ratio = adj / b.kline.close;
                out.push(Kline {
                    open_time: b.kline.open_time,
                    open: b.kline.open * ratio,
                    high: b.kline.high * ratio,
                    low: b.kline.low * ratio,
                    close: adj,
                    volume: b.kline.volume,
                    close_time: b.kline.close_time,
                });
            }
            Ok(out)
        }
    }
}

/// 一个序列的缓存行 —— 与 `data_klines` 表同形(`Kline` + 可选复权收盘)。
pub type CachedBar = Bar;

/// 数据服务: 源注册表 + 本地缓存 + 回补。
pub struct DataHub {
    registry: SourceRegistry,
    db: Database,
    /// 源级最小请求间隔(限速); `Duration::ZERO` = 不限速。
    min_interval: Duration,
    last_call: Mutex<HashMap<&'static str, Instant>>,
    /// 同键并发去重: 同一序列同时只允许一次回补。
    key_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 已确认"头部没有更多数据"的序列(见模块级说明)。
    head_checked: Mutex<HashSet<String>>,
}

impl DataHub {
    /// 新建(不限速)。
    pub fn new(registry: SourceRegistry, db: Database) -> Self {
        Self {
            registry,
            db,
            min_interval: Duration::ZERO,
            last_call: Mutex::new(HashMap::new()),
            key_locks: Mutex::new(HashMap::new()),
            head_checked: Mutex::new(HashSet::new()),
        }
    }

    /// 设置源级最小请求间隔(链式)。
    pub fn with_min_interval(mut self, min_interval: Duration) -> Self {
        self.min_interval = min_interval;
        self
    }

    /// 本地库句柄(装配层做与数据无关的读写时用)。
    #[cfg(test)]
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// 按序列键取源; 未知来源名报错并列出可用来源。
    pub fn require(&self, key: &SeriesKey) -> CoreResult<Arc<dyn ricow_core::KlineSource>> {
        self.registry.require(&key.source)
    }

    /// 读本地缓存 `[from_ms, to_ms)`, 升序。
    pub async fn load_cached(
        &self,
        key: &SeriesKey,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<CachedBar>> {
        let rows = self
            .db
            .get_data_klines(key, from_ms, to_ms)
            .await
            .map_err(|e| CoreError::InvalidArgument(format!("读 data_klines 失败: {e}")))?;
        Ok(rows.into_iter().map(|(k, adj)| Bar::with_adj_close(k, adj)).collect())
    }

    /// 读本地缓存尾窗(最近 `n` 根, 升序)。
    pub async fn load_tail(&self, key: &SeriesKey, n: u32) -> CoreResult<Vec<CachedBar>> {
        let rows = self
            .db
            .data_klines_tail(key, n)
            .await
            .map_err(|e| CoreError::InvalidArgument(format!("读 data_klines 失败: {e}")))?;
        Ok(rows.into_iter().map(|(k, adj)| Bar::with_adj_close(k, adj)).collect())
    }

    /// 读本地缓存尾窗, 但只取**截至 `before_ms` 已收盘**的 bar(`close_time <= before_ms`)。
    ///
    /// 回测无前视的读取口径: 与 [`Self::load_tail`] 的差别只在 SQL 多一个 `close_time` 上界。
    pub async fn load_tail_before(
        &self,
        key: &SeriesKey,
        n: u32,
        before_ms: i64,
    ) -> CoreResult<Vec<CachedBar>> {
        let rows = self
            .db
            .data_klines_tail_before(key, n, before_ms)
            .await
            .map_err(|e| CoreError::InvalidArgument(format!("读 data_klines 失败: {e}")))?;
        Ok(rows.into_iter().map(|(k, adj)| Bar::with_adj_close(k, adj)).collect())
    }

    /// 序列窗口装载 (028 FR-005/FR-013) —— 装配层的唯一取数入口。
    ///
    /// 三条路径:
    /// 1. 源**原生支持**请求周期 → 直读尾窗(口径 `native`);
    /// 2. 源不支持、但有更细粒度能整除它 → 用**最粗的可整除粒度**取够 `n × 比例` 根再重采样,
    ///    口径如实标 `resampled`(策略据此知道"这条不是我原生要的粒度");
    /// 3. 都不行 → 报错并列出该源支持的周期(不猜、不静默降级)。
    ///
    /// `allow_fetch`: `false`(回测) 只读本地库 —— 缺数据不联网(D3 可复现底线), 由装配层报
    /// 明确错误并提示 `ricow data pull`; `true`(Dry Run / 实盘) 先回补缺口再读。
    ///
    /// `before_ms`: 可见性单点 —— 只有 `close_time <= before_ms` 的 bar 会返回。
    pub async fn load_series_window(
        &self,
        key: &SeriesKey,
        n: u32,
        before_ms: i64,
        price_mode: PriceMode,
        allow_fetch: bool,
    ) -> CoreResult<SeriesWindow> {
        let source = self.require(key)?;
        let supported = source.supported_intervals();

        if supported.contains(&key.interval) {
            let bars = self.read_window(key, n, before_ms, price_mode, allow_fetch).await?;
            return Ok(SeriesWindow::native(bars, key.interval));
        }

        let feed = supported
            .iter()
            .copied()
            .filter(|iv| iv.ms() < key.interval.ms() && key.interval.ms() % iv.ms() == 0)
            .max_by_key(|iv| iv.ms());
        let Some(feed) = feed else {
            return Err(CoreError::InvalidArgument(format!(
                "序列 {key}: 来源 '{}' 不支持周期 '{}' 且没有可整除的更细粒度可合成; 该源支持: {}",
                key.source,
                key.interval.label(),
                supported.iter().map(|iv| iv.label()).collect::<Vec<_>>().join(", ")
            )));
        };

        let ratio = key.interval.ms() / feed.ms();
        let feed_key =
            SeriesKey { source: key.source.clone(), symbol: key.symbol.clone(), interval: feed };
        // 多取一个桶的量: 重采样只保留完整桶, 多出来的部分正好被丢掉。
        let need = (i64::from(n) * ratio + ratio).clamp(1, i64::from(u32::MAX)) as u32;
        let raw = self.read_window(&feed_key, need, before_ms, price_mode, allow_fetch).await?;
        let mut bars = resample_to_interval(&raw, key.interval);
        // 合成后的桶也要过一遍可见性: 桶的 close_time 不能越过 before_ms。
        bars.retain(|b| b.close_time.timestamp_millis() <= before_ms);
        if bars.len() > n as usize {
            bars.drain(..bars.len() - n as usize);
        }
        Ok(SeriesWindow::resampled(bars, feed))
    }

    /// 读尾窗(必要时先回补缺口), 并把复权列按 `price_mode` 折叠成策略可见的 OHLC。
    async fn read_window(
        &self,
        key: &SeriesKey,
        n: u32,
        before_ms: i64,
        price_mode: PriceMode,
        allow_fetch: bool,
    ) -> CoreResult<Vec<Kline>> {
        if allow_fetch && n > 0 {
            // 缺口余量取 2 倍窗口长度: 覆盖周期缺口(周末/停牌)而不至于拉过量。
            let span = i64::from(n).saturating_mul(key.interval.ms()).saturating_mul(2);
            let from = before_ms.saturating_sub(span);
            self.ensure_cached(key, from, before_ms).await?;
        }
        let bars = self.load_tail_before(key, n, before_ms).await?;
        apply_price_mode(bars, price_mode, key)
    }

    /// 确保本地缓存覆盖 `[from_ms, to_ms)` —— 缺口才回源, 命中零请求。
    ///
    /// 返回本次**从源拉取并新写入**的行数。同一序列并发调用只会触发一次回补(同键去重)。
    pub async fn ensure_cached(
        &self,
        key: &SeriesKey,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<u64> {
        // 未知来源名在此先报错(不进入锁与网络)。
        self.require(key)?;
        if from_ms >= to_ms {
            return Ok(0);
        }
        let lock = self.key_lock(&key.cache_key());
        let _guard = lock.lock().await;

        let step = key.interval.ms();
        let cached = self.load_cached(key, from_ms, to_ms).await?;
        let mut fetched = 0u64;

        // 头部缺口: 本地最早一根晚于 from → 补 [from, first_open)
        let head_key = key.cache_key();
        let head_pending = !self.head_checked.lock().expect("head_checked").contains(&head_key);
        if let Some(first) = cached.first() {
            let first_ms = first.kline.open_time.timestamp_millis();
            if head_pending && first_ms > from_ms {
                fetched += self.fetch_from_source(key, from_ms, first_ms).await?;
            }
            self.head_checked.lock().expect("head_checked").insert(head_key);
        } else if head_pending {
            // 本地全空: 从 from 一直补到 to。
            fetched += self.fetch_from_source(key, from_ms, to_ms).await?;
            self.head_checked.lock().expect("head_checked").insert(key.cache_key());
            return Ok(fetched);
        }

        // 尾部缺口: 水位 + 周期 < to → 补 (watermark, to)
        let watermark = self
            .db
            .data_kline_watermark(key)
            .await
            .map_err(|e| CoreError::InvalidArgument(format!("读水位失败: {e}")))?;
        if let Some(w) = watermark {
            let next = w + step;
            if next < to_ms {
                fetched += self.fetch_from_source(key, next, to_ms).await?;
            }
        }
        Ok(fetched)
    }

    /// 显式从源拉取 `[from_ms, to_ms)` 并写库(不做缺口分析); 返回新增行数。
    ///
    /// 供 `ricow data pull`(T013)与回补缺口共同使用。
    pub async fn fetch_from_source(
        &self,
        key: &SeriesKey,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<u64> {
        let source = self.require(key)?;
        if from_ms >= to_ms {
            return Ok(0);
        }
        self.throttle(source.name()).await;
        let bars = source.fetch_bars(&key.symbol, key.interval, from_ms, to_ms).await?;
        if bars.is_empty() {
            return Ok(0);
        }
        // 复权列随 bar 一起落库 (T012): Yahoo 源有值, Nasdaq/币安写 NULL。
        let rows: Vec<(Kline, Option<Decimal>)> = bars.into_iter().map(Bar::into_parts).collect();
        self.db
            .insert_data_klines(key, &rows)
            .await
            .map_err(|e| CoreError::InvalidArgument(format!("写 data_klines 失败: {e}")))
    }

    /// 源级限速: 与上次该源请求间隔不足 `min_interval` 时补足等待。
    async fn throttle(&self, source: &'static str) {
        if self.min_interval.is_zero() {
            return;
        }
        let wait = {
            let mut last = self.last_call.lock().expect("last_call");
            let now = Instant::now();
            let wait = match last.get(source) {
                Some(prev) => self.min_interval.saturating_sub(now.duration_since(*prev)),
                None => Duration::ZERO,
            };
            last.insert(source, now + wait);
            wait
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// 同键互斥锁(标记"该序列正在回补")。
    fn key_lock(&self, cache_key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.key_locks.lock().expect("key_locks");
        locks
            .entry(cache_key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use ricow_core::{Interval, Kline};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 纯逻辑替身(已知向量式): 生成固定 bar 并记录调用次数/区间; 不替代任何真实数据源路径。
    struct FakeSource {
        name: &'static str,
        bars: Vec<Kline>,
        calls: Arc<AtomicUsize>,
    }

    impl FakeSource {
        fn make(
            name: &'static str,
            bars: Vec<Kline>,
        ) -> (Arc<dyn ricow_core::KlineSource>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let src = Arc::new(Self { name, bars, calls: calls.clone() });
            (src, calls)
        }
    }

    #[async_trait]
    impl ricow_core::KlineSource for FakeSource {
        fn name(&self) -> &'static str {
            self.name
        }

        fn supported_intervals(&self) -> &'static [Interval] {
            &[Interval::H1]
        }

        async fn fetch_klines(
            &self,
            _symbol: &str,
            _interval: Interval,
            from_ms: i64,
            to_ms: i64,
        ) -> CoreResult<Vec<Kline>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
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

    fn bar(open_ms: i64) -> Kline {
        Kline {
            open_time: DateTime::<Utc>::from_timestamp_millis(open_ms).unwrap(),
            open: Decimal::from(100),
            high: Decimal::from(101),
            low: Decimal::from(99),
            close: Decimal::from(100),
            volume: Decimal::from(1),
            close_time: DateTime::<Utc>::from_timestamp_millis(open_ms + 3_599_999).unwrap(),
        }
    }

    fn hourly_bars(n: i64) -> Vec<Kline> {
        (0..n).map(|i| bar(i * 3_600_000)).collect()
    }

    fn key() -> SeriesKey {
        SeriesKey::new("test_src", "TESTSYM", Interval::H1).unwrap()
    }

    async fn hub_with(bars: Vec<Kline>) -> (DataHub, Arc<AtomicUsize>) {
        let (src, calls) = FakeSource::make("test_src", bars);
        let mut registry = SourceRegistry::new();
        registry.register(src);
        let db = Database::open_in_memory().await.unwrap();
        (DataHub::new(registry, db), calls)
    }

    #[tokio::test]
    async fn test_ensure_cached_fetches_then_hits_without_network() {
        let (hub, calls) = hub_with(hourly_bars(10)).await;
        let k = key();
        let fetched = hub.ensure_cached(&k, 0, 10 * 3_600_000).await.unwrap();
        assert_eq!(fetched, 10, "首次回补 10 根");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "首次一次请求");

        let again = hub.ensure_cached(&k, 0, 10 * 3_600_000).await.unwrap();
        assert_eq!(again, 0, "命中零新增");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "命中不得再发请求");
    }

    #[tokio::test]
    async fn test_ensure_cached_backfills_only_tail_gap() {
        let (hub, calls) = hub_with(hourly_bars(10)).await;
        let k = key();
        // 先缓存 0..5
        hub.ensure_cached(&k, 0, 5 * 3_600_000).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 再要 0..10 → 只补尾部缺口 (5..10)
        let fetched = hub.ensure_cached(&k, 0, 10 * 3_600_000).await.unwrap();
        assert_eq!(fetched, 5, "只补尾部 5 根");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let rows = hub.load_cached(&k, 0, 10 * 3_600_000).await.unwrap();
        assert_eq!(rows.len(), 10);
    }

    #[tokio::test]
    async fn test_ensure_cached_head_gap_checked_once_per_process() {
        let (hub, calls) = hub_with(hourly_bars(10)).await;
        let k = key();
        // 先把 5..10 写进本地(直接写库, 模拟"只有后半段")
        hub.db().insert_data_klines_plain(&k, &hourly_bars(10)[5..10]).await.unwrap();

        // 要 0..10: 头部缺口 → 补 [0, 5h)
        let fetched = hub.ensure_cached(&k, 0, 10 * 3_600_000).await.unwrap();
        assert_eq!(fetched, 5, "补头部 5 根");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 再要一次更早的区间: 头部已确认过 → 不再重复探测
        let again = hub.ensure_cached(&k, 0, 10 * 3_600_000).await.unwrap();
        assert_eq!(again, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "头部只探测一次(进程内)");
    }

    #[tokio::test]
    async fn test_unknown_source_error_lists_available() {
        let (hub, _) = hub_with(hourly_bars(1)).await;
        let bad = SeriesKey::new("stooq", "QQQ", Interval::D1).unwrap();
        let err = match hub.ensure_cached(&bad, 0, 86_400_000).await {
            Ok(_) => panic!("未知来源名必须报错"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("未知数据源 'stooq'"), "{err}");
        assert!(err.contains("test_src"), "{err}");
    }

    #[tokio::test]
    async fn test_concurrent_same_key_backfills_once() {
        let (hub, calls) = hub_with(hourly_bars(10)).await;
        let hub = Arc::new(hub);
        let k = key();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let hub = hub.clone();
            let k = k.clone();
            handles.push(tokio::spawn(async move {
                hub.ensure_cached(&k, 0, 10 * 3_600_000).await.unwrap()
            }));
        }
        let mut total = 0u64;
        for h in handles {
            total += h.await.unwrap();
        }
        assert_eq!(total, 10, "并发四次共写入 10 行(其余命中)");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "同键并发只回源一次");
    }

    #[tokio::test]
    async fn test_rate_limit_spaces_requests() {
        let (src, _) = FakeSource::make("test_src", hourly_bars(10));
        let mut registry = SourceRegistry::new();
        registry.register(src);
        let db = Database::open_in_memory().await.unwrap();
        let hub = DataHub::new(registry, db).with_min_interval(Duration::from_millis(120));
        let k = key();
        // 两次不同的取数请求(不同窗口, 避开去重)应被拉开 ≥ min_interval。
        let t0 = Instant::now();
        hub.fetch_from_source(&k, 0, 3_600_000).await.unwrap();
        hub.fetch_from_source(&k, 3_600_000, 7_200_000).await.unwrap();
        assert!(
            t0.elapsed() >= Duration::from_millis(100),
            "限速应拉开请求间隔, 实测 {:?}",
            t0.elapsed()
        );
    }

    #[tokio::test]
    async fn test_fetch_from_source_empty_result_is_not_an_error() {
        let (hub, calls) = hub_with(Vec::new()).await;
        let k = key();
        let fetched = hub.fetch_from_source(&k, 0, 3_600_000).await.unwrap();
        assert_eq!(fetched, 0, "源无数据 = 0 行, 不是错误");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(hub.load_tail(&k, 10).await.unwrap().len(), 0);
    }

    // ---- 区间结果统一点 `normalize_range` (T010 起各源共用) ----

    fn kb(open_ms: i64) -> Kline {
        Kline {
            open_time: DateTime::<Utc>::from_timestamp_millis(open_ms).unwrap(),
            open: Decimal::from(1),
            high: Decimal::from(1),
            low: Decimal::from(1),
            close: Decimal::from(1),
            volume: Decimal::from(1),
            close_time: DateTime::<Utc>::from_timestamp_millis(open_ms + 3_599_999).unwrap(),
        }
    }

    #[test]
    fn test_normalize_range_is_half_open_sorted_deduped() {
        let out = normalize_range(
            vec![kb(7_200_000), kb(0), kb(3_600_000), kb(3_600_000), kb(7_200_000)],
            0,
            7_200_000,
        );
        let times: Vec<i64> = out.iter().map(|b| b.open_time.timestamp_millis()).collect();
        assert_eq!(times, vec![0, 3_600_000], "to 边界被裁掉, 重复去掉, 升序");
    }

    #[test]
    fn test_normalize_range_clips_both_ends() {
        let out = normalize_range(vec![kb(0), kb(3_600_000), kb(7_200_000)], 3_600_000, 7_200_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].open_time.timestamp_millis(), 3_600_000);
    }

    #[test]
    fn test_normalize_range_empty_input() {
        assert!(normalize_range(Vec::new(), 0, 3_600_000).is_empty());
    }

    // ---- 价格口径 (T012/FR-015) ----

    fn bar_with_adj(close: &str, adj: &str) -> Bar {
        let k = kb(0);
        Bar::with_adj_close(
            Kline {
                open: Decimal::from(100),
                high: Decimal::from(110),
                low: Decimal::from(90),
                close: Decimal::from_str(close).unwrap(),
                ..k
            },
            Some(Decimal::from_str(adj).unwrap()),
        )
    }

    #[test]
    fn test_apply_price_mode_adjclose_scales_ohlc() {
        let key = SeriesKey::new("yahoo", "QQQ", Interval::D1).unwrap();
        // close=100, adj=50 → ratio 0.5: OHLC 同步缩放, close 取复权值。
        let out = apply_price_mode(vec![bar_with_adj("100", "50")], PriceMode::AdjClose, &key)
            .expect("有复权列");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].close, Decimal::from(50));
        assert_eq!(out[0].open, Decimal::from(50), "open 100 × 0.5");
        assert_eq!(out[0].high, Decimal::from(55), "high 110 × 0.5");
        assert_eq!(out[0].low, Decimal::from(45), "low 90 × 0.5");
    }

    #[test]
    fn test_apply_price_mode_close_is_passthrough() {
        let key = SeriesKey::new("yahoo", "QQQ", Interval::D1).unwrap();
        let out =
            apply_price_mode(vec![bar_with_adj("100", "50")], PriceMode::Close, &key).unwrap();
        assert_eq!(out[0].close, Decimal::from(100), "原始价不动");
    }

    #[test]
    fn test_apply_price_mode_adjclose_errors_when_source_has_none() {
        let key = SeriesKey::new("nasdaq", "QQQ", Interval::D1).unwrap();
        let err = apply_price_mode(vec![Bar::new(kb(0))], PriceMode::AdjClose, &key).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("不提供复权收盘"), "{msg}");
        assert!(msg.contains("nasdaq"), "错误里点出来源名: {msg}");
    }

    #[test]
    fn test_normalize_bars_keeps_adj_close_on_dedupe() {
        let bars = vec![bar_with_adj("100", "50"), bar_with_adj("100", "50")];
        let out = normalize_bars(bars, 0, 3_600_000);
        assert_eq!(out.len(), 1, "同 open_time 去重");
        assert_eq!(out[0].adj_close, Some(Decimal::from(50)), "复权列保留");
    }

    // ---- 028 T024: 序列窗口装载(可见性割断 / 重采样回退 / 不支持周期报错) ----

    /// 支持粒度可定制的替身(重采样与"不支持"路径用): 只原生提供 `supported` 里的周期。
    struct MultiSource {
        name: &'static str,
        supported: &'static [Interval],
        bars: Vec<Kline>,
    }

    #[async_trait]
    impl ricow_core::KlineSource for MultiSource {
        fn name(&self) -> &'static str {
            self.name
        }

        fn supported_intervals(&self) -> &'static [Interval] {
            self.supported
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

    async fn hub_with_source(src: Arc<dyn ricow_core::KlineSource>) -> DataHub {
        let mut registry = SourceRegistry::new();
        registry.register(src);
        let db = Database::open_in_memory().await.unwrap();
        DataHub::new(registry, db)
    }

    /// 可见性单点 = `close_time`: 未收盘的那根不可见(FR-013)。
    #[tokio::test]
    async fn test_load_series_window_cuts_at_close_time() {
        let (hub, _) = hub_with(hourly_bars(5)).await;
        let k = key();
        hub.ensure_cached(&k, 0, 5 * 3_600_000).await.unwrap();

        // 第 5 根(open 14:00)尚未收盘 → 只能看到 4 根
        let before = 4 * 3_600_000 - 1;
        let w = hub.load_series_window(&k, 10, before, PriceMode::Close, false).await.unwrap();
        assert_eq!(w.bars.len(), 4, "未收盘的 bar 不可见");
        assert_eq!(w.mode, ricow_core::SeriesMode::Native);
        assert_eq!(w.feed_interval, Interval::H1);

        // 时刻走到第 5 根收盘 → 5 根都在
        let w =
            hub.load_series_window(&k, 10, 5 * 3_600_000, PriceMode::Close, false).await.unwrap();
        assert_eq!(w.bars.len(), 5);
    }

    /// 源不原生提供该周期时: 用最粗的可整除粒度合成, 口径如实标 `resampled`(FR-005)。
    #[tokio::test]
    async fn test_load_series_window_resamples_when_not_native() {
        let mut bars = Vec::new();
        for i in 0..8i64 {
            let open = i * 900_000;
            let mut b = bar(open);
            b.close_time = DateTime::<Utc>::from_timestamp_millis(open + 899_999).unwrap();
            bars.push(b);
        }
        let hub = hub_with_source(Arc::new(MultiSource {
            name: "q15",
            supported: &[Interval::M15],
            bars,
        }))
        .await;
        let m15 = SeriesKey::new("q15", "SYM", Interval::M15).unwrap();
        hub.ensure_cached(&m15, 0, 8 * 900_000).await.unwrap();

        let h1 = SeriesKey::new("q15", "SYM", Interval::H1).unwrap();
        let w =
            hub.load_series_window(&h1, 10, 8 * 900_000, PriceMode::Close, false).await.unwrap();
        assert_eq!(w.mode, ricow_core::SeriesMode::Resampled, "合成的要标明口径");
        assert_eq!(w.feed_interval, Interval::M15, "实际取数粒度");
        assert_eq!(w.bars.len(), 2, "8 根 15m = 2 个完整 1h 桶");
        assert_eq!(w.bars[0].open_time.timestamp_millis(), 0);
        assert_eq!(w.bars[1].open_time.timestamp_millis(), 3_600_000);
    }

    /// 既不原生支持、又没有可整除的细粒度 → 报错并列出该源支持的周期(不静默降级)。
    #[tokio::test]
    async fn test_load_series_window_errors_when_interval_unsupported_and_not_divisible() {
        let hub = hub_with_source(Arc::new(MultiSource {
            name: "h1only",
            supported: &[Interval::H1],
            bars: hourly_bars(3),
        }))
        .await;
        let k = SeriesKey::new("h1only", "SYM", Interval::M30).unwrap();
        let err = hub
            .load_series_window(&k, 10, 10 * 3_600_000, PriceMode::Close, false)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("不支持周期 '30m'"), "{msg}");
        assert!(msg.contains("1h"), "要列出该源支持的周期: {msg}");
    }

    /// 回测口径 (`allow_fetch=false`) 缺数据时不联网: 源一次都不该被调用(D3 可复现底线)。
    #[tokio::test]
    async fn test_load_series_window_readonly_never_calls_source() {
        let (hub, calls) = hub_with(hourly_bars(3)).await;
        let k = key();
        let w =
            hub.load_series_window(&k, 10, 10 * 3_600_000, PriceMode::Close, false).await.unwrap();
        assert!(w.bars.is_empty(), "本地库为空 → 空窗口");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "只读路径零网络请求");
    }
}
