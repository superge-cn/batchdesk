//! C10 ZIP 解压端口实现（方案 §5.11）：原始名读取、GBK 回退与条目解压。

use std::fs;
use std::io::Write;
use std::path::Path;

use kernel::archive::{ArchiveEntryInfo, ArchiveEntryKind, ArchiveInspect, ArchiveService};
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
