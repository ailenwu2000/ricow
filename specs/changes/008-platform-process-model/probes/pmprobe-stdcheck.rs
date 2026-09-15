// 对照: std frontend (process-wrap::std) 的生命周期语义。
#![forbid(unsafe_code)]

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

fn alive(pid: u32) -> bool {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

fn sid_of(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| {
            let f: Vec<&str> = s.split_whitespace().collect();
            format!("sid={} pgid={}", f.get(5).unwrap_or(&"?"), f.get(4).unwrap_or(&"?"))
        })
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn main() -> std::io::Result<()> {
    use process_wrap::std::*;
    let child = StdCommandWrap::with_new("sleep", |c| {
        c.arg("30");
    })
    .wrap(ProcessSession)
    .spawn()?;
    let pid = child.id();
    println!("[std] spawned pid={pid} sid/pgid={}", sid_of(pid));
    println!("[std] alive(t+0s) = {}", alive(pid));
    std::thread::sleep(std::time::Duration::from_secs(2));
    println!("[std] alive(t+2s, child 在作用域) = {}", alive(pid));
    drop(child);
    std::thread::sleep(std::time::Duration::from_secs(1));
    println!("[std] alive(t+3s, drop(child) 之后) = {}", alive(pid));
    println!("[std] main 即将返回 (进程退出), 外部应仍能看到 pid={pid}");
    Ok(())
}
