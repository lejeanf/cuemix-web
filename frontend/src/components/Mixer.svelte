<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, trimDbToPos, trimPosToDb } from '../lib/fader';
  import Fader from './Fader.svelte';
  import Strip from './Strip.svelte';
  import Meter from './Meter.svelte';
  import { METER_SLOT, OUTPUTS } from '../lib/meterMap';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  /** Meter slots of the selected mix's output: mix m feeds output pair 2m (Main, Line 3-4 … Phones); the Reverb mix shows the reverb return. */
  const mixMeters = $derived.by(() => {
    const m = store.bus;
    if (2 * m + 1 < OUTPUTS) return [METER_SLOT.output(2 * m), METER_SLOT.output(2 * m + 1)];
    return [METER_SLOT.mixerInput(30), METER_SLOT.mixerInput(31)];
  });
</script>

{#if st}
  <div class="mixer">
    <div class="buses" role="tablist" aria-label="Mix bus">
      {#each store.mixOrder as i (i)}
        <button
          role="tab"
          aria-selected={store.bus === i}
          class:sel={store.bus === i}
          onclick={() => (store.bus = i)}>{store.mixName(i)}</button
        >
      {/each}
    </div>
    <div class="row">
      <div class="strips">
        {#each store.strips as s (s.ch)}
          <Strip ch={s.ch} pair={s.pair} />
        {/each}
      </div>
      <section class="main" aria-label={S.main}>
        <div class="title">{S.main}</div>
        <div class="mrow">
        <Fader
          pos={trimDbToPos(st.main.db)}
          label="{S.main} level"
          valueText="{formatDb(st.main.db, 0)} dB"
          resetPos={1}
          mark={1}
          disabled={off}
          hint={store.hint('main_vol')}
          onchange={(p, f) => store.set({ param: 'main_vol', db: Math.round(trimPosToDb(p, TRIM_MIN_DB)) }, f)}
        />
          <div class="mm" title="{store.mixName(store.bus)} output level (before the volume)">
            {#each mixMeters as slot (slot)}<Meter index={slot} label="{store.mixName(store.bus)} output meter" />{/each}
          </div>
        </div>
        <div class="db">{formatDb(st.main.db, 0)} dB</div>
      </section>
    </div>
  </div>
{/if}

<style>
  .mrow {
    flex: 1;
    display: flex;
    gap: 3px;
    min-height: 0;
    align-self: stretch;
  }
  .mm {
    display: flex;
    gap: 2px;
  }
  .mixer {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    gap: 6px;
  }
  .buses {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    flex: none;
  }
  .buses button {
    flex: none;
    min-height: 30px;
    padding: 0 12px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 15px;
    font-size: 13px;
    cursor: pointer;
  }
  .buses button.sel {
    background: var(--cyan);
    border-color: var(--cyan);
    color: #111;
    font-weight: 600;
  }
  .row {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 10px;
  }
  .strips {
    flex: 1;
    display: flex;
    gap: 0;
    overflow-x: auto;
    overflow-y: hidden;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding-bottom: 6px;
    min-width: 0;
    /* popovers inside strips must not be clipped vertically */
    align-items: stretch;
  }
  .main {
    flex: none;
    width: 80px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 6px 4px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .title {
    font-weight: 700;
  }
  .db {
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
</style>
