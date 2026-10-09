//! Batch mode: load a folder, apply a preset, export. No listening required.

use crate::audio;
use crate::chain::LevelMode;
use crate::export::{self, ExportResult, Sources};
use crate::ops;
use crate::presets::{self, Preset};
use crate::project::{self, Project};
use crate::Result;
use std::path::Path;
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
    let mut p = Project::default();
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
