<script lang="ts">
  // Title prompt: every track gets a title before export starts.
  import { api, type Project } from '../lib/api';
  import { ui, clone, fail, setView, flushNow } from '../lib/store.svelte';

  let p = $state<Project>(clone(ui.view!.project));
  const order = $derived(
    [...p.tracks].sort((a, b) => {
      const sa = p.sources.findIndex((s) => s.id === a.source_id);
      const sb = p.sources.findIndex((s) => s.id === b.source_id);
      return sa - sb || a.start_s - b.start_s;
    }),
  );
  const blank = $derived(p.tracks.filter((t) => !t.title.trim()).length);

  async function go() {
    await flushNow();
    try {
      setView(await api.projectSet(p, 'Export details'));
      ui.dialog = null;
      await api.exportRun();
    } catch (e) {
      fail(e);
    }
  }
</script>

<div class="modal-back" role="presentation" onkeydown={(e) => e.stopPropagation()}>
  <div class="modal wide">
    <header><h2 class="grow">Export</h2><span class="muted small">{p.tracks.length} tracks, {p.export.bit_depth}-bit FLAC</span></header>
    <div class="body col">
      <div class="albumgrid">
        {#each [['artist', 'Artist'], ['album', 'Album'], ['album_artist', 'Album artist'], ['year', 'Year'], ['genre', 'Genre']] as [k, lab]}
          <label class="col"><span>{lab}</span><input type="text" bind:value={(p.album as any)[k]} /></label>
        {/each}
      </div>
      <table class="grid">
        <thead><tr><th style="width:52px">No.</th><th>Title</th><th>File name</th></tr></thead>
        <tbody>
          {#each order as t (t.id)}
            <tr>
              <td><input type="number" class="numin" value={t.number ?? ''} min="1"
                oninput={(e) => { const v = parseInt((e.target as HTMLInputElement).value); t.number = isFinite(v) ? v : null; }} /></td>
              <td><input type="text" class="w" class:missing={!t.title.trim()} bind:value={t.title} placeholder="Title" /></td>
              <td>
                {#if t.file_name != null}
                  <input type="text" class="w mono" bind:value={t.file_name} title="Imported files keep their name unless you change it" />
                {:else if t.number == null}
                  <input type="text" class="w mono" placeholder={t.title || 'file name'} oninput={(e) => (t.file_name = (e.target as HTMLInputElement).value || null)} />
                {:else}
                  <span class="mono small muted">{String(t.number).padStart(2, '0')}. {t.title || '...'}.flac</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <footer>
      {#if blank}<span class="copper small grow">{blank} track{blank > 1 ? 's' : ''} still need a title.</span>{/if}
      <button onclick={() => { ui.dialogData = { fromExport: true }; ui.dialog = 'lookup'; }}>Look up tracklist</button>
      <button onclick={() => (ui.dialog = null)}>Cancel</button>
      <button class="primary" disabled={blank > 0} onclick={go}>Export</button>
    </footer>
  </div>
</div>

<style>
  .wide { width: min(900px, 94vw); }
  .albumgrid { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 8px; margin-bottom: 8px; }
  .numin { width: 52px; }
  .w { width: 100%; }
  .missing { border-color: var(--copper); }
  td { vertical-align: middle; }
</style>
