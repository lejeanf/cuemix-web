<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import Dialog from './Dialog.svelte';

  // Simple view preset recall: a list with one Load button per preset, so a mis-tap on the
  // Preset row never rewrites the mixes by itself.
  const session = $derived(store.c.session);
  const off = $derived(!store.ready);
  const close = () => (store.presetsOpen = false);
  function load(p: string) {
    store.loadPreset(p);
    close();
  }
</script>

<Dialog title={S.simple.presetsTitle} titleId="simple-presets-title" onclose={close}>
  <p class="note">{S.simple.presetsNote}</p>
  <ul>
    {#each store.simplePresets as p (p)}
      {@const cur = p === session.active_preset}
      <li class:cur>
        <span class="nm">{p}{#if cur}<span class="tag">{S.simple.current}{#if session.modified} · {S.modified}{/if}</span>{/if}</span>
        <button class="load" disabled={off} aria-label="{S.load} {p}" onclick={() => load(p)}>{S.load}</button>
      </li>
    {/each}
  </ul>
  <div class="row">
    <button onclick={close}>{S.close}</button>
  </div>
</Dialog>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 60vh;
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 12px;
    min-height: 52px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  li.cur {
    border-color: var(--cyan);
  }
  .nm {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    font-size: 15px;
    overflow-wrap: anywhere;
  }
  .tag {
    font-size: 12px;
    color: var(--text-dim);
  }
  .load {
    flex: none;
    min-width: 72px;
    min-height: 44px;
    background: var(--card);
    color: var(--cyan);
    border: 1px solid var(--cyan);
    border-radius: 8px;
    font-weight: 600;
    cursor: pointer;
    touch-action: manipulation;
  }
  .load:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
