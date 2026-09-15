// 探针: daemon→策略子进程 的停机通道 (D2 核心机制)
//   a) 正常停机: daemon 向子进程 stdin 写 "stop\n" → 子进程执行清理后退出
//   b) daemon 崩溃: 不写指令, 父进程直接消失(管道写端关闭) → 子进程读到 EOF → 自行清理退出
// 只用 std, 无第三方依赖, 三平台语义一致。
#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 子进程模式: 与策略进程 run 循环里的停机监听同构
    if args.get(1).map(String::as_str) == Some("child") {
        println!("child pid={} 开始运行, 监听 stdin", std::process::id());
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if l.trim() == "stop" {
                        println!("child: 收到 stop 指令 → 执行清理 (撤单/平仓) → 退出");
                        std::process::exit(0);
                    }
                    println!("child: 忽略未知指令 {l:?}");
                }
                Err(e) => {
                    println!("child: stdin 读错误 {e} → 视为停机");
                    break;
                }
            }
        }
        println!("child: stdin EOF (daemon 已不在) → 自行执行清理 → 退出");
        std::process::exit(0);
    }

    // daemon 模式
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(&exe);
    cmd.arg("child").stdin(Stdio::piped()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut child = cmd.spawn().expect("spawn child");
    println!("daemon: spawned child pid={}", child.id());
    let mut w = child.stdin.take().expect("child stdin");
    std::thread::sleep(Duration::from_secs(2));

    if std::env::var("PROBE_MODE").as_deref() == Ok("crash") {
        println!("daemon: 模拟崩溃 —— 不写指令直接退出 (管道写端随之关闭)");
        std::process::exit(0);
    }

    print!("daemon: 发送优雅停机指令 stop\\n → ");
    w.write_all(b"stop\n").expect("write stop");
    w.flush().expect("flush");
    drop(w);
    let status = child.wait().expect("wait child");
    println!("daemon: child 已退出 status={status:?}");
}
