//! std::fs implementation of the kernel `FsAdapter` port for the CLI slice.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use kernel::error::KernelError;
use kernel::fs::{
    CaseSense, ConflictPolicy, DirEntryInfo, EntryAttrs, EntryKind, FsAdapter, PathOutcome,
    PathOutcomeKind, ScanFilter, StatInfo,
};

pub struct CliFsAdapter {
    quarantine_dir: Option<PathBuf>,
    counter: AtomicU64,
}

impl CliFsAdapter {
    pub fn new(quarantine_dir: Option<PathBuf>) -> Self {
        Self {
            quarantine_dir,
            counter: AtomicU64::new(0),
        }
    }

    fn next_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }

    fn resolve_target(
        &self,
        from: &Path,
        to: &Path,
        policy: ConflictPolicy,
    ) -> Result<Option<PathBuf>, KernelError> {
        if !path_exists(to) || from == to {
            return Ok(Some(to.to_path_buf()));
        }
        match policy {
            ConflictPolicy::Skip => Ok(None),
            ConflictPolicy::Overwrite => Ok(Some(to.to_path_buf())),
            ConflictPolicy::Sequence => {
                let parent = to.parent().unwrap_or_else(|| Path::new(""));
                let stem = to
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("file");
                let extension = to.extension().and_then(|value| value.to_str());
                for index in 1.. {
                    let name = match extension {
                        Some(extension) => format!("{stem} ({index}).{extension}"),
                        None => format!("{stem} ({index})"),
                    };
                    let candidate = parent.join(name);
                    if !path_exists(&candidate) {
                        return Ok(Some(candidate));
                    }
                }
                unreachable!("sequence search is unbounded")
            }
        }
    }

    fn move_file(&self, from: &Path, to: &Path) -> Result<(), KernelError> {
        let from = fs_path(from);
        let to = fs_path(to);
        if let Some(parent) = to.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| io_error(&error, &to))?;
            }
        }
        if to.exists() {
            fs::remove_file(&to).map_err(|error| io_error(&error, &to))?;
        }
        match fs::rename(&from, &to) {
            Ok(()) => Ok(()),
            Err(error) if is_cross_device(&error) => {
                fs::copy(&from, &to).map_err(|error| io_error(&error, &from))?;
                fs::remove_file(&from).map_err(|error| {
                    let _ = fs::remove_file(&to);
                    io_error(&error, &from)
                })?;
                Ok(())
            }
            Err(error) => Err(io_error(&error, &from)),
        }
    }

    /// 隔离目录内的唯一新路径（`stem.nanos.id` 命名，与 remove_to_quarantine 同式）。
    fn unique_quarantine_path(
        &self,
        quarantine_dir: &Path,
        base_name: &str,
    ) -> Result<PathBuf, KernelError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let stem = Path::new(base_name)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("file");
        let extension = Path::new(base_name)
            .extension()
            .and_then(|value| value.to_str());
        loop {
            let id = self.next_id();
            let name = match extension {
                Some(extension) => format!("{stem}.{nanos}.{id}.{extension}"),
                None => format!("{stem}.{nanos}.{id}"),
            };
            let candidate = quarantine_dir.join(name);
            if !path_exists(&candidate) {
                return Ok(candidate);
            }
        }
    }
}

impl FsAdapter for CliFsAdapter {
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError> {
        fs::read(fs_path(Path::new(path))).map_err(|error| io_error(&error, Path::new(path)))
    }

    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), KernelError> {
        let target = fs_path(Path::new(path));
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                KernelError::new("E1003", "target has no parent directory", None, false)
            })?;
        fs::create_dir_all(parent).map_err(|error| io_error(&error, parent))?;
        let temp = parent.join(format!(
            ".batchdesk-write-{}-{}",
            std::process::id(),
            self.next_id()
        ));
        fs::write(&temp, data).map_err(|error| io_error(&error, &temp))?;
        #[cfg(windows)]
        if target.exists() {
            fs::remove_file(&target).map_err(|error| io_error(&error, &target))?;
        }
        fs::rename(&temp, &target).map_err(|error| {
            let _ = fs::remove_file(&temp);
            io_error(&error, &target)
        })
    }

    fn rename(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        let from_path = Path::new(from);
        let to_path = Path::new(to);
        if from_path == to_path {
            return Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: to.to_string(),
            });
        }
        let Some(target) = self.resolve_target(from_path, to_path, policy)? else {
            return Ok(PathOutcome {
                outcome: PathOutcomeKind::Skipped,
                final_path: to.to_string(),
            });
        };
        let replaced = path_exists(&target);
        self.move_file(from_path, &target)?;
        Ok(PathOutcome {
            outcome: if replaced {
                PathOutcomeKind::Replaced
            } else {
                PathOutcomeKind::Applied
            },
            final_path: strip_extended_path(&target.to_string_lossy()),
        })
    }

    fn copy(
        &self,
        from: &str,
        to: &str,
        policy: ConflictPolicy,
    ) -> Result<PathOutcome, KernelError> {
        let from_path = Path::new(from);
        let to_path = Path::new(to);
        if from_path == to_path {
            return Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: to.to_string(),
            });
        }
        let Some(target) = self.resolve_target(from_path, to_path, policy)? else {
            return Ok(PathOutcome {
                outcome: PathOutcomeKind::Skipped,
                final_path: to.to_string(),
            });
        };
        let replaced = path_exists(&target);
        if replaced {
            fs::remove_file(fs_path(&target)).map_err(|error| io_error(&error, &target))?;
        }
        fs::copy(fs_path(from_path), fs_path(&target))
            .map_err(|error| io_error(&error, from_path))?;
        Ok(PathOutcome {
            outcome: if replaced {
                PathOutcomeKind::Replaced
            } else {
                PathOutcomeKind::Applied
            },
            final_path: strip_extended_path(&target.to_string_lossy()),
        })
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
        let from = fs_path(Path::new(source));
        let allocated = fs::metadata(&from)
            .map_err(|error| io_error(&error, Path::new(source)))?
            .len();
        if end > allocated {
            return Err(KernelError::new(
                "E2010",
                "source file shrank since planning; re-plan before applying",
                Some(format!(
                    "{} has {allocated} bytes, part needs up to {end}",
                    source
                )),
                true,
            ));
        }
        let Some(destination) =
            self.resolve_target(Path::new(source), Path::new(target), policy)?
        else {
            return Ok(PathOutcome {
                outcome: PathOutcomeKind::Skipped,
                final_path: target.to_string(),
            });
        };
        let replaced = path_exists(&destination);
        let destination = fs_path(&destination);
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                KernelError::new("E1003", "target has no parent directory", None, false)
            })?;
        fs::create_dir_all(parent).map_err(|error| io_error(&error, parent))?;
        let temp = parent.join(format!(
            ".batchdesk-split-{}-{}",
            std::process::id(),
            self.next_id()
        ));
        let copied = copy_range_stream(&from, start, end, &temp);
        if let Err(error) = copied {
            let _ = fs::remove_file(&temp);
            return Err(io_error(&error, &destination));
        }
        #[cfg(windows)]
        if destination.exists() {
            fs::remove_file(&destination).map_err(|error| io_error(&error, &destination))?;
        }
        if let Err(error) = fs::rename(&temp, &destination) {
            let _ = fs::remove_file(&temp);
            return Err(io_error(&error, &destination));
        }
        Ok(PathOutcome {
            outcome: if replaced {
                PathOutcomeKind::Replaced
            } else {
                PathOutcomeKind::Applied
            },
            final_path: strip_extended_path(&destination.to_string_lossy()),
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
        let quarantine_dir = self.quarantine_dir.as_ref().ok_or_else(|| {
            KernelError::new(
                "E5001",
                "quarantine directory is not configured",
                None,
                false,
            )
        })?;
        fs::create_dir_all(fs_path(quarantine_dir))
            .map_err(|error| io_error(&error, quarantine_dir))?;

        let source = Path::new(path);
        if source.file_name().is_none() {
            return Err(KernelError::new(
                "E1003",
                "source path has no file name",
                None,
                false,
            ));
        }
        let stem = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("file");
        let extension = source.extension().and_then(|value| value.to_str());
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let target = loop {
            let id = self.next_id();
            let name = match extension {
                Some(extension) => {
                    format!("{stem}.{nanos}.{id}.{extension}")
                }
                None => format!("{stem}.{nanos}.{id}"),
            };
            let candidate = quarantine_dir.join(name);
            if !path_exists(&candidate) {
                break candidate;
            }
        };
        self.move_file(source, &target)?;
        Ok(target.to_string_lossy().into_owned())
    }

    fn backup_to_quarantine(&self, path: &str) -> Result<String, KernelError> {
        let quarantine_dir = self.quarantine_dir.as_ref().ok_or_else(|| {
            KernelError::new(
                "E5001",
                "quarantine directory is not configured",
                None,
                false,
            )
        })?;
        fs::create_dir_all(fs_path(quarantine_dir))
            .map_err(|error| io_error(&error, quarantine_dir))?;
        let source = Path::new(path);
        if source.file_name().is_none() {
            return Err(KernelError::new(
                "E1003",
                "source path has no file name",
                None,
                false,
            ));
        }
        let base_name = source
            .file_name()
            .map(|value| format!("{}.bak", value.to_string_lossy()))
            .unwrap_or_else(|| "file.bak".to_string());
        let target = self.unique_quarantine_path(quarantine_dir, &base_name)?;
        // 备份是复制而非移动（设计 01 §8）：原文件留在原地，撤销时用备份还原。
        fs::copy(fs_path(source), fs_path(&target)).map_err(|error| io_error(&error, source))?;
        Ok(target.to_string_lossy().into_owned())
    }

    fn quarantine_target_path(&self, file_name: &str) -> Result<String, KernelError> {
        let quarantine_dir = self.quarantine_dir.as_ref().ok_or_else(|| {
            KernelError::new(
                "E5001",
                "quarantine directory is not configured",
                None,
                false,
            )
        })?;
        fs::create_dir_all(fs_path(quarantine_dir))
            .map_err(|error| io_error(&error, quarantine_dir))?;
        let safe_name = Path::new(file_name)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .ok_or_else(|| KernelError::new("E1003", "file name is required", None, false))?;
        let target = self.unique_quarantine_path(quarantine_dir, &safe_name)?;
        Ok(target.to_string_lossy().into_owned())
    }

    fn list_dir(&self, path: &str, filter: &ScanFilter) -> Result<Vec<DirEntryInfo>, KernelError> {
        let root = fs_path(Path::new(path));
        let mut result = Vec::new();
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let entries = fs::read_dir(&dir).map_err(|error| io_error(&error, &dir))?;
            for entry in entries {
                let entry = entry.map_err(|error| io_error(&error, &dir))?;
                let entry_path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                let metadata = fs::symlink_metadata(&entry_path)
                    .map_err(|error| io_error(&error, &entry_path))?;
                if !filter.include_hidden && is_hidden(&entry_path, &metadata) {
                    continue;
                }
                let kind = entry_kind(&metadata);
                if kind == EntryKind::Dir {
                    if filter.recursive
                        && filter
                            .max_depth
                            .map(|max_depth| depth < max_depth)
                            .unwrap_or(true)
                    {
                        stack.push((entry_path.clone(), depth + 1));
                    }
                    continue;
                }
                if !filter.extensions.is_empty() {
                    let extension = entry_path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if !filter
                        .extensions
                        .iter()
                        .any(|wanted| wanted.to_ascii_lowercase() == extension)
                    {
                        continue;
                    }
                }
                let entry_attrs = attrs(&metadata, &name);
                result.push(DirEntryInfo {
                    path: strip_extended_path(&entry_path.to_string_lossy()),
                    name,
                    kind,
                    size: metadata.len(),
                    mtime: mtime(&metadata),
                    attrs: entry_attrs,
                });
            }
        }
        result.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(result)
    }

    fn stat(&self, path: &str) -> Result<StatInfo, KernelError> {
        let target = Path::new(path);
        let metadata =
            fs::symlink_metadata(fs_path(target)).map_err(|error| io_error(&error, target))?;
        let name = target
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(StatInfo {
            kind: entry_kind(&metadata),
            size: metadata.len(),
            mtime: mtime(&metadata),
            attrs: attrs(&metadata, &name),
        })
    }

    fn canonicalize(&self, path: &str) -> Result<String, KernelError> {
        canonicalize_display(Path::new(path)).ok_or_else(|| {
            KernelError::new(
                "E2001",
                "cannot canonicalize path",
                Some(path.to_string()),
                false,
            )
        })
    }

    fn case_sensitivity(&self) -> CaseSense {
        if cfg!(windows) {
            CaseSense::CaseInsensitive
        } else {
            CaseSense::Sensitive
        }
    }

    fn content_hash(&self, path: &str, algorithm: &str) -> Result<Option<String>, KernelError> {
        if algorithm != "blake3" {
            return Ok(None);
        }
        let target = Path::new(path);
        let mut file = fs::File::open(fs_path(target)).map_err(|error| io_error(&error, target))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| io_error(&error, target))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(Some(hex_encode(hasher.finalize().as_bytes())))
    }
}

/// 按固定块把源文件 `[start, end)` 复制到 `temp`（流式，内存恒定）。
fn copy_range_stream(source: &Path, start: u64, end: u64, temp: &Path) -> io::Result<()> {
    const COPY_CHUNK: usize = 64 * 1024;
    let mut input = fs::File::open(source)?;
    input.seek(SeekFrom::Start(start))?;
    let mut remaining = end - start;
    let mut output = fs::File::create(temp)?;
    let mut buffer = vec![0u8; COPY_CHUNK];
    while remaining > 0 {
        let want = (buffer.len() as u64).min(remaining) as usize;
        let read = input.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source shrank while copying split range",
            ));
        }
        output.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    output.flush()
}

pub(crate) fn fs_path(path: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(extended_path(&path.as_ref().to_string_lossy()))
}

fn path_exists(path: &Path) -> bool {
    fs_path(path).exists()
}

fn extended_path(path: &str) -> String {
    #[cfg(windows)]
    {
        if path.starts_with(r"\\?\") {
            return path.to_string();
        }
        // 以两种分隔符形式探测绝对路径：`\\UNC`/`//UNC` 与 `C:\`/`C:/`。
        // 相对路径与 `C:` 盘符相对形式不加前缀，保持 Win32 原语义。
        let slash_form = path.replace('\\', "/");
        let looks_absolute = slash_form.starts_with("//")
            || (slash_form.as_bytes().get(1) == Some(&b':')
                && slash_form.as_bytes().get(2) == Some(&b'/'));
        if looks_absolute {
            // 设计 04 §N4/§"Windows 长路径"：`\\?\` 前缀不吃 Win32 归一化，
            // 加前缀前先归一分隔符与 `.`/`..` 段（kernel 纯函数）；归一失败
            // （越界）时保留原串，让系统调用按既有错误路径报错。
            let normalized = kernel::path::try_normalize(path)
                .map(|value| value.replace('/', "\\"))
                .unwrap_or_else(|_| path.to_string());
            if let Some(rest) = normalized.strip_prefix(r"\\") {
                return format!(r"\\?\UNC\{rest}");
            }
            if normalized.as_bytes().get(1) == Some(&b':') {
                return format!(r"\\?\{normalized}");
            }
        }
        path.to_string()
    }
    #[cfg(not(windows))]
    {
        path.to_string()
    }
}

pub(crate) fn strip_extended_path(path: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = path.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// 规范化用户路径并返回剥离 `\\?\` 前缀的显示形式。
///
/// Windows 上 `std::fs::canonicalize` 恒返回带 `\\?\` 前缀的路径，直接透出会
/// 污染 Journal/UI；壳层一律经由本助手取 canonical 显示值（设计 04 §"Windows
/// 长路径"：前缀逻辑属适配层内部，业务层禁止自拼，不变量 8）。
pub fn canonicalize_display(path: &Path) -> Option<String> {
    fs::canonicalize(fs_path(path))
        .ok()
        .map(|canonical| strip_extended_path(&canonical.to_string_lossy()))
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

fn entry_kind(metadata: &fs::Metadata) -> EntryKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}

fn attrs(metadata: &fs::Metadata, name: &str) -> EntryAttrs {
    EntryAttrs {
        readonly: metadata.permissions().readonly(),
        hidden: is_hidden(Path::new(name), metadata),
        placeholder: is_placeholder(metadata),
    }
}

pub(crate) fn is_hidden(path: &Path, metadata: &fs::Metadata) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with('.'))
        .unwrap_or(false)
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(windows)]
pub(crate) fn is_placeholder(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    metadata.file_attributes()
        & (FILE_ATTRIBUTE_REPARSE_POINT
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

#[cfg(not(windows))]
pub(crate) fn is_placeholder(_metadata: &fs::Metadata) -> bool {
    false
}

/// 供 S01 预检使用的占位文件探测（只读元数据，不下载内容）。
pub fn is_placeholder_file(path: &Path) -> bool {
    fs::metadata(fs_path(path))
        .map(|metadata| is_placeholder(&metadata))
        .unwrap_or(false)
}

fn mtime(metadata: &fs::Metadata) -> String {
    metadata
        .modified()
        .map(format_rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn is_cross_device(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(18) | Some(17))
}

fn io_error(error: &io::Error, path: &Path) -> KernelError {
    let (code, retryable) = match error.kind() {
        io::ErrorKind::NotFound => ("E2001", false),
        io::ErrorKind::PermissionDenied => ("E2002", false),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => ("E2003", true),
        io::ErrorKind::TimedOut
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset => ("E2005", true),
        _ if matches!(error.raw_os_error(), Some(28) | Some(112)) => ("E2004", true),
        _ => ("E2008", false),
    };
    KernelError::new(
        code,
        error.to_string(),
        Some(path.to_string_lossy().into_owned()),
        retryable,
    )
}

pub fn now_rfc3339() -> String {
    format_rfc3339(SystemTime::now())
}

pub fn rfc3339_days_ago(days: u64) -> String {
    let seconds = days.saturating_mul(86_400);
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH);
    format_rfc3339(cutoff)
}

fn format_rfc3339(time: SystemTime) -> String {
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-fs-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn quarantine_rename_restores_the_file() {
        let dir = TempDir::new();
        let source = dir.0.join("source.txt");
        let quarantine = dir.0.join("quarantine");
        fs::write(&source, b"content").unwrap();
        let adapter = CliFsAdapter::new(Some(quarantine.clone()));

        let quarantined = adapter
            .remove_to_quarantine(source.to_str().unwrap())
            .unwrap();
        assert!(!source.exists());
        assert!(Path::new(&quarantined).exists());

        let outcome = adapter
            .rename(&quarantined, source.to_str().unwrap(), ConflictPolicy::Skip)
            .unwrap();
        assert_eq!(outcome.outcome, PathOutcomeKind::Applied);
        assert!(source.exists());
        assert_eq!(fs::read(&source).unwrap(), b"content");
    }

    #[test]
    fn backup_copies_into_quarantine_and_keeps_the_original() {
        // 设计 01 §8：备份是复制而非移动；撤销用 dataRef 指向的备份还原。
        let dir = TempDir::new();
        let source = dir.0.join("source.txt");
        let quarantine = dir.0.join("quarantine");
        fs::write(&source, b"original").unwrap();
        let adapter = CliFsAdapter::new(Some(quarantine));

        let backup = adapter
            .backup_to_quarantine(source.to_str().unwrap())
            .unwrap();
        assert!(Path::new(&backup).exists());
        assert!(source.exists(), "backup must not move the original");
        assert_eq!(fs::read(&backup).unwrap(), b"original");

        let reserved = adapter.quarantine_target_path("out.csv").unwrap();
        assert!(
            !Path::new(&reserved).exists(),
            "reserved path must not exist yet"
        );
        // 路径分隔符随平台不同（Windows 为 `\`），断言必须走 Path 而非硬编码 `/`。
        let reserved_name = Path::new(&reserved)
            .file_name()
            .and_then(|value| value.to_str())
            .expect("reserved path should have a file name");
        assert!(
            reserved_name.ends_with(".csv") && reserved_name.starts_with("out."),
            "reserved name {reserved_name} should keep stem and extension"
        );
        let reserved_again = adapter.quarantine_target_path("out.csv").unwrap();
        assert_ne!(reserved, reserved_again, "reserved paths are unique");
    }

    #[test]
    fn rename_sequence_avoids_existing_target() {
        let dir = TempDir::new();
        let source = dir.0.join("source.txt");
        let target = dir.0.join("target.txt");
        fs::write(&source, b"source").unwrap();
        fs::write(&target, b"target").unwrap();
        let adapter = CliFsAdapter::new(None);

        let outcome = adapter
            .rename(
                source.to_str().unwrap(),
                target.to_str().unwrap(),
                ConflictPolicy::Sequence,
            )
            .unwrap();
        assert_eq!(outcome.outcome, PathOutcomeKind::Applied);
        assert_ne!(outcome.final_path, target.to_string_lossy());
        assert!(Path::new(&outcome.final_path).exists());
        assert_eq!(fs::read(&target).unwrap(), b"target");
    }

    #[test]
    fn rfc3339_formats_a_known_timestamp() {
        let time = UNIX_EPOCH + std::time::Duration::from_secs(0);
        assert_eq!(format_rfc3339(time), "1970-01-01T00:00:00Z");
        let time = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(format_rfc3339(time), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn content_hash_matches_blake3_and_rejects_unknown_algorithm() {
        let dir = TempDir::new();
        let path = dir.0.join("hash.txt");
        fs::write(&path, b"abc").unwrap();
        let adapter = CliFsAdapter::new(None);
        let actual = adapter
            .content_hash(path.to_str().unwrap(), "blake3")
            .unwrap();
        assert_eq!(
            actual.as_deref(),
            Some(blake3::hash(b"abc").to_hex().as_str())
        );
        assert_eq!(
            adapter
                .content_hash(path.to_str().unwrap(), "sha256")
                .unwrap(),
            None
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn extended_path_is_identity_on_unix() {
        assert_eq!(extended_path("/tmp/a.txt"), "/tmp/a.txt");
        assert_eq!(strip_extended_path("/tmp/a.txt"), "/tmp/a.txt");
    }

    #[cfg(windows)]
    #[test]
    fn extended_path_prefixes_drive_and_unc_paths() {
        assert_eq!(extended_path(r"C:\tmp\a.txt"), r"\\?\C:\tmp\a.txt");
        assert_eq!(
            extended_path(r"\\server\share\a.txt"),
            r"\\?\UNC\server\share\a.txt"
        );
        assert_eq!(strip_extended_path(r"\\?\C:\tmp\a.txt"), r"C:\tmp\a.txt");
        assert_eq!(
            strip_extended_path(r"\\?\UNC\server\share\a.txt"),
            r"\\server\share\a.txt"
        );
    }

    #[test]
    fn copy_range_atomic_writes_only_the_requested_slice() {
        let dir = TempDir::new();
        let source = dir.0.join("source.txt");
        let target = dir.0.join("part.txt");
        fs::write(&source, b"aaa\nbbb\nccc\n").unwrap();
        let adapter = CliFsAdapter::new(Some(dir.0.join("quarantine")));

        let outcome = adapter
            .copy_range_atomic(
                &source.to_string_lossy(),
                0,
                8,
                &target.to_string_lossy(),
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(outcome.outcome, PathOutcomeKind::Applied);
        assert_eq!(fs::read(&target).unwrap(), b"aaa\nbbb\n");
        assert_eq!(
            fs::read(&source).unwrap(),
            b"aaa\nbbb\nccc\n",
            "range copy keeps the source"
        );
    }

    #[cfg(windows)]
    #[test]
    fn extended_path_normalizes_before_prefixing_and_keeps_relative_paths() {
        // `\\?\` 前缀不吃 Win32 归一化：加前缀前先统一分隔符并消解 `.`/`..`。
        assert_eq!(extended_path("C:/tmp/a.txt"), r"\\?\C:\tmp\a.txt");
        assert_eq!(extended_path(r"C:\tmp\.\sub\..\a.txt"), r"\\?\C:\tmp\a.txt");
        assert_eq!(
            extended_path("//server/share/a.txt"),
            r"\\?\UNC\server\share\a.txt"
        );
        assert_eq!(extended_path("relative/a.txt"), "relative/a.txt");
        assert_eq!(
            extended_path("C:data"),
            "C:data",
            "drive-relative stays unprefixed"
        );
    }
}
