<script lang="ts">
  // Batch mode: folders in, finished albums out, one preset for all.
  import { open } from '@tauri-apps/plugin-dialog';
  import { api } from '../lib/api';
  import { ui, fail } from '../lib/store.svelte';

  let folders = $state<{ path: string; on: boolean }[]>([]);
  let preset = $state('');
  let cleaned = $state(false);
  let root = $state(ui.view?.project.export.root ?? '~/Music/Runout/Artists');
  let bits = $state(16);

  $effect(() => {
    if (!ui.presets.length) api.presets().then((p) => (ui.presets = p)).catch(fail);
  });

  async function pick() {
    const d = await open({ directory: true });
    if (!d) return;
    try {
      const list = await api.batchPlan(String(d));
      const have = new Set(folders.map((f) => f.path));
      for (const p of list) if (!have.has(p)) folders.push({ path: p, on: true });
    } catch (e) { fail(e); }
  }

  async function start() {
    const sel = folders.filter((f) => f.on).map((f) => f.path);
    try {
      ui.dialog = null;
      await api.batchRun(sel, preset || null, { cleaned, export_root: root, bit_depth: bits, target_lufs: null, ceiling_dbtp: null, album_mode: false, no_level: false });
    } catch (e) { fail(e); }
  }
</script>

<div class="modal-back" role="presentation" onkeydown={(e) => e.stopPropagation()}>
  <div class="modal">
    <header><h2 class="grow">Batch mode</h2></header>
    <div class="body col">
      <p class="muted small">Tune one record by hand, save the settings as a preset, then run the rest here. Each folder is one album: files in tag order, subfolders as discs. Leveling uses the preset's settings.</p>
      <div class="row"><button onclick={pick}>Add folder...</button><span class="muted small">Pick an album, or a folder of albums.</span></div>
      <div class="list">
        {#each folders as f, i (f.path)}
          <label class="row item"><input type="checkbox" bind:checked={f.on} /><span class="mono small grow">{f.path}</span>
            <button class="ghost" onclick={() => folders.splice(i, 1)}>Remove</button></label>
        {/each}
      </div>
      <label class="row"><span style="width:90px">Preset</span>
        <select class="grow" bind:value={preset}>
          <option value="">Default settings</option>
          {#each ui.presets as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
        </select></label>
      <div class="row"><span style="width:90px" class="muted">Files are</span>
        <button class:on={!cleaned} onclick={() => (cleaned = false)}>Raw</button>
        <button class:on={cleaned} onclick={() => (cleaned = true)}>Already cleaned</button></div>
      <div class="row"><span style="width:90px" class="muted">FLAC</span>
        <button class:on={bits === 16} onclick={() => (bits = 16)}>16-bit</button>
        <button class:on={bits === 24} onclick={() => (bits = 24)}>24-bit</button></div>
      <label class="row"><span style="width:90px">Export to</span><input type="text" class="grow mono" bind:value={root} /></label>
    </div>
    <footer>
      <button onclick={() => (ui.dialog = null)}>Cancel</button>
      <button class="primary" disabled={!folders.some((f) => f.on)} onclick={start}>Run {folders.filter((f) => f.on).length} album(s)</button>
    </footer>
  </div>
</div>

<style>
  .list { display: flex; flex-direction: column; gap: 2px; max-height: 240px; overflow: auto; }
  .item { padding: 2px 4px; }
</style>
