//! Filesystem port (design 01 §3, design 04 §4).
//!
//! Kernel code never touches the filesystem directly; every path operation goes
//! through this trait. Implementations live in the shell (desktop) or the Web
//! adapter.

use crate::error::KernelError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    Skip,
    Sequence,
    Overwrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOutcomeKind {
    Applied,
    Skipped,
    Replaced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathOutcome {
    pub outcome: PathOutcomeKind,
    pub final_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanFilter {
    pub extensions: Vec<String>,
    pub include_hidden: bool,
    pub recursive: bool,
    pub max_depth: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryAttrs {
    pub readonly: bool,
    pub hidden: bool,
    pub placeholder: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub path: String,
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime: String,
    pub attrs: EntryAttrs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatInfo {
    pub kind: EntryKind,
    pub size: u64,
    pub mtime: String,
    pub attrs: EntryAttrs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseSense {
    Sensitive,
    CaseInsensitive,
}

/// Filesystem operations used by apply and undo.
///
/// The quarantine restore is expressed as `rename(quarantine_path, original,
/// ConflictPolicy::Skip)` so the frozen `FsOp::Rename` journal reverse can be
/// replayed without another operation variant.
pub trait FsAdapter: Send + Sync {
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError>;
    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), KernelError>;
    fn rename(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError>;
    fn copy(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError>;
    fn move_path(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError>;
    fn remove_to_quarantine(&self, path: &str) -> Result<String, KernelError>;
    fn list_dir(&self, path: &str, filter: &ScanFilter) -> Result<Vec<DirEntryInfo>, KernelError>;
    fn stat(&self, path: &str) -> Result<StatInfo, KernelError>;
    fn canonicalize(&self, path: &str) -> Result<String, KernelError>;
    fn case_sensitivity(&self) -> CaseSense;
    /// Optional content digest used for plan→apply revalidation.
    ///
    /// Returns `Ok(None)` when the adapter cannot hash with the requested
    /// algorithm; callers must then fall back to the size check.
    fn content_hash(&self, _path: &str, _algorithm: &str) -> Result<Option<String>, KernelError> {
        Ok(None)
    }
}
