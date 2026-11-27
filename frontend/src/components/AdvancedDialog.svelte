<script lang="ts">
  import Dialog from './Dialog.svelte';
  import SpeakerPlug from './SpeakerPlug.svelte';
  import OscPanel from './OscPanel.svelte';
  import VersionPanel from './VersionPanel.svelte';
  import { store } from '../lib/store.svelte';
  import { admin } from '../lib/admin.svelte';
  import { S } from '../lib/strings';
  import { ADVANCED_TABS, nextTab, type AdvancedTab } from '../lib/advanced';

  // One dialog behind the ⋯ button. All three panels stay mounted (the inactive ones hidden),
  // so edits survive a tab switch; the dialog refuses to close over unsaved edits like each
  // panel did on its own (Save or Cancel decides).
  let plugDirty = $state(false);
  let oscDirty = $state(false);
  const dirty = $derived(plugDirty || oscDirty);
  const tab = $derived(store.advancedTab);

  function close() {
    store.advancedOpen = false;
  }
  function select(t: AdvancedTab, focus = false) {
    store.setAdvancedTab(t);
    if (focus) queueMicrotask(() => document.getElementById(`adv-tab-${t}`)?.focus());
  }
  function key(e: KeyboardEvent) {
    const to = nextTab(tab, e.key);
    if (to) {
      e.preventDefault();
      select(to, true);
    }
  }
</script>

<Dialog title={S.advanced.title} titleId="advanced-title" {dirty} onclose={close}>
  <div class="tabs" role="tablist" aria-label={S.advanced.tabsLabel} tabindex="-1" onkeydown={key}>
    {#each ADVANCED_TABS as t (t)}
      <button
        type="button"
        role="tab"
        id="adv-tab-{t}"
        aria-selected={tab === t}
        aria-controls="adv-panel-{t}"
        tabindex={tab === t ? 0 : -1}
        class:sel={tab === t}
        onclick={() => select(t)}
      >
        <span class="long">{S.advanced.tabs[t]}</span><span class="short" aria-hidden="true">{S.advanced.short[t]}</span>
      </button>
    {/each}
  </div>

  {#if admin.message}<p class="admin-msg" role="status">{admin.message}</p>{/if}
  {#if admin.locked}
    <div class="admin-locked" role="note">
      <strong>{S.admin.lockedTitle}</strong>
      <p>{S.admin.locked}</p>
    </div>
  {/if}

  <!-- A disabled fieldset makes every control of the panels read-only at once. -->
  <fieldset class="panels" disabled={admin.locked}>
    <div role="tabpanel" id="adv-panel-plug" aria-labelledby="adv-tab-plug" hidden={tab !== 'plug'}>
      <SpeakerPlug onDirty={(d) => (plugDirty = d)} />
    </div>
    <div role="tabpanel" id="adv-panel-osc" aria-labelledby="adv-tab-osc" hidden={tab !== 'osc'}>
      <OscPanel onDirty={(d) => (oscDirty = d)} />
    </div>
    <div role="tabpanel" id="adv-panel-version" aria-labelledby="adv-tab-version" hidden={tab !== 'version'}>
      <VersionPanel />
    </div>
  </fieldset>

  {#if admin.info?.required && admin.info.paired}
    <p class="admin-paired">{S.admin.pairedNote} <button type="button" class="link" onclick={() => admin.unpair()}>{S.admin.unpair}</button></p>
  {/if}
</Dialog>

<style>
  .tabs {
    display: flex;
    gap: 4px;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    flex: 1;
    min-width: 0;
    min-height: 44px;
    padding: 0 8px;
    background: none;
    color: var(--text-dim);
    border: 0;
    border-bottom: 2px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tabs button.sel {
    color: var(--cyan);
    border-bottom-color: var(--cyan);
  }
  .tabs button:focus-visible {
    outline: 2px solid var(--cyan);
    outline-offset: -2px;
  }
  .short {
    display: none;
  }
  @media (max-width: 420px) {
    .long {
      display: none;
    }
    .short {
      display: inline;
    }
  }
  [role='tabpanel'][hidden] {
    display: none;
  }
  .panels {
    border: 0;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  .panels:disabled {
    opacity: 0.6;
  }
  .admin-locked {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    margin-top: 8px;
  }
  .admin-locked p,
  .admin-msg,
  .admin-paired {
    margin: 4px 0 0;
    color: var(--text-dim);
    font-size: 0.9em;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--cyan);
    text-decoration: underline;
    cursor: pointer;
    font: inherit;
  }
  :global(.panel) {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
</style>
