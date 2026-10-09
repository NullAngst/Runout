//! runout: the headless side of Runout.
//!
//!   runout --self-test
//!   runout process [options] PATH...
//!   runout analyze FILE...
//!   runout presets
//!   runout gen-test-clips DIR

use runout_engine::audio::{self, SourceAudio};
use runout_engine::export::{self, Sources};
use runout_engine::project::{self, Project, SourceKind};
use runout_engine::{auto, batch, ops, presets, selftest};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const HELP: &str = "runout - vinyl restoration, headless

USAGE
  runout --self-test
      Render a generated test signal through the full chain and check clicks
      repaired, hum removed, LUFS and true peak. Exits non-zero on failure.

  runout process [OPTIONS] PATH...
      Restore and export. A PATH that is a folder is imported as one album
      (files in tag/track order, subfolders as discs). A PATH that is a file is
      one side, split into tracks on silence.
      --preset NAME     clean-modern, light-wear, worn-70s, or a saved preset
      --batch           treat each subfolder of PATH as its own album
      --cleaned         folder files are already cleaned (no restoration, no leveling)
      --out DIR         export root (default ~/Music/Runout/Artists)
      --bits 16|24      FLAC bit depth (default 16)
      --target LUFS     loudness target (default -14)
      --ceiling DBTP    true-peak ceiling (default -1.0)
      --album-mode      level the album as one, keeping the quiet songs quiet
      --no-level        skip loudness leveling
      --artist, --album, --album-artist, --year, --genre TEXT
      --cover FILE      cover image, embedded and saved as cover.jpg

  runout analyze FILE...
      Loudness, true peak, clicks per second, hum, noise floor.

  runout presets
      List presets.

  runout gen-test-clips DIR
      Write the reference clips used by the tests.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let r = match args[0].as_str() {
        "--self-test" | "self-test" => self_test(),
        "process" => process(&args[1..]),
        "analyze" => analyze(&args[1..]),
        "presets" => {
            for p in presets::list() {
                println!("{:<16} {}{}", p.id, p.name, if p.builtin { "" } else { "  (yours)" });
            }
            Ok(())
        }
        "gen-test-clips" => gen_clips(args.get(1).map(String::as_str).unwrap_or("tests")),
        other => Err(format!("unknown command {other}; try runout --help")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("runout: {e}");
            ExitCode::FAILURE
        }
    }
}

fn self_test() -> Result<(), String> {
    println!("Runout self-test: generated sweep with hum, hiss and 40 clicks\n");
    let (checks, secs) = selftest::run();
    for c in &checks {
        println!("  {:<28} {:<40} {}", c.name, c.value, if c.pass { "ok" } else { "FAIL" });
    }
    println!("\n  took {secs:.1} s");
    if checks.iter().all(|c| c.pass) {
        println!("  all checks passed");
        Ok(())
    } else {
        Err(format!("{} check(s) failed", checks.iter().filter(|c| !c.pass).count()))
    }
}

struct Opts {
    preset: Option<String>,
    batch: bool,
    cleaned: bool,
    out: Option<String>,
    bits: u16,
    target: Option<f32>,
    ceiling: Option<f32>,
    album_mode: bool,
    no_level: bool,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    year: Option<String>,
    genre: Option<String>,
    cover: Option<String>,
    paths: Vec<PathBuf>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        preset: None,
        batch: false,
        cleaned: false,
        out: None,
        bits: 16,
        target: None,
        ceiling: None,
        album_mode: false,
        no_level: false,
        artist: None,
        album: None,
        album_artist: None,
        year: None,
        genre: None,
        cover: None,
        paths: Vec::new(),
    };
    let mut i = 0;
    let val = |i: &mut usize| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--preset" => o.preset = Some(val(&mut i)?),
            "--batch" => o.batch = true,
            "--cleaned" => o.cleaned = true,
            "--out" => o.out = Some(val(&mut i)?),
            "--bits" => o.bits = val(&mut i)?.parse().map_err(|_| "--bits takes 16 or 24")?,
            "--target" => o.target = Some(val(&mut i)?.parse().map_err(|_| "--target takes a number")?),
            "--ceiling" => o.ceiling = Some(val(&mut i)?.parse().map_err(|_| "--ceiling takes a number")?),
            "--album-mode" => o.album_mode = true,
            "--no-level" => o.no_level = true,
            "--artist" => o.artist = Some(val(&mut i)?),
            "--album" => o.album = Some(val(&mut i)?),
            "--album-artist" => o.album_artist = Some(val(&mut i)?),
            "--year" => o.year = Some(val(&mut i)?),
            "--genre" => o.genre = Some(val(&mut i)?),
            "--cover" => o.cover = Some(val(&mut i)?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => {
                let p = project::expand_home(s);
                o.paths.push(std::fs::canonicalize(&p).unwrap_or(p));
            }
        }
        i += 1;
    }
    if o.paths.is_empty() {
        return Err("process needs at least one file or folder".into());
    }
    if o.bits != 16 && o.bits != 24 {
        return Err("--bits takes 16 or 24".into());
    }
    Ok(o)
}

fn load(path: &str) -> runout_engine::Result<Arc<SourceAudio>> {
    eprintln!("  reading {path}");
    audio::decode_file(Path::new(path)).map(Arc::new)
}

fn process(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    // Work out the albums to run.
    let mut jobs: Vec<Vec<PathBuf>> = Vec::new();
    for p in &o.paths {
        if p.is_dir() && o.batch {
            for s in batch::album_folders(p) {
                jobs.push(vec![s]);
            }
        } else if p.is_dir() {
            jobs.push(vec![p.clone()]);
        } else if p.is_file() {
            // consecutive files form one record (sides)
            match jobs.last_mut() {
                Some(j) if j.iter().all(|x| x.is_file()) => j.push(p.clone()),
                _ => jobs.push(vec![p.clone()]),
            }
        } else {
            return Err(format!("{} not found", p.display()));
        }
    }
    if jobs.is_empty() {
        return Err("nothing to process".into());
    }
    let preset = match &o.preset {
        Some(n) => Some(presets::find(n).ok_or_else(|| format!("no preset called {n}; see runout presets"))?),
        None => None,
    };
    let mut failed = 0;
    for (i, job) in jobs.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, jobs.len(), job.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
        match run_job(job, &o, preset.as_ref()) {
            Ok(r) => {
                println!("  -> {}", r.album_dir.display());
                for w in &r.warnings {
                    println!("  note: {w}");
                }
            }
            Err(e) => {
                failed += 1;
                eprintln!("  failed: {e}");
            }
        }
    }
    if failed > 0 {
        Err(format!("{failed} of {} album(s) failed", jobs.len()))
    } else {
        Ok(())
    }
}

fn run_job(job: &[PathBuf], o: &Opts, preset: Option<&presets::Preset>) -> Result<export::ExportResult, String> {
    let mut p = Project::default();
    let mut sources: Sources = Default::default();
    if let Some(pr) = preset {
        p.chain = presets::apply(&p.chain, pr);
    }
    let bopts = batch::BatchOptions {
        cleaned: o.cleaned,
        export_root: o.out.clone(),
        bit_depth: Some(o.bits),
        target_lufs: o.target,
        ceiling_dbtp: o.ceiling,
        album_mode: o.album_mode,
        no_level: o.no_level,
    };
    if job.len() == 1 && job[0].is_dir() {
        let (fp, fs) = batch::folder_project(&job[0], &bopts, preset, &|m| eprintln!("  {m}")).map_err(|e| e.to_string())?;
        p = fp;
        sources = fs;
    } else {
        for (n, f) in job.iter().enumerate() {
            let a = load(&f.to_string_lossy()).map_err(|e| e.to_string())?;
            let name = format!("Side {}", (b'A' + n as u8) as char);
            let sid = ops::add_source(&mut p, &f.to_string_lossy(), SourceKind::File, true, &name, &a);
            ops::auto_new_source(&mut p, sid, &a);
            p.split.threshold_db = auto::auto_split_threshold(&a);
            ops::split_source(&mut p, sid, &a);
            sources.insert(sid, a);
        }
        if let Some(pr) = preset {
            p.chain = presets::apply(&p.chain, pr);
        }
        let stem = job[0].file_stem().unwrap_or_default().to_string_lossy().to_string();
        if p.album.album.is_empty() {
            p.album.album = stem;
        }
        for t in p.tracks.iter_mut() {
            if t.title.is_empty() {
                t.title = format!("Track {}", t.number.unwrap_or(0));
            }
        }
    }
    batch::apply_options(&mut p, &bopts);
    let set = |dst: &mut String, v: &Option<String>| {
        if let Some(v) = v {
            *dst = v.clone();
        }
    };
    set(&mut p.album.artist, &o.artist);
    set(&mut p.album.album, &o.album);
    set(&mut p.album.album_artist, &o.album_artist);
    set(&mut p.album.year, &o.year);
    set(&mut p.album.genre, &o.genre);
    if let Some(c) = &o.cover {
        let cp = project::expand_home(c);
        p.album.cover_path = Some(std::fs::canonicalize(&cp).unwrap_or(cp).to_string_lossy().to_string());
    }
    let cancel = AtomicBool::new(false);
    let last = std::sync::Mutex::new(String::new());
    let progress = |_f: f32, msg: String| {
        let mut l = last.lock().unwrap();
        if *l != msg {
            eprintln!("  {msg}");
            *l = msg;
        }
    };
    export::export(&mut p, Path::new("/"), &sources, &progress, &cancel).map_err(|e| e.to_string())
}

fn analyze(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("analyze needs a file".into());
    }
    for f in args {
        let path = project::expand_home(f);
        let a = audio::decode_file(&path).map_err(|e| e.to_string())?;
        let ch = a.read(0, a.frames as i64);
        let (l, _) = runout_engine::dsp::loudness::measure(&ch, a.rate);
        println!("{}", path.display());
        println!("  {} Hz, {} ch, {}-bit, {:.1} s", a.rate, a.channels, a.bits, a.duration());
        println!("  loudness {:.1} LUFS, LRA {:.1} LU, true peak {:.2} dBTP, sample peak {:.2} dBFS", l.integrated, l.lra, l.true_peak, l.sample_peak);
        println!("  noise floor {:.1} dBFS", runout_engine::dsp::loudness::noise_floor_db(&ch, a.rate));
        println!("  clicks about {:.1} per second", auto::click_density(&a));
        match auto::detect_hum(&a) {
            Some(f) => println!("  hum at {f} Hz"),
            None => println!("  no mains hum found"),
        }
        if let Some(c) = auto::detect_clipping(&a) {
            println!("  clipped at {c:.2} dBFS");
        }
        let az = auto::detect_azimuth(&a).unwrap_or(0.0);
        println!("  channel offset {az:.0} us");
        let regions = auto::split_on_silence(&a, &project::SplitSettings { threshold_db: auto::auto_split_threshold(&a), min_gap_s: 1.2 });
        println!("  {} track(s) by silence", regions.len());
    }
    Ok(())
}

fn gen_clips(dir: &str) -> Result<(), String> {
    let d = PathBuf::from(dir);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let s = selftest::signal(4.0, false, 12);
    audio::write_wav(&d.join("clicks_sweep.wav"), s.rate, 16, &s.dirty).map_err(|e| e.to_string())?;
    audio::write_wav(&d.join("clicks_sweep_clean.wav"), s.rate, 16, &s.clean).map_err(|e| e.to_string())?;
    let positions: Vec<String> = s.click_pos.iter().map(|p| p.to_string()).collect();
    std::fs::write(d.join("clicks_sweep.txt"), positions.join("\n") + "\n").map_err(|e| e.to_string())?;
    let h = selftest::signal(4.0, true, 0);
    audio::write_wav(&d.join("hum_sweep.wav"), h.rate, 16, &h.dirty).map_err(|e| e.to_string())?;
    println!("wrote clicks_sweep.wav, clicks_sweep_clean.wav, clicks_sweep.txt, hum_sweep.wav to {}", d.display());
    Ok(())
}
