//! Tauri commands. The UI talks to the project model and the transport only
//! through these.

use crate::capture;
use crate::lookup;
use crate::player::Cmd;
use crate::prefs::projects_root;
use crate::state::{progress, App, Inner, ProjectView};
use runout_engine::audio::{self, SourceAudio};
use runout_engine::chain::{Chain, Repair};
use runout_engine::project::{self, ImportItem, Project, SourceKind, AUTOSAVE_FILE, PROJECT_FILE};
use runout_engine::{auto, batch, export, ops, presets, render, tiles};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

fn app_state(h: &AppHandle) -> State<'_, App> {
    h.state::<App>()
}

// ---------------------------------------------------------------- project

#[derive(Serialize)]
pub struct AppInfo {
    version: String,
    projects_root: String,
    recovery: Option<Recovery>,
    recent: Vec<String>,
    music_root: String,
}

#[derive(Serialize)]
pub struct Recovery {
    project: Option<String>,
    autosave_newer: bool,
    capture: Option<String>,
    capture_name: Option<String>,
}

#[tauri::command]
pub fn app_info(st: State<'_, App>) -> AppInfo {
    let rec = st.recovery.lock().unwrap().clone().map(|s| {
        let autosave_newer = s
            .open_project
            .as_ref()
            .map(|d| {
                let d = PathBuf::from(d);
                let m = |f: &str| std::fs::metadata(d.join(f)).and_then(|m| m.modified()).ok();
                match (m(AUTOSAVE_FILE), m(PROJECT_FILE)) {
                    (Some(a), Some(p)) => a > p,
                    (Some(_), None) => true,
                    _ => false,
                }
            })
            .unwrap_or(false);
        Recovery { project: s.open_project.clone(), autosave_newer, capture: s.capture.clone().filter(|c| Path::new(c).exists()), capture_name: s.capture_name.clone() }
    });
    let home = std::env::var("HOME").unwrap_or_default();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        projects_root: projects_root().to_string_lossy().to_string(),
        recovery: rec,
        recent: st.prefs.lock().unwrap().recent.clone(),
        music_root: format!("{home}/Music"),
    }
}

#[tauri::command]
pub fn recovery_dismiss(st: State<'_, App>) {
    *st.recovery.lock().unwrap() = None;
}

fn mark_open(st: &App, dir: &Path) {
    let mut s = st.session.lock().unwrap();
    s.open_project = Some(dir.to_string_lossy().to_string());
    s.save();
    st.prefs.lock().unwrap().add_recent(&dir.to_string_lossy());
}

/// Called on a clean exit.
pub fn mark_closed(st: &App) {
    let mut s = st.session.lock().unwrap();
    s.open_project = None;
    s.capture = None;
    s.save();
}

#[tauri::command]
pub fn project_new(st: State<'_, App>, name: String, parent: Option<String>) -> R<ProjectView> {
    if st.capture.lock().unwrap().is_some() {
        return Err("stop the capture first".into());
    }
    let name = export::safe_name(name.trim());
    let parent = parent.map(PathBuf::from).unwrap_or_else(projects_root);
    let dir = parent.join(format!("{name}.runout"));
    if dir.join(PROJECT_FILE).exists() {
        return Err(format!("{} already exists; open it instead", dir.display()));
    }
    std::fs::create_dir_all(dir.join("captures")).map_err(e)?;
    let p = Project { name: name.clone(), ..Default::default() };
    p.save(&dir).map_err(e)?;
    let mut inner = st.inner.lock().unwrap();
    reset(&mut inner, p, Some(dir.clone()));
    st.push_ctx(&inner);
    st.player.send(Cmd::Pause);
    mark_open(&st, &dir);
    Ok(st.view(&inner))
}

fn reset(inner: &mut Inner, p: Project, dir: Option<PathBuf>) {
    inner.project = p;
    inner.dir = dir;
    inner.undo.clear();
    inner.redo.clear();
    inner.dirty = false;
    inner.audio.clear();
    inner.clicks.clear();
    inner.peaks.clear();
    inner.gen += 1;
}

fn load_sources(app: &AppHandle, job: &str, p: &Project, dir: &Path) -> R<std::collections::HashMap<u64, Arc<SourceAudio>>> {
    let mut map = std::collections::HashMap::new();
    let n = p.sources.len().max(1);
    for (i, s) in p.sources.iter().enumerate() {
        progress(app, job, i as f32 / n as f32, &format!("Loading {}", s.name));
        let path = p.resolve(dir, &s.path);
        let a = audio::decode_file(&path).map_err(e)?;
        map.insert(s.id, Arc::new(a));
    }
    Ok(map)
}

#[tauri::command]
pub fn project_open(app: AppHandle, st: State<'_, App>, dir: String, autosave: Option<bool>) -> R<()> {
    if st.capture.lock().unwrap().is_some() {
        return Err("stop the capture first".into());
    }
    st.begin_job("open")?;
    std::thread::spawn(move || {
        let st = app_state(&app);
        let r = (|| -> R<serde_json::Value> {
            let mut d = PathBuf::from(&dir);
            if d.file_name().map(|f| f == PROJECT_FILE).unwrap_or(false) {
                d.pop();
            }
            let file = if autosave.unwrap_or(false) && d.join(AUTOSAVE_FILE).exists() { d.join(AUTOSAVE_FILE) } else { d.join(PROJECT_FILE) };
            let p = Project::load_file(&file).map_err(e)?;
            let audio = load_sources(&app, "open", &p, &d)?;
            let mut inner = st.inner.lock().unwrap();
            reset(&mut inner, p, Some(d.clone()));
            inner.audio = audio;
            inner.dirty = autosave.unwrap_or(false);
            st.player.send(Cmd::Pause);
            st.push_ctx(&inner);
            mark_open(&st, &d);
            Ok(serde_json::to_value(st.view(&inner)).map_err(e)?)
        })();
        st.end_job(&app, "open", r);
    });
    Ok(())
}

#[tauri::command]
pub fn project_get(st: State<'_, App>) -> ProjectView {
    let inner = st.inner.lock().unwrap();
    st.view(&inner)
}

#[tauri::command]
pub fn project_save(st: State<'_, App>) -> R<ProjectView> {
    let mut inner = st.inner.lock().unwrap();
    let dir = inner.dir.clone().ok_or("no project folder")?;
    inner.project.save(&dir).map_err(e)?;
    inner.dirty = false;
    Ok(st.view(&inner))
}

#[tauri::command]
pub fn project_set(st: State<'_, App>, project: Project, label: String) -> ProjectView {
    let mut inner = st.inner.lock().unwrap();
    st.commit(&mut inner, project, &label);
    st.view(&inner)
}

#[tauri::command]
pub fn undo(st: State<'_, App>) -> ProjectView {
    let mut inner = st.inner.lock().unwrap();
    if let Some((p, _, _)) = inner.undo.pop() {
        let cur = std::mem::replace(&mut inner.project, p);
        inner.redo.push(cur);
        inner.dirty = true;
        inner.gen += 1;
        st.push_ctx(&inner);
    }
    st.view(&inner)
}

#[tauri::command]
pub fn redo(st: State<'_, App>) -> ProjectView {
    let mut inner = st.inner.lock().unwrap();
    if let Some(p) = inner.redo.pop() {
        let cur = std::mem::replace(&mut inner.project, p);
        inner.undo.push((cur, String::new(), std::time::Instant::now()));
        inner.dirty = true;
        inner.gen += 1;
        st.push_ctx(&inner);
    }
    st.view(&inner)
}

/// Apply an edit closure to a copy of the project and commit it.
fn edit(st: &App, label: &str, f: impl FnOnce(&mut Project, &Inner) -> R<()>) -> R<ProjectView> {
    let mut inner = st.inner.lock().unwrap();
    let mut p = inner.project.clone();
    f(&mut p, &inner)?;
    st.commit(&mut inner, p, label);
    Ok(st.view(&inner))
}

// ---------------------------------------------------------------- import

fn need_dir(inner: &Inner) -> R<PathBuf> {
    inner.dir.clone().ok_or_else(|| "create or open a project first".into())
}

#[tauri::command]
pub fn import_files(app: AppHandle, st: State<'_, App>, paths: Vec<String>) -> R<()> {
    need_dir(&st.inner.lock().unwrap())?;
    st.begin_job("import")?;
    std::thread::spawn(move || {
        let st = app_state(&app);
        let r = (|| -> R<serde_json::Value> {
            let n = paths.len().max(1);
            let mut loaded = Vec::new();
            for (i, path) in paths.iter().enumerate() {
                progress(&app, "import", i as f32 / n as f32, &format!("Reading {}", Path::new(path).file_name().unwrap_or_default().to_string_lossy()));
                let a = Arc::new(audio::decode_file(Path::new(path)).map_err(e)?);
                loaded.push((path.clone(), a));
            }
            progress(&app, "import", 0.9, "Analysing");
            let mut inner = st.inner.lock().unwrap();
            let mut p = inner.project.clone();
            for (path, a) in loaded {
                let n_sides = p.sources.iter().filter(|s| s.kind != SourceKind::Folder).count();
                let name = format!("Side {}", (b'A' + (n_sides as u8).min(25)) as char);
                let sid = ops::add_source(&mut p, &path, SourceKind::File, true, &name, &a);
                ops::auto_new_source(&mut p, sid, &a);
                if p.sources.len() == 1 {
                    p.split.threshold_db = auto::auto_split_threshold(&a);
                }
                ops::split_source(&mut p, sid, &a);
                inner.audio.insert(sid, a);
            }
            st.commit(&mut inner, p, "Import files");
            Ok(serde_json::to_value(st.view(&inner)).map_err(e)?)
        })();
        st.end_job(&app, "import", r);
    });
    Ok(())
}

#[tauri::command]
pub fn folder_plan(dir: String) -> R<Vec<ImportItem>> {
    project::plan_folder_import(Path::new(&dir)).map_err(e)
}

#[tauri::command]
pub fn folder_commit(app: AppHandle, st: State<'_, App>, items: Vec<ImportItem>) -> R<()> {
    need_dir(&st.inner.lock().unwrap())?;
    st.begin_job("folder")?;
    std::thread::spawn(move || {
        let st = app_state(&app);
        let r = (|| -> R<serde_json::Value> {
            let mut p = st.inner.lock().unwrap().project.clone();
            let n = items.len().max(1);
            let mut i = 0usize;
            let added = ops::import_items(&mut p, &items, &mut |path| {
                progress(&app, "folder", i as f32 / n as f32, &format!("Reading {}", Path::new(path).file_name().unwrap_or_default().to_string_lossy()));
                i += 1;
                audio::decode_file(Path::new(path)).map(Arc::new)
            })
            .map_err(e)?;
            if let Some((sid, a)) = added.iter().find(|(sid, _)| p.source(*sid).map(|s| s.raw).unwrap_or(false)) {
                ops::auto_new_source(&mut p, *sid, a);
            }
            let mut inner = st.inner.lock().unwrap();
            for (sid, a) in added {
                inner.audio.insert(sid, a);
            }
            st.commit(&mut inner, p, "Import folder");
            Ok(serde_json::to_value(st.view(&inner)).map_err(e)?)
        })();
        st.end_job(&app, "folder", r);
    });
    Ok(())
}

#[tauri::command]
pub fn remove_source(st: State<'_, App>, source_id: u64) -> R<ProjectView> {
    edit(&st, "Remove side", |p, _| {
        p.sources.retain(|s| s.id != source_id);
        p.tracks.retain(|t| t.source_id != source_id);
        ops::renumber(p);
        Ok(())
    })
}

// ---------------------------------------------------------------- split

#[tauri::command]
pub fn split_auto(st: State<'_, App>, source_id: Option<u64>) -> R<ProjectView> {
    edit(&st, "Auto-split", |p, inner| {
        let ids: Vec<u64> = p.sources.iter().filter(|s| s.kind != SourceKind::Folder).map(|s| s.id).filter(|id| source_id.map(|x| x == *id).unwrap_or(true)).collect();
        for id in ids {
            if let Some(a) = inner.audio.get(&id) {
                ops::split_source(p, id, a);
            }
        }
        Ok(())
    })
}

#[tauri::command]
pub fn split_auto_threshold(st: State<'_, App>) -> R<ProjectView> {
    edit(&st, "Split threshold", |p, inner| {
        let first = p.sources.iter().find(|s| s.kind != SourceKind::Folder).map(|s| s.id).ok_or("no captured or imported side")?;
        let a = inner.audio.get(&first).ok_or("audio not loaded")?;
        p.split.threshold_db = auto::auto_split_threshold(a);
        Ok(())
    })
}

#[tauri::command]
pub fn mark_boundary(st: State<'_, App>, at: f64) -> R<ProjectView> {
    edit(&st, "Mark track end", |p, inner| {
        let (sid, s) = App::to_source(inner, at).ok_or("the playhead is not on a side")?;
        if !p.tracks.iter().any(|t| t.source_id == sid && t.start_s < s && t.end_s > s) {
            // between tracks: start a new track here up to the next one or the end
            let src = p.source(sid).ok_or("no side")?;
            let end = p.tracks.iter().filter(|t| t.source_id == sid && t.start_s > s).map(|t| t.start_s).fold(src.frames as f64 / src.rate as f64, f64::min);
            ops::new_track(p, sid, s, end);
            ops::renumber(p);
            return Ok(());
        }
        ops::mark_boundary(p, sid, s).map(|_| ()).ok_or_else(|| "too close to a track edge".into())
    })
}

#[tauri::command]
pub fn add_track(st: State<'_, App>, from: f64, to: f64) -> R<ProjectView> {
    edit(&st, "Add track", |p, inner| {
        let (sid, a) = App::to_source(inner, from.min(to)).ok_or("not on a side")?;
        let (sid2, b) = App::to_source(inner, from.max(to)).ok_or("not on a side")?;
        if sid != sid2 {
            return Err("a track cannot span two sides".into());
        }
        ops::new_track(p, sid, a, b);
        ops::renumber(p);
        Ok(())
    })
}

#[tauri::command]
pub fn merge_next(st: State<'_, App>, track_id: u64) -> R<ProjectView> {
    edit(&st, "Merge tracks", |p, _| if ops::merge_with_next(p, track_id) { Ok(()) } else { Err("there is no next track on the same side".into()) })
}

#[tauri::command]
pub fn delete_track(st: State<'_, App>, track_id: u64) -> R<ProjectView> {
    edit(&st, "Delete track", |p, _| {
        p.tracks.retain(|t| t.id != track_id);
        ops::renumber(p);
        Ok(())
    })
}

#[tauri::command]
pub fn set_cleaned(st: State<'_, App>, track_id: u64, cleaned: bool) -> R<ProjectView> {
    edit(&st, "Raw/Cleaned", |p, _| {
        let sid = {
            let t = p.tracks.iter_mut().find(|t| t.id == track_id).ok_or("no such track")?;
            ops::set_cleaned(t, cleaned);
            t.source_id
        };
        if let Some(s) = p.source_mut(sid) {
            if s.kind == SourceKind::Folder {
                s.raw = !cleaned;
            }
        }
        Ok(())
    })
}

#[tauri::command]
pub fn set_override(st: State<'_, App>, track_id: u64, on: bool) -> R<ProjectView> {
    edit(&st, "Track settings", |p, _| {
        let chain = p.chain.clone();
        let t = p.tracks.iter_mut().find(|t| t.id == track_id).ok_or("no such track")?;
        t.chain = if on { Some(chain) } else { None };
        if !on {
            t.cleaned = false;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------- clean

fn chain_of<'a>(p: &'a mut Project, track_id: Option<u64>) -> &'a mut Chain {
    if let Some(id) = track_id {
        let has = p.tracks.iter().any(|t| t.id == id && t.chain.is_some());
        if has {
            return p.tracks.iter_mut().find(|t| t.id == id).unwrap().chain.as_mut().unwrap();
        }
    }
    &mut p.chain
}

fn analysis_source(p: &Project, inner: &Inner, track_id: Option<u64>) -> Option<(u64, Arc<SourceAudio>)> {
    let sid = track_id
        .and_then(|t| p.track(t).map(|t| t.source_id))
        .or_else(|| p.sources.iter().find(|s| s.raw).map(|s| s.id))
        .or_else(|| p.sources.first().map(|s| s.id))?;
    inner.audio.get(&sid).cloned().map(|a| (sid, a))
}

#[tauri::command]
pub fn auto_stage(st: State<'_, App>, stage: String, track_id: Option<u64>) -> R<ProjectView> {
    edit(&st, &format!("Auto {stage}"), |p, inner| {
        let (sid, a) = analysis_source(p, inner, track_id).ok_or("add some audio first")?;
        match stage.as_str() {
            "rumble" => auto::auto_rumble(&a, chain_of(p, track_id)),
            "hum" => {
                let c = chain_of(p, track_id);
                match auto::detect_hum(&a) {
                    Some(f) => {
                        c.hum.enabled = true;
                        c.hum.mains_hz = f;
                    }
                    None => c.hum.enabled = false,
                }
            }
            "declick" => auto::auto_declick(&a, chain_of(p, track_id)),
            "decrackle" => auto::auto_decrackle(&a, chain_of(p, track_id)),
            "declip" => {
                let c = chain_of(p, track_id);
                match auto::detect_clipping(&a) {
                    Some(l) => {
                        c.declip.enabled = true;
                        c.declip.level_db = l;
                    }
                    None => c.declip.enabled = false,
                }
            }
            "denoise" => {
                let prof = auto::learn_noise(&a);
                if let Some(s) = p.source_mut(sid) {
                    s.fx.noise = prof;
                }
                let fx = p.source(sid).map(|s| s.fx.clone()).unwrap_or_default();
                auto::auto_denoise(&fx, chain_of(p, track_id));
            }
            "channel" => {
                let c = chain_of(p, track_id);
                let az = auto::detect_azimuth(&a).unwrap_or(0.0);
                let l = a.read_channel(0, 0, a.frames as i64);
                let r = a.read_channel(1.min(a.channels - 1), 0, a.frames as i64);
                let rms = |v: &[f32]| (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt();
                let d = 20.0 * (rms(&l).max(1e-9) / rms(&r).max(1e-9)).log10();
                c.channel.azimuth_us = if az.abs() > 5.0 { (az * 10.0).round() / 10.0 } else { 0.0 };
                c.channel.balance_db = if d.abs() > 0.5 { (d as f32 * 10.0).round() / 10.0 } else { 0.0 };
                c.channel.swap = false;
                c.channel.enabled = c.channel.azimuth_us != 0.0 || c.channel.balance_db != 0.0;
            }
            "eq" => {
                let c = chain_of(p, track_id);
                c.eq = Default::default();
            }
            "level" => {
                let c = chain_of(p, track_id);
                c.level = Default::default();
            }
            other => return Err(format!("no auto for {other}")),
        }
        Ok(())
    })
}

#[tauri::command]
pub fn learn_noise(st: State<'_, App>, from: f64, to: f64) -> R<ProjectView> {
    edit(&st, "Learn noise", |p, inner| {
        let (sid, a) = App::to_source(inner, from.min(to)).ok_or("select some noise on a side")?;
        let (_, b) = App::to_source(inner, from.max(to)).ok_or("select some noise on a side")?;
        let audio = inner.audio.get(&sid).ok_or("audio not loaded")?;
        let rate = audio.rate as f64;
        if b - a < 0.2 {
            return Err("select at least 0.2 s of noise".into());
        }
        let prof = auto::learn_noise_at(audio, (a * rate) as usize, (b * rate) as usize).ok_or("could not learn a profile there")?;
        p.source_mut(sid).ok_or("no side")?.fx.noise = Some(prof);
        p.chain.denoise.enabled = true;
        Ok(())
    })
}

#[tauri::command]
pub fn add_repair(st: State<'_, App>, from: f64, to: f64, band: Option<(f32, f32)>) -> R<ProjectView> {
    edit(&st, "Repair", |p, inner| {
        let (sid, a) = App::to_source(inner, from.min(to)).ok_or("select a region on a side")?;
        let (_, b) = App::to_source(inner, from.max(to)).ok_or("select a region on a side")?;
        if b - a > 5.0 {
            return Err("repair regions are limited to 5 s".into());
        }
        let id = p.new_id();
        p.source_mut(sid).ok_or("no side")?.fx.repairs.push(Repair { id, start_s: a, end_s: b, band });
        Ok(())
    })
}

#[tauri::command]
pub fn remove_repair(st: State<'_, App>, repair_id: u64) -> R<ProjectView> {
    edit(&st, "Remove repair", |p, _| {
        for s in p.sources.iter_mut() {
            s.fx.repairs.retain(|r| r.id != repair_id);
        }
        Ok(())
    })
}

#[tauri::command]
pub fn analyze(app: AppHandle, st: State<'_, App>, all: bool) -> R<()> {
    st.begin_job("analyze")?;
    std::thread::spawn(move || {
        let st = app_state(&app);
        let r = (|| -> R<serde_json::Value> {
            let (p, audio) = {
                let inner = st.inner.lock().unwrap();
                (inner.project.clone(), inner.audio.clone())
            };
            let order: Vec<u64> = p.ordered_tracks().iter().filter(|t| all || !p.analysis_fresh(t)).map(|t| t.id).collect();
            let n = order.len().max(1);
            let mut results = Vec::new();
            for (i, id) in order.iter().enumerate() {
                if st.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let t = p.track(*id).ok_or("track vanished")?;
                progress(&app, "analyze", i as f32 / n as f32, &format!("Analysing {}", if t.title.is_empty() { format!("track {}", t.number.unwrap_or(0)) } else { t.title.clone() }));
                let a = audio.get(&t.source_id).ok_or("audio not loaded")?;
                if let Some(an) = render::analyze_track(&p, t, a) {
                    results.push((*id, t.source_id, an));
                }
            }
            let mut inner = st.inner.lock().unwrap();
            for (id, sid, an) in results {
                // keep the result only if the track still matches what was analysed
                let key_now = inner.project.track(id).map(|t| inner.project.track_key(t));
                if key_now == Some(an.analysis.key) {
                    if let Some(t) = inner.project.tracks.iter_mut().find(|t| t.id == id) {
                        t.analysis = Some(an.analysis.clone());
                    }
                }
                let clicks = inner.clicks.entry(sid).or_default();
                let (lo, hi) = (an.start, an.start + (an.peaks.len() * runout_engine::audio::PEAK_BUCKET) as i64);
                clicks.retain(|c| c.pos < lo || c.pos >= hi);
                clicks.extend(an.clicks.iter().copied());
                clicks.sort_by_key(|c| c.pos);
                let frames = inner.project.source(sid).map(|s| runout_engine::chain::processed_frames(inner.audio.get(&sid).unwrap(), &s.fx)).unwrap_or(0);
                let pk = inner.peaks.entry(sid).or_insert_with(Vec::new);
                let nb = frames.div_ceil(runout_engine::audio::PEAK_BUCKET);
                if pk.len() != nb {
                    *pk = vec![(0.0, 0.0); nb];
                }
                let b0 = (an.start.max(0) as usize) / runout_engine::audio::PEAK_BUCKET;
                for (j, v) in an.peaks.iter().enumerate() {
                    if let Some(slot) = pk.get_mut(b0 + j) {
                        *slot = *v;
                    }
                }
            }
            inner.dirty = true;
            inner.gen += 1;
            st.push_ctx(&inner);
            Ok(serde_json::to_value(st.view(&inner)).map_err(e)?)
        })();
        st.end_job(&app, "analyze", r);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_job(st: State<'_, App>) {
    st.cancel.store(true, Ordering::Relaxed);
}

/// Clicks for the click map: [global seconds, size dBFS, crackle].
#[tauri::command]
pub fn clicks(st: State<'_, App>) -> Vec<(f64, f32, bool)> {
    let inner = st.inner.lock().unwrap();
    let layout = inner.project.layout();
    let mut out = Vec::new();
    for l in &layout {
        if let Some(v) = inner.clicks.get(&l.source_id) {
            for c in v {
                out.push((l.start + c.pos as f64 / l.rate as f64, 20.0 * c.size.max(1e-6).log10(), c.crackle));
            }
        }
    }
    out
}

#[tauri::command]
pub fn presets_list() -> Vec<presets::Preset> {
    presets::list()
}

#[tauri::command]
pub fn preset_apply(st: State<'_, App>, id: String, track_id: Option<u64>) -> R<ProjectView> {
    let pr = presets::find(&id).ok_or("no such preset")?;
    edit(&st, "Apply preset", |p, _| {
        let c = chain_of(p, track_id);
        *c = presets::apply(c, &pr);
        Ok(())
    })
}

#[tauri::command]
pub fn preset_save(st: State<'_, App>, name: String, track_id: Option<u64>) -> R<Vec<presets::Preset>> {
    let inner = st.inner.lock().unwrap();
    let chain = track_id.and_then(|t| inner.project.track(t).and_then(|t| t.chain.clone())).unwrap_or_else(|| inner.project.chain.clone());
    presets::save(&name, &chain).map_err(e)?;
    Ok(presets::list())
}

#[tauri::command]
pub fn preset_delete(id: String) -> R<Vec<presets::Preset>> {
    presets::delete(&id).map_err(e)?;
    Ok(presets::list())
}

// ---------------------------------------------------------------- export

#[tauri::command]
pub fn export_run(app: AppHandle, st: State<'_, App>) -> R<()> {
    st.begin_job("export")?;
    std::thread::spawn(move || {
        let st = app_state(&app);
        let r = (|| -> R<serde_json::Value> {
            let (mut p, audio, dir) = {
                let inner = st.inner.lock().unwrap();
                (inner.project.clone(), inner.audio.clone(), inner.dir.clone().unwrap_or_else(|| PathBuf::from("/")))
            };
            let a2 = app.clone();
            let res = export::export(&mut p, &dir, &audio, &move |f, m| progress(&a2, "export", f, &m), &st.cancel).map_err(e)?;
            // keep refreshed analysis (ceiling fixes, after-loudness)
            let mut inner = st.inner.lock().unwrap();
            for t in p.tracks {
                if let Some(cur) = inner.project.tracks.iter_mut().find(|x| x.id == t.id) {
                    cur.analysis = t.analysis;
                }
            }
            inner.dirty = true;
            if let Some(d) = inner.dir.clone() {
                let _ = inner.project.save(&d);
                inner.dirty = false;
            }
            Ok(serde_json::to_value(&res).map_err(e)?)
        })();
        st.end_job(&app, "export", r);
    });
    Ok(())
}

#[tauri::command]
pub fn open_folder(path: String) -> R<()> {
    std::process::Command::new("xdg-open").arg(&path).spawn().map_err(e)?;
    Ok(())
}

#[tauri::command]
pub fn read_report(path: String) -> R<String> {
    std::fs::read_to_string(path).map_err(e)
}

// ---------------------------------------------------------------- lookup

#[tauri::command]
pub async fn lookup_search(artist: String, album: String) -> R<Vec<lookup::ReleaseHit>> {
    tauri::async_runtime::spawn_blocking(move || lookup::search(&artist, &album)).await.map_err(e)?
}

#[tauri::command]
pub async fn lookup_release(id: String) -> R<lookup::Release> {
    tauri::async_runtime::spawn_blocking(move || lookup::release(&id)).await.map_err(e)?
}

#[derive(Deserialize)]
pub struct TracklistEntry {
    pub title: String,
    pub length_s: Option<f64>,
    pub side: Option<String>,
}

#[tauri::command]
pub fn apply_tracklist(st: State<'_, App>, tracks: Vec<TracklistEntry>, snap: bool, artist: String, album: String, year: String, genre: String) -> R<ProjectView> {
    edit(&st, "Tracklist", |p, inner| {
        if !artist.is_empty() {
            p.album.artist = artist.clone();
        }
        if !album.is_empty() {
            p.album.album = album.clone();
        }
        if !year.is_empty() {
            p.album.year = year.clone();
        }
        if !genre.is_empty() && p.album.genre.is_empty() {
            p.album.genre = genre.clone();
        }
        let sides: Vec<u64> = p.sources.iter().map(|s| s.id).collect();
        // Group the tracklist by side letter when the release has them and they
        // line up with the sides in the project.
        let letters: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for t in &tracks {
                if let Some(s) = &t.side {
                    if !v.contains(s) {
                        v.push(s.clone());
                    }
                }
            }
            v
        };
        let by_side = tracks.iter().all(|t| t.side.is_some()) && letters.len() == sides.len() && sides.len() > 1;
        let mut groups: Vec<(u64, Vec<&TracklistEntry>)> = Vec::new();
        if by_side {
            for (i, sid) in sides.iter().enumerate() {
                groups.push((*sid, tracks.iter().filter(|t| t.side.as_deref() == Some(letters[i].as_str())).collect()));
            }
        } else {
            let mut it = tracks.iter();
            for sid in &sides {
                let count = p.tracks.iter().filter(|t| t.source_id == *sid).count();
                let take: Vec<&TracklistEntry> = if *sid == *sides.last().unwrap() { it.by_ref().collect() } else { it.by_ref().take(count).collect() };
                groups.push((*sid, take));
            }
        }
        for (sid, list) in groups {
            if list.is_empty() {
                continue;
            }
            let lengths: Vec<f64> = list.iter().filter_map(|t| t.length_s).collect();
            let can_snap = snap && lengths.len() == list.len() && p.source(sid).map(|s| s.kind != SourceKind::Folder).unwrap_or(false);
            if can_snap {
                let a = inner.audio.get(&sid).ok_or("audio not loaded")?;
                let first = p.tracks.iter().filter(|t| t.source_id == sid).map(|t| t.start_s).fold(f64::MAX, f64::min);
                let first = if first == f64::MAX { 0.0 } else { first };
                let bounds = auto::snap_to_lengths(a, first, &lengths);
                p.tracks.retain(|t| t.source_id != sid);
                for (i, (s, e2)) in bounds.into_iter().enumerate() {
                    let id = ops::new_track(p, sid, s, e2);
                    if let Some(t) = p.tracks.iter_mut().find(|t| t.id == id) {
                        t.title = list.get(i).map(|x| x.title.clone()).unwrap_or_default();
                    }
                }
            } else {
                let mut ids: Vec<(f64, u64)> = p.tracks.iter().filter(|t| t.source_id == sid).map(|t| (t.start_s, t.id)).collect();
                ids.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                for ((_, id), entry) in ids.iter().zip(list.iter()) {
                    if let Some(t) = p.tracks.iter_mut().find(|t| t.id == *id) {
                        t.title = entry.title.clone();
                    }
                }
            }
        }
        ops::renumber(p);
        Ok(())
    })
}

// ---------------------------------------------------------------- capture

#[derive(Serialize)]
pub struct Devices {
    inputs: Vec<capture::InputDevice>,
    selected: Option<String>,
}

#[tauri::command]
pub async fn devices(app: AppHandle) -> Devices {
    let remembered = app_state(&app).prefs.lock().unwrap().input_device.clone();
    let inputs = tauri::async_runtime::spawn_blocking(capture::list_inputs).await.unwrap_or_default();
    let selected = capture::pick_default(&inputs, remembered.as_deref());
    Devices { inputs, selected }
}

#[tauri::command]
pub fn select_device(st: State<'_, App>, id: String) {
    let mut p = st.prefs.lock().unwrap();
    p.input_device = Some(id);
    p.save();
}

#[tauri::command]
pub fn level_check(st: State<'_, App>, device: Option<String>) -> R<()> {
    let mut cap = st.capture.lock().unwrap();
    if cap.is_some() {
        return Err("already capturing".into());
    }
    *cap = Some(capture::start(capture::Opts { device, path: None, auto_stop_s: None, max_s: Some(20.0) }, "Level check".into())?);
    Ok(())
}

#[tauri::command]
pub fn capture_start(st: State<'_, App>, name: String, device: Option<String>, auto_stop_s: Option<f32>) -> R<()> {
    let dir = need_dir(&st.inner.lock().unwrap())?;
    let mut cap = st.capture.lock().unwrap();
    if cap.is_some() {
        return Err("already capturing".into());
    }
    st.player.send(Cmd::Pause);
    let fname = export::safe_name(&name);
    let mut path = dir.join("captures").join(format!("{fname}.wav"));
    let mut n = 2;
    while path.exists() {
        path = dir.join("captures").join(format!("{fname} ({n}).wav"));
        n += 1;
    }
    *cap = Some(capture::start(capture::Opts { device, path: Some(path.clone()), auto_stop_s, max_s: Some(60.0 * 60.0) }, name.clone())?);
    let mut s = st.session.lock().unwrap();
    s.capture = Some(path.to_string_lossy().to_string());
    s.capture_name = Some(name);
    s.save();
    Ok(())
}

#[derive(Serialize)]
pub struct CaptureResult {
    pub level_check: bool,
    pub peak_db: f32,
    pub seconds: f64,
    pub clipped: bool,
    pub auto_stopped: bool,
    pub view: Option<ProjectView>,
}

/// Stop (or collect a finished) capture or level check.
#[tauri::command]
pub async fn capture_stop(app: AppHandle) -> R<CaptureResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app_state(&app);
        let mut cap = st.capture.lock().unwrap().take().ok_or("not capturing")?;
        cap.stop.store(true, Ordering::Relaxed);
        let done = cap.thread.take().ok_or("capture thread missing")?.join().map_err(|_| "capture thread crashed")??;
        let seconds = done.frames as f64 / done.rate.max(1) as f64;
        let Some(path) = done.path.clone() else {
            return Ok(CaptureResult { level_check: true, peak_db: done.peak_db, seconds, clipped: done.clipped, auto_stopped: false, view: None });
        };
        let view = add_capture(&st, &path, &cap.name)?;
        Ok(CaptureResult { level_check: false, peak_db: done.peak_db, seconds, clipped: done.clipped, auto_stopped: done.auto_stopped, view: Some(view) })
    })
    .await
    .map_err(e)?
}

fn add_capture(st: &App, path: &Path, name: &str) -> R<ProjectView> {
    let a = Arc::new(audio::decode_file(path).map_err(e)?);
    let mut inner = st.inner.lock().unwrap();
    let dir = inner.dir.clone().ok_or("no project")?;
    let rel = path.strip_prefix(&dir).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| path.to_string_lossy().to_string());
    let mut p = inner.project.clone();
    let sid = ops::add_source(&mut p, &rel, SourceKind::Capture, true, name, &a);
    ops::auto_new_source(&mut p, sid, &a);
    if p.sources.len() == 1 {
        p.split.threshold_db = auto::auto_split_threshold(&a);
    }
    ops::split_source(&mut p, sid, &a);
    inner.audio.insert(sid, a);
    st.commit(&mut inner, p, "Capture");
    let _ = inner.project.save(&dir);
    inner.dirty = false;
    let mut s = st.session.lock().unwrap();
    s.capture = None;
    s.capture_name = None;
    s.save();
    Ok(st.view(&inner))
}

/// After a crash: repair the half-written capture's header and add it.
#[tauri::command]
pub fn recover_capture(st: State<'_, App>, path: String, name: Option<String>) -> R<ProjectView> {
    let p = PathBuf::from(&path);
    audio::repair_wav_header(&p).map_err(e)?;
    *st.recovery.lock().unwrap() = None;
    add_capture(&st, &p, name.as_deref().unwrap_or("Recovered side"))
}

#[derive(Serialize)]
pub struct CaptureStatus {
    pub active: bool,
    pub level_check: bool,
    pub finished: bool,
    pub level: Option<capture::LevelReading>,
}

pub fn capture_status_of(st: &App) -> CaptureStatus {
    let cap = st.capture.lock().unwrap();
    match cap.as_ref() {
        Some(c) => CaptureStatus {
            active: true,
            level_check: c.path.is_none(),
            finished: c.thread.as_ref().map(|t| t.is_finished()).unwrap_or(true),
            level: Some(c.level.read()),
        },
        None => CaptureStatus { active: false, level_check: false, finished: false, level: None },
    }
}

// ---------------------------------------------------------------- transport

#[tauri::command]
pub fn play(st: State<'_, App>, from: f64) {
    st.player.send(Cmd::Play(from));
}

#[tauri::command]
pub fn pause(st: State<'_, App>) {
    st.player.send(Cmd::Pause);
}

#[tauri::command]
pub fn seek(st: State<'_, App>, to: f64) {
    st.player.send(Cmd::Seek(to));
}

#[tauri::command]
pub fn set_loop(st: State<'_, App>, range: Option<(f64, f64)>) {
    st.player.send(Cmd::Loop(range));
}

#[tauri::command]
pub fn set_monitor(st: State<'_, App>, mode: u8) {
    st.player.shared.monitor.store(mode.min(2), Ordering::Relaxed);
}

// ---------------------------------------------------------------- batch

#[tauri::command]
pub fn batch_plan(root: String) -> Vec<String> {
    let p = PathBuf::from(&root);
    let mut v: Vec<String> = batch::album_folders(&p).into_iter().map(|d| d.to_string_lossy().to_string()).collect();
    let direct = std::fs::read_dir(&p).map(|rd| rd.filter_map(|e| e.ok()).any(|e| project::is_audio(&e.path()))).unwrap_or(false);
    if v.is_empty() || direct {
        v.insert(0, root);
    }
    v
}

#[tauri::command]
pub fn batch_run(app: AppHandle, st: State<'_, App>, folders: Vec<String>, preset: Option<String>, opts: batch::BatchOptions) -> R<()> {
    let pr = match preset.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => Some(presets::find(id).ok_or("no such preset")?),
        None => None,
    };
    st.begin_job("batch")?;
    std::thread::spawn(move || {
        let st = app_state(&app);
        let n = folders.len().max(1);
        let mut done = Vec::new();
        let mut failed = Vec::new();
        for (i, f) in folders.iter().enumerate() {
            if st.cancel.load(Ordering::Relaxed) {
                break;
            }
            let name = Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string();
            let a2 = app.clone();
            let base = i as f32 / n as f32;
            let r = batch::process_folder(Path::new(f), &opts, pr.as_ref(), &move |fr, m| progress(&a2, "batch", base + fr / n as f32, &format!("{name}: {m}")), &st.cancel);
            match r {
                Ok(res) => done.push(res.album_dir.to_string_lossy().to_string()),
                Err(err) => failed.push(format!("{f}: {err}")),
            }
        }
        st.end_job(&app, "batch", Ok(serde_json::json!({"done": done, "failed": failed})));
    });
    Ok(())
}

// ---------------------------------------------------------------- pictures

#[tauri::command]
pub fn tile_wave(st: State<'_, App>, t0: f64, t1: f64, w: u32, h: u32, vzoom: f32) -> tauri::ipc::Response {
    let inner = st.inner.lock().unwrap();
    let layout = inner.project.layout();
    let segs: Vec<tiles::Seg> = layout
        .iter()
        .filter_map(|l| inner.audio.get(&l.source_id).map(|a| tiles::Seg { place: l, src: a, processed: inner.peaks.get(&l.source_id).map(|v| v.as_slice()) }))
        .collect();
    tauri::ipc::Response::new(tiles::waveform(&segs, t0, t1.max(t0 + 1e-3), w, h, vzoom.clamp(0.5, 64.0)))
}

#[tauri::command]
pub fn tile_spec(st: State<'_, App>, t0: f64, t1: f64, w: u32, h: u32) -> tauri::ipc::Response {
    let inner = st.inner.lock().unwrap();
    let layout = inner.project.layout();
    let segs: Vec<tiles::Seg> = layout.iter().filter_map(|l| inner.audio.get(&l.source_id).map(|a| tiles::Seg { place: l, src: a, processed: None })).collect();
    tauri::ipc::Response::new(tiles::spectrogram(&segs, t0, t1.max(t0 + 1e-3), w, h, -100.0))
}
