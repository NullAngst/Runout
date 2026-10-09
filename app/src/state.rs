//! Application state: the open project, loaded audio, undo history, analysis
//! results, and the background job slot.

use crate::capture::Capture;
use crate::player::{Cmd, Player, RenderCtx};
use crate::prefs::{Prefs, Session};
use runout_engine::audio::SourceAudio;
use runout_engine::chain::ClickHit;
use runout_engine::project::{Placement, Project, AUTOSAVE_FILE};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter};

pub struct Inner {
    pub project: Project,
    pub dir: Option<PathBuf>,
    pub undo: Vec<(Project, String, Instant)>,
    pub redo: Vec<Project>,
    pub dirty: bool,
    pub audio: HashMap<u64, Arc<SourceAudio>>,
    /// Per source, processed-time clicks from the last analysis.
    pub clicks: HashMap<u64, Vec<ClickHit>>,
    /// Per source, processed peaks from the last analysis.
    pub peaks: HashMap<u64, Vec<(f32, f32)>>,
    pub gen: u64,
}

pub struct App {
    pub inner: Mutex<Inner>,
    pub player: Player,
    pub capture: Mutex<Option<Capture>>,
    pub busy: Mutex<Option<String>>,
    pub cancel: AtomicBool,
    pub prefs: Mutex<Prefs>,
    pub session: Mutex<Session>,
    /// Session found at launch, for the recovery prompt.
    pub recovery: Mutex<Option<Session>>,
}

#[derive(Serialize)]
pub struct TrackView {
    pub id: u64,
    pub start_g: f64,
    pub end_g: f64,
    pub gain_db: f32,
    pub short_db: f32,
    pub fresh: bool,
    pub passthrough: bool,
}

#[derive(Serialize)]
pub struct ProjectView {
    pub project: Project,
    pub dir: Option<String>,
    pub layout: Vec<Placement>,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub album_lufs: Option<f32>,
    pub tracks: Vec<TrackView>,
    pub stale: usize,
    pub export_dir: String,
    pub analysed: bool,
}

impl App {
    pub fn new() -> Self {
        let session = Session::load();
        let recovery = if session.open_project.is_some() || session.capture.is_some() { Some(session.clone()) } else { None };
        App {
            inner: Mutex::new(Inner {
                project: Project::default(),
                dir: None,
                undo: Vec::new(),
                redo: Vec::new(),
                dirty: false,
                audio: HashMap::new(),
                clicks: HashMap::new(),
                peaks: HashMap::new(),
                gen: 1,
            }),
            player: Player::new(),
            capture: Mutex::new(None),
            busy: Mutex::new(None),
            cancel: AtomicBool::new(false),
            prefs: Mutex::new(Prefs::load()),
            session: Mutex::new(Session::default()),
            recovery: Mutex::new(recovery),
        }
    }

    pub fn push_ctx(&self, inner: &Inner) {
        let ctx = RenderCtx { project: inner.project.clone(), audio: inner.audio.clone(), layout: inner.project.layout(), gen: inner.gen };
        self.player.send(Cmd::Update(Arc::new(ctx)));
    }

    /// Replace the project as one undoable edit. Edits with the same label
    /// within a second merge, so dragging a slider is one undo step.
    pub fn commit(&self, inner: &mut Inner, new: Project, label: &str) {
        if new == inner.project {
            return;
        }
        let now = Instant::now();
        let merge = inner.undo.last().map(|(_, l, t)| l == label && !label.is_empty() && now.duration_since(*t).as_millis() < 1000).unwrap_or(false);
        if merge {
            if let Some(last) = inner.undo.last_mut() {
                last.2 = now;
            }
        } else {
            let old = inner.project.clone();
            inner.undo.push((old, label.to_string(), now));
        }
        inner.redo.clear();
        inner.project = new;
        inner.dirty = true;
        inner.gen += 1;
        self.push_ctx(inner);
    }

    pub fn view(&self, inner: &Inner) -> ProjectView {
        let p = &inner.project;
        let layout = p.layout();
        let tracks = p
            .tracks
            .iter()
            .map(|t| {
                let g = p.gain_for(t);
                let (a, b) = match layout.iter().find(|l| l.source_id == t.source_id) {
                    Some(l) => (l.start + t.start_s / l.ratio, l.start + t.end_s / l.ratio),
                    None => (0.0, 0.0),
                };
                TrackView { id: t.id, start_g: a, end_g: b, gain_db: g.gain_db, short_db: g.short_db, fresh: p.analysis_fresh(t), passthrough: p.is_passthrough(t) }
            })
            .collect::<Vec<_>>();
        let stale = tracks.iter().filter(|t| !t.fresh).count();
        ProjectView {
            project: p.clone(),
            dir: inner.dir.as_ref().map(|d| d.to_string_lossy().to_string()),
            layout,
            dirty: inner.dirty,
            can_undo: !inner.undo.is_empty(),
            can_redo: !inner.redo.is_empty(),
            album_lufs: p.album_lufs(),
            tracks,
            stale,
            export_dir: runout_engine::export::album_dir(p).to_string_lossy().to_string(),
            analysed: !inner.clicks.is_empty(),
        }
    }

    /// Global timeline seconds -> (source id, source seconds).
    pub fn to_source(inner: &Inner, g: f64) -> Option<(u64, f64)> {
        let l = inner.project.layout();
        let p = l.iter().find(|p| g >= p.start && g <= p.start + p.duration)?;
        Some((p.source_id, (g - p.start) * p.ratio))
    }

    pub fn autosave(&self) {
        let inner = self.inner.lock().unwrap();
        if let (true, Some(dir)) = (inner.dirty, inner.dir.as_ref()) {
            let _ = inner.project.save_as(dir, AUTOSAVE_FILE);
        }
    }

    /// Start a background job. Only one runs at a time.
    pub fn begin_job(&self, name: &str) -> Result<(), String> {
        let mut b = self.busy.lock().unwrap();
        if let Some(j) = b.as_ref() {
            return Err(format!("busy: {j}"));
        }
        *b = Some(name.to_string());
        self.cancel.store(false, Ordering::Relaxed);
        Ok(())
    }

    pub fn end_job(&self, app: &AppHandle, name: &str, result: Result<serde_json::Value, String>) {
        *self.busy.lock().unwrap() = None;
        let payload = match result {
            Ok(v) => serde_json::json!({"job": name, "ok": true, "data": v}),
            Err(e) => serde_json::json!({"job": name, "ok": false, "error": e}),
        };
        let _ = app.emit("job", payload);
    }
}

pub fn progress(app: &AppHandle, job: &str, frac: f32, msg: &str) {
    let _ = app.emit("progress", serde_json::json!({"job": job, "frac": frac, "msg": msg}));
}
