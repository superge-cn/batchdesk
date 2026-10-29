//! Protected-directory preparation (S02) for shell commands.

use std::env;
use std::path::{Path, PathBuf};

use kernel::error::KernelError;
use kernel::path::{try_normalize, ProtectedDir};

pub fn make_absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

pub fn protected_dirs_from_paths(paths: &[PathBuf]) -> Result<Vec<ProtectedDir>, KernelError> {
    paths.iter().map(|path| prepare(path)).collect()
}

pub fn protected_dirs_from_strings(paths: &[String]) -> Result<Vec<ProtectedDir>, KernelError> {
    paths.iter().map(|path| prepare(Path::new(path))).collect()
}

fn prepare(path: &Path) -> Result<ProtectedDir, KernelError> {
    let absolute = make_absolute(path);
    let literal = absolute.to_string_lossy().into_owned();
    try_normalize(&literal)?;
    // canonical 显示值经适配层剥 `\\?\` 前缀；kernel 比较前会再归一（设计 04 §N4）。
    let canonical = crate::fs::canonicalize_display(&absolute);
    Ok(ProtectedDir::with_canonical(literal, canonical))
}
