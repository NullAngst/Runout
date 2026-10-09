<script lang="ts">
  // Transport, A/B switch, Listen to removed, and meters.
  import { api } from '../lib/api';
  import { ui, timelineEnd } from '../lib/store.svelte';
  import { time, db } from '../lib/format';

  let { onPlay }: { onPlay: () => void } = $props();

  function setMon(m: number) {
    ui.monitor = m;
    api.setMonitor(m);
  }

  const bar = (v: number) => Math.max(0, Math.min(100, ((v + 60) / 60) * 100));
  const m = $derived(ui.tick.meters);
  let holdL = $state(-120);
  let holdR = $state(-120);
  let holdAt = 0;
  $effect(() => {
    const [l, r] = ui.tick.peak;
    const now = performance.now();
    if (l > holdL || r > holdR || now - holdAt > 1500) {
      holdL = Math.max(l, now - holdAt > 1500 ? -120 : holdL);
      holdR = Math.max(r, now - holdAt > 1500 ? -120 : holdR);
      holdAt = now;
    }
  });
</script>

<div class="bar row">
  <div class="panel row box">
    <button class="play" class:on={ui.tick.playing} title="Play or pause (Space)" onclick={onPlay}>{ui.tick.playing ? 'Pause' : 'Play'}<span class="key">Space</span></button>
    <span class="mono clock" title="Playhead / total length">{time(ui.tick.pos, true)} <span class="muted">/ {time(timelineEnd())}</span></span>
    <button class:on={ui.loop} title="Loop the selection (L)" onclick={() => { ui.loop = !ui.loop; api.setLoop(ui.loop && ui.selection ? [ui.selection.a, ui.selection.b] : null); }}>Loop<span class="key">L</span></button>
  </div>

  <div class="panel row box">
    <span class="muted small">A/B</span>
    <button class:on={ui.monitor === 1} title="A: the original at the same playhead position (A)" onclick={() => setMon(ui.monitor === 1 ? 0 : 1)}>Original</button>
    <button class:on={ui.monitor === 0} title="B: the processed result (A toggles)" onclick={() => setMon(0)}>Processed</button>
  </div>

  <div class="panel row box">
    <button class:on={ui.monitor === 2} title="Play only what the chain took out. If you hear music in it, a setting is too aggressive (R)" onclick={() => setMon(ui.monitor === 2 ? 0 : 2)}>Listen to removed<span class="key">R</span></button>
  </div>

  <div class="panel row box meters grow" title="Meters on what you hear. Before = original, after = processed.">
    <div class="pk col">
      {#each [[ui.tick.peak[0], holdL], [ui.tick.peak[1], holdR]] as [v, h], i}
        <div class="meter" title={i === 0 ? 'Left peak' : 'Right peak'}>
          <div class="fill" class:hot={v > -1} style="width:{bar(v)}%"></div>
          {#if h > -60}<div class="hold" style="left:{bar(h)}%"></div>{/if}
        </div>
      {/each}
    </div>
    <div class="vals mono small">
      <span title="True peak after processing">TP {db(m.tp, 1, 'dBTP')}</span>
      <span title="Momentary loudness (400 ms)">M {db(m.m, 1, 'LUFS')}</span>
      <span title="Short-term loudness (3 s)">S {db(m.s, 1, 'LUFS')}</span>
      <span title="Integrated loudness since you pressed play">I {db(m.i, 1, 'LUFS')}</span>
      <span title="Loudness range since you pressed play">LRA {m.lra.toFixed(1)} LU</span>
      <span class="muted" title="The same, measured on the original">before: I {db(m.pre_i, 1, 'LUFS')}, TP {db(m.pre_tp, 1, 'dBTP')}, LRA {m.pre_lra.toFixed(1)} LU</span>
    </div>
  </div>
</div>
{#if ui.tick.warn}
  <div class="warn copper small">{ui.tick.warn}</div>
{/if}

<style>
  .bar { gap: 8px; padding: 6px 8px; }
  .box { padding: 6px 8px; gap: 6px; }
  .play { min-width: 92px; }
  .clock { min-width: 120px; }
  .meters { gap: 12px; }
  .pk { gap: 3px; width: 160px; }
  .meter { position: relative; height: 6px; background: var(--bg); border-radius: 3px; overflow: hidden; }
  .fill { position: absolute; left: 0; top: 0; bottom: 0; background: var(--verdigris); transition: width 50ms linear; }
  .fill.hot { background: var(--copper); }
  .hold { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--text); }
  .vals { display: flex; gap: 12px; flex-wrap: wrap; }
  .warn { position: absolute; bottom: 62px; left: 50%; transform: translateX(-50%); background: var(--raised); border: 1px solid var(--copper-dim); padding: 6px 12px; border-radius: 6px; }
</style>
