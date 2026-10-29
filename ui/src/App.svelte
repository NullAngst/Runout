<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { api, type Tick } from './lib/api';
  import { ui, fail, toast, setView, fit, flushNow, run, selectedTrack, applyTheme, loadMedia, family, isTape } from './lib/store.svelte';
  import * as act from './lib/actions';
  import Timeline from './components/Timeline.svelte';
  import TrackList from './components/TrackList.svelte';
  import BottomBar from './components/BottomBar.svelte';
  import Welcome from './components/Welcome.svelte';
  import CapturePanel from './panels/CapturePanel.svelte';
  import SplitPanel from './panels/SplitPanel.svelte';
  import CleanPanel from './panels/CleanPanel.svelte';
  import LevelPanel from './panels/LevelPanel.svelte';
  import ExportPanel from './panels/ExportPanel.svelte';
  import ExportDialog from './dialogs/ExportDialog.svelte';
  import FolderDialog from './dialogs/FolderDialog.svelte';
  import LookupDialog from './dialogs/LookupDialog.svelte';
  import BatchDialog from './dialogs/BatchDialog.svelte';
  import SmallDialogs from './dialogs/SmallDialogs.svelte';
  import MediumDialog from './dialogs/MediumDialog.svelte';
  import SettingsDialog from './dialogs/SettingsDialog.svelte';
  import type { OverlayKind } from './lib/api';

  const steps = ['Capture/Import', 'Split', 'Clean', 'Level', 'Export'];
  let timeline: Timeline | undefined = $state();
  const hasProject = $derived(!!ui.view?.dir);
  const fam = $derived(family());
  // overlays that mean something for this medium
  const overlayChoices = $derived.by((): [OverlayKind, string, string][] => {
    if (fam === 'digital') return [['errors', 'Errors', 'Digital errors: repaired runs, and longer ones flagged (bright)']];
    if (isTape(fam) || fam === 'generic')
      return [
        ['speed', 'Speed', 'Speed curve from the wow reference: up is fast. Needs Analyze with Wow on.'],
        ['azimuth', 'Azimuth', 'Right channel time offset against the left, over time. Needs Analyze with Azimuth on.'],
        ['dropouts', 'Dropouts', 'Dropout map: repaired dips, and ones too long to repair (bright)'],
      ];
    return [];
  });

  // startup and backend events
  $effect(() => {
    api.appInfo().then((i) => { ui.info = i; applyTheme(i.settings.theme); }).catch(fail);
    loadMedia();
    api.projectGet().then((v) => { if (v.dir) setView(v); }).catch(fail);
    const offs: Promise<() => void>[] = [
      listen<Tick>('tick', (e) => {
        const t = e.payload;
        // keep the local playhead while scrubbing a paused transport
        if (t.playing || ui.tick.playing !== t.playing) ui.tick.pos = t.pos;
        ui.tick.playing = t.playing;
        ui.tick.warn = t.warn;
        ui.tick.meters = t.meters;
        ui.tick.peak = t.peak;
        ui.tick.capture = t.capture;
        ui.tick.busy = t.busy;
      }),
      listen<{ job: string; frac: number; msg: string }>('progress', (e) => (ui.job = e.payload)),
      listen<{ job: string; ok: boolean; data: any; error?: string }>('job', (e) => {
        act.onJob(e.payload.job, e.payload.ok, e.payload.data, e.payload.error);
        const rc = (window as any).__runoutRecover;
        if (e.payload.job === 'open' && e.payload.ok && rc) {
          (window as any).__runoutRecover = null;
          run(() => api.recoverCapture(rc.path, rc.name), 'Capture recovered.');
        }
      }),
    ];
    const win = getCurrentWindow();
    const close = win.onCloseRequested(async (ev) => {
      await flushNow();
      if (ui.tick.capture.active && !ui.tick.capture.level_check) {
        ev.preventDefault();
        toast('Stop the recording before closing.', true);
        return;
      }
      if (ui.view?.dirty && ui.view.dir) {
        const yes = await ask('Save changes to this project before closing?', { title: 'Runout', kind: 'warning', okLabel: 'Save', cancelLabel: "Don't save" });
        if (yes) await api.projectSave().catch(fail);
      }
    });
    return () => {
      offs.forEach((p) => p.then((f) => f()));
      close.then((f) => f());
    };
  });

  // window title
  $effect(() => {
    const v = ui.view;
    const t = v?.dir ? `${v.project.name}${v.dirty ? ' *' : ''} - Runout` : 'Runout';
    getCurrentWindow().setTitle(t).catch(() => {});
  });

  function key(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key;
    const handled = () => e.preventDefault();

    if (ui.dialog) {
      if (k === 'Escape') { ui.dialog = null; handled(); }
      return;
    }
    if (ctrl && k.toLowerCase() === 'o') { act.openProject(); return handled(); }
    if (ctrl && k.toLowerCase() === 'n') { act.newProject(); return handled(); }
    if (ctrl && k === ',') { ui.dialog = 'settings'; return handled(); }
    if (!hasProject) return;

    if (ctrl) {
      switch (k.toLowerCase()) {
        case 'z': e.shiftKey ? act.redo() : act.undo(); return handled();
        case 'y': act.redo(); return handled();
        case 's': act.save(); return handled();
        case 'e': act.openExport(); return handled();
        case 'b': ui.dialog = 'batch'; return handled();
        case 'l':
          if (ui.view?.project.export.kind !== 'album') { toast('Tracklist lookup is for albums. Set the project kind to Album in the Export step to use it.'); return handled(); }
          ui.dialog = 'lookup'; return handled();
        case 'm': ui.dialog = 'medium'; return handled();
        case 'i': e.shiftKey ? act.importFolder() : act.importFiles(); return handled();
        case 'r': ui.step = 0; ui.captureCmd = 'record'; return handled();
        case 'enter': act.analyze(true); return handled();
        case '=': case '+': timeline?.zoom(0.5); return handled();
        case '-': timeline?.zoom(2); return handled();
      }
      return;
    }
    switch (k) {
      case ' ':
        if (e.shiftKey) ui.captureCmd = 'stop';
        else act.togglePlay();
        return handled();
      case '1': case '2': case '3': case '4': case '5': ui.step = parseInt(k) - 1; return handled();
      case 'm': case 'M': act.markBoundary(); return handled();
      case ']': act.jumpClick(1); return handled();
      case '[': act.jumpClick(-1); return handled();
      case 'a': case 'A': ui.monitor = ui.monitor === 1 ? 0 : 1; api.setMonitor(ui.monitor); return handled();
      case 'r': case 'R': ui.monitor = ui.monitor === 2 ? 0 : 2; api.setMonitor(ui.monitor); return handled();
      case 'l': case 'L':
        ui.loop = !ui.loop;
        api.setLoop(ui.loop && ui.selection ? [ui.selection.a, ui.selection.b] : null);
        toast(ui.loop ? (ui.selection ? 'Looping the selection.' : 'Loop on: select a region to loop.') : 'Loop off.');
        return handled();
      case 'x': case 'X': act.repairSelection(); return handled();
      case 't': case 'T': act.addTrackFromSelection(); return handled();
      case 'j': case 'J': { const t = selectedTrack(); if (t) run(() => api.mergeNext(t.id)); return handled(); }
      case 'c': case 'C': ui.step = 0; ui.captureCmd = 'check'; return handled();
      case 'w': case 'W': ui.mode = ui.mode === 'both' ? 'wave' : ui.mode === 'wave' ? 'spec' : 'both'; return handled();
      case 'Delete': case 'Backspace': act.deleteSelectedTrack(); return handled();
      case 'ArrowLeft': act.seekTo(ui.tick.pos - (e.shiftKey ? 1 : 5)); return handled();
      case 'ArrowRight': act.seekTo(ui.tick.pos + (e.shiftKey ? 1 : 5)); return handled();
      case 'ArrowUp': act.jumpTrack(-1); return handled();
      case 'ArrowDown': act.jumpTrack(1); return handled();
      case 'Home': act.seekTo(0); return handled();
      case 'End': act.seekTo(1e9); return handled();
      case '+': case '=': timeline?.zoom(0.5); return handled();
      case '-': timeline?.zoom(2); return handled();
      case '0': fit(); return handled();
      case 'Escape': ui.selection = null; if (ui.loop) { ui.loop = false; api.setLoop(null); } return handled();
      case '?': ui.dialog = 'help'; return handled();
    }
  }
</script>

<svelte:window onkeydown={key} />

{#if !hasProject}
  <Welcome />
{:else}
  <div class="shell">
    <nav class="steps">
      {#each steps as s, i}
        <button class="step" class:active={ui.step === i} onclick={() => (ui.step = i)} title="{s} ({i + 1})">{s}</button>
      {/each}
      <span class="grow"></span>
      <div class="row tools">
        <button class="medium" onclick={() => (ui.dialog = 'medium')} title="The medium this project is: sets the stages, capture, splitting and export (Ctrl+M)">{ui.view?.project.medium.preset.name ?? 'Medium'}</button>
        <button class="ghost" disabled={!ui.view?.can_undo} onclick={act.undo} title="Undo (Ctrl+Z)">Undo</button>
        <button class="ghost" disabled={!ui.view?.can_redo} onclick={act.redo} title="Redo (Ctrl+Shift+Z)">Redo</button>
        <button class="ghost" onclick={act.save} title="Save the project (Ctrl+S). It also autosaves every 30 seconds.">{ui.view?.dirty ? 'Save *' : 'Saved'}</button>
        <button class="ghost" onclick={() => (ui.dialog = 'help')} title="Keyboard shortcuts (?)">Keys</button>
        <button class="ghost" onclick={() => (ui.dialog = 'settings')} title="Theme, deck profiles and library folder (Ctrl+,)">Settings</button>
        <button class="ghost" onclick={act.newProject} title="Close this project and start or open another (Ctrl+N)">Projects</button>
      </div>
    </nav>

    <div class="main">
      <aside class="panel left"><TrackList /></aside>
      <section class="panel center">
        <div class="tlhead row">
          <h3 class="grow">{ui.view?.project.name}</h3>
          <div class="row seg">
            <button class:on={ui.mode === 'wave'} onclick={() => (ui.mode = 'wave')} title="Waveform only (W cycles)">Waveform</button>
            <button class:on={ui.mode === 'spec'} onclick={() => (ui.mode = 'spec')} title="Spectrogram only: clicks show as vertical lines, hum as a flat horizontal one">Spectrogram</button>
            <button class:on={ui.mode === 'both'} onclick={() => (ui.mode = 'both')} title="Both, stacked">Both</button>
          </div>
          {#if overlayChoices.length}
            <div class="row seg">
              {#each overlayChoices as [k, lab, tip]}
                <button class:on={ui.overlays[k]} onclick={() => (ui.overlays[k] = !ui.overlays[k])} title={tip}>{lab}</button>
              {/each}
            </div>
          {/if}
          <div class="row seg">
            <button class="ghost" onclick={() => timeline?.zoom(2)} title="Zoom out (-)">-</button>
            <button class="ghost" onclick={() => timeline?.zoom(0.5)} title="Zoom in (+)">+</button>
            <button class="ghost" onclick={fit} title="Fit everything (0)">Fit</button>
            <button class="ghost" onclick={() => (ui.vzoom = ui.vzoom >= 16 ? 1 : ui.vzoom * 2)} title="Vertical zoom for quiet records">x{ui.vzoom}</button>
          </div>
        </div>
        <div class="tlbody"><Timeline bind:this={timeline} /></div>
      </section>
      <aside class="panel right scroll">
        {#if ui.step === 0}<CapturePanel />
        {:else if ui.step === 1}<SplitPanel />
        {:else if ui.step === 2}<CleanPanel />
        {:else if ui.step === 3}<LevelPanel />
        {:else}<ExportPanel />{/if}
      </aside>
    </div>

    <BottomBar onPlay={act.togglePlay} />
  </div>
{/if}

{#if ui.job && ui.tick.busy}
  <div class="job panel">
    <div class="row"><span class="grow small">{ui.job.msg}</span><button class="ghost small" onclick={() => api.cancelJob()}>Cancel</button></div>
    <div class="bar"><div class="fill" style="width:{Math.round(ui.job.frac * 100)}%"></div></div>
  </div>
{/if}

{#if ui.dialog === 'export'}<ExportDialog />
{:else if ui.dialog === 'folder'}<FolderDialog />
{:else if ui.dialog === 'lookup'}<LookupDialog />
{:else if ui.dialog === 'batch'}<BatchDialog />
{:else if ui.dialog === 'medium' || ui.dialog === 'medium-save'}<MediumDialog />
{:else if ui.dialog === 'settings'}<SettingsDialog />
{:else}<SmallDialogs />{/if}

<div class="toasts">
  {#each ui.toasts as t (t.id)}<div class="toast" class:err={t.err}>{t.text}</div>{/each}
</div>

<style>
  .shell { height: 100%; display: flex; flex-direction: column; }
  .steps { display: flex; gap: 6px; padding: 8px 8px 0; align-items: center; }
  .step { flex: 0 1 170px; padding: 7px 10px; font-weight: 500; background: var(--surface); }
  .step.active { border-color: var(--accent); color: var(--accent); background: var(--accent-wash); }
  .medium { font-size: 12px; border-color: var(--accent-dim); color: var(--accent); }
  .tools button { font-size: 12px; }
  .main { flex: 1; min-height: 0; display: grid; grid-template-columns: 270px 1fr 340px; gap: 8px; padding: 8px; }
  .left { min-height: 0; overflow: hidden; }
  .center { min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .right { min-height: 0; padding: 12px; overflow-x: hidden; }
  .tlhead { padding: 6px 8px; gap: 10px; border-bottom: 1px solid var(--border); }
  .seg { gap: 2px; }
  .seg button { padding: 3px 8px; font-size: 12px; }
  .tlbody { flex: 1; min-height: 0; }
  .job { position: fixed; top: 56px; left: 50%; transform: translateX(-50%); width: 420px; padding: 10px 12px; z-index: 40; box-shadow: 0 10px 30px var(--drop); }
  .bar { height: 5px; background: var(--bg); border-radius: 3px; margin-top: 6px; overflow: hidden; }
  .fill { height: 100%; background: var(--accent); transition: width .2s; }
</style>
