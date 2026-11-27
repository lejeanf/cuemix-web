<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, volKnobFromNorm, volKnobToNorm } from '../lib/fader';
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
  /** Same curve as the sliders (most travel near listening levels), 1 dB per key or swipe. */
  const curve = { toNorm: (v: number) => volKnobToNorm(v, TRIM_MIN_DB), fromNorm: (n: number) => volKnobFromNorm(n, TRIM_MIN_DB) };

  // Knob size follows the width so both knob + meter blocks fit side by side.
  const GAP = 10; // between the two cards
  const PAD = 10; // card padding
  const INNER = 8; // knob to meters
  const METERS_W = 2 * 12 + 2;
  /** Knobs grow with the screen (tablet, laptop), up to this. */
  const MAX_KNOB = 220;
  let width = $state(0);
  const stacked = $derived(width > 0 && width < 340);
  const size = $derived.by(() => {
    if (width <= 0) return 96;
    const block = stacked ? width : (width - GAP) / 2;
    return Math.round(Math.min(MAX_KNOB, Math.max(96, block - 2 * PAD - INNER - METERS_W - 2)));
  });
  const travel = $derived(Math.round(size * 2.5));

  const setVol = (which: 'main_vol' | 'phones_vol', v: number, final: boolean) =>
    store.set({ param: which, db: Math.round(v) }, final);

  // On/off per channel: the server fades and remembers the level; we show what it reports.
  const mainPower = $derived(powerView(store.c.power, 'main', main));
  const phonesPower = $derived(powerView(store.c.power, 'phones', phones));
  /** "Off" instead of the floor level while a channel is off (and not fading). */
  const levelText = (v: PowerView) => (db: number) => (!v.on && v.fading === null ? S.simple.off : dbText(db));
  /** "returns to −18 dB" under an off channel: the level the server's On fades back to. */
  const returnText = (v: PowerView) => (!v.on && v.fading === null && v.returnDb !== null ? S.simple.returnsTo(formatDb(v.returnDb, 0)) : '');

  // Plug warm-up countdown: the warm-up time comes from the plug settings, counted from
  // when the server reported "warming". Null when the settings are not known.
  let warmStart = $state<number | null>(null);
  let now = $state(Date.now());
  $effect(() => {
    if (mainPower.plug !== 'warming') {
      warmStart = null;
      return;
    }
    warmStart = Date.now();
    now = warmStart;
    const t = setInterval(() => (now = Date.now()), 250);
    return () => clearInterval(t);
  });
  const warmLeft = $derived.by(() => {
    const ms = store.c.plug?.settings.warmup_ms;
    if (warmStart === null || !ms) return null;
    return Math.max(0, Math.ceil((ms - (now - warmStart)) / 1000));
  });

  const powerLabel = (v: PowerView) =>
    v.fading === 'out' ? S.fadingOut : v.plug === 'warming' ? S.simple.warmingUp(warmLeft) : v.fading === 'in' ? S.fadingIn : v.on ? S.powerOn : S.powerOff;

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
    class:warming={v.plug === 'warming'}
    title={v.fading !== null ? S.simple.tapToCancel : undefined}
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
    <span class="pl">{powerLabel(v)}</span>
    {#if v.fading !== null}<span class="prog {v.fading}" aria-hidden="true"></span>{/if}
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
          keyStep={1}
          {curve}
          label={S.monitor}
          ariaLabel="{S.monitor} level"
          {size}
          {travel}
          format={levelText(mainPower)}
          disabled={off}
          surface={mainCard}
          onchange={(v, f) => setVol('main_vol', v, f)}
        />
        <div class="km" style:height="{size}px" aria-hidden="true">
          <Meter index={METER_SLOT.output(0)} label="Main L meter" />
          <Meter index={METER_SLOT.output(1)} label="Main R meter" />
        </div>
      </div>
      <!-- Always there (empty when on) so the button below never jumps. -->
      <p class="ret" aria-live="polite">{returnText(mainPower)}</p>
      {@render power(mainPower, 'monitor')}
    </section>
    <section class="card" aria-label="{S.phones} volume" bind:this={phonesCard}>
      <div class="blk">
        <Knob
          value={phones}
          min={TRIM_MIN_DB}
          max={0}
          step={1}
          keyStep={1}
          {curve}
          label={S.phones}
          ariaLabel="{S.phones} level"
          {size}
          {travel}
          format={levelText(phonesPower)}
          disabled={off}
          surface={phonesCard}
          onchange={(v, f) => setVol('phones_vol', v, f)}
        />
        <div class="km" style:height="{size}px" aria-hidden="true">
          <Meter index={METER_SLOT.output(10)} label="Phones L meter" />
          <Meter index={METER_SLOT.output(11)} label="Phones R meter" />
        </div>
      </div>
      <!-- Always there (empty when on) so the button below never jumps. -->
      <p class="ret" aria-live="polite">{returnText(phonesPower)}</p>
      {@render power(phonesPower, 'phones')}
    </section>
  </div>

  {#if store.c.presets.length}
    <!-- Opens a list with a Load button per preset: recall is a deliberate second tap. -->
    <button class="preset" disabled={off} aria-haspopup="dialog" onclick={() => (store.presetsOpen = true)}>
      <span class="pk">{S.simple.presetRow}</span>
      <span class="pv">{session.active_preset ?? S.simple.choosePreset}</span>
      {#if session.active_preset && session.modified}<span class="mod">{S.modified}</span>{/if}
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 6l6 6-6 6" /></svg>
    </button>
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
    max-width: 760px;
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
  .ret {
    margin: -6px 0 0;
    min-height: 1.3em;
    text-align: center;
    font-size: 13px;
    color: var(--text-dim);
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
  /* During a fade the button keeps the colour of where it is going (On blue, Off grey)
     and a bar sweeps along its bottom edge: busy, not disabled. */
  .power {
    position: relative;
    overflow: hidden;
  }
  .pl {
    text-align: center;
    line-height: 1.15;
  }
  .power.warming {
    color: var(--amber);
    border-color: var(--amber);
  }
  .prog {
    position: absolute;
    left: 0;
    bottom: 0;
    height: 3px;
    width: 40%;
    background: currentColor;
    border-radius: 2px;
    animation: sweep 1s ease-in-out infinite;
  }
  .prog.out {
    animation-direction: reverse;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }
  /* Speaker plug state: a small dot in the corner of the Monitor button. */
  .plug {
    position: absolute;
    top: 6px;
    right: 8px;
    width: 10px;
    height: 10px;
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
    /* The text says it; a still bar marks the button busy. */
    .prog {
      animation: none;
      width: 100%;
      opacity: 0.5;
    }
  }
  .preset {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 52px;
    padding: 0 10px 0 14px;
    font-size: 15px;
    text-align: left;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
    touch-action: manipulation;
  }
  .pk {
    color: var(--text-dim);
  }
  .pv {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .mod {
    flex: none;
    font-size: 12px;
    color: var(--amber);
  }
  .preset svg {
    flex: none;
    width: 18px;
    height: 18px;
    fill: none;
    stroke: var(--text-dim);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
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
  button:disabled {
    opacity: 0.45;
  }
</style>
