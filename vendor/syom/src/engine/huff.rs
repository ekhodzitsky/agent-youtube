//! Spectral Huffman decode — ISO/IEC 14496-3 §4.6.3 + Tables 4.A.2–4.A.12.

use super::bits::BitReader;
use super::error::{Error, Result};
use super::{huff_esc as esc, huff_pair as pair, huff_quad as quad};
use std::sync::LazyLock;

/// One codebook symbol: integer tuple plus `invquant` of those integers.
/// Unsigned books store positive magnitudes; signs are applied at read time.
/// 32-byte alignment keeps the pair on one cache line.
#[repr(C, align(32))]
#[derive(Clone, Copy)]
struct Sym {
    q: [i32; 4],
    mag: [f32; 4],
}

/// Binary prefix tree. `0` in `left`/`right` means no child (root is node 0).
struct HuffTree {
    left: Vec<u16>,
    right: Vec<u16>,
    leaf: Vec<i16>,
    /// 12-bit prefix → (symbol, nbits). `nbits == 0` ⇒ code longer than 12.
    lut_sym: [i16; 4096],
    lut_len: [u8; 4096],
    syms: Vec<Sym>,
    width: u8,
    sign_bits: bool,
    escape: bool,
}

impl HuffTree {
    fn from_tables(
        len: &[u8],
        code: &[u16],
        expand: fn(usize) -> [i32; 4],
        width: u8,
        sign_bits: bool,
        escape: bool,
    ) -> Self {
        let mut left = vec![0u16];
        let mut right = vec![0u16];
        let mut leaf = vec![-1i16];
        let mut lut_sym = [-1i16; 4096];
        let mut lut_len = [0u8; 4096];
        for (idx, (&l, &c)) in len.iter().zip(code.iter()).enumerate() {
            let mut node = 0u16;
            for b in (0..l).rev() {
                let bit = (c >> b) & 1;
                let existing = if bit == 0 {
                    left[node as usize]
                } else {
                    right[node as usize]
                };
                if existing == 0 {
                    let id = left.len() as u16;
                    left.push(0);
                    right.push(0);
                    leaf.push(-1);
                    if bit == 0 {
                        left[node as usize] = id;
                    } else {
                        right[node as usize] = id;
                    }
                    node = id;
                } else {
                    node = existing;
                }
            }
            leaf[node as usize] = idx as i16;
            if l > 0 && l <= 12 {
                let shift = 12 - l;
                let base = (c as usize) << shift;
                for extra in 0..(1usize << shift) {
                    lut_sym[base + extra] = idx as i16;
                    lut_len[base + extra] = l;
                }
            }
        }
        let mut syms = Vec::with_capacity(len.len());
        for i in 0..len.len() {
            let q = expand(i);
            syms.push(Sym {
                mag: [
                    super::spectrum::invquant(q[0]),
                    super::spectrum::invquant(q[1]),
                    super::spectrum::invquant(q[2]),
                    super::spectrum::invquant(q[3]),
                ],
                q,
            });
        }
        Self {
            left,
            right,
            leaf,
            lut_sym,
            lut_len,
            syms,
            width,
            sign_bits,
            escape,
        }
    }

    #[inline(always)]
    fn decode(&self, br: &mut BitReader<'_>) -> Result<usize> {
        if let Some(p) = br.try_peek12() {
            let p = p as usize;
            let n = self.lut_len[p];
            if n != 0 {
                br.eat(u32::from(n));
                return Ok(self.lut_sym[p] as usize);
            }
        }
        let mut node = 0u16;
        loop {
            let bit = br.read_bit()?;
            let next = if bit {
                self.right[node as usize]
            } else {
                self.left[node as usize]
            };
            if next == 0 {
                return Err(Error::HuffmanInvalid);
            }
            node = next;
            let sym = self.leaf[node as usize];
            if sym >= 0 {
                return Ok(sym as usize);
            }
        }
    }

    #[inline(always)]
    fn load(&self, idx: usize) -> Sym {
        debug_assert!(idx < self.syms.len());
        // SAFETY: `decode` returns a symbol index inserted by `from_tables`.
        unsafe { *self.syms.get_unchecked(idx) }
    }
}

fn expand_signed_quad(idx: usize) -> [i32; 4] {
    [
        (idx / 27) as i32 - 1,
        ((idx / 9) % 3) as i32 - 1,
        ((idx / 3) % 3) as i32 - 1,
        (idx % 3) as i32 - 1,
    ]
}

fn expand_unsigned_quad(idx: usize) -> [i32; 4] {
    [
        (idx / 27) as i32,
        ((idx / 9) % 3) as i32,
        ((idx / 3) % 3) as i32,
        (idx % 3) as i32,
    ]
}

fn expand_signed_pair(idx: usize) -> [i32; 4] {
    const DIM: usize = 9;
    const LAV: i32 = 4;
    [(idx / DIM) as i32 - LAV, (idx % DIM) as i32 - LAV, 0, 0]
}

fn expand_upair<const DIM: usize>(idx: usize) -> [i32; 4] {
    [(idx / DIM) as i32, (idx % DIM) as i32, 0, 0]
}

static BOOKS: LazyLock<[HuffTree; 11]> = LazyLock::new(|| {
    [
        HuffTree::from_tables(
            &quad::H1_LEN,
            &quad::H1_CODE,
            expand_signed_quad,
            4,
            false,
            false,
        ),
        HuffTree::from_tables(
            &quad::H2_LEN,
            &quad::H2_CODE,
            expand_signed_quad,
            4,
            false,
            false,
        ),
        HuffTree::from_tables(
            &quad::H3_LEN,
            &quad::H3_CODE,
            expand_unsigned_quad,
            4,
            true,
            false,
        ),
        HuffTree::from_tables(
            &quad::H4_LEN,
            &quad::H4_CODE,
            expand_unsigned_quad,
            4,
            true,
            false,
        ),
        HuffTree::from_tables(
            &pair::H5_LEN,
            &pair::H5_CODE,
            expand_signed_pair,
            2,
            false,
            false,
        ),
        HuffTree::from_tables(
            &pair::H6_LEN,
            &pair::H6_CODE,
            expand_signed_pair,
            2,
            false,
            false,
        ),
        HuffTree::from_tables(
            &pair::H7_LEN,
            &pair::H7_CODE,
            expand_upair::<8>,
            2,
            true,
            false,
        ),
        HuffTree::from_tables(
            &pair::H8_LEN,
            &pair::H8_CODE,
            expand_upair::<8>,
            2,
            true,
            false,
        ),
        HuffTree::from_tables(
            &pair::H9_LEN,
            &pair::H9_CODE,
            expand_upair::<13>,
            2,
            true,
            false,
        ),
        HuffTree::from_tables(
            &pair::H10_LEN,
            &pair::H10_CODE,
            expand_upair::<13>,
            2,
            true,
            false,
        ),
        HuffTree::from_tables(
            &esc::H11_LEN,
            &esc::H11_CODE,
            expand_upair::<17>,
            2,
            true,
            true,
        ),
    ]
});

fn book(n: u8) -> Result<&'static HuffTree> {
    let i = usize::from(n).wrapping_sub(1);
    BOOKS.get(i).ok_or(Error::InvalidCodebook(n))
}

/// Codebook opened once per band. The no-pulse path writes floats from here.
pub(crate) struct SpectralBook {
    tree: &'static HuffTree,
}

impl SpectralBook {
    pub(crate) fn open(cb: u8) -> Result<Self> {
        Ok(Self { tree: book(cb)? })
    }

    /// One tuple as signed `invquant` values. `n` is 2 or 4. Sign and escape
    /// bits of a tuple that spills past the band are still consumed.
    #[inline(always)]
    pub(crate) fn pull(&self, br: &mut BitReader<'_>) -> Result<(usize, [f32; 4])> {
        let (_, mag) = read_tuple(self.tree, br)?;
        Ok((usize::from(self.tree.width), mag))
    }

    pub(crate) fn fill(&self, br: &mut BitReader<'_>, gain: f32, dst: &mut [f32]) -> Result<()> {
        let plain = !self.tree.sign_bits && !self.tree.escape;
        if self.tree.width == 4 {
            if plain {
                fill_width::<4, true>(self.tree, br, gain, dst)
            } else {
                fill_width::<4, false>(self.tree, br, gain, dst)
            }
        } else if plain {
            fill_width::<2, true>(self.tree, br, gain, dst)
        } else {
            fill_width::<2, false>(self.tree, br, gain, dst)
        }
    }
}

#[inline(always)]
fn read_tuple(tree: &HuffTree, br: &mut BitReader<'_>) -> Result<([i32; 4], [f32; 4])> {
    let mut sym = tree.load(tree.decode(br)?);
    let n = usize::from(tree.width);
    if tree.sign_bits {
        for i in 0..n {
            if sym.q[i] != 0 && br.read_bit()? {
                sym.q[i] = -sym.q[i];
                sym.mag[i] = -sym.mag[i];
            }
        }
    }
    if tree.escape {
        for i in 0..n {
            if sym.q[i].abs() == 16 {
                decode_esc(br, &mut sym.q[i])?;
                sym.mag[i] = super::spectrum::invquant(sym.q[i]);
            }
        }
    }
    Ok((sym.q, sym.mag))
}

#[inline(always)]
fn next_mag<const PLAIN: bool>(tree: &HuffTree, br: &mut BitReader<'_>) -> Result<[f32; 4]> {
    if PLAIN {
        Ok(tree.load(tree.decode(br)?).mag)
    } else {
        Ok(read_tuple(tree, br)?.1)
    }
}

fn fill_width<const W: usize, const PLAIN: bool>(
    tree: &HuffTree,
    br: &mut BitReader<'_>,
    gain: f32,
    dst: &mut [f32],
) -> Result<()> {
    let mut i = 0usize;
    while i + W <= dst.len() {
        let mag = next_mag::<PLAIN>(tree, br)?;
        for (slot, &m) in dst[i..i + W].iter_mut().zip(mag[..W].iter()) {
            *slot = m * gain;
        }
        i += W;
    }
    if i < dst.len() {
        let mag = next_mag::<PLAIN>(tree, br)?;
        for (slot, &m) in dst[i..].iter_mut().zip(mag.iter()) {
            *slot = m * gain;
        }
    }
    Ok(())
}

/// Decode one n-tuple from codebook `cb` (1..=11) into `out` (length 2 or 4).
/// Returns the number of coefficients written.
pub fn decode_tuple(br: &mut BitReader<'_>, cb: u8, out: &mut [i32]) -> Result<usize> {
    let tree = book(cb)?;
    let (q, _) = read_tuple(tree, br)?;
    let n = usize::from(tree.width);
    out[..n].copy_from_slice(&q[..n]);
    Ok(n)
}

/// §4.6.3.3 escape_sequence: book 11 magnitude 16 is a flag.
fn decode_esc(br: &mut BitReader<'_>, v: &mut i32) -> Result<()> {
    if v.abs() != 16 {
        return Ok(());
    }
    let neg = *v < 0;
    let mut n = 4u32;
    while br.read_bit()? {
        n += 1;
        if n > 21 {
            return Err(Error::HuffmanInvalid);
        }
    }
    let off = br.read(n)?;
    let mag = (1i32 << n) + off as i32;
    *v = if neg { -mag } else { mag };
    Ok(())
}
