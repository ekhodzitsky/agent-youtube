//! ASR batch preparation, adoption and recognition lifecycle.
use super::*;

impl Pipeline {
    pub(super) fn recognition_plan(
        &self,
        checkpoint: &mut Checkpoint,
        frames: u64,
    ) -> Result<crate::boundaries::Plan> {
        let chunk_frames = u64::from(self.options.chunk_seconds) * u64::from(audio::SAMPLE_RATE);
        let window = self.options.boundary_window_ms;
        let selection = self.options.range.map(|r| r.resolve(frames)).transpose()?;
        let mut plan = crate::boundaries::Plan::selected(frames, chunk_frames, selection)?;
        if window == 0 {
            return Ok(plan);
        }
        let path = self.options.work_dir.join("recognition.json");
        let planned = u64::try_from(plan.count()?)?
            .saturating_add(1)
            .saturating_mul(64)
            .saturating_add(64 * 1024);
        crate::memory::require(planned, &path, "recognition_plan")?;
        if checkpoint.completed("recognition.json")? {
            crate::memory::require(
                fs::metadata(&path)?
                    .len()
                    .saturating_mul(16)
                    .saturating_add(planned),
                &path,
                "recognition_plan",
            )?;
            plan = serde_json::from_str(&storage::read_text(&path, planned)?).map_err(|e| {
                crate::checkpoint::evidence_failure("recognition_plan_invalid", &path, e)
            })?;
        } else {
            for index in 0..frames.div_ceil(chunk_frames) {
                if !checkpoint.completed(&format!("chunks/{index:06}.wav"))? {
                    return Err(crate::checkpoint::evidence_failure(
                        "decoded_chunk_missing",
                        &path,
                        "boundary planning requires complete PCM",
                    ));
                }
            }
            plan.adjust(&self.options.work_dir, window, &self.runner.cancelled)?;
            checkpoint.publish_bytes(&serde_json::to_vec(&plan)?, "recognition.json")?;
        }
        if plan.selection() != selection {
            return Err("recognition range evidence differs from saved request".into());
        }
        plan.validate(frames, chunk_frames, window).map_err(|e| {
            crate::checkpoint::evidence_failure("recognition_plan_invalid", &path, e)
        })?;
        Ok(plan)
    }
    pub(super) fn adopt_batch(&self, checkpoint: &mut Checkpoint, frames: u64) -> Result<()> {
        let Some(batch) = checkpoint.batch() else {
            return Ok(());
        };
        let (_, output) = batch.dirs(&self.options.work_dir)?;
        let plan = self.recognition_plan(checkpoint, frames)?;
        for group in batch.indices.chunks(PUBLICATION_BATCH) {
            let mut ready = Vec::new();
            for index in group {
                let (_, span_frames) = plan.span(*index)?;
                let name = format!("raw/{index:06}.json");
                if checkpoint.verified(&name)? {
                    continue;
                }
                let path = output.join(format!("{index:06}.json"));
                storage::reject_nonregular(&path)?;
                if !path.exists() {
                    continue;
                }
                let span = span_frames as f64 / f64::from(audio::SAMPLE_RATE);
                // A killed batch may leave one incomplete JSON. Preserve it until success;
                // only complete validated results become recognition checkpoints.
                if read_asr_result(&path, span).is_err() {
                    continue;
                }
                ready.push((path, name));
            }
            let names: Vec<_> = ready.iter().map(|(_, name)| name.as_str()).collect();
            let pending = checkpoint.pending_many(&names)?;
            let mut files = Vec::new();
            for (pending, (path, name)) in pending.into_iter().zip(ready) {
                storage::atomic_write(&pending, &fs::read(path)?, false)?;
                files.push((pending, name));
            }
            checkpoint.publish_many(&files)?;
        }
        Ok(())
    }
    pub(super) fn recognize_pending(
        &self,
        runner: &Runner,
        checkpoint: &mut Checkpoint,
        audio: &mut DecodedAudio,
    ) -> Result<()> {
        let plan = self.recognition_plan(checkpoint, audio.frames)?;
        self.adopt_batch(checkpoint, audio.frames)?;
        // Previous failed outputs were adopted above. Media chunks stay owned by
        // the checkpoint; staging is only hard links and private ASR outputs.
        checkpoint.retire_batch()?;
        let chunk_frames = u64::from(self.options.chunk_seconds) * u64::from(audio::SAMPLE_RATE);
        let count = plan.count()?;
        let mut indices = Vec::new();
        for index in 0..count {
            if !checkpoint.verified(&format!("raw/{index:06}.json"))? {
                if indices.len() == indices.capacity() {
                    let additional = indices.capacity().max(8).min(count - indices.len());
                    let bytes = indices
                        .capacity()
                        .saturating_add(additional)
                        .saturating_add(indices.capacity())
                        .saturating_mul(std::mem::size_of::<usize>());
                    let path = self.options.work_dir.join("state.json");
                    crate::memory::require(bytes as u64, &path, "prepare_recognition")?;
                    indices.try_reserve_exact(additional).map_err(|e| {
                        crate::memory::allocation_error(&path, "prepare_recognition", e)
                    })?;
                }
                indices.push(index);
            }
        }
        if indices.is_empty() {
            return Ok(());
        }
        if let Some(wav) = audio.legacy.as_mut() {
            if self.options.range.is_none() {
                self.prepare_chunks(checkpoint, wav, &indices)?;
            } else {
                for index in &indices {
                    let (start, frames) = plan.span(*index)?;
                    for source in start / chunk_frames..=(start + frames - 1) / chunk_frames {
                        self.prepare_chunks(checkpoint, wav, &[usize::try_from(source)?])?;
                    }
                }
            }
        }
        let mut previous_source = None;
        for index in &indices {
            let (start, frames) = plan.span(*index)?;
            for source in start / chunk_frames..=(start + frames - 1) / chunk_frames {
                if previous_source == Some(source) {
                    continue;
                }
                previous_source = Some(source);
                let name = format!("chunks/{source:06}.wav");
                if !checkpoint.completed(&name)? {
                    return Err(crate::checkpoint::evidence_failure(
                        "decoded_chunk_missing",
                        &self.options.work_dir.join(&name),
                        "decoded chunk is missing; media preserved",
                    ));
                }
                let expected = (audio.frames - source * chunk_frames).min(chunk_frames);
                if audio::Wav::open(&self.options.work_dir.join(&name))?.frames != expected {
                    return Err(crate::checkpoint::evidence_failure(
                        "decoded_chunk_invalid",
                        &self.options.work_dir.join(&name),
                        "decoded chunk length mismatch; media preserved",
                    ));
                }
            }
        }
        let space = storage::disk_space_at(&self.options.work_dir, "recognize")?;
        space.require_files(
            &self.options.work_dir,
            (indices.len() as u64).saturating_mul(2).saturating_add(4),
            "recognize",
        )?;
        let batch = checkpoint.begin_batch(indices)?;
        let (input, output) = batch.dirs(&self.options.work_dir)?;
        for index in &batch.indices {
            let (start, frames) = plan.span(*index)?;
            let destination = input.join(format!("{index:06}.wav"));
            if start % chunk_frames == 0 && frames == (audio.frames - start).min(chunk_frames) {
                fs::hard_link(
                    self.options
                        .work_dir
                        .join(format!("chunks/{:06}.wav", start / chunk_frames)),
                    destination,
                )?;
            } else {
                for source in start / chunk_frames..=(start + frames - 1) / chunk_frames {
                    if !checkpoint.completed(&format!("chunks/{source:06}.wav"))? {
                        return Err(crate::checkpoint::evidence_failure(
                            "decoded_chunk_missing",
                            &destination,
                            "ASR boundary source missing",
                        ));
                    }
                }
                let space = storage::disk_space_at(&input, "recognize")?;
                space.require_at(
                    &destination,
                    space.allocation(frames * 2 + 80)?,
                    "recognize",
                )?;
                audio::write_span(
                    &self.options.work_dir,
                    &destination,
                    chunk_frames,
                    start,
                    frames,
                    &runner.cancelled,
                )?;
            }
        }
        let mut args = strings(&["transcribe-batch"]);
        args.push(input.into_os_string());
        args.push(output.clone().into_os_string());
        args.extend(strings(&[
            "--format",
            "json",
            "--offline",
            "--model-variant",
            self.options.model_variant.as_str(),
            "--punctuation",
            "off",
            "--itn",
            "off",
            "--pool-size",
            "1",
            "--retries",
            "0",
            "--log-level",
            "error",
            "--model-dir",
        ]));
        args.push(self.options.model_dir.as_os_str().into());
        if let Some(threads) = self.options.encoder_threads {
            args.extend(["--encoder-intra-threads".into(), threads.to_string().into()]);
        }
        crate::protocol::progress(
            "recognize",
            format!(
                "recognizing {} pending chunks with one reusable engine",
                batch.indices.len()
            ),
            serde_json::json!({"completed_chunks":0,"total_chunks":batch.indices.len(),"durable":false,"scope":"pending_batch"}),
        );
        let mut watch = crate::asr_output::OutputWatch::new(&output)?;
        let mut seen = std::collections::BTreeSet::new();
        let mut checked = std::time::Instant::now();
        let outcome =
            self.command_with_progress(runner, &self.options.gigastt, &args, "recognize", || {
                if checked.elapsed() < std::time::Duration::from_millis(200) {
                    return Ok(false);
                }
                checked = std::time::Instant::now();
                let before = seen.len();
                for path in watch.candidates()? {
                    let index = validate_batch_output(&path, &batch.indices)?;
                    if seen.contains(&index) {
                        continue;
                    }
                    let (_, frames) = plan.span(index)?;
                    let span = frames as f64 / f64::from(audio::SAMPLE_RATE);
                    // Observation only resets the inactivity deadline. Publication
                    // waits for teardown because batch output writes are not atomic.
                    if read_asr_result(&path, span).is_ok() {
                        seen.insert(index);
                    } else {
                        watch.retry(path);
                    }
                }
                if seen.len() > before {
                    crate::protocol::progress("recognize", format!("recognized {}/{} pending chunks", seen.len(), batch.indices.len()),
                        serde_json::json!({"completed_chunks":seen.len(),"total_chunks":batch.indices.len(),"durable":false,"scope":"pending_batch"}));
                }
                Ok(seen.len() > before)
            });
        // Teardown precedes inspection: gigastt writes non-atomically. Never read
        // or publish a result while the child group could still be writing it.
        if outcome
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<crate::protocol::Failure>())
            .is_some_and(|e| e.code == "process_teardown_failed")
        {
            return outcome.map(|_| ());
        }
        self.adopt_batch(checkpoint, audio.frames)?;
        outcome?;
        // One final reconciliation also checks outputs created just before exit.
        for entry in fs::read_dir(&output)? {
            let path = entry?.path();
            validate_batch_output(&path, &batch.indices)?;
        }
        for index in &batch.indices {
            if !checkpoint.verified(&format!("raw/{index:06}.json"))? {
                return Err("batch did not produce every valid ASR result; media preserved".into());
            }
        }
        crate::protocol::progress(
            "recognize",
            "validated recognition outputs",
            serde_json::json!({"completed_chunks":batch.indices.len(),"total_chunks":batch.indices.len(),"durable":true,"scope":"pending_batch"}),
        );
        checkpoint.retire_batch()
    }
    pub(super) fn recognize(
        &self,
        runner: &Runner,
        checkpoint: &mut Checkpoint,
        audio: &mut DecodedAudio,
        header: &Transcript,
    ) -> Result<()> {
        use std::io::Write;
        self.recognize_pending(runner, checkpoint, audio)?;
        let plan = self.recognition_plan(checkpoint, audio.frames)?;
        let rate = u64::from(audio::SAMPLE_RATE);
        let chunk_frames = u64::from(self.options.chunk_seconds) * rate;
        let count = plan.count()?;
        let pending = checkpoint.pending("transcript.json")?;
        {
            let file = storage::private_file().open(&pending)?;
            let mut writer = std::io::BufWriter::new(file);
            let segments = (0..count).map(|index| {
                if runner.cancelled.load(Ordering::SeqCst) {
                    return Err("interrupted; run the same command to resume".into());
                }
                let (start, frames) = plan.span(index)?;
                let offset = start as f64 / rate as f64;
                let span = frames as f64 / rate as f64;
                let (processed,total,scope)=header.range.map(|r|((start-r.start_frame) as f64/rate as f64,(r.end_frame-r.start_frame) as f64/rate as f64,"selection")).unwrap_or((offset,header.duration,"recording"));
                crate::protocol::progress("assemble", format!("[3/4] chunk {}/{} ({processed:.1}s / {total:.1}s)", index+1, count),
                    serde_json::json!({"scope":scope,"completed_chunks":index,"total_chunks":count,"processed_seconds":processed,"total_seconds":total,"durable":true}));
                checkpoint.completed(&format!("raw/{index:06}.json"))?;
                let result = read_asr_result(
                    &self.options.work_dir.join(format!("raw/{index:06}.json")),
                    span,
                )?;
                absolute_segment(header, result, start, frames)
            });
            transcript::write_streaming(&mut writer, header, segments)?;
            writer.flush()?;
        }
        if !checkpoint.completed("coverage.json")? {
            let producer = crate::agent::producer();
            let coverage = crate::agent::Coverage {
                schema_version: header.schema_version,
                transcript_sha256: storage::digest(&pending)?,
                frames: audio.frames,
                range: header.range,
                sample_rate: audio::SAMPLE_RATE,
                chunk_frames,
                chunks: count,
                boundary_window_ms: self.options.boundary_window_ms,
                producer,
            };
            checkpoint.publish_bytes(&serde_json::to_vec(&coverage)?, "coverage.json")?;
        }
        crate::protocol::progress(
            "publish",
            "[4/4] publishing validated transcript.json",
            serde_json::json!({}),
        );
        checkpoint.publish(&pending, "transcript.json")
    }
}

fn read_asr_result(path: &Path, duration: f64) -> Result<GigasttResult> {
    let mut result: GigasttResult = serde_json::from_str(&read_text(path, JSON_LIMIT)?)?;
    result.validate()?;
    if (result.duration - duration).abs() > 0.2 {
        return Err("gigastt duration does not match the audio chunk".into());
    }
    // Preserve raw bytes and actual word times; only the container span is canonical.
    result.duration = duration;
    result.validate()?;
    Ok(result)
}

fn validate_batch_output(path: &Path, indices: &[usize]) -> Result<usize> {
    let index = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or("unexpected ASR batch output")?;
    if path.extension().is_none_or(|ext| ext != "json") || indices.binary_search(&index).is_err() {
        return Err("unexpected ASR batch output".into());
    }
    storage::reject_nonregular(path)?;
    if fs::metadata(path)?.len() > JSON_LIMIT {
        return Err(crate::protocol::failure(
            "asr_result_limit",
            Some(false),
            "ASR result exceeded 16 MiB for one chunk",
        ));
    }
    Ok(index)
}

// Convert a validated chunk to the recording timeline using its sample span.
fn absolute_segment(
    header: &Transcript,
    result: GigasttResult,
    start: u64,
    frames: u64,
) -> Result<transcript::Segment> {
    let rate = f64::from(audio::SAMPLE_RATE);
    let offset = start as f64 / rate;
    let end = start.checked_add(frames).ok_or("sample span overflow")? as f64 / rate;
    if result.duration != frames as f64 / rate {
        return Err("recognition duration differs from sample span".into());
    }
    let mut segment = Transcript::from_chunks(
        header.source.clone(),
        header.engine.clone(),
        vec![(offset, result)],
    )?
    .segments
    .pop()
    .ok_or("recognizer produced no segment")?;
    // Relative times were validated before addition. Rounded float addition can
    // exceed the exact sample-derived endpoint by one ULP; never extend the span.
    for word in &mut segment.words {
        word.start = word.start.min(end);
        word.end = word.end.min(end);
    }
    segment.start = segment.start.min(end);
    segment.end = if segment.words.is_empty() {
        end
    } else {
        segment.end.min(end)
    };
    Ok(segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_sample_span_does_not_round_beyond_recording_end() {
        let mut header = transcript::tests::sample();
        header.duration = 70.02;
        let start = 1_108_160; // 69.26 seconds.
        let frames = 12_160; // 0.76 seconds, whose float sum exceeds 70.02.
        for words in [
            vec![],
            vec![transcript::Word {
                word: "word".into(),
                start: 0.0,
                end: 0.76,
                confidence: None,
                speaker: None,
            }],
        ] {
            let result = GigasttResult {
                duration: 0.76,
                text: "word".into(),
                words,
            };
            let segment = absolute_segment(&header, result, start, frames).unwrap();
            segment.validate(0.0, header.duration).unwrap();
            assert_eq!(segment.end, header.duration);
        }
        let invalid = GigasttResult {
            duration: 0.76,
            text: "invalid".into(),
            words: vec![transcript::Word {
                word: "invalid".into(),
                start: 0.0,
                end: 0.761,
                confidence: None,
                speaker: None,
            }],
        };
        assert!(absolute_segment(&header, invalid, start, frames).is_err());
        assert!(
            absolute_segment(
                &header,
                GigasttResult {
                    duration: 0.77,
                    text: String::new(),
                    words: vec![],
                },
                start,
                frames
            )
            .is_err()
        );
    }
}
