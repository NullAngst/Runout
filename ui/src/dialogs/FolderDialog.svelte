<script lang="ts">
  // Folder import: check the order, mark files Raw or Cleaned, then commit.
  import { api, type ImportItem } from '../lib/api';
  import { ui, fail } from '../lib/store.svelte';

  let items = $state<ImportItem[]>(ui.dialogData.items);
  const dir: string = ui.dialogData.dir;
  let dragFrom = $state<number | null>(null);
  let asked = $state(false);

  function setAll(raw: boolean) {
    for (const it of items) it.raw = raw;
    asked = true;
  }

  function drop(to: number) {
    if (dragFrom == null || dragFrom === to) return;
    const [x] = items.splice(dragFrom, 1);
    items.splice(to, 0, x);
    dragFrom = null;
  }

  function move(i: number, d: number) {
    const j = i + d;
    if (j < 0 || j >= items.length) return;
    [items[i], items[j]] = [items[j], items[i]];
  }

  async function commit() {
    try {
      ui.dialog = null;
      await api.folderCommit(JSON.parse(JSON.stringify(items)));
    } catch (e) {
      fail(e);
    }
  }
</script>

<div class="modal-back" role="presentation" onkeydown={(e) => e.stopPropagation()}>
  <div class="modal">
    <header class="col" style="align-items:stretch">
      <h2>Import folder</h2>
      <span class="mono small muted">{dir}</span>
    </header>
    <div class="body col">
      {#if !asked}
        <div class="card col">
          <p>Are these files raw rips, or already cleaned?</p>
          <div class="row">
            <button class="primary" onclick={() => setAll(true)} title="Copy each original into Raw/ and start the full restoration chain">All raw</button>
            <button onclick={() => setAll(false)} title="No raw copy, no restoration, no leveling: they export bit for bit with new tags">All cleaned</button>
          </div>
          <p class="muted small">You can change any single file below, and later from the track list.</p>
        </div>
      {/if}
      <p class="muted small">Order: disc, then track tag, then file name. Drag to reorder.</p>
      <div class="list">
        {#each items as it, i (it.path)}
          <div class="item row" draggable="true" class:dragging={dragFrom === i}
            ondragstart={() => (dragFrom = i)} ondragover={(e) => e.preventDefault()} ondrop={() => drop(i)} role="listitem">
            <span class="grip muted" title="Drag to reorder">::</span>
            <span class="mono small muted n">{it.disc}.{it.track ?? '-'}</span>
            <span class="grow nm">{it.tags.title ?? it.name}<span class="muted small"> {it.name}</span></span>
            <button class="ghost small" onclick={() => move(i, -1)} title="Move up">Up</button>
            <button class="ghost small" onclick={() => move(i, 1)} title="Move down">Down</button>
            <button class="small" class:on={it.raw} onclick={() => (it.raw = true)}>Raw</button>
            <button class="small" class:on={!it.raw} onclick={() => (it.raw = false)}>Cleaned</button>
          </div>
        {/each}
      </div>
    </div>
    <footer>
      <span class="muted small grow">{items.length} files, {items.filter((i) => i.raw).length} raw</span>
      <button onclick={() => (ui.dialog = null)}>Cancel</button>
      <button class="primary" onclick={commit}>Import {items.length} files</button>
    </footer>
  </div>
</div>

<style>
  .list { display: flex; flex-direction: column; gap: 3px; }
  .item { padding: 4px 6px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); gap: 6px; cursor: grab; }
  .item.dragging { opacity: 0.5; }
  .grip { font-family: var(--mono); }
  .n { width: 38px; }
  .nm { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item button { font-size: 11px; padding: 1px 7px; }
</style>
