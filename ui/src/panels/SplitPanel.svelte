<script lang="ts">
  // Step 2: split sides into tracks.
  import { api } from '../lib/api';
  import { ui, edit, run, selectedTrack } from '../lib/store.svelte';
  import { markBoundary, addTrackFromSelection, deleteSelectedTrack } from '../lib/actions';
  import Num from '../components/Num.svelte';
  import { time } from '../lib/format';

  const sp = $derived(ui.view?.project.split);
  const t = $derived(selectedTrack());
  const tv = $derived(ui.view?.tracks.find((x) => x.id === ui.selTrack));
  const hasSides = $derived((ui.view?.project.sources ?? []).some((s) => s.kind !== 'folder'));
</script>

<div class="col gap">
  <h2>Split</h2>
  {#if sp}
    <Num label="Silence threshold" value={sp.threshold_db} min={-80} max={-20} step={0.5} unit="dBFS"
      tip="Anything quieter than this between songs counts as a gap"
      onchange={(v) => edit('Split threshold', (p) => (p.split.threshold_db = v))} />
    <Num label="Shortest gap" value={sp.min_gap_s} min={0.3} max={6} step={0.1} unit="s"
      tip="A gap must be at least this long to split there; shorter dips stay inside a song"
      onchange={(v) => edit('Split gap', (p) => (p.split.min_gap_s = v))} />
    <div class="row wrap">
      <button title="Set the threshold 10 dB above the surface noise between songs" onclick={() => run(() => api.splitAutoThreshold())} disabled={!hasSides}>Auto threshold</button>
      <button class="primary" title="Replace the tracks on every side with a split on silence. Titles on those sides are cleared." disabled={!hasSides}
        onclick={() => run(() => api.splitAuto(null), 'Split on silence.')}>Auto-split all sides</button>
    </div>
    {#if t}
      {@const src = ui.view?.project.sources.find((s) => s.id === t.source_id)}
      {#if src && src.kind !== 'folder'}
        <button title="Re-split only the side this track is on" onclick={() => run(() => api.splitAuto(src.id), `Split ${src.name}.`)}>Auto-split {src.name} only</button>
      {/if}
    {/if}
  {/if}

  <div class="sep"></div>
  <h3>Boundaries</h3>
  <p class="muted small">Drag a boundary on the timeline to move it. While listening, press M where a song ends.</p>
  <div class="row wrap">
    <button title="Drop a boundary at the playhead (M)" onclick={markBoundary}>Mark track end<span class="key">M</span></button>
    <button title="Make a new track from the selection (T)" onclick={addTrackFromSelection} disabled={!ui.selection}>Track from selection<span class="key">T</span></button>
    <button title="Join the selected track with the next one on the same side (J)" disabled={!t} onclick={() => t && run(() => api.mergeNext(t.id))}>Merge with next<span class="key">J</span></button>
    <button class="danger" title="Remove the selected track (Delete)" disabled={!t} onclick={deleteSelectedTrack}>Delete<span class="key">Del</span></button>
  </div>
  <button title="Search MusicBrainz for the tracklist. This is the only thing in the app that uses the network. (Ctrl+L)" onclick={() => (ui.dialog = 'lookup')}>Look up tracklist<span class="key">Ctrl+L</span></button>

  {#if t && tv}
    <div class="sep"></div>
    <h3>Track {t.number ?? ''}</h3>
    <label class="col"><span>Title</span>
      <input type="text" value={t.title} placeholder="Asked for at export" onkeydown={(e) => e.stopPropagation()}
        onchange={(e) => { const v = (e.target as HTMLInputElement).value; edit('Title', (p) => { const x = p.tracks.find((x) => x.id === t.id); if (x) x.title = v; }); }} />
    </label>
    <p class="mono small muted">{time(tv.start_g, true)} to {time(tv.end_g, true)} ({time(tv.end_g - tv.start_g)})</p>
    <Num label="Fade in" value={t.fade_in_ms} min={0} max={2000} step={10} unit="ms" digits={0}
      tip="Fades the lead-in noise up at the start of the track instead of cutting it"
      onchange={(v) => edit('Fade in', (p) => { const x = p.tracks.find((x) => x.id === t.id); if (x) x.fade_in_ms = v; })} />
    <Num label="Fade out" value={t.fade_out_ms} min={0} max={5000} step={10} unit="ms" digits={0}
      tip="Fades the end of the track down to silence"
      onchange={(v) => edit('Fade out', (p) => { const x = p.tracks.find((x) => x.id === t.id); if (x) x.fade_out_ms = v; })} />
  {:else}
    <p class="muted small">Select a track to edit its title and fades.</p>
  {/if}
</div>

<style>
  .gap { gap: 10px; }
</style>
