<script lang="ts">
  import { onMount } from 'svelte';
  import { store, type Tab } from './lib/store.svelte';
  import { S } from './lib/strings';
  import TopBar from './components/TopBar.svelte';
  import Mixer from './components/Mixer.svelte';
  import Preamps from './components/Preamps.svelte';
  import Routing from './components/Routing.svelte';
  import Reverb from './components/Reverb.svelte';
  import ControlRoom from './components/ControlRoom.svelte';
  import DspPanel from './components/DspPanel.svelte';

  onMount(() => {
    store.start();
    return () => store.stop();
  });

  const tabs: Tab[] = ['mixer', 'preamps', 'routing', 'reverb'];
</script>

<div class="app">
  <TopBar />
  <div class="tabs" role="tablist" aria-label="Views">
    {#each tabs as t (t)}
      <button role="tab" aria-selected={store.tab === t} class:sel={store.tab === t} onclick={() => (store.tab = t)}>
        {S.tabs[t]}
      </button>
    {/each}
  </div>
  {#if !store.socketOpen}
    <div class="offline" role="alert">{S.offlineBanner}</div>
  {/if}

  <main class="view">
    {#if store.tab === 'mixer'}
      <Mixer />
    {:else if store.tab === 'preamps'}
      <Preamps />
    {:else if store.tab === 'routing'}
      <Routing />
    {:else}
      <Reverb />
    {/if}
  </main>

  <ControlRoom />
  <DspPanel />

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
    flex: none;
    display: flex;
    gap: 4px;
    padding: 6px 14px 0;
    background: var(--bg-alt);
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
  }
  .tabs button {
    min-height: 40px;
    padding: 0 18px;
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
    padding: 12px 14px;
  }
  .toasts {
    position: fixed;
    right: 12px;
    bottom: 76px;
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
