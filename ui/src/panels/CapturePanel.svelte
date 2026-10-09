<script lang="ts">
  // Step 1: record from the turntable, import files, or import a folder.
  import { importFiles, importFolder } from '../lib/actions';
  import { api, type InputDevice } from '../lib/api';
  import { ui, run, fail, toast, setView, edit, fit } from '../lib/store.svelte';
  import { time } from '../lib/format';

  let devices = $state<InputDevice[]>([]);
  let device = $state<string | null>(null);
  let autoStop = $state(true);
  let holdS = $state(10);
  let stopping = false;

  const sides = $derived((ui.view?.project.sources ?? []).filter((s) => s.kind !== 'folder'));
  let name = $state('');
  $effect(() => {
    if (!ui.tick.capture.active) name = `Side ${String.fromCharCode(65 + Math.min(25, sides.length))}`;
  });

  async function loadDevices() {
    try {
      const d = await api.devices();
      devices = d.inputs;
      device = d.selected;
    } catch (e) { fail(e); }
  }
  $effect(() => { loadDevices(); });

  const cap = $derived(ui.tick.capture);
  const lvl = $derived(cap.level);
  const pct = (v: number) => Math.max(0, Math.min(100, ((v + 60) / 60) * 100));

  async function stop() {
    if (stopping) return;
    stopping = true;
    try {
      const r = await api.captureStop();
      if (r.level_check) {
        const head = -r.peak_db;
        toast(r.clipped || head < 0.5
          ? `Level check: the loudest part hit ${r.peak_db.toFixed(1)} dBFS. Turn the input down before recording.`
          : `Level check: peak ${r.peak_db.toFixed(1)} dBFS, so ${head.toFixed(1)} dB of headroom.`, r.clipped || head < 0.5);
      } else if (r.view) {
        setView(r.view);
        fit();
        toast(`${name || 'Side'} added: ${time(r.seconds)}${r.auto_stopped ? ', stopped at the run-out groove' : ''}.`);
        if (r.clipped) toast('This side clipped during capture. Declip is on, but a clipped capture cannot be fully fixed. Consider recording it again lower.', true);
      }
    } catch (e) { fail(e); }
    stopping = false;
  }

  // a level check ends by itself after 20 s; a capture ends at the run-out
  $effect(() => {
    if (cap.active && cap.finished) stop();
  });

  async function record() {
    try {
      if (device) await api.selectDevice(device);
      await api.captureStart(name.trim() || 'Side', device, autoStop ? holdS : null);
    } catch (e) { fail(e); }
  }

  async function check() {
    try {
      if (device) await api.selectDevice(device);
      await api.levelCheck(device);
      toast('Play the loudest part of the record for 20 seconds.');
    } catch (e) { fail(e); }
  }

  // keyboard shortcuts arrive through the store
  $effect(() => {
    const c = ui.captureCmd;
    if (!c) return;
    ui.captureCmd = null;
    if (c === 'record' && !cap.active) record();
    else if (c === 'check' && !cap.active) check();
    else if (c === 'stop' && cap.active) stop();
  });
</script>

<div class="col gap">
  <h2>Capture</h2>
  <label class="col">
    <span>Input device</span>
    <div class="row">
      <select class="grow" bind:value={device} disabled={cap.active} title="Where to record from. Your USB turntable shows up as a USB Audio device.">
        {#each devices as d (d.id)}<option value={d.id}>{d.name}{d.default ? ' (default)' : ''}</option>{/each}
      </select>
      <button class="ghost" title="Look for devices again" onclick={loadDevices} disabled={cap.active}>Refresh</button>
    </div>
  </label>
  <p class="muted small">Captures at 48 kHz, 16-bit when the device offers it. No RIAA curve is applied: the turntable's preamp already did that.</p>

  <div class="card col">
    <div class="row">
      <span class="muted small grow">{cap.active ? (cap.level_check ? 'Level check' : 'Recording') : 'Input level'}</span>
      <span class="clip" class:lit={lvl?.clipped} title="Lights if the input hit 0 dBFS. A clipped capture cannot be fully fixed later.">CLIP</span>
    </div>
    {#each [[lvl?.l ?? -120, lvl?.hold_l ?? -120], [lvl?.r ?? -120, lvl?.hold_r ?? -120]] as [v, h]}
      <div class="meter"><div class="fill" class:hot={v > -1} style="width:{pct(v)}%"></div>{#if h > -60}<div class="hold" style="left:{pct(h)}%"></div>{/if}</div>
    {/each}
    <div class="row small mono muted">
      <span>peak hold {(Math.max(lvl?.hold_l ?? -120, lvl?.hold_r ?? -120)).toFixed(1)} dBFS</span>
      <span class="grow"></span>
      {#if cap.active}<span class="verdigris">{time(lvl?.seconds ?? 0)}</span>{/if}
    </div>
    {#if lvl?.clipped}<p class="copper small">The input hit 0 dBFS. Turn the input level down and record this side again.</p>{/if}
  </div>

  <button disabled={cap.active} onclick={check} title="Play the loudest part of the record for 20 s; the app reports the headroom before you record">Capture level check<span class="key">C</span></button>

  <label class="col"><span>Side name</span><input type="text" bind:value={name} disabled={cap.active} onkeydown={(e) => e.stopPropagation()} /></label>
  <label class="row" title="Stop by itself once the run-out groove has played for this long, so you can walk away during a side">
    <input type="checkbox" bind:checked={autoStop} disabled={cap.active} />
    <span>Stop at the run-out groove after</span>
    <input type="number" min="3" max="60" bind:value={holdS} disabled={cap.active || !autoStop} onkeydown={(e) => e.stopPropagation()} />
    <span>s</span>
  </label>
  {#if cap.active && !cap.level_check}
    <button class="danger" onclick={stop}>Stop recording<span class="key">Shift+Space</span></button>
  {:else if cap.active}
    <button onclick={stop}>Stop level check</button>
  {:else}
    <button class="primary" disabled={!ui.view?.dir} onclick={record} title="Record this side into the project folder">Record {name}<span class="key">Ctrl+R</span></button>
  {/if}
  <p class="muted small">Record by side: Side A, flip, Side B. The raw WAV is written as it records and is never changed.</p>

  <div class="sep"></div>
  <h2>Import</h2>
  <div class="row wrap">
    <button onclick={importFiles} disabled={cap.active} title="WAV, FLAC, AIFF, OGG or MP3. One file per side works best.">Import files<span class="key">Ctrl+I</span></button>
    <button onclick={importFolder} disabled={cap.active} title="An old digitization already split into tracks: every file becomes a region, in order">Import folder<span class="key">Ctrl+Shift+I</span></button>
  </div>

  {#if ui.view?.project.sources.length}
    <div class="sep"></div>
    <h3>In this project</h3>
    {#each ui.view.project.sources as s (s.id)}
      <div class="col src">
        <div class="row">
          <input type="text" class="grow" value={s.name} title="Name used for the raw file and the timeline"
            onchange={(e) => { const v = (e.target as HTMLInputElement).value; edit('Rename side', (p) => { const x = p.sources.find((x) => x.id === s.id); if (x) x.name = v; }); }}
            onkeydown={(e) => e.stopPropagation()} />
          <button class="ghost" title="Remove this side and its tracks from the project (the file stays on disk)" onclick={() => run(() => api.removeSource(s.id))}>Remove</button>
        </div>
        <span class="mono small muted">{time(s.frames / s.rate)}, {s.rate} Hz, {s.bits}-bit, {s.kind === 'folder' ? (s.raw ? 'raw' : 'cleaned') : s.kind}</span>
      </div>
    {/each}
  {/if}
</div>

<style>
  .gap { gap: 10px; }
  .meter { position: relative; height: 8px; background: var(--surface); border-radius: 4px; overflow: hidden; }
  .fill { position: absolute; left: 0; top: 0; bottom: 0; background: var(--verdigris); }
  .fill.hot { background: var(--copper); }
  .hold { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--text); }
  .clip { font-size: 10px; font-weight: 700; padding: 1px 5px; border-radius: 3px; color: var(--muted); border: 1px solid var(--border); }
  .clip.lit { background: var(--copper); color: #1a0e05; border-color: var(--copper); }
  .src { gap: 2px; }
  .src input { padding: 3px 6px; min-width: 0; }
  label input[type=number] { width: 56px; }
</style>
