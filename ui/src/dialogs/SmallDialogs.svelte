<script lang="ts">
  // Report viewer, preset name prompt and the shortcut sheet.
  import { api } from '../lib/api';
  import { ui, fail, toast } from '../lib/store.svelte';

  let report = $state('');
  let presetName = $state('');

  $effect(() => {
    if (ui.dialog === 'report' && ui.lastExport) {
      api.readReport(ui.lastExport.report).then((r) => (report = r)).catch(fail);
    }
  });

  async function savePreset() {
    if (!presetName.trim()) return;
    try {
      ui.presets = await api.presetSave(presetName.trim(), ui.scope === 'track' ? ui.selTrack : null);
      toast(`Preset "${presetName.trim()}" saved.`);
      ui.dialog = null;
      presetName = '';
    } catch (e) { fail(e); }
  }

  const keys: [string, string][] = [
    ['Space', 'Play / pause'], ['Shift+Space', 'Stop recording'], ['Click', 'Seek'], ['Drag', 'Select (on the spectrogram: paint a band)'],
    ['Ctrl+Wheel', 'Zoom'], ['Wheel', 'Scroll'], ['+ / -', 'Zoom in / out'], ['0', 'Fit the whole timeline'],
    ['Left / Right', 'Back / forward 5 s (Shift: 1 s)'], ['Home / End', 'Start / end'], ['Up / Down', 'Previous / next track'],
    ['[ / ]', 'Previous / next click'], ['A', 'A/B: original vs processed'], ['R', 'Listen to removed'], ['L', 'Loop the selection'],
    ['M', 'Mark track end'], ['T', 'Track from selection'], ['J', 'Merge with next'], ['Delete', 'Delete track'],
    ['X', 'Repair selection'], ['C', 'Capture level check'], ['Ctrl+R', 'Record a side'], ['Ctrl+Enter', 'Analyze'],
    ['1 to 5', 'Steps'], ['W', 'Waveform / spectrogram / both'], ['Ctrl+Z', 'Undo'], ['Ctrl+Shift+Z', 'Redo'], ['Ctrl+S', 'Save'],
    ['Ctrl+N', 'New project'], ['Ctrl+O', 'Open project'], ['Ctrl+I', 'Import files'], ['Ctrl+Shift+I', 'Import folder'],
    ['Ctrl+L', 'Look up tracklist'], ['Ctrl+E', 'Export'], ['Ctrl+B', 'Batch mode'], ['Esc', 'Clear selection / close'], ['?', 'This sheet'],
  ];
</script>

{#if ui.dialog === 'report' && ui.lastExport}
  <div class="modal-back" role="presentation" onkeydown={(e) => e.stopPropagation()}>
    <div class="modal wide">
      <header><h2 class="grow">Export finished</h2></header>
      <div class="body col">
        <p class="mono small selectable">{ui.lastExport.album_dir}</p>
        {#each ui.lastExport.warnings as w}<p class="copper small">{w}</p>{/each}
        <pre class="mono small report selectable">{report}</pre>
      </div>
      <footer>
        <button onclick={() => api.openFolder(ui.lastExport!.album_dir).catch(fail)}>Open folder</button>
        <button class="primary" onclick={() => (ui.dialog = null)}>Done</button>
      </footer>
    </div>
  </div>
{:else if ui.dialog === 'preset-save'}
  <div class="modal-back" role="presentation" onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') savePreset(); }}>
    <div class="modal narrow">
      <header><h2>Save preset</h2></header>
      <div class="body col">
        <!-- svelte-ignore a11y_autofocus -->
        <input type="text" placeholder="Name, e.g. Scratchy 60s mono-era stereo" bind:value={presetName} autofocus />
        <p class="muted small">Saved to ~/.config/runout/presets. Hum and declip settings are kept per record when a preset is applied.</p>
      </div>
      <footer><button onclick={() => (ui.dialog = null)}>Cancel</button><button class="primary" disabled={!presetName.trim()} onclick={savePreset}>Save</button></footer>
    </div>
  </div>
{:else if ui.dialog === 'help'}
  <div class="modal-back" role="presentation" onclick={() => (ui.dialog = null)} onkeydown={() => {}}>
    <div class="modal">
      <header><h2>Keyboard shortcuts</h2></header>
      <div class="body keys">
        {#each keys as [k, d]}<span class="mono k">{k}</span><span>{d}</span>{/each}
      </div>
    </div>
  </div>
{/if}

<style>
  .wide { width: min(900px, 94vw); }
  .narrow { width: min(460px, 92vw); }
  .report { white-space: pre-wrap; background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 10px; margin: 0; max-height: 52vh; overflow: auto; }
  .keys { display: grid; grid-template-columns: 140px 1fr 140px 1fr; gap: 5px 12px; }
  .k { color: var(--verdigris); }
</style>
