//! F05 CSV 分片布局扫描（方案 §5.3）。
//!
//! 壳层职责：采样头部窗口做分隔符嗅探与编码判定，随后**流式**扫描全文件驱动
//! kernel 的纯记录扫描器与分片累加器。本模块不缓存文件内容，内存 O(分片数)。
//!
//! UTF-16 源（含 BOM 或无 BOM 但被判定为 UTF-16）无法按字节解析，直接产出
//! 一条覆盖全文件的占位布局，由 kernel 计划层阻断（E3001，提示先走 C01 转码）。

use std::io;
use std::path::Path;

use kernel::csv::{
    sniff_delimiter, CsvRecordScanner, CsvShard, CsvSplitAccumulator, CsvSplitBasis,
    CsvSplitLayout, Delimiter,
};
use kernel::encoding::{detect, PlatformPrior, SourceEncoding};

use crate::text::{sample_bytes, stream_segments, SampleSpec, StreamFlow};

/// CSV 扫描参数（采样窗口由调用方显式传入，不在此发明阈值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvScanOptions {
    /// 显式分隔符；`None` 表示嗅探（失败时按逗号兜底并标记未检测到）。
    pub delimiter: Option<Delimiter>,
    /// 是否每片保留表头。
    pub keep_header: bool,
    /// 分片计量基准。
    pub basis: CsvSplitBasis,
    /// 头部嗅探/编码判定的采样窗口字节数（调用方显式传入，> 0）。
    pub sample_window_bytes: u64,
}

/// 扫描 CSV，产出分片布局（纯消费 kernel 类型）。
pub fn csv_split_layout(path: &Path, options: CsvScanOptions) -> io::Result<CsvSplitLayout> {
    let sampled = sample_bytes(
        path,
        SampleSpec {
            window_bytes: options.sample_window_bytes,
            blocks: 0,
        },
    )?;
    let detection = detect(&sampled.bytes, PlatformPrior::Other);
    let source_encoding = detection.encoding;

    let sniffed = sniff_delimiter(&sampled.bytes);
    let (delimiter, delimiter_detected) = match options.delimiter {
        Some(delimiter) => (delimiter, true),
        None => (sniffed.unwrap_or(Delimiter::Comma), sniffed.is_some()),
    };

    // UTF-16 无法按字节识别记录边界：不扫描、不猜测，产出占位布局供计划层阻断。
    if matches!(
        source_encoding,
        Some(SourceEncoding::Utf16Le | SourceEncoding::Utf16Be)
    ) {
        let file_size = sampled.file_size;
        let shards = if file_size > 0 {
            vec![CsvShard {
                index: 1,
                start: 0,
                end: file_size,
                rows: 0,
                dirty_rows: 0,
            }]
        } else {
            Vec::new()
        };
        return Ok(CsvSplitLayout {
            file_size,
            delimiter,
            delimiter_detected,
            header: None,
            header_columns: 0,
            shards,
            records: 0,
            dirty_rows: 0,
            multi_line_header: false,
            unclosed_quote: false,
            source_encoding,
        });
    }

    let mut accumulator = match options.basis {
        CsvSplitBasis::Bytes(bytes) => CsvSplitAccumulator::new_bytes(bytes, options.keep_header),
        CsvSplitBasis::Rows(rows) => CsvSplitAccumulator::new_rows(rows, options.keep_header),
    }
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.message))?;

    let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
    let summary = stream_segments(path, None, |segment| {
        scanner.feed(segment.bytes, |record| accumulator.push_record(record));
        StreamFlow::Continue
    })?;
    let tail = scanner.finish();
    if let Some(record) = tail.tail {
        accumulator.push_record(record);
    }

    Ok(accumulator.finish(
        summary.bytes_read,
        delimiter,
        delimiter_detected,
        tail.unclosed_quote,
        source_encoding,
    ))
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
            let path = std::env::temp_dir().join(format!("batchdesk-csv-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn options(basis: CsvSplitBasis) -> CsvScanOptions {
        CsvScanOptions {
            delimiter: None,
            keep_header: true,
            basis,
            sample_window_bytes: 4096,
        }
    }

    #[test]
    fn scans_records_and_repeats_header() {
        let dir = TempDir::new();
        let path = dir.0.join("data.csv");
        fs::write(&path, b"h1,h2\n1,2\n3,4\n5,6\n").unwrap();
        let layout = csv_split_layout(&path, options(CsvSplitBasis::Rows(2))).unwrap();
        assert_eq!(layout.file_size, 18);
        assert_eq!(layout.header.unwrap().end, 6);
        assert_eq!(layout.shards.len(), 2);
        assert_eq!(layout.shards[0].start, 6);
        assert_eq!(layout.shards[0].rows, 2);
        assert_eq!(layout.shards[1].rows, 1);
        assert_eq!(layout.delimiter, Delimiter::Comma);
    }

    #[test]
    fn records_do_not_break_on_quoted_newlines() {
        let dir = TempDir::new();
        let path = dir.0.join("quoted.csv");
        fs::write(&path, b"h1,h2\n\"x\ny\",z\n1,2\n").unwrap();
        let layout = csv_split_layout(&path, options(CsvSplitBasis::Rows(1))).unwrap();
        assert_eq!(layout.shards.len(), 2);
        assert_eq!(layout.shards[0].rows, 1);
        assert_eq!(layout.shards[0].end, 14);
    }

    #[test]
    fn sniffs_semicolon_delimiter() {
        let dir = TempDir::new();
        let path = dir.0.join("semi.csv");
        fs::write(&path, b"a;b;c\n1;2;3\n").unwrap();
        let layout = csv_split_layout(&path, options(CsvSplitBasis::Rows(1))).unwrap();
        assert_eq!(layout.delimiter, Delimiter::Semicolon);
        assert!(layout.delimiter_detected);
    }

    #[test]
    fn utf16_source_is_short_circuited_for_blocking() {
        let dir = TempDir::new();
        let path = dir.0.join("utf16.csv");
        fs::write(&path, [0xFF, 0xFE, b'h', 0, b'\n', 0]).unwrap();
        let layout = csv_split_layout(&path, options(CsvSplitBasis::Rows(1))).unwrap();
        assert_eq!(layout.source_encoding, Some(SourceEncoding::Utf16Le));
        assert_eq!(layout.shards.len(), 1);
        assert_eq!((layout.shards[0].start, layout.shards[0].end), (0, 6));
        assert_eq!(layout.records, 0);
    }

    #[test]
    fn empty_file_has_no_shards() {
        let dir = TempDir::new();
        let path = dir.0.join("empty.csv");
        fs::write(&path, b"").unwrap();
        let layout = csv_split_layout(&path, options(CsvSplitBasis::Rows(1))).unwrap();
        assert!(layout.shards.is_empty());
        assert_eq!(layout.file_size, 0);
    }

    #[test]
    fn explicit_delimiter_overrides_sniffing() {
        let dir = TempDir::new();
        let path = dir.0.join("pipe.csv");
        fs::write(&path, b"a|b\n1|2\n").unwrap();
        let mut scan = options(CsvSplitBasis::Rows(1));
        scan.delimiter = Some(Delimiter::Comma);
        let layout = csv_split_layout(&path, scan).unwrap();
        assert_eq!(layout.delimiter, Delimiter::Comma);
        assert!(layout.delimiter_detected);
        assert_eq!(layout.header_columns, 1);
        assert_eq!(layout.dirty_rows, 0);
    }
}
