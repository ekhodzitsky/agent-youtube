//! Exact finite-state restart for edit-list LC mono/stereo sources.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resume {
    version: u32,
    codec: Vec<u8>,
    resampler: Vec<u8>,
    rate: u32,
    samples: u64,
    written: u64,
    index: usize,
    pcm: Vec<u8>,
    chunk_frames: u64,
}
impl Resume {
    pub fn completed_chunks(&self) -> usize {
        self.index
    }
}
pub(crate) enum DecodeEvent<'a> {
    Chunk(usize, &'a [u8]),
    Checkpoint(usize, &'a Resume),
}
#[derive(Debug)]
struct Pause;
impl std::fmt::Display for Pause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("codec checkpoint boundary")
    }
}
impl std::error::Error for Pause {}

pub(crate) fn decode_resumable(
    source: &Path,
    chunk_frames: u64,
    cancelled: &AtomicBool,
    resume: Option<Resume>,
    event: impl FnMut(DecodeEvent<'_>) -> Result<()>,
) -> Result<u64> {
    decode_inner(source, chunk_frames, cancelled, resume, None, None, event).map(|v| v.frames)
}

pub(crate) struct Decoded {
    pub frames: u64,
    pub prefix_frames: Option<u64>,
}

impl Decoded {
    fn full(frames: u64) -> Self {
        Self {
            frames,
            prefix_frames: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SourcePrefix {
    pub samples: u64,
    pub sample_rate: u32,
}

pub(crate) fn full_source_required() -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure(
        "range_full_source_required",
        Some(false),
        "source prefix cannot establish the requested PCM; full acquisition required",
    )
}
fn replay(
    source: &Path,
    chunk_frames: u64,
    cancelled: &AtomicBool,
    prefix: Option<SourcePrefix>,
    mut event: impl FnMut(DecodeEvent<'_>) -> Result<()>,
) -> Result<Decoded> {
    if prefix.is_some() {
        return Err(full_source_required());
    }
    decode_chunks(source, chunk_frames, cancelled, |index, pcm| {
        event(DecodeEvent::Chunk(index, pcm))
    })
    .map(Decoded::full)
}

pub(crate) fn decode_prefix(
    source: &Path,
    chunk_frames: u64,
    cancelled: &AtomicBool,
    resume: Option<Resume>,
    end_frame: Option<u64>,
    source_prefix: Option<SourcePrefix>,
    event: impl FnMut(DecodeEvent<'_>) -> Result<()>,
) -> Result<Decoded> {
    match (end_frame, source_prefix) {
        (Some(end), prefix) => decode_inner(
            source,
            chunk_frames,
            cancelled,
            resume,
            Some(end),
            prefix,
            event,
        ),
        (None, None) => {
            decode_resumable(source, chunk_frames, cancelled, resume, event).map(Decoded::full)
        }
        (None, Some(_)) => Err(full_source_required()),
    }
}

fn decode_inner(
    source: &Path,
    chunk_frames: u64,
    cancelled: &AtomicBool,
    resume: Option<Resume>,
    end_frame: Option<u64>,
    source_prefix: Option<SourcePrefix>,
    mut event: impl FnMut(DecodeEvent<'_>) -> Result<()>,
) -> Result<Decoded> {
    check_cancelled(cancelled)?;
    if chunk_frames == 0 {
        return Err(invalid(source));
    }
    let bytes = usize::try_from(chunk_frames.checked_mul(2).ok_or_else(|| invalid(source))?)?;
    crate::memory::require(bytes as u64 + (32 << 20), source, "decode_audio")?;
    let opts = decode_options(source)?;
    let opened = syom::M4aSeek::open(
        File::open(source).map_err(|error| decode_error(source, syom::AacError::Io(error)))?,
        opts,
    );
    let mut codec = match opened {
        Ok(codec) if codec.supports_checkpoint() => codec,
        Err(error @ (syom::AacError::Limit { .. } | syom::AacError::Io(_))) => {
            return Err(decode_error(source, error));
        }
        _ if resume.is_none() => {
            crate::protocol::event("decoder checkpoint unavailable; using exact replay");
            return replay(source, chunk_frames, cancelled, source_prefix, event);
        }
        _ => return Err(invalid(source)),
    };
    let expected_rate = codec.output_sample_rate();
    if let Some(prefix) = source_prefix
        && (prefix.sample_rate != expected_rate
            || prefix.samples < codec.presentation_len()
            || prefix.samples == 0
            || codec.bounded_presentation())
    {
        return Err(full_source_required());
    }
    // The container index supplies a source-length estimate, not a claim that
    // the unprocessed suffix decoded successfully. Preserve that distinction.
    let indexed_samples = source_prefix.map_or_else(|| codec.presentation_len(), |p| p.samples);
    let indexed_frames = indexed_samples
        .checked_mul(u64::from(SAMPLE_RATE))
        .ok_or_else(|| invalid(source))?
        .div_ceil(u64::from(expected_rate));
    let stop_chunk = end_frame.map(|end| end.div_ceil(chunk_frames));
    let prefix_result = |index: usize| -> Result<Option<Decoded>> {
        let through = (index as u64)
            .checked_mul(chunk_frames)
            .ok_or_else(|| invalid(source))?;
        Ok(
            if stop_chunk.is_some_and(|stop| stop > 0 && index as u64 >= stop)
                && through < indexed_frames
            {
                Some(Decoded {
                    frames: indexed_frames,
                    prefix_frames: Some(through),
                })
            } else {
                None
            },
        )
    };
    let mut index = 0;
    let mut samples = 0u64;
    let mut written = 0u64;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(bytes)
        .map_err(|e| crate::memory::allocation_error(source, "decode_audio", e))?;
    let mut stream = bandlimit::Stream::new(
        bandlimit::Rate::new(expected_rate.into()).map_err(resample_error)?,
        bandlimit::Rate::new(SAMPLE_RATE.into()).map_err(resample_error)?,
    )
    .map_err(resample_error)?;
    let restoring = resume.is_some();
    if let Some(state) = resume {
        if state.version != 1
            || state.chunk_frames != chunk_frames
            || state.rate != expected_rate
            || state.pcm.len() >= bytes
            || state.pcm.len() % 2 != 0
            || state.index == 0
            || (state.index as u64)
                .checked_mul(chunk_frames)
                .and_then(|n| n.checked_add(state.pcm.len() as u64 / 2))
                != Some(state.written)
        {
            return Err(invalid(source));
        }
        codec
            .restore_checkpoint(&state.codec)
            .map_err(|_| invalid(source))?;
        if codec.position() != state.samples {
            return Err(invalid(source));
        }
        stream =
            bandlimit::Stream::restore_checkpoint(&state.resampler).map_err(|_| invalid(source))?;
        if !stream.checkpoint_position_matches(
            bandlimit::Rate::new(expected_rate.into()).map_err(resample_error)?,
            bandlimit::Rate::new(SAMPLE_RATE.into()).map_err(resample_error)?,
            state.samples,
            state.written,
        ) {
            return Err(invalid(source));
        }
        samples = state.samples;
        written = state.written;
        index = state.index;
        buffer.extend_from_slice(&state.pcm);
        crate::protocol::event(format!(
            "restoring exact decoder state after {index} completed chunks"
        ));
    }
    if codec.checkpoint_compatible()
        && let Some(done) = prefix_result(index)?
    {
        check_cancelled(cancelled)?;
        return Ok(done);
    }
    let mut converted = Vec::new();
    loop {
        let mut failure = None;
        let mut checkpoint = false;
        let mut unsupported_rate = false;
        let outcome = codec.decode(|frame| {
            if frame.sample_rate != expected_rate {
                unsupported_rate = true;
                return Err(syom::AacError::Io(io::Error::other(Pause)));
            }
            #[cfg(test)]
            super::FRAME_COUNT.set(super::FRAME_COUNT.get() + 1);
            let result = (|| -> Result<()> {
                check_cancelled(cancelled)?;
                if frame.planar.len() != 1
                    || frame.planar[0].len() != frame.samples
                    || frame.planar[0].iter().any(|v| !v.is_finite())
                {
                    return Err(invalid(source));
                }
                converted.clear();
                let capacity = (frame.samples as u64 * u64::from(SAMPLE_RATE))
                    .div_ceil(u64::from(expected_rate))
                    + u64::from(SAMPLE_RATE);
                converted
                    .try_reserve(usize::try_from(capacity)?)
                    .map_err(|e| crate::memory::allocation_error(source, "resample", e))?;
                stream
                    .write(frame.planar[0], &mut converted)
                    .map_err(resample_error)?;
                samples = samples
                    .checked_add(frame.samples as u64)
                    .ok_or_else(|| invalid(source))?;
                written = written
                    .checked_add(converted.len() as u64)
                    .ok_or_else(|| invalid(source))?;
                let pcm = ryf::f32_to_s16le(&converted);
                let mut pcm = pcm.as_slice();
                while !pcm.is_empty() {
                    let count = (bytes - buffer.len()).min(pcm.len());
                    buffer.extend_from_slice(&pcm[..count]);
                    pcm = &pcm[count..];
                    if buffer.len() == bytes {
                        event(DecodeEvent::Chunk(index, &buffer))?;
                        index = index.checked_add(1).ok_or_else(|| invalid(source))?;
                        buffer.clear();
                        checkpoint = true;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                let message = error.to_string();
                failure = Some(error);
                return Err(syom::AacError::Io(io::Error::other(message)));
            }
            if checkpoint {
                return Err(syom::AacError::Io(io::Error::other(Pause)));
            }
            Ok(())
        });
        if let Some(error) = failure {
            return Err(error);
        }
        if unsupported_rate && !restoring && index == 0 {
            return replay(source, chunk_frames, cancelled, source_prefix, event);
        }
        match outcome {
            Err(syom::AacError::Io(error))
                if error.get_ref().is_some_and(|e| e.is::<Pause>()) && !unsupported_rate =>
            {
                if !codec.checkpoint_compatible() {
                    crate::protocol::event("stateful AAC extension requires exact replay");
                    return replay(source, chunk_frames, cancelled, source_prefix, event);
                }
                if let Some(state) = codec.checkpoint() {
                    event(DecodeEvent::Checkpoint(
                        index - 1,
                        &Resume {
                            version: 1,
                            codec: state,
                            resampler: stream.checkpoint().map_err(resample_error)?,
                            rate: expected_rate,
                            samples,
                            written,
                            index,
                            pcm: buffer.clone(),
                            chunk_frames,
                        },
                    ))?;
                    if let Some(done) = prefix_result(index)? {
                        check_cancelled(cancelled)?;
                        return Ok(done);
                    }
                }
            }
            Err(error) => return Err(decode_error(source, error)),
            Ok(_) => {
                if !codec.checkpoint_compatible() {
                    crate::protocol::event("stateful AAC extension requires exact replay");
                    return replay(source, chunk_frames, cancelled, source_prefix, event);
                }
                if source_prefix.is_some() {
                    return Err(full_source_required());
                }
                break;
            }
        }
    }
    if codec.remaining_samples() != 0 && !codec.bounded_presentation() {
        crate::protocol::event("unbounded presentation requires exact replay");
        return replay(source, chunk_frames, cancelled, source_prefix, event);
    }
    if !codec.presentation_complete() || samples == 0 || codec.position() != samples {
        return Err(decode_error(
            source,
            syom::AacError::format("incomplete M4A presentation"),
        ));
    }
    let (target, pcm) = resampler_tail(source, expected_rate, samples, written, stream)?;
    // The drain is shorter than one chunk, but can cross the next boundary.
    let mut pcm = pcm.as_slice();
    while !pcm.is_empty() {
        let count = (bytes - buffer.len()).min(pcm.len());
        buffer.extend_from_slice(&pcm[..count]);
        pcm = &pcm[count..];
        if buffer.len() == bytes {
            event(DecodeEvent::Chunk(index, &buffer))?;
            index += 1;
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        event(DecodeEvent::Chunk(index, &buffer))?;
    }
    Ok(Decoded::full(target))
}
fn invalid(source: &Path) -> Box<dyn std::error::Error + Send + Sync> {
    crate::checkpoint::evidence_failure(
        "decode_resume_invalid",
        source,
        "invalid or incompatible exact decoder state; source and chunks preserved",
    )
}
