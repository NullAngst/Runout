// Typed wrappers around the Tauri commands. Types mirror the Rust structs.
import { invoke } from '@tauri-apps/api/core';

export type LevelMode = 'track' | 'album';

export interface Chain {
  rumble: { enabled: boolean; freq_hz: number; slope_db_oct: number; linear_phase: boolean };
  hum: { enabled: boolean; mains_hz: number; harmonics: number; width_hz: number };
  declick: { enabled: boolean; threshold_db: number; max_len_ms: number };
  decrackle: { enabled: boolean; threshold_db: number; max_len_ms: number };
  declip: { enabled: boolean; level_db: number; max_len_ms: number };
  denoise: { enabled: boolean; reduction_db: number; smoothing_pct: number };
  channel: { enabled: boolean; balance_db: number; swap: boolean; azimuth_us: number };
  eq: { enabled: boolean; low_shelf_hz: number; low_shelf_db: number; high_shelf_hz: number; high_shelf_db: number; bands: { freq_hz: number; gain_db: number; q: number }[] };
  level: { enabled: boolean; mode: LevelMode; target_lufs: number; ceiling_dbtp: number; max_gr_db: number };
}

export interface NoiseProfile { rate: number; mag: number[]; from_s: number; to_s: number }
export interface Repair { id: number; start_s: number; end_s: number; band: [number, number] | null }
export interface SourceFx {
  speed: { captured_rpm: number; correct_rpm: number; cents: number };
  noise: NoiseProfile | null;
  repairs: Repair[];
}
export interface Source {
  id: number; name: string; path: string; kind: 'capture' | 'file' | 'folder'; raw: boolean;
  rate: number; channels: number; bits: number; frames: number; fx: SourceFx;
}
export interface Loudness { integrated: number; lra: number; true_peak: number; sample_peak: number }
export interface TrackAnalysis {
  key: number; before: Loudness; after: Loudness | null; gain: { gain_db: number; short_db: number };
  limiter_gr_db: number; clicks: number; crackles: number; noise_before_db: number; noise_after_db: number;
  duration_s: number; ceiling_fix_db: number;
}
export interface FileTags {
  title: string | null; artist: string | null; album: string | null; album_artist: string | null;
  year: string | null; genre: string | null; track: number | null; disc: number | null;
}
export interface Track {
  id: number; source_id: number; start_s: number; end_s: number; title: string;
  number: number | null; disc: number | null; fade_in_ms: number; fade_out_ms: number;
  chain: Chain | null; cleaned: boolean; file_name: string | null; tags: FileTags; analysis: TrackAnalysis | null;
}
export interface Project {
  version: number; name: string; chain: Chain; sources: Source[]; tracks: Track[];
  album: { artist: string; album: string; album_artist: string; year: string; genre: string; cover_path: string | null };
  split: { threshold_db: number; min_gap_s: number };
  export: { bit_depth: number; root: string };
  next_id: number;
}
export interface Placement { source_id: number; start: number; duration: number; ratio: number; rate: number }
export interface TrackView { id: number; start_g: number; end_g: number; gain_db: number; short_db: number; fresh: boolean; passthrough: boolean }
export interface ProjectView {
  project: Project; dir: string | null; layout: Placement[]; dirty: boolean; can_undo: boolean; can_redo: boolean;
  album_lufs: number | null; tracks: TrackView[]; stale: number; export_dir: string; analysed: boolean;
}
export interface ImportItem { path: string; name: string; disc: number; track: number | null; tags: FileTags; raw: boolean }
export interface Preset { id: string; name: string; builtin: boolean; chain: Chain }
export interface Meters { m: number; s: number; i: number; tp: number; lra: number; pre_m: number; pre_s: number; pre_i: number; pre_tp: number; pre_lra: number }
export interface LevelReading { l: number; r: number; hold_l: number; hold_r: number; clipped: boolean; seconds: number }
export interface CaptureStatus { active: boolean; level_check: boolean; finished: boolean; level: LevelReading | null }
export interface Tick { playing: boolean; pos: number; warn: string | null; meters: Meters; peak: [number, number]; capture: CaptureStatus; busy: string | null }
export interface AppInfo {
  version: string; projects_root: string; recent: string[]; music_root: string;
  recovery: { project: string | null; autosave_newer: boolean; capture: string | null; capture_name: string | null } | null;
}
export interface ReleaseHit { id: string; title: string; artist: string; date: string; country: string; format: string; tracks: number }
export interface LookupTrack { number: string; title: string; length_s: number | null; side: string | null }
export interface Release { id: string; title: string; artist: string; date: string; genre: string; tracks: LookupTrack[] }
export interface ExportResult { album_dir: string; files: string[]; report: string; warnings: string[] }
export interface InputDevice { id: string; name: string; default: boolean }
export interface CaptureResult { level_check: boolean; peak_db: number; seconds: number; clipped: boolean; auto_stopped: boolean; view: ProjectView | null }
export interface BatchOptions { cleaned: boolean; export_root: string | null; bit_depth: number | null; target_lufs: number | null; ceiling_dbtp: number | null; album_mode: boolean; no_level: boolean }

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  recoveryDismiss: () => invoke<void>('recovery_dismiss'),
  projectNew: (name: string, parent: string | null) => invoke<ProjectView>('project_new', { name, parent }),
  projectOpen: (dir: string, autosave = false) => invoke<void>('project_open', { dir, autosave }),
  projectGet: () => invoke<ProjectView>('project_get'),
  projectSave: () => invoke<ProjectView>('project_save'),
  projectSet: (project: Project, label: string) => invoke<ProjectView>('project_set', { project, label }),
  undo: () => invoke<ProjectView>('undo'),
  redo: () => invoke<ProjectView>('redo'),
  importFiles: (paths: string[]) => invoke<void>('import_files', { paths }),
  folderPlan: (dir: string) => invoke<ImportItem[]>('folder_plan', { dir }),
  folderCommit: (items: ImportItem[]) => invoke<void>('folder_commit', { items }),
  removeSource: (sourceId: number) => invoke<ProjectView>('remove_source', { sourceId }),
  splitAuto: (sourceId: number | null) => invoke<ProjectView>('split_auto', { sourceId }),
  splitAutoThreshold: () => invoke<ProjectView>('split_auto_threshold'),
  markBoundary: (at: number) => invoke<ProjectView>('mark_boundary', { at }),
  addTrack: (from: number, to: number) => invoke<ProjectView>('add_track', { from, to }),
  mergeNext: (trackId: number) => invoke<ProjectView>('merge_next', { trackId }),
  deleteTrack: (trackId: number) => invoke<ProjectView>('delete_track', { trackId }),
  setCleaned: (trackId: number, cleaned: boolean) => invoke<ProjectView>('set_cleaned', { trackId, cleaned }),
  setOverride: (trackId: number, on: boolean) => invoke<ProjectView>('set_override', { trackId, on }),
  autoStage: (stage: string, trackId: number | null) => invoke<ProjectView>('auto_stage', { stage, trackId }),
  learnNoise: (from: number, to: number) => invoke<ProjectView>('learn_noise', { from, to }),
  addRepair: (from: number, to: number, band: [number, number] | null) => invoke<ProjectView>('add_repair', { from, to, band }),
  removeRepair: (repairId: number) => invoke<ProjectView>('remove_repair', { repairId }),
  analyze: (all: boolean) => invoke<void>('analyze', { all }),
  cancelJob: () => invoke<void>('cancel_job'),
  clicks: () => invoke<[number, number, boolean][]>('clicks'),
  presets: () => invoke<Preset[]>('presets_list'),
  presetApply: (id: string, trackId: number | null) => invoke<ProjectView>('preset_apply', { id, trackId }),
  presetSave: (name: string, trackId: number | null) => invoke<Preset[]>('preset_save', { name, trackId }),
  presetDelete: (id: string) => invoke<Preset[]>('preset_delete', { id }),
  exportRun: () => invoke<void>('export_run'),
  openFolder: (path: string) => invoke<void>('open_folder', { path }),
  readReport: (path: string) => invoke<string>('read_report', { path }),
  lookupSearch: (artist: string, album: string) => invoke<ReleaseHit[]>('lookup_search', { artist, album }),
  lookupRelease: (id: string) => invoke<Release>('lookup_release', { id }),
  applyTracklist: (tracks: { title: string; length_s: number | null; side: string | null }[], snap: boolean, artist: string, album: string, year: string, genre: string) =>
    invoke<ProjectView>('apply_tracklist', { tracks, snap, artist, album, year, genre }),
  devices: () => invoke<{ inputs: InputDevice[]; selected: string | null }>('devices'),
  selectDevice: (id: string) => invoke<void>('select_device', { id }),
  levelCheck: (device: string | null) => invoke<void>('level_check', { device }),
  captureStart: (name: string, device: string | null, autoStopS: number | null) => invoke<void>('capture_start', { name, device, autoStopS }),
  captureStop: () => invoke<CaptureResult>('capture_stop'),
  recoverCapture: (path: string, name: string | null) => invoke<ProjectView>('recover_capture', { path, name }),
  play: (from: number) => invoke<void>('play', { from }),
  pause: () => invoke<void>('pause'),
  seek: (to: number) => invoke<void>('seek', { to }),
  setLoop: (range: [number, number] | null) => invoke<void>('set_loop', { range }),
  setMonitor: (mode: number) => invoke<void>('set_monitor', { mode }),
  batchPlan: (root: string) => invoke<string[]>('batch_plan', { root }),
  batchRun: (folders: string[], preset: string | null, opts: BatchOptions) => invoke<void>('batch_run', { folders, preset, opts }),
  tileWave: (t0: number, t1: number, w: number, h: number, vzoom: number) => invoke<ArrayBuffer>('tile_wave', { t0, t1, w, h, vzoom }),
  tileSpec: (t0: number, t1: number, w: number, h: number) => invoke<ArrayBuffer>('tile_spec', { t0, t1, w, h }),
};
