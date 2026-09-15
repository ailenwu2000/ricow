// 探针: 验证「后台进程管理」所需 API 在 #![forbid(unsafe_code)] 下可用。
// 1) unix: CommandExt::process_group (safe?)  2) windows: CommandExt::creation_flags
// 3) sysinfo: 进程存活检测 + 终止
#![forbid(unsafe_code)]

use std::process::Command;

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0); // 期望: safe 方法, 无需 unsafe 块
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

fn alive(pid: u32) -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 子进程模式: 挂 30s, 父进程退出后应仍存活
    if args.get(1).map(String::as_str) == Some("child") {
        println!("child pid={} started", std::process::id());
        std::thread::sleep(std::time::Duration::from_secs(30));
        return;
    }

    let self_exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(&self_exe);
    cmd.arg("child");
    detach(&mut cmd);
    let child = cmd.spawn().expect("spawn");
    let pid = child.id();
    println!("parent={} spawned detached child={}", std::process::id(), pid);
    println!("alive(child) = {}", alive(pid));
    // 父进程立刻退出, 不 wait → 子进程应被 init 收养并继续存活
    drop(child);
    println!("parent exiting now");
}
