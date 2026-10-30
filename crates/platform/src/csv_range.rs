//! F09 CSV 位置行区间的流式扫描（方案 §5.15）。
//!
//! 壳层职责：采样头部窗口做分隔符嗅探与编码判定，随后按记录流式扫描源文件。
//! 扫描到第 Y 条数据记录即停止；只有 Y（或 X）超出实际行数时才继续到 EOF，
//! 用于给出越界告警。返回表头与选中记录的字节区间，供 apply 只读这些区间。
//!
//! UTF-16 无法按字节识别记录边界，返回占位布局由 kernel 计划层阻断（E3001）。

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use kernel::csv::{sniff_delimiter, CsvRecord, CsvRecordScanner, Delimiter};
use kernel::csv_merge::parse_records;
use kernel::encoding::{
    detect, transcode, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding,
};

use crate::fs::fs_path;
use crate::text::{sample_bytes, stream_segments, SampleSpec, StreamFlow};

/// 字节区间（左闭右开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvRangeSpan {
    pub start: u64,
    pub end: u64,
}

impl CsvRangeSpan {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 位置行扫描参数（采样窗口与平台先验由调用方显式传入）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvRangeScanOptions {
    pub delimiter: Option<Delimiter>,
    pub has_header: bool,
    /// 1-based 数据记录起点（含）。
    pub start_row: u64,
    /// 1-based 数据记录终点（含）。
    pub end_row: u64,
    pub sample_window_bytes: u64,
    pub platform_prior: PlatformPrior,
    pub source_encoding: Option<SourceEncoding>,
}

/// 位置行扫描结果；`selected` 为选中数据记录的字节区间。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvRangeLayout {
    pub file_size: u64,
    pub delimiter: Delimiter,
    pub delimiter_detected: bool,
    pub source_encoding: Option<SourceEncoding>,
    pub header: Option<CsvRangeSpan>,
    /// 表头原始字段（壳层解码后的文本；无表头时为空）。
    pub header_columns: Vec<String>,
    pub column_count: u64,
    /// 观察到的数据记录数；`total_known=false` 时等于提前终止的 endRow。
    pub data_rows: u64,
    pub total_known: bool,
    pub selected: Option<CsvRangeSpan>,
    pub selected_rows: u64,
    /// 选中区间内字段数与表头/首条选中记录不一致的记录数。
    pub dirty_rows: u64,
    pub multi_line_header: bool,
    pub unclosed_quote: bool,
}

pub fn csv_range_layout(path: &Path, options: CsvRangeScanOptions) -> io::Result<CsvRangeLayout> {
    if options.start_row == 0 || options.end_row < options.start_row {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "startRow and endRow must be 1-based with endRow >= startRow",
        ));
    }
    let sampled = sample_bytes(
        path,
        SampleSpec {
            window_bytes: options.sample_window_bytes,
            blocks: 0,
        },
    )?;
    let source_encoding = match options.source_encoding {
        Some(encoding) => Some(encoding),
        None => detect(&sampled.bytes, options.platform_prior)
            .require_encoding()
            .ok(),
    };
    let sniffed = sniff_delimiter(&sampled.bytes);
    let (delimiter, delimiter_detected) = match options.delimiter {
        Some(delimiter) => (delimiter, true),
        None => (sniffed.unwrap_or(Delimiter::Comma), sniffed.is_some()),
    };

    if !matches!(
        source_encoding,
        Some(
            SourceEncoding::Utf8
                | SourceEncoding::Gb18030
                | SourceEncoding::Big5
                | SourceEncoding::ShiftJis
                | SourceEncoding::Latin1
        )
    ) {
        return Ok(placeholder(
            sampled.file_size,
            delimiter,
            delimiter_detected,
            source_encoding,
        ));
    }
    let encoding = source_encoding.expect("checked above");

    let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
    let mut state = RangeState::default();
    stream_segments(path, None, |segment| {
        if state.done {
            return StreamFlow::Stop;
        }
        scanner.feed(segment.bytes, |record| {
            state.push(
                record,
                options.has_header,
                options.start_row,
                options.end_row,
            );
        });
        if state.done {
            StreamFlow::Stop
        } else {
            StreamFlow::Continue
        }
    })?;
    let stopped_early = state.done;
    let tail = scanner.finish();
    if !stopped_early {
        if let Some(record) = tail.tail {
            state.push(
                record,
                options.has_header,
                options.start_row,
                options.end_row,
            );
        }
    }

    let header_columns = match state.header {
        Some(span) => decode_header(path, span, encoding, delimiter)?,
        None => Vec::new(),
    };
    let column_count = if state.header.is_some() {
        header_columns.len() as u64
    } else {
        state.max_selected_columns
    };
    let selected = (state.selected_rows > 0).then(|| CsvRangeSpan {
        start: state.selected_start.expect("selected rows have a start"),
        end: state.selected_end.expect("selected rows have an end"),
    });

    Ok(CsvRangeLayout {
        file_size: sampled.file_size,
        delimiter,
        delimiter_detected,
        source_encoding,
        header: state.header,
        header_columns,
        column_count,
        data_rows: state.data_rows,
        total_known: !stopped_early,
        selected,
        selected_rows: state.selected_rows,
        dirty_rows: state.dirty_rows,
        multi_line_header: state.multi_line_header,
        unclosed_quote: if stopped_early {
            false
        } else {
            tail.unclosed_quote
        },
    })
}

#[derive(Default)]
struct RangeState {
    consumed: u64,
    record_index: u64,
    header: Option<CsvRangeSpan>,
    header_columns: u64,
    multi_line_header: bool,
    data_rows: u64,
    selected_start: Option<u64>,
    selected_end: Option<u64>,
    selected_rows: u64,
    dirty_rows: u64,
    first_selected_columns: Option<u64>,
    max_selected_columns: u64,
    done: bool,
}

impl RangeState {
    fn push(&mut self, record: CsvRecord, has_header: bool, start_row: u64, end_row: u64) {
        if self.done {
            return;
        }
        let start = self.consumed;
        let end = start.saturating_add(record.len);
        self.consumed = end;

        if has_header && self.record_index == 0 {
            self.header = Some(CsvRangeSpan { start: 0, end });
            self.header_columns = record.columns;
            self.multi_line_header = record.multiline;
            self.record_index = 1;
            return;
        }

        self.record_index += 1;
        self.data_rows += 1;
        if self.data_rows >= start_row && self.data_rows <= end_row {
            if self.selected_start.is_none() {
                self.selected_start = Some(start);
            }
            self.selected_end = Some(end);
            self.selected_rows += 1;
            if has_header {
                if record.columns != self.header_columns {
                    self.dirty_rows += 1;
                }
            } else {
                match self.first_selected_columns {
                    None => {
                        self.first_selected_columns = Some(record.columns);
                        self.max_selected_columns = record.columns;
                    }
                    Some(columns) => {
                        self.max_selected_columns = self.max_selected_columns.max(record.columns);
                        if columns != record.columns {
                            self.dirty_rows += 1;
                        }
                    }
                }
            }
            if self.data_rows == end_row {
                self.done = true;
            }
        }
    }
}

fn placeholder(
    file_size: u64,
    delimiter: Delimiter,
    delimiter_detected: bool,
    source_encoding: Option<SourceEncoding>,
) -> CsvRangeLayout {
    CsvRangeLayout {
        file_size,
        delimiter,
        delimiter_detected,
        source_encoding,
        header: None,
        header_columns: Vec::new(),
        column_count: 0,
        data_rows: 0,
        total_known: true,
        selected: None,
        selected_rows: 0,
        dirty_rows: 0,
        multi_line_header: false,
        unclosed_quote: false,
    }
}

fn decode_header(
    path: &Path,
    span: CsvRangeSpan,
    encoding: SourceEncoding,
    delimiter: Delimiter,
) -> io::Result<Vec<String>> {
    let raw = read_span(path, span)?;
    let decoded = transcode(
        &raw,
        encoding,
        TargetEncoding::Utf8 { bom: false },
        InvalidSequencePolicy::ReplaceAndCount,
    )
    .map_err(|error| io::Error::other(format!("{}: {}", error.code, error.message)))?;
    Ok(parse_records(&decoded.bytes, delimiter.as_byte())
        .first()
        .cloned()
        .unwrap_or_default())
}

fn read_span(path: &Path, span: CsvRangeSpan) -> io::Result<Vec<u8>> {
    if span.is_empty() {
        return Ok(Vec::new());
    }
    let mut file = File::open(fs_path(path))?;
    file.seek(SeekFrom::Start(span.start))?;
    let mut buffer = vec![0u8; span.len() as usize];
    file.read_exact(&mut buffer)?;
    Ok(buffer)
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
            let path = std::env::temp_dir().join(format!("batchdesk-csv-range-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn options(start_row: u64, end_row: u64) -> CsvRangeScanOptions {
        CsvRangeScanOptions {
            delimiter: None,
            has_header: true,
            start_row,
            end_row,
            sample_window_bytes: 4096,
            platform_prior: PlatformPrior::Other,
            source_encoding: None,
        }
    }

    #[test]
    fn scans_header_and_selected_byte_range_and_stops_at_end_row() {
        let dir = TempDir::new();
        let path = dir.0.join("data.csv");
        let data = b"h1,h2\r\n1,2\r\n3,4\r\n5,6\r\n";
        fs::write(&path, data).unwrap();
        let layout = csv_range_layout(&path, options(2, 3)).unwrap();
        assert_eq!(layout.header_columns, vec!["h1", "h2"]);
        assert_eq!(layout.selected_rows, 2);
        assert_eq!(layout.data_rows, 3);
        assert!(!layout.total_known);
        let selected = layout.selected.unwrap();
        assert_eq!(
            &data[selected.start as usize..selected.end as usize],
            b"3,4\r\n5,6\r\n"
        );
    }

    #[test]
    fn quoted_newlines_stay_inside_one_record() {
        let dir = TempDir::new();
        let path = dir.0.join("quoted.csv");
        let data = b"h\n\"x\ny\"\n1\n2\n";
        fs::write(&path, data).unwrap();
        let layout = csv_range_layout(&path, options(1, 1)).unwrap();
        assert_eq!(layout.selected_rows, 1);
        let selected = layout.selected.unwrap();
        assert_eq!(
            &data[selected.start as usize..selected.end as usize],
            b"\"x\ny\"\n"
        );
    }

    #[test]
    fn range_beyond_rows_reads_to_eof_and_clamps_selection() {
        let dir = TempDir::new();
        let path = dir.0.join("short.csv");
        let data = b"h\n1\n2\n3\n";
        fs::write(&path, data).unwrap();
        let layout = csv_range_layout(&path, options(2, 9)).unwrap();
        assert!(layout.total_known);
        assert_eq!(layout.data_rows, 3);
        assert_eq!(layout.selected_rows, 2);
        let selected = layout.selected.unwrap();
        assert_eq!(
            &data[selected.start as usize..selected.end as usize],
            b"2\n3\n"
        );
    }

    #[test]
    fn start_beyond_rows_selects_nothing() {
        let dir = TempDir::new();
        let path = dir.0.join("short.csv");
        fs::write(&path, b"h\n1\n2\n").unwrap();
        let layout = csv_range_layout(&path, options(9, 10)).unwrap();
        assert!(layout.total_known);
        assert_eq!(layout.data_rows, 2);
        assert_eq!(layout.selected_rows, 0);
        assert!(layout.selected.is_none());
    }

    #[test]
    fn headerless_scan_starts_at_the_first_data_record() {
        let dir = TempDir::new();
        let path = dir.0.join("headerless.csv");
        let data = b"1,2\n3,4\n";
        fs::write(&path, data).unwrap();
        let mut scan = options(1, 1);
        scan.has_header = false;
        let layout = csv_range_layout(&path, scan).unwrap();
        assert!(layout.header.is_none());
        assert_eq!(layout.selected_rows, 1);
        let selected = layout.selected.unwrap();
        assert_eq!(
            &data[selected.start as usize..selected.end as usize],
            b"1,2\n"
        );
    }

    #[test]
    fn utf16_source_is_short_circuited_for_blocking() {
        let dir = TempDir::new();
        let path = dir.0.join("utf16.csv");
        fs::write(&path, [0xFF, 0xFE, b'h', 0, b'\n', 0]).unwrap();
        let layout = csv_range_layout(&path, options(1, 1)).unwrap();
        assert_eq!(layout.source_encoding, Some(SourceEncoding::Utf16Le));
        assert!(layout.selected.is_none());
        assert_eq!(layout.data_rows, 0);
    }

    #[test]
    fn t02_corpus_quoted_newline_is_one_selected_record() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/T02/out/T02.csv");
        let data = fs::read(&path).unwrap();
        let layout = csv_range_layout(&path, options(3, 3)).unwrap();
        assert_eq!(layout.selected_rows, 1);
        let selected = layout.selected.unwrap();
        let slice = &data[selected.start as usize..selected.end as usize];
        let records = parse_records(slice, b',');
        assert_eq!(records.len(), 1, "quoted newlines must not split records");
        assert!(records[0]
            .iter()
            .any(|field| field.contains('\n') || field.contains('\r')));
    }

    #[test]
    fn invalid_range_is_rejected() {
        let dir = TempDir::new();
        let path = dir.0.join("data.csv");
        fs::write(&path, b"h\n1\n").unwrap();
        assert!(csv_range_layout(&path, options(0, 1)).is_err());
        assert!(csv_range_layout(&path, options(2, 1)).is_err());
    }
}
