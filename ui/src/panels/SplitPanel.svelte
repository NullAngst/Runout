<script lang="ts">
  // Step 2: split sides into tracks. Nothing splits until a button here is
  // pressed; the medium picks the strategy the buttons run.
  import { ask } from '@tauri-apps/plugin-dialog';
  import { api, type SplitMode } from '../lib/api';
  import { ui, edit, run, selectedTrack, toast } from '../lib/store.svelte';
  import { seekTo } from '../lib/actions';
  import { markBoundary, addTrackFromSelection, deleteSelectedTrack } from '../lib/actions';
  import Num from '../components/Num.svelte';
  import { time } from '../lib/format';

  const sp = $derived(ui.view?.project.split);
  const t = $derived(selectedTrack());
  const tv = $derived(ui.view?.tracks.find((x) => x.id === ui.selTrack));
  const hasSides = $derived((ui.view?.project.sources ?? []).some((s) => s.kind !== 'folder'));
  const album = $derived(ui.view?.project.export.kind === 'album');
  const changes = $derived((ui.view?.events ?? []).filter((e) => e.kind === 'programchange'));
  let searched = $state(false);

  const modes: [SplitMode, string, string][] = [
    ['absolute', 'Silence', 'Gaps quieter than a fixed level. Right for records.'],
    ['noise_floor', 'Hiss floor', 'Gaps a few dB above the measured hiss floor. Right for tape, where the floor moves from tape to tape.'],
    ['program_change', 'Program changes', '8-track: split where the head moves to the next program'],
    ['fixed_length', 'Fixed length', 'Chapters of a set length, for long recordings'],
  ];

  async function find() {
    await run(() => api.findProgramChanges(null));
    searched = true;
    const n = (ui.view?.events ?? []).filter((e) => e.kind === 'programchange').length;
    toast(n ? `${n} program change${n > 1 ? 's' : ''} found.` : 'No program changes found.');
  }

  async function join(c: { source_id: number; start_s: number; end_s: number; g: number }) {
    seekTo(c.g);
    const yes = await ask('A song runs through this program change. Join the two halves with a short crossfade?', { title: 'Runout', kind: 'info', okLabel: 'Join', cancelLabel: 'Leave split' });
    if (yes) run(() => api.joinProgramChange(c.source_id, c.start_s, c.end_s, 20), 'Joined across the program change.');
  }
</script>

<div class="col gap">
  <h2>Split</h2>
  {#if sp}
    <p class="muted small">Every side arrives as one track. These buttons split it; each run replaces that side's boundaries and can be undone.</p>
    <label class="row" title="How the Auto-split buttons find the gaps. Set by the medium."><span class="lab">Strategy</span>
      <select class="grow" value={sp.mode} onchange={(e) => edit('Split strategy', (p) => (p.split.mode = (e.target as HTMLSelectElement).value as SplitMode))}>
        {#each modes as [k, lab, tip]}<option value={k} title={tip}>{lab}</option>{/each}
      </select></label>

    {#if sp.mode === 'absolute'}
      <Num label="Silence threshold" value={sp.threshold_db} min={-80} max={-20} step={0.5} unit="dBFS"
        tip="Anything quieter than this between songs counts as a gap"
        onchange={(v) => edit('Split threshold', (p) => (p.split.threshold_db = v))} />
    {:else if sp.mode === 'noise_floor'}
      <Num label="Above the hiss" value={sp.floor_margin_db} min={1} max={20} step={0.5} unit="dB"
        tip="A gap is anything less than this far above the measured hiss floor"
        onchange={(v) => edit('Split margin', (p) => (p.split.floor_margin_db = v))} />
    {:else if sp.mode === 'fixed_length'}
      <Num label="Chapter length" value={sp.chapter_min} min={1} max={120} step={1} unit="min" digits={0}
        tip="Every chapter is this long, the last one whatever is left"
        onchange={(v) => edit('Chapter length', (p) => (p.split.chapter_min = v))} />
    {/if}
    {#if sp.mode === 'absolute' || sp.mode === 'noise_floor'}
      <Num label="Shortest gap" value={sp.min_gap_s} min={0.3} max={6} step={0.1} unit="s"
        tip="A gap must be at least this long to split there; shorter dips stay inside a song"
        onchange={(v) => edit('Split gap', (p) => (p.split.min_gap_s = v))} />
    {/if}

    {#if sp.mode === 'program_change'}
      <div class="row wrap">
        <button title="Look for the short mute and the thump of the head moving to the next program" onclick={find} disabled={!hasSides || !!ui.tick.busy}>Find program changes</button>
        <button class="primary" title="Replace the tracks on every side with one track per program" disabled={!hasSides}
          onclick={() => run(() => api.splitProgramChanges(), 'Split at the program changes.')}>Split at program changes</button>
      </div>
      {#if changes.length}
        <div class="col changes">
          {#each changes as c (c.g)}
            <div class="row small">
              <button class="ghost mono" onclick={() => seekTo(c.g)} title="Go there">{time(c.g)}</button>
              <span class="grow" class:attention={c.spans_song} class:muted={!c.spans_song}>{c.spans_song ? 'A song runs through it' : 'Between songs'}</span>
              {#if c.spans_song}<button title="Join the halves with a short crossfade. Split at program changes first." onclick={() => join(c)}>Join</button>{/if}
            </div>
          {/each}
        </div>
      {:else if searched}
        <p class="muted small">None found. Split by hand with M while listening.</p>
      {/if}
    {:else}
      <div class="row wrap">
        {#if sp.mode === 'absolute'}
          <button title="Set the threshold 10 dB above the surface noise between songs" onclick={() => run(() => api.splitAutoThreshold())} disabled={!hasSides}>Auto threshold</button>
        {/if}
        <button class="primary" title="Replace the tracks on every side with an automatic split. Titles on those sides are cleared." disabled={!hasSides}
          onclick={() => run(() => api.splitAuto(null), sp.mode === 'fixed_length' ? 'Split into chapters.' : 'Split on the gaps.')}>Auto-split all sides</button>
      </div>
      {#if t}
        {@const src = ui.view?.project.sources.find((s) => s.id === t.source_id)}
        {#if src && src.kind !== 'folder'}
          <button title="Re-split only the side this track is on" onclick={() => run(() => api.splitAuto(src.id), `Split ${src.name}.`)}>Auto-split {src.name} only</button>
        {/if}
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
  {#if album}
    <button title="Search MusicBrainz for the tracklist. This is the only thing in the app that uses the network. (Ctrl+L)" onclick={() => (ui.dialog = 'lookup')}>Look up tracklist<span class="key">Ctrl+L</span></button>
  {/if}

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
  .lab { width: 70px; color: var(--muted); }
  .changes { gap: 2px; max-height: 220px; overflow: auto; }
</style>
