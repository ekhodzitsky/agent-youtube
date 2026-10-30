//! One-shot AAC-LD. The push [`crate::Encoder`] owns the frames; this
//! only chooses LOAS or M4A. ADTS and raw one-shot are rejected earlier.

use super::mux;
use crate::error::{AacError, Result};
use crate::options::{EncodeContainer, EncodeOptions};

pub(super) fn encode_ld<P: AsRef<[f32]>>(
    pcm: &[P],
    sample_rate: u32,
    opts: &EncodeOptions,
) -> Result<Vec<u8>> {
    match opts.container {
        EncodeContainer::Adts => Err(AacError::encode("encode: AAC-LD cannot be carried in ADTS")),
        EncodeContainer::Raw => Err(AacError::Unsupported(
            crate::UnsupportedFeature::EncodeRawOneShot,
        )),
        EncodeContainer::M4a => {
            let mut cur = std::io::Cursor::new(Vec::new());
            mux::encode_write_m4a(&mut cur, pcm, sample_rate, opts)?;
            Ok(cur.into_inner())
        }
        EncodeContainer::Latm => {
            let mut out = Vec::new();
            mux::encode_write(&mut out, pcm, sample_rate, opts)?;
            Ok(out)
        }
    }
}
