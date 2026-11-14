//! C10 ZIP 解压端口实现（方案 §5.11）：原始名读取、GBK 回退与条目解压。

use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use kernel::archive::{
    ArchiveCompression, ArchiveEntryInfo, ArchiveEntryKind, ArchiveInspect, ArchiveManifestItem,
    ArchiveService, CompressedArchive,
};
use kernel::encoding::{transcode, InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use kernel::error::KernelError;

pub struct ZipArchiveService;

impl ArchiveService for ZipArchiveService {
    fn inspect(&self, archive: &str) -> Result<ArchiveInspect, KernelError> {
        let path = crate::fs::fs_path(Path::new(archive));
        let metadata = fs::metadata(&path).map_err(|error| io_error(archive, &error))?;
        let file = fs::File::open(&path).map_err(|error| io_error(archive, &error))?;
        let zip = zip::ZipArchive::new(file).map_err(zip_error)?;
        let mut entries = Vec::with_capacity(zip.len());
        for index in 0..zip.len() {
            // 只读中央目录元数据（`by_index_data`），不解码任何条目数据：加密包也能列出
            // 条目，密码只在 `extract_entry` 处才需要（ADR-0074 §3.1）。旧的
            // `by_index` 会对加密条目直接返回 "Password required to decrypt file"，
            // 使加密包连预览都出不来。
            let entry = zip.by_index_data(index).map_err(zip_error)?;
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
        password: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let path = crate::fs::fs_path(Path::new(archive));
        let file = fs::File::open(&path).map_err(|error| io_error(archive, &error))?;
        let mut zip = zip::ZipArchive::new(file).map_err(zip_error)?;
        let index = usize::try_from(index)
            .map_err(|_| KernelError::invalid_params("archive entry index is out of range"))?;
        let secret = password
            .map(str::as_bytes)
            .filter(|value| !value.is_empty());
        let mut entry = match secret {
            Some(secret) => zip.by_index_decrypt(index, secret).map_err(zip_error)?,
            // 不给密码时 `by_index` 对加密条目返回 PASSWORD_REQUIRED，由 `zip_error`
            // 映射成 E3008；未加密条目照常读。
            None => zip.by_index(index).map_err(zip_error)?,
        };
        let encrypted = entry.encrypted();
        std::io::copy(&mut entry, writer).map_err(|error| {
            let mapped = crate::fs::io_error(&error, Path::new(archive));
            if mapped.code == "E2005" {
                return mapped;
            }
            // 加密条目的读取失败（ZipCrypto 的 CRC、AES 的认证码）与"密码错"不可区分，
            // 统一按 E3009 报（ADR-0074 §5.6）。
            if encrypted && secret.is_some() {
                KernelError::new(
                    "E3009",
                    "archive password is wrong or the archive is damaged",
                    Some(error.to_string()),
                    false,
                )
            } else {
                KernelError::new(
                    "E3005",
                    "archive entry write failed",
                    Some(error.to_string()),
                    false,
                )
            }
        })?;
        Ok(())
    }

    fn compress(
        &self,
        sources: &[String],
        compression: ArchiveCompression,
    ) -> Result<CompressedArchive, KernelError> {
        let options =
            zip::write::SimpleFileOptions::default().compression_method(match compression {
                ArchiveCompression::Store => zip::CompressionMethod::Stored,
                ArchiveCompression::Deflate => zip::CompressionMethod::Deflated,
            });
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let mut names = BTreeSet::new();
        // 方案 §5.11「压缩完成后自动生成文件清单」：在遍历源的同时收集逐文件事实，
        // 不再二次解析产物（对大压缩包那是整整多一遍全量读）。
        let mut manifest = Vec::new();
        for source in sources {
            let path = crate::fs::fs_path(Path::new(source));
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| KernelError::invalid_params("source path has no file name"))?;
            add_path(
                &mut writer,
                &path,
                name.to_string(),
                options,
                &mut names,
                &mut manifest,
            )?;
        }
        let cursor = writer.finish().map_err(zip_error)?;
        Ok(CompressedArchive {
            bytes: cursor.into_inner(),
            manifest,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn add_path<W: Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    path: &Path,
    entry_name: String,
    options: zip::write::SimpleFileOptions,
    names: &mut BTreeSet<String>,
    manifest: &mut Vec<ArchiveManifestItem>,
) -> Result<(), KernelError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        let mapped = crate::fs::io_error(&error, path);
        if mapped.code == "E2005" {
            return mapped;
        }
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
                let mapped = crate::fs::io_error(&error, path);
                if mapped.code == "E2005" {
                    return mapped;
                }
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
                manifest,
            )?;
        }
        return Ok(());
    }
    if !names.insert(entry_name.clone()) {
        return Err(KernelError::invalid_params(format!(
            "duplicate archive entry: {entry_name}"
        )));
    }
    // 先登记再写：清单反映的是"产物里有哪些文件"，写失败时整体报错、清单随之丢弃。
    manifest.push(ArchiveManifestItem {
        name: entry_name.clone(),
        size: metadata.len(),
    });
    writer.start_file(entry_name, options).map_err(zip_error)?;
    let mut file = fs::File::open(path).map_err(|error| {
        let mapped = crate::fs::io_error(&error, path);
        if mapped.code == "E2005" {
            return mapped;
        }
        KernelError::new(
            "E1004",
            "source is not readable",
            Some(format!("{}: {error}", path.display())),
            false,
        )
    })?;
    std::io::copy(&mut file, writer).map_err(|error| {
        let mapped = crate::fs::io_error(&error, path);
        if mapped.code == "E2005" {
            return mapped;
        }
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
    use zip::result::ZipError;
    // 密码相关的三种情形各有不同的处置动作，不能都折进"格式不支持"（ADR-0074 §3.3）：
    // 缺密码 → 补密码；密码错 → 换密码；其余 → 换工具。
    match &error {
        ZipError::UnsupportedArchive(message) if *message == ZipError::PASSWORD_REQUIRED => {
            KernelError::new(
                "E3008",
                "archive is encrypted; a password is required",
                Some("pass the password when applying".to_string()),
                false,
            )
        }
        ZipError::InvalidPassword => KernelError::new(
            "E3009",
            "archive password is wrong or the archive is damaged",
            Some(error.to_string()),
            false,
        ),
        // D1：ZIP 中央目录/条目读取中遇到的网络断线要保持 E2005，才能让 C10 执行器
        // 的重试端口接住；其余 I/O 仍按 E3003「换工具」处置。
        ZipError::Io(io) => {
            let mapped = crate::fs::io_error(io, Path::new("<archive>"));
            if mapped.code == "E2005" {
                mapped
            } else {
                KernelError::new(
                    "E3003",
                    "archive cannot be read",
                    Some(error.to_string()),
                    false,
                )
            }
        }
        _ => KernelError::new(
            "E3003",
            "archive cannot be read",
            Some(error.to_string()),
            false,
        ),
    }
}

fn io_error(path: &str, error: &std::io::Error) -> KernelError {
    // D1：网络断线的 I/O 错误必须保持 E2005（retryable），不能被归档层的
    // E1004「不可读」吞掉，否则 C10 的 inspect/extract 重试永远等不到。
    let mapped = crate::fs::io_error(error, Path::new(path));
    if mapped.code == "E2005" {
        return mapped;
    }
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

    /// Z01 的**写入侧**对称面：压缩产出的 ZIP 必须让 Windows 资源管理器／7-Zip／
    /// macOS 都能正确读出非 ASCII 文件名。做法是「名字字节写 UTF-8 ＋ 通用标志位
    /// bit 11 置上」；**本地头与中央目录两处都要置**——只写一处会让按本地头解码的
    /// 工具读成 CP437 乱码。
    ///
    /// 台账原写「压缩时不写 bit11/GB18030 标志」，实测为误：zip 9.1.3 对合法 UTF-8
    /// 名会在两处置 bit 11。本用例把这条事实钉住，防止将来换库／改选项时悄悄回退。
    #[test]
    fn compress_marks_non_ascii_names_utf8_with_bit11_in_both_headers() {
        let dir = std::env::temp_dir().join(format!(
            "batchdesk-compress-utf8-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("中文文件.txt"), b"payload").unwrap();
        fs::write(dir.join("plain.txt"), b"plain").unwrap();
        let sources = vec![dir.to_string_lossy().into_owned()];
        let service = ZipArchiveService;
        let bytes = service
            .compress(&sources, ArchiveCompression::Deflate)
            .unwrap()
            .bytes;

        // ① 中央目录（zip crate 读取口）：非 ASCII 名必须是 UTF-8 字节且 bit 11 置上
        let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let mut non_ascii: Vec<Vec<u8>> = Vec::new();
        for index in 0..zip.len() {
            let entry = zip.by_index_data(index).unwrap();
            let raw = entry.name_raw().to_vec();
            let flags = entry.flags().as_u16();
            if raw.iter().all(u8::is_ascii) {
                assert_eq!(
                    flags & 0x0800,
                    0,
                    "纯 ASCII 名不该置 bit 11（保持与老工具的兼容）：{raw:?}"
                );
                continue;
            }
            assert!(
                std::str::from_utf8(&raw).is_ok(),
                "非 ASCII 名必须是合法 UTF-8 字节：{raw:?}"
            );
            assert!(
                flags & 0x0800 != 0,
                "中央目录里非 ASCII 名必须置 bit 11（UTF-8 标志）：{raw:?}"
            );
            non_ascii.push(raw.clone());
        }
        assert_eq!(non_ascii.len(), 1, "应恰好一个非 ASCII 条目");

        // ② 本地头（原始字节扫描）：名字出现处往前 30 字节是头起点，+6 是 flags
        for raw in &non_ascii {
            let needle = bytes
                .windows(raw.len())
                .position(|window| window == raw.as_slice())
                .expect("本地头里应能找到该名字");
            // 本地头布局：签名(4)＋版本(2)＋**flags(2)**＋方法(2)…，名字在 +30
            let flags_at = needle - 30 + 6;
            let flags = u16::from_le_bytes([bytes[flags_at], bytes[flags_at + 1]]);
            assert!(
                flags & 0x0800 != 0,
                "本地头里非 ASCII 名也必须置 bit 11，否则按本地头解码的工具会乱码：{raw:?}"
            );
        }

        // ③ 回环：自己压自己解，名字原样保留，且不触发 GBK 回退（说明读侧认为是 UTF-8）
        let zip_path = dir.join("out.zip");
        fs::write(&zip_path, &bytes).unwrap();
        let inspect = service.inspect(zip_path.to_str().unwrap()).unwrap();
        let names: Vec<_> = inspect
            .entries
            .iter()
            .filter_map(|entry| entry.name.as_deref())
            .collect();
        assert!(
            names.iter().any(|name| name.ends_with("中文文件.txt")),
            "回环后中文名必须原样：{names:?}"
        );
        assert!(
            inspect.entries.iter().all(|entry| !entry.gbk_fallback),
            "我们自己写的是 UTF-8＋bit11，读侧不该走 GBK 回退：{:?}",
            inspect.entries
        );

        let _ = fs::remove_dir_all(&dir);
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
        let compressed = service
            .compress(&sources, ArchiveCompression::Deflate)
            .unwrap();
        // 方案 §5.11「压缩完成后自动生成文件清单」：清单与产物同源，逐文件可查。
        // 源是目录时，ZIP 会保留顶层目录名（与"双击解压出一个文件夹"的预期一致）。
        let mut manifest_names: Vec<&str> = compressed
            .manifest
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        manifest_names.sort_unstable();
        let root = dir
            .file_name()
            .and_then(|name| name.to_str())
            .expect("temp dir has a name");
        assert_eq!(
            manifest_names,
            vec![format!("{root}/a.txt"), format!("{root}/sub/b.txt")]
        );
        assert_eq!(
            compressed
                .manifest
                .iter()
                .map(|item| item.size)
                .sum::<u64>(),
            9,
            "清单总字节数必须与实际写出的文件体积一致"
        );
        let bytes = compressed.bytes;
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
            .extract_entry(zip_path.to_str().unwrap(), index, None, &mut content)
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
            .extract_entry(path.to_str().unwrap(), 0, None, &mut output)
            .unwrap();
        assert_eq!(output.len(), 41);
        assert!(!output.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn archive_network_io_error_keeps_e2005() {
        let error = std::io::Error::from_raw_os_error(64);
        let mapped = io_error(r"\\server\share\a.zip", &error);
        assert_eq!(mapped.code, "E2005");
        assert!(mapped.retryable);
    }

    #[cfg(windows)]
    #[test]
    fn archive_zip_io_network_keeps_e2005() {
        let mapped = zip_error(zip::result::ZipError::Io(
            std::io::Error::from_raw_os_error(64),
        ));
        assert_eq!(mapped.code, "E2005");
        assert!(mapped.retryable);
    }
}
