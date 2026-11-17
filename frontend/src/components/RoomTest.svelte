<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { freqToX } from '../lib/eq';
  import { MicCapture, micSupport } from '../lib/roomCapture';
  import { TRUSTED_RANGE, analyzeRoom, bandWrites, type RoomAnalysis } from '../lib/roomEq';

  // Room test panel: record the room's background, ask the server to play pink
  // noise (MPD -> MOTU line input), record it, analyse, propose Main output EQ
  // cuts. `measure_stop` is always sent once a test was started: on finish,
  // cancel, error, close, page hide and destroy.

  type Phase = 'intro' | 'mic' | 'background' | 'starting' | 'settle' | 'measuring' | 'analysing' | 'restoring' | 'result' | 'error';
  const QUIET_WAIT_MS = 500;
  const BACKGROUND_MS = 2500;
  /** Stop recording the background this long before the server's `until`. */
  const QUIET_MARGIN_MS = 300;
  const SETTLE_MS = 1500;
  const MEASURE_MS = 10_000;
  const QUIET_TIMEOUT_MS = 8000;
  const PLAY_TIMEOUT_MS = 8000;
  const RESTORE_TIMEOUT_MS = 20_000;
  const TICK_MS = 300;

  class Abort extends Error {}

  let phase = $state<Phase>('intro');
  let note = $state('');
  let error = $state('');
  let progress = $state(0);
  let level = $state(-60);
  let result = $state<RoomAnalysis | null>(null);
  let appliedMsg = $state('');

  let cap: MicCapture | null = null;
  let started = false;
  let toastAtStart = 0;
  let cancelled = false;
  let abortMsg = '';
  let wake: (() => void) | null = null;

  const support = micSupport();
  const supportMsg = $derived(support === 'insecure' ? S.room.httpsNeeded : support === 'unsupported' ? S.room.noMic : '');
  const running = $derived(phase === 'mic' || phase === 'background' || phase === 'starting' || phase === 'settle' || phase === 'measuring' || phase === 'analysing' || phase === 'restoring');
  const recording = $derived(phase === 'background' || phase === 'settle' || phase === 'measuring');
  const serverRestoring = $derived(store.measure.state === 'restoring');
  const canStart = $derived(support === 'ok' && store.ready && store.measure.state === 'idle');
  const canApply = $derived(!!result && result.reliable && result.bands.length > 0 && store.measure.state === 'idle' && store.ready);

  function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const done = () => {
        clearTimeout(t);
        if (wake === done) wake = null;
        resolve();
      };
      const t = setTimeout(done, ms);
      wake = done;
    });
  }

  /** Throws when the run must end: cancelled, link lost, or the server left the test. */
  function guard(): void {
    if (cancelled) throw new Abort(abortMsg);
    if (!store.socketOpen) throw new Abort(S.room.offline);
    // A refused start comes back as an `error` event (toast) while the state stays idle.
    if (started && store.measure.state === 'idle' && store.c.toastSeq !== toastAtStart) {
      throw new Abort(store.c.toasts[store.c.toasts.length - 1]?.message || S.room.busy);
    }
    if (started) {
      const m = store.measure;
      if (m.state === 'error') throw new Abort(m.message || S.room.notPlaying);
    }
  }

  /** Samples the microphone for `ms`, one spectrum every TICK_MS; `need` is the server state that must hold. */
  async function record(ms: number, need: 'quiet' | 'playing'): Promise<void> {
    const t0 = performance.now();
    for (;;) {
      guard();
      if (store.measure.state !== need) throw new Abort(need === 'playing' ? S.room.lostPlaying : S.room.notPlaying);
      const elapsed = performance.now() - t0;
      progress = Math.min(1, elapsed / ms);
      if (elapsed >= ms) break;
      cap?.sample();
      level = cap?.level() ?? -60;
      await sleep(TICK_MS);
    }
  }

  /** Waits for the server to reach `target` (starting -> quiet -> playing). */
  async function waitState(target: 'quiet' | 'playing', timeoutMs: number): Promise<void> {
    const t0 = performance.now();
    let seenActive = false;
    while (store.measure.state !== target) {
      guard();
      const s = store.measure.state;
      if (s === 'starting' || s === 'quiet') seenActive = true;
      if (s === 'playing' || (seenActive && s === 'idle')) throw new Abort(S.room.notPlaying);
      if (performance.now() - t0 > timeoutMs) throw new Abort(S.room.notPlaying);
      progress = Math.min(1, (performance.now() - t0) / timeoutMs);
      await sleep(100);
    }
  }

  async function waitIdle(): Promise<void> {
    const t0 = performance.now();
    while (store.measure.state !== 'idle' && store.socketOpen && performance.now() - t0 < RESTORE_TIMEOUT_MS) await sleep(150);
  }

  function ensureStop(): void {
    if (!started) return;
    started = false;
    store.measureStop();
  }
  function closeMic(): void {
    cap?.close();
    cap = null;
  }
  function cleanup(): void {
    ensureStop();
    closeMic();
  }

  function micErrorText(e: unknown): string {
    const name = e instanceof DOMException ? e.name : '';
    return name === 'NotAllowedError' || name === 'SecurityError' ? S.room.micDenied : S.room.noMic;
  }

  async function run(): Promise<void> {
    if (running || !canStart) return;
    cancelled = false;
    abortMsg = S.room.cancelled;
    note = '';
    error = '';
    result = null;
    appliedMsg = '';
    progress = 0;
    try {
      phase = 'mic';
      try {
        cap = await MicCapture.open();
      } catch (e) {
        throw new Abort(micErrorText(e));
      }
      guard();
      // The analyser's first window is not full yet (the server's quiet period is long enough after this).
      await sleep(750);
      phase = 'starting';
      progress = 0;
      if (store.measure.state !== 'idle') throw new Abort(S.room.busy);
      toastAtStart = store.c.toastSeq;
      started = true;
      store.measureStart();
      // The server mutes everything else and pauses the music: the room is silent now.
      await waitState('quiet', QUIET_TIMEOUT_MS);
      phase = 'background';
      progress = 0;
      await sleep(QUIET_WAIT_MS);
      const until = store.measure.until ? Date.parse(store.measure.until) : Number.NaN;
      const room = Number.isFinite(until) ? until - Date.now() - QUIET_MARGIN_MS : BACKGROUND_MS;
      await record(Math.max(1000, Math.min(BACKGROUND_MS, room)), 'quiet');
      const background = cap.take();

      progress = 0;
      await waitState('playing', PLAY_TIMEOUT_MS);
      phase = 'settle';
      await record(SETTLE_MS, 'playing');
      cap.take(); // drop what was heard while the noise settled
      phase = 'measuring';
      await record(MEASURE_MS, 'playing');
      const signal = cap.take();
      const rate = cap.sampleRate;

      ensureStop();
      closeMic();
      phase = 'analysing';
      result = analyzeRoom(signal, background, rate);
      phase = 'restoring';
      await waitIdle();
      phase = 'result';
    } catch (e) {
      cleanup();
      const msg = e instanceof Abort ? e.message : S.room.noMic;
      if (msg === S.room.cancelled) {
        note = msg;
        phase = 'intro';
      } else {
        error = msg;
        phase = 'error';
      }
    } finally {
      cleanup();
      progress = 0;
    }
  }

  function cancelRun(msg: string = S.room.cancelled): void {
    if (!running) return;
    cancelled = true;
    abortMsg = msg;
    ensureStop();
    wake?.();
  }

  function close(): void {
    cancelRun();
    cleanup();
    store.roomOpen = false;
  }

  function onHide(): void {
    if (running) cancelRun(S.room.hidden);
    else cleanup();
  }
  function onVisibility(): void {
    if (document.hidden) onHide();
  }
  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') close();
  }

  onMount(() => {
    window.addEventListener('pagehide', onHide);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('pagehide', onHide);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  });
  onDestroy(() => {
    cancelled = true;
    wake?.();
    cleanup();
  });

  // --- applying / undoing ---------------------------------------------
  function apply(): void {
    const st = store.state;
    if (!st || !result || !canApply) return;
    const prev = st.output_eq[0]?.map((b) => ({ ...b })) ?? [];
    if (!store.roomUndo) store.roomUndo = prev;
    bandWrites(result.bands, prev).forEach((w, band) => store.setEq('output', 0, band, w, true));
    appliedMsg = S.room.applied;
  }
  function undo(): void {
    const prev = store.roomUndo;
    if (!prev || !store.ready) return;
    prev.forEach((b, band) =>
      store.setEq('output', 0, band, { mode: b.mode, bypass: b.bypass, freq_hz: b.freq_hz, gain_db: b.gain_db, bw_oct: b.bw_oct }, true),
    );
    store.roomUndo = null;
    appliedMsg = S.room.undone;
  }

  // --- curve -----------------------------------------------------------
  const W = 320;
  const H = 150;
  const ML = 26;
  const MR = 6;
  const MT = 6;
  const MB = 16;
  const PW = W - ML - MR;
  const PH = H - MT - MB;
  const DB = 15;
  const xOf = (f: number) => ML + freqToX(f) * PW;
  const yOf = (db: number) => MT + (1 - (Math.max(-DB, Math.min(DB, db)) + DB) / (2 * DB)) * PH;
  const path = (f: number[], d: number[]) => f.map((x, i) => `${i ? 'L' : 'M'}${xOf(x).toFixed(1)} ${yOf(d[i] ?? 0).toFixed(1)}`).join('');
  const shade = { x: xOf(TRUSTED_RANGE[0]), w: xOf(TRUSTED_RANGE[1]) - xOf(TRUSTED_RANGE[0]) };
  const meterPct = $derived(Math.max(0, Math.min(100, ((level + 60) / 60) * 100)));
</script>

<svelte:window onkeydown={onKey} />

<div class="scrim" role="presentation">
  <div class="dlg" role="dialog" aria-modal="true" aria-label={S.room.title}>
    <header>
      <h2>{S.room.title}</h2>
      <button class="x" onclick={close} aria-label={S.room.close}>×</button>
    </header>

    <div class="body">
      {#if serverRestoring && phase !== 'restoring'}
        <p class="banner" role="status">{S.room.phases.restoring}</p>
      {/if}

      {#if phase === 'intro' || phase === 'error'}
        <p>{S.room.intro}</p>
        <ul>
          {#each S.room.steps as s (s)}<li>{s}</li>{/each}
        </ul>
        <p class="warn">{S.room.warning}</p>
        {#if supportMsg}<p class="err" role="alert">{supportMsg}</p>{/if}
        {#if phase === 'error'}<p class="err" role="alert">{error}</p>{/if}
        {#if note}<p class="dim" role="status">{note}</p>{/if}
        {#if appliedMsg}<p class="ok" role="status">{appliedMsg}</p>{/if}
      {:else if running}
        <div class="run">
          {#if recording}
            <div class="meter" role="img" aria-label="{S.room.micLevel} {Math.round(level)} dB" title={S.room.micLevel}>
              <div class="fill" style:height="{meterPct}%"></div>
            </div>
          {/if}
          <div class="state">
            <p class="phase" role="status">{S.room.phases[phase as keyof typeof S.room.phases]}</p>
            <div class="bar" aria-hidden="true"><div style:width="{progress * 100}%"></div></div>
          </div>
        </div>
      {:else if result}
        <h3>{S.room.resultTitle}</h3>
        {#if !result.reliable}
          <p class="err" role="alert">{S.room.unreliable}</p>
        {:else}
          <svg viewBox="0 0 {W} {H}" role="img" aria-label={S.room.resultTitle} class="chart">
            <rect x={shade.x} y={MT} width={shade.w} height={PH} class="shade" />
            {#each [-10, 0, 10] as g (g)}
              <line x1={ML} x2={W - MR} y1={yOf(g)} y2={yOf(g)} class={g === 0 ? 'target' : 'grid'} />
              <text x={ML - 3} y={yOf(g) + 3} class="tick" text-anchor="end">{g}</text>
            {/each}
            {#each [100, 1000, 10000] as f (f)}
              <line x1={xOf(f)} x2={xOf(f)} y1={MT} y2={MT + PH} class="grid" />
              <text x={xOf(f)} y={H - 4} class="tick" text-anchor="middle">{f >= 1000 ? `${f / 1000}k` : f}</text>
            {/each}
            <path d={path(result.freqs, result.measuredDb)} class="measured" />
            <path d={path(result.freqs, result.predictedDb)} class="predicted" />
          </svg>
          <p class="legend">
            <span class="k m"></span>{S.room.legend.measured}
            <span class="k p"></span>{S.room.legend.predicted}
            <span class="k t"></span>{S.room.legend.target}
          </p>
          <p class="dim">{S.room.trusted}</p>
          {#if result.bands.length === 0}
            <p>{S.room.flat}</p>
          {:else}
            <h4>{S.room.proposed}</h4>
            <ul class="bands">
              {#each result.bands as b, i (i)}<li>{S.room.bandRow(i + 1, b.freq_hz, b.gain_db, b.q)}</li>{/each}
            </ul>
          {/if}
          <p class="dim">{S.room.caveat}</p>
          {#if result.bands.length}<p class="dim">{S.room.replaces}</p>{/if}
        {/if}
        {#if appliedMsg}<p class="ok" role="status">{appliedMsg}</p>{/if}
      {/if}
    </div>

    <footer>
      {#if running}
        <button onclick={() => cancelRun()}>{S.room.cancel}</button>
      {:else if phase === 'result'}
        {#if store.roomUndo}<button onclick={undo} disabled={!store.ready}>{S.room.undo}</button>{/if}
        <button onclick={() => (phase = 'intro')}>{S.room.again}</button>
        {#if result?.reliable && result.bands.length}
          <button class="primary" onclick={apply} disabled={!canApply}>{S.room.apply}</button>
        {:else}
          <button class="primary" onclick={close}>{S.room.close}</button>
        {/if}
      {:else}
        {#if store.roomUndo}<button onclick={undo} disabled={!store.ready}>{S.room.undo}</button>{/if}
        <button onclick={close}>{S.room.close}</button>
        <button class="primary" onclick={run} disabled={!canStart}>{S.room.start}</button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: stretch;
    justify-content: center;
  }
  .dlg {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-width: 0;
    background: var(--bg);
    color: var(--text);
    overflow: hidden;
    padding: env(safe-area-inset-top, 0px) env(safe-area-inset-right, 0px) env(safe-area-inset-bottom, 0px) env(safe-area-inset-left, 0px);
  }
  @media (min-width: 641px) {
    .scrim {
      align-items: center;
    }
    .dlg {
      padding: 0;
      max-width: 480px;
      max-height: min(90vh, 640px);
      border: 1px solid var(--border);
      border-radius: 14px;
    }
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  h3,
  h4 {
    margin: 0 0 4px;
    font-size: 14px;
  }
  h4 {
    font-size: 13px;
    color: var(--text-dim);
  }
  .x {
    width: 40px;
    height: 40px;
    font-size: 24px;
    line-height: 1;
    background: none;
    border: 0;
    color: var(--text-dim);
    cursor: pointer;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 10px 12px;
    font-size: 14px;
    line-height: 1.4;
  }
  .body p,
  .body ul {
    margin: 0 0 8px;
  }
  .body ul {
    padding-left: 18px;
  }
  .warn {
    color: var(--amber);
  }
  .err {
    color: var(--red);
  }
  .ok {
    color: var(--cyan);
  }
  .dim,
  .legend {
    color: var(--text-dim);
    font-size: 12px;
  }
  .banner {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 8px;
  }
  .run {
    display: flex;
    gap: 14px;
    align-items: stretch;
    min-height: 70px;
  }
  .state {
    flex: 1;
    min-width: 0;
    align-self: center;
  }
  .phase {
    font-size: 16px;
    font-weight: 600;
  }
  .meter {
    width: 12px;
    height: 70px;
    background: #0d0e11;
    border: 1px solid var(--border);
    border-radius: 3px;
    display: flex;
    align-items: flex-end;
    flex: none;
  }
  .fill {
    width: 100%;
    background: var(--meter-gradient);
  }
  .bar {
    height: 6px;
    background: var(--card);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--cyan);
  }
  .chart {
    width: 100%;
    height: auto;
    display: block;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .shade {
    fill: var(--cyan);
    opacity: 0.1;
  }
  .grid {
    stroke: var(--border);
    stroke-width: 0.6;
  }
  .target {
    stroke: var(--text-dim);
    stroke-width: 0.8;
    stroke-dasharray: 3 3;
  }
  .tick {
    fill: var(--text-dim);
    font-size: 8px;
  }
  .measured,
  .predicted {
    fill: none;
    stroke-width: 1.6;
    stroke-linejoin: round;
  }
  .measured {
    stroke: var(--text-dim);
  }
  .predicted {
    stroke: var(--cyan);
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 8px;
    align-items: center;
    margin: 4px 0 2px;
  }
  .k {
    display: inline-block;
    width: 14px;
    height: 0;
    border-top: 2px solid;
    margin-right: -4px;
  }
  .k.m {
    border-color: var(--text-dim);
  }
  .k.p {
    border-color: var(--cyan);
  }
  .k.t {
    border-top-style: dashed;
    border-color: var(--text-dim);
  }
  .bands {
    font-variant-numeric: tabular-nums;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid var(--border);
  }
  footer button {
    min-height: 44px;
    padding: 0 16px;
    font-size: 15px;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    touch-action: manipulation;
  }
  footer .primary {
    background: var(--cyan);
    color: var(--bg);
    border-color: var(--cyan);
    font-weight: 600;
  }
  footer button:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
