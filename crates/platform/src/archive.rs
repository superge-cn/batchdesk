//! C10 ZIP 解压端口实现（方案 §5.11）：原始名读取、GBK 回退与条目解压。

use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use kernel::archive::{
    ArchiveCompression, ArchiveEntryInfo, ArchiveEntryKind, ArchiveInspect, ArchiveService,
};
use kernel::encoding::{transcode, InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use kernel::error::KernelError;

pub struct ZipArchiveService;

impl ArchiveService for ZipArchiveService {
    fn inspect(&self, archive: &str) -> Result<ArchiveInspect, KernelError> {
        let path = crate::fs::fs_path(Path::new(archive));
        let metadata = fs::metadata(&path).map_err(|error| io_error(archive, &error))?;
        let file = fs::File::open(&path).map_err(|error| io_error(archive, &error))?;
        let mut zip = zip::ZipArchive::new(file).map_err(zip_error)?;
        let mut entries = Vec::with_capacity(zip.len());
        for index in 0..zip.len() {
            let entry = zip.by_index(index).map_err(zip_error)?;
            let utf8_flag = entry.flags().as_u16() & 0x0800 != 0;
            let (name, gbk_fallback) = decode_name(entry.name_raw(), utf8_flag);
            let supported = matches!(
                entry.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            );
            entries.push(ArchiveEntryInfo {
                index: index as u64,
                name,
                kind: if entry.is_dir() {
                    ArchiveEntryKind::Dir
                } else {
                    ArchiveEntryKind::File
                },
                size: entry.size(),
                compressed_size: entry.compressed_size(),
                gbk_fallback,
                encrypted: entry.encrypted(),
                supported,
            });
        }
        Ok(ArchiveInspect {
            archive_size: metadata.len(),
            entries,
        })
    }

    fn extract_entry(
        &self,
        archive: &str,
        index: u64,
        writer: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let path = crate::fs::fs_path(Path::new(archive));
        let file = fs::File::open(&path).map_err(|error| io_error(archive, &error))?;
        let mut zip = zip::ZipArchive::new(file).map_err(zip_error)?;
        let mut entry = zip
            .by_index(
                usize::try_from(index).map_err(|_| {
                    KernelError::invalid_params("archive entry index is out of range")
                })?,
            )
            .map_err(zip_error)?;
        if entry.encrypted() {
            return Err(KernelError::new(
                "E3003",
                "encrypted archive entries are not supported",
                None,
                false,
            ));
        }
        std::io::copy(&mut entry, writer).map_err(|error| {
            KernelError::new(
                "E3005",
                "archive entry write failed",
                Some(error.to_string()),
                false,
            )
        })?;
        Ok(())
    }

    fn compress(
        &self,
        sources: &[String],
        compression: ArchiveCompression,
    ) -> Result<Vec<u8>, KernelError> {
        let options =
            zip::write::SimpleFileOptions::default().compression_method(match compression {
                ArchiveCompression::Store => zip::CompressionMethod::Stored,
                ArchiveCompression::Deflate => zip::CompressionMethod::Deflated,
            });
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let mut names = BTreeSet::new();
        for source in sources {
            let path = crate::fs::fs_path(Path::new(source));
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| KernelError::invalid_params("source path has no file name"))?;
            add_path(&mut writer, &path, name.to_string(), options, &mut names)?;
        }
        let cursor = writer.finish().map_err(zip_error)?;
        Ok(cursor.into_inner())
    }
}

fn add_path<W: Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    path: &Path,
    entry_name: String,
    options: zip::write::SimpleFileOptions,
    names: &mut BTreeSet<String>,
) -> Result<(), KernelError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        KernelError::new(
            "E1004",
            "source is not readable",
            Some(format!("{}: {error}", path.display())),
            false,
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    let entry_name = entry_name.replace('\\', "/");
    if metadata.is_dir() {
        let directory_name = format!("{}/", entry_name.trim_end_matches('/'));
        if !names.insert(directory_name.clone()) {
            return Err(KernelError::invalid_params(format!(
                "duplicate archive entry: {directory_name}"
            )));
        }
        writer
            .add_directory(directory_name, options)
            .map_err(zip_error)?;
        let mut children: Vec<_> = fs::read_dir(path)
            .map_err(|error| {
                KernelError::new(
                    "E1004",
                    "source directory is not readable",
                    Some(format!("{}: {error}", path.display())),
                    false,
                )
            })?
            .filter_map(Result::ok)
            .collect();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            add_path(
                writer,
                &child.path(),
                format!("{entry_name}/{name}"),
                options,
                names,
            )?;
        }
        return Ok(());
    }
    if !names.insert(entry_name.clone()) {
        return Err(KernelError::invalid_params(format!(
            "duplicate archive entry: {entry_name}"
        )));
    }
    writer.start_file(entry_name, options).map_err(zip_error)?;
    let mut file = fs::File::open(path).map_err(|error| {
        KernelError::new(
            "E1004",
            "source is not readable",
            Some(format!("{}: {error}", path.display())),
            false,
        )
    })?;
    std::io::copy(&mut file, writer).map_err(|error| {
        KernelError::new(
            "E3005",
            "archive write failed",
            Some(error.to_string()),
            false,
        )
    })?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CompressInspect {
    pub file_count: u64,
    pub dir_count: u64,
    pub total_bytes: u64,
}

pub fn inspect_sources(sources: &[String]) -> Result<CompressInspect, KernelError> {
    let mut inspect = CompressInspect::default();
    for source in sources {
        let path = crate::fs::fs_path(Path::new(source));
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            KernelError::new(
                "E1004",
                "source is not readable",
                Some(format!("{}: {error}", path.display())),
                false,
            )
        })?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            inspect.dir_count += 1;
            walk_sources(&path, &mut inspect)?;
        } else {
            inspect.file_count += 1;
            inspect.total_bytes += metadata.len();
        }
    }
    Ok(inspect)
}

fn walk_sources(dir: &Path, inspect: &mut CompressInspect) -> Result<(), KernelError> {
    for entry in fs::read_dir(dir)
        .map_err(|error| {
            KernelError::new(
                "E1004",
                "source directory is not readable",
                Some(format!("{}: {error}", dir.display())),
                false,
            )
        })?
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            inspect.dir_count += 1;
            walk_sources(&path, inspect)?;
        } else {
            inspect.file_count += 1;
            inspect.total_bytes += metadata.len();
        }
    }
    Ok(())
}

fn decode_name(raw: &[u8], utf8_flag: bool) -> (Option<String>, bool) {
    if utf8_flag {
        return match std::str::from_utf8(raw) {
            Ok(name) => (Some(name.to_string()), false),
            Err(_) => (None, false),
        };
    }
    if raw.is_ascii() {
        return (Some(String::from_utf8_lossy(raw).into_owned()), false);
    }
    match transcode(
        raw,
        SourceEncoding::Gb18030,
        TargetEncoding::Utf8 { bom: false },
        InvalidSequencePolicy::Abort,
    ) {
        Ok(result) => match String::from_utf8(result.bytes) {
            Ok(name) => (Some(name), true),
            Err(_) => (None, true),
        },
        Err(_) => (None, true),
    }
}

/// 输出目录下已存在的路径（解压冲突预览用；只读、不跟随符号链接）。
pub fn existing_paths(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    walk(root, &mut paths);
    paths
}

fn walk(dir: &Path, paths: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(crate::fs::fs_path(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            paths.push(path.to_string_lossy().into_owned());
            walk(&path, paths);
        } else {
            paths.push(path.to_string_lossy().into_owned());
        }
    }
}

fn zip_error(error: zip::result::ZipError) -> KernelError {
    KernelError::new(
        "E3003",
        "archive cannot be read",
        Some(error.to_string()),
        false,
    )
}

fn io_error(path: &str, error: &std::io::Error) -> KernelError {
    KernelError::new(
        "E1004",
        "archive is not readable",
        Some(format!("{path}: {error}")),
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn corpus(fid: &str, file: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus")
            .join(fid)
            .join("out")
            .join(file)
    }

    #[test]
    fn z01_gbk_name_is_decoded_with_fallback() {
        let service = ZipArchiveService;
        let inspect = service
            .inspect(corpus("Z01", "gbk_name.zip").to_str().unwrap())
            .unwrap();
        assert_eq!(inspect.entries.len(), 1);
        assert_eq!(inspect.entries[0].name.as_deref(), Some("中文资料.txt"));
        assert!(inspect.entries[0].gbk_fallback);
        assert_eq!(inspect.entries[0].size, 77);
    }

    #[test]
    fn compress_writes_directory_tree_and_inspect_counts_sources() {
        let dir = std::env::temp_dir().join(format!(
            "batchdesk-compress-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        fs::write(dir.join("sub").join("b.txt"), b"beta").unwrap();
        let sources = vec![dir.to_string_lossy().into_owned()];
        let inspect = inspect_sources(&sources).unwrap();
        assert_eq!(inspect.file_count, 2);
        assert_eq!(inspect.total_bytes, 9);

        let service = ZipArchiveService;
        let bytes = service
            .compress(&sources, ArchiveCompression::Deflate)
            .unwrap();
        let zip_path = dir.join("out.zip");
        fs::write(&zip_path, &bytes).unwrap();
        let archive = service.inspect(zip_path.to_str().unwrap()).unwrap();
        let names: Vec<_> = archive
            .entries
            .iter()
            .filter_map(|entry| entry.name.as_deref())
            .collect();
        assert!(names.iter().any(|name| name.ends_with("a.txt")));
        assert!(names.iter().any(|name| name.ends_with("sub/b.txt")));
        let index = archive
            .entries
            .iter()
            .find(|entry| {
                entry
                    .name
                    .as_deref()
                    .is_some_and(|name| name.ends_with("a.txt"))
            })
            .unwrap()
            .index;
        let mut content = Vec::new();
        service
            .extract_entry(zip_path.to_str().unwrap(), index, &mut content)
            .unwrap();
        assert_eq!(content, b"alpha");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn z02_lists_safe_and_traversal_entries_and_extracts_safe_one() {
        let service = ZipArchiveService;
        let path = corpus("Z02", "traversal.zip");
        let inspect = service.inspect(path.to_str().unwrap()).unwrap();
        assert_eq!(inspect.entries.len(), 3);
        assert_eq!(inspect.entries[0].name.as_deref(), Some("safe/ok.txt"));
        assert!(inspect.entries[1].name.as_deref().unwrap().contains(".."));
        let mut output = Vec::new();
        service
            .extract_entry(path.to_str().unwrap(), 0, &mut output)
            .unwrap();
        assert_eq!(output.len(), 41);
        assert!(!output.is_empty());
    }
}
