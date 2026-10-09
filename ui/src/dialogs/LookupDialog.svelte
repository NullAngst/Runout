<script lang="ts">
  // Tracklist lookup on MusicBrainz. Network is used only when Search is pressed.
  import { api, type Release, type ReleaseHit } from '../lib/api';
  import { ui, run, fail } from '../lib/store.svelte';
  import { time } from '../lib/format';

  const album = ui.view!.project.album;
  let artist = $state(album.album_artist || album.artist);
  let title = $state(album.album);
  let hits = $state<ReleaseHit[]>([]);
  let release = $state<Release | null>(null);
  let busy = $state(false);
  let snap = $state(true);
  const back = ui.dialogData?.fromExport ? 'export' : null;

  async function search() {
    busy = true;
    release = null;
    try { hits = await api.lookupSearch(artist, title); } catch (e) { fail(e); }
    busy = false;
  }
  async function pick(id: string) {
    busy = true;
    try { release = await api.lookupRelease(id); } catch (e) { fail(e); }
    busy = false;
  }
  async function apply() {
    if (!release) return;
    const r = release;
    await run(() => api.applyTracklist(r.tracks.map((t) => ({ title: t.title, length_s: t.length_s, side: t.side })), snap, r.artist, r.title, r.date, r.genre), 'Tracklist applied.');
    ui.dialogData = null;
    ui.dialog = back as any;
  }
</script>

<div class="modal-back" role="presentation" onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter' && !release) search(); }}>
  <div class="modal">
    <header><h2 class="grow">Look up tracklist</h2><span class="muted small">MusicBrainz</span></header>
    <div class="body col">
      <div class="row">
        <input type="text" class="grow" placeholder="Artist" bind:value={artist} />
        <input type="text" class="grow" placeholder="Album" bind:value={title} />
        <button class="primary" disabled={busy} onclick={search}>Search</button>
      </div>
      {#if release}
        <div class="row"><h3 class="grow">{release.artist} - {release.title} {release.date ? `(${release.date})` : ''}</h3><button class="ghost" onclick={() => (release = null)}>Back to results</button></div>
        <table class="grid small">
          <thead><tr><th>No.</th><th>Title</th><th class="num">Length</th></tr></thead>
          <tbody>{#each release.tracks as t}<tr><td class="mono">{t.number}</td><td>{t.title}</td><td class="num mono">{t.length_s ? time(t.length_s) : ''}</td></tr>{/each}</tbody>
        </table>
        <label class="row" title="Move each boundary to the quietest point near where the track lengths put it">
          <input type="checkbox" bind:checked={snap} /> Use the track lengths to snap split boundaries</label>
        <p class="muted small">Titles go to the tracks in order. Side letters (A1, B1) are matched to your sides when the counts line up.</p>
      {:else}
        {#each hits as h (h.id)}
          <button class="hit" onclick={() => pick(h.id)}>
            <span class="grow">{h.artist} - {h.title}</span>
            <span class="muted small">{h.date} {h.country} {h.format} {h.tracks ? `${h.tracks} tracks` : ''}</span>
          </button>
        {/each}
        {#if !hits.length && !busy}<p class="muted small">Search by artist and album. Vinyl releases are listed first.</p>{/if}
      {/if}
      {#if busy}<p class="muted small">Asking MusicBrainz...</p>{/if}
    </div>
    <footer>
      <button onclick={() => { ui.dialogData = null; ui.dialog = back as any; }}>Cancel</button>
      <button class="primary" disabled={!release} onclick={apply}>Use this tracklist</button>
    </footer>
  </div>
</div>

<style>
  .hit { display: flex; gap: 10px; text-align: left; width: 100%; background: var(--bg); }
</style>
