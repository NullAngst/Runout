//! Tauri commands. The UI talks to the project model and the transport only
//! through these.

use crate::capture;
use crate::lookup;
use crate::player::Cmd;
use crate::prefs::projects_root;
use crate::state::{progress, App, Inner, ProjectView};
use runout_engine::audio::{self, SourceAudio};
use runout_engine::chain::{Chain, ChannelModeKind, Repair};
use runout_engine::media::{self, CapturePath, DeckProfile, Family, MediumPreset};
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
    settings: Settings,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub theme: String,
    pub last_medium: String,
    pub deck_by_family: std::collections::BTreeMap<String, String>,
    pub library_root: String,
}

fn settings_of(st: &App) -> Settings {
    let p = st.prefs.lock().unwrap();
    Settings {
        theme: p.theme.clone(),
        last_medium: p.last_medium.clone().unwrap_or_else(|| "vinyl-lp".into()),
        deck_by_family: p.deck_by_family.clone(),
        library_root: p.library_root.clone().unwrap_or_else(|| project::DEFAULT_ROOT.into()),
    }
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
        settings: settings_of(&st),
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
pub fn project_new(st: State<'_, App>, name: String, parent: Option<String>, medium: Option<String>, condition: Option<String>, deck: Option<String>) -> R<ProjectView> {
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
    let mut p = Project { name: name.clone(), ..Default::default() };
    let mid = medium.unwrap_or_else(|| "vinyl-lp".into());
    let m = media::find(&mid).ok_or_else(|| format!("no medium called {mid}"))?;
    let cond = match condition.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => Some(presets::find(c).ok_or_else(|| format!("no condition preset called {c}"))?),
        None => m.default_condition.as_deref().and_then(presets::find),
    };
    let dk = deck.as_deref().filter(|d| !d.is_empty()).and_then(media::find_deck);
    // Vinyl LP with nothing on top is today's Runout: leave the default alone.
    if m.id != "vinyl-lp" || cond.is_some() || dk.is_some() {
        media::apply_to_project(&mut p, &m, cond.as_ref(), dk.as_ref());
    }
    {
        let mut pr = st.prefs.lock().unwrap();
        if let Some(root) = pr.library_root.clone() {
            p.export.root = root;
        }
        pr.last_medium = Some(m.id.clone());
        if let Some(d) = &dk {
            pr.deck_by_family.insert(format!("{:?}", m.family).to_lowercase(), d.id.clone());
        }
        pr.save();
    }
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
                let name = p.medium.preset.side_naming.name(n_sides);
                let sid = ops::add_source(&mut p, &path, SourceKind::File, true, &name, &a);
                ops::auto_new_source(&mut p, sid, &a);
                if p.sources.len() == 1 && p.split.mode == project::SplitMode::Absolute {
                    p.split.threshold_db = auto::auto_split_threshold(&a);
                }
                // one track per side: nothing splits until a split button is pressed
                ops::whole_source_track(&mut p, sid, &a);
                tone_on_arrival(&mut p, sid, &a);
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
            "speed" => {
                // a reference tone at the head, else the hum's mean
                let t = auto::detect_tone(&a);
                let err = match t.as_ref().and_then(|t| t.speed_err_pct) {
                    Some(e) => Some(e),
                    None => {
                        let w = chain_of(p, track_id).wow.clone();
                        auto::measure_wow(&a, &w).static_err_pct
                    }
                };
                let s = p.source_mut(sid).ok_or("no side")?;
                s.fx.analysis.tone = t;
                match err {
                    Some(e) => s.fx.speed.trim_pct = (e * 1000.0).round() / 1000.0,
                    None => return Err("no reference tone or tape hum to measure the speed from".into()),
                }
            }
            "wow" => {
                let w = chain_of(p, track_id).wow.clone();
                let r = auto::measure_wow(&a, &w);
                let s = p.source_mut(sid).ok_or("no side")?;
                s.fx.analysis.wow_note = Some(r.note.clone());
                s.fx.analysis.wow_peak_pct = r.peak_pct;
                s.fx.analysis.wow_weighted_pct = r.weighted_pct;
                let ok = r.track.is_some();
                s.fx.analysis.set_wow(sid, r.track);
                chain_of(p, track_id).wow.enabled = ok;
                if !ok {
                    return Err(r.note);
                }
            }
            "azimuth" => {
                let c = auto::measure_azimuth(&a);
                let (mean, stat) = c.as_ref().map(|c| (c.mean(), c.is_static())).unwrap_or((0.0, true));
                p.source_mut(sid).ok_or("no side")?.fx.analysis.set_azimuth(sid, c.clone());
                let ch = chain_of(p, track_id);
                ch.azimuth.enabled = c.is_some();
                ch.azimuth.mode = runout_engine::chain::AzimuthMode::Auto;
                ch.azimuth.static_us = (mean * 10.0).round() / 10.0;
                let _ = stat;
            }
            "channel_mode" => {
                let b = auto::best_channel(&a);
                p.source_mut(sid).ok_or("no side")?.fx.analysis.best_channel = Some(b);
                let ch = chain_of(p, track_id);
                ch.channel_mode.enabled = true;
                ch.channel_mode.mode = ChannelModeKind::BestAuto;
            }
            "nr" => {
                let t = auto::detect_tone(&a).ok_or("no level tone at the head of this tape: type the reference level in")?;
                p.source_mut(sid).ok_or("no side")?.fx.analysis.tone = Some(t.clone());
                let ch = chain_of(p, track_id);
                ch.nr.ref_dbfs = (t.level_dbfs * 10.0).round() / 10.0;
                ch.nr.ref_measured = true;
            }
            "dropout" => {
                let d = p.medium.preset.chain.dropout.clone();
                let ch = chain_of(p, track_id);
                ch.dropout = d;
                ch.dropout.enabled = true;
            }
            "band_limit" => {
                let b = p.medium.preset.chain.band_limit.clone();
                let ch = chain_of(p, track_id);
                ch.band_limit = b;
                ch.band_limit.enabled = true;
            }
            "error_repair" => {
                let n = auto::analyze_media(p, sid, &a);
                let ch = chain_of(p, track_id);
                ch.error_repair.enabled = n.iter().any(|x| x.contains("digital error"));
            }
            "deemphasis" => {
                let ch = chain_of(p, track_id);
                ch.deemphasis.enabled = !ch.deemphasis.enabled;
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
            let (mut p, audio, dir) = {
                let inner = st.inner.lock().unwrap();
                (inner.project.clone(), inner.audio.clone(), inner.dir.clone())
            };
            // 1. what the tape and digital stages need: wow and azimuth curves,
            // best channel, digital errors. Redone when missing, or for Analyze all.
            let mut notes: Vec<String> = Vec::new();
            let ids: Vec<u64> = p.sources.iter().filter(|s| s.raw || s.kind != SourceKind::Folder).map(|s| s.id).collect();
            for (i, sid) in ids.iter().enumerate() {
                let Some(s) = p.source(*sid) else { continue };
                let an = &s.fx.analysis;
                let c = &p.chain;
                let fam = p.medium.preset.family;
                let need = all
                    || an.missing()
                    || (c.wow.enabled && an.speed_curve.is_none())
                    || (c.azimuth.enabled && an.azimuth_curve.is_none())
                    || (c.channel_mode.enabled && c.channel_mode.mode == ChannelModeKind::BestAuto && an.best_channel.is_none())
                    || (c.digital && an.error_map.is_none() && !c.error_repair.enabled)
                    || (fam.is_tape() && an.tone.is_none() && an.is_empty());
                if !need {
                    continue;
                }
                if st.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                progress(&app, "analyze", 0.0, &format!("Measuring {} ({}/{})", s.name, i + 1, ids.len()));
                let a = audio.get(sid).ok_or("audio not loaded")?.clone();
                notes.extend(auto::analyze_media(&mut p, *sid, &a));
            }
            {
                // the measurements go in straight away, so the track keys match below
                let mut inner = st.inner.lock().unwrap();
                for s in p.sources.iter() {
                    if let Some(cur) = inner.project.source_mut(s.id) {
                        cur.fx.analysis = s.fx.analysis.clone();
                    }
                }
                if p.chain.error_repair.enabled && !inner.project.chain.error_repair.enabled {
                    inner.project.chain.error_repair.enabled = true;
                }
                if let Some(d) = dir.as_ref() {
                    let _ = inner.project.write_analysis(d);
                }
                inner.gen += 1;
                st.push_ctx(&inner);
                p = inner.project.clone();
            }
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
                // dropouts go on the source's map, for the overlay and [ ]
                if !an.events.is_empty() || inner.project.source(sid).map(|s| !s.fx.analysis.dropouts().is_empty()).unwrap_or(false) {
                    let range = (an.start, an.start + (an.peaks.len() * runout_engine::audio::PEAK_BUCKET) as i64);
                    auto::store_dropouts(&mut inner.project, sid, &an.events, range);
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
            if let Some(d) = inner.dir.clone() {
                let _ = inner.project.write_analysis(&d);
            }
            inner.dirty = true;
            inner.gen += 1;
            st.push_ctx(&inner);
            let mut v = serde_json::to_value(st.view(&inner)).map_err(e)?;
            v["notes"] = serde_json::json!(notes);
            Ok(v)
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
    presets::save_for(&name, &chain, inner.project.medium.preset.family).map_err(e)?;
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
pub async fn lookup_search(app: AppHandle, artist: String, album: String) -> R<Vec<lookup::ReleaseHit>> {
    let format = app_state(&app).inner.lock().unwrap().project.medium.preset.mb_format.clone();
    tauri::async_runtime::spawn_blocking(move || lookup::search(&artist, &album, &format)).await.map_err(e)?
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
    path: CapturePath,
}

/// Inputs for the current medium's capture path.
#[tauri::command]
pub async fn devices(app: AppHandle) -> Devices {
    let st = app_state(&app);
    let path = st.inner.lock().unwrap().project.medium.preset.capture.path;
    let remembered = {
        let p = st.prefs.lock().unwrap();
        if path == CapturePath::Spdif {
            p.digital_device.clone()
        } else {
            p.input_device.clone()
        }
    };
    let inputs = tauri::async_runtime::spawn_blocking(move || capture::list_inputs(path)).await.unwrap_or_default();
    let selected = capture::pick_default(&inputs, remembered.as_deref());
    Devices { inputs, selected, path }
}

#[tauri::command]
pub fn select_device(st: State<'_, App>, id: String) {
    let digital = st.inner.lock().unwrap().project.medium.preset.capture.path == CapturePath::Spdif;
    let mut p = st.prefs.lock().unwrap();
    if digital {
        p.digital_device = Some(id);
    } else {
        p.input_device = Some(id);
    }
    p.save();
}

fn capture_opts(st: &App, device: Option<String>, path: Option<PathBuf>, auto_stop_s: Option<f32>, max_s: Option<f32>) -> capture::Opts {
    let m = st.inner.lock().unwrap().project.medium.preset.clone();
    let rpms: Vec<f32> = if m.speed.unit == runout_engine::chain::SpeedUnit::Rpm { m.speed.choices.clone() } else { Vec::new() };
    let mut spec = m.capture.clone();
    if path.is_none() {
        // a level check is always analog
        spec.path = CapturePath::Analog;
    }
    capture::Opts { device, path, auto_stop_s, max_s, spec, rpms }
}

#[tauri::command]
pub fn level_check(st: State<'_, App>, device: Option<String>) -> R<()> {
    let mut cap = st.capture.lock().unwrap();
    if cap.is_some() {
        return Err("already capturing".into());
    }
    if st.inner.lock().unwrap().project.medium.preset.capture.path == CapturePath::Spdif {
        return Err("digital sources have no level check: digital levels are what they are".into());
    }
    let opts = capture_opts(&st, device, None, None, Some(20.0));
    *cap = Some(capture::start(opts, "Level check".into())?);
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
    // long enough for a 3-hour reel
    let opts = capture_opts(&st, device, Some(path.clone()), auto_stop_s, Some(4.0 * 60.0 * 60.0));
    *cap = Some(capture::start(opts, name.clone())?);
    drop(cap);
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
    pub bit_perfect: Option<bool>,
    pub notes: Vec<String>,
    pub rate: u32,
    pub bits: u16,
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
            return Ok(CaptureResult { level_check: true, peak_db: done.peak_db, seconds, clipped: done.clipped, auto_stopped: false, view: None, bit_perfect: None, notes: done.notes, rate: done.rate, bits: done.bits });
        };
        let view = add_capture(&st, &path, &cap.name, done.emphasis == Some(true))?;
        Ok(CaptureResult { level_check: false, peak_db: done.peak_db, seconds, clipped: done.clipped, auto_stopped: done.auto_stopped, view: Some(view), bit_perfect: done.bit_perfect, notes: done.notes, rate: done.rate, bits: done.bits })
    })
    .await
    .map_err(e)?
}

/// A tape's reference tone, looked for as soon as the side arrives so the
/// Capture panel can show it: the first minute only, so it is quick.
fn tone_on_arrival(p: &mut Project, sid: u64, a: &SourceAudio) {
    let fam = p.medium.preset.family;
    if !(fam.is_tape() || fam == Family::Generic) {
        return;
    }
    if let Some(s) = p.source_mut(sid) {
        s.fx.analysis.tone = auto::detect_tone(a);
    }
}

fn add_capture(st: &App, path: &Path, name: &str, emphasis: bool) -> R<ProjectView> {
    let a = Arc::new(audio::decode_file(path).map_err(e)?);
    let mut inner = st.inner.lock().unwrap();
    let dir = inner.dir.clone().ok_or("no project")?;
    let rel = path.strip_prefix(&dir).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| path.to_string_lossy().to_string());
    let mut p = inner.project.clone();
    let sid = ops::add_source(&mut p, &rel, SourceKind::Capture, true, name, &a);
    ops::auto_new_source(&mut p, sid, &a);
    if p.sources.len() == 1 && p.split.mode == project::SplitMode::Absolute {
        p.split.threshold_db = auto::auto_split_threshold(&a);
    }
    // one track per side: nothing splits until a split button is pressed
    ops::whole_source_track(&mut p, sid, &a);
    tone_on_arrival(&mut p, sid, &a);
    if emphasis {
        p.chain.deemphasis.enabled = true;
    }
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
    add_capture(&st, &p, name.as_deref().unwrap_or("Recovered side"), false)
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
pub fn batch_run(app: AppHandle, st: State<'_, App>, folders: Vec<String>, preset: Option<String>, opts: batch::BatchOptions, medium: Option<String>) -> R<()> {
    let pr = match preset.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => Some(presets::find(id).ok_or("no such preset")?),
        None => None,
    };
    let med = match medium.as_deref().filter(|s| !s.is_empty() && *s != "vinyl-lp") {
        Some(id) => Some(media::find(id).ok_or("no such medium")?),
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
            let prog = move |fr: f32, m: String| progress(&a2, "batch", base + fr / n as f32, &format!("{name}: {m}"));
            let r = (|| {
                let mut start = Project::default();
                if let Some(m) = &med {
                    media::apply_to_project(&mut start, m, None, None);
                }
                let (mut p, sources) = batch::folder_project_in(start, Path::new(f), &opts, pr.as_ref(), &|m| prog(0.0, m))?;
                export::export(&mut p, Path::new("/"), &sources, &prog, &st.cancel)
            })();
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
    let pal = st.palette.lock().unwrap().clone();
    tauri::ipc::Response::new(tiles::waveform_with(&segs, t0, t1.max(t0 + 1e-3), w, h, vzoom.clamp(0.5, 64.0), &pal))
}

#[tauri::command]
pub fn tile_spec(st: State<'_, App>, t0: f64, t1: f64, w: u32, h: u32) -> tauri::ipc::Response {
    let inner = st.inner.lock().unwrap();
    let layout = inner.project.layout();
    let segs: Vec<tiles::Seg> = layout.iter().filter_map(|l| inner.audio.get(&l.source_id).map(|a| tiles::Seg { place: l, src: a, processed: None })).collect();
    let pal = st.palette.lock().unwrap().clone();
    tauri::ipc::Response::new(tiles::spectrogram_with(&segs, t0, t1.max(t0 + 1e-3), w, h, -100.0, &pal))
}

/// A strip under the waveform: speed, azimuth, dropouts or errors.
#[tauri::command]
pub fn tile_overlay(st: State<'_, App>, t0: f64, t1: f64, w: u32, h: u32, kind: tiles::Overlay) -> tauri::ipc::Response {
    let inner = st.inner.lock().unwrap();
    let layout = inner.project.layout();
    let segs: Vec<tiles::OverlaySeg> = layout.iter().filter_map(|l| inner.project.source(l.source_id).map(|s| tiles::OverlaySeg { place: l, analysis: &s.fx.analysis })).collect();
    let pal = st.palette.lock().unwrap().clone();
    tauri::ipc::Response::new(tiles::overlay(&segs, kind, t0, t1.max(t0 + 1e-3), w, h, &pal))
}

// ---------------------------------------------------------------- media

#[derive(Serialize)]
pub struct MediaInfo {
    media: Vec<MediumPreset>,
    families: Vec<(Family, String)>,
    decks: Vec<DeckProfile>,
    curves: Vec<runout_engine::dsp::tape_eq::NamedCurve>,
    stages: Vec<String>,
}

fn media_info() -> MediaInfo {
    MediaInfo {
        media: media::list(),
        families: Family::all().iter().map(|f| (*f, f.label().to_string())).collect(),
        decks: media::decks(),
        curves: runout_engine::dsp::tape_eq::all_curves(),
        stages: media::STAGES.iter().map(|s| s.to_string()).collect(),
    }
}

/// Every medium preset (built-in and saved), the families, decks and EQ curves.
#[tauri::command]
pub fn media_info_get() -> MediaInfo {
    media_info()
}

/// Switch the project to a medium, with an optional condition preset and deck.
/// Captures, tracks and manual repairs stay.
#[tauri::command]
pub fn medium_set(st: State<'_, App>, id: String, condition: Option<String>, deck: Option<String>) -> R<ProjectView> {
    let m = media::find(&id).ok_or_else(|| format!("no medium called {id}"))?;
    let cond = match condition.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => Some(presets::find(c).ok_or_else(|| format!("no condition preset called {c}"))?),
        None => None,
    };
    let dk = match deck.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => Some(media::find_deck(d).ok_or_else(|| format!("no deck called {d}"))?),
        None => None,
    };
    let v = edit(&st, "Medium", |p, _| {
        media::apply_to_project(p, &m, cond.as_ref(), dk.as_ref());
        Ok(())
    })?;
    let mut pr = st.prefs.lock().unwrap();
    pr.last_medium = Some(m.id.clone());
    let fam = format!("{:?}", m.family).to_lowercase();
    match &dk {
        Some(d) => {
            pr.deck_by_family.insert(fam, d.id.clone());
        }
        None => {
            pr.deck_by_family.remove(&fam);
        }
    }
    pr.save();
    Ok(v)
}

/// Save the project's current medium settings as a user medium preset.
/// Only the differences from the built-in it rests on are stored.
#[tauri::command]
pub fn medium_save(st: State<'_, App>, name: String) -> R<MediaInfo> {
    let m = {
        let inner = st.inner.lock().unwrap();
        let p = &inner.project;
        let mut m = p.medium.preset.clone();
        m.chain = p.chain.clone();
        m.split = p.split.clone();
        m.export.kind = p.export.kind;
        m.export.rate = p.export.rate;
        m.export.bits = p.export.bit_depth;
        m.export.dither = p.export.dither;
        m
    };
    media::save(&name, &m).map_err(e)?;
    Ok(media_info())
}

#[tauri::command]
pub fn medium_delete(id: String) -> R<MediaInfo> {
    let m = media::find(&id).ok_or("no such medium")?;
    if m.builtin {
        return Err("built-in media cannot be deleted".into());
    }
    media::delete(&id).map_err(e)?;
    Ok(media_info())
}

/// Change which stage cards the Clean panel shows for this project.
#[tauri::command]
pub fn medium_visible(st: State<'_, App>, visible: Vec<String>) -> R<ProjectView> {
    edit(&st, "Stages shown", |p, _| {
        p.medium.preset.visible = media::STAGES.iter().filter(|s| visible.iter().any(|v| v == *s)).map(|s| s.to_string()).collect();
        Ok(())
    })
}

#[tauri::command]
pub fn deck_save(deck: DeckProfile) -> R<MediaInfo> {
    media::save_deck(&deck).map_err(e)?;
    Ok(media_info())
}

#[tauri::command]
pub fn deck_delete(id: String) -> R<MediaInfo> {
    media::delete_deck(&id).map_err(e)?;
    Ok(media_info())
}

/// A deck profile filled in from the open project: the speed error from the
/// reference tone (or the hum), the azimuth from the azimuth curve and the
/// balance from the tone's channel levels. Play a test tape and capture it,
/// run Analyze, then press Measure.
#[tauri::command]
pub fn deck_measure(st: State<'_, App>) -> R<DeckProfile> {
    let inner = st.inner.lock().unwrap();
    let p = &inner.project;
    let s = p.sources.iter().find(|s| s.raw).or_else(|| p.sources.first()).ok_or("capture a test tape first")?;
    let a = inner.audio.get(&s.id).ok_or("audio not loaded")?.clone();
    let fam = p.medium.preset.family;
    let wow = p.chain.wow.clone();
    let an = s.fx.analysis.clone();
    drop(inner);
    let tone = an.tone.clone().or_else(|| auto::detect_tone(&a));
    let speed = tone.as_ref().and_then(|t| t.speed_err_pct).or_else(|| auto::measure_wow(&a, &wow).static_err_pct);
    let az = an.azimuth().map(|c| c.mean()).or_else(|| auto::measure_azimuth(&a).map(|c| c.mean()));
    let mut bal = 0.0f32;
    if a.channels == 2 {
        let (from, to) = match &tone {
            Some(t) => ((t.start_s * a.rate as f64) as i64, ((t.start_s + t.len_s) * a.rate as f64) as i64),
            None => (0, (a.frames as i64).min(a.rate as i64 * 30)),
        };
        let rms = |c: usize| {
            let x = a.read_channel(c, from, to);
            let s: f64 = x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len().max(1) as f64;
            10.0 * s.max(1e-20).log10()
        };
        // positive moves the image right: right is quieter, so boost it
        bal = ((rms(0) - rms(1)) as f32 * 10.0).round() / 10.0;
        if bal.abs() > 6.0 {
            bal = 0.0;
        }
    }
    if speed.is_none() && az.is_none() {
        return Err("nothing to measure: capture a test tape (a steady tone at the head), then Analyze".into());
    }
    Ok(DeckProfile {
        id: String::new(),
        name: String::new(),
        family: fam,
        speed_pct: speed.map(|v| (v * 100.0).round() / 100.0).unwrap_or(0.0),
        azimuth_us: az.map(|v| (v * 10.0).round() / 10.0).unwrap_or(0.0),
        balance_db: bal,
        eq_low_db: 0.0,
        eq_high_db: 0.0,
    })
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub fn settings_get(st: State<'_, App>) -> Settings {
    settings_of(&st)
}

#[tauri::command]
pub fn settings_set(st: State<'_, App>, settings: Settings) -> Settings {
    {
        let mut p = st.prefs.lock().unwrap();
        p.theme = settings.theme.clone();
        p.last_medium = Some(settings.last_medium.clone());
        p.deck_by_family = settings.deck_by_family.clone();
        let root = settings.library_root.trim();
        p.library_root = if root.is_empty() || root == project::DEFAULT_ROOT { None } else { Some(root.to_string()) };
        p.save();
    }
    *st.palette.lock().unwrap() = tiles::Palette::named(&settings.theme);
    settings_of(&st)
}

/// Switch the theme: the window restyles from CSS, the pictures drawn in
/// Rust take the new palette from here.
#[tauri::command]
pub fn set_theme(st: State<'_, App>, theme: String) -> Settings {
    {
        let mut p = st.prefs.lock().unwrap();
        p.theme = theme.clone();
        p.save();
    }
    *st.palette.lock().unwrap() = tiles::Palette::named(&theme);
    settings_of(&st)
}

// ---------------------------------------------------------------- tape tools

/// Find 8-track program changes on every side (or one). The UI shows the
/// ones that cut a song and asks before joining.
#[tauri::command]
pub fn find_program_changes(st: State<'_, App>, source_id: Option<u64>) -> R<ProjectView> {
    let mut inner = st.inner.lock().unwrap();
    let ids: Vec<u64> = inner.project.sources.iter().filter(|s| s.kind != SourceKind::Folder).map(|s| s.id).filter(|id| source_id.map(|x| x == *id).unwrap_or(true)).collect();
    for id in ids {
        if let Some(a) = inner.audio.get(&id).cloned() {
            let v = auto::find_program_changes(&a);
            inner.program_changes.insert(id, v);
        }
    }
    Ok(st.view(&inner))
}

/// Split every side at its program changes.
#[tauri::command]
pub fn split_program_changes(st: State<'_, App>) -> R<ProjectView> {
    edit(&st, "Split at program changes", |p, inner| {
        let ids: Vec<u64> = p.sources.iter().filter(|s| s.kind != SourceKind::Folder).map(|s| s.id).collect();
        for id in ids {
            let Some(a) = inner.audio.get(&id) else { continue };
            let cuts: Vec<(f64, f64)> = match inner.program_changes.get(&id) {
                Some(v) if !v.is_empty() => {
                    let mut out = Vec::new();
                    let mut t = 0.0;
                    for c in v {
                        let m = (c.start_s + c.end_s) / 2.0;
                        out.push((t, m));
                        t = m;
                    }
                    out.push((t, a.duration()));
                    out
                }
                _ => auto::split_program_changes(a),
            };
            p.tracks.retain(|t| t.source_id != id);
            for (s, e) in cuts {
                ops::new_track(p, id, s, e);
            }
        }
        ops::renumber(p);
        Ok(())
    })
}

/// Join the two tracks either side of a program change: the song plays
/// through and the mute is cut out with a crossfade.
#[tauri::command]
pub fn join_program_change(st: State<'_, App>, source_id: u64, start_s: f64, end_s: f64, xfade_ms: Option<f32>) -> R<ProjectView> {
    edit(&st, "Join across program change", |p, _| {
        let mid = (start_s + end_s) / 2.0;
        let first = p.tracks.iter().filter(|t| t.source_id == source_id && t.end_s <= mid + 0.05 && t.end_s >= start_s - 0.5).map(|t| t.id).next().ok_or("no track ends at that program change: split at program changes first")?;
        let second = p.tracks.iter().filter(|t| t.source_id == source_id && t.start_s >= mid - 0.05 && t.start_s <= end_s + 0.5).map(|t| t.id).next().ok_or("no track starts after that program change")?;
        if ops::join_across(p, first, second, (start_s, end_s), xfade_ms.unwrap_or(20.0).clamp(1.0, 200.0)) {
            Ok(())
        } else {
            Err("those tracks cannot be joined".into())
        }
    })
}

/// Set a side's speed trim from a known running time: "this song runs
/// 3:41" with a track selected, or "this side runs 23:41" without one.
#[tauri::command]
pub fn speed_from_length(st: State<'_, App>, source_id: u64, track_id: Option<u64>, want_s: f64) -> R<ProjectView> {
    edit(&st, "Speed from length", |p, inner| {
        let a = inner.audio.get(&source_id).ok_or("audio not loaded")?;
        // source seconds the length covers
        let span = match track_id.and_then(|id| p.track(id)).filter(|t| t.source_id == source_id) {
            Some(t) => {
                let cut: f64 = t.joins.iter().map(|j| j.end_s - j.start_s).sum();
                (t.end_s - t.start_s - cut).max(0.0)
            }
            None => a.duration(),
        };
        let s = p.source_mut(source_id).ok_or("no side")?;
        let have = span / s.fx.speed.ratio();
        let trim = s.fx.speed.solve_trim_for_length(have, want_s);
        if !(-20.0..=20.0).contains(&trim) {
            return Err(format!("that would need a {trim:.1}% speed change: check the length"));
        }
        s.fx.speed.trim_pct = (trim * 1000.0).round() / 1000.0;
        Ok(())
    })
}
