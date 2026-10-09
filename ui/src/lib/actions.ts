// Actions shared by buttons and keyboard shortcuts.
import { open, ask } from '@tauri-apps/plugin-dialog';
import { api } from './api';
import { ui, run, fail, toast, flushNow, timelineEnd, selectedTrack, trackAt, setView, fit } from './store.svelte';

export async function importFiles() {
  if (!ui.view?.dir) return toast('Create or open a project first.', true);
  const sel = await open({ multiple: true, filters: [{ name: 'Audio', extensions: ['wav', 'flac', 'aif', 'aiff', 'ogg', 'mp3'] }] });
  if (!sel) return;
  const paths = Array.isArray(sel) ? sel : [sel];
  try {
    await api.importFiles(paths.map(String));
  } catch (e) {
    fail(e);
  }
}

export async function importFolder() {
  if (!ui.view?.dir) return toast('Create or open a project first.', true);
  const dir = await open({ directory: true });
  if (!dir) return;
  try {
    const items = await api.folderPlan(String(dir));
    if (!items.length) return toast('No audio files in that folder.', true);
    ui.dialogData = { items, dir: String(dir) };
    ui.dialog = 'folder';
  } catch (e) {
    fail(e);
  }
}

export async function openProject() {
  const dir = await open({ directory: true, defaultPath: ui.info?.projects_root });
  if (!dir) return;
  try {
    await api.projectOpen(String(dir));
  } catch (e) {
    fail(e);
  }
}

export async function newProject() {
  await flushNow();
  if (ui.view?.dirty && ui.view.dir) {
    const yes = await ask('Save changes to this project first?', { title: 'Runout', kind: 'warning', okLabel: 'Save', cancelLabel: "Don't save" });
    if (yes) await api.projectSave().catch(fail);
  }
  api.pause();
  ui.view = null;
  ui.clicks = [];
  ui.selTrack = null;
  ui.selection = null;
  api.appInfo().then((i) => (ui.info = i)).catch(fail);
}

export async function save() {
  if (!ui.view?.dir) return;
  await run(() => api.projectSave(), 'Saved.');
}

export async function togglePlay() {
  await flushNow();
  if (ui.tick.playing) {
    api.pause();
  } else {
    let from = ui.tick.pos;
    if (from >= timelineEnd() - 0.05) from = 0;
    if (ui.loop && ui.selection) {
      api.setLoop([ui.selection.a, ui.selection.b]);
      if (from < ui.selection.a || from > ui.selection.b) from = ui.selection.a;
    }
    api.play(from);
  }
}

export function seekTo(t: number) {
  t = Math.max(0, Math.min(timelineEnd(), t));
  ui.tick.pos = t;
  api.seek(t);
  const span = ui.t1 - ui.t0;
  if (t < ui.t0 || t > ui.t1) {
    ui.t0 = Math.max(0, t - span * 0.1);
    ui.t1 = ui.t0 + span;
  }
}

export function markBoundary() {
  run(() => api.markBoundary(ui.tick.pos));
}

export function jumpClick(dir: 1 | -1) {
  const p = ui.tick.pos;
  const eps = 0.002;
  const list = ui.clicks;
  if (!list.length) return toast('No click map yet: run Analyze in the Clean step.');
  let target: number | null = null;
  if (dir > 0) {
    for (const [t] of list) if (t > p + eps) { target = t; break; }
  } else {
    for (let i = list.length - 1; i >= 0; i--) if (list[i][0] < p - eps) { target = list[i][0]; break; }
  }
  if (target == null) return;
  seekTo(target);
  // zoom in around the click so it can be seen
  const span = Math.min(ui.t1 - ui.t0, 1.0);
  ui.t0 = Math.max(0, target - span / 2);
  ui.t1 = ui.t0 + span;
}

export function jumpTrack(dir: 1 | -1) {
  const v = ui.view;
  if (!v) return;
  const order = [...v.tracks].sort((a, b) => a.start_g - b.start_g);
  const cur = order.findIndex((t) => t.id === ui.selTrack);
  const next = order[Math.max(0, Math.min(order.length - 1, cur < 0 ? 0 : cur + dir))];
  if (next) {
    ui.selTrack = next.id;
    seekTo(next.start_g);
  }
}

export function repairSelection() {
  const s = ui.selection;
  if (!s) return toast('Select a region on the waveform, or paint one on the spectrogram, then press Repair.');
  run(() => api.addRepair(s.a, s.b, s.band), s.band ? 'Band repaired.' : 'Region repaired.');
  ui.selection = null;
}

export async function analyze(all = true) {
  await flushNow();
  try {
    await api.analyze(all);
  } catch (e) {
    fail(e);
  }
}

export async function undo() {
  await flushNow();
  run(() => api.undo());
}

export async function redo() {
  await flushNow();
  run(() => api.redo());
}

export function openExport() {
  if (!ui.view?.project.tracks.length) return toast('There are no tracks to export yet.', true);
  ui.dialog = 'export';
}

export function deleteSelectedTrack() {
  const t = selectedTrack();
  if (t) run(() => api.deleteTrack(t.id));
}

export function addTrackFromSelection() {
  const s = ui.selection;
  if (!s) return toast('Select a stretch of the timeline first.');
  run(() => api.addTrack(s.a, s.b));
}

export function selectTrackAtPlayhead() {
  const tv = trackAt(ui.tick.pos);
  if (tv) ui.selTrack = tv.id;
}

export function onJob(job: string, ok: boolean, data: any, error: string | undefined) {
  ui.job = null;
  if (!ok) {
    if (error !== 'cancelled') fail(error ?? 'failed');
    return;
  }
  switch (job) {
    case 'open':
      setView(data);
      fit();
      ui.step = data.project.sources.length ? (data.project.tracks.some((t: any) => t.analysis) ? 2 : 1) : 0;
      ui.clicks = [];
      toast(`Opened ${data.project.name}.`);
      break;
    case 'import':
    case 'folder':
      setView(data);
      fit();
      ui.step = 1;
      toast(job === 'folder' ? 'Folder imported.' : 'Imported and split on silence. Check the boundaries.');
      break;
    case 'analyze':
      setView(data);
      api.clicks().then((c) => (ui.clicks = c));
      toast('Analysis done.');
      break;
    case 'export':
      ui.lastExport = data;
      ui.dialog = 'report';
      api.projectGet().then(setView);
      break;
    case 'batch': {
      const d = data as { done: string[]; failed: string[] };
      toast(`Batch: ${d.done.length} album(s) exported${d.failed.length ? `, ${d.failed.length} failed` : ''}.`, d.failed.length > 0);
      for (const f of d.failed) toast(f, true);
      break;
    }
  }
}
