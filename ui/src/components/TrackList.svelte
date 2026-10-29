<script lang="ts">
  // Left column: every track, grouped by side, with length, loudness and status.
  import { api } from '../lib/api';
  import { ui, edit, run } from '../lib/store.svelte';
  import { time } from '../lib/format';

  let renaming = $state<number | null>(null);
  let text = $state('');

  const groups = $derived.by(() => {
    const v = ui.view;
    if (!v) return [];
    return v.project.sources.map((s) => ({
      source: s,
      tracks: v.project.tracks
        .filter((t) => t.source_id === s.id)
        .sort((a, b) => a.start_s - b.start_s)
        .map((t) => ({ t, tv: v.tracks.find((x) => x.id === t.id)! })),
    }));
  });

  function status(t: (typeof groups)[number]['tracks'][number]) {
    const a = t.t.analysis;
    if (t.tv?.passthrough) return { cls: 'ok', tip: 'Cleaned import: passes through untouched' };
    if (!a) return { cls: '', tip: 'Not analysed yet' };
    if (!t.tv.fresh) return { cls: 'half', tip: 'Settings changed since the last analysis' };
    if (t.tv.short_db > 0.05) return { cls: 'warn', tip: `Needs attention: ${t.tv.short_db.toFixed(1)} dB short of the target because the limiter cap was reached` };
    return { cls: 'ok', tip: 'Cleaned and leveled' };
  }

  function loud(t: (typeof groups)[number]['tracks'][number]): string {
    const a = t.t.analysis;
    if (!a) return '';
    const chain = t.t.chain ?? ui.view!.project.chain;
    const v = chain.level.enabled ? a.before.integrated + t.tv.gain_db : a.before.integrated;
    return v <= -70 ? '' : v.toFixed(1);
  }

  function pick(id: number, start: number) {
    ui.selTrack = id;
    ui.tick.pos = start;
    api.seek(start);
    const span = ui.t1 - ui.t0;
    if (start < ui.t0 || start > ui.t1) {
      ui.t0 = Math.max(0, start - span * 0.1);
      ui.t1 = ui.t0 + span;
    }
  }

  function commitName(id: number) {
    const v = text.trim();
    renaming = null;
    edit('Rename', (p) => {
      const t = p.tracks.find((t) => t.id === id);
      if (t) t.title = v;
    });
  }
</script>

<div class="list scroll">
  {#each groups as g (g.source.id)}
    <div class="side row">
      <h3>{g.source.name}</h3>
      <span class="muted small">{g.source.kind === 'folder' ? 'imported file' : g.source.kind}</span>
    </div>
    {#each g.tracks as row (row.t.id)}
      {@const st = status(row)}
      <div class="trk row" class:sel={ui.selTrack === row.t.id} onclick={() => pick(row.t.id, row.tv.start_g)}
        ondblclick={() => { renaming = row.t.id; text = row.t.title; }} role="button" tabindex="-1" onkeydown={() => {}}>
        <span class="dot {st.cls}" title={st.tip}></span>
        <span class="n mono">{row.t.number ?? ''}</span>
        {#if renaming === row.t.id}
          <!-- svelte-ignore a11y_autofocus -->
          <input type="text" class="name-in" bind:value={text} autofocus onblur={() => commitName(row.t.id)}
            onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') commitName(row.t.id); if (e.key === 'Escape') renaming = null; }} onclick={(e) => e.stopPropagation()} />
        {:else}
          <span class="title grow" title="Double-click to rename">{row.t.title || 'Untitled'}</span>
        {/if}
        <span class="len mono small muted" title="Length">{time(row.tv.end_g - row.tv.start_g)}</span>
        <span class="lufs mono small" title="Loudness after leveling, LUFS">{loud(row)}</span>
      </div>
      {#if g.source.kind === 'folder'}
        <div class="sub row">
          <button class="ghost small" class:on={!row.t.cleaned} title="Raw: copy the original into Raw/ and start the full restoration chain"
            onclick={(e) => { e.stopPropagation(); run(() => api.setCleaned(row.t.id, false)); }}>Raw</button>
          <button class="ghost small" class:on={row.t.cleaned} title="Cleaned: no raw copy, no restoration, no leveling. Exports bit for bit with new tags"
            onclick={(e) => { e.stopPropagation(); run(() => api.setCleaned(row.t.id, true)); }}>Cleaned</button>
          {#if row.t.chain && !row.t.cleaned}<span class="tag accent">own settings</span>{/if}
        </div>
      {:else if row.t.chain}
        <div class="sub row"><span class="tag accent">own settings</span></div>
      {/if}
    {/each}
  {/each}
  {#if !groups.length}
    <p class="muted small pad">No tracks yet.</p>
  {/if}
</div>

<style>
  .list { height: 100%; padding: 6px 4px 12px; }
  .side { padding: 10px 8px 4px; gap: 8px; align-items: baseline; }
  .side h3 { font-size: 13px; }
  .trk { padding: 4px 8px; border-radius: 5px; cursor: pointer; gap: 7px; }
  .trk:hover { background: var(--raised); }
  .trk.sel { background: var(--raised); box-shadow: inset 2px 0 0 var(--accent); }
  .n { width: 18px; text-align: right; color: var(--muted); }
  .title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .len { width: 34px; text-align: right; }
  .lufs { width: 36px; text-align: right; }
  .name-in { flex: 1; padding: 1px 4px; min-width: 0; }
  .sub { padding: 0 8px 4px 41px; gap: 2px; }
  .sub button { font-size: 11px; padding: 0 6px; }
  .pad { padding: 8px; }
</style>
