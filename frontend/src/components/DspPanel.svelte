<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { DSP_LIMITS } from '../lib/protocol';
  import EqCurve from './EqCurve.svelte';
  import Knob from './Knob.svelte';
  import NumField from './NumField.svelte';

  const ch = $derived(store.dspChannel);
  const dsp = $derived(ch !== null ? store.state?.channels[ch] : undefined);
  const off = $derived(!store.ready);
  let band = $state(0);
  const L = DSP_LIMITS;

  const close = () => (store.dspChannel = null);
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') close();
  }
  const ms = (v: number) => `${v < 10 ? v.toFixed(1) : Math.round(v)} ms`;
</script>

<svelte:window onkeydown={key} />

{#if ch !== null && dsp}
  <button class="backdrop" aria-label={S.close} onclick={close} tabindex="-1"></button>
  <div class="drawer" role="dialog" aria-label="{S.dsp} {store.labelName(ch)}">
    <header>
      <h2>{store.labelName(ch)} — {S.dsp}</h2>
      <button class="x" onclick={close} aria-label={S.close}>✕</button>
    </header>

    <section>
      <div class="sh">
        <h3>{S.eq}</h3>
        <button
          class="b"
          class:on={!dsp.eq.bypass}
          aria-pressed={!dsp.eq.bypass}
          disabled={off}
          onclick={() => store.set({ param: 'eq_bypass', ch, on: !dsp.eq.bypass }, true)}
          >{dsp.eq.bypass ? `${S.eqBypass}: on` : `${S.eq} active`}</button
        >
      </div>
      <EqCurve {ch} bind:selected={band} />
      <div class="hpf">
        <button
          class="b"
          class:on={dsp.eq.hpf.enabled}
          aria-pressed={dsp.eq.hpf.enabled}
          disabled={off}
          onclick={() => store.setHpf(ch, { enabled: !dsp.eq.hpf.enabled }, true)}>{S.hpf}</button
        >
        <NumField
          label="{S.hpf} frequency"
          value={dsp.eq.hpf.freq_hz}
          min={L.FREQ_HZ[0]}
          max={L.FREQ_HZ[1]}
          unit="Hz"
          disabled={off}
          onchange={(v) => store.setHpf(ch, { freq_hz: v }, true)}
        />
      </div>
      <div class="bands">
        {#each dsp.eq.bands as b, i (i)}
          <div class="band" class:sel={band === i}>
            <button
              class="b"
              class:on={b.enabled}
              aria-pressed={b.enabled}
              aria-label="Band {i + 1} enable"
              disabled={off}
              onclick={() => {
                band = i;
                store.setEqBand(ch, i, { enabled: !b.enabled }, true);
              }}>{i + 1}</button
            >
            <NumField label="Band {i + 1} frequency" value={b.freq_hz} min={L.FREQ_HZ[0]} max={L.FREQ_HZ[1]} step={1} unit="Hz" disabled={off} onchange={(v) => store.setEqBand(ch, i, { freq_hz: v }, true)} />
            <NumField label="Band {i + 1} gain" value={b.gain_db} min={L.EQ_GAIN_DB[0]} max={L.EQ_GAIN_DB[1]} step={0.1} unit="dB" disabled={off} onchange={(v) => store.setEqBand(ch, i, { gain_db: v }, true)} />
            <NumField label="Band {i + 1} Q" value={b.q} min={L.EQ_Q[0]} max={L.EQ_Q[1]} step={0.05} disabled={off} onchange={(v) => store.setEqBand(ch, i, { q: v }, true)} />
          </div>
        {/each}
      </div>
    </section>

    <section>
      <div class="sh">
        <h3>{S.compressor}</h3>
        <button
          class="b"
          class:on={dsp.comp.enabled}
          aria-pressed={dsp.comp.enabled}
          aria-label="{S.compressor} enable"
          disabled={off}
          onclick={() => store.setComp(ch, { enabled: !dsp.comp.enabled }, true)}>{dsp.comp.enabled ? 'On' : 'Off'}</button
        >
      </div>
      <div class="knobs">
        <Knob label="Threshold" value={dsp.comp.threshold_db} min={L.COMP_THRESHOLD_DB[0]} max={L.COMP_THRESHOLD_DB[1]} reset={0} step={0.5} format={(v) => `${v.toFixed(1)} dB`} disabled={off} onchange={(v, f) => store.setComp(ch, { threshold_db: v }, f)} />
        <Knob label="Ratio" value={dsp.comp.ratio} min={L.COMP_RATIO[0]} max={L.COMP_RATIO[1]} reset={2} step={0.1} format={(v) => `${v.toFixed(1)}:1`} disabled={off} onchange={(v, f) => store.setComp(ch, { ratio: v }, f)} />
        <Knob label="Attack" value={dsp.comp.attack_ms} min={L.ATTACK_MS[0]} max={L.ATTACK_MS[1]} reset={10} log format={ms} disabled={off} onchange={(v, f) => store.setComp(ch, { attack_ms: v }, f)} />
        <Knob label="Release" value={dsp.comp.release_ms} min={L.RELEASE_MS[0]} max={L.RELEASE_MS[1]} reset={100} log format={ms} disabled={off} onchange={(v, f) => store.setComp(ch, { release_ms: v }, f)} />
        <Knob label="Makeup" value={dsp.comp.gain_db} min={L.COMP_GAIN_DB[0]} max={L.COMP_GAIN_DB[1]} reset={0} step={0.5} format={(v) => `${v.toFixed(1)} dB`} disabled={off} onchange={(v, f) => store.setComp(ch, { gain_db: v }, f)} />
      </div>
    </section>

    <section>
      <div class="sh">
        <h3>{S.gate}</h3>
        <button
          class="b"
          class:on={dsp.gate.enabled}
          aria-pressed={dsp.gate.enabled}
          aria-label="{S.gate} enable"
          disabled={off}
          onclick={() => store.setGate(ch, { enabled: !dsp.gate.enabled }, true)}>{dsp.gate.enabled ? 'On' : 'Off'}</button
        >
      </div>
      <div class="knobs">
        <Knob label="Threshold" value={dsp.gate.threshold_db} min={L.GATE_THRESHOLD_DB[0]} max={L.GATE_THRESHOLD_DB[1]} reset={-60} step={0.5} format={(v) => `${v.toFixed(1)} dB`} disabled={off} onchange={(v, f) => store.setGate(ch, { threshold_db: v }, f)} />
        <Knob label="Attack" value={dsp.gate.attack_ms} min={L.ATTACK_MS[0]} max={L.ATTACK_MS[1]} reset={1} log format={ms} disabled={off} onchange={(v, f) => store.setGate(ch, { attack_ms: v }, f)} />
        <Knob label="Release" value={dsp.gate.release_ms} min={L.RELEASE_MS[0]} max={L.RELEASE_MS[1]} reset={100} log format={ms} disabled={off} onchange={(v, f) => store.setGate(ch, { release_ms: v }, f)} />
      </div>
    </section>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: #0007;
    border: 0;
    z-index: 40;
    cursor: default;
  }
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(680px, 100vw);
    z-index: 41;
    overflow-y: auto;
    background: var(--bg-alt);
    border-left: 1px solid var(--border);
    padding: 12px 16px 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    animation: slide 0.18s ease-out;
  }
  @keyframes slide {
    from {
      transform: translateX(40px);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .drawer {
      animation: none;
    }
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  h3 {
    margin: 0;
    font-size: 14px;
  }
  .x {
    min-width: 40px;
    min-height: 40px;
    background: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  section {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .sh,
  .hpf {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .sh {
    justify-content: space-between;
  }
  .bands {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 8px;
  }
  .band {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .band.sel {
    border-color: var(--cyan);
  }
  .band :global(input) {
    width: 64px;
  }
  .knobs {
    display: flex;
    flex-wrap: wrap;
    gap: 18px;
  }
  .b {
    min-width: 44px;
    min-height: 36px;
    padding: 0 12px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .b.on {
    background: var(--cyan);
    border-color: var(--cyan);
    color: #111;
    font-weight: 700;
  }
  .b:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
