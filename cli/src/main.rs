//! runout: the headless side of Runout.
//!
//!   runout --self-test
//!   runout process [options] PATH...
//!   runout analyze FILE...
//!   runout presets
//!   runout media
//!   runout verify FILE...
//!   runout gen-test-clips DIR

use runout_engine::audio;
use runout_engine::export::{self, Sources};
use runout_engine::project::{self, Project};
use runout_engine::{auto, batch, media, presets, selftest};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

const HELP: &str = "runout - analog and digital media restoration, headless

USAGE
  runout --self-test
      Render generated test signals through the vinyl, tape and digital chains
      and check them: clicks repaired, hum removed, LUFS and true peak, speed
      and wow recovered, azimuth, dropouts, B-type NR round trip, bit-perfect
      digital passthrough. Exits non-zero on failure.

  runout process [OPTIONS] PATH...
      Restore and export. A PATH that is a folder is imported as one album
      (files in tag/track order, subfolders as discs). A PATH that is a file is
      one side, split into tracks by the medium's split strategy (silence for
      vinyl, the hiss floor for tape) unless --no-split.
      --medium ID       medium preset (default vinyl-lp); see runout media
      --deck NAME       deck profile to apply
      --preset NAME     condition preset: clean-modern, light-wear, worn-70s,
                        cassette-clean, cassette-worn, ..., or a saved preset
      --no-split        keep each file as one track
      --batch           treat each subfolder of PATH as its own album
      --cleaned         folder files are already cleaned (no restoration, no leveling)
      --out DIR         export root (default ~/Music/Runout)
      --bits 16|24      FLAC bit depth (default: the medium's, 16 for vinyl)
      --target LUFS     loudness target (default -14)
      --ceiling DBTP    true-peak ceiling (default -1.0)
      --album-mode      level the album as one, keeping the quiet songs quiet
      --no-level        skip loudness leveling
      --artist, --album, --album-artist, --year, --genre TEXT
      --cover FILE      cover image, embedded and saved as cover.jpg

  runout analyze FILE...
      Loudness, true peak, clicks per second, hum, noise floor, reference tone.

  runout presets
      List condition presets.

  runout media
      List medium presets.

  runout verify FILE...
      Bit-perfect check of a digital capture: whether the low bits of a
      24-bit capture of a 16-bit source are all zero, and any runs of zeros or
      held samples that mark a deck's uncorrectable errors.

  runout gen-test-clips DIR
      Write the reference clips used by the tests, tape clip included.
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
                println!("{:<18} {:<9} {}{}", p.id, p.family.label(), p.name, if p.builtin { "" } else { "  (yours)" });
            }
            Ok(())
        }
        "media" => {
            for m in media::list() {
                println!("{:<20} {:<9} {}{}", m.id, m.family.label(), m.name, if m.builtin { "" } else { "  (yours)" });
            }
            Ok(())
        }
        "verify" => verify(&args[1..]),
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
    println!("Runout self-test: vinyl sweep with hum, hiss and 40 clicks; a 96 kHz tape clip; a 16-bit digital fixture\n");
    let (checks, secs) = selftest::run_all();
    for c in &checks {
        println!("  {:<28} {:<56} {}", c.name, c.value, if c.pass { "ok" } else { "FAIL" });
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
    medium: Option<String>,
    deck: Option<String>,
    no_split: bool,
    batch: bool,
    cleaned: bool,
    out: Option<String>,
    bits: Option<u16>,
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
        medium: None,
        deck: None,
        no_split: false,
        batch: false,
        cleaned: false,
        out: None,
        bits: None,
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
            "--medium" => o.medium = Some(val(&mut i)?),
            "--deck" => o.deck = Some(val(&mut i)?),
            "--no-split" => o.no_split = true,
            "--batch" => o.batch = true,
            "--cleaned" => o.cleaned = true,
            "--out" => o.out = Some(val(&mut i)?),
            "--bits" => o.bits = Some(val(&mut i)?.parse().map_err(|_| "--bits takes 16 or 24")?),
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
    if o.bits.map(|b| b != 16 && b != 24).unwrap_or(false) {
        return Err("--bits takes 16 or 24".into());
    }
    Ok(o)
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

fn job_options(o: &Opts) -> batch::JobOptions {
    batch::JobOptions {
        batch: batch::BatchOptions {
            cleaned: o.cleaned,
            export_root: o.out.clone(),
            bit_depth: o.bits,
            target_lufs: o.target,
            ceiling_dbtp: o.ceiling,
            album_mode: o.album_mode,
            no_level: o.no_level,
        },
        medium: o.medium.clone(),
        deck: o.deck.clone(),
        no_split: o.no_split,
        artist: o.artist.clone(),
        album: o.album.clone(),
        album_artist: o.album_artist.clone(),
        year: o.year.clone(),
        genre: o.genre.clone(),
        cover: o.cover.clone(),
    }
}

fn run_job(job: &[PathBuf], o: &Opts, preset: Option<&presets::Preset>) -> Result<export::ExportResult, String> {
    let (mut p, sources): (Project, Sources) = batch::job_project(job, &job_options(o), preset, &|m| eprintln!("  {m}")).map_err(|e| e.to_string())?;
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
        let regions = auto::split_on_silence(&a, &project::SplitSettings { threshold_db: auto::auto_split_threshold(&a), min_gap_s: 1.2, ..Default::default() });
        println!("  {} track(s) by silence", regions.len());
        println!("  hiss floor {:.1} dBFS", auto::hiss_floor_db(&a));
        if let Some(t) = auto::detect_tone(&a) {
            println!(
                "  reference tone {:.2} Hz at {:.1} dBFS from {:.0} s{}",
                t.freq_hz,
                t.level_dbfs,
                t.start_s,
                t.speed_err_pct.map(|e| format!(", speed {e:+.2}%")).unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn verify(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("verify needs a file".into());
    }
    let mut bad = 0;
    for f in args {
        let path = project::expand_home(f);
        let r = runout_engine::audio::verify_bit_perfect(&path).map_err(|e| e.to_string())?;
        println!("{}", path.display());
        println!("  {} Hz, {} ch, {}-bit container, {:.1} s", r.rate, r.channels, r.bits, r.seconds);
        match r.low_bits_nonzero {
            None => println!("  16-bit file: nothing below 16 bits to check"),
            Some(0) => println!("  low 8 bits all zero: a clean 16-bit source, nothing in the path touched it"),
            Some(n) => {
                bad += 1;
                println!("  NOT BIT-PERFECT: {n} samples have low bits set. Something in the path is processing the audio (resampling, volume, dither), or the source really is 24-bit.");
            }
        }
        if r.errors.is_empty() {
            println!("  no runs of zeros or held samples");
        } else {
            println!("  {} run(s) of zeros or held samples, the marks of uncorrectable errors:", r.errors.len());
            for (t, ms) in r.errors.iter().take(20) {
                println!("    at {t:.3} s, {ms:.2} ms");
            }
            if r.errors.len() > 20 {
                println!("    and {} more", r.errors.len() - 20);
            }
        }
    }
    if bad > 0 {
        Err(format!("{bad} file(s) not bit-perfect"))
    } else {
        Ok(())
    }
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
    let t = selftest::tape_signal(selftest::TAPE_CLIP_S);
    audio::write_wav(&d.join("tape_clip.wav"), t.rate, 24, &t.dirty).map_err(|e| e.to_string())?;
    println!("wrote clicks_sweep.wav, clicks_sweep_clean.wav, clicks_sweep.txt, hum_sweep.wav, tape_clip.wav to {}", d.display());
    Ok(())
}
