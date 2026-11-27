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
  import RoomTest from './components/RoomTest.svelte';
  import AdvancedDialog from './components/AdvancedDialog.svelte';
  import PresetPicker from './components/PresetPicker.svelte';
  import { updates } from './lib/updates.svelte';

  onMount(() => {
    store.start();
    return () => store.stop();
  });

  /** The meter probe is a diagnostic view: shown with `?probe` or `#probe` in the URL. */
  const probe = /probe/.test(location.search + location.hash);
  const tabs: Tab[] = $derived(store.tabList(probe));
  // A hidden tab (e.g. #reverb on a MOTU) falls back to the mixer.
  $effect(() => {
    if (!tabs.includes(store.tab)) store.tab = 'mixer';
  });
  // A reload that waited for an open panel goes ahead once it is closed.
  $effect(() => {
    if (!store.panelOpen && store.reloadWaiting) store.checkStale();
  });
  // A drop and reconnect during an install means the new version is running.
  $effect(() => updates.noteSocket(store.socketOpen));
</script>

<div class="app" inert={store.panelOpen}>
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

  <main class="view" class:simple={store.simple}>
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
</div>

<!-- Panels and toasts sit outside the (inert while a panel is open) app. -->
{#if store.roomOpen}
  <RoomTest />
{/if}

{#if store.presetsOpen}
  <PresetPicker />
{/if}

{#if store.advancedOpen}
  <AdvancedDialog />
{/if}

<!-- Errors are announced at once and stay until tapped; notices are polite and fade. -->
<div class="toasts">
  <div class="tq" role="alert">
    {#each store.c.toasts.filter((t) => !t.level) as t (t.id)}
      <button class="toast" aria-label="{t.message}. {S.dismiss}" onclick={() => store.dismissToast(t.id)}><span>{t.message}</span><span class="x" aria-hidden="true">×</span></button>
    {/each}
  </div>
  <div class="tq" aria-live="polite">
    {#each store.c.toasts.filter((t) => t.level) as t (t.id)}
      <button class="toast" class:warning={t.level === 'warning'} class:info={t.level === 'info'} onclick={() => store.dismissToast(t.id)}>{t.message}</button>
    {/each}
  </div>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
    /* The visible height, following the phone browser's toolbars. */
    height: 100dvh;
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
    padding: 6px max(14px, env(safe-area-inset-right, 0px)) 6px max(14px, env(safe-area-inset-left, 0px));
    background: #3b1212;
    color: #fecaca;
    font-size: 13px;
  }
  .view {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 8px max(10px, env(safe-area-inset-right, 0px)) 8px max(10px, env(safe-area-inset-left, 0px));
  }
  /* No bottom bar in Simple view: keep clear of the home indicator. */
  .view.simple {
    padding-bottom: calc(8px + env(safe-area-inset-bottom, 0px));
  }
  .toasts {
    position: fixed;
    right: max(12px, env(safe-area-inset-right, 0px));
    bottom: calc(44px + env(safe-area-inset-bottom, 0px));
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: min(360px, 92vw);
  }
  .tq {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    text-align: left;
    padding: 10px 14px;
    background: #3b1212;
    color: #fecaca;
    border: 1px solid var(--red);
    border-radius: var(--radius);
    cursor: pointer;
  }
  .toast .x {
    margin-left: auto;
    font-size: 18px;
    line-height: 1;
  }
  .toast.warning {
    background: var(--warn-bg);
    color: var(--warn-text);
    border-color: var(--amber);
  }
  .toast.info {
    background: var(--card);
    color: var(--text);
    border-color: var(--border);
  }
</style>
