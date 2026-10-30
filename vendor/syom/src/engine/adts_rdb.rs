//! ADTS frames may contain 1..=4 `raw_data_block()` payloads.

use super::adts::AdtsHeader;
use super::decode::{INV_S16, StreamDecoder};
use super::error::{Error, Result};

impl StreamDecoder {
    /// Decode `n_rdb` consecutive `raw_data_block()`s. When CRC is present
    /// and `n_rdb > 1`, a 16-bit field after each block is skipped here;
    /// [`super::adts_crc::verify_adts_crc`] checks it on the full frame.
    /// `n_rdb == 1` is the historical single-block path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode_adts_blocks(
        &mut self,
        aot: u8,
        fs_index: u8,
        sample_rate: u32,
        channel_configuration: u8,
        n_rdb: u8,
        protection_absent: bool,
        payload: &[u8],
    ) -> Result<u32> {
        let n = n_rdb.max(1);
        if n == 1 {
            return self.decode_into_bufs(
                aot,
                fs_index,
                sample_rate,
                channel_configuration,
                payload,
            );
        }
        let mut rest = payload;
        let mut acc: Vec<Vec<f32>> = Vec::new();
        let mut rate = sample_rate;
        for _ in 0..n {
            if rest.is_empty() {
                return Err(Error::UnexpectedEnd);
            }
            rate =
                self.decode_into_bufs(aot, fs_index, sample_rate, channel_configuration, rest)?;
            let used = self.last_rdb_bytes;
            if used == 0 || used > rest.len() {
                return Err(Error::Format("ADTS raw_data_block length invalid"));
            }
            rest = &rest[used..];
            if !protection_absent {
                if rest.len() < 2 {
                    return Err(Error::UnexpectedEnd);
                }
                rest = &rest[2..];
            }
            if acc.is_empty() {
                acc = std::mem::take(&mut self.frame_ch);
            } else {
                if acc.len() != self.frame_ch.len() {
                    return Err(Error::Format("ADTS multi-block channel count changed"));
                }
                for (dst, src) in acc.iter_mut().zip(&self.frame_ch) {
                    dst.extend_from_slice(src);
                }
            }
        }
        self.frame_ch = acc;
        Ok(rate)
    }

    /// One ADTS access unit: `n` raw data blocks, optional CRC skip, optional
    /// mono accumulate into `dst`.
    pub(crate) fn decode_adts_header_payload(
        &mut self,
        hdr: &AdtsHeader,
        payload: &[u8],
        dst: Option<&mut Vec<f32>>,
    ) -> Result<u32> {
        let aot = hdr.audio_object_type();
        let fs = hdr.sampling_frequency_index;
        let sr = hdr.sample_rate();
        let ch = hdr.channel_configuration;
        let n = hdr.number_of_raw_data_blocks_in_frame;
        if let Some(dst) = dst {
            let was = self.mix_down_mono;
            self.mix_down_mono = true;
            self.fast_mono = true;
            let rate = self.decode_adts_blocks(aot, fs, sr, ch, n, hdr.protection_absent, payload);
            self.fast_mono = false;
            self.mix_down_mono = was;
            let rate = rate?;
            let src = if self.sbr_active {
                self.frame_ch.first().map(Vec::as_slice).unwrap_or(&[])
            } else {
                self.pcm_l.as_slice()
            };
            dst.extend(src.iter().map(|&v| v * INV_S16));
            Ok(rate)
        } else {
            let rate =
                self.decode_adts_blocks(aot, fs, sr, ch, n, hdr.protection_absent, payload)?;
            let t_sc = super::prof::stamp();
            for plane in &mut self.frame_ch {
                for v in plane.iter_mut() {
                    *v *= INV_S16;
                }
            }
            self.prof_scale += super::prof::ns(t_sc);
            if super::prof::on() {
                self.prof_frames = self.prof_frames.saturating_add(1);
            }
            Ok(rate)
        }
    }
}
