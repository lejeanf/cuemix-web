<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { CLOCK_SOURCES, SAMPLE_RATES, PRESET_NAME_RE, type ClockSource, type SampleRate } from '../lib/protocol';
  import { isValidPresetName } from '../lib/validate';
  import { PRESET_SCHEMA_VERSION, exportFileName, presetNameFromFile } from '../lib/presetFile';
  import type { Snippet } from 'svelte';

  /** The view tabs, shown in the same bar. */
  let { children }: { children?: Snippet } = $props();

  let root: HTMLElement;
  let moreBtn = $state<HTMLButtonElement>();
  let menuEl = $state<HTMLElement>();
  let open = $state<'' | 'presets' | 'clock' | 'menu'>('');
  let newName = $state('');

  const session = $derived(store.c.session);
  const clock = $derived(store.state?.clock);
  const off = $derived(!store.ready);
  const nameOk = $derived(isValidPresetName(newName));
  const link = $derived(store.link);
  const linkText = $derived(
    link === 'online' ? S.online : link === 'connecting' ? S.connecting : link === 'demo' ? S.demo : S.offline,
  );

  // The status is a small dot; its text shows only on hover or focus, or for
  // a few seconds after a tap (phones have no hover).
  let showStatus = $state(false);
  let statusTimer: ReturnType<typeof setTimeout> | undefined;
  function flashStatus(ms = 3000) {
    showStatus = true;
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => (showStatus = false), ms);
  }
  $effect(() => () => clearTimeout(statusTimer));

  function toggle(which: 'presets' | 'clock' | 'menu') {
    open = open === which ? '' : which;
    if (open === 'menu') queueMicrotask(() => menuEl?.querySelector<HTMLElement>('[role=menuitem]')?.focus());
    if (open === 'presets') store.listPresets();
  }
  function outside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = '';
  }
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      if (open === 'menu') moreBtn?.focus();
      open = '';
    }
  }
  /** Arrow keys, Home and End move between the menu items; Tab closes the menu. */
  function menuKey(e: KeyboardEvent) {
    const items = [...(menuEl?.querySelectorAll<HTMLElement>('[role=menuitem]') ?? [])];
    if (!items.length) return;
    const i = items.indexOf(document.activeElement as HTMLElement);
    const to =
      e.key === 'ArrowDown' ? items[(i + 1) % items.length]
      : e.key === 'ArrowUp' ? items[(i <= 0 ? items.length : i) - 1]
      : e.key === 'Home' ? items[0]
      : e.key === 'End' ? items[items.length - 1]
      : null;
    if (to) {
      e.preventDefault();
      to.focus();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      open = '';
      moreBtn?.focus();
    }
  }
  function setRate(rate: SampleRate) {
    store.set({ param: 'sample_rate', rate }, true);
  }
  function setSource(source: ClockSource) {
    store.set({ param: 'clock_source', source }, true);
  }
  function saveAs() {
    if (!nameOk) return;
    store.savePreset(newName);
    newName = '';
  }
  const khz = (r: string) => `${Number(r) / 1000} kHz`;

  let fileInput = $state<HTMLInputElement>();
  /** Downloads the live state in the preset file format. */
  function exportFile() {
    const st = store.state;
    if (!st) return;
    const name = session.active_preset ?? 'cuemix-settings';
    const data = {
      schema_version: PRESET_SCHEMA_VERSION,
      name,
      exported_at: new Date().toISOString(),
      device: session.device ? { model: session.device.model, firmware: session.device.firmware } : null,
      state: st,
    };
    const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 1)], { type: 'application/json' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = exportFileName(name, new Date());
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function importFile(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    try {
      // Compact it: the WebSocket accepts up to 256 KiB per message.
      const text = JSON.stringify(JSON.parse(await file.text()));
      store.importPreset(presetNameFromFile(file.name), text);
      open = '';
    } catch {
      store.c.toasts.push({ id: ++store.c.toastSeq, message: `${file.name} is not a valid settings file` });
    }
  }
</script>

<svelte:window onpointerdown={outside} onkeydown={key} />

<header class="top" bind:this={root}>
  <span class="brand">{S.appName}</span>
  {#if !store.simple}<nav class="tabs">{@render children?.()}</nav>{/if}

  <div class="pw">
    <button class="btn clock" aria-haspopup="dialog" aria-expanded={open === 'clock'} disabled={off} onclick={() => toggle('clock')}>
      {#if clock}{khz(clock.sample_rate)} · {clock.source}{:else}{S.clock}{/if}
    </button>
    {#if open === 'clock' && clock}
      <div class="pop" role="dialog" aria-label={S.clock}>
        {#if store.hint('sample_rate')}<p class="hint">{store.hint('sample_rate')}</p>{/if}
        <label data-unmapped={store.unmapped('sample_rate') ? '' : undefined}>{S.sampleRate}
          <select disabled={store.locked('sample_rate')} value={clock.sample_rate} onchange={(e) => setRate(e.currentTarget.value as SampleRate)}>
            {#each SAMPLE_RATES as r (r)}<option value={r}>{khz(r)}</option>{/each}
          </select>
        </label>
        <label data-unmapped={store.unmapped('clock_source') ? '' : undefined}>{S.clockSource}
          <select disabled={store.locked('clock_source')} value={clock.source} onchange={(e) => setSource(e.currentTarget.value as ClockSource)}>
            {#each CLOCK_SOURCES as c (c)}<option value={c}>{c}</option>{/each}
          </select>
        </label>
      </div>
    {/if}
  </div>

  {#if !store.simple}
  <div class="pw">
    <button class="btn" aria-haspopup="dialog" aria-expanded={open === 'presets'} disabled={off} title="{S.presetLabel}: {session.active_preset ?? S.noPreset}{session.modified ? ` (${S.modified})` : ''}"
      onclick={() => toggle('presets')}
      >{session.active_preset ?? S.presets}{#if session.modified}<span class="modified" aria-label={S.modified}>●</span>{/if}</button
    >
    {#if open === 'presets'}
      <div class="pop wide" role="dialog" aria-label={S.presets}>
        <p class="note">{S.presetRecallNote}</p>
        {#if store.c.presets.length === 0}
          <p class="note">{S.noPresets}</p>
        {:else}
          <ul>
            {#each store.c.presets as p (p)}
              <li class:active={p === session.active_preset}>
                <span>{p}</span>
                <button class="btn sm" aria-label="{S.load} {p}" onclick={() => { store.loadPreset(p); open = ''; }}>{S.load}</button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if session.active_preset}
          <button class="btn" onclick={() => store.savePreset(session.active_preset!)}>{S.save} “{session.active_preset}”</button>
        {/if}
        <form onsubmit={(e) => { e.preventDefault(); saveAs(); }}>
          <input type="text" bind:value={newName} placeholder={S.presetName} maxlength="64" aria-label={S.presetName} aria-invalid={newName !== '' && !nameOk} pattern={PRESET_NAME_RE.source} />
          <button class="btn" type="submit" disabled={!nameOk}>{S.saveAs}</button>
        </form>
        {#if newName !== '' && !nameOk}<small class="err">{S.presetNameInvalid}</small>{/if}
        <div class="files">
          <button class="btn" onclick={exportFile}>{S.exportFile}</button>
          <button class="btn" onclick={() => fileInput?.click()}>{S.importFile}</button>
          <input bind:this={fileInput} type="file" accept=".json,application/json" hidden onchange={importFile} />
        </div>
        <p class="note">{S.fileNote}</p>
      </div>
    {/if}
  </div>
  <div class="pw">
    <button class="btn more" data-more-menu bind:this={moreBtn} aria-haspopup="menu" aria-expanded={open === 'menu'} aria-label={S.menu} title={S.menu} onclick={() => toggle('menu')}>⋯</button>
    {#if open === 'menu'}
      <div class="pop menu" role="menu" aria-label={S.menu} tabindex="-1" bind:this={menuEl} onkeydown={menuKey}>
        <button class="item" role="menuitem" onclick={() => { open = ''; store.plugOpen = true; }}>{S.plugMenuItem}</button>
        <button class="item" role="menuitem" onclick={() => { open = ''; store.oscOpen = true; }}>{S.oscMenuItem}</button>
        <button class="item" role="menuitem" onclick={() => { open = ''; store.versionOpen = true; }}>{S.versionMenuItem}</button>
      </div>
    {/if}
  </div>
  {/if}

  {#if !store.embed}
  <button class="btn mode" aria-pressed={!store.simple} title={store.simple ? 'Show every control' : 'Volume remote for phones'} onclick={() => store.setSimple(!store.simple)}
    >{store.simple ? 'Advanced' : 'Simple'}</button
  >
  <span class="sr" role="status">{linkText}</span>
  <button class="status {link}" class:show={showStatus} aria-label="Connection: {linkText}" title={linkText} onclick={() => flashStatus()}>
    <span class="dot" aria-hidden="true"></span><span class="txt" aria-hidden="true">{linkText}</span>
  </button>
  {/if}
</header>

<style>
  .top {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    /* Safe areas (notch, status bar, landscape sides) are 0 on desktop. */
    padding: env(safe-area-inset-top, 0px) max(10px, env(safe-area-inset-right, 0px)) 0 max(10px, env(safe-area-inset-left, 0px));
    min-height: calc(44px + env(safe-area-inset-top, 0px));
    background: var(--bg-alt);
    border-bottom: 1px solid var(--border);
    position: relative;
    z-index: 20;
  }
  .brand {
    font-weight: 700;
    white-space: nowrap;
  }
  .tabs {
    flex: 1;
    min-width: 0;
    align-self: stretch;
    display: flex;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .modified {
    margin-left: 6px;
    color: var(--amber);
  }
  .pw {
    position: relative;
  }
  .btn {
    min-height: 32px;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 12px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .btn.more {
    min-width: 34px;
    padding: 0 8px;
    font-size: 18px;
    line-height: 1;
  }
  .pop.menu {
    min-width: 160px;
    padding: 4px;
    gap: 2px;
  }
  .item {
    min-height: 44px;
    padding: 0 10px;
    text-align: left;
    background: none;
    border: 0;
    border-radius: 4px;
    color: var(--text);
    cursor: pointer;
  }
  .item:hover,
  .item:focus-visible {
    background: var(--bg-alt);
  }
  .btn.sm {
    min-height: 30px;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .pop {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    min-width: 220px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow-pop);
    z-index: 30;
  }
  .hint {
    margin: 0;
    max-width: 220px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .pop.wide {
    width: min(340px, 92vw);
  }
  .pop label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  select,
  input[type='text'] {
    min-height: 34px;
    background: #0d0e11;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 8px;
    font: inherit;
  }
  .note {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 2px 4px 2px 8px;
    border-radius: 4px;
  }
  li.active {
    background: #232733;
  }
  form {
    display: flex;
    gap: 6px;
  }
  form input {
    flex: 1;
    min-width: 0;
  }
  .files {
    display: flex;
    gap: 6px;
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .err {
    color: var(--red);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
  .status {
    --c: var(--text-dim);
    position: relative;
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--c);
    font-size: 12px;
    white-space: nowrap;
    cursor: default;
    touch-action: manipulation;
  }
  .status.online {
    --c: var(--meter-green);
  }
  .status.connecting {
    --c: var(--amber);
  }
  .status.demo {
    --c: var(--red);
  }
  .dot {
    flex: none;
    width: 13px;
    height: 13px;
    border-radius: 50%;
    background: var(--c);
  }
  .status.connecting .dot {
    animation: pulse 1s ease-in-out infinite alternate;
  }
  @keyframes pulse {
    to {
      opacity: 0.3;
    }
  }
  /* The label floats over the bar just left of the dot, so showing it never
     moves or wraps anything. */
  .txt {
    position: absolute;
    right: calc(100% + 4px);
    top: 50%;
    transform: translateY(-50%);
    z-index: 30;
    padding: 3px 10px;
    border: 1px solid var(--c);
    border-radius: 999px;
    background: var(--bg-alt);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.2s;
  }
  .status:hover .txt,
  .status:focus-visible .txt,
  .status.show .txt {
    opacity: 1;
  }
  /* Phones: tabs get their own scrolling row under the buttons; the clock
     (rarely changed) is left to larger screens. */
  @media (max-width: 640px) {
    .top {
      flex-wrap: wrap;
      gap: 6px;
      padding: env(safe-area-inset-top, 0px) max(8px, env(safe-area-inset-right, 0px)) 0 max(8px, env(safe-area-inset-left, 0px));
    }
    /* The first row is as tall in Simple and Advanced views. */
    .brand {
      flex: 1;
      display: flex;
      align-items: center;
      min-height: 44px;
    }
    .tabs {
      order: 10;
      flex-basis: 100%;
      min-height: 40px;
    }
    .clock {
      display: none;
    }
    .btn {
      min-height: 30px;
      padding: 0 10px;
    }
  }
</style>
