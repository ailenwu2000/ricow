//! 序列驱动 (028 T024/FR-004/FR-013): 回测与实盘的"时间轴 × 序列"推进器。
//!
//! 为什么要它: 策略声明的每条序列是**独立时间轴**(日线/1h/5m 各有各的收盘时刻)。回测只能
//! 有一个主时钟, 因此需要一层把主时钟翻译成"此刻哪些 bar 刚刚收盘"。
//!
//! 两条硬规则:
//! 1. **可见性单点 = `close_time`**: 只有 `close_time <= 当前刻度` 的 bar 才会被推给策略 ——
//!    装载时(`EngineHost::load_window`)与推进时(`advance`)用的是同一个判据, 不会出现
//!    "装载按一种口径、推进按另一种口径"的裂缝。
//! 2. **游标单调**: 每条序列各自一个游标, 只前进不后退; 因此同一根 bar 至多派发一次,
//!    且派发顺序 = 时间顺序(不可能"先看到明天再看到今天")。

use ricow_core::{CoreError, CoreResult, SeriesKey};
use ricow_strategy::{SeriesDecl, SeriesInfo};

use super::host_impl::EngineHost;

/// 一条被驱动的序列: 声明 + 装载结果 + 已派发游标。
struct DrivenSeries {
    /// 声明原样保留(实盘增量取数要按同一份声明再读)。
    decl: SeriesDecl,
    info: SeriesInfo,
    bars: Vec<ricow_core::Kline>,
    /// 已派发到第几根(bars[cursor..] 尚未可见)。
    cursor: usize,
    /// 窗口根数(策略可见上限)。
    window: usize,
    /// 上次向数据源取数的墙钟时刻(实盘退避用; 回测路径不设)。
    last_fetch_ms: Option<i64>,
}

/// 一条驱动事件: `(序列描述, 刚收盘的 bar)`。
pub type SeriesBar = (SeriesInfo, ricow_core::Kline);
/// 一轮增量取数里某条序列的结果: `(序列 id, 是否取数成功)`。
pub type SeriesHealth = (String, bool);

/// 按声明驱动多条序列。
pub struct SeriesDriver {
    series: Vec<DrivenSeries>,
}

impl SeriesDriver {
    /// 空驱动(无声明 —— 旧路径: 策略自己用 `ctx:klines`)。
    pub fn empty() -> Self {
        Self { series: Vec::new() }
    }

    /// 按声明装载 `[from_ms, to_ms]` 全段。
    ///
    /// 为什么整段装载而不是每刻度读库: 回测里每个刻度都查库会放大 I/O; 整段装载一次后
    /// 用游标推进, 复杂度 = O(总 bar 数)。代价是内存, 因此 `n` 由声明窗口 + 预热推出。
    ///
    /// **预热不派发**: `close_time < from_ms` 的 bar 是预热段(策略句柄在声明期已由宿主
    /// 预填, 指标因此是热的), 游标直接从窗口内第一根起 —— 否则策略会在回测窗口开始前
    /// 收到一堆 `on_bar`(旧 bar 上做决策 = 无意义交易)。
    ///
    /// 缺数据/过短的错误文案都带 `ricow data pull` 命令(FR-031)。
    pub fn load(
        host: &EngineHost,
        decls: &[SeriesDecl],
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Self> {
        let mut series = Vec::with_capacity(decls.len());
        for decl in decls {
            let window = decl.effective_window();
            // 读取区间 = [from, to], 但尾窗按声明窗口裁; 预热段由调用方给的 from 覆盖。
            let span = (to_ms - from_ms).max(0);
            let step = decl.key.interval.ms().max(1);
            let base = (span / step) as usize + window + 1;
            // 读取根数至少覆盖 `min_bars + 1`(否则"过短"误报会出现在数据其实够的场景)。
            let need = base.max(decl.min_bars.unwrap_or(0) + 1).min(u32::MAX as usize) as u32;
            let mut loaded = host.load_window(decl, need, to_ms)?;
            // 028 审核修复: 窗口右端必须生效 —— 原来只把 `to_ms` 用于"读多少根", 于是回测会一路
            // 跑到本地库最后一根(`--days 30` 却跑出 90 天)。窗口口径 = `[from_ms, to_ms)`(半开)。
            loaded.bars.retain(|b| b.open_time.timestamp_millis() < to_ms);

            if loaded.bars.is_empty() {
                return Err(CoreError::InvalidArgument(format!(
                    "序列 {} 在本地库没有可用数据(区间 {} → {}); 先拉取: \
                     ricow data pull --source {} --symbol {} --interval {}",
                    decl.key,
                    ms_to_date(from_ms),
                    ms_to_date(to_ms),
                    decl.key.source,
                    decl.key.symbol,
                    decl.key.interval.label()
                )));
            }
            if let Some(min) = decl.min_bars {
                if loaded.bars.len() < min {
                    return Err(CoreError::InvalidArgument(format!(
                        "序列 {} 过短: 区间内只有 {} 根, 声明需要至少 {} 根(指标预热不足); \
                         先补更长的历史: ricow data pull --source {} --symbol {} --interval {}",
                        decl.key,
                        loaded.bars.len(),
                        min,
                        decl.key.source,
                        decl.key.symbol,
                        decl.key.interval.label()
                    )));
                }
            }
            let info = SeriesInfo::new(decl, loaded.mode, loaded.feed_interval);
            // 预热段 = `close_time <= from_ms` 的 bar(不派发; 策略句柄声明期已按同一条判据装载)。
            // ⚠️ 这里必须是 `<=` 而不是 `<`: 装载侧是 `close_time <= before_ms`, 若两边等号方向
            // 不一致, 某根 close_time **恰好等于**窗口起点的 bar 会既进句柄又被派发一次
            // (审核实测复现: 预热根被重复交付, 策略无法区分"刚收盘"与"启动时已可见")。
            let cursor =
                loaded.bars.partition_point(|b| b.close_time.timestamp_millis() <= from_ms);
            series.push(DrivenSeries {
                decl: decl.clone(),
                info,
                bars: loaded.bars,
                cursor,
                window,
                last_fetch_ms: None,
            });
        }
        Ok(Self { series })
    }

    /// **实盘/Dry Run** 推进: 先按需向宿主**再取**新收盘的 bar, 再走游标派发。
    ///
    /// 为什么不能只吃装载时那一份: 回测的数据是整段已知的, 而实盘的数据是**长出来的** ——
    /// 只走静态列表的话, 启动之后新收盘的 bar 永远进不了列表, `on_bar` 一辈子不触发
    /// (真机 Dry Run 实测踩到: 定时器按点响, bar 一次没来)。
    ///
    /// 返回 `(事件, 每条序列的取数结果)`: 取数失败 → 该序列置 `stale`(FR-016), 恢复后清掉;
    /// 循环继续跑(实盘不因为一次查询失败就退出)。
    pub fn advance_live(
        &mut self,
        host: &EngineHost,
        now_ms: i64,
    ) -> (Vec<SeriesBar>, Vec<SeriesHealth>) {
        let mut health: Vec<SeriesHealth> = Vec::new();
        for s in self.series.iter_mut() {
            if s.cursor < s.bars.len() {
                continue; // 还有待派发的 bar: 先派完再取
            }
            let last_close = s.bars.last().map(|b| b.close_time.timestamp_millis());
            // 还没到"下一根可能已收盘"的时刻 → 不查库。定时器最小 1s, 每秒查库没必要。
            if let Some(lc) = last_close {
                if now_ms.saturating_sub(lc) < s.info.interval.ms() {
                    continue;
                }
            }
            // **重取退避**(审核发现): 上面那条守卫只挡"下一根还没到收盘时刻"的窗口, 而
            // "上一根已收盘、下一根还没发布"的窗口可以长达一整个周期(1d 序列 = 一整天)。
            // 主循环 1s 轮询 → 每个周期内会向数据源打上千次空查询(1m 序列 = 每分钟 60 次),
            // 既被源限速拖慢主循环, 也可能招来 IP 封禁。退避 = max(周期/4, 5s)。
            let min_refetch = (s.info.interval.ms() / 4).max(5_000);
            if let Some(lf) = s.last_fetch_ms {
                if now_ms.saturating_sub(lf) < min_refetch {
                    continue;
                }
            }
            let need = (s.window + 8).min(u32::MAX as usize) as u32;
            s.last_fetch_ms = Some(now_ms);
            match host.load_window(&s.decl, need, now_ms) {
                Ok(win) => {
                    let last_open = s.bars.last().map(|b| b.open_time.timestamp_millis());
                    for b in win.bars {
                        let is_new =
                            last_open.map(|o| b.open_time.timestamp_millis() > o).unwrap_or(true);
                        if is_new {
                            s.bars.push(b);
                        }
                    }
                    // 长跑内存上限: 只留窗口 + 余量, 裁掉的游标同步前移。
                    let cap = s.window + 64;
                    if s.bars.len() > cap {
                        let cut = s.bars.len() - cap;
                        s.bars.drain(0..cut);
                        s.cursor = s.cursor.saturating_sub(cut);
                    }
                    health.push((s.info.id.clone(), true));
                }
                Err(e) => {
                    tracing::warn!(
                        target: "data",
                        series = %s.info.id, error = %e,
                        "序列增量取数失败 → 置 stale(策略可用 s:stale() 感知)"
                    );
                    health.push((s.info.id.clone(), false));
                }
            }
        }
        (self.advance(now_ms), health)
    }

    /// 主时钟 bar 序列 = 第 `primary` 条驱动序列的**窗口内** bar(升序)。
    ///
    /// 回测需要一个单一时间轴来推进账本(`ctx.step_bar`), 取第一条驱动序列作主时钟 ——
    /// 与 `advance` 用同一份内存数据, 不重复读库。
    pub fn clock_bars(&self, primary: usize) -> Vec<ricow_core::Kline> {
        match self.series.get(primary) {
            Some(s) => s.bars[s.cursor..].to_vec(),
            None => Vec::new(),
        }
    }

    /// 声明序的序列描述(日志/诊断用)。
    pub fn infos(&self) -> Vec<SeriesInfo> {
        self.series.iter().map(|s| s.info.clone()).collect()
    }

    /// 推进到 `tick_ms`, 返回**本次新收盘**的 `(序列描述, bar)`, 按序列声明序排列。
    ///
    /// 已经派发过的 bar 不会再返回; `tick_ms` 之前落后的刻度一次补齐(缺口不丢 bar)。
    pub fn advance(&mut self, tick_ms: i64) -> Vec<(SeriesInfo, ricow_core::Kline)> {
        let mut out = Vec::new();
        for s in self.series.iter_mut() {
            while s.cursor < s.bars.len() {
                let bar = &s.bars[s.cursor];
                if bar.close_time.timestamp_millis() > tick_ms {
                    break;
                }
                out.push((s.info.clone(), bar.clone()));
                s.cursor += 1;
            }
        }
        out
    }

    /// 各序列在 `tick_ms` 时刻**正在形成的 bar**(`(标的, bar)`), 键 = 该序列声明的 symbol。
    ///
    /// 为什么需要它(审核发现的 🔴): 多标的声明回测里, 引擎只把主时钟的 bar 交给账本, 于是
    /// 交易非主时钟标的时会**用主时钟序列的价格成交**(实测: 交易 BBBUSDT 却以 AAAUSDT 的
    /// 1010 成交, 而 BBB 真实价 ≈ 10)。修法 = 让每条序列各自给出"此刻正在形成的那根",
    /// 账本按 `req.pair` 路由 —— 与单标的路径同一条规则: 成交参考价 = 正在形成 bar 的 `open`。
    ///
    /// 找不到"已开始但未收盘"的 bar(还没开始交易/数据已到尽头/该标的休市) → **不返回该序列**:
    /// 调用方据此让该 pair 无参考价(市价单按既有语义拒单), 而不是悄悄拿别的标的的价成交。
    pub fn forming_bars(&self, tick_ms: i64) -> Vec<(String, ricow_core::Kline)> {
        // 同标的声明多条周期时(如 BTCUSDT 1h + 4h)撮合参考价取**最细周期**那条 —— 与声明顺序
        // 无关(否则同一张单只改声明顺序就换成交价, 审核实测复现)。
        let mut best: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for s in &self.series {
            let ms = s.info.interval.ms();
            best.entry(s.info.symbol.clone())
                .and_modify(|cur| {
                    if ms < *cur {
                        *cur = ms;
                    }
                })
                .or_insert(ms);
        }
        let mut out = Vec::new();
        for s in &self.series {
            // 按 open_time 升序, 二分找最后一根 open_time <= tick 的 bar。
            let idx = s.bars.partition_point(|b| b.open_time.timestamp_millis() <= tick_ms);
            if idx == 0 {
                continue; // 该序列在这个时刻还没开始
            }
            let bar = &s.bars[idx - 1];
            if bar.close_time.timestamp_millis() > tick_ms
                && best.get(&s.info.symbol).copied() == Some(s.info.interval.ms())
            {
                out.push((s.info.symbol.clone(), bar.clone())); // 正在形成(且是该标的最细周期)
            }
        }
        out
    }

    /// 序列键集合(日志用)。
    pub fn keys(&self) -> Vec<SeriesKey> {
        self.series.iter().map(|s| s.info.key()).collect()
    }
}

/// ms → `YYYY-MM-DD`(错误文案里点出缺数据的区间; 失败时退化为毫秒值)。
fn ms_to_date(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ms.to_string())
}
