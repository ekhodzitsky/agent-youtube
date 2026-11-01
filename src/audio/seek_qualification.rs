//! Executable qualification of bounded-preroll seeking, not a production path.
use super::*;

struct Window {
    native: Vec<f32>,
    pcm: Vec<u8>,
    read_bytes: u64,
    elapsed_seconds: f64,
}

struct MeasuredFile {
    file: File,
    bytes: std::rc::Rc<std::cell::Cell<u64>>,
}
impl Read for MeasuredFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read(buffer)?;
        self.bytes.set(self.bytes.get() + count as u64);
        Ok(count)
    }
}
impl Seek for MeasuredFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

fn append_window<T: Copy>(output: &mut Vec<T>, block: &[T], position: u64, start: u64, end: u64) {
    let first = start.saturating_sub(position).min(block.len() as u64) as usize;
    let last = end.saturating_sub(position).min(block.len() as u64) as usize;
    output.extend_from_slice(&block[first..last]);
}

// Stream preceding samples away. Memory grows with the selected window, not its
// position in the recording. All origins lie on the global resampler phase grid.
fn window(path: &Path, origin: u64, start_ms: u64, length_ms: u64) -> Window {
    let begin = std::time::Instant::now();
    let bytes = std::rc::Rc::new(std::cell::Cell::new(0));
    let reader = MeasuredFile {
        file: File::open(path).unwrap(),
        bytes: bytes.clone(),
    };
    let mut codec = syom::M4aSeek::open(reader, decode_options(path).unwrap()).unwrap();
    let rate = u64::from(codec.output_sample_rate());
    assert_eq!(origin * 16000 % rate, 0);
    codec.seek(i64::try_from(origin).unwrap()).unwrap();
    let native_start = start_ms * rate / 1000;
    let native_end = (start_ms + length_ms) * rate / 1000;
    let pcm_start = start_ms * 16;
    let pcm_end = (start_ms + length_ms) * 16;
    assert!(origin <= native_start);
    let mut native = Vec::new();
    let mut selected = Vec::new();
    let mut native_at = origin;
    let mut pcm_at = origin * 16000 / rate;
    let mut converted = Vec::with_capacity(65536);
    let mut stream = bandlimit::Stream::new(
        bandlimit::Rate::new(rate as i64).unwrap(),
        bandlimit::Rate::new(16000).unwrap(),
    )
    .unwrap();
    let mut complete = false;
    let result = codec.decode(|frame| {
        assert_eq!(u64::from(frame.sample_rate), rate);
        assert_eq!(frame.planar.len(), 1);
        append_window(
            &mut native,
            frame.planar[0],
            native_at,
            native_start,
            native_end,
        );
        native_at += frame.samples as u64;
        converted.clear();
        stream.write(frame.planar[0], &mut converted).unwrap();
        append_window(&mut selected, &converted, pcm_at, pcm_start, pcm_end);
        pcm_at += converted.len() as u64;
        if native_at >= native_end && pcm_at >= pcm_end {
            complete = true;
            return Err(syom::AacError::Io(io::Error::other(
                "qualification window complete",
            )));
        }
        Ok(())
    });
    assert!(complete && matches!(result, Err(syom::AacError::Io(_))));
    assert_eq!(native.len() as u64, native_end - native_start);
    assert_eq!(selected.len() as u64, pcm_end - pcm_start);
    Window {
        native,
        pcm: ryf::f32_to_s16le(&selected),
        read_bytes: bytes.get(),
        elapsed_seconds: begin.elapsed().as_secs_f64(),
    }
}

fn compare(reference: &Window, candidate: &Window) -> serde_json::Value {
    assert_eq!(reference.native.len(), candidate.native.len());
    assert_eq!(reference.pcm.len(), candidate.pcm.len());
    serde_json::json!({
        "native_samples":reference.native.len(),
        "reference_read_bytes":reference.read_bytes,
        "candidate_read_bytes":candidate.read_bytes,
        "reference_seconds":reference.elapsed_seconds,
        "candidate_seconds":candidate.elapsed_seconds,
        "native_differences":reference.native.iter().zip(&candidate.native).filter(|(a,b)|a.to_bits()!=b.to_bits()).count(),
        "pcm_samples":reference.pcm.len()/2,
        "pcm_differences":reference.pcm.as_chunks::<2>().0.iter().zip(candidate.pcm.as_chunks::<2>().0).filter(|(a,b)|a!=b).count()
    })
}

#[test]
fn bounded_preroll_does_not_reconstruct_pns_history() {
    let temp = super::tests::Temp::new();
    for rate in [16000u32, 44100] {
        for pns in [false, true] {
            let mut noise = 42u32;
            let samples: Vec<_> = (0..rate * 6)
                .map(|i| {
                    noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
                    0.2 * (i as f32 * 0.17).sin() + 0.2 * (noise as i32 as f32 / i32::MAX as f32)
                })
                .collect();
            let encoded = syom::encode_with(
                &[&samples],
                rate,
                &syom::EncodeOptions::default()
                    .with_bitrate_bps(64000)
                    .with_pns(pns)
                    .with_container(syom::EncodeContainer::M4a),
            )
            .unwrap();
            let path = temp.0.join("seek.m4a");
            std::fs::write(&path, encoded).unwrap();
            let reference = window(&path, 0, 4001, 500);
            let candidate = window(&path, u64::from(rate) * 3, 4001, 500);
            let comparison = compare(&reference, &candidate);
            eprintln!(
                "seek_fixture {}",
                serde_json::json!({"rate":rate,"pns":pns,"comparison":comparison})
            );
            if pns {
                assert!(comparison["native_differences"].as_u64().unwrap() > 0);
                assert!(comparison["pcm_differences"].as_u64().unwrap() > 0);
            } else {
                assert_eq!(comparison["native_differences"], 0);
                assert_eq!(comparison["pcm_differences"], 0);
            }
        }
    }
}

#[test]
fn resampler_restart_requires_the_global_rational_phase() {
    for rate in [44100u64, 48000] {
        let input: Vec<_> = (0..rate * 6)
            .map(|i| 0.2 * (i as f32 * 0.173).sin())
            .collect();
        let resampled = |origin: u64| {
            let mut stream = bandlimit::Stream::new(
                bandlimit::Rate::new(rate as i64).unwrap(),
                bandlimit::Rate::new(16000).unwrap(),
            )
            .unwrap();
            let mut at = origin * 16000 / rate;
            let mut selected = Vec::new();
            let mut converted = Vec::with_capacity(65536);
            for block in input[origin as usize..].chunks(1024) {
                converted.clear();
                stream.write(block, &mut converted).unwrap();
                append_window(&mut selected, &converted, at, 64016, 72016);
                at += converted.len() as u64;
            }
            assert_eq!(selected.len(), 8000);
            ryf::f32_to_s16le(&selected)
        };
        let reference = resampled(0);
        assert_eq!(reference, resampled(3 * rate));
        assert_ne!(reference, resampled(3 * rate + 1));
    }
}

#[test]
#[ignore = "compare bounded seek preroll with a complete saved real recording"]
fn real_interval_seek_qualification() {
    let path = std::path::PathBuf::from(std::env::var_os("AGENT_YOUTUBE_SEEK_SOURCE").unwrap());
    let start_ms: u64 = std::env::var("AGENT_YOUTUBE_SEEK_START_MS")
        .unwrap()
        .parse()
        .unwrap();
    assert!(start_ms >= 5000);
    let codec =
        syom::M4aSeek::open(File::open(&path).unwrap(), decode_options(&path).unwrap()).unwrap();
    let rate = u64::from(codec.output_sample_rate());
    drop(codec);
    let reference = window(&path, 0, start_ms, 1000);
    for preroll_seconds in [1, 4] {
        let origin = (start_ms / 1000 - preroll_seconds) * rate;
        let candidate = window(&path, origin, start_ms, 1000);
        eprintln!(
            "real_seek_comparison {}",
            serde_json::json!({
                "rate":rate,"start_ms":start_ms,"preroll_seconds":preroll_seconds,
                "comparison":compare(&reference,&candidate)
            })
        );
    }
}
