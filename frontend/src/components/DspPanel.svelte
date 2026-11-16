<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { DSP_LIMITS } from '../lib/protocol';
  import { formatDb, gainToDb } from '../lib/fader';
  import EqCurve from './EqCurve.svelte';
  import EqBands from './EqBands.svelte';
  import Knob from './Knob.svelte';

  // DSP exists only on the analog inputs (input EQ + compressor) and the mic
  // inputs (gate); the strip only offers the drawer on those.
  const ch = $derived(store.dspChannel);
  const comp = $derived(ch !== null ? store.state?.compressors[ch] : undefined);
  const gate = $derived(ch !== null && store.hasGate(ch) ? store.state?.gates[ch] : undefined);
  const off = $derived(!store.ready);
  const hint = $derived(store.hint('input_eq_freq'));
  let band = $state(0);
  const L = DSP_LIMITS;

  const close = () => (store.dspChannel = null);
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') close();
  }
  const ms = (v: number) => `${Math.round(v)} ms`;
  const db = (v: number) => `${v.toFixed(1)} dB`;
  /** Make-up gain is linear on the MOTU; shown in dB. */
  const makeupText = (g: number) => `${formatDb(gainToDb(g))} dB`;
</script>

<svelte:window onkeydown={key} />

{#if ch !== null && store.hasDsp(ch) && comp}
  <button class="backdrop" aria-label={S.close} onclick={close} tabindex="-1"></button>
  <div class="drawer" role="dialog" aria-label="{S.dsp} {store.labelName(ch)}">
    <header>
      <h2>{store.labelName(ch)} — {S.dsp}</h2>
      <button class="x" onclick={close} aria-label={S.close}>✕</button>
    </header>
    {#if hint}<p class="hint">{hint}</p>{/if}

    <section>
      <h3>{S.eq}</h3>
      <EqCurve kind="input" {ch} bind:selected={band} height={180} />
      <EqBands kind="input" {ch} bind:selected={band} />
    </section>

    <section>
      <div class="sh">
        <h3>{S.compressor}</h3>
        <button
          class="b"
          class:on={!comp.bypass}
          aria-pressed={!comp.bypass}
          aria-label="{S.compressor} {comp.bypass ? 'bypassed' : 'active'}"
          disabled={off}
          onclick={() => store.set({ param: 'comp_bypass', ch, on: !comp.bypass }, true)}>{comp.bypass ? S.bypass : 'On'}</button
        >
      </div>
      <div class="knobs">
        <Knob label="Threshold" value={comp.threshold_db} min={-60} max={0} reset={-10} step={0.5} size={36} format={db} disabled={off} onchange={(v, f) => store.set({ param: 'comp_threshold', ch, db: v }, f)} />
        <Knob label="Ratio" value={comp.ratio} min={L.COMP_RATIO[0]} max={20} reset={2} step={0.1} log size={36} format={(v) => `${v.toFixed(1)}:1`} disabled={off} onchange={(v, f) => store.set({ param: 'comp_ratio', ch, ratio: v }, f)} />
        <Knob label="Attack" value={Math.max(comp.attack_ms, 1)} min={1} max={L.ATTACK_MS[1]} reset={10} step={1} log size={36} format={ms} disabled={off} onchange={(v, f) => store.set({ param: 'comp_attack', ch, ms: Math.round(v) }, f)} />
        <Knob label="Release" value={Math.max(comp.release_ms, 1)} min={1} max={L.RELEASE_MS[1]} reset={1000} step={1} log size={36} format={ms} disabled={off} onchange={(v, f) => store.set({ param: 'comp_release', ch, ms: Math.round(v) }, f)} />
        <Knob label="Makeup" value={comp.makeup} min={1} max={L.COMP_MAKEUP[1]} reset={1} log size={36} format={makeupText} disabled={off} onchange={(v, f) => store.set({ param: 'comp_makeup', ch, gain: v }, f)} />
      </div>
    </section>

    {#if gate}
      <section>
        <div class="sh">
          <h3>{S.gate}</h3>
          <button
            class="b"
            class:on={!gate.bypass}
            aria-pressed={!gate.bypass}
            aria-label="{S.gate} {gate.bypass ? 'bypassed' : 'active'}"
            disabled={off}
            onclick={() => store.set({ param: 'gate_bypass', ch, on: !gate.bypass }, true)}>{gate.bypass ? S.bypass : 'On'}</button
          >
        </div>
        <div class="knobs">
          <Knob label="Threshold" value={gate.threshold_db} min={-80} max={0} reset={-46} step={0.5} size={36} format={db} disabled={off} onchange={(v, f) => store.set({ param: 'gate_threshold', ch, db: v }, f)} />
          <Knob label="Attack" value={Math.max(gate.attack_ms, 1)} min={1} max={L.ATTACK_MS[1]} reset={10} step={1} log size={36} format={ms} disabled={off} onchange={(v, f) => store.set({ param: 'gate_attack', ch, ms: Math.round(v) }, f)} />
          <Knob label="Release" value={Math.max(gate.release_ms, 1)} min={1} max={L.RELEASE_MS[1]} reset={1000} step={1} log size={36} format={ms} disabled={off} onchange={(v, f) => store.set({ param: 'gate_release', ch, ms: Math.round(v) }, f)} />
        </div>
      </section>
    {/if}
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
    width: min(620px, 100vw);
    z-index: 41;
    overflow-y: auto;
    background: var(--bg-alt);
    border-left: 1px solid var(--border);
    padding: 10px 14px 20px;
    display: flex;
    flex-direction: column;
    gap: 10px;
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
    font-size: 15px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .x {
    min-width: 36px;
    min-height: 36px;
    background: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  section {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .sh {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .knobs {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }
  .b {
    min-width: 44px;
    min-height: 30px;
    padding: 0 10px;
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
