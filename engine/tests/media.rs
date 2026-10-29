//! The media expansion's CI checks that are not self-test signal checks:
//! mono and 78 exports, the run-out detector at 78, presets round-tripping,
//! RF64 past 4 GB, export layouts, the true-peak ceiling, the themes.

use runout_engine::audio::{self, decode_file, SourceAudio, WavWriter};
use runout_engine::auto::{self, RunoutDetector};
use runout_engine::chain::Chain;
use runout_engine::export;
use runout_engine::media::{self, MediumPreset};
use runout_engine::ops;
use runout_engine::presets;
use runout_engine::project::{ExportKind, Project, SourceKind};
use runout_engine::{render, selftest};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("runout-media-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A project of one medium over one in-memory source, exported to `root`.
fn export_with(m: &MediumPreset, src: Arc<SourceAudio>, root: &Path, title: &str) -> export::ExportResult {
    let mut p = Project::default();
    media::apply_to_project(&mut p, m, None, None);
    p.export.root = root.to_string_lossy().to_string();
    p.album.artist = "Test Artist".into();
    p.album.album = "Test Album".into();
    let sid = ops::add_source(&mut p, "mem.wav", SourceKind::File, false, "Side A", &src);
    ops::whole_source_track(&mut p, sid, &src);
    for n in auto::analyze_media(&mut p, sid, &src) {
        eprintln!("{n}");
    }
    p.tracks[0].title = title.into();
    let sources: export::Sources = [(sid, src)].into_iter().collect();
    export::export(&mut p, Path::new("/"), &sources, &|_, _| {}, &AtomicBool::new(false)).unwrap()
}

fn flac_of(r: &export::ExportResult) -> PathBuf {
    r.files.iter().find(|f| f.extension().map(|e| e == "flac").unwrap_or(false)).cloned().unwrap()
}

fn stereo_sweep(rate: u32, secs: f32) -> Arc<SourceAudio> {
    let sig = selftest::signal(secs, true, 10);
    if rate == sig.rate {
        return Arc::new(SourceAudio::from_planar(rate, 16, &sig.dirty));
    }
    let up = render::resample_to(&sig.dirty, sig.rate, rate);
    Arc::new(SourceAudio::from_planar(rate, 24, &up))
}

#[test]
fn mono_lp_and_78_export_one_channel() {
    let root = tmpdir("mono");
    for id in ["vinyl-mono-lp", "vinyl-78", "cassette-spoken", "microcassette", "reel-full-track", "video-linear"] {
        let m = media::find(id).unwrap();
        let rate = if m.capture.rate == 96000 { 96000 } else { 48000 };
        let r = export_with(&m, stereo_sweep(rate, 6.0), &root.join(id), "Side");
        let f = flac_of(&r);
        let a = decode_file(&f).unwrap();
        assert_eq!(a.channels, 1, "{id}: {} has {} channels", f.display(), a.channels);
        let want_rate = match media::export_rate(&m, m.speed.nominal) {
            0 => rate,
            r => r,
        };
        assert_eq!(a.rate, want_rate, "{id}: export rate");
        let tags = runout_engine::project::read_tags(&f);
        assert_eq!(tags.title.as_deref(), Some("Side"));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tape_exports_24_bit_48k_and_tags_say_what_it_was() {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    let root = tmpdir("tape");
    let m = media::find("cassette-type-ii").unwrap();
    let r = export_with(&m, stereo_sweep(96000, 5.0), &root, "Song");
    let f = flac_of(&r);
    let a = decode_file(&f).unwrap();
    assert_eq!((a.rate, a.bits, a.channels), (48000, 24, 2));
    let mut file = std::fs::File::open(&f).unwrap();
    let ff = lofty::flac::FlacFile::read_from(&mut file, ParseOptions::new()).unwrap();
    let vc = ff.vorbis_comments().unwrap();
    assert_eq!(vc.get("MEDIA"), Some("Cassette"));
    assert_eq!(vc.get("RUNOUT_MEDIUM"), Some("Cassette Type II / IV"));
    assert_eq!(vc.get("RUNOUT_NR"), None, "no NR line when NR is off");
    assert_eq!(vc.get("RUNOUT_DECK"), None, "no deck line without a deck");
    // raw keeps the capture rate
    let raw = r.files.iter().find(|f| f.extension().map(|e| e == "wav").unwrap_or(false));
    assert!(raw.is_none(), "an in-memory test source is not raw");
    assert!(r.album_dir.join("preset.json").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn runout_detector_at_78_and_the_old_lags() {
    assert_eq!(RunoutDetector::new(48000, 20.0).lags(), &[180, 133]);
    assert_eq!(auto::revolution_hops(33.333), 180);
    assert_eq!(auto::revolution_hops(45.0), 133);
    // a locked groove at 78.26 rpm: quiet surface noise with a tick once per revolution
    let rate = 48000u32;
    let mut d = RunoutDetector::for_speeds(rate, 30.0, &[78.26, 77.92, 78.0, 80.0]);
    let rev = (60.0 / 78.26 * rate as f64) as usize;
    let mut seed = 5u64;
    let mut fired_at = None;
    // 5 s of music first, then the run-out
    for i in 0..rate as usize * 40 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let n = ((seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5;
        let v = if i < rate as usize * 5 {
            0.3 * (i as f32 * 0.05).sin()
        } else {
            let k = (i - rate as usize * 5) % rev;
            0.001 * n + if k < 2400 { 0.004 * (k as f32 * 0.3).sin() } else { 0.0 }
        };
        if d.feed(&[v]) {
            fired_at = Some(i as f32 / rate as f32);
            break;
        }
    }
    let t = fired_at.expect("the detector should fire on a 78.26 rpm locked groove");
    assert!(t < 5.0 + 25.0, "fired at {t} s, before the 30 s plain-quiet fallback");
}

#[test]
fn every_builtin_medium_saves_and_loads_back() {
    // Everything that touches $XDG_CONFIG_HOME happens in this one test.
    let cfg = tmpdir("cfg");
    std::env::set_var("XDG_CONFIG_HOME", &cfg);
    for m in media::builtin() {
        let saved = media::save(&format!("Mine {}", m.id), &m).unwrap();
        let back = media::find(&saved.id).unwrap();
        let mut a = back.clone();
        a.id = m.id.clone();
        a.name = m.name.clone();
        a.base = None;
        a.builtin = true;
        assert_eq!(a, m, "{} changed on a save and load", m.id);
        let txt = std::fs::read_to_string(media::user_media_dir().join(format!("{}.json", saved.id))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["diff"], serde_json::json!({}), "{}: an unchanged preset stores no fields", m.id);
    }
    // one field changed: only that field is stored
    let mut m = media::find("reel-half-track").unwrap();
    m.chain.nr.ref_dbfs = -14.0;
    let saved = media::save("My reels", &m).unwrap();
    let txt = std::fs::read_to_string(media::user_media_dir().join(format!("{}.json", saved.id))).unwrap();
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["diff"], serde_json::json!({"chain": {"nr": {"ref_dbfs": -14.0}}}));
    assert_eq!(v["base"], "reel-half-track");
    // a fix to the built-in reaches it: the base is read fresh every time
    assert_eq!(media::find("my-reels").unwrap().chain.nr.ref_dbfs, -14.0);
    // a deck profile
    let d = media::save_deck(&media::DeckProfile { name: "Onkyo".into(), family: media::Family::Cassette, speed_pct: 0.8, azimuth_us: -12.0, ..Default::default() }).unwrap();
    assert_eq!(media::find_deck("onkyo").unwrap(), d);
    let mut p = Project::default();
    let c2 = media::find("cassette-type-i").unwrap();
    media::apply_to_project(&mut p, &c2, None, Some(&d));
    assert!(p.chain.azimuth.enabled && p.chain.azimuth.static_us == -12.0);
    let a = stereo_sweep(48000, 1.0);
    let sid = ops::add_source(&mut p, "x.wav", SourceKind::Capture, true, "Side A", &a);
    assert_eq!(p.source(sid).unwrap().fx.speed.trim_pct, 0.8);
    // condition presets by family, old ones as vinyl
    assert_eq!(presets::for_family(media::Family::Cassette).len(), 3);
    let _ = std::fs::remove_dir_all(&cfg);
}

#[test]
fn capture_past_4gb_is_rf64_and_reads_back() {
    let d = tmpdir("rf64");
    let p = d.join("long.wav");
    let mut w = WavWriter::create(&p, 96000, 2, 24).unwrap();
    let head: Vec<i32> = (0..9600).map(|i| (i * 100) % 8_000_000 - 4_000_000).collect();
    w.write(&head).unwrap();
    // 4.1 GB of silence as a sparse hole, then a known tail
    let frames = (4_400_000_000u64 / 6) as u64;
    w.write_silence(frames).unwrap();
    let tail: Vec<i32> = (0..9600).map(|i| 1000 + i).collect();
    w.write(&tail).unwrap();
    let total = w.finish().unwrap();
    let info = audio::wav_info(&p).unwrap();
    assert!(info.rf64, "past 4 GB the file is RF64");
    assert!(info.data_len > u32::MAX as u64);
    let a = decode_file(&p).unwrap();
    assert_eq!(a.frames as u64, total);
    assert_eq!(a.read_int_interleaved(0, 4800, 24), head);
    assert_eq!(a.read_int_interleaved(a.frames - 4800, a.frames, 24), tail);
    drop(a);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn album_mixtape_and_recording_layouts() {
    let root = tmpdir("layout");
    let src = stereo_sweep(48000, 3.0);
    // Album
    let r = export_with(&media::vinyl_lp(), src.clone(), &root, "Song");
    assert_eq!(r.album_dir, root.join("Test Artist/Test Artist - Test Album"));
    assert!(r.album_dir.join("01. Song.flac").exists());
    // Mixtape
    let mut m = media::find("cassette-type-i").unwrap();
    m.export.kind = ExportKind::Mixtape;
    let r = export_with(&m, stereo_sweep(96000, 3.0), &root, "Tune");
    assert_eq!(r.album_dir, root.join("Mixtapes/Test Album"));
    let f = r.album_dir.join("01. Test Artist - Tune.flac");
    assert!(f.exists(), "{:?}", r.files);
    assert_eq!(runout_engine::project::read_tags(&f).album_artist.as_deref(), Some("Various Artists"));
    // Recording, with an automatic title
    let m = media::find("microcassette").unwrap();
    let r = export_with(&m, stereo_sweep(96000, 3.0), &root, "");
    assert_eq!(r.album_dir, root.join("Recordings/Test Album"));
    assert!(r.album_dir.join("01. Part 1.flac").exists(), "{:?}", r.files);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn exports_never_exceed_the_ceiling() {
    let root = tmpdir("peak");
    for id in ["vinyl-lp", "cassette-type-ii", "vinyl-78", "video-hifi"] {
        let m = media::find(id).unwrap();
        let rate = if m.capture.rate == 96000 { 96000 } else { 48000 };
        // hot material: the sweep pushed near full scale
        let sig = selftest::signal(6.0, false, 0);
        let hot: Vec<Vec<f32>> = sig.dirty.iter().map(|c| c.iter().map(|v| v * 9.0).collect()).collect();
        let hot = if rate == 48000 { hot } else { render::resample_to(&hot, 48000, rate) };
        let r = export_with(&m, Arc::new(SourceAudio::from_planar(rate, 24, &hot)), &root.join(id), "Hot");
        let a = decode_file(&flac_of(&r)).unwrap();
        let ch = a.read(0, a.frames as i64);
        let l = runout_engine::dsp::loudness::measure(&ch, a.rate).0;
        let ceiling = m.chain.level.ceiling_dbtp;
        assert!(l.true_peak <= ceiling + 0.05, "{id}: {:.2} dBTP over {ceiling}", l.true_peak);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_capture_or_import_arrives_as_one_track() {
    let mut p = Project::default();
    let a = stereo_sweep(48000, 20.0);
    let sid = ops::add_source(&mut p, "a.wav", SourceKind::File, true, "Side A", &a);
    ops::auto_new_source(&mut p, sid, &a);
    ops::whole_source_track(&mut p, sid, &a);
    assert_eq!(p.tracks.len(), 1);
    assert!((p.tracks[0].end_s - a.duration()).abs() < 1e-9);
    // only the button splits
    ops::split_source(&mut p, sid, &a);
    assert!(!p.tracks.is_empty());
}

/// WCAG contrast of one color pair.
fn contrast(a: &str, b: &str) -> f64 {
    let lum = |h: &str| {
        let h = h.trim_start_matches('#');
        let c = |i: usize| {
            let v = u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * c(0) + 0.7152 * c(2) + 0.0722 * c(4)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Theme blocks of styles.css: name -> token -> value.
fn themes() -> Vec<(String, std::collections::BTreeMap<String, String>)> {
    let css = std::fs::read_to_string(repo().join("ui/src/styles.css")).unwrap();
    let mut out = Vec::new();
    let mut rest = css.as_str();
    while let Some(i) = rest.find(":root[data-theme=\"") {
        let after = &rest[i + 18..];
        let name = after[..after.find('"').unwrap()].to_string();
        let body_start = after.find('{').unwrap() + 1;
        let body_end = after.find('}').unwrap();
        let mut m = std::collections::BTreeMap::new();
        for l in after[body_start..body_end].split(';') {
            if let Some((k, v)) = l.trim().split_once(':') {
                if let Some(k) = k.trim().strip_prefix("--") {
                    m.insert(k.to_string(), v.trim().to_string());
                }
            }
        }
        out.push((name, m));
        rest = &after[body_end..];
    }
    out
}

#[test]
fn every_theme_passes_wcag_aa() {
    let t = themes();
    let names: Vec<&str> = t.iter().map(|(n, _)| n.as_str()).collect();
    for want in ["verdigris", "light", "dark", "kawaii-light", "kawaii-dark"] {
        assert!(names.contains(&want), "styles.css has no {want} theme");
    }
    for (name, m) in &t {
        let g = |k: &str| m.get(k).unwrap_or_else(|| panic!("{name}: no --{k}")).clone();
        for fg in ["text", "muted", "accent", "attention"] {
            for bg in ["bg", "surface", "raised"] {
                let c = contrast(&g(fg), &g(bg));
                assert!(c >= 4.5, "{name}: --{fg} on --{bg} is {c:.2}:1");
            }
        }
        let c = contrast(&g("on-accent"), &g("accent"));
        assert!(c >= 4.5, "{name}: --on-accent on --accent is {c:.2}:1");
    }
}

#[test]
fn verdigris_is_byte_for_byte_todays_palette() {
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(repo().join("engine/tests/golden/verdigris.json")).unwrap()).unwrap();
    let t = themes();
    let (_, v) = t.iter().find(|(n, _)| n == "verdigris").expect("verdigris theme");
    // old token names became semantic ones
    let rename = |k: &str| match k {
        "verdigris" => "accent".to_string(),
        "verdigris-dim" => "accent-dim".to_string(),
        "copper" => "attention".to_string(),
        "copper-dim" => "attention-dim".to_string(),
        k => k.to_string(),
    };
    for (k, val) in g["css_root"].as_object().unwrap() {
        if ["serif", "sans", "mono"].contains(&k.as_str()) {
            continue; // fonts live in the shared :root, unchanged
        }
        let nk = rename(k);
        assert_eq!(v.get(&nk).map(|s| s.as_str()), Some(val.as_str().unwrap()), "verdigris --{nk}");
    }
    assert_eq!(v.get("on-accent").map(|s| s.as_str()), Some(g["tiles"]["primary_button_text"].as_str().unwrap()));
    // and the Rust-drawn pictures
    let p = runout_engine::tiles::Palette::verdigris();
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    assert_eq!(hex(p.wave_raw), g["tiles"]["wave_raw"].as_str().unwrap());
    assert_eq!(hex(p.wave_proc), g["tiles"]["wave_proc"].as_str().unwrap());
    assert_eq!(hex(p.wave_mid), g["tiles"]["wave_mid"].as_str().unwrap());
    assert_eq!(hex(p.tile_bg), g["tiles"]["tile_bg"].as_str().unwrap());
    let ramp: Vec<String> = p.ramp.iter().map(|c| hex(*c)).collect();
    let want: Vec<String> = g["tiles"]["ramp"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
    assert_eq!(ramp, want);
    assert_eq!(runout_engine::tiles::RAMP_STOPS.to_vec(), vec![0.0, 0.35, 0.65, 0.85, 1.0]);
    // each theme's tile palette matches its css
    for (name, m) in &t {
        let pal = runout_engine::tiles::Palette::named(name);
        assert_eq!(hex(pal.wave_raw), m["wave-raw"], "{name} wave-raw");
        assert_eq!(hex(pal.wave_proc), m["accent"], "{name} processed wave = accent");
        assert_eq!(hex(pal.tile_bg), m["bg"], "{name} tile background");
    }
    let _ = Chain::default();
}

#[test]
fn a_digital_source_stays_bit_perfect_when_added() {
    // a full-scale, hum-free digital source would trip auto declip on vinyl
    let n = 48000 * 4;
    let mut inter: Vec<i16> = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = ((i as f64 * 2.0 * std::f64::consts::PI * 1000.0 / 48000.0).sin() * 32767.0).round() as i16;
        inter.push(v);
        inter.push(v);
    }
    let a = Arc::new(SourceAudio::from_interleaved_i16(48000, 2, inter));
    let dat = media::find("dat").unwrap();
    let mut p = Project::default();
    media::apply_to_project(&mut p, &dat, None, None);
    let before = p.chain.clone();
    let sid = ops::add_source(&mut p, "dat.wav", SourceKind::Capture, true, "Tape", &a);
    ops::auto_new_source(&mut p, sid, &a);
    ops::whole_source_track(&mut p, sid, &a);
    assert_eq!(p.chain, before, "adding a digital source switched a stage on");
    assert!(p.is_passthrough(&p.tracks[0]));
}
