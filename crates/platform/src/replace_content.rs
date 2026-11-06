//! C03 文件检查：按块读盘并驱动内核的流式采集器（内存与文件体积无关）。
//!
//! 三条通道由 `kernel::replace_content::ReplaceScan` 当场决定：
//! - plain 文本、以及**静态可界定上界**的正则：逐块喂流式采集器，内存 O(块＋窗口)；
//! - CSV 单元格：记录级增量（`RecordFeeder`）＋`CsvRecordScanner` 的未闭合引号判定，
//!   内存 O(单条记录)；分隔符未定时先按整段版同口径流式嗅探全文（只 feed、不 finish）；
//! - 界定不出的正则（无界量词／锚点／`\b`／分组量化／零宽）：**仍整段读**，与 ADR-0064
//!   给改写段划的是同一条边界，内存 O(文件)。
//!
//! 预览上下文不再靠整段原文：内核给出首个匹配的字节偏移，这里按「±半径＋边界余量」
//! 做一次有界回读，再交给与整段版同一条 `context_from_window` 规则重建。

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Take};
use std::path::Path;

use kernel::csv::{Delimiter, DelimiterSniffer};
use kernel::error::KernelError;
use kernel::replace_content::{
    context_from_window, inspect_bytes, ContentStructure, CsvMode, ReplaceInspect, ReplaceMode,
    ReplaceScan, ReplaceStreamInspector, StreamInspect, CONTEXT_RADIUS_BYTES, CONTEXT_SLACK_BYTES,
};

use crate::fs::STREAM_CHUNK_BYTES;

pub struct ReplaceFileInspect {
    pub sample: Vec<u8>,
    pub file_size: u64,
    pub structure: ContentStructure,
    pub inspect: ReplaceInspect,
}

pub fn inspect_file(
    path: &Path,
    mode: ReplaceMode,
    find: &str,
    replacement: &str,
    csv_mode: CsvMode,
    delimiter: Option<Delimiter>,
    sample_bytes: usize,
) -> Result<ReplaceFileInspect, KernelError> {
    if sample_bytes == 0 {
        return Err(KernelError::invalid_params(
            "sample bytes must be a positive count",
        ));
    }
    let fs_path = crate::fs::fs_path(path);
    let metadata = fs::metadata(&fs_path).map_err(|error| read_error(path, &error))?;
    if !metadata.is_file() {
        return Err(KernelError::invalid_params("source must be a regular file"));
    }
    let structure = ContentStructure::resolve(&path.to_string_lossy());
    // 编码样本的口径不变：头部 sample_bytes 字节（文件更短就是全文）。
    let sample = read_head(&fs_path, sample_bytes).map_err(|error| read_error(path, &error))?;
    let cell_arm = structure == ContentStructure::Csv && csv_mode == CsvMode::Cell;
    let delimiter = match delimiter {
        Some(delimiter) => Some(delimiter),
        None if cell_arm => sniff_file_delimiter(path, &fs_path)?,
        None => None,
    };
    if cell_arm && delimiter.is_none() {
        // 嗅探不出分隔符时整段版也不计数、不报错，只把 structure 带出（计划层按告警处理）。
        return Ok(ReplaceFileInspect {
            sample,
            file_size: metadata.len(),
            structure,
            inspect: ReplaceInspect {
                structure,
                ..ReplaceInspect::default()
            },
        });
    }
    let (mut inspect, context_probe, text_len) = match ReplaceStreamInspector::start(
        structure,
        csv_mode,
        mode,
        find,
        replacement,
        delimiter,
    )? {
        ReplaceScan::Streaming(inspector) => {
            let outcome = stream_through(path, &fs_path, inspector)?;
            (outcome.inspect, outcome.context_probe, outcome.text_len)
        }
        ReplaceScan::WholeFile => {
            let data = fs::read(&fs_path).map_err(|error| read_error(path, &error))?;
            let inspect = inspect_bytes(
                &data,
                mode,
                find,
                replacement,
                csv_mode,
                structure,
                delimiter,
            )?;
            return Ok(ReplaceFileInspect {
                sample,
                file_size: metadata.len(),
                structure,
                inspect,
            });
        }
        ReplaceScan::SniffFirst => {
            return Err(KernelError::internal(
                "CSV cell scanning reached the sniff branch after the delimiter was resolved",
                format!("{path:?}"),
            ));
        }
    };
    if let Some(probe) = context_probe {
        // 有界回读：只取首个匹配附近那一小段，不做全文二次扫描。
        let pad = CONTEXT_RADIUS_BYTES + CONTEXT_SLACK_BYTES;
        let window_start = probe.start.saturating_sub(pad);
        let window_end = (probe.start + probe.length + pad).min(text_len);
        let window = read_range(path, &fs_path, window_start, window_end)?;
        inspect.context = Some(context_from_window(&window, window_start, text_len, probe));
    }
    Ok(ReplaceFileInspect {
        sample,
        file_size: metadata.len(),
        structure,
        inspect,
    })
}

/// 逐块喂采集器（64 KiB 一块）。内存与文件体积无关，这是本次改造的全部目的。
fn stream_through(
    path: &Path,
    fs_path: &Path,
    mut inspector: ReplaceStreamInspector,
) -> Result<StreamInspect, KernelError> {
    let mut file = File::open(fs_path).map_err(|error| read_error(path, &error))?;
    let mut chunk = vec![0u8; STREAM_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| read_error(path, &error))?;
        if read == 0 {
            break;
        }
        inspector.push(&chunk[..read]);
    }
    Ok(inspector.finish())
}

/// 分隔符嗅探的流式驱动：判定与 `sniff_delimiter(全文)` 一致（同样不 finish 末条记录），
/// 区别是不把全文读进内存。
fn sniff_file_delimiter(path: &Path, fs_path: &Path) -> Result<Option<Delimiter>, KernelError> {
    let mut file = File::open(fs_path).map_err(|error| read_error(path, &error))?;
    let mut sniffer = DelimiterSniffer::new();
    let mut chunk = vec![0u8; STREAM_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| read_error(path, &error))?;
        if read == 0 {
            break;
        }
        sniffer.push(&chunk[..read]);
    }
    Ok(sniffer.finish())
}

fn read_head(fs_path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    read_limited(File::open(fs_path)?.take(limit as u64))
}

fn read_range(
    path: &Path,
    fs_path: &Path,
    start: usize,
    end: usize,
) -> Result<Vec<u8>, KernelError> {
    let mut file = File::open(fs_path).map_err(|error| read_error(path, &error))?;
    if start > 0 {
        file.seek(SeekFrom::Start(start as u64))
            .map_err(|error| read_error(path, &error))?;
    }
    let bytes = read_limited(file.take(end.saturating_sub(start) as u64))
        .map_err(|error| read_error(path, &error))?;
    // 窗口必须覆盖到 want_end（末端再加边界余量）；短了就说明偏移越界，宁可报错也不要算错上下文。
    if bytes.len() < end - start {
        return Err(KernelError::new(
            "E1004",
            "file changed while reading the preview window",
            Some(format!("{path:?}")),
            false,
        ));
    }
    Ok(bytes)
}

fn read_limited(mut reader: Take<File>) -> std::io::Result<Vec<u8>> {
    let limit = reader.limit();
    let mut bytes = Vec::with_capacity(usize::try_from(limit.min(1 << 20)).unwrap_or(1 << 20));
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_error(path: &Path, error: &std::io::Error) -> KernelError {
    KernelError::new(
        "E1004",
        "file is not readable",
        Some(format!("{}: {error}", path.display())),
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "batchdesk-replace-{}-{nanos}-{sequence}",
                std::process::id()
            ));
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
    fn inspects_text_matches_and_context() {
        let dir = TempDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, b"alpha beta alpha").unwrap();
        let inspect = inspect_file(
            &path,
            ReplaceMode::Plain,
            "alpha",
            "gamma",
            CsvMode::Cell,
            None,
            4,
        )
        .unwrap();
        assert_eq!(inspect.structure, ContentStructure::Text);
        assert_eq!(inspect.inspect.match_count, 2);
        assert_eq!(inspect.sample, b"alph");
        assert!(inspect.inspect.context.is_some());
    }

    #[test]
    fn inspects_csv_cells_and_quoted_newlines() {
        let dir = TempDir::new();
        let path = dir.0.join("a.csv");
        fs::write(&path, b"name,note\nA,\"x y\"\nB,\"line\r\nx y\"\n").unwrap();
        let inspect = inspect_file(
            &path,
            ReplaceMode::Plain,
            "x y",
            "z",
            CsvMode::Cell,
            None,
            64,
        )
        .unwrap();
        assert_eq!(inspect.structure, ContentStructure::Csv);
        assert_eq!(inspect.inspect.match_count, 2);
        assert_eq!(inspect.inspect.delimiter, Some(Delimiter::Comma));
        assert!(!inspect.inspect.unclosed_quote);
    }

    #[test]
    fn short_file_sample_is_truncated() {
        let dir = TempDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, b"abc").unwrap();
        let inspect = inspect_file(
            &path,
            ReplaceMode::Plain,
            "a",
            "b",
            CsvMode::Cell,
            None,
            4096,
        )
        .unwrap();
        assert_eq!(inspect.sample, b"abc");
        assert_eq!(inspect.file_size, 3);
    }

    /// 读盘侧的等价性：真文件、跨 64 KiB 块边界、多条臂，逐字段对账整段版 `inspect_bytes`。
    /// 内核那条等价性测试喂的是内存切片，这里补的是「分块读盘＋有界回读上下文」这半程。
    #[test]
    fn inspect_file_matches_whole_file_semantics_across_chunk_boundaries() {
        let chunk = crate::fs::STREAM_CHUNK_BYTES;
        let comma = Some(Delimiter::Comma);

        // 首条：匹配恰好跨块边界；第二条：多字节字符恰好跨块边界；第三条：单元格记录跨块。
        let mut straddle = vec![b'z'; chunk - 4];
        straddle.extend_from_slice(b"normal-row tail normal-row");
        let mut multibyte = vec![b'a'; chunk - 2];
        multibyte.extend_from_slice("😀 normal-row".as_bytes());
        let mut cell_rows = String::new();
        cell_rows.push_str("h1,h2\n");
        for index in 0..6_000 {
            cell_rows.push_str(&format!("{index},normal-row\n"));
        }
        // 八元组的显式标注读起来比推断更难，交给类型推断；结构在下面的构造里已写明。
        let cases = vec![
            (
                "straddle-boundary",
                straddle.clone(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
                None,
            ),
            (
                "multibyte-boundary",
                multibyte.clone(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
                None,
            ),
            (
                "cell-multi-chunk",
                cell_rows.as_bytes().to_vec(),
                ContentStructure::Csv,
                CsvMode::Cell,
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
                comma,
            ),
            (
                "cell-sniffed",
                cell_rows.as_bytes().to_vec(),
                ContentStructure::Csv,
                CsvMode::Cell,
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
                None,
            ),
            (
                "text-invalid-utf8",
                {
                    let mut d = b"head normal-row ".to_vec();
                    d.extend_from_slice(&[0xff, 0xfe]);
                    d
                },
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Plain,
                "normal-row",
                "X",
                None,
            ),
            (
                "text-first-match-at-zero",
                b"normal-row at the very start".to_vec(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Plain,
                "normal-row",
                "X",
                None,
            ),
            (
                "text-last-line-no-newline",
                b"a
normal-row"
                    .to_vec(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Plain,
                "normal-row",
                "X",
                None,
            ),
            (
                "regex-unbounded",
                b"aXXXXXXXXXXb aXXXXXXXXXXb".to_vec(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Regex,
                "a.*b",
                "X",
                None,
            ),
            (
                "regex-bounded-long",
                b"x 123456789 y 987654321 z".to_vec(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Regex,
                "[0-9]{9}",
                "0",
                None,
            ),
            (
                "empty-file",
                Vec::new(),
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Plain,
                "a",
                "b",
                None,
            ),
        ];
        for (name, data, structure, csv_mode, mode, find, replacement, delimiter) in cases {
            let dir = TempDir::new();
            let extension = match structure {
                ContentStructure::Csv => "csv",
                ContentStructure::Text => "txt",
            };
            let path = dir.0.join(format!("case.{extension}"));
            fs::write(&path, &data).unwrap();
            // 整段版是权威：内存里直接算，再与读盘版逐字段对账。
            let want = inspect_bytes(
                &data,
                mode,
                find,
                replacement,
                csv_mode,
                structure,
                delimiter,
            )
            .unwrap_or_else(|error| panic!("{name}: 整段基准失败 {error:?}"));
            let want_sample = data.iter().copied().take(8192).collect::<Vec<u8>>();
            let got = inspect_file(&path, mode, find, replacement, csv_mode, delimiter, 8192)
                .unwrap_or_else(|error| panic!("{name}: 读盘采集失败 {error:?}"));
            assert_eq!(got.sample, want_sample, "{name}: 编码样本");
            assert_eq!(got.file_size, data.len() as u64, "{name}: 文件体积");
            assert_eq!(got.inspect.structure, want.structure, "{name}: 结构");
            assert_eq!(got.inspect.delimiter, want.delimiter, "{name}: 分隔符");
            assert_eq!(got.inspect.match_count, want.match_count, "{name}: 命中数");
            assert_eq!(got.inspect.context, want.context, "{name}: 预览上下文");
            assert_eq!(
                got.inspect.unclosed_quote, want.unclosed_quote,
                "{name}: 未闭合引号"
            );
            assert_eq!(
                got.inspect.column_drift, want.column_drift,
                "{name}: 列数漂移"
            );
            assert_eq!(
                got.inspect.utf8_error, want.utf8_error,
                "{name}: 非法 UTF-8"
            );
        }
    }
}
