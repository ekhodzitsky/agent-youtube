//! TASK-134 measurement hook. Not the `EncodeOptions::with_he_v2` path
//! (that stays `iid_mode` 1 unless the fine grid is adopted).

use crate::Result;
use crate::options::EncodeOptions;

/// Encode stereo HE v2 ADTS at `bitrate_bps`.
///
/// `fine_iid` selects `iid_mode` 4. `false` is byte-identical to
/// `with_he_v2`. Returns `(adts, ps_data bits, extended-data block bits)`.
#[doc(hidden)]
pub fn encode_he_v2_adts<P: AsRef<[f32]>>(
    pcm: &[P],
    sample_rate: u32,
    bitrate_bps: u32,
    fine_iid: bool,
) -> Result<(Vec<u8>, u64, u64)> {
    let opts = EncodeOptions::adts()
        .with_bitrate_bps(bitrate_bps)
        .with_he_v2(true)
        .with_ps_iid_fine(fine_iid);
    let planes: Vec<&[f32]> = pcm.iter().map(AsRef::as_ref).collect();
    crate::encode::encode_he(&planes, sample_rate, &opts)
}

#[cfg(test)]
#[path = "ps_measure_tests.rs"]
mod ps_measure_tests;
