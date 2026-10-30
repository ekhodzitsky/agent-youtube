//! ADTS, LOAS, or bounded fMP4 from a generic [`std::io::Read`]. Flat M4A needs random access
//! ([`crate::decode_streaming`] on a slice, or TASK-57; bounded fMP4 streams
//! through [`Decoder::feed`] since TASK-125). Also the one-shot slice
//! drivers behind `decode_streaming` / `decode_with`.

use std::io::{self, Read};

use crate::error::{AacError, Result};
use crate::options::DecodeOptions;

use super::{Decoder, Frame, StreamInfo};

impl Decoder {
    /// One-shot over a complete slice (ADTS, LATM, or fMP4): sniffs and pumps without
    /// copying the input into the push buffer. Equivalent to one `feed` of
    /// the whole slice plus `finish`.
    pub(super) fn decode_slice<F>(mut self, data: &[u8], on_frame: F) -> Result<StreamInfo>
    where
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        self.opts.validate()?;
        let mut cb = on_frame;
        self.pump(data, None, None, &mut cb, true)?;
        self.tallies()
    }

    /// One-shot mono over a complete slice: decode straight into `dst`,
    /// skipping the per-frame scratch + callback copy. Caller guarantees
    /// [`ChannelMode::Mono`](crate::options::ChannelMode); `est_frames`
    /// pre-sizes `dst` after frame 1.
    pub(super) fn decode_slice_mono(
        mut self,
        data: &[u8],
        est_frames: Option<usize>,
        dst: &mut Vec<f32>,
    ) -> Result<StreamInfo> {
        fn ignore(_: Frame<'_>) -> Result<()> {
            Ok(())
        }
        self.opts.validate()?;
        self.pump(data, Some(dst), est_frames, &mut ignore, true)?;
        self.tallies()
    }

    /// Pull bytes from `reader` and [`Decoder::feed`] them. Short reads are
    /// normal. [`io::ErrorKind::Interrupted`] is retried. Other I/O errors
    /// fail the instance ([`AacError::Io`]). Does not [`Decoder::finish`]:
    /// a truncated tail is decided there, matching a push session.
    ///
    /// The read scratch is at most the resident input budget (4 KiB typical).
    pub fn feed_read<R, F>(&mut self, reader: &mut R, mut on_frame: F) -> Result<u64>
    where
        R: Read,
        F: FnMut(Frame<'_>) -> Result<()>,
    {
        self.ensure_open()?;
        self.opts.validate()?;
        let cap = usize::try_from(self.opts.memory.max_buffered_input_bytes).unwrap_or(usize::MAX);
        let chunk = cap.clamp(1, 4096);
        let mut tmp = vec![0u8; chunk];
        let mut total = 0u64;
        loop {
            let n = match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return self.fail(AacError::from(e)),
            };
            self.feed(&tmp[..n], &mut on_frame)?;
            total += n as u64;
        }
        Ok(total)
    }

    #[cfg(test)]
    pub(crate) fn input_capacity(&self) -> usize {
        self.buf.capacity()
    }
}

/// Stream-decode ADTS, LOAS or bounded fMP4 from `reader`. Same lifecycle
/// as a push [`Decoder`]: `feed_read` then [`Decoder::finish`]. Flat M4A is
/// [`AacError::Unsupported`].
///
/// ```
/// use std::io::Read;
/// use syom::{DecodeOptions, decode_read_streaming};
/// struct OneByte<'a>(&'a [u8]);
/// impl Read for OneByte<'_> {
///     fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
///         if self.0.is_empty() || buf.is_empty() {
///             return Ok(0);
///         }
///         buf[0] = self.0[0];
///         self.0 = &self.0[1..];
///         Ok(1)
///     }
/// }
/// let adts = include_bytes!("../goldens/sine48.adts");
/// let mut samples = 0usize;
/// let info = decode_read_streaming(OneByte(adts), &DecodeOptions::speech(), |f| {
///     samples += f.samples;
///     Ok(())
/// })?;
/// assert_eq!(info.sample_rate, 48_000);
/// assert_eq!(samples as u64, info.samples);
/// # Ok::<(), syom::AacError>(())
/// ```
pub fn decode_read_streaming<R, F>(
    mut reader: R,
    opts: &DecodeOptions,
    mut on_frame: F,
) -> Result<StreamInfo>
where
    R: Read,
    F: FnMut(Frame<'_>) -> Result<()>,
{
    let mut dec = Decoder::new(opts.clone());
    dec.feed_read(&mut reader, &mut on_frame)?;
    dec.finish(&mut on_frame)
}
