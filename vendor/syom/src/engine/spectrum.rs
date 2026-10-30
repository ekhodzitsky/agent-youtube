//! Spectral Huffman walk, pulse, inverse quant, scalefactor gain — §4.6.1–4.6.3.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::huff;
use super::ics::IcsInfo;
use super::section::{SectionData, has_spectral};
use super::sf::ScaleFactors;
use super::swb::LONG_WINDOW_LEN;
use std::sync::LazyLock;

/// `SF_OFFSET` in §4.6.2.3.3 — scalefactor 100 is unit gain.
pub const SF_OFFSET: i32 = 100;

/// Built on the heap: as a by-value array the initializer needed 52 KiB of
/// stack on the first decoded frame (TASK-118).
static POW43: LazyLock<Box<[f32; 8192]>> = LazyLock::new(|| {
    let mut t = super::heap::heap_array::<f32, 8192>(0.0);
    for (i, slot) in t.iter_mut().enumerate() {
        let a = i as f32;
        *slot = a * a.cbrt();
    }
    t
});

static SF_GAIN: LazyLock<[f32; 256]> = LazyLock::new(|| {
    let mut t = [0.0f32; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        *slot = (0.25 * (i as f32 - SF_OFFSET as f32)).exp2();
    }
    t
});

/// Inverse-quantise one coefficient (§4.6.1.3).
///
/// The cube-root fallback uses the unsigned magnitude. `i32::MIN.abs()`
/// overflows, and no AAC coefficient reaches that value.
#[must_use]
pub fn invquant(q: i32) -> f32 {
    let a = q.unsigned_abs() as usize;
    let mag = if a < POW43.len() {
        POW43[a]
    } else {
        let x = a as f32;
        x * x.cbrt()
    };
    if q < 0 { -mag } else { mag }
}

/// `2^(0.25 * (sf - 100))`.
#[must_use]
pub fn sf_gain(sf: i32) -> f32 {
    if (0..256).contains(&sf) {
        SF_GAIN[sf as usize]
    } else {
        (0.25 * (sf - SF_OFFSET) as f32).exp2()
    }
}

/// Pulse record after Table 4.7.
#[derive(Debug, Clone)]
pub struct PulseData {
    /// `pulse_start_sfb`.
    pub start_sfb: u8,
    pub n: u8,
    /// `(offset, amp)` pairs, 1..=4.
    pub pulses: [(u8, u8); 4],
}

impl PulseData {
    /// Table 4.7 `pulse_data()`.
    pub fn parse(br: &mut BitReader<'_>) -> Result<Self> {
        let n = br.read(2)? as usize + 1;
        let start_sfb = br.read(6)? as u8;
        let mut pulses = [(0u8, 0u8); 4];
        for p in pulses.iter_mut().take(n) {
            let offset = br.read(5)? as u8;
            let amp = br.read(4)? as u8;
            *p = (offset, amp);
        }
        Ok(PulseData {
            start_sfb,
            n: n as u8,
            pulses,
        })
    }
}

/// Fill `quant` (cleared / resized, capacity reused).
pub fn parse_quant_into(
    br: &mut BitReader<'_>,
    ics: &IcsInfo,
    sections: &SectionData,
    fs_index: u8,
    quant: &mut Vec<i32>,
) -> Result<()> {
    let win_len = ics.window_len();
    let nwin = ics.num_windows as usize;
    let n = nwin * win_len;
    if quant.len() != n {
        quant.resize(n, 0);
    } else {
        quant.fill(0);
    }
    let offsets = ics.swb_offsets(fs_index)?;
    let mut wbase = 0usize;
    let mut tuple = [0i32; 4];
    for g in 0..ics.num_window_groups as usize {
        let glen = ics.window_group_length[g] as usize;
        let cbs = sections.sfb_cb.get(g).ok_or(Error::SpectrumInvalid)?;
        for sfb in 0..ics.max_sfb as usize {
            let cb = *cbs.get(sfb).ok_or(Error::SpectrumInvalid)?;
            if !has_spectral(cb) {
                continue;
            }
            let start = *offsets.get(sfb).ok_or(Error::SpectrumInvalid)? as usize;
            let end = *offsets.get(sfb + 1).ok_or(Error::SpectrumInvalid)? as usize;
            if end < start {
                return Err(Error::SpectrumInvalid);
            }
            let width = end - start;
            let bins = width * glen;
            if glen == 1 {
                let base = wbase * win_len + start;
                if base + width > quant.len() {
                    return Err(Error::SpectrumInvalid);
                }
                let mut filled = 0usize;
                while filled < width {
                    let n = huff::decode_tuple(br, cb, &mut tuple)?;
                    let take = n.min(width - filled);
                    quant[base + filled..base + filled + take].copy_from_slice(&tuple[..take]);
                    filled += take;
                    if take < n {
                        break;
                    }
                }
            } else {
                let mut filled = 0usize;
                while filled < bins {
                    let n = huff::decode_tuple(br, cb, &mut tuple)?;
                    for &sample in tuple.iter().take(n) {
                        if filled >= bins {
                            break;
                        }
                        let b = filled / width;
                        let i = filled % width;
                        let idx = (wbase + b) * win_len + start + i;
                        if idx >= quant.len() {
                            return Err(Error::SpectrumInvalid);
                        }
                        quant[idx] = sample;
                        filled += 1;
                    }
                }
            }
        }
        wbase += glen;
    }
    Ok(())
}

/// §4.6.13 pulse reconstruction on a long-window quantised spectrum.
pub fn apply_pulse(quant: &mut [i32], offsets: &[u16], pulse: &PulseData) -> Result<()> {
    let start = pulse.start_sfb as usize;
    if start >= offsets.len().saturating_sub(1) {
        return Err(Error::SpectrumInvalid);
    }
    let mut k = offsets[start] as usize;
    for &(off, amp) in pulse.pulses.iter().take(pulse.n as usize) {
        k += off as usize;
        if k >= quant.len().min(LONG_WINDOW_LEN) {
            return Err(Error::SpectrumInvalid);
        }
        let a = amp as i32;
        if quant[k] > 0 {
            quant[k] += a;
        } else {
            quant[k] -= a;
        }
    }
    Ok(())
}

/// `invquant(q) * gain` over one scalefactor band.
fn rescale_band(spec: &mut [f32], quant: &[i32], gain: f32) {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx512f") {
        // SAFETY: AVX-512F was probed. Gathers stay inside `POW43` because
        // lanes with `|q| >= 8192` (and `i32::MIN`) take the scalar loop.
        unsafe { rescale_band_avx512(spec, quant, gain) }
        return;
    }
    for (slot, &q) in spec.iter_mut().zip(quant) {
        *slot = invquant(q) * gain;
    }
}

/// # Safety
///
/// AVX-512F is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn rescale_band_avx512(spec: &mut [f32], quant: &[i32], gain: f32) {
    use std::arch::x86_64::{
        _mm512_abs_epi32, _mm512_castps_si512, _mm512_castsi512_ps, _mm512_cmpeq_epi32_mask,
        _mm512_cmpge_epi32_mask, _mm512_cmplt_epi32_mask, _mm512_i32gather_ps, _mm512_loadu_si512,
        _mm512_mask_blend_ps, _mm512_mul_ps, _mm512_set1_epi32, _mm512_set1_ps,
        _mm512_setzero_si512, _mm512_storeu_ps, _mm512_xor_si512,
    };
    let n = spec.len().min(quant.len());
    let gain_v = _mm512_set1_ps(gain);
    let sign = _mm512_set1_ps(-0.0);
    let table = POW43.as_ptr();
    let mut i = 0usize;
    while i + 16 <= n {
        // SAFETY: 16 `i32` quantisers and 16 `f32` outputs are in range.
        // Gathers use `|q| < 8192`, so every index lands in `POW43`.
        unsafe {
            let q = _mm512_loadu_si512(quant.as_ptr().add(i).cast());
            let min_q = _mm512_cmpeq_epi32_mask(q, _mm512_set1_epi32(i32::MIN));
            let abs = _mm512_abs_epi32(q);
            let big = _mm512_cmpge_epi32_mask(abs, _mm512_set1_epi32(8192));
            if min_q != 0 || big != 0 {
                for (slot, &v) in spec[i..i + 16].iter_mut().zip(&quant[i..i + 16]) {
                    *slot = invquant(v) * gain;
                }
            } else {
                let neg = _mm512_cmplt_epi32_mask(q, _mm512_setzero_si512());
                let mag = _mm512_i32gather_ps::<4>(abs, table);
                let flipped = _mm512_castsi512_ps(_mm512_xor_si512(
                    _mm512_castps_si512(mag),
                    _mm512_castps_si512(sign),
                ));
                let signed = _mm512_mask_blend_ps(neg, mag, flipped);
                _mm512_storeu_ps(spec.as_mut_ptr().add(i), _mm512_mul_ps(signed, gain_v));
            }
        }
        i += 16;
    }
    for (slot, &q) in spec[i..n].iter_mut().zip(&quant[i..n]) {
        *slot = invquant(q) * gain;
    }
}

/// Fill `spec` (cleared / resized, capacity reused).
pub fn rescale_into(
    quant: &[i32],
    ics: &IcsInfo,
    sections: &SectionData,
    sf: &ScaleFactors,
    fs_index: u8,
    spec: &mut Vec<f32>,
) -> Result<()> {
    let win_len = ics.window_len();
    let nwin = ics.num_windows as usize;
    let n = nwin * win_len;
    if spec.len() != n {
        spec.resize(n, 0.0);
    }
    spec.fill(0.0);
    let offsets = ics.swb_offsets(fs_index)?;
    let mut wbase = 0usize;
    for g in 0..ics.num_window_groups as usize {
        let glen = ics.window_group_length[g] as usize;
        let cbs = sections.sfb_cb.get(g).ok_or(Error::SpectrumInvalid)?;
        for sfb in 0..ics.max_sfb as usize {
            let cb = *cbs.get(sfb).unwrap_or(&0);
            if !has_spectral(cb) {
                continue;
            }
            let gain = sf_gain(*sf.sf.get(g).and_then(|v| v.get(sfb)).unwrap_or(&0));
            let start = *offsets.get(sfb).ok_or(Error::SpectrumInvalid)? as usize;
            let end = *offsets.get(sfb + 1).ok_or(Error::SpectrumInvalid)? as usize;
            for b in 0..glen {
                let base = (wbase + b) * win_len;
                let lo = base + start;
                let hi = base + end;
                // Same values as the per-bin `get` (missing quant stays 0,
                // which `spec.fill(0)` already stored). Hot path is in-range.
                if start <= end && hi <= spec.len() && hi <= quant.len() {
                    rescale_band(&mut spec[lo..hi], &quant[lo..hi], gain);
                } else {
                    for i in start..end {
                        let idx = base + i;
                        spec[idx] = invquant(*quant.get(idx).unwrap_or(&0)) * gain;
                    }
                }
            }
        }
        wbase += glen;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{invquant, rescale_band, sf_gain};

    #[test]
    fn rescale_band_matches_scalar_bits() {
        let mut state = 0x1234_5678u32;
        let mut quant = Vec::with_capacity(80);
        for _ in 0..64 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            quant.push((state % 401) as i32 - 200);
        }
        quant.extend_from_slice(&[0, 1, -1, 8191, -8191, 8192, -9000, i32::MIN]);
        let gain = sf_gain(137);
        let mut fast = vec![1.0f32; quant.len()];
        rescale_band(&mut fast, &quant, gain);
        for (i, (&got, &q)) in fast.iter().zip(&quant).enumerate() {
            let want = invquant(q) * gain;
            assert_eq!(got.to_bits(), want.to_bits(), "i={i} q={q}");
        }
    }
}
