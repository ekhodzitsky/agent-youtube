//! AAC-LD access unit. 512-sample frames, `ONLY_LONG`, no element id and
//! no `ID_END`. Opt-in through `EncodeOptions::with_ld` (LOAS or M4A).

use super::adts::ADTS_SAMPLE_RATES_HZ;
use super::bits::BitWriter;
use super::enc_alloc::noise_targets;
use super::enc_frame::TARGET_Q;
use super::enc_psy::{AttackDetector, Psy};
use super::enc_quant::{self, QuantChannel};
use super::enc_section::{emit_scale_factors, emit_section_data, emit_spectral, plan_books};
use super::enc_tns::{self, EncTns};
use super::error::{Error, Result};
use super::filterbank::ld_analysis_window;
use super::ics::{WindowSequence, WindowShape};
use super::mdct::mdct_into_f32;
use super::swb::{self, LD_WINDOW_LEN};
use super::tns;

const FRAME: usize = LD_WINDOW_LEN;
const WIN: usize = FRAME * 2;
/// Coarsest allowed-noise offset tried when a frame is over budget.
const OFFSET_CAP: i32 = 60;
/// Same hard ceiling as LC (`enc_frame::MAX_BITS_PER_CHANNEL`). Not a new LD cap.
const HARD_BITS_PER_CH: u64 = 6144;

/// One or two channels, sine window unless the attack detector asks for
/// the low-overlap shape. `bitrate_bps == 0` keeps the psy target.
pub(crate) struct LdEncoder {
    prev: [[f32; FRAME]; 2],
    det: [AttackDetector; 2],
    prev_shape: Option<WindowShape>,
    psy: Psy,
    fs_index: u8,
    sample_rate: u32,
    bitrate_bps: u32,
    channels: usize,
    offsets: &'static [u16],
}

impl LdEncoder {
    pub(crate) fn new(sample_rate: u32) -> Result<Self> {
        Self::with_channels(sample_rate, 1)
    }

    pub(crate) fn stereo(sample_rate: u32) -> Result<Self> {
        Self::with_channels(sample_rate, 2)
    }

    /// Whole-stream ABR target. Zero disables the frame cap.
    pub(crate) fn set_bitrate(&mut self, bitrate_bps: u32) {
        self.bitrate_bps = bitrate_bps;
    }

    fn with_channels(sample_rate: u32, channels: usize) -> Result<Self> {
        if channels != 1 && channels != 2 {
            return Err(Error::Format("LD encoder: mono or stereo"));
        }
        let fs_index = ADTS_SAMPLE_RATES_HZ
            .iter()
            .position(|&r| r == sample_rate)
            .ok_or(Error::UnsupportedSampleRateIndex(0x0f))?;
        let fs_index =
            u8::try_from(fs_index).map_err(|_| Error::UnsupportedSampleRateIndex(0x0f))?;
        let offsets = swb::long_offsets_ld(fs_index)?;
        Ok(Self {
            prev: [[0.0; FRAME]; 2],
            det: [AttackDetector::new(), AttackDetector::new()],
            prev_shape: None,
            psy: Psy::new_ld(offsets, sample_rate),
            fs_index,
            sample_rate,
            bitrate_bps: 0,
            channels,
            offsets,
        })
    }

    #[cfg(test)]
    pub(crate) fn push_mono(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        self.push(&[pcm])
    }

    #[cfg(test)]
    pub(crate) fn push_loas(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        let au = self.push_mono(pcm)?;
        self.wrap_loas(&au)
    }

    #[cfg(test)]
    pub(crate) fn push_stereo_loas(&mut self, left: &[f32], right: &[f32]) -> Result<Vec<u8>> {
        let au = self.push(&[left, right])?;
        self.wrap_loas(&au)
    }

    pub(crate) fn asc(&self) -> Vec<u8> {
        asc_bytes(self.fs_index, self.channels as u8)
    }

    #[cfg(test)]
    fn wrap_loas(&self, au: &[u8]) -> Result<Vec<u8>> {
        let asc = self.asc();
        let bits = super::latm_write::asc_bit_len(&asc)?;
        let mut out = Vec::new();
        super::latm_write::loas_frame_into(&asc, bits, au, &mut out)?;
        Ok(out)
    }

    pub(crate) fn fs_index(&self) -> u8 {
        self.fs_index
    }

    /// Drop overlap and the attack detector. Rate and channel count stay.
    pub(crate) fn reset(&mut self) {
        self.prev = [[0.0; FRAME]; 2];
        self.det = [AttackDetector::new(), AttackDetector::new()];
        self.prev_shape = None;
    }

    pub(crate) fn push(&mut self, planes: &[&[f32]]) -> Result<Vec<u8>> {
        if planes.len() != self.channels || planes.iter().any(|p| p.len() != FRAME) {
            return Err(Error::Format("LD encoder: frame is 512 samples"));
        }
        let attack = (0..self.channels).any(|ch| self.det[ch].push(planes[ch]));
        let shape = if attack {
            WindowShape::Kbd
        } else {
            WindowShape::Sine
        };
        let left = self.prev_shape.unwrap_or(shape);
        let mut specs = [spec_buf(), spec_buf()];
        for ch in 0..self.channels {
            specs[ch] = mdct_channel(&self.prev[ch], planes[ch], left, shape);
            self.prev[ch].copy_from_slice(planes[ch]);
        }
        self.prev_shape = Some(shape);
        let limit = self.limit_bits();
        let mut offset = 0i32;
        let mut au = self.emit_at(&specs, shape, offset)?;
        while bit_len_of(&au) > limit && offset < OFFSET_CAP {
            offset += 4;
            au = self.emit_at(&specs, shape, offset)?;
        }
        let hard = HARD_BITS_PER_CH * self.channels as u64;
        if bit_len_of(&au) > hard {
            return Err(Error::Format("LD encoder: frame exceeds 6144 bits/channel"));
        }
        Ok(au)
    }

    /// ABR budget `bitrate * 512 / rate`, never above 6144 bits/channel.
    /// A zero bitrate is the hard ceiling alone (psy target, no ABR).
    fn limit_bits(&self) -> u64 {
        let hard = HARD_BITS_PER_CH * self.channels as u64;
        if self.bitrate_bps == 0 {
            return hard;
        }
        let budget =
            u64::from(self.bitrate_bps) * FRAME as u64 / u64::from(self.sample_rate.max(1));
        budget.min(hard)
    }

    fn emit_at(
        &mut self,
        specs: &[Box<[f32; 1024]>; 2],
        shape: WindowShape,
        offset: i32,
    ) -> Result<Vec<u8>> {
        let (q0, t0) = self.quantize(&specs[0], offset)?;
        if self.channels == 1 {
            return Ok(emit_mono(self.offsets, &q0, &t0, shape));
        }
        let (q1, t1) = self.quantize(&specs[1], offset)?;
        Ok(emit_stereo(self.offsets, &q0, &t0, &q1, &t1, shape))
    }

    fn quantize(&mut self, spec: &[f32; 1024], offset: i32) -> Result<(QuantChannel, EncTns)> {
        let n = self.offsets.len() - 1;
        let mut coded = [false; enc_quant::MAX_BANDS];
        let mut cap = [0.0f32; enc_quant::MAX_BANDS];
        self.psy
            .analyze(spec, self.offsets, TARGET_Q, &mut coded, &mut cap);
        let (energy, noise, sum_sqrt) = self.psy.bands();
        let energy = *energy;
        let noise = *noise;
        let sum_sqrt = *sum_sqrt;
        let mut width = [0.0f32; enc_quant::MAX_BANDS];
        let mut peaks = [0.0f32; enc_quant::MAX_BANDS];
        enc_quant::band_peaks(spec, self.offsets, &mut peaks);
        for (b, w) in width.iter_mut().enumerate().take(n) {
            *w = f32::from(self.offsets[b + 1] - self.offsets[b]);
        }
        let mut target_q = [0.0f32; enc_quant::MAX_BANDS];
        noise_targets(
            &peaks,
            &energy,
            &sum_sqrt,
            &noise,
            &width,
            &cap,
            offset,
            n,
            &mut coded,
            &mut target_q,
        );
        let mut quant_spec = *spec;
        let mut tns = enc_tns::decide_long(&mut quant_spec, self.offsets, self.fs_index, &coded);
        let cap_bands = usize::from(tns::max_bands(
            self.fs_index,
            WindowSequence::OnlyLong,
            true,
        )?);
        if tns.band_range().is_some_and(|(_, end)| end > cap_bands) {
            quant_spec = *spec;
            tns = EncTns::off();
        }
        let mut q = QuantChannel::new(n);
        q.coded[..n].copy_from_slice(&coded[..n]);
        enc_quant::raw_scalefactors(&peaks, &target_q, 0, &mut q);
        let _ = enc_quant::normalize_sf(&mut q);
        enc_quant::cache_mags(&quant_spec, &mut q.mag);
        enc_quant::quantize_cached(&quant_spec, self.offsets, &mut q);
        Ok((q, tns))
    }
}

fn spec_buf() -> Box<[f32; 1024]> {
    super::heap::heap_array(0.0)
}

fn mdct_channel(
    prev: &[f32; FRAME],
    pcm: &[f32],
    left: WindowShape,
    right: WindowShape,
) -> Box<[f32; 1024]> {
    let wl = ld_analysis_window(left);
    let wr = ld_analysis_window(right);
    let mut time = [0.0f32; WIN];
    for i in 0..FRAME {
        time[i] = prev[i] * wl[i] * 32768.0;
        time[FRAME + i] = pcm[i] * wr[FRAME + i] * 32768.0;
    }
    let mut lines = [0.0f32; FRAME];
    mdct_into_f32(&time, &mut lines);
    let mut spec = spec_buf();
    spec[..FRAME].copy_from_slice(&lines);
    spec
}

fn bit_len_of(au: &[u8]) -> u64 {
    // Byte-aligned length: up to 7 bits above the unpadded payload.
    (au.len() as u64).saturating_mul(8)
}

fn asc_bytes(fs_index: u8, channels: u8) -> Vec<u8> {
    // FDK's LD LOAS layout, which libxaac's ASC parser also requires:
    // extensionFlag 1, three zero resilience flags, extensionFlag3 0,
    // epConfig 0. extensionFlag 0 omits epConfig on that parser and the
    // mux length that follows is read as config.
    let mut w = BitWriter::new();
    w.write(23, 5);
    w.write(u32::from(fs_index), 4);
    w.write(u32::from(channels), 4);
    w.write_bit(false); // frameLengthFlag: 512, not 480
    w.write_bit(false); // dependsOnCoreCoder
    w.write_bit(true);
    w.write(0, 3);
    w.write_bit(false);
    w.write(0, 2); // epConfig
    w.finish()
}

fn gain_of(q: &QuantChannel) -> u8 {
    q.sf.iter()
        .zip(q.coded.iter())
        .find(|(_, c)| **c)
        .map(|(sf, _)| *sf as u8)
        .unwrap_or(0)
}

fn emit_body(
    w: &mut BitWriter,
    offsets: &[u16],
    q: &QuantChannel,
    tns: &EncTns,
    shape: WindowShape,
    with_ics: bool,
) {
    let books = plan_books(q);
    let global_gain = gain_of(q);
    w.write(u32::from(global_gain), 8);
    if with_ics {
        write_ics(w, q.n_bands as u8, shape);
    }
    emit_section_data(w, &books, q.n_bands);
    emit_scale_factors(w, &books, q, global_gain);
    w.write_bit(false);
    w.write_bit(tns.band_range().is_some());
    w.write_bit(false);
    tns.emit_body(w);
    emit_spectral(w, offsets, &books, q);
}

fn write_ics(w: &mut BitWriter, max_sfb: u8, shape: WindowShape) {
    w.write_bit(false);
    w.write(0, 2);
    w.write_bit(shape == WindowShape::Kbd);
    w.write(u32::from(max_sfb), 6);
    w.write_bit(false);
}

fn emit_mono(offsets: &[u16], q: &QuantChannel, tns: &EncTns, shape: WindowShape) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write(0, 4);
    emit_body(&mut w, offsets, q, tns, shape, true);
    w.finish()
}

fn emit_stereo(
    offsets: &[u16],
    l: &QuantChannel,
    tl: &EncTns,
    r: &QuantChannel,
    tr: &EncTns,
    shape: WindowShape,
) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write(0, 4); // tag
    w.write_bit(true); // common_window
    write_ics(&mut w, l.n_bands as u8, shape);
    w.write(0, 2); // ms_mask_present = 0
    emit_body(&mut w, offsets, l, tl, shape, false);
    emit_body(&mut w, offsets, r, tr, shape, false);
    w.finish()
}

#[cfg(test)]
#[path = "enc_ld_tests.rs"]
mod enc_ld_tests;
