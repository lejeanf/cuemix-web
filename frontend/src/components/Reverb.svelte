<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { DSP_LIMITS } from '../lib/protocol';
  import { formatDb, gainToDb, gainToPos, posToGain } from '../lib/fader';
  import Fader from './Fader.svelte';
  import Knob from './Knob.svelte';
  import NumField from './NumField.svelte';

  // The reverb block holds only the processor settings. Its sends are the
  // faders of the Reverb mix, and its return is a mixer input (30-31 on the
  // mk5) present in every mix: both are mix params (`mix_vol`).
  const st = $derived(store.state);
  const rv = $derived(st?.reverb);
  const off = $derived(!store.ready);
  const rmix = $derived(store.reverbMix);
  const ret = $derived(store.reverbReturn);
  /** Every strip but the reverb return itself. */
  const sendStrips = $derived(store.strips.filter((s) => s.ch !== ret));
  /** Mixes that can receive the return (all but the Reverb mix), in display order. */
  const returnMixes = $derived(store.mixOrder.filter((m) => m !== rmix));
  const pct = (v: number) => `${Math.round(v)}`;
  const gainText = (g: number) => `${formatDb(gainToDb(g))} dB`;
</script>

{#if st && rv}
  <div class="panel">
    <section class="head" aria-label={S.reverb}>
      <h3>{S.reverb}</h3>
      <Knob value={rv.decay} min={0} max={100} reset={86} step={1} size={40} label={S.reverbDecay} format={pct} disabled={off} hint={store.hint('reverb_decay')} onchange={(v, f) => store.set({ param: 'reverb_decay', value: Math.round(v) }, f)} />
      <Knob value={rv.damping} min={0} max={100} reset={90} step={1} size={40} label={S.reverbDamping} format={pct} disabled={off} hint={store.hint('reverb_damping')} onchange={(v, f) => store.set({ param: 'reverb_damping', value: Math.round(v) }, f)} />
      <Knob
        value={rv.predelay_ms}
        min={DSP_LIMITS.REVERB_PREDELAY_MS[0]}
        max={DSP_LIMITS.REVERB_PREDELAY_MS[1]}
        reset={50}
        step={1}
        size={40}
        label={S.reverbPredelay}
        format={(v) => `${Math.round(v)} ms`}
        disabled={off}
        hint={store.hint('reverb_predelay')}
        onchange={(v, f) => store.set({ param: 'reverb_predelay', ms: Math.round(v) }, f)}
      />
      <Knob value={rv.width} min={0} max={100} reset={50} step={1} size={40} label={S.reverbWidth} format={pct} disabled={off} hint={store.hint('reverb_width')} onchange={(v, f) => store.set({ param: 'reverb_width', value: Math.round(v) }, f)} />
      <NumField label={S.reverbPreset} value={rv.preset} min={0} max={255} step={1} disabled={off} onchange={(v) => store.set({ param: 'reverb_preset', preset: Math.round(v) }, true)} />
      {#if store.hint('reverb_decay')}<p class="hint">{store.hint('reverb_decay')}</p>{/if}
    </section>

    <div class="mixes">
    {#if ret !== null && returnMixes.length}
      <section class="ret" aria-label={S.reverbReturn}>
        <h3 title={S.reverbReturnHint}>{S.reverbReturn} <span class="sub">{S.reverbReturnHint}</span></h3>
        <div class="row">
          {#each returnMixes as m (m)}
            {@const g = st.mixes[m]?.channels[ret]?.gain ?? 0}
            <div class="cell">
              <Fader
                pos={gainToPos(g)}
                label="{S.reverbReturn} {store.mixName(m)}"
                valueText={gainText(g)}
                mark={0.75}
                disabled={off}
                hint={store.hint('mix_vol')}
                onchange={(p, f) => store.set({ param: 'mix_vol', bus: m, ch: ret, gain: posToGain(p) }, f)}
              />
              <span class="cap">{formatDb(gainToDb(g))}</span>
              <span class="nm">{store.mixName(m)}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if rmix !== null}
      <section class="grow" aria-label={S.reverbSends}>
        <h3 title={S.reverbSendsHint}>{S.reverbSends} <span class="sub">{S.reverbSendsHint}</span></h3>
        <div class="row">
          {#each sendStrips as s (s.ch)}
            {@const g = st.mixes[rmix]?.channels[s.ch]?.gain ?? 0}
            {@const name = store.stripName(s.ch, s.pair !== undefined)}
            <div class="cell">
              <Fader
                pos={gainToPos(g)}
                label="{S.reverbSends} {name}"
                valueText={gainText(g)}
                mark={0.75}
                disabled={off}
                hint={store.hint('mix_vol')}
                onchange={(p, f) => store.set({ param: 'mix_vol', bus: rmix, ch: s.ch, gain: posToGain(p) }, f)}
              />
              <span class="cap">{formatDb(gainToDb(g))}</span>
              <span class="nm" style:border-color={store.label(s.ch)?.color ?? 'transparent'}>{name}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}
    </div>
  </div>
{/if}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 8px;
    height: 100%;
    min-height: 0;
  }
  section {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .mixes {
    flex: 1;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    min-height: 0;
  }
  section.ret {
    flex: none;
    max-width: 100%;
  }
  section.head {
    flex-direction: row;
    flex-wrap: wrap;
    align-items: center;
    gap: 14px;
  }
  section.grow {
    flex: 1 1 320px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    color: var(--text-dim);
  }
  .sub,
  .hint {
    font-size: 11px;
    font-weight: 400;
    color: var(--text-dim);
  }
  .hint {
    margin: 0;
    flex-basis: 100%;
  }
  .row {
    flex: 1;
    display: flex;
    gap: 4px;
    overflow-x: auto;
    overflow-y: hidden;
    min-height: 210px;
  }
  .cell {
    flex: none;
    width: 58px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 4px 2px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .cap {
    font-size: 11px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .nm {
    font-size: 11px;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 3px solid transparent;
  }
</style>
