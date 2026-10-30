//! ER AAC LD (AOT 23) `raw_data_block()` walk — TASK-129.
//!
//! The ER syntax carries **no 3-bit element IDs and no `ID_END`**: element
//! types are implied by `channel_configuration` (FDK reads its
//! `elementsTab` for `AC_ER` streams; libxaac likewise), each element still
//! leads with its 4-bit `element_instance_tag`. After the mapped elements
//! the AU tail holds self-delimiting `extension_payload()`s while more than
//! 7 bits remain, then byte alignment.

use super::bits::BitReader;
use super::channel_map::ElemKind;
use super::decode::StreamDecoder;
use super::error::{Error, Result};
use super::extension_payload::{FILL_DATA_BYTE, FILL_DATA_NIBBLE};
use super::fb_pool::reject_dup;
use super::ics_body::parse_ics_into;

/// Channel elements implied by `channel_configuration` (FDK `elementsTab`,
/// the same layout order [`super::channel_map`] uses for LC).
fn ld_elements(cfg: u8) -> &'static [ElemKind] {
    use ElemKind::{Cpe, Lfe, Sce};
    match cfg {
        1 => &[Sce],
        2 => &[Cpe],
        3 => &[Sce, Cpe],
        4 => &[Sce, Cpe, Sce],
        5 => &[Sce, Cpe, Cpe],
        6 => &[Sce, Cpe, Cpe, Lfe],
        _ => &[Sce, Cpe, Cpe, Cpe, Lfe],
    }
}

impl StreamDecoder {
    /// Parse one LD access unit's channel elements and extension tail.
    pub(crate) fn decode_ld_elements(
        &mut self,
        br: &mut BitReader<'_>,
        fs_index: u8,
        aot: u8,
        channel_configuration: u8,
    ) -> Result<()> {
        if channel_configuration == 0 || self.pce.is_some() {
            return Err(Error::Format("AAC-LD with PCE channel mapping"));
        }
        let multichannel = channel_configuration >= 3;
        for &kind in ld_elements(channel_configuration) {
            let tag = br.read(4)? as u8;
            match kind {
                ElemKind::Sce | ElemKind::Lfe => {
                    if multichannel {
                        reject_dup(&self.elems, kind, tag)?;
                    }
                    let (ics, tns, _gain) = parse_ics_into(
                        br,
                        fs_index,
                        aot,
                        None,
                        self.frame_samples,
                        &mut self.quant,
                        &mut self.spec_l,
                        &mut self.sections_l,
                        &mut self.sf_l,
                    )?;
                    self.finish_sce_pns(&ics, fs_index)?;
                    self.stash_chan(kind, tag, 0, ics, tns, true, true);
                }
                ElemKind::Cpe => {
                    if multichannel {
                        reject_dup(&self.elems, ElemKind::Cpe, tag)?;
                    }
                    let (ics_l, tns_l, _gain_l, ics_r, tns_r, _gain_r) =
                        self.decode_cpe(br, fs_index, aot, tag)?;
                    self.stash_chan(ElemKind::Cpe, tag, 0, ics_l, tns_l, true, true);
                    self.stash_chan(ElemKind::Cpe, tag, 1, ics_r, tns_r, false, true);
                }
                _ => return Err(Error::Format("AAC-LD element map")),
            }
        }
        self.decode_ld_extensions(br)
    }

    /// ER AU tail: `extension_payload()` while more than 7 bits remain.
    fn decode_ld_extensions(&mut self, br: &mut BitReader<'_>) -> Result<()> {
        while br.bits_remaining() > 7 {
            let ty = br.read(4)? as u8;
            match ty {
                // EXT_FILL_DATA: zero nibble, then 0xa5 fill bytes to the end.
                1 => {
                    if br.read(4)? as u8 != FILL_DATA_NIBBLE {
                        return Err(Error::ExtensionPayloadInvalid);
                    }
                    while br.bits_remaining() >= 8 {
                        if br.read(8)? as u8 != FILL_DATA_BYTE {
                            return Err(Error::ExtensionPayloadInvalid);
                        }
                    }
                }
                // EXT_DATA_ELEMENT: version 0 (ANC_DATA), 255-escaped
                // byte length, then that many bytes.
                2 => {
                    if br.read(4)? != 0 {
                        return Err(Error::ExtensionPayloadInvalid);
                    }
                    let mut len = 0u32;
                    loop {
                        let b = br.read(8)?;
                        len = len.saturating_add(b);
                        if b != 255 {
                            break;
                        }
                    }
                    br.skip(len.saturating_mul(8))?;
                }
                // EXT_DYNAMIC_RANGE: parsed and not applied (like the LC FIL path).
                11 => super::extension_payload::skip_dynamic_range(br)?,
                13 | 14 => return Err(Error::UnsupportedExtensionSbr(ty)),
                _ => return Err(Error::UnsupportedExtensionType(ty)),
            }
        }
        Ok(())
    }
}
