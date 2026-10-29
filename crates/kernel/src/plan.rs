//! C09 plan generation. Pure: it never reads or writes the filesystem.
//!
//! The returned `PlanDocument` follows design 01 §5. C09 cleanup is a
//! `delete` action with no target; apply is responsible for moving the file
//! to the quarantine directory. Candidate (fast-mode) groups are rejected:
//! cleanup requires full-hash confirmation (baseline §5.10, 11.6-20).
//! The caller supplies `planId`; this module never generates IDs or reads time.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::duplicate::{Confidence, DuplicateGroup, DuplicateReport, GroupingMode};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{protected_by, try_normalize, ProtectedDir};

pub const C09_KIND: &str = "c09";

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Metadata the planner may use for the keep suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    pub id: u64,
    pub path: String,
    pub canonical_path: Option<String>,
    pub mtime_nanos: i128,
    pub full_hash: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepPolicy {
    Earliest,
    ShortestPath,
    Newest,
}

impl KeepPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Earliest => "earliest",
            Self::ShortestPath => "shortest-path",
            Self::Newest => "newest",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "earliest" => Some(Self::Earliest),
            "shortest-path" => Some(Self::ShortestPath),
            "newest" => Some(Self::Newest),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlanParamValue {
    String(String),
    Number(u64),
    Bool(bool),
    Null,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Safe,
    Caution,
    Danger,
}

impl RiskLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Caution => "caution",
            Self::Danger => "danger",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanAction {
    Rename,
    Move,
    Copy,
    Archive,
    Extract,
    CreateDir,
    Transcode,
    EditContent,
    Delete,
}

impl PlanAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Archive => "archive",
            Self::Extract => "extract",
            Self::CreateDir => "create-dir",
            Self::Transcode => "transcode",
            Self::EditContent => "edit-content",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanResolution {
    Proceed,
    Skip,
    Sequence,
    Overwrite,
}

impl PlanResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proceed => "proceed",
            Self::Skip => "skip",
            Self::Sequence => "sequence",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanRole {
    Source,
    Output,
}

impl PlanRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Output => "output",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRelation {
    pub group_id: String,
    pub role: PlanRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_total: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanConflict {
    pub kind: String,
    pub risk_level: RiskLevel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBlocked {
    pub code: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanExpected {
    pub size: u64,
    pub algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub entry_id: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub action: PlanAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<PlanRelation>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<PlanConflict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<PlanResolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<PlanBlocked>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<PlanExpected>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWarning {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub risk_level: RiskLevel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStats {
    pub total: usize,
    pub conflict: usize,
    pub warning: usize,
    pub blocked: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDocument {
    pub plan_id: String,
    pub kind: String,
    pub params: BTreeMap<String, PlanParamValue>,
    pub entries: Vec<PlanEntry>,
    pub stats: PlanStats,
    pub warnings: Vec<PlanWarning>,
}

#[derive(Clone, Debug)]
pub struct PlanC09Input {
    pub plan_id: String,
    pub keep: KeepPolicy,
    pub mode: GroupingMode,
    pub hash_algorithm: String,
    pub quick_window_bytes: Option<u64>,
    pub include_zero_byte: bool,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanFile>,
    pub report: DuplicateReport,
    pub skipped_paths: Vec<String>,
}

/// Build a C09 cleanup plan for confirmed duplicate groups.
///
/// Groups with unknown file identity are excluded and surfaced as a warning;
/// candidate groups and fast-mode inputs are rejected because cleanup requires
/// full-hash confirmation.
pub fn plan_c09(input: PlanC09Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.hash_algorithm.trim().is_empty() {
        return Err(KernelError::missing_param("hashAlgorithm is required"));
    }
    if input.mode == GroupingMode::Fast {
        return Err(KernelError::invalid_params(
            "fast mode produces candidate groups only; run full hashing before planning cleanup",
        ));
    }
    if input
        .report
        .groups
        .iter()
        .any(|group| group.confidence == Confidence::Candidate)
    {
        return Err(KernelError::invalid_params(
            "candidate duplicate groups cannot be planned for cleanup",
        ));
    }

    let files = index_files(&input.files)?;
    let mut params = BTreeMap::new();
    params.insert(
        "keep".to_string(),
        PlanParamValue::String(input.keep.as_str().to_string()),
    );
    params.insert(
        "mode".to_string(),
        PlanParamValue::String("full".to_string()),
    );
    params.insert(
        "hash".to_string(),
        PlanParamValue::String(input.hash_algorithm.clone()),
    );
    params.insert(
        "quickWindowBytes".to_string(),
        match input.quick_window_bytes {
            Some(window) => PlanParamValue::Number(window),
            None => PlanParamValue::Null,
        },
    );
    params.insert(
        "includeZeroByte".to_string(),
        PlanParamValue::Bool(input.include_zero_byte),
    );

    let mut entries = Vec::new();
    let mut warnings = Vec::new();

    for (group_index, group) in input.report.groups.iter().enumerate() {
        let members = group_members(group, &files)?;
        if group.needs_manual_review {
            warnings.push(PlanWarning {
                kind: "needs-manual-review".to_string(),
                suggestion: Some(format!(
                    "group {} (size {}, {} files) has unknown file identity; excluded from cleanup",
                    group_index + 1,
                    group.size,
                    group.members.len()
                )),
                risk_level: RiskLevel::Caution,
            });
            continue;
        }
        let keep = select_keep(&members, input.keep)?;
        for member in &members {
            if member.id == keep.id {
                continue;
            }
            let blocked = match try_normalize(&member.path) {
                Ok(normalized) => protected_by(
                    &normalized,
                    member.canonical_path.as_deref(),
                    &input.protected_dirs,
                    input.case_sense,
                )
                .map(|hit| PlanBlocked {
                    code: "E2009".to_string(),
                    reason: format!("path is under protected directory {hit}"),
                    hint: Some("remove it from protectedDirs or choose another source".to_string()),
                }),
                Err(error) => Some(PlanBlocked {
                    code: error.code,
                    reason: error.message,
                    hint: None,
                }),
            };
            entries.push(PlanEntry {
                entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
                source: member.path.clone(),
                target: None,
                action: PlanAction::Delete,
                relation: None,
                reason: format!(
                    "duplicate of {} (group size {}, keep by {}); apply moves this file to the quarantine directory",
                    keep.path,
                    group.size,
                    input.keep.as_str()
                ),
                conflict: None,
                resolution: None,
                blocked,
                expected: Some(PlanExpected {
                    size: group.size,
                    algorithm: input.hash_algorithm.clone(),
                    hash: member.full_hash.as_deref().map(hex_encode),
                }),
            });
        }
    }

    if !input.report.hardlink_skipped.is_empty() {
        warnings.push(PlanWarning {
            kind: "hardlink-skipped".to_string(),
            suggestion: Some(format!(
                "{} hardlink entries were skipped as the same file",
                input.report.hardlink_skipped.len()
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !input.report.incomplete.is_empty() {
        warnings.push(PlanWarning {
            kind: "incomplete-hash".to_string(),
            suggestion: Some(format!(
                "{} candidates lack a full digest; excluded from cleanup",
                input.report.incomplete.len()
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !input.report.zero_byte_ids.is_empty() {
        warnings.push(PlanWarning {
            kind: "zero-byte-ignored".to_string(),
            suggestion: Some(format!(
                "{} zero-byte files ignored",
                input.report.zero_byte_ids.len()
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !input.skipped_paths.is_empty() {
        warnings.push(PlanWarning {
            kind: "scan-skipped".to_string(),
            suggestion: Some(format!(
                "{} entries were skipped during scanning; not planned",
                input.skipped_paths.len()
            )),
            risk_level: RiskLevel::Caution,
        });
    }

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
        kind: C09_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn index_files(files: &[PlanFile]) -> Result<BTreeMap<u64, &PlanFile>, KernelError> {
    let mut index = BTreeMap::new();
    for file in files {
        if index.insert(file.id, file).is_some() {
            return Err(KernelError::internal(
                "duplicate file id in plan input",
                format!("id {}", file.id),
            ));
        }
    }
    Ok(index)
}

fn group_members<'a>(
    group: &DuplicateGroup,
    files: &'a BTreeMap<u64, &'a PlanFile>,
) -> Result<Vec<&'a PlanFile>, KernelError> {
    group
        .members
        .iter()
        .map(|id| {
            files.get(id).copied().ok_or_else(|| {
                KernelError::internal(
                    "plan input is missing a file referenced by the duplicate report",
                    format!("missing id {id}"),
                )
            })
        })
        .collect()
}

fn select_keep<'a>(
    members: &[&'a PlanFile],
    policy: KeepPolicy,
) -> Result<&'a PlanFile, KernelError> {
    members
        .iter()
        .copied()
        .min_by(|left, right| compare_keep(left, right, policy))
        .ok_or_else(|| {
            KernelError::internal("duplicate group has no members", "group members were empty")
        })
}

fn compare_keep(left: &PlanFile, right: &PlanFile, policy: KeepPolicy) -> Ordering {
    let by_path = path_len(left)
        .cmp(&path_len(right))
        .then_with(|| left.path.cmp(&right.path));
    match policy {
        KeepPolicy::Earliest => left.mtime_nanos.cmp(&right.mtime_nanos).then(by_path),
        KeepPolicy::Newest => right.mtime_nanos.cmp(&left.mtime_nanos).then(by_path),
        KeepPolicy::ShortestPath => by_path,
    }
}

fn path_len(file: &PlanFile) -> usize {
    file.path.chars().count()
}

fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.as_bytes()[0] <= b'7'
        && value.bytes().all(|byte| CROCKFORD.contains(&byte))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    fn file(id: u64, path: &str, mtime_nanos: i128) -> PlanFile {
        PlanFile {
            id,
            path: path.to_string(),
            canonical_path: None,
            mtime_nanos,
            full_hash: Some(vec![id as u8; 4]),
        }
    }

    fn group(size: u64, members: Vec<u64>) -> DuplicateGroup {
        DuplicateGroup {
            size,
            hash: None,
            members,
            confidence: Confidence::Confirmed,
            needs_manual_review: false,
        }
    }

    fn input(files: Vec<PlanFile>, groups: Vec<DuplicateGroup>, keep: KeepPolicy) -> PlanC09Input {
        PlanC09Input {
            plan_id: PLAN_ID.to_string(),
            keep,
            mode: GroupingMode::Full,
            hash_algorithm: "blake3".to_string(),
            quick_window_bytes: Some(4096),
            include_zero_byte: false,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
            report: DuplicateReport {
                groups,
                zero_byte_ids: Vec::new(),
                hardlink_skipped: Vec::new(),
                incomplete: Vec::new(),
            },
            skipped_paths: Vec::new(),
        }
    }

    #[test]
    fn earliest_keeps_oldest_mtime() {
        let plan = plan_c09(input(
            vec![file(1, "a", 20), file(2, "b", 10), file(3, "c", 30)],
            vec![group(100, vec![1, 2, 3])],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.source.ends_with("b")));
        assert!(plan.entries[0].reason.contains("b"));
        assert_eq!(plan.stats.total, 2);
        assert_eq!(
            plan.params["keep"],
            PlanParamValue::String("earliest".into())
        );
    }

    #[test]
    fn entries_carry_expected_size_and_hex_hash() {
        let plan = plan_c09(input(
            vec![file(1, "a", 20), file(2, "b", 10)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        let expected = plan.entries[0].expected.as_ref().unwrap();
        assert_eq!(expected.size, 100);
        assert_eq!(expected.algorithm, "blake3");
        assert_eq!(expected.hash.as_deref(), Some("01010101"));
    }

    #[test]
    fn newest_keeps_latest_mtime() {
        let plan = plan_c09(input(
            vec![file(1, "a", 20), file(2, "b", 10), file(3, "c", 30)],
            vec![group(100, vec![1, 2, 3])],
            KeepPolicy::Newest,
        ))
        .unwrap();
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.source.ends_with("c")));
        assert!(plan.entries[0].reason.contains("c"));
    }

    #[test]
    fn shortest_path_keeps_shortest_and_breaks_ties_lexicographically() {
        let plan = plan_c09(input(
            vec![
                file(1, "root/long.txt", 20),
                file(2, "root/b", 20),
                file(3, "root/a", 20),
            ],
            vec![group(100, vec![1, 2, 3])],
            KeepPolicy::ShortestPath,
        ))
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.source.ends_with("root/a")));
        assert!(plan.entries[0].reason.contains("root/a"));
    }

    #[test]
    fn manual_review_groups_are_excluded_with_a_warning() {
        let mut duplicate_group = group(100, vec![1, 2]);
        duplicate_group.needs_manual_review = true;
        let plan = plan_c09(input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![duplicate_group],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        assert!(plan.entries.is_empty());
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].kind, "needs-manual-review");
        assert_eq!(plan.stats.warning, 1);
    }

    #[test]
    fn protected_group_member_is_blocked_with_e2009() {
        let mut plan_input = input(
            vec![file(1, "/allowed/a", 10), file(2, "/protected/b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.protected_dirs = vec![ProtectedDir::new("/protected")];
        let plan = plan_c09(plan_input).unwrap();
        assert_eq!(plan.entries.len(), 1);
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E2009");
        assert_eq!(plan.stats.blocked, 1);
    }

    #[test]
    fn candidate_groups_are_rejected() {
        let mut candidate_group = group(100, vec![1, 2]);
        candidate_group.confidence = Confidence::Candidate;
        let error = plan_c09(input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![candidate_group],
            KeepPolicy::Earliest,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn fast_mode_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.mode = GroupingMode::Fast;
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn empty_plan_id_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.plan_id = "  ".to_string();
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1001");
    }

    #[test]
    fn invalid_plan_id_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.plan_id = "not-a-ulid".to_string();
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn empty_hash_algorithm_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.hash_algorithm = "  ".to_string();
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1001");
    }

    #[test]
    fn incomplete_zero_byte_and_scan_skips_become_warnings() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.report.incomplete = vec![7];
        plan_input.report.zero_byte_ids = vec![8];
        plan_input.report.hardlink_skipped = vec![9];
        plan_input.skipped_paths = vec!["locked".to_string()];
        let plan = plan_c09(plan_input).unwrap();
        let kinds: Vec<&str> = plan
            .warnings
            .iter()
            .map(|warning| warning.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "hardlink-skipped",
                "incomplete-hash",
                "zero-byte-ignored",
                "scan-skipped"
            ]
        );
        assert_eq!(plan.stats.warning, 4);
    }

    #[test]
    fn missing_group_member_is_an_internal_error() {
        let error = plan_c09(input(
            vec![file(1, "a", 10)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E9001");
    }

    #[test]
    fn entry_ids_are_unique_and_include_the_plan_id() {
        let plan = plan_c09(input(
            vec![
                file(1, "a", 10),
                file(2, "b", 20),
                file(3, "c", 30),
                file(4, "d", 40),
            ],
            vec![group(100, vec![1, 2, 3, 4])],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        let mut ids: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.entry_id.starts_with(PLAN_ID)));
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.action == PlanAction::Delete && entry.target.is_none()));
    }
}
