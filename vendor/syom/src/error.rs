//! Typed errors for the AAC reader (stable `Display`, no `anyhow`).

use crate::budgets::{BudgetExceeded, BudgetKind};
use crate::engine::error::Error as EngineError;
use std::fmt;
use std::io;

#[path = "error_feat.rs"]
mod error_feat;

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, AacError>;

/// Unsupported AOT / transport / tool (not malformed syntax).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnsupportedFeature {
    /// `audioObjectType` other than LC, SBR, PS, or LD.
    AudioObjectType(u8),
    /// Display text for this variant is fixed. 960-sample LC and the
    /// 480-sample LD grid do not return it.
    FrameLength960,
    /// HE-AAC (SBR or PS) on a 960-sample core.
    HeFrame960,
    /// `channel_configuration` 8–15 (6.1, 7.1 back/top, reserved); 1–7 and PCE decode.
    ChannelConfiguration(u8),
    /// No SWB table / reserved ADTS rate index.
    SampleRateIndex(u8),
    /// LATM `audioMuxVersionA == 1`.
    LatmVersionA,
    /// LATM `frameLengthType` is not AAC.
    LatmFrameLengthType,
    /// FIL `extension_type` we do not implement.
    Extension(u8),
    /// Push [`crate::Decoder`] cannot seek ISOBMFF.
    M4aPush,
    /// Push [`crate::Encoder`] cannot write M4A (`stco` needs finish).
    EncodeM4aStreaming,
    /// One-shot encode cannot emit raw AUs (no per-AU sizes); use [`crate::Encoder`].
    EncodeRawOneShot,
    /// HE-AAC v1 encode needs an output rate whose half is an ADTS rate.
    EncodeHeRate(u32),
    /// `decode_au` needs [`crate::Decoder::from_asc`]; `feed` is ADTS/LATM/fMP4.
    RawAccessUnit,
    /// Mid-stream AudioSpecificConfig change (call [`crate::Decoder::reset`]).
    AscChange,
    /// Bounded fMP4 outside the envelope (lab/fmp4/REPORT.md §2).
    FragmentedMp4(&'static str),
}

/// Malformed syntax (CRC, Huffman, lengths). Truncation is [`AacError::Truncated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MalformedKind {
    AdtsSync,
    AdtsLayer,
    AdtsFrameLength,
    AdtsCrc,
    Huffman,
    Ics,
    Section,
    Spectrum,
    Filterbank,
    Codebook,
    LatmCrc,
    LatmConfig,
    LatmMux,
    Sbr,
    Ps,
    Syntax,
}

/// Push `Decoder` / `Encoder` used after finish or fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Failed,
    Finished,
}

/// Encoder rejected the PCM domain (not a bitrate/container knob).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PcmReject {
    NonFinite,
    Amplitude,
    Empty,
    PlaneLength,
    ChannelCount,
}

/// Errors produced while sniffing, decoding, or encoding AAC.
///
/// `#[non_exhaustive]`: match with `_` or named variants. Prefer
/// [`Self::Unsupported`], [`Self::Truncated`], [`Self::Malformed`],
/// [`Self::Lifecycle`], [`Self::InvalidPcm`] over string-matching
/// [`Self::Format`] / [`Self::Decode`] / [`Self::Encode`].
///
/// ```
/// fn classify(e: syom::AacError) -> &'static str {
///     match e {
///         syom::AacError::NotAac => "not-aac",
///         syom::AacError::Unsupported(_) => "unsupported",
///         syom::AacError::Truncated { .. } => "truncated",
///         syom::AacError::NeedMore { .. } => "need-more",
///         syom::AacError::Malformed(_) => "malformed",
///         syom::AacError::Limit { .. } | syom::AacError::TooLong { .. } => "limit",
///         syom::AacError::InvalidPcm(_) | syom::AacError::InvalidLimits(_) => "invalid",
///         syom::AacError::Lifecycle { .. } => "lifecycle",
///         _ => "other",
///     }
/// }
/// assert_eq!(classify(syom::AacError::NotAac), "not-aac");
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum AacError {
    /// Underlying `Read` / `Seek` failure.
    Io(io::Error),
    /// Stream is not ADTS, LATM/LOAS, or M4A/AAC.
    NotAac,
    /// Sample rate is zero or above the configured ceiling.
    UnsupportedSampleRate { rate: u32, max: u32 },
    /// Decoded (or declared) duration exceeds the configured budget.
    TooLong { observed_secs: f64, max_secs: f64 },
    /// `DecodeOptions` are not a usable limit set (NaN, negative, etc.).
    InvalidLimits(String),
    /// An independent memory budget was exceeded (`planned == max` is ok).
    Limit {
        kind: BudgetKind,
        observed: u64,
        max: u64,
    },
    /// Legal bitstream we do not implement.
    Unsupported(UnsupportedFeature),
    /// Input ended mid-header or mid-payload. `at` is a byte offset when known.
    Truncated { at: Option<u64> },
    /// Prefix is too short to finish a header or config. `need` is a lower
    /// bound on the total size from the start of the slice; retry with more.
    NeedMore { have: u64, need: u64 },
    /// CRC / Huffman / length / ICS syntax.
    Malformed(MalformedKind),
    /// `feed` / `finish` after a failed or finished streaming instance.
    Lifecycle { state: LifecycleState },
    /// Encoder PCM is empty, non-finite, `|x|>1`, or plane-shaped wrong.
    InvalidPcm(PcmReject),
    /// Structural / demux failure (leftover; prefer the variants above).
    Format(String),
    /// Decoder engine rejected a frame (leftover mid-stream semantics).
    Decode(String),
    /// Encoder rejected input or could not meet constraints (leftover).
    Encode(String),
}

impl AacError {
    #[inline]
    pub fn format(msg: impl Into<String>) -> Self {
        Self::Format(msg.into())
    }

    #[inline]
    pub fn decode(msg: impl Into<String>) -> Self {
        Self::Decode(msg.into())
    }

    #[inline]
    pub fn encode(msg: impl Into<String>) -> Self {
        Self::Encode(msg.into())
    }

    #[inline]
    pub fn too_long(observed_secs: f64, max_secs: f64) -> Self {
        Self::TooLong {
            observed_secs,
            max_secs,
        }
    }

    #[inline]
    pub fn sample_rate(rate: u32, max: u32) -> Self {
        Self::UnsupportedSampleRate { rate, max }
    }

    #[inline]
    pub fn invalid_limits(msg: impl Into<String>) -> Self {
        Self::InvalidLimits(msg.into())
    }

    #[inline]
    pub fn limit(kind: BudgetKind, observed: u64, max: u64) -> Self {
        Self::Limit {
            kind,
            observed,
            max,
        }
    }

    #[inline]
    pub fn truncated_at(at: Option<u64>) -> Self {
        Self::Truncated { at }
    }

    #[inline]
    pub fn with_truncated_at(self, at: u64) -> Self {
        match self {
            Self::Truncated { at: None } => Self::Truncated { at: Some(at) },
            other => other,
        }
    }

    /// Whether this should surface as generic unsupported-format upstream.
    pub fn is_format_class(&self) -> bool {
        matches!(self, Self::NotAac | Self::Format(_))
    }
}

impl fmt::Display for MalformedKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AdtsSync => "aac: ADTS sync not found",
            Self::AdtsLayer => "aac: ADTS layer must be 0",
            Self::AdtsFrameLength => "aac: ADTS frame length too small",
            Self::AdtsCrc => "aac: ADTS CRC mismatch",
            Self::Huffman => "aac: Huffman codeword invalid",
            Self::Ics => "aac: invalid ics_info",
            Self::Section => "aac: section_data overrun",
            Self::Spectrum => "aac: spectral geometry invalid",
            Self::Filterbank => "aac: filterbank length mismatch",
            Self::Codebook => "aac: invalid codebook",
            Self::LatmCrc => "aac: LATM CRC mismatch",
            Self::LatmConfig => "aac: LATM config out of range",
            Self::LatmMux => "aac: LATM missing StreamMuxConfig",
            Self::Sbr => "aac: SBR payload invalid",
            Self::Ps => "aac: PS data invalid",
            Self::Syntax => "aac: malformed syntax",
        };
        f.write_str(s)
    }
}

impl fmt::Display for AacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::NotAac => write!(f, "Unsupported audio format"),
            Self::UnsupportedSampleRate { rate, max } => {
                write!(f, "Unsupported sample rate: {rate}Hz (max {max}Hz)")
            }
            Self::TooLong {
                observed_secs,
                max_secs,
            } => write!(
                f,
                "Audio file too long ({observed_secs:.0}s). Maximum supported: {max_secs:.0}s."
            ),
            Self::InvalidLimits(msg) => write!(f, "invalid decode limits: {msg}"),
            Self::Limit {
                kind,
                observed,
                max,
            } => write!(f, "decode budget exceeded ({kind}: {observed} > {max})"),
            Self::Unsupported(feat) => write!(f, "{feat}"),
            Self::Truncated { at } => match at {
                Some(n) => write!(f, "aac: unexpected end of bitstream at byte {n}"),
                None => write!(f, "aac: unexpected end of bitstream"),
            },
            Self::NeedMore { have, need } => {
                write!(f, "aac: need more bytes ({have} of {need})")
            }
            Self::Malformed(kind) => write!(f, "{kind}"),
            Self::Lifecycle { state } => match state {
                LifecycleState::Failed => write!(f, "stream failed; call reset()"),
                LifecycleState::Finished => write!(f, "stream already finished; call reset()"),
            },
            Self::InvalidPcm(PcmReject::NonFinite) => write!(f, "encode: non-finite sample"),
            Self::InvalidPcm(PcmReject::Amplitude) => {
                write!(f, "encode: sample amplitude exceeds ±1")
            }
            Self::InvalidPcm(PcmReject::Empty) => write!(f, "encode: empty input"),
            Self::InvalidPcm(PcmReject::PlaneLength) => {
                write!(f, "encode: channel planes differ in length")
            }
            Self::InvalidPcm(PcmReject::ChannelCount) => {
                write!(f, "encode: channel plane count mismatch")
            }
            Self::Format(msg) | Self::Decode(msg) | Self::Encode(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AacError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for AacError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<BudgetExceeded> for AacError {
    fn from(e: BudgetExceeded) -> Self {
        Self::Limit {
            kind: e.kind,
            observed: e.observed,
            max: e.max,
        }
    }
}

impl From<EngineError> for AacError {
    fn from(value: EngineError) -> Self {
        use MalformedKind as M;
        use UnsupportedFeature as U;
        match value {
            EngineError::UnexpectedEnd => Self::Truncated { at: None },
            EngineError::AdtsSyncNotFound => Self::Malformed(M::AdtsSync),
            EngineError::AdtsLayerNonZero => Self::Malformed(M::AdtsLayer),
            EngineError::AdtsReservedSampleRateIndex => Self::Unsupported(U::SampleRateIndex(0xFF)),
            EngineError::AdtsFrameLengthTooSmall => Self::Malformed(M::AdtsFrameLength),
            EngineError::AdtsCrcMismatch => Self::Malformed(M::AdtsCrc),
            EngineError::UnsupportedAot(a) => Self::Unsupported(U::AudioObjectType(a)),
            EngineError::UnsupportedFrameLength => Self::Unsupported(U::FrameLength960),
            EngineError::UnsupportedHeFrame960 => Self::Unsupported(U::HeFrame960),
            EngineError::UnsupportedChannelConfiguration(c) => {
                Self::Unsupported(U::ChannelConfiguration(c))
            }
            EngineError::UnsupportedSampleRateIndex(i) => Self::Unsupported(U::SampleRateIndex(i)),
            EngineError::IcsInfoInvalid => Self::Malformed(M::Ics),
            EngineError::SectionDataOverrun => Self::Malformed(M::Section),
            EngineError::InvalidCodebook(_) => Self::Malformed(M::Codebook),
            EngineError::HuffmanInvalid => Self::Malformed(M::Huffman),
            EngineError::SpectrumInvalid => Self::Malformed(M::Spectrum),
            EngineError::FilterbankInvalid => Self::Malformed(M::Filterbank),
            EngineError::LoasSyncInvalid => Self::Malformed(M::LatmMux),
            EngineError::LatmAudioMuxVersionAReserved => Self::Unsupported(U::LatmVersionA),
            EngineError::LatmConfigOutOfRange => Self::Malformed(M::LatmConfig),
            EngineError::LatmCrcMismatch => Self::Malformed(M::LatmCrc),
            EngineError::LatmNoPreviousMuxConfig => Self::Malformed(M::LatmMux),
            EngineError::LatmUnsupportedFrameLengthType => {
                Self::Unsupported(U::LatmFrameLengthType)
            }
            EngineError::SbrQmfInvalid
            | EngineError::SbrFreqBandInvalid
            | EngineError::SbrGridInvalid
            | EngineError::SbrHuffInvalid
            | EngineError::ExtensionPayloadInvalid => Self::Malformed(M::Sbr),
            EngineError::PsDataInvalid => Self::Malformed(M::Ps),
            EngineError::UnsupportedExtensionSbr(t) | EngineError::UnsupportedExtensionType(t) => {
                Self::Unsupported(U::Extension(t))
            }
            EngineError::UnsupportedCce => Self::Unsupported(U::Extension(0)),
            EngineError::Format(msg) => Self::Decode(format!("aac: {msg}")),
        }
    }
}
