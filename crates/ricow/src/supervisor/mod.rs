//! 策略进程管理器 (008) — 常驻 daemon + 本机控制通道 + 实例台账。
//!
//! 模型 (决策已拍板, 见 `specs/changes/008-platform-process-model/`):
//! - daemon 持有策略子进程; daemon 退出即中止全部子进程 (父子同生命周期)
//! - CLI 经仅绑 127.0.0.1 的 TCP + token 驱动 daemon; token 在 `run/daemon.json` (0600)
//! - 优雅停机: daemon 向子进程 stdin 写 `stop`; daemon 消失 → 管道 EOF → 子进程自行清理退出
//! - 零新增第三方依赖: stdin/stdout 管道 + std 进程 API + tokio TCP

pub mod client;
pub mod ledger;
pub mod procs;
pub mod proto;
pub mod server;

use std::path::Path;

/// 本地只读数据的**来源三态** (026 D10 / FR-014 / FR-020): 供新增 AI 工具与 Web 端点共用。
///
/// 存在的意义是把"确实没有"与"读不到"分开 —— 与「daemon 连不上被说成已经停着」是同一类
/// 假阴性。**不允许**把 [`Source::Unreadable`] / [`Source::DaemonDown`] 说成"没有"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// daemon 在跑, 且本地库读得到 → 数据可当"当前"用。
    Ok,
    /// daemon 未运行(无 `run/daemon.json` 或连不上) → 只能给最后一次快照, **不得**说成"当前"。
    DaemonDown,
    /// 本地库读不到(附原因) → **不得**说成"没有"。
    Unreadable(String),
}

impl Source {
    /// 传给 Web 前端的稳定短串(JSON 字段 `source`)。
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Ok => "ok",
            Source::DaemonDown => "daemon_down",
            Source::Unreadable(_) => "unreadable",
        }
    }
}

/// daemon 是否在运行 —— 口径与 [`client::Client::connect`] 的前两步一致: 读元信息 + TCP 连接。
pub async fn daemon_is_running(root: &Path) -> bool {
    let Some(info) = ledger::read_daemon_info(root) else {
        return false;
    };
    // 本机回环正常瞬时返回(有人监听 = 连上, 无人监听 = 立即 refused); 留 1s 上限只为防
    // 极端情况(本机防火墙丢包)把面板与工具卡住。
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, info.port)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// 把「读本地库的结果」与「daemon 存活」合成为 [`Source`] 三态。
///
/// - 读库报错 → [`Source::Unreadable`](这是"读不到"的**实证**, 优先级最高);
/// - 读到了但 daemon 不在 → [`Source::DaemonDown`](数据是最后一次快照);
/// - 两者都好 → [`Source::Ok`]。
///
/// 返回 `Option<T>`: 读不到时没有数据, 调用方只能如实说"无法判断"。
pub async fn classify<T>(root: &Path, read: Result<T, String>) -> (Source, Option<T>) {
    match read {
        Err(msg) => (Source::Unreadable(msg), None),
        Ok(value) => {
            let source =
                if daemon_is_running(root).await { Source::Ok } else { Source::DaemonDown };
            (source, Some(value))
        }
    }
}
