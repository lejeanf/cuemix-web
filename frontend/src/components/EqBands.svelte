<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { DSP_LIMITS, EQ_MODE, eqModeName, eqModesForBand, type EqBand } from '../lib/protocol';
  import { EQ_COLORS } from '../lib/eq';

  /** One column per band, as in CueMix 5: on/off, frequency, gain, Q and the band's filter types. */
  let { kind, ch, selected = $bindable(0) }: { kind: 'input' | 'output'; ch: number; selected?: number } = $props();

  const bands: EqBand[] | undefined = $derived(kind === 'input' ? store.state?.input_eq[ch] : store.state?.output_eq[ch]);
  const off = $derived(!store.ready);
  const L = DSP_LIMITS;
  /** The band's types, plus its own value when the MOTU reports one it does not offer. */
  const modes = (band: number, m: number) => {
    const known = eqModesForBand(band, bands?.length ?? 0);
    return known.includes(m) ? known : [...known, m];
  };
  /** Small curve icon per filter type (24 × 14 box). */
  const ICON: Record<number, string> = {
    [EQ_MODE.BELL]: 'M2 12 C8 12 9 2 12 2 C15 2 16 12 22 12',
    [EQ_MODE.LOW_SHELF]: 'M2 3 H8 C11 3 12 11 15 11 H22',
    [EQ_MODE.HIGH_SHELF]: 'M2 11 H9 C12 11 13 3 16 3 H22',
    [EQ_MODE.HIGH_PASS]: 'M3 13 C6 4 9 3 13 3 H22',
  };

  function commit(e: Event, band: number, field: 'freq_hz' | 'gain_db' | 'bw_oct', [min, max]: readonly number[]) {
    const el = e.currentTarget as HTMLInputElement;
    const v = Number(el.value);
    if (el.value.trim() === '' || !Number.isFinite(v)) {
      el.value = String(bands?.[band]?.[field] ?? '');
      return;
    }
    store.setEq(kind, ch, band, { [field]: Math.min(max, Math.max(min, v)) }, true);
  }
  const fmt = (v: number, d: number) => (Math.round(v * 10 ** d) / 10 ** d).toString();
</script>

{#if bands}
  <div class="bands" role="group" aria-label="EQ bands" style:grid-template-columns="repeat({bands.length}, minmax(0, 1fr))">
    {#each bands as b, i (i)}
      {@const color = EQ_COLORS[i % EQ_COLORS.length]}
      <div class="band" class:sel={selected === i} class:byp={b.bypass} style:--c={color} role="group" aria-label="{S.band} {i + 1}">
        <div class="head">
          <button
            class="pw"
            class:on={!b.bypass}
            aria-pressed={!b.bypass}
            aria-label="{S.band} {i + 1} {b.bypass ? 'off' : 'on'}"
            title={b.bypass ? 'Band off' : 'Band on'}
            disabled={off}
            onclick={() => store.setEq(kind, ch, i, { bypass: !b.bypass }, true)}
          >
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 4.5 A5 5 0 1 0 11 4.5 M8 2 V8" /></svg>
          </button>
          <button class="line" aria-label="Select {S.band} {i + 1}" onclick={() => (selected = i)}></button>
        </div>
        <label class="f"><span>Freq</span>
          <input type="number" value={fmt(b.freq_hz, 0)} min={L.FREQ_HZ[0]} max={L.FREQ_HZ[1]} step="1" disabled={off}
            aria-label="{S.band} {i + 1} frequency" onfocus={() => (selected = i)} onchange={(e) => commit(e, i, 'freq_hz', L.FREQ_HZ)} /><em>Hz</em>
        </label>
        <label class="f" class:na={b.mode === EQ_MODE.HIGH_PASS}><span>Gain</span>
          <input type="number" value={fmt(b.gain_db, 1)} min={L.EQ_GAIN_DB[0]} max={L.EQ_GAIN_DB[1]} step="0.1" disabled={off || b.mode === EQ_MODE.HIGH_PASS}
            aria-label="{S.band} {i + 1} gain" onfocus={() => (selected = i)} onchange={(e) => commit(e, i, 'gain_db', L.EQ_GAIN_DB)} /><em>dB</em>
        </label>
        <label class="f" class:na={b.mode !== EQ_MODE.BELL}><span>Q</span>
          <input type="number" value={fmt(b.bw_oct, 2)} min={L.EQ_BW_OCT[0]} max={L.EQ_BW_OCT[1]} step="0.01" disabled={off || b.mode !== EQ_MODE.BELL}
            aria-label="{S.band} {i + 1} Q" onfocus={() => (selected = i)} onchange={(e) => commit(e, i, 'bw_oct', L.EQ_BW_OCT)} /><em></em>
        </label>
        {#if modes(i, b.mode).length > 1}
          <div class="types" role="radiogroup" aria-label="{S.band} {i + 1} type">
            {#each modes(i, b.mode) as m (m)}
              <button class="ty" class:on={b.mode === m} role="radio" aria-checked={b.mode === m} title={eqModeName(m)} aria-label={eqModeName(m)} disabled={off}
                onclick={() => store.setEq(kind, ch, i, { mode: m }, true)}>
                {#if ICON[m]}<svg viewBox="0 0 24 14" aria-hidden="true"><path d={ICON[m]} /></svg>{:else}{eqModeName(m)}{/if}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .bands {
    display: grid;
    gap: 6px;
  }
  .band {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 5px 6px 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 12px;
    min-width: 0;
  }
  .band.sel {
    border-color: var(--c);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .pw {
    width: 24px;
    height: 24px;
    padding: 0;
    flex: none;
    background: none;
    border: 0;
    cursor: pointer;
    color: var(--text-dim);
  }
  .pw.on {
    color: var(--c);
  }
  .pw svg,
  .ty svg {
    width: 100%;
    height: 100%;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
  }
  .line {
    flex: 1;
    height: 5px;
    padding: 0;
    border: 0;
    border-radius: 3px;
    background: var(--c);
    cursor: pointer;
  }
  .byp .line {
    opacity: 0.3;
  }
  .f {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr) 20px;
    align-items: center;
    gap: 3px;
    color: var(--text-dim);
  }
  .f.na input {
    opacity: 0.35;
  }
  .f input {
    width: 100%;
    min-width: 0;
    min-height: 24px;
    padding: 0 4px;
    background: #0d0e11;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    font: inherit;
    font-variant-numeric: tabular-nums;
  }
  .f em {
    font-style: normal;
    font-size: 11px;
  }
  .types {
    display: flex;
    gap: 3px;
    margin-top: 2px;
  }
  .ty {
    width: 32px;
    height: 22px;
    padding: 3px 4px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-dim);
    cursor: pointer;
  }
  .ty.on {
    color: var(--c);
    border-color: var(--c);
  }
  button:disabled,
  input:disabled {
    cursor: not-allowed;
  }
</style>
