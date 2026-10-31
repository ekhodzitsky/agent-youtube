use super::*;
#[cfg(unix)]
fn fixture(frames: u64) -> (PathBuf, Pipeline, Checkpoint, audio::Wav) {
    let root = std::env::temp_dir().join(format!(
        "agent-youtube-long-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pipeline = Pipeline::new(
        Options {
            range: None,
            url: "https://youtu.be/a2hbJlPxj4M".into(),
            work_dir: root.clone(),
            model_dir: root.join("models"),
            model_variant: Default::default(),
            gigastt: "/bin/false".into(),
            chunk_seconds: 10,
            boundary_window_ms: 0,
            asr_timeout_seconds: 1800,
            asr_memory_mib: None,
            encoder_threads: None,
            retry_window_seconds: 0,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let checkpoint = Checkpoint::open(
        &root,
        Config {
            range: None,
            url: pipeline.options.url.clone(),
            model_dir: root.join("models"),
            model_variant: Default::default(),
            model_fingerprint: BTreeMap::new(),
            gigastt_version: "fixture".into(),
            gigastt_binary: "fixture".into(),
            chunk_seconds: 10,
            boundary_window_ms: 0,
            encoder_threads: None,
            engine_environment: BTreeMap::new(),
            media_backend: audio::BACKEND.into(),
        },
    )
    .unwrap();
    let path = root.join("long.wav");
    audio::sparse_test_wav(&path, frames);
    let wav = audio::Wav::open(&path).unwrap();
    (root, pipeline, checkpoint, wav)
}

#[cfg(unix)]
#[test]
fn unverified_teardown_does_not_adopt_staging_outputs() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut pipeline, mut checkpoint, wav) = fixture(160000);
    let engine = root.join("engine");
    fs::write(&engine, b"#!/bin/sh\nprintf '%s' '{\"duration\":10,\"text\":\"keep staging\",\"words\":[]}' > \"$3/000000.json\"\n").unwrap();
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o700)).unwrap();
    pipeline.options.gigastt = engine;
    let mut audio = DecodedAudio {
        frames: wav.frames,
        legacy: Some(wav),
    };
    crate::process::TEST_UNVERIFIED.set(true);
    let result = pipeline.recognize_pending(&pipeline.runner, &mut checkpoint, &mut audio);
    crate::process::TEST_UNVERIFIED.set(false);
    let error = result.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "process_teardown_failed"
    );
    assert!(!checkpoint.completed("raw/000000.json").unwrap());
    let batch = checkpoint.batch().unwrap();
    assert!(batch.dirs(&root).unwrap().1.join("000000.json").exists());
    assert!(root.join("chunks/000000.wav").exists());
    assert!(!root.join("transcript.json").exists());
    // Reopening can safely adopt the staged result once teardown is verified.
    pipeline.adopt_batch(&mut checkpoint, audio.frames).unwrap();
    assert!(checkpoint.completed("raw/000000.json").unwrap());
    drop(audio);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn pending_index_memory_refusal_preserves_media_before_asr() {
    let (root, mut pipeline, mut checkpoint, wav) = fixture(160001);
    let source = storage::digest(&root.join("long.wav")).unwrap();
    let state = fs::read(root.join("state.json")).unwrap();
    let mut audio = DecodedAudio {
        frames: wav.frames,
        legacy: Some(wav),
    };
    let error = crate::memory::with_capacity_for_test(0, || {
        pipeline.recognize_pending(&pipeline.runner, &mut checkpoint, &mut audio)
    })
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "resource_limit"
    );
    assert!(checkpoint.batch().is_none());
    assert_eq!(storage::digest(&root.join("long.wav")).unwrap(), source);
    assert_eq!(fs::read(root.join("state.json")).unwrap(), state);
    assert!(fs::read_dir(root.join("logs")).unwrap().next().is_none());
    assert!(fs::read_dir(root.join("chunks")).unwrap().next().is_none());
    use std::os::unix::fs::PermissionsExt;
    let engine = root.join("engine-retry");
    fs::write(&engine, b"#!/usr/bin/python3\nimport sys,pathlib,wave,json\nfor p in pathlib.Path(sys.argv[2]).glob('*.wav'):\n with wave.open(str(p),'rb') as w: duration=w.getnframes()/w.getframerate()\n (pathlib.Path(sys.argv[3])/(p.stem+'.json')).write_text(json.dumps({'duration':duration,'text':'preserved','words':[]}))\n").unwrap();
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o700)).unwrap();
    pipeline.options.gigastt = engine;
    pipeline
        .recognize_pending(&pipeline.runner, &mut checkpoint, &mut audio)
        .unwrap();
    assert!(checkpoint.completed("raw/000000.json").unwrap());
    assert!(checkpoint.completed("raw/000001.json").unwrap());
    drop(audio);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn recognition_starts_above_the_former_chunk_count_ceiling() {
    let (root, pipeline, mut checkpoint, mut wav) =
        fixture(100_001_u64 * 10 * u64::from(audio::SAMPLE_RATE));
    pipeline
        .prepare_chunks(&mut checkpoint, &mut wav, &[100_000])
        .unwrap();
    let chunk = root.join("chunks/100000.wav");
    assert_eq!(chunk.file_name().unwrap(), "100000.wav");
    assert!(checkpoint.completed("chunks/100000.wav").unwrap());
    // Cross the 32-file publication boundary for both media and raw adoption.
    let indices: Vec<_> = (0..33).collect();
    pipeline
        .prepare_chunks(&mut checkpoint, &mut wav, &indices)
        .unwrap();
    let batch = checkpoint.begin_batch(indices.clone()).unwrap();
    let (_, output) = batch.dirs(&root).unwrap();
    let raw =
        b"{\"duration\":10,\"text\":\" exact ASR text \",\"words\":[],\"extra\":\"preserved\"}";
    for index in &indices {
        fs::write(output.join(format!("{index:06}.json")), raw).unwrap();
    }
    pipeline.adopt_batch(&mut checkpoint, wav.frames).unwrap();
    for index in &indices {
        let name = format!("raw/{index:06}.json");
        assert!(checkpoint.completed(&name).unwrap());
        assert_eq!(fs::read(root.join(name)).unwrap(), raw);
        assert!(
            checkpoint
                .completed(&format!("chunks/{index:06}.wav"))
                .unwrap()
        );
    }
    assert!(!root.join("transcript.json").exists());
    drop(wav);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn disk_preflight_preserves_media_and_resumes_only_missing_chunks() {
    let (root, pipeline, mut checkpoint, mut wav) = fixture(160_001);
    let space = |available_bytes| storage::DiskSpace {
        available_bytes,
        allocation_unit: 4096,
        available_files: None,
    };
    pipeline
        .prepare_chunks_with_space(&mut checkpoint, &mut wav, &[0], |_| Ok(space(323584)))
        .unwrap();
    let state = fs::read(root.join("state.json")).unwrap();
    let chunk = fs::read(root.join("chunks/000000.wav")).unwrap();
    let source_hash = storage::digest(&root.join("long.wav")).unwrap();
    let error = pipeline
        .prepare_chunks_with_space(&mut checkpoint, &mut wav, &[0, 1], |path| {
            assert_eq!(path, root.join("chunks"));
            Ok(space(0))
        })
        .unwrap_err();
    let failure = error.downcast_ref::<crate::protocol::Failure>().unwrap();
    assert_eq!(failure.code, "insufficient_disk_space");
    assert!(error.to_string().contains("require 4096 bytes"));
    assert_eq!(fs::read(root.join("state.json")).unwrap(), state);
    assert_eq!(fs::read(root.join("chunks/000000.wav")).unwrap(), chunk);
    assert_eq!(
        storage::digest(&root.join("long.wav")).unwrap(),
        source_hash
    );
    assert!(!root.join("chunks/000001.wav").exists());
    assert_eq!(fs::read_dir(root.join("chunks")).unwrap().count(), 1);
    assert!(checkpoint.batch().is_none());
    assert!(!root.join("transcript.json").exists());
    let error = pipeline
        .prepare_chunks_with_space(&mut checkpoint, &mut wav, &[1], |_| {
            storage::disk_space(&root.join("missing"))
        })
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "disk_space_unavailable"
    );
    assert_eq!(fs::read(root.join("state.json")).unwrap(), state);
    // Verified existing files require no additional allocation or query.
    pipeline
        .prepare_chunks_with_space(&mut checkpoint, &mut wav, &[0], |_| {
            panic!("cached chunk queried capacity")
        })
        .unwrap();
    pipeline
        .prepare_chunks_with_space(&mut checkpoint, &mut wav, &[0, 1], |_| Ok(space(4096)))
        .unwrap();
    assert!(checkpoint.completed("chunks/000001.wav").unwrap());
    assert_eq!(
        fs::metadata(root.join("chunks/000001.wav")).unwrap().len(),
        46
    );
    assert_eq!(fs::read(root.join("chunks/000000.wav")).unwrap(), chunk);
    drop(wav);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_decode_checks_space_before_decoding_and_resumes_owned_chunks() {
    use std::os::unix::fs::MetadataExt;
    let (root, pipeline, mut checkpoint, wav) = fixture(1);
    drop(wav);
    let source = root.join("input.m4a");
    fs::write(&source, b"not audio").unwrap();
    let space = |available_bytes| storage::DiskSpace {
        available_bytes,
        allocation_unit: 4096,
        available_files: None,
    };
    let error = pipeline
        .extract_audio_with_space(&pipeline.runner, &mut checkpoint, &source, Some(12), |_| {
            Ok(space(0))
        })
        .err()
        .unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "insufficient_disk_space"
    );
    assert!(!root.join("chunks/000000.wav").exists());
    assert!(!root.join("media/audio.json").exists());
    let error = pipeline
        .extract_audio_with_space(
            &pipeline.runner,
            &mut checkpoint,
            &source,
            Some(1_000_000_000_000),
            |_| Ok(space(0)),
        )
        .err()
        .unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "insufficient_disk_space"
    );
    let pcm = vec![0.0; 12 * 16000 + 1];
    fs::write(
        &source,
        syom::encode_with(
            &[&pcm],
            16000,
            &syom::EncodeOptions::default()
                .with_bitrate_bps(64000)
                .with_container(syom::EncodeContainer::M4a),
        )
        .unwrap(),
    )
    .unwrap();
    let original = storage::digest(&source).unwrap();
    let mut calls = 0;
    let error = pipeline
        .extract_audio_with_space(&pipeline.runner, &mut checkpoint, &source, None, |_| {
            calls += 1;
            Ok(space(if calls == 1 { u64::MAX } else { 0 }))
        })
        .err()
        .unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::protocol::Failure>()
            .unwrap()
            .code,
        "insufficient_disk_space"
    );
    assert!(checkpoint.completed("chunks/000000.wav").unwrap());
    assert!(!checkpoint.completed("media/audio.json").unwrap());
    assert!(!root.join("media/audio.wav").exists());
    let inode = fs::metadata(root.join("chunks/000000.wav")).unwrap().ino();
    let snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
    let config: Config = serde_json::from_value(snapshot["config"].clone()).unwrap();
    drop(checkpoint);
    let mut checkpoint = Checkpoint::open(&root, config).unwrap();
    checkpoint.recover().unwrap();
    let decoded = pipeline
        .extract_audio_with_space(&pipeline.runner, &mut checkpoint, &source, Some(12), |_| {
            // Only the missing two-second tail needs allocation.
            Ok(space(65536))
        })
        .unwrap();
    assert_eq!(decoded.frames, 192001);
    assert!(decoded.legacy.is_none());
    assert_eq!(
        fs::metadata(root.join("chunks/000000.wav")).unwrap().ino(),
        inode
    );
    assert!(checkpoint.completed("chunks/000001.wav").unwrap());
    assert!(checkpoint.completed("media/audio.json").unwrap());
    assert_eq!(storage::digest(&source).unwrap(), original);
    let decoded = pipeline
        .extract_audio_with_space(&pipeline.runner, &mut checkpoint, &source, None, |_| {
            panic!("completed decode queried disk")
        })
        .unwrap();
    assert_eq!(decoded.frames, 192001);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn legacy_full_wav_skips_decoding_and_capacity_queries() {
    let (root, pipeline, mut checkpoint, wav) = fixture(160001);
    drop(wav);
    let pending = checkpoint.pending("media/audio.wav").unwrap();
    fs::copy(root.join("long.wav"), &pending).unwrap();
    checkpoint.publish(&pending, "media/audio.wav").unwrap();
    let decoded = pipeline
        .extract_audio_with_space(
            &pipeline.runner,
            &mut checkpoint,
            &root.join("missing-source"),
            Some(12),
            |_| panic!("legacy WAV queried disk"),
        )
        .unwrap();
    assert_eq!(decoded.frames, 160001);
    assert!(decoded.legacy.is_some());
    drop(decoded);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn url_validation_normalizes_only_video_urls() {
    for url in [
        "https://youtu.be/a2hbJlPxj4M",
        "https://www.youtube.com/watch?list=ignored&v=a2hbJlPxj4M&t=7",
        "https://youtube.com/shorts/a2hbJlPxj4M",
    ] {
        assert_eq!(video_id(url).unwrap(), "a2hbJlPxj4M");
    }
    for url in [
        "--exec=evil",
        "file:///etc/passwd",
        "https://youtube.com.evil/watch?v=a2hbJlPxj4M",
        "https://youtube.com/watch?v=../../x",
        "https://youtu.be/a2hbJlPxj4M/evil",
    ] {
        assert!(video_id(url).is_err(), "{url}");
    }
}

#[cfg(unix)]
#[test]
fn legacy_invocation_and_checkpoint_model_selection_defaults_to_e2e() {
    let (root, pipeline, checkpoint, _) = fixture(160000);
    let mut options = serde_json::to_value(&pipeline.options).unwrap();
    options.as_object_mut().unwrap().remove("model_variant");
    let legacy: Options = serde_json::from_value(options).unwrap();
    assert_eq!(legacy.model_variant, ModelVariant::E2eRnnt);
    let state = root.join("state.json");
    drop(checkpoint);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    value["config"]
        .as_object_mut()
        .unwrap()
        .remove("model_variant");
    fs::write(&state, serde_json::to_vec(&value).unwrap()).unwrap();
    Checkpoint::validate_model(&root, ModelVariant::E2eRnnt).unwrap();
    let before = fs::read(&state).unwrap();
    assert!(Checkpoint::validate_model(&root, ModelVariant::MlCtc).is_err());
    assert_eq!(fs::read(&state).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}
