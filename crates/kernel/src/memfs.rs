//! 会话内内存文件系统：`FsAdapter` 的第三种实现（ADR-0108）。
//!
//! 存在理由：Web 形态的执行器必须**同步**读写，而浏览器能给的只有异步的
//! File API／OPFS；用户选中的源文件也不在 OPFS 里。于是执行前先把输入字节
//! **登记**进一个会话沙箱，执行器对沙箱跑真实的 `generic_apply`／`undo`，
//! 产物再交回主线程下载。本模块就是那个沙箱的存储后端。
//!
//! 边界（写在这里避免误用）：
//! - 它**不碰用户磁盘**，也不读时钟——`mtime` 是固定值，只有大小参与
//!   plan→apply 复核；需要真实时间戳的调用方应由适配层另给；
//! - 路径先按 `path::try_normalize` 归一（反斜杠、`.`/`..`），因此同一份
//!   计划里的 `out\a.txt` 与 `/out/a.txt` 是同一个对象；
//! - 语义与桌面 `CliFsAdapter` 对齐：写目标自动建父目录、目标冲突按
//!   `ConflictPolicy` 处理、删除一律进隔离目录、`create_dir` 只建单级。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::sync::Mutex;

use crate::error::KernelError;
use crate::fs::{
    CaseSense, ConflictPolicy, DirEntryInfo, EntryAttrs, EntryKind, FsAdapter, PathOutcome,
    PathOutcomeKind, RewriteCallback, ScanFilter, StatInfo,
};

/// 沙箱内固定时间戳：内存文件系统没有真实 mtime，也不允许读时钟（不变量 1 的
/// 同一口径）。计划→执行复核只看大小；界面若显示 mtime，须由调用方另行标注。
pub const SANDBOX_MTIME: &str = "1970-01-01T00:00:00Z";

/// 隔离目录根（不变量 7：删除与覆盖备份都落在这里）。
pub const QUARANTINE_ROOT: &str = "/.batchdesk-quarantine";

#[derive(Default)]
struct Inner {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    next_id: u64,
}

/// 会话内内存文件系统。
pub struct MemoryFs {
    inner: Mutex<Inner>,
    case_sense: CaseSense,
}

impl Default for MemoryFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryFs {
    pub fn new() -> Self {
        let mut dirs = BTreeSet::new();
        dirs.insert("/".to_string());
        dirs.insert(QUARANTINE_ROOT.to_string());
        Self {
            inner: Mutex::new(Inner {
                files: BTreeMap::new(),
                dirs,
                next_id: 1,
            }),
            case_sense: CaseSense::Sensitive,
        }
    }

    /// 大小写能力由调用方声明（Web 侧一律 `Sensitive`：沙箱是新建的键空间，
    /// 不继承宿主平台的折叠规则）。
    pub fn with_case_sense(mut self, case_sense: CaseSense) -> Self {
        self.case_sense = case_sense;
        self
    }

    /// 登记一份输入字节（路径按计划里出现的原样给，内部再归一）。
    ///
    /// 父目录自动补齐——登记的输入是「执行时已经存在于源位置的文件」，
    /// 不是新建动作，不需要调用方先建目录。
    pub fn insert_file(&self, path: &str, data: Vec<u8>) -> Result<(), KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        if inner.dirs.contains(&key) {
            return Err(KernelError::new(
                "E2007",
                "a directory with the same path already exists",
                Some(path.to_string()),
                false,
            ));
        }
        ensure_parents(&mut inner, &key);
        inner.files.insert(key, data);
        Ok(())
    }

    /// 只登记一个**目录**（不登记内容）。
    ///
    /// 为什么需要它：F10 目录清单的执行臂先 `stat(源)` 确认「源是目录」，而浏览器的
    /// 目录事实由前端采集、不经 `insert_file`（没有字节）。目录不建出来，预检会报
    /// 「源不存在」。语义与 `create_dir_all` 的区别是**幂等**：staging 会重复登记
    /// 同一批目录（多根目录共享父级），重复建不该失败。
    pub fn insert_dir(&self, path: &str) -> Result<(), KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        if inner.files.contains_key(&key) {
            return Err(KernelError::new(
                "E2007",
                "a file with the same path already exists",
                Some(path.to_string()),
                false,
            ));
        }
        ensure_parents(&mut inner, &key);
        inner.dirs.insert(key);
        Ok(())
    }

    /// 读出沙箱内文件的当前内容（产物下载用；不存在报 E2001）。
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, KernelError> {
        let key = key(path);
        self.lock()
            .files
            .get(&key)
            .cloned()
            .ok_or_else(|| not_found(path))
    }

    /// 列出沙箱内全部文件的规范化路径（发现「产物」与「被改写的输入」用）。
    pub fn file_paths(&self) -> Vec<String> {
        self.lock().files.keys().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn not_found(path: &str) -> KernelError {
    KernelError::new(
        "E2001",
        "file or directory does not exist",
        Some(path.to_string()),
        false,
    )
}

/// 归一后的键：反斜杠、相对路径、`.`/`..` 都收口到同一个绝对形态。
///
/// 对外公开（`normalize_key`）：适配层要对沙箱路径做前缀匹配时必须是同一口径，
/// 自己拼一遍就等于另立一套。
pub fn normalize_key(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    let absolute = if slashed.starts_with('/') {
        slashed
    } else {
        format!("/{slashed}")
    };
    crate::path::try_normalize(&absolute).unwrap_or(absolute)
}

fn key(path: &str) -> String {
    normalize_key(path)
}

fn parent_of(key: &str) -> Option<&str> {
    if key == "/" {
        return None;
    }
    match key.rfind('/') {
        Some(0) => Some("/"),
        Some(index) => Some(&key[..index]),
        None => None,
    }
}

fn ensure_parents(inner: &mut Inner, key: &str) {
    let mut current = parent_of(key).map(str::to_string);
    while let Some(dir) = current {
        inner.dirs.insert(dir.clone());
        current = parent_of(&dir).map(str::to_string);
    }
}

fn file_name_of(path: &str) -> String {
    key(path).rsplit('/').next().unwrap_or("file").to_string()
}

/// 与桌面适配层同式的序号目标：`name (1).ext`。
fn sequence_target(to: &str, inner: &Inner) -> String {
    let (parent, name) = match to.rfind('/') {
        Some(index) => (&to[..index], &to[index + 1..]),
        None => ("", to),
    };
    let (stem, extension) = match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], Some(&name[index + 1..])),
        _ => (name, None),
    };
    for index in 1u64.. {
        let candidate = match extension {
            Some(extension) => format!("{parent}/{stem} ({index}).{extension}"),
            None => format!("{parent}/{stem} ({index})"),
        };
        if !inner.files.contains_key(&candidate) && !inner.dirs.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("sequence search is unbounded")
}

fn resolve_conflict(
    inner: &mut Inner,
    to: &str,
    policy: ConflictPolicy,
) -> Option<PathOutcomeKind> {
    let occupied = inner.files.contains_key(to) || inner.dirs.contains(to);
    if !occupied {
        return None;
    }
    match policy {
        ConflictPolicy::Skip => Some(PathOutcomeKind::Skipped),
        ConflictPolicy::Overwrite => Some(PathOutcomeKind::Replaced),
        ConflictPolicy::Sequence => None,
    }
}

struct Sink(Vec<u8>);

impl Write for Sink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl FsAdapter for MemoryFs {
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError> {
        let key = key(path);
        self.lock()
            .files
            .get(&key)
            .cloned()
            .ok_or_else(|| not_found(path))
    }

    fn read_stream(
        &self,
        path: &str,
        read: &mut dyn FnMut(&[u8]) -> Result<(), KernelError>,
    ) -> Result<(), KernelError> {
        let data = self.read_bytes(path)?;
        const CHUNK: usize = 64 * 1024;
        for chunk in data.chunks(CHUNK) {
            read(chunk)?;
        }
        Ok(())
    }

    fn open_read_stream(&self, path: &str) -> Result<Box<dyn Read + Send>, KernelError> {
        Ok(Box::new(Cursor::new(self.read_bytes(path)?)))
    }

    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        if inner.dirs.contains(&key) {
            return Err(KernelError::new(
                "E2007",
                "target path is a directory",
                Some(path.to_string()),
                false,
            ));
        }
        ensure_parents(&mut inner, &key);
        inner.files.insert(key, data.to_vec());
        Ok(())
    }

    fn create_dir(&self, path: &str) -> Result<(), KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        if inner.files.contains_key(&key) {
            return Err(KernelError::new(
                "E2007",
                "a file with the same path already exists",
                Some(path.to_string()),
                false,
            ));
        }
        if !inner.dirs.insert(key) {
            return Err(KernelError::new(
                "E2007",
                "directory already exists",
                Some(path.to_string()),
                false,
            ));
        }
        Ok(())
    }

    fn create_dir_all(&self, path: &str) -> Result<(), KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        let mut current = Some(key);
        while let Some(dir) = current {
            if inner.files.contains_key(&dir) {
                return Err(KernelError::new(
                    "E2007",
                    "a file blocks the directory path",
                    Some(dir),
                    false,
                ));
            }
            inner.dirs.insert(dir.clone());
            current = parent_of(&dir).map(str::to_string);
        }
        Ok(())
    }

    fn remove_dir_if_empty(&self, path: &str) -> Result<(), KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        if !inner.dirs.contains(&key) {
            return Err(not_found(path));
        }
        let prefix = format!("{key}/");
        let occupied = inner.files.keys().any(|file| file.starts_with(&prefix))
            || inner.dirs.iter().any(|dir| dir.starts_with(&prefix));
        if occupied {
            return Err(KernelError::new(
                "E2007",
                "directory is not empty",
                Some(path.to_string()),
                false,
            ));
        }
        inner.dirs.remove(&key);
        Ok(())
    }

    fn write_atomic_with(
        &self,
        path: &str,
        write: &mut dyn FnMut(&mut dyn Write) -> Result<(), KernelError>,
    ) -> Result<(), KernelError> {
        // 先写进临时缓冲，回调成功才提交——与桌面「临时文件 + 原子改名」同语义。
        let mut sink = Sink(Vec::new());
        write(&mut sink)?;
        self.write_atomic(path, &sink.0)
    }

    fn rewrite_atomic_with(
        &self,
        path: &str,
        rewrite: &mut RewriteCallback<'_>,
    ) -> Result<(), KernelError> {
        let original = self.read_bytes(path)?;
        let mut sink = Sink(Vec::new());
        let mut reader = Cursor::new(original);
        rewrite(&mut reader, &mut sink)?;
        self.write_atomic(path, &sink.0)
    }

    fn rename(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        let from_key = key(from);
        let mut to_key = key(to);
        let mut inner = self.lock();

        if from_key == to_key {
            if inner.files.contains_key(&from_key) || inner.dirs.contains(&from_key) {
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Applied,
                    final_path: from_key,
                });
            }
            return Err(not_found(from));
        }
        if inner.files.contains_key(&from_key) {
            let data = inner
                .files
                .remove(&from_key)
                .ok_or_else(|| not_found(from))?;
            if let Some(kind) = resolve_conflict(&mut inner, &to_key, policy) {
                if kind == PathOutcomeKind::Skipped {
                    inner.files.insert(from_key, data);
                    return Ok(PathOutcome {
                        outcome: PathOutcomeKind::Skipped,
                        final_path: to_key,
                    });
                }
            } else if inner.files.contains_key(&to_key) || inner.dirs.contains(&to_key) {
                to_key = sequence_target(&to_key, &inner);
            }
            ensure_parents(&mut inner, &to_key);
            let replaced = inner.files.insert(to_key.clone(), data).is_some();
            return Ok(PathOutcome {
                outcome: if replaced {
                    PathOutcomeKind::Replaced
                } else {
                    PathOutcomeKind::Applied
                },
                final_path: to_key,
            });
        }
        if inner.dirs.contains(&from_key) {
            if let Some(kind) = resolve_conflict(&mut inner, &to_key, policy) {
                if kind == PathOutcomeKind::Skipped {
                    return Ok(PathOutcome {
                        outcome: PathOutcomeKind::Skipped,
                        final_path: to_key,
                    });
                }
            } else if inner.files.contains_key(&to_key) || inner.dirs.contains(&to_key) {
                to_key = sequence_target(&to_key, &inner);
            }
            let prefix = format!("{from_key}/");
            let files: Vec<(String, Vec<u8>)> = inner
                .files
                .iter()
                .filter(|(path, _)| path.starts_with(&prefix))
                .map(|(path, data)| (path.clone(), data.clone()))
                .collect();
            for (path, data) in files {
                let suffix = &path[prefix.len()..];
                inner.files.remove(&path);
                inner.files.insert(format!("{to_key}/{suffix}"), data);
            }
            let dirs: Vec<String> = inner
                .dirs
                .iter()
                .filter(|path| path.starts_with(&prefix))
                .cloned()
                .collect();
            for dir in dirs {
                let suffix = &dir[prefix.len()..];
                inner.dirs.remove(&dir);
                inner.dirs.insert(format!("{to_key}/{suffix}"));
            }
            inner.dirs.remove(&from_key);
            inner.dirs.insert(to_key.clone());
            return Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: to_key,
            });
        }
        Err(not_found(from))
    }

    fn copy(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        let from_key = key(from);
        let mut to_key = key(to);
        let data = self.read_bytes(&from_key).map_err(|_| not_found(from))?;
        let mut inner = self.lock();
        if let Some(kind) = resolve_conflict(&mut inner, &to_key, policy) {
            if kind == PathOutcomeKind::Skipped {
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Skipped,
                    final_path: to_key,
                });
            }
        } else if inner.files.contains_key(&to_key) || inner.dirs.contains(&to_key) {
            to_key = sequence_target(&to_key, &inner);
        }
        ensure_parents(&mut inner, &to_key);
        let replaced = inner.files.insert(to_key.clone(), data).is_some();
        Ok(PathOutcome {
            outcome: if replaced {
                PathOutcomeKind::Replaced
            } else {
                PathOutcomeKind::Applied
            },
            final_path: to_key,
        })
    }

    fn move_path(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        self.rename(from, to, policy)
    }

    fn remove_to_quarantine(&self, path: &str) -> Result<String, KernelError> {
        let key = key(path);
        let mut inner = self.lock();
        let data = inner.files.remove(&key).ok_or_else(|| not_found(path))?;
        inner.next_id += 1;
        let destination = format!("{QUARANTINE_ROOT}/{}-{}", inner.next_id, file_name_of(&key));
        inner.files.insert(destination.clone(), data);
        Ok(destination)
    }

    fn backup_to_quarantine(&self, path: &str) -> Result<String, KernelError> {
        let data = self.read_bytes(path)?;
        let mut inner = self.lock();
        inner.next_id += 1;
        let destination = format!(
            "{QUARANTINE_ROOT}/backup-{}-{}",
            inner.next_id,
            file_name_of(path)
        );
        inner.files.insert(destination.clone(), data);
        Ok(destination)
    }

    fn quarantine_target_path(&self, file_name: &str) -> Result<String, KernelError> {
        let mut inner = self.lock();
        inner.next_id += 1;
        Ok(format!(
            "{QUARANTINE_ROOT}/{}-{}",
            inner.next_id,
            file_name.trim()
        ))
    }

    fn same_volume(&self, _a: &str, _b: &str) -> bool {
        true
    }

    fn remove_file(&self, path: &str) -> Result<(), KernelError> {
        let key = key(path);
        self.lock()
            .files
            .remove(&key)
            .map(|_| ())
            .ok_or_else(|| not_found(path))
    }

    fn copy_range_atomic(
        &self,
        source: &str,
        start: u64,
        end: u64,
        target: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        if start >= end {
            return Err(KernelError::new(
                "E1002",
                "split range must be non-empty",
                None,
                false,
            ));
        }
        let data = self.read_bytes(source)?;
        if end > data.len() as u64 {
            return Err(KernelError::new(
                "E2010",
                "source file shrank since planning; re-plan before applying",
                Some(source.to_string()),
                true,
            ));
        }
        let slice = data[start as usize..end as usize].to_vec();
        self.write_with_policy(target, slice, policy)
    }

    fn copy_ranges_atomic(
        &self,
        source: &str,
        ranges: &[(u64, u64)],
        prefix: &[u8],
        target: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        if ranges.is_empty() {
            return Err(KernelError::new(
                "E1002",
                "range copy requires at least one byte range",
                None,
                false,
            ));
        }
        let data = self.read_bytes(source)?;
        let mut joined = prefix.to_vec();
        for (start, end) in ranges {
            if start >= end || *end > data.len() as u64 {
                return Err(KernelError::new(
                    "E2010",
                    "source file shrank since planning; re-plan before applying",
                    Some(source.to_string()),
                    true,
                ));
            }
            joined.extend_from_slice(&data[*start as usize..*end as usize]);
        }
        self.write_with_policy(target, joined, policy)
    }

    fn read_ranges(&self, source: &str, ranges: &[(u64, u64)]) -> Result<Vec<u8>, KernelError> {
        let data = self.read_bytes(source)?;
        let mut joined = Vec::new();
        for (start, end) in ranges {
            if start >= end || *end > data.len() as u64 {
                return Err(out_of_bounds(source));
            }
            joined.extend_from_slice(&data[*start as usize..*end as usize]);
        }
        Ok(joined)
    }

    fn read_ranges_stream(
        &self,
        source: &str,
        ranges: &[(u64, u64)],
        read: &mut dyn FnMut(&[u8]) -> Result<(), KernelError>,
    ) -> Result<(), KernelError> {
        let data = self.read_bytes(source)?;
        for (start, end) in ranges {
            if start >= end || *end > data.len() as u64 {
                return Err(out_of_bounds(source));
            }
            read(&data[*start as usize..*end as usize])?;
        }
        Ok(())
    }

    fn list_dir(&self, path: &str, filter: &ScanFilter) -> Result<Vec<DirEntryInfo>, KernelError> {
        let root = key(path);
        let inner = self.lock();
        if !inner.dirs.contains(&root) {
            return Err(not_found(path));
        }
        let prefix = if root == "/" {
            "/".to_string()
        } else {
            format!("{root}/")
        };
        let mut result = Vec::new();
        for (file, data) in inner.files.iter() {
            if !file.starts_with(&prefix) {
                continue;
            }
            let relative = &file[prefix.len()..];
            let depth = relative.matches('/').count();
            if depth > 0 && !filter.recursive {
                continue;
            }
            if let Some(max_depth) = filter.max_depth {
                if depth > max_depth {
                    continue;
                }
            }
            let name = relative.rsplit('/').next().unwrap_or(relative).to_string();
            if !filter.include_hidden && name.starts_with('.') {
                continue;
            }
            if !filter.extensions.is_empty() {
                let extension = name
                    .rsplit_once('.')
                    .map(|(_, extension)| extension.to_ascii_lowercase())
                    .unwrap_or_default();
                if !filter
                    .extensions
                    .iter()
                    .any(|wanted| wanted.to_ascii_lowercase() == extension)
                {
                    continue;
                }
            }
            result.push(DirEntryInfo {
                path: file.clone(),
                name,
                kind: EntryKind::File,
                size: data.len() as u64,
                mtime: SANDBOX_MTIME.to_string(),
                attrs: EntryAttrs {
                    readonly: false,
                    hidden: false,
                    placeholder: false,
                },
            });
        }
        result.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(result)
    }

    fn stat(&self, path: &str) -> Result<StatInfo, KernelError> {
        let key = key(path);
        let inner = self.lock();
        if let Some(data) = inner.files.get(&key) {
            return Ok(StatInfo {
                kind: EntryKind::File,
                size: data.len() as u64,
                mtime: SANDBOX_MTIME.to_string(),
                attrs: EntryAttrs {
                    readonly: false,
                    hidden: false,
                    placeholder: false,
                },
            });
        }
        if inner.dirs.contains(&key) {
            return Ok(StatInfo {
                kind: EntryKind::Dir,
                size: 0,
                mtime: SANDBOX_MTIME.to_string(),
                attrs: EntryAttrs {
                    readonly: false,
                    hidden: false,
                    placeholder: false,
                },
            });
        }
        Err(not_found(path))
    }

    fn canonicalize(&self, path: &str) -> Result<String, KernelError> {
        Ok(key(path))
    }

    fn case_sensitivity(&self) -> CaseSense {
        self.case_sense
    }

    fn name_rules(&self) -> crate::conflict::NameRules {
        crate::conflict::NameRules::posix()
    }

    /// 沙箱不做内容哈希：调用方按契约退化为大小复核（与「适配层不支持」同义）。
    fn content_hash(&self, _path: &str, _algorithm: &str) -> Result<Option<String>, KernelError> {
        Ok(None)
    }
}

impl MemoryFs {
    fn write_with_policy(
        &self,
        target: &str,
        data: Vec<u8>,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        let mut key = key(target);
        let mut inner = self.lock();
        if let Some(kind) = resolve_conflict(&mut inner, &key, policy) {
            if kind == PathOutcomeKind::Skipped {
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Skipped,
                    final_path: key,
                });
            }
        } else if inner.files.contains_key(&key) || inner.dirs.contains(&key) {
            key = sequence_target(&key, &inner);
        }
        ensure_parents(&mut inner, &key);
        let replaced = inner.files.insert(key.clone(), data).is_some();
        Ok(PathOutcome {
            outcome: if replaced {
                PathOutcomeKind::Replaced
            } else {
                PathOutcomeKind::Applied
            },
            final_path: key,
        })
    }
}

fn out_of_bounds(source: &str) -> KernelError {
    KernelError::new(
        "E2010",
        "range is out of bounds for the source file",
        Some(source.to_string()),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> MemoryFs {
        MemoryFs::new()
    }

    #[test]
    fn write_read_and_stat_share_one_path_namespace() {
        let fs = fs();
        fs.write_atomic("/out/a.txt", b"hello").unwrap();
        assert_eq!(fs.read_bytes("out\\a.txt").unwrap(), b"hello");
        assert_eq!(fs.read_bytes("/out/./a.txt").unwrap(), b"hello");
        let stat = fs.stat("/out/a.txt").unwrap();
        assert_eq!(stat.kind, EntryKind::File);
        assert_eq!(stat.size, 5);
        assert_eq!(fs.stat("/out").unwrap().kind, EntryKind::Dir);
        assert_eq!(fs.stat("/missing").unwrap_err().code, "E2001");
    }

    #[test]
    fn write_atomic_creates_parent_dirs_like_the_desktop_adapter() {
        let fs = fs();
        fs.write_atomic("/a/b/c/d.txt", b"x").unwrap();
        assert_eq!(fs.stat("/a/b/c").unwrap().kind, EntryKind::Dir);
        assert_eq!(fs.read_bytes("/a/b/c/d.txt").unwrap(), b"x");
    }

    #[test]
    fn copy_range_writes_only_the_requested_slice_and_keeps_the_source() {
        let fs = fs();
        fs.insert_file("report.txt", b"aaa\nbbb\nccc\n".to_vec())
            .unwrap();
        let outcome = fs
            .copy_range_atomic(
                "report.txt",
                0,
                8,
                "/parts/report.part1",
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(outcome.outcome, PathOutcomeKind::Applied);
        assert_eq!(fs.read_bytes("/parts/report.part1").unwrap(), b"aaa\nbbb\n");
        assert_eq!(fs.read_bytes("report.txt").unwrap(), b"aaa\nbbb\nccc\n");
    }

    #[test]
    fn copy_ranges_writes_the_prefix_exactly_once() {
        let fs = fs();
        fs.insert_file(
            "/utf16.csv",
            vec![0xFF, 0xFE, b'h', 0, b'\n', 0, b'1', 0, b'\n', 0],
        )
        .unwrap();
        fs.copy_ranges_atomic(
            "/utf16.csv",
            &[(6, 10)],
            &[0xFF, 0xFE],
            "/parts/p1.csv",
            ConflictPolicy::Skip,
        )
        .unwrap();
        assert_eq!(
            fs.read_bytes("/parts/p1.csv").unwrap(),
            vec![0xFF, 0xFE, b'1', 0, b'\n', 0]
        );
    }

    #[test]
    fn conflict_policies_match_the_desktop_contract() {
        let fs = fs();
        fs.write_atomic("/a.txt", b"new").unwrap();
        fs.write_atomic("/b.txt", b"old").unwrap();

        let skipped = fs
            .copy("/a.txt", "/b.txt", ConflictPolicy::Skip)
            .map(|outcome| outcome.outcome)
            .unwrap();
        assert_eq!(skipped, PathOutcomeKind::Skipped);
        assert_eq!(fs.read_bytes("/b.txt").unwrap(), b"old");

        let replaced = fs
            .copy("/a.txt", "/b.txt", ConflictPolicy::Overwrite)
            .unwrap();
        assert_eq!(replaced.outcome, PathOutcomeKind::Replaced);
        assert_eq!(fs.read_bytes("/b.txt").unwrap(), b"new");

        fs.write_atomic("/b.txt", b"old").unwrap();
        let sequenced = fs
            .copy("/a.txt", "/b.txt", ConflictPolicy::Sequence)
            .unwrap();
        assert_eq!(sequenced.outcome, PathOutcomeKind::Applied);
        assert_eq!(sequenced.final_path, "/b (1).txt");
        assert_eq!(fs.read_bytes("/b (1).txt").unwrap(), b"new");
    }

    #[test]
    fn rename_moves_the_whole_subtree_and_preserves_contents() {
        let fs = fs();
        fs.write_atomic("/src/a.txt", b"a").unwrap();
        fs.write_atomic("/src/nested/b.txt", b"b").unwrap();
        let outcome = fs.rename("/src", "/dst", ConflictPolicy::Skip).unwrap();
        assert_eq!(outcome.outcome, PathOutcomeKind::Applied);
        assert_eq!(fs.read_bytes("/dst/a.txt").unwrap(), b"a");
        assert_eq!(fs.read_bytes("/dst/nested/b.txt").unwrap(), b"b");
        assert_eq!(fs.stat("/src").unwrap_err().code, "E2001");
    }

    #[test]
    fn quarantine_keeps_the_removed_bytes_and_can_be_renamed_back() {
        let fs = fs();
        fs.write_atomic("/out/product.txt", b"product").unwrap();
        let quarantined = fs.remove_to_quarantine("/out/product.txt").unwrap();
        assert!(quarantined.starts_with(QUARANTINE_ROOT));
        assert_eq!(fs.read_bytes(&quarantined).unwrap(), b"product");
        assert_eq!(fs.stat("/out/product.txt").unwrap_err().code, "E2001");

        let reserved = fs.quarantine_target_path("x.txt").unwrap();
        assert!(reserved.starts_with(QUARANTINE_ROOT));

        fs.rename(&quarantined, "/out/product.txt", ConflictPolicy::Skip)
            .unwrap();
        assert_eq!(fs.read_bytes("/out/product.txt").unwrap(), b"product");
    }

    #[test]
    fn backup_copies_instead_of_moving() {
        let fs = fs();
        fs.write_atomic("/a.txt", b"v1").unwrap();
        let backup = fs.backup_to_quarantine("/a.txt").unwrap();
        assert_eq!(fs.read_bytes("/a.txt").unwrap(), b"v1");
        assert_eq!(fs.read_bytes(&backup).unwrap(), b"v1");
        fs.write_atomic("/a.txt", b"v2").unwrap();
        // 逆操作 = 用备份内容写回（Journal 的 dataRef 语义）。
        fs.write_atomic("/a.txt", &fs.read_bytes(&backup).unwrap())
            .unwrap();
        assert_eq!(fs.read_bytes("/a.txt").unwrap(), b"v1");
    }

    #[test]
    fn directory_removal_refuses_non_empty_directories() {
        let fs = fs();
        fs.create_dir("/empty").unwrap();
        assert_eq!(
            fs.create_dir("/empty").unwrap_err().code,
            "E2007",
            "重复建目录是冲突"
        );
        fs.remove_dir_if_empty("/empty").unwrap();

        fs.create_dir_all("/a/b/c").unwrap();
        fs.write_atomic("/a/b/c/x.txt", b"x").unwrap();
        assert_eq!(fs.remove_dir_if_empty("/a/b").unwrap_err().code, "E2007");
        fs.remove_file("/a/b/c/x.txt").unwrap();
        fs.remove_dir_if_empty("/a/b/c").unwrap();
    }

    #[test]
    fn insert_dir_is_idempotent_and_blocks_files() {
        let fs = fs();
        // 幂等：staging 会重复登记同一批目录（多根目录共享父级）。
        fs.insert_dir("photos").unwrap();
        fs.insert_dir("photos").unwrap();
        fs.insert_dir("photos/2024").unwrap();
        let stat = fs.stat("/photos/2024").unwrap();
        assert_eq!(stat.kind, EntryKind::Dir);
        // 父级自动补齐（与 `insert_file` 同口径）。
        assert_eq!(fs.stat("/photos").unwrap().kind, EntryKind::Dir);

        // 同名文件占位时报冲突，而不是悄悄把文件变目录。
        fs.write_atomic("/photos/a.txt", b"x").unwrap();
        assert_eq!(fs.insert_dir("/photos/a.txt").unwrap_err().code, "E2007");
    }

    #[test]
    fn read_ranges_and_stream_reject_out_of_bounds_ranges() {
        let fs = fs();
        fs.insert_file("/a.txt", b"0123456789".to_vec()).unwrap();
        assert_eq!(
            fs.read_ranges("/a.txt", &[(0, 3), (7, 10)]).unwrap(),
            b"012789"
        );
        let mut chunks = Vec::new();
        fs.read_ranges_stream("/a.txt", &[(0, 2), (8, 10)], &mut |chunk| {
            chunks.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(chunks, b"0189");
        assert_eq!(
            fs.read_ranges("/a.txt", &[(9, 11)]).unwrap_err().code,
            "E2010"
        );
    }

    #[test]
    fn rewrite_atomic_does_not_commit_when_the_callback_fails() {
        let fs = fs();
        fs.write_atomic("/a.txt", b"original").unwrap();
        let error = fs
            .rewrite_atomic_with("/a.txt", &mut |reader, writer| {
                let mut buffer = [0u8; 4];
                reader
                    .read_exact(&mut buffer)
                    .map_err(|error| KernelError::new("E2001", error.to_string(), None, false))?;
                writer
                    .write_all(b"partial")
                    .map_err(|error| KernelError::new("E2004", error.to_string(), None, false))?;
                Err(KernelError::new("E3001", "injected", None, false))
            })
            .unwrap_err();
        assert_eq!(error.code, "E3001");
        assert_eq!(fs.read_bytes("/a.txt").unwrap(), b"original");
    }

    #[test]
    fn list_dir_filters_files_like_the_desktop_adapter() {
        let fs = fs();
        fs.write_atomic("/root/a.txt", b"a").unwrap();
        fs.write_atomic("/root/b.csv", b"bb").unwrap();
        fs.write_atomic("/root/nested/c.txt", b"ccc").unwrap();
        fs.write_atomic("/root/.hidden.txt", b"h").unwrap();

        let flat = fs.list_dir("/root", &ScanFilter::default()).unwrap();
        let names: Vec<&str> = flat.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.csv"]);

        let txt = fs
            .list_dir(
                "/root",
                &ScanFilter {
                    extensions: vec!["TXT".to_string()],
                    recursive: true,
                    ..ScanFilter::default()
                },
            )
            .unwrap();
        let names: Vec<&str> = txt.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "c.txt"]);
    }

    #[test]
    fn memory_journal_persists_entries_and_marks_them_reverted() {
        use crate::journal::{FsOp, JournalSink, JournalStatus, JournalStore, MemoryJournal};

        let mut journal = MemoryJournal::new();
        let entry = journal
            .append_with_kind(
                "plan-1",
                "entry-1",
                "2026-11-22T00:00:00Z",
                "f03",
                FsOp::WriteAtomic {
                    path: "/out/a.part1".to_string(),
                    data_ref: None,
                },
                FsOp::Quarantine {
                    from: "/out/a.part1".to_string(),
                    to: "/q/1".to_string(),
                },
            )
            .unwrap();
        assert_eq!(entry.status, JournalStatus::Applied);
        assert_eq!(journal.entries_for_plan("plan-1").unwrap().len(), 1);
        assert!(journal.entries_for_plan("plan-2").unwrap().is_empty());

        journal.mark_reverted("plan-1", &[entry.seq]).unwrap();
        assert_eq!(
            journal.entries_for_plan("plan-1").unwrap()[0].status,
            JournalStatus::Reverted
        );
    }
}
