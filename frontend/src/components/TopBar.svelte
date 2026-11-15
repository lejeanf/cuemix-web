<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { CLOCK_SOURCES, SAMPLE_RATES, PRESET_NAME_RE, type ClockSource, type SampleRate } from '../lib/protocol';
  import { isValidPresetName } from '../lib/validate';

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
  function setClock(patch: Partial<{ sample_rate: SampleRate; source: ClockSource }>) {
    if (!clock) return;
    store.set({ param: 'clock', sample_rate: clock.sample_rate, source: clock.source, ...patch }, true);
  }
  function saveAs() {
    if (!nameOk) return;
    store.savePreset(newName);
    newName = '';
  }
  const khz = (r: string) => `${Number(r) / 1000} kHz`;
</script>

<svelte:window onpointerdown={outside} onkeydown={key} />

<header class="top" bind:this={root}>
  <span class="brand">{S.appName}</span>
  <span class="preset">
    {S.presetLabel}: <strong>{session.active_preset ?? S.noPreset}</strong>
    {#if session.modified}<span class="modified">{S.modified}</span>{/if}
  </span>

  <div class="pw">
    <button class="btn" aria-haspopup="dialog" aria-expanded={open === 'clock'} disabled={off} onclick={() => toggle('clock')}>
      {#if clock}{khz(clock.sample_rate)} · {clock.source}{:else}{S.clock}{/if}
    </button>
    {#if open === 'clock' && clock}
      <div class="pop" role="dialog" aria-label={S.clock}>
        <label>{S.sampleRate}
          <select value={clock.sample_rate} onchange={(e) => setClock({ sample_rate: e.currentTarget.value as SampleRate })}>
            {#each SAMPLE_RATES as r (r)}<option value={r}>{khz(r)}</option>{/each}
          </select>
        </label>
        <label>{S.clockSource}
          <select value={clock.source} onchange={(e) => setClock({ source: e.currentTarget.value as ClockSource })}>
            {#each CLOCK_SOURCES as c (c)}<option value={c}>{c}</option>{/each}
          </select>
        </label>
      </div>
    {/if}
  </div>

  <div class="pw">
    <button class="btn" aria-haspopup="dialog" aria-expanded={open === 'presets'} disabled={off} onclick={() => toggle('presets')}>{S.presets}</button>
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
      </div>
    {/if}
  </div>

  <span class="pill {link}" role="status">{linkText}</span>
</header>

<style>
  .top {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 14px;
    background: var(--bg-alt);
    border-bottom: 1px solid var(--border);
    position: relative;
    z-index: 20;
  }
  .brand {
    font-weight: 700;
  }
  .preset {
    flex: 1;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .preset strong {
    color: var(--text);
  }
  .modified {
    margin-left: 8px;
    color: var(--amber);
  }
  .pw {
    position: relative;
  }
  .btn {
    min-height: 36px;
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
  @media (max-width: 640px) {
    .brand {
      display: none;
    }
  }
</style>
