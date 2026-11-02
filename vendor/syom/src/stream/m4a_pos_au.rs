//! Access-unit reading and exact-replay geometry validation.
use super::*;

impl<R: Read + Seek> M4aSeek<R> {
    pub(super) fn decode_au(&mut self, idx: usize) -> Result<(u32, usize)> {
        let result = self.decode_au_inner(idx).and_then(|(rate, n)| {
            if self.exact_mode {
                self.check_exact_frame(rate, n)?;
            }
            Ok((rate, n))
        });
        if result.is_err() {
            self.noise_free_prefix = 0;
            self.exact_failed = self.exact_mode;
        }
        result
    }

    fn decode_au_inner(&mut self, idx: usize) -> Result<(u32, usize)> {
        let &(off, len) = self
            .track
            .frames
            .get(idx)
            .ok_or_else(|| AacError::format("aac: sample index past table"))?;
        self.opts.memory.check_declared_au(u64::from(len))?;
        let n = usize::try_from(len).map_err(|_| AacError::format("aac: sample range overflow"))?;
        self.reader
            .seek(SeekFrom::Start(off))
            .map_err(AacError::from)?;
        let mut payload = vec![0u8; n];
        read_exact(&mut self.reader, &mut payload)?;
        let rate = if self.mono {
            self.scratch.clear();
            self.dec
                .decode_raw_mono_f32(
                    self.asc.aot,
                    self.asc.sampling_frequency_index,
                    self.asc.sample_rate,
                    self.asc.channel_configuration,
                    1,
                    &payload,
                    &mut self.scratch,
                )
                .map_err(AacError::from)?
        } else {
            self.dec
                .decode_frame_scaled(
                    self.asc.aot,
                    self.asc.sampling_frequency_index,
                    self.asc.sample_rate,
                    self.asc.channel_configuration,
                    &payload,
                )
                .map_err(AacError::from)?
        };
        if idx == self.noise_free_prefix
            && self.dec.rng == crate::engine::pns::Lcg::new()
            && !self.dec.sbr_active
        {
            self.noise_free_prefix += 1;
        }
        let n_samp = if self.mono {
            self.scratch.len()
        } else {
            self.dec.frame_planes().first().map_or(0, Vec::len)
        };
        Ok((rate, n_samp))
    }
}
