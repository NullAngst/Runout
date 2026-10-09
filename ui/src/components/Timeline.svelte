<script lang="ts">
  // The timeline: waveform and spectrogram pictures rendered in Rust, with the
  // click map, track regions, repairs, selection and playhead drawn on top.
  import { api } from '../lib/api';
  import { ui, edit, fail, timelineEnd, fit, trackAt } from '../lib/store.svelte';
  import { time, hz, rowFreq } from '../lib/format';

  let host: HTMLDivElement;
  let W = $state(800);
  let waveH = $state(160);
  let specH = $state(160);
  let waveUrl = $state('');
  let specUrl = $state('');
  let rulerCanvas: HTMLCanvasElement;
  let clickCanvas: HTMLCanvasElement;
  let reqWave = 0;
  let reqSpec = 0;

  const span = $derived(Math.max(1e-3, ui.t1 - ui.t0));
  const x = (t: number) => ((t - ui.t0) / span) * W;
  const tAt = (px: number) => ui.t0 + (px / W) * span;
  const nyq = $derived((ui.view?.layout[0]?.rate ?? 48000) / 2);

  function layoutLanes() {
    if (!host) return;
    W = Math.max(100, host.clientWidth);
    const h = Math.max(120, host.clientHeight - 24 - 14 - 2);
    if (ui.mode === 'both') {
      waveH = Math.floor(h * 0.45);
      specH = h - waveH;
    } else if (ui.mode === 'wave') {
      waveH = h;
      specH = 0;
    } else {
      waveH = 0;
      specH = h;
    }
  }

  $effect(() => {
    const ro = new ResizeObserver(layoutLanes);
    ro.observe(host);
    return () => ro.disconnect();
  });

  $effect(() => {
    void ui.mode;
    layoutLanes();
  });

  function toUrl(buf: ArrayBuffer, old: string) {
    if (old) URL.revokeObjectURL(old);
    return URL.createObjectURL(new Blob([buf], { type: 'image/png' }));
  }

  // Pictures follow the visible range, size, vertical zoom and the project.
  $effect(() => {
    const [t0, t1, w, h, vz] = [ui.t0, ui.t1, W, waveH, ui.vzoom];
    const gen = ui.view?.layout.length + ':' + (ui.view?.analysed ?? false) + ':' + (ui.view?.stale ?? 0) + ':' + JSON.stringify(ui.view?.layout.map((l) => l.ratio));
    void gen;
    if (!ui.view || h < 10 || !ui.view.layout.length) return;
    const id = ++reqWave;
    const dpr = window.devicePixelRatio || 1;
    const timer = setTimeout(async () => {
      try {
        const buf = await api.tileWave(t0, t1, Math.round(w * dpr), Math.round(h * dpr), vz);
        if (id === reqWave) waveUrl = toUrl(buf, waveUrl);
      } catch (e) { fail(e); }
    }, 40);
    return () => clearTimeout(timer);
  });

  $effect(() => {
    const [t0, t1, w, h] = [ui.t0, ui.t1, W, specH];
    const gen = (ui.view?.layout.length ?? 0) + JSON.stringify(ui.view?.layout.map((l) => l.ratio));
    void gen;
    if (!ui.view || h < 10 || !ui.view.layout.length) return;
    const id = ++reqSpec;
    const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
    const timer = setTimeout(async () => {
      try {
        const buf = await api.tileSpec(t0, t1, Math.round(w * dpr), Math.round(h * dpr));
        if (id === reqSpec) specUrl = toUrl(buf, specUrl);
      } catch (e) { fail(e); }
    }, 80);
    return () => clearTimeout(timer);
  });

  // ruler
  $effect(() => {
    const c = rulerCanvas;
    if (!c) return;
    const [t0, t1, w] = [ui.t0, ui.t1, W];
    const dpr = window.devicePixelRatio || 1;
    c.width = w * dpr;
    c.height = 24 * dpr;
    const g = c.getContext('2d')!;
    g.scale(dpr, dpr);
    g.clearRect(0, 0, w, 24);
    const steps = [0.01, 0.05, 0.1, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
    const per = (t1 - t0) / (w / 90);
    const st = steps.find((s) => s >= per) ?? 600;
    g.fillStyle = '#8fa39d';
    g.strokeStyle = '#2a3d39';
    g.font = '10.5px ' + getComputedStyle(document.body).getPropertyValue('--mono');
    for (let t = Math.ceil(t0 / st) * st; t <= t1; t += st) {
      const px = ((t - t0) / (t1 - t0)) * w;
      g.beginPath();
      g.moveTo(px + 0.5, 14);
      g.lineTo(px + 0.5, 24);
      g.stroke();
      g.fillText(st < 1 ? time(t, true) : time(t), px + 3, 11);
    }
  });

  // click map
  $effect(() => {
    const c = clickCanvas;
    if (!c) return;
    const [t0, t1, w, clicks] = [ui.t0, ui.t1, W, ui.clicks];
    const dpr = window.devicePixelRatio || 1;
    c.width = w * dpr;
    c.height = 14 * dpr;
    const g = c.getContext('2d')!;
    g.scale(dpr, dpr);
    g.clearRect(0, 0, w, 14);
    for (const [t, sz, crackle] of clicks) {
      if (t < t0 || t > t1) continue;
      const px = Math.round(((t - t0) / (t1 - t0)) * w) + 0.5;
      // brighter = harder repair; copper only for repairs near full scale
      const k = Math.min(1, Math.max(0, (sz + 50) / 50));
      g.strokeStyle = sz > -3 ? 'rgba(212,136,74,0.95)' : `rgba(95,184,165,${0.2 + k * 0.8})`;
      g.beginPath();
      g.moveTo(px, crackle ? 9 : 2);
      g.lineTo(px, 14);
      g.stroke();
    }
  });

  // follow the playhead while playing
  $effect(() => {
    const p = ui.tick.pos;
    if (!ui.tick.playing) return;
    if (p > ui.t1 - span * 0.02 || p < ui.t0) {
      const s = span;
      ui.t0 = Math.max(0, p - s * 0.05);
      ui.t1 = ui.t0 + s;
    }
  });

  // ---------------------------------------------------------------- pointer

  type Drag =
    | { kind: 'select'; x0: number; y0: number; area: 'wave' | 'spec'; moved: boolean; top: number; h: number }
    | { kind: 'edge'; edges: { id: number; which: 'start' | 'end' }[]; sid: number }
    | { kind: 'scrub' };
  let drag: Drag | null = null;
  let lastScrub = 0;

  function edges() {
    const v = ui.view;
    if (!v) return [];
    const out: { g: number; id: number; which: 'start' | 'end'; sid: number }[] = [];
    for (const tv of v.tracks) {
      const t = v.project.tracks.find((x) => x.id === tv.id);
      if (!t) continue;
      out.push({ g: tv.start_g, id: t.id, which: 'start', sid: t.source_id });
      out.push({ g: tv.end_g, id: t.id, which: 'end', sid: t.source_id });
    }
    return out;
  }

  function localX(e: PointerEvent) {
    return e.clientX - host.getBoundingClientRect().left;
  }

  function down(e: PointerEvent, area: 'wave' | 'spec' | 'ruler') {
    if (e.button !== 0 || !ui.view) return;
    const px = localX(e);
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    if (area === 'ruler') {
      drag = { kind: 'scrub' };
      scrub(px);
      return;
    }
    const near = edges().filter((ed) => Math.abs(x(ed.g) - px) <= 5);
    if (near.length && ui.step === 1) {
      const sid = near[0].sid;
      drag = { kind: 'edge', sid, edges: near.filter((n) => n.sid === sid).map((n) => ({ id: n.id, which: n.which })) };
      return;
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    drag = { kind: 'select', x0: px, y0: e.clientY - r.top, area, moved: false, top: r.top, h: r.height };
  }

  function scrub(px: number) {
    const t = Math.max(0, Math.min(timelineEnd(), tAt(px)));
    ui.tick.pos = t;
    const now = performance.now();
    if (now - lastScrub > 60) {
      lastScrub = now;
      api.seek(t);
    }
  }

  function move(e: PointerEvent) {
    if (!drag || !ui.view) {
      // cursor feedback over track edges in the Split step
      if (ui.view && ui.step === 1 && host) {
        const px = localX(e);
        host.style.cursor = edges().some((ed) => Math.abs(x(ed.g) - px) <= 5) ? 'ew-resize' : '';
      }
      return;
    }
    const px = localX(e);
    if (drag.kind === 'scrub') {
      scrub(px);
    } else if (drag.kind === 'select') {
      if (Math.abs(px - drag.x0) > 3) drag.moved = true;
      if (!drag.moved) return;
      const a = tAt(Math.min(px, drag.x0));
      const b = tAt(Math.max(px, drag.x0));
      let band: [number, number] | null = null;
      if (drag.area === 'spec') {
        const y1 = Math.max(0, Math.min(drag.h, e.clientY - drag.top));
        const lo = rowFreq(Math.max(y1, drag.y0) / drag.h, nyq);
        const hi = rowFreq(Math.min(y1, drag.y0) / drag.h, nyq);
        band = [Math.round(lo), Math.round(hi)];
      }
      ui.selection = { a: Math.max(0, a), b: Math.min(timelineEnd(), b), band };
    } else if (drag.kind === 'edge') {
      const g = tAt(px);
      const d = drag;
      const place = ui.view.layout.find((l) => l.source_id === d.sid);
      if (!place) return;
      const srcS = Math.max(0, Math.min(place.duration, g - place.start)) * place.ratio;
      edit('Move boundary', (p) => {
        for (const ed of d.edges) {
          const t = p.tracks.find((t) => t.id === ed.id);
          if (!t) continue;
          if (ed.which === 'start') t.start_s = Math.min(srcS, t.end_s - 0.5);
          else t.end_s = Math.max(srcS, t.start_s + 0.5);
        }
      });
    }
  }

  function up(e: PointerEvent) {
    if (!drag) return;
    const d = drag;
    drag = null;
    if (d.kind === 'select' && !d.moved) {
      const t = tAt(localX(e));
      ui.selection = null;
      ui.tick.pos = t;
      api.seek(t);
      const tv = trackAt(t);
      if (tv) ui.selTrack = tv.id;
    } else if (d.kind === 'select' && ui.selection && ui.loop) {
      api.setLoop([ui.selection.a, ui.selection.b]);
    }
  }

  function wheel(e: WheelEvent) {
    e.preventDefault();
    const end = Math.max(timelineEnd(), 1);
    if (e.ctrlKey || e.metaKey) {
      const px = localX(e as unknown as PointerEvent);
      const at = tAt(px);
      const k = Math.exp(e.deltaY * 0.0015);
      const ns = Math.max(0.05, Math.min(end * 1.05, span * k));
      ui.t0 = Math.max(0, at - (px / W) * ns);
      ui.t1 = ui.t0 + ns;
    } else {
      const d = (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY) / W * span;
      const s = span;
      ui.t0 = Math.max(0, Math.min(end - s * 0.5, ui.t0 + d));
      ui.t1 = ui.t0 + s;
    }
  }

  export function zoom(k: number) {
    const c = ui.tick.pos >= ui.t0 && ui.tick.pos <= ui.t1 ? ui.tick.pos : (ui.t0 + ui.t1) / 2;
    const ns = Math.max(0.05, Math.min(Math.max(timelineEnd(), 1) * 1.05, span * k));
    ui.t0 = Math.max(0, c - ns / 2);
    ui.t1 = ui.t0 + ns;
  }

  const repairs = $derived.by(() => {
    const v = ui.view;
    if (!v) return [];
    const out: { id: number; a: number; b: number; band: [number, number] | null }[] = [];
    for (const s of v.project.sources) {
      const l = v.layout.find((l) => l.source_id === s.id);
      if (!l) continue;
      for (const r of s.fx.repairs) out.push({ id: r.id, a: l.start + r.start_s / l.ratio, b: l.start + r.end_s / l.ratio, band: r.band });
    }
    return out;
  });

  const yOfFreq = (f: number, h: number) => (1 - Math.log(f / 20) / Math.log(nyq / 20)) * h;
</script>

<div class="tl" bind:this={host} onwheel={wheel} onpointermove={move} onpointerup={up} role="application" aria-label="Timeline">
  <canvas class="ruler" bind:this={rulerCanvas} style="width:{W}px;height:24px" onpointerdown={(e) => down(e, 'ruler')} title="Drag here to scrub"></canvas>
  <canvas class="clicks" bind:this={clickCanvas} style="width:{W}px;height:14px" title="Click map: one tick per repaired click, brighter for harder repairs (copper: near full scale). Short ticks are crackle. Jump with [ and ]"></canvas>

  <div class="lanes">
    {#if waveH > 0}
      <div class="lane" style="height:{waveH}px" onpointerdown={(e) => down(e, 'wave')} role="presentation">
        {#if waveUrl}<img src={waveUrl} alt="" draggable="false" />{/if}
      </div>
    {/if}
    {#if specH > 0}
      <div class="lane spec" style="height:{specH}px" onpointerdown={(e) => down(e, 'spec')} role="presentation">
        {#if specUrl}<img src={specUrl} alt="" draggable="false" />{/if}
        {#each repairs.filter((r) => r.band) as r (r.id)}
          {#if r.b > ui.t0 && r.a < ui.t1}
            <div class="repair band" style="left:{x(r.a)}px;width:{Math.max(2, x(r.b) - x(r.a))}px;top:{yOfFreq(r.band![1], specH)}px;height:{Math.max(2, yOfFreq(r.band![0], specH) - yOfFreq(r.band![1], specH))}px"></div>
          {/if}
        {/each}
        {#if ui.selection?.band}
          <div class="sel band" style="left:{x(ui.selection.a)}px;width:{x(ui.selection.b) - x(ui.selection.a)}px;top:{yOfFreq(ui.selection.band[1], specH)}px;height:{yOfFreq(ui.selection.band[0], specH) - yOfFreq(ui.selection.band[1], specH)}px">
            <span class="mono small bandlab">{hz(ui.selection.band[0])} to {hz(ui.selection.band[1])}</span>
          </div>
        {/if}
      </div>
    {/if}

    <!-- overlays across both lanes -->
    <div class="over">
      {#if ui.view}
        {#each ui.view.layout as l (l.source_id)}
          {#if l.start + l.duration > ui.t0 && l.start < ui.t1}
            <div class="side" style="left:{x(l.start)}px;width:{x(l.start + l.duration) - x(l.start)}px">
              <span class="sidelab small">{ui.view.project.sources.find((s) => s.id === l.source_id)?.name}</span>
            </div>
          {/if}
        {/each}
        {#each ui.view.tracks as tv (tv.id)}
          {#if tv.end_g > ui.t0 && tv.start_g < ui.t1}
            {@const t = ui.view.project.tracks.find((t) => t.id === tv.id)}
            <div class="trk" class:sel={ui.selTrack === tv.id} style="left:{x(tv.start_g)}px;width:{x(tv.end_g) - x(tv.start_g)}px">
              <span class="trklab small">{t?.number ?? ''}{t?.title ? '. ' + t.title : ''}</span>
            </div>
            <div class="edge" class:hot={ui.step === 1} style="left:{x(tv.start_g)}px"></div>
            <div class="edge" class:hot={ui.step === 1} style="left:{x(tv.end_g)}px"></div>
          {/if}
        {/each}
        {#each repairs.filter((r) => !r.band) as r (r.id)}
          {#if r.b > ui.t0 && r.a < ui.t1}
            <div class="repair" style="left:{x(r.a)}px;width:{Math.max(2, x(r.b) - x(r.a))}px"></div>
          {/if}
        {/each}
        {#if ui.selection && !ui.selection.band}
          <div class="sel" class:loop={ui.loop} style="left:{x(ui.selection.a)}px;width:{x(ui.selection.b) - x(ui.selection.a)}px">
            <span class="mono small sellab">{time(ui.selection.b - ui.selection.a, true)}</span>
          </div>
        {/if}
        {#if ui.tick.pos >= ui.t0 && ui.tick.pos <= ui.t1}
          <div class="playhead" style="left:{x(ui.tick.pos)}px"></div>
        {/if}
      {/if}
    </div>
  </div>
  {#if ui.view && !ui.view.layout.length}
    <div class="empty muted">Record a side or import audio in the Capture/Import step.</div>
  {/if}
</div>

<style>
  .tl { position: relative; height: 100%; overflow: hidden; display: flex; flex-direction: column; }
  .ruler { display: block; cursor: col-resize; border-bottom: 1px solid var(--border); }
  .clicks { display: block; }
  .lanes { position: relative; flex: 1; }
  .lane { position: relative; overflow: hidden; border-top: 1px solid var(--border); cursor: crosshair; }
  .lane img { position: absolute; inset: 0; width: 100%; height: 100%; image-rendering: auto; pointer-events: none; }
  .over { position: absolute; inset: 0; pointer-events: none; }
  .side { position: absolute; top: 0; bottom: 0; border-left: 1px solid var(--border); border-right: 1px solid var(--border); }
  .sidelab { position: absolute; bottom: 2px; left: 4px; color: var(--muted); }
  .trk { position: absolute; top: 0; height: 18px; background: rgba(95, 184, 165, 0.07); border-bottom: 1px solid var(--verdigris-dim); overflow: hidden; }
  .trk.sel { background: rgba(95, 184, 165, 0.18); }
  .trklab { padding: 1px 5px; color: var(--text); white-space: nowrap; }
  .edge { position: absolute; top: 0; bottom: 0; width: 1px; background: rgba(95, 184, 165, 0.35); }
  .edge.hot { background: var(--verdigris); width: 2px; margin-left: -1px; }
  .playhead { position: absolute; top: 0; bottom: 0; width: 1px; background: var(--verdigris); box-shadow: 0 0 0 0.5px rgba(95,184,165,.4); }
  .sel { position: absolute; top: 0; bottom: 0; background: rgba(230, 236, 233, 0.08); border-left: 1px solid var(--muted); border-right: 1px solid var(--muted); }
  .sel.loop { background: rgba(95, 184, 165, 0.12); border-color: var(--verdigris); }
  .sel.band { border: 1px solid var(--text); bottom: auto; }
  .sellab, .bandlab { position: absolute; top: 20px; left: 4px; color: var(--text); white-space: nowrap; }
  .bandlab { top: 2px; }
  .repair { position: absolute; top: 18px; bottom: 0; background: rgba(95, 184, 165, 0.14); border: 1px dashed var(--verdigris-dim); }
  .repair.band { bottom: auto; }
  .spec .repair { pointer-events: none; }
  .empty { position: absolute; inset: 60px 0 0 0; display: grid; place-items: center; pointer-events: none; }
</style>
