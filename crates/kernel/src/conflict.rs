//! 冲突枚举与裁决（方案 §5.13、设计 01 §5.1）。
//!
//! 纯函数层：不 stat、不写盘。壳层把"目标是否已存在"的探测结果作为参数传入，
//! kernel 只负责分类、给出策略裁决与命名建议（不变量 1）。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::fs::CaseSense;
use crate::plan::RiskLevel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// 与现存条目同名。
    Existing,
    /// 批内多个条目指向同一目标（按平台大小写规则比较）。
    WithinBatch,
    /// 仅大小写不同（Windows/macOS 视为同一名）。
    CaseOnly,
}

impl ConflictKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::WithinBatch => "within-batch",
            Self::CaseOnly => "case-only",
        }
    }

    pub const fn risk_level(self) -> RiskLevel {
        match self {
            Self::Existing | Self::WithinBatch => RiskLevel::Danger,
            Self::CaseOnly => RiskLevel::Caution,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameWarning {
    /// 设计 01 §5 的 `PlanWarning.kind` 取值。
    pub kind: String,
    pub suggestion: Option<String>,
    pub risk_level: RiskLevel,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictScan {
    /// 条目下标 → 冲突类别（三类互斥，现存同名优先）。
    pub conflicts: BTreeMap<usize, ConflictKind>,
    /// 条目下标 → 命名警告（非法字符/保留名/结尾点空格/空名）。
    pub warnings: BTreeMap<usize, Vec<NameWarning>>,
}

/// Windows 保留名（方案 §5.13）；比较时不区分大小写，且含 `CON.txt` 形式。
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Windows 文件名非法字符（`< > : " | ? *` 与 ASCII 控制字符）。
const WINDOWS_ILLEGAL_CHARS: &[char] = &['<', '>', ':', '"', '|', '?', '*'];

/// 目标名合法性规则（方案 §5.13、设计 04 §2.1）。
///
/// **平台事实不在内核推断**：规则值由适配层给出、经计划输入注入，内核只做判定
/// （不变量 1；设计 04 §2「不写平台 if/else」）。这条边界不是洁癖——同一份二进制
/// 在 Linux 上会挂载 FAT/NTFS 卷，按平台名而不是按卷名判定才是可解释的；
/// 而把 `cfg!(windows)` 写进内核，测试就永远只能覆盖编译它的那个平台。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRules {
    /// 目标名中不允许出现的字符（**只列这些**，不做额外推断）。
    pub illegal_chars: &'static [char],
    /// 是否把 ASCII 控制字符（含 NUL）也算非法。Windows 不许，POSIX 只禁 NUL。
    pub reject_control_chars: bool,
    /// 保留设备名（按主名、ASCII 大小写不敏感比较）；空表＝该平台没有保留名。
    pub reserved_names: &'static [&'static str],
    /// 结尾的空格或点是否非法（Windows 会静默截断，POSIX 原样保留）。
    pub trailing_dot_space: bool,
}

impl NameRules {
    /// Windows 语义：`< > : " | ? *`＋控制字符非法、保留名、结尾点/空格。
    pub const fn windows() -> Self {
        Self {
            illegal_chars: WINDOWS_ILLEGAL_CHARS,
            reject_control_chars: true,
            reserved_names: WINDOWS_RESERVED_NAMES,
            trailing_dot_space: true,
        }
    }

    /// POSIX 语义：文件名里只有 `/` 与 NUL 不可出现；无保留名，结尾点/空格合法。
    ///
    /// 控制字符（除 NUL）在 ext4/APFS 上是**合法**文件名，故 `reject_control_chars`
    /// 为 `false`——Linux 上按 Windows 清单拦名会把 `报告:2026.csv` 这类合法名误判。
    pub const fn posix() -> Self {
        Self {
            illegal_chars: &['/', '\0'],
            reject_control_chars: false,
            reserved_names: &[],
            trailing_dot_space: false,
        }
    }

    /// 该字符是否非法（区分「字符本身非法」与「控制字符按规则非法」）。
    fn rejects(self, ch: char) -> bool {
        self.illegal_chars.contains(&ch) || (self.reject_control_chars && ch.is_ascii_control())
    }
}

fn fold(value: &str, sense: CaseSense) -> String {
    match sense {
        CaseSense::Sensitive => value.to_string(),
        CaseSense::CaseInsensitive => value.to_ascii_lowercase(),
    }
}

/// 目标路径的最后一段（文件名），同时接受 `/` 与 `\`。
pub fn file_name_of(target: &str) -> &str {
    target.rsplit(['/', '\\']).next().unwrap_or(target)
}

/// 枚举三类冲突（方案 §5.13：批内重名、与现存重名、仅大小写不同）。
///
/// `existing` 是壳层已探测到的现存目标路径集合（原样字符串）；
/// `entries` 为 `(source, target)` 对，顺序即计划条目顺序；
/// `rules` 是目标名合法性规则（平台能力，见 [`NameRules`]）。
pub fn scan_conflicts(
    entries: &[(String, String)],
    existing: &BTreeSet<String>,
    sense: CaseSense,
    rules: NameRules,
) -> ConflictScan {
    let existing_folded: HashMap<String, Vec<&String>> = {
        let mut map: HashMap<String, Vec<&String>> = HashMap::new();
        for path in existing {
            map.entry(fold(path, sense)).or_default().push(path);
        }
        map
    };
    let mut first_by_fold: HashMap<String, usize> = HashMap::new();
    let mut scan = ConflictScan::default();

    for (index, (_source, target)) in entries.iter().enumerate() {
        let name_warnings = validate_target_name(file_name_of(target), rules);
        if !name_warnings.is_empty() {
            scan.warnings.insert(index, name_warnings);
        }

        let exact_existing = existing.contains(target);
        let folded_existing = existing_folded
            .get(&fold(target, sense))
            .map(|paths| paths.iter().any(|path| path.as_str() != target))
            .unwrap_or(false);
        let folded = fold(target, sense);

        if exact_existing {
            scan.conflicts.insert(index, ConflictKind::Existing);
            first_by_fold.entry(folded).or_insert(index);
            continue;
        }
        if folded_existing {
            scan.conflicts.insert(index, ConflictKind::CaseOnly);
            first_by_fold.entry(folded).or_insert(index);
            continue;
        }
        match first_by_fold.get(&folded) {
            Some(first) => {
                let kind = if entries[*first].1 == *target {
                    ConflictKind::WithinBatch
                } else {
                    ConflictKind::CaseOnly
                };
                scan.conflicts.insert(index, kind);
            }
            None => {
                first_by_fold.insert(folded, index);
            }
        }
    }
    scan
}

/// 命名规则校验（方案 §5.13：非法字符、保留名、结尾空格/点、空名）。
///
/// 「什么是非法」由 `rules` 决定，函数本身不含任何平台判断：Windows 规则见
/// [`NameRules::windows`]，POSIX 规则见 [`NameRules::posix`]（设计 04 §2.1）。
pub fn validate_target_name(name: &str, rules: NameRules) -> Vec<NameWarning> {
    let mut warnings = Vec::new();
    if name.trim().is_empty() {
        warnings.push(NameWarning {
            kind: "empty-name".to_string(),
            suggestion: None,
            risk_level: RiskLevel::Danger,
        });
        return warnings;
    }
    if rules.trailing_dot_space && (name.ends_with(' ') || name.ends_with('.')) {
        warnings.push(NameWarning {
            kind: "trailing-dot-space".to_string(),
            suggestion: Some(name.trim_end_matches([' ', '.']).to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    if rules
        .reserved_names
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        warnings.push(NameWarning {
            kind: "reserved-name".to_string(),
            suggestion: Some(format!("{stem}_")),
            risk_level: RiskLevel::Danger,
        });
    }
    if name.chars().any(|ch| rules.rejects(ch)) {
        warnings.push(NameWarning {
            kind: "invalid-char".to_string(),
            suggestion: Some(
                name.chars()
                    .map(|ch| if rules.rejects(ch) { '_' } else { ch })
                    .collect(),
            ),
            risk_level: RiskLevel::Caution,
        });
    }
    warnings
}

/// `sequence` 策略的目标命名：`name.ext` → `name (1).ext`，直到不占用。
///
/// `occupied` 应包含"现存目标"与"本批次已裁决的目标"，避免批内再次撞名。
pub fn sequence_target(target: &str, occupied: &BTreeSet<String>, sense: CaseSense) -> String {
    if !is_occupied(target, occupied, sense) {
        return target.to_string();
    }
    let name = file_name_of(target);
    let prefix = &target[..target.len() - name.len()];
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    for counter in 1..=u32::MAX {
        let candidate = match extension {
            Some(ext) => format!("{prefix}{stem} ({counter}).{ext}"),
            None => format!("{prefix}{stem} ({counter})"),
        };
        if !is_occupied(&candidate, occupied, sense) {
            return candidate;
        }
    }
    format!("{prefix}{stem} (copy)")
}

fn is_occupied(candidate: &str, occupied: &BTreeSet<String>, sense: CaseSense) -> bool {
    if occupied.contains(candidate) {
        return true;
    }
    match sense {
        CaseSense::Sensitive => false,
        CaseSense::CaseInsensitive => {
            let folded = fold(candidate, sense);
            occupied.iter().any(|path| fold(path, sense) == folded)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试里的默认口径：Windows 规则（本模块的历史期望都按它写）。
    /// POSIX 规则另有专门用例，不靠"顺手换个参数"覆盖。
    fn scan_windows(
        entries: &[(String, String)],
        existing: &BTreeSet<String>,
        sense: CaseSense,
    ) -> ConflictScan {
        scan_conflicts(entries, existing, sense, NameRules::windows())
    }

    fn pair(source: &str, target: &str) -> (String, String) {
        (source.to_string(), target.to_string())
    }

    fn existing(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| (*path).to_string()).collect()
    }

    #[test]
    fn detects_conflicts_with_existing_files() {
        let entries = [pair("/src/a.txt", "/out/a.txt")];
        let scan = scan_windows(&entries, &existing(&["/out/a.txt"]), CaseSense::Sensitive);
        assert_eq!(scan.conflicts.get(&0), Some(&ConflictKind::Existing));
    }

    #[test]
    fn detects_case_only_conflicts_only_on_case_insensitive_hosts() {
        let entries = [pair("/src/A.txt", "/out/a.txt")];
        let scan = scan_windows(
            &entries,
            &existing(&["/out/A.txt"]),
            CaseSense::CaseInsensitive,
        );
        assert_eq!(scan.conflicts.get(&0), Some(&ConflictKind::CaseOnly));
        let scan = scan_windows(&entries, &existing(&["/out/A.txt"]), CaseSense::Sensitive);
        assert!(scan.conflicts.is_empty(), "Linux keeps names distinct");
    }

    #[test]
    fn detects_within_batch_duplicates_and_case_variants() {
        let entries = [
            pair("/src/a.txt", "/out/x.txt"),
            pair("/src/b.txt", "/out/x.txt"),
            pair("/src/c.txt", "/out/X.txt"),
        ];
        let scan = scan_windows(&entries, &existing(&[]), CaseSense::CaseInsensitive);
        assert_eq!(scan.conflicts.get(&1), Some(&ConflictKind::WithinBatch));
        assert_eq!(scan.conflicts.get(&2), Some(&ConflictKind::CaseOnly));
        let scan = scan_windows(&entries, &existing(&[]), CaseSense::Sensitive);
        assert_eq!(scan.conflicts.get(&1), Some(&ConflictKind::WithinBatch));
        assert!(
            !scan.conflicts.contains_key(&2),
            "different case is a distinct name on Linux"
        );
    }

    #[test]
    fn existing_wins_over_batch_classification() {
        let entries = [
            pair("/src/a.txt", "/out/x.txt"),
            pair("/src/b.txt", "/out/x.txt"),
        ];
        let scan = scan_windows(&entries, &existing(&["/out/x.txt"]), CaseSense::Sensitive);
        assert_eq!(scan.conflicts.get(&0), Some(&ConflictKind::Existing));
        assert_eq!(scan.conflicts.get(&1), Some(&ConflictKind::Existing));
    }

    #[test]
    fn reports_name_warnings_for_platform_rules() {
        let entries = [
            pair("/src/a", "/out/CON.txt"),
            pair("/src/b", "/out/bad:name.txt"),
            pair("/src/c", "/out/trailing."),
            pair("/src/d", "/out/"),
            pair("/src/e", "/out/trailing "),
        ];
        let scan = scan_windows(&entries, &existing(&[]), CaseSense::Sensitive);
        let kinds = |index: usize| -> Vec<String> {
            scan.warnings
                .get(&index)
                .map(|warnings| warnings.iter().map(|w| w.kind.clone()).collect())
                .unwrap_or_default()
        };
        assert!(kinds(0).contains(&"reserved-name".to_string()));
        assert!(kinds(1).contains(&"invalid-char".to_string()));
        assert!(kinds(2).contains(&"trailing-dot-space".to_string()));
        assert!(kinds(3).contains(&"empty-name".to_string()));
        assert!(kinds(4).contains(&"trailing-dot-space".to_string()));
        let suggestion = scan.warnings[&1][0].suggestion.clone().unwrap();
        assert!(suggestion.contains("bad_name"));
    }

    #[test]
    fn sequence_target_skips_occupied_names() {
        let occupied = existing(&["/out/a.txt", "/out/a (1).txt", "/out/noext"]);
        let candidate = sequence_target("/out/a.txt", &occupied, CaseSense::Sensitive);
        assert_eq!(candidate, "/out/a (2).txt");
        let candidate = sequence_target("/out/noext", &occupied, CaseSense::Sensitive);
        assert_eq!(candidate, "/out/noext (1)");
        let candidate = sequence_target("/out/free.txt", &occupied, CaseSense::Sensitive);
        assert_eq!(candidate, "/out/free.txt", "a free name is used as-is");
    }

    #[test]
    fn sequence_target_respects_case_insensitive_occupancy() {
        let occupied = existing(&["/out/A.TXT"]);
        let candidate = sequence_target("/out/a.txt", &occupied, CaseSense::CaseInsensitive);
        assert_eq!(candidate, "/out/a (1).txt");
        let candidate = sequence_target("/out/a.txt", &occupied, CaseSense::Sensitive);
        assert_eq!(candidate, "/out/a.txt");
    }

    #[test]
    fn conflict_kinds_map_to_contract_strings_and_risk() {
        assert_eq!(ConflictKind::Existing.as_str(), "existing");
        assert_eq!(ConflictKind::WithinBatch.as_str(), "within-batch");
        assert_eq!(ConflictKind::CaseOnly.as_str(), "case-only");
        assert_eq!(ConflictKind::Existing.risk_level(), RiskLevel::Danger);
        assert_eq!(ConflictKind::CaseOnly.risk_level(), RiskLevel::Caution);
    }

    #[test]
    fn file_name_of_handles_both_separators() {
        assert_eq!(file_name_of("/out/a.txt"), "a.txt");
        assert_eq!(file_name_of(r"C:\out\a.txt"), "a.txt");
        assert_eq!(file_name_of("a.txt"), "a.txt");
        assert_eq!(file_name_of("/out/"), "");
    }

    /// POSIX 上 `CON.txt`、`报告:2026.csv`、`trailing.`、`trailing ` 全是**合法**文件名。
    /// 按 Windows 规则拦它们，就是 Linux 上"重命名一整批全部被跳过"的真实故障。
    #[test]
    fn posix_rules_accept_names_only_windows_forbids() {
        for name in [
            "CON.txt",
            "con",
            "PRN",
            "报告:2026.csv",
            "a<b>.txt",
            "trailing.",
            "trailing ",
        ] {
            assert!(
                validate_target_name(name, NameRules::posix()).is_empty(),
                "{name} is a legal POSIX file name"
            );
            assert!(
                !validate_target_name(name, NameRules::windows()).is_empty(),
                "{name} must still trip the Windows rules"
            );
        }
    }

    /// POSIX 只禁 `/` 与 NUL；ASCII 控制字符（除 NUL）在 ext4/APFS 上是合法名。
    #[test]
    fn posix_rules_reject_only_slash_and_nul() {
        let kinds = |name: &str| -> Vec<String> {
            validate_target_name(name, NameRules::posix())
                .into_iter()
                .map(|warning| warning.kind)
                .collect()
        };
        assert_eq!(kinds("a/b.txt"), vec!["invalid-char".to_string()]);
        assert_eq!(kinds("a\0b.txt"), vec!["invalid-char".to_string()]);
        assert!(
            kinds("a\u{1}b.txt").is_empty(),
            "only NUL is illegal, not every control char"
        );
        let suggestion = validate_target_name("a/b.txt", NameRules::posix())[0]
            .suggestion
            .clone()
            .unwrap();
        assert_eq!(suggestion, "a_b.txt");
    }

    /// 空名与"纯空白"名对所有平台都无意义：这条是产品级拒绝，不是平台规则。
    #[test]
    fn empty_names_are_rejected_under_both_rule_sets() {
        for rules in [NameRules::windows(), NameRules::posix()] {
            assert_eq!(validate_target_name("", rules)[0].kind, "empty-name");
            assert_eq!(validate_target_name("   ", rules)[0].kind, "empty-name");
        }
    }

    #[test]
    fn scan_conflicts_is_silent_about_posix_legal_names() {
        let entries = [
            pair("/src/a", "/out/CON.txt"),
            pair("/src/b", "/out/bad:name.txt"),
            pair("/src/c", "/out/trailing."),
        ];
        let scan = scan_conflicts(
            &entries,
            &existing(&[]),
            CaseSense::Sensitive,
            NameRules::posix(),
        );
        assert!(
            scan.warnings.is_empty(),
            "no false positives on POSIX hosts"
        );
        let scan = scan_windows(&entries, &existing(&[]), CaseSense::Sensitive);
        assert_eq!(
            scan.warnings.len(),
            3,
            "the same names still trip Windows rules"
        );
    }
}
