//! F06 CSV 合并的输入检查（方案 §5.4）。
//!
//! 壳层职责：采样判定编码与分隔符，解码后读取表头/列数与行列漂移计数，
//! 交给 kernel 的纯函数计划层构建列并集。本切片按整文件解码后解析；
//! 大文件"第一遍只读首行"的零成本优化与第二遍流式写出待补（F06 台账缺口）。

use std::io;
use std::path::Path;

use kernel::csv::{sniff_delimiter, Delimiter};
use kernel::csv_merge::parse_records;
use kernel::encoding::{
    detect, transcode, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding,
};

use crate::fs::fs_path;
use crate::text::{sample_bytes, SampleSpec};

/// F06 输入检查参数（采样窗口由调用方显式传入，不在此发明阈值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvMergeScanOptions {
    pub delimiter: Option<Delimiter>,
    pub has_header: bool,
    pub sample_window_bytes: u64,
    pub sample_blocks: usize,
    pub platform_prior: PlatformPrior,
    /// 调用方显式指定的源编码；`Some` 时跳过检测（仍用采样窗口嗅探分隔符）。
    pub source_encoding: Option<SourceEncoding>,
}

/// 单个输入文件的表头/列宽/漂移信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvMergeInspection {
    pub file_size: u64,
    /// 检测到的源编码；`None` 表示需人工确认（计划层按 E3001 阻断）。
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: Delimiter,
    pub delimiter_detected: bool,
    pub has_header: bool,
    /// 原始表头字段（未规范化；kernel 计划层再剥离 BOM/空白）。
    pub columns: Vec<String>,
    /// 列数（有表头时取表头字段数；无表头时取各数据行的最大字段数）。
    pub column_count: u64,
    pub data_rows: u64,
    /// 字段数与列宽不一致的数据行计数（只上报，不静默修正）。
    pub dirty_rows: u64,
}

pub fn csv_merge_inspect(
    path: &Path,
    options: CsvMergeScanOptions,
) -> io::Result<CsvMergeInspection> {
    let sampled = sample_bytes(
        path,
        SampleSpec {
            window_bytes: options.sample_window_bytes,
            blocks: options.sample_blocks,
        },
    )?;
    let source_encoding = match options.source_encoding {
        Some(encoding) => Some(encoding),
        None => {
            let detection = detect(&sampled.bytes, options.platform_prior);
            detection.require_encoding().ok()
        }
    };

    let sniffed = sniff_delimiter(&sampled.bytes);
    let (delimiter, delimiter_detected) = match options.delimiter {
        Some(delimiter) => (delimiter, true),
        None => (sniffed.unwrap_or(Delimiter::Comma), sniffed.is_some()),
    };

    let Some(encoding) = source_encoding else {
        return Ok(CsvMergeInspection {
            file_size: sampled.file_size,
            source_encoding: None,
            delimiter,
            delimiter_detected,
            has_header: options.has_header,
            columns: Vec::new(),
            column_count: 0,
            data_rows: 0,
            dirty_rows: 0,
        });
    };

    let data = std::fs::read(fs_path(path))?;
    let decoded = transcode(
        &data,
        encoding,
        TargetEncoding::Utf8 { bom: false },
        InvalidSequencePolicy::ReplaceAndCount,
    )
    .map_err(|error| io::Error::other(format!("{}: {}", error.code, error.message)))?;
    let records = parse_records(&decoded.bytes, delimiter.as_byte());

    let (columns, rows): (Vec<String>, &[Vec<String>]) = if options.has_header {
        match records.split_first() {
            Some((header, rows)) => (header.clone(), rows),
            None => (Vec::new(), &[]),
        }
    } else {
        (Vec::new(), &records)
    };
    let column_count = if options.has_header {
        columns.len() as u64
    } else {
        records
            .iter()
            .map(|row| row.len() as u64)
            .max()
            .unwrap_or(0)
    };
    let dirty_rows = rows
        .iter()
        .filter(|row| row.len() as u64 != column_count)
        .count() as u64;

    Ok(CsvMergeInspection {
        file_size: sampled.file_size,
        source_encoding: Some(encoding),
        delimiter,
        delimiter_detected,
        has_header: options.has_header,
        columns,
        column_count,
        data_rows: rows.len() as u64,
        dirty_rows,
    })
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
            let path = std::env::temp_dir().join(format!("batchdesk-csv-merge-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn options() -> CsvMergeScanOptions {
        CsvMergeScanOptions {
            delimiter: None,
            has_header: true,
            sample_window_bytes: 4096,
            sample_blocks: 0,
            platform_prior: PlatformPrior::Other,
            source_encoding: None,
        }
    }

    #[test]
    fn inspects_header_rows_and_drift() {
        let dir = TempDir::new();
        let path = dir.0.join("data.csv");
        fs::write(&path, b"id,name\n1,Alice\n2\n").unwrap();
        let inspection = csv_merge_inspect(&path, options()).unwrap();
        assert_eq!(inspection.file_size, 18);
        assert_eq!(inspection.columns, vec!["id", "name"]);
        assert_eq!(inspection.column_count, 2);
        assert_eq!(inspection.data_rows, 2);
        assert_eq!(inspection.dirty_rows, 1);
        assert_eq!(inspection.delimiter, Delimiter::Comma);
        assert!(inspection.delimiter_detected);
    }

    #[test]
    fn no_header_uses_the_max_column_count() {
        let dir = TempDir::new();
        let path = dir.0.join("data.csv");
        fs::write(&path, b"1,2\na,b,c\n").unwrap();
        let inspection = csv_merge_inspect(
            &path,
            CsvMergeScanOptions {
                has_header: false,
                ..options()
            },
        )
        .unwrap();
        assert!(inspection.columns.is_empty());
        assert_eq!(inspection.column_count, 3);
        assert_eq!(inspection.data_rows, 2);
        assert_eq!(inspection.dirty_rows, 1);
    }

    #[test]
    fn sniffs_semicolon_and_reads_quoted_fields() {
        let dir = TempDir::new();
        let path = dir.0.join("semi.csv");
        fs::write(&path, b"id;name\n1;\"a; b\"\n").unwrap();
        let inspection = csv_merge_inspect(&path, options()).unwrap();
        assert_eq!(inspection.delimiter, Delimiter::Semicolon);
        assert_eq!(inspection.columns, vec!["id", "name"]);
        assert_eq!(inspection.dirty_rows, 0);
    }

    #[test]
    fn utf16_source_is_decoded_for_inspection() {
        let dir = TempDir::new();
        let path = dir.0.join("utf16.csv");
        let mut bytes = vec![0xFF, 0xFE];
        for character in "id,name\n1,2\n".chars() {
            let code = character as u32;
            bytes.push((code & 0xFF) as u8);
            bytes.push((code >> 8) as u8);
        }
        fs::write(&path, &bytes).unwrap();
        let inspection = csv_merge_inspect(&path, options()).unwrap();
        assert_eq!(inspection.source_encoding, Some(SourceEncoding::Utf16Le));
        assert_eq!(inspection.columns, vec!["id", "name"]);
        assert_eq!(inspection.data_rows, 1);
    }

    #[test]
    fn undetermined_encoding_is_reported_as_none() {
        let dir = TempDir::new();
        let path = dir.0.join("bad.csv");
        fs::write(&path, [0xFF, 0x00, 0xFE, 0x00]).unwrap();
        let inspection = csv_merge_inspect(&path, options()).unwrap();
        assert_eq!(inspection.source_encoding, None);
        assert!(inspection.columns.is_empty());
    }
}
