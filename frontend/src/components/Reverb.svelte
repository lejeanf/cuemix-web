<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { DSP_LIMITS } from '../lib/protocol';
  import { formatDb, gainToDb, gainToPos, posToGain } from '../lib/fader';
  import Fader from './Fader.svelte';
  import Knob from './Knob.svelte';

  const st = $derived(store.state);
  const rv = $derived(st?.reverb);
  const off = $derived(!store.ready);
</script>

{#if st && rv}
  <div class="panel">
    <div class="head">
      <button
        class="b"
        class:on={rv.enabled}
        aria-pressed={rv.enabled}
        aria-label="{S.reverb} {rv.enabled ? 'on' : 'off'}"
        disabled={off}
        data-unmapped={store.unmapped('reverb') ? '' : undefined}
        title={store.hint('reverb')}
        onclick={() => store.setReverb({ enabled: !rv.enabled }, true)}>{S.reverb}</button
      >
      <Knob
        value={rv.time_s}
        min={DSP_LIMITS.REVERB_TIME_S[0]}
        max={DSP_LIMITS.REVERB_TIME_S[1]}
        reset={2}
        log
        label={S.reverbTime}
        format={(v) => `${v.toFixed(1)} s`}
        disabled={off}
        hint={store.hint('reverb')}
        onchange={(v, f) => store.setReverb({ time_s: Math.round(v * 10) / 10 }, f)}
      />
      <Knob
        value={rv.predelay_ms}
        min={DSP_LIMITS.REVERB_PREDELAY_MS[0]}
        max={DSP_LIMITS.REVERB_PREDELAY_MS[1]}
        reset={0}
        step={1}
        label={S.reverbPredelay}
        format={(v) => `${Math.round(v)} ms`}
        disabled={off}
        hint={store.hint('reverb')}
        onchange={(v, f) => store.setReverb({ predelay_ms: v }, f)}
      />
      <div class="ret">
        <Fader
          pos={gainToPos(rv.return_gain)}
          label={S.reverbReturn}
          valueText="{formatDb(gainToDb(rv.return_gain))} dB"
          mark={0.75}
          disabled={off}
          hint={store.hint('reverb')}
          onchange={(p, f) => store.setReverb({ return_gain: posToGain(p) }, f)}
        />
        <span class="cap">{S.reverbReturn} {formatDb(gainToDb(rv.return_gain))}</span>
      </div>
    </div>

    <h3>{S.reverbSends}</h3>
    <div class="sends">
      {#each rv.sends as g, ch (ch)}
        <div class="send">
          <Fader
            pos={gainToPos(g)}
            label="{S.reverbSends} {store.labelName(ch)}"
            valueText="{formatDb(gainToDb(g))} dB"
            mark={0.75}
            disabled={off}
            hint={store.hint('reverb_send')}
            onchange={(p, f) => store.set({ param: 'reverb_send', ch, gain: posToGain(p) }, f)}
          />
          <span class="cap">{formatDb(gainToDb(g))}</span>
          <span class="nm" style:border-color={store.label(ch)?.color ?? 'transparent'}>{store.labelName(ch)}</span>
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    height: 100%;
    min-height: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 20px;
    flex-wrap: wrap;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 14px;
  }
  .ret {
    display: flex;
    flex-direction: column;
    align-items: center;
    height: 160px;
    width: 70px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    color: var(--text-dim);
  }
  .sends {
    flex: 1;
    min-height: 200px;
    display: flex;
    gap: 6px;
    overflow-x: auto;
  }
  .send {
    flex: none;
    width: 72px;
    display: flex;
    flex-direction: column;
    align-items: center;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 4px;
    gap: 4px;
  }
  .cap {
    font-size: 11px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .nm {
    font-size: 12px;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 3px solid transparent;
  }
  .b {
    min-height: 40px;
    padding: 0 16px;
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
