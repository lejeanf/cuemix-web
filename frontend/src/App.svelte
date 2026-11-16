<script lang="ts">
  import { onMount } from 'svelte';
  import { store, type Tab } from './lib/store.svelte';
  import { S } from './lib/strings';
  import TopBar from './components/TopBar.svelte';
  import Mixer from './components/Mixer.svelte';
  import Preamps from './components/Preamps.svelte';
  import Output from './components/Output.svelte';
  import Reverb from './components/Reverb.svelte';
  import ControlRoom from './components/ControlRoom.svelte';
  import DspPanel from './components/DspPanel.svelte';
  import Probe from './components/Probe.svelte';
  import Remote from './components/Remote.svelte';

  onMount(() => {
    store.start();
    return () => store.stop();
  });

  /** The meter probe is a diagnostic view: shown with `?probe` or `#probe` in the URL. */
  const probe = /probe/.test(location.search + location.hash);
  const tabs: Tab[] = ['mixer', 'input', 'output', 'reverb', ...(probe ? (['probe'] as Tab[]) : [])];
</script>

<div class="app">
  <TopBar>
    {#if !store.simple}
    <div class="tabs" role="tablist" aria-label="Views">
      {#each tabs as t (t)}
        <button role="tab" aria-selected={store.tab === t} class:sel={store.tab === t} onclick={() => {
            store.tab = t;
            history.replaceState(null, '', `#${t}`);
          }}>
          {S.tabs[t]}
        </button>
      {/each}
    </div>
    {/if}
  </TopBar>
  {#if !store.socketOpen}
    <div class="offline" role="alert">{S.offlineBanner}</div>
  {/if}

  <main class="view">
    {#if store.simple}
      <Remote />
    {:else if store.tab === 'mixer'}
      <Mixer />
    {:else if store.tab === 'input'}
      <Preamps />
    {:else if store.tab === 'output'}
      <Output />
    {:else if store.tab === 'probe'}
      <Probe />
    {:else}
      <Reverb />
    {/if}
  </main>

  {#if !store.simple}
    <ControlRoom />
    <DspPanel />
  {/if}

  <div class="toasts" aria-live="polite">
    {#each store.c.toasts as t (t.id)}
      <button class="toast" onclick={() => store.dismissToast(t.id)}>{t.message}</button>
    {/each}
  </div>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .tabs {
    display: flex;
    gap: 2px;
  }
  .tabs button {
    height: 100%;
    padding: 0 12px;
    white-space: nowrap;
    background: none;
    border: 0;
    border-bottom: 3px solid transparent;
    color: var(--text-dim);
    cursor: pointer;
  }
  .tabs button.sel {
    color: var(--text);
    border-bottom-color: var(--cyan);
  }
  .offline {
    flex: none;
    padding: 6px 14px;
    background: #3b1212;
    color: #fecaca;
    font-size: 13px;
  }
  .view {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 8px 10px;
  }
  .toasts {
    position: fixed;
    right: 12px;
    bottom: 44px;
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: min(360px, 92vw);
  }
  .toast {
    text-align: left;
    padding: 10px 14px;
    background: #3b1212;
    color: #fecaca;
    border: 1px solid var(--red);
    border-radius: var(--radius);
    cursor: pointer;
  }
</style>
