<script lang="ts">
  // Step 3: the restoration chain as a stack of cards, in processing order.
  import { api, type Chain, type Project } from '../lib/api';
  import { ui, edit, run, chainIn, currentChain, currentSourceId, selectedTrack, fail, toast } from '../lib/store.svelte';
  import { analyze, repairSelection } from '../lib/actions';
  import StageCard from '../components/StageCard.svelte';
  import Num from '../components/Num.svelte';
  import { time, hz } from '../lib/format';

  const c = $derived(currentChain());
  const t = $derived(selectedTrack());
  const sid = $derived(currentSourceId());
  const src = $derived(ui.view?.project.sources.find((s) => s.id === sid) ?? null);
  const trackId = $derived(ui.scope === 'track' && t?.chain ? t.id : null);
  let preset = $state('');

  function ch(label: string, f: (c: Chain, p: Project) => void) {
    edit(label, (p) => f(chainIn(p), p));
  }
  function fx(label: string, f: (s: Project['sources'][number]) => void) {
    edit(label, (p) => {
      const s = p.sources.find((s) => s.id === sid);
      if (s) f(s);
    });
  }
  const auto = (stage: string) => run(() => api.autoStage(stage, trackId), `Auto ${stage} done.`);

  async function loadPresets() {
    try { ui.presets = await api.presets(); } catch (e) { fail(e); }
  }
  $effect(() => { loadPresets(); });

  function learnFromSelection() {
    const s = ui.selection;
    if (!s) return toast('Select a stretch of plain surface noise (lead-in or run-out) first.');
    run(() => api.learnNoise(s.a, s.b), 'Noise profile learned from the selection.');
  }

  function scopeTrack() {
    if (!t) return;
    ui.scope = 'track';
    if (!t.chain) run(() => api.setOverride(t.id, true));
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
</script>

{#if c && ui.view}
<div class="col gap">
  <div class="row">
    <h2 class="grow">Clean</h2>
    <button class="primary" title="Render every track through the chain: click map, loudness and the before/after report (Ctrl+Enter)" onclick={() => analyze(true)} disabled={!ui.view.project.tracks.length || !!ui.tick.busy}>
      Analyze{ui.view.stale ? ` (${ui.view.stale} stale)` : ''}<span class="key">Ctrl+Enter</span>
    </button>
  </div>

  <div class="row wrap">
    <span class="muted small">Settings for</span>
    <button class:on={ui.scope === 'all' || !t?.chain} title="Edit the settings every track uses" onclick={() => (ui.scope = 'all')}>All tracks</button>
    <button class:on={ui.scope === 'track' && !!t?.chain} disabled={!t} title="Give the selected track its own settings, for a worn part of the record" onclick={scopeTrack}>
      {t ? `Track ${t.number ?? ''} only` : 'One track only'}
    </button>
    {#if ui.scope === 'track' && t?.chain && !t.cleaned}
      <button class="ghost" title="Drop this track's own settings and use the project settings again" onclick={() => { ui.scope = 'all'; run(() => api.setOverride(t.id, false)); }}>Use project settings</button>
    {/if}
  </div>

  <div class="row">
    <select class="grow" bind:value={preset} title="Presets by record condition. Each is a full chain.">
      <option value="">Preset...</option>
      {#each ui.presets as p (p.id)}<option value={p.id}>{p.name}{p.builtin ? '' : ' (yours)'}</option>{/each}
    </select>
    <button disabled={!preset} onclick={() => run(() => api.presetApply(preset, trackId), 'Preset applied.')}>Apply</button>
    <button class="ghost" title="Save the current settings as a preset" onclick={() => (ui.dialog = 'preset-save')}>Save as</button>
  </div>

  <StageCard title="DC offset" enabled={true} fixed tip="A 5 Hz highpass takes out the constant offset some USB codecs add. Always on.">
    <p class="muted small">Always on, no controls.</p>
  </StageCard>

  {#if src}
    {@const sp = src.fx.speed}
    <StageCard title="Speed and pitch" badge={src.name} enabled={sp.captured_rpm !== sp.correct_rpm || sp.cents !== 0}
      tip="Fix a side captured at the wrong speed, or a platter running slightly fast or slow"
      onToggle={(on) => { if (!on) fx('Speed', (s) => { s.fx.speed.correct_rpm = s.fx.speed.captured_rpm; s.fx.speed.cents = 0; }); else fx('Speed', (s) => (s.fx.speed.cents = s.fx.speed.cents || 0.1)); }}>
      <div class="row">
        <label class="row grow" title="The speed the record was actually played at">Played at
          <select value={sp.captured_rpm} onchange={(e) => fx('Speed', (s) => (s.fx.speed.captured_rpm = parseFloat((e.target as HTMLSelectElement).value)))}>
            <option value={33.333}>33 1/3</option><option value={45}>45</option>
          </select></label>
        <label class="row grow" title="The speed it should have been played at">Should be
          <select value={sp.correct_rpm} onchange={(e) => fx('Speed', (s) => (s.fx.speed.correct_rpm = parseFloat((e.target as HTMLSelectElement).value)))}>
            <option value={33.333}>33 1/3</option><option value={45}>45</option>
          </select></label>
      </div>
      <Num label="Fine adjust" value={sp.cents} min={-100} max={100} step={0.5} unit="cents" tip="Raise or lower speed and pitch slightly. 100 cents is a semitone."
        onchange={(v) => fx('Speed cents', (s) => (s.fx.speed.cents = v))} />
    </StageCard>
  {/if}

  <StageCard title="Rumble filter" enabled={c.rumble.enabled} tip="High-pass that removes motor and tonearm rumble below the music"
    onToggle={(on) => ch('Rumble', (c) => (c.rumble.enabled = on))} onAuto={() => auto('rumble')}>
    <Num label="Cutoff" value={c.rumble.freq_hz} min={10} max={80} step={1} unit="Hz" digits={0} tip="Everything below this frequency is rolled off"
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

  <StageCard title="Hum removal" enabled={c.hum.enabled} tip="Notches out mains hum and its harmonics"
    onToggle={(on) => ch('Hum', (c) => (c.hum.enabled = on))} onAuto={() => auto('hum')}>
    <label class="row" title="Mains frequency: 60 Hz in the US, 50 Hz in most other places">Mains
      <select value={c.hum.mains_hz} onchange={(e) => ch('Hum mains', (c) => (c.hum.mains_hz = parseFloat((e.target as HTMLSelectElement).value)))}>
        <option value={60}>60 Hz</option><option value={50}>50 Hz</option>
      </select></label>
    <Num label="Harmonics" value={c.hum.harmonics} min={1} max={8} step={1} digits={0} tip="How many multiples of the mains frequency get notched"
      onchange={(v) => ch('Hum harmonics', (c) => (c.hum.harmonics = v))} />
    {#snippet advanced()}
      <Num label="Notch width" value={c.hum.width_hz} min={0.2} max={5} step={0.1} unit="Hz" tip="Wider notches catch drifting hum but take out more music around it"
        onchange={(v) => ch('Hum width', (c) => (c.hum.width_hz = v))} />
    {/snippet}
  </StageCard>

  <StageCard title="Declick" enabled={c.declick.enabled} tip="Finds clicks the music cannot explain and rebuilds the audio under them from both sides"
    onToggle={(on) => ch('Declick', (c) => (c.declick.enabled = on))} onAuto={() => auto('declick')}>
    <Num label="Threshold" value={c.declick.threshold_db} min={8} max={24} step={0.5} unit="dB" tip="How far above the background a sample must stick out to count as a click. Lower catches more."
      onchange={(v) => ch('Declick threshold', (c) => (c.declick.threshold_db = v))} />
    <Num label="Longest click" value={c.declick.max_len_ms} min={0.5} max={6} step={0.1} unit="ms" tip="Anything longer is left alone, which protects drum hits"
      onchange={(v) => ch('Declick length', (c) => (c.declick.max_len_ms = v))} />
  </StageCard>

  <StageCard title="Decrackle" enabled={c.decrackle.enabled} tip="Same idea, tuned for the dense tiny ticks of worn records"
    onToggle={(on) => ch('Decrackle', (c) => (c.decrackle.enabled = on))} onAuto={() => auto('decrackle')}>
    <Num label="Threshold" value={c.decrackle.threshold_db} min={6} max={18} step={0.5} unit="dB" tip="Lower removes more crackle; too low starts to dull transients"
      onchange={(v) => ch('Decrackle threshold', (c) => (c.decrackle.threshold_db = v))} />
    {#snippet advanced()}
      <Num label="Longest tick" value={c.decrackle.max_len_ms} min={0.05} max={0.6} step={0.01} unit="ms" digits={2} tip="Only ticks this short count as crackle"
        onchange={(v) => ch('Decrackle length', (c) => (c.decrackle.max_len_ms = v))} />
    {/snippet}
  </StageCard>

  <StageCard title="Manual repair" enabled={true} fixed badge={src?.name ?? ''} tip="For the scratch the automatic pass missed: select it on the waveform, or paint it on the spectrogram, and press Repair">
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

  <StageCard title="Declip" enabled={c.declip.enabled} tip="Rebuilds peaks that were flattened by a capture that ran too hot. Off unless clipping is found."
    onToggle={(on) => ch('Declip', (c) => (c.declip.enabled = on))} onAuto={() => auto('declip')}>
    <Num label="Clip level" value={c.declip.level_db} min={-6} max={0} step={0.05} unit="dBFS" digits={2} tip="Runs of samples at or above this level get rebuilt"
      onchange={(v) => ch('Declip level', (c) => (c.declip.level_db = v))} />
    {#snippet advanced()}
      <Num label="Longest run" value={c.declip.max_len_ms} min={0.2} max={5} step={0.1} unit="ms" tip="Longer flat runs are left alone"
        onchange={(v) => ch('Declip length', (c) => (c.declip.max_len_ms = v))} />
    {/snippet}
  </StageCard>

  <StageCard title="Denoise" enabled={c.denoise.enabled} tip="Removes steady surface hiss. Gentle by default: over-denoised vinyl sounds watery, which is worse than hiss."
    onToggle={(on) => ch('Denoise', (c) => (c.denoise.enabled = on))} onAuto={() => auto('denoise')}>
    <Num label="Reduction" value={c.denoise.reduction_db} min={0} max={20} step={0.5} unit="dB" tip="The most the hiss is turned down by"
      onchange={(v) => ch('Denoise amount', (c) => (c.denoise.reduction_db = v))} />
    <Num label="Smoothing" value={c.denoise.smoothing_pct} min={0} max={100} step={5} unit="%" digits={0} tip="Smooths the reduction over time to avoid a chirpy, watery sound"
      onchange={(v) => ch('Denoise smoothing', (c) => (c.denoise.smoothing_pct = v))} />
    {#if src?.fx.noise}
      <p class="small muted">Noise profile from {src.name}, {time(src.fx.noise.from_s, true)} to {time(src.fx.noise.to_s, true)}, about {noiseDb?.toFixed(0)} dBFS.</p>
    {:else}
      <p class="small copper">No noise profile for {src?.name ?? 'this side'}. Press Auto, or select some lead-in noise and learn from it.</p>
    {/if}
    <button class="ghost" onclick={learnFromSelection} disabled={!ui.selection} title="Learn the noise from the selected stretch instead">Learn from selection</button>
  </StageCard>

  <StageCard title="Channel tools" enabled={c.channel.enabled} tip="Balance, channel swap and azimuth (time offset between left and right from a misaligned cartridge)"
    onToggle={(on) => ch('Channel', (c) => (c.channel.enabled = on))} onAuto={() => auto('channel')}>
    <Num label="Balance" value={c.channel.balance_db} min={-6} max={6} step={0.1} unit="dB" tip="Positive moves the image right"
      onchange={(v) => ch('Balance', (c) => (c.channel.balance_db = v))} />
    <Num label="Azimuth" value={c.channel.azimuth_us} min={-500} max={500} step={1} unit="us" digits={0} tip="Delays the right channel against the left, in microseconds"
      onchange={(v) => ch('Azimuth', (c) => (c.channel.azimuth_us = v))} />
    {#snippet advanced()}
      <label class="row" title="Swap left and right"><input type="checkbox" checked={c.channel.swap} onchange={(e) => ch('Swap', (c) => (c.channel.swap = (e.target as HTMLInputElement).checked))} /> Swap channels</label>
    {/snippet}
  </StageCard>

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

  <p class="muted small">Leveling runs last; it is in the Level step.</p>
</div>
{/if}

<style>
  .gap { gap: 8px; }
  .band { padding: 6px 8px; gap: 4px; }
  select { padding: 3px 6px; }
</style>
