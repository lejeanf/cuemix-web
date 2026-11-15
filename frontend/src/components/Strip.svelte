<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { formatDb, gainToDb, gainToPos, posToGain } from '../lib/fader';
  import Fader from './Fader.svelte';
  import Knob from './Knob.svelte';
  import Meter from './Meter.svelte';
  import LabelEditor from './LabelEditor.svelte';

  /** `pair`: right channel of a stereo pair shown on this strip (the strip drives the left one). */
  let { ch, pair }: { ch: number; pair?: number } = $props();
  const bus = $derived(store.bus);
  const mc = $derived(store.state?.mixes[bus]?.channels[ch]);
  const mcR = $derived(pair === undefined ? undefined : store.state?.mixes[bus]?.channels[pair]);
  const name = $derived(store.stripName(ch, pair !== undefined));
  /** The device keeps both halves of a pair; flag when they differ (e.g. left at -inf). */
  const rDiffers = $derived(mcR !== undefined && mc !== undefined && Math.abs(mcR.gain - mc.gain) > 1e-6);
  const eq = $derived(store.state?.channels[ch]?.eq);
  const label = $derived(store.label(ch));
  const off = $derived(!store.ready);
  let editing = $state(false);

  const panText = (v: number) => (Math.abs(v) < 0.005 ? 'C' : v < 0 ? `L${Math.round(-v * 100)}` : `R${Math.round(v * 100)}`);
</script>

{#if mc && eq}
  <section class="strip" class:stereo={pair !== undefined} aria-label="Channel {name}">
    <div class="bar" style:background={label?.color ?? 'transparent'}></div>
    <div class="namewrap">
      <button class="name" onclick={() => (editing = !editing)} aria-label="{S.rename}: {name}" disabled={off}>
        {name}
      </button>
      {#if editing}<LabelEditor {ch} onclose={() => (editing = false)} />{/if}
    </div>

    <Knob
      value={mc.pan}
      min={-1}
      max={1}
      reset={0}
      step={0.01}
      label=""
      ariaLabel="{S.pan} {name}"
      size={28}
      format={panText}
      disabled={off}
      hint={store.hint('mix_pan')}
      onchange={(v, f) => store.set({ param: 'mix_pan', bus, ch, value: v }, f)}
    />

    <div class="faderrow">
      <Fader
        pos={gainToPos(mc.gain)}
        label="Level {name}"
        valueText="{formatDb(gainToDb(mc.gain))} dB"
        mark={0.75}
        disabled={off}
        hint={store.hint('mix_vol')}
        onchange={(p, f) => store.set({ param: 'mix_vol', bus, ch, gain: posToGain(p) }, f)}
      />
      <Meter index={ch} label="Meter {name}" />
    </div>
    <div class="db">
      {formatDb(gainToDb(mc.gain))}{#if rDiffers && mcR}<span
          class="rdiff"
          title="Stereo pair: the left channel is at {formatDb(gainToDb(mc.gain))} dB, the right at {formatDb(gainToDb(mcR.gain))} dB on the MOTU"
          >R {formatDb(gainToDb(mcR.gain))}</span
        >{/if}
    </div>

    <div class="btns">
      <button
        class="b red"
        class:on={mc.mute}
        aria-pressed={mc.mute}
        aria-label="{S.mute} {name}"
        disabled={off}
        data-unmapped={store.unmapped('mix_mute') ? '' : undefined}
        title={store.hint('mix_mute')}
        onclick={() => store.set({ param: 'mix_mute', bus, ch, on: !mc.mute }, true)}>M</button
      >
      <button
        class="b amber"
        class:on={mc.solo}
        aria-pressed={mc.solo}
        aria-label="{S.solo} {name}"
        disabled={off}
        data-unmapped={store.unmapped('mix_solo') ? '' : undefined}
        title={store.hint('mix_solo')}
        onclick={() => store.set({ param: 'mix_solo', bus, ch, on: !mc.solo }, true)}>S</button
      >
    </div>
    <div class="btns fx">
    <button
      class="b cyan"
      class:on={!eq.bypass}
      aria-pressed={!eq.bypass}
      aria-label="{S.eq} {eq.bypass ? 'bypassed' : 'active'} — {S.eqBypass} {name}"
      title={S.eqBypass}
      disabled={off}
      data-unmapped={store.unmapped('eq_bypass') ? '' : undefined}
      onclick={() => store.set({ param: 'eq_bypass', ch, on: !eq.bypass }, true)}>EQ</button
    >
    <button class="b" aria-label="{S.dsp} {name}" title="{S.dsp} {name}" disabled={off} onclick={() => (store.dspChannel = ch)}>
      {S.dsp}
    </button>
    </div>
  </section>
{/if}

<style>
  .strip {
    flex: none;
    width: 66px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 0 3px 4px;
    background: var(--card);
    border-right: 1px solid var(--border);
    overflow: visible;
  }
  .strip.stereo {
    width: 76px;
  }
  .bar {
    align-self: stretch;
    height: 5px;
    border-radius: 7px 7px 0 0;
    margin: 0 -3px;
  }
  .namewrap {
    position: relative;
    align-self: stretch;
  }
  .name {
    width: 100%;
    background: none;
    border: 0;
    padding: 3px 1px;
    text-align: center;
    font-weight: 600;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
    border-radius: 4px;
  }
  .name:hover:not(:disabled) {
    background: var(--bg-alt);
  }
  .faderrow {
    flex: 1;
    display: flex;
    gap: 4px;
    align-items: stretch;
    align-self: stretch;
    min-height: 140px;
  }
  .faderrow :global(.fader) {
    margin: 0;
  }
  .db {
    font-variant-numeric: tabular-nums;
    font-size: 12px;
    color: var(--text-dim);
  }
  .btns {
    display: flex;
    gap: 4px;
  }
  .b {
    min-width: 28px;
    min-height: 28px;
    padding: 0 4px;
    font-size: 12px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
    --on: var(--cyan);
  }
  .b.red {
    --on: var(--red);
  }
  .b.amber {
    --on: var(--amber);
  }
  /* EQ "on" is the normal state: outline it instead of filling every strip. */
  .b.cyan.on {
    background: transparent;
    color: var(--cyan);
    font-weight: 600;
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
  .rdiff {
    display: block;
    font-size: 0.75em;
    color: var(--amber, #f59e0b);
  }
</style>
