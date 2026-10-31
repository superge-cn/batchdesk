//! C03 文件检查：读取内容并调用内核计数/CSV 结构检查（整段读取；流式窗口后续补齐）。

use std::fs;
use std::path::Path;

use kernel::csv::Delimiter;
use kernel::error::KernelError;
use kernel::replace_content::{
    inspect_bytes, ContentStructure, CsvMode, ReplaceInspect, ReplaceMode,
};

pub struct ReplaceFileInspect {
    pub sample: Vec<u8>,
    pub file_size: u64,
    pub structure: ContentStructure,
    pub inspect: ReplaceInspect,
}

pub fn inspect_file(
    path: &Path,
    mode: ReplaceMode,
    find: &str,
    replacement: &str,
    csv_mode: CsvMode,
    delimiter: Option<Delimiter>,
    sample_bytes: usize,
) -> Result<ReplaceFileInspect, KernelError> {
    if sample_bytes == 0 {
        return Err(KernelError::invalid_params(
            "sample bytes must be a positive count",
        ));
    }
    let fs_path = crate::fs::fs_path(path);
    let metadata = fs::metadata(&fs_path).map_err(|error| read_error(path, &error))?;
    if !metadata.is_file() {
        return Err(KernelError::invalid_params("source must be a regular file"));
    }
    let data = fs::read(&fs_path).map_err(|error| read_error(path, &error))?;
    let structure = ContentStructure::resolve(&path.to_string_lossy());
    let inspect = inspect_bytes(
        &data,
        mode,
        find,
        replacement,
        csv_mode,
        structure,
        delimiter,
    )?;
    let mut sample = data;
    sample.truncate(sample_bytes);
    Ok(ReplaceFileInspect {
        sample,
        file_size: metadata.len(),
        structure,
        inspect,
    })
}

fn read_error(path: &Path, error: &std::io::Error) -> KernelError {
    KernelError::new(
        "E1004",
        "file is not readable",
        Some(format!("{}: {error}", path.display())),
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "batchdesk-replace-{}-{nanos}-{sequence}",
                std::process::id()
            ));
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
    fn inspects_text_matches_and_context() {
        let dir = TempDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, b"alpha beta alpha").unwrap();
        let inspect = inspect_file(
            &path,
            ReplaceMode::Plain,
            "alpha",
            "gamma",
            CsvMode::Cell,
            None,
            4,
        )
        .unwrap();
        assert_eq!(inspect.structure, ContentStructure::Text);
        assert_eq!(inspect.inspect.match_count, 2);
        assert_eq!(inspect.sample, b"alph");
        assert!(inspect.inspect.context.is_some());
    }

    #[test]
    fn inspects_csv_cells_and_quoted_newlines() {
        let dir = TempDir::new();
        let path = dir.0.join("a.csv");
        fs::write(&path, b"name,note\nA,\"x y\"\nB,\"line\r\nx y\"\n").unwrap();
        let inspect = inspect_file(
            &path,
            ReplaceMode::Plain,
            "x y",
            "z",
            CsvMode::Cell,
            None,
            64,
        )
        .unwrap();
        assert_eq!(inspect.structure, ContentStructure::Csv);
        assert_eq!(inspect.inspect.match_count, 2);
        assert_eq!(inspect.inspect.delimiter, Some(Delimiter::Comma));
        assert!(!inspect.inspect.unclosed_quote);
    }

    #[test]
    fn short_file_sample_is_truncated() {
        let dir = TempDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, b"abc").unwrap();
        let inspect = inspect_file(
            &path,
            ReplaceMode::Plain,
            "a",
            "b",
            CsvMode::Cell,
            None,
            4096,
        )
        .unwrap();
        assert_eq!(inspect.sample, b"abc");
        assert_eq!(inspect.file_size, 3);
    }
}
