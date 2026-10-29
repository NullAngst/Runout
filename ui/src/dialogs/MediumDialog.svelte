<script lang="ts">
  // The medium picker with the full parameter panel. Picking a medium and
  // pressing Apply re-resolves the presets; captures, tracks and manual
  // repairs stay. The parameter panel edits this project's copy of the
  // medium, and Save as preset keeps it for next time.
  import { api, type MediumPreset, type Project, type SpeedUnit, type CurveSel } from '../lib/api';
  import { ui, edit, run, fail, toast } from '../lib/store.svelte';
  import MediumPicker from '../components/MediumPicker.svelte';
  import Num from '../components/Num.svelte';
  import { nice } from '../lib/format';

  const p = $derived(ui.view!.project);
  const m = $derived(p.medium.preset);
  // svelte-ignore state_referenced_locally
  let medium = $state(ui.view!.project.medium.id);
  // svelte-ignore state_referenced_locally
  let condition = $state(ui.view!.project.medium.condition ?? '');
  // svelte-ignore state_referenced_locally
  let deck = $state(ui.view!.project.deck ?? '');
  let tab = $state<'pick' | 'params'>('pick');
  let saveName = $state('');

  const changed = $derived(medium !== p.medium.id || (condition || null) !== p.medium.condition || (deck || null) !== p.deck);
  const base = $derived(m.base ?? (m.builtin ? m.id : null));
  const curves = $derived(ui.media?.curves ?? []);

  async function apply() {
    await run(() => api.mediumSet(medium, condition || null, deck || null), 'Medium set. Captures, tracks and repairs are unchanged.');
    api.appInfo().then((i) => (ui.info = i)).catch(() => {});
    ui.dialog = null;
  }

  async function resetToBase() {
    if (!base) return;
    await run(() => api.mediumSet(base, p.medium.condition, p.deck), 'Back to the built-in settings.');
    medium = base;
  }

  async function save() {
    const n = saveName.trim();
    if (!n) return;
    try {
      ui.media = await api.mediumSave(n);
      toast(`Medium preset "${n}" saved.`);
      saveName = '';
      ui.dialog = 'medium';
    } catch (e) { fail(e); }
  }

  async function del(id: string) {
    try {
      ui.media = await api.mediumDelete(id);
      if (medium === id) medium = p.medium.id === id ? 'vinyl-lp' : p.medium.id;
    } catch (e) { fail(e); }
  }

  function med(label: string, f: (m: MediumPreset, p: Project) => void) {
    edit(label, (p) => f(p.medium.preset, p));
  }

  function setUnit(u: SpeedUnit) {
    med('Speed unit', (m, p) => {
      m.speed.unit = u;
      const nom: Record<SpeedUnit, [number, number[]]> = { rpm: [33.333, [33.333, 45]], cms: [4.76, [2.38, 4.76, 9.53]], ips: [7.5, [3.75, 7.5, 15]], none: [1, []] };
      [m.speed.nominal, m.speed.choices] = nom[u];
      for (const s of p.sources) {
        s.fx.speed.unit = u;
        s.fx.speed.captured = m.speed.nominal;
        s.fx.speed.correct = m.speed.nominal;
      }
    });
  }

  function curveSel(id: string): CurveSel {
    const c = curves.find((c) => c.id === id);
    return { id, curve: c ? { ...c.curve } : { lf_pole_us: 0, zero_us: 0, hf_pole_us: 0 } };
  }

  const stageNames: Record<string, string> = {
    speed: 'Speed', wow: 'Wow and flutter', azimuth: 'Azimuth', channel_mode: 'Channel mode', playback_eq: 'Playback EQ',
    nr: 'DNR (NR decode)', rumble: 'Rumble / high-pass', hum: 'Hum and buzz', dropout: 'Dropout repair', declick: 'Declick',
    decrackle: 'Decrackle', declip: 'Declip', denoise: 'Denoise', band_limit: 'Band limit', channel: 'Channel tools', eq: 'EQ',
    error_repair: 'Error repair', deemphasis: 'De-emphasis', level: 'Leveling',
  };
  const userMedia = $derived((ui.media?.media ?? []).filter((x) => !x.builtin));
</script>

<div class="modal-back" role="presentation" onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Escape') ui.dialog = null; }}>
  <div class="modal big">
    <header>
      <h2 class="grow">Medium</h2>
      <div class="row seg">
        <button class:on={tab === 'pick'} onclick={() => (tab = 'pick')}>Pick</button>
        <button class:on={tab === 'params'} onclick={() => (tab = 'params')} title="Every setting of this project's medium">Parameters</button>
      </div>
    </header>
    <div class="body col">
      {#if tab === 'pick'}
        <p class="muted small">Now: <span class="accent">{m.name}</span>{p.medium.condition ? `, ${p.medium.condition}` : ''}{p.deck ? `, deck ${p.deck}` : ''}. Changing it re-resolves the stage settings and keeps your captures, tracks and manual repairs.</p>
        <MediumPicker bind:medium bind:condition bind:deck keep />
        {#if userMedia.length}
          <div class="sep"></div>
          <span class="muted small">Your medium presets</span>
          {#each userMedia as u (u.id)}
            <div class="row small"><span class="grow">{u.name}</span><span class="muted">built on {u.base ?? 'nothing'}</span>
              <button class="ghost" onclick={() => del(u.id)}>Delete</button></div>
          {/each}
        {/if}
      {:else}
        <div class="grid">
          <span class="lab">Side naming</span>
          <select value={m.side_naming} onchange={(e) => med('Side naming', (m) => (m.side_naming = (e.target as HTMLSelectElement).value as any))}>
            <option value="side_ab">Side A, Side B</option><option value="program">Program 1 to 4</option>
            <option value="reel">Reel 1, Reel 2</option><option value="tape">Tape</option>
          </select>

          <span class="lab">Speed unit</span>
          <select value={m.speed.unit} onchange={(e) => setUnit((e.target as HTMLSelectElement).value as SpeedUnit)}>
            <option value="rpm">rpm</option><option value="cms">cm/s</option><option value="ips">ips</option><option value="none">none (hidden)</option>
          </select>

          {#if m.speed.unit !== 'none'}
            <span class="lab">Speeds offered</span>
            <input type="text" class="mono" value={m.speed.choices.map(nice).join(', ')} title="Comma-separated: the choices in the Speed card"
              onkeydown={(e) => e.stopPropagation()}
              onchange={(e) => med('Speed choices', (m) => (m.speed.choices = (e.target as HTMLInputElement).value.split(',').map((x) => parseFloat(x)).filter((x) => isFinite(x) && x > 0)))} />
            <span class="lab">Nominal speed</span>
            <input type="number" step="0.01" value={nice(m.speed.nominal)} onkeydown={(e) => e.stopPropagation()}
              onchange={(e) => med('Nominal speed', (m) => (m.speed.nominal = parseFloat((e.target as HTMLInputElement).value) || m.speed.nominal))} />
          {/if}

          <span class="lab">Capture path</span>
          <select value={m.capture.path} onchange={(e) => med('Capture path', (m) => (m.capture.path = (e.target as HTMLSelectElement).value as any))}>
            <option value="analog">Analog line-in</option><option value="spdif">S/PDIF, bit-perfect (ALSA hw)</option>
          </select>
          <span class="lab">Capture format</span>
          <div class="row">
            <select value={m.capture.rate} onchange={(e) => med('Capture rate', (m) => (m.capture.rate = parseInt((e.target as HTMLSelectElement).value)))}>
              {#each [0, 44100, 48000, 88200, 96000, 192000] as r}<option value={r}>{r ? `${r / 1000} kHz` : 'As the source'}</option>{/each}
            </select>
            <select value={m.capture.bits} onchange={(e) => med('Capture bits', (m) => (m.capture.bits = parseInt((e.target as HTMLSelectElement).value)))}>
              <option value={16}>16-bit</option><option value={24}>24-bit</option>
            </select>
          </div>
          <span class="lab">Auto-stop</span>
          <select value={m.capture.stop} onchange={(e) => med('Auto-stop', (m) => (m.capture.stop = (e.target as HTMLSelectElement).value as any))}>
            <option value="runout">At the run-out groove</option><option value="tape_end">At the end of the tape</option>
            <option value="digital_silence">At digital silence</option><option value="never">Never</option>
          </select>

          <span class="lab">Channel mode</span>
          <select value={p.chain.channel_mode.enabled ? p.chain.channel_mode.mode : 'stereo'}
            onchange={(e) => edit('Channel mode', (p) => { const v = (e.target as HTMLSelectElement).value as any; p.chain.channel_mode.mode = v; p.chain.channel_mode.enabled = v !== 'stereo'; })}>
            <option value="stereo">Stereo</option><option value="mono_sum">Mono, sum</option><option value="mono_left">Mono, left</option>
            <option value="mono_right">Mono, right</option><option value="best_auto">Best channel auto</option>
          </select>

          <span class="lab">Playback EQ</span>
          <div class="row">
            <select value={p.chain.playback_eq.from.id} title="The curve it was played through"
              onchange={(e) => edit('Playback EQ', (p) => { p.chain.playback_eq.from = curveSel((e.target as HTMLSelectElement).value); p.chain.playback_eq.enabled = p.chain.playback_eq.from.id !== p.chain.playback_eq.to.id; })}>
              {#each curves as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
            </select>
            <span class="muted small">to</span>
            <select value={p.chain.playback_eq.to.id} title="The curve it should have been played through"
              onchange={(e) => edit('Playback EQ', (p) => { p.chain.playback_eq.to = curveSel((e.target as HTMLSelectElement).value); p.chain.playback_eq.enabled = p.chain.playback_eq.from.id !== p.chain.playback_eq.to.id; })}>
              {#each curves as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
            </select>
          </div>

          <span class="lab">DNR</span>
          <div class="row">
            <label class="row"><input type="checkbox" checked={p.chain.nr.enabled} onchange={(e) => edit('DNR', (p) => (p.chain.nr.enabled = (e.target as HTMLInputElement).checked))} /> On</label>
            <select value={p.chain.nr.kind} onchange={(e) => edit('DNR type', (p) => (p.chain.nr.kind = (e.target as HTMLSelectElement).value as any))}>
              <option value="btype">B-type</option><option value="dbx2">dbx II</option>
            </select>
            {#if p.chain.nr.kind === 'btype'}
              <input type="number" step="0.5" value={p.chain.nr.ref_dbfs} title="Reference level, dBFS" onkeydown={(e) => e.stopPropagation()}
                onchange={(e) => edit('DNR reference', (p) => { p.chain.nr.ref_dbfs = parseFloat((e.target as HTMLInputElement).value); p.chain.nr.ref_measured = false; })} /><span class="muted small">dBFS</span>
            {/if}
          </div>

          <span class="lab">Wow reference</span>
          <div class="row">
            <select value={p.chain.wow.enabled ? p.chain.wow.reference : 'off'}
              onchange={(e) => edit('Wow reference', (p) => { const v = (e.target as HTMLSelectElement).value; p.chain.wow.enabled = v !== 'off'; if (v !== 'off') p.chain.wow.reference = v as any; })}>
              <option value="off">Off</option><option value="hum">Recorded hum</option><option value="tone">Named tone</option>
            </select>
            {#if p.chain.wow.reference === 'tone'}
              <input type="number" step="1" value={p.chain.wow.tone_hz} onkeydown={(e) => e.stopPropagation()}
                onchange={(e) => edit('Wow tone', (p) => (p.chain.wow.tone_hz = parseFloat((e.target as HTMLInputElement).value) || 3150))} /><span class="muted small">Hz</span>
            {:else}
              <select value={p.chain.wow.hum_hz} onchange={(e) => edit('Wow hum', (p) => (p.chain.wow.hum_hz = parseFloat((e.target as HTMLSelectElement).value)))}>
                <option value={50}>50 Hz</option><option value={60}>60 Hz</option>
              </select>
            {/if}
          </div>

          <span class="lab">Band limit</span>
          <div class="row">
            <label class="row"><input type="checkbox" checked={p.chain.band_limit.enabled} onchange={(e) => edit('Band limit', (p) => (p.chain.band_limit.enabled = (e.target as HTMLInputElement).checked))} /> On</label>
            <input type="number" step="1" value={p.chain.band_limit.low_hz} onkeydown={(e) => e.stopPropagation()} onchange={(e) => edit('Band limit low', (p) => (p.chain.band_limit.low_hz = parseFloat((e.target as HTMLInputElement).value)))} />
            <span class="muted small">to</span>
            <input type="number" step="100" value={p.chain.band_limit.high_hz} onkeydown={(e) => e.stopPropagation()} onchange={(e) => edit('Band limit high', (p) => (p.chain.band_limit.high_hz = parseFloat((e.target as HTMLInputElement).value)))} />
            <span class="muted small">Hz</span>
          </div>

          <span class="lab">Split strategy</span>
          <div class="row">
            <select value={p.split.mode} onchange={(e) => edit('Split strategy', (p) => { p.split.mode = (e.target as HTMLSelectElement).value as any; p.medium.preset.split = { ...p.split }; })}>
              <option value="absolute">Absolute silence</option><option value="noise_floor">Relative to the hiss floor</option>
              <option value="program_change">Program changes</option><option value="fixed_length">Fixed-length chapters</option>
            </select>
            {#if p.split.mode === 'fixed_length'}
              <input type="number" step="1" value={p.split.chapter_min} onkeydown={(e) => e.stopPropagation()} onchange={(e) => edit('Chapter length', (p) => (p.split.chapter_min = Math.max(1, parseFloat((e.target as HTMLInputElement).value) || 30)))} /><span class="muted small">min</span>
            {:else if p.split.mode === 'noise_floor'}
              <input type="number" step="0.5" value={p.split.floor_margin_db} onkeydown={(e) => e.stopPropagation()} onchange={(e) => edit('Floor margin', (p) => (p.split.floor_margin_db = parseFloat((e.target as HTMLInputElement).value) || 6))} /><span class="muted small">dB over the floor</span>
            {/if}
          </div>

          <span class="lab">Leveling</span>
          <div class="row">
            <label class="row"><input type="checkbox" checked={p.chain.level.enabled} onchange={(e) => edit('Leveling', (p) => (p.chain.level.enabled = (e.target as HTMLInputElement).checked))} /> On</label>
            <select value={p.chain.level.mode} onchange={(e) => edit('Level mode', (p) => (p.chain.level.mode = (e.target as HTMLSelectElement).value as any))}>
              <option value="track">Per track</option><option value="album">Album</option>
            </select>
            <input type="number" step="0.5" value={p.chain.level.target_lufs} title="Target, LUFS" onkeydown={(e) => e.stopPropagation()} onchange={(e) => edit('Level target', (p) => (p.chain.level.target_lufs = parseFloat((e.target as HTMLInputElement).value)))} /><span class="muted small">LUFS</span>
            <input type="number" step="0.1" value={p.chain.level.ceiling_dbtp} title="Ceiling, dBTP" onkeydown={(e) => e.stopPropagation()} onchange={(e) => edit('Level ceiling', (p) => (p.chain.level.ceiling_dbtp = parseFloat((e.target as HTMLInputElement).value)))} /><span class="muted small">dBTP</span>
          </div>

          <span class="lab">Export</span>
          <div class="row">
            <select value={p.export.kind} onchange={(e) => edit('Project kind', (p) => (p.export.kind = (e.target as HTMLSelectElement).value as any))}>
              <option value="album">Album</option><option value="mixtape">Mixtape</option><option value="recording">Recording</option>
            </select>
            <select value={p.export.rate} onchange={(e) => edit('Export rate', (p) => (p.export.rate = parseInt((e.target as HTMLSelectElement).value)))}>
              {#each [0, 44100, 48000, 96000] as r}<option value={r}>{r ? `${r / 1000} kHz` : 'Source rate'}</option>{/each}
            </select>
            <select value={p.export.bit_depth} onchange={(e) => edit('Bit depth', (p) => (p.export.bit_depth = parseInt((e.target as HTMLSelectElement).value)))}>
              <option value={16}>16-bit</option><option value={24}>24-bit</option>
            </select>
            <select value={p.export.channels} onchange={(e) => edit('Export channels', (p) => (p.export.channels = parseInt((e.target as HTMLSelectElement).value)))}>
              <option value={0}>Channels as processed</option><option value={1}>Mono</option><option value={2}>Stereo</option>
            </select>
          </div>
        </div>

        <span class="muted small">Stages shown in the Clean panel, in chain order</span>
        <div class="stages">
          {#each ui.media?.stages ?? [] as st}
            <label class="row small"><input type="checkbox" checked={m.visible.includes(st)}
              onchange={(e) => { const on = (e.target as HTMLInputElement).checked; med('Stages shown', (m) => { m.visible = (ui.media?.stages ?? []).filter((x) => (x === st ? on : m.visible.includes(x))); }); }} />{stageNames[st] ?? st}</label>
          {/each}
        </div>
        <Num label="Hum fundamental" value={p.chain.hum.fundamental_hz} min={20} max={1000} step={0.01} unit="Hz" digits={2}
          tip="Mains hum (50 or 60 Hz), or a video field rate (59.94 NTSC, 50 PAL) for head-switching buzz"
          onchange={(v) => edit('Hum fundamental', (p) => (p.chain.hum.fundamental_hz = v))} />
      {/if}
    </div>
    <footer>
      {#if tab === 'params'}
        {#if ui.dialog === 'medium-save'}
          <!-- svelte-ignore a11y_autofocus -->
          <input type="text" class="grow" placeholder="Name, e.g. My Nakamichi cassettes" bind:value={saveName} autofocus
            onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') save(); }} />
          <button onclick={() => (ui.dialog = 'medium')}>Cancel</button>
          <button class="primary" disabled={!saveName.trim()} onclick={save}>Save</button>
        {:else}
          <span class="muted small grow">Edits apply to this project now.</span>
          <button disabled={!base} onclick={resetToBase} title="Back to the built-in medium this one rests on">Reset to base</button>
          <button onclick={() => (ui.dialog = 'medium-save')} title="Keep these settings as a medium preset in ~/.config/runout/media">Save as preset</button>
          <button class="primary" onclick={() => (ui.dialog = null)}>Done</button>
        {/if}
      {:else}
        <button onclick={() => (ui.dialog = null)}>Cancel</button>
        <button class="primary" disabled={!changed} onclick={apply}>Apply</button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .big { width: min(900px, 94vw); }
  .seg { gap: 2px; }
  .seg button { padding: 3px 10px; font-size: 12px; }
  .grid { display: grid; grid-template-columns: 130px 1fr; gap: 7px 12px; align-items: center; }
  .lab { color: var(--muted); }
  .grid input[type=number] { width: 76px; }
  .stages { display: grid; grid-template-columns: repeat(auto-fill, minmax(170px, 1fr)); gap: 2px 10px; }
  select { padding: 3px 6px; }
</style>
