//! C02 换行符统计：读取头部样本并流式扫描结构换行（方案 §5.15）。

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use kernel::error::KernelError;
use kernel::line_endings::{LineEndingCounts, LineEndingProcessor, LineEndingStructure};

pub struct LineEndingInspect {
    pub sample: Vec<u8>,
    pub file_size: u64,
    pub counts: LineEndingCounts,
    pub structure: LineEndingStructure,
}

pub fn inspect_line_endings(
    path: &Path,
    structure: LineEndingStructure,
    sample_bytes: usize,
) -> Result<LineEndingInspect, KernelError> {
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
    let mut file = fs::File::open(&fs_path).map_err(|error| read_error(path, &error))?;
    let mut sample = vec![0u8; sample_bytes];
    let read = file
        .read(&mut sample)
        .map_err(|error| read_error(path, &error))?;
    sample.truncate(read);
    file.seek(SeekFrom::Start(0))
        .map_err(|error| read_error(path, &error))?;

    let mut processor = LineEndingProcessor::new(structure, None);
    let mut sink = std::io::sink();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| read_error(path, &error))?;
        if read == 0 {
            break;
        }
        processor.feed(&buffer[..read], &mut sink)?;
    }
    processor.finish(&mut sink)?;
    Ok(LineEndingInspect {
        sample,
        file_size: metadata.len(),
        counts: processor.counts(),
        structure,
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
                "batchdesk-line-endings-{}-{nanos}-{sequence}",
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
    fn counts_mixed_text_newlines() {
        let dir = TempDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, b"a\r\nb\nc\r").unwrap();
        let inspect = inspect_line_endings(&path, LineEndingStructure::Text, 8).unwrap();
        assert_eq!(
            inspect.counts,
            LineEndingCounts {
                crlf: 1,
                lf: 1,
                cr: 1
            }
        );
        assert_eq!(inspect.sample, b"a\r\nb\nc\r");
        assert_eq!(inspect.file_size, 7);
    }

    #[test]
    fn csv_quoted_newlines_are_not_counted_as_records() {
        let dir = TempDir::new();
        let path = dir.0.join("a.csv");
        fs::write(&path, b"a,\"b\r\nc\"\r\nx,y\r\n").unwrap();
        let inspect = inspect_line_endings(&path, LineEndingStructure::Csv, 4).unwrap();
        assert_eq!(
            inspect.counts,
            LineEndingCounts {
                crlf: 2,
                lf: 0,
                cr: 0
            }
        );
        assert_eq!(inspect.sample, b"a,\"b");
    }

    #[test]
    fn sample_is_truncated_for_short_files() {
        let dir = TempDir::new();
        let path = dir.0.join("a.txt");
        fs::write(&path, b"abc").unwrap();
        let inspect = inspect_line_endings(&path, LineEndingStructure::Text, 4096).unwrap();
        assert_eq!(inspect.sample, b"abc");
        assert_eq!(inspect.counts, LineEndingCounts::default());
    }
}
