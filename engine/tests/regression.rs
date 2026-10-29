//! Vinyl no-regression: everything vinyl did before the media expansion, as
//! recorded once in tests/golden/ from the unmodified code, must still come
//! out bit for bit. See golden/README.md. The goldens are never regenerated.

use runout_engine::audio::decode_file;
use runout_engine::batch;
use runout_engine::chain::{Chain, SourceFx};
use runout_engine::export;
use runout_engine::presets::{self, Preset};
use runout_engine::project::Project;
use runout_engine::{media, render, selftest};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn golden(name: &str) -> Value {
    let p = repo().join("engine/tests/golden").join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn hex(h: Sha256) -> String {
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Same procedure the goldens were recorded with: decoded integers at the
/// file's depth, 4 bytes little endian each.
fn pcm_sha256(path: &Path) -> (String, Value) {
    let a = decode_file(path).unwrap();
    let bits = if a.bits <= 16 { 16 } else { 24 };
    let mut h = Sha256::new();
    for s in a.read_int_interleaved(0, a.frames, bits) {
        h.update(s.to_le_bytes());
    }
    (hex(h), serde_json::json!({"rate": a.rate, "channels": a.channels, "bits": bits, "frames": a.frames}))
}

fn f32_sha256(ch: &[Vec<f32>]) -> String {
    let mut h = Sha256::new();
    for c in ch {
        for v in c {
            h.update(v.to_bits().to_le_bytes());
        }
    }
    hex(h)
}

fn tags(path: &Path) -> Vec<(String, String)> {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    let mut f = std::fs::File::open(path).unwrap();
    let ff = lofty::flac::FlacFile::read_from(&mut f, ParseOptions::new()).unwrap();
    let mut m: Vec<(String, String)> = ff.vorbis_comments().map(|vc| vc.items().map(|(k, v)| (k.to_ascii_uppercase(), v.to_string())).collect()).unwrap_or_default();
    m.sort();
    m
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

const REPORT_KEEP: &[&str] = &["  Clicks repaired:", "  Noise floor:", "  Loudness:", "  True peak:", "  NOTE:", "  Chain:", "  File:", "  Cleaned import:", "Leveling:", "Output:"];

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("runout-regr-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run one golden job through the same code `runout process` uses.
fn run_job(job: &Value, work: &Path, folder: &Path) -> PathBuf {
    let args: Vec<String> = job["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().replace("{repo}", &repo().to_string_lossy()).replace("{folder}", &folder.to_string_lossy())).collect();
    let out = work.join(job["name"].as_str().unwrap());
    let mut o = batch::JobOptions::default();
    o.batch.export_root = Some(out.to_string_lossy().to_string());
    let mut preset: Option<Preset> = None;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--preset" => {
                i += 1;
                preset = Some(presets::find(&args[i]).unwrap());
            }
            "--cleaned" => o.batch.cleaned = true,
            s => paths.push(PathBuf::from(s)),
        }
        i += 1;
    }
    let (mut p, sources) = batch::job_project(&paths, &o, preset.as_ref(), &|_| {}).unwrap();
    export::export(&mut p, Path::new("/"), &sources, &|_, _| {}, &AtomicBool::new(false)).unwrap();
    out
}

#[test]
fn vinyl_exports_match_the_goldens() {
    let jobs = golden("vinyl_exports.json");
    let work = tmpdir("exports");
    let folder = work.join("cleaned-in");
    std::fs::create_dir_all(&folder).unwrap();
    for c in ["clicks_sweep.wav", "clicks_sweep_clean.wav", "hum_sweep.wav"] {
        std::fs::copy(repo().join("tests").join(c), folder.join(c)).unwrap();
    }
    for job in jobs.as_array().unwrap() {
        let name = job["name"].as_str().unwrap();
        let out = run_job(job, &work, &folder);
        let mut files = Vec::new();
        walk(&out, &mut files);
        let rel = |p: &Path| p.strip_prefix(&out).unwrap().to_string_lossy().to_string();
        // the Album layout gained the artist folder: compare from the album folder down
        let find = |golden_rel: &str| -> PathBuf {
            let hit: Vec<&PathBuf> = files.iter().filter(|f| {
                let r = rel(f);
                r.ends_with(golden_rel) && r.len() > golden_rel.len() && r[..r.len() - golden_rel.len()].trim_end_matches('/').split('/').count() == 1
            }).collect();
            assert_eq!(hit.len(), 1, "{name}: {golden_rel} not found once under {}: {:?}", out.display(), files.iter().map(|f| rel(f)).collect::<Vec<_>>());
            hit[0].clone()
        };
        for g in job["flac"].as_array().unwrap() {
            let f = find(g["path"].as_str().unwrap());
            let (h, fmt) = pcm_sha256(&f);
            assert_eq!(h, g["pcm_sha256"].as_str().unwrap(), "{name}: {} audio changed", g["path"]);
            assert_eq!(fmt, g["format"], "{name}: {} format changed", g["path"]);
            let mut got = tags(&f);
            let media: Vec<(String, String)> = got.iter().filter(|(k, _)| k == "MEDIA").cloned().collect();
            assert_eq!(media, vec![("MEDIA".to_string(), "Vinyl".to_string())], "{name}: MEDIA tag");
            got.retain(|(k, _)| k != "MEDIA");
            let want: Vec<(String, String)> = g["tags"].as_array().unwrap().iter().map(|kv| (kv[0].as_str().unwrap().to_string(), kv[1].as_str().unwrap().to_string())).collect();
            assert_eq!(got, want, "{name}: {} tags changed", g["path"]);
        }
        for g in job["raw"].as_array().unwrap() {
            let f = find(g["path"].as_str().unwrap());
            let (h, fmt) = pcm_sha256(&f);
            assert_eq!(h, g["pcm_sha256"].as_str().unwrap(), "{name}: raw {} changed", g["path"]);
            assert_eq!(fmt, g["format"]);
        }
        let report = files.iter().find(|f| f.ends_with("Runout report.txt")).unwrap_or_else(|| panic!("{name}: no report"));
        let txt = std::fs::read_to_string(report).unwrap();
        let got: Vec<String> = txt.lines().filter(|l| REPORT_KEEP.iter().any(|k| l.starts_with(k))).map(|l| l.to_string()).collect();
        let want: Vec<String> = job["report"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
        assert_eq!(got, want, "{name}: analysis numbers in the report changed");
        // the album folder sits under an artist folder now
        let flac = files.iter().find(|f| f.extension().map(|e| e == "flac").unwrap_or(false)).unwrap();
        assert_eq!(rel(flac).split('/').count(), 3, "{name}: {{root}}/{{artist}}/{{album artist}} - {{album}}/file");
    }
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn vinyl_self_test_matches_the_goldens() {
    let g = golden("selftest.json");
    let (checks, _) = selftest::run();
    let got: Vec<(String, String)> = checks.iter().filter(|c| c.name != "full chain speed").map(|c| (c.name.clone(), c.value.clone())).collect();
    let want: Vec<(String, String)> = g["checks"].as_array().unwrap().iter().map(|kv| (kv[0].as_str().unwrap().to_string(), kv[1].as_str().unwrap().to_string())).collect();
    assert_eq!(got, want, "vinyl self-test results changed");
    assert!(checks.iter().all(|c| c.pass));
    // Throughput within 5% of the golden only means something on the machine
    // the golden was measured on.
    let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|c| c.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())).unwrap_or_default();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let same = cpu == g["cpu"].as_str().unwrap_or("") && threads as u64 == g["threads"].as_u64().unwrap_or(0);
    let mut runs: Vec<f32> = (0..3).map(|_| selftest::vinyl_throughput()).collect();
    runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let gold = g["throughput_x_median"].as_f64().unwrap() as f32;
    if same {
        assert!(runs[1] >= gold * 0.95, "vinyl chain throughput {:.1}x is more than 5% under the golden {gold:.1}x", runs[1]);
    } else {
        eprintln!("throughput {:.1}x here; golden {gold:.1}x was measured on {} with {} threads, so the 5% check is skipped", runs[1], g["cpu"], g["threads"]);
    }
}

/// A version 2 chain or SourceFx serialized, reduced to the version 1 shape.
fn v1_shape(mut v: Value) -> Value {
    if let Value::Object(m) = &mut v {
        for k in ["wow", "azimuth", "channel_mode", "playback_eq", "nr", "dropout", "band_limit", "error_repair", "deemphasis", "digital"] {
            m.remove(k);
        }
        if let Some(Value::Object(h)) = m.get_mut("hum") {
            if let Some(f) = h.remove("fundamental_hz") {
                h.insert("mains_hz".into(), f);
            }
        }
    }
    v
}

#[test]
fn version_1_project_loads_as_vinyl_lp_and_renders_the_same() {
    let fx = golden("fixtures.json");
    let dir = repo().join("engine/tests/golden/v1-project.runout");
    let p = Project::load(&dir).unwrap();
    assert_eq!(p.medium.id, "vinyl-lp");
    assert_eq!(p.medium.preset, media::vinyl_lp());
    assert!(p.chain.new_stages_default());
    assert_eq!(v1_shape(serde_json::to_value(&p.chain).unwrap()), fx["v1_project_chain"], "chain settings changed on load");
    assert_eq!(p.export.root, "~/Music/Runout", "old default root reads as the new one");
    let s = p.source(1).unwrap();
    assert!(s.fx.new_fields_default());
    let a = decode_file(&p.resolve(&dir, &s.path)).unwrap();
    let blk = render::render_play_block(&p, 1, &a, 0, 10 * 48000).unwrap();
    assert_eq!(f32_sha256(&blk.out), fx["v1_project_block_10s_f32_sha256"].as_str().unwrap(), "10-second block changed");
    let full = render::render_range(&a, &s.fx, &p.chain, 0, runout_engine::chain::processed_frames(&a, &s.fx), false, false, false).out;
    assert_eq!(f32_sha256(&full), fx["v1_project_full_render_f32_sha256"].as_str().unwrap(), "full render changed");
    assert_eq!(p.track_key(&p.tracks[0]).to_string(), fx["v1_project_track_key"].as_str().unwrap(), "stored analyses would go stale");
    // saving writes version 2 with the new field names only
    let out = tmpdir("v1save");
    p.save(&out).unwrap();
    let txt = std::fs::read_to_string(out.join("project.json")).unwrap();
    let v: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["version"], 2);
    assert!(txt.contains("\"fundamental_hz\"") && txt.contains("\"captured\"") && txt.contains("\"correct\""));
    assert!(!txt.contains("mains_hz") && !txt.contains("captured_rpm") && !txt.contains("correct_rpm"));
    let back = Project::load(&out).unwrap();
    assert_eq!(back.chain, p.chain);
    assert_eq!(back.sources, p.sources);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn version_1_project_exports_under_the_new_root() {
    let dir = repo().join("engine/tests/golden/v1-project.runout");
    let mut p = Project::load(&dir).unwrap();
    let out = tmpdir("v1root");
    // what ~/Music/Runout expands to, pointed somewhere safe
    p.export.root = out.join("Music/Runout").to_string_lossy().to_string();
    let a = std::sync::Arc::new(decode_file(&p.resolve(&dir, &p.sources[0].path)).unwrap());
    let sources: export::Sources = [(1u64, a)].into_iter().collect();
    let r = export::export(&mut p, &dir, &sources, &|_, _| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(r.album_dir, out.join("Music/Runout/Golden/Golden - Fixture"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn version_1_user_preset_loads_as_vinyl() {
    let fx = golden("fixtures.json");
    let txt = std::fs::read_to_string(repo().join("engine/tests/golden/v1-user-preset.json")).unwrap();
    // the same parse presets::list does for each file in presets/
    let p: Preset = serde_json::from_str(&txt).unwrap();
    assert_eq!(p.family, media::Family::Vinyl);
    assert!(p.chain.new_stages_default());
    assert_eq!(v1_shape(serde_json::to_value(&p.chain).unwrap()), fx["v1_preset_chain"]);
    let clicks = decode_file(&repo().join("tests/clicks_sweep.wav")).unwrap();
    let out = render::render_range(&clicks, &SourceFx::default(), &p.chain, 0, clicks.frames, false, false, false).out;
    assert_eq!(f32_sha256(&out), fx["v1_preset_render_clicks_sweep_f32_sha256"].as_str().unwrap());
    // and it shows in the vinyl list
    assert_eq!(presets::builtin().iter().filter(|b| b.family == media::Family::Vinyl).count(), 3);
    let _ = Chain::default();
}
