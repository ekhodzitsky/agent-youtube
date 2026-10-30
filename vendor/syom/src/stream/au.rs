//! Raw access-unit decode: a validated ASC plus complete `raw_data_block()`
//! payloads. No ADTS/LATM wrap; the caller is the demuxer.

use crate::engine::asc::AudioSpecificConfig;
use crate::error::{AacError, Result};
use crate::options::{ChannelMode, DecodeOptions};

use super::{Container, Decoder, Frame};

impl Decoder {
    /// Configure a push decoder from an `AudioSpecificConfig` blob and then
    /// [`decode_au`](Self::decode_au) complete access units.
    ///
    /// The ASC is parsed and validated (LC / HE-AAC v1 / HE-AAC v2 / AAC-LD
    /// AOT 23 at 512 or 480 samples per frame, and 960-sample LC). HE-AAC
    /// on a 960-sample core is [`AacError::Unsupported`]. PCM is
    /// delivered through the same borrowed [`Frame`] callback as [`Self::feed`].
    ///
    /// ```
    /// use syom::{DecodeOptions, Decoder};
    /// // Simulated demux: ASC out of band, then complete AUs (here: ADTS
    /// // payloads with the 7-byte header stripped — the decoder never sees it).
    /// let adts = include_bytes!("../goldens/sine48.adts");
    /// let asc = &[0x11, 0x88]; // AAC-LC, 48 kHz, mono
    /// let mut dec = Decoder::from_asc(asc, DecodeOptions::speech())?;
    /// let mut samples = 0usize;
    /// let mut pos = 0usize;
    /// while pos + 7 <= adts.len() {
    ///     let len = (((adts[pos + 3] as usize) & 0x03) << 11)
    ///         | ((adts[pos + 4] as usize) << 3)
    ///         | ((adts[pos + 5] as usize) >> 5);
    ///     if len < 7 || pos + len > adts.len() {
    ///         break;
    ///     }
    ///     dec.decode_au(&adts[pos + 7..pos + len], |f| {
    ///         samples += f.samples;
    ///         Ok(())
    ///     })?;
    ///     pos += len;
    /// }
    /// let info = dec.finish(|_| Ok(()))?;
    /// assert_eq!(info.sample_rate, 48_000);
    /// assert_eq!(samples as u64, info.samples);
    /// # Ok::<(), syom::AacError>(())
    /// ```
    pub fn from_asc(asc: &[u8], opts: DecodeOptions) -> Result<Self> {
        let mut dec = Self::new(opts);
        dec.set_asc(asc)?;
        Ok(dec)
    }

    /// Install or replace the ASC on an **open** decoder.
    ///
    /// After a frame has been emitted, a different parsed ASC is
    /// [`AacError::Unsupported`] (`AscChange`); call [`Self::reset`] first.
    /// The same ASC may be repeated. `feed` (ADTS/LATM/fMP4) and `decode_au`
    /// do not mix on one instance.
    pub fn set_asc(&mut self, asc: &[u8]) -> Result<()> {
        self.ensure_open()?;
        self.opts.validate()?;
        if matches!(
            self.container,
            Container::Adts | Container::Latm | Container::Fmp4
        ) {
            return Err(AacError::Unsupported(
                crate::UnsupportedFeature::RawAccessUnit,
            ));
        }
        let (cfg, _) = AudioSpecificConfig::parse(asc).map_err(AacError::from)?;
        let out = cfg.output_sample_rate;
        if out == 0 || out > self.opts.max_sample_rate {
            return Err(AacError::sample_rate(out, self.opts.max_sample_rate));
        }
        if self.emitted > 0 {
            if self.au.as_ref() != Some(&cfg) {
                return Err(AacError::Unsupported(crate::UnsupportedFeature::AscChange));
            }
            return Ok(());
        }
        let mono = self.dec.mix_down_mono;
        self.dec.reset();
        self.dec.mix_down_mono = mono;
        if let Some(pce) = cfg.pce.clone() {
            self.dec.set_config_pce(pce);
        }
        self.dec
            .set_he_config(cfg.sbr_present, cfg.ps_present, cfg.output_sample_rate);
        self.dec.set_frame_samples(cfg.frame_samples);
        self.au = Some(cfg);
        self.container = Container::Au;
        Ok(())
    }

    /// Decode one complete AAC access unit (`raw_data_block()` bytes).
    ///
    /// Requires [`Self::from_asc`] / [`Self::set_asc`]. Empty input is
    /// [`AacError::Truncated`]. Errors fail the instance until [`Self::reset`].
    pub fn decode_au<F>(&mut self, au: &[u8], mut on_frame: F) -> Result<()>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        self.ensure_open()?;
        self.opts.validate()?;
        if !matches!(self.container, Container::Au) {
            return self.fail(AacError::Unsupported(
                crate::UnsupportedFeature::RawAccessUnit,
            ));
        }
        let (aot, fs, sr, ch) = match self.au.as_ref() {
            Some(c) => (
                c.aot,
                c.sampling_frequency_index,
                c.sample_rate,
                c.channel_configuration,
            ),
            None => {
                return self.fail(AacError::Unsupported(
                    crate::UnsupportedFeature::RawAccessUnit,
                ));
            }
        };
        if au.is_empty() {
            return self.fail(AacError::truncated_at(Some(0)));
        }
        if let Err(e) = self.opts.memory.check_declared_au(au.len() as u64) {
            return self.fail(AacError::from(e));
        }
        let rate = if matches!(self.opts.channel_mode, ChannelMode::Mono) {
            self.mono_scratch.clear();
            match self
                .dec
                .decode_raw_mono_f32(aot, fs, sr, ch, 1, au, &mut self.mono_scratch)
            {
                Ok(r) => r,
                Err(e) => return self.fail(AacError::from(e)),
            }
        } else {
            match self.dec.decode_frame_scaled(aot, fs, sr, ch, au) {
                Ok(r) => r,
                Err(e) => return self.fail(AacError::from(e)),
            }
        };
        if let Err(e) = self.emit(rate, &mut on_frame) {
            return self.fail(e);
        }
        Ok(())
    }
}
