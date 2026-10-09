//! DSP tests against the short reference clips in the repo's tests/ folder.
//! Regenerate them with `runout gen-test-clips tests`.

use runout_engine::audio::{decode_file, SourceAudio};
use runout_engine::chain::{Chain, SourceFx};
use runout_engine::export;
use runout_engine::ops;
use runout_engine::project::{self, Project};
use runout_engine::render;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn clip(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("tests").join(name)
}

fn only(f: impl Fn(&mut Chain)) -> Chain {
    let mut c = Chain::passthrough();
    f(&mut c);
    c
}

#[test]
fn declick_reference_clip() {
    let dirty = decode_file(&clip("clicks_sweep.wav")).expect("clicks_sweep.wav");
    let clean = decode_file(&clip("clicks_sweep_clean.wav")).expect("clicks_sweep_clean.wav");
    let positions: Vec<usize> = std::fs::read_to_string(clip("clicks_sweep.txt")).unwrap().lines().filter_map(|l| l.trim().parse().ok()).collect();
    let chain = only(|c| c.declick.enabled = true);
    let r = render::render_range(&dirty, &SourceFx::default(), &chain, 0, dirty.frames, false, false, true);
    let reference = render::render_range(&clean, &SourceFx::default(), &Chain::passthrough(), 0, clean.frames, false, false, false);
    let hits = render::dedupe_clicks(r.clicks, dirty.rate);
    let mut found = 0;
    for &p in &positions {
        if hits.iter().any(|h| (h.pos - p as i64).abs() < 100) {
            found += 1;
        }
        let worst = (p..p + 48).map(|i| (r.out[0][i] - reference.out[0][i]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.01, "click at {p}: residual {worst}");
    }
    assert_eq!(found, positions.len(), "every click found");
    // No damage between clicks: compare a stretch with no clicks.
    let quiet = positions[0] / 2;
    let err = (quiet..quiet + 4800).map(|i| (r.out[0][i] - reference.out[0][i]).abs()).fold(0.0f32, f32::max);
    assert!(err < 0.002, "untouched audio changed by {err}");
}

/// Level of one frequency, Hann windowed so a loud sweep elsewhere does not leak in.
fn tone_db(x: &[f32], rate: u32, f: f64) -> f32 {
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    let n = x.len() as f64;
    for (i, &v) in x.iter().enumerate() {
        let h = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
        let s0 = v as f64 * h + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (10.0 * ((s1 * s1 + s2 * s2 - c * s1 * s2) / (x.len() as f64).powi(2)).max(1e-30).log10()) as f32
}

#[test]
fn hum_reference_clip() {
    let a = decode_file(&clip("hum_sweep.wav")).expect("hum_sweep.wav");
    assert_eq!(runout_engine::auto::detect_hum(&a), Some(60.0));
    let chain = only(|c| {
        c.hum.enabled = true;
        c.hum.mains_hz = 60.0;
    });
    let r = render::render_range(&a, &SourceFx::default(), &chain, 0, a.frames, false, false, false);
    // skip the first 2 s: a 1 Hz wide notch needs time to settle after the
    // hum starts abruptly at sample 0
    let raw = a.read_channel(0, 2 * 48000, 39 * 4800);
    let out = &r.out[0][2 * 48000..39 * 4800];
    for f in [60.0, 180.0] {
        let drop = tone_db(&raw, a.rate, f) - tone_db(out, a.rate, f);
        assert!(drop > 25.0, "{f} Hz only down {drop} dB");
    }
    // the sweep itself is untouched at 1 kHz-ish
    let keep = (tone_db(&raw, a.rate, 1000.0) - tone_db(out, a.rate, 1000.0)).abs();
    assert!(keep < 0.5, "music changed by {keep} dB");
}

#[test]
fn denoise_reduces_hiss_keeps_tone() {
    let rate = 48000u32;
    let n = rate as usize * 6;
    let mut seed = 9u64;
    let mut rnd = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    };
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let tone = if i > rate as usize * 2 { 0.2 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() } else { 0.0 };
            tone + 0.004 * rnd()
        })
        .collect();
    let src = SourceAudio::from_planar(rate, 16, &[x.clone(), x.clone()]);
    let mut fx = SourceFx::default();
    fx.noise = runout_engine::auto::learn_noise_at(&src, 0, rate as usize * 2);
    assert!(fx.noise.is_some());
    let chain = only(|c| {
        c.denoise.enabled = true;
        c.denoise.reduction_db = 10.0;
    });
    let r = render::render_range(&src, &fx, &chain, 0, n, false, false, false);
    let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
    let hiss_in = rms(&x[rate as usize / 2..rate as usize * 2 - 4800]);
    let hiss_out = rms(&r.out[0][rate as usize / 2..rate as usize * 2 - 4800]);
    let drop = 20.0 * (hiss_in / hiss_out).log10();
    assert!(drop > 7.0 && drop < 12.0, "hiss reduced {drop} dB (asked for 10)");
    let tone_in = tone_db(&x[rate as usize * 3..rate as usize * 5], rate, 1000.0);
    let tone_out = tone_db(&r.out[0][rate as usize * 3..rate as usize * 5], rate, 1000.0);
    assert!((tone_in - tone_out).abs() < 0.5, "tone changed {} dB", tone_in - tone_out);
}

#[test]
fn cleaned_folder_import_is_bit_exact() {
    let tmp = std::env::temp_dir().join(format!("runout-test-{}", std::process::id()));
    let album = tmp.join("in");
    std::fs::create_dir_all(&album).unwrap();
    // two "already cleaned" files with tags, out of filename order
    let sig = runout_engine::selftest::signal(3.0, false, 0);
    for (name, n) in [("10 - Last.flac", 2u32), ("2 - First.flac", 1)] {
        let ints = export::to_int_dithered(&sig.clean, 16, n as u64);
        let p = album.join(name);
        export::encode_flac(&p, &ints, 2, 16, 48000).unwrap();
        export::write_tags(&p, &export::TagSet { title: format!("Song {n}"), artist: "A".into(), album: "B".into(), album_artist: String::new(), year: "1977".into(), genre: String::new(), track: Some(n), track_total: None, disc: None }, None, None).unwrap();
    }
    let mut items = project::plan_folder_import(&album).unwrap();
    assert_eq!(items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["2 - First", "10 - Last"]);
    for it in items.iter_mut() {
        it.raw = false;
    }
    let mut p = Project::default();
    p.export.root = tmp.join("out").to_string_lossy().to_string();
    let added = ops::import_items(&mut p, &items, &mut |path| decode_file(std::path::Path::new(path)).map(Arc::new)).unwrap();
    let sources: export::Sources = added.into_iter().collect();
    assert_eq!(p.album.album, "B");
    let res = export::export(&mut p, &tmp, &sources, &|_, _| {}, &AtomicBool::new(false)).unwrap();
    for (orig, name) in [("2 - First.flac", "2 - First.flac"), ("10 - Last.flac", "10 - Last.flac")] {
        let a = decode_file(&album.join(orig)).unwrap();
        let b = decode_file(&res.album_dir.join(name)).unwrap();
        assert_eq!(a.frames, b.frames);
        let (x, y) = (a.read_int_interleaved(0, a.frames, 16), b.read_int_interleaved(0, b.frames, 16));
        assert!(x == y, "{name} audio changed");
        let tags = project::read_tags(&res.album_dir.join(name));
        assert_eq!(tags.year.as_deref(), Some("1977"));
    }
    assert!(!res.album_dir.join("Raw").exists(), "cleaned files get no raw copy");
    let _ = std::fs::remove_dir_all(&tmp);
}
