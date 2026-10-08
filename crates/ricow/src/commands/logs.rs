//! `ricow logs` — 查看策略进程日志 (008): 读 `logs/<name>.log`, `--follow` 轮询尾随。
//! 043: `--level` 按级别阈值过滤 (兼容纯文本与 JSON 两种日志形态)。

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
    /// 按级别过滤 (DEBUG/INFO/WARN/ERROR, 阈值语义: WARN = WARN 及以上)
    #[arg(long)]
    pub level: Option<String>,
}

/// 日志级别 (数值越大越严重, 阈值过滤用)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl LogLevel {
    /// 解析级别词 (不区分大小写)。
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" | "WARNING" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }

    /// 从一行日志提取级别 (043): 兼容 tracing 纯文本 (`... WARN ricow_engine: ...`)
    /// 与 JSON 行 (`"level":"WARN"`) 两种形态; 识别不出返回 None (调用方视为不过滤)。
    pub(crate) fn from_line(line: &str) -> Option<Self> {
        // JSON 形态: `"level":"ERROR"` (tracing json 的 level 字段)
        for key in ["\"level\":\"", "\"level\": \""] {
            if let Some(pos) = line.find(key) {
                let rest = &line[pos + key.len()..];
                let end = rest.find('"').unwrap_or(rest.len());
                return Self::parse(&rest[..end]);
            }
        }
        // 纯文本形态: 级别词独立成词 (tracing fmt 在时间戳后输出大写级别)
        for (word, level) in [
            ("DEBUG", Self::Debug),
            ("INFO", Self::Info),
            ("WARN", Self::Warn),
            ("ERROR", Self::Error),
        ] {
            if line.contains(&format!(" {word} ")) || line.contains(&format!("{word} ")) {
                return Some(level);
            }
        }
        None
    }
}

pub fn run(args: LogsArgs) -> CoreResult<()> {
    let threshold = match &args.level {
        Some(s) => LogLevel::parse(s).ok_or_else(|| {
            CoreError::InvalidArgument(format!("未知级别 {s:?} (可选 DEBUG/INFO/WARN/ERROR)"))
        })?,
        None => LogLevel::Debug, // 不过滤时给最低阈值 (无级别行原样输出)
    };
    let filter = args.level.is_some();

    let root = crate::commands::project_root();
    let path = ledger::log_path(&root, &args.name);
    if !path.exists() {
        // 009: 前台 `ricow run` 的输出只走 stderr, 不落这份文件 —— 报错里如实说明,
        // 免得用户对着"尚未启动过?"的猜测去找一个永远不会出现的前台日志。
        return Err(CoreError::InvalidArgument(format!(
            "日志不存在: {} (daemon 托管(`ricow start`)才落此文件; 前台 `ricow run` 的输出直接在终端, 不落盘)",
            path.display()
        )));
    }

    let text = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::InvalidArgument(format!("读取日志失败: {e}")))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(args.lines);
    for line in &lines[start..] {
        print_line(line, threshold, filter);
    }

    if args.follow {
        println!("--- 跟踪中 (Ctrl-C 退出) ---");
        follow(&path, text.len() as u64, threshold, filter)?;
    }
    Ok(())
}

/// 输出一行 (043): 开了 `--level` 才过滤; 无级别词的行 (自定义输出/空行) 不拦。
fn print_line(line: &str, threshold: LogLevel, filter: bool) {
    if filter {
        match LogLevel::from_line(line) {
            Some(lv) if lv >= threshold => {}
            Some(_) => return,
            None => {} // 无级别行保留, 避免截断多行消息的续行
        }
    }
    println!("{line}");
}

/// 轮询尾随 (200ms; 无需额外依赖)。文件被轮转后自动从头继续。
fn follow(
    path: &std::path::Path,
    mut offset: u64,
    threshold: LogLevel,
    filter: bool,
) -> CoreResult<()> {
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
                for line in buf.lines() {
                    print_line(line, threshold, filter);
                }
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            offset = len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parse_accepts_common_words() {
        assert_eq!(LogLevel::parse("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse(" ERROR "), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("bogus"), None);
    }

    #[test]
    fn from_line_plain_text() {
        let l = "2026-09-29T08:00:00.123Z  WARN ricow_engine: 通知投递失败: timeout";
        assert_eq!(LogLevel::from_line(l), Some(LogLevel::Warn));
        let l2 = "2026-09-29T08:00:00.123Z ERROR ricow_engine: 建仓失败";
        assert_eq!(LogLevel::from_line(l2), Some(LogLevel::Error));
        let l3 = "2026-09-29T08:00:00.123Z  INFO ricow_engine: live run started";
        assert_eq!(LogLevel::from_line(l3), Some(LogLevel::Info));
    }

    #[test]
    fn from_line_json() {
        let l = r#"{"timestamp":"2026-09-29T08:00:00Z","level":"ERROR","target":"engine","fields":{"message":"boom"}}"#;
        assert_eq!(LogLevel::from_line(l), Some(LogLevel::Error));
        let l2 = r#"{"level": "WARN", "fields": {"message": "x"}}"#;
        assert_eq!(LogLevel::from_line(l2), Some(LogLevel::Warn));
    }

    #[test]
    fn from_line_unrecognized() {
        assert_eq!(LogLevel::from_line("普通输出, 无级别词"), None);
        // 字段值里出现的级别词不算 (JSON 已按 "level" 键取, 不误伤)
        assert_eq!(LogLevel::from_line(r#"{"msg":"level not a key"}"#), None);
    }

    #[test]
    fn threshold_semantics_warn_includes_error() {
        let warn = "2026-09-29T00:00:00Z  WARN ricow_engine: w";
        let err = "2026-09-29T00:00:00Z ERROR ricow_engine: e";
        let info = "2026-09-29T00:00:00Z  INFO ricow_engine: i";
        let keep = |l: &str| match LogLevel::from_line(l) {
            Some(lv) => lv >= LogLevel::Warn,
            None => true,
        };
        assert!(keep(warn) && keep(err) && !keep(info));
    }
}
