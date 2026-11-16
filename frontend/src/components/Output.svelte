<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb } from '../lib/fader';
  import { OUTPUT_EQ_PRESETS, matchPreset, presetBands } from '../lib/eqPresets';
  import Knob from './Knob.svelte';
  import Meter from './Meter.svelte';
  import { METER_SLOT, OUTPUTS } from '../lib/meterMap';
  import EqCurve from './EqCurve.svelte';
  import EqBands from './EqBands.svelte';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  const dbText = (v: number) => `${formatDb(v, 0)} dB`;
  let eqOut = $state(0);
  let eqBand = $state(0);
  const eqHint = $derived(store.hint('output_eq_freq'));
  // The EQ curve fills the free height: its viewBox (640 wide) follows the box's aspect ratio.
  let curveW = $state(0);
  let curveH = $state(0);
  const curveHeight = $derived(curveW > 0 ? Math.min(480, Math.max(90, Math.round((640 * curveH) / curveW))) : 150);

  /** CueMix 5 names each output pair after the mix that feeds it by default. */
  function pairName(ch: number): string {
    return store.device?.mix_names[Math.floor(ch / 2)] ?? `Out ${(ch & ~1) + 1}-${(ch & ~1) + 2}`;
  }
  /** Trims come in pairs (left, right). */
  const trimPairs = $derived.by(() => {
    const t = st?.output_trims ?? [];
    const out: number[] = [];
    for (let ch = 0; ch < t.length; ch += 2) out.push(ch);
    return out;
  });

  const pair = $derived([eqOut & ~1, (eqOut & ~1) + 1].filter((c) => c < (st?.output_eq.length ?? 0)));
  const current = $derived(matchPreset(st?.output_eq[eqOut]));

  /** Applies a preset to both outputs of the selected pair. */
  function applyPreset(id: string) {
    const p = OUTPUT_EQ_PRESETS.find((x) => x.id === id);
    if (!p) return;
    const bands = presetBands(p);
    for (const ch of pair) bands.forEach((band, i) => store.setEq('output', ch, i, band, true));
  }
</script>

{#if st}
  <div class="out">
    <div class="top">
      <section class="card mon" aria-label={S.monitoring}>
        <h3>{S.monitoring}</h3>
        <div class="row">
          <Knob
            value={st.main.db}
            min={TRIM_MIN_DB}
            max={0}
            reset={0}
            step={1}
            label={S.monitor}
            ariaLabel="{S.monitor} level"
            size={48}
            format={dbText}
            disabled={off}
            hint={store.hint('main_vol')}
            onchange={(v, f) => store.set({ param: 'main_vol', db: Math.round(v) }, f)}
          />
          <div class="km" title="Main output level (before the volume)">
            <Meter index={METER_SLOT.output(0)} label="Main L meter" />
            <Meter index={METER_SLOT.output(1)} label="Main R meter" />
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
          <Knob
            value={st.control_room.phones_db}
            min={TRIM_MIN_DB}
            max={0}
            reset={0}
            step={1}
            label={S.phones}
            ariaLabel="{S.phones} level"
            size={48}
            format={dbText}
            disabled={off}
            hint={store.hint('phones_vol')}
            onchange={(v, f) => store.set({ param: 'phones_vol', db: Math.round(v) }, f)}
          />
          <div class="km" title="Phones level (before the volume)">
            <Meter index={METER_SLOT.output(10)} label="Phones L meter" />
            <Meter index={METER_SLOT.output(11)} label="Phones R meter" />
          </div>
        </div>
      </section>

      <section class="card trimcard" aria-label={S.outputTrim}>
        <h3>{S.outputTrim}</h3>
        <div class="trims">
          {#each trimPairs as ch0 (ch0)}
            <div class="tp">
              <span class="pn" title={pairName(ch0)}>{pairName(ch0)}</span>
              <div class="tk">
                {#each [ch0, ch0 + 1].filter((c) => c < st.output_trims.length) as ch (ch)}
                  <Knob
                    value={st.output_trims[ch]}
                    min={TRIM_MIN_DB}
                    max={0}
                    reset={0}
                    step={1}
                    size={32}
                    label={String(ch + 1)}
                    ariaLabel="{S.outputTrim} output {ch + 1} ({pairName(ch)})"
                    format={dbText}
                    disabled={off}
                    hint={store.hint('output_trim')}
                    onchange={(v, f) => store.set({ param: 'output_trim', ch, db: Math.round(v) }, f)}
                  />
                {/each}
                <div class="vm" title="Output level before the trim">
                  {#each [ch0, ch0 + 1].filter((c) => c < OUTPUTS) as ch (ch)}
                    <Meter index={METER_SLOT.output(ch)} label="Output {ch + 1} meter" />
                  {/each}
                </div>
              </div>
            </div>
          {/each}
        </div>
      </section>
    </div>

    {#if st.output_eq.length}
      <section class="card eqcard" aria-label={S.outputEq}>
        <div class="eqhead">
          <h3>{S.outputEq}</h3>
          <div class="outs" role="tablist" aria-label="{S.outputEq} output">
            {#each st.output_eq as bands, ch (ch)}
              <button
                role="tab"
                class="o"
                class:sel={eqOut === ch}
                class:act={bands.some((b) => !b.bypass && b.gain_db !== 0)}
                class:gap={ch % 2 === 0 && ch > 0}
                aria-selected={eqOut === ch}
                title="{pairName(ch)} ({ch % 2 ? 'R' : 'L'})"
                onclick={() => (eqOut = ch)}>{ch + 1}</button
              >
            {/each}
          </div>
          <label class="preset">
            {S.eqPreset}
            <select
              value={current?.id ?? ''}
              disabled={off}
              title="Applies to outputs {pair.map((c) => c + 1).join(' and ')} ({pairName(eqOut)})"
              onchange={(e) => applyPreset(e.currentTarget.value)}
            >
              <option value="" disabled>{S.eqPresetCustom}</option>
              {#each OUTPUT_EQ_PRESETS as p (p.id)}<option value={p.id} title={p.info}>{p.name}</option>{/each}
            </select>
          </label>
          <span class="for">{pairName(eqOut)} · {pair.map((c) => c + 1).join('-')}</span>
          {#if eqHint}<span class="sub">{eqHint}</span>{/if}
        </div>
        {#if current}<p class="info">{current.info}</p>{/if}
        <div class="eqbody">
          <div class="curve" bind:clientWidth={curveW} bind:clientHeight={curveH}>
            <EqCurve kind="output" ch={eqOut} bind:selected={eqBand} height={curveHeight} />
          </div>
          <div class="bandcol"><EqBands kind="output" ch={eqOut} bind:selected={eqBand} /></div>
        </div>
      </section>
    {/if}
  </div>
{/if}

<style>
  .out {
    display: flex;
    flex-direction: column;
    gap: 8px;
    height: 100%;
    min-height: 0;
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .card {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 6px 10px 8px;
  }
  .trimcard {
    flex: 1;
    min-width: 0;
  }
  h3 {
    margin: 0 0 4px;
    font-size: 13px;
    color: var(--text-dim);
    font-weight: 600;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .btns {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .t {
    min-width: 72px;
    min-height: 26px;
    padding: 0 8px;
    font-size: 12px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
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
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .tp {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 2px 4px 4px;
    border: 1px solid var(--border);
    border-radius: 6px;
    min-width: 0;
  }
  .pn {
    font-size: 11px;
    color: var(--text-dim);
    max-width: 100px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tk {
    display: flex;
    gap: 4px;
  }
  .km,
  .vm {
    display: flex;
    gap: 2px;
    align-self: stretch;
  }
  .km {
    height: 72px;
    align-self: center;
  }
  .vm :global(canvas),
  .km :global(canvas) {
    min-height: 0;
  }
  .eqhead {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
    margin-bottom: 4px;
  }
  .eqhead h3 {
    margin: 0;
  }
  .outs {
    display: flex;
    gap: 2px;
  }
  .o {
    min-width: 26px;
    min-height: 24px;
    font-size: 11px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 5px;
    cursor: pointer;
  }
  .o.gap {
    margin-left: 4px;
  }
  .o.act {
    color: var(--cyan);
  }
  .o.sel {
    border-color: var(--cyan);
    font-weight: 700;
  }
  .preset {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .preset select {
    min-height: 26px;
    font-size: 12px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .for,
  .sub,
  .info {
    font-size: 11px;
    color: var(--text-dim);
  }
  .info {
    margin: 0 0 4px;
  }
  .eqcard {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .eqbody {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .curve {
    flex: 1;
    min-width: 0;
    min-height: 110px;
    position: relative;
  }
  .curve :global(.eq) {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    max-height: none !important;
  }
  .bandcol {
    flex: none;
  }
</style>
