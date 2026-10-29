<script lang="ts">
  // Theme, deck profiles and library root. The theme switches live.
  import { open } from '@tauri-apps/plugin-dialog';
  import { api, type DeckProfile, type Family } from '../lib/api';
  import { ui, fail, toast, applyTheme, THEMES } from '../lib/store.svelte';
  import Num from '../components/Num.svelte';

  // svelte-ignore state_referenced_locally
  let root = $state(ui.info?.settings.library_root ?? '~/Music/Runout');
  let editing = $state<DeckProfile | null>(null);
  const decks = $derived(ui.media?.decks ?? []);
  const families = $derived((ui.media?.families ?? []).filter(([f]) => f !== 'digital' && f !== 'generic'));

  async function theme(id: string) {
    applyTheme(id);
    try {
      const s = await api.setTheme(id);
      if (ui.info) ui.info.settings = s;
    } catch (e) { fail(e); }
  }

  async function saveRoot() {
    if (!ui.info) return;
    try {
      ui.info.settings = await api.settingsSet({ ...ui.info.settings, library_root: root.trim() });
      toast('Library folder saved. New projects export there.');
    } catch (e) { fail(e); }
  }

  async function pickRoot() {
    const d = await open({ directory: true });
    if (d) {
      root = String(d);
      saveRoot();
    }
  }

  function blank(): DeckProfile {
    const f = (ui.view?.project.medium.preset.family ?? 'cassette') as Family;
    return { id: '', name: '', family: f === 'digital' || f === 'generic' ? 'cassette' : f, speed_pct: 0, azimuth_us: 0, balance_db: 0, eq_low_db: 0, eq_high_db: 0 };
  }

  async function measure() {
    try {
      const d = await api.deckMeasure();
      editing = { ...(editing ?? blank()), speed_pct: d.speed_pct, azimuth_us: d.azimuth_us, balance_db: d.balance_db, family: d.family };
      toast('Measured from the open project. Check the numbers, name the deck and save.');
    } catch (e) { fail(e); }
  }

  async function saveDeck() {
    if (!editing) return;
    try {
      ui.media = await api.deckSave(editing);
      toast(`Deck "${editing.name}" saved.`);
      editing = null;
    } catch (e) { fail(e); }
  }

  async function delDeck(id: string) {
    try { ui.media = await api.deckDelete(id); } catch (e) { fail(e); }
  }
</script>

<div class="modal-back" role="presentation" onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Escape') ui.dialog = null; }}>
  <div class="modal">
    <header><h2 class="grow">Settings</h2></header>
    <div class="body col">
      <h3>Theme</h3>
      <div class="row wrap">
        {#each THEMES as [id, name]}
          <button class="swatch" class:on={ui.theme === id} onclick={() => theme(id)}>{name}</button>
        {/each}
      </div>

      <div class="sep"></div>
      <h3>Library folder</h3>
      <div class="row">
        <input type="text" class="grow mono" bind:value={root} onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') saveRoot(); }} onblur={saveRoot} />
        <button class="ghost" onclick={pickRoot}>Browse</button>
      </div>
      <p class="muted small">Where new projects export. Albums go to {root}/Artist/Album Artist - Album, mixtapes to {root}/Mixtapes, recordings to {root}/Recordings.</p>

      <div class="sep"></div>
      <div class="row"><h3 class="grow">Deck profiles</h3>
        <button onclick={() => (editing = blank())}>New deck</button></div>
      <p class="muted small">What your playback machine does wrong: speed, azimuth, balance and tone. Picked with the medium, applied to every side. Measure fills one in from the open project: capture a test tape with tones if you have one, otherwise a tape or record you know well, run Analyze, then press Measure.</p>
      {#each decks as d (d.id)}
        <div class="row small deck">
          <span class="grow">{d.name}</span>
          <span class="muted mono">{d.family}, {d.speed_pct >= 0 ? '+' : ''}{d.speed_pct.toFixed(2)}%, {d.azimuth_us.toFixed(1)} us, {d.balance_db.toFixed(1)} dB</span>
          <button class="ghost" onclick={() => (editing = { ...d })}>Edit</button>
          <button class="ghost" onclick={() => delDeck(d.id)}>Delete</button>
        </div>
      {/each}
      {#if !decks.length && !editing}<p class="muted small">No decks yet.</p>{/if}

      {#if editing}
        <div class="card col">
          <div class="row">
            <input type="text" class="grow" placeholder="Name, e.g. Onkyo TA-RW" bind:value={editing.name} onkeydown={(e) => e.stopPropagation()} />
            <select bind:value={editing.family}>
              {#each families as [f, label]}<option value={f}>{label}</option>{/each}
            </select>
          </div>
          <Num label="Speed error" value={editing.speed_pct} min={-6} max={6} step={0.01} unit="%" digits={2} tip="Positive: the deck runs fast" onchange={(v) => editing && (editing.speed_pct = v)} />
          <Num label="Azimuth" value={editing.azimuth_us} min={-1000} max={1000} step={0.5} unit="us" tip="Right channel against the left" onchange={(v) => editing && (editing.azimuth_us = v)} />
          <Num label="Balance" value={editing.balance_db} min={-6} max={6} step={0.1} unit="dB" tip="Positive moves the image right" onchange={(v) => editing && (editing.balance_db = v)} />
          <Num label="Low shelf" value={editing.eq_low_db} min={-12} max={12} step={0.5} unit="dB" tip="Bass trim" onchange={(v) => editing && (editing.eq_low_db = v)} />
          <Num label="High shelf" value={editing.eq_high_db} min={-12} max={12} step={0.5} unit="dB" tip="Treble trim, for a worn head" onchange={(v) => editing && (editing.eq_high_db = v)} />
          <div class="row">
            <button disabled={!ui.view?.project.sources.length} onclick={measure} title="Fill speed, azimuth and balance from the open project">Measure</button>
            <span class="grow"></span>
            <button onclick={() => (editing = null)}>Cancel</button>
            <button class="primary" disabled={!editing.name.trim()} onclick={saveDeck}>Save deck</button>
          </div>
        </div>
      {/if}
    </div>
    <footer><button class="primary" onclick={() => (ui.dialog = null)}>Done</button></footer>
  </div>
</div>

<style>
  .swatch { min-width: 110px; }
  .deck { padding: 2px 0; }
</style>
