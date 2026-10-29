<script lang="ts">
  // Shown with no project open: new, open, recent, and crash recovery.
  import { open } from '@tauri-apps/plugin-dialog';
  import { api } from '../lib/api';
  import { ui, fail, setView } from '../lib/store.svelte';
  import { openProject } from '../lib/actions';
  import MediumPicker from './MediumPicker.svelte';

  const today = new Date().toISOString().slice(0, 10);
  let name = $state(`Record ${today}`);
  let parent = $state<string | null>(null);
  const rec = $derived(ui.info?.recovery ?? null);
  // the last medium used is preselected, Vinyl LP on first run
  let medium = $state('vinyl-lp');
  let condition = $state('');
  let deck = $state('');
  let seeded = false;
  $effect(() => {
    const last = ui.info?.settings.last_medium;
    if (seeded || !last || !ui.media) return;
    seeded = true;
    if (ui.media.media.some((m) => m.id === last)) medium = last;
  });

  async function create() {
    try {
      setView(await api.projectNew(name, parent, medium, condition || null, deck || null));
      api.appInfo().then((i) => (ui.info = i)).catch(() => {});
      ui.step = 0;
    } catch (e) { fail(e); }
  }
  async function pickParent() {
    const d = await open({ directory: true, defaultPath: ui.info?.projects_root });
    if (d) parent = String(d);
  }
  async function restore(autosave: boolean) {
    if (!rec?.project) return;
    if (rec.capture) (window as any).__runoutRecover = { path: rec.capture, name: rec.capture_name };
    try { await api.projectOpen(rec.project, autosave); } catch (e) { fail(e); }
    await api.recoveryDismiss();
    if (ui.info) ui.info.recovery = null;
  }
  async function dismiss() {
    await api.recoveryDismiss();
    if (ui.info) ui.info.recovery = null;
  }
</script>

<div class="welcome">
  <div class="inner col">
    <h1>Runout</h1>
    <p class="muted">Records, tapes and digital sources from raw capture to clean, level-matched, tagged files.</p>

    {#if rec && (rec.project || rec.capture)}
      <div class="card col rec">
        <h3 class="attention">Runout did not close cleanly last time</h3>
        {#if rec.project}<p class="small">Project: <span class="mono">{rec.project}</span>{rec.autosave_newer ? ' (an autosave is newer than the last save)' : ''}</p>{/if}
        {#if rec.capture}<p class="small">A capture was in progress: <span class="mono">{rec.capture}</span>. It can be recovered up to the last second written.</p>{/if}
        <div class="row">
          {#if rec.project && rec.autosave_newer}<button class="primary" onclick={() => restore(true)}>Restore autosave{rec.capture ? ' and capture' : ''}</button>{/if}
          {#if rec.project}<button onclick={() => restore(false)}>Open last save{rec.capture ? ' and recover capture' : ''}</button>{/if}
          <button class="ghost" onclick={dismiss}>Dismiss</button>
        </div>
      </div>
    {/if}

    <div class="card col">
      <h2>New project</h2>
      <label class="col"><span>Name</span><input type="text" bind:value={name} onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') create(); }} /></label>
      <MediumPicker bind:medium bind:condition bind:deck />
      <div class="row small"><span class="muted">Saved in</span><span class="mono grow">{parent ?? ui.info?.projects_root ?? ''}</span><button class="ghost" onclick={pickParent}>Change</button></div>
      <button class="primary" onclick={create} disabled={!name.trim()}>Create<span class="key">Enter</span></button>
    </div>

    <div class="row">
      <button onclick={openProject}>Open project...<span class="key">Ctrl+O</span></button>
      <span class="muted small">A project is a <span class="mono">.runout</span> folder holding the captures and settings.</span>
    </div>

    {#if ui.info?.recent.length}
      <h3>Recent</h3>
      {#each ui.info.recent as r}
        <button class="ghost recent mono small" onclick={() => api.projectOpen(r).catch(fail)}>{r}</button>
      {/each}
    {/if}
  </div>
</div>

<style>
  .welcome { height: 100%; display: grid; place-items: center; overflow: auto; }
  .inner { width: min(760px, 92vw); gap: 14px; padding: 30px 0; }
  h1 { font-size: 34px; color: var(--accent); }
  .recent { text-align: left; justify-content: flex-start; }
  .rec { border-color: var(--attention-dim); }
</style>
