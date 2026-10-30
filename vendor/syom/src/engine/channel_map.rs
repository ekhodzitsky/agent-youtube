//! Channel element → output plane mapping.
//!
//! PCE wins when present (front/side/back/LFE declaration order; LC, matching
//! `sf_index`, exact tags, ≤6 planes). Else Table 4.1 / lavc order
//! (5.1 = FL FR FC LFE BL BR). Speech mono = mean of non-LFE planes.

use super::bits::BitReader;
use super::error::{Error, Result};
use crate::layout::Channel;

/// Decoded syntactic element kind relevant to channel mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElemKind {
    Sce,
    Cpe,
    Lfe,
    Cce,
}

/// One decoded element; `plane` is its first plane in decode (element) order.
/// A CPE spans `plane` and `plane + 1`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Element {
    pub kind: ElemKind,
    pub tag: u8,
    pub plane: usize,
}

impl Element {
    pub(crate) fn planes(&self) -> usize {
        match self.kind {
            ElemKind::Cpe => 2,
            ElemKind::Sce | ElemKind::Lfe | ElemKind::Cce => 1,
        }
    }
}

/// Parsed `program_config_element()` channel lists, in declaration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PceChannelMap {
    pub tag: u8,
    /// MPEG-2 `object_type` (1 = LC).
    pub object_type: u8,
    pub sf_index: u8,
    /// `(is_cpe, element_instance_tag)`.
    pub front: Vec<(bool, u8)>,
    pub side: Vec<(bool, u8)>,
    pub back: Vec<(bool, u8)>,
    pub lfe: Vec<u8>,
}

/// `program_config_element()` — parsed and applied to channel ordering.
pub(crate) fn parse_pce(br: &mut BitReader<'_>) -> Result<PceChannelMap> {
    parse_pce_checked(br, None)
}

/// ASC-embedded PCE: `object_type` must be LC and `sf_index` must match the core.
pub(crate) fn parse_pce_in_asc(br: &mut BitReader<'_>, sf_index: u8) -> Result<PceChannelMap> {
    parse_pce_checked(br, Some(sf_index))
}

fn parse_pce_checked(br: &mut BitReader<'_>, asc_sf: Option<u8>) -> Result<PceChannelMap> {
    let tag = br.read(4)? as u8;
    let object_type = br.read(2)? as u8;
    let sf_index = br.read(4)? as u8;
    let num_front = br.read(4)? as usize;
    let num_side = br.read(4)? as usize;
    let num_back = br.read(4)? as usize;
    let num_lfe = br.read(2)? as usize;
    let num_assoc = br.read(3)? as usize;
    let num_cc = br.read(4)? as usize;
    if br.read_bit()? {
        let _mono_mixdown = br.read(4)?;
    }
    if br.read_bit()? {
        let _stereo_mixdown = br.read(4)?;
    }
    if br.read_bit()? {
        let _matrix_mixdown_idx = br.read(3)?;
        let _pseudo_surround = br.read_bit()?;
    }
    let front = read_element_list(br, num_front)?;
    let side = read_element_list(br, num_side)?;
    let back = read_element_list(br, num_back)?;
    let mut lfe = Vec::with_capacity(num_lfe);
    for _ in 0..num_lfe {
        lfe.push(br.read(4)? as u8);
    }
    for _ in 0..num_assoc {
        let _assoc_data_tag = br.read(4)?;
    }
    for _ in 0..num_cc {
        let _is_ind_sw = br.read_bit()?;
        let _cc_tag = br.read(4)?;
    }
    br.byte_align()?;
    let comment_bytes = br.read(8)?;
    br.skip(comment_bytes.saturating_mul(8))?;
    let pce = PceChannelMap {
        tag,
        object_type,
        sf_index,
        front,
        side,
        back,
        lfe,
    };
    reject_duplicate_tags(&pce)?;
    if let Some(want_sf) = asc_sf {
        if object_type != 1 {
            return Err(Error::Format("PCE object_type is not LC"));
        }
        if sf_index != want_sf {
            return Err(Error::Format("PCE sf_index does not match ASC"));
        }
        if pce.front.is_empty() && pce.side.is_empty() && pce.back.is_empty() && pce.lfe.is_empty()
        {
            return Err(Error::Format("PCE declares no channel elements"));
        }
    }
    Ok(pce)
}

fn reject_duplicate_tags(pce: &PceChannelMap) -> Result<()> {
    let mut sce = [false; 16];
    let mut cpe = [false; 16];
    let mut lfe = [false; 16];
    for &(is_cpe, tag) in pce.front.iter().chain(&pce.side).chain(&pce.back) {
        let seen = if is_cpe { &mut cpe } else { &mut sce };
        if seen[tag as usize] {
            return Err(Error::Format("PCE duplicate element tag"));
        }
        seen[tag as usize] = true;
    }
    for &tag in &pce.lfe {
        if lfe[tag as usize] {
            return Err(Error::Format("PCE duplicate element tag"));
        }
        lfe[tag as usize] = true;
    }
    Ok(())
}

fn read_element_list(br: &mut BitReader<'_>, n: usize) -> Result<Vec<(bool, u8)>> {
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        let is_cpe = br.read_bit()?;
        let tag = br.read(4)? as u8;
        list.push((is_cpe, tag));
    }
    Ok(list)
}

/// One output plane: which decode-order plane feeds it (`None` → silence)
/// and whether it is LFE (excluded from the speech mono mix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PlaneMap {
    pub src: Option<usize>,
    pub lfe: bool,
    pub label: Channel,
}

pub(super) fn plane(src: Option<usize>, lfe: bool, label: Channel) -> PlaneMap {
    PlaneMap { src, lfe, label }
}

/// PCE declaration order: front, side, back (CPE as L then R), then LFEs.
pub(super) fn pce_map(
    pce: &PceChannelMap,
    elems: &[Element],
    fs_index: u8,
) -> Result<Vec<PlaneMap>> {
    if pce.object_type != 1 {
        return Err(Error::Format("PCE object_type is not LC"));
    }
    if pce.sf_index != fs_index {
        return Err(Error::Format("PCE sf_index does not match stream"));
    }
    // One flag per decoded element. A fixed 16-slot map panicked when a
    // raw_data_block carried a 17th element (legal syntax; the plane
    // ceiling still rejects it below). An empty block keeps the old
    // "extra element" error: there is a phantom slot nothing consumed.
    let mut used = vec![false; elems.len()];
    let mut out = Vec::new();
    pce_list(&mut out, elems, &mut used, &pce.front, List::Front)?;
    pce_list(&mut out, elems, &mut used, &pce.side, List::Side)?;
    pce_list(&mut out, elems, &mut used, &pce.back, List::Back)?;
    for &tag in &pce.lfe {
        push_tagged(
            &mut out,
            elems,
            &mut used,
            ElemKind::Lfe,
            tag,
            &[Channel::Lfe],
        )?;
    }
    if elems.is_empty() || used.iter().any(|&u| !u) {
        return Err(Error::Format("PCE extra channel element"));
    }
    if out.len() > crate::layout::MAX_PLANES {
        return Err(Error::Format("PCE layout exceeds 7.1"));
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum List {
    Front,
    Side,
    Back,
}

fn pce_list(
    out: &mut Vec<PlaneMap>,
    elems: &[Element],
    used: &mut [bool],
    list: &[(bool, u8)],
    where_: List,
) -> Result<()> {
    let mut sce = 0usize;
    let mut cpe = 0usize;
    for &(is_cpe, tag) in list {
        let kind = if is_cpe { ElemKind::Cpe } else { ElemKind::Sce };
        // Second front CPE = ISO "outside front" pair, the config-7 side
        // pair (see `layout::mpeg_channels`).
        let labels: &[Channel] = match (where_, is_cpe, if is_cpe { cpe } else { sce }) {
            (List::Front, true, 0) => &[Channel::FrontLeft, Channel::FrontRight],
            (List::Front, true, 1) | (List::Side, true, _) => {
                &[Channel::SideLeft, Channel::SideRight]
            }
            (List::Front, false, 0) => &[Channel::FrontCenter],
            (List::Back, true, 0) => &[Channel::BackLeft, Channel::BackRight],
            (List::Back, false, 0) => &[Channel::BackCenter],
            _ => &[Channel::Other],
        };
        if is_cpe {
            cpe += 1;
        } else {
            sce += 1;
        }
        push_tagged(out, elems, used, kind, tag, labels)?;
    }
    Ok(())
}

fn push_tagged(
    out: &mut Vec<PlaneMap>,
    elems: &[Element],
    used: &mut [bool],
    kind: ElemKind,
    tag: u8,
    labels: &[Channel],
) -> Result<()> {
    let hit = elems
        .iter()
        .enumerate()
        .find(|(i, e)| !used[*i] && e.kind == kind && e.tag == tag);
    let n = match kind {
        ElemKind::Cpe => 2,
        ElemKind::Sce | ElemKind::Lfe | ElemKind::Cce => 1,
    };
    let Some((i, e)) = hit else {
        return Err(Error::Format("PCE missing channel element"));
    };
    used[i] = true;
    for p in 0..n {
        out.push(plane(
            Some(e.plane + p),
            kind == ElemKind::Lfe,
            labels.get(p).copied().unwrap_or(Channel::Other),
        ));
    }
    Ok(())
}

/// Reorder `planes` per `order`, filling `None` slots with silence.
pub(crate) fn reorder(planes: &mut Vec<Vec<f32>>, order: &[PlaneMap]) {
    let len = planes.first().map_or(0, Vec::len);
    let mut old: [Option<Vec<f32>>; 8] = Default::default();
    for (i, p) in planes.drain(..).enumerate().take(8) {
        old[i] = Some(p);
    }
    for slot in order {
        let plane = slot
            .src
            .and_then(|i| old.get_mut(i).and_then(Option::take))
            .unwrap_or_else(|| vec![0.0; len]);
        planes.push(plane);
    }
}

/// Speech mono: arithmetic mean of the non-LFE planes (all planes when every
/// plane is LFE). Stereo reduces to `0.5·(L+R)`.
pub(crate) fn mono_mix(planes: &[Vec<f32>], order: &[PlaneMap]) -> Vec<f32> {
    let len = planes.first().map_or(0, Vec::len);
    let non_lfe: Vec<usize> = (0..planes.len()).filter(|&i| !order[i].lfe).collect();
    let idx: Vec<usize> = if non_lfe.is_empty() {
        (0..planes.len()).collect()
    } else {
        non_lfe
    };
    let mut mono = vec![0.0f32; len];
    for &i in &idx {
        for (m, &s) in mono.iter_mut().zip(planes[i].iter()) {
            *m += s;
        }
    }
    if !idx.is_empty() {
        let inv = 1.0 / idx.len() as f32;
        for m in &mut mono {
            *m *= inv;
        }
    }
    mono
}

#[path = "channel_map_slots.rs"]
mod slots;
#[cfg(test)]
pub(crate) use slots::map_planes;
pub(crate) use slots::map_planes_into;
