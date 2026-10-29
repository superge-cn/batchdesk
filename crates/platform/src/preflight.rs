//! S01 预检事实采集（壳层；方案 §3.3、§4.9、§9.1、§5.13）。
//!
//! kernel 只做判定，磁盘可用空间、只读/占用、占位、长路径支持开关与 UNC
//! 可达性都必须由壳层采集后传入（kernel 不读盘，不变量 1）。本模块只采集
//! 事实，不决定"是否阻断"。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use kernel::error::KernelError;
use kernel::preflight::{PathCheck, UncCheck};

#[cfg(windows)]
use kernel::path::normalize;
#[cfg(unix)]
use std::io::ErrorKind;

use crate::fs::is_placeholder_file;

/// 磁盘安全余量下限（方案 §9.1 磁盘空间安全余量行、11.6-12）。
pub const DEFAULT_DISK_MARGIN_FLOOR_BYTES: u64 = 200 * 1024 * 1024;
/// 磁盘安全余量比例（方案 §9.1 同一行、11.6-12）。
pub const DEFAULT_DISK_MARGIN_RATIO: f64 = 0.10;
/// UNC 三项查询统一超时（方案 §4.9）。
pub const UNC_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 距离 `path` 最近的存在目录（用于对尚不存在的目标探测所在卷）。
pub fn probe_root(path: &Path) -> PathBuf {
    let mut current = if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    for _ in 0..64 {
        if current.exists() {
            return current;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    current
}

/// 返回目标卷的可用字节数（Unix statvfs；Windows GetDiskFreeSpaceExW）。
pub fn available_space(path: &Path) -> Result<u64, KernelError> {
    let root = probe_root(path);
    platform_available_space(&root)
}

#[cfg(unix)]
fn platform_available_space(path: &Path) -> Result<u64, KernelError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        KernelError::new(
            "E1003",
            "path contains a NUL byte",
            Some(path.display().to_string()),
            false,
        )
    })?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `stat` 指向本函数栈上的有效 MaybeUninit；c_path 以 NUL 结尾且
    // 生命周期覆盖调用；statvfs 只写该结构体。
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return Err(KernelError::new(
            if error.kind() == ErrorKind::NotFound {
                "E2001"
            } else {
                "E9001"
            },
            "cannot read free disk space",
            Some(format!("{}: {error}", path.display())),
            false,
        ));
    }
    // SAFETY: statvfs 成功返回时 `stat` 已完成初始化。
    let stat = unsafe { stat.assume_init() };
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(windows)]
fn platform_available_space(path: &Path) -> Result<u64, KernelError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: wide 以 NUL 结尾且长度覆盖调用；三个 out 指针均指向栈上有效 u64。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        return Err(KernelError::new(
            "E9001",
            "cannot read free disk space",
            Some(path.display().to_string()),
            false,
        ));
    }
    Ok(available)
}

#[cfg(not(any(unix, windows)))]
fn platform_available_space(path: &Path) -> Result<u64, KernelError> {
    Err(KernelError::new(
        "E5001",
        "free disk space is not supported on this platform",
        Some(path.display().to_string()),
        false,
    ))
}

/// 两个路径是否位于同一卷（跨卷移动需要额外空间，同卷改名不需要）。
pub fn same_volume(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = fs::metadata(probe_root(left));
        let right = fs::metadata(probe_root(right));
        match (left, right) {
            (Ok(left), Ok(right)) => left.dev() == right.dev(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        let left = volume_root(left);
        let right = volume_root(right);
        left.eq_ignore_ascii_case(&right)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        true
    }
}

#[cfg(windows)]
fn volume_root(path: &Path) -> String {
    let canonical = fs::canonicalize(path)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    let normalized = normalize(&canonical);
    if let Some(rest) = normalized.strip_prefix("//") {
        let mut parts = rest.splitn(3, '/');
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        return format!("//{server}/{share}");
    }
    if normalized.len() >= 2 && normalized.as_bytes().get(1) == Some(&b':') {
        return normalized[..2].to_string();
    }
    normalized
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteProbe {
    Writable,
    Readonly,
    Locked,
}

fn probe_write(path: &Path) -> WriteProbe {
    match fs::OpenOptions::new().write(true).open(path) {
        Ok(file) => {
            drop(file);
            WriteProbe::Writable
        }
        Err(error) => match error.raw_os_error() {
            // Windows 共享冲突 / 锁冲突；Unix ETXTBSY。
            Some(32) | Some(33) | Some(26) => WriteProbe::Locked,
            // Windows ERROR_ACCESS_DENIED；Unix EACCES / EPERM。
            Some(5) | Some(13) | Some(1) => WriteProbe::Readonly,
            _ => WriteProbe::Writable,
        },
    }
}

/// 采集单个路径的预检事实；路径不存在时不报错（由计划/执行层再判定）。
pub fn path_check(path: &Path, long_path_supported: bool) -> PathCheck {
    let display = path.to_string_lossy().into_owned();
    let metadata = fs::metadata(path).ok();
    let attribute_readonly = metadata
        .as_ref()
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(false);
    let probe = if metadata
        .as_ref()
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
        && !attribute_readonly
    {
        probe_write(path)
    } else {
        WriteProbe::Writable
    };
    PathCheck {
        path: display,
        canonical_path: fs::canonicalize(path)
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        readonly: attribute_readonly || probe == WriteProbe::Readonly,
        locked: probe == WriteProbe::Locked,
        placeholder: is_placeholder_file(path),
        long_path_supported,
    }
}

/// 判断路径是否为 UNC/网络形式（`\\server\share` 或 `//server/share`）。
pub fn is_network_path(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.starts_with("\\\\") || value.starts_with("//")
}

/// 探测 UNC 路径可达性；非网络路径返回 `None`。
///
/// 超时返回 `None`（暂不可达），调用方按方案 §4.9 降级为提示而非阻断。
/// 超时线程可能继续阻塞在系统调用上直到内核返回；其结果被丢弃。
pub fn probe_unc(path: &Path) -> Option<UncCheck> {
    if !is_network_path(path) {
        return None;
    }
    let target = path.to_path_buf();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("batchdesk-unc-probe".to_string())
        .spawn(move || {
            let reachable = fs::metadata(&target).is_ok();
            let _ = sender.send(reachable);
        });
    let reachable = match worker {
        Ok(_) => receiver.recv_timeout(UNC_PROBE_TIMEOUT).ok(),
        Err(_) => None,
    };
    Some(UncCheck {
        path: path.to_string_lossy().into_owned(),
        reachable,
    })
}

/// 系统长路径支持开关（方案 §5.13）：非 Windows 恒为 true。
#[cfg(not(windows))]
pub fn long_path_supported() -> bool {
    true
}

#[cfg(windows)]
pub fn long_path_supported() -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD,
    };

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let sub_key = wide(r"SYSTEM\CurrentControlSet\Control\FileSystem");
    let value_name = wide("LongPathsEnabled");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: 两个宽字符串均以 NUL 结尾；key 指向栈上有效 HKEY；
    // 注册表调用只读写传入句柄与缓冲区。
    unsafe {
        let status = RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub_key.as_ptr(), 0, KEY_READ, &mut key);
        if status != 0 {
            return false;
        }
        let mut data = 0u32;
        let mut data_size = std::mem::size_of::<u32>() as u32;
        let mut kind = 0u32;
        let status = RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            &mut data as *mut u32 as *mut u8,
            &mut data_size,
        );
        RegCloseKey(key);
        status == 0 && kind == REG_DWORD && data != 0
    }
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
            let path = std::env::temp_dir().join(format!("batchdesk-preflight-{nanos}"));
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
    fn default_margin_constants_match_the_baseline() {
        assert_eq!(DEFAULT_DISK_MARGIN_FLOOR_BYTES, 200 * 1024 * 1024);
        assert_eq!(DEFAULT_DISK_MARGIN_RATIO, 0.10);
        assert_eq!(UNC_PROBE_TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn available_space_reads_a_positive_value_for_a_temp_directory() {
        let temp = TempDir::new();
        let available = available_space(&temp.0).unwrap();
        assert!(available > 0);
    }

    #[test]
    fn same_volume_is_true_for_two_paths_on_the_same_temp_dir() {
        let temp = TempDir::new();
        let first = temp.0.join("a.txt");
        let second = temp.0.join("b.txt");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();
        assert!(same_volume(&first, &second));
        assert!(same_volume(&first, &temp.0));
    }

    #[test]
    fn path_check_reads_metadata_without_error_for_missing_paths() {
        let temp = TempDir::new();
        let missing = temp.0.join("missing.txt");
        let check = path_check(&missing, true);
        assert!(check.canonical_path.is_none());
        assert!(!check.readonly);
        assert!(!check.locked);
        assert!(!check.placeholder);
        assert!(check.long_path_supported);
    }

    #[test]
    fn path_check_reports_readonly_files() {
        let temp = TempDir::new();
        let path = temp.0.join("readonly.txt");
        fs::write(&path, b"x").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let check = path_check(&path, true);
        assert!(check.readonly);
        assert!(!check.locked);
        // 还原可写，避免临时目录清理失败。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o644);
            fs::set_permissions(&path, permissions).unwrap();
        }
        #[cfg(windows)]
        {
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(false);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }

    #[test]
    fn non_network_paths_are_not_probed() {
        let temp = TempDir::new();
        assert!(probe_unc(&temp.0).is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn long_paths_are_always_supported_outside_windows() {
        assert!(long_path_supported());
    }
}
