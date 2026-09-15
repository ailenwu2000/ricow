use sysinfo::{ProcessesToUpdate, System, RefreshKind, ProcessRefreshKind};
fn main() {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()));
    sys.refresh_processes(ProcessesToUpdate::All, true);
    println!("total = {}", sys.processes().len());
    let mut hits = 0;
    for (pid, p) in sys.processes() {
        let name = p.name().to_string_lossy().to_string();
        let cmd: Vec<String> = p.cmd().iter().map(|s| s.to_string_lossy().to_string()).collect();
        if name.contains("proc") || cmd.iter().any(|c| c.contains("proc-probe")) {
            hits += 1;
            println!("HIT pid={} name={:?} exe={:?} cmd={:?} start={}", pid.as_u32(), name,
                     p.exe().map(|e| e.display().to_string()), cmd, p.start_time());
        }
    }
    println!("hits = {hits}");
}
