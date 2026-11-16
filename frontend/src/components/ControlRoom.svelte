<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, trimDbToPos, trimPosToDb } from '../lib/fader';
  import UpdateBadge from './UpdateBadge.svelte';

  const cr = $derived(store.state?.control_room);
  const main = $derived(store.state?.main.db ?? TRIM_MIN_DB);
  const off = $derived(!store.ready);

  function slider(e: Event, apply: (db: number) => void) {
    const pos = Number((e.currentTarget as HTMLInputElement).value) / 1000;
    apply(Math.round(trimPosToDb(pos, TRIM_MIN_DB)));
  }
</script>

<footer class="cr" aria-label={S.controlRoom}>
  <span class="label">{S.controlRoom}</span>
  {#if cr}
    <button class="t amber" class:on={cr.dim} aria-pressed={cr.dim} disabled={off || store.locked('dim')} data-unmapped={store.unmapped('dim') ? '' : undefined} title={store.hint('dim')} onclick={() => store.set({ param: 'dim', on: !cr.dim }, true)}>{S.dim}</button>
    <button class="t" class:on={cr.mono} aria-pressed={cr.mono} disabled={off} onclick={() => store.set({ param: 'mono', on: !cr.mono }, true)}>{S.mono}</button>
    <button class="t" class:on={cr.speaker_b} aria-pressed={cr.speaker_b} aria-label="{S.speakerAB}: speakers {cr.speaker_b ? 'B' : 'A'}" disabled={off || store.locked('speaker_b')} data-unmapped={store.unmapped('speaker_b') ? '' : undefined} title={store.hint('speaker_b')} onclick={() => store.set({ param: 'speaker_b', on: !cr.speaker_b }, true)}>{cr.speaker_b ? 'B' : 'A'}</button>
    <button class="t red" class:on={cr.talkback} aria-pressed={cr.talkback} disabled={off} onclick={() => store.set({ param: 'talkback', on: !cr.talkback }, true)}>{S.talkback}</button>

    <label class="lv" data-unmapped={store.unmapped('phones_vol') ? '' : undefined} title={store.hint('phones_vol')}>
      <span>{S.phones}</span>
      <input type="range" min="0" max="1000" step="1" aria-label="{S.phones} level" aria-valuetext="{formatDb(cr.phones_db, 0)} dB" value={Math.round(trimDbToPos(cr.phones_db) * 1000)} disabled={off}
        oninput={(e) => slider(e, (db) => store.set({ param: 'phones_vol', db }))}
        onchange={(e) => slider(e, (db) => store.set({ param: 'phones_vol', db }, true))} />
      <output>{formatDb(cr.phones_db, 0)} dB</output>
    </label>
    <label class="lv" data-unmapped={store.unmapped('main_vol') ? '' : undefined} title={store.hint('main_vol')}>
      <span>{S.main}</span>
      <input type="range" min="0" max="1000" step="1" aria-label="{S.main} level" aria-valuetext="{formatDb(main, 0)} dB" value={Math.round(trimDbToPos(main) * 1000)} disabled={off}
        oninput={(e) => slider(e, (db) => store.set({ param: 'main_vol', db }))}
        onchange={(e) => slider(e, (db) => store.set({ param: 'main_vol', db }, true))}
        ondblclick={() => store.set({ param: 'main_vol', db: 0 }, true)} />
      <output>{formatDb(main, 0)} dB</output>
    </label>
  {/if}
  <span class="ver">
    <UpdateBadge />
    {#if store.version}<span title="Running version">v{store.version}</span>{/if}
    {#if store.device}<span title="Connected MOTU: model {store.device.model}, firmware {store.device.firmware}">{store.device.model} {store.device.firmware}</span>{/if}
  </span>
</footer>

<style>
  .cr {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 3px 10px;
    font-size: 12px;
    background: var(--bg-alt);
    border-top: 1px solid var(--border);
  }
  .label {
    color: var(--text-dim);
    margin-right: 4px;
  }
  .t {
    min-width: 48px;
    min-height: 24px;
    padding: 0 8px;
    font-size: 12px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 5px;
    cursor: pointer;
    --on: var(--cyan);
  }
  .t.amber {
    --on: var(--amber);
  }
  .t.red {
    --on: var(--red);
  }
  .t.on {
    background: var(--on);
    border-color: var(--on);
    color: #111;
    font-weight: 700;
  }
  .t:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .lv {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
    border-radius: 6px;
  }
  .lv input {
    width: 120px;
    min-height: 20px;
    accent-color: var(--cyan);
  }
  .ver {
    margin-left: auto;
    display: flex;
    gap: 8px;
    font-size: 11px;
    align-items: center;
    color: var(--text-dim);
    white-space: nowrap;
  }
  output {
    min-width: 54px;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  @media (max-width: 640px) {
    /* Phones: only the two volumes (the buttons are on the Output tab). */
    .label,
    .ver,
    .t {
      display: none;
    }
    .lv {
      flex: 1 1 150px;
    }
    .lv input {
      flex: 1;
      min-width: 0;
    }
    .cr {
      padding: 4px 8px;
    }
  }
</style>
