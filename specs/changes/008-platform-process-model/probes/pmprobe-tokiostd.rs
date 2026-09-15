// 决定实现组合: CLI 本体是 #[tokio::main], 但 start 用 std::process spawn 后台进程。
// 验证: 这种组合下, 启动器立即退出时子进程是否存活。
#![forbid(unsafe_code)]

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

fn alive(pid: u32) -> bool {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("40");
    detach(&mut cmd);
    let child = cmd.spawn()?;
    let pid = child.id();
    println!("[tokio-main + std::process] spawned pid={pid} alive={}", alive(pid));
    println!("parent returning immediately (tokio runtime will shut down)");
    Ok(())
}
