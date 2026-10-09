<script lang="ts">
  // Step 5: where the files go and how they are written.
  import { open } from '@tauri-apps/plugin-dialog';
  import { api } from '../lib/api';
  import { ui, edit, fail } from '../lib/store.svelte';
  import { openExport } from '../lib/actions';

  const p = $derived(ui.view?.project);

  async function pickCover() {
    const f = await open({ filters: [{ name: 'Images', extensions: ['jpg', 'jpeg', 'png'] }] });
    if (f) edit('Cover', (p) => (p.album.cover_path = String(f)));
  }
  async function pickRoot() {
    const d = await open({ directory: true });
    if (d) edit('Export folder', (p) => (p.export.root = String(d)));
  }
</script>

{#if p && ui.view}
<div class="col gap">
  <h2>Export</h2>
  <p class="muted small">Every export writes the cleaned tracks as tagged FLAC and the untouched raw capture of each side as WAV. Each export renders from the original audio through the full chain.</p>

  <div class="row">
    <span class="muted">FLAC</span>
    <button class:on={p.export.bit_depth === 16} title="16-bit with TPDF dither, matching a 16-bit source" onclick={() => edit('Bit depth', (p) => (p.export.bit_depth = 16))}>16-bit</button>
    <button class:on={p.export.bit_depth === 24} title="24-bit keeps the processing at full precision" onclick={() => edit('Bit depth', (p) => (p.export.bit_depth = 24))}>24-bit</button>
  </div>

  <label class="col"><span>Export folder</span>
    <div class="row">
      <input type="text" class="grow mono" value={p.export.root} onkeydown={(e) => e.stopPropagation()}
        onchange={(e) => edit('Export folder', (p) => (p.export.root = (e.target as HTMLInputElement).value))} />
      <button class="ghost" onclick={pickRoot}>Browse</button>
    </div>
  </label>
  <p class="small muted">Album goes to <span class="mono selectable">{ui.view.export_dir}</span></p>

  <div class="sep"></div>
  <h3>Record</h3>
  {#each [['artist', 'Artist'], ['album', 'Album'], ['album_artist', 'Album artist'], ['year', 'Year'], ['genre', 'Genre']] as [k, lab]}
    <label class="row field"><span class="lab">{lab}</span>
      <input type="text" class="grow" value={(p.album as any)[k]} onkeydown={(e) => e.stopPropagation()}
        onchange={(e) => { const v = (e.target as HTMLInputElement).value; edit('Album info', (p) => ((p.album as any)[k] = v)); }} /></label>
  {/each}
  <label class="col"><span>Cover art</span>
    <div class="row">
      <span class="mono small grow path">{p.album.cover_path || 'none'}</span>
      <button class="ghost" title="Pick an image. It is embedded in every track and saved as cover.jpg." onclick={pickCover}>Choose</button>
      {#if p.album.cover_path}<button class="ghost" onclick={() => edit('Cover', (p) => (p.album.cover_path = null))}>Clear</button>{/if}
    </div>
  </label>

  <div class="sep"></div>
  <button class="primary" onclick={openExport} disabled={!!ui.tick.busy} title="Name the tracks and export (Ctrl+E)">Export...<span class="key">Ctrl+E</span></button>
  <button onclick={() => (ui.dialog = 'batch')} title="Load folders, apply a preset, export. No listening required. (Ctrl+B)">Batch mode<span class="key">Ctrl+B</span></button>

  {#if ui.lastExport}
    <div class="sep"></div>
    <h3>Last export</h3>
    <p class="mono small selectable">{ui.lastExport.album_dir}</p>
    <div class="row">
      <button onclick={() => api.openFolder(ui.lastExport!.album_dir).catch(fail)}>Open folder</button>
      <button onclick={() => (ui.dialog = 'report')}>Report</button>
    </div>
  {/if}
</div>
{/if}

<style>
  .gap { gap: 9px; }
  .field .lab { width: 92px; }
  .path { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
