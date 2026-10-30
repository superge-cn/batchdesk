//! 文本采样与流式读取（Wave 2-0b；方案 §4.8 三档策略、§5.2 检测采样、§5.15）。
//!
//! 三件事：
//! 1. **采样读取**：读头部窗口＋若干等距窗口，供 C01 编码检测（§5.2 §②）；
//! 2. **行数估算**：由采样新行数按字节比例外推（§4.8 "> 2 GB 先快速扫描"的
//!    进度百分比来源），不整文件读入内存；
//! 3. **分段流式读**：固定块读取、块内按 `\n` 切段回调，内存占用与文件大小
//!    解耦（§4.8 10 MB–2 GB 纯流式）；同时给出超长行计数（T03）与"全程无
//!    换行"信号（T06）。
//!
//! 采样窗口与超长行阈值一律由调用方显式传入，本 crate 不定义产品默认值
//! （与 `scan::ScanOptions::quick_window` 同一约定）；块大小只是 I/O 粒度，
//! 不是产品阈值，故为内部常量。
//!
//! 注意：采样块可能从多字节字符中间开始，拼接样本做 UTF-8 严格校验时可能
//! 出现"边界误报"；kernel 的置信规则把解码错误一律降级为需人工确认
//! （ADR-0014），因此该情形只会多一次确认，不会静默猜错。

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::fs::fs_path;

/// 流式读取的 I/O 块大小（实现粒度，非产品阈值；§4.8 只给"4 MB 量级"示例）。
const READ_CHUNK: usize = 64 * 1024;

/// 采样规格：头部窗口 `window_bytes`，其后 `blocks` 个等距窗口（可含末窗）。
///
/// `blocks = 0` 表示只读头部窗口；两窗口互相重叠时自动去重，不重复读盘。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleSpec {
    pub window_bytes: u64,
    pub blocks: usize,
}

/// 采样结果：拼接后的字节、文件大小、是否已覆盖整个文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampledBytes {
    pub bytes: Vec<u8>,
    pub file_size: u64,
    /// 采样窗口是否已覆盖整个文件（小文件整读时恒为 true）。
    pub complete: bool,
}

/// 按需读取采样窗口（方案 §5.2 "头部与随机采样块"）。
///
/// 小文件（不超过单个窗口）整读；否则读 `[0, window)` 与
/// `[(file_size-window)*i/blocks, +window)`（`i = 1..=blocks`），
/// 跳过与已读区间重叠的部分。`window_bytes == 0` 视为调用方错误。
pub fn sample_bytes(path: &Path, spec: SampleSpec) -> io::Result<SampledBytes> {
    if spec.window_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sample window_bytes must be positive",
        ));
    }
    if spec.window_bytes > usize::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sample window_bytes exceeds addressable memory",
        ));
    }

    let mut file = File::open(fs_path(path))?;
    let file_size = file.metadata()?.len();
    let window = spec.window_bytes;

    if file_size <= window {
        let mut bytes = Vec::with_capacity(file_size as usize);
        file.take(file_size).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != file_size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file shrank while sampling",
            ));
        }
        return Ok(SampledBytes {
            bytes,
            file_size,
            complete: true,
        });
    }

    let mut offsets = Vec::with_capacity(spec.blocks + 1);
    offsets.push(0u64);
    let span = file_size - window;
    for index in 1..=spec.blocks as u64 {
        offsets.push(span.saturating_mul(index) / spec.blocks as u64);
    }

    let mut bytes = Vec::new();
    let mut cursor = 0u64;
    let mut contiguous = true;
    for start in offsets {
        if start > cursor {
            contiguous = false;
        }
        // 跳过与上一窗口重叠的部分（窗口间距小于 window 时会发生）。
        let start = start.max(cursor);
        if start >= file_size {
            break;
        }
        let end = (start + window).min(file_size);
        let length = (end - start) as usize;
        let mut buffer = vec![0u8; length];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer)?;
        bytes.extend_from_slice(&buffer);
        cursor = end;
    }

    Ok(SampledBytes {
        bytes,
        file_size,
        complete: contiguous && cursor == file_size,
    })
}

/// 采样行数估算结果（供进度百分比；非精确行数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEstimate {
    pub estimated_lines: u64,
    pub sampled_bytes: u64,
    pub sampled_newlines: u64,
    pub file_size: u64,
    /// 采样窗口内没有任何换行（T06 "无换行超大单行"的早期信号）。
    pub sampled_without_newline: bool,
}

/// 由采样字节按比例外推行数（纯函数，不触盘）。
///
/// 口径：以 `\n` 计行（与 F01 的切分边界一致）；采样完整且末字节不是换行时
/// 末行按一行补计。采样不全时按"新行数/采样字节 × 文件大小"取整估算。
pub fn estimate_lines(sampled: &SampledBytes) -> LineEstimate {
    let sampled_newlines = sampled.bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let sampled_bytes = sampled.bytes.len() as u64;
    let mut estimated_lines = if sampled_bytes == 0 {
        0
    } else {
        sampled_newlines
            .saturating_mul(sampled.file_size)
            .checked_div(sampled_bytes)
            .unwrap_or(0)
    };
    if sampled.complete && sampled_newlines > 0 && sampled.bytes.last() != Some(&b'\n') {
        estimated_lines = estimated_lines.saturating_add(1);
    }
    LineEstimate {
        estimated_lines,
        sampled_bytes,
        sampled_newlines,
        file_size: sampled.file_size,
        sampled_without_newline: sampled_bytes > 0 && sampled_newlines == 0,
    }
}

/// 段回调的返回值：继续或提前终止（终端用户取消、或调用方已凑够目标体积）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamFlow {
    Continue,
    Stop,
}

/// 流式读出的一段字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment<'a> {
    pub bytes: &'a [u8],
    /// 该段所属行号（0 起）；同一行的后续段沿用同号。
    pub line_index: u64,
    /// 该段是否以 `\n` 结束（结束即该行完结）。
    pub ends_line: bool,
    /// 该段是否为当前行的第一段（F01 据此决定分片起点）。
    pub line_start: bool,
}

/// 流式读取汇总（精确计数，内存占用恒定）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamSummary {
    pub bytes_read: u64,
    /// 行数：`\n` 完结的行每行计一；末行无换行时补计一行。
    pub lines: u64,
    pub max_line_bytes: u64,
    /// 长度超过 `max_line_bytes` 阈值的行数（T03；阈值为 None 时恒为 0）。
    pub overlong_lines: u64,
    /// 文件是否出现过 `\n`（false 即 T06 "无任何换行"，F01 需按字节硬切并告警）。
    pub had_newline: bool,
    /// 回调是否提前终止（此时 `lines`/`overlong_lines` 只统计到终止处）。
    pub stopped_early: bool,
}

/// 逐段流式读取：固定块读盘，块内按 `\n` 切段交给回调（方案 §4.3 统一管道）。
///
/// 不整文件读入内存；单行再长也只是分段回调（T06 的超大单行不会撑爆内存）。
/// `max_line_bytes` 为 T03 的"超长行"判定阈值，由调用方按目标分片值传入。
pub fn stream_segments<F>(
    path: &Path,
    max_line_bytes: Option<u64>,
    mut on_segment: F,
) -> io::Result<StreamSummary>
where
    F: FnMut(Segment<'_>) -> StreamFlow,
{
    let mut file = File::open(fs_path(path))?;
    let mut buffer = vec![0u8; READ_CHUNK];
    let mut summary = StreamSummary::default();
    let mut current_line_bytes = 0u64;
    let mut line_start = true;

    'read: loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        summary.bytes_read += read as u64;

        let mut start = 0usize;
        while let Some(position) = buffer[start..read].iter().position(|byte| *byte == b'\n') {
            let end = start + position;
            let piece = &buffer[start..=end];
            current_line_bytes += piece.len() as u64;
            let flow = on_segment(Segment {
                bytes: piece,
                line_index: summary.lines,
                ends_line: true,
                line_start,
            });
            summary.lines += 1;
            summary.max_line_bytes = summary.max_line_bytes.max(current_line_bytes);
            summary.overlong_lines += overlong(max_line_bytes, current_line_bytes);
            summary.had_newline = true;
            current_line_bytes = 0;
            line_start = true;
            start = end + 1;
            if flow == StreamFlow::Stop {
                summary.stopped_early = true;
                break 'read;
            }
        }

        if start < read {
            let piece = &buffer[start..read];
            current_line_bytes += piece.len() as u64;
            let flow = on_segment(Segment {
                bytes: piece,
                line_index: summary.lines,
                ends_line: false,
                line_start,
            });
            summary.max_line_bytes = summary.max_line_bytes.max(current_line_bytes);
            line_start = false;
            if flow == StreamFlow::Stop {
                summary.stopped_early = true;
                break 'read;
            }
        }
    }

    // 末行没有以换行结尾时补计一行（与 `estimate_lines` 的口径一致）。
    if current_line_bytes > 0 {
        summary.lines += 1;
        summary.max_line_bytes = summary.max_line_bytes.max(current_line_bytes);
        summary.overlong_lines += overlong(max_line_bytes, current_line_bytes);
    }

    Ok(summary)
}

/// F01 分片布局：流式扫描全文件，逐行驱动 kernel 的纯分片累加器。
///
/// 只保留分片边界（内存 O(分片数)），不缓存文件内容（方案 §5.1）。
pub fn split_layout(path: &Path, target_bytes: u64) -> io::Result<kernel::split::SplitLayout> {
    let accumulator = kernel::split::SplitAccumulator::new(target_bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;
    scan_layout(path, accumulator)
}

/// F02 分片布局：每满 `lines_per_part` 行收一片，尾片保留（方案 §5.15）。
///
/// 与 F01 共用同一流式扫描与累加器，只是计量基准换成行数。
pub fn split_layout_by_lines(
    path: &Path,
    lines_per_part: u64,
) -> io::Result<kernel::split::SplitLayout> {
    let accumulator = kernel::split::SplitAccumulator::new_lines(lines_per_part)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;
    scan_layout(path, accumulator)
}

/// F03 分片布局：先做行数预扫描，再按"总行数÷份数取整、余数进前几片"分配（方案 §5.15）。
///
/// 预扫描只计行数（不缓存内容）；第二遍流式扫描把字节边界落到行尾。
/// 份数超过总行数时按行数封顶（每片至少 1 行，不产生空片）。
pub fn split_layout_by_parts(path: &Path, parts: u64) -> io::Result<kernel::split::SplitLayout> {
    if parts == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "parts must be greater than zero",
        ));
    }
    let mut line_bytes = 0u64;
    let mut total_lines = 0u64;
    stream_segments(path, None, |segment| {
        line_bytes += segment.bytes.len() as u64;
        if segment.ends_line {
            total_lines += 1;
            line_bytes = 0;
        }
        StreamFlow::Continue
    })?;
    if line_bytes > 0 {
        total_lines += 1;
    }
    let quotas = kernel::split::part_line_quotas(total_lines, parts);
    let accumulator = kernel::split::SplitAccumulator::new_quotas(quotas)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;
    scan_layout(path, accumulator)
}

fn scan_layout(
    path: &Path,
    mut accumulator: kernel::split::SplitAccumulator,
) -> io::Result<kernel::split::SplitLayout> {
    let file_size = File::open(fs_path(path))?.metadata()?.len();
    let mut line_bytes = 0u64;
    stream_segments(path, None, |segment| {
        line_bytes += segment.bytes.len() as u64;
        if segment.ends_line {
            accumulator.push_line(line_bytes, true);
            line_bytes = 0;
        }
        StreamFlow::Continue
    })?;
    if line_bytes > 0 {
        accumulator.push_line(line_bytes, false);
    }
    Ok(accumulator.finish(file_size))
}

fn overlong(threshold: Option<u64>, line_bytes: u64) -> u64 {
    match threshold {
        Some(limit) if line_bytes > limit => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-text-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn samples_small_file_in_full() {
        let dir = TempDir::new();
        let path = dir.0.join("small.txt");
        fs::write(&path, b"hello").unwrap();
        let sampled = sample_bytes(
            &path,
            SampleSpec {
                window_bytes: 8,
                blocks: 2,
            },
        )
        .unwrap();
        assert!(sampled.complete);
        assert_eq!(sampled.bytes, b"hello");
        assert_eq!(sampled.file_size, 5);
    }

    #[test]
    fn samples_head_and_evenly_spaced_blocks() {
        let dir = TempDir::new();
        let path = dir.0.join("big.txt");
        let mut content = vec![b'a'; 20_480];
        content[..3].copy_from_slice(b"TOP");
        content[20_480 - 3..].copy_from_slice(b"END");
        fs::write(&path, &content).unwrap();

        let sampled = sample_bytes(
            &path,
            SampleSpec {
                window_bytes: 4096,
                blocks: 2,
            },
        )
        .unwrap();
        assert_eq!(sampled.file_size, 20_480);
        assert!(!sampled.complete, "等距窗口之间仍有未采样空隙");
        assert_eq!(sampled.bytes.len(), 4096 * 3);
        assert_eq!(&sampled.bytes[..3], b"TOP");
        assert_eq!(&sampled.bytes[sampled.bytes.len() - 3..], b"END");
    }

    #[test]
    fn samples_adjacent_windows_once_when_they_overlap() {
        let dir = TempDir::new();
        let path = dir.0.join("overlap.txt");
        fs::write(&path, vec![b'x'; 12_000]).unwrap();
        let sampled = sample_bytes(
            &path,
            SampleSpec {
                window_bytes: 4096,
                blocks: 8,
            },
        )
        .unwrap();
        assert_eq!(sampled.bytes.len() as u64, sampled.file_size);
        assert!(sampled.complete);
    }

    #[test]
    fn rejects_zero_window() {
        let dir = TempDir::new();
        let path = dir.0.join("x.txt");
        fs::write(&path, b"x").unwrap();
        let error = sample_bytes(
            &path,
            SampleSpec {
                window_bytes: 0,
                blocks: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn estimates_lines_by_byte_ratio() {
        let sampled = SampledBytes {
            bytes: b"a\nb\nc\n".to_vec(),
            file_size: 12,
            complete: false,
        };
        let estimate = estimate_lines(&sampled);
        assert_eq!(estimate.sampled_newlines, 3);
        assert_eq!(estimate.estimated_lines, 6);
        assert!(!estimate.sampled_without_newline);
    }

    #[test]
    fn estimate_adds_last_line_without_newline_when_complete() {
        let sampled = SampledBytes {
            bytes: b"a\nb".to_vec(),
            file_size: 3,
            complete: true,
        };
        assert_eq!(estimate_lines(&sampled).estimated_lines, 2);
    }

    #[test]
    fn estimate_flags_missing_newline() {
        let sampled = SampledBytes {
            bytes: b"plain ascii".to_vec(),
            file_size: 100,
            complete: false,
        };
        let estimate = estimate_lines(&sampled);
        assert_eq!(estimate.estimated_lines, 0);
        assert!(estimate.sampled_without_newline);
    }

    #[test]
    fn streams_segments_and_counts_lines() {
        let dir = TempDir::new();
        let path = dir.0.join("lines.txt");
        fs::write(&path, b"a\nbb\nccc").unwrap();

        let mut collected = Vec::new();
        let summary = stream_segments(&path, None, |segment| {
            collected.extend_from_slice(segment.bytes);
            StreamFlow::Continue
        })
        .unwrap();

        assert_eq!(summary.lines, 3);
        assert_eq!(summary.bytes_read, 8);
        assert_eq!(summary.max_line_bytes, 3);
        assert_eq!(summary.overlong_lines, 0);
        assert!(summary.had_newline);
        assert!(!summary.stopped_early);
        assert_eq!(collected, b"a\nbb\nccc");
    }

    #[test]
    fn streams_line_without_newline_and_flags_it() {
        let dir = TempDir::new();
        let path = dir.0.join("single.txt");
        fs::write(&path, vec![b'z'; 5000]).unwrap();

        let summary = stream_segments(&path, None, |_| StreamFlow::Continue).unwrap();
        assert_eq!(summary.lines, 1);
        assert_eq!(summary.max_line_bytes, 5000);
        assert!(!summary.had_newline, "T06：全程无换行需由调用方告警");
        assert_eq!(summary.bytes_read, 5000);
    }

    #[test]
    fn counts_overlong_lines_against_caller_threshold() {
        let dir = TempDir::new();
        let path = dir.0.join("overlong.txt");
        fs::write(&path, b"1234567890\nok\n").unwrap();

        let summary = stream_segments(&path, Some(4), |_| StreamFlow::Continue).unwrap();
        assert_eq!(summary.lines, 2);
        assert_eq!(summary.overlong_lines, 1, "T03：超过目标分片值的长行计数");
        assert_eq!(summary.max_line_bytes, 11);
    }

    #[test]
    fn streams_across_chunk_boundaries() {
        let dir = TempDir::new();
        let path = dir.0.join("boundary.txt");
        let mut content = Vec::new();
        for index in 0..READ_CHUNK / 8 + 64 {
            content.extend_from_slice(format!("line{index:04}\n").as_bytes());
        }
        fs::write(&path, &content).unwrap();

        let mut collected = Vec::new();
        let summary = stream_segments(&path, None, |segment| {
            collected.extend_from_slice(segment.bytes);
            StreamFlow::Continue
        })
        .unwrap();

        assert_eq!(collected, content, "跨块拼接必须与原文逐字节一致");
        assert_eq!(summary.bytes_read, content.len() as u64);
        assert_eq!(summary.lines, (READ_CHUNK / 8 + 64) as u64);
    }

    #[test]
    fn reports_line_start_and_index() {
        let dir = TempDir::new();
        let path = dir.0.join("starts.txt");
        fs::write(&path, b"one\ntwo\n").unwrap();

        let mut starts = Vec::new();
        stream_segments(&path, None, |segment| {
            starts.push((segment.line_index, segment.line_start, segment.ends_line));
            StreamFlow::Continue
        })
        .unwrap();
        assert_eq!(starts, vec![(0, true, true), (1, true, true)]);
    }

    #[test]
    fn stops_early_when_caller_requests() {
        let dir = TempDir::new();
        let path = dir.0.join("stop.txt");
        fs::write(&path, b"a\nb\nc\n").unwrap();

        let summary = stream_segments(&path, None, |segment| {
            if segment.ends_line && segment.line_index == 0 {
                StreamFlow::Stop
            } else {
                StreamFlow::Continue
            }
        })
        .unwrap();
        assert!(summary.stopped_early);
        assert_eq!(summary.lines, 1);
        // bytes_read 是读盘量（整块 6 字节），不是已消费位置。
        assert_eq!(summary.bytes_read, 6);
    }

    #[test]
    fn empty_file_has_no_lines() {
        let dir = TempDir::new();
        let path = dir.0.join("empty.txt");
        fs::write(&path, b"").unwrap();

        let summary = stream_segments(&path, None, |_| StreamFlow::Continue).unwrap();
        assert_eq!(summary.lines, 0);
        assert_eq!(summary.bytes_read, 0);
        assert!(!summary.had_newline);
    }

    #[test]
    fn splits_layout_by_line_count_and_keeps_tail() {
        let dir = TempDir::new();
        let path = dir.0.join("rows.txt");
        fs::write(&path, b"a\nb\nc\nd\ne\n").unwrap();

        let layout = split_layout_by_lines(&path, 2).unwrap();
        assert_eq!(layout.file_size, 10);
        assert_eq!(layout.spans.len(), 3);
        assert_eq!(
            layout
                .spans
                .iter()
                .map(|span| span.lines)
                .collect::<Vec<_>>(),
            vec![2, 2, 1],
            "尾片保留（方案 §5.15）"
        );
        assert!(layout.spans.iter().all(|span| !span.hard_cut));
    }

    #[test]
    fn line_layout_keeps_unterminated_tail_in_same_part() {
        let dir = TempDir::new();
        let path = dir.0.join("tail.txt");
        fs::write(&path, b"a\nb\nlast").unwrap();

        let layout = split_layout_by_lines(&path, 2).unwrap();
        assert_eq!(layout.spans.len(), 2);
        assert_eq!(layout.spans[0].lines, 2);
        assert_eq!(layout.spans[1].lines, 1, "末行无换行单独计入尾片");
        assert_eq!((layout.spans[1].start, layout.spans[1].end), (4, 8));
    }

    #[test]
    fn splits_layout_by_parts_with_remainder_on_first_parts() {
        let dir = TempDir::new();
        let path = dir.0.join("rows.txt");
        fs::write(&path, b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n").unwrap();

        let layout = split_layout_by_parts(&path, 3).unwrap();
        assert_eq!(layout.file_size, 20);
        assert_eq!(
            layout
                .spans
                .iter()
                .map(|span| span.lines)
                .collect::<Vec<_>>(),
            vec![4, 3, 3],
            "总行数÷份数取整，余数进前几片（方案 §5.15）"
        );
        assert_eq!(
            layout
                .spans
                .iter()
                .map(|span| span.len())
                .collect::<Vec<_>>(),
            vec![8, 6, 6]
        );
        assert_eq!(
            layout.spans.iter().map(|span| span.end).collect::<Vec<_>>(),
            vec![8, 14, 20]
        );
    }

    #[test]
    fn part_layout_caps_at_line_count_and_keeps_unterminated_tail() {
        let dir = TempDir::new();
        let capped = dir.0.join("capped.txt");
        fs::write(&capped, b"a\nb\nc\n").unwrap();
        let layout = split_layout_by_parts(&capped, 5).unwrap();
        assert_eq!(layout.spans.len(), 3, "份数超行数时每片 1 行，不产生空片");

        let tail = dir.0.join("tail.txt");
        fs::write(&tail, b"a\nb\nc").unwrap();
        let layout = split_layout_by_parts(&tail, 2).unwrap();
        assert_eq!(
            layout
                .spans
                .iter()
                .map(|span| span.lines)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!((layout.spans[1].start, layout.spans[1].end), (4, 5));
    }

    #[test]
    fn part_layout_rejects_zero_parts() {
        let dir = TempDir::new();
        let path = dir.0.join("rows.txt");
        fs::write(&path, b"a\nb\n").unwrap();
        assert!(split_layout_by_parts(&path, 0).is_err());
    }
}
