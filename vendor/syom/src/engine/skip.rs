//! Skip FIL / DSE so a raw_data_block stays bit-aligned.
//! CCE lives in [`super::cce`]. PCE is parsed in `channel_map::parse_pce`.

use super::bits::BitReader;
use super::error::Result;

pub use super::cce::parse_cce;

/// `fill_element()` count field (bytes of `extension_payload`).
pub fn fill_count(br: &mut BitReader<'_>) -> Result<u32> {
    let mut count = br.read(4)?;
    if count == 15 {
        count += br.read(8)?;
        count = count.saturating_sub(1);
    }
    Ok(count)
}

/// `fill_element()` — 4-bit count, optional 8-bit escape, then `count` bytes.
#[cfg(test)]
pub fn skip_fil(br: &mut BitReader<'_>) -> Result<()> {
    let count = fill_count(br)?;
    br.skip(count.saturating_mul(8))?;
    Ok(())
}

/// `data_stream_element()`.
pub fn skip_dse(br: &mut BitReader<'_>) -> Result<()> {
    let _tag = br.read(4)?;
    let align = br.read_bit()?;
    let mut count = br.read(8)?;
    if count == 255 {
        count += br.read(8)?;
    }
    if align {
        br.byte_align()?;
    }
    br.skip(count.saturating_mul(8))?;
    Ok(())
}
