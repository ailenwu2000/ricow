// 探针: MSRV 1.83 下用 process-wrap(8.2.1, tokio frontend) + sysinfo(0.36.1) 起后台进程。
#![forbid(unsafe_code)]

#[cfg(unix)]
async fn spawn_detached() -> std::io::Result<u32> {
    use process_wrap::tokio::*;
    let mut child = TokioCommandWrap::with_new("sleep", |c| {
        c.arg("30");
    })
    .wrap(ProcessSession)
    .spawn()?;
    Ok(child.id().unwrap_or(0))
}

#[cfg(windows)]
async fn spawn_detached() -> std::io::Result<u32> {
    use process_wrap::tokio::*;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let mut child = TokioCommandWrap::with_new("cmd", |c| {
        c.args(["/C", "timeout", "30"]);
    })
    .wrap(CreationFlags(DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP))
    .spawn()?;
    Ok(child.id().unwrap_or(0))
}

fn sysinfo_probe(pid: u32) -> (bool, String, u64) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);
    match sys.process(sysinfo::Pid::from_u32(pid)) {
        Some(p) => (true, p.name().to_string_lossy().to_string(), p.start_time()),
        None => (false, String::new(), 0),
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let pid = spawn_detached().await?;
    println!("spawned detached pid={pid}");
    let (alive, name, start) = sysinfo_probe(pid);
    println!("sysinfo: alive={alive} name={name} start_time={start}");
    println!("parent exiting now");
    Ok(())
}
