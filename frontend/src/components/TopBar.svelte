<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { CLOCK_SOURCES, SAMPLE_RATES, PRESET_NAME_RE, type ClockSource, type SampleRate } from '../lib/protocol';
  import { isValidPresetName } from '../lib/validate';
  import { PRESET_SCHEMA_VERSION, exportFileName, presetNameFromFile } from '../lib/presetFile';
  import type { Snippet } from 'svelte';

  /** The view tabs, shown in the same bar. `phoneTabs`: also on phones (else a bottom bar has them). */
  let { children, phoneTabs = true }: { children?: Snippet; phoneTabs?: boolean } = $props();

  let root: HTMLElement;
  let moreBtn = $state<HTMLButtonElement>();
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

  function toggle(which: 'presets' | 'clock') {
    open = open === which ? '' : which;
    if (open === 'presets') store.listPresets();
  }
  function outside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = '';
  }
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      open = '';
    }
  }
  function setRate(rate: SampleRate) {
    store.set({ param: 'sample_rate', rate }, true);
  }
  function setSource(source: ClockSource) {
    store.set({ param: 'clock_source', source }, true);
  }
  // Preset management: rename and delete per row, and a confirm before any overwrite.
  let renaming = $state<string | null>(null);
  let renameTo = $state('');
  let deleting = $state<string | null>(null);
  let replacing = $state<string | null>(null);
  const hidden = $derived(new Set(store.c.presetsHidden));
  const renameOk = $derived(isValidPresetName(renameTo) && !store.c.presets.includes(renameTo));
  function startRename(p: string) {
    renaming = p;
    renameTo = p;
    deleting = null;
  }
  function doRename() {
    if (!renaming || !renameOk) return;
    store.renamePreset(renaming, renameTo);
    renaming = null;
  }
  function doDelete() {
    if (deleting) store.deletePreset(deleting);
    deleting = null;
  }
  function doReplace() {
    if (replacing) store.savePreset(replacing);
    if (replacing === newName) newName = '';
    replacing = null;
  }
  function saveAs() {
    if (!nameOk) return;
    // Saving over an existing preset asks first.
    if (store.c.presets.includes(newName)) {
      replacing = newName;
      return;
    }
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
      // The message also carries the name and JSON escapes: measure it as sent (limit 256 KiB).
      const sent = new TextEncoder().encode(JSON.stringify({ cmd: 'import_preset', name: presetNameFromFile(file.name), text })).length;
      if (sent > 250 * 1024) throw new Error('too large');
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
  {#if !store.simple}<nav class="tabs" class:wideonly={!phoneTabs}>{@render children?.()}</nav>{:else}<span class="grow"></span>{/if}

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
                {#if renaming === p}
                  <form class="inl" onsubmit={(e) => { e.preventDefault(); doRename(); }}>
                    <!-- svelte-ignore a11y_autofocus -->
                    <input type="text" bind:value={renameTo} maxlength="64" aria-label="{S.presetUi.renameTo} ({p})" aria-invalid={renameTo !== p && !renameOk} autofocus />
                    <button class="btn sm" type="submit" disabled={!renameOk}>{S.presetUi.renameBtn}</button>
                    <button class="btn sm" type="button" onclick={() => (renaming = null)}>{S.cancel}</button>
                  </form>
                  {#if renameTo !== p && store.c.presets.includes(renameTo)}<small class="err">{S.presetUi.taken}</small>{:else if renameTo !== '' && !isValidPresetName(renameTo)}<small class="err">{S.presetNameInvalid}</small>{/if}
                {:else if deleting === p}
                  <div class="ask" role="alert">
                    <span>{S.presetUi.delAsk(p)}</span>
                    <button class="btn sm danger" onclick={doDelete}>{S.presetUi.delBtn}</button>
                    <button class="btn sm" onclick={() => (deleting = null)}>{S.cancel}</button>
                  </div>
                {:else}
                  <span class="pn">{p}{#if hidden.has(p)}<small class="tag">{S.presetUi.hiddenTag}</small>{/if}</span>
                  <button class="btn sm" aria-label="{S.load} {p}" onclick={() => { store.loadPreset(p); open = ''; }}>{S.load}</button>
                  <button class="ic" aria-pressed={!hidden.has(p)} aria-label={S.presetUi.inSimple(p)} title={S.presetUi.inSimple(p)} onclick={() => store.setPresetInSimple(p, hidden.has(p))}>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="7" y="3" width="10" height="18" rx="2" />{#if hidden.has(p)}<path d="M4 4l16 16" />{/if}</svg>
                  </button>
                  <button class="ic" aria-label={S.presetUi.rename(p)} title={S.presetUi.rename(p)} onclick={() => startRename(p)}>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16v4z" /></svg>
                  </button>
                  <button class="ic" aria-label={S.presetUi.del(p)} title={S.presetUi.del(p)} onclick={() => { deleting = p; renaming = null; }}>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13" /></svg>
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#if replacing}
          <div class="ask" role="alert">
            <span>{S.presetUi.replaceAsk(replacing)}</span>
            <button class="btn sm" onclick={doReplace}>{S.presetUi.replaceBtn}</button>
            <button class="btn sm" onclick={() => (replacing = null)}>{S.cancel}</button>
          </div>
        {:else if session.active_preset}
          <button class="btn" onclick={() => (replacing = session.active_preset)}>{S.save} “{session.active_preset}”</button>
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
  {/if}

  <button class="btn more" data-more-menu bind:this={moreBtn} aria-haspopup="dialog" aria-expanded={store.advancedOpen} aria-label={S.advanced.title} title={S.advanced.title} onclick={() => store.openAdvanced()}>⋯</button>

  {#if !store.embed}
  <!-- Shows where you are: the current view is the highlighted half. Icons on phones. -->
  <div class="seg" role="group" aria-label={S.view.label}>
    <button class="segb" aria-pressed={store.simple} aria-label={S.view.simple} title={S.view.simpleHint} onclick={() => store.setSimple(true)}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8" /><path d="M12 12l4-4" /></svg>
      <span class="segt">{S.view.simple}</span>
    </button>
    <button class="segb" aria-pressed={!store.simple} aria-label={S.view.advanced} title={S.view.advancedHint} onclick={() => store.setSimple(false)}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 4v16M12 4v16M18 4v16" /><path d="M4 14h4M10 8h4M16 16h4" /></svg>
      <span class="segt">{S.view.advanced}</span>
    </button>
  </div>
  <span class="sr" role="status">{linkText}</span>
  <button class="status {link}" class:show={showStatus} aria-label="Connection: {linkText}" title={linkText} onclick={() => flashStatus()}>
    <span class="dot" aria-hidden="true"></span><span class="txt" aria-hidden="true">{link === 'demo' ? S.demoShort : linkText}</span>
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
  li {
    flex-wrap: wrap;
    gap: 4px;
  }
  .pn {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow-wrap: anywhere;
  }
  .tag {
    font-size: 11px;
    color: var(--text-dim);
  }
  .ic {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    padding: 0;
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text-dim);
    cursor: pointer;
  }
  .ic:hover,
  .ic:focus-visible {
    border-color: var(--border);
    color: var(--text);
  }
  .ic[aria-pressed='true'] {
    color: var(--cyan);
  }
  .ic svg {
    width: 18px;
    height: 18px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .inl {
    flex: 1;
    display: flex;
    gap: 4px;
  }
  .ask {
    flex: 1;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }
  .ask span {
    flex: 1 1 100%;
  }
  .btn.danger {
    border-color: var(--red);
    color: var(--red);
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
  /* Demo is not a fault: amber, and its label always shows (in the flow, so it
     never covers a button). Red stays for real problems. */
  .status.demo {
    --c: var(--amber);
    width: auto;
    gap: 6px;
    padding: 0 10px 0 8px;
    border: 1px solid var(--c);
    border-radius: 999px;
    height: 28px;
  }
  .status.demo .dot {
    width: 9px;
    height: 9px;
  }
  .status.demo .txt {
    position: static;
    transform: none;
    padding: 0;
    border: 0;
    background: none;
    opacity: 1;
  }
  .grow {
    flex: 1;
  }
  .seg {
    flex: none;
    display: inline-flex;
    padding: 2px;
    gap: 2px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .segb {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 28px;
    padding: 0 10px;
    background: none;
    border: 0;
    border-radius: 6px;
    color: var(--text-dim);
    cursor: pointer;
    white-space: nowrap;
  }
  .segb[aria-pressed='true'] {
    background: var(--bg-alt);
    color: var(--text);
    box-shadow: inset 0 0 0 1px var(--cyan);
  }
  .segb svg {
    display: none;
    width: 20px;
    height: 20px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
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
    /* The bottom tab bar has the views on phones: the header keeps one row. */
    .tabs.wideonly {
      display: none;
    }
    .clock {
      display: none;
    }
    .btn {
      min-height: 30px;
      padding: 0 10px;
    }
    /* One row on a 375 px phone even with a long preset name and the demo
       pill: the preset button is capped and the brand gives way first. */
    .brand {
      display: block;
      min-width: 0;
      line-height: 44px;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .btn {
      max-width: 120px;
    }
    .grow {
      display: none;
    }
    /* Icons only: the two halves fit beside Presets and ⋯. */
    .segb {
      min-width: 40px;
      min-height: 32px;
      justify-content: center;
      padding: 0 8px;
    }
    .segb svg {
      display: block;
    }
    .segt {
      display: none;
    }
    /* The presets panel is wider than the space left of its button: pin it to
       both screen edges under the header instead (was cut off at 375 px). */
    .pop.wide {
      position: fixed;
      top: calc(env(safe-area-inset-top, 0px) + 50px);
      left: max(8px, env(safe-area-inset-left, 0px));
      right: max(8px, env(safe-area-inset-right, 0px));
      width: auto;
      max-height: calc(100dvh - env(safe-area-inset-top, 0px) - 66px);
      overflow-y: auto;
    }
  }
</style>
