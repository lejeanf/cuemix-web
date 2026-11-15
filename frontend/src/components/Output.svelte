<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb } from '../lib/fader';
  import Knob from './Knob.svelte';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  const dbText = (v: number) => `${formatDb(v, 0)} dB`;

  /** CueMix 5 names each analog output pair after the mix that feeds it by default. */
  function outputName(ch: number): string {
    const mix = store.device?.mix_names[Math.floor(ch / 2)];
    return mix ? `${mix} Mix` : `Output ${ch + 1}`;
  }
</script>

{#if st}
  <div class="out">
    <section class="card" aria-label={S.monitoring}>
      <h3>{S.monitoring}</h3>
      <div class="row">
        <div class="ctl">
          <Knob
            value={st.main.db}
            min={TRIM_MIN_DB}
            max={0}
            reset={0}
            step={1}
            label={S.monitor}
            ariaLabel="{S.monitor} level"
            size={64}
            format={dbText}
            disabled={off}
            hint={store.hint('main_vol')}
            onchange={(v, f) => store.set({ param: 'main_vol', db: Math.round(v) }, f)}
          />
        </div>
        <div class="btns">
          <button class="t" class:on={st.control_room.mono} aria-pressed={st.control_room.mono} disabled={off}
            onclick={() => store.set({ param: 'mono', on: !st.control_room.mono }, true)}>{S.mono}</button>
          <button class="t" class:on={st.control_room.speaker_b} aria-pressed={st.control_room.speaker_b} disabled={off}
            data-unmapped={store.unmapped('speaker_b') ? '' : undefined} title={store.hint('speaker_b')}
            onclick={() => store.set({ param: 'speaker_b', on: !st.control_room.speaker_b }, true)}
            >{S.speakerAB}: {st.control_room.speaker_b ? 'B' : 'A'}</button>
          <button class="t red" class:on={st.control_room.talkback} aria-pressed={st.control_room.talkback} disabled={off}
            onclick={() => store.set({ param: 'talkback', on: !st.control_room.talkback }, true)}>{S.talkback}</button>
        </div>
      </div>
    </section>

    <section class="card" aria-label={S.phones}>
      <h3>{S.phones}</h3>
      <div class="ctl">
        <Knob
          value={st.control_room.phones_db}
          min={TRIM_MIN_DB}
          max={0}
          reset={0}
          step={1}
          label={S.phones}
          ariaLabel="{S.phones} level"
          size={64}
          format={dbText}
          disabled={off}
          hint={store.hint('phones_vol')}
          onchange={(v, f) => store.set({ param: 'phones_vol', db: Math.round(v) }, f)}
        />
      </div>
    </section>

    <section class="card wide" aria-label={S.outputTrim}>
      <h3>{S.outputTrim}</h3>
      <div class="trims">
        {#each st.output_trims ?? [] as db, ch (ch)}
          <div class="trim">
            <span class="num">{ch + 1}</span>
            <Knob
              value={db}
              min={TRIM_MIN_DB}
              max={0}
              reset={0}
              step={1}
              label={outputName(ch)}
              ariaLabel="{S.outputTrim} output {ch + 1} ({outputName(ch)})"
              size={48}
              format={dbText}
              disabled={off}
              hint={store.hint('output_trim')}
              onchange={(v, f) => store.set({ param: 'output_trim', ch, db: Math.round(v) }, f)}
            />
          </div>
        {/each}
      </div>
    </section>
  </div>
{/if}

<style>
  .out {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 12px;
    align-content: start;
  }
  .card {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 14px 14px;
  }
  .card.wide {
    grid-column: 1 / -1;
  }
  h3 {
    margin: 0 0 10px;
    font-size: 1rem;
    color: var(--text-dim);
    font-weight: 600;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }
  .ctl {
    display: flex;
    justify-content: center;
  }
  .btns {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .t {
    min-width: 64px;
    min-height: 40px;
    padding: 0 14px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    --on: var(--cyan);
  }
  .t.red {
    --on: var(--red);
  }
  .t.on {
    background: var(--on);
    color: var(--bg);
    border-color: var(--on);
  }
  .t[data-unmapped] {
    border-style: dashed;
  }
  .trims {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 12px;
  }
  @media (max-width: 600px) {
    .trims {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  .trim {
    position: relative;
    display: flex;
    justify-content: center;
    padding: 8px 4px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .num {
    position: absolute;
    right: 6px;
    bottom: 4px;
    font-size: 0.75rem;
    color: var(--text-dim);
  }
</style>
