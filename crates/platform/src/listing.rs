//! F10 目录清单：流式遍历元数据并写出 CSV（UTF-8 BOM）。
//!
//! 遍历只读文件名/大小/修改时间/层级，不读取文件内容；每个目录内按名称排序，
//! 深度优先输出。占位文件、符号链接与不可读目录不写入清单，分别计数/跳过。

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use kernel::csv_merge::write_record;
use kernel::error::KernelError;
use kernel::fs::EntryKind;
use kernel::listing::{DirectoryLister, ListingOptions, ListingStats, LISTING_HEADER};

use crate::fs::{fs_path, is_placeholder, mtime};

pub struct FsDirectoryLister;

impl DirectoryLister for FsDirectoryLister {
    fn write_listing(
        &self,
        root: &str,
        options: &ListingOptions,
        out: &mut dyn Write,
    ) -> Result<ListingStats, KernelError> {
        out.write_all(b"\xEF\xBB\xBF")
            .map_err(|error| listing_error(root, error))?;
        let header: Vec<String> = LISTING_HEADER.iter().map(ToString::to_string).collect();
        let mut line = String::new();
        write_record(&header, b',', &mut line);
        out.write_all(line.as_bytes())
            .map_err(|error| listing_error(root, error))?;
        walk(Path::new(root), options, &mut |row| {
            let fields = vec![
                row.path.clone(),
                row.name.clone(),
                row.kind_label().to_string(),
                row.size.to_string(),
                row.mtime.clone(),
                row.level.to_string(),
            ];
            let mut line = String::new();
            write_record(&fields, b',', &mut line);
            out.write_all(line.as_bytes())
        })
        .map_err(|error| listing_error(root, error))
    }
}

/// 计划期摘要扫描（不写输出；只读元数据）。
pub fn inspect_tree(root: &Path, options: &ListingOptions) -> io::Result<ListingStats> {
    walk(root, options, &mut |_| Ok(()))
}

struct ListingRow {
    path: String,
    name: String,
    kind: EntryKind,
    size: u64,
    mtime: String,
    level: u32,
}

impl ListingRow {
    fn kind_label(&self) -> &'static str {
        match self.kind {
            EntryKind::Dir => "目录",
            EntryKind::File => "文件",
            EntryKind::Symlink => "链接",
        }
    }
}

fn walk<F>(root: &Path, options: &ListingOptions, on_row: &mut F) -> io::Result<ListingStats>
where
    F: FnMut(&ListingRow) -> io::Result<()>,
{
    let metadata = fs::metadata(fs_path(root))?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "listing root must be a directory",
        ));
    }
    let mut stats = ListingStats::default();
    walk_dir(root, String::new(), 0, options, on_row, &mut stats, true)?;
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn walk_dir<F>(
    dir: &Path,
    rel_dir: String,
    level: u32,
    options: &ListingOptions,
    on_row: &mut F,
    stats: &mut ListingStats,
    is_root: bool,
) -> io::Result<()>
where
    F: FnMut(&ListingRow) -> io::Result<()>,
{
    let entries = match fs::read_dir(fs_path(dir)) {
        Ok(entries) => entries,
        Err(error) if is_root => return Err(error),
        Err(_) => {
            stats.skipped += 1;
            return Ok(());
        }
    };
    let mut children = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            stats.skipped += 1;
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let metadata = match fs::symlink_metadata(fs_path(&path)) {
            Ok(metadata) => metadata,
            Err(_) => {
                stats.skipped += 1;
                continue;
            }
        };
        let rel = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        children.push((name, rel, path, metadata));
    }
    children.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, rel, path, metadata) in children {
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            stats.skipped += 1;
            continue;
        }
        let child_level = level + 1;
        if metadata.is_dir() {
            stats.dirs += 1;
            stats.max_depth = stats.max_depth.max(child_level);
            if options.include_dirs {
                on_row(&ListingRow {
                    path: rel.clone(),
                    name,
                    kind: EntryKind::Dir,
                    size: 0,
                    mtime: mtime(&metadata),
                    level: child_level,
                })?;
            }
            walk_dir(&path, rel, child_level, options, on_row, stats, false)?;
        } else if metadata.is_file() {
            if is_placeholder(&metadata) {
                stats.placeholders += 1;
                continue;
            }
            stats.files += 1;
            stats.max_depth = stats.max_depth.max(child_level);
            if options.include_files {
                on_row(&ListingRow {
                    path: rel,
                    name,
                    kind: EntryKind::File,
                    size: metadata.len(),
                    mtime: mtime(&metadata),
                    level: child_level,
                })?;
            }
        } else {
            stats.skipped += 1;
        }
    }
    Ok(())
}

fn listing_error(path: &str, error: io::Error) -> KernelError {
    KernelError::new("E2008", error.to_string(), Some(path.to_string()), false)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-listing-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn options() -> ListingOptions {
        ListingOptions::default()
    }

    #[test]
    fn inspect_counts_files_dirs_and_depth() {
        let dir = TempDir::new();
        let root = dir.0.join("tree");
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::write(root.join("a.txt"), b"abc").unwrap();
        fs::write(root.join("sub/b.txt"), b"hello").unwrap();
        let stats = inspect_tree(&root, &options()).unwrap();
        assert_eq!(stats.files, 2);
        assert_eq!(stats.dirs, 2);
        assert_eq!(stats.max_depth, 2);
        assert_eq!(stats.placeholders, 0);
    }

    #[test]
    fn writes_bom_header_and_streamed_rows() {
        let dir = TempDir::new();
        let root = dir.0.join("tree");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"abc").unwrap();
        fs::write(root.join("sub/b.txt"), b"hello").unwrap();
        let mut output = Vec::new();
        let stats = FsDirectoryLister
            .write_listing(root.to_str().unwrap(), &options(), &mut output)
            .unwrap();
        assert_eq!(stats.files, 2);
        assert_eq!(&output[..3], b"\xEF\xBB\xBF");
        let text = String::from_utf8(output[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "相对路径,名称,类型,大小,修改时间,层级");
        assert_eq!(lines[1].split(',').next(), Some("a.txt"));
        assert!(lines.iter().any(|line| line.contains("sub/b.txt")));
        assert!(lines.iter().any(|line| line.contains(",目录,")));
        assert!(lines.iter().any(|line| line.contains(",文件,")));
    }

    #[test]
    fn empty_directory_writes_only_the_header() {
        let dir = TempDir::new();
        let root = dir.0.join("empty");
        fs::create_dir_all(&root).unwrap();
        let mut output = Vec::new();
        let stats = FsDirectoryLister
            .write_listing(root.to_str().unwrap(), &options(), &mut output)
            .unwrap();
        assert_eq!(stats.files + stats.dirs, 0);
        let text = String::from_utf8(output[3..].to_vec()).unwrap();
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn file_root_is_rejected() {
        let dir = TempDir::new();
        let file = dir.0.join("a.txt");
        fs::write(&file, b"x").unwrap();
        assert!(inspect_tree(&file, &options()).is_err());
    }
}
