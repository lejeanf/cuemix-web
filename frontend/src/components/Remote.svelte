<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, trimDbToPos, trimPosToDb } from '../lib/fader';
  import { METER_SLOT } from '../lib/meterMap';
  import Meter from './Meter.svelte';

  // Simple view for phones: the two volumes people reach for, big enough
  // for a thumb, with meters; everything else is one tap away (Advanced).
  const cr = $derived(store.state?.control_room);
  const main = $derived(store.state?.main.db ?? TRIM_MIN_DB);
  const off = $derived(!store.ready);
  const session = $derived(store.c.session);

  type Which = 'main_vol' | 'phones_vol';
  const clampDb = (db: number) => Math.min(0, Math.max(TRIM_MIN_DB, Math.round(db)));
  function setDb(which: Which, db: number, final: boolean) {
    store.set({ param: which, db: clampDb(db) }, final);
  }
  function fromSlider(which: Which, e: Event, final: boolean) {
    const pos = Number((e.currentTarget as HTMLInputElement).value) / 1000;
    setDb(which, trimPosToDb(pos, TRIM_MIN_DB), final);
  }

  const cards = $derived([
    { which: 'main_vol' as const, title: S.main, db: main, meters: [METER_SLOT.output(0), METER_SLOT.output(1)] },
    { which: 'phones_vol' as const, title: S.phones, db: cr?.phones_db ?? TRIM_MIN_DB, meters: [METER_SLOT.output(10), METER_SLOT.output(11)] },
  ]);

  $effect(() => {
    if (store.ready) store.listPresets();
  });
</script>

<div class="remote">
  {#each cards as c (c.which)}
    <section class="card" aria-label="{c.title} volume">
      <div class="head">
        <h2>{c.title}</h2>
        <output class="db" aria-live="polite">{formatDb(c.db, 0)} <small>dB</small></output>
      </div>
      <div class="meters" aria-hidden="true">
        {#each c.meters as m (m)}<Meter index={m} horizontal label="{c.title} meter" />{/each}
      </div>
      <div class="ctl">
        <button class="step" aria-label="{c.title} down 1 dB" disabled={off} onclick={() => setDb(c.which, c.db - 1, true)}>−</button>
        <input
          type="range"
          min="0"
          max="1000"
          step="1"
          aria-label="{c.title} volume"
          aria-valuetext="{formatDb(c.db, 0)} dB"
          value={Math.round(trimDbToPos(c.db) * 1000)}
          disabled={off}
          oninput={(e) => fromSlider(c.which, e, false)}
          onchange={(e) => fromSlider(c.which, e, true)}
        />
        <button class="step" aria-label="{c.title} up 1 dB" disabled={off} onclick={() => setDb(c.which, c.db + 1, true)}>+</button>
      </div>
    </section>
  {/each}

  {#if cr}
    <section class="card row">
      <button class="t" class:on={cr.mono} aria-pressed={cr.mono} disabled={off} onclick={() => store.set({ param: 'mono', on: !cr.mono }, true)}>{S.mono}</button>
      <button class="t red" class:on={cr.talkback} aria-pressed={cr.talkback} disabled={off} onclick={() => store.set({ param: 'talkback', on: !cr.talkback }, true)}>{S.talkback}</button>
    </section>
  {/if}

  {#if store.c.presets.length}
    <section class="card">
      <label class="preset">
        <span>{S.presets}</span>
        <select value={session.active_preset ?? ''} disabled={off} onchange={(e) => store.loadPreset(e.currentTarget.value)}>
          <option value="" disabled>{S.noPreset}</option>
          {#each store.c.presets as p (p)}<option value={p}>{p}</option>{/each}
        </select>
      </label>
    </section>
  {/if}

  <button class="adv" onclick={() => store.setSimple(false)}>Advanced controls →</button>
</div>

<style>
  .remote {
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 560px;
    margin: 0 auto;
    padding: 4px 0 16px;
  }
  .card {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 14px 16px;
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  h2 {
    margin: 0;
    font-size: 18px;
  }
  .db {
    font-size: 30px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .db small {
    font-size: 14px;
    color: var(--text-dim);
  }
  .meters {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 8px 0 12px;
  }
  .ctl {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .ctl input {
    flex: 1;
    min-width: 0;
    height: 44px;
    accent-color: var(--cyan);
  }
  .step {
    width: 52px;
    height: 52px;
    flex: none;
    font-size: 26px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
  }
  .row {
    display: flex;
    gap: 10px;
  }
  .t {
    flex: 1;
    min-height: 48px;
    font-size: 15px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    --on: var(--cyan);
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
  .preset {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 15px;
  }
  .preset select {
    flex: 1;
    min-height: 44px;
    font-size: 15px;
    background: #0d0e11;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0 8px;
  }
  .adv {
    align-self: center;
    min-height: 44px;
    padding: 0 18px;
    background: none;
    color: var(--cyan);
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 15px;
    cursor: pointer;
  }
  button:disabled,
  input:disabled,
  select:disabled {
    opacity: 0.45;
  }
</style>
