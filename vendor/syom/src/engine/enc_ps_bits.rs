//! `ps_data()` writer for the HE v2 encoder (TASK-92) and its carriage in
//! the SBR `bs_extended_data` block (ISO/IEC 14496-3 Tables 8.1 / 4.65).
//!
//! Supported syntax: 20-band IID (`iid_mode` 1 coarse, or 4 fine), 20-band
//! ICC (`icc_mode` 1), no extension layer, `frame_class` 0 with one
//! envelope (or zero envelopes = "hold"). The product path is mode 1.
//! Each parameter row is coded frequency- or time-differentially,
//! whichever is shorter; a frame that carries the header is always
//! frequency-differential so a decoder can join there.

use super::bits::BitWriter;
use super::enc_ps_est::{PS_BANDS, PsFrameParams};
use super::error::{Error, Result};
use super::ps_huffman::{
    HUFF_ICC_DF, HUFF_ICC_DT, HUFF_IID_DF, HUFF_IID_DT, HUFF_IID_FINE_DF, HUFF_IID_FINE_DT,
};
use super::sbr_element::EXTENSION_ID_PS;

/// `bs_extension_size` ceiling: 4-bit count 15 plus the 8-bit escape.
pub(crate) const PS_EXT_MAX_BYTES: usize = 15 + 255;

/// One serialized `ps_data()` (not byte-aligned: `bits` are valid).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PsBits {
    pub bytes: Vec<u8>,
    pub bits: usize,
}

/// Differential-coding state across frames.
#[derive(Debug, Clone, Default)]
pub(crate) struct PsWriter {
    prev: Option<PsFrameParams>,
    /// `iid_mode` 4 (Table 8.26) instead of 1 (Table 8.25).
    fine: bool,
    /// Sum of `ps_data()` bit lengths.
    ps_bits: u64,
    /// Sum of on-wire extended-data blocks (size field + byte payload).
    ext_bits: u64,
}

impl PsWriter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 20-band fine IID (`iid_mode` 4). ICC stays mode 1.
    pub(crate) fn with_fine_iid(mut self) -> Self {
        self.fine = true;
        self
    }

    pub(crate) fn ps_bits(&self) -> u64 {
        self.ps_bits
    }

    pub(crate) fn ext_bits(&self) -> u64 {
        self.ext_bits
    }

    pub(crate) fn reset(&mut self) {
        self.prev = None;
        self.ps_bits = 0;
        self.ext_bits = 0;
    }

    /// Serialize one frame. `header` re-sends the configuration (and
    /// forces frequency-differential rows). `params = None` writes a
    /// zero-envelope frame: the decoder holds the previous parameters.
    pub(crate) fn frame(&mut self, params: Option<&PsFrameParams>, header: bool) -> Result<PsBits> {
        let mut w = BitWriter::new();
        w.write_bit(header); // enable_ps_header
        if header {
            w.write_bit(true); // enable_iid
            w.write(if self.fine { 4 } else { 1 }, 3);
            w.write_bit(true); // enable_icc
            w.write(1, 3); // icc_mode 1: 20 bands
            w.write_bit(false); // enable_ext
        } else if self.prev.is_none() && params.is_some() {
            // Parameters before any configuration cannot be decoded.
            return Err(Error::PsDataInvalid);
        }
        w.write_bit(false); // frame_class: FIX
        let Some(p) = params else {
            w.write(0, 2); // num_env_idx 0: no envelopes
            return Ok(self.account(w));
        };
        let bound: i8 = if self.fine { 15 } else { 7 };
        if p.iid.iter().any(|i| !(-bound..=bound).contains(i)) || p.icc.iter().any(|&c| c > 7) {
            return Err(Error::PsDataInvalid);
        }
        w.write(1, 2); // num_env_idx 1: one envelope
        let prev = if header { None } else { self.prev.as_ref() };
        let iid: [i32; PS_BANDS] = p.iid.map(i32::from);
        let icc: [i32; PS_BANDS] = p.icc.map(i32::from);
        let (df, dt, lav) = if self.fine {
            (&HUFF_IID_FINE_DF[..], &HUFF_IID_FINE_DT[..], 30)
        } else {
            (&HUFF_IID_DF[..], &HUFF_IID_DT[..], 14)
        };
        write_row(
            &mut w,
            &iid,
            prev.map(|q| q.iid.map(i32::from)),
            df,
            dt,
            lav,
        )?;
        write_row(
            &mut w,
            &icc,
            prev.map(|q| q.icc.map(i32::from)),
            &HUFF_ICC_DF,
            &HUFF_ICC_DT,
            7,
        )?;
        self.prev = Some(*p);
        Ok(self.account(w))
    }

    /// `ps_data` bits plus the extended-data block the rate loop pays for
    /// (4- or 12-bit size, then `cnt` payload bytes). The leading
    /// `bs_extended_data` flag is present either way, so it is not counted.
    fn account(&mut self, w: BitWriter) -> PsBits {
        let bits = done(w);
        self.ps_bits += bits.bits as u64;
        let cnt = (2 + bits.bits).div_ceil(8);
        let size_field: u64 = if cnt < 15 { 4 } else { 12 };
        self.ext_bits += size_field + 8 * cnt as u64;
        bits
    }
}

fn done(w: BitWriter) -> PsBits {
    let bits = w.bit_len() as usize;
    PsBits {
        bytes: w.finish(),
        bits,
    }
}

/// `dt` flag + 20 Huffman-coded deltas, the shorter of DF and DT.
fn write_row(
    w: &mut BitWriter,
    now: &[i32; PS_BANDS],
    prev: Option<[i32; PS_BANDS]>,
    df: &[(u8, u32)],
    dt: &[(u8, u32)],
    lav: i32,
) -> Result<()> {
    let mut d_f = [0i32; PS_BANDS];
    for b in 0..PS_BANDS {
        d_f[b] = now[b] - if b == 0 { 0 } else { now[b - 1] };
    }
    let cost = |table: &[(u8, u32)], d: &[i32; PS_BANDS]| -> Result<u32> {
        d.iter().try_fold(0u32, |acc, &v| {
            let (len, _) = table.get((v + lav) as usize).ok_or(Error::PsDataInvalid)?;
            Ok(acc + u32::from(*len))
        })
    };
    let mut pick = (false, d_f, df);
    if let Some(prev) = prev {
        let mut d_t = [0i32; PS_BANDS];
        for b in 0..PS_BANDS {
            d_t[b] = now[b] - prev[b];
        }
        if cost(dt, &d_t)? < cost(df, &d_f)? {
            pick = (true, d_t, dt);
        }
    }
    cost(pick.2, &pick.1)?; // range check before any bit is written
    w.write_bit(pick.0);
    for &v in &pick.1 {
        let (len, code) = pick.2[(v + lav) as usize];
        w.write(code, u32::from(len));
    }
    Ok(())
}

/// The `bs_extended_data` block carrying one PS payload:
/// `bs_extension_size` (+ escape), `bs_extension_id = 2`, `ps_data()`,
/// zero fill to the declared byte count.
pub(crate) fn write_extended_data(w: &mut BitWriter, ps: &PsBits) -> Result<()> {
    let cnt = (2 + ps.bits).div_ceil(8);
    if ps.bits == 0 || cnt > PS_EXT_MAX_BYTES || ps.bytes.len() * 8 < ps.bits {
        return Err(Error::PsDataInvalid);
    }
    w.write_bit(true); // bs_extended_data
    if cnt < 15 {
        w.write(cnt as u32, 4);
    } else {
        w.write(15, 4);
        w.write((cnt - 15) as u32, 8); // bs_esc_count
    }
    w.write(u32::from(EXTENSION_ID_PS), 2);
    let mut left = ps.bits;
    for &byte in &ps.bytes {
        let n = left.min(8);
        if n == 0 {
            break;
        }
        w.write(u32::from(byte) >> (8 - n), n as u32);
        left -= n;
    }
    let fill = cnt * 8 - 2 - ps.bits;
    if fill > 0 {
        w.write(0, fill as u32);
    }
    Ok(())
}

#[cfg(test)]
#[path = "enc_ps_bits_tests.rs"]
mod enc_ps_bits_tests;
