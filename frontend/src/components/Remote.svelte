<script lang="ts">
  import { onDestroy } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb } from '../lib/fader';
  import { METER_SLOT } from '../lib/meterMap';
  import { FADE_STEP_MS, forgetRemembered, isOff, loadRemembered, planIn, planOut, saveRemembered, valueAt, type FadePlan } from '../lib/fade';
  import Knob from './Knob.svelte';
  import Meter from './Meter.svelte';

  // Simple view for phones: the two rotary volumes people reach for, with
  // vertical meters, a master on/off that fades Main, and preset recall.
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

  // --- master fade (Main volume only) ---------------------------------
  type Dir = 'out' | 'in';
  let fading = $state<Dir | null>(null);
  let timer: ReturnType<typeof setInterval> | undefined;
  /** Last value written by the fade; a reversal starts from it, so it is continuous. */
  let level = TRIM_MIN_DB;

  function stopFade() {
    if (timer !== undefined) clearInterval(timer);
    timer = undefined;
    fading = null;
  }
  function run(dir: Dir, plan: FadePlan) {
    stopFade();
    fading = dir;
    const t0 = performance.now();
    let lastSent = Number.NaN;
    // Main as this fade last left it; anything else means another client, OSC,
    // a preset or a reconnect snapshot moved it: stop instead of fighting it.
    let expect = main;
    const write = (v: number, final: boolean) => {
      setVol('main_vol', v, final);
      expect = v;
    };
    const tick = () => {
      const now = store.state?.main.db;
      if (!store.ready || now === undefined || Math.abs(now - expect) > 0.5) {
        stopFade();
        return;
      }
      const t = plan.ms > 0 ? (performance.now() - t0) / plan.ms : 1;
      const v = valueAt(plan, t);
      level = v;
      if (t >= 1) {
        stopFade();
        write(v, true);
        // The remembered level is used once: a later Off remembers afresh.
        if (dir === 'in') forgetRemembered();
      } else if (v !== lastSent) {
        lastSent = v;
        write(v, false);
      }
    };
    timer = setInterval(tick, FADE_STEP_MS);
    tick();
  }
  const masterOn = $derived(fading === 'in' || (fading === null && !isOff(main)));
  function toggleMaster() {
    if (off) return;
    if (masterOn) {
      // Remember the level only when it is a real one (not mid-fade).
      if (fading === null) saveRemembered(main);
      run('out', planOut(fading ? level : main));
    } else {
      run('in', planIn(fading ? level : main, loadRemembered()));
    }
  }
  /** User drags of Main: stop any fade; a level the user sets replaces the remembered one. */
  function setMain(v: number, final: boolean) {
    stopFade();
    setVol('main_vol', v, final);
    if (final && !isOff(v)) forgetRemembered();
  }
  const masterLabel = $derived(
    fading === 'out' ? S.fadingOut : fading === 'in' ? S.fadingIn : masterOn ? S.mainOn : S.mainOff,
  );

  onDestroy(stopFade);

  $effect(() => {
    if (store.ready) store.listPresets();
  });
</script>

<div class="remote" bind:clientWidth={width}>
  <div class="pair" class:stacked>
    <section class="card" aria-label="{S.main} volume">
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
          onstart={stopFade}
          onchange={setMain}
        />
        <div class="km" style:height="{size}px" aria-hidden="true">
          <Meter index={METER_SLOT.output(0)} label="Main L meter" />
          <Meter index={METER_SLOT.output(1)} label="Main R meter" />
        </div>
      </div>
    </section>
    <section class="card" aria-label="{S.phones} volume">
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
          onchange={(v, f) => setVol('phones_vol', v, f)}
        />
        <div class="km" style:height="{size}px" aria-hidden="true">
          <Meter index={METER_SLOT.output(10)} label="Phones L meter" />
          <Meter index={METER_SLOT.output(11)} label="Phones R meter" />
        </div>
      </div>
    </section>
  </div>

  <button
    class="power"
    class:on={masterOn}
    class:busy={fading !== null}
    aria-pressed={masterOn}
    aria-label={S.mainPower}
    aria-busy={fading !== null}
    disabled={off}
    onclick={toggleMaster}
  >
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 3v9" />
      <path d="M6.4 6.6a8 8 0 1 0 11.2 0" />
    </svg>
    <span>{masterLabel}</span>
  </button>

  {#if store.c.presets.length}
    <label class="preset">
      <span>{S.presets}</span>
      <select value={session.active_preset ?? ''} disabled={off} onchange={(e) => store.loadPreset(e.currentTarget.value)}>
        <option value="" disabled>{S.noPreset}</option>
        {#each store.c.presets as p (p)}<option value={p}>{p}</option>{/each}
      </select>
    </label>
  {/if}
</div>

<style>
  .remote {
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 560px;
    margin: 0 auto;
    padding: 4px 0 16px;
  }
  .pair {
    display: flex;
    gap: 10px;
  }
  .pair.stacked {
    flex-direction: column;
  }
  .card {
    flex: 1 1 0;
    min-width: 0;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 12px 10px;
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
    gap: 10px;
    min-height: 56px;
    font-size: 17px;
    font-weight: 600;
    background: var(--bg-alt);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 14px;
    cursor: pointer;
    touch-action: manipulation;
  }
  .power svg {
    width: 26px;
    height: 26px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
  }
  .power.on {
    color: var(--cyan);
    border-color: var(--cyan);
  }
  .power.busy {
    opacity: 0.85;
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
  button:disabled,
  select:disabled {
    opacity: 0.45;
  }
</style>
