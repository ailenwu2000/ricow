//! 结构化运行事件流 (035 / 3.3) —— `run/<name>/events.jsonl`, 一行一事件。
//!
//! **为什么需要它**: 运行期(试跑 / 测试网 / 实盘)的"到底发生了什么"此前只存在于**给人看**的
//! 文本里(`logs/<name>.log`, 且 `println!` 与 `tracing` 混排)。事后想回答"这 40 分钟下了几单、
//! 被拒几单、为什么拒、有没有触发风控", 只能人肉 grep 文本日志 —— 既不可靠也不可机读。
//! 这里补一份**旁路**流水: 每行一个 JSON 对象, **写完即 flush**, 进程被 kill 也不丢最后一条。
//!
//! 口径(与项目其它部分一致):
//! - 事件流是**旁路**: 写失败只提醒一次, 既不 panic 也不返回错误 —— 交易本身比观测重要;
//! - 字段稳定优先于字段多: 加字段可以, 改字段名/语义必须升 [`EVENTS_SCHEMA_VERSION`];
//! - 数值一律以**字符串**存 Decimal(与库表口径一致), 避免 JSON 浮点把价格变成近似值。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 事件流 schema 版本(每条事件都带, 让"读到半截文件"的解析器也能自证口径)。
pub const EVENTS_SCHEMA_VERSION: u32 = 1;

/// 事件文件名(位于 `<root>/run/<name>/`)。
pub const EVENTS_FILE: &str = "events.jsonl";

// ---- 事件类型(kind)常量: 下游按它分流, 字符串只在这里出现一次 ----
/// 实例启动。
pub const KIND_STARTED: &str = "started";
/// 订单已提交(含成交回报)。
pub const KIND_ORDER_PLACED: &str = "order_placed";
/// 订单被拒(对齐失败 / 工程护栏 / 交易所拒单)。
pub const KIND_ORDER_REJECTED: &str = "order_rejected";
/// 订单已撤销。
pub const KIND_ORDER_CANCELED: &str = "order_canceled";
/// 启动接管: 撤销上一轮遗留的本实例归属挂单 (037 P0-A)。
///
/// 与 `order_canceled` 分开记一个 kind, 是因为它**不是**策略运行期的动作, 而是引擎在
/// **启动装配阶段**发现"上一次没有正常退出"的痕迹并做恢复 —— 下游按 kind 分流即可把
/// "崩溃恢复"与"策略主动撤单"区分开, 不必去解析 reason 文本。
pub const KIND_ORPHAN_CANCELED: &str = "orphan_canceled";
/// 实例停止(带停机原因与统计)。
pub const KIND_STOPPED: &str = "stopped";

/// 一条运行事件。
///
/// 全部金额/数量字段都是**字符串**: 与 SQLite 表里的 Decimal 口径一致, 不让 JSON 的
/// double 把 `0.1` 变成 `0.1000000000000000055`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    /// schema 版本。
    pub schema_version: u32,
    /// 事件时刻(Unix 毫秒)。
    pub ts_ms: i64,
    /// 事件类别, 取值见本模块的 `KIND_*` 常量。
    pub kind: String,
    /// 策略名。
    pub strategy: String,
    /// 运行模式原样透传(`dry_run` / `demo` / `live`)。
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filled_size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 拒单原因 / 停机原因 / 其它人类可读说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RunEvent {
    /// 造一条**只填必需字段**的事件(其余字段用 `with_*` 补)。
    pub fn new(kind: &str, strategy: &str, mode: &str, ts_ms: i64) -> Self {
        Self {
            schema_version: EVENTS_SCHEMA_VERSION,
            ts_ms,
            kind: kind.to_string(),
            strategy: strategy.to_string(),
            mode: mode.to_string(),
            pair: None,
            side: None,
            price: None,
            size: None,
            filled_size: None,
            order_id: None,
            client_order_id: None,
            status: None,
            reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// JSONL 事件写入器(一行一事件, 写完即 flush)。
///
/// 刻意**不缓冲**: 事件流的全部价值就在"进程猝死时最后一条还在", 攒批量写会把这一点丢掉。
/// 每条事件只有几百字节, 这个量级下 flush 的开销可以忽略。
pub struct EventWriter {
    path: PathBuf,
    file: Mutex<File>,
    /// 只在**第一次**写失败时提醒, 免得每 tick 刷屏把真问题淹掉。
    warned: AtomicBool,
}

impl EventWriter {
    /// 创建(或追加打开) `<root>/run/<name>/events.jsonl`。
    ///
    /// 返回 `Err` 时调用方应当**继续跑**(不写事件), 而不是拒绝启动 —— 观测能力缺失不该
    /// 阻断交易。
    pub fn create(root: &Path, name: &str) -> std::io::Result<Self> {
        let dir = root.join("run").join(name);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(EVENTS_FILE);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, file: Mutex::new(file), warned: AtomicBool::new(false) })
    }

    /// 事件文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 记录一条事件。**永不返回错误**(见模块头: 事件流是旁路)。
    pub fn emit(&self, ev: &RunEvent) {
        let Ok(line) = serde_json::to_string(ev) else {
            return;
        };
        let ok = match self.file.lock() {
            Ok(mut f) => f
                .write_all(line.as_bytes())
                .and_then(|_| f.write_all(b"\n"))
                .and_then(|_| f.flush())
                .is_ok(),
            // 锁毒化: 事件流里没有需要保护的不变量, 取回守卫继续用即可。
            Err(poisoned) => {
                let mut f = poisoned.into_inner();
                f.write_all(line.as_bytes())
                    .and_then(|_| f.write_all(b"\n"))
                    .and_then(|_| f.flush())
                    .is_ok()
            }
        };
        if !ok && !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: "events",
                path = %self.path.display(),
                "运行事件流写入失败; 交易主流程不受影响, 后续同类失败不再重复提醒"
            );
        }
    }
}

/// 读回事件流(供测试与诊断)。
///
/// **坏行跳过**: 文件是边写边读的, 末尾很可能停在一行写了一半的地方 —— 那种情况下
/// 丢掉最后半行、保住前面所有完整事件, 才是正确的取舍。
pub fn read_events(path: &Path) -> Vec<RunEvent> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RunEvent>(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ricow-events-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_events_land_under_run_dir_and_round_trip() {
        let root = tmp("rt");
        let w = EventWriter::create(&root, "grid01").unwrap();
        assert_eq!(w.path(), root.join("run").join("grid01").join(EVENTS_FILE));

        w.emit(&RunEvent::new(KIND_STARTED, "grid01", "demo", 1_700_000_000_000));
        let mut placed = RunEvent::new(KIND_ORDER_PLACED, "grid01", "demo", 1_700_000_001_000);
        placed.pair = Some("ETHUSDT".into());
        placed.side = Some("Buy".into());
        placed.price = Some("3000.5".into());
        placed.size = Some("0.1".into());
        placed.order_id = Some("123".into());
        placed.status = Some("Filled".into());
        w.emit(&placed);

        let got = read_events(w.path());
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind, KIND_STARTED);
        assert_eq!(got[1], placed, "事件必须原样往返");
        assert_eq!(got[1].schema_version, EVENTS_SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 半截行(写一半就被 kill)不得让**前面所有**完整事件一起作废。
    #[test]
    fn test_reader_skips_truncated_trailing_line() {
        let root = tmp("trunc");
        let w = EventWriter::create(&root, "s").unwrap();
        w.emit(&RunEvent::new(KIND_STARTED, "s", "live", 1));
        {
            let mut f = OpenOptions::new().append(true).open(w.path()).unwrap();
            f.write_all(b"{\"schema_version\":1,\"ts_ms\":2,\"ki").unwrap();
        }
        let got = read_events(w.path());
        assert_eq!(got.len(), 1, "坏行跳过, 完整事件保留");
        assert_eq!(got[0].kind, KIND_STARTED);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 额度字段用字符串存 Decimal —— 不能被 JSON 浮点改写成近似值。
    #[test]
    fn test_decimal_fields_survive_as_exact_strings() {
        let root = tmp("dec");
        let w = EventWriter::create(&root, "s").unwrap();
        let mut ev = RunEvent::new(KIND_ORDER_PLACED, "s", "live", 1);
        ev.price = Some("0.1".into());
        ev.size = Some("0.30000000000000004".into());
        w.emit(&ev);
        let line = std::fs::read_to_string(w.path()).unwrap();
        assert!(line.contains(r#""price":"0.1""#), "价格必须逐字保留: {line}");
        assert!(line.contains(r#""size":"0.30000000000000004""#), "{line}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 追加语义: 同一个实例重启后不得把历史事件清掉。
    #[test]
    fn test_reopen_appends_instead_of_truncating() {
        let root = tmp("app");
        EventWriter::create(&root, "s").unwrap().emit(&RunEvent::new(KIND_STARTED, "s", "demo", 1));
        EventWriter::create(&root, "s").unwrap().emit(&RunEvent::new(KIND_STOPPED, "s", "demo", 2));
        let got = read_events(&root.join("run").join("s").join(EVENTS_FILE));
        assert_eq!(got.len(), 2, "重启后应追加, 不能清空: {got:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 可选字段不写时不得出现 `null` —— 下游少一层判空。
    #[test]
    fn test_optional_fields_are_omitted_when_unset() {
        let root = tmp("opt");
        let w = EventWriter::create(&root, "s").unwrap();
        w.emit(&RunEvent::new(KIND_STARTED, "s", "demo", 1));
        let line = std::fs::read_to_string(w.path()).unwrap();
        assert!(!line.contains("null"), "未设置的字段应省略而非写 null: {line}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
