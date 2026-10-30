//! Filesystem port (design 01 §3, design 04 §4).
//!
//! Kernel code never touches the filesystem directly; every path operation goes
//! through this trait. Implementations live in the shell (desktop) or the Web
//! adapter.

use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseSense {
    #[default]
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
    /// 把原文件内容备份进隔离目录（复制、不移动），返回备份路径。
    ///
    /// 设计 01 §8：覆盖类动作（overwrite 的 copy/内容写回）在落盘前先备份
    /// 原内容，Journal 的 `dataRef` 指向备份路径，撤销据此还原。
    /// 适配层不支持时返回 E5001（调用方按契约拒绝该动作，不静默降级）。
    fn backup_to_quarantine(&self, path: &str) -> Result<String, KernelError> {
        let _ = path;
        Err(KernelError::new(
            "E5001",
            "quarantine backup is not supported by this adapter",
            None,
            false,
        ))
    }
    /// 在隔离目录内为目标文件名预留一个不存在的新路径（不创建文件）。
    ///
    /// 设计 01 §8：新建产物（无原始内容可备份）的撤销语义是"产物进隔离目录"，
    /// Journal 的 reverse 为 `Quarantine{from: 产物, to: 此处返回的路径}`；
    /// 路径由适配层在 apply 期生成并如实记入 Journal（执行期不得现场发明）。
    fn quarantine_target_path(&self, file_name: &str) -> Result<String, KernelError> {
        let _ = file_name;
        Err(KernelError::new(
            "E5001",
            "quarantine path reservation is not supported by this adapter",
            None,
            false,
        ))
    }
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
