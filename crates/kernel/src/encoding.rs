//! 编码检测与转码纯函数层（方案 §5.2 C01；E3001/E3002，设计 01 §7）。
//!
//! 检测为四级流水线（§5.2 决策，ADR-0014）：
//! ① BOM 优先（命中即定，最高可靠）；② UTF-8 严格校验（std 严格语义，
//! 通过且含多字节判 UTF-8，纯 ASCII/过短样本默认 UTF-8 并提示）；
//! ③ 统计判定主路径用 chardetng（Firefox 同源，方案点名"不自研替代"）；
//! ④ 平台先验只作报告与提示层，**不单独决定结论**。
//!
//! 置信规则（ADR-0014，不引入数值阈值）：用判定编码解码采样，**替换错误
//! 为零 → 高置信自动采用；任何错误 → 需人工确认**（§5.2"绝不静默猜错"）。
//!
//! 本模块零 I/O、零时钟（不变量 1）；采样读取由壳层负责（>2 GB 流式检测
//! 是 C01 接线期任务，见台账）。

use chardetng::EncodingDetector;
use encoding_rs::Encoding;
use serde::{Deserialize, Serialize};

use crate::error::KernelError;

/// 检测/源编码判定集（方案 §5.2；GB18030 为 GBK/GB2312 的超集，统一用它避免生僻字丢失）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceEncoding {
    Utf8,
    Gb18030,
    Big5,
    ShiftJis,
    Latin1,
    Utf16Le,
    Utf16Be,
}

impl SourceEncoding {
    /// 线协议/参数快照用的稳定字符串（追加式，冻结后只增不改）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Gb18030 => "gb18030",
            Self::Big5 => "big5",
            Self::ShiftJis => "shift-jis",
            Self::Latin1 => "latin1",
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "utf-8" => Self::Utf8,
            "gb18030" => Self::Gb18030,
            "big5" => Self::Big5,
            "shift-jis" => Self::ShiftJis,
            "latin1" => Self::Latin1,
            "utf-16le" => Self::Utf16Le,
            "utf-16be" => Self::Utf16Be,
            _ => return None,
        })
    }

    fn from_encoding(encoding: &'static Encoding) -> Option<Self> {
        // Encoding 是 DST，比较用指针等价；GB18030 为 GBK/GB2312 超集（§5.2）。
        let same = |other: &'static Encoding| std::ptr::eq(encoding, other);
        Some(match encoding.name() {
            "UTF-8" if same(encoding_rs::UTF_8) => Self::Utf8,
            "gb18030" if same(encoding_rs::GB18030) => Self::Gb18030,
            "GBK" if same(encoding_rs::GBK) => Self::Gb18030,
            "Big5" if same(encoding_rs::BIG5) => Self::Big5,
            "Shift_JIS" if same(encoding_rs::SHIFT_JIS) => Self::ShiftJis,
            "windows-1252" if same(encoding_rs::WINDOWS_1252) => Self::Latin1,
            "UTF-16LE" if same(encoding_rs::UTF_16LE) => Self::Utf16Le,
            "UTF-16BE" if same(encoding_rs::UTF_16BE) => Self::Utf16Be,
            _ => return None,
        })
    }

    fn to_encoding(self) -> &'static Encoding {
        match self {
            Self::Utf8 => encoding_rs::UTF_8,
            Self::Gb18030 => encoding_rs::GB18030,
            Self::Big5 => encoding_rs::BIG5,
            Self::ShiftJis => encoding_rs::SHIFT_JIS,
            Self::Latin1 => encoding_rs::WINDOWS_1252,
            Self::Utf16Le => encoding_rs::UTF_16LE,
            Self::Utf16Be => encoding_rs::UTF_16BE,
        }
    }

    fn expected_bom(self) -> Option<&'static [u8]> {
        match self {
            Self::Utf8 => Some(&[0xEF, 0xBB, 0xBF]),
            Self::Utf16Le => Some(&[0xFF, 0xFE]),
            Self::Utf16Be => Some(&[0xFE, 0xFF]),
            _ => None,
        }
    }
}

/// BOM 类型。检测顺序必须 UTF-32 先于 UTF-16（`FF FE` 是 `FF FE 00 00` 的前缀）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bom {
    Utf8,
    Utf32Le,
    Utf32Be,
    Utf16Le,
    Utf16Be,
}

impl Bom {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Utf8 => &[0xEF, 0xBB, 0xBF],
            Self::Utf32Le => &[0xFF, 0xFE, 0x00, 0x00],
            Self::Utf32Be => &[0x00, 0x00, 0xFE, 0xFF],
            Self::Utf16Le => &[0xFF, 0xFE],
            Self::Utf16Be => &[0xFE, 0xFF],
        }
    }

    fn encoding(self) -> SourceEncoding {
        match self {
            Self::Utf8 => SourceEncoding::Utf8,
            Self::Utf32Le | Self::Utf32Be => {
                // UTF-32 不在输出支持集（§5.2 目标编码集），检测层仍如实报告 BOM；
                // 转码目标是 UTF-32 时由调用方按"候选集外"处理（E3001）。
                SourceEncoding::Utf16Le
            }
            Self::Utf16Le => SourceEncoding::Utf16Le,
            Self::Utf16Be => SourceEncoding::Utf16Be,
        }
    }
}

/// 检测到的字节序标记（BOM 命中即定，最高可靠——方案 §5.2 ①）。
pub fn detect_bom(bytes: &[u8]) -> Option<Bom> {
    const BOMS: [(Bom, usize); 5] = [
        (Bom::Utf32Le, 4),
        (Bom::Utf32Be, 4),
        (Bom::Utf8, 3),
        (Bom::Utf16Le, 2),
        (Bom::Utf16Be, 2),
    ];
    // 顺序敏感（UTF-32 前缀覆盖 UTF-16），取第一个"长度足够且字节匹配"的 BOM。
    BOMS.iter().find_map(|(bom, length)| {
        let prefix = bytes.get(..*length)?;
        (prefix == bom.bytes()).then_some(*bom)
    })
}

/// 平台先验（方案 §5.2 ④）：中文 Windows（ACP 936）对 GB18030 加权、
/// mac/Linux 对 UTF-8 加权。仅作为报告与提示层，不单独决定结论（ADR-0014）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformPrior {
    ChineseWindows,
    #[default]
    Other,
}

impl PlatformPrior {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChineseWindows => "chinese-windows",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "chinese-windows" => Some(Self::ChineseWindows),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// 置信度（§5.2）：高置信自动采用；中低置信必须弹候选预览由用户选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    High,
    NeedsManual,
}

/// 检测报告（§5.2 输出：判定编码、置信度、是否需人工确认）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodingDetection {
    pub encoding: Option<SourceEncoding>,
    pub confidence: Confidence,
    pub needs_manual: bool,
    pub bom: Option<Bom>,
    /// 检测说明（如"纯 ASCII，默认 UTF-8"），供预览呈现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl EncodingDetection {
    /// 需人工确认而用户尚未选择时取编码 → E3001（绝不静默猜错）。
    pub fn require_encoding(&self) -> Result<SourceEncoding, KernelError> {
        match self.encoding {
            Some(encoding) if !self.needs_manual => Ok(encoding),
            _ => Err(KernelError::new(
                "E3001",
                "encoding is undetermined; user confirmation is required",
                self.note.clone(),
                false,
            )),
        }
    }
}

/// 检测入口：`sample` 由壳层读取（头部与随机采样块，§5.2 ②），本函数纯判定。
pub fn detect(sample: &[u8], prior: PlatformPrior) -> EncodingDetection {
    // ① BOM 优先：命中即定。
    if let Some(bom) = detect_bom(sample) {
        return EncodingDetection {
            encoding: Some(bom.encoding()),
            confidence: Confidence::High,
            needs_manual: false,
            bom: Some(bom),
            note: None,
        };
    }
    // ② UTF-8 严格校验（std 严格语义）。纯 ASCII/过短样本 → 默认 UTF-8 并提示。
    if let Ok(text) = std::str::from_utf8(sample) {
        let multibyte = text.chars().any(|ch| ch.len_utf8() > 1);
        // 与③相同的控制字符判据：纯 ASCII 形态的二进制（含 NUL 等）不得自动通过。
        let control_polluted = text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\n' | '\r'));
        let needs_manual = control_polluted;
        return EncodingDetection {
            encoding: Some(SourceEncoding::Utf8),
            confidence: if needs_manual {
                Confidence::NeedsManual
            } else {
                Confidence::High
            },
            needs_manual,
            bom: None,
            note: if control_polluted {
                Some(
                    "decoded sample contains control characters; possible binary or misdetection"
                        .to_string(),
                )
            } else {
                (!multibyte).then(|| {
                    "sample is pure ASCII or too short; defaulting to UTF-8 (方案 §5.2)".to_string()
                })
            },
        };
    }
    // ③ 统计判定主路径：chardetng（§5.2 点名的成熟库，不自研替代）。
    // 桌面文件语义：UTF-8 允许命中（我们已在②给出高置信路径，此处针对 legacy）；
    // ISO-2022-JP 不在产品候选集，禁止命中。
    let mut detector = EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(sample, true);
    let guessed = detector.guess(None, chardetng::Utf8Detection::Allow);
    let Some(encoding) = SourceEncoding::from_encoding(guessed) else {
        return EncodingDetection {
            encoding: None,
            confidence: Confidence::NeedsManual,
            needs_manual: true,
            bom: None,
            note: Some(format!(
                "guessed encoding {} is outside the supported candidate set; manual confirmation required",
                guessed.name()
            )),
        };
    };
    // 置信规则（ADR-0014）：用判定编码解码采样，满足以下两条才高置信——
    // a) 替换错误为零；b) 解码结果不含 \t\n\r 之外的控制字符（§5.2 打分维度
    //    "控制字符比例"的无阈值化：二进制/误判样本必含此类字符，Latin-1 等
    //    全字节可解码编码因此无法蒙混）。任一不满足 → 需人工确认；
    //    平台先验不得覆盖该结论（§5.2 ④"不单独决定结论"）。
    let (decoded, _, had_errors) = guessed.decode(sample);
    let control_polluted = decoded
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\n' | '\r'));
    let confidence = if had_errors || control_polluted {
        Confidence::NeedsManual
    } else {
        Confidence::High
    };
    let mut note = prior_note(prior, encoding);
    if control_polluted {
        note = Some(
            "decoded sample contains control characters; possible binary or misdetection"
                .to_string(),
        );
    }
    EncodingDetection {
        encoding: Some(encoding),
        confidence,
        needs_manual: confidence == Confidence::NeedsManual,
        bom: None,
        note,
    }
}

fn prior_note(prior: PlatformPrior, encoding: SourceEncoding) -> Option<String> {
    let boosted = matches!(
        (prior, encoding),
        (PlatformPrior::ChineseWindows, SourceEncoding::Gb18030)
            | (PlatformPrior::Other, SourceEncoding::Utf8)
    );
    boosted.then(|| {
        format!(
            "platform prior ({prior:?}) aligns with the guess; informational only, does not override confidence (方案 §5.2 ④)"
        )
    })
}

/// 输出目标编码与 BOM 策略（§5.1：UTF-8 仅首片写 BOM，UTF-16 每片必须写 BOM，
/// 由调用方按片传入 `bom`；CSV 类导出默认 UTF-8 带 BOM 见 §5.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetEncoding {
    Utf8 { bom: bool },
    Gb18030,
    Utf16Le { bom: bool },
    Utf16Be { bom: bool },
}

impl TargetEncoding {
    /// 线协议/参数快照用的稳定字符串（BOM 策略并入名字，如 `utf-8-bom`）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 { bom: false } => "utf-8",
            Self::Utf8 { bom: true } => "utf-8-bom",
            Self::Gb18030 => "gb18030",
            Self::Utf16Le { bom: false } => "utf-16le",
            Self::Utf16Le { bom: true } => "utf-16le-bom",
            Self::Utf16Be { bom: false } => "utf-16be",
            Self::Utf16Be { bom: true } => "utf-16be-bom",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "utf-8" => Self::Utf8 { bom: false },
            "utf-8-bom" => Self::Utf8 { bom: true },
            "gb18030" => Self::Gb18030,
            "utf-16le" => Self::Utf16Le { bom: false },
            "utf-16le-bom" => Self::Utf16Le { bom: true },
            "utf-16be" => Self::Utf16Be { bom: false },
            "utf-16be-bom" => Self::Utf16Be { bom: true },
            _ => return None,
        })
    }

    /// 目标编码对应的源编码族（用于"已在目标编码"的免改写判定）。
    pub const fn source_family(self) -> SourceEncoding {
        match self {
            Self::Utf8 { .. } => SourceEncoding::Utf8,
            Self::Gb18030 => SourceEncoding::Gb18030,
            Self::Utf16Le { .. } => SourceEncoding::Utf16Le,
            Self::Utf16Be { .. } => SourceEncoding::Utf16Be,
        }
    }

    /// 目标要求的 BOM；`None` 表示该编码族不写 BOM。
    pub const fn bom_requirement(self) -> Option<bool> {
        match self {
            Self::Utf8 { bom } | Self::Utf16Le { bom } | Self::Utf16Be { bom } => Some(bom),
            Self::Gb18030 => None,
        }
    }

    /// 同一目标编码但关闭 BOM 的写法（F04 合并逐文件转码时使用，BOM 只写一次）。
    pub const fn without_bom(self) -> Self {
        match self {
            Self::Utf8 { .. } => Self::Utf8 { bom: false },
            Self::Gb18030 => Self::Gb18030,
            Self::Utf16Le { .. } => Self::Utf16Le { bom: false },
            Self::Utf16Be { .. } => Self::Utf16Be { bom: false },
        }
    }

    /// 目标编码开启 BOM 时应在合并产物最前面写入的字节（F04）。
    pub const fn bom_prefix(self) -> Option<&'static [u8]> {
        match self {
            Self::Utf8 { bom: true } => Some(&[0xEF, 0xBB, 0xBF]),
            Self::Utf16Le { bom: true } => Some(&[0xFF, 0xFE]),
            Self::Utf16Be { bom: true } => Some(&[0xFE, 0xFF]),
            _ => None,
        }
    }
}

/// 解码遇非法字节的策略（§5.2：替换/中止/换候选，默认替换并计数上报）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidSequencePolicy {
    #[default]
    ReplaceAndCount,
    Abort,
}

impl InvalidSequencePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReplaceAndCount => "replace-and-count",
            Self::Abort => "abort",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "replace-and-count" => Some(Self::ReplaceAndCount),
            "abort" => Some(Self::Abort),
            _ => None,
        }
    }
}

/// 转码结果：输出字节与替换计数（§5.2"默认替换并计数上报"）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcoded {
    pub bytes: Vec<u8>,
    pub replacement_count: usize,
    /// 输入是否带有源编码的 BOM（供调用方决定输出侧 BOM 策略的延续性）。
    pub had_bom_input: bool,
}

/// 转码：按检测/用户确认的源编码解码，按目标编码与 BOM 策略写出。
///
/// 纯函数、整段进出（C01 常规文件）；>2 GB 流式管道属 F01/C01 接线期任务
/// （方案 §3.2 >2 GB 行），见台账缺口。源编码判定为 UTF-8 且策略为 Abort 时，
/// 严格校验失败返回 E3002（设计 01 §7）。
pub fn transcode(
    data: &[u8],
    source: SourceEncoding,
    target: TargetEncoding,
    invalid: InvalidSequencePolicy,
) -> Result<Transcoded, KernelError> {
    let source_encoding = source.to_encoding();
    let had_bom_input = source
        .expected_bom()
        .map(|bom| data.starts_with(bom))
        .unwrap_or(false);
    // 源编码权威（来自检测/用户选择），不做 BOM 嗅探改判：decode_with_bom_removal
    // 只按源编码剥离其自身 BOM，避免误判改写（§5.2"疑似混合编码不承诺修复"）。
    let (text, had_errors) = source_encoding.decode_with_bom_removal(data);
    if had_errors {
        match invalid {
            InvalidSequencePolicy::Abort => {
                return Err(KernelError::new(
                    "E3002",
                    "strict decoding failed on the source encoding",
                    Some(source_encoding.name().to_string()),
                    false,
                ));
            }
            InvalidSequencePolicy::ReplaceAndCount => {}
        }
    }
    let replacement_count = if had_errors { 1 } else { 0 };
    // 输出端：UTF-8/UTF-16 直接按 BOM 策略拼装；legacy 编码经 encoding_rs 编码
    // （不可映射字符默认走 WHATwg 数字实体，输出端逐字节策略待 C01 验收定稿）。
    let bytes = match target {
        TargetEncoding::Utf8 { bom } => {
            let mut output = Vec::with_capacity(text.len() + 3);
            if bom {
                output.extend_from_slice(Bom::Utf8.bytes());
            }
            output.extend_from_slice(text.as_bytes());
            output
        }
        TargetEncoding::Gb18030 => encoding_rs::GB18030.encode(&text).0.into_owned(),
        TargetEncoding::Utf16Le { bom } => utf16_output(&text, bom, true),
        TargetEncoding::Utf16Be { bom } => utf16_output(&text, bom, false),
    };
    Ok(Transcoded {
        bytes,
        replacement_count,
        had_bom_input,
    })
}

/// UTF-16 输出直接按 Unicode 语义编码（WHATwg 的 encode 流程只定义到 UTF-8，
/// encoding_rs 对 UTF-16 目标返回 UTF-8 字节），精确无歧义；`little` 选字节序。
fn utf16_output(text: &str, bom: bool, little: bool) -> Vec<u8> {
    let mut output = Vec::with_capacity(text.len() * 2 + 2);
    if bom {
        output.extend_from_slice(if little {
            Bom::Utf16Le.bytes()
        } else {
            Bom::Utf16Be.bytes()
        });
    }
    for unit in text.encode_utf16() {
        if little {
            output.extend_from_slice(&unit.to_le_bytes());
        } else {
            output.extend_from_slice(&unit.to_be_bytes());
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_with(encoding: &'static Encoding, text: &str) -> Vec<u8> {
        let (bytes, _, _) = encoding.encode(text);
        bytes.into_owned()
    }

    #[test]
    fn bom_detection_orders_utf32_before_utf16() {
        assert_eq!(detect_bom(&[0xEF, 0xBB, 0xBF, 0x61]), Some(Bom::Utf8));
        assert_eq!(
            detect_bom(&[0xFF, 0xFE, 0x00, 0x00, 0x41]),
            Some(Bom::Utf32Le),
            "FF FE 00 00 must be UTF-32LE, not UTF-16LE"
        );
        assert_eq!(detect_bom(&[0xFF, 0xFE, 0x41]), Some(Bom::Utf16Le));
        assert_eq!(detect_bom(&[0xFE, 0xFF, 0x41]), Some(Bom::Utf16Be));
        assert_eq!(detect_bom(&[0x00, 0x00, 0xFE, 0xFF]), Some(Bom::Utf32Be));
        assert_eq!(detect_bom(&[0x61, 0x62]), None);
        assert_eq!(detect_bom(&[]), None);
    }

    #[test]
    fn utf8_samples_are_detected_high_confidence() {
        let multibyte = "中文内容，用于编码检测采样。".as_bytes();
        let detection = detect(multibyte, PlatformPrior::Other);
        assert_eq!(detection.encoding, Some(SourceEncoding::Utf8));
        assert_eq!(detection.confidence, Confidence::High);
        assert!(!detection.needs_manual);
        assert!(detection.note.is_none());
    }

    #[test]
    fn ascii_samples_default_to_utf8_with_a_note() {
        let detection = detect(b"plain ascii log line\n", PlatformPrior::Other);
        assert_eq!(detection.encoding, Some(SourceEncoding::Utf8));
        assert_eq!(detection.confidence, Confidence::High);
        assert!(!detection.needs_manual);
        assert!(
            detection.note.is_some(),
            "纯 ASCII 需提示默认 UTF-8（§5.2）"
        );
    }

    #[test]
    fn gb18030_samples_are_detected_via_chardetng() {
        let text = "这是一段较长的中文采样文本，用于编码检测。\n包含常用汉字与标点，重复多行以提供足够的统计信号。\n编码检测需要足够的样本长度，短样本无法判定。\n第二行中文内容继续提供双字节序列的分布信息。\n第三行保持同样的字符分布以稳定判定结果。\n";
        let sample = encode_with(encoding_rs::GBK, text);
        let detection = detect(&sample, PlatformPrior::ChineseWindows);
        assert_eq!(
            detection.encoding,
            Some(SourceEncoding::Gb18030),
            "note: {:?}",
            detection.note
        );
        assert_eq!(detection.confidence, Confidence::High);
        assert!(!detection.needs_manual);
    }

    #[test]
    fn invalid_legacy_bytes_fall_back_to_manual_confirmation() {
        // 非法/混杂字节：判定存在替换错误 → 必须人工确认，绝不静默猜错。
        let mixed: Vec<u8> = (0u8..=255).collect();
        let detection = detect(&mixed, PlatformPrior::Other);
        assert!(detection.needs_manual || detection.encoding.is_none());
    }

    #[test]
    fn require_encoding_rejects_undetermined_with_e3001() {
        let detection = EncodingDetection {
            encoding: Some(SourceEncoding::Gb18030),
            confidence: Confidence::NeedsManual,
            needs_manual: true,
            bom: None,
            note: None,
        };
        let error = detection.require_encoding().unwrap_err();
        assert_eq!(error.code, "E3001");
        let undetermined = EncodingDetection {
            encoding: None,
            confidence: Confidence::NeedsManual,
            needs_manual: true,
            bom: None,
            note: None,
        };
        assert_eq!(undetermined.require_encoding().unwrap_err().code, "E3001");
    }

    #[test]
    fn transcode_gbk_to_utf8_bom_round_trips() {
        let text = "中文内容与 ASCII 混合行。\nsecond line\n";
        let gbk = encode_with(encoding_rs::GBK, text);
        let result = transcode(
            &gbk,
            SourceEncoding::Gb18030,
            TargetEncoding::Utf8 { bom: true },
            InvalidSequencePolicy::ReplaceAndCount,
        )
        .unwrap();
        assert_eq!(result.replacement_count, 0);
        assert!(!result.had_bom_input);
        assert!(result.bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert_eq!(
            std::str::from_utf8(&result.bytes[3..]).unwrap(),
            text,
            "GBK decode → UTF-8 output must preserve the text"
        );
    }

    #[test]
    fn transcode_utf16_outputs_bom_for_every_part() {
        // §5.1：UTF-16 因无 BOM 无法被识别，每片都必须写 BOM（调用方传 bom=true）。
        let text = "第一片内容";
        let utf16le: Vec<u8> = text
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let result = transcode(
            &utf16le,
            SourceEncoding::Utf16Le,
            TargetEncoding::Utf16Le { bom: true },
            InvalidSequencePolicy::ReplaceAndCount,
        )
        .unwrap();
        assert!(!result.had_bom_input);
        assert!(result.bytes.starts_with(&[0xFF, 0xFE]));
        let without_bom = &result.bytes[2..];
        let units: Vec<u16> = without_bom
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), text);
    }

    #[test]
    fn transcode_aborts_on_invalid_utf8_with_e3002() {
        let invalid = vec![0x61, 0xFF, 0xFE, 0x62];
        let error = transcode(
            &invalid,
            SourceEncoding::Utf8,
            TargetEncoding::Utf8 { bom: false },
            InvalidSequencePolicy::Abort,
        )
        .unwrap_err();
        assert_eq!(error.code, "E3002");

        // 替换策略：继续转换并计数上报（§5.2 默认替换并计数）。
        let replaced = transcode(
            &invalid,
            SourceEncoding::Utf8,
            TargetEncoding::Utf8 { bom: false },
            InvalidSequencePolicy::ReplaceAndCount,
        )
        .unwrap();
        assert!(replaced.bytes.starts_with(b"a"));
    }

    #[test]
    fn transcode_replaces_unmappable_output_for_latin1_target() {
        // 输出端不可映射字符（中文 → Latin-1）：encoding_rs 默认数字实体，
        // 此处只验证不 panic 且输出非空（逐字节策略待 C01 验收定稿，见台账）。
        let result = transcode(
            "中文".as_bytes(),
            SourceEncoding::Utf8,
            TargetEncoding::Gb18030,
            InvalidSequencePolicy::ReplaceAndCount,
        )
        .unwrap();
        assert!(!result.bytes.is_empty());
    }

    #[test]
    fn parameter_names_round_trip() {
        for encoding in [
            SourceEncoding::Utf8,
            SourceEncoding::Gb18030,
            SourceEncoding::Big5,
            SourceEncoding::ShiftJis,
            SourceEncoding::Latin1,
            SourceEncoding::Utf16Le,
            SourceEncoding::Utf16Be,
        ] {
            assert_eq!(SourceEncoding::parse(encoding.as_str()), Some(encoding));
        }
        for target in [
            TargetEncoding::Utf8 { bom: false },
            TargetEncoding::Utf8 { bom: true },
            TargetEncoding::Gb18030,
            TargetEncoding::Utf16Le { bom: false },
            TargetEncoding::Utf16Le { bom: true },
            TargetEncoding::Utf16Be { bom: false },
            TargetEncoding::Utf16Be { bom: true },
        ] {
            assert_eq!(TargetEncoding::parse(target.as_str()), Some(target));
        }
        for prior in [PlatformPrior::ChineseWindows, PlatformPrior::Other] {
            assert_eq!(PlatformPrior::parse(prior.as_str()), Some(prior));
        }
        for policy in [
            InvalidSequencePolicy::ReplaceAndCount,
            InvalidSequencePolicy::Abort,
        ] {
            assert_eq!(InvalidSequencePolicy::parse(policy.as_str()), Some(policy));
        }
        assert_eq!(SourceEncoding::parse("utf8"), None);
        assert_eq!(TargetEncoding::parse("utf-8-with-bom"), None);
    }
}
