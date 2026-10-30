//! `Display` for [`super::UnsupportedFeature`].

use super::UnsupportedFeature;
use std::fmt;

impl fmt::Display for UnsupportedFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AudioObjectType(a) => write!(f, "aac: unsupported audioObjectType {a}"),
            Self::FrameLength960 => write!(f, "aac: 960-line frames are Media"),
            Self::HeFrame960 => write!(f, "aac: HE-AAC over a 960-sample core"),
            Self::ChannelConfiguration(c) => {
                write!(f, "aac: unsupported channel_configuration {c}")
            }
            Self::SampleRateIndex(i) => {
                write!(f, "aac: no SWB table for samplingFrequencyIndex {i}")
            }
            Self::LatmVersionA => write!(f, "aac: LATM audioMuxVersionA reserved"),
            Self::LatmFrameLengthType => write!(f, "aac: LATM frameLengthType not AAC"),
            Self::Extension(t) => write!(f, "aac: extension_type {t}"),
            Self::M4aPush => write!(
                f,
                "aac: M4A/ISOBMFF needs random access; use decode_streaming"
            ),
            Self::EncodeM4aStreaming => {
                write!(
                    f,
                    "encode: M4A/ISOBMFF needs finish-time sizes; use encode_with"
                )
            }
            Self::EncodeRawOneShot => {
                write!(
                    f,
                    "encode: raw access units need the push Encoder (per-AU sizes)"
                )
            }
            Self::EncodeHeRate(r) => {
                write!(f, "encode: HE-AAC needs 16-48 kHz (2x ADTS), got {r} Hz")
            }
            Self::RawAccessUnit => {
                write!(f, "aac: decode_au is ASC+AU; feed is ADTS/LATM/fMP4")
            }
            Self::AscChange => {
                write!(
                    f,
                    "aac: AudioSpecificConfig changed mid-stream; call reset()"
                )
            }
            Self::FragmentedMp4(reason) => write!(f, "aac: fragmented MP4: {reason}"),
        }
    }
}
