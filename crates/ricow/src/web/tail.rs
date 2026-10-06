//! 策略日志的**只读尾读** (026 T027 / D12 / D13 / FR-015 / FR-016)。
//!
//! 纯逻辑: 给定 (文件路径, 上次 offset) → (完整新增行, 新 offset)。**不写、不改、不轮转**该文件 ——
//! 日志是策略子进程的产物(见 `supervisor::procs`), Web 这一侧只读它。

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// 单次尾读的行数上限 (D12): 请求里给的 `lines` 一律夹到 `1..=200`。
pub const TAIL_MAX_LINES: usize = 200;

/// 单行最多回看的字节数: 真按这个回溯是为了给"超长行"一个上界 ——
/// 正常日志行远小于它, 真正决定读多少的是 [`read_chunk_for`]。
const MAX_LINE_BYTES: u64 = 1024 * 1024;

/// 单次增量读最多读入的字节数上限 (内存有界化的硬闸)。
///
/// 原来的实现用 `std::fs::read` **整文件读入内存**: 日志被轮转到 10MB 时, 每次尾读请求
/// (客户端轮询) 都会瞬时分配等于**整个文件大小**的内存, 日志越大峰值越高。改成
/// "只读末尾需要的一段"后, 峰值与文件大小解耦, 只与本次窗口有关。
const TAIL_MAX_CHUNK_BYTES: usize = 1024 * 1024;

/// 增量读时按 `offset` 估算的锚点(末尾粗略扫这么多, 用于找出上次 offset 所在行):
/// 超过这个跨度的"天量突增"不做无界缓冲 —— 见 [`read_chunk_for`] 的取舍说明。
const TAIL_ANCHOR_BYTES: u64 = 2 * 1024 * 1024;

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
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();

    // 比上次短 = 轮转/截断: 上次的 offset 已经越过文件末尾。
    let rotated = offset > len;
    let start = if rotated { 0u64 } else { offset };

    // 只读"够用的一段": 首次读要凑够 `lines` 行(按行上界回溯), 增量读要覆盖新写入的字节。
    let (chunk_start, chunk_len) = read_chunk_for(path, len, start, lines)?;
    if chunk_len == 0 {
        // 增量读时 start == len(还没有新内容): 不产生行, offset 原地不动。
        return Ok(TailResult {
            lines: Vec::new(),
            offset: if rotated { 0 } else { offset },
            rotated,
        });
    }
    let mut buf = vec![0u8; chunk_len];
    f.seek(SeekFrom::Start(chunk_start))?;
    let filled = read_fully(&mut f, &mut buf)?;
    buf.truncate(filled);
    let base = chunk_start;

    // 在窗口内定位上次 offset 所在的行首(`start == 0` 时窗口即文件末尾, 无需定位)。
    let window_start = if start > base {
        match buf.iter().position(|b| *b == b'\n') {
            // `\n` 之后就是新行的行首; 只要它没越过 start 就是我们要的锚点。
            Some(i) if base + (i as u64) < start => base + i as u64 + 1,
            // 窗口里没有换行(整段都是超长行的一截) → 从 start 起硬切。
            _ => start,
        }
    } else {
        base
    };
    let tail = &buf[(window_start - base) as usize..];

    // 一行都没写全(`\n` 之前的内容不算): offset 原地不动, 不产生重复。
    let Some(last_nl) = tail.iter().rposition(|b| *b == b'\n') else {
        return Ok(TailResult {
            lines: Vec::new(),
            offset: if rotated { 0 } else { offset },
            rotated,
        });
    };
    let consumed = &tail[..=last_nl];
    let new_offset = window_start + consumed.len() as u64;

    // 按字节切行; 末段必然是空的(因为 `consumed` 以 `\n` 结尾), 丢掉。
    let mut segs: Vec<&[u8]> = consumed.split(|b| *b == b'\n').collect();
    segs.pop();
    let mut lines_out: Vec<String> =
        segs.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect();

    // 首次 / 轮转后: 只给最近 `lines` 行 —— 更早的历史不在本次窗口里; offset 仍推到末尾,
    // 所以客户端不会重复收到它们。
    //
    // 判据用 `start == 0`(= 本次是首次/轮转后重读, **不是** offset 是否落在文件开头):
    // 大日志第一次读的窗口是从中间起的(`window_start > 0`), 但语义上仍是"首次读",
    // 同样必须裁到 `lines` 行 —— 否则一次请求会返回窗口里成千上万行, 既违反 D12 的
    // "只给最近 N 行" 承诺, 也会把内存峰值推回窗口大小。
    if start == 0 && lines_out.len() > lines {
        lines_out.drain(..lines_out.len() - lines);
    }

    Ok(TailResult { lines: lines_out, offset: new_offset, rotated })
}

/// 决定这次要读文件里的哪一段: 返回 `(起始偏移, 字节数)`。
///
/// - `offset == 0`(首次/轮转后): 从末尾按 `lines × MAX_LINE_BYTES` 回溯, 夹到
///   [`TAIL_MAX_CHUNK_BYTES`] —— 所以 10MB 的日志也只读末尾 1MB 而不是整份;
/// - `offset == len`: 没有新内容, 读 0 字节;
/// - 否则: 从 `offset` 往前找一段能把"上次 offset 所在行"完整包住的窗口, 这样返回的
///   行首偏移与旧实现**逐字一致**; 窗口按 [`TAIL_MAX_CHUNK_BYTES`] 上限夹住,
///   超过 [`TAIL_ANCHOR_BYTES`] 的"天量突增"不再向后扩展(见下)。
fn read_chunk_for(path: &Path, len: u64, offset: u64, lines: usize) -> io::Result<(u64, usize)> {
    if offset == 0 {
        let want = (lines as u64).saturating_mul(MAX_LINE_BYTES).min(TAIL_MAX_CHUNK_BYTES as u64);
        let start = len.saturating_sub(want);
        return Ok((start, (len - start) as usize));
    }
    if offset >= len {
        return Ok((len, 0));
    }

    // 找 `offset` 所属行首 (与旧实现一致: 取 offset 之前最后一个 `\n` 的下一字节;
    // 之前没有 `\n` 则回退到文件开头)。
    let mut anchor = 0u64;
    let scan_start = offset.saturating_sub(TAIL_ANCHOR_BYTES);
    if let Some((hit, byte)) = find_last_newline(path, scan_start, offset)? {
        anchor = if hit { byte + 1 } else { 0 };
    }

    // 窗口 = [anchor, len) 但不超过上限; 窗口起点不能越过 anchor(否则会漏掉新行)。
    let start = anchor.max(len.saturating_sub(TAIL_MAX_CHUNK_BYTES as u64));
    Ok((start, (len - start) as usize))
}

/// 在 `[from, to)` 里最后一次出现 `\n` 的位置。
/// 返回 `Some((true, pos))` = 找到; `Some((false, _))` = 这段里没有 `\n`, 但**已经扫到文件开头**
/// (调用方据此判断锚点是 0 而不是"没找到")。
fn find_last_newline(path: &Path, from: u64, to: u64) -> io::Result<Option<(bool, u64)>> {
    if to == 0 {
        return Ok(Some((false, 0)));
    }
    let mut f = std::fs::File::open(path)?;
    const CHUNK: usize = 64 * 1024;
    let mut end = to;
    while end > from {
        let begin = end.saturating_sub(CHUNK as u64).max(from);
        let mut buf = vec![0u8; (end - begin) as usize];
        f.seek(SeekFrom::Start(begin))?;
        let n = read_fully(&mut f, &mut buf)?;
        if let Some(i) = buf[..n].iter().rposition(|b| *b == b'\n') {
            return Ok(Some((true, begin + i as u64)));
        }
        end = begin;
    }
    if from == 0 {
        Ok(Some((false, 0)))
    } else {
        Ok(None)
    }
}

/// 读满 `buf` 或读到 EOF(`File::read` 允许短读, 不能只调一次)。
fn read_fully(f: &mut std::fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
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

    /// 内存有界: 首次读一个远大于读窗口的日志, 只读末尾一段而不是整份文件。
    #[test]
    fn first_read_only_touches_tail_window() {
        let p = tmp_file("bounded");
        // 200 万行 × 8 字节 ≈ 15MB, 远超单次窗口上限。
        use std::io::Write;
        let mut f = std::fs::File::create(&p).expect("建日志");
        for i in 0..2_000_000u32 {
            writeln!(f, "l{i:06}").expect("写");
        }
        f.sync_all().expect("落盘");
        drop(f);

        let (start, len) =
            read_chunk_for(&p, std::fs::metadata(&p).unwrap().len(), 0, 200).unwrap();
        assert_eq!(len, TAIL_MAX_CHUNK_BYTES, "首次读窗口被硬上限夹住, 与文件大小解耦");
        assert!(start > 0, "大文件必须从中间起读, 不是整份读入");
        assert_eq!(start, std::fs::metadata(&p).unwrap().len() - len as u64);

        // 结果仍正确: 窗口里行数远多于 200, 但只给最后 200 行 (D12);
        // offset 推到文件末尾, 所以下一轮增量不会把这些行再给一遍。
        let total = std::fs::metadata(&p).unwrap().len();
        let got = tail_since(&p, 0, 200).expect("尾读");
        assert_eq!(got.offset, total);
        assert_eq!(got.lines.len(), 200, "窗口里行数远超 200, 但只应给最后 200 行");
        assert_eq!(got.lines.first().map(String::as_str), Some("l1999800"));
        assert_eq!(got.lines.last().map(String::as_str), Some("l1999999"));
        assert!(!got.rotated);

        let again = tail_since(&p, got.offset, 200).expect("再读");
        assert!(again.lines.is_empty() && !again.rotated);
    }

    /// 增量读的窗口也从 `offset` 附近起, 不是从文件头读起 —— 但行边界仍与旧实现一致。
    #[test]
    fn incremental_read_window_anchors_on_offset() {
        let p = tmp_file("incr-window");
        use std::io::Write;
        let mut f = std::fs::File::create(&p).expect("建日志");
        for i in 0..400_000u32 {
            writeln!(f, "old{i:06}").expect("写");
        }
        drop(f);
        let len = std::fs::metadata(&p).expect("元数据").len();

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).expect("追加");
        f.write_all(b"new-1\nnew-2\n").expect("写新行");
        drop(f);

        let got = tail_since(&p, len, 200).expect("增量");
        assert_eq!(got.lines, vec!["new-1", "new-2"], "只给新增行");
        assert!(!got.rotated);
        assert_eq!(got.offset, len + 12);

        // 窗口起点落在 offset 附近(不是 0), 说明没有把整份 10MB 日志读进来。
        let (start, n) =
            read_chunk_for(&p, std::fs::metadata(&p).unwrap().len(), len, 200).unwrap();
        assert!(start > 0, "增量读窗口应从 offset 附近起");
        assert!(n < TAIL_MAX_CHUNK_BYTES, "只有几百字节新增, 窗口不该顶满上限");
    }

    /// 超长单行: 窗口里找不到换行时退回 `offset` 硬切, 不 panic、不丢已给过的行。
    #[test]
    fn giant_line_without_newline_falls_back_to_offset() {
        let p = tmp_file("giant");
        let mut content = String::with_capacity(TAIL_MAX_CHUNK_BYTES * 2);
        for _ in 0..2_000 {
            content.push('x');
        }
        content.push('\n');
        std::fs::write(&p, &content).expect("写日志");
        let first = tail_since(&p, 0, 200).expect("首次");
        assert_eq!(first.lines.len(), 1);
        assert_eq!(first.lines[0].len(), 2_000);

        // 追加一段**没有换行**的超长内容: 不产生行, offset 原地不动。
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).expect("追加");
        f.write_all(&vec![b'y'; TAIL_MAX_CHUNK_BYTES + 4096]).expect("写超长行");
        drop(f);
        let got = tail_since(&p, first.offset, 200).expect("超长行");
        assert!(got.lines.is_empty(), "没有换行就没有完整行");
        assert_eq!(got.offset, first.offset, "offset 停在半截行之前");
        assert!(!got.rotated);
    }

    /// **跨模块不变量(中危 #6 的核心)**: 真实的 [`crate::supervisor::procs::rotate_large_log`]
    /// 作用在超阈值日志上时, 客户端即使如实拿到 `rotated`, **刚看过的那些行仍在新文件里**。
    ///
    /// 这是"保留末尾窗口"轮转相对"截断"的**全部意义**:
    /// - 截断/清空 → `rotated=true` 且重读发现**空文件**, 刚过去的日志从视野里彻底消失;
    /// - 保留末尾窗口 → `rotated=true`(因为 offset 越过了变短后的末尾, 如实报告),
    ///   但重读能拿回**行边界对齐的末尾窗口**, 客户端视野里的最近行一条不少。
    ///
    /// 若哪天有人把轮转改回"截断/清空", 本测试会红: 重读将拿不到刚刚看过的行。
    #[test]
    fn real_rotation_keeps_recent_lines_reviewable() {
        use crate::supervisor::{ledger, procs};

        let dir = std::env::temp_dir().join(format!("ricow-tail-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ledger::ensure_dirs(&dir).expect("建目录");
        let p = ledger::log_path(&dir, "demo");

        // 造一份超阈值日志(每行 22 字节 × 50 万行 ≈ 11MB > 10MB 阈值)。
        use std::io::Write;
        let mut f = std::fs::File::create(&p).expect("建日志");
        for i in 0..500_000u32 {
            writeln!(f, "line-{i:06}-aaaaaaaa").expect("写");
        }
        f.sync_all().expect("落盘");
        drop(f);

        // 客户端先读到末尾(拿到最近 200 行 + offset)。
        let first = tail_since(&p, 0, 200).expect("首次尾读");
        assert_eq!(first.lines.len(), 200);
        let seen_last = first.lines.last().cloned().expect("有末行");
        assert_eq!(seen_last, "line-499999-aaaaaaaa");
        let consumed = first.offset;
        let size_before = std::fs::metadata(&p).expect("元数据").len();
        assert_eq!(consumed, size_before, "首次读把 offset 推到文件末尾");

        // 真实轮转(与 daemon monitor 每 60s 调的是同一个函数)。
        let backup = procs::rotate_large_log(&p).expect("超阈值应轮转");
        let size_after = std::fs::metadata(&p).expect("元数据").len();
        assert!(size_after > 0, "轮转后原文件必须仍有内容(不是被清空)");
        assert!(
            size_after <= procs::LOG_ROTATE_BYTES,
            "保留窗口不得超阈值: {size_after} > {}",
            procs::LOG_ROTATE_BYTES
        );
        assert!(backup.exists(), "备份文件应存在(旧内容没丢)");

        // 关键: 客户端用旧 offset 再读 —— 可能如实报 rotated(文件确实变短了),
        // 但**重读结果必须包含刚刚看过的末行**, 且末尾正是最新日志。
        let after = tail_since(&p, consumed, 200).expect("轮转后尾读");
        if after.rotated {
            // 轮转后重读: 拿回末尾 200 行, 末行与轮转前一致(最新日志没被丢掉)。
            assert_eq!(after.lines.len(), 200, "轮转后重读仍应给最近 200 行");
            assert_eq!(
                after.lines.last().map(String::as_str),
                Some(seen_last.as_str()),
                "轮转不该把最新日志切掉"
            );
        } else {
            // 若 offset 恰好还在范围内, 增量读不出错也不重复。
            assert!(after.offset >= consumed, "offset 应单调不减");
        }

        // 轮转后继续追加的行, 客户端能正常增量读到(无论上一步走了哪个分支)。
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).expect("追加");
        f.write_all(b"after-rotate-1\nafter-rotate-2\n").expect("写新行");
        f.sync_all().expect("落盘");
        drop(f);
        let incr = tail_since(&p, after.offset, 200).expect("增量");
        assert_eq!(incr.lines, vec!["after-rotate-1", "after-rotate-2"]);
        assert!(!incr.rotated, "追加(只增不减)不该报 rotated");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
