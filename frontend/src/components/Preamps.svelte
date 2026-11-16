<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { LINE_GAIN_MAX_DB, PREAMP_GAIN_MAX_DB } from '../lib/protocol';
  import Knob from './Knob.svelte';
  import Meter from './Meter.svelte';
  import { METER_SLOT } from '../lib/meterMap';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  let confirming = $state<Record<number, boolean>>({});
  const nMic = $derived(st?.topology.mic_inputs ?? 0);
  const all = $derived((st?.preamps ?? []).map((pa, ch) => ({ pa, ch })));
  /** Mic preamps and line inputs, each in its own group. */
  const groups = $derived([
    { title: S.mics, items: all.filter((x) => x.ch < nMic) },
    { title: S.lines, items: all.filter((x) => x.ch >= nMic) },
  ].filter((g) => g.items.length));

  function phantom(ch: number, on: boolean) {
    if (on) {
      confirming[ch] = true;
    } else {
      confirming[ch] = false;
      store.set({ param: 'phantom', ch, on: false }, true);
    }
  }
  function confirm(ch: number) {
    confirming[ch] = false;
    store.set({ param: 'phantom', ch, on: true }, true);
  }
</script>

{#if st}
  <div class="groups">
    {#each groups as g (g.title)}
      <section class="group" aria-label={g.title}>
        <h3>{g.title}</h3>
        <div class="chans">
          {#each g.items as { pa, ch } (ch)}
            {@const mic = ch < nMic}
            <div class="ch" aria-label="Preamp {store.labelName(ch)}" role="group">
              <div class="bar" style:background={store.label(ch)?.color ?? 'transparent'}></div>
              <span class="nm" title={store.labelName(ch)}>{store.labelName(ch)}</span>
              <Knob
                value={pa.gain_db}
                min={0}
                max={mic ? PREAMP_GAIN_MAX_DB : LINE_GAIN_MAX_DB}
                reset={0}
                step={1}
                size={44}
                label={S.gain}
                ariaLabel="Gain {store.labelName(ch)}"
                format={(v) => `${Math.round(v)} dB`}
                disabled={off}
                hint={store.hint('preamp_gain')}
                onchange={(v, f) => store.set({ param: 'preamp_gain', ch, db: Math.round(v) }, f)}
              />
              <div class="hm" title="Input level after the preamp (before EQ and dynamics)">
                <Meter index={METER_SLOT.preamp(ch)} horizontal label="Input meter {store.labelName(ch)}" />
              </div>
              <div class="btns">
                <button
                  class="b"
                  class:on={pa.phase}
                  aria-pressed={pa.phase}
                  aria-label="{S.phase} {store.labelName(ch)}"
                  title={S.phase}
                  disabled={off}
                  onclick={() => store.set({ param: 'phase', ch, on: !pa.phase }, true)}>Ø</button
                >
                {#if mic}
                  <button
                    class="b"
                    class:on={pa.pad}
                    aria-pressed={pa.pad}
                    aria-label="{S.pad} {store.labelName(ch)}"
                    disabled={off}
                    onclick={() => store.set({ param: 'pad', ch, on: !pa.pad }, true)}>{S.pad}</button
                  >
                  <button
                    class="b red"
                    class:on={pa.phantom}
                    aria-pressed={pa.phantom}
                    aria-label="{S.phantom} {store.labelName(ch)}"
                    disabled={off}
                    onclick={() => phantom(ch, !pa.phantom)}>{S.phantom}</button
                  >
                {/if}
              </div>
              {#if confirming[ch]}
                <div class="confirm" role="alertdialog" aria-label={S.phantomConfirm}>
                  <p>{S.phantomConfirm}</p>
                  <div class="row">
                    <button class="b red on" onclick={() => confirm(ch)}>{S.enable}</button>
                    <button class="b" onclick={() => (confirming[ch] = false)}>{S.cancel}</button>
                  </div>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/each}
  </div>
{/if}

<style>
  .groups {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: flex-start;
  }
  .group {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 6px 8px 8px;
  }
  h3 {
    margin: 0 0 4px;
    font-size: 13px;
    color: var(--text-dim);
    font-weight: 600;
  }
  .chans {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .ch {
    position: relative;
    min-width: 64px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 0 4px 5px;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: visible;
  }
  .bar {
    align-self: stretch;
    height: 3px;
    margin: 0 -4px;
    border-radius: 6px 6px 0 0;
  }
  .nm {
    font-size: 12px;
    font-weight: 600;
    max-width: 90px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hm {
    align-self: stretch;
    padding: 0 2px;
  }
  .btns,
  .row {
    display: flex;
    gap: 3px;
  }
  .b {
    min-width: 24px;
    min-height: 22px;
    padding: 0 5px;
    font-size: 11px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
    --on: var(--cyan);
  }
  .b.red {
    --on: var(--red);
  }
  .b.on {
    background: var(--on);
    border-color: var(--on);
    color: #111;
    font-weight: 700;
  }
  .b:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .confirm {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 10;
    width: 180px;
    margin-top: 4px;
    background: var(--card);
    border: 1px solid var(--red);
    border-radius: 6px;
    padding: 6px;
    font-size: 12px;
    box-shadow: 0 6px 18px #000a;
  }
  .confirm p {
    margin: 0 0 6px;
  }
</style>
