//! C09 duplicate grouping, adapted from fdupes v2.4.0.
//!
//! Source: <https://github.com/adrianlopezroche/fdupes> tag v2.4.0,
//! commit 5e86f87042b6472a4871881435fae1cc438f12d0 (MIT).
//! Original functions consulted: `checkmatch`, `registerpair`,
//! `is_same_file`, `has_same_file`; this is a pure-function rewrite.
//! Full license text: `THIRD_PARTY_NOTICES.md`.
//!
//! This module performs no I/O, clock, environment, logging or deletion.

use std::collections::{BTreeMap, BTreeSet};

/// Opaque file identity supplied by the FsAdapter.
///
/// Windows: volume serial + file ID; Unix: device + inode. The kernel only
/// compares values, it never obtains them itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileIdentity {
    volume: u64,
    file: u128,
}

impl FileIdentity {
    /// Windows: volume serial + file ID; Unix: device + inode.
    pub const fn new(volume: u64, file: u128) -> Self {
        Self { volume, file }
    }
}

/// One file's already-computed metadata and digests.
///
/// Hash values are supplied by the shell/streaming layer; this module does not
/// hash anything. The digest length is not interpreted, so BLAKE3, SHA-256 and
/// MD5 can use the same grouping function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateCandidate {
    pub id: u64,
    pub size: u64,
    pub quick_hash: Option<Vec<u8>>,
    pub full_hash: Option<Vec<u8>>,
    pub identity: Option<FileIdentity>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupingMode {
    /// Only emit groups confirmed by a full digest.
    #[default]
    Full,
    /// Emit high-probability groups from quick digests only.
    Fast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupingOptions {
    pub mode: GroupingMode,
    pub include_zero_byte: bool,
}

impl Default for GroupingOptions {
    fn default() -> Self {
        Self {
            mode: GroupingMode::Full,
            include_zero_byte: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Confirmed,
    Candidate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub size: u64,
    pub hash: Option<Vec<u8>>,
    pub members: Vec<u64>,
    pub confidence: Confidence,
    /// True when at least one member has no FileIdentity; such a group must
    /// not be auto-selected for cleanup (PDR-001 T5).
    pub needs_manual_review: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DuplicateReport {
    pub groups: Vec<DuplicateGroup>,
    pub zero_byte_ids: Vec<u64>,
    pub hardlink_skipped: Vec<u64>,
    /// Candidates that lack the digest required by the selected mode.
    pub incomplete: Vec<u64>,
}

/// Group duplicate candidates purely.
///
/// The algorithm follows the project's three stages: size, quick digest, full
/// digest. The full stage is the authority; fast mode intentionally returns
/// `Confidence::Candidate` and must not be used to clean files.
pub fn group_duplicates(
    candidates: &[DuplicateCandidate],
    options: GroupingOptions,
) -> DuplicateReport {
    let mut report = DuplicateReport::default();
    let mut by_size: BTreeMap<u64, Vec<&DuplicateCandidate>> = BTreeMap::new();
    for candidate in candidates {
        by_size.entry(candidate.size).or_default().push(candidate);
    }

    for (size, mut group) in by_size {
        if group.len() < 2 {
            continue;
        }
        group.sort_unstable_by_key(|candidate| candidate.id);

        if size == 0 {
            if options.include_zero_byte {
                let members: Vec<u64> = group.iter().map(|candidate| candidate.id).collect();
                let needs_manual_review =
                    group.iter().any(|candidate| candidate.identity.is_none());
                report.groups.push(DuplicateGroup {
                    size,
                    hash: None,
                    members,
                    confidence: Confidence::Confirmed,
                    needs_manual_review,
                });
            } else {
                report
                    .zero_byte_ids
                    .extend(group.iter().map(|candidate| candidate.id));
            }
            continue;
        }

        let mut kept = Vec::with_capacity(group.len());
        let mut seen_identities = BTreeSet::new();
        for candidate in group {
            if let Some(identity) = candidate.identity {
                if !seen_identities.insert(identity) {
                    report.hardlink_skipped.push(candidate.id);
                    continue;
                }
            }
            kept.push(candidate);
        }
        if kept.len() < 2 {
            continue;
        }

        let identity_unknown: BTreeSet<u64> = kept
            .iter()
            .filter(|candidate| candidate.identity.is_none())
            .map(|candidate| candidate.id)
            .collect();

        let mut by_hash: BTreeMap<Vec<u8>, Vec<u64>> = BTreeMap::new();
        for candidate in kept {
            let digest = match options.mode {
                GroupingMode::Full => candidate.full_hash.as_ref(),
                GroupingMode::Fast => candidate.quick_hash.as_ref(),
            };
            match digest {
                Some(hash) => by_hash.entry(hash.clone()).or_default().push(candidate.id),
                None => report.incomplete.push(candidate.id),
            }
        }

        for (hash, mut members) in by_hash {
            if members.len() < 2 {
                continue;
            }
            members.sort_unstable();
            let needs_manual_review = members.iter().any(|id| identity_unknown.contains(id));
            report.groups.push(DuplicateGroup {
                size,
                hash: Some(hash),
                members,
                confidence: match options.mode {
                    GroupingMode::Full => Confidence::Confirmed,
                    GroupingMode::Fast => Confidence::Candidate,
                },
                needs_manual_review,
            });
        }
    }

    report.zero_byte_ids.sort_unstable();
    report.hardlink_skipped.sort_unstable();
    report.incomplete.sort_unstable();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> Vec<u8> {
        vec![byte; 32]
    }

    fn candidate(
        id: u64,
        size: u64,
        quick: Option<u8>,
        full: Option<u8>,
        identity: Option<u128>,
    ) -> DuplicateCandidate {
        DuplicateCandidate {
            id,
            size,
            quick_hash: quick.map(digest),
            full_hash: full.map(digest),
            identity: identity.map(|id| FileIdentity::new(0, id)),
        }
    }

    #[test]
    fn groups_identical_content() {
        let candidates = vec![
            candidate(2, 10, Some(1), Some(1), Some(2)),
            candidate(1, 10, Some(1), Some(1), Some(1)),
        ];
        let report = group_duplicates(&candidates, GroupingOptions::default());
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].members, vec![1, 2]);
        assert_eq!(report.groups[0].confidence, Confidence::Confirmed);
        assert!(!report.groups[0].needs_manual_review);
    }

    #[test]
    fn different_size_or_full_hash_does_not_group() {
        let candidates = vec![
            candidate(1, 10, Some(1), Some(1), Some(1)),
            candidate(2, 11, Some(1), Some(1), Some(2)),
            candidate(3, 10, Some(2), Some(2), Some(3)),
        ];
        let report = group_duplicates(&candidates, GroupingOptions::default());
        assert!(report.groups.is_empty());
    }

    #[test]
    fn fast_mode_groups_by_quick_hash_as_candidate() {
        let candidates = vec![
            candidate(1, 10, Some(1), Some(1), None),
            candidate(2, 10, Some(1), Some(2), None),
        ];
        let report = group_duplicates(
            &candidates,
            GroupingOptions {
                mode: GroupingMode::Fast,
                include_zero_byte: false,
            },
        );
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].confidence, Confidence::Candidate);
        assert!(report.groups[0].needs_manual_review);
    }

    #[test]
    fn full_mode_requires_full_hash() {
        let candidates = vec![
            candidate(1, 10, Some(1), None, Some(1)),
            candidate(2, 10, Some(1), None, Some(2)),
        ];
        let report = group_duplicates(&candidates, GroupingOptions::default());
        assert!(report.groups.is_empty());
        assert_eq!(report.incomplete, vec![1, 2]);
    }

    #[test]
    fn fast_mode_requires_quick_hash() {
        let candidates = vec![
            candidate(1, 10, None, Some(1), Some(1)),
            candidate(2, 10, None, Some(1), Some(2)),
        ];
        let report = group_duplicates(
            &candidates,
            GroupingOptions {
                mode: GroupingMode::Fast,
                include_zero_byte: false,
            },
        );
        assert!(report.groups.is_empty());
        assert_eq!(report.incomplete, vec![1, 2]);
    }

    #[test]
    fn hardlinks_are_skipped() {
        let candidates = vec![
            candidate(1, 10, Some(1), Some(1), Some(7)),
            candidate(2, 10, Some(1), Some(1), Some(7)),
            candidate(3, 10, Some(1), Some(1), Some(8)),
        ];
        let report = group_duplicates(&candidates, GroupingOptions::default());
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].members, vec![1, 3]);
        assert_eq!(report.hardlink_skipped, vec![2]);
    }

    #[test]
    fn zero_byte_default_is_ignored_and_optional_grouping() {
        let candidates = vec![
            candidate(2, 0, Some(1), Some(1), Some(2)),
            candidate(1, 0, Some(1), Some(1), Some(1)),
        ];
        let ignored = group_duplicates(&candidates, GroupingOptions::default());
        assert!(ignored.groups.is_empty());
        assert_eq!(ignored.zero_byte_ids, vec![1, 2]);

        let included = group_duplicates(
            &candidates,
            GroupingOptions {
                mode: GroupingMode::Full,
                include_zero_byte: true,
            },
        );
        assert_eq!(included.groups.len(), 1);
        assert_eq!(included.groups[0].members, vec![1, 2]);
    }

    #[test]
    fn missing_identity_flags_manual_review() {
        let candidates = vec![
            candidate(1, 10, Some(1), Some(1), None),
            candidate(2, 10, Some(1), Some(1), Some(2)),
        ];
        let report = group_duplicates(&candidates, GroupingOptions::default());
        assert_eq!(report.groups.len(), 1);
        assert!(report.groups[0].needs_manual_review);
    }

    #[test]
    fn single_candidate_does_not_group() {
        let report = group_duplicates(
            &[candidate(1, 10, Some(1), Some(1), Some(1))],
            GroupingOptions::default(),
        );
        assert!(report.groups.is_empty());
    }
}
