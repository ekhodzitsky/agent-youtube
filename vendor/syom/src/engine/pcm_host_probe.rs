//! Cross-host bit hashes of decoder tables and planar PCM.
//!
//! `cargo test --lib pcm_host_probe -- --nocapture`

use super::filterbank::window_left;
use super::ics::WindowShape;
use super::imdct::probe_plan_2048;
use super::spectrum::{invquant, sf_gain};
use crate::DecodeOptions;

fn mix(h: u64, bits: u64) -> u64 {
    h.wrapping_mul(0x9E37_79B1_85EB_CA87).wrapping_add(bits)
}

fn hash_f32(xs: &[f32]) -> u64 {
    let mut h = 0xC0FF_EE00_D15E_A5E5;
    for &x in xs {
        h = mix(h, u64::from(x.to_bits()));
    }
    h
}

fn hash_pairs(re: &[f32], im: &[f32]) -> u64 {
    let mut h = 0xC0FF_EE00_D15E_A5E5;
    for (&r, &i) in re.iter().zip(im.iter()) {
        h = mix(h, u64::from(r.to_bits()));
        h = mix(h, u64::from(i.to_bits()));
    }
    h
}

fn hash_pcm(channels: &[Vec<f32>]) -> (u64, usize) {
    let mut h = 0xC0FF_EE00_D15E_A5E5;
    let mut n = 0usize;
    for plane in channels {
        for &s in plane {
            h = mix(h, u64::from(s.to_bits()));
            n += 1;
        }
    }
    (h, n)
}

#[test]
fn pcm_host_probe() -> Result<(), crate::AacError> {
    let sine_long = window_left(2048, WindowShape::Sine);
    let sine_short = window_left(256, WindowShape::Sine);
    let kbd_long = window_left(2048, WindowShape::Kbd);
    let kbd_short = window_left(256, WindowShape::Kbd);
    println!(
        "PROBE sine_long {:016x} len={}",
        hash_f32(sine_long),
        sine_long.len()
    );
    println!(
        "PROBE sine_short {:016x} len={}",
        hash_f32(sine_short),
        sine_short.len()
    );
    println!(
        "PROBE kbd_long {:016x} len={}",
        hash_f32(kbd_long),
        kbd_long.len()
    );
    println!(
        "PROBE kbd_short {:016x} len={}",
        hash_f32(kbd_short),
        kbd_short.len()
    );

    let (pre_re, pre_im, tw_re, tw_im, post_re, post_im) = probe_plan_2048();
    println!(
        "PROBE imdct2048_pre {:016x} len={}",
        hash_pairs(&pre_re, &pre_im),
        pre_re.len()
    );
    println!(
        "PROBE imdct2048_twiddle {:016x} len={}",
        hash_pairs(&tw_re, &tw_im),
        tw_re.len()
    );
    println!(
        "PROBE imdct2048_post {:016x} len={}",
        hash_pairs(&post_re, &post_im),
        post_re.len()
    );

    let opts = DecodeOptions::unbounded();
    let fixtures: [(&str, &[u8]); 4] = [
        ("sine48", include_bytes!("../goldens/sine48.adts")),
        ("tns48", include_bytes!("../goldens/tns48.adts")),
        ("he48", include_bytes!("../goldens/he48.adts")),
        ("ps48", include_bytes!("../goldens/ps48.adts")),
    ];
    for (name, bytes) in fixtures {
        let decoded = crate::decode_with(bytes, &opts)?;
        let (h, n) = hash_pcm(&decoded.channels);
        println!(
            "PROBE pcm {name} {h:016x} ch={} n={n}",
            decoded.channels.len()
        );
    }

    let mut gains = [0.0f32; 256];
    for (i, slot) in gains.iter_mut().enumerate() {
        *slot = sf_gain(i as i32);
    }
    let mut pow43 = [0.0f32; 8192];
    for (i, slot) in pow43.iter_mut().enumerate() {
        *slot = invquant(i as i32);
    }
    println!("PROBE diag_sf_gain {:016x} len=256", hash_f32(&gains));
    println!("PROBE diag_pow43 {:016x} len=8192", hash_f32(&pow43));
    Ok(())
}
