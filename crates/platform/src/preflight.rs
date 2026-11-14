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

use crate::fs::{canonicalize_display, fs_path, is_placeholder_file};

/// 磁盘安全余量下限（方案 §9.1 磁盘空间安全余量行、11.6-12）。
pub const DEFAULT_DISK_MARGIN_FLOOR_BYTES: u64 = 200 * 1024 * 1024;
/// 磁盘安全余量比例（方案 §9.1 同一行、11.6-12）。
pub const DEFAULT_DISK_MARGIN_RATIO: f64 = 0.10;
/// UNC 三项查询统一超时（方案 §4.9）。
pub const UNC_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 距离 `path` 最近的存在目录（用于对尚不存在的目标探测所在卷）。
pub fn probe_root(path: &Path) -> PathBuf {
    // 长路径下 `Path::exists` 不经前缀会误判不存在，统一走 `fs_path`。
    let is_file = fs::metadata(fs_path(path))
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    let mut current = if is_file {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    for _ in 0..64 {
        if fs::metadata(fs_path(&current)).is_ok() {
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
    match fs::OpenOptions::new().write(true).open(fs_path(path)) {
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
    let metadata = fs::metadata(fs_path(path)).ok();
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
        // canonical 显示值由适配层剥前缀（设计 04 §"Windows 长路径"）。
        canonical_path: canonicalize_display(path),
        readonly: attribute_readonly || probe == WriteProbe::Readonly,
        locked: probe == WriteProbe::Locked,
        placeholder: is_placeholder_file(path),
        long_path_supported,
    }
}

/// 计划期的轻量写入权限事实：只读走元数据属性位（POSIX 写位／Windows
/// FILE_ATTRIBUTE_READONLY），**不做写句柄探测**——占用是瞬时状态且要为每个文件
/// 开一次写句柄，十万级条目下这个代价不该由"生成预览"来付；占用判定留在预检面
/// （`path_check`）。`locked` 因此恒为 false，`canonical_path` 不采集（计划按字面
/// 路径命中事实，规范形式由预检与执行层各自复核）。
pub fn path_check_light(path: &Path, long_path_supported: bool) -> PathCheck {
    let metadata = fs::metadata(fs_path(path)).ok();
    let readonly = metadata
        .as_ref()
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(false);
    PathCheck {
        path: path.to_string_lossy().into_owned(),
        canonical_path: None,
        readonly,
        locked: false,
        placeholder: is_placeholder_file(path),
        long_path_supported,
    }
}

/// 单个路径的只读属性位（POSIX 写位／Windows FILE_ATTRIBUTE_READONLY）。
/// 给"条目本身已带只读事实"的计划入口用（C03 原地改写，源即写入目标），
/// 避免为这类入口构造整份 `PathCheck`。
pub fn is_read_only(path: &Path) -> bool {
    fs::metadata(fs_path(path))
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(false)
}

/// 判断路径是否为 UNC/网络形式（`\\server\share` 或 `//server/share`）。
pub fn is_network_path(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.starts_with("\\\\") || value.starts_with("//")
}

/// 取网络路径的**共享根**（`//host/share`）；非网络路径或信息不足时返回 `None`。
///
/// 为什么不能复用 `probe_root`：它找的是"最近的存在目录"（为卷剩余空间服务），
/// 而共享不可达时 `\\host\share` 本身就不存在，会一路退到本机根目录——于是 S01 的
/// 可达性探测实际探的是本机盘，方案 §4.9 要求的 E2005「暂不可达」降级永远不出现。
/// 两种写法（`\\` 与 `//`）都归一到 `//host/share`，交给 `probe_unc` 探测。
pub fn unc_share_root(path: &Path) -> Option<PathBuf> {
    if !is_network_path(path) {
        return None;
    }
    let value = path.to_string_lossy().replace('\\', "/");
    let mut segments = value.split('/').filter(|segment| !segment.is_empty());
    let host = segments.next()?;
    let share = segments.next()?;
    if host.is_empty() || share.is_empty() {
        return None;
    }
    Some(PathBuf::from(format!("//{host}/{share}")))
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
            let reachable = fs::metadata(crate::fs::fs_path(&target)).is_ok();
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
            // Windows 只读是属性位，这里就是恢复可写的正确做法；Unix 分支在上面用 set_mode。
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }

    #[test]
    fn path_check_light_reads_the_attribute_bit_without_probing_writes() {
        let temp = TempDir::new();
        let read_only = temp.0.join("readonly.txt");
        fs::write(&read_only, b"x").unwrap();
        let mut permissions = fs::metadata(&read_only).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&read_only, permissions).unwrap();

        let check = path_check_light(&read_only, true);
        assert!(check.readonly, "只读属性位必须被计划期事实看见");
        assert!(
            !check.locked,
            "占用要开写句柄才判得出，属预检面；计划期不得声称判过占用"
        );
        assert_eq!(check.canonical_path, None, "轻量档不做 canonicalize");
        assert!(is_read_only(&read_only), "条目级只读端口与轻量档同口径");

        let writable = temp.0.join("writable.txt");
        fs::write(&writable, b"x").unwrap();
        assert!(!path_check_light(&writable, true).readonly);
        assert!(!is_read_only(&writable));

        // 路径不存在不是错误：计划/执行层再判定（E2001），事实层只报"看不见"。
        let missing = temp.0.join("missing.txt");
        let check = path_check_light(&missing, true);
        assert!(!check.readonly && !check.locked && !check.placeholder);
        assert!(!is_read_only(&missing), "路径不存在时不得凭空报只读");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&read_only).unwrap().permissions();
            permissions.set_mode(0o644);
            fs::set_permissions(&read_only, permissions).unwrap();
        }
        #[cfg(windows)]
        {
            let mut permissions = fs::metadata(&read_only).unwrap().permissions();
            // Windows 只读是属性位，这里就是恢复可写的正确做法；Unix 分支在上面用 set_mode。
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(&read_only, permissions).unwrap();
        }
    }

    #[test]
    fn unc_share_root_picks_the_share_not_the_local_root() {
        // 共享不可达时 probe_root 会退到本机根目录，可达性预检就变成探本机盘（S01 的
        // E2005 永不出现）。共享根解析必须停在 `//host/share`。
        for (spelling, expected) in [
            (r"\\nas01\资料\2026\a.txt", "//nas01/资料"),
            ("//nas01/资料/2026/a.txt", "//nas01/资料"),
            (r"\\NAS01\资料", "//NAS01/资料"),
        ] {
            let share = unc_share_root(Path::new(spelling))
                .unwrap_or_else(|| panic!("{spelling} 应解析出共享根"));
            assert_eq!(
                share.to_string_lossy().replace('\\', "/"),
                expected,
                "{spelling} 解析结果不对"
            );
        }
        // 主机或共享名为空 → 不猜；本地路径 → None。
        assert!(unc_share_root(Path::new(r"\\nas01")).is_none());
        assert!(unc_share_root(Path::new("\\\\")).is_none());
        assert!(unc_share_root(Path::new("/data/out/a.txt")).is_none());
        assert!(unc_share_root(Path::new("C:\\data\\out")).is_none());
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

    /// 文件树快照：路径 →（长度, 内容摘要, 修改时间）。用于"零写入"断言——
    /// 采集前后各拍一次，任何新增/修改/删除都会让两次快照不等。
    fn tree_snapshot(root: &Path) -> Vec<(String, u64, u64, Option<SystemTime>)> {
        let mut entries = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let read = match fs::read_dir(&dir) {
                Ok(read) => read,
                Err(_) => continue,
            };
            for item in read.flatten() {
                let path = item.path();
                let metadata = match fs::metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                if metadata.is_dir() {
                    stack.push(path);
                    continue;
                }
                let len = metadata.len();
                let modified = metadata.modified().ok();
                let hash = fs::read(&path)
                    .map(|bytes| {
                        let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
                        for byte in bytes {
                            digest ^= u64::from(byte);
                            digest = digest.wrapping_mul(0x0000_0100_0000_0001);
                        }
                        digest
                    })
                    .unwrap_or(0);
                entries.push((path.to_string_lossy().into_owned(), len, hash, modified));
            }
        }
        entries.sort();
        entries
    }

    /// S01-EXE-02：壳层事实采集（`probe_root`／`stat`／`path_check(_light)`／
    /// `is_read_only`／`is_network_path`／`unc_share_root`／`same_volume`／
    /// `long_path_supported`／`available_space`）**不得改动文件树**。
    /// 预检的意义就是把"执行到一半失败"前移为"执行前阻断"，采集阶段一旦写盘
    /// 就自相矛盾；本用例把这条接线钉住，而不只是"跑一遍不报错"。
    #[test]
    fn fact_gathering_writes_nothing_to_the_tree() {
        let temp = TempDir::new();
        let nested = temp.0.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("x.txt");
        fs::write(&file, b"payload").unwrap();
        let missing = temp.0.join("a/b/ghost.txt");
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();

        let before = tree_snapshot(&temp.0);

        // 全部走只读口：包含"路径不存在"与"只读文件"两种输入。
        let _ = probe_root(&missing);
        let _ = probe_root(&file);
        let _ = available_space(&missing).expect("临时目录应能取到可用空间");
        let _ = path_check(&file, true);
        let _ = path_check_light(&file, true);
        let _ = path_check(&missing, false);
        let _ = is_read_only(&file);
        let _ = is_placeholder_file(&file);
        let _ = is_network_path(&file);
        let _ = unc_share_root(&file);
        let _ = same_volume(&file, &temp.0);
        let _ = long_path_supported();

        let after = tree_snapshot(&temp.0);
        assert_eq!(before, after, "事实采集不得改动文件树（预检阶段零写入）");

        // 反向对照：真写一次必须让快照变化，否则上面的断言是空转。
        // （`file` 已被置只读，故另取一个可写文件做对照。）
        let control = nested.join("control.txt");
        fs::write(&control, b"before").unwrap();
        let with_control = tree_snapshot(&temp.0);
        fs::write(&control, b"after").unwrap();
        assert_ne!(
            tree_snapshot(&temp.0),
            with_control,
            "快照必须能感知写入，否则零写入断言无意义"
        );
    }
}
