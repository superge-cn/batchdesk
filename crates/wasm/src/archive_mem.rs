//! C10 的 `ArchiveService` 在浏览器沙箱上的实现（ADR-0112）。
//!
//! 桌面那份（`platform::ZipArchiveService`）建在 `std::fs` 上：按**路径**打开文件、
//! 逐块读。浏览器沙箱里的「文件」只是 `MemoryFs` 里的字节，没有可供 zip 库打开的
//! 文件句柄——所以这里把包整体读进 `Cursor`，其余语义逐条对齐桌面：同一个 `zip`
//! crate（9.1.3／deflate-flate2＋aes-crypto，纯 Rust 后端，可编到 wasm32）、
//! 同一个名字解码（`kernel::archive::decode_entry_name`）、同一套错误码
//! （E3008 缺密码／E3009 密码错／E3003 读不了）。
//!
//! 端口放在 wasm crate 而不是 kernel：kernel 是纯函数层（不变量 1），ZIP 编解码是
//! 适配层的事；wasm crate 是「浏览器壳层」，与 platform 在桌面侧的角色对称。
//!
//! 边界：
//! - 只认沙箱内的路径（`MemoryFs` 的键口径，见 `memfs::normalize_key`）；
//! - 压缩的目录源按「文件系统里已有的事实」展开——沙箱里没有空目录，因此不会
//!   出现「只有一条目录条目」的产物（与「目录选择」在浏览器里只能拿到文件同源）；
//! - 加密条目要密码时按 E3008 报，密码错按 E3009，绝不解半截。

use std::collections::BTreeSet;
use std::io::{Cursor, Seek, Write};

use kernel::archive::{
    decode_entry_name, ArchiveCompression, ArchiveEntryInfo, ArchiveEntryKind, ArchiveInspect,
    ArchiveManifestItem, ArchiveService, CompressedArchive,
};
use kernel::conflict::file_name_of;
use kernel::error::KernelError;
use kernel::fs::{EntryKind, FsAdapter};
use kernel::memfs::{normalize_key, MemoryFs};

/// 沙箱内的归档端口：包只以字节形式存在，不落盘。
pub struct MemoryArchiveService<'a> {
    fs: &'a MemoryFs,
}

impl<'a> MemoryArchiveService<'a> {
    pub fn new(fs: &'a MemoryFs) -> Self {
        Self { fs }
    }

    fn archive_bytes(&self, archive: &str) -> Result<Vec<u8>, KernelError> {
        self.fs.read_file(archive)
    }
}

impl ArchiveService for MemoryArchiveService<'_> {
    fn inspect(&self, archive: &str) -> Result<ArchiveInspect, KernelError> {
        let bytes = self.archive_bytes(archive)?;
        let archive_size = bytes.len() as u64;
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
        let mut entries = Vec::with_capacity(zip.len());
        for index in 0..zip.len() {
            // 只读中央目录元数据（`by_index_data`），不解码条目数据：加密包也能列出
            // 条目，密码只在 `extract_entry` 处才需要（ADR-0074 §3.1，与桌面同一手法）。
            let entry = zip.by_index_data(index).map_err(zip_error)?;
            let utf8_flag = entry.flags().as_u16() & 0x0800 != 0;
            let (name, gbk_fallback) = decode_entry_name(entry.name_raw(), utf8_flag);
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
            archive_size,
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
        let bytes = self.archive_bytes(archive)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
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
            // 加密条目的读取失败（ZipCrypto 的 CRC、AES 的认证码）与「密码错」不可
            // 区分，统一按 E3009 报（ADR-0074 §5.6，与桌面同一处置）。
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
        // 清单在遍历源的同时收集（方案 §5.11），不二次解析产物。
        let mut manifest = Vec::new();
        for source in sources {
            let name = file_name_of(source);
            if name.is_empty() {
                return Err(KernelError::invalid_params("source path has no file name"));
            }
            self.add_path(
                &mut writer,
                source,
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

impl MemoryArchiveService<'_> {
    /// 把一个源（文件或目录子树）写进包；目录按「先目录条目、后其下文件」的顺序。
    fn add_path<W: Write + Seek>(
        &self,
        writer: &mut zip::ZipWriter<W>,
        path: &str,
        entry_name: String,
        options: zip::write::SimpleFileOptions,
        names: &mut BTreeSet<String>,
        manifest: &mut Vec<ArchiveManifestItem>,
    ) -> Result<(), KernelError> {
        let stat = self.fs.stat(path).map_err(|_| {
            KernelError::new(
                "E1004",
                "source is not readable",
                Some(path.to_string()),
                false,
            )
        })?;
        let entry_name = entry_name.replace('\\', "/").replace("//", "/");
        let entry_name = entry_name.trim_matches('/').to_string();
        if entry_name.is_empty() {
            return Err(KernelError::invalid_params("source path has no file name"));
        }
        if stat.kind == EntryKind::Dir {
            let root = normalize_key(path);
            let prefix = if root == "/" {
                "/".to_string()
            } else {
                format!("{root}/")
            };
            let dir_name = format!("{}/", entry_name.trim_end_matches('/'));
            if !names.insert(dir_name.clone()) {
                return Err(KernelError::invalid_params(format!(
                    "duplicate archive entry: {dir_name}"
                )));
            }
            writer
                .add_directory(dir_name.clone(), options)
                .map_err(zip_error)?;
            // 子树：沙箱只存文件键（目录是派生的），所以从文件键反推中间目录。
            let mut descendants: Vec<(String, String)> = Vec::new();
            for file in self.fs.file_paths() {
                if let Some(relative) = file.strip_prefix(&prefix) {
                    if !relative.is_empty() {
                        descendants.push((relative.to_string(), file));
                    }
                }
            }
            let mut directories: BTreeSet<String> = BTreeSet::new();
            for (relative, _) in &descendants {
                let mut parts: Vec<&str> = relative.split('/').collect();
                parts.pop();
                let mut current = String::new();
                for part in parts {
                    if !current.is_empty() {
                        current.push('/');
                    }
                    current.push_str(part);
                    directories.insert(current.clone());
                }
            }
            // 前缀序保证父目录先于子目录（父是子的字符串前缀，排序必在前）。
            for directory in directories {
                let name = format!("{dir_name}{directory}/");
                if !names.insert(name.clone()) {
                    return Err(KernelError::invalid_params(format!(
                        "duplicate archive entry: {name}"
                    )));
                }
                writer.add_directory(name, options).map_err(zip_error)?;
            }
            for (relative, file) in descendants {
                self.add_file(
                    writer,
                    &file,
                    format!("{dir_name}{relative}"),
                    options,
                    names,
                    manifest,
                )?;
            }
            return Ok(());
        }
        self.add_file(writer, path, entry_name, options, names, manifest)
    }

    fn add_file<W: Write + Seek>(
        &self,
        writer: &mut zip::ZipWriter<W>,
        path: &str,
        entry_name: String,
        options: zip::write::SimpleFileOptions,
        names: &mut BTreeSet<String>,
        manifest: &mut Vec<ArchiveManifestItem>,
    ) -> Result<(), KernelError> {
        let data = self.fs.read_file(path).map_err(|_| {
            KernelError::new(
                "E1004",
                "source is not readable",
                Some(path.to_string()),
                false,
            )
        })?;
        if !names.insert(entry_name.clone()) {
            return Err(KernelError::invalid_params(format!(
                "duplicate archive entry: {entry_name}"
            )));
        }
        // 先登记再写：清单反映的是「产物里有哪些文件」，写失败时整体报错、清单丢弃。
        manifest.push(ArchiveManifestItem {
            name: entry_name.clone(),
            size: data.len() as u64,
        });
        writer.start_file(entry_name, options).map_err(zip_error)?;
        writer.write_all(&data).map_err(|error| {
            KernelError::new(
                "E3005",
                "archive write failed",
                Some(error.to_string()),
                false,
            )
        })?;
        Ok(())
    }
}

/// ZIP 错误 → 内核错误码（与 `platform::ZipArchiveService` 同口径）。
///
/// 唯一差异是 `ZipError::Io`：桌面把其中的网络断线保留成 E2005 好让执行器的重试
/// 端口接住，而这里的读写对象是内存游标，不可能出现网络错误，因此统一按 E3003。
fn zip_error(error: zip::result::ZipError) -> KernelError {
    use zip::result::ZipError;
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
        _ => KernelError::new(
            "E3003",
            "archive cannot be read",
            Some(error.to_string()),
            false,
        ),
    }
}
