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
