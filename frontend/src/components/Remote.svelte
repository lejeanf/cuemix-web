<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb } from '../lib/fader';
  import { METER_SLOT } from '../lib/meterMap';
  import { powerView, type PowerView } from '../lib/power';
  import Knob from './Knob.svelte';
  import Meter from './Meter.svelte';

  // Simple view for phones: the two rotary volumes people reach for, with
  // vertical meters, an on/off button each (fades Monitor / Phones), and preset recall.
  // Everything else is one tap away (the header's mode button).
  const cr = $derived(store.state?.control_room);
  const main = $derived(store.state?.main.db ?? TRIM_MIN_DB);
  const phones = $derived(cr?.phones_db ?? TRIM_MIN_DB);
  const off = $derived(!store.ready);
  const session = $derived(store.c.session);
  const dbText = (v: number) => `${formatDb(v, 0)} dB`;

  // Knob size follows the width so both knob + meter blocks fit side by side.
  const GAP = 10; // between the two cards
  const PAD = 10; // card padding
  const INNER = 8; // knob to meters
  const METERS_W = 2 * 12 + 2;
  let width = $state(0);
  const stacked = $derived(width > 0 && width < 340);
  const size = $derived.by(() => {
    if (width <= 0) return 96;
    const block = stacked ? width : (width - GAP) / 2;
    return Math.round(Math.min(150, Math.max(96, block - 2 * PAD - INNER - METERS_W - 2)));
  });
  const travel = $derived(Math.round(size * 2.5));

  const setVol = (which: 'main_vol' | 'phones_vol', v: number, final: boolean) =>
    store.set({ param: which, db: Math.round(v) }, final);

  // On/off per channel: the server fades and remembers the level; we show what it reports.
  const mainPower = $derived(powerView(store.c.power, 'main', main));
  const phonesPower = $derived(powerView(store.c.power, 'phones', phones));
  const powerLabel = (v: PowerView) =>
    v.fading === 'out' ? S.fadingOut : v.plug === 'warming' ? S.plug.warmingUp : v.fading === 'in' ? S.fadingIn : v.on ? S.powerOn : S.powerOff;

  // The whole card is the drag surface of its knob.
  let mainCard = $state<HTMLElement | null>(null);
  let phonesCard = $state<HTMLElement | null>(null);

  $effect(() => {
    if (store.ready) store.listPresets();
  });
</script>


{#snippet power(v: PowerView, kind: 'monitor' | 'phones')}
  <button
    class="power"
    class:on={v.on}
    class:busy={v.fading !== null}
    aria-pressed={v.on}
    aria-label={kind === 'monitor' ? (v.plug ? `${S.monitorPower}, ${S.plugState[v.plug]}` : S.monitorPower) : S.phonesPower}
    aria-busy={v.fading !== null}
    disabled={off}
    onclick={() => store.power(kind === 'monitor' ? 'main' : 'phones', !v.on)}
  >
    <svg viewBox="0 0 24 24" aria-hidden="true">
      {#if kind === 'monitor'}
        <path d="M4 9.5v5h3.5L12 18.5v-13L7.5 9.5H4z" />
        <path d="M15.5 9a4 4 0 0 1 0 6" />
        <path d="M18 6.5a7.5 7.5 0 0 1 0 11" />
      {:else}
        <path d="M4 16v-4a8 8 0 0 1 16 0v4" />
        <rect x="3" y="14" width="4" height="7" rx="1.5" />
        <rect x="17" y="14" width="4" height="7" rx="1.5" />
      {/if}
    </svg>
    <span>{powerLabel(v)}</span>
    {#if v.plug}
      <span class="plug {v.plug}" role="img" aria-label={S.plugState[v.plug]} title={S.plugState[v.plug]}></span>
    {/if}
  </button>
{/snippet}

<div class="remote" bind:clientWidth={width}>
  <div class="pair" class:stacked>
    <section class="card" aria-label="{S.monitor} volume" bind:this={mainCard}>
      <div class="blk">
        <Knob
          value={main}
          min={TRIM_MIN_DB}
          max={0}
          step={1}
          label={S.monitor}
          ariaLabel="{S.monitor} level"
          {size}
          {travel}
          format={dbText}
          disabled={off}
          surface={mainCard}
          onchange={(v, f) => setVol('main_vol', v, f)}
        />
        <div class="km" style:height="{size}px" aria-hidden="true">
          <Meter index={METER_SLOT.output(0)} label="Main L meter" />
          <Meter index={METER_SLOT.output(1)} label="Main R meter" />
        </div>
      </div>
      {@render power(mainPower, 'monitor')}
    </section>
    <section class="card" aria-label="{S.phones} volume" bind:this={phonesCard}>
      <div class="blk">
        <Knob
          value={phones}
          min={TRIM_MIN_DB}
          max={0}
          step={1}
          label={S.phones}
          ariaLabel="{S.phones} level"
          {size}
          {travel}
          format={dbText}
          disabled={off}
          surface={phonesCard}
          onchange={(v, f) => setVol('phones_vol', v, f)}
        />
        <div class="km" style:height="{size}px" aria-hidden="true">
          <Meter index={METER_SLOT.output(10)} label="Phones L meter" />
          <Meter index={METER_SLOT.output(11)} label="Phones R meter" />
        </div>
      </div>
      {@render power(phonesPower, 'phones')}
    </section>
  </div>

  {#if store.c.presets.length}
    <label class="preset">
      <span>{S.presets}</span>
      <select value={session.active_preset ?? ''} disabled={off} onchange={(e) => store.loadPreset(e.currentTarget.value)}>
        <option value="" disabled>{S.noPreset}</option>
        {#each store.c.presets as p (p)}<option value={p}>{p}</option>{/each}
      </select>
    </label>
  {/if}

  {#if store.roomTestAvailable}
    <button class="room" disabled={off} onclick={() => (store.roomOpen = true)}>{S.roomTest}</button>
  {/if}
</div>

<style>
  .remote {
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 560px;
    min-height: 100%;
    margin: 0 auto;
    padding: 4px 0 16px;
    box-sizing: border-box;
  }
  /* Auto margins centre the two units in the free height, and (unlike
     justify-content) never clip: a short screen simply scrolls. */
  .pair {
    display: flex;
    gap: 10px;
    margin: auto 0;
  }
  .pair.stacked {
    flex-direction: column;
  }
  .card {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 12px 10px;
    /* The whole card drags its knob; the on/off button stays a button. */
    touch-action: none;
    cursor: ns-resize;
    user-select: none;
    -webkit-user-select: none;
    -webkit-touch-callout: none;
    -webkit-tap-highlight-color: transparent;
  }
  .blk {
    display: flex;
    justify-content: center;
    align-items: flex-start;
    gap: 8px;
  }
  .blk :global(.val) {
    font-size: 20px;
    font-weight: 700;
    margin-top: 4px;
  }
  .blk :global(.lbl) {
    font-size: 14px;
  }
  .km {
    display: flex;
    gap: 2px;
    flex: none;
  }
  .km :global(canvas) {
    width: 12px;
    height: 100%;
    min-height: 0;
  }
  .power {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 48px;
    font-size: 15px;
    font-weight: 600;
    background: var(--bg-alt);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
    touch-action: manipulation;
  }
  .power svg {
    width: 26px;
    height: 26px;
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .power.on {
    color: var(--cyan);
    border-color: var(--cyan);
  }
  .power.busy {
    opacity: 0.85;
  }
  .power {
    position: relative;
  }
  /* Speaker plug state: a small dot in the corner of the Monitor button. */
  .plug {
    position: absolute;
    top: 6px;
    right: 8px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    border: 1px solid var(--text-dim);
    box-sizing: border-box;
  }
  .plug.on {
    background: var(--meter-green);
    border-color: var(--meter-green);
  }
  .plug.warming {
    background: var(--amber);
    border-color: var(--amber);
    animation: plugpulse 0.9s ease-in-out infinite alternate;
  }
  .plug.error {
    background: var(--red);
    border-color: var(--red);
  }
  .plug.unknown {
    border-style: dotted;
  }
  @keyframes plugpulse {
    to {
      opacity: 0.25;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .plug.warming {
      animation: none;
    }
  }
  .preset {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 14px;
    color: var(--text-dim);
  }
  .preset select {
    flex: 1;
    min-height: 40px;
    font-size: 14px;
    background: #0d0e11;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0 8px;
  }
  .room {
    align-self: center;
    min-height: 40px;
    padding: 0 16px;
    font-size: 14px;
    background: var(--bg-alt);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    touch-action: manipulation;
  }
  button:disabled,
  select:disabled {
    opacity: 0.45;
  }
</style>
