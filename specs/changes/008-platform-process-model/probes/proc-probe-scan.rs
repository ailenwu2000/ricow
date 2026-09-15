// 探针: 用 sysinfo 扫描进程表, 识别 "locus 实例" 并读取 argv (供 status 发现外部启动的实例)。
// 判定: 进程名/exe 含 locus 且 cmd 里出现 "run" → 视为候选实例。
#![forbid(unsafe_code)]

use sysinfo::{ProcessesToUpdate, System};

fn main() {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    println!("total processes = {}", sys.processes().len());
    for (pid, p) in sys.processes() {
        let cmd: Vec<String> = p
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        let exe = p.exe().map(|e| e.display().to_string()).unwrap_or_default();
        let looks_like = (exe.contains("locus") || p.name().to_string_lossy().contains("locus"))
            || cmd.iter().any(|c| c.contains("proc-probe"));
        if looks_like {
            println!(
                "pid={:<8} name={:<16} start={:<12} cpu={:>6.2}s exe={:<40} cmd={:?}",
                pid.as_u32(),
                p.name().to_string_lossy(),
                p.start_time(),
                p.accumulated_cpu_time() as f64 / 1000.0,
                exe,
                cmd
            );
        }
    }
    // 演示: argv 中提取策略名 (locus run <name>)
    for (_pid, p) in sys.processes() {
        let cmd: Vec<String> = p
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        if let Some(i) = cmd.iter().position(|c| c == "run") {
            if let Some(name) = cmd.get(i + 1) {
                println!(
                    "identified strategy instance: name={name} argv_tail={:?}",
                    &cmd[i..]
                );
            }
        }
    }
}
