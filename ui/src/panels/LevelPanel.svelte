<script lang="ts">
  // Step 4: loudness leveling with a hard true-peak ceiling.
  import { ui, edit, chainIn, currentChain } from '../lib/store.svelte';
  import { analyze } from '../lib/actions';
  import Num from '../components/Num.svelte';
  import type { Chain, Project } from '../lib/api';

  const c = $derived(currentChain());
  function ch(label: string, f: (c: Chain, p: Project) => void) {
    edit(label, (p) => f(chainIn(p), p));
  }

  const rows = $derived.by(() => {
    const v = ui.view;
    if (!v) return [];
    return [...v.tracks]
      .sort((a, b) => a.start_g - b.start_g)
      .map((tv) => ({ tv, t: v.project.tracks.find((t) => t.id === tv.id)! }));
  });
  const fmt = (v: number | undefined | null, d = 1) => (v == null || v <= -69.9 ? '--' : v.toFixed(d));
</script>

{#if c && ui.view}
<div class="col gap">
  <div class="row">
    <h2 class="grow">Level</h2>
    <button class="switch" class:on={c.level.enabled} aria-label="toggle" title={c.level.enabled ? 'Turn leveling off' : 'Turn leveling on'}
      onclick={() => ch('Leveling', (c) => (c.level.enabled = !c.level.enabled))}></button>
  </div>
  <p class="muted small">Each track is measured in LUFS, gained to the target, and run through a true-peak limiter. The output never goes over the ceiling.</p>

  <div class="row">
    <button class:on={c.level.mode === 'track'} title="Every track hits the same target" onclick={() => ch('Level mode', (c) => (c.level.mode = 'track'))}>Track mode</button>
    <button class:on={c.level.mode === 'album'} title="The album moves as one, so a quiet ballad stays quieter than the song next to it" onclick={() => ch('Level mode', (c) => (c.level.mode = 'album'))}>Album mode</button>
  </div>
  <Num label="Target" value={c.level.target_lufs} min={-24} max={-8} step={0.5} unit="LUFS" disabled={!c.level.enabled}
    tip="How loud each track should sound. -14 LUFS is about where streaming services sit."
    onchange={(v) => ch('Target', (c) => (c.level.target_lufs = v))} />
  <Num label="Ceiling" value={c.level.ceiling_dbtp} min={-3} max={0} step={0.1} unit="dBTP" disabled={!c.level.enabled}
    tip="No sample, and no peak between samples, goes above this"
    onchange={(v) => ch('Ceiling', (c) => (c.level.ceiling_dbtp = v))} />
  <Num label="Limiter cap" value={c.level.max_gr_db} min={0} max={12} step={0.5} unit="dB" disabled={!c.level.enabled}
    tip="The most the limiter may squash peaks. A track that needs more stops short of the target instead, and gets flagged."
    onchange={(v) => ch('Limiter cap', (c) => (c.level.max_gr_db = v))} />

  <button class="primary" onclick={() => analyze(true)} disabled={!ui.view.project.tracks.length || !!ui.tick.busy} title="Measure every track (Ctrl+Enter)">
    Analyze{ui.view.stale ? ` (${ui.view.stale} stale)` : ''}<span class="key">Ctrl+Enter</span>
  </button>
  {#if ui.view.album_lufs != null && c.level.mode === 'album'}
    <p class="small muted">Album loudness {ui.view.album_lufs.toFixed(1)} LUFS (duration weighted).</p>
  {/if}

  <table class="grid small">
    <thead><tr><th>#</th><th>Title</th><th class="num" title="Integrated loudness before leveling">Before</th><th class="num" title="Gain applied before the limiter">Gain</th><th class="num" title="Loudness after leveling">After</th><th class="num" title="True peak after leveling">TP</th></tr></thead>
    <tbody>
      {#each rows as r (r.tv.id)}
        {@const a = r.t.analysis}
        {@const lv = (r.t.chain ?? ui.view.project.chain).level}
        {@const on = lv.enabled && !r.tv.passthrough}
        <tr class:stale={a && !r.tv.fresh} onclick={() => (ui.selTrack = r.t.id)}>
          <td class="mono">{r.t.number ?? ''}</td>
          <td class="ttl">{r.t.title || 'Untitled'}</td>
          <td class="num mono">{fmt(a?.before.integrated)}</td>
          <td class="num mono">{a ? (r.tv.passthrough ? 'off' : (r.tv.gain_db >= 0 ? '+' : '') + r.tv.gain_db.toFixed(1)) : '--'}</td>
          <td class="num mono" class:copper={r.tv.short_db > 0.05}>{a ? fmt(on ? a.before.integrated + r.tv.gain_db : a.before.integrated) : '--'}</td>
          <td class="num mono">{a ? fmt(on ? Math.min(lv.ceiling_dbtp, a.before.true_peak + r.tv.gain_db) : a.before.true_peak, 2) : '--'}</td>
        </tr>
        {#if a && r.tv.short_db > 0.05}
          <tr><td></td><td colspan="5" class="copper">Stops {r.tv.short_db.toFixed(1)} dB short: reaching the target needs more than {(r.t.chain ?? ui.view.project.chain).level.max_gr_db} dB of limiting.</td></tr>
        {/if}
      {/each}
    </tbody>
  </table>
  <p class="muted small">Quiet vinyl is usually quiet on purpose: cutting engineers keep levels down so the groove fits and the stylus can track it. Pushing much past -14 LUFS on dynamic records means real squashing, and the table flags it.</p>
</div>
{/if}

<style>
  .gap { gap: 9px; }
  tr.stale td { opacity: 0.55; }
  tbody tr { cursor: pointer; }
  .ttl { max-width: 120px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
