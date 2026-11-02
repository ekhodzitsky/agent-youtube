//! Stream-level LC/LD driver: `raw_data_block()` → planar PCM.

#[cfg(test)]
use super::adts::AdtsHeader;
use super::bits::BitReader;
use super::cce::PendingChan;
use super::channel_map::{Element, PceChannelMap, map_planes_into, mono_mix, reorder};
use super::error::Result;
use super::fb_pool::FbPool;
use super::filterbank::Filterbank;
use super::pns::Lcg;
use super::sbr_attach::SbrPool;
use super::section::SectionData;
use super::sf::ScaleFactors;

/// Filterbank scale is ±32768; public decode maps with this to ~[-1, 1].
pub(crate) const INV_S16: f32 = 1.0 / 32768.0;

/// Owned per-frame output for engine tests (product paths borrow `frame_planes`).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    pub planar: Vec<Vec<f32>>,
    pub channels: usize,
    pub sample_rate: u32,
}

#[derive(Debug, Default)]
pub struct StreamDecoder {
    pub(crate) fb_l: Filterbank,
    pub(crate) fb_r: Filterbank,
    pub(crate) rng: Lcg,
    pub mix_down_mono: bool,
    pub(crate) sbr_pool: SbrPool,
    pub(crate) sbr_active: bool,
    sbr_declared: bool,
    ps_declared: bool,
    pub(crate) sbr_out_rate: Option<u32>,
    pub(crate) pcm_l: Vec<f32>,
    pub(crate) pcm_r: Vec<f32>,
    pub(crate) frame_ch: Vec<Vec<f32>>,
    pub(crate) n_ch: usize,
    pub(crate) quant: Vec<i32>,
    pub(crate) spec_l: Vec<f32>,
    pub(crate) spec_r: Vec<f32>,
    pub(crate) sections_l: SectionData,
    pub(crate) sections_r: SectionData,
    pub(crate) sf_l: ScaleFactors,
    pub(crate) sf_r: ScaleFactors,
    pub(crate) fast_mono: bool,
    pub(crate) elems: Vec<Element>,
    pub(crate) pce: Option<PceChannelMap>,
    pub(crate) fb_pool: FbPool,
    pub(crate) pending: Vec<PendingChan>,
    pub(crate) cces: Vec<super::cce::CcePayload>,
    pub(crate) last_rdb_bytes: usize,
    last_meta: crate::layout::FrameMeta,
    /// Recycled spectral buffers (TASK-78). `stash_chan` swaps filled
    /// `spec_l`/`spec_r` out; finish returns them here instead of dropping.
    pub(crate) spec_pool: Vec<Vec<f32>>,
    /// Reused layout order (TASK-78); filled by `map_planes_into`.
    pub(crate) plane_order: Vec<super::channel_map::PlaneMap>,
    /// Independent-CCE IMDCT scratch (TASK-78).
    pub(crate) cce_pcm: Vec<f32>,
    /// Main predictor, LTP delay line, and SSR filterbank. LC leaves it idle.
    pub(crate) tools: super::tools::Tools,
    pub(crate) pns_shared: Vec<f32>,
    pub(crate) pending_sbrs: Vec<(
        (super::channel_map::ElemKind, u8),
        Box<super::sbr_extension::SbrExtensionData>,
    )>,
    /// Stage totals, printed on drop when `SYOM_PROF` is set.
    pub(crate) prof_lc: u64,
    pub(crate) prof_fin: u64,
    pub(crate) prof_sbr: u64,
    pub(crate) prof_scale: u64,
    pub(crate) prof_frames: u64,
    /// ASC core length (0 = 1024 LC / 512 LD). 960 and 480 select those grids.
    pub(crate) frame_samples: u16,
}

impl StreamDecoder {
    /// Boxed: the state is about 25 KiB and used to sit in every caller's
    /// stack frame (TASK-118).
    pub fn new() -> Box<Self> {
        Box::default()
    }

    /// Seed mapping from an ASC-embedded PCE (M4A/LATM). In-band PCE still wins.
    pub(crate) fn set_config_pce(&mut self, pce: PceChannelMap) {
        self.pce = Some(pce);
    }

    /// Seed HE/PS from ASC so output rate does not wait on a FIL payload.
    pub(crate) fn set_he_config(&mut self, sbr: bool, ps: bool, output_rate: u32) {
        self.sbr_declared = sbr;
        self.ps_declared = ps;
        if sbr && output_rate != 0 {
            self.sbr_active = true;
            self.sbr_out_rate = Some(output_rate);
        }
    }

    #[cfg(test)]
    pub(crate) fn he_config(&self) -> (bool, bool, Option<u32>) {
        (self.sbr_declared, self.ps_declared, self.sbr_out_rate)
    }

    #[cfg(test)]
    pub fn decode_frame(&mut self, header: &AdtsHeader, payload: &[u8]) -> Result<DecodedFrame> {
        self.decode_raw_data_block(
            header.audio_object_type(),
            header.sampling_frequency_index,
            header.sample_rate(),
            header.channel_configuration,
            header.number_of_raw_data_blocks_in_frame,
            payload,
        )
    }

    #[cfg(test)]
    pub fn decode_raw_data_block(
        &mut self,
        aot: u8,
        fs_index: u8,
        sample_rate: u32,
        channel_configuration: u8,
        num_raw_data_blocks: u8,
        payload: &[u8],
    ) -> Result<DecodedFrame> {
        let sample_rate = if num_raw_data_blocks <= 1 {
            self.decode_into_bufs(aot, fs_index, sample_rate, channel_configuration, payload)?
        } else {
            self.decode_adts_blocks(
                aot,
                fs_index,
                sample_rate,
                channel_configuration,
                num_raw_data_blocks,
                true,
                payload,
            )?
        };
        Ok(DecodedFrame {
            planar: self.frame_ch.clone(),
            channels: self.frame_ch.len(),
            sample_rate,
        })
    }

    /// Streaming seam: decode one `raw_data_block()` and scale the planes
    /// to ~[-1, 1] **in place** — borrow them with [`Self::frame_planes`]
    /// instead of cloning a [`DecodedFrame`].
    pub(crate) fn decode_frame_scaled(
        &mut self,
        aot: u8,
        fs_index: u8,
        sample_rate: u32,
        channel_configuration: u8,
        payload: &[u8],
    ) -> Result<u32> {
        let rate =
            self.decode_into_bufs(aot, fs_index, sample_rate, channel_configuration, payload)?;
        let t_sc = super::prof::stamp();
        for ch in &mut self.frame_ch {
            for v in ch.iter_mut() {
                *v *= INV_S16;
            }
        }
        self.prof_scale += super::prof::ns(t_sc);
        if super::prof::on() {
            self.prof_frames = self.prof_frames.saturating_add(1);
        }
        Ok(rate)
    }

    /// Last `decode_into_bufs` planes (filterbank scale unless scaled).
    pub(crate) fn frame_planes(&self) -> &[Vec<f32>] {
        &self.frame_ch
    }

    pub(crate) fn decode_into_bufs(
        &mut self,
        aot: u8,
        fs_index: u8,
        sample_rate: u32,
        channel_configuration: u8,
        payload: &[u8],
    ) -> Result<u32> {
        meta::check_config(aot, channel_configuration)?;
        self.refill_spec_bufs();
        let core_aot = match aot {
            1 | 3 | 4 | 23 => aot,
            _ => 2,
        };
        self.tools.aot = core_aot;
        let mut br = BitReader::new(payload);
        // cfg 0 / ≥3 / PCE: per-channel FBs. cfg 1/2 keep the fast path.
        let multichannel =
            channel_configuration == 0 || channel_configuration >= 3 || self.pce.is_some();
        self.tools.multichannel = multichannel;
        self.tools.begin_frame();
        if multichannel {
            self.fast_mono = false;
        }
        self.n_ch = 0;
        self.elems.clear();
        self.pending.clear();
        self.cces.clear();
        self.fb_pool.begin_frame();
        self.sbr_pool.begin_frame();
        self.pending_sbrs.clear();
        let t_lc = super::prof::stamp();
        let ld = core_aot == super::asc::AOT_LD;
        if ld {
            self.decode_ld_elements(&mut br, fs_index, core_aot, channel_configuration)?;
        } else {
            self.decode_lc_elements(
                &mut br,
                fs_index,
                core_aot,
                sample_rate,
                channel_configuration,
                multichannel,
            )?;
        }
        if self.frame_samples == 960
            && (self.sbr_declared
                || self.ps_declared
                || self.sbr_active
                || !self.pending_sbrs.is_empty())
        {
            return Err(super::error::Error::UnsupportedHeFrame960);
        }
        self.prof_lc += super::prof::ns(t_lc);
        br.byte_align()?;
        self.last_rdb_bytes = (br.bit_position() / 8) as usize;
        let he = self.sbr_active || !self.pending_sbrs.is_empty();
        let was_fast = self.fast_mono;
        if he {
            self.fast_mono = false;
        }
        let t_fin = super::prof::stamp();
        self.finish_pending(fs_index, multichannel, he)?;
        self.prof_fin += super::prof::ns(t_fin);
        if was_fast && !he {
            self.fb_pool.retain_seen();
            self.tools.retain_seen();
            self.fast_mono = was_fast;
            self.store_meta(channel_configuration, sample_rate, sample_rate, None, 1);
            return Ok(sample_rate);
        }
        if he {
            for p in self.frame_ch.iter_mut().skip(self.n_ch) {
                p.clear();
            }
        } else {
            self.frame_ch.truncate(self.n_ch);
        }
        let mut order_buf = [super::channel_map::PlaneMap::default(); 8];
        let mut n_order = 0usize;
        let has_order = self.pce.is_some() || (multichannel && !self.frame_ch.is_empty());
        if has_order {
            map_planes_into(
                &mut self.plane_order,
                &self.elems,
                self.pce.as_ref(),
                channel_configuration,
                fs_index,
            )?;
            n_order = self.plane_order.len().min(8);
            order_buf[..n_order].copy_from_slice(&self.plane_order[..n_order]);
        }
        let order = has_order.then_some(&order_buf[..n_order]);
        if !he {
            if let Some(order) = order {
                reorder(&mut self.frame_ch, order);
                if self.mix_down_mono {
                    let mono = mono_mix(&self.frame_ch, order);
                    self.pcm_l.clear();
                    self.pcm_l.extend_from_slice(&mono);
                    self.frame_ch.truncate(1);
                    self.frame_ch[0] = mono;
                }
            }
            self.fb_pool.retain_seen();
            self.tools.retain_seen();
            self.fast_mono = was_fast;
            self.store_meta(
                channel_configuration,
                sample_rate,
                sample_rate,
                order,
                self.frame_ch.len(),
            );
            return Ok(sample_rate);
        }
        let core = sample_rate;
        let t_sbr = super::prof::stamp();
        let out_rate = super::sbr_attach::apply_and_layout(
            &mut self.sbr_pool,
            &self.elems,
            &mut self.frame_ch,
            sample_rate,
            self.sbr_out_rate,
            &mut self.pending_sbrs,
            &mut self.sbr_active,
            self.mix_down_mono,
            order,
        )?;
        self.prof_sbr += super::prof::ns(t_sbr);
        self.fb_pool.retain_seen();
        self.tools.retain_seen();
        self.sbr_pool.retain_seen();
        self.fast_mono = was_fast;
        self.store_meta(
            channel_configuration,
            core,
            out_rate,
            order,
            self.frame_ch.len(),
        );
        Ok(out_rate)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode_raw_mono_f32(
        &mut self,
        aot: u8,
        fs_index: u8,
        sample_rate: u32,
        channel_configuration: u8,
        _num_raw_data_blocks: u8,
        payload: &[u8],
        dst: &mut Vec<f32>,
    ) -> Result<u32> {
        let was = self.mix_down_mono;
        self.mix_down_mono = true;
        self.fast_mono = true;
        let rate = self.decode_adts_blocks(
            aot,
            fs_index,
            sample_rate,
            channel_configuration,
            _num_raw_data_blocks,
            true,
            payload,
        );
        self.fast_mono = false;
        self.mix_down_mono = was;
        let rate = rate?;
        let src = if self.sbr_active {
            self.frame_ch.first().map(Vec::as_slice).unwrap_or(&[])
        } else {
            self.pcm_l.as_slice()
        };
        dst.extend(src.iter().map(|&v| v * INV_S16));
        Ok(rate)
    }

    pub(crate) fn push_pcm(&mut self, right: bool) {
        if self.fast_mono {
            self.n_ch = 1;
            return;
        }
        let i = self.n_ch;
        let src = if right { &self.pcm_r } else { &self.pcm_l };
        if i < self.frame_ch.len() {
            self.frame_ch[i].clear();
            self.frame_ch[i].extend_from_slice(src);
        } else {
            self.frame_ch.push(src.to_vec());
        }
        self.n_ch += 1;
    }
}

impl Drop for StreamDecoder {
    fn drop(&mut self) {
        if self.prof_frames == 0 {
            return;
        }
        let n = self.prof_frames;
        eprintln!(
            "dec_prof frames={n} lc_ms={:.3} fin_ms={:.3} sbr_ms={:.3} scale_ms={:.3}",
            self.prof_lc as f64 / 1e6,
            self.prof_fin as f64 / 1e6,
            self.prof_sbr as f64 / 1e6,
            self.prof_scale as f64 / 1e6
        );
    }
}

#[path = "decode_meta.rs"]
mod meta;

#[path = "decode_reset.rs"]
mod reset;
