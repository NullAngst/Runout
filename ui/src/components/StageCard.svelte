<script lang="ts">
  // One restoration stage: on/off switch, Auto button, main controls, Advanced fold.
  import type { Snippet } from 'svelte';
  let { title, enabled, tip, onToggle, onAuto, children, advanced, badge = '', fixed = false }: {
    title: string; enabled: boolean; tip: string; onToggle?: (on: boolean) => void; onAuto?: () => void;
    children?: Snippet; advanced?: Snippet; badge?: string; fixed?: boolean;
  } = $props();
  let open = $state(false);
</script>

<div class="card stage" class:off={!enabled}>
  <div class="row head">
    {#if !fixed}
      <button class="switch" class:on={enabled} title={enabled ? `Turn ${title} off` : `Turn ${title} on`} aria-label="toggle"
        onclick={() => onToggle?.(!enabled)}></button>
    {/if}
    <h3 title={tip}>{title}</h3>
    {#if badge}<span class="tag accent">{badge}</span>{/if}
    <span class="grow"></span>
    {#if onAuto}<button class="ghost" title="Analyze the audio and pick starting values" onclick={onAuto}>Auto</button>{/if}
  </div>
  <div class="controls col">
    {@render children?.()}
    {#if advanced}
      <button class="ghost adv" onclick={() => (open = !open)}>{open ? 'Hide advanced' : 'Advanced'}</button>
      {#if open}<div class="col">{@render advanced()}</div>{/if}
    {/if}
  </div>
</div>

<style>
  .stage { display: flex; flex-direction: column; gap: 8px; }
  .head h3 { font-size: 13.5px; }
  .adv { align-self: flex-start; font-size: 11.5px; padding: 1px 4px; }
</style>
