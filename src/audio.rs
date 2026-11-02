use crate::{Result, storage};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

mod resume;
#[cfg(test)]
mod seek_qualification;
#[cfg(test)]
use resume::decode_resumable;
pub(crate) use resume::{DecodeEvent, Resume, SourcePrefix, decode_prefix, full_source_required};

#[cfg(test)]
thread_local! { static FRAME_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

pub const SAMPLE_RATE: u32 = 16_000;
pub const BACKEND: &str = "syom-0.9.0-6019908-checkpoint1-bandlimit-0.1.7-ryf-0.7.3-mono-pcm16-v3";

pub(crate) fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("interrupted; run the same command to resume".into());
    }
    Ok(())
}

fn resample_error(error: impl std::fmt::Debug) -> Box<dyn std::error::Error + Send + Sync> {
    crate::protocol::failure_details(
        "audio_conversion_failed",
        Some(false),
        format!("resampling failed: {error:?}"),
        serde_json::json!({"stage":"resample","preserved":true}),
    )
}
fn decode_error(source: &Path, error: syom::AacError) -> Box<dyn std::error::Error + Send + Sync> {
    let (code, retryable) = match &error {
        syom::AacError::Unsupported(_) | syom::AacError::UnsupportedSampleRate { .. } => {
            ("audio_decode_unsupported", Some(false))
        }
        syom::AacError::Limit { .. } => ("resource_limit", None),
        syom::AacError::Io(_) => ("audio_decode_io", None),
        _ => ("audio_decode_invalid", Some(false)),
    };
    let mut details = serde_json::json!({"path":source.to_string_lossy(),"stage":"decode_audio","preserved":true});
    if let syom::AacError::Limit {
        kind,
        observed,
        max,
    } = &error
    {
        details["resource"] =
            serde_json::json!({"kind":kind.to_string(),"observed":observed,"limit":max});
    }
    crate::protocol::failure_details(
        code,
        retryable,
        format!("cannot decode AAC/M4A source: {error}; source retained"),
        details,
    )
}

fn writer(path: &Path) -> Result<ryf::WavWriter<File>> {
    let file = storage::private_file().read(true).open(path)?;
    Ok(ryf::WavWriter::new(
        file,
        ryf::WriteSpec::s16(SAMPLE_RATE, 1),
    )?)
}

/// Copy a contiguous sample span across fixed decoder chunks without re-decoding.
pub(crate) fn copy_pcm_span(
    root: &Path,
    chunk_frames: u64,
    mut start: u64,
    mut frames: u64,
    cancelled: &AtomicBool,
    mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut buffer = [0; 64 * 1024];
    while frames > 0 {
        check_cancelled(cancelled)?;
        let mut wav = Wav::open(&root.join(format!("chunks/{:06}.wav", start / chunk_frames)))?;
        let offset = start % chunk_frames;
        let take = frames.min(
            wav.frames
                .checked_sub(offset)
                .ok_or("ASR span exceeds chunk")?,
        );
        if take == 0 {
            return Err("empty chunk in ASR span".into());
        }
        wav.source
            .seek(SeekFrom::Start(wav.data_pos + offset * 2))?;
        let mut bytes = take * 2;
        while bytes > 0 {
            check_cancelled(cancelled)?;
            let count = bytes.min(buffer.len() as u64) as usize;
            wav.source.read_exact(&mut buffer[..count])?;
            sink(&buffer[..count])?;
            bytes -= count as u64;
        }
        start += take;
        frames -= take;
    }
    Ok(())
}
pub(crate) fn write_span(
    root: &Path,
    output: &Path,
    chunk_frames: u64,
    start: u64,
    frames: u64,
    cancelled: &AtomicBool,
) -> Result<()> {
    let mut wav = writer(output)?;
    copy_pcm_span(root, chunk_frames, start, frames, cancelled, |bytes| {
        wav.write_pcm(bytes)?;
        Ok(())
    })?;
    wav.finalize()?;
    drop(wav);
    if Wav::open(output)?.frames != frames {
        return Err("ASR boundary WAV length mismatch".into());
    }
    Ok(())
}

/// Decode one seekable M4A into mono PCM, retaining only a frame and filter history.
#[cfg(test)]
pub fn decode(source: &Path, output: &Path, cancelled: &AtomicBool) -> Result<()> {
    check_cancelled(cancelled)?;
    let mut wav = writer(output)?;
    let frames = decode_pcm(source, cancelled, |pcm| {
        wav.write_pcm(pcm)?;
        Ok(())
    })?;
    wav.finalize()?;
    drop(wav);
    if Wav::open(output)?.frames != frames {
        return Err("written WAV length does not match resampled audio".into());
    }
    Ok(())
}

/// Continuous resampling across chunk boundaries; at most one chunk is buffered.
pub fn decode_chunks(
    source: &Path,
    chunk_frames: u64,
    cancelled: &AtomicBool,
    mut chunk: impl FnMut(usize, &[u8]) -> Result<()>,
) -> Result<u64> {
    if chunk_frames == 0 {
        return Err("chunk size must be positive".into());
    }
    let bytes = usize::try_from(chunk_frames.checked_mul(2).ok_or("chunk size overflow")?)?;
    let mut buffer = Vec::new();
    crate::memory::require(bytes as u64 + (32 << 20), source, "decode_audio")?;
    buffer
        .try_reserve_exact(bytes)
        .map_err(|error| crate::memory::allocation_error(source, "decode_audio", error))?;
    let mut index = 0usize;
    let frames = decode_pcm(source, cancelled, |mut pcm| {
        while !pcm.is_empty() {
            let count = (bytes - buffer.len()).min(pcm.len());
            buffer.extend_from_slice(&pcm[..count]);
            pcm = &pcm[count..];
            if buffer.len() == bytes {
                chunk(index, &buffer)?;
                index = index.checked_add(1).ok_or("chunk count overflow")?;
                buffer.clear();
            }
        }
        Ok(())
    })?;
    if !buffer.is_empty() {
        chunk(index, &buffer)?;
    }
    Ok(frames)
}

pub fn write_chunk(path: &Path, pcm: &[u8]) -> Result<()> {
    let mut wav = writer(path)?;
    wav.write_pcm(pcm)?;
    wav.finalize()?;
    Ok(())
}

fn decode_pcm(
    source: &Path,
    cancelled: &AtomicBool,
    write: impl FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    decode_pcm_inner(source, cancelled, write).map_err(|error| {
        if error.is::<crate::protocol::Failure>() { error }
        else { crate::protocol::failure_details("audio_conversion_failed", Some(false), error, serde_json::json!({"path":source.to_string_lossy(),"stage":"decode_audio","preserved":true})) }
    })
}

fn decode_options(source: &Path) -> Result<syom::DecodeOptions> {
    let mut opts = syom::DecodeOptions::speech().with_max_duration_secs(f64::INFINITY);
    opts.max_decode_sample_rate = opts.max_sample_rate;
    // PCM goes to disk. Index allocation fences scale with actual source bytes,
    // rather than imposing a recording-duration ceiling. Per-AU/workspace and
    // container range/depth validation remain enabled.
    let bytes = std::fs::metadata(source)
        .map_err(|error| decode_error(source, syom::AacError::Io(error)))?
        .len();
    opts.memory.max_output_bytes = u64::MAX;
    opts.memory.max_metadata_bytes = opts.memory.max_metadata_bytes.max(bytes.saturating_mul(32));
    opts.memory.max_index_entries = opts
        .memory
        .max_index_entries
        .max(u32::try_from(bytes).unwrap_or(u32::MAX));
    if let Some(budget) = crate::memory::budget() {
        opts.memory.max_metadata_bytes = opts.memory.max_metadata_bytes.min(budget.max(1));
        opts.memory.max_index_entries = opts
            .memory
            .max_index_entries
            .min(u32::try_from(budget / 32).unwrap_or(u32::MAX).max(1));
    }
    Ok(opts)
}

fn decode_pcm_inner(
    source: &Path,
    cancelled: &AtomicBool,
    mut write: impl FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    check_cancelled(cancelled)?;
    let opts = decode_options(source)?;
    let mut written = 0_u64;
    let mut sink_error = None;
    let mut resampler: Option<(u32, bandlimit::Stream)> = None;
    let mut converted = Vec::new();
    let mut samples = 0_u64;
    let decoded = syom::decode_seek_streaming(
        File::open(source).map_err(|error| decode_error(source, syom::AacError::Io(error)))?,
        &opts,
        |frame| {
            #[cfg(test)]
            FRAME_COUNT.set(FRAME_COUNT.get() + 1);
            (|| -> Result<()> {
                check_cancelled(cancelled)?;
                if frame.planar.len() != 1 || frame.planar[0].len() != frame.samples {
                    return Err("AAC decoder returned an invalid mono frame".into());
                }
                if resampler.is_none() {
                    resampler = Some((
                        frame.sample_rate,
                        bandlimit::Stream::new(
                            bandlimit::Rate::new(i64::from(frame.sample_rate))
                                .map_err(resample_error)?,
                            bandlimit::Rate::new(i64::from(SAMPLE_RATE)).map_err(resample_error)?,
                        )
                        .map_err(resample_error)?,
                    ));
                }
                let (rate, stream) = resampler.as_mut().ok_or("missing resampler")?;
                if *rate != frame.sample_rate || frame.planar[0].iter().any(|s| !s.is_finite()) {
                    return Err("AAC decoder changed sample rate or returned non-finite PCM".into());
                }
                // write needs caller-owned spare capacity, including for upsampling.
                let capacity = (frame.samples as u64 * u64::from(SAMPLE_RATE))
                    .div_ceil(u64::from(*rate))
                    + u64::from(SAMPLE_RATE);
                converted.clear();
                converted
                    .try_reserve(usize::try_from(capacity)?)
                    .map_err(|error| crate::memory::allocation_error(source, "resample", error))?;
                stream
                    .write(frame.planar[0], &mut converted)
                    .map_err(resample_error)?;
                write(&ryf::f32_to_s16le(&converted))?;
                written = written
                    .checked_add(converted.len() as u64)
                    .ok_or("PCM length overflow")?;
                samples = samples
                    .checked_add(frame.samples as u64)
                    .ok_or("audio length overflow")?;
                Ok(())
            })()
            .map_err(|error| {
                let message = error.to_string();
                sink_error = Some(error);
                syom::AacError::Io(io::Error::other(message))
            })
        },
    );
    if let Some(error) = sink_error {
        return Err(error);
    }
    let info = decoded.map_err(|error| decode_error(source, error))?;
    check_cancelled(cancelled)?;
    let (rate, stream) = resampler.ok_or("source contains no decoded audio")?;
    if samples == 0 || samples != info.samples {
        return Err("AAC sample count does not match decoded audio".into());
    }
    let (target, tail) = resampler_tail(source, rate, samples, written, stream)?;
    write(&tail)?;
    Ok(target)
}

/// Flush the rational-rate padding once, preserving the real fractional last sample.
fn resampler_tail(
    source: &Path,
    rate: u32,
    samples: u64,
    written: u64,
    mut stream: bandlimit::Stream,
) -> Result<(u64, Vec<u8>)> {
    let target = samples
        .checked_mul(u64::from(SAMPLE_RATE))
        .ok_or("audio length overflow")?
        .div_ceil(u64::from(rate));
    // bandlimit requires an integral rational output length. Pad to that grid,
    // then discard padding output while retaining the real fractional last sample.
    let ratio = bandlimit::Ratio::reduce(
        bandlimit::Rate::new(i64::from(rate)).map_err(resample_error)?,
        bandlimit::Rate::new(i64::from(SAMPLE_RATE)).map_err(resample_error)?,
    );
    let padding = (ratio.numerator() - samples % ratio.numerator()) % ratio.numerator();
    let mut converted = Vec::new();
    converted
        .try_reserve(usize::try_from(
            padding * u64::from(SAMPLE_RATE) / u64::from(rate) + u64::from(SAMPLE_RATE),
        )?)
        .map_err(|error| crate::memory::allocation_error(source, "resample", error))?;
    stream
        .write(&vec![0.0; usize::try_from(padding)?], &mut converted)
        .map_err(resample_error)?;
    converted.extend(stream.finish().map_err(resample_error)?);
    let remaining = target
        .checked_sub(written)
        .ok_or("resampler produced too many samples")?;
    let tail = converted
        .get(..usize::try_from(remaining)?)
        .ok_or("resampler produced too few samples")?;
    Ok((target, ryf::f32_to_s16le(tail)))
}

/// Only the pipeline's little-endian mono PCM16 WAVs are accepted here.
pub struct Wav {
    source: ryf::ByteSource<'static>,
    data_pos: u64,
    pub frames: u64,
}

impl Wav {
    pub fn open(path: &Path) -> Result<Self> {
        storage::reject_nonregular(path)?;
        let mut source = ryf::ByteSource::from_file(File::open(path)?);
        let mut prefix = [0; 12];
        source.read_exact(&mut prefix)?;
        if !matches!(&prefix[..4], b"RIFF" | b"RF64") || &prefix[8..] != b"WAVE" {
            return Err("expected a little-endian PCM WAV".into());
        }
        // ryf tolerates truncated payloads by clamping their declared length.
        // Our completed artifacts must instead match their original container size.
        let declared_size = if &prefix[..4] == b"RIFF" {
            u64::from(u32::from_le_bytes(prefix[4..8].try_into()?))
        } else {
            let mut ds64 = [0; 16];
            source.read_exact(&mut ds64)?;
            if &ds64[..4] != b"ds64" {
                return Err("expected RF64 size header".into());
            }
            u64::from_le_bytes(ds64[8..16].try_into()?)
        };
        if declared_size.checked_add(8) != source.byte_len() {
            return Err("WAV container size does not match its file length".into());
        }
        source.seek(SeekFrom::Start(0))?;
        let probe = ryf::probe(&mut source)?;
        let frames = probe.declared_frames.ok_or("WAV has no sample count")?;
        if probe.sample_rate != SAMPLE_RATE
            || probe.channels != 1
            || probe.sample_width != 2
            || probe.codec != ryf::ProbeCodec::PcmS16
            || frames == 0
        {
            return Err("expected nonempty mono PCM16 WAV at 16000 Hz".into());
        }
        let end = frames
            .checked_mul(2)
            .and_then(|bytes| probe.data_pos.checked_add(bytes))
            .ok_or("WAV length overflow")?;
        if Some(end) != source.byte_len() {
            return Err("WAV sample count does not match its file length".into());
        }
        Ok(Self {
            source,
            data_pos: probe.data_pos,
            frames,
        })
    }

    pub fn chunk(
        &mut self,
        output: &Path,
        offset: u64,
        frames: u64,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        check_cancelled(cancelled)?;
        if frames == 0
            || offset
                .checked_add(frames)
                .is_none_or(|end| end > self.frames)
        {
            return Err("chunk exceeds its audio span".into());
        }
        self.source
            .seek(SeekFrom::Start(self.data_pos + offset * 2))?;
        let mut wav = writer(output)?;
        let mut remaining = frames * 2;
        let mut buffer = [0; 64 * 1024];
        while remaining > 0 {
            check_cancelled(cancelled)?;
            let count = remaining.min(buffer.len() as u64) as usize;
            self.source.read_exact(&mut buffer[..count])?;
            wav.write_pcm(&buffer[..count])?;
            remaining -= count as u64;
        }
        wav.finalize()?;
        drop(wav);
        if Self::open(output)?.frames != frames {
            return Err("written chunk length does not match its audio span".into());
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn sparse_test_wav(path: &Path, frames: u64) {
    use std::io::Write;
    let mut header = ryf::encode_rf64(ryf::WriteSpec::s16(SAMPLE_RATE, 1), &[0; 2]).unwrap();
    let data_pos = header.len() as u64 - 2;
    assert_eq!(&header[12..16], b"ds64");
    header[20..28].copy_from_slice(&(data_pos + frames * 2 - 8).to_le_bytes());
    header[28..36].copy_from_slice(&(frames * 2).to_le_bytes());
    header[36..44].copy_from_slice(&frames.to_le_bytes());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(&header[..data_pos as usize]).unwrap();
    file.set_len(data_pos + frames * 2).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::atomic::AtomicU64};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(super) struct Temp(pub(super) std::path::PathBuf);
    impl Temp {
        pub(super) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agent-youtube-audio-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn streamed_decode_matches_batch_pcm_for_short_and_fractional_rate_lengths() {
        let temp = Temp::new();
        let input = temp.0.join("input.m4a");
        let output = temp.0.join("output.wav");
        for hz in [8000_u32, 16000, 22050, 24000, 32000, 44100, 48000] {
            for frames in [1, hz as usize / 10 + 1] {
                let pcm: Vec<f32> = (0..frames).map(|n| 0.2 * (n as f32 * 0.09).sin()).collect();
                let right: Vec<f32> = pcm.iter().map(|x| x * 0.5).collect();
                let aac = syom::encode_with(
                    &[&pcm, &right],
                    hz,
                    &syom::EncodeOptions::default()
                        .with_bitrate_bps(64_000)
                        .with_container(syom::EncodeContainer::M4a),
                )
                .unwrap();
                fs::write(&input, &aac).unwrap();
                decode(&input, &output, &AtomicBool::new(false)).unwrap();
                let mono = syom::decode(&aac).unwrap();
                assert_eq!(mono.channels[0].len(), frames);
                let from = bandlimit::Rate::new(i64::from(hz)).unwrap();
                let to = bandlimit::Rate::new(i64::from(SAMPLE_RATE)).unwrap();
                let quantum = bandlimit::Ratio::reduce(from, to).numerator() as usize;
                let mut padded = mono.channels[0].clone();
                padded.resize(frames.div_ceil(quantum) * quantum, 0.0);
                let expected = bandlimit::convert(&padded, from, to).unwrap();
                let count =
                    (frames as u64 * u64::from(SAMPLE_RATE)).div_ceil(u64::from(hz)) as usize;
                let (rate, actual) = ryf::read_s16(&output).unwrap();
                assert_eq!(rate, SAMPLE_RATE);
                assert_eq!(
                    actual,
                    ryf::f32_to_s16le(&expected[..count]),
                    "{hz} Hz, {frames} frames"
                );
                let mut joined = Vec::new();
                let mut index = 0;
                let total = decode_chunks(&input, 37, &AtomicBool::new(false), |number, bytes| {
                    assert_eq!(number, index);
                    assert!(!bytes.is_empty() && bytes.len() <= 74);
                    index += 1;
                    joined.extend_from_slice(bytes);
                    Ok(())
                })
                .unwrap();
                assert_eq!(total, count as u64);
                assert_eq!(joined, actual);
                fs::remove_file(&output).unwrap();
            }
        }
    }

    #[test]
    fn codec_and_resampler_states_restore_exact_pcm_at_seven_rates() {
        let temp = Temp::new();
        for hz in [8000_u32, 16000, 22050, 24000, 32000, 44100, 48000] {
            let input = temp.0.join("state.m4a");
            let pcm: Vec<f32> = (0..hz + 1)
                .map(|i| ((i).wrapping_mul(1664525).wrapping_add(1013904223) as i32) as f32 * 1e-10)
                .collect();
            let right: Vec<_> = pcm.iter().map(|x| -*x * 0.25).collect();
            let bytes = syom::encode_with(
                &[&pcm, &right],
                hz,
                &syom::EncodeOptions::default()
                    .with_bitrate_bps(64000)
                    .with_pns(true)
                    .with_lookahead(true)
                    .with_short_group(true)
                    .with_container(syom::EncodeContainer::M4a),
            )
            .unwrap();
            fs::write(&input, &bytes).unwrap();
            let opts = syom::DecodeOptions::speech();
            let mut expected = Vec::new();
            syom::decode_seek_streaming(File::open(&input).unwrap(), &opts, |frame| {
                expected.extend_from_slice(frame.planar[0]);
                Ok(())
            })
            .unwrap();
            let mut first = syom::M4aSeek::open(File::open(&input).unwrap(), opts.clone()).unwrap();
            assert!(first.supports_checkpoint());
            let mut actual = Vec::new();
            let mut frames = 0;
            first
                .decode(|frame| {
                    actual.extend_from_slice(frame.planar[0]);
                    frames += 1;
                    if frames == 3 {
                        return Err(syom::AacError::Io(io::Error::other("pause")));
                    }
                    Ok(())
                })
                .unwrap_err();
            let state = first.checkpoint().unwrap();
            let mut restored = syom::M4aSeek::open(File::open(&input).unwrap(), opts).unwrap();
            restored.restore_checkpoint(&state).unwrap();
            restored
                .decode(|frame| {
                    actual.extend_from_slice(frame.planar[0]);
                    Ok(())
                })
                .unwrap();
            assert_eq!(restored.remaining_samples(), 0);
            assert_eq!(
                actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "codec {hz}"
            );
            let from = bandlimit::Rate::new(hz.into()).unwrap();
            let to = bandlimit::Rate::new(16000).unwrap();
            let mut stream = bandlimit::Stream::new(from, to).unwrap();
            let mut baseline = Vec::with_capacity(expected.len() * 2 + 16000);
            let mut resumed = Vec::with_capacity(expected.len() * 2 + 16000);
            for (i, block) in expected.chunks(511).enumerate() {
                stream.write(block, &mut baseline).unwrap();
                if i == 2 {
                    resumed = baseline.clone();
                    resumed.reserve(expected.len() * 2 + 16000);
                }
                if i == 2 {
                    let state = stream.checkpoint().unwrap();
                    let mut other = bandlimit::Stream::restore_checkpoint(&state).unwrap();
                    for block in expected[1533..].chunks(511) {
                        other.write(block, &mut resumed).unwrap();
                    }
                }
            }
            assert_eq!(
                ryf::f32_to_s16le(&baseline),
                ryf::f32_to_s16le(&resumed),
                "resampler {hz}"
            );
            let mut bad = state;
            bad[0] ^= 1;
            assert!(restored.restore_checkpoint(&bad).is_err());
        }
    }

    #[test]
    fn finalization_preserves_fractional_tail_after_checkpoint_restore() {
        for hz in [8000_u32, 16000, 22050, 24000, 32000, 44100, 48000] {
            let from = bandlimit::Rate::new(hz.into()).unwrap();
            let to = bandlimit::Rate::new(SAMPLE_RATE.into()).unwrap();
            let quantum = bandlimit::Ratio::reduce(from, to).numerator() as usize;
            for count in [1, 13, 511, 512, hz as usize + 13] {
                let input: Vec<_> = (0..count)
                    .map(|i| 0.2 * (i as f32 * 0.19 + 0.3).sin())
                    .collect();
                let target = (count * SAMPLE_RATE as usize).div_ceil(hz as usize);
                let mut padded = input.clone();
                padded.resize(count.div_ceil(quantum) * quantum, 0.0);
                let expected = bandlimit::convert(&padded, from, to).unwrap();
                let expected = ryf::f32_to_s16le(&expected[..target]);
                for split in [0, count / 2, count] {
                    let mut stream = bandlimit::Stream::new(from, to).unwrap();
                    let mut prefix = Vec::with_capacity(count * 3 + 16000);
                    for block in input[..split].chunks(511) {
                        stream.write(block, &mut prefix).unwrap();
                    }
                    // Serialized state must also work immediately before EOF.
                    let state = stream.checkpoint().unwrap();
                    let restored = bandlimit::Stream::restore_checkpoint(&state).unwrap();
                    let mut outputs = Vec::new();
                    for mut stream in [stream, restored] {
                        let mut output = Vec::with_capacity(count * 3 + 16000);
                        output.extend_from_slice(&prefix);
                        for block in input[split..].chunks(511) {
                            stream.write(block, &mut output).unwrap();
                        }
                        let (frames, tail) = resampler_tail(
                            Path::new("fractional-tail-fixture"),
                            hz,
                            count as u64,
                            output.len() as u64,
                            stream,
                        )
                        .unwrap();
                        let mut pcm = ryf::f32_to_s16le(&output);
                        pcm.extend(tail);
                        assert_eq!(frames, target as u64);
                        assert_eq!(pcm.len(), target * 2);
                        assert_eq!(pcm, expected, "{hz}/{count}/{split}");
                        outputs.push(pcm);
                    }
                    assert_eq!(outputs[0], outputs[1]);
                }
            }
        }
    }

    #[test]
    fn resumed_chunks_match_continuous_pcm_including_fractional_tail() {
        let temp = Temp::new();
        for hz in [8000_u32, 16000, 22050, 24000, 32000, 44100, 48000] {
            for open_fragment in [false, true] {
                let source = temp.0.join("resume.m4a");
                let pcm: Vec<_> = (0..hz + 1).map(|i| 0.2 * (i as f32 * 0.19).sin()).collect();
                fs::write(
                    &source,
                    syom::encode_with(
                        &[&pcm],
                        hz,
                        &syom::EncodeOptions::default()
                            .with_bitrate_bps(32000)
                            .with_container(syom::EncodeContainer::M4a),
                    )
                    .unwrap(),
                )
                .unwrap();
                if open_fragment {
                    let (bytes, _) = fragmented(&fs::read(&source).unwrap(), true);
                    fs::write(&source, bytes).unwrap();
                }
                let mut expected = Vec::new();
                decode_chunks(&source, 1600, &AtomicBool::new(false), |_, pcm| {
                    expected.extend_from_slice(pcm);
                    Ok(())
                })
                .unwrap();
                // The fragmented fixture deliberately omits the edit list;
                // only the indexed fixture preserves the encoder input length.
                if !open_fragment {
                    assert_eq!(
                        expected.len() / 2,
                        ((hz as usize + 1) * SAMPLE_RATE as usize).div_ceil(hz as usize)
                    );
                }
                let mut actual = Vec::new();
                let mut saved = None;
                decode_resumable(&source, 1600, &AtomicBool::new(false), None, |event| {
                    match event {
                        DecodeEvent::Chunk(_, pcm) => actual.extend_from_slice(pcm),
                        DecodeEvent::Checkpoint(index, state) if index >= 2 => {
                            saved = Some(serde_json::to_vec(state).unwrap());
                            return Err("fixture interruption".into());
                        }
                        _ => {}
                    }
                    Ok(())
                })
                .unwrap_err();
                let resume = serde_json::from_slice(saved.as_ref().unwrap()).unwrap();
                let prefix = actual.len();
                let frames = decode_resumable(
                    &source,
                    1600,
                    &AtomicBool::new(false),
                    Some(resume),
                    |event| {
                        if let DecodeEvent::Chunk(index, pcm) = event {
                            assert!(index >= 3);
                            actual.extend_from_slice(pcm);
                        }
                        Ok(())
                    },
                )
                .unwrap();
                assert!(prefix > 0);
                assert_eq!(frames, expected.len() as u64 / 2);
                assert_eq!(actual, expected, "{hz} Hz fractional resume");
            }
        }
    }

    #[test]
    fn prefix_decoding_matches_full_pcm_and_restores_a_completed_prefix() {
        let temp = Temp::new();
        for hz in [16000_u32, 22050, 44100, 48000] {
            for open_fragment in [false, true] {
                let source = temp.0.join("prefix.m4a");
                let pcm: Vec<_> = (0..hz + 1).map(|i| 0.2 * (i as f32 * 0.19).sin()).collect();
                let encoded = syom::encode_with(
                    &[&pcm],
                    hz,
                    &syom::EncodeOptions::default()
                        .with_bitrate_bps(32000)
                        .with_pns(true)
                        .with_container(syom::EncodeContainer::M4a),
                )
                .unwrap();
                fs::write(
                    &source,
                    if open_fragment {
                        fragmented(&encoded, true).0
                    } else {
                        encoded
                    },
                )
                .unwrap();
                let cancelled = AtomicBool::new(false);
                let mut expected = Vec::new();
                FRAME_COUNT.set(0);
                let full = decode_resumable(&source, 1600, &cancelled, None, |event| {
                    if let DecodeEvent::Chunk(_, pcm) = event {
                        expected.extend_from_slice(pcm);
                    }
                    Ok(())
                })
                .unwrap();
                let full_work = FRAME_COUNT.get();
                FRAME_COUNT.set(0);
                let mut actual = Vec::new();
                let mut saved = None;
                let decoded = resume::decode_prefix(
                    &source,
                    1600,
                    &cancelled,
                    None,
                    Some(4801),
                    None,
                    |event| {
                        match event {
                            DecodeEvent::Chunk(_, pcm) => actual.extend_from_slice(pcm),
                            DecodeEvent::Checkpoint(_, state) => {
                                saved = Some(serde_json::to_vec(state).unwrap())
                            }
                        }
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(decoded.frames, full);
                assert_eq!(decoded.prefix_frames, Some(6400));
                assert_eq!(actual, expected[..12800]);
                assert!(FRAME_COUNT.get() < full_work * 3 / 4);
                FRAME_COUNT.set(0);
                let restored = resume::decode_prefix(
                    &source,
                    1600,
                    &cancelled,
                    Some(serde_json::from_slice(saved.as_ref().unwrap()).unwrap()),
                    Some(4801),
                    None,
                    |_| panic!("a completed prefix must not emit more chunks or checkpoints"),
                )
                .unwrap();
                assert_eq!(restored.prefix_frames, Some(6400));
                assert_eq!(restored.frames, full);
                assert_eq!(FRAME_COUNT.get(), 0);
                let mut resumed_pcm = Vec::new();
                let mut interrupted = None;
                resume::decode_prefix(&source, 1600, &cancelled, None, Some(4801), None, |event| {
                    match event {
                        DecodeEvent::Chunk(_, pcm) => resumed_pcm.extend_from_slice(pcm),
                        DecodeEvent::Checkpoint(_, state) => {
                            interrupted = Some(serde_json::to_vec(state).unwrap());
                            return Err("fixture interruption".into());
                        }
                    }
                    Ok(())
                })
                .err()
                .unwrap();
                let continued = resume::decode_prefix(
                    &source,
                    1600,
                    &cancelled,
                    Some(serde_json::from_slice(interrupted.as_ref().unwrap()).unwrap()),
                    Some(4801),
                    None,
                    |event| {
                        if let DecodeEvent::Chunk(_, pcm) = event {
                            resumed_pcm.extend_from_slice(pcm);
                        }
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(continued.prefix_frames, Some(6400));
                assert_eq!(resumed_pcm, expected[..12800]);
                let mut clamped = Vec::new();
                let complete = resume::decode_prefix(
                    &source,
                    1600,
                    &cancelled,
                    None,
                    Some(full + 16000),
                    None,
                    |event| {
                        if let DecodeEvent::Chunk(_, pcm) = event {
                            clamped.extend_from_slice(pcm);
                        }
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(complete.prefix_frames, None);
                assert_eq!(complete.frames, full);
                assert_eq!(clamped, expected);
            }
        }
    }

    #[test]
    fn prefix_decoding_replays_unqualified_he_aac_in_full() {
        let temp = Temp::new();
        let source = temp.0.join("he.m4a");
        fs::write(&source, include_bytes!("../tests/fixtures/ps48.m4a")).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut expected = Vec::new();
        let full = decode_chunks(&source, 1600, &cancelled, |_, pcm| {
            expected.extend_from_slice(pcm);
            Ok(())
        })
        .unwrap();
        let mut actual = Vec::new();
        let decoded =
            resume::decode_prefix(&source, 1600, &cancelled, None, Some(800), None, |event| {
                if let DecodeEvent::Chunk(_, pcm) = event {
                    actual.extend_from_slice(pcm);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(decoded.prefix_frames, None);
        assert_eq!(decoded.frames, full);
        assert_eq!(actual, expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "local long-input decode/resume RSS and disk profile; run alone"]
    fn long_decode_resume_profile() {
        use sha2::{Digest, Sha256};
        let temp = Temp::new();
        let seconds = std::env::var("AGENT_YOUTUBE_PROFILE_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(600)
            .max(600);
        let seed = syom::encode_with(
            &[&[0.0; 1024]],
            48000,
            &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
        )
        .unwrap();
        let stsz = seed.windows(4).position(|w| w == b"stsz").unwrap();
        let length = u32::from_be_bytes(seed[stsz + 16..stsz + 20].try_into().unwrap()) as usize;
        let mdat = seed.windows(4).position(|w| w == b"mdat").unwrap();
        let count = seconds as usize * 48000 / 1024 + 1;
        let source = temp.0.join("long.m4a");
        fs::write(
            &source,
            syom::mux_raw_lc_m4a(
                &vec![&seed[mdat + 4..mdat + 4 + length]; count],
                48000,
                1,
                (count as u64 - 1) * 1024,
            )
            .unwrap(),
        )
        .unwrap();
        let cancelled = AtomicBool::new(false);
        FRAME_COUNT.set(0);
        let begin = std::time::Instant::now();
        let mut expected = Sha256::new();
        let total = decode_chunks(&source, 60 * 16000, &cancelled, |_, pcm| {
            expected.update(pcm);
            Ok(())
        })
        .unwrap();
        let full_ms = begin.elapsed().as_millis();
        let full_frames = FRAME_COUNT.get();
        let stop = (total / (60 * 16000) * 4 / 5).max(1) - 1;
        let mut state = None;
        let mut peak = 0;
        let mut observe = || {
            let bytes = fs::read_dir(&temp.0)
                .unwrap()
                .map(|p| p.unwrap().metadata().unwrap().len())
                .sum::<u64>();
            peak = peak.max(bytes);
        };
        FRAME_COUNT.set(0);
        decode_resumable(&source, 60 * 16000, &cancelled, None, |event| {
            match event {
                DecodeEvent::Chunk(index, pcm) => {
                    write_chunk(&temp.0.join(format!("{index:06}.wav")), pcm)?;
                }
                DecodeEvent::Checkpoint(index, checkpoint) => {
                    let bytes = serde_json::to_vec(checkpoint)?;
                    crate::storage::atomic_write(
                        &temp.0.join(format!("{index:06}.resume.json")),
                        &bytes,
                        false,
                    )?;
                    if index as u64 == stop {
                        state = Some(bytes);
                        observe();
                        return Err("fixture interruption".into());
                    }
                }
            }
            observe();
            Ok(())
        })
        .unwrap_err();
        let prefix_frames = FRAME_COUNT.get();
        FRAME_COUNT.set(0);
        let begin = std::time::Instant::now();
        let result = decode_resumable(
            &source,
            60 * 16000,
            &cancelled,
            Some(serde_json::from_slice(state.as_ref().unwrap()).unwrap()),
            |event| {
                match event {
                    DecodeEvent::Chunk(index, pcm) => {
                        write_chunk(&temp.0.join(format!("{index:06}.wav")), pcm)?
                    }
                    DecodeEvent::Checkpoint(index, state) => crate::storage::atomic_write(
                        &temp.0.join(format!("{index:06}.resume.json")),
                        &serde_json::to_vec(state)?,
                        false,
                    )?,
                }
                observe();
                Ok(())
            },
        )
        .unwrap();
        let resume_ms = begin.elapsed().as_millis();
        let resume_frames = FRAME_COUNT.get();
        assert_eq!(result, total);
        assert_eq!(prefix_frames + resume_frames, full_frames);
        assert!(resume_frames < full_frames / 2);
        let mut actual = Sha256::new();
        for index in 0..total.div_ceil(60 * 16000) {
            actual.update(
                ryf::read_s16(temp.0.join(format!("{index:06}.wav")))
                    .unwrap()
                    .1,
            );
        }
        assert_eq!(actual.finalize()[..], expected.finalize()[..]);
        let rss = fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|s| s.starts_with("VmHWM:"))
            .unwrap()
            .to_owned();
        eprintln!(
            "decode_profile seconds={} full_ms={full_ms} resume_ms={resume_ms} full_frames={full_frames} prefix_frames={prefix_frames} resume_frames={resume_frames} peak_owned_bytes={peak} source_bytes={} {rss}",
            total / 16000,
            fs::metadata(source).unwrap().len()
        );
    }

    #[test]
    #[ignore = "local real-source multi-interruption verification; requires explicit paths"]
    fn real_long_decoder_restart_profile() {
        let source =
            std::path::PathBuf::from(std::env::var_os("AGENT_YOUTUBE_REAL_SOURCE").unwrap());
        let reference =
            std::path::PathBuf::from(std::env::var_os("AGENT_YOUTUBE_REAL_WAV").unwrap());
        let root = std::path::PathBuf::from(std::env::var_os("AGENT_YOUTUBE_REAL_OUTPUT").unwrap());
        fs::create_dir_all(&root).unwrap();
        let mut original = Wav::open(&reference).unwrap();
        let cancelled = AtomicBool::new(false);
        let chunk_frames = 300 * 16000;
        let mut state = None;
        let mut compared = 0_u64;
        let started = std::time::Instant::now();
        for stop in [Some(10), Some(25), Some(40), None] {
            let result =
                decode_resumable(&source, chunk_frames, &cancelled, state.take(), |event| {
                    match event {
                        DecodeEvent::Chunk(index, pcm) => {
                            original.source.seek(SeekFrom::Start(
                                original.data_pos + index as u64 * chunk_frames * 2,
                            ))?;
                            let mut expected = vec![0; pcm.len()];
                            original.source.read_exact(&mut expected)?;
                            assert_eq!(pcm, expected, "real PCM differs in chunk {index}");
                            write_chunk(&root.join(format!("{index:06}.wav")), pcm)?;
                            compared += pcm.len() as u64 / 2;
                        }
                        DecodeEvent::Checkpoint(index, checkpoint) => {
                            let path = root.join(format!("{index:06}.resume.json"));
                            crate::storage::atomic_write(
                                &path,
                                &serde_json::to_vec(checkpoint)?,
                                false,
                            )?;
                            if Some(index) == stop {
                                state = Some(serde_json::from_slice(&fs::read(path)?)?);
                                return Err("real profile interruption".into());
                            }
                        }
                    }
                    Ok(())
                });
            if stop.is_some() {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("real profile interruption")
                );
                assert!(state.is_some());
            } else {
                assert_eq!(result.unwrap(), original.frames);
            }
            eprintln!(
                "real_restart stop={stop:?} compared_frames={compared} elapsed_ms={}",
                started.elapsed().as_millis()
            );
        }
        assert_eq!(compared, original.frames);
        fs::write(root.join("verification.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "pcm_identical":true,"frames":compared,"interruptions":[10,25,40],"elapsed_ms":started.elapsed().as_millis(),
            "vm_status":fs::read_to_string("/proc/self/status").unwrap()
        })).unwrap()).unwrap();
    }

    #[test]
    fn decoder_variants_keep_stable_unsupported_resource_and_conversion_codes() {
        let source = Path::new("retained.audio");
        for (error, code, retryable) in [
            (
                syom::AacError::UnsupportedSampleRate {
                    rate: 768000,
                    max: 384000,
                },
                "audio_decode_unsupported",
                Some(false),
            ),
            (
                syom::AacError::Limit {
                    kind: syom::BudgetKind::Metadata,
                    observed: 1025,
                    max: 1024,
                },
                "resource_limit",
                None,
            ),
            (
                syom::AacError::Io(io::Error::from(io::ErrorKind::PermissionDenied)),
                "audio_decode_io",
                None,
            ),
        ] {
            let error = decode_error(source, error);
            let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
            assert_eq!(failure.code, code);
            assert_eq!(failure.retryable, retryable);
        }
        let error = resample_error(bandlimit::Error::NonFinite { index: 0 });
        assert_eq!(
            error
                .downcast_ref::<crate::protocol::Failure>()
                .unwrap()
                .code,
            "audio_conversion_failed"
        );
    }

    #[test]
    fn unqualified_rate_preserves_exact_replay_fallback() {
        let temp = Temp::new();
        let source = temp.0.join("unqualified.m4a");
        let pcm: Vec<_> = (0..6401).map(|i| 0.2 * (i as f32 * 0.09).sin()).collect();
        fs::write(
            &source,
            syom::encode_with(
                &[&pcm],
                64000,
                &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
            )
            .unwrap(),
        )
        .unwrap();
        let mut expected = Vec::new();
        decode_chunks(&source, 1600, &AtomicBool::new(false), |_, pcm| {
            expected.extend_from_slice(pcm);
            Ok(())
        })
        .unwrap();
        let mut actual = Vec::new();
        decode_resumable(&source, 1600, &AtomicBool::new(false), None, |event| {
            match event {
                DecodeEvent::Chunk(_, pcm) => actual.extend_from_slice(pcm),
                DecodeEvent::Checkpoint(..) => panic!("unqualified rate must replay"),
            };
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, expected);
    }

    // Repackage our encoder's access units as one real moof/mdat fragment.
    fn fragmented(flat: &[u8], open_end: bool) -> (Vec<u8>, u64) {
        fn location(data: &[u8], kind: &[u8; 4]) -> usize {
            data.windows(4).position(|w| w == kind).unwrap()
        }
        fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            out.extend(kind);
            out.extend(body);
            out
        }
        fn numbers(values: &[u32]) -> Vec<u8> {
            values.iter().flat_map(|n| n.to_be_bytes()).collect()
        }
        let stsz = location(flat, b"stsz");
        assert_eq!(&flat[stsz + 8..stsz + 12], &[0; 4]);
        let count = u32::from_be_bytes(flat[stsz + 12..stsz + 16].try_into().unwrap());
        let sizes = &flat[stsz + 16..stsz + 16 + count as usize * 4];
        let moov_pos = location(flat, b"moov");
        let moov_len =
            u32::from_be_bytes(flat[moov_pos - 4..moov_pos].try_into().unwrap()) as usize;
        let mut moov = flat[moov_pos + 4..moov_pos - 4 + moov_len].to_vec();
        let edit = location(&moov, b"elst");
        let skip = u32::from_be_bytes(moov[edit + 16..edit + 20].try_into().unwrap());
        if open_end {
            moov[edit + 12..edit + 16].copy_from_slice(&0_u32.to_be_bytes());
        }
        moov.extend(atom(
            b"mvex",
            &atom(b"trex", &numbers(&[0, 1, 1, 1024, 0, 0])),
        ));
        let mut run = numbers(&[0x301, count, 0]);
        for size in sizes.as_chunks::<4>().0 {
            run.extend(1024_u32.to_be_bytes());
            run.extend(size);
        }
        let mut traf = atom(b"tfhd", &numbers(&[0x020000, 1]));
        traf.extend(atom(b"trun", &run));
        let mut moof = atom(b"mfhd", &numbers(&[0, 1]));
        moof.extend(atom(b"traf", &traf));
        let mut moof = atom(b"moof", &moof);
        let run_pos = location(&moof, b"trun");
        let offset = (moof.len() + 8) as u32;
        moof[run_pos + 12..run_pos + 16].copy_from_slice(&offset.to_be_bytes());
        let mdat = location(flat, b"mdat");
        let mdat_len = u32::from_be_bytes(flat[mdat - 4..mdat].try_into().unwrap()) as usize;
        let mut out = atom(b"moov", &moov);
        out.extend(moof);
        out.extend_from_slice(&flat[mdat - 4..mdat - 4 + mdat_len]);
        (out, u64::from(count) * 1024 - u64::from(skip))
    }

    #[test]
    fn rounded_lc_presentation_matches_checkpoint_and_replay_paths() {
        let temp = Temp::new();
        let input = temp.0.join("lc.m4a");
        let output = temp.0.join("lc.wav");
        let mut bytes =
            syom::encode_with(&[&[0.1f32][..]], 48000, &syom::EncodeOptions::m4a()).unwrap();
        let edit = bytes.windows(4).position(|w| w == b"elst").unwrap();
        // Measure all available coded PCM after priming, before applying a rounded edit.
        bytes[edit + 12..edit + 16].copy_from_slice(&48000_u32.to_be_bytes());
        let mut codec =
            syom::M4aSeek::open(std::io::Cursor::new(&bytes), syom::DecodeOptions::speech())
                .unwrap();
        codec.decode(|_| Ok(())).unwrap();
        let actual_samples = codec.position();
        assert_ne!(actual_samples % 48, 0);
        drop(codec);
        let movie = bytes.windows(4).position(|w| w == b"mvhd").unwrap();
        bytes[movie + 16..movie + 20].copy_from_slice(&1000_u32.to_be_bytes());
        let ticks = u32::try_from(actual_samples.div_ceil(48)).unwrap();
        bytes[edit + 12..edit + 16].copy_from_slice(&ticks.to_be_bytes());
        fs::write(&input, &bytes).unwrap();
        decode(&input, &output, &AtomicBool::new(false)).unwrap();
        let expected = ryf::read_s16(&output).unwrap().1;
        let mut actual = Vec::new();
        let frames = decode_resumable(&input, 160, &AtomicBool::new(false), None, |event| {
            if let DecodeEvent::Chunk(_, pcm) = event {
                actual.extend_from_slice(pcm);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(frames, actual_samples.div_ceil(3));
        assert_eq!(actual, expected);
        let mut saved = None;
        let mut restarted = Vec::new();
        let interrupted = decode_resumable(&input, 160, &AtomicBool::new(false), None, |event| {
            match event {
                DecodeEvent::Chunk(_, pcm) => restarted.extend_from_slice(pcm),
                DecodeEvent::Checkpoint(_, state) => {
                    saved = Some(serde_json::to_vec(state)?);
                    return Err("test interruption at persisted decoder state".into());
                }
            }
            Ok(())
        });
        assert!(interrupted.is_err());
        let state = serde_json::from_slice(&saved.unwrap()).unwrap();
        decode_resumable(&input, 160, &AtomicBool::new(false), Some(state), |event| {
            if let DecodeEvent::Chunk(_, pcm) = event {
                restarted.extend_from_slice(pcm);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(restarted, expected);
        bytes[edit + 12..edit + 16].copy_from_slice(&(ticks + 1).to_be_bytes());
        fs::write(&input, bytes).unwrap();
        assert!(decode_resumable(&input, 160, &AtomicBool::new(false), None, |_| Ok(())).is_err());
    }

    #[test]
    fn complete_he_aac_accepts_sub_tick_rounding_but_not_missing_audio() {
        let original = include_bytes!("../tests/fixtures/ps48.m4a");
        let temp = Temp::new();
        let input = temp.0.join("ps.m4a");
        let output = temp.0.join("ps.wav");
        fs::write(&input, original).unwrap();
        decode(&input, &output, &AtomicBool::new(false)).unwrap();
        assert_eq!(Wav::open(&output).unwrap().frames, 51136_u64.div_ceil(3));
        let expected = ryf::read_s16(&output).unwrap().1;
        let mut actual = Vec::new();
        decode_resumable(&input, 1600, &AtomicBool::new(false), None, |event| {
            if let DecodeEvent::Chunk(_, pcm) = event {
                actual.extend_from_slice(pcm);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, expected);
        fs::remove_file(&output).unwrap();
        let mut bytes = original.to_vec();
        let edit = bytes.windows(4).position(|w| w == b"elst").unwrap();
        let duration = u32::from_be_bytes(bytes[edit + 12..edit + 16].try_into().unwrap());
        bytes[edit + 12..edit + 16].copy_from_slice(&(duration + 1).to_be_bytes());
        fs::write(&input, bytes).unwrap();
        assert!(
            decode(&input, &output, &AtomicBool::new(false))
                .unwrap_err()
                .to_string()
                .contains("incomplete M4A presentation")
        );
    }

    #[test]
    fn bounded_and_open_fragment_presentations_preserve_their_sample_counts() {
        let temp = Temp::new();
        let input = temp.0.join("input.m4a");
        let output = temp.0.join("output.wav");
        let pcm = vec![0.1_f32; 4801];
        let flat = syom::encode_with(
            &[&pcm],
            48000,
            &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
        )
        .unwrap();
        for open_end in [false, true] {
            let (mut bytes, coded) = fragmented(&flat, open_end);
            fs::write(&input, &bytes).unwrap();
            decode(&input, &output, &AtomicBool::new(false)).unwrap();
            if open_end {
                let expected = ryf::read_s16(&output).unwrap().1;
                let mut actual = Vec::new();
                decode_resumable(&input, 1600, &AtomicBool::new(false), None, |event| {
                    match event {
                        DecodeEvent::Chunk(_, pcm) => actual.extend_from_slice(pcm),
                        DecodeEvent::Checkpoint(..) => {}
                    }
                    Ok(())
                })
                .unwrap();
                assert_eq!(actual, expected);
            }
            let expected = if open_end { coded } else { pcm.len() as u64 };
            assert_eq!(Wav::open(&output).unwrap().frames, expected.div_ceil(3));
            fs::remove_file(&output).unwrap();
            if !open_end {
                let edit = bytes.windows(4).position(|w| w == b"elst").unwrap();
                bytes[edit + 12..edit + 16].copy_from_slice(&48000_u32.to_be_bytes());
                fs::write(&input, &bytes).unwrap();
                assert!(
                    decode(&input, &output, &AtomicBool::new(false))
                        .unwrap_err()
                        .to_string()
                        .contains("incomplete M4A presentation")
                );
                fs::remove_file(&output).unwrap();
            }
        }
    }

    #[test]
    fn aac_index_beyond_one_million_frames_reaches_decoding_and_can_be_cancelled() {
        let temp = Temp::new();
        let seed = syom::encode_with(
            &[&[0.0_f32; 1024]],
            48000,
            &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
        )
        .unwrap();
        let stsz = seed.windows(4).position(|w| w == b"stsz").unwrap();
        let size = u32::from_be_bytes(seed[stsz + 16..stsz + 20].try_into().unwrap()) as usize;
        let mdat = seed.windows(4).position(|w| w == b"mdat").unwrap();
        let unit = &seed[mdat + 4..mdat + 4 + size];
        let count = (1 << 19) + 1;
        let flat =
            syom::mux_raw_lc_m4a(&vec![unit; count], 48000, 1, (count as u64 - 1) * 1024).unwrap();
        let (mut bytes, _) = fragmented(&flat, true);
        let pos = bytes.windows(4).position(|w| w == b"moof").unwrap() - 4;
        let mut second = bytes[pos..].to_vec();
        let mfhd = second.windows(4).position(|w| w == b"mfhd").unwrap();
        second[mfhd + 8..mfhd + 12].copy_from_slice(&2_u32.to_be_bytes());
        bytes.extend(second);
        let input = temp.0.join("many-frames.m4a");
        let output = temp.0.join("output.wav");
        fs::write(&input, bytes).unwrap();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let marker = output.clone();
        let thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !marker.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            signal.store(true, Ordering::SeqCst);
        });
        let result = decode(&input, &output, &cancelled);
        thread.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("interrupted"));
    }

    #[test]
    fn wav_validation_accepts_rf64_and_rejects_empty_wrong_format_and_truncation() {
        let temp = Temp::new();
        let path = temp.0.join("input.wav");
        let pcm = [0; 128];
        let rf64 = ryf::encode_rf64(ryf::WriteSpec::s16(SAMPLE_RATE, 1), &pcm).unwrap();
        fs::write(&path, &rf64).unwrap();
        assert_eq!(Wav::open(&path).unwrap().frames, 64);
        let riff = ryf::encode_s16(&pcm, SAMPLE_RATE).unwrap();
        for bytes in [
            ryf::encode_s16(&[], SAMPLE_RATE).unwrap(),
            ryf::encode_s16(&pcm, 48000).unwrap(),
            ryf::encode(ryf::WriteSpec::s16(SAMPLE_RATE, 2), &pcm).unwrap(),
            ryf::encode_f32(&[0.0; 64], SAMPLE_RATE, 1).unwrap(),
            riff[..riff.len() - 2].to_vec(),
            rf64[..rf64.len() - 2].to_vec(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(Wav::open(&path).is_err());
        }
    }

    #[test]
    fn rf64_chunking_seeks_beyond_4_gib_and_a_hundred_thousand_chunk_spans() {
        let temp = Temp::new();
        let path = temp.0.join("long.wav");
        let frames = 100_001_u64 * 10 * u64::from(SAMPLE_RATE);
        sparse_test_wav(&path, frames);
        let mut wav = Wav::open(&path).unwrap();
        assert_eq!(wav.frames, frames);
        let output = temp.0.join("last.wav");
        wav.chunk(
            &output,
            frames - u64::from(SAMPLE_RATE),
            u64::from(SAMPLE_RATE),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(Wav::open(&output).unwrap().frames, u64::from(SAMPLE_RATE));
        assert_eq!(
            ryf::read_s16(&output).unwrap().1,
            vec![0; SAMPLE_RATE as usize * 2]
        );
    }

    #[test]
    fn cancellation_and_invalid_spans_do_not_create_or_overwrite_outputs() {
        let temp = Temp::new();
        let source = temp.0.join("source.wav");
        let output = temp.0.join("output.wav");
        ryf::write_s16(&source, &[0; 128], SAMPLE_RATE).unwrap();
        let mut wav = Wav::open(&source).unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(decode(&source, &output, &cancelled).is_err());
        assert!(wav.chunk(&output, 0, 32, &cancelled).is_err());
        assert!(wav.chunk(&output, 64, 1, &AtomicBool::new(false)).is_err());
        assert!(!output.exists());
        fs::write(&output, b"owned by caller").unwrap();
        assert!(wav.chunk(&output, 0, 32, &AtomicBool::new(false)).is_err());
        assert_eq!(fs::read(output).unwrap(), b"owned by caller");
    }
}

#[cfg(test)]
mod qualification;
