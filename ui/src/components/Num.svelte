<script lang="ts">
  // A labelled numeric control: "Threshold: 13.0 dB" with a slider.
  let { label, value, min, max, step = 1, unit = '', digits = 1, tip = '', disabled = false, onchange }: {
    label: string; value: number; min: number; max: number; step?: number; unit?: string; digits?: number;
    tip?: string; disabled?: boolean; onchange: (v: number) => void;
  } = $props();

  let editing = $state(false);
  let text = $state('');

  function commit() {
    editing = false;
    const v = parseFloat(text);
    if (isFinite(v)) onchange(Math.min(max, Math.max(min, v)));
  }
</script>

<div class="num" title={tip}>
  <div class="row top">
    <span class="lab">{label}:</span>
    {#if editing}
      <input type="text" class="mono val-in" bind:value={text} onblur={commit}
        onkeydown={(e) => { if (e.key === 'Enter') commit(); if (e.key === 'Escape') editing = false; e.stopPropagation(); }} />
    {:else}
      <button class="ghost val mono" {disabled} onclick={() => { text = value.toFixed(digits); editing = true; }}>{value.toFixed(digits)}{unit ? ' ' + unit : ''}</button>
    {/if}
  </div>
  <input type="range" {min} {max} {step} {value} {disabled}
    oninput={(e) => onchange(parseFloat((e.target as HTMLInputElement).value))}
    onkeydown={(e) => e.stopPropagation()} />
</div>

<style>
  .num { display: flex; flex-direction: column; gap: 1px; }
  .top { justify-content: space-between; gap: 4px; }
  .lab { color: var(--muted); font-size: 12px; }
  .val { padding: 0 4px; color: var(--text); font-size: 12px; }
  .val-in { width: 70px; padding: 1px 4px; font-size: 12px; }
  input[type=range] { height: 16px; margin: 0; }
</style>
