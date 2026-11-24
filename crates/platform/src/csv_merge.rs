//! F06 CSV 合并的输入检查（方案 §5.4）。
//!
//! 壳层职责：采样判定编码与分隔符，解码后读取表头/列数与行列漂移计数，
//! 交给 kernel 的纯函数计划层构建列并集。本切片按整文件解码后解析；
//! 大文件"第一遍只读首行"的零成本优化与第二遍流式写出待补（F06 台账缺口）。

use std::io;
use std::path::Path;

use kernel::csv::{sniff_delimiter, Delimiter};
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
    inspect_with_records(path, options).map(|(inspection, _)| inspection)
}

/// C11 CSV 去重的输入检查结果：F06 检查结果＋解码后的全部记录（方案 §5.15）。
pub struct CsvDedupInspection {
    pub inspection: CsvMergeInspection,
    /// 解码后的全部记录；有表头时首行为表头，编码未确认时为空。
    pub records: Vec<Vec<String>>,
}

pub fn csv_dedup_inspect(
    path: &Path,
    options: CsvMergeScanOptions,
) -> io::Result<CsvDedupInspection> {
    let (inspection, records) = inspect_with_records(path, options)?;
    Ok(CsvDedupInspection {
        inspection,
        records,
    })
}

fn inspect_with_records(
    path: &Path,
    options: CsvMergeScanOptions,
) -> io::Result<(CsvMergeInspection, Vec<Vec<String>>)> {
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
        return Ok((
            CsvMergeInspection {
                file_size: sampled.file_size,
                source_encoding: None,
                delimiter,
                delimiter_detected,
                has_header: options.has_header,
                columns: Vec::new(),
                column_count: 0,
                data_rows: 0,
                dirty_rows: 0,
            },
            Vec::new(),
        ));
    };

    let data = std::fs::read(fs_path(path))?;
    let decoded = transcode(
        &data,
        encoding,
        TargetEncoding::Utf8 { bom: false },
        InvalidSequencePolicy::ReplaceAndCount,
    )
    .map_err(|error| io::Error::other(format!("{}: {}", error.code, error.message)))?;
    // ADR-0102 A2-3：行数/列信息改由 kernel 的字节级入口算（`CsvInspection`），
    // 与 Web 形态共用同一份算法——避免两形态对同一文件算出不同列数。
    let mut inspection = kernel::csv_merge::CsvInspection {
        file_size: sampled.file_size,
        source_encoding: Some(encoding),
        delimiter,
        delimiter_detected,
        ..kernel::csv_merge::CsvInspection::default()
    };
    let records =
        kernel::csv_merge::complete_records(&mut inspection, &decoded.bytes, options.has_header);

    Ok((
        CsvMergeInspection {
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            delimiter_detected: inspection.delimiter_detected,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
        },
        records,
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// ADR-0102 A2-3 对拍：**桌面流式路径**（本模块）与 **kernel 字节级入口**
    /// （Web 形态用）必须对同一文件给出同一组事实。
    ///
    /// 这条断言存在的理由：两处若各自实现，同一 CSV 在桌面与 Web 可能算出不同的
    /// 列数／脏行数（方案 §13.5 明排除的双实现）。
    #[test]
    fn desktop_path_and_kernel_bytes_path_agree() {
        let samples: &[&[u8]] = &[
            b"name,age\nann,30\nbob,25\n",
            b"a,b\n1,2,3\n4\n",           // 脏行
            b"1,2,3\n4,5\n",              // 无表头
            b"\"quoted,field\",x\n1,2\n", // 引号内分隔符
            b"",                          // 空文件
        ];
        for data in samples {
            let dir = TempDir::new();
            let path = dir.0.join("sample.csv");
            fs::write(&path, data).unwrap();
            let options = CsvMergeScanOptions {
                source_encoding: Some(kernel::encoding::SourceEncoding::Utf8),
                has_header: true,
                delimiter: None,
                platform_prior: kernel::encoding::PlatformPrior::Other,
                sample_window_bytes: 4096,
                sample_blocks: 0,
            };
            let (desktop, _) = inspect_with_records(&path, options).unwrap();

            // 同一文件走 kernel 字节入口：读全文当整段字节。
            let mut kernel_side = kernel::csv_merge::inspect_head(
                data,
                data.len() as u64,
                options.platform_prior,
                options.delimiter,
            );
            let _ = kernel::csv_merge::complete_records(&mut kernel_side, data, options.has_header);

            assert_eq!(desktop.file_size, kernel_side.file_size, "fileSize 不一致");
            assert_eq!(
                desktop.source_encoding, kernel_side.source_encoding,
                "编码判定不一致"
            );
            assert_eq!(desktop.delimiter, kernel_side.delimiter, "分隔符不一致");
            assert_eq!(desktop.columns, kernel_side.columns, "表头列不一致");
            assert_eq!(desktop.column_count, kernel_side.column_count, "列数不一致");
            assert_eq!(desktop.data_rows, kernel_side.data_rows, "数据行数不一致");
            assert_eq!(desktop.dirty_rows, kernel_side.dirty_rows, "脏行数不一致");
        }
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            // Windows 的时钟分辨率较粗，并行测试可能取到同一时间戳；进程内序号 + PID
            // 保证路径唯一（同 config_store 修复）。
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "batchdesk-csv-merge-{}-{nanos}-{sequence}",
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
    fn dedup_inspect_returns_all_records_including_header() {
        let dir = TempDir::new();
        let path = dir.0.join("data.csv");
        fs::write(&path, b"id,name\n1,A\n1,A\n2,B\n").unwrap();
        let inspected = csv_dedup_inspect(&path, options()).unwrap();
        assert_eq!(inspected.inspection.data_rows, 3);
        assert_eq!(
            inspected.records,
            vec![
                vec!["id".to_string(), "name".to_string()],
                vec!["1".to_string(), "A".to_string()],
                vec!["1".to_string(), "A".to_string()],
                vec!["2".to_string(), "B".to_string()],
            ]
        );
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
