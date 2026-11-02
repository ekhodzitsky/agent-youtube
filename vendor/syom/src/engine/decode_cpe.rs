//! SCE finish and CPE pair for [`super::decode::StreamDecoder`].

use super::bits::BitReader;
use super::cce::{PendingChan, parse_cce};
use super::channel_map::{ElemKind, Element};
use super::decode::StreamDecoder;
use super::error::Result;
use super::ics::IcsInfo;
use super::ics_body::parse_ics_into;
use super::pns;
use super::stereo::{self, MsInfo};
use super::tns;

type CpeBody = (
    IcsInfo,
    Option<tns::TnsData>,
    Option<super::ssr::GainControl>,
    IcsInfo,
    Option<tns::TnsData>,
    Option<super::ssr::GainControl>,
);

impl StreamDecoder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stash_chan(
        &mut self,
        kind: ElemKind,
        tag: u8,
        part: u8,
        ics: IcsInfo,
        tns: Option<tns::TnsData>,
        take_l: bool,
        push_elem: bool,
    ) {
        let spec = self.take_spec(!take_l);
        if push_elem && part == 0 {
            self.elems.push(Element {
                kind,
                tag,
                plane: self.n_ch,
            });
        }
        self.pending.push(PendingChan {
            kind,
            tag,
            part,
            ics,
            tns,
            spec,
            gain: None,
        });
        self.n_ch += 1;
    }

    fn take_spec(&mut self, right: bool) -> Vec<f32> {
        let buf = if right {
            &mut self.spec_r
        } else {
            &mut self.spec_l
        };
        let filled = std::mem::take(buf);
        *buf = self.spec_pool.pop().unwrap_or_default();
        filled
    }

    pub(crate) fn recycle_spec(&mut self, mut spec: Vec<f32>) {
        spec.clear();
        self.spec_pool.push(spec);
    }

    pub(crate) fn finish_sce_pns(&mut self, ics: &IcsInfo, fs_index: u8) -> Result<()> {
        pns::apply(
            &mut self.spec_l,
            ics,
            &self.sections_l,
            &self.sf_l,
            fs_index,
            &mut self.rng,
            None,
        )
    }

    pub(crate) fn decode_cpe(
        &mut self,
        br: &mut BitReader<'_>,
        fs_index: u8,
        aot: u8,
        _tag: u8,
    ) -> Result<CpeBody> {
        let common_window = br.read_bit()?;
        let (ics_common, ms) = if common_window {
            let ics = super::ics::IcsInfo::parse_mode(br, fs_index, true, aot, self.frame_samples)?;
            let ms = MsInfo::parse(br, &ics)?;
            (Some(ics), Some(ms))
        } else {
            (None, None)
        };
        let (ics_l, tns_l, gain_l) = parse_ics_into(
            br,
            fs_index,
            aot,
            ics_common.as_ref(),
            self.frame_samples,
            &mut self.quant,
            &mut self.spec_l,
            &mut self.sections_l,
            &mut self.sf_l,
        )?;
        let (ics_r, tns_r, gain_r) = parse_ics_into(
            br,
            fs_index,
            aot,
            ics_common.as_ref(),
            self.frame_samples,
            &mut self.quant,
            &mut self.spec_r,
            &mut self.sections_r,
            &mut self.sf_r,
        )?;
        if let Some(ms) = ms.as_ref() {
            stereo::apply_ms(
                &mut self.spec_l,
                &mut self.spec_r,
                &ics_l,
                &self.sections_l,
                &self.sections_r,
                ms,
                fs_index,
            )?;
        }
        let mut have_shared = false;
        {
            let mut pair = pns::PairPns {
                ms: ms.as_ref(),
                other_cb: Some(&self.sections_r.sfb_cb),
                shared: &mut self.pns_shared,
                have_shared: &mut have_shared,
            };
            pns::apply(
                &mut self.spec_l,
                &ics_l,
                &self.sections_l,
                &self.sf_l,
                fs_index,
                &mut self.rng,
                Some(&mut pair),
            )?;
        }
        {
            let mut pair = pns::PairPns {
                ms: ms.as_ref(),
                other_cb: Some(&self.sections_l.sfb_cb),
                shared: &mut self.pns_shared,
                have_shared: &mut have_shared,
            };
            pns::apply(
                &mut self.spec_r,
                &ics_r,
                &self.sections_r,
                &self.sf_r,
                fs_index,
                &mut self.rng,
                Some(&mut pair),
            )?;
        }
        if self.tools.aot == super::asc::AOT_MAIN {
            if self.tools.multichannel {
                self.tools.swap_pair(_tag);
            }
            self.tools
                .apply_main(&mut self.spec_l, &ics_l, fs_index, false)?;
            self.tools
                .apply_main(&mut self.spec_r, &ics_r, fs_index, true)?;
            if self.tools.multichannel {
                self.tools.swap_pair(_tag);
            }
        }
        if let Some(ms) = ms.as_ref() {
            stereo::apply_intensity(
                &self.spec_l,
                &mut self.spec_r,
                &ics_r,
                &self.sections_r,
                &self.sf_r,
                ms,
                fs_index,
            )?;
        }
        Ok((ics_l, tns_l, gain_l, ics_r, tns_r, gain_r))
    }

    pub(crate) fn refill_spec_bufs(&mut self) {
        if self.spec_l.capacity() == 0 {
            self.spec_l = self.spec_pool.pop().unwrap_or_default();
        }
        if self.spec_r.capacity() == 0 {
            self.spec_r = self.spec_pool.pop().unwrap_or_default();
        }
    }

    /// LC / HE element loop: 3-bit element IDs up to `ID_END` (the ER AAC LD
    /// id-less walk lives in `decode_ld`).
    pub(crate) fn decode_lc_elements(
        &mut self,
        br: &mut BitReader<'_>,
        fs_index: u8,
        core_aot: u8,
        sample_rate: u32,
        channel_configuration: u8,
        multichannel: bool,
    ) -> Result<()> {
        let mut last_elem = None;
        loop {
            if br.bits_remaining() < 3 {
                break;
            }
            let id = super::raw_data_block::IdSynEle::from_bits(br.read(3)? as u8);
            match id {
                super::raw_data_block::IdSynEle::End => break,
                super::raw_data_block::IdSynEle::Sce | super::raw_data_block::IdSynEle::Lfe => {
                    let tag = br.read(4)? as u8;
                    let kind = if id == super::raw_data_block::IdSynEle::Sce {
                        ElemKind::Sce
                    } else {
                        ElemKind::Lfe
                    };
                    last_elem = Some((kind, tag));
                    if multichannel {
                        super::fb_pool::reject_dup(&self.elems, kind, tag)?;
                    }
                    let (ics, tns, gain) = parse_ics_into(
                        br,
                        fs_index,
                        core_aot,
                        None,
                        self.frame_samples,
                        &mut self.quant,
                        &mut self.spec_l,
                        &mut self.sections_l,
                        &mut self.sf_l,
                    )?;
                    self.finish_sce_pns(&ics, fs_index)?;
                    if self.tools.aot == super::asc::AOT_MAIN {
                        if multichannel {
                            self.tools.swap(kind, tag, 0);
                        }
                        self.tools
                            .apply_main(&mut self.spec_l, &ics, fs_index, false)?;
                        if multichannel {
                            self.tools.swap(kind, tag, 0);
                        }
                    }
                    self.stash_chan(kind, tag, 0, ics, tns, true, true);
                    if let (Some(g), Some(p)) = (gain, self.pending.last_mut()) {
                        p.gain = Some(Box::new(g));
                    }
                }
                super::raw_data_block::IdSynEle::Cpe => {
                    let tag = br.read(4)? as u8;
                    last_elem = Some((ElemKind::Cpe, tag));
                    if multichannel {
                        super::fb_pool::reject_dup(&self.elems, ElemKind::Cpe, tag)?;
                    }
                    let (ics_l, tns_l, gain_l, ics_r, tns_r, gain_r) =
                        self.decode_cpe(br, fs_index, core_aot, tag)?;
                    self.stash_chan(ElemKind::Cpe, tag, 0, ics_l, tns_l, true, true);
                    if let (Some(g), Some(p)) = (gain_l, self.pending.last_mut()) {
                        p.gain = Some(Box::new(g));
                    }
                    self.stash_chan(ElemKind::Cpe, tag, 1, ics_r, tns_r, false, true);
                    if let (Some(g), Some(p)) = (gain_r, self.pending.last_mut()) {
                        p.gain = Some(Box::new(g));
                    }
                }
                super::raw_data_block::IdSynEle::Cce => {
                    self.cces
                        .push(parse_cce(br, fs_index, core_aot, self.frame_samples)?);
                }
                super::raw_data_block::IdSynEle::Dse => super::skip::skip_dse(br)?,
                super::raw_data_block::IdSynEle::Pce => {
                    self.pce = Some(super::channel_map::parse_pce(br)?);
                }
                super::raw_data_block::IdSynEle::Fil => {
                    let cnt = super::skip::fill_count(br)?;
                    let fs_sbr = self.sbr_out_rate.unwrap_or(sample_rate.saturating_mul(2));
                    super::sbr_attach::ingest_fil(
                        br,
                        cnt,
                        last_elem,
                        fs_sbr,
                        &mut self.sbr_pool,
                        &mut self.pending_sbrs,
                    )?;
                }
            }
        }
        let _ = channel_configuration;
        Ok(())
    }
}
