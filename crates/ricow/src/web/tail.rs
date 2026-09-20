//! 策略日志的**只读尾读** (026 T027 / D12 / D13 / FR-015 / FR-016)。
//!
//! 纯逻辑: 给定 (文件路径, 上次 offset) → (完整新增行, 新 offset)。**不写、不改、不轮转**该文件 ——
//! 日志是策略子进程的产物(见 `supervisor::procs`), Web 这一侧只读它。

use std::io;
use std::path::Path;

/// 单次尾读的行数上限 (D12): 请求里给的 `lines` 一律夹到 `1..=200`。
pub const TAIL_MAX_LINES: usize = 200;

/// 一次尾读的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailResult {
    /// 本次新增的**完整**行(行尾 `\n` 已去掉; 末尾没写完的那截不算, 留到下次)。
    pub lines: Vec<String>,
    /// 下次调用要带回来的 offset(字节)。
    pub offset: u64,
    /// 检测到轮转/截断(文件比上次短) → 已归零重读, 调用方应**如实提示**。
    pub rotated: bool,
}

/// 从 `offset` 起读出**完整的**新增行。
///
/// - `offset == 0` = 首次(或轮转后重读): 只给**最近** `lines` 行(夹到 [`TAIL_MAX_LINES`]);
/// - `offset > 0` = 增量: 给这段时间写进来的**全部**完整行;
/// - 文件比 `offset` 短 = 被轮转/截断(`procs` 的 10MB 轮转、人工清空都会这样) → `rotated = true`,
///   从 0 重读: 已写入的行**不丢**, 已经给过的行**不重**;
/// - 末尾没有 `\n` 的那截是"正在写", 不当作一行(offset 停在它之前) —— 下次补齐后才给。
///
/// 文件不存在 → `io::ErrorKind::NotFound`, 由调用方如实说"还没有日志", **不**说成"没有交易"。
pub fn tail_since(path: &Path, offset: u64, lines: usize) -> io::Result<TailResult> {
    let lines = lines.clamp(1, TAIL_MAX_LINES);
    let buf = std::fs::read(path)?;
    let len = buf.len() as u64;

    // 比上次短 = 轮转/截断: 上次的 offset 已经越过文件末尾。
    let rotated = offset > len;
    let start = if rotated { 0usize } else { offset as usize };

    let tail = &buf[start..];
    // 一行都没写全(`\n` 之前的内容不算): offset 原地不动, 不产生重复。
    let Some(last_nl) = tail.iter().rposition(|b| *b == b'\n') else {
        return Ok(TailResult {
            lines: Vec::new(),
            offset: if rotated { 0 } else { offset },
            rotated,
        });
    };
    let consumed = &tail[..=last_nl];
    let new_offset = start as u64 + consumed.len() as u64;

    // 按字节切行; 末段必然是空的(因为 `consumed` 以 `\n` 结尾), 丢掉。
    let mut segs: Vec<&[u8]> = consumed.split(|b| *b == b'\n').collect();
    segs.pop();
    let mut lines_out: Vec<String> =
        segs.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect();

    // 首次 / 轮转后: 只给最近 `lines` 行 —— 更早的历史不在本次窗口里; offset 仍推到末尾,
    // 所以客户端不会重复收到它们。
    if start == 0 && lines_out.len() > lines {
        lines_out.drain(..lines_out.len() - lines);
    }

    Ok(TailResult { lines: lines_out, offset: new_offset, rotated })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// 一个干净的临时日志文件路径(每次调用换 tag, 免撞车)。
    fn tmp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ricow-tail-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("s.log")
    }

    /// 首次读只给**最近** `lines` 行, 且 offset 推到末尾。
    #[test]
    fn first_read_gives_last_n_lines() {
        let p = tmp_file("first");
        let all: Vec<String> = (1..=300).map(|i| format!("l{i}\n")).collect();
        std::fs::write(&p, all.concat()).expect("写日志");

        let got = tail_since(&p, 0, 10).expect("尾读");
        assert_eq!(got.lines, (291..=300).map(|i| format!("l{i}")).collect::<Vec<_>>());
        assert_eq!(got.offset, std::fs::metadata(&p).expect("元数据").len());
        assert!(!got.rotated);
        // offset 已在末尾 → 紧接着的增量读什么也不给(不重复)。
        let again = tail_since(&p, got.offset, 10).expect("再尾读");
        assert!(again.lines.is_empty() && !again.rotated);
    }

    /// 增量读只给**新增**的行。
    #[test]
    fn incremental_read_gives_only_new_lines() {
        let p = tmp_file("incr");
        std::fs::write(&p, "a\nb\n").expect("写日志");
        let first = tail_since(&p, 0, 200).expect("首次");
        assert_eq!(first.lines, vec!["a", "b"]);

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).expect("打开追加");
        f.write_all(b"c\nd\n").expect("追加");
        drop(f);

        let got = tail_since(&p, first.offset, 200).expect("增量");
        assert_eq!(got.lines, vec!["c", "d"], "只给新增, 不含已给过的");
        assert_eq!(got.offset, std::fs::metadata(&p).expect("元数据").len());
    }

    /// D12: 请求超大的 N 一律夹到 [`TAIL_MAX_LINES`]。
    #[test]
    fn huge_n_is_clamped_to_max() {
        let p = tmp_file("clamp");
        let all: Vec<String> = (1..=300).map(|i| format!("l{i}\n")).collect();
        std::fs::write(&p, all.concat()).expect("写日志");

        let got = tail_since(&p, 0, 100_000).expect("尾读");
        assert_eq!(got.lines.len(), TAIL_MAX_LINES);
        assert_eq!(got.lines.first().map(String::as_str), Some("l101"), "取的是最后 200 行");
        assert_eq!(got.lines.last().map(String::as_str), Some("l300"));
    }

    /// D13 / SC-008: 文件被截断/轮转后**不丢行、不重复**; 且如实标 `rotated`。
    #[test]
    fn rotation_loses_nothing_and_repeats_nothing() {
        let p = tmp_file("rotate");
        std::fs::write(&p, "o1\no2\no3\n").expect("写日志");
        let first = tail_since(&p, 0, 200).expect("首次");
        assert_eq!(first.lines, vec!["o1", "o2", "o3"]);

        // 轮转: 旧内容被换掉, 文件比上次 offset 短。
        std::fs::write(&p, "n1\n").expect("轮转后写");
        let got = tail_since(&p, first.offset, 200).expect("轮转后");
        assert!(got.rotated, "变短必须报 rotated");
        assert_eq!(got.lines, vec!["n1"], "新内容不丢");
        assert_eq!(got.offset, 3);
        // 旧行不会再给一遍。
        let again = tail_since(&p, got.offset, 200).expect("再读");
        assert!(again.lines.is_empty() && !again.rotated);

        // 轮转后继续增量: 接着给新文件里新写的行。
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).expect("打开追加");
        f.write_all(b"n2\n").expect("追加");
        drop(f);
        let tail = tail_since(&p, got.offset, 200).expect("增量");
        assert_eq!(tail.lines, vec!["n2"]);
    }

    /// 末尾没写完的那截不算一行 —— 补齐后才给, 且**只给一次**。
    #[test]
    fn partial_last_line_waits_for_newline() {
        let p = tmp_file("partial");
        std::fs::write(&p, "x\nhalf").expect("写日志");
        let got = tail_since(&p, 0, 200).expect("尾读");
        assert_eq!(got.lines, vec!["x"], "半截行不当作一行");
        assert_eq!(got.offset, 2, "offset 停在半截行之前");

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).expect("打开追加");
        f.write_all(b"\n").expect("补换行");
        drop(f);

        let done = tail_since(&p, got.offset, 200).expect("补齐后");
        assert_eq!(done.lines, vec!["half"], "补齐后才给, 且不带重复的 x");
        let empty = tail_since(&p, done.offset, 200).expect("再读");
        assert!(empty.lines.is_empty());
    }

    /// 文件不存在 → `NotFound`(调用方据此如实说"还没有日志")。
    #[test]
    fn missing_file_is_not_found() {
        let p = tmp_file("missing");
        let err = tail_since(&p, 0, 200).expect_err("不存在应报错");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
