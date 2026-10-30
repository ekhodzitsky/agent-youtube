//! SBR bitstream writer (TASK-88): `sbr_extension_data()` for an SCE or
//! an uncoupled CPE from [`SbrFrameParams`], the `EXT_SBR_DATA`
//! `extension_payload()` bytes, and the `fill_element()` wrapper.
//! Mirrors the in-tree parsers field for field (Tables 4.63–4.73);
//! Huffman codes are the inverse lookup of the ISO tables in
//! `sbr_huffman`. Unrepresentable parameters are errors, never
//! silently clamped. No CRC: `EXT_SBR_DATA_CRC` is not written (the
//! in-tree and libavcodec decoders only skip `bs_sbr_crc_bits`).

use super::bits::BitWriter;
use super::enc_ps_bits::PsBits;
use super::enc_sbr_est::SbrFrameParams;
use super::error::{Error, Result};
use super::sbr_envelope::{SbrEnvelopeData, SbrNoiseData};
use super::sbr_freq_bands::HiLoTables;
use super::sbr_grid::{FrameClass, SBR_MAX_NUM_ENV, SbrDtdf, SbrGrid, SbrInvf, ptr_bits};
use super::sbr_header::{
    DEFAULT_ALTER_SCALE, DEFAULT_FREQ_SCALE, DEFAULT_INTERPOL_FREQ, DEFAULT_LIMITER_BANDS,
    DEFAULT_LIMITER_GAINS, DEFAULT_NOISE_BANDS, DEFAULT_SMOOTHING_MODE, SbrHeader,
};
use super::sbr_huffman::{SbrHuffCodebook, SbrHuffContext, env_tables, noise_tables};

/// `EXT_SBR_DATA` extension type (ISO/IEC 13818-7 Table 40).
pub(crate) const EXT_SBR_DATA: u32 = 0b1101;
/// `ID_FIL`.
const ID_FIL: u32 = 6;
/// Largest `fill_element()` payload: `count 15 + esc_count 255 − 1`.
pub(crate) const FILL_MAX_BYTES: usize = 269;

fn field(w: &mut BitWriter, value: u32, bits: u32) -> Result<()> {
    if bits < 32 && value >> bits != 0 {
        return Err(Error::SbrGridInvalid);
    }
    w.write(value, bits);
    Ok(())
}

/// `sbr_header()` (Table 4.63). Extra blocks are written only when
/// their flag is set; a clear flag with non-default values is
/// unrepresentable.
pub(crate) fn write_header(w: &mut BitWriter, h: &SbrHeader) -> Result<()> {
    let extra_1_default = h.freq_scale == DEFAULT_FREQ_SCALE
        && h.alter_scale == DEFAULT_ALTER_SCALE
        && h.noise_bands == DEFAULT_NOISE_BANDS;
    let extra_2_default = h.limiter_bands == DEFAULT_LIMITER_BANDS
        && h.limiter_gains == DEFAULT_LIMITER_GAINS
        && h.interpol_freq == DEFAULT_INTERPOL_FREQ
        && h.smoothing_mode == DEFAULT_SMOOTHING_MODE;
    if (!h.header_extra_1 && !extra_1_default) || (!h.header_extra_2 && !extra_2_default) {
        return Err(Error::SbrFreqBandInvalid);
    }
    w.write_bit(h.amp_res);
    field(w, u32::from(h.start_freq), 4)?;
    field(w, u32::from(h.stop_freq), 4)?;
    field(w, u32::from(h.xover_band), 3)?;
    field(w, u32::from(h.reserved), 2)?;
    w.write_bit(h.header_extra_1);
    w.write_bit(h.header_extra_2);
    if h.header_extra_1 {
        field(w, u32::from(h.freq_scale), 2)?;
        w.write_bit(h.alter_scale);
        field(w, u32::from(h.noise_bands), 2)?;
    }
    if h.header_extra_2 {
        field(w, u32::from(h.limiter_bands), 2)?;
        field(w, u32::from(h.limiter_gains), 2)?;
        w.write_bit(h.interpol_freq);
        w.write_bit(h.smoothing_mode);
    }
    Ok(())
}

fn write_borders(w: &mut BitWriter, rel: &[u8]) -> Result<()> {
    rel.iter().try_for_each(|&b| field(w, u32::from(b), 2))
}

/// `sbr_grid()` (Table 4.69), all four frame classes.
pub(crate) fn write_grid(w: &mut BitWriter, g: &SbrGrid) -> Result<()> {
    let le = g.num_env;
    if le == 0 || le > SBR_MAX_NUM_ENV || g.freq_res.len() != le {
        return Err(Error::SbrGridInvalid);
    }
    w.write(g.frame_class.to_bits(), 2);
    match g.frame_class {
        FrameClass::FixFix => {
            if !le.is_power_of_two() || le > 4 || g.freq_res.iter().any(|&f| f != g.freq_res[0]) {
                return Err(Error::SbrGridInvalid);
            }
            w.write(le.trailing_zeros(), 2);
            w.write_bit(g.freq_res[0]);
        }
        FrameClass::FixVar => {
            if g.rel_bord_1.len() + 1 != le {
                return Err(Error::SbrGridInvalid);
            }
            field(w, u32::from(g.var_bord_1), 2)?;
            w.write((le - 1) as u32, 2);
            write_borders(w, &g.rel_bord_1)?;
            field(w, g.pointer, ptr_bits(le))?;
            for &f in g.freq_res.iter().rev() {
                w.write_bit(f);
            }
        }
        FrameClass::VarFix => {
            if g.rel_bord_0.len() + 1 != le {
                return Err(Error::SbrGridInvalid);
            }
            field(w, u32::from(g.var_bord_0), 2)?;
            w.write((le - 1) as u32, 2);
            write_borders(w, &g.rel_bord_0)?;
            field(w, g.pointer, ptr_bits(le))?;
            for &f in &g.freq_res {
                w.write_bit(f);
            }
        }
        FrameClass::VarVar => {
            if g.rel_bord_0.len() + g.rel_bord_1.len() + 1 != le {
                return Err(Error::SbrGridInvalid);
            }
            field(w, u32::from(g.var_bord_0), 2)?;
            field(w, u32::from(g.var_bord_1), 2)?;
            field(w, g.rel_bord_0.len() as u32, 2)?;
            field(w, g.rel_bord_1.len() as u32, 2)?;
            write_borders(w, &g.rel_bord_0)?;
            write_borders(w, &g.rel_bord_1)?;
            field(w, g.pointer, ptr_bits(le))?;
            for &f in &g.freq_res {
                w.write_bit(f);
            }
        }
    }
    Ok(())
}

/// `sbr_dtdf()` (Table 4.70).
fn write_dtdf(w: &mut BitWriter, d: &SbrDtdf, g: &SbrGrid) -> Result<()> {
    if d.df_env.len() != g.num_env || d.df_noise.len() != g.num_noise {
        return Err(Error::SbrGridInvalid);
    }
    d.df_env
        .iter()
        .chain(&d.df_noise)
        .for_each(|&b| w.write_bit(b));
    Ok(())
}

/// `sbr_invf()` (Table 4.71).
fn write_invf(w: &mut BitWriter, inv: &SbrInvf, n_q: usize) -> Result<()> {
    if inv.invf_mode.len() != n_q {
        return Err(Error::SbrGridInvalid);
    }
    inv.invf_mode
        .iter()
        .try_for_each(|&m| field(w, u32::from(m), 2))
}

/// One Huffman codeword: inverse lookup of `(length, code)` at
/// `delta + LAV`; a delta outside the table is unrepresentable.
fn write_code(w: &mut BitWriter, book: SbrHuffCodebook, delta: i32) -> Result<()> {
    let idx = usize::try_from(delta + book.1).map_err(|_| Error::SbrHuffInvalid)?;
    let &(len, code) = book.0.get(idx).ok_or(Error::SbrHuffInvalid)?;
    w.write(code, u32::from(len));
    Ok(())
}

/// One coded row: absolute start value (`start_bits`, frequency
/// direction) followed by frequency deltas, or time deltas throughout.
fn write_row(
    w: &mut BitWriter,
    row: &[i32],
    time: bool,
    start_bits: u32,
    books: (SbrHuffCodebook, SbrHuffCodebook),
) -> Result<()> {
    let (first, rest) = row.split_first().ok_or(Error::SbrGridInvalid)?;
    if time {
        write_code(w, books.0, *first)?;
    } else {
        let v = u32::try_from(*first).map_err(|_| Error::SbrGridInvalid)?;
        field(w, v, start_bits)?;
    }
    let book = if time { books.0 } else { books.1 };
    rest.iter().try_for_each(|&d| write_code(w, book, d))
}

/// `sbr_envelope()` (Table 4.72) for an uncoupled channel.
fn write_envelope(
    w: &mut BitWriter,
    env: &SbrEnvelopeData,
    g: &SbrGrid,
    d: &SbrDtdf,
    bands: &HiLoTables,
    amp_res: bool,
) -> Result<()> {
    let ctx = SbrHuffContext {
        coupling: false,
        ch: false,
        amp_res,
    };
    if env.data.len() != g.num_env {
        return Err(Error::SbrGridInvalid);
    }
    for (l, row) in env.data.iter().enumerate() {
        let n = if g.freq_res[l] {
            bands.n_high()
        } else {
            bands.n_low()
        };
        if row.len() != n {
            return Err(Error::SbrGridInvalid);
        }
        let start_bits = if amp_res { 6 } else { 7 };
        write_row(w, row, d.df_env[l], start_bits, env_tables(ctx))?;
    }
    Ok(())
}

/// `sbr_noise()` (Table 4.73) for an uncoupled channel.
fn write_noise(
    w: &mut BitWriter,
    noise: &SbrNoiseData,
    g: &SbrGrid,
    d: &SbrDtdf,
    n_q: usize,
) -> Result<()> {
    let ctx = SbrHuffContext {
        coupling: false,
        ch: false,
        amp_res: false,
    };
    if noise.data.len() != g.num_noise {
        return Err(Error::SbrGridInvalid);
    }
    for (l, row) in noise.data.iter().enumerate() {
        if row.len() != n_q {
            return Err(Error::SbrGridInvalid);
        }
        write_row(w, row, d.df_noise[l], 5, noise_tables(ctx))?;
    }
    Ok(())
}

/// `bs_add_harmonic_flag` + `sbr_sinusoidal_coding()`.
fn write_harmonics(w: &mut BitWriter, add: &[bool], n_high: usize) -> Result<()> {
    w.write_bit(!add.is_empty());
    if !add.is_empty() && add.len() != n_high {
        return Err(Error::SbrGridInvalid);
    }
    add.iter().for_each(|&b| w.write_bit(b));
    Ok(())
}

/// `sbr_single_channel_element()` (Table 4.65) or an uncoupled
/// `sbr_channel_pair_element()` (Table 4.66), no extended data.
/// `amp_res` is the header value; a single-envelope FIXFIX grid forces
/// 1.5 dB per channel.
#[cfg(test)]
pub(crate) fn write_sbr_data(
    w: &mut BitWriter,
    amp_res: bool,
    bands: &HiLoTables,
    channels: &[&SbrFrameParams],
) -> Result<()> {
    write_sbr_data_ps(w, amp_res, bands, channels, None)
}

/// [`write_sbr_data`] with an optional PS payload in `bs_extended_data`
/// (HE v2: a mono SCE element only).
pub(crate) fn write_sbr_data_ps(
    w: &mut BitWriter,
    amp_res: bool,
    bands: &HiLoTables,
    channels: &[&SbrFrameParams],
    ps: Option<&PsBits>,
) -> Result<()> {
    let n_q = bands.n_q();
    w.write_bit(false); // bs_data_extra
    let eff = |c: &SbrFrameParams| amp_res && !c.grid.amp_res_override;
    match channels {
        [c] => {
            write_grid(w, &c.grid)?;
            write_dtdf(w, &c.dtdf, &c.grid)?;
            write_invf(w, &c.invf, n_q)?;
            write_envelope(w, &c.envelope, &c.grid, &c.dtdf, bands, eff(c))?;
            write_noise(w, &c.noise, &c.grid, &c.dtdf, n_q)?;
            write_harmonics(w, &c.add_harmonic, bands.n_high())?;
        }
        [a, b] => {
            w.write_bit(false); // bs_coupling
            write_grid(w, &a.grid)?;
            write_grid(w, &b.grid)?;
            write_dtdf(w, &a.dtdf, &a.grid)?;
            write_dtdf(w, &b.dtdf, &b.grid)?;
            write_invf(w, &a.invf, n_q)?;
            write_invf(w, &b.invf, n_q)?;
            write_envelope(w, &a.envelope, &a.grid, &a.dtdf, bands, eff(a))?;
            write_envelope(w, &b.envelope, &b.grid, &b.dtdf, bands, eff(b))?;
            write_noise(w, &a.noise, &a.grid, &a.dtdf, n_q)?;
            write_noise(w, &b.noise, &b.grid, &b.dtdf, n_q)?;
            write_harmonics(w, &a.add_harmonic, bands.n_high())?;
            write_harmonics(w, &b.add_harmonic, bands.n_high())?;
        }
        _ => return Err(Error::SbrGridInvalid),
    }
    match ps {
        Some(_) if channels.len() != 1 => Err(Error::SbrGridInvalid),
        Some(ps) => super::enc_ps_bits::write_extended_data(w, ps),
        None => {
            w.write_bit(false); // bs_extended_data
            Ok(())
        }
    }
}

/// `sbr_extension_data()` after the `extension_type` nibble, no CRC:
/// `bs_header_flag`, the header when `transmit_header`, then the
/// element. Returns `num_sbr_bits`.
pub(crate) fn write_sbr_extension(
    w: &mut BitWriter,
    header: &SbrHeader,
    transmit_header: bool,
    bands: &HiLoTables,
    channels: &[&SbrFrameParams],
    ps: Option<&PsBits>,
) -> Result<u64> {
    let start = w.bit_len();
    w.write_bit(transmit_header);
    if transmit_header {
        write_header(w, header)?;
    }
    write_sbr_data_ps(w, header.amp_res, bands, channels, ps)?;
    Ok(w.bit_len() - start)
}

/// Complete `extension_payload(cnt)` bytes for an `EXT_SBR_DATA` FIL:
/// type nibble, the extension, then zero `bs_fill_bits` to the byte
/// boundary (`cnt` = the returned length, ≤ [`FILL_MAX_BYTES`]).
pub(crate) fn sbr_extension_payload(
    header: &SbrHeader,
    transmit_header: bool,
    bands: &HiLoTables,
    channels: &[&SbrFrameParams],
    ps: Option<&PsBits>,
) -> Result<Vec<u8>> {
    let mut w = BitWriter::new();
    w.write(EXT_SBR_DATA, 4);
    write_sbr_extension(&mut w, header, transmit_header, bands, channels, ps)?;
    let bytes = w.finish();
    if bytes.len() > FILL_MAX_BYTES {
        return Err(Error::SbrGridInvalid);
    }
    Ok(bytes)
}

/// `fill_element()`: `ID_FIL`, the 4-bit count with its 8-bit escape,
/// then the payload bytes (the AU need not be byte-aligned here).
pub(crate) fn write_fill_element(w: &mut BitWriter, payload: &[u8]) -> Result<()> {
    let cnt = payload.len();
    if cnt == 0 || cnt > FILL_MAX_BYTES {
        return Err(Error::SbrGridInvalid);
    }
    w.write(ID_FIL, 3);
    if cnt < 15 {
        w.write(cnt as u32, 4);
    } else {
        w.write(15, 4);
        w.write((cnt - 14) as u32, 8);
    }
    payload.iter().for_each(|&b| w.write(u32::from(b), 8));
    Ok(())
}

#[cfg(test)]
#[path = "enc_sbr_bits_tests.rs"]
pub(crate) mod enc_sbr_bits_tests;
