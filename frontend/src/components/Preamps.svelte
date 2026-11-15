<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { LINE_GAIN_MAX_DB, PREAMP_GAIN_MAX_DB } from '../lib/protocol';
  import Knob from './Knob.svelte';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  let confirming = $state<Record<number, boolean>>({});

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
  <div class="grid">
    {#each st.preamps as pa, ch (ch)}
      {@const mic = ch < st.topology.mic_inputs}
      <section class="card" aria-label="Preamp {store.labelName(ch)}">
        <div class="bar" style:background={store.label(ch)?.color ?? 'transparent'}></div>
        <h3>{store.labelName(ch)}</h3>
        <Knob
          value={pa.gain_db}
          min={0}
          max={mic ? PREAMP_GAIN_MAX_DB : LINE_GAIN_MAX_DB}
          reset={0}
          step={1}
          size={56}
          label="Gain {store.labelName(ch)}"
          format={(v) => `${Math.round(v)} dB`}
          disabled={off}
          hint={store.hint('preamp_gain')}
          onchange={(v, f) => store.set({ param: 'preamp_gain', ch, db: Math.round(v) }, f)}
        />
        <div class="btns">
          <button
            class="b"
            class:on={pa.phase}
            aria-pressed={pa.phase}
            aria-label="{S.phase} {store.labelName(ch)}"
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
      </section>
    {/each}
  </div>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 10px;
    align-content: start;
  }
  .card {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0 10px 10px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    overflow: hidden;
  }
  .bar {
    align-self: stretch;
    height: 5px;
    margin: 0 -10px;
  }
  h3 {
    margin: 0;
    font-size: 14px;
  }
  .btns,
  .row {
    display: flex;
    gap: 6px;
  }
  .b {
    min-width: 40px;
    min-height: 38px;
    padding: 0 8px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
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
    border: 1px solid var(--red);
    border-radius: 6px;
    padding: 6px;
    font-size: 12px;
  }
  .confirm p {
    margin: 0 0 6px;
  }
</style>
