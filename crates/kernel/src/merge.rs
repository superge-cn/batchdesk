//! F04 TXT 合并（方案 §5.15 "顺序追加写入，编码不一致时先统一解码再按目标编码写出"）。
//!
//! 纯函数层：只消费壳层准备好的输入清单（路径、大小、检测/确认的源编码），
//! 生成一条 N:1 的 `merge` 计划条目；不读盘、不取时钟、不做编码嗅探。
//! 壳层负责采样与 [`crate::encoding::detect`] 判定，apply 负责逐文件解码、
//! 按目标编码写出并在临时文件上原子改名（设计 01 §5/§8）。
//!
//! 本切片的合并产物按输入顺序拼接；BOM 只在产物最前面写一次（目标编码开启
//! BOM 时），每个输入自身的 BOM 由 `transcode` 按源编码剥离。
//! 大文件流式写入仍待补（当前 apply 逐文件整段读取后再原子写）。

use std::collections::BTreeSet;

use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanDocument, PlanEntry, PlanExpected,
    PlanMerge, PlanMergeSource, PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const F04_KIND: &str = "f04";

/// F04 的单个输入：壳层已判定（或用户确认）的源编码；`None` 表示需人工确认。
#[derive(Clone, Debug)]
pub struct PlanF04File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
}

#[derive(Clone, Debug)]
pub struct PlanF04Input {
    pub plan_id: String,
    /// 合并产物的完整路径（N:1 的唯一输出）。
    pub target: String,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 输出路径是否已被占用（壳层在 plan 期 stat 得到；默认不覆盖）。
    pub target_occupied: bool,
    pub files: Vec<PlanF04File>,
}

/// 生成 F04 计划：一条 `merge` 条目按输入顺序汇总全部文件。
pub fn plan_f04(input: PlanF04Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.target.trim().is_empty() {
        return Err(KernelError::missing_param("target is required"));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param(
            "merge plans require at least one file",
        ));
    }

    let mut warnings = Vec::new();
    let mut blocked = target_block(&input.target, &input.protected_dirs, input.case_sense);
    let mut sources = Vec::with_capacity(input.files.len());
    let mut detected = BTreeSet::new();
    let mut ambiguous = Vec::new();
    for file in &input.files {
        if blocked.is_none() {
            blocked = source_block(
                &file.path,
                file.canonical_path.as_deref(),
                &input.protected_dirs,
                input.case_sense,
            );
        }
        if let Some(encoding) = file.source_encoding {
            detected.insert(encoding.as_str());
        } else {
            ambiguous.push(file.path.clone());
        }
        sources.push(PlanMergeSource {
            path: file.path.clone(),
            encoding: file.source_encoding,
            size: file.file_size,
        });
    }
    if blocked.is_none() && !ambiguous.is_empty() {
        blocked = Some(PlanBlocked {
            code: "E3001".to_string(),
            reason: format!(
                "source encoding requires manual confirmation: {}",
                ambiguous.join(", ")
            ),
            hint: Some(
                "confirm each source encoding (or pass a global --source override)".to_string(),
            ),
        });
    }
    if detected.len() > 1 {
        warnings.push(PlanWarning {
            kind: "mixed-source-encodings".to_string(),
            suggestion: Some(format!(
                "inputs use {} encodings ({}); each file is decoded and re-encoded to {}",
                detected.len(),
                detected.iter().copied().collect::<Vec<_>>().join(", "),
                input.target_encoding.as_str()
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let conflict = input.target_occupied.then(|| PlanConflict {
        kind: "existing-target".to_string(),
        risk_level: RiskLevel::Danger,
    });
    let target_name = crate::conflict::file_name_of(&input.target);
    let entry = PlanEntry {
        entry_id: format!("{}-0001", input.plan_id),
        source: input.files[0].path.clone(),
        target: Some(input.target.clone()),
        action: PlanAction::Merge,
        relation: None,
        reason: format!(
            "merge {} file(s) into {} as {}",
            input.files.len(),
            target_name,
            input.target_encoding.as_str()
        ),
        conflict: conflict.clone(),
        resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
        blocked,
        expected: Some(PlanExpected {
            size: input.files[0].file_size,
            algorithm: "size".to_string(),
            hash: None,
        }),
        range: None,
        merge: Some(PlanMerge {
            sources,
            target_encoding: input.target_encoding.as_str().to_string(),
            invalid: input.invalid.as_str().to_string(),
        }),
        csv: None,
    };
    let entries = vec![entry];

    let mut params = std::collections::BTreeMap::new();
    params.insert("target".to_string(), PlanParamValue::String(input.target));
    params.insert(
        "targetEncoding".to_string(),
        PlanParamValue::String(input.target_encoding.as_str().to_string()),
    );
    params.insert(
        "invalid".to_string(),
        PlanParamValue::String(input.invalid.as_str().to_string()),
    );
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
        kind: F04_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn source_block(
    path: &str,
    canonical: Option<&str>,
    protected: &[ProtectedDir],
    sense: CaseSense,
) -> Option<PlanBlocked> {
    match try_normalize(path) {
        Ok(normalized) => {
            protected_by(&normalized, canonical, protected, sense).map(|hit| PlanBlocked {
                code: "E2009".to_string(),
                reason: format!("path is under protected directory {hit}"),
                hint: Some("remove it from protectedDirs or choose another source".to_string()),
            })
        }
        Err(error) => Some(PlanBlocked {
            code: error.code,
            reason: error.message,
            hint: None,
        }),
    }
}

fn target_block(path: &str, protected: &[ProtectedDir], sense: CaseSense) -> Option<PlanBlocked> {
    match try_normalize(path) {
        Ok(normalized) => {
            protected_by(&normalized, None, protected, sense).map(|hit| PlanBlocked {
                code: "E2009".to_string(),
                reason: format!("target is under protected directory {hit}"),
                hint: Some("choose another output path".to_string()),
            })
        }
        Err(error) => Some(PlanBlocked {
            code: error.code,
            reason: error.message,
            hint: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn file(path: &str, size: u64, encoding: Option<SourceEncoding>) -> PlanF04File {
        PlanF04File {
            path: path.to_string(),
            canonical_path: None,
            file_size: size,
            source_encoding: encoding,
        }
    }

    fn input(files: Vec<PlanF04File>) -> PlanF04Input {
        PlanF04Input {
            plan_id: PLAN_ID.to_string(),
            target: "/out/merged.txt".to_string(),
            target_encoding: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            target_occupied: false,
            files,
        }
    }

    #[test]
    fn plan_keeps_input_order_and_encodings() {
        let plan = plan_f04(input(vec![
            file("/data/a.txt", 10, Some(SourceEncoding::Utf8)),
            file("/data/b.txt", 20, Some(SourceEncoding::Gb18030)),
        ]))
        .unwrap();
        assert_eq!(plan.kind, F04_KIND);
        assert_eq!(plan.stats.total, 1);
        let entry = &plan.entries[0];
        assert_eq!(entry.action, PlanAction::Merge);
        assert_eq!(entry.source, "/data/a.txt");
        assert_eq!(entry.target.as_deref(), Some("/out/merged.txt"));
        assert_eq!(entry.expected.as_ref().unwrap().size, 10);
        let merge = entry.merge.as_ref().unwrap();
        assert_eq!(merge.target_encoding, "utf-8");
        assert_eq!(merge.invalid, "replace-and-count");
        assert_eq!(merge.sources.len(), 2);
        assert_eq!(merge.sources[0].path, "/data/a.txt");
        assert_eq!(merge.sources[0].encoding, Some(SourceEncoding::Utf8));
        assert_eq!(merge.sources[1].path, "/data/b.txt");
        assert_eq!(merge.sources[1].encoding, Some(SourceEncoding::Gb18030));
        assert_eq!(merge.sources[1].size, 20);
        assert_eq!(plan.stats.warning, 1);
        assert_eq!(plan.warnings[0].kind, "mixed-source-encodings");
    }

    #[test]
    fn same_encoding_inputs_do_not_warn() {
        let plan = plan_f04(input(vec![
            file("/data/a.txt", 10, Some(SourceEncoding::Utf8)),
            file("/data/b.txt", 20, Some(SourceEncoding::Utf8)),
        ]))
        .unwrap();
        assert_eq!(plan.stats.warning, 0);
    }

    #[test]
    fn existing_target_is_a_conflict_skipped_by_default() {
        let mut plan_input = input(vec![file("/data/a.txt", 10, Some(SourceEncoding::Utf8))]);
        plan_input.target_occupied = true;
        let plan = plan_f04(plan_input).unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.conflict.as_ref().unwrap().kind, "existing-target");
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
        assert_eq!(plan.stats.conflict, 1);
    }

    #[test]
    fn ambiguous_encoding_blocks_the_merge_with_e3001() {
        let plan = plan_f04(input(vec![
            file("/data/a.txt", 10, Some(SourceEncoding::Utf8)),
            file("/data/b.txt", 20, None),
        ]))
        .unwrap();
        let entry = &plan.entries[0];
        let blocked = entry.blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E3001");
        assert!(blocked.reason.contains("/data/b.txt"));
        assert_eq!(plan.stats.blocked, 1);
    }

    #[test]
    fn protected_source_blocks_the_merge_with_e2009() {
        let mut plan_input = input(vec![file(
            "/data/secure/a.txt",
            10,
            Some(SourceEncoding::Utf8),
        )]);
        plan_input.protected_dirs = vec![ProtectedDir::new("/data/secure")];
        let plan = plan_f04(plan_input).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E2009");
    }

    #[test]
    fn protected_target_blocks_the_merge_with_e2009() {
        let mut plan_input = input(vec![file("/data/a.txt", 10, Some(SourceEncoding::Utf8))]);
        plan_input.target = "/data/secure/merged.txt".to_string();
        plan_input.protected_dirs = vec![ProtectedDir::new("/data/secure")];
        let plan = plan_f04(plan_input).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E2009");
    }

    #[test]
    fn plan_rejects_empty_input_and_zero_sources() {
        let mut plan_input = input(Vec::new());
        plan_input.plan_id = "  ".to_string();
        assert_eq!(plan_f04(plan_input).unwrap_err().code, "E1001");
        let error = plan_f04(input(Vec::new())).unwrap_err();
        assert_eq!(error.code, "E1001");
        assert!(error.message.contains("at least one file"));
    }

    #[test]
    fn plan_params_carry_target_and_encoding() {
        let plan = plan_f04(input(vec![file(
            "/data/a.txt",
            10,
            Some(SourceEncoding::Utf8),
        )]))
        .unwrap();
        match plan.params.get("targetEncoding").unwrap() {
            PlanParamValue::String(value) => assert_eq!(value, "utf-8"),
            _ => panic!("targetEncoding must be a string"),
        }
        match plan.params.get("invalid").unwrap() {
            PlanParamValue::String(value) => assert_eq!(value, "replace-and-count"),
            _ => panic!("invalid must be a string"),
        }
        match plan.params.get("fileCount").unwrap() {
            PlanParamValue::Number(value) => assert_eq!(*value, 1),
            _ => panic!("fileCount must be a number"),
        }
    }

    #[test]
    fn merge_payload_round_trips_through_json() {
        let plan = plan_f04(input(vec![file(
            "/data/a.txt",
            10,
            Some(SourceEncoding::Gb18030),
        )]))
        .unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        let decoded: PlanDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, decoded);
    }
}
