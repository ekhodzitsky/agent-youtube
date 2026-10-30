//! Speech grid of two unshipped Kaiser lengths. The builders and the scalar
//! sample path are the production ones. Nothing here changes the shipped
//! tap counts.

use crate::coefficients::{PHASES, Table, phase_count, speech_kaiser};
use crate::oneshot::scalar_after_delay;
use crate::{Rate, Ratio, convert, group_delay, output_len};

const WINDOW_START: usize = 1_600;
const WINDOW_LEN: usize = 16_000;
const RIPPLE_GATE_DB: f64 = 0.01;
const OUTPUT_HZ: i64 = 16_000;

struct Design {
    input_hz: i64,
    taps: u32,
    atten_db: f64,
    alias_gate_db: f64,
    frames: usize,
}

fn rate(hz: i64) -> Rate {
    match Rate::new(hz) {
        Ok(rate) => rate,
        Err(err) => panic!("{hz} Hz is a positive rate, got {err:?}"),
    }
}

fn fc_for(input_hz: i64) -> f64 {
    match input_hz {
        22_050 => 7_500.0 / 22_050.0,
        44_100 => 7_500.0 / 44_100.0,
        _ => panic!("{input_hz} Hz is not a gate-length rate"),
    }
}

fn assert_same_rows(built: &Table, shipped: &Table, label: &str) {
    assert_eq!(built.taps, shipped.taps, "{label} taps");
    assert_eq!(built.phases(), shipped.phases(), "{label} phases");
    for phase in 0..shipped.phases() {
        let left = match built.row(phase) {
            Ok(row) => row,
            Err(err) => panic!("{label} phase {phase}: {err:?}"),
        };
        let right = match shipped.row(phase) {
            Ok(row) => row,
            Err(err) => panic!("{label} shipped phase {phase}: {err:?}"),
        };
        assert_eq!(left.len(), right.len(), "{label} phase {phase} length");
        for (index, (got, expected)) in left.iter().zip(right).enumerate() {
            assert_eq!(
                got.to_bits(),
                expected.to_bits(),
                "{label} phase {phase} tap {index}"
            );
        }
    }
}

fn short_input(frames: usize) -> Vec<f32> {
    let mut input = Vec::new();
    if input.try_reserve_exact(frames).is_err() {
        panic!("short input reserve {frames}");
    }
    for index in 0..frames {
        input.push((index as f32 + 1.0) * 0.001);
    }
    input
}

fn assert_short_matches_convert(input_hz: i64, shipped_taps: u32, outputs: usize) {
    let from = rate(input_hz);
    let to = rate(OUTPUT_HZ);
    // 441 input frames divide both reduced ratios: 320 outputs at 22.05 kHz, 160 at 44.1 kHz.
    let input = short_input(441);
    let ratio = Ratio::reduce(from, to);
    let phases = phase_count(ratio.numerator(), ratio.denominator());
    let built = match speech_kaiser(fc_for(input_hz), 160.0, shipped_taps, phases) {
        Ok(table) => table,
        Err(err) => panic!("{input_hz} builder: {err:?}"),
    };
    let shipped = match Table::new(from, to) {
        Ok(table) => table,
        Err(err) => panic!("{input_hz} shipped table: {err:?}"),
    };
    assert_same_rows(&built, &shipped, &format!("{input_hz}"));
    let got = match scalar_after_delay(&input, &built, from, to, 0, outputs) {
        Ok(got) => got,
        Err(err) => panic!("{input_hz} scalar: {err:?}"),
    };
    let expected = match convert(&input, from, to) {
        Ok(expected) => expected,
        Err(err) => panic!("{input_hz} convert: {err:?}"),
    };
    assert_eq!(got.len(), outputs, "{input_hz} scalar length");
    assert_eq!(expected.len(), outputs, "{input_hz} convert length");
    for (index, (got, expected)) in got.iter().zip(&expected).enumerate() {
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "{input_hz} sample {index}"
        );
    }
}

fn assert_ideal_length(design: &Design) {
    let ratio = Ratio::reduce(rate(design.input_hz), rate(OUTPUT_HZ));
    let frames = match u64::try_from(design.frames) {
        Ok(frames) => frames,
        Err(_) => panic!("{} frames do not fit", design.input_hz),
    };
    let raw = match output_len(design.taps, PHASES, ratio, frames) {
        Ok(raw) => raw,
        Err(err) => panic!("{} length: {err:?}", design.input_hz),
    };
    let delay = match group_delay(design.taps, PHASES, ratio) {
        Ok(delay) => delay,
        Err(err) => panic!("{} delay: {err:?}", design.input_hz),
    };
    let ideal = match raw.checked_sub(delay) {
        Some(ideal) => ideal,
        None => panic!("{} delay {delay} exceeds raw {raw}", design.input_hz),
    };
    assert_eq!(ideal, 32_000, "{} ideal outputs", design.input_hz);
    let window_end = match u64::try_from(WINDOW_START + WINDOW_LEN) {
        Ok(end) => end,
        Err(_) => panic!("window does not fit in u64"),
    };
    assert!(
        delay + window_end <= raw,
        "{} window ends past the raw buffer",
        design.input_hz
    );
}

fn tone(input_hz: i64, frames: usize, freq_hz: u32) -> Vec<f32> {
    let mut input = Vec::new();
    if input.try_reserve_exact(frames).is_err() {
        panic!("{input_hz} Hz tone reserve");
    }
    for n in 0..frames {
        let phase = std::f64::consts::TAU * f64::from(freq_hz) * n as f64 / input_hz as f64;
        input.push((phase.sin() as f32) * 0.5);
    }
    input
}

fn dft_amplitude(window: &[f32], freq_hz: u32) -> f64 {
    let analysis_hz = if freq_hz > 8_000 {
        16_000 - freq_hz
    } else {
        freq_hz
    };
    let mut re = 0.0_f64;
    let mut im = 0.0_f64;
    for (n, sample) in window.iter().enumerate() {
        let angle = std::f64::consts::TAU * f64::from(analysis_hz) * n as f64 / 16_000.0;
        re += f64::from(*sample) * angle.cos();
        im += f64::from(*sample) * angle.sin();
    }
    2.0 * re.hypot(im) / 16_000.0
}

fn amplitude(table: &Table, design: &Design, freq_hz: u32) -> f64 {
    let input = tone(design.input_hz, design.frames, freq_hz);
    let window = match scalar_after_delay(
        &input,
        table,
        rate(design.input_hz),
        rate(OUTPUT_HZ),
        WINDOW_START,
        WINDOW_LEN,
    ) {
        Ok(window) => window,
        Err(err) => panic!("{} Hz tone {freq_hz}: {err:?}", design.input_hz),
    };
    assert_eq!(
        window.len(),
        WINDOW_LEN,
        "{} tone {freq_hz}",
        design.input_hz
    );
    dft_amplitude(&window, freq_hz)
}

struct Report {
    reference: f64,
    worst_hz: u32,
    worst_db: f64,
    alias_db: f64,
}

fn measure(design: &Design) -> Report {
    assert_ideal_length(design);
    let ratio = Ratio::reduce(rate(design.input_hz), rate(OUTPUT_HZ));
    let phases = phase_count(ratio.numerator(), ratio.denominator());
    let table = match speech_kaiser(
        fc_for(design.input_hz),
        design.atten_db,
        design.taps,
        phases,
    ) {
        Ok(table) => table,
        Err(err) => panic!("{} builder: {err:?}", design.input_hz),
    };
    let mut freqs = Vec::new();
    for hz in (100_u32..=7_000).step_by(100) {
        freqs.push(hz);
    }
    freqs.push(8_050);
    let mut cells = Vec::new();
    if cells.try_reserve_exact(freqs.len()).is_err() {
        panic!("{} cell reserve", design.input_hz);
    }
    let table = &table;
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for hz in freqs {
            handles.push(scope.spawn(move || (hz, amplitude(table, design, hz))));
        }
        for handle in handles {
            match handle.join() {
                Ok(cell) => cells.push(cell),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
    });
    let mut reference = None;
    for (hz, amp) in &cells {
        if *hz == 1_000 {
            reference = Some(*amp);
        }
    }
    let reference = match reference {
        Some(reference) if reference.is_finite() && reference > 0.0 => reference,
        Some(reference) => panic!("{} reference {reference:?}", design.input_hz),
        None => panic!("{} reference missing", design.input_hz),
    };
    eprintln!(
        "{} reference_amplitude {:?} {:.20}",
        design.input_hz, reference, reference
    );
    let mut worst_hz = 100_u32;
    let mut worst_abs = -1.0_f64;
    let mut worst_db = 0.0_f64;
    let mut alias_db = None;
    for (hz, amp) in cells {
        let db = 20.0 * (amp / reference).log10();
        eprintln!(
            "{} taps {} tone {hz} db {:?} {:.20}",
            design.input_hz, design.taps, db, db
        );
        assert!(
            db.is_finite(),
            "{} tone {hz} ripple {db:?} {:.20}",
            design.input_hz,
            db
        );
        if hz == 8_050 {
            alias_db = Some(db);
            continue;
        }
        if db.abs() > worst_abs {
            worst_abs = db.abs();
            worst_db = db;
            worst_hz = hz;
        }
    }
    let alias_db = match alias_db {
        Some(alias_db) => alias_db,
        None => panic!("{} alias missing", design.input_hz),
    };
    eprintln!(
        "{} taps {} worst_hz {worst_hz} worst_db {:?} {:.20} worst_abs {:?} {:.20}",
        design.input_hz, design.taps, worst_db, worst_db, worst_abs, worst_abs
    );
    eprintln!(
        "{} taps {} alias_8050 {:?} {:.20} gate {:?} {:.20}",
        design.input_hz,
        design.taps,
        alias_db,
        alias_db,
        design.alias_gate_db,
        design.alias_gate_db
    );
    Report {
        reference,
        worst_hz,
        worst_db,
        alias_db,
    }
}

fn assert_gates(design: &Design, report: &Report) {
    assert!(
        report.reference.is_finite() && report.reference > 0.0,
        "{} reference {:?}",
        design.input_hz,
        report.reference
    );
    let worst_abs = report.worst_db.abs();
    assert!(
        report.worst_db.is_finite() && worst_abs <= RIPPLE_GATE_DB,
        "{} worst {} Hz ripple {:?} {:.20} dB",
        design.input_hz,
        report.worst_hz,
        report.worst_db,
        report.worst_db
    );
    assert!(
        report.alias_db.is_finite() && report.alias_db <= design.alias_gate_db,
        "{} 8050 Hz alias {:?} {:.20} dB, gate {:?} {:.20} dB",
        design.input_hz,
        report.alias_db,
        report.alias_db,
        design.alias_gate_db,
        design.alias_gate_db
    );
}

#[test]
fn shipping_rows_and_a_short_buffer_match_convert() {
    assert_short_matches_convert(22_050, 235, 320);
    assert_short_matches_convert(44_100, 467, 160);
}

#[test]
#[ignore = "two-second speech grid of unshipped Kaiser lengths 215 and 429"]
fn gate_lengths_on_the_speech_grid() {
    assert_short_matches_convert(22_050, 235, 320);
    assert_short_matches_convert(44_100, 467, 160);
    let designs = [
        Design {
            input_hz: 22_050,
            taps: 215,
            atten_db: 147.202,
            alias_gate_db: -147.202,
            frames: 44_100,
        },
        Design {
            input_hz: 44_100,
            taps: 429,
            atten_db: 147.190,
            alias_gate_db: -147.190,
            frames: 88_200,
        },
    ];
    for design in &designs {
        let report = measure(design);
        assert_gates(design, &report);
    }
}
