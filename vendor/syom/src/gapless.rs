//! Apple/ffmpeg `iTunSMPB` as an ID3v2.4 `TXXX` frame.
//!
//! ADTS has no edit list. A one-shot ADTS encode prepends one tag whose
//! priming, remainder and source length are the encoder tallies (HE
//! priming is 3018 output-rate samples). One-shot decode drops those
//! edges only when the decoded sample count equals the tag sum, and
//! leaves a stale tag untouched. A push feed applies a tag whose edges
//! are each at most [`MAX_PUSH_GAP`] as frames arrive, and ignores a
//! larger edge. `finish` reports the edges only when the count matches.

use crate::budgets::{BudgetExceeded, BudgetKind};
use crate::engine::adts::AdtsHeader;
use crate::engine::enc_he::HE_PRIMING_OUT;
use crate::engine::enc_sbr_prep::{CORE_FRAME, FIR_DELAY, OUT_FRAME};
use crate::error::{AacError, Result};

/// Push decode applies a tag immediately when both edges fit in this many
/// samples (LC 1024, HE 3018, the common Apple delay 2112). A larger edge
/// is ignored on a push feed so the resident buffer stays one access unit.
/// One-shot decode still checks the tag against the decoded length.
pub(crate) const MAX_PUSH_GAP: u64 = 4096;

/// Encoder delay record, in decoded samples at the output rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Delay {
    pub priming: u64,
    pub remainder: u64,
    pub source: u64,
}

/// What a leading ID3 tag is, without copying its body.
pub(crate) enum Id3At {
    /// Not an ID3 tag (or not one we can treat as one).
    Absent,
    /// `ID3` started but the header or the declared body is short.
    Need,
    /// Full tag. `delay` is set only for an `iTunSMPB` text we could read.
    Ready { len: usize, delay: Option<Delay> },
}

/// LC: one 1024-sample block of priming, tail pad inside the last content
/// frame. Matches [`crate::EncodeInfo`] for LC and surround.
pub(crate) fn delay_lc(samples: u64) -> Delay {
    let block = 1024u64;
    let remainder = (block - (samples % block)) % block;
    Delay {
        priming: block,
        remainder,
        source: samples,
    }
}

/// HE v1/v2 output-rate delay. Access-unit count follows `HeEncoder::finish`:
/// content frames, then at least one silent drain, until coded samples
/// cover priming plus the source. Lookahead still emits one AU per core frame.
pub(crate) fn delay_he(samples: u64) -> Delay {
    let priming = HE_PRIMING_OUT;
    let core = (samples + FIR_DELAY as u64).div_ceil(2);
    let content = core.div_ceil(CORE_FRAME as u64);
    let need = priming.saturating_add(samples);
    let out = OUT_FRAME as u64;
    let mut frames = content;
    loop {
        frames = frames.saturating_add(1);
        if frames.saturating_mul(out) >= need {
            break;
        }
    }
    let coded = frames.saturating_mul(out);
    Delay {
        priming,
        remainder: coded.saturating_sub(need),
        source: samples,
    }
}

/// ID3 bytes for `samples` of LC (`he == false`) or HE output-rate PCM.
pub(crate) fn tag_bytes(samples: u64, he: bool) -> Vec<u8> {
    build_id3(&if he {
        delay_he(samples)
    } else {
        delay_lc(samples)
    })
}

/// Prepend an LC tag. The ADTS access units themselves are not rewritten.
pub(crate) fn prefix_lc(adts: Vec<u8>, samples: u64) -> Vec<u8> {
    prefix(adts, &delay_lc(samples))
}

/// Prepend a tag built from the encoder's own tallies (HE uses these).
pub(crate) fn prefix_delay(adts: Vec<u8>, priming: u64, remainder: u64, source: u64) -> Vec<u8> {
    prefix(
        adts,
        &Delay {
            priming,
            remainder,
            source,
        },
    )
}

fn prefix(mut adts: Vec<u8>, delay: &Delay) -> Vec<u8> {
    let tag = build_id3(delay);
    let mut out = Vec::with_capacity(tag.len() + adts.len());
    out.extend_from_slice(&tag);
    out.append(&mut adts);
    out
}

/// Drop one valid leading tag. Anything else is returned unchanged.
/// Does not allocate the tag body. Test-only: production decode peels in
/// the stream pump.
#[cfg(test)]
pub(crate) fn strip_id3(data: &[u8]) -> &[u8] {
    match id3_at(data, u64::MAX) {
        Ok(Id3At::Ready { len, .. }) if len <= data.len() => &data[len..],
        _ => data,
    }
}

/// The twelve-field text ffmpeg writes, leading space included.
pub(crate) fn itunsmpb_value(d: &Delay) -> String {
    format!(
        " {:08X} {:08X} {:08X} {:016X} 00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000",
        d.priming, d.remainder, 0u32, d.source
    )
}

fn build_id3(delay: &Delay) -> Vec<u8> {
    let value = itunsmpb_value(delay);
    let mut body = Vec::with_capacity(1 + 9 + value.len());
    body.push(3); // UTF-8
    body.extend_from_slice(b"iTunSMPB\0");
    body.extend_from_slice(value.as_bytes());
    let mut tag = Vec::with_capacity(10 + 10 + body.len());
    tag.extend_from_slice(b"ID3");
    tag.push(4);
    tag.push(0);
    tag.push(0);
    let frame = 10 + body.len();
    tag.extend_from_slice(&syncsafe(frame as u32));
    tag.extend_from_slice(b"TXXX");
    tag.extend_from_slice(&syncsafe(body.len() as u32));
    tag.extend_from_slice(&[0, 0]);
    tag.extend_from_slice(&body);
    tag
}

fn syncsafe(n: u32) -> [u8; 4] {
    [
        ((n >> 21) & 0x7f) as u8,
        ((n >> 14) & 0x7f) as u8,
        ((n >> 7) & 0x7f) as u8,
        (n & 0x7f) as u8,
    ]
}

fn unsyncsafe(b: &[u8]) -> Option<u32> {
    if b.len() < 4 || b[..4].iter().any(|x| x & 0x80 != 0) {
        return None;
    }
    Some(
        (u32::from(b[0]) << 21)
            | (u32::from(b[1]) << 14)
            | (u32::from(b[2]) << 7)
            | u32::from(b[3]),
    )
}

/// Parse a leading ID3v2.3/2.4 tag. A declared size above `budget` is a
/// [`AacError::Limit`] and is never allocated.
pub(crate) fn id3_at(data: &[u8], budget: u64) -> Result<Id3At> {
    if data.len() < 3 {
        if !data.is_empty() && data == &b"ID3"[..data.len()] {
            return Ok(Id3At::Need);
        }
        return Ok(Id3At::Absent);
    }
    if &data[..3] != b"ID3" {
        return Ok(Id3At::Absent);
    }
    if data.len() < 10 {
        return Ok(Id3At::Need);
    }
    let major = data[3];
    if major != 3 && major != 4 {
        return Ok(Id3At::Absent);
    }
    let flags = data[5];
    let Some(body) = unsyncsafe(&data[6..10]) else {
        return Ok(Id3At::Absent);
    };
    // v2.3 stores a plain size. Accept it only when it still fits the budget
    // and the high bit of each byte was clear (same shape as syncsafe).
    let mut total = 10u64.saturating_add(u64::from(body));
    if flags & 0x10 != 0 {
        total = total.saturating_add(10);
    }
    if total > budget {
        return Err(AacError::from(BudgetExceeded {
            kind: BudgetKind::Input,
            observed: total,
            max: budget,
        }));
    }
    let Ok(len) = usize::try_from(total) else {
        return Err(AacError::from(BudgetExceeded {
            kind: BudgetKind::Input,
            observed: total,
            max: budget,
        }));
    };
    if (data.len() as u64) < total {
        return Ok(Id3At::Need);
    }
    let delay = if flags & 0xC0 != 0 {
        // Unsynchronisation or an extended header: skip the tag, do not guess.
        None
    } else {
        let end = 10 + body as usize;
        gapless_in_tag(&data[10..end])
    };
    Ok(Id3At::Ready { len, delay })
}

fn gapless_in_tag(body: &[u8]) -> Option<Delay> {
    let mut pos = 0usize;
    while pos + 10 <= body.len() {
        let id = &body[pos..pos + 4];
        let size = unsyncsafe(&body[pos + 4..pos + 8])?;
        let format_flags = body[pos + 9];
        pos += 10;
        let size = size as usize;
        if pos + size > body.len() {
            return None;
        }
        let frame = &body[pos..pos + size];
        pos += size;
        if format_flags != 0 {
            continue;
        }
        if (id == b"TXXX" || id == b"COMM")
            && let Some(d) = text_delay(id, frame)
        {
            return Some(d);
        }
    }
    None
}

fn text_delay(id: &[u8], frame: &[u8]) -> Option<Delay> {
    let enc = *frame.first()?;
    if enc != 0 && enc != 3 {
        return None;
    }
    let mut p = 1usize;
    if id == b"COMM" {
        p = 4; // encoding + 3-byte language
        if frame.len() < p {
            return None;
        }
    }
    let rel = frame[p..].iter().position(|&b| b == 0)?;
    if &frame[p..p + rel] != b"iTunSMPB" {
        return None;
    }
    parse_window(&frame[p + rel + 1..])
}

fn parse_window(w: &[u8]) -> Option<Delay> {
    let mut i = 0usize;
    while i < w.len() {
        if let Some(d) = parse_at(w, i) {
            return Some(d);
        }
        i += 1;
    }
    None
}

fn parse_at(w: &[u8], mut i: usize) -> Option<Delay> {
    if i < w.len() && w[i] == b' ' {
        i += 1;
    }
    let priming = hex_n(w, &mut i, 8)?;
    if w.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;
    let remainder = hex_n(w, &mut i, 8)?;
    if w.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;
    let _ignored = hex_n(w, &mut i, 8)?;
    if w.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;
    let source = hex_n(w, &mut i, 16)?;
    Some(Delay {
        priming,
        remainder,
        source,
    })
}

fn hex_n(w: &[u8], p: &mut usize, n: usize) -> Option<u64> {
    let bytes = w.get(*p..*p + n)?;
    let mut v = 0u64;
    for &c in bytes {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        v = (v << 4) | u64::from(d);
    }
    *p += n;
    Some(v)
}

/// Complete ADTS frame count, or [`None`] when a byte falls outside a frame.
pub(crate) fn exact_adts_frames(data: &[u8]) -> Option<u64> {
    let mut pos = 0usize;
    let mut n = 0u64;
    while pos < data.len() {
        let (hdr, _) = AdtsHeader::parse(&data[pos..]).ok()?;
        let len = usize::from(hdr.aac_frame_length);
        if len == 0 || pos.saturating_add(len) > data.len() {
            return None;
        }
        pos += len;
        n += 1;
    }
    Some(n)
}

/// Decoded samples one AAC frame yields for this ASC.
pub(crate) fn output_frame_samples(aot: u8, sbr: bool, ps: bool) -> u64 {
    if sbr || ps {
        2048
    } else if aot == 23 {
        512
    } else {
        1024
    }
}

/// `iTunSMPB` inside a `moov` (the buffer may be a whole file or the box).
pub(crate) fn itunsmpb_in(data: &[u8]) -> Option<Delay> {
    let body = moov_payload(data)?;
    let key = b"iTunSMPB";
    let mut from = 0usize;
    while let Some(rel) = find_bytes(&body[from..], key) {
        let at = from + rel + key.len();
        let end = at.saturating_add(180).min(body.len());
        if let Some(d) = body.get(at..end).and_then(parse_window) {
            return Some(d);
        }
        from = at;
        if from >= body.len() {
            break;
        }
    }
    None
}

fn moov_payload(data: &[u8]) -> Option<&[u8]> {
    if data.len() >= 8 && &data[4..8] == b"moov" {
        return box_body(data, 0);
    }
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size32 = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        if size32 < 8 || pos.saturating_add(size32) > data.len() {
            return None;
        }
        let typ = &data[pos + 4..pos + 8];
        if typ == b"moov" {
            return Some(&data[pos + 8..pos + size32]);
        }
        pos += size32;
    }
    None
}

fn box_body(data: &[u8], pos: usize) -> Option<&[u8]> {
    let size = u32::from_be_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
    if size < 8 || pos + size > data.len() {
        return None;
    }
    Some(&data[pos + 8..pos + size])
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
