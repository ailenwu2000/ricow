//! 定时器声明与调度 (028 FR-006): `data.timer{...}` 声明, 引擎按虚拟/真实时钟触发 `on_timer`。
//!
//! 两种节奏(都可选, 至少给一个):
//! - `secs = N`: 每 N 秒一次(相对节奏);
//! - `at = "HH:MM"`: 每天该时刻一次, **时区由策略显式声明**(`tz` 缺省 `UTC`)。
//!
//! 回测走虚拟时钟(`due(tick_ms)` 由引擎在每个刻度调用), 实盘/Dry Run 走真实墙钟 —— 同一份
//! 调度逻辑, 因此"回测里几点触发"与"实盘几点触发"不会漂移(这也是回测可信的前提之一)。

use serde::{Deserialize, Serialize};

/// 定时器声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerDecl {
    /// 触发时传给 `on_timer(ctx, label)` 的标签(策略用自己的标签区分多个定时器)。
    pub label: String,
    /// 相对节奏: 每 N 秒。
    pub secs: Option<u64>,
    /// 绝对节奏: 每天 `HH:MM`(见 [`TimerDecl::tz_offset_minutes`])。
    pub at: Option<(u8, u8)>,
    /// 时区偏移(分钟, 相对 UTC); 缺省 0 = UTC。策略显式声明, 平台不猜本地时区。
    pub tz_offset_minutes: i32,
}

impl TimerDecl {
    /// 每 N 秒。
    pub fn every_secs(label: &str, secs: u64) -> Self {
        Self { label: label.to_string(), secs: Some(secs), at: None, tz_offset_minutes: 0 }
    }

    /// 每天 `HH:MM`(UTC)。
    pub fn daily_at(label: &str, hh: u8, mm: u8) -> Self {
        Self { label: label.to_string(), secs: None, at: Some((hh, mm)), tz_offset_minutes: 0 }
    }

    /// 设时区偏移(分钟)。
    pub fn with_tz_offset(mut self, minutes: i32) -> Self {
        self.tz_offset_minutes = minutes;
        self
    }

    /// 声明是否可用(至少一种节奏, label 非空, 时刻合法)。
    pub fn validate(&self) -> Result<(), String> {
        if self.label.is_empty() {
            return Err("data.timer 必须给 label".to_string());
        }
        if self.secs.is_none() && self.at.is_none() {
            return Err(format!("定时器 '{}' 必须给 secs 或 at", self.label));
        }
        // 审核发现: 两者同给时 `secs` 会静默生效、`at` 被无声忽略 —— 用户以为"两个都设了"。
        if self.secs.is_some() && self.at.is_some() {
            return Err(format!(
                "定时器 '{}' 的 secs 与 at 只能给一个(secs=相对节奏, at=每日固定时刻)",
                self.label
            ));
        }
        if let Some(0) = self.secs {
            return Err(format!("定时器 '{}' 的 secs 必须 > 0", self.label));
        }
        if let Some((h, m)) = self.at {
            if h > 23 || m > 59 {
                return Err(format!(
                    "定时器 '{}' 的 at 必须是 HH:MM (收到 {:02}:{:02})",
                    self.label, h, m
                ));
            }
        }
        if self.tz_offset_minutes.abs() > 14 * 60 {
            return Err(format!("定时器 '{}' 的时区偏移超出 ±14 小时", self.label));
        }
        Ok(())
    }
}

/// 引擎每个刻度调用一次: 返回本刻应当触发的 label(可能 0 个或多个, 按声明序)。
pub struct TimerScheduler {
    decls: Vec<TimerDecl>,
    /// 每条定时器的"下次触发时刻"(ms)。
    next: Vec<i64>,
    /// 单次调用最多补几次(防止长间隔后补爆); 超出则记警告并跳到当前时刻之后。
    max_catch_up: usize,
}

impl TimerScheduler {
    /// `start_ms` = 调度起点(回测 = 首刻度; 实盘 = 当前墙钟), 早于起点的触发点不补。
    pub fn new(decls: Vec<TimerDecl>, start_ms: i64) -> Self {
        let next = decls.iter().map(|d| first_fire_after(d, start_ms)).collect();
        Self { decls, next, max_catch_up: 10 }
    }

    /// 推进到 `now_ms`, 返回需要触发的 label(按声明序, 同一条可能重复 = 需要连续触发多次)。
    ///
    /// 密集刻度下通常每次 0~1 个; 回测里若刻度跨度大于节奏(例如 `secs=30` 配 1h 刻度),
    /// 会连续补触发, 但单次调用最多补 `max_catch_up` 次。
    pub fn due(&mut self, now_ms: i64) -> Vec<String> {
        let mut out = Vec::new();
        for (i, decl) in self.decls.iter().enumerate() {
            let mut fired = 0usize;
            while self.next[i] <= now_ms {
                out.push(decl.label.clone());
                fired += 1;
                match (decl.secs, decl.at) {
                    (Some(secs), _) => {
                        let step = (secs as i64) * 1000;
                        if step <= 0 {
                            break;
                        }
                        self.next[i] += step;
                    }
                    (None, Some((h, m))) => {
                        // 日节奏: 下一次该时刻(按声明时区)。+1ms 是为了"严格之后",
                        // 否则会正好落在同一个时刻上, 再被顺延一天(跳过一次触发)。
                        self.next[i] = align_daily(self.next[i] + 1, h, m, decl.tz_offset_minutes);
                    }
                    (None, None) => break,
                }
                if fired >= self.max_catch_up {
                    // 不静默丢: 提醒调用方"节奏比刻度密", 并把 next **按原相位重对齐**到当前时刻之后。
                    // (审核发现: 原来写 `now_ms + 1`, 于是下一个刻度又从 +1ms 处补 10 次 ——
                    //  刻度跨度远大于节奏时, 每个刻度稳定产生一串连发; 用 first_fire_after 重对齐
                    //  后每个刻度至多补一次, 且相位不漂。)
                    tracing::warn!(
                        target: "timer",
                        "定时器 '{}' 单刻度补触发达到上限 {} 次(节奏比刻度密), 已按原相位重对齐到当前时刻之后",
                        decl.label,
                        self.max_catch_up
                    );
                    self.next[i] = first_fire_after(decl, now_ms);
                    break;
                }
            }
        }
        out
    }
}

/// 首次触发时刻(严格晚于 `after_ms`)。
fn first_fire_after(d: &TimerDecl, after_ms: i64) -> i64 {
    match (d.secs, d.at) {
        (Some(secs), _) => after_ms + (secs as i64) * 1000,
        (None, Some((h, m))) => align_daily(after_ms, h, m, d.tz_offset_minutes),
        (None, None) => i64::MAX,
    }
}

/// 把时刻对齐到"下一个该时区的 `HH:MM`"(若当天已过则顺延一天)。
fn align_daily(after_ms: i64, hh: u8, mm: u8, tz_offset_minutes: i32) -> i64 {
    let off = i64::from(tz_offset_minutes) * 60_000;
    let local = after_ms + off;
    let day = local.div_euclid(86_400_000);
    let target = day * 86_400_000 + i64::from(hh) * 3_600_000 + i64::from(mm) * 60_000;
    let target = if target > local { target } else { target + 86_400_000 };
    target - off
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secs_timer_fires_on_schedule() {
        let mut s = TimerScheduler::new(vec![TimerDecl::every_secs("30s", 30)], 0);
        assert!(s.due(29_999).is_empty(), "未到点不触发");
        assert_eq!(s.due(30_000), vec!["30s".to_string()]);
        assert!(s.due(59_999).is_empty());
        assert_eq!(s.due(60_000), vec!["30s".to_string()]);
    }

    #[test]
    fn test_secs_timer_catches_up_but_is_capped() {
        let mut s = TimerScheduler::new(vec![TimerDecl::every_secs("1s", 1)], 0);
        // 一格跨 3 秒 → 连续补 3 次
        assert_eq!(s.due(0), Vec::<String>::new(), "起点不立刻触发");
        assert_eq!(s.due(3_000).len(), 3, "缺口按节奏补齐");
        // 跨得过多 → 触发上限 + 跳到当前之后
        let fired = s.due(1_000_000);
        assert_eq!(fired.len(), 10, "单次补触发有上限");
        assert!(s.due(1_000_001).len() <= 1, "跳表后不再爆量");
    }

    #[test]
    fn test_daily_timer_uses_declared_timezone() {
        // UTC 日 00:00 起点 → 当天 09:30(UTC) 触发
        let mut s = TimerScheduler::new(vec![TimerDecl::daily_at("open", 9, 30)], 0);
        assert!(s.due(9 * 3_600_000 - 1).is_empty());
        assert_eq!(s.due(9 * 3_600_000 + 30 * 60_000), vec!["open".to_string()]);
        // 次日再触发一次
        assert!(s.due(30 * 3_600_000).is_empty());
        assert_eq!(s.due(33 * 3_600_000 + 30 * 60_000), vec!["open".to_string()]);
    }

    #[test]
    fn test_daily_timer_honours_tz_offset() {
        // 声明 UTC+8 的 09:30 → UTC 01:30 触发
        let decl = TimerDecl::daily_at("cn_open", 9, 30).with_tz_offset(8 * 60);
        let mut s = TimerScheduler::new(vec![decl], 0);
        assert!(s.due(3_600_000 + 29 * 60_000).is_empty());
        assert_eq!(s.due(3_600_000 + 30 * 60_000), vec!["cn_open".to_string()]);
    }

    /// 第三轮审核 🟡: `secs` 与 `at` 同给必须报错(原来静默取 `secs`, 用户以为两个都生效)。
    #[test]
    fn test_timer_rejects_both_secs_and_at() {
        let mut d = TimerDecl::every_secs("t", 60);
        d.at = Some((9, 30));
        let err = d.validate().expect_err("同给必须报错");
        assert!(err.contains("只能给一个"), "got: {err}");
    }

    #[test]
    fn test_timer_decl_validation() {
        assert!(TimerDecl::every_secs("a", 0).validate().is_err(), "secs=0 非法");
        assert!(TimerDecl::daily_at("a", 24, 0).validate().is_err(), "24 点非法");
        assert!(TimerDecl::daily_at("", 9, 0).validate().is_err(), "缺 label 非法");
        let none = TimerDecl { label: "a".into(), secs: None, at: None, tz_offset_minutes: 0 };
        assert!(none.validate().is_err(), "没有任何节奏非法");
        assert!(TimerDecl::every_secs("a", 60).validate().is_ok());
    }
}
