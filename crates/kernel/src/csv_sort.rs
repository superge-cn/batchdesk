//! 纯函数层：C13 CSV 排序计划与执行（方案 §5.15"小文件内存排序；大文件外排
//! （分块排序＋多路归并）"、§4.12"经典外排归并实现"）。
//!
//! 口径（ADR-0038）：
//! - 排序键：表头名或 1-based 序号（与 F07 同规解析，E3004）；可多个键（声明
//!   顺序即优先级）；`--order asc|desc` 作用于全部键；
//! - **类型感知**：计划期按采样值推断键类型——全部可解析为数值→Numeric、
//!   全部匹配 `yyyy-mm-dd`→Date、否则 Lexicographic；执行期单行解析失败时
//!   **回退字典序**（不报错、不丢行，dirty 语义同源）；
//! - 外排触发阈值（数据行数）由调用方显式传入（OQ-28 同口径：不设产品默认
//!   值）；超阈值走"分块排序＋多路归并"（§4.12）：排序键与行号绑定后分块
//!   排序写临时文件（目标目录内 `.sort-chunk-N.tmp`），归并时多路取最小，
//!   临时文件归并后尽力删除——临时文件为实现细节，不记 Journal；
//! - 表头保留原位（不参与排序）；CRLF/引号经 RFC 4180 管道（与 F05 同源）；
//!   排序稳定（键相等保持原行序）；
//! - 本模块不读盘、不取时钟（不变量 1）；文件事实由壳层采集。

use std::cmp::Ordering;

use crate::csv::Delimiter;
use crate::csv_extract::ColumnPick;
use crate::csv_merge::{normalized_columns, source_block, target_block};
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::journal::FsOp;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvSort, PlanCsvSortKey, PlanDocument,
    PlanEntry, PlanExpected, PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const C13_KIND: &str = "c13";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "asc" => Some(Self::Asc),
            "desc" => Some(Self::Desc),
            _ => None,
        }
    }
}

/// 键类型（类型感知；执行期解析失败的行回退字典序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKeyKind {
    Numeric,
    Date,
    Lexicographic,
}

impl SortKeyKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Numeric => "numeric",
            Self::Date => "date",
            Self::Lexicographic => "lexicographic",
        }
    }
}

/// C13 的单个输入文件；键列采样值由壳层解析后提供（类型感知输入）。
#[derive(Clone)]
pub struct PlanC13File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: Delimiter,
    pub has_header: bool,
    pub columns: Vec<String>,
    pub column_count: u64,
    pub data_rows: u64,
    pub dirty_rows: u64,
    /// 排序键列在全部数据行中的取值（壳层解析；类型感知输入）。
    pub key_values: Vec<String>,
    pub target: String,
    pub target_occupied: bool,
}

#[derive(Clone)]
pub struct PlanC13Input {
    pub plan_id: String,
    pub keys: Vec<ColumnPick>,
    pub order: SortOrder,
    pub external_chunk_rows: Option<u64>,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanC13File>,
}

/// 类型感知：采样值全部可解析数值→Numeric；全部为 10 位 ISO 日期→Date；
/// 否则 Lexicographic。空采样值不参与判定（视为缺失）。
pub fn infer_key_kind(values: &[String]) -> SortKeyKind {
    let non_empty: Vec<&String> = values.iter().filter(|value| !value.is_empty()).collect();
    if non_empty.is_empty() {
        return SortKeyKind::Lexicographic;
    }
    if non_empty.iter().all(|value| value.parse::<f64>().is_ok()) {
        return SortKeyKind::Numeric;
    }
    if non_empty.iter().all(|value| is_iso_date(value)) {
        return SortKeyKind::Date;
    }
    SortKeyKind::Lexicographic
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// 生成 C13 计划：每个输入文件一条 `csv-sort` 条目（1:1）。
pub fn plan_c13(input: PlanC13Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param(
            "sort plans require at least one file",
        ));
    }
    if input.keys.is_empty() {
        return Err(KernelError::missing_param(
            "sort plans require at least one sort key",
        ));
    }
    let has_header = input.files[0].has_header;
    if input.files.iter().any(|file| file.has_header != has_header) {
        return Err(KernelError::invalid_params(
            "inputs mix files with and without a header row",
        ));
    }
    crate::csv_extract::ensure_unique_targets(
        input.files.iter().map(|file| file.target.as_str()),
        input.case_sense,
    )?;

    let mut warnings = Vec::new();
    let mut dirty_files = Vec::new();
    let mut entries = Vec::with_capacity(input.files.len());

    for file in &input.files {
        let normalized = normalized_columns(&file.columns);
        let mut blocked = target_block(&file.target, &input.protected_dirs, input.case_sense);
        if blocked.is_none() {
            blocked = source_block(
                &file.path,
                file.canonical_path.as_deref(),
                &input.protected_dirs,
                input.case_sense,
            );
        }
        if blocked.is_none() && file.source_encoding.is_none() {
            blocked = Some(PlanBlocked {
                code: "E3001".to_string(),
                reason: "source encoding requires manual confirmation".to_string(),
                hint: Some(
                    "confirm the source encoding (or pass a global --source override)".to_string(),
                ),
            });
        }

        let mut keys = Vec::with_capacity(input.keys.len());
        if blocked.is_none() {
            for pick in &input.keys {
                let index = match pick {
                    ColumnPick::Name(name) => {
                        if !has_header {
                            blocked = Some(PlanBlocked {
                                code: "E3004".to_string(),
                                reason: "name-based sort keys require a header row".to_string(),
                                hint: Some(
                                    "use --sort-column-index for headerless files".to_string(),
                                ),
                            });
                            break;
                        }
                        let wanted = crate::csv_merge::normalize_header_name(name);
                        match normalized
                            .iter()
                            .position(|column| *column == wanted)
                            .map(|position| position as u64 + 1)
                        {
                            Some(index) => index,
                            None => {
                                blocked = Some(PlanBlocked {
                                    code: "E3004".to_string(),
                                    reason: format!("sort key column not found: {name}"),
                                    hint: None,
                                });
                                break;
                            }
                        }
                    }
                    ColumnPick::Index(index) => {
                        if *index == 0 || *index > file.column_count {
                            blocked = Some(PlanBlocked {
                                code: "E3004".to_string(),
                                reason: format!("sort key column out of range: {index}"),
                                hint: None,
                            });
                            break;
                        }
                        *index
                    }
                };
                keys.push(PlanCsvSortKey {
                    index,
                    name: normalized.get(index as usize - 1).cloned(),
                    order: input.order.as_str().to_string(),
                    kind: infer_key_kind(&file.key_values).as_str().to_string(),
                });
            }
        }
        if file.dirty_rows > 0 {
            dirty_files.push(format!("{} ({} row(s))", file.path, file.dirty_rows));
        }

        let conflict = file.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        let external = match input.external_chunk_rows {
            Some(chunk_rows) if file.data_rows > chunk_rows => {
                warnings.push(PlanWarning {
                    kind: "external-sort".to_string(),
                    suggestion: Some(format!(
                        "{} data rows exceed the chunk threshold ({chunk_rows}); \
                         external merge sort will use temporary chunk files",
                        file.data_rows
                    )),
                    risk_level: RiskLevel::Caution,
                });
                Some(chunk_rows)
            }
            _ => None,
        };
        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: Some(file.target.clone()),
            action: PlanAction::CsvSort,
            relation: None,
            reason: format!(
                "sort {} by {} ({}) into {} as {}",
                file.path,
                keys.iter()
                    .map(|key| key.name.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(","),
                input.order.as_str(),
                crate::conflict::file_name_of(&file.target),
                input.target_encoding.as_str()
            ),
            conflict: conflict.clone(),
            resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
            blocked: blocked.clone(),
            expected: Some(PlanExpected {
                size: file.file_size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: None,
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: Some(PlanCsvSort {
                encoding: file.source_encoding,
                size: file.file_size,
                delimiter: file.delimiter.as_str().to_string(),
                has_header,
                columns: if has_header {
                    normalized.clone()
                } else {
                    Vec::new()
                },
                column_count: file.column_count,
                keys,
                external_chunk_rows: external,
                target_encoding: input.target_encoding.as_str().to_string(),
                invalid: input.invalid.as_str().to_string(),
                dirty_rows: file.dirty_rows,
            }),
        });
    }

    if !dirty_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "csv-row-width-mismatch".to_string(),
            suggestion: Some(format!(
                "records with a different field count (missing key fields sort as empty): {}",
                dirty_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "order".to_string(),
        PlanParamValue::String(input.order.as_str().to_string()),
    );
    params.insert(
        "targetEncoding".to_string(),
        PlanParamValue::String(input.target_encoding.as_str().to_string()),
    );
    params.insert(
        "invalid".to_string(),
        PlanParamValue::String(input.invalid.as_str().to_string()),
    );
    params.insert("hasHeader".to_string(), PlanParamValue::Bool(has_header));
    params.insert(
        "fileCount".to_string(),
        PlanParamValue::Number(input.files.len() as u64),
    );

    let stats = PlanStats {
        total: entries.len(),
        conflict: entries
            .iter()
            .filter(|entry| entry.conflict.is_some())
            .count(),
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C13_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn compare_by_keys(
    left_row: &[String],
    left: usize,
    right_row: &[String],
    right: usize,
    keys: &[PlanCsvSortKey],
) -> Ordering {
    let left = (left_row, left);
    let right = (right_row, right);
    for key in keys {
        let left_value = left
            .0
            .get(key.index as usize - 1)
            .cloned()
            .unwrap_or_default();
        let right_value = right
            .0
            .get(key.index as usize - 1)
            .cloned()
            .unwrap_or_default();
        let ordering = match key.kind.as_str() {
            "numeric" => match (left_value.parse::<f64>(), right_value.parse::<f64>()) {
                (Ok(a), Ok(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                // 单行解析失败回退字典序（ADR-0038；不丢行）。
                _ => left_value.cmp(&right_value),
            },
            _ => left_value.cmp(&right_value),
        };
        let ordering = if key.order == "desc" {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    // 稳定排序：键相等保持原行序（行号小者在前）。
    left.1.cmp(&right.1)
}

/// 外排（§4.12 分块排序＋多路归并）：排序键与行号绑定后分块排序写临时文件，
/// 归并时多路取最小；临时文件归并后尽力删除（不记 Journal，实现细节）。
/// C13 外排临时块的清理守卫（ADR-0058）：作用域退出即删，成功与失败路径都不留
/// `.batchdesk-sort-*.tmp`。临时文件属实现细节，不记 Journal。
struct ChunkCleanup<'a> {
    fs: &'a dyn crate::fs::FsAdapter,
    paths: Vec<String>,
}

impl Drop for ChunkCleanup<'_> {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = self.fs.remove_file(path);
        }
    }
}

/// 按排序键稳定排序「行＋原序号」：键相等时用原序号兜底（`usize::MAX` 表示
/// 跨块归并相，此时不替换已选中的游标，稳定性由块序承担）。
fn sort_indexed(
    mut rows: Vec<(Vec<String>, usize)>,
    keys: &[PlanCsvSortKey],
) -> Vec<(Vec<String>, usize)> {
    rows.sort_by(|(left_row, left_index), (right_row, right_index)| {
        compare_by_keys(left_row, *left_index, right_row, *right_index, keys)
    });
    rows
}

/// 把一批已排序的行写成 UTF-8 块文件（外排的落盘相）。
fn write_chunk(
    view: &crate::generic_apply::WorkerView<'_>,
    parent: &str,
    plan_size: u64,
    chunk_index: usize,
    rows: &[(Vec<String>, usize)],
    delimiter: u8,
) -> Result<String, KernelError> {
    use crate::csv_merge::write_record;

    let mut text = String::new();
    for (row, _) in rows {
        write_record(row, delimiter, &mut text);
    }
    let path = format!("{parent}/.batchdesk-sort-{plan_size}-{chunk_index}.tmp");
    view.fs.write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

pub(crate) fn perform_csv_sort(
    view: &crate::generic_apply::WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvSort,
    backup: &mut Option<String>,
) -> Result<crate::generic_apply::Performed, KernelError> {
    use crate::csv_merge::CsvRecordStream;
    use crate::encoding::StreamTranscoder;
    use crate::generic_apply::{stream_csv_records, write_stream, CsvStreamWriter, StreamCsvInput};

    let encoding = payload.encoding.ok_or_else(|| {
        KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        )
    })?;
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvSort target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvSort invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvSort delimiter"))?;

    let backup = if overwrite {
        Some(crate::generic_apply::backup_once(view.fs, target, backup)?)
    } else {
        None
    };

    let parent = match target.rfind(['/', '\\']) {
        Some(index) => target[..index].to_string(),
        None => ".".to_string(),
    };
    let chunk_limit = payload.external_chunk_rows.map(|rows| rows.max(1) as usize);

    // 分块排序相（ADR-0058）：流式读源，缓冲到阈值行即排序落块——
    // 内存 O(块行数＋单条记录)，不再整文件读入。
    let mut header: Option<Vec<String>> = None;
    let mut header_checked = false;
    let mut buffer: Vec<(Vec<String>, usize)> = Vec::new();
    // 守卫在分块相之前建立：输入相中途失败也要清掉已落盘的块文件。
    let mut chunks = ChunkCleanup {
        fs: view.fs,
        paths: Vec::new(),
    };
    let mut next_index = 0usize;

    stream_csv_records(
        view,
        &StreamCsvInput {
            source,
            expected_size: None,
            encoding,
            invalid,
            delimiter: delimiter.as_byte(),
        },
        &mut |row| {
            if payload.has_header && !header_checked {
                header_checked = true;
                if normalized_columns(&row) != payload.columns {
                    return Err(KernelError::new(
                        "E2010",
                        "source header changed since plan preview; re-plan before applying",
                        None,
                        true,
                    ));
                }
                header = Some(row);
                return Ok(());
            }
            buffer.push((row, next_index));
            next_index += 1;
            if let Some(limit) = chunk_limit {
                if buffer.len() >= limit {
                    let rows = sort_indexed(std::mem::take(&mut buffer), &payload.keys);
                    let path = write_chunk(
                        view,
                        &parent,
                        payload.size,
                        chunks.paths.len(),
                        &rows,
                        delimiter.as_byte(),
                    )?;
                    chunks.paths.push(path);
                }
            }
            Ok(())
        },
    )?;
    if payload.has_header && !header_checked {
        // 与整段路径同口径：有表头而源里没有表头 ⇒ E2010。
        return Err(KernelError::new(
            "E2010",
            "source header changed since plan preview; re-plan before applying",
            None,
            true,
        ));
    }

    let in_memory = chunks.paths.is_empty();
    if !in_memory && !buffer.is_empty() {
        // 尾块：不足一块阈值也必须落盘，否则归并相会漏行。
        let rows = sort_indexed(std::mem::take(&mut buffer), &payload.keys);
        let path = write_chunk(
            view,
            &parent,
            payload.size,
            chunks.paths.len(),
            &rows,
            delimiter.as_byte(),
        )?;
        chunks.paths.push(path);
    }

    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        if let Some(header) = &header {
            sink.emit_record(header, delimiter.as_byte())?;
        }
        if in_memory {
            let sorted = sort_indexed(std::mem::take(&mut buffer), &payload.keys);
            for (row, _) in &sorted {
                sink.emit_record(row, delimiter.as_byte())?;
            }
            return Ok(());
        }
        // k 路归并相：每块一个拉取式记录流，逐条取最小——块文件不整块驻留。
        let mut streams: Vec<CsvRecordStream<Box<dyn std::io::Read + Send>>> =
            Vec::with_capacity(chunks.paths.len());
        for path in &chunks.paths {
            streams.push(CsvRecordStream::new(
                view.fs.open_read_stream(path)?,
                delimiter.as_byte(),
            ));
        }
        let mut cursors: Vec<Option<Vec<String>>> = (0..streams.len()).map(|_| None).collect();
        loop {
            for index in 0..streams.len() {
                if cursors[index].is_none() {
                    cursors[index] = streams[index].next_record()?;
                }
            }
            let mut best: Option<usize> = None;
            for index in 0..cursors.len() {
                let Some(row) = cursors[index].as_ref() else {
                    continue;
                };
                match best {
                    None => best = Some(index),
                    Some(current) => {
                        let current_row = cursors[current]
                            .as_ref()
                            .expect("the selected cursor is filled above");
                        if compare_by_keys(row, usize::MAX, current_row, usize::MAX, &payload.keys)
                            == Ordering::Less
                        {
                            best = Some(index);
                        }
                    }
                }
            }
            let Some(index) = best else { break };
            let row = cursors[index]
                .take()
                .expect("the selected cursor is filled above");
            sink.emit_record(&row, delimiter.as_byte())?;
        }
        Ok(())
    })?;

    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view
                .fs
                .quarantine_target_path(crate::conflict::file_name_of(&final_path))?,
        },
    };
    Ok(crate::generic_apply::Performed {
        skipped: false,
        replaced: overwrite,
        final_path: final_path.clone(),
        forward: FsOp::WriteAtomic {
            path: final_path,
            data_ref: None,
        },
        reverse,
        detail: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, columns: &[&str], key_values: Vec<String>, data_rows: u64) -> PlanC13File {
        PlanC13File {
            path: path.to_string(),
            canonical_path: None,
            file_size: 100,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            has_header: true,
            columns: columns.iter().map(|name| name.to_string()).collect(),
            column_count: columns.len() as u64,
            data_rows,
            dirty_rows: 0,
            key_values,
            target: format!("/out/{}", crate::conflict::file_name_of(path)),
            target_occupied: false,
        }
    }

    fn input(files: Vec<PlanC13File>, chunk_rows: Option<u64>) -> PlanC13Input {
        PlanC13Input {
            plan_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8".to_string(),
            keys: vec![ColumnPick::Name("value".to_string())],
            order: SortOrder::Asc,
            external_chunk_rows: chunk_rows,
            target_encoding: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
        }
    }

    #[test]
    fn numeric_key_kind_is_inferred_from_sample_values() {
        let document = plan_c13(input(
            vec![file(
                "/data/a.csv",
                &["name", "value"],
                vec!["3".to_string(), "10".to_string(), "2".to_string()],
                3,
            )],
            None,
        ))
        .unwrap();
        let payload = document.entries[0].csv_sort.as_ref().unwrap();
        assert_eq!(payload.keys[0].kind, "numeric");
        assert_eq!(payload.keys[0].index, 2);
        assert_eq!(payload.keys[0].order, "asc");
        assert!(payload.external_chunk_rows.is_none());
    }

    #[test]
    fn date_and_lexicographic_kinds_are_inferred() {
        let date_file = file(
            "/data/dates.csv",
            &["value"],
            vec![
                "2026-01-01".to_string(),
                "2026-06-15".to_string(),
                "2026-12-31".to_string(),
            ],
            3,
        );
        let document = plan_c13(input(vec![date_file], None)).unwrap();
        assert_eq!(
            document.entries[0].csv_sort.as_ref().unwrap().keys[0].kind,
            "date"
        );

        let text_file = file(
            "/data/text.csv",
            &["value"],
            vec!["banana".to_string(), "apple".to_string()],
            2,
        );
        let document = plan_c13(input(vec![text_file], None)).unwrap();
        assert_eq!(
            document.entries[0].csv_sort.as_ref().unwrap().keys[0].kind,
            "lexicographic"
        );
    }

    #[test]
    fn external_threshold_is_recorded_and_warns() {
        let document = plan_c13(input(
            vec![file(
                "/data/big.csv",
                &["value"],
                vec!["5".to_string(), "3".to_string()],
                9,
            )],
            Some(5),
        ))
        .unwrap();
        let payload = document.entries[0].csv_sort.as_ref().unwrap();
        assert_eq!(payload.external_chunk_rows, Some(5));
        assert!(document
            .warnings
            .iter()
            .any(|warning| warning.kind == "external-sort"));
    }

    #[test]
    fn missing_key_column_blocks_with_e3004() {
        let mut input = input(
            vec![file("/data/a.csv", &["name", "value"], vec![], 2)],
            None,
        );
        input.keys = vec![ColumnPick::Name("missing".to_string())];
        let document = plan_c13(input).unwrap();
        // 键列缺失按条目 blocked E3004（与 F07 同规），不静默补空。
        assert_eq!(document.entries[0].blocked.as_ref().unwrap().code, "E3004");
        assert!(document.entries[0]
            .csv_sort
            .as_ref()
            .unwrap()
            .keys
            .is_empty());
    }

    #[test]
    fn numeric_comparison_is_value_ordered_not_lexicographic() {
        // "10" < "2" 数值序；执行器比较用同一 compare_by_keys。
        let keys = vec![PlanCsvSortKey {
            index: 1,
            name: Some("value".to_string()),
            order: "asc".to_string(),
            kind: "numeric".to_string(),
        }];
        let low = vec!["2".to_string()];
        let high = vec!["10".to_string()];
        assert_eq!(
            compare_by_keys(&high, 0, &low, 1, &keys),
            Ordering::Greater,
            "numeric 10 sorts after 2"
        );
        // 字典序对照："10" < "2"——证明两种类型序不同。
        let keys = vec![PlanCsvSortKey {
            index: 1,
            name: None,
            order: "asc".to_string(),
            kind: "lexicographic".to_string(),
        }];
        assert_eq!(
            compare_by_keys(&high, 0, &low, 1, &keys),
            Ordering::Less,
            "lexicographic 10 sorts before 2"
        );
        let banana = vec!["banana".to_string()];
        let apple = vec!["apple".to_string()];
        assert_eq!(
            compare_by_keys(&banana, 0, &apple, 1, &keys),
            Ordering::Greater
        );
    }
}
