//! Opt-in process-restart probe; all inputs and outputs are owned by the runner.
use super::*;
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::PathBuf};

#[test]
#[ignore = "isolated codec migration probe; use tools/qualify_codec_migration.py"]
fn migration_probe() {
    let root = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CODEC_PROBE").unwrap());
    let mode = std::env::var("AGENT_YOUTUBE_CODEC_MODE").unwrap();
    if mode == "generate" {
        fs::create_dir_all(&root).unwrap();
        for rate in [8000_u32, 16000, 22050, 24000, 32000, 44100, 48000] {
            let input: Vec<_> = (0..rate * 2 + 13)
                .map(|i| 0.2 * (i as f32 * 0.19).sin())
                .collect();
            let right: Vec<_> = input.iter().map(|x| x * 0.7).collect();
            fs::write(
                root.join(format!("{rate}.m4a")),
                syom::encode_with(
                    &[&input, &right],
                    rate,
                    &syom::EncodeOptions::default()
                        .with_bitrate_bps(32000)
                        .with_container(syom::EncodeContainer::M4a),
                )
                .unwrap(),
            )
            .unwrap();
        }
        // Repeat an encoded packet: bounded generation memory, deterministic input.
        let seed = syom::encode_with(
            &[&[0.0; 1024]],
            48000,
            &syom::EncodeOptions::default().with_container(syom::EncodeContainer::M4a),
        )
        .unwrap();
        let stsz = seed.windows(4).position(|w| w == b"stsz").unwrap();
        let length = u32::from_be_bytes(seed[stsz + 16..stsz + 20].try_into().unwrap()) as usize;
        let mdat = seed.windows(4).position(|w| w == b"mdat").unwrap();
        let count = 600 * 48000 / 1024 + 1;
        fs::write(
            root.join("long.m4a"),
            syom::mux_raw_lc_m4a(
                &vec![&seed[mdat + 4..mdat + 4 + length]; count],
                48000,
                1,
                (count as u64 - 1) * 1024,
            )
            .unwrap(),
        )
        .unwrap();
        return;
    }
    let source = PathBuf::from(std::env::var_os("AGENT_YOUTUBE_CODEC_SOURCE").unwrap());
    fs::create_dir_all(&root).unwrap();
    let output = root.join(if mode == "full" {
        "full.pcm"
    } else {
        "resumed.pcm"
    });
    let state_path = root.join("resume.json");
    let mut sink = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(mode != "resume")
        .append(mode == "resume")
        .open(&output)
        .unwrap();
    let mut hash = Sha256::new();
    if mode == "resume" {
        std::io::copy(&mut File::open(&output).unwrap(), &mut hash).unwrap();
    }
    let resume = (mode == "resume")
        .then(|| serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap());
    let now = std::time::Instant::now();
    let result = decode_resumable(&source, 16000, &AtomicBool::new(false), resume, |event| {
        match event {
            DecodeEvent::Chunk(_, pcm) => {
                sink.write_all(pcm)?;
                hash.update(pcm);
            }
            DecodeEvent::Checkpoint(_, state) if mode == "interrupt" => {
                sink.sync_all()?;
                storage::atomic_write(&state_path, &serde_json::to_vec(state)?, false)?;
                return Err("migration probe interruption".into());
            }
            _ => {}
        }
        Ok(())
    });
    let frames = if mode == "interrupt" {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("migration probe interruption")
        );
        None
    } else {
        Some(result.unwrap())
    };
    sink.sync_all().unwrap();
    let bytes = sink.metadata().unwrap().len();
    if let Some(frames) = frames {
        assert_eq!(bytes, frames * 2);
    }
    let report = serde_json::json!({"backend":BACKEND,"mode":mode,"frames":frames,
        "bytes":bytes,"sha256":format!("{:x}",hash.finalize()),"elapsed_ms":now.elapsed().as_millis()});
    fs::write(
        root.join(format!("{mode}.json")),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
