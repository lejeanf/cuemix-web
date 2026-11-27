<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, trimDbToPos, trimPosToDb } from '../lib/fader';
  import Fader from './Fader.svelte';
  import Strip from './Strip.svelte';
  import Meter from './Meter.svelte';
  import { METER_SLOT, OUTPUTS } from '../lib/meterMap';
  import { groupStrips } from '../lib/mixerGroups';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  /** Meter slots of the selected mix's output: mix m feeds output pair 2m (Main, Line 3-4 … Phones); the Reverb mix shows the reverb return. */
  const mixMeters = $derived.by(() => {
    const m = store.bus;
    if (2 * m + 1 < OUTPUTS) return [METER_SLOT.output(2 * m), METER_SLOT.output(2 * m + 1)];
    return [METER_SLOT.mixerInput(30), METER_SLOT.mixerInput(31)];
  });
  /** Strips grouped by kind (Mic, Line, S/PDIF, Optical, Computer, Reverb). */
  const groups = $derived(
    groupStrips(store.strips, {
      inputs: st?.topology.mixer_inputs ?? 0,
      mics: st?.topology.mic_inputs ?? 0,
      analog: st?.preamps.length ?? 0,
      reverbReturn: store.reverbReturn,
    }),
  );
  /** Scale beside the Monitor fader, at the fader's own positions. */
  const SCALE_DB = [0, -10, -20, -40, -60];
  /** Same geometry as Fader.svelte: the thumb centre travels between HALF and height − HALF. */
  const HALF = 11;
  const tickTop = (db: number) => `calc(${HALF}px + ${1 - trimDbToPos(db)} * (100% - ${2 * HALF}px))`;
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
        {#each groups as g, gi (gi)}
          <div class="grp" role="group" aria-label={g.title}>
            <div class="gh">{g.title}</div>
            <div class="gs">
              {#each g.strips as s (s.ch)}
                <Strip ch={s.ch} pair={s.pair} />
              {/each}
            </div>
          </div>
        {/each}
      </div>
      <section class="main" aria-label={S.monitor}>
        <div class="title">{S.monitor}</div>
        <div class="mrow">
          <div class="scale" aria-hidden="true">
            {#each SCALE_DB as db (db)}<span style:top={tickTop(db)}>{db}</span>{/each}
          </div>
        <Fader
          pos={trimDbToPos(st.main.db)}
          label="{S.monitor} level"
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
  /* Groups: a small heading over each run of strips, a gap between runs. */
  .grp {
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .grp + .grp {
    margin-left: 6px;
    border-left: 1px solid var(--border);
  }
  .gh {
    flex: none;
    padding: 3px 6px 2px;
    font-size: 11px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
    white-space: nowrap;
    background: var(--bg-alt);
    border-bottom: 1px solid var(--border);
  }
  .gs {
    flex: 1;
    min-height: 0;
    display: flex;
    align-items: stretch;
  }
  .scale {
    position: relative;
    flex: none;
    width: 22px;
    font-size: 10px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .scale span {
    position: absolute;
    right: 0;
    transform: translateY(-50%);
  }
  .main {
    flex: none;
    width: 104px;
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
  /* Phones: a narrower Monitor column leaves room for more strips. */
  @media (max-width: 640px) {
    .main {
      width: 84px;
    }
    .scale {
      width: 18px;
      font-size: 9px;
    }
  }
</style>
