// 定位 process-wrap (tokio frontend) 的生命周期语义: 子进程到底何时死。
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
fn sid_of(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| {
            let f: Vec<&str> = s.split_whitespace().collect();
            format!("sid={} pgid={}", f.get(5).unwrap_or(&"?"), f.get(4).unwrap_or(&"?"))
        })
        .unwrap_or_else(|e| format!("<{e}>"))
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    use process_wrap::tokio::*;
    let mut child = TokioCommandWrap::with_new("sleep", |c| {
        c.arg("30");
    })
    .wrap(ProcessSession)
    .spawn()?;
    let pid = child.id().unwrap_or(0);
    println!("[tokio] spawned pid={pid} sid/pgid_after_spawn={}", sid_of(pid));
    println!("[tokio] alive(t+0s) = {}", alive(pid));
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    println!("[tokio] alive(t+2s, child 仍在作用域) = {}", alive(pid));
    drop(child);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    println!("[tokio] alive(t+3s, drop(child) 之后) = {}", alive(pid));
    println!("[tokio] 现在 runtime 将关闭; 进程若仍活, t+6s 外部再查");
    let pid2 = pid;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(4));
        println!("[tokio] alive(外部线程, 主逻辑结束后) = {} pid={pid2}", alive(pid2));
    })
    .join()
    .ok();
    Ok(())
}
