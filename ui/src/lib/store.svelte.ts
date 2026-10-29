// Shared UI state (Svelte 5 runes).
import { api, type AppInfo, type Chain, type ExportResult, type Family, type MediaInfo, type MediumPreset, type OverlayKind, type Preset, type Project, type ProjectView, type Tick, type Track } from './api';

export type Dialog = null | 'export' | 'folder' | 'lookup' | 'batch' | 'help' | 'new' | 'report' | 'preset-save' | 'medium' | 'settings' | 'medium-save';

export interface Selection { a: number; b: number; band: [number, number] | null }

const emptyTick: Tick = {
  playing: false, pos: 0, warn: null, busy: null, peak: [-120, -120],
  meters: { m: -70, s: -70, i: -70, tp: -70, lra: 0, pre_m: -70, pre_s: -70, pre_i: -70, pre_tp: -70, pre_lra: 0 },
  capture: { active: false, level_check: false, finished: false, level: null },
};

export const ui = $state({
  info: null as AppInfo | null,
  view: null as ProjectView | null,
  step: 0,
  tick: emptyTick as Tick,
  selTrack: null as number | null,
  selection: null as Selection | null,
  t0: 0,
  t1: 60,
  mode: 'both' as 'wave' | 'spec' | 'both',
  monitor: 0,
  loop: false,
  vzoom: 1,
  clicks: [] as [number, number, boolean][],
  presets: [] as Preset[],
  job: null as { job: string; frac: number; msg: string } | null,
  toasts: [] as { id: number; text: string; err: boolean }[],
  dialog: null as Dialog,
  dialogData: null as any,
  lastExport: null as ExportResult | null,
  scope: 'all' as 'all' | 'track',
  captureCmd: null as null | 'record' | 'check' | 'stop',
  theme: 'verdigris',
  /** Bumped on every theme switch, so the pictures drawn in Rust redraw. */
  themeGen: 0,
  overlays: { speed: false, azimuth: false, dropouts: true, errors: true } as Record<OverlayKind, boolean>,
  media: null as MediaInfo | null,
});

export const THEMES: [string, string][] = [
  ['verdigris', 'Verdigris'],
  ['light', 'Light'],
  ['dark', 'Dark'],
  ['kawaii-light', 'Kawaii Light'],
  ['kawaii-dark', 'Kawaii Dark'],
];

/** Restyle the window. The tile palette is switched in Rust by set_theme. */
export function applyTheme(theme: string) {
  if (!THEMES.some(([id]) => id === theme)) theme = 'verdigris';
  ui.theme = theme;
  document.documentElement.dataset.theme = theme;
  ui.themeGen++;
}

/** A token's value from the current theme. */
export function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** A #rrggbb token with an alpha, for canvas drawing. */
export function tokenAlpha(name: string, a: number): string {
  const v = token(name);
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(v);
  if (!m) return v;
  return `rgba(${parseInt(m[1], 16)},${parseInt(m[2], 16)},${parseInt(m[3], 16)},${a})`;
}

export async function loadMedia() {
  try {
    ui.media = await api.mediaInfo();
  } catch (e) {
    fail(e);
  }
}

/** The open project's medium. */
export function medium(): MediumPreset | null {
  return ui.view?.project.medium.preset ?? null;
}

export function family(): Family {
  return ui.view?.project.medium.preset.family ?? 'vinyl';
}

export const isTape = (f: Family) => f === 'cassette' || f === 'cartridge' || f === 'reel' || f === 'video';

let toastId = 0;
export function toast(text: string, err = false) {
  const id = ++toastId;
  ui.toasts.push({ id, text, err });
  setTimeout(() => { ui.toasts = ui.toasts.filter((t) => t.id !== id); }, err ? 7000 : 3500);
}

export function fail(e: unknown) {
  toast(String(e).replace(/^Error: /, ''), true);
}

export function setView(v: ProjectView) {
  const had = ui.view?.project.sources.length ?? 0;
  ui.view = v;
  if (ui.selTrack != null && !v.project.tracks.some((t) => t.id === ui.selTrack)) ui.selTrack = null;
  const end = timelineEnd();
  if (end > 0 && (had === 0 || ui.t1 > end + 5)) fit();
}

export function timelineEnd(): number {
  const l = ui.view?.layout ?? [];
  return l.length ? l[l.length - 1].start + l[l.length - 1].duration : 0;
}

export function fit() {
  const end = timelineEnd();
  ui.t0 = 0;
  ui.t1 = Math.max(end, 10);
}

export function clone<T>(x: T): T {
  return JSON.parse(JSON.stringify(x));
}

// Edits go to the backend as whole projects. Rapid edits with the same label
// (slider drags) are throttled here and merged into one undo step there.
let pending: { p: Project; label: string } | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

export function edit(label: string, f: (p: Project) => void) {
  if (!ui.view) return;
  const base = pending ? pending.p : clone(ui.view.project);
  f(base);
  pending = { p: base, label };
  // optimistic local update so controls track the pointer
  ui.view.project = clone(base);
  if (!timer) {
    timer = setTimeout(flush, 70);
  }
}

async function flush() {
  timer = null;
  const job = pending;
  pending = null;
  if (!job) return;
  try {
    const v = await api.projectSet(job.p, job.label);
    if (!pending) setView(v);
  } catch (e) {
    fail(e);
  }
}

export async function flushNow() {
  if (timer) {
    clearTimeout(timer);
    await flush();
  }
}

/** Call a command that returns a fresh ProjectView. */
export async function run(f: () => Promise<ProjectView>, ok?: string) {
  await flushNow();
  try {
    setView(await f());
    if (ok) toast(ok);
  } catch (e) {
    fail(e);
  }
}

export function selectedTrack(): Track | null {
  return ui.view?.project.tracks.find((t) => t.id === ui.selTrack) ?? null;
}

/** The chain the Clean and Level panels edit right now. */
export function chainIn(p: Project): Chain {
  if (ui.scope === 'track' && ui.selTrack != null) {
    const t = p.tracks.find((t) => t.id === ui.selTrack);
    if (t?.chain) return t.chain;
  }
  return p.chain;
}

export function currentChain(): Chain | null {
  return ui.view ? chainIn(ui.view.project) : null;
}

/** Source under the selected track, or the first raw source. */
export function currentSourceId(): number | null {
  const v = ui.view;
  if (!v) return null;
  const t = selectedTrack();
  if (t) return t.source_id;
  return v.project.sources.find((s) => s.raw)?.id ?? v.project.sources[0]?.id ?? null;
}

export function trackAt(g: number) {
  return ui.view?.tracks.find((t) => g >= t.start_g && g < t.end_g) ?? null;
}

export async function refreshClicks() {
  try {
    ui.clicks = await api.clicks();
  } catch {
    /* ignore */
  }
}
