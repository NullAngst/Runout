<script lang="ts">
  // Step 3: the restoration chain as a stack of cards, in processing order.
  // Which cards show comes from the medium's visible-stage list.
  import { api, type Chain, type Project, type CurveSel, type Curve } from '../lib/api';
  import { ui, edit, run, chainIn, currentChain, currentSourceId, selectedTrack, fail, toast, medium } from '../lib/store.svelte';
  import { analyze, repairSelection } from '../lib/actions';
  import StageCard from '../components/StageCard.svelte';
  import Num from '../components/Num.svelte';
  import { time, hz, parseTime, speed as speedLabel } from '../lib/format';

  const c = $derived(currentChain());
  const t = $derived(selectedTrack());
  const sid = $derived(currentSourceId());
  const src = $derived(ui.view?.project.sources.find((s) => s.id === sid) ?? null);
  const trackId = $derived(ui.scope === 'track' && t?.chain ? t.id : null);
  const m = $derived(medium());
  const fam = $derived(m?.family ?? 'vinyl');
  const vinyl = $derived(fam === 'vinyl');
  let preset = $state('');
  let lengthText = $state('');

  // chain order, with the fixed cards placed where they run
  const ANALOG = ['dc', 'speed', 'wow', 'azimuth', 'channel_mode', 'playback_eq', 'nr', 'rumble', 'hum', 'dropout', 'declick', 'decrackle', 'manual', 'declip', 'denoise', 'band_limit', 'channel', 'eq'];
  const DIGITAL = ['error_repair', 'deemphasis', 'manual'];
  const order = $derived.by(() => {
    if (!c || !m) return [] as string[];
    const vis = new Set(m.visible);
    vis.add('manual');
    if (!c.digital) vis.add('dc');
    // a stage someone switched on stays visible
    for (const k of ['wow', 'azimuth', 'channel_mode', 'playback_eq', 'nr', 'dropout', 'band_limit', 'error_repair', 'deemphasis']) {
      if ((c as any)[k]?.enabled) vis.add(k);
    }
    if (m.speed.unit === 'none') vis.delete('speed');
    return (c.digital ? DIGITAL : ANALOG).filter((s) => vis.has(s));
  });

  function ch(label: string, f: (c: Chain, p: Project) => void) {
    edit(label, (p) => f(chainIn(p), p));
  }
  function fx(label: string, f: (s: Project['sources'][number]) => void) {
    edit(label, (p) => {
      const s = p.sources.find((s) => s.id === sid);
      if (s) f(s);
    });
  }
  const auto = (stage: string, ok = `Auto ${stage.replace('_', ' ')} done.`) => run(() => api.autoStage(stage, trackId), ok);

  async function loadPresets() {
    try { ui.presets = await api.presets(); } catch (e) { fail(e); }
  }
  $effect(() => { loadPresets(); });
  const familyPresets = $derived(ui.presets.filter((p) => (p.family ?? 'vinyl') === fam));

  function learnFromSelection() {
    const s = ui.selection;
    if (!s) return toast(vinyl ? 'Select a stretch of plain surface noise (lead-in or run-out) first.' : 'Select a stretch of plain hiss (the leader before the music, or a gap) first.');
    run(() => api.learnNoise(s.a, s.b), 'Noise profile learned from the selection.');
  }

  function scopeTrack() {
    if (!t) return;
    ui.scope = 'track';
    if (!t.chain) run(() => api.setOverride(t.id, true));
  }

  function setLength() {
    if (!src) return;
    const want = parseTime(lengthText);
    if (want == null || want <= 0) return toast('Type the length as minutes:seconds, like 3:41.', true);
    const tid = t && t.source_id === src.id ? t.id : null;
    run(() => api.speedFromLength(src.id, tid, want), tid ? `Speed set so track ${t?.number ?? ''} runs ${lengthText}.` : `Speed set so ${src.name} runs ${lengthText}.`);
    lengthText = '';
  }

  const repairs = $derived((src?.fx.repairs ?? []).map((r) => r));
  const noiseDb = $derived.by(() => {
    const n = src?.fx.noise;
    if (!n) return null;
    const N = 2048;
    let wsum = 0;
    for (let i = 0; i < N; i++) { const w = 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / N); wsum += w * w; }
    const p = n.mag.reduce((a, m) => a + m * m, 0) * 2 / (N * wsum);
    return 10 * Math.log10(Math.max(p, 1e-20));
  });

  // totals from the last analysis
  const counts = $derived.by(() => {
    let d = 0, df = 0, e = 0, ef = 0;
    for (const tr of ui.view?.project.tracks ?? []) {
      const a = tr.analysis;
      if (!a) continue;
      d += a.dropouts ?? 0; df += a.dropouts_flagged ?? 0; e += a.errors ?? 0; ef += a.errors_flagged ?? 0;
    }
    return { d, df, e, ef };
  });

  // speed choices: the medium's, plus whatever the side is set to now
  const speedChoices = $derived.by(() => {
    const ch = [...(m?.speed.choices ?? [])];
    if (src) for (const v of [src.fx.speed.captured, src.fx.speed.correct]) if (!ch.some((x) => Math.abs(x - v) < 1e-3)) ch.push(v);
    return ch.sort((a, b) => a - b);
  });
  const optLabel = (v: number) => (vinyl && m?.id !== 'vinyl-78' ? (Math.abs(v - 33.333) < 0.01 ? '33 1/3' : `${+v.toFixed(2)}`) : speedLabel(v, m?.speed.unit ?? 'rpm'));
  const unitLabel = $derived(m?.speed.unit === 'cms' ? 'cm/s' : (m?.speed.unit ?? 'rpm'));

  // playback EQ curves that fit this medium
  const curves = $derived((ui.media?.curves ?? []).filter((x) => x.kind === 'any' || x.kind === (vinyl ? 'disc' : 'tape')));
  function curveSel(id: string, old: CurveSel): CurveSel {
    if (id === 'custom') return { id, curve: { ...old.curve } };
    const x = ui.media?.curves.find((x) => x.id === id);
    return { id, curve: x ? { ...x.curve } : { lf_pole_us: 0, zero_us: 0, hf_pole_us: 0 } };
  }
  // the 78 controls: bass turnover in Hz and treble rolloff in dB at 10 kHz
  const turnoverHz = (cv: Curve) => (cv.zero_us > 0 ? 1e6 / (2 * Math.PI * cv.zero_us) : 0);
  const rolloffDb = (cv: Curve) => (cv.hf_pole_us > 0 ? 10 * Math.log10(1 + (2 * Math.PI * 1e4 * cv.hf_pole_us * 1e-6) ** 2) : 0);
  const usFromTurnover = (f: number) => (f > 0 ? 1e6 / (2 * Math.PI * f) : 0);
  const usFromRolloff = (d: number) => (d > 0.01 ? (Math.sqrt(10 ** (d / 10) - 1) / (2 * Math.PI * 1e4)) * 1e6 : 0);
  function setCustom(which: 'from' | 'to', f: (cv: Curve) => void) {
    ch('Playback EQ curve', (c) => {
      const s = c.playback_eq[which];
      s.id = 'custom';
      f(s.curve);
      c.playback_eq.enabled = true;
    });
  }
  const bestName = ['left', 'right', 'both, summed'];
  const TONES = [315, 333, 400, 1000, 3000, 3150, 10000];
</script>

{#snippet curvePick(which: 'from' | 'to', label: string, tip: string)}
  {#if c}
    {@const sel = c.playback_eq[which]}
    <label class="row" title={tip}><span class="lab">{label}</span>
      <select class="grow" value={sel.id} onchange={(e) => ch('Playback EQ', (c) => { c.playback_eq[which] = curveSel((e.target as HTMLSelectElement).value, c.playback_eq[which]); c.playback_eq.enabled = true; })}>
        {#each curves as cv (cv.id)}<option value={cv.id} title={cv.note}>{cv.name}</option>{/each}
        <option value="custom">Custom</option>
      </select></label>
    {#if sel.id === 'custom'}
      {#if vinyl}
        <Num label="Bass turnover" value={turnoverHz(sel.curve)} min={0} max={1500} step={5} unit="Hz" digits={0} tip="Where the bass boost starts. 0 for none."
          onchange={(v) => setCustom(which, (cv) => (cv.zero_us = usFromTurnover(v)))} />
        <Num label="Treble rolloff" value={rolloffDb(sel.curve)} min={0} max={24} step={0.1} unit="dB at 10 kHz" tip="How far the treble is cut at 10 kHz. 0 for flat treble."
          onchange={(v) => setCustom(which, (cv) => (cv.hf_pole_us = usFromRolloff(v)))} />
      {:else}
        <Num label="Treble" value={sel.curve.zero_us} min={0} max={200} step={1} unit="us" digits={0} tip="The treble time constant: 120 or 70 for cassette, 50, 35 or 90 for reels"
          onchange={(v) => setCustom(which, (cv) => (cv.zero_us = v))} />
        <Num label="Bass" value={sel.curve.lf_pole_us} min={0} max={5000} step={10} unit="us" digits={0} tip="The bass time constant: 3180 for most, 0 for none"
          onchange={(v) => setCustom(which, (cv) => (cv.lf_pole_us = v))} />
      {/if}
    {/if}
  {/if}
{/snippet}

{#if c && ui.view && m}
<div class="col gap">
  <div class="row">
    <h2 class="grow">Clean</h2>
    <button class="primary" title="Render every track through the chain: click and dropout maps, wow and azimuth curves, loudness and the before/after report (Ctrl+Enter)" onclick={() => analyze(true)} disabled={!ui.view.project.tracks.length || !!ui.tick.busy}>
      Analyze{ui.view.stale ? ` (${ui.view.stale} stale)` : ''}<span class="key">Ctrl+Enter</span>
    </button>
  </div>

  <div class="row wrap">
    <span class="muted small">Settings for</span>
    <button class:on={ui.scope === 'all' || !t?.chain} title="Edit the settings every track uses" onclick={() => (ui.scope = 'all')}>All tracks</button>
    <button class:on={ui.scope === 'track' && !!t?.chain} disabled={!t} title="Give the selected track its own settings, for a worn part of the {vinyl ? 'record' : 'tape'}" onclick={scopeTrack}>
      {t ? `Track ${t.number ?? ''} only` : 'One track only'}
    </button>
    {#if ui.scope === 'track' && t?.chain && !t.cleaned}
      <button class="ghost" title="Drop this track's own settings and use the project settings again" onclick={() => { ui.scope = 'all'; run(() => api.setOverride(t.id, false)); }}>Use project settings</button>
    {/if}
  </div>

  <div class="row">
    <select class="grow" bind:value={preset} title="Presets by condition. Each is a full chain.">
      <option value="">Preset...</option>
      {#each familyPresets as p (p.id)}<option value={p.id}>{p.name}{p.builtin ? '' : ' (yours)'}</option>{/each}
    </select>
    <button disabled={!preset} onclick={() => run(() => api.presetApply(preset, trackId), 'Preset applied.')}>Apply</button>
    <button class="ghost" title="Save the current settings as a preset" onclick={() => (ui.dialog = 'preset-save')}>Save as</button>
  </div>

  {#each order as st (st)}
    {#if st === 'dc'}
      <StageCard title="DC offset" enabled={true} fixed tip="A 5 Hz highpass takes out the constant offset some USB codecs add. Always on.">
        <p class="muted small">Always on, no controls.</p>
      </StageCard>

    {:else if st === 'speed' && src}
      {@const sp = src.fx.speed}
      <StageCard title="Speed and pitch" badge={src.name} enabled={sp.captured !== sp.correct || sp.cents !== 0 || sp.trim_pct !== 0}
        tip={vinyl ? 'Fix a side captured at the wrong speed, or a platter running slightly fast or slow' : 'Fix a tape played at the wrong speed, or a deck running fast or slow'}
        onToggle={(on) => { if (!on) fx('Speed', (s) => { s.fx.speed.correct = s.fx.speed.captured; s.fx.speed.cents = 0; s.fx.speed.trim_pct = 0; }); else fx('Speed', (s) => (s.fx.speed.cents = s.fx.speed.cents || 0.1)); }}
        onAuto={vinyl ? undefined : () => auto('speed', 'Speed set from the reference.')}>
        <div class="row">
          <label class="row grow" title="The speed it was actually played at">Played at
            <select value={sp.captured} onchange={(e) => fx('Speed', (s) => (s.fx.speed.captured = parseFloat((e.target as HTMLSelectElement).value)))}>
              {#each speedChoices as v}<option value={v}>{optLabel(v)}</option>{/each}
            </select></label>
          <label class="row grow" title="The speed it should have been played at">Should be
            <select value={sp.correct} onchange={(e) => fx('Speed', (s) => (s.fx.speed.correct = parseFloat((e.target as HTMLSelectElement).value)))}>
              {#each speedChoices as v}<option value={v}>{optLabel(v)}</option>{/each}
            </select></label>
        </div>
        <Num label="Fine adjust" value={sp.cents} min={-100} max={100} step={0.5} unit="cents" tip="Raise or lower speed and pitch slightly. 100 cents is a semitone."
          onchange={(v) => fx('Speed cents', (s) => (s.fx.speed.cents = v))} />
        {#if !vinyl}
          <Num label="Deck runs" value={sp.trim_pct} min={-6} max={6} step={0.01} unit="% fast" digits={2} tip="The playback deck's speed error: from a deck profile, the reference tone (Auto), or a known length. Negative is slow."
            onchange={(v) => fx('Speed trim', (s) => (s.fx.speed.trim_pct = v))} />
          {#if src.fx.analysis?.tone}
            {@const tn = src.fx.analysis.tone}
            <p class="small muted">Reference tone {tn.freq_hz.toFixed(1)} Hz at {tn.level_dbfs.toFixed(1)} dBFS{tn.speed_err_pct != null ? `: the deck runs ${Math.abs(tn.speed_err_pct).toFixed(2)}% ${tn.speed_err_pct >= 0 ? 'fast' : 'slow'}` : ''}.</p>
          {/if}
          <div class="row" title="Type the real length of the selected track (or the whole side with no track selected); the speed is solved from it">
            <span class="lab">Known length</span>
            <input type="text" class="mono len" placeholder="3:41" bind:value={lengthText} onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') setLength(); }} />
            <button class="ghost" disabled={!lengthText.trim()} onclick={setLength}>Set</button>
          </div>
        {/if}
        {#snippet advanced()}
          <Num label="Played at, exact" value={sp.captured} min={0.5} max={100} step={0.01} unit={unitLabel} digits={3} tip="For speeds not in the list, like 80 rpm or a custom 78"
            onchange={(v) => fx('Speed', (s) => (s.fx.speed.captured = v))} />
          <Num label="Should be, exact" value={sp.correct} min={0.5} max={100} step={0.01} unit={unitLabel} digits={3} tip="The speed it was recorded at"
            onchange={(v) => fx('Speed', (s) => (s.fx.speed.correct = v))} />
          {#if vinyl}
            <Num label="Platter runs" value={sp.trim_pct} min={-6} max={6} step={0.01} unit="% fast" digits={2} tip="A platter's measured speed error. Negative is slow."
              onchange={(v) => fx('Speed trim', (s) => (s.fx.speed.trim_pct = v))} />
          {/if}
        {/snippet}
      </StageCard>

    {:else if st === 'wow'}
      {@const an = src?.fx.analysis}
      <StageCard title="Wow and flutter" enabled={c.wow.enabled} tip="Takes the slow and fast speed wobble out, tracked from a steady reference recorded on the tape. Needs one: hum recorded onto the tape, or a test tone."
        onToggle={(on) => ch('Wow', (c) => (c.wow.enabled = on))} onAuto={() => auto('wow', 'Wow measured.')}>
        <label class="row" title="What to track: mains hum recorded onto the tape, or a test tone"><span class="lab">Reference</span>
          <select class="grow" value={c.wow.reference} onchange={(e) => ch('Wow reference', (c) => (c.wow.reference = (e.target as HTMLSelectElement).value as any))}>
            <option value="hum">Recorded hum</option><option value="tone">Test tone</option>
          </select></label>
        {#if c.wow.reference === 'hum'}
          <label class="row" title="The mains frequency where the tape was recorded"><span class="lab">Mains</span>
            <select class="grow" value={c.wow.hum_hz} onchange={(e) => ch('Wow hum', (c) => (c.wow.hum_hz = parseFloat((e.target as HTMLSelectElement).value)))}>
              <option value={60}>60 Hz</option><option value={50}>50 Hz</option>
            </select></label>
        {:else}
          <label class="row" title="3150 Hz also gives the weighted wow and flutter figure"><span class="lab">Tone</span>
            <select class="grow" value={c.wow.tone_hz} onchange={(e) => ch('Wow tone', (c) => (c.wow.tone_hz = parseFloat((e.target as HTMLSelectElement).value)))}>
              {#each TONES as f}<option value={f}>{hz(f)}</option>{/each}
              {#if !TONES.includes(c.wow.tone_hz)}<option value={c.wow.tone_hz}>{hz(c.wow.tone_hz)}</option>{/if}
            </select></label>
        {/if}
        {#if an?.wow_note}
          <p class="small" class:attention={!an.speed_curve} class:muted={!!an.speed_curve}>{an.wow_note}</p>
        {:else}
          <p class="small muted">Measured by Auto or Analyze. Without a stable reference it does nothing, and says so.</p>
        {/if}
      </StageCard>

    {:else if st === 'azimuth'}
      {@const an = src?.fx.analysis}
      <StageCard title="Azimuth" enabled={c.azimuth.enabled} tip="Lines the right channel up with the left when the playback head was tilted against the recording head. Tilt dulls the treble, worst in mono."
        onToggle={(on) => ch('Azimuth', (c) => (c.azimuth.enabled = on))} onAuto={() => auto('azimuth', 'Azimuth measured.')}>
        <label class="row" title="Auto: static when the offset barely moves, tracked when it drifts"><span class="lab">Mode</span>
          <select class="grow" value={c.azimuth.mode} onchange={(e) => ch('Azimuth mode', (c) => (c.azimuth.mode = (e.target as HTMLSelectElement).value as any))}>
            <option value="auto">Auto</option><option value="static">Static</option><option value="tracked">Tracked</option>
          </select></label>
        {#if c.azimuth.mode !== 'tracked'}
          <Num label="Offset" value={c.azimuth.static_us} min={-1000} max={1000} step={0.5} unit="us" tip="Delays the right channel against the left, in microseconds. Used for Static, and for Auto when there is no curve."
            onchange={(v) => ch('Azimuth offset', (c) => (c.azimuth.static_us = v))} />
        {/if}
        <p class="small muted">{an?.azimuth_curve ? 'Curve measured; the Azimuth overlay shows it.' : 'No curve yet: press Auto, or Analyze.'}</p>
      </StageCard>

    {:else if st === 'channel_mode'}
      {@const best = src?.fx.analysis?.best_channel}
      <StageCard title="Channel mode" enabled={c.channel_mode.enabled} tip="Mono sources: sum the channels, or keep the better one. Mono exports are 1-channel FLAC."
        onToggle={(on) => ch('Channel mode', (c) => (c.channel_mode.enabled = on))} onAuto={() => auto('channel_mode', 'Best channel measured.')}>
        <label class="row"><span class="lab">Mode</span>
          <select class="grow" value={c.channel_mode.mode} onchange={(e) => ch('Channel mode', (c) => (c.channel_mode.mode = (e.target as HTMLSelectElement).value as any))}>
            <option value="stereo">Stereo</option>
            <option value="mono_sum" title="Summing cancels noise the two channels do not share: about 3 dB less hiss">Mono, sum</option>
            <option value="mono_left">Mono, left only</option>
            <option value="mono_right">Mono, right only</option>
            <option value="best_auto" title="Compares noise floor and dropouts per channel: uses the better one, or sums when they are close">Best channel auto</option>
          </select></label>
        {#if c.channel_mode.mode === 'best_auto'}
          <p class="small muted">{best != null ? `Best channel: ${bestName[best]}.` : 'Measured by Auto or Analyze.'}</p>
        {/if}
      </StageCard>

    {:else if st === 'playback_eq'}
      <StageCard title="Playback EQ" enabled={c.playback_eq.enabled} tip={vinyl ? "Converts from RIAA to the record's own curve. Old records used many." : 'Converts between playback curves when a tape was played with the wrong one'}
        onToggle={(on) => ch('Playback EQ', (c) => (c.playback_eq.enabled = on))}>
        {@render curvePick('from', 'Played with', vinyl ? 'The curve your preamp applied: RIAA for almost every phono stage' : 'The curve the deck played it with')}
        {@render curvePick('to', 'Should be', vinyl ? "The record's own curve" : 'The curve it was recorded for')}
        {#if c.playback_eq.from.id === c.playback_eq.to.id && c.playback_eq.from.id !== 'custom'}<p class="small muted">Same curve both sides: nothing to correct.</p>{/if}
      </StageCard>

    {:else if st === 'nr'}
      <StageCard title="DNR (noise reduction decode)" enabled={c.nr.enabled} tip="Decodes a tape recorded with noise reduction. Best: deck NR off, capture undecoded, switch this on. The raw stays encoded, so you can redo the decode and A/B it."
        onToggle={(on) => ch('DNR', (c) => (c.nr.enabled = on))} onAuto={c.nr.kind === 'btype' ? () => auto('nr', 'Reference level read from the tone.') : undefined}>
        <label class="row"><span class="lab">Type</span>
          <select class="grow" value={c.nr.kind} onchange={(e) => ch('DNR type', (c) => (c.nr.kind = (e.target as HTMLSelectElement).value as any))}>
            <option value="btype" title="For tapes recorded with Dolby B">B-type</option>
            <option value="dbx2" title="Level independent, no calibration">dbx II</option>
          </select></label>
        {#if c.nr.kind === 'btype'}
          <Num label="Reference level" value={c.nr.ref_dbfs} min={-36} max={0} step={0.1} unit="dBFS" tip="Where the tape's NR reference level lands in the capture. Auto reads it from a level tone at the head of the tape."
            onchange={(v) => ch('DNR reference', (c) => { c.nr.ref_dbfs = v; c.nr.ref_measured = false; })} />
          <p class="small" class:muted={c.nr.ref_measured} class:attention={!c.nr.ref_measured}>
            {c.nr.ref_measured ? 'Set from the level tone.' : 'Typed in.'} A 2 dB error is audible: dull treble means the level is set too high, spitty treble too low.
          </p>
        {:else}
          <p class="small muted">dbx II needs no calibration.</p>
        {/if}
        <p class="small muted">Capture with the deck's own NR switched off.</p>
      </StageCard>

    {:else if st === 'rumble'}
      <StageCard title={vinyl ? 'Rumble filter' : 'High-pass'} enabled={c.rumble.enabled} tip={vinyl ? 'High-pass that removes motor and tonearm rumble below the music' : 'High-pass that removes motor noise, handling thumps and DC drift below the music'}
        onToggle={(on) => ch('Rumble', (c) => (c.rumble.enabled = on))} onAuto={() => auto('rumble')}>
        <Num label="Cutoff" value={c.rumble.freq_hz} min={10} max={vinyl ? 80 : 200} step={1} unit="Hz" digits={0} tip="Everything below this frequency is rolled off"
          onchange={(v) => ch('Rumble cutoff', (c) => (c.rumble.freq_hz = v))} />
        <label class="row" title="How steeply it rolls off below the cutoff">Slope
          <select value={c.rumble.slope_db_oct} onchange={(e) => ch('Rumble slope', (c) => (c.rumble.slope_db_oct = parseInt((e.target as HTMLSelectElement).value)))}>
            {#each [12, 24, 36, 48] as s}<option value={s}>{s} dB/octave</option>{/each}
          </select></label>
        {#snippet advanced()}
          <label class="row" title="No phase shift in the bass, at the cost of a little pre-ringing"><input type="checkbox" checked={c.rumble.linear_phase}
            onchange={(e) => ch('Rumble phase', (c) => (c.rumble.linear_phase = (e.target as HTMLInputElement).checked))} /> Linear phase</label>
        {/snippet}
      </StageCard>

    {:else if st === 'hum'}
      {@const mains = vinyl ? [60, 50] : [50, 59.94, 60]}
      <StageCard title={vinyl ? 'Hum removal' : 'Hum and buzz'} enabled={c.hum.enabled} tip={vinyl ? 'Notches out mains hum and its harmonics' : 'Notches out mains hum, or video head-switching buzz at the field rate, and their harmonics'}
        onToggle={(on) => ch('Hum', (c) => (c.hum.enabled = on))} onAuto={() => auto('hum')}>
        <label class="row" title={vinyl ? 'Mains frequency: 60 Hz in the US, 50 Hz in most other places' : 'Mains: 60 Hz in the US, 50 Hz elsewhere. Video buzz: 59.94 Hz NTSC, 50 Hz PAL.'}>{vinyl ? 'Mains' : 'Fundamental'}
          <select value={c.hum.fundamental_hz} onchange={(e) => ch('Hum mains', (c) => (c.hum.fundamental_hz = parseFloat((e.target as HTMLSelectElement).value)))}>
            {#each mains as f}<option value={f}>{f} Hz</option>{/each}
            {#if !mains.includes(c.hum.fundamental_hz)}<option value={c.hum.fundamental_hz}>{c.hum.fundamental_hz} Hz</option>{/if}
          </select></label>
        <Num label="Harmonics" value={c.hum.harmonics} min={1} max={vinyl ? 8 : 200} step={1} digits={0} tip="How many multiples of the fundamental get notched. Video buzz runs up to 10 kHz."
          onchange={(v) => ch('Hum harmonics', (c) => (c.hum.harmonics = v))} />
        {#snippet advanced()}
          <Num label="Notch width" value={c.hum.width_hz} min={0.2} max={5} step={0.1} unit="Hz" tip="Wider notches catch drifting hum but take out more music around it"
            onchange={(v) => ch('Hum width', (c) => (c.hum.width_hz = v))} />
          {#if !vinyl}
            <Num label="Fundamental" value={c.hum.fundamental_hz} min={20} max={1000} step={0.01} unit="Hz" digits={2} tip="Any fundamental, for odd buzz"
              onchange={(v) => ch('Hum fundamental', (c) => (c.hum.fundamental_hz = v))} />
          {/if}
        {/snippet}
      </StageCard>

    {:else if st === 'dropout'}
      <StageCard title="Dropout repair" enabled={c.dropout.enabled} tip="Finds the short level dips where the tape lost contact with the head. Short ones are rebuilt; longer ones get their level back but not their lost treble; the longest are flagged for you."
        onToggle={(on) => ch('Dropout', (c) => (c.dropout.enabled = on))} onAuto={() => auto('dropout')}>
        <Num label="Depth" value={c.dropout.depth_db} min={3} max={24} step={0.5} unit="dB" tip="How far below the local level a dip must go to count"
          onchange={(v) => ch('Dropout depth', (c) => (c.dropout.depth_db = v))} />
        <Num label="Longest repaired" value={c.dropout.max_ms} min={20} max={2000} step={10} unit="ms" digits={0} tip="Longer dips are only flagged, for a manual repair"
          onchange={(v) => ch('Dropout length', (c) => (c.dropout.max_ms = v))} />
        {#if counts.d || counts.df}
          <p class="small" class:attention={counts.df > 0} class:muted={!counts.df}>{counts.d} repaired{counts.df ? `, ${counts.df} too long and flagged: hop to them with [ and ]` : ''}.</p>
        {/if}
        {#snippet advanced()}
          <Num label="Rebuilt up to" value={c.dropout.interp_ms} min={2} max={20} step={0.5} unit="ms" tip="Dips this short are rebuilt from the audio around them; longer ones get gain compensation"
            onchange={(v) => ch('Dropout interp', (c) => (c.dropout.interp_ms = v))} />
        {/snippet}
      </StageCard>

    {:else if st === 'declick'}
      <StageCard title="Declick" enabled={c.declick.enabled} tip="Finds clicks the music cannot explain and rebuilds the audio under them from both sides"
        onToggle={(on) => ch('Declick', (c) => (c.declick.enabled = on))} onAuto={() => auto('declick')}>
        <Num label="Threshold" value={c.declick.threshold_db} min={8} max={24} step={0.5} unit="dB" tip="How far above the background a sample must stick out to count as a click. Lower catches more."
          onchange={(v) => ch('Declick threshold', (c) => (c.declick.threshold_db = v))} />
        <Num label="Longest click" value={c.declick.max_len_ms} min={0.5} max={6} step={0.1} unit="ms" tip="Anything longer is left alone, which protects drum hits"
          onchange={(v) => ch('Declick length', (c) => (c.declick.max_len_ms = v))} />
      </StageCard>

    {:else if st === 'decrackle'}
      <StageCard title="Decrackle" enabled={c.decrackle.enabled} tip="Same idea, tuned for the dense tiny ticks of worn records"
        onToggle={(on) => ch('Decrackle', (c) => (c.decrackle.enabled = on))} onAuto={() => auto('decrackle')}>
        <Num label="Threshold" value={c.decrackle.threshold_db} min={6} max={18} step={0.5} unit="dB" tip="Lower removes more crackle; too low starts to dull transients"
          onchange={(v) => ch('Decrackle threshold', (c) => (c.decrackle.threshold_db = v))} />
        {#snippet advanced()}
          <Num label="Longest tick" value={c.decrackle.max_len_ms} min={0.05} max={0.6} step={0.01} unit="ms" digits={2} tip="Only ticks this short count as crackle"
            onchange={(v) => ch('Decrackle length', (c) => (c.decrackle.max_len_ms = v))} />
        {/snippet}
      </StageCard>

    {:else if st === 'manual'}
      <StageCard title="Manual repair" enabled={true} fixed badge={src?.name ?? ''} tip="For the damage the automatic pass missed: select it on the waveform, or paint it on the spectrogram, and press Repair">
        <button onclick={repairSelection} disabled={!ui.selection} title="Repair the selected region (X)">
          Repair {ui.selection?.band ? 'band' : 'selection'}<span class="key">X</span>
        </button>
        {#each repairs as r (r.id)}
          <div class="row small">
            <span class="mono grow">{time(r.start_s, true)} to {time(r.end_s, true)}{r.band ? `, ${hz(r.band[0])} to ${hz(r.band[1])}` : ''}</span>
            <button class="ghost" title="Undo this repair" onclick={() => run(() => api.removeRepair(r.id))}>Remove</button>
          </div>
        {/each}
      </StageCard>

    {:else if st === 'declip'}
      <StageCard title="Declip" enabled={c.declip.enabled} tip="Rebuilds peaks that were flattened by a capture that ran too hot. Off unless clipping is found."
        onToggle={(on) => ch('Declip', (c) => (c.declip.enabled = on))} onAuto={() => auto('declip')}>
        <Num label="Clip level" value={c.declip.level_db} min={-6} max={0} step={0.05} unit="dBFS" digits={2} tip="Runs of samples at or above this level get rebuilt"
          onchange={(v) => ch('Declip level', (c) => (c.declip.level_db = v))} />
        {#snippet advanced()}
          <Num label="Longest run" value={c.declip.max_len_ms} min={0.2} max={5} step={0.1} unit="ms" tip="Longer flat runs are left alone"
            onchange={(v) => ch('Declip length', (c) => (c.declip.max_len_ms = v))} />
        {/snippet}
      </StageCard>

    {:else if st === 'denoise'}
      <StageCard title="Denoise" enabled={c.denoise.enabled} tip={vinyl ? 'Removes steady surface hiss. Gentle by default: over-denoised vinyl sounds watery, which is worse than hiss.' : 'Removes steady tape hiss. The main stage for tape. Gentle by default: overdone, it sounds watery.'}
        onToggle={(on) => ch('Denoise', (c) => (c.denoise.enabled = on))} onAuto={() => auto('denoise')}>
        <Num label="Reduction" value={c.denoise.reduction_db} min={0} max={20} step={0.5} unit="dB" tip="The most the hiss is turned down by"
          onchange={(v) => ch('Denoise amount', (c) => (c.denoise.reduction_db = v))} />
        <Num label="Smoothing" value={c.denoise.smoothing_pct} min={0} max={100} step={5} unit="%" digits={0} tip="Smooths the reduction over time to avoid a chirpy, watery sound"
          onchange={(v) => ch('Denoise smoothing', (c) => (c.denoise.smoothing_pct = v))} />
        {#if src?.fx.noise}
          <p class="small muted">Noise profile from {src.name}, {time(src.fx.noise.from_s, true)} to {time(src.fx.noise.to_s, true)}, about {noiseDb?.toFixed(0)} dBFS.</p>
        {:else}
          <p class="small attention">No noise profile for {src?.name ?? 'this side'}. Press Auto, or select some {vinyl ? 'lead-in noise' : 'plain hiss'} and learn from it.</p>
        {/if}
        <button class="ghost" onclick={learnFromSelection} disabled={!ui.selection} title="Learn the noise from the selected stretch instead">Learn from selection</button>
      </StageCard>

    {:else if st === 'band_limit'}
      <StageCard title="Band limit" enabled={c.band_limit.enabled} tip="Cuts above and below what the medium can actually carry, so the hiss out there goes"
        onToggle={(on) => ch('Band limit', (c) => (c.band_limit.enabled = on))} onAuto={() => auto('band_limit', "Band limit set to the medium's range.")}>
        <Num label="Low" value={c.band_limit.low_hz} min={10} max={500} step={5} unit="Hz" digits={0} tip="Everything below is cut"
          onchange={(v) => ch('Band limit low', (c) => (c.band_limit.low_hz = v))} />
        <Num label="High" value={c.band_limit.high_hz} min={2000} max={24000} step={100} unit="Hz" digits={0} tip="Everything above is cut"
          onchange={(v) => ch('Band limit high', (c) => (c.band_limit.high_hz = v))} />
        {#snippet advanced()}
          <label class="row">Slope
            <select value={c.band_limit.slope_db_oct} onchange={(e) => ch('Band limit slope', (c) => (c.band_limit.slope_db_oct = parseInt((e.target as HTMLSelectElement).value)))}>
              {#each [12, 24, 36, 48] as s}<option value={s}>{s} dB/octave</option>{/each}
            </select></label>
        {/snippet}
      </StageCard>

    {:else if st === 'channel'}
      <StageCard title="Channel tools" enabled={c.channel.enabled} tip={vinyl ? 'Balance, channel swap and azimuth (time offset between left and right from a misaligned cartridge)' : 'Balance and channel swap. Azimuth has its own stage.'}
        onToggle={(on) => ch('Channel', (c) => (c.channel.enabled = on))} onAuto={() => auto('channel')}>
        <Num label="Balance" value={c.channel.balance_db} min={-6} max={6} step={0.1} unit="dB" tip="Positive moves the image right"
          onchange={(v) => ch('Balance', (c) => (c.channel.balance_db = v))} />
        {#if vinyl}
          <Num label="Azimuth" value={c.channel.azimuth_us} min={-500} max={500} step={1} unit="us" digits={0} tip="Delays the right channel against the left, in microseconds"
            onchange={(v) => ch('Azimuth', (c) => (c.channel.azimuth_us = v))} />
        {/if}
        {#snippet advanced()}
          <label class="row" title="Swap left and right"><input type="checkbox" checked={c.channel.swap} onchange={(e) => ch('Swap', (c) => (c.channel.swap = (e.target as HTMLInputElement).checked))} /> Swap channels</label>
        {/snippet}
      </StageCard>

    {:else if st === 'eq'}
      <StageCard title="EQ" enabled={c.eq.enabled} tip="Low and high shelves plus parametric bands for common fixes"
        onToggle={(on) => ch('EQ', (c) => (c.eq.enabled = on))} onAuto={() => auto('eq')}>
        <div class="row wrap">
          <button class="ghost" title="Lift the top end a little" onclick={() => ch('EQ preset', (c) => { c.eq.enabled = true; c.eq.high_shelf_hz = 6000; c.eq.high_shelf_db = 3; })}>Dull top end</button>
          <button class="ghost" title="Take some mud out of the low mids" onclick={() => ch('EQ preset', (c) => { c.eq.enabled = true; c.eq.bands = [{ freq_hz: 150, gain_db: -3, q: 1 }]; })}>Boomy low end</button>
          <button class="ghost" title="Back to flat" onclick={() => ch('EQ preset', (c) => { c.eq.low_shelf_db = 0; c.eq.high_shelf_db = 0; c.eq.bands = []; })}>Flat</button>
        </div>
        <Num label="Low shelf" value={c.eq.low_shelf_db} min={-12} max={12} step={0.5} unit="dB" tip="Raises or lowers everything below the shelf frequency"
          onchange={(v) => ch('Low shelf', (c) => (c.eq.low_shelf_db = v))} />
        <Num label="High shelf" value={c.eq.high_shelf_db} min={-12} max={12} step={0.5} unit="dB" tip="Raises or lowers everything above the shelf frequency"
          onchange={(v) => ch('High shelf', (c) => (c.eq.high_shelf_db = v))} />
        {#snippet advanced()}
          <Num label="Low shelf at" value={c.eq.low_shelf_hz} min={40} max={500} step={5} unit="Hz" digits={0} tip="Where the low shelf starts" onchange={(v) => ch('Low shelf freq', (c) => (c.eq.low_shelf_hz = v))} />
          <Num label="High shelf at" value={c.eq.high_shelf_hz} min={2000} max={16000} step={100} unit="Hz" digits={0} tip="Where the high shelf starts" onchange={(v) => ch('High shelf freq', (c) => (c.eq.high_shelf_hz = v))} />
          {#each c.eq.bands as b, i}
            <div class="card col band">
              <div class="row"><span class="small muted grow">Band {i + 1}</span><button class="ghost" onclick={() => ch('EQ band', (c) => c.eq.bands.splice(i, 1))}>Remove</button></div>
              <Num label="Frequency" value={b.freq_hz} min={20} max={16000} step={10} unit="Hz" digits={0} tip="Centre of the band" onchange={(v) => ch('EQ band', (c) => (c.eq.bands[i].freq_hz = v))} />
              <Num label="Gain" value={b.gain_db} min={-12} max={12} step={0.5} unit="dB" tip="How much the band is raised or lowered" onchange={(v) => ch('EQ band', (c) => (c.eq.bands[i].gain_db = v))} />
              <Num label="Width (Q)" value={b.q} min={0.2} max={10} step={0.1} tip="Higher Q is narrower" onchange={(v) => ch('EQ band', (c) => (c.eq.bands[i].q = v))} />
            </div>
          {/each}
          <button class="ghost" onclick={() => ch('EQ band', (c) => c.eq.bands.push({ freq_hz: 1000, gain_db: 0, q: 1 }))}>Add band</button>
        {/snippet}
      </StageCard>

    {:else if st === 'error_repair'}
      <StageCard title="Error repair" enabled={c.error_repair.enabled} tip="Finds the marks of a deck muting or holding a sample on errors it could not correct: runs of exact zero inside music, and runs of identical samples. Short ones are rebuilt, longer ones flagged."
        onToggle={(on) => ch('Error repair', (c) => (c.error_repair.enabled = on))} onAuto={() => auto('error_repair', 'Digital errors searched.')}>
        <Num label="Shortest run" value={c.error_repair.min_run} min={2} max={32} step={1} unit="samples" digits={0} tip="Identical samples in a row before it counts as an error"
          onchange={(v) => ch('Error run', (c) => (c.error_repair.min_run = v))} />
        <Num label="Longest repaired" value={c.error_repair.max_ms} min={1} max={50} step={0.5} unit="ms" tip="Longer errors are only flagged"
          onchange={(v) => ch('Error length', (c) => (c.error_repair.max_ms = v))} />
        {#if counts.e || counts.ef}
          <p class="small" class:attention={counts.ef > 0} class:muted={!counts.ef}>{counts.e} repaired{counts.ef ? `, ${counts.ef} flagged: hop to them with [ and ]` : ''}.</p>
        {:else}
          <p class="small muted">Off unless errors are found. With every stage off the export is bit-identical to the source.</p>
        {/if}
      </StageCard>

    {:else if st === 'deemphasis'}
      <StageCard title="De-emphasis" enabled={c.deemphasis.enabled} tip="The 50/15 us treble cut for recordings made with pre-emphasis. Switched on by itself when the S/PDIF stream says so."
        onToggle={(on) => ch('De-emphasis', (c) => (c.deemphasis.enabled = on))}>
        <p class="small muted">50/15 us. Leave off unless the source was pre-emphasized: on a flat source it dulls the treble.</p>
      </StageCard>
    {/if}
  {/each}

  <p class="muted small">Leveling runs last; it is in the Level step.</p>
</div>
{/if}

<style>
  .gap { gap: 8px; }
  .band { padding: 6px 8px; gap: 4px; }
  select { padding: 3px 6px; }
  .lab { width: 92px; color: var(--muted); flex: none; }
  .len { width: 80px; }
</style>
