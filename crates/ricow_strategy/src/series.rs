//! 序列句柄 (028 T017): 策略声明"我要哪条 K 线", 拿回一个可算指标的句柄。
//!
//! 为什么是**序列级**而不是 pair 级: 旧模型里指标绑定 `ctx` 的单个 pair, 于是"多标的 ×
//! 多周期 × 多来源"每加一种组合就得加一条通道。改成"句柄绑定一条序列"
//! (`(source, symbol, interval)`) 之后, 组合由策略自己拼, 引擎不再有组合爆炸。
//!
//! 窗口口径 (FR-010): 默认 [`SERIES_WINDOW_DEFAULT`] 根尾窗; 策略给 `bars` 就按 `bars`;
//! 下限 = `min_bars`(策略按自己指标算好, 至少 [`SERIES_WINDOW_MIN`]); 上限
//! [`SERIES_WINDOW_MAX`](资源护栏, 超出即**截到上限**, 由装配层记日志)。
//!
//! 指标实现与 `ctx:ema/atr/...` 同源(都调 [`crate::indicators_api`]), 因此**同一输入长度**
//! 下逐位一致; 输入更长时结果不同属预期(EMA 种子/窗口长度差异), 见 T018 用例。
//!
//! `stale` (FR-016): 增量回补失败时置位 —— 策略可见(不去猜数据是不是新的), 是否据此收敛
//! 由策略决定。

use std::collections::HashMap;

use ricow_core::{CoreError, CoreResult, Interval, Kline, PriceMode, SeriesKey, SeriesMode};
use serde::{Deserialize, Serialize};

use crate::indicators_api;

/// 尾窗默认根数(策略没声明 `bars` 时)。
pub const SERIES_WINDOW_DEFAULT: usize = 300;
/// 尾窗下限(至少两根才能谈"上一根收盘")。
pub const SERIES_WINDOW_MIN: usize = 2;
/// 尾窗上限(资源护栏; 超出即截到上限)。
pub const SERIES_WINDOW_MAX: usize = 5000;

/// 同一策略可并持的序列条数上限 (028 FR-012)。
///
/// 为什么要有硬上限: 每条序列都要按窗口装载 + 每 tick 维持尾窗, 32 条已经远超任何实际
/// 策略(多标的 × 多周期), 再多基本是脚本写错(循环里声明); 硬报错比"静默吃内存"好定位。
pub const MAX_SERIES_PER_STRATEGY: usize = 32;

/// 序列声明: 策略要哪条序列、要多少根、用什么价、是否驱动 `on_bar`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesDecl {
    /// 策略侧标识(回调 `on_bar(ctx, id, bar)` 与句柄取回都用它)。
    pub id: String,
    /// 数据服务的序列键 `(source, symbol, interval)`。
    pub key: SeriesKey,
    /// 请求尾窗根数; `None` = 默认 300。
    pub bars: Option<usize>,
    /// 策略指标所需最小根数(策略自己算), 用于下限保护; `None` = 不设额外下限。
    pub min_bars: Option<usize>,
    /// 价格口径(复权/原始)。
    pub price_mode: PriceMode,
    /// true = 该序列收盘驱动 `on_bar`(默认 true)。
    pub drive: bool,
}

impl SeriesDecl {
    /// 新建(id + 序列键, 其余取默认)。
    pub fn new(id: &str, key: SeriesKey) -> Self {
        Self {
            id: id.to_string(),
            key,
            bars: None,
            min_bars: None,
            price_mode: PriceMode::Close,
            drive: true,
        }
    }

    /// 声明尾窗根数。
    pub fn with_bars(mut self, bars: usize) -> Self {
        self.bars = Some(bars);
        self
    }

    /// 声明指标所需最小根数。
    pub fn with_min_bars(mut self, min_bars: usize) -> Self {
        self.min_bars = Some(min_bars);
        self
    }

    /// 声明价格口径。
    pub fn with_price_mode(mut self, price_mode: PriceMode) -> Self {
        self.price_mode = price_mode;
        self
    }

    /// 是否驱动 `on_bar`。
    pub fn with_drive(mut self, drive: bool) -> Self {
        self.drive = drive;
        self
    }

    /// 实际生效的尾窗根数: 默认 300 → 抬到下限 → 压到上限。
    pub fn effective_window(&self) -> usize {
        let want = self.bars.unwrap_or(SERIES_WINDOW_DEFAULT);
        let floor = self.min_bars.unwrap_or(0).max(SERIES_WINDOW_MIN);
        want.max(floor).min(SERIES_WINDOW_MAX)
    }

    /// FR-011: 声明的窗口/预热根数超出上限时给出**可报错的说明**(返回 `Some(说明)`)。
    ///
    /// 口径按 spec 字面: **超上限报错**, 不是静默截断 —— 策略以为有 10 万根、实际只拿到 5000 根
    /// 会让指标与信号静默变味(审核发现原实现是截断 + warn)。
    pub fn window_error(&self) -> Option<String> {
        let want = self.bars.unwrap_or(SERIES_WINDOW_DEFAULT);
        let min = self.min_bars.unwrap_or(0);
        if want > SERIES_WINDOW_MAX {
            return Some(format!(
                "序列 '{}' 的 bars={} 超过上限 {} 根",
                self.id, want, SERIES_WINDOW_MAX
            ));
        }
        if min > SERIES_WINDOW_MAX {
            return Some(format!(
                "序列 '{}' 的 min_bars={} 超过上限 {} 根",
                self.id, min, SERIES_WINDOW_MAX
            ));
        }
        None
    }

    /// 校验(空 id / 空 symbol 等)。
    pub fn validate(&self) -> CoreResult<()> {
        if self.id.trim().is_empty() {
            return Err(CoreError::InvalidArgument("序列声明缺少 id".into()));
        }
        if self.id.contains('|') {
            return Err(CoreError::InvalidArgument(format!("序列 id 不能含 '|': {}", self.id)));
        }
        Ok(())
    }
}

/// 序列句柄(策略侧视图): 尾窗 K 线 + 12 个指标。
pub struct Series {
    decl: SeriesDecl,
    bars: Vec<Kline>,
    stale: bool,
}

impl Series {
    /// 用装配层装载的 K 线建句柄(自动截到 `effective_window()` 根尾窗)。
    pub fn new(decl: SeriesDecl, mut bars: Vec<Kline>) -> Self {
        let window = decl.effective_window();
        if bars.len() > window {
            bars.drain(..bars.len() - window);
        }
        Self { decl, bars, stale: false }
    }

    pub fn decl(&self) -> &SeriesDecl {
        &self.decl
    }

    pub fn id(&self) -> &str {
        &self.decl.id
    }

    pub fn bars(&self) -> &[Kline] {
        &self.bars
    }

    /// 尾窗 n 根(升序); `n > len` 时返回全段。
    ///
    /// 给"策略自己算自定义口径"用(028 T044: 平台提供数据、策略自己算 —— 例如 VWAP),
    /// 与 `Series::close(back)` 一样只读、不改变句柄内容。
    pub fn tail(&self, n: usize) -> Vec<Kline> {
        let start = self.bars.len().saturating_sub(n);
        self.bars[start..].to_vec()
    }

    /// 最后一根(已收盘)。
    pub fn last(&self) -> Option<&Kline> {
        self.bars.last()
    }

    /// 倒数第 `back` 根的收盘价(`back = 0` → 最后一根)。
    pub fn close(&self, back: usize) -> Option<f64> {
        let n = self.bars.len();
        if back >= n {
            return None;
        }
        use rust_decimal::prelude::ToPrimitive;
        self.bars[n - 1 - back].close.to_f64()
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    /// 增量回补失败 → 标脏(策略可见)。
    pub fn mark_stale(&mut self, stale: bool) {
        self.stale = stale;
    }

    /// 该序列当前是否不新鲜。
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// 推进序列: 把"刚收盘的这根"并进尾窗, 保持窗口上限。
    ///
    /// 幂等边界(重要): 引擎在回测/实盘里**每推一根调用一次**, 而装配时装载的尾窗可能已经
    /// 包含这根 —— 因此
    /// - `open_time` 与最后一根相同 → **覆盖**最后一根(同一根重复推送, 不产生重复 bar);
    /// - `open_time` 早于最后一根 → 忽略(迟到 bar 不回插, 避免改写历史与指标);
    /// - 晚于最后一根 → 追加, 并按声明窗口裁掉最老的。
    pub fn push_bar(&mut self, bar: Kline) {
        match self.bars.last() {
            Some(last) if bar.open_time == last.open_time => {
                let n = self.bars.len();
                self.bars[n - 1] = bar;
                return;
            }
            Some(last) if bar.open_time < last.open_time => return,
            _ => {}
        }
        self.bars.push(bar);
        let window = self.decl.effective_window();
        if self.bars.len() > window {
            let drop = self.bars.len() - window;
            self.bars.drain(..drop);
        }
    }
}

/// 12 个指标: 全部委托 [`crate::indicators_api`], 与 `ctx:*` 同源同口径。
macro_rules! series_indicator {
    ($name:ident) => {
        /// 见 `ctx:*` 同名指标(同源实现, 同一输入长度下逐位一致)。
        pub fn $name(&self, period: usize) -> Option<f64> {
            indicators_api::$name(&self.bars, period)
        }
    };
}

impl Series {
    series_indicator!(ema);
    series_indicator!(sma);
    series_indicator!(wma);
    series_indicator!(rsi);
    series_indicator!(atr);
    series_indicator!(adx);
    series_indicator!(stoch);
    series_indicator!(cci);
    series_indicator!(roc);
    series_indicator!(mom);

    /// MACD(周期固定 12/26/9, 与 `ctx:macd` 同源)。
    pub fn macd(&self) -> Option<indicators_api::MacdOutput> {
        indicators_api::macd(&self.bars)
    }

    /// 布林带(`period` + `dev` 倍标准差)。
    pub fn boll(&self, period: usize, dev: f64) -> Option<indicators_api::BollOutput> {
        indicators_api::boll(&self.bars, period, dev)
    }
}

/// `on_bar` 的序列描述 (028 FR-005): 策略据此区分多条序列, 并知道数据的口径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesInfo {
    /// 策略侧标识(声明时的 `id`)。
    pub id: String,
    pub source: String,
    pub symbol: String,
    pub interval: Interval,
    pub price_mode: PriceMode,
    /// 口径: `native`(源原生提供该周期) / `resampled`(平台由更细粒度合成)。
    pub mode: SeriesMode,
    /// 实际取数粒度: `native` 时等于 `interval`; `resampled` 时 = 参与合成的细粒度。
    pub feed_interval: Interval,
}

impl SeriesInfo {
    /// 由声明 + 装载结果构造(装配层调用)。
    pub fn new(decl: &SeriesDecl, mode: SeriesMode, feed_interval: Interval) -> Self {
        Self {
            id: decl.id.clone(),
            source: decl.key.source.clone(),
            symbol: decl.key.symbol.clone(),
            interval: decl.key.interval,
            price_mode: decl.price_mode,
            mode,
            feed_interval,
        }
    }

    /// 由声明构造 `native` 口径(单测与默认路径)。
    pub fn native(decl: &SeriesDecl) -> Self {
        Self::new(decl, SeriesMode::Native, decl.key.interval)
    }

    /// 回到序列键。
    pub fn key(&self) -> SeriesKey {
        SeriesKey {
            source: self.source.clone(),
            symbol: self.symbol.clone(),
            interval: self.interval,
        }
    }
}

/// 声明表(策略侧): `id` → 句柄, 保持声明顺序(装配与日志按声明序输出)。
#[derive(Default)]
pub struct SeriesSet {
    map: HashMap<String, Series>,
    order: Vec<String>,
}

impl SeriesSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入/替换一条序列(同 id 覆盖, 顺序保持首次声明位置)。
    pub fn insert(&mut self, series: Series) {
        let id = series.id().to_string();
        if !self.map.contains_key(&id) {
            self.order.push(id.clone());
        }
        self.map.insert(id, series);
    }

    pub fn get(&self, id: &str) -> Option<&Series> {
        self.map.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Series> {
        self.map.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.map.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Series> {
        self.order.iter().filter_map(|id| self.map.get(id))
    }

    /// 声明顺序的 id 列表。
    pub fn ids(&self) -> &[String] {
        &self.order
    }

    /// 条数(测试/诊断用; 生产路径不需要)。
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 是否为空(与 `len` 成对; 否则 clippy 会报 `len_without_is_empty`)。
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 标记某条序列 stale(回补失败时逐条置位)。
    pub fn mark_stale(&mut self, id: &str, stale: bool) -> bool {
        match self.map.get_mut(id) {
            Some(s) => {
                s.mark_stale(stale);
                true
            }
            None => false,
        }
    }

    /// 所有序列的声明(装配层装载用)。
    pub fn decls(&self) -> Vec<SeriesDecl> {
        self.iter().map(|s| s.decl().clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn key() -> SeriesKey {
        SeriesKey::new("binance_spot", "ETHUSDT", ricow_core::Interval::H1).unwrap()
    }

    fn klines(n: usize) -> Vec<Kline> {
        (0..n)
            .map(|i| {
                let t = DateTime::<Utc>::from_timestamp_millis(i as i64 * 3_600_000).unwrap();
                Kline {
                    open_time: t,
                    open: dec!(100),
                    high: dec!(101),
                    low: dec!(99),
                    close: dec!(100) + Decimal::from(i as u64),
                    volume: dec!(1),
                    close_time: t + chrono::Duration::milliseconds(3_599_999),
                }
            })
            .collect()
    }

    #[test]
    fn test_effective_window_default_floor_cap() {
        let d = SeriesDecl::new("a", key());
        assert_eq!(d.effective_window(), SERIES_WINDOW_DEFAULT, "默认 300");

        let d = SeriesDecl::new("a", key()).with_bars(50);
        assert_eq!(d.effective_window(), 50, "声明就按声明");

        let d = SeriesDecl::new("a", key()).with_bars(1).with_min_bars(20);
        assert_eq!(d.effective_window(), 20, "抬到 min_bars 下限");

        // FR-011 收敛口径: 超上限在**声明期硬报错**(`window_error`), 不再静默截断;
        // `effective_window` 的封顶只作兜底(正常路径进不来)。
        let d = SeriesDecl::new("a", key()).with_bars(99_999);
        assert_eq!(d.effective_window(), SERIES_WINDOW_MAX, "兜底封顶到 5000");
        assert!(d.window_error().is_some(), "超上限必须可报错");
        assert!(SeriesDecl::new("a", key()).with_bars(100).window_error().is_none());
    }

    #[test]
    fn test_series_trims_to_tail_window() {
        let decl = SeriesDecl::new("a", key()).with_bars(10);
        let s = Series::new(decl, klines(50));
        assert_eq!(s.len(), 10, "只留尾窗");
        assert_eq!(s.close(0), Some(149.0), "最后一根的收盘");
        assert_eq!(s.close(9), Some(140.0), "倒数第 10 根");
        assert_eq!(s.close(10), None, "越界 None");
    }

    #[test]
    fn test_series_push_bar_keeps_window() {
        let decl = SeriesDecl::new("a", key()).with_bars(3);
        let mut s = Series::new(decl, klines(3));
        s.push_bar(klines(4).pop().unwrap());
        assert_eq!(s.len(), 3, "追加后仍是尾窗 3 根");
        assert_eq!(s.close(0), Some(103.0));
    }

    #[test]
    fn test_handle_indicators_match_input_length() {
        let bars = klines(300);
        let decl = SeriesDecl::new("a", key()).with_bars(300);
        let s = Series::new(decl, bars.clone());
        // 同源实现: 句柄指标 == 直接用 indicators_api 算(相同输入长度) → 逐位一致
        assert_eq!(s.ema(20), indicators_api::ema(&bars, 20));
        assert_eq!(s.atr(14), indicators_api::atr(&bars, 14));
        assert_eq!(s.rsi(14), indicators_api::rsi(&bars, 14));
        assert_eq!(s.ema(200), indicators_api::ema(&bars, 200));
        assert!(s.ema(20).is_some());
    }

    #[test]
    fn test_handle_ema200_none_when_window_too_short() {
        // 尾窗只有 100 根时 ema(200) 恒 None(数据不足, 与 ctx 口径一致)
        let decl = SeriesDecl::new("a", key()).with_bars(100);
        let s = Series::new(decl, klines(300));
        assert_eq!(s.len(), 100);
        assert_eq!(s.ema(200), None);
    }

    #[test]
    fn test_series_set_order_and_stale() {
        let mut set = SeriesSet::new();
        set.insert(Series::new(SeriesDecl::new("b", key()), klines(5)));
        set.insert(Series::new(SeriesDecl::new("a", key()), klines(5)));
        assert_eq!(set.ids(), &["b".to_string(), "a".to_string()], "保持声明顺序");
        assert_eq!(set.len(), 2);
        assert!(set.mark_stale("a", true));
        assert!(set.get("a").unwrap().is_stale());
        assert!(!set.get("b").unwrap().is_stale(), "只置位目标序列");
        assert!(!set.mark_stale("nope", true), "未知 id 返回 false");
        assert_eq!(set.decls().len(), 2);
    }

    #[test]
    fn test_decl_validate_rejects_blank_and_separator() {
        assert!(SeriesDecl::new("  ", key()).validate().is_err());
        assert!(SeriesDecl::new("a|b", key()).validate().is_err());
        assert!(SeriesDecl::new("qqq_d", key()).validate().is_ok());
    }

    #[test]
    fn test_price_mode_default_is_close() {
        assert_eq!(SeriesDecl::new("a", key()).price_mode, PriceMode::Close);
        assert_eq!(
            SeriesDecl::new("a", key()).with_price_mode(PriceMode::AdjClose).price_mode,
            PriceMode::AdjClose
        );
    }

    /// `push_bar` 的幂等/迟到边界: 重复推同一根不产生重复 bar, 迟到 bar 不回插。
    #[test]
    fn test_push_bar_is_idempotent_and_ignores_late_bars() {
        let decl = SeriesDecl::new("eth", key()).with_bars(3);
        let bars = klines(3);
        let mut s = Series::new(decl, bars.clone());
        assert_eq!(s.len(), 3);

        // 同一根重复推送(引擎装配时已含这根) → 覆盖, 不追加
        let mut same = bars[2].clone();
        same.close = dec!(999);
        s.push_bar(same);
        assert_eq!(s.len(), 3, "重复推同一根不追加");
        assert_eq!(s.last().unwrap().close, dec!(999), "覆盖为最新值");

        // 迟到 bar(早于最后一根) → 忽略
        s.push_bar(bars[1].clone());
        assert_eq!(s.len(), 3);
        assert_eq!(s.last().unwrap().close, dec!(999), "迟到 bar 不改写当前尾根");

        // 新 bar → 追加并裁掉最老
        let mut next = bars[2].clone();
        next.open_time = bars[2].open_time + chrono::Duration::hours(1);
        next.close_time = bars[2].close_time + chrono::Duration::hours(1);
        s.push_bar(next);
        assert_eq!(s.len(), 3, "窗口上限保持");
        assert_eq!(s.bars()[0].open_time, bars[1].open_time, "裁掉最老一根");
    }
}
