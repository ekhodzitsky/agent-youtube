//! Conservative prefix planning from a complete, direct-reference segment index.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Index {
    pub samples: u64,
    pub sample_rate: u32,
    pub prefix_bytes: u64,
}
impl Index {
    pub(crate) fn frames(self) -> Result<u64> {
        if self.sample_rate == 0 || self.samples == 0 {
            return Err("invalid segment index duration".into());
        }
        Ok(self
            .samples
            .checked_mul(u64::from(crate::audio::SAMPLE_RATE))
            .ok_or("indexed duration overflow")?
            .div_ceil(u64::from(self.sample_rate)))
    }
}
pub(super) enum Probe {
    Pending,
    Unsupported,
    Ready(Index),
}

pub(super) fn probe(path: &Path, total_bytes: u64, through_frame: u64) -> Result<Probe> {
    let mut file = File::open(path)?;
    let present = file.metadata()?.len();
    let mut offset = 0u64;
    while offset < 16 * 1024 * 1024 {
        if offset.saturating_add(8) > present {
            return Ok(Probe::Pending);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0; 8];
        file.read_exact(&mut header)?;
        let size = u64::from(u32::from_be_bytes(header[..4].try_into()?));
        let Some(end) = offset
            .checked_add(size)
            .filter(|n| size >= 8 && *n <= total_bytes)
        else {
            return Ok(Probe::Unsupported);
        };
        match &header[4..] {
            b"sidx" => {
                if size > 1024 * 1024 {
                    return Ok(Probe::Unsupported);
                }
                if end > present {
                    return Ok(Probe::Pending);
                }
                let mut data = vec![0; usize::try_from(size - 8)?];
                file.read_exact(&mut data)?;
                return Ok(parse(&data, end, total_bytes, through_frame)
                    .map_or(Probe::Unsupported, Probe::Ready));
            }
            b"ftyp" | b"moov" | b"free" | b"skip" => offset = end,
            _ => return Ok(Probe::Unsupported),
        }
    }
    Ok(Probe::Unsupported)
}

fn parse(data: &[u8], end: u64, total: u64, through: u64) -> Option<Index> {
    let word = |at| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let version = *data.first()?;
    if !matches!(version, 0 | 1) || data.get(1..4)? != [0; 3] {
        return None;
    }
    let rate = word(8)?;
    if rate == 0 {
        return None;
    }
    let mut at = 12;
    let width = if version == 0 { 4 } else { 8 };
    // Nonzero origins/offsets and hierarchical indexes need a different planner.
    if data.get(at..at + width * 2)?.iter().any(|b| *b != 0) {
        return None;
    }
    at += width * 2;
    if data.get(at..at + 2)? != [0; 2] {
        return None;
    }
    let count = usize::from(u16::from_be_bytes(
        data.get(at + 2..at + 4)?.try_into().ok()?,
    ));
    at += 4;
    if count == 0 || at.checked_add(count.checked_mul(12)?)? != data.len() {
        return None;
    }
    let target = through.checked_add(u64::from(crate::audio::SAMPLE_RATE))?;
    let mut samples = 0u64;
    let mut bytes = end;
    let mut prefix = None;
    for _ in 0..count {
        let size = word(at)?;
        let duration = word(at + 4)?;
        if size & 0x8000_0000 != 0 || size == 0 || duration == 0 {
            return None;
        }
        bytes = bytes.checked_add(u64::from(size))?;
        samples = samples.checked_add(u64::from(duration))?;
        if prefix.is_none()
            && u128::from(samples) * u128::from(crate::audio::SAMPLE_RATE)
                >= u128::from(target) * u128::from(rate)
        {
            prefix = Some(bytes);
        }
        at += 12;
    }
    if bytes != total {
        return None;
    }
    let prefix_bytes = prefix.filter(|bytes| *bytes < total)?;
    Some(Index {
        samples,
        sample_rate: rate,
        prefix_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn index() -> Vec<u8> {
        let mut out = Vec::new();
        for value in [
            0u32, 1, 16000, 0, 0, 3, 100, 160000, 0, 200, 160000, 0, 300, 160000, 0,
        ] {
            out.extend_from_slice(&value.to_be_bytes());
        }
        out
    }
    #[test]
    fn prefix_plan_requires_complete_unambiguous_extents() {
        assert_eq!(
            parse(&index(), 100, 700, 160000),
            Some(Index {
                samples: 480000,
                sample_rate: 16000,
                prefix_bytes: 400,
            })
        );
        assert_eq!(parse(&index(), 100, 700, 400000), None);
        assert_eq!(parse(&index(), 100, 699, 160000), None);
        for at in [0, 1, 12, 16, 20, 24] {
            let mut bad = index();
            bad[at] = 0x80;
            assert_eq!(parse(&bad, 100, 700, 160000), None, "offset {at}");
        }
        for n in 0..index().len() {
            assert_eq!(parse(&index()[..n], 100, 700, 160000), None);
        }
    }
}
