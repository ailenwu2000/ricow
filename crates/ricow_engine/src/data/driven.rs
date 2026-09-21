//! 声明驱动的运行期装配 (028 T023): 把"策略声明 → 数据面 → 每轮该派发什么"收在一处,
//! 让 Dry Run 与实盘两条主循环共用同一份装配与推进逻辑(避免两套漂移)。
//!
//! 与回测路径的关系: 回测由 `run_backtest_with_series` 用**虚拟时钟**推进同一个
//! [`SeriesDriver`]; 这里用**真实墙钟**推进。可见性判据、bar 去重、定时器节奏全都复用一个实现,
//! 因此"回测里何时看到什么"与"实盘里何时看到什么"不会因为两套代码而分歧。

use std::sync::Arc;

use ricow_core::{CoreResult, Kline};
use ricow_strategy::{SeriesInfo, Strategy, TimerDecl, TimerScheduler};

use super::host_impl::EngineHost;
use super::series_driver::SeriesDriver;

/// 本刻需要派发给策略的驱动事件(按"先 bar、后定时器"的顺序)。
#[derive(Debug, Clone)]
pub enum DriveEvent {
    /// 某条序列的一根已收盘 bar。
    Bar(SeriesInfo, Kline),
    /// 定时器标签(FR-006)。
    Timer(String),
}

/// 声明驱动的运行期状态。
pub struct DrivenRuntime {
    host: Arc<EngineHost>,
    driver: Option<SeriesDriver>,
    timers: Option<TimerScheduler>,
    /// 上一轮各序列的取数结果 `(id, ok)` —— 由调用方转成 `stale` 置位(FR-016)。
    stale_updates: Vec<(String, bool)>,
    /// 策略是否声明了数据面(空声明 = 旧路径: 引擎不驱动任何东西)。
    pub active: bool,
}

impl DrivenRuntime {
    /// 未声明任何数据面的运行期(旧路径)。
    pub fn inactive(host: Arc<EngineHost>) -> Self {
        Self { host, driver: None, timers: None, stale_updates: Vec::new(), active: false }
    }

    /// 在 `on_init` **之后**装配: 读策略声明 → 装序列 → 建定时器。
    ///
    /// `start_ms` = 装配起点(实盘/Dry Run = 当前墙钟)。**只驱动起点之后的 bar** ——
    /// 起点之前的历史是预热, 由声明期句柄预填(见方法内注释)。
    /// 回补失败 → 直接报错(实盘启动时数据不全, 让它跑起来只会用错数据)。
    pub fn assemble(
        host: Arc<EngineHost>,
        strategy: &dyn Strategy,
        start_ms: i64,
    ) -> CoreResult<Self> {
        let decls = strategy.driving_declarations();
        let timers: Vec<TimerDecl> = strategy.timer_declarations();
        if decls.is_empty() && timers.is_empty() {
            return Ok(Self::inactive(host));
        }

        let driver = if decls.is_empty() {
            None
        } else {
            // 读取窗口与**派发起点**都是 start_ms。
            //
            // 为什么不是 [start - 预热, start]: 起点之前的历史属于预热, 而预热已经由**声明期**
            // 的句柄预填了(`data:series` 在宿主 now = start 时装载, 只能看到已收盘的过去)。
            // 若这里把预热段也交给驱动, 启动第一个轮询就会把上百根历史 bar 以 `on_bar` 的形式
            // 一次性涌给策略 —— 策略会在"实盘启动瞬间"对着旧 bar 连开一堆仓(实测踩过)。
            Some(SeriesDriver::load(&host, &decls, start_ms, start_ms)?)
        };
        let timers =
            if timers.is_empty() { None } else { Some(TimerScheduler::new(timers, start_ms)) };

        for info in driver.as_ref().map(|d| d.infos()).unwrap_or_default() {
            tracing::info!(
                target: "data", name = %info.id,
                source = %info.source, symbol = %info.symbol, interval = %info.interval.label(),
                mode = %info.mode.label(), feed = %info.feed_interval.label(),
                "序列装配完成"
            );
        }

        Ok(Self { host, driver, timers, stale_updates: Vec::new(), active: true })
    }

    /// 每轮循环调用一次: 推进可见时刻并返回本刻的事件(通常为空)。
    ///
    /// 内部先做**增量取数**(实盘数据是长出来的, 见 [`SeriesDriver::advance_live`]),
    /// 再派发; 取数结果留在 [`Self::take_stale_updates`] 由调用方置 `stale`。
    pub fn advance(&mut self, now_ms: i64) -> Vec<DriveEvent> {
        if !self.active {
            return Vec::new();
        }
        // 可见时刻只在**向外**取数时用(`data:history` 等); 推进本身按传入时刻判断。
        self.host.set_now_ms(now_ms);
        let mut events = Vec::new();
        if let Some(d) = self.driver.as_mut() {
            let host = self.host.clone();
            let (bars, health) = d.advance_live(&host, now_ms);
            self.stale_updates.extend(health);
            for (info, bar) in bars {
                events.push(DriveEvent::Bar(info, bar));
            }
        }
        if let Some(t) = self.timers.as_mut() {
            for label in t.due(now_ms) {
                events.push(DriveEvent::Timer(label));
            }
        }
        events
    }

    /// 取走上一步的序列取数结果 `(id, ok)`(取后清空)。
    pub fn take_stale_updates(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.stale_updates)
    }

    /// 每秒检查一次即可(最小定时器节奏 1s; 更密没有意义)。
    pub fn poll_interval() -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }
}
