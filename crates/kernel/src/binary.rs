//!
//! 二进制文件魔数嗅探（方案 §5.14 边界：二进制文件误入选 → 魔数探测自动排除）。
//!
//! 纯函数层：只看样本头部字节，不读盘、不做编码判定。

const SIGNATURES: &[(&str, &[u8])] = &[
    ("png", b"\x89PNG\r\n\x1a\n"),
    ("jpeg", b"\xff\xd8\xff"),
    ("gif", b"GIF87a"),
    ("gif", b"GIF89a"),
    ("pdf", b"%PDF-"),
    ("zip", b"PK\x03\x04"),
    ("zip", b"PK\x05\x06"),
    ("zip", b"PK\x07\x08"),
    ("gzip", b"\x1f\x8b"),
    ("bzip2", b"BZh"),
    ("xz", b"\xfd7zXZ\x00"),
    ("7z", b"7z\xbc\xaf\x27\x1c"),
    ("rar", b"Rar!\x1a\x07"),
    ("elf", b"\x7fELF"),
    ("pe", b"MZ"),
    ("bmp", b"BM"),
    ("tiff", b"II*\x00"),
    ("tiff", b"MM\x00*"),
    ("sqlite", b"SQLite format 3\x00"),
    ("ole", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"),
];

/// 返回命中的魔数名称；未命中返回 `None`。
pub fn magic_name(sample: &[u8]) -> Option<&'static str> {
    SIGNATURES
        .iter()
        .find_map(|(name, signature)| sample.starts_with(signature).then_some(*name))
}

/// 是否为明确的二进制格式。
pub fn is_binary(sample: &[u8]) -> bool {
    magic_name(sample).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_binary_signatures() {
        assert_eq!(magic_name(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(magic_name(b"%PDF-1.7"), Some("pdf"));
        assert_eq!(magic_name(b"PK\x03\x04rest"), Some("zip"));
        assert_eq!(magic_name(b"\x7fELF"), Some("elf"));
        assert_eq!(magic_name(b"SQLite format 3\x00"), Some("sqlite"));
    }

    #[test]
    fn plain_text_and_short_samples_are_not_binary() {
        assert_eq!(magic_name(b"hello, world\r\n"), None);
        assert_eq!(magic_name(b""), None);
        assert_eq!(magic_name(b"M"), None, "short prefix must not match MZ");
        assert_eq!(magic_name("中文文本".as_bytes()), None);
    }
}
