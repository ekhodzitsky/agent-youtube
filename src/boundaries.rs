//! Optional pause-adjusted ASR cuts. Decoder checkpoints remain on their fixed grid.
use crate::{Result, audio};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::atomic::AtomicBool};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    schema_version: u32,
    frames: u64,
    chunk_frames: u64,
    window_ms: u32,
    edges: Vec<u64>,
}
impl Plan {
    pub fn fixed(frames: u64, chunk_frames: u64) -> Result<Self> {
        if frames == 0 || chunk_frames == 0 {
            return Err("recognition span must be nonempty".into());
        }
        Ok(Self {
            schema_version: 1,
            frames,
            chunk_frames,
            window_ms: 0,
            edges: Vec::new(),
        })
    }
    pub fn validate(&self, frames: u64, chunk_frames: u64, window_ms: u32) -> Result<()> {
        if frames == 0 || chunk_frames == 0 {
            return Err("recognition span must be nonempty".into());
        }
        if window_ms == 0
            && self.schema_version == 1
            && self.frames == frames
            && self.chunk_frames == chunk_frames
            && self.window_ms == 0
            && self.edges.is_empty()
        {
            return Ok(());
        }
        let count = usize::try_from(frames.div_ceil(chunk_frames))?;
        let window = u64::from(window_ms) * 16;
        if self.schema_version != 1
            || self.frames != frames
            || self.chunk_frames != chunk_frames
            || self.window_ms != window_ms
            || self.edges.len() != count.checked_add(1).ok_or("boundary count overflow")?
            || self.edges.first() != Some(&0)
            || self.edges.last() != Some(&frames)
            || self.edges.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .edges
                .iter()
                .enumerate()
                .skip(1)
                .take(count.saturating_sub(1))
                .any(|(i, edge)| edge.abs_diff(i as u64 * chunk_frames) > window)
        {
            return Err("invalid recognition boundary plan; evidence preserved".into());
        }
        Ok(())
    }
    pub fn span(&self, index: usize) -> Result<(u64, u64)> {
        if self.window_ms == 0 {
            let start = (index as u64)
                .checked_mul(self.chunk_frames)
                .ok_or("boundary index overflow")?;
            let frames = self
                .frames
                .checked_sub(start)
                .filter(|frames| *frames > 0)
                .ok_or("missing recognition boundary")?;
            return Ok((start, frames.min(self.chunk_frames)));
        }
        let pair = self
            .edges
            .get(index..index.checked_add(2).ok_or("boundary index overflow")?)
            .ok_or("missing recognition boundary")?;
        Ok((pair[0], pair[1] - pair[0]))
    }
    pub fn adjust(&mut self, root: &Path, window_ms: u32, cancelled: &AtomicBool) -> Result<()> {
        if window_ms == 0 {
            return Ok(());
        }
        let count = usize::try_from(self.frames.div_ceil(self.chunk_frames))?;
        let mut edges = Vec::new();
        edges.try_reserve(count.checked_add(1).ok_or("boundary count overflow")?)?;
        for i in 0..count {
            edges.push(i as u64 * self.chunk_frames);
        }
        edges.push(self.frames);
        self.edges = edges;
        self.window_ms = window_ms;
        let window = u64::from(window_ms) * 16;
        for index in 1..self.edges.len() - 1 {
            audio::check_cancelled(cancelled)?;
            let nominal = self.edges[index];
            let start = nominal.saturating_sub(window + 1600);
            let end = (nominal + window + 1600).min(self.frames);
            let mut pcm = Vec::new();
            pcm.try_reserve(usize::try_from((end - start) * 2)?)?;
            audio::copy_pcm_span(
                root,
                self.chunk_frames,
                start,
                end - start,
                cancelled,
                |bytes| {
                    pcm.extend_from_slice(bytes);
                    Ok(())
                },
            )?;
            self.edges[index] = quiet_edge(&pcm, start, nominal, window);
        }
        self.validate(self.frames, self.chunk_frames, window_ms)
    }
}

fn quiet_edge(pcm: &[u8], start: u64, nominal: u64, window: u64) -> u64 {
    // 200 ms mean-square window, 20 ms search step, below approximately -35 dBFS.
    // Prefer the nominal cut on equal energy. No pause means no adjustment.
    let energy = |center: u64| -> Option<u64> {
        let offset = usize::try_from(center.checked_sub(start)?.checked_sub(1600)? * 2).ok()?;
        Some(
            pcm.get(offset..offset + 6400)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| {
                    let n = i64::from(i16::from_le_bytes([b[0], b[1]]));
                    (n * n) as u64
                })
                .sum::<u64>()
                / 3200,
        )
    };
    let mut best = None;
    for center in (nominal.saturating_sub(window)..=nominal.saturating_add(window)).step_by(320) {
        if let Some(power) = energy(center).filter(|p| *p <= 340_000) {
            let candidate = (power, center.abs_diff(nominal), center);
            if best.is_none_or(|previous| candidate < previous) {
                best = Some(candidate);
            }
        }
    }
    if let Some(power) = energy(nominal).filter(|p| *p <= 340_000) {
        let candidate = (power, 0, nominal);
        if best.is_none_or(|previous| candidate < previous) {
            best = Some(candidate);
        }
    }
    best.map_or(nominal, |(_, _, center)| center)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_plan_needs_no_per_chunk_allocation_or_duration_ceiling() {
        let plan = Plan::fixed(u64::MAX - 1, 1).unwrap();
        assert!(plan.edges.is_empty());
        assert_eq!(plan.span(0).unwrap(), (0, 1));
        let plan = Plan::fixed(21, 10).unwrap();
        assert_eq!(plan.span(2).unwrap(), (20, 1));
        assert!(plan.span(3).is_err());
    }

    #[test]
    fn pause_search_does_not_drop_or_duplicate_samples() {
        let mut pcm = vec![0; 64000];
        for sample in pcm.as_chunks_mut::<2>().0 {
            sample.copy_from_slice(&12000_i16.to_le_bytes());
        }
        for sample in pcm[38000..46000].as_chunks_mut::<2>().0 {
            sample.fill(0);
        }
        let edge = quiet_edge(&pcm, 144000, 160000, 16000);
        assert!(edge > 160000);
        let mut plan = Plan::fixed(400000, 160000).unwrap();
        plan.window_ms = 1000;
        plan.edges = vec![0, edge, 320000, 400000];
        plan.validate(400000, 160000, 1000).unwrap();
        assert_eq!((0..3).map(|i| plan.span(i).unwrap().1).sum::<u64>(), 400000);
        plan.edges[2] = edge;
        assert!(plan.validate(400000, 160000, 1000).is_err());
    }
    #[test]
    fn silence_ties_and_continuous_speech_keep_nominal_cut() {
        assert_eq!(quiet_edge(&vec![0; 70400], 142400, 160000, 16000), 160000);
        let pcm: Vec<_> = (0..35200).flat_map(|_| 12000_i16.to_le_bytes()).collect();
        assert_eq!(quiet_edge(&pcm, 142400, 160000, 16000), 160000);
    }
}
