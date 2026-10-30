//! `coupling_channel_element()` — ISO/IEC 14496-3 Table 4.4.8.
//!
//! Parse matches FAAD2 / FFmpeg `decode_cce`. Dependent coupling is applied
//! to spectral coefficients (FFmpeg `apply_dependent_coupling`) at
//! BEFORE_TNS (`domain == 0`) or BETWEEN_TNS_AND_IMDCT (`domain == 1`).
//! Independent (after-IMDCT) coupling is not applied here.

use super::bits::BitReader;
use super::channel_map::ElemKind;
use super::det_math;
use super::error::{Error, Result};
use super::ics::IcsInfo;
use super::ics_body::parse_ics_len;
use super::section::{SectionData, ZERO_HCB};
use super::sf::{self, ScaleFactors};
use super::tns::TnsData;

/// One CCE target (SCE or CPE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CceTarget {
    pub is_cpe: bool,
    pub tag: u8,
    pub cc_l: bool,
    pub cc_r: bool,
}

/// Fully parsed CCE, including the coupling-channel spectrum and gains.
#[derive(Debug, Clone)]
pub struct CcePayload {
    pub tag: u8,
    pub independent: bool,
    #[allow(dead_code)]
    pub n_coupled: usize,
    #[allow(dead_code)]
    pub n_gain_lists: usize,
    /// 0 = before TNS, 1 = after TNS (dependent only).
    pub domain: u8,
    pub targets: Vec<CceTarget>,
    pub ics: IcsInfo,
    pub sections: SectionData,
    pub sf: ScaleFactors,
    pub spec: Vec<f32>,
    pub tns: Option<TnsData>,
    /// `gains[list][group * max_sfb + sfb]` for coded CCE bands.
    pub gains: Vec<Vec<f32>>,
}

/// Per-channel stash until coupling + TNS + IMDCT.
#[derive(Debug, Clone)]
pub(crate) struct PendingChan {
    pub kind: ElemKind,
    pub tag: u8,
    pub part: u8,
    pub ics: IcsInfo,
    pub tns: Option<TnsData>,
    pub spec: Vec<f32>,
    /// SSR gain for this channel. LC leaves it empty.
    pub gain: Option<std::boxed::Box<super::ssr::GainControl>>,
}

/// Parse a CCE completely (FAAD2 Table 4.4.8 / FFmpeg `decode_cce`).
pub fn parse_cce(
    br: &mut BitReader<'_>,
    fs_index: u8,
    aot: u8,
    frame_len: u16,
) -> Result<CcePayload> {
    let tag = br.read(4)? as u8;
    let independent = br.read_bit()?;
    let n_coupled = br.read(3)? as usize + 1;
    let mut n_gain_lists = 0usize;
    let mut targets = Vec::with_capacity(n_coupled);
    for _ in 0..n_coupled {
        n_gain_lists += 1;
        let is_cpe = br.read_bit()?;
        let tgt_tag = br.read(4)? as u8;
        let (cc_l, cc_r) = if is_cpe {
            let l = br.read_bit()?;
            let r = br.read_bit()?;
            if l && r {
                n_gain_lists += 1;
            }
            (l, r)
        } else {
            (true, false)
        };
        targets.push(CceTarget {
            is_cpe,
            tag: tgt_tag,
            cc_l,
            cc_r,
        });
    }
    let domain = u8::from(br.read_bit()?);
    let sign = br.read_bit()?;
    let scale_idx = br.read(2)? as u8;
    let body = parse_ics_len(br, fs_index, aot, None, frame_len)?;
    let groups = body.ics.num_window_groups as usize;
    let max_sfb = body.ics.max_sfb as usize;
    let n_band = groups.saturating_mul(max_sfb);
    let scale_e = [0.125f32, 0.25, 0.5, 1.0][scale_idx as usize];
    let mut gains = vec![vec![0.0f32; n_band]; n_gain_lists];
    for (c, list) in gains.iter_mut().enumerate() {
        let mut cge = true;
        let mut gain_acc = 0i32;
        let mut cache = 1.0f32;
        if c > 0 {
            cge = independent || br.read_bit()?;
            if cge {
                gain_acc = i32::from(sf::decode_dpcm(br)?);
                cache = det_math::exp2(-(gain_acc as f32) * scale_e);
            }
        }
        if independent {
            if let Some(slot) = list.first_mut() {
                *slot = cache;
            }
            continue;
        }
        let mut idx = 0usize;
        for g in 0..groups {
            let row = body.sections.sfb_cb.get(g);
            for sfb in 0..max_sfb {
                let cb = row.and_then(|r| r.get(sfb)).copied().unwrap_or(ZERO_HCB);
                if cb != ZERO_HCB {
                    if !cge {
                        let t = i32::from(sf::decode_dpcm(br)?);
                        if t != 0 {
                            gain_acc += t;
                            let mut tt = gain_acc;
                            let mut s = 1.0f32;
                            if sign {
                                s = 1.0 - 2.0 * (tt & 1) as f32;
                                tt >>= 1;
                            }
                            cache = det_math::exp2(-(tt as f32) * scale_e) * s;
                        }
                    }
                    list[idx] = cache;
                }
                idx += 1;
            }
        }
    }
    Ok(CcePayload {
        tag,
        independent,
        n_coupled,
        n_gain_lists,
        domain,
        targets,
        ics: body.ics,
        sections: body.sections,
        sf: body.sf,
        spec: body.spec,
        tns: body.tns,
        gains,
    })
}

/// Add `gain * cce_spec` into `target` on non-zero CCE bands (FFmpeg).
pub fn apply_dependent(
    target: &mut [f32],
    cce: &CcePayload,
    list: usize,
    fs_index: u8,
) -> Result<()> {
    let Some(gains) = cce.gains.get(list) else {
        return Err(Error::Format("CCE gain list missing"));
    };
    if target.len() != cce.spec.len() {
        return Err(Error::Format("CCE window mismatch"));
    }
    let ics = &cce.ics;
    let offsets = ics.swb_offsets(fs_index)?;
    let win_len = ics.window_len();
    let mut wbase = 0usize;
    let mut idx = 0usize;
    for g in 0..ics.num_window_groups as usize {
        let glen = ics.window_group_length[g] as usize;
        let row = cce.sections.sfb_cb.get(g);
        for sfb in 0..ics.max_sfb as usize {
            let cb = row.and_then(|r| r.get(sfb)).copied().unwrap_or(ZERO_HCB);
            if cb != ZERO_HCB {
                let gain = *gains.get(idx).unwrap_or(&0.0);
                let start = *offsets.get(sfb).ok_or(Error::SpectrumInvalid)? as usize;
                let end = *offsets.get(sfb + 1).ok_or(Error::SpectrumInvalid)? as usize;
                for b in 0..glen {
                    let base = (wbase + b) * win_len;
                    for k in start..end {
                        let i = base + k;
                        if let (Some(d), Some(&s)) = (target.get_mut(i), cce.spec.get(i)) {
                            *d += gain * s;
                        }
                    }
                }
            }
            idx += 1;
        }
        wbase += glen;
    }
    Ok(())
}

fn add_pcm(dst: &mut [f32], src: &[f32], gain: f32) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d += gain * *s;
    }
}

fn plane_index(ids: &[(ElemKind, u8, u8)], kind: ElemKind, tag: u8, part: u8) -> Result<usize> {
    ids.iter()
        .position(|id| id.0 == kind && id.1 == tag && id.2 == part)
        .ok_or(Error::Format("CCE missing target"))
}

/// After-IMDCT independent coupling: `dest += gain[list][0] * cce_pcm`.
pub fn apply_independent_pcm(
    planes: &mut [Vec<f32>],
    ids: &[(ElemKind, u8, u8)],
    cce_pcm: &[f32],
    cce: &CcePayload,
) -> Result<()> {
    let mut list = 0usize;
    for t in &cce.targets {
        let gain = |i: usize| {
            cce.gains
                .get(i)
                .and_then(|g| g.first())
                .copied()
                .unwrap_or(1.0)
        };
        if t.is_cpe {
            let ch_select = (u8::from(t.cc_l) << 1) | u8::from(t.cc_r);
            if ch_select != 1 {
                let i = plane_index(ids, ElemKind::Cpe, t.tag, 0)?;
                add_pcm(&mut planes[i], cce_pcm, gain(list));
                if ch_select != 0 {
                    list += 1;
                }
            }
            if ch_select != 2 {
                let i = plane_index(ids, ElemKind::Cpe, t.tag, 1)?;
                add_pcm(&mut planes[i], cce_pcm, gain(list));
                list += 1;
            }
        } else {
            let i = plane_index(ids, ElemKind::Sce, t.tag, 0)?;
            add_pcm(&mut planes[i], cce_pcm, gain(list));
            list += 1;
        }
    }
    Ok(())
}

fn find_chan(
    pending: &mut [PendingChan],
    kind: ElemKind,
    tag: u8,
    part: u8,
) -> Result<&mut PendingChan> {
    pending
        .iter_mut()
        .find(|p| p.kind == kind && p.tag == tag && p.part == part)
        .ok_or(Error::Format("CCE missing target"))
}

/// Apply one CCE to matching pending channels. `list` walks gain lists.
pub fn apply_cce_to_pending(
    pending: &mut [PendingChan],
    cce: &CcePayload,
    fs_index: u8,
) -> Result<()> {
    let mut list = 0usize;
    for t in &cce.targets {
        if t.is_cpe {
            let ch_select = (u8::from(t.cc_l) << 1) | u8::from(t.cc_r);
            if ch_select != 1 {
                apply_dependent(
                    &mut find_chan(pending, ElemKind::Cpe, t.tag, 0)?.spec,
                    cce,
                    list,
                    fs_index,
                )?;
                if ch_select != 0 {
                    list += 1;
                }
            }
            if ch_select != 2 {
                apply_dependent(
                    &mut find_chan(pending, ElemKind::Cpe, t.tag, 1)?.spec,
                    cce,
                    list,
                    fs_index,
                )?;
                list += 1;
            }
        } else {
            apply_dependent(
                &mut find_chan(pending, ElemKind::Sce, t.tag, 0)?.spec,
                cce,
                list,
                fs_index,
            )?;
            list += 1;
        }
    }
    Ok(())
}
