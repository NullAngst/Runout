<script lang="ts">
  // Medium cards grouped by family, then the condition preset and deck under
  // them. Used by Welcome (new project) and the Medium dialog.
  import { api, type DeckProfile, type MediumPreset, type Preset } from '../lib/api';
  import { ui, fail } from '../lib/store.svelte';
  import { speed } from '../lib/format';

  let { medium = $bindable(), condition = $bindable(), deck = $bindable(), keep = false }: {
    medium: string; condition: string; deck: string;
    /** Keep the condition and deck passed in until another medium is picked. */
    keep?: boolean;
  } = $props();

  let presets = $state<Preset[]>([]);
  $effect(() => {
    api.presets().then((p) => (presets = p)).catch(fail);
  });

  const groups = $derived.by(() => {
    const m = ui.media;
    if (!m) return [] as [string, MediumPreset[]][];
    return m.families
      .map(([f, label]) => [label, m.media.filter((x) => x.family === f)] as [string, MediumPreset[]])
      .filter(([, list]) => list.length);
  });
  const sel = $derived(ui.media?.media.find((m) => m.id === medium) ?? null);
  const conditions = $derived(presets.filter((p) => p.family === (sel?.family ?? 'vinyl')));
  const decks = $derived((ui.media?.decks ?? []).filter((d: DeckProfile) => d.family === sel?.family));

  // picking a medium resets condition and deck to that medium's defaults
  // svelte-ignore state_referenced_locally
  let lastPicked = keep ? medium : '';
  $effect(() => {
    const s = sel;
    if (!s || s.id === lastPicked) return;
    lastPicked = s.id;
    condition = s.default_condition ?? '';
    deck = ui.info?.settings.deck_by_family[s.family] ?? '';
  });

  function speedLabel(m: MediumPreset) {
    if (m.speed.unit === 'none') return m.capture.path === 'spdif' ? 'S/PDIF' : '';
    return speed(m.speed.nominal, m.speed.unit);
  }
</script>

<div class="col picker">
  {#each groups as [label, list] (label)}
    <div class="col fam">
      <span class="muted small">{label}</span>
      <div class="cards">
        {#each list as m (m.id)}
          <button class="mcard" class:on={m.id === medium} onclick={() => (medium = m.id)} title={m.notes || m.name}>
            <span class="name">{m.name}</span>
            <span class="small muted">{speedLabel(m)}{m.builtin ? '' : ' (yours)'}</span>
          </button>
        {/each}
      </div>
    </div>
  {/each}

  {#if sel}
    <div class="row wrap under">
      <label class="row grow"><span class="lab">Condition</span>
        <select class="grow" bind:value={condition} title="How worn it is. Sets how hard the restoration stages work.">
          <option value="">{sel.family === 'vinyl' ? 'Default settings' : 'None'}</option>
          {#each conditions as p (p.id)}<option value={p.id}>{p.name}{p.builtin ? '' : ' (yours)'}</option>{/each}
        </select></label>
      <label class="row grow"><span class="lab">Deck</span>
        <select class="grow" bind:value={deck} disabled={!decks.length} title="Your playback machine's known faults: speed, azimuth, balance. Make one in Settings.">
          <option value="">{decks.length ? 'No deck profile' : 'No deck profiles yet'}</option>
          {#each decks as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
        </select></label>
    </div>
  {/if}
</div>

<style>
  .picker { gap: 10px; }
  .fam { gap: 4px; }
  .cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 6px; }
  .mcard { display: flex; flex-direction: column; align-items: flex-start; gap: 1px; padding: 7px 9px; text-align: left; white-space: normal; background: var(--bg); }
  .mcard.on { border-color: var(--accent); background: var(--accent-wash); }
  .mcard .name { font-weight: 600; }
  .under { gap: 10px; }
  .lab { width: 70px; }
</style>
