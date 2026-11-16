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
  let open = $state<'' | 'presets' | 'clock'>('');
  let newName = $state('');

  const session = $derived(store.c.session);
  const clock = $derived(store.state?.clock);
  const off = $derived(!store.ready);
  const nameOk = $derived(isValidPresetName(newName));
  const link = $derived(store.link);
  const linkText = $derived(
    link === 'online' ? S.online : link === 'connecting' ? S.connecting : link === 'demo' ? S.demo : S.offline,
  );

  function toggle(which: 'presets' | 'clock') {
    open = open === which ? '' : which;
    if (open === 'presets') store.listPresets();
  }
  function outside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = '';
  }
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') open = '';
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
        <label>{S.sampleRate}
          <select value={clock.sample_rate} onchange={(e) => setRate(e.currentTarget.value as SampleRate)}>
            {#each SAMPLE_RATES as r (r)}<option value={r}>{khz(r)}</option>{/each}
          </select>
        </label>
        <label>{S.clockSource}
          <select value={clock.source} onchange={(e) => setSource(e.currentTarget.value as ClockSource)}>
            {#each CLOCK_SOURCES as c (c)}<option value={c}>{c}</option>{/each}
          </select>
        </label>
      </div>
    {/if}
  </div>

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

  <button class="btn mode" aria-pressed={!store.simple} title={store.simple ? 'Show every control' : 'Volume remote for phones'} onclick={() => store.setSimple(!store.simple)}
    >{store.simple ? 'Advanced' : 'Simple'}</button
  >
  <span class="pill {link}" role="status">{linkText}</span>
</header>

<style>
  .top {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px;
    min-height: 44px;
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
    box-shadow: 0 8px 24px #000a;
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
  .pill {
    padding: 3px 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--card);
    color: var(--text-dim);
    font-size: 12px;
    white-space: nowrap;
  }
  .pill.online {
    color: var(--meter-green);
    border-color: var(--meter-green);
  }
  .pill.connecting {
    color: var(--amber);
    border-color: var(--amber);
  }
  .pill.offline {
    color: var(--red);
    border-color: var(--red);
  }
  /* Phones: tabs get their own scrolling row under the buttons; the clock
     (rarely changed) is left to larger screens. */
  @media (max-width: 640px) {
    .top {
      flex-wrap: wrap;
      gap: 6px;
      padding: 4px 8px 0;
    }
    .brand {
      flex: 1;
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
