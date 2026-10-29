//! Batch mode: load a folder, apply a preset, export. No listening required.

use crate::audio;
use crate::chain::LevelMode;
use crate::export::{self, ExportResult, Sources};
use crate::ops;
use crate::presets::{self, Preset};
use crate::project::{self, Project, SourceKind};
use crate::{auto, media, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct BatchOptions {
    /// Files are already cleaned: no restoration, no leveling.
    pub cleaned: bool,
    pub export_root: Option<String>,
    pub bit_depth: Option<u16>,
    pub target_lufs: Option<f32>,
    pub ceiling_dbtp: Option<f32>,
    pub album_mode: bool,
    pub no_level: bool,
}

/// Build a project from a folder the way the app's folder import does.
pub fn folder_project(dir: &Path, opts: &BatchOptions, preset: Option<&Preset>, log: &(dyn Fn(String) + Sync)) -> Result<(Project, Sources)> {
    folder_project_in(Project::default(), dir, opts, preset, log)
}

/// `folder_project` starting from a project already set up (a medium, say).
pub fn folder_project_in(mut p: Project, dir: &Path, opts: &BatchOptions, preset: Option<&Preset>, log: &(dyn Fn(String) + Sync)) -> Result<(Project, Sources)> {
    let mut sources: Sources = Default::default();
    let mut items = project::plan_folder_import(dir)?;
    if items.is_empty() {
        return Err(crate::Error::Project(format!("{}: no audio files", dir.display())));
    }
    for it in items.iter_mut() {
        it.raw = !opts.cleaned;
    }
    let added = ops::import_items(&mut p, &items, &mut |path| {
        log(format!("Reading {}", Path::new(path).file_name().unwrap_or_default().to_string_lossy()));
        audio::decode_file(Path::new(path)).map(Arc::new)
    })?;
    if !opts.cleaned {
        if let Some((sid, a)) = added.first() {
            ops::auto_new_source(&mut p, *sid, a);
        }
        if let Some(pr) = preset {
            p.chain = presets::apply(&p.chain, pr);
        }
    }
    for (sid, a) in added {
        sources.insert(sid, a);
    }
    apply_options(&mut p, opts);
    Ok((p, sources))
}

pub fn apply_options(p: &mut Project, opts: &BatchOptions) {
    if let Some(v) = opts.target_lufs {
        p.chain.level.target_lufs = v;
    }
    if let Some(v) = opts.ceiling_dbtp {
        p.chain.level.ceiling_dbtp = v.min(0.0);
    }
    if opts.album_mode {
        p.chain.level.mode = LevelMode::Album;
    }
    if opts.no_level {
        p.chain.level.enabled = false;
    }
    if let Some(b) = opts.bit_depth {
        p.export.bit_depth = if b >= 24 { 24 } else { 16 };
    }
    if let Some(r) = &opts.export_root {
        p.export.root = r.clone();
    }
    let lvl = p.chain.level.clone();
    for t in p.tracks.iter_mut() {
        if let (Some(c), false) = (t.chain.as_mut(), t.cleaned) {
            c.level = lvl.clone();
        }
    }
}

/// Folder in, finished album out.
pub fn process_folder(dir: &Path, opts: &BatchOptions, preset: Option<&Preset>, progress: &(dyn Fn(f32, String) + Sync), cancel: &AtomicBool) -> Result<ExportResult> {
    let (mut p, sources) = folder_project(dir, opts, preset, &|m| progress(0.0, m))?;
    export::export(&mut p, Path::new("/"), &sources, progress, cancel)
}

/// Subfolders of `root` that hold audio (directly or in disc subfolders).
pub fn album_folders(root: &Path) -> Vec<std::path::PathBuf> {
    let has_audio = |d: &Path| std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok()).any(|e| project::is_audio(&e.path()))).unwrap_or(false);
    let mut subs: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|d| d.is_dir()).collect())
        .unwrap_or_default();
    subs.sort_by(|a, b| project::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    subs.into_iter()
        .filter(|s| {
            has_audio(s) || std::fs::read_dir(s).map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.path().is_dir() && has_audio(&e.path()))).unwrap_or(false)
        })
        .collect()
}

/// Everything `runout process` takes for one job.
#[derive(Clone, Debug, Default)]
pub struct JobOptions {
    pub batch: BatchOptions,
    /// Medium preset id; None keeps Vinyl LP.
    pub medium: Option<String>,
    pub deck: Option<String>,
    /// Files stay one track per side instead of splitting on silence.
    pub no_split: bool,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub cover: Option<String>,
}

/// Build the project for one headless job: a folder (one album) or a run of
/// files (one side each). Splitting happens here, on the medium's strategy,
/// since nobody is there to press the button.
pub fn job_project(job: &[PathBuf], o: &JobOptions, preset: Option<&Preset>, log: &(dyn Fn(String) + Sync)) -> Result<(Project, Sources)> {
    let mut p = Project::default();
    let mut sources: Sources = Default::default();
    if let Some(id) = &o.medium {
        let m = media::find(id).ok_or_else(|| crate::Error::Project(format!("no medium called {id}; see runout media")))?;
        let deck = match &o.deck {
            Some(d) => Some(media::find_deck(d).ok_or_else(|| crate::Error::Project(format!("no deck profile called {d}")))?),
            None => None,
        };
        media::apply_to_project(&mut p, &m, None, deck.as_ref());
    } else if let Some(d) = &o.deck {
        let deck = media::find_deck(d).ok_or_else(|| crate::Error::Project(format!("no deck profile called {d}")))?;
        let m = p.medium.preset.clone();
        media::apply_to_project(&mut p, &m, None, Some(&deck));
    }
    if job.len() == 1 && job[0].is_dir() {
        let (fp, fs) = folder_project_in(p, &job[0], &o.batch, preset, log)?;
        p = fp;
        sources = fs;
    } else {
        if let Some(pr) = preset {
            p.chain = presets::apply(&p.chain, pr);
        }
        for (n, f) in job.iter().enumerate() {
            log(format!("reading {}", f.display()));
            let a = Arc::new(audio::decode_file(f)?);
            let name = p.medium.preset.side_naming.name(n);
            let sid = ops::add_source(&mut p, &f.to_string_lossy(), SourceKind::File, true, &name, &a);
            ops::auto_new_source(&mut p, sid, &a);
            if o.no_split {
                ops::new_track(&mut p, sid, 0.0, a.duration());
                ops::renumber(&mut p);
            } else {
                if p.split.mode == project::SplitMode::Absolute {
                    p.split.threshold_db = auto::auto_split_threshold(&a);
                }
                ops::split_source(&mut p, sid, &a);
            }
            sources.insert(sid, a);
        }
        if let Some(pr) = preset {
            p.chain = presets::apply(&p.chain, pr);
        }
        // tape and digital measurements (wow, azimuth, best channel, errors)
        let mut ids: Vec<u64> = sources.keys().copied().collect();
        ids.sort();
        for sid in ids {
            let a = sources[&sid].clone();
            for n in auto::analyze_media(&mut p, sid, &a) {
                log(n);
            }
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
    apply_options(&mut p, &o.batch);
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
    Ok((p, sources))
}
