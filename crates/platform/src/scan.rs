//! Read-only filesystem scan for the C09 vertical slice.
//!
//! This is shell/CLI code: it reads files and computes digests. The kernel
//! stays pure and only groups the resulting candidates.
//!
//! The scan implements the project's three stages: size grouping, an optional
//! head/tail quick digest, and the full digest. The quick window is always an
//! explicit caller parameter; this crate defines no product default (the
//! baseline only says "head/tail some KB", see PDR-001 and OQ-28).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use kernel::duplicate::FileIdentity;

use crate::fs::{fs_path, strip_extended_path};

const READ_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanMode {
    /// Confirm groups with a full digest after the optional quick stage.
    Full,
    /// Emit high-probability groups from the quick digest only.
    Fast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    pub mode: ScanMode,
    /// Head/tail sampling window in bytes. `None` skips the quick stage and
    /// full-hashes every same-size group. Fast mode requires a window.
    pub quick_window: Option<u64>,
}

impl ScanOptions {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(window) = self.quick_window {
            if window == 0 {
                return Err("quick window must be greater than zero".to_string());
            }
        }
        if self.mode == ScanMode::Fast && self.quick_window.is_none() {
            return Err("fast mode requires --quick-window-bytes".to_string());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_nanos: i128,
    pub quick_hash: Option<Vec<u8>>,
    pub full_hash: Option<Vec<u8>>,
    pub identity: Option<FileIdentity>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub quick_hashed: u64,
    pub full_hashed: u64,
    /// Non-zero files in size groups of one; they cannot be duplicates.
    pub size_pruned: u64,
    /// Same-size files with a unique quick digest; no full digest is read.
    pub quick_pruned: u64,
}

#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub files: Vec<ScannedFile>,
    pub skipped: Vec<(PathBuf, String)>,
    pub stats: ScanStats,
}

pub fn scan_path(root: &Path, options: ScanOptions) -> std::io::Result<ScanOutcome> {
    options
        .validate()
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;

    let mut paths = Vec::new();
    let mut skipped = Vec::new();
    collect_files(root, &mut paths, &mut skipped)?;
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = match fs::symlink_metadata(fs_path(&path)) {
            Ok(metadata) => metadata,
            Err(error) => {
                skipped.push((path, error.to_string()));
                continue;
            }
        };
        files.push(ScannedFile {
            identity: identity_from(&path, &metadata),
            path,
            size: metadata.len(),
            mtime_nanos: modified_nanos(&metadata),
            quick_hash: None,
            full_hash: None,
        });
    }

    let mut outcome = ScanOutcome {
        files,
        skipped,
        stats: ScanStats::default(),
    };

    let mut by_size: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (index, file) in outcome.files.iter().enumerate() {
        by_size.entry(file.size).or_default().push(index);
    }

    for (size, indexes) in by_size {
        if size == 0 {
            // The kernel reports zero-byte files separately; hashing them is
            // pointless and they are never grouped as content duplicates.
            continue;
        }
        if indexes.len() < 2 {
            outcome.stats.size_pruned += indexes.len() as u64;
            continue;
        }

        match options.quick_window {
            Some(window) => {
                let mut by_quick: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
                for index in indexes {
                    let path = outcome.files[index].path.clone();
                    match hash_quick(&path, size, window) {
                        Ok(hash) => {
                            outcome.stats.quick_hashed += 1;
                            outcome.files[index].quick_hash = Some(hash.clone());
                            by_quick.entry(hash).or_default().push(index);
                        }
                        Err(error) => outcome.skipped.push((path, error.to_string())),
                    }
                }

                for (_, quick_group) in by_quick {
                    if quick_group.len() < 2 {
                        outcome.stats.quick_pruned += quick_group.len() as u64;
                        continue;
                    }
                    if options.mode == ScanMode::Full {
                        for index in quick_group {
                            hash_full_into(&mut outcome, index);
                        }
                    }
                }
            }
            None => {
                // Validation guarantees Full mode when no quick window is set.
                for index in indexes {
                    hash_full_into(&mut outcome, index);
                }
            }
        }
    }

    Ok(outcome)
}

fn hash_full_into(outcome: &mut ScanOutcome, index: usize) {
    let path = outcome.files[index].path.clone();
    match hash_full(&path) {
        Ok(hash) => {
            outcome.stats.full_hashed += 1;
            outcome.files[index].full_hash = Some(hash);
        }
        Err(error) => outcome.skipped.push((path, error.to_string())),
    }
}

fn collect_files(
    root: &Path,
    out: &mut Vec<PathBuf>,
    skipped: &mut Vec<(PathBuf, String)>,
) -> std::io::Result<()> {
    // 长路径收口（设计 04 §"Windows 长路径"）：所有 std::fs 调用经 `fs_path`
    // 统一加 `\\?\` 前缀，枚举出的路径剥回前缀后存入结果/计划/Journal。
    let metadata = fs::symlink_metadata(fs_path(root))?;
    if metadata.is_file() {
        out.push(root.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(fs_path(&dir)) {
            Ok(entries) => entries,
            Err(error) => {
                skipped.push((dir, error.to_string()));
                continue;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    skipped.push((dir.clone(), error.to_string()));
                    continue;
                }
            };
            let path = PathBuf::from(strip_extended_path(&entry.path().to_string_lossy()));
            let metadata = match fs::symlink_metadata(fs_path(&path)) {
                Ok(metadata) => metadata,
                Err(error) => {
                    skipped.push((path, error.to_string()));
                    continue;
                }
            };
            if crate::fs::is_hidden(&path, &metadata) {
                continue;
            }
            if metadata.is_file() && crate::fs::is_placeholder(&metadata) {
                skipped.push((path, "placeholder file skipped".to_string()));
                continue;
            }
            if metadata.is_file() {
                out.push(path);
            } else if metadata.is_dir() {
                children.push(path);
            }
        }
        children.sort();
        children.reverse();
        stack.extend(children);
    }
    Ok(())
}

fn hash_quick(path: &Path, size: u64, window: u64) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(fs_path(path))?;
    let mut hasher = blake3::Hasher::new();
    if size <= window.saturating_mul(2) {
        // The two windows would overlap; hash the whole file once.
        hash_reader(&mut file, size, &mut hasher)?;
    } else {
        hash_reader(&mut file, window, &mut hasher)?;
        file.seek(SeekFrom::Start(size - window))?;
        hash_reader(&mut file, window, &mut hasher)?;
    }
    Ok(hasher.finalize().as_bytes().to_vec())
}

fn hash_full(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(fs_path(path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; READ_CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().as_bytes().to_vec())
}

fn hash_reader<R: Read>(
    reader: &mut R,
    mut remaining: u64,
    hasher: &mut blake3::Hasher,
) -> std::io::Result<()> {
    let mut buffer = [0u8; READ_CHUNK];
    while remaining > 0 {
        let want = remaining.min(READ_CHUNK as u64) as usize;
        reader.read_exact(&mut buffer[..want])?;
        hasher.update(&buffer[..want]);
        remaining -= want as u64;
    }
    Ok(())
}

fn modified_nanos(metadata: &fs::Metadata) -> i128 {
    match metadata.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        },
        Err(_) => 0,
    }
}

#[cfg(unix)]
fn identity_from(_path: &Path, metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(FileIdentity::new(metadata.dev(), metadata.ino() as u128))
}

#[cfg(windows)]
fn identity_from(path: &Path, _metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    // `MetadataExt::volume_serial_number`/`file_index` 仍受 `windows_by_handle`
    // 不稳定特性门控（stable 报 E0658），故走 WinAPI（ADR-0012）。设计 04 §7
    // 要求的正是 volume serial + file index。
    let file = fs::File::open(crate::fs::fs_path(path)).ok()?;
    // SAFETY: `file` 在本调用期间持有有效句柄；`info` 是独占的出参结构，
    // 不与任何别名共享；失败时按"身份不可用"回退（不 panic）。
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return None;
    }
    let index = ((info.nFileIndexHigh as u128) << 32) | info.nFileIndexLow as u128;
    Some(FileIdentity::new(info.dwVolumeSerialNumber as u64, index))
}

#[cfg(not(any(unix, windows)))]
fn identity_from(_path: &Path, _metadata: &fs::Metadata) -> Option<FileIdentity> {
    // 其他平台没有稳定文件身份；kernel 将该组标 needs_manual_review，
    // 不自动清理（设计 04 §7）。
    None
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
            let path = std::env::temp_dir().join(format!("batchdesk-c09-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn full_options(quick_window: Option<u64>) -> ScanOptions {
        ScanOptions {
            mode: ScanMode::Full,
            quick_window,
        }
    }

    #[test]
    fn scans_and_hashes_duplicate_files() {
        let dir = TempDir::new();
        fs::write(dir.0.join("a.txt"), b"same").unwrap();
        fs::write(dir.0.join("b.txt"), b"same").unwrap();
        fs::write(dir.0.join("c.txt"), b"diff").unwrap();

        let outcome = scan_path(&dir.0, full_options(None)).unwrap();
        assert_eq!(outcome.files.len(), 3);
        assert_eq!(
            outcome.files[0].full_hash.as_ref().unwrap(),
            outcome.files[1].full_hash.as_ref().unwrap()
        );
        assert_ne!(
            outcome.files[0].full_hash.as_ref().unwrap(),
            outcome.files[2].full_hash.as_ref().unwrap()
        );
    }

    #[test]
    fn scans_nested_directories() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.0.join("nested")).unwrap();
        fs::write(dir.0.join("nested").join("x.txt"), b"x").unwrap();
        let outcome = scan_path(&dir.0, full_options(None)).unwrap();
        assert_eq!(outcome.files.len(), 1);
        assert!(outcome.files[0].path.ends_with("nested/x.txt"));
        assert_eq!(outcome.stats.size_pruned, 1);
    }

    #[test]
    fn skips_hidden_files_and_directories_by_default() {
        let dir = TempDir::new();
        fs::write(dir.0.join("visible.txt"), b"visible").unwrap();
        fs::write(dir.0.join(".hidden.txt"), b"hidden").unwrap();
        fs::create_dir_all(dir.0.join(".secret")).unwrap();
        fs::write(dir.0.join(".secret").join("nested.txt"), b"hidden").unwrap();

        let outcome = scan_path(&dir.0, full_options(None)).unwrap();
        assert_eq!(outcome.files.len(), 1);
        assert!(outcome.files[0].path.ends_with("visible.txt"));
    }

    #[test]
    fn quick_hash_samples_head_and_tail_then_full_hash_separates() {
        let dir = TempDir::new();
        fs::write(dir.0.join("a.bin"), b"AAAAxxxxBBBB").unwrap();
        fs::write(dir.0.join("b.bin"), b"AAAAyyyyBBBB").unwrap();

        let outcome = scan_path(&dir.0, full_options(Some(4))).unwrap();
        assert_eq!(outcome.stats.quick_hashed, 2);
        assert_eq!(outcome.stats.quick_pruned, 0);
        assert_eq!(outcome.stats.full_hashed, 2);
        assert_eq!(
            outcome.files[0].quick_hash.as_ref().unwrap(),
            outcome.files[1].quick_hash.as_ref().unwrap()
        );
        assert_ne!(
            outcome.files[0].full_hash.as_ref().unwrap(),
            outcome.files[1].full_hash.as_ref().unwrap()
        );
    }

    #[test]
    fn quick_hash_prunes_unique_same_size_files() {
        let dir = TempDir::new();
        fs::write(dir.0.join("a.bin"), b"AAAAxxxxBBBB").unwrap();
        fs::write(dir.0.join("b.bin"), b"CCCCyyyyBBBB").unwrap();

        let outcome = scan_path(&dir.0, full_options(Some(4))).unwrap();
        assert_eq!(outcome.stats.quick_hashed, 2);
        assert_eq!(outcome.stats.quick_pruned, 2);
        assert_eq!(outcome.stats.full_hashed, 0);
        assert!(outcome.files[0].full_hash.is_none());
        assert!(outcome.files[1].full_hash.is_none());
    }

    #[test]
    fn fast_mode_hashes_only_quick() {
        let dir = TempDir::new();
        fs::write(dir.0.join("a.bin"), b"AAAAxxxxBBBB").unwrap();
        fs::write(dir.0.join("b.bin"), b"AAAAxxxxBBBB").unwrap();

        let outcome = scan_path(
            &dir.0,
            ScanOptions {
                mode: ScanMode::Fast,
                quick_window: Some(4),
            },
        )
        .unwrap();
        assert_eq!(outcome.stats.quick_hashed, 2);
        assert_eq!(outcome.stats.full_hashed, 0);
        assert_eq!(
            outcome.files[0].quick_hash.as_ref().unwrap(),
            outcome.files[1].quick_hash.as_ref().unwrap()
        );
        assert!(outcome.files.iter().all(|file| file.full_hash.is_none()));
    }

    #[test]
    fn fast_mode_requires_a_quick_window() {
        let dir = TempDir::new();
        fs::write(dir.0.join("a.bin"), b"same").unwrap();
        let options = ScanOptions {
            mode: ScanMode::Fast,
            quick_window: None,
        };
        assert!(options.validate().is_err());
        assert!(scan_path(&dir.0, options).is_err());
    }

    #[test]
    fn full_mode_with_window_only_full_hashes_quick_survivors() {
        let dir = TempDir::new();
        fs::write(dir.0.join("a.bin"), b"AAAAxxxxBBBB").unwrap();
        fs::write(dir.0.join("b.bin"), b"AAAAyyyyBBBB").unwrap();
        fs::write(dir.0.join("c.bin"), b"CCCCzzzzBBBB").unwrap();

        let outcome = scan_path(&dir.0, full_options(Some(4))).unwrap();
        assert_eq!(outcome.stats.quick_hashed, 3);
        assert_eq!(outcome.stats.quick_pruned, 1);
        assert_eq!(outcome.stats.full_hashed, 2);
        assert!(outcome.files[2].full_hash.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_identity_uses_volume_and_file_index() {
        let dir = TempDir::new();
        let path = dir.0.join("identity.txt");
        fs::write(&path, b"content").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        // WinAPI GetFileInformationByHandle 可取得 volume serial + file index；
        // 两路径指向同一文件时身份必须相同（硬链接判定依赖此点）。
        let identity = identity_from(&path, &metadata);
        assert!(identity.is_some());
        let hard = dir.0.join("hard.txt");
        if fs::hard_link(&path, &hard).is_ok() {
            let metadata = fs::metadata(&hard).unwrap();
            assert_eq!(identity_from(&hard, &metadata), identity);
        }
    }

    #[cfg(windows)]
    #[test]
    fn scans_long_paths_beyond_the_win32_limit() {
        // 方案 §5.13 超长档（>260）：适配层统一加 `\\?\` 前缀后，即使系统未开启
        // LongPathsEnabled，扫描/哈希/文件身份也能到达深层文件；返回给计划与
        // Journal 的路径必须剥回无前缀形式（设计 04 §"Windows 长路径"）。
        let dir = TempDir::new();
        let mut deep = dir.0.clone();
        while deep.as_os_str().to_string_lossy().len() < 260 {
            deep.push("level-0123456789");
        }
        fs::create_dir_all(crate::fs::fs_path(&deep)).unwrap();
        fs::write(crate::fs::fs_path(&deep.join("a.txt")), b"same").unwrap();
        fs::write(crate::fs::fs_path(&deep.join("b.txt")), b"same").unwrap();
        assert!(deep.join("a.txt").to_string_lossy().len() > 260);

        let outcome = scan_path(&dir.0, full_options(None)).unwrap();
        assert!(
            outcome.skipped.is_empty(),
            "deep files must not be skipped: {:?}",
            outcome.skipped
        );
        assert_eq!(outcome.files.len(), 2);
        for file in &outcome.files {
            let text = file.path.to_string_lossy();
            assert!(text.len() > 260);
            assert!(!text.starts_with(r"\\?\"));
        }
        assert_eq!(outcome.files[0].full_hash, outcome.files[1].full_hash);

        // TempDir 的 Drop 用的是无前缀路径，深层树需在此显式清理。
        fs::remove_dir_all(crate::fs::fs_path(&dir.0)).unwrap();
    }
}
