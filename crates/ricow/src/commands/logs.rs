//! `ricow logs` — 查看策略进程日志 (008): 读 `logs/<name>.log`, `--follow` 轮询尾随。

use std::io::{Read, Seek, SeekFrom};

use clap::Args;
use ricow_core::{CoreError, CoreResult};

use crate::supervisor::ledger;

#[derive(Args)]
pub struct LogsArgs {
    /// 策略名 (实例名, 即 strategies/<name>.toml 的文件名)
    pub name: String,
    /// 持续跟踪新日志 (Ctrl-C 退出)
    #[arg(long)]
    pub follow: bool,
    /// 首次输出末尾行数
    #[arg(long, default_value_t = 50)]
    pub lines: usize,
}

pub fn run(args: LogsArgs) -> CoreResult<()> {
    let root = crate::commands::project_root();
    let path = ledger::log_path(&root, &args.name);
    if !path.exists() {
        return Err(CoreError::InvalidArgument(format!(
            "日志不存在: {} (该策略尚未启动过?)",
            path.display()
        )));
    }

    let text = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::InvalidArgument(format!("读取日志失败: {e}")))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(args.lines);
    for line in &lines[start..] {
        println!("{line}");
    }

    if args.follow {
        println!("--- 跟踪中 (Ctrl-C 退出) ---");
        follow(&path, text.len() as u64)?;
    }
    Ok(())
}

/// 轮询尾随 (200ms; 无需额外依赖)。文件被轮转后自动从头继续。
fn follow(path: &std::path::Path, mut offset: u64) -> CoreResult<()> {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let Ok(mut f) = std::fs::File::open(path) else { continue };
        let len = match f.metadata() {
            Ok(m) => m.len(),
            Err(_) => continue,
        };
        if len < offset {
            offset = 0; // 轮转后重读新文件
        }
        if len > offset {
            if f.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut buf = String::new();
            if f.read_to_string(&mut buf).is_ok() {
                print!("{buf}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            offset = len;
        }
    }
}
