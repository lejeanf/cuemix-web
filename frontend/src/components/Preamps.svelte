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
  // Digital inputs are mixer inputs 8-9 (S/PDIF) and 10-17 (optical).
  const SPDIF_IN = [8, 9];
  const OPTICAL_IN = [10, 11, 12, 13, 14, 15, 16, 17];
  const opticalIn = $derived(st?.clock.optical_in ?? 'adat');
  /** TOSLINK carries a stereo pair: only the first two optical channels are used. */
  const opticalShown = $derived(opticalIn === 'toslink' ? OPTICAL_IN.slice(0, 2) : OPTICAL_IN);
  function setOptical(mode: 'adat' | 'toslink') {
    store.set({ param: 'optical_mode', output: false, mode }, true);
  }
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
      <div class="chans" role="group" aria-label={g.title}>
          {#each g.items as { pa, ch } (ch)}
            {@const mic = ch < nMic}
            <div class="ch" aria-label="Preamp {store.labelName(ch)}" role="group">
              <div class="bar" style:background={store.label(ch)?.color ?? 'transparent'}></div>
              <span class="nm" title={store.labelName(ch)}>{store.labelName(ch)}</span>
              <div class="kr">
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
              <div class="vm" title="Input level after the preamp (before EQ and dynamics)">
                <Meter index={METER_SLOT.preamp(ch)} label="Input meter {store.labelName(ch)}" />
              </div>
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
    {/each}
    {#if st.topology.mixer_inputs >= 18}
      <div class="ch" role="group" aria-label="S/PDIF">
        <div class="bar"></div>
        <span class="nm">S/PDIF</span>
        <div class="dig" title="S/PDIF input level">
          {#each SPDIF_IN as ch (ch)}<Meter index={METER_SLOT.mixerInput(ch)} label="S/PDIF {ch - 7} meter" />{/each}
        </div>
      </div>
      <div class="ch" role="group" aria-label="Optical in">
        <div class="bar"></div>
        <span class="nm">Optical in</span>
        <div class="optrow">
          <div class="fmt" role="radiogroup" aria-label="Optical input format">
            <span class="lbl">Format</span>
            {#each [['adat', 'ADAT'], ['toslink', 'TOS']] as [m, label] (m)}
              <button
                class="b"
                class:on={opticalIn === m}
                role="radio"
                aria-checked={opticalIn === m}
                disabled={off}
                title={m === 'adat' ? 'ADAT: 8 channels' : 'TOSLINK: optical S/PDIF, stereo'}
                onclick={() => setOptical(m as 'adat' | 'toslink')}>{label}</button
              >
            {/each}
          </div>
          <div class="dig" title="Optical input level">
            {#each opticalShown as ch (ch)}<Meter index={METER_SLOT.mixerInput(ch)} label="Optical {ch - 9} meter" />{/each}
          </div>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .groups {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: stretch;
  }
  /* Invisible holder: keeps mics and lines together when the row wraps. */
  .chans {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: stretch;
  }
  /* Phones: each group takes the full width and its cards share it evenly, so
     rows end flush instead of leaving ragged gaps. */
  @media (max-width: 640px) {
    .chans {
      flex: 1 1 100%;
    }
    /* 96 px basis: three per row at 375 px, so the six line inputs split 3 + 3. */
    .chans > .ch {
      flex: 1 1 96px;
    }
    .groups > .ch:last-child {
      flex: 1;
    }
  }
  .ch {
    position: relative;
    min-width: 64px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 0 4px 5px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
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
  .dig {
    flex: 1;
    display: flex;
    gap: 3px;
    min-height: 60px;
    padding: 2px 4px;
  }
  .dig :global(canvas) {
    min-height: 0;
  }
  .optrow {
    flex: 1;
    display: flex;
    gap: 10px;
    align-items: stretch;
  }
  .fmt {
    display: flex;
    flex-direction: column;
    gap: 4px;
    justify-content: center;
  }
  .lbl {
    font-size: 11px;
    color: var(--text-dim);
  }
  .kr {
    display: flex;
    gap: 4px;
    align-items: stretch;
  }
  .vm {
    display: flex;
  }
  .vm :global(canvas) {
    min-height: 0;
  }
  .btns,
  .row {
    display: flex;
    gap: 3px;
  }
  .btns {
    margin-top: auto;
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
    color: var(--on-accent);
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
  /* Touch screens: controls at least 44 px tall (the mouse keeps the dense layout). */
  @media (pointer: coarse) {
    .b {
      min-height: 40px;
      min-width: 36px;
    }
  }
</style>
