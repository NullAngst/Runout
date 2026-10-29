// Typed wrappers around the Tauri commands. Types mirror the Rust structs.
import { invoke } from '@tauri-apps/api/core';

export type LevelMode = 'track' | 'album';
export type Family = 'vinyl' | 'cassette' | 'cartridge' | 'reel' | 'video' | 'digital' | 'generic';
export type SpeedUnit = 'rpm' | 'cms' | 'ips' | 'none';
export type ChannelModeKind = 'stereo' | 'mono_sum' | 'mono_left' | 'mono_right' | 'best_auto';
export type SplitMode = 'absolute' | 'noise_floor' | 'program_change' | 'fixed_length';
export type ExportKind = 'album' | 'mixtape' | 'recording';
export type SideNaming = 'side_ab' | 'program' | 'reel' | 'tape';
export type CapturePath = 'analog' | 'spdif';
export type StopRule = 'runout' | 'tape_end' | 'digital_silence' | 'never';
export type EventKind = 'dropout' | 'dropoutflagged' | 'error' | 'errorflagged' | 'programchange';
export type OverlayKind = 'speed' | 'azimuth' | 'dropouts' | 'errors';

export interface Curve { lf_pole_us: number; zero_us: number; hf_pole_us: number }
export interface CurveSel { id: string; curve: Curve }
export interface NamedCurve { id: string; name: string; kind: 'tape' | 'disc' | 'any'; curve: Curve; note: string }

export interface Chain {
  wow: { enabled: boolean; reference: 'hum' | 'tone'; hum_hz: number; tone_hz: number };
  azimuth: { enabled: boolean; mode: 'auto' | 'static' | 'tracked'; static_us: number };
  channel_mode: { enabled: boolean; mode: ChannelModeKind };
  playback_eq: { enabled: boolean; from: CurveSel; to: CurveSel };
  nr: { enabled: boolean; kind: 'btype' | 'dbx2'; ref_dbfs: number; ref_measured: boolean };
  rumble: { enabled: boolean; freq_hz: number; slope_db_oct: number; linear_phase: boolean };
  hum: { enabled: boolean; fundamental_hz: number; harmonics: number; width_hz: number };
  dropout: { enabled: boolean; depth_db: number; max_ms: number; interp_ms: number };
  declick: { enabled: boolean; threshold_db: number; max_len_ms: number };
  decrackle: { enabled: boolean; threshold_db: number; max_len_ms: number };
  declip: { enabled: boolean; level_db: number; max_len_ms: number };
  denoise: { enabled: boolean; reduction_db: number; smoothing_pct: number };
  band_limit: { enabled: boolean; low_hz: number; high_hz: number; slope_db_oct: number };
  channel: { enabled: boolean; balance_db: number; swap: boolean; azimuth_us: number };
  eq: { enabled: boolean; low_shelf_hz: number; low_shelf_db: number; high_shelf_hz: number; high_shelf_db: number; bands: { freq_hz: number; gain_db: number; q: number }[] };
  error_repair: { enabled: boolean; min_run: number; max_ms: number };
  deemphasis: { enabled: boolean };
  level: { enabled: boolean; mode: LevelMode; target_lufs: number; ceiling_dbtp: number; max_gr_db: number };
  digital: boolean;
}

export interface NoiseProfile { rate: number; mag: number[]; from_s: number; to_s: number }
export interface Repair { id: number; start_s: number; end_s: number; band: [number, number] | null }
export interface ToneInfo { freq_hz: number; nominal_hz: number | null; level_dbfs: number; start_s: number; len_s: number; speed_err_pct: number | null }
export interface TapeAnalysis {
  speed_curve: string | null; azimuth_curve: string | null; dropout_map: string | null; error_map: string | null;
  best_channel: number | null; tone: ToneInfo | null; wow_note: string | null; wow_peak_pct: number | null; wow_weighted_pct: number | null;
}
export interface Speed { captured: number; correct: number; unit: SpeedUnit; cents: number; trim_pct: number }
export interface SourceFx {
  speed: Speed;
  noise: NoiseProfile | null;
  repairs: Repair[];
  analysis: TapeAnalysis;
}
export interface Source {
  id: number; name: string; path: string; kind: 'capture' | 'file' | 'folder'; raw: boolean;
  rate: number; channels: number; bits: number; frames: number; fx: SourceFx;
}
export interface Loudness { integrated: number; lra: number; true_peak: number; sample_peak: number }
export interface TrackAnalysis {
  key: number; before: Loudness; after: Loudness | null; gain: { gain_db: number; short_db: number };
  limiter_gr_db: number; clicks: number; crackles: number; noise_before_db: number; noise_after_db: number;
  duration_s: number; ceiling_fix_db: number; dropouts: number; dropouts_flagged: number; errors: number; errors_flagged: number;
}
export interface FileTags {
  title: string | null; artist: string | null; album: string | null; album_artist: string | null;
  year: string | null; genre: string | null; track: number | null; disc: number | null;
}
export interface Join { start_s: number; end_s: number; xfade_ms: number }
export interface Track {
  id: number; source_id: number; start_s: number; end_s: number; title: string;
  number: number | null; disc: number | null; fade_in_ms: number; fade_out_ms: number;
  chain: Chain | null; cleaned: boolean; file_name: string | null; tags: FileTags; analysis: TrackAnalysis | null; joins: Join[];
}
export interface SplitSettings { threshold_db: number; min_gap_s: number; mode: SplitMode; floor_margin_db: number; chapter_min: number }
export interface MediumPreset {
  id: string; name: string; family: Family; builtin: boolean; base: string | null; side_naming: SideNaming;
  default_condition: string | null;
  speed: { unit: SpeedUnit; nominal: number; choices: number[] };
  capture: { path: CapturePath; rate: number; fallback_rate: number; bits: number; stop: StopRule };
  chain: Chain; visible: string[]; split: SplitSettings;
  export: { kind: ExportKind; rate: number; full_rate_from_speed: number; bits: number; dither: boolean };
  media_tag: string; mb_format: string; notes: string;
}
export interface MediumRef { id: string; preset: MediumPreset; condition: string | null }
export interface DeckProfile { id: string; name: string; family: Family; speed_pct: number; azimuth_us: number; balance_db: number; eq_low_db: number; eq_high_db: number }
export interface MediaInfo { media: MediumPreset[]; families: [Family, string][]; decks: DeckProfile[]; curves: NamedCurve[]; stages: string[] }
export interface Project {
  version: number; name: string; chain: Chain; sources: Source[]; tracks: Track[];
  album: { artist: string; album: string; album_artist: string; year: string; genre: string; cover_path: string | null };
  split: SplitSettings;
  export: { bit_depth: number; root: string; kind: ExportKind; rate: number; channels: number; dither: boolean };
  medium: MediumRef;
  deck: string | null;
  next_id: number;
}
export interface Placement { source_id: number; start: number; duration: number; ratio: number; rate: number }
export interface TrackView { id: number; start_g: number; end_g: number; gain_db: number; short_db: number; fresh: boolean; passthrough: boolean }
export interface EventView { g: number; len: number; kind: EventKind; source_id: number; start_s: number; end_s: number; spans_song: boolean }
export interface ProjectView {
  project: Project; dir: string | null; layout: Placement[]; dirty: boolean; can_undo: boolean; can_redo: boolean;
  album_lufs: number | null; tracks: TrackView[]; stale: number; export_dir: string; analysed: boolean; events: EventView[];
}
export interface ImportItem { path: string; name: string; disc: number; track: number | null; tags: FileTags; raw: boolean }
export interface Preset { id: string; name: string; builtin: boolean; family: Family; chain: Chain }
export interface Meters { m: number; s: number; i: number; tp: number; lra: number; pre_m: number; pre_s: number; pre_i: number; pre_tp: number; pre_lra: number }
export interface LevelReading { l: number; r: number; hold_l: number; hold_r: number; clipped: boolean; seconds: number; bit_perfect: boolean | null; note: string | null }
export interface CaptureStatus { active: boolean; level_check: boolean; finished: boolean; level: LevelReading | null }
export interface Tick { playing: boolean; pos: number; warn: string | null; meters: Meters; peak: [number, number]; capture: CaptureStatus; busy: string | null }
export interface Settings { theme: string; last_medium: string; deck_by_family: Record<string, string>; library_root: string }
export interface AppInfo {
  version: string; projects_root: string; recent: string[]; music_root: string; settings: Settings;
  recovery: { project: string | null; autosave_newer: boolean; capture: string | null; capture_name: string | null } | null;
}
export interface ReleaseHit { id: string; title: string; artist: string; date: string; country: string; format: string; tracks: number }
export interface LookupTrack { number: string; title: string; length_s: number | null; side: string | null }
export interface Release { id: string; title: string; artist: string; date: string; genre: string; tracks: LookupTrack[] }
export interface ExportResult { album_dir: string; files: string[]; report: string; warnings: string[] }
export interface InputDevice { id: string; name: string; default: boolean; digital: boolean }
export interface CaptureResult {
  level_check: boolean; peak_db: number; seconds: number; clipped: boolean; auto_stopped: boolean; view: ProjectView | null;
  bit_perfect: boolean | null; notes: string[]; rate: number; bits: number;
}
export interface BatchOptions { cleaned: boolean; export_root: string | null; bit_depth: number | null; target_lufs: number | null; ceiling_dbtp: number | null; album_mode: boolean; no_level: boolean }

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  recoveryDismiss: () => invoke<void>('recovery_dismiss'),
  projectNew: (name: string, parent: string | null, medium: string | null, condition: string | null, deck: string | null) =>
    invoke<ProjectView>('project_new', { name, parent, medium, condition, deck }),
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
  devices: () => invoke<{ inputs: InputDevice[]; selected: string | null; path: CapturePath }>('devices'),
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
  batchRun: (folders: string[], preset: string | null, opts: BatchOptions, medium: string | null) => invoke<void>('batch_run', { folders, preset, opts, medium }),
  tileWave: (t0: number, t1: number, w: number, h: number, vzoom: number) => invoke<ArrayBuffer>('tile_wave', { t0, t1, w, h, vzoom }),
  tileSpec: (t0: number, t1: number, w: number, h: number) => invoke<ArrayBuffer>('tile_spec', { t0, t1, w, h }),
  tileOverlay: (t0: number, t1: number, w: number, h: number, kind: OverlayKind) => invoke<ArrayBuffer>('tile_overlay', { t0, t1, w, h, kind }),
  mediaInfo: () => invoke<MediaInfo>('media_info_get'),
  mediumSet: (id: string, condition: string | null, deck: string | null) => invoke<ProjectView>('medium_set', { id, condition, deck }),
  mediumSave: (name: string) => invoke<MediaInfo>('medium_save', { name }),
  mediumDelete: (id: string) => invoke<MediaInfo>('medium_delete', { id }),
  mediumVisible: (visible: string[]) => invoke<ProjectView>('medium_visible', { visible }),
  deckSave: (deck: DeckProfile) => invoke<MediaInfo>('deck_save', { deck }),
  deckDelete: (id: string) => invoke<MediaInfo>('deck_delete', { id }),
  deckMeasure: () => invoke<DeckProfile>('deck_measure'),
  settingsGet: () => invoke<Settings>('settings_get'),
  settingsSet: (settings: Settings) => invoke<Settings>('settings_set', { settings }),
  setTheme: (theme: string) => invoke<Settings>('set_theme', { theme }),
  findProgramChanges: (sourceId: number | null) => invoke<ProjectView>('find_program_changes', { sourceId }),
  splitProgramChanges: () => invoke<ProjectView>('split_program_changes'),
  joinProgramChange: (sourceId: number, startS: number, endS: number, xfadeMs: number | null) =>
    invoke<ProjectView>('join_program_change', { sourceId, startS, endS, xfadeMs }),
  speedFromLength: (sourceId: number, trackId: number | null, wantS: number) => invoke<ProjectView>('speed_from_length', { sourceId, trackId, wantS }),
};
