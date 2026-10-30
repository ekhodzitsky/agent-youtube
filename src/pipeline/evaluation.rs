//! Opt-in reference qualification reuses the production ASR/checkpoint lifecycle.
use super::*;
use serde_json::{Value, json};

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn edits<T: PartialEq>(reference: &[T], hypothesis: &[T]) -> usize {
    let mut row: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, word) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, other) in hypothesis.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (diagonal + usize::from(word != other))
                .min(row[j] + 1)
                .min(old + 1);
            diagonal = old;
        }
    }
    row[hypothesis.len()]
}
fn metrics(reference: &str, hypothesis: &str) -> Value {
    let r = normalize(reference);
    let h = normalize(hypothesis);
    let rc: Vec<_> = r.concat().chars().collect();
    let hc: Vec<_> = h.concat().chars().collect();
    let we = edits(&r, &h);
    let ce = edits(&rc, &hc);
    json!({"reference_words":r.len(),"hypothesis_words":h.len(),"word_errors":we,
        "wer":if r.is_empty() {None} else {Some(we as f64/r.len() as f64)},
        "reference_characters":rc.len(),"character_errors":ce,
        "cer":if rc.is_empty() {None} else {Some(ce as f64/rc.len() as f64)},
        "silence_insertions":if r.is_empty() {h.len()} else {0}})
}
#[test]
fn reference_metrics_count_substitutions_insertions_deletions_and_silence() {
    assert_eq!(normalize("Ёж, ДВА!"), vec!["еж", "два"]);
    assert_eq!(edits(&[1, 2, 3], &[1, 4, 3, 5]), 2);
    assert_eq!(edits(&[1, 2, 3], &[1]), 2);
    assert_eq!(metrics("", "лишнее")["silence_insertions"], 1);
    assert_eq!(metrics("", "")["wer"], Value::Null);
    assert_eq!(metrics("Привет, мир!", "привет мир")["wer"], 0.0);
}

fn trial(
    root: &Path,
    model_dir: &Path,
    gigastt: &Path,
    pcm: &[u8],
    reference: &str,
    mode: &str,
) -> Result<Value> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut pipeline = Pipeline::new(
        Options {
            url: "https://youtu.be/a2hbJlPxj4M".into(),
            work_dir: root.into(),
            model_dir: model_dir.into(),
            model_variant: Default::default(),
            gigastt: gigastt.into(),
            chunk_seconds: if mode == "full" { 600 } else { 10 },
            boundary_window_ms: if mode == "quiet" { 1000 } else { 0 },
            asr_timeout_seconds: 120,
            asr_memory_mib: Some(1024),
            encoder_threads: Some(2),
            retry_window_seconds: 0,
        },
        cancelled.clone(),
    )?;
    let mut lock = storage::Lock::acquire(root)?;
    pipeline.runner = pipeline.runner.with_lock(lock.child_handle()?);
    let version = pipeline.doctor()?;
    let pcm_sha256 = format!("{:x}", Sha256::digest(pcm));
    let config = pipeline.config(
        format!("https://huggingface.co/datasets/google/fleurs#pcm-sha256={pcm_sha256}"),
        version.clone(),
    )?;
    let configuration = serde_json::to_value(&config)?;
    let mut checkpoint = Checkpoint::open(root, config)?;
    checkpoint.recover()?;
    let frames = pcm.len() as u64 / 2;
    // This fixture starts from verified PCM rather than a downloaded container.
    // Supply its known duration through the same evidence contract as acquisition.
    let duration = frames.div_ceil(16000);
    for (name, evidence) in [
        ("source.json", json!({"duration": duration})),
        ("download.json", json!({"duration_seconds": duration})),
    ] {
        if !checkpoint.completed(name)? {
            checkpoint.publish_bytes(&serde_json::to_vec(&evidence)?, name)?;
        }
    }
    let chunk_frames = u64::from(pipeline.options.chunk_seconds) * 16000;
    let canonical = root.join("transcript.json");
    if !checkpoint.completed("transcript.json")? {
        for (index, bytes) in pcm.chunks(chunk_frames as usize * 2).enumerate() {
            let name = format!("chunks/{index:06}.wav");
            if !checkpoint.completed(&name)? {
                let path = checkpoint.pending(&name)?;
                audio::write_chunk(&path, bytes)?;
                checkpoint.publish(&path, &name)?;
            }
        }
        let header = Transcript {
            schema_version: 1,
            source: Source {
                url: "https://huggingface.co/datasets/google/fleurs".into(),
                video_id: root
                    .file_name()
                    .ok_or("missing trial name")?
                    .to_string_lossy()
                    .into(),
                title: Some("FLEURS reference qualification".into()),
            },
            language: "ru".into(),
            duration: frames as f64 / 16000.0,
            engine: Engine {
                name: "gigastt".into(),
                version,
                model_variant: "e2e_rnnt".into(),
            },
            segments: Vec::new(),
        };
        let result = pipeline.recognize(
            &pipeline.runner,
            &mut checkpoint,
            &mut DecodedAudio {
                frames,
                legacy: None,
            },
            &header,
        );
        if result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<crate::protocol::Failure>())
            .is_some_and(|e| e.code == "process_teardown_failed")
        {
            lock.leave_locked_for_child();
        }
        result?;
    }
    checkpoint.completed("transcript.json")?;
    checkpoint.ensure_manifest(&cancelled)?;
    let mut compact = Vec::new();
    crate::agent::write_view(&mut compact, &canonical, false, &cancelled)?;
    storage::atomic_write(&root.join("compact.json"), &compact, true)?;
    let compact: Value = serde_json::from_slice(&compact)?;
    let manifest: Value = serde_json::from_slice(&fs::read(root.join("manifest.json"))?)?;
    let chunks = manifest["chunks"]
        .as_array()
        .ok_or("missing chunk evidence")?;
    let passages = compact["passages"].as_array().ok_or("missing passages")?;
    assert_eq!(compact["accuracy"], "unverified");
    assert_eq!(compact["timing"], "asr_estimates");
    for index in 0..frames.div_ceil(chunk_frames) {
        checkpoint.completed(&format!("raw/{index:06}.json"))?;
    }
    let mut text = String::new();
    let mut index = 0;
    let mut end_frame = 0;
    transcript::read_streaming(std::io::BufReader::new(fs::File::open(&canonical)?), |s| {
        let evidence = &chunks[index];
        let path = root.join(evidence["path"].as_str().ok_or("missing raw path")?);
        assert_eq!(storage::digest(&path)?, evidence["sha256"]);
        let raw: transcript::GigasttResult = serde_json::from_slice(&fs::read(path)?)?;
        assert_eq!(s.text, raw.text);
        assert_eq!(evidence["input_start_frame"].as_u64(), Some(end_frame));
        let offset = end_frame as f64 / 16000.0;
        end_frame = evidence["input_end_frame"]
            .as_u64()
            .ok_or("missing end frame")?;
        assert_eq!(s.words.len(), raw.words.len());
        for (word, original) in s.words.iter().zip(&raw.words) {
            assert_eq!(word.word, original.word);
            assert_eq!(word.confidence, original.confidence);
            assert_eq!(word.speaker, original.speaker);
            assert!((word.start - (original.start + offset)).abs() < 1e-9);
            assert!((word.end - (original.end + offset)).abs() < 1e-9);
        }
        let projected: String = passages
            .iter()
            .filter(|p| p["asr_chunk"].as_u64() == Some(index as u64))
            .map(|p| p["text"].as_str().unwrap())
            .collect();
        assert_eq!(projected, s.text);
        index += 1;
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&s.text);
        Ok(())
    })?;
    assert_eq!(index, chunks.len());
    assert_eq!(end_frame, frames);
    let plan = if pipeline.options.boundary_window_ms == 0 {
        Value::Null
    } else {
        serde_json::from_str(&storage::read_text(&root.join("recognition.json"), 16384)?)?
    };
    let result = json!({"mode":mode,"frames":frames,"pcm_sha256":pcm_sha256,"reference_sha256":format!("{:x}",Sha256::digest(reference.as_bytes())),"metrics":metrics(reference,&text),"recognition_plan":plan,"transcript":canonical,
        "configuration":configuration});
    storage::atomic_write(
        &root.join("quality.json"),
        &serde_json::to_vec_pretty(&result)?,
        true,
    )?;
    // Same production rule: validated durable transcript before owned media cleanup.
    checkpoint.cleanup_media()?;
    Ok(result)
}
#[test]
#[ignore = "requires explicit FLEURS reference audio, real GigaSTT and model paths; writes JSON qualification evidence"]
fn reference_corpus_quality() -> Result<()> {
    let sources = PathBuf::from(
        std::env::var_os("AGENT_YOUTUBE_REFERENCE_AUDIO")
            .ok_or("set AGENT_YOUTUBE_REFERENCE_AUDIO")?,
    );
    let root = PathBuf::from(
        std::env::var_os("AGENT_YOUTUBE_QUALITY_OUTPUT")
            .ok_or("set AGENT_YOUTUBE_QUALITY_OUTPUT")?,
    );
    storage::private_directory(&root, true)?;
    let model_dir = PathBuf::from(
        std::env::var_os("AGENT_YOUTUBE_QUALITY_MODELS")
            .ok_or("set AGENT_YOUTUBE_QUALITY_MODELS")?,
    );
    let gigastt = PathBuf::from(
        std::env::var_os("AGENT_YOUTUBE_QUALITY_GIGASTT")
            .ok_or("set AGENT_YOUTUBE_QUALITY_GIGASTT")?,
    );
    let manifest: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/asr-references.json"))?;
    let cases = manifest["cases"].as_array().ok_or("missing references")?;
    let mut results = Vec::new();
    let mut first_pcm = None;
    let mut first_reference = None;
    let mut sequential_pcm = Vec::new();
    let mut paused_pcm = Vec::new();
    let mut sequential_reference = String::new();
    for case in cases {
        let name = case["file"].as_str().ok_or("missing filename")?;
        if Path::new(name).components().count() != 1 {
            return Err("invalid reference filename".into());
        }
        let input = sources.join(name);
        if storage::digest(&input)? != case["sha256"].as_str().ok_or("missing source hash")? {
            return Err("reference audio digest mismatch".into());
        }
        let (rate, samples) = ryf::read_f32(&input)?;
        if rate != 16000 || Some(samples.len() as u64) != case["samples"].as_u64() {
            return Err("reference sample span mismatch".into());
        }
        let pcm = ryf::f32_to_s16le(&samples);
        let reference = case["reference"].as_str().ok_or("missing reference text")?;
        if !sequential_reference.is_empty() {
            sequential_reference.push(' ');
            paused_pcm.resize(paused_pcm.len() + 2 * 16000 * 2, 0);
        }
        sequential_reference.push_str(reference);
        sequential_pcm.extend_from_slice(&pcm);
        paused_pcm.extend_from_slice(&pcm);
        if first_pcm.is_none() {
            first_pcm = Some(pcm.clone());
            first_reference = Some(reference.to_owned());
        }
        for mode in ["full", "fixed", "quiet"] {
            let work = root.join(format!(
                "{}-{mode}",
                case["id"].as_str().ok_or("missing case id")?
            ));
            results.push(trial(&work, &model_dir, &gigastt, &pcm, reference, mode)?);
        }
    }
    let pcm = first_pcm.ok_or("empty corpus")?;
    let mut samples = ryf::s16le_to_f32(&pcm)?;
    // Explicit synthetic stress case, not a recording of real environmental noise.
    let mut seed = 42u32;
    for sample in &mut samples {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *sample =
            (*sample + (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.003).clamp(-1.0, 1.0);
    }
    results.push(trial(
        &root.join("synthetic-noise-full"),
        &model_dir,
        &gigastt,
        &ryf::f32_to_s16le(&samples),
        first_reference.as_deref().ok_or("missing reference")?,
        "full",
    )?);
    results.push(trial(
        &root.join("silence-full"),
        &model_dir,
        &gigastt,
        &vec![0; 16000 * 2 * 10],
        "",
        "full",
    )?);
    // Synthetic concatenation preserves published text order. It is not dialogue,
    // overlap, diarization ground truth, or a recording of environmental noise.
    let sequential_samples = ryf::s16le_to_f32(&sequential_pcm)?;
    let quiet_pcm = ryf::f32_to_s16le(
        &sequential_samples
            .iter()
            .map(|sample| sample * 0.1)
            .collect::<Vec<_>>(),
    );
    let mut noise_seed = 42u32;
    let noisy_pcm = ryf::f32_to_s16le(
        &sequential_samples
            .iter()
            .map(|sample| {
                noise_seed = noise_seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (sample + (noise_seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.003)
                    .clamp(-1.0, 1.0)
            })
            .collect::<Vec<_>>(),
    );
    for (condition, input) in [
        ("sequential-speakers", &sequential_pcm),
        ("sequential-pauses", &paused_pcm),
        ("sequential-quiet", &quiet_pcm),
        ("sequential-noise", &noisy_pcm),
    ] {
        for mode in ["full", "fixed", "quiet"] {
            results.push(trial(
                &root.join(format!("{condition}-{mode}")),
                &model_dir,
                &gigastt,
                input,
                &sequential_reference,
                mode,
            )?);
        }
    }
    let report = json!({"schema_version":1,"corpus":manifest,"results":results,
        "derived_conditions":{"sequential_speakers":"five published recordings concatenated in manifest order, not conversation",
            "sequential_pauses":{"silence_between_recordings_seconds":2},
            "sequential_quiet":{"linear_gain":0.1,"nominal_attenuation_db":-20},
            "sequential_noise":{"seed":42,"uniform_peak_amplitude":0.003,"clipped_to_unit_range":true}},
        "limits":["Small read-speech qualification, not a general accuracy guarantee", "Published prompts are not independently corrected here", "Orthographic WER penalizes digit/word and proper-name differences", "Synthetic noise is not representative real-world noise; music and spontaneous multilingual conversations remain unqualified"]});
    storage::atomic_write(
        &root.join("quality.json"),
        &serde_json::to_vec_pretty(&report)?,
        true,
    )?;
    Ok(())
}
