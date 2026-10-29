<script lang="ts">
  // Step 5: where the files go and how they are written.
  import { open } from '@tauri-apps/plugin-dialog';
  import { api } from '../lib/api';
  import { ui, edit, fail } from '../lib/store.svelte';
  import { openExport } from '../lib/actions';

  const p = $derived(ui.view?.project);
  const kinds: [string, string, string][] = [
    ['album', 'Album', 'A commercial release: Artist / Album Artist - Album / 01. Title.flac'],
    ['mixtape', 'Mixtape', 'Compilations, radio rips, mixes: Mixtapes / Title / 01. Artist - Title.flac, album artist Various Artists'],
    ['recording', 'Recording', 'Dictation, family tapes, airchecks, lectures: Recordings / Date Title / 01. Title.flac, untitled parts named Part 1, Part 2'],
  ];
  const fields = $derived(
    p?.export.kind === 'album'
      ? [['artist', 'Artist'], ['album', 'Album'], ['album_artist', 'Album artist'], ['year', 'Year'], ['genre', 'Genre']]
      : p?.export.kind === 'mixtape'
        ? [['album', 'Tape title'], ['year', 'Year'], ['genre', 'Genre']]
        : [['album', 'Title'], ['artist', 'Who'], ['year', 'Date'], ['genre', 'Genre']],
  );

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
  {#if p.chain.digital}<p class="muted small">Digital source: with every stage off, the FLAC is bit-identical to what came off the S/PDIF input.</p>{/if}

  <div class="row" title="Decides the folder layout and the tags">
    <span class="muted">Kind</span>
    {#each kinds as [k, lab, tip]}
      <button class:on={p.export.kind === k} title={tip} onclick={() => edit('Project kind', (p) => (p.export.kind = k as any))}>{lab}</button>
    {/each}
  </div>

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
  <p class="small muted">{p.export.kind === 'album' ? 'Album' : p.export.kind === 'mixtape' ? 'Mixtape' : 'Recording'} goes to <span class="mono selectable">{ui.view.export_dir}</span></p>
  {#if p.medium.preset.media_tag}<p class="small muted">Tagged MEDIA={p.medium.preset.media_tag}.</p>{/if}

  <div class="sep"></div>
  <h3>{p.export.kind === 'album' ? 'Record' : p.export.kind === 'mixtape' ? 'Mixtape' : 'Recording'}</h3>
  {#each fields as [k, lab]}
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
