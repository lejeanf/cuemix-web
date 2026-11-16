<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { DSP_LIMITS, EQ_MODE, eqModeName, eqModesForBand, type EqBand } from '../lib/protocol';
  import NumField from './NumField.svelte';

  /** Compact per-band table (type, bypass, frequency, gain, bandwidth) of one input or output EQ. */
  let { kind, ch, selected = $bindable(0) }: { kind: 'input' | 'output'; ch: number; selected?: number } = $props();

  const bands: EqBand[] | undefined = $derived(kind === 'input' ? store.state?.input_eq[ch] : store.state?.output_eq[ch]);
  const off = $derived(!store.ready);
  const L = DSP_LIMITS;
  /** The band's types, plus its own value when the MOTU reports one it does not offer. */
  const modes = (band: number, m: number) => {
    const known = eqModesForBand(band, bands?.length ?? 0);
    return known.includes(m) ? known : [...known, m];
  };
</script>

{#if bands}
  <div class="bands" role="group" aria-label="EQ bands">
    {#each bands as b, i (i)}
      <div class="band" class:sel={selected === i} class:byp={b.bypass}>
        <button class="n" aria-label="{S.band} {i + 1}" aria-pressed={selected === i} onclick={() => (selected = i)}>{i + 1}</button>
        <select
          aria-label="{S.band} {i + 1} type"
          value={b.mode}
          disabled={off}
          onchange={(e) => store.setEq(kind, ch, i, { mode: Number(e.currentTarget.value) }, true)}
        >
          {#each modes(i, b.mode) as m (m)}<option value={m}>{eqModeName(m)}</option>{/each}
        </select>
        <button
          class="t"
          class:on={!b.bypass}
          aria-pressed={!b.bypass}
          aria-label="{S.band} {i + 1} {b.bypass ? 'bypassed' : 'active'}"
          disabled={off}
          onclick={() => store.setEq(kind, ch, i, { bypass: !b.bypass }, true)}>{b.bypass ? 'Off' : 'On'}</button
        >
        <NumField label="" ariaLabel="{S.band} {i + 1} frequency" unit="Hz" value={b.freq_hz} min={L.FREQ_HZ[0]} max={L.FREQ_HZ[1]} step={1} disabled={off} onchange={(v) => store.setEq(kind, ch, i, { freq_hz: v }, true)} />
        {#if b.mode !== EQ_MODE.HIGH_PASS}
          <NumField label="" ariaLabel="{S.band} {i + 1} gain" unit="dB" value={b.gain_db} min={L.EQ_GAIN_DB[0]} max={L.EQ_GAIN_DB[1]} step={0.1} disabled={off} onchange={(v) => store.setEq(kind, ch, i, { gain_db: v }, true)} />
          {#if b.mode === EQ_MODE.BELL}
            <NumField label="" ariaLabel="{S.band} {i + 1} Q" unit="Q" value={b.bw_oct} min={L.EQ_BW_OCT[0]} max={L.EQ_BW_OCT[1]} step={0.01} disabled={off} onchange={(v) => store.setEq(kind, ch, i, { bw_oct: v }, true)} />
          {/if}
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .bands {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 4px 8px;
  }
  .band {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 6px;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 12px;
  }
  .band.sel {
    border-color: var(--cyan);
  }
  .band.byp {
    opacity: 0.6;
  }
  .band :global(input) {
    width: 58px;
  }
  .n,
  .t,
  select {
    min-height: 28px;
    font-size: 12px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .n {
    min-width: 28px;
    cursor: pointer;
    font-weight: 700;
  }
  .t {
    min-width: 36px;
    cursor: pointer;
  }
  .t.on {
    background: var(--cyan);
    border-color: var(--cyan);
    color: #111;
    font-weight: 700;
  }
  .t:disabled,
  select:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
