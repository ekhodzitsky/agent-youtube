//! MPEG layout slots for [`super::map_planes`] (line cap / TASK-78).

use super::{ElemKind, Element, PceChannelMap, PlaneMap, pce_map, plane};
use crate::engine::error::Result;
use crate::layout::Channel;

/// Map decode-order element planes to output planes (see parent module docs).
#[cfg(test)]
pub(crate) fn map_planes(
    elems: &[Element],
    pce: Option<&PceChannelMap>,
    channel_configuration: u8,
    fs_index: u8,
) -> Result<Vec<PlaneMap>> {
    let mut out = Vec::new();
    map_planes_into(&mut out, elems, pce, channel_configuration, fs_index)?;
    Ok(out)
}

/// Fill `out` (capacity reused). Used-element flags stay on the stack.
pub(crate) fn map_planes_into(
    out: &mut Vec<PlaneMap>,
    elems: &[Element],
    pce: Option<&PceChannelMap>,
    channel_configuration: u8,
    fs_index: u8,
) -> Result<()> {
    out.clear();
    if let Some(p) = pce {
        *out = pce_map(p, elems, fs_index)?;
        return Ok(());
    }
    let n = elems.len().min(16);
    let mut used = [false; 16];
    default_slots_into(out, channel_configuration, elems, &mut used[..n.max(1)]);
    for (i, e) in elems.iter().enumerate() {
        if i < used.len() && !used[i] {
            for p in 0..e.planes() {
                out.push(plane(
                    Some(e.plane + p),
                    e.kind == ElemKind::Lfe,
                    Channel::Other,
                ));
            }
        }
    }
    Ok(())
}

fn default_slots_into(out: &mut Vec<PlaneMap>, config: u8, elems: &[Element], used: &mut [bool]) {
    use ElemKind::{Cpe, Lfe, Sce};
    const L: usize = 0;
    const R: usize = 1;
    let labels = crate::layout::mpeg_channels(config);
    let slots: &[(ElemKind, usize, usize)] = match config {
        1 => &[(Sce, 0, L)],
        2 => &[(Cpe, 0, L), (Cpe, 0, R)],
        3 => &[(Cpe, 0, L), (Cpe, 0, R), (Sce, 0, L)],
        4 => &[(Cpe, 0, L), (Cpe, 0, R), (Sce, 0, L), (Sce, 1, L)],
        5 => &[
            (Cpe, 0, L),
            (Cpe, 0, R),
            (Sce, 0, L),
            (Cpe, 1, L),
            (Cpe, 1, R),
        ],
        6 => &[
            (Cpe, 0, L),
            (Cpe, 0, R),
            (Sce, 0, L),
            (Lfe, 0, L),
            (Cpe, 1, L),
            (Cpe, 1, R),
        ],
        // 7.1: SCE(C) CPE(front) CPE(outside front) CPE(back) LFE ->
        // FL FR FC LFE BL BR SL SR (lavc order; the outside-front pair
        // is the side pair).
        7 => &[
            (Cpe, 0, L),
            (Cpe, 0, R),
            (Sce, 0, L),
            (Lfe, 0, L),
            (Cpe, 2, L),
            (Cpe, 2, R),
            (Cpe, 1, L),
            (Cpe, 1, R),
        ],
        _ => {
            identity_into(out, elems, used);
            return;
        }
    };
    for (i, &(kind, occ, part)) in slots.iter().enumerate() {
        let label = labels.get(i).copied().unwrap_or(Channel::Other);
        let hit = elems
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == kind)
            .nth(occ);
        match hit {
            Some((ei, e)) => {
                if ei < used.len() {
                    used[ei] = true;
                }
                out.push(plane(Some(e.plane + part), kind == Lfe, label));
            }
            None => out.push(plane(None, kind == Lfe, label)),
        }
    }
}

fn identity_into(out: &mut Vec<PlaneMap>, elems: &[Element], used: &mut [bool]) {
    for (i, e) in elems.iter().enumerate() {
        if i < used.len() {
            used[i] = true;
        }
        for p in 0..e.planes() {
            out.push(plane(
                Some(e.plane + p),
                e.kind == ElemKind::Lfe,
                Channel::Other,
            ));
        }
    }
}
