<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, trimDbToPos, trimPosToDb } from '../lib/fader';
  import UpdateBadge from './UpdateBadge.svelte';
  import { powerView, type PowerView } from '../lib/power';

  const cr = $derived(store.state?.control_room);
  const main = $derived(store.state?.main.db ?? TRIM_MIN_DB);
  const off = $derived(!store.ready);

  /** Phones: the DIM / MONO / A-B / Talkback buttons fold behind "More" to keep the bar short. */
  let more = $state(false);
  const mainPower = $derived(powerView(store.c.power, 'main', main));
  const phonesPower = $derived(powerView(store.c.power, 'phones', cr?.phones_db ?? TRIM_MIN_DB));

  function slider(e: Event, apply: (db: number) => void) {
    const pos = Number((e.currentTarget as HTMLInputElement).value) / 1000;
    apply(Math.round(trimPosToDb(pos, TRIM_MIN_DB)));
  }
</script>

{#snippet level(name: string, param: 'main_vol' | 'phones_vol', db: number, pv: PowerView, ch: 'main' | 'phones')}
  <div class="lvrow">
    <label class="lv" data-unmapped={store.unmapped(param) ? '' : undefined} title={store.hint(param)}>
      <span>{name}</span>
      <input type="range" min="0" max="1000" step="1" aria-label="{name} level" aria-valuetext="{formatDb(db, 0)} dB" value={Math.round(trimDbToPos(db) * 1000)} disabled={off}
        oninput={(e) => slider(e, (v) => store.set({ param, db: v }))}
        onchange={(e) => slider(e, (v) => store.set({ param, db: v }, true))} />
      <output>{formatDb(db, 0)} dB</output>
    </label>
    <button class="pw" class:on={pv.on} class:busy={pv.fading !== null} aria-pressed={pv.on} aria-label="{name} {pv.on ? S.powerOn : S.powerOff}" title="{name}: {pv.fading === 'out' ? S.fadingOut : pv.fading === 'in' ? S.fadingIn : pv.on ? S.powerOn : S.powerOff}" disabled={off} onclick={() => store.power(ch, !pv.on)}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v8" /><path d="M6.3 6.8a8 8 0 1 0 11.4 0" /></svg>
    </button>
  </div>
{/snippet}

<footer class="cr" class:open={more} aria-label={S.controlRoom}>
  <span class="label">{S.controlRoom}</span>
  {#if cr}
    <div class="tg" class:open={more}>
      <button class="t amber" class:on={cr.dim} aria-pressed={cr.dim} disabled={off || store.locked('dim')} data-unmapped={store.unmapped('dim') ? '' : undefined} title={store.hint('dim')} onclick={() => store.set({ param: 'dim', on: !cr.dim }, true)}>{S.dim}</button>
      <button class="t" class:on={cr.mono} aria-pressed={cr.mono} disabled={off} onclick={() => store.set({ param: 'mono', on: !cr.mono }, true)}>{S.mono}</button>
      <button class="t" class:on={cr.speaker_b} aria-pressed={cr.speaker_b} aria-label="{S.speakerAB}: speakers {cr.speaker_b ? 'B' : 'A'}" disabled={off || store.locked('speaker_b')} data-unmapped={store.unmapped('speaker_b') ? '' : undefined} title={store.hint('speaker_b')} onclick={() => store.set({ param: 'speaker_b', on: !cr.speaker_b }, true)}>{cr.speaker_b ? 'B' : 'A'}</button>
      <button class="t red" class:on={cr.talkback} aria-pressed={cr.talkback} disabled={off} onclick={() => store.set({ param: 'talkback', on: !cr.talkback }, true)}>{S.talkback}</button>
    </div>

    {@render level(S.monitor, 'main_vol', main, mainPower, 'main')}
    {@render level(S.phones, 'phones_vol', cr.phones_db, phonesPower, 'phones')}
    <button class="morebtn" aria-expanded={more} aria-label={S.crMore} title={S.crMore} onclick={() => (more = !more)}>{more ? '×' : '⋯'}</button>
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
    padding: 3px max(10px, env(safe-area-inset-right, 0px)) calc(3px + env(safe-area-inset-bottom, 0px)) max(10px, env(safe-area-inset-left, 0px));
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
    color: var(--on-accent);
    font-weight: 700;
  }
  .t:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .tg {
    display: flex;
    gap: 6px;
  }
  .lvrow {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .pw {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 24px;
    padding: 0;
    background: var(--card);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 5px;
    cursor: pointer;
  }
  .pw.on {
    color: var(--cyan);
    border-color: var(--cyan);
  }
  .pw.busy {
    border-style: dashed;
  }
  .pw svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
  }
  .pw:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .morebtn {
    display: none;
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
    /* Phones: Monitor and Phones, each with its on/off, on one row each; the buttons fold
       behind "More" (also on the Output tab). Touch-sized targets. */
    .label,
    .ver {
      display: none;
    }
    .cr {
      padding: 4px 8px;
      gap: 4px 6px;
    }
    .tg {
      display: none;
      order: 5;
      flex-basis: 100%;
    }
    .cr.open .tg {
      display: flex;
    }
    .tg .t {
      flex: 1;
      min-height: 40px;
    }
    /* Both rows leave room for the More button, so sliders and on/off buttons line up. */
    .lvrow {
      flex: 0 0 calc(100% - 50px);
    }
    .lv {
      flex: 1;
      min-width: 0;
    }
    .lv span {
      width: 52px;
    }
    .lv input {
      flex: 1;
      min-width: 0;
      min-height: 32px;
    }
    .pw {
      width: 44px;
      height: 40px;
    }
    .morebtn {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      flex: none;
      width: 44px;
      height: 40px;
      font-size: 18px;
      background: var(--card);
      color: var(--text);
      border: 1px solid var(--border);
      border-radius: 5px;
      cursor: pointer;
    }
  }
  /* Touch screens: controls at least 44 px tall (the mouse keeps the dense layout). */
  @media (pointer: coarse) {
    .t,
    .pw,
    .morebtn {
      min-height: 44px;
    }
    .pw,
    .morebtn {
      min-width: 44px;
    }
    .lv input {
      min-height: 36px;
    }
  }
</style>
