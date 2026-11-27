<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { wheel } from '../lib/gesture';
  import { EQ_MODE, type EqBand } from '../lib/protocol';
  import { EQ_RANGE_DB, eqResponseDb, freqToX, xToFreq, EQ_COLORS } from '../lib/eq';

  let {
    kind,
    ch,
    selected = $bindable(0),
    height = 200,
  }: { kind: 'input' | 'output'; ch: number; selected?: number; height?: number } = $props();

  const W = 640;
  const H = $derived(height);
  const bands: EqBand[] | undefined = $derived(kind === 'input' ? store.state?.input_eq[ch] : store.state?.output_eq[ch]);
  const off = $derived(!store.ready);
  let svg = $state<SVGSVGElement>()!;
  // The drawing scales with its box; handles and text keep a fixed on-screen size.
  let shownW = $state(0);
  const k = $derived(shownW > 0 ? W / shownW : 1);

  const GRID_F = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
  const GRID_DB = [-24, -12, 0, 12, 24];

  const yOf = (db: number) => H / 2 - (db / EQ_RANGE_DB) * (H / 2);
  const dbOf = (y: number) => ((H / 2 - y) / (H / 2)) * EQ_RANGE_DB;
  const fmtF = (f: number) => (f >= 1000 ? `${f / 1000}k` : String(f));
  /** A high-pass band has no gain: its handle sits on the 0 dB line. */
  const handleDb = (b: EqBand) => (b.mode === EQ_MODE.HIGH_PASS ? 0 : b.gain_db);

  const path = $derived.by(() => {
    if (!bands) return '';
    const pts: string[] = [];
    for (let i = 0; i <= 160; i++) {
      const x = i / 160;
      const db = Math.max(-EQ_RANGE_DB, Math.min(EQ_RANGE_DB, eqResponseDb(bands, xToFreq(x))));
      pts.push(`${i === 0 ? 'M' : 'L'}${(x * W).toFixed(1)} ${yOf(db).toFixed(1)}`);
    }
    return pts.join(' ');
  });

  // Pointer tracking for drag + pinch (pinch = bandwidth).
  const pointers = new Map<number, { x: number; y: number }>();
  let dragBand = -1;
  let pinchStart = 0;
  let pinchBw = 0;

  function local(e: PointerEvent) {
    const r = svg.getBoundingClientRect();
    return { x: ((e.clientX - r.left) / r.width) * W, y: ((e.clientY - r.top) / r.height) * H };
  }

  function down(e: PointerEvent, band: number) {
    if (off) return;
    selected = band;
    svg.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, local(e));
    if (pointers.size === 1) dragBand = band;
    else if (pointers.size === 2 && bands) {
      const [a, b] = [...pointers.values()];
      pinchStart = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      pinchBw = bands[selected].bw_oct;
      dragBand = -1;
    }
  }
  function move(e: PointerEvent) {
    if (!pointers.has(e.pointerId) || !bands) return;
    const p = local(e);
    pointers.set(e.pointerId, p);
    if (pointers.size >= 2) {
      const [a, b] = [...pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      // Spreading the fingers widens the bell, i.e. lowers its Q.
      store.setEq(kind, ch, selected, { bw_oct: round(pinchBw * (pinchStart / d), 2) });
    } else if (dragBand >= 0) {
      const patch: { freq_hz: number; gain_db?: number } = { freq_hz: xToFreq(p.x / W) };
      if (bands[dragBand].mode !== EQ_MODE.HIGH_PASS) patch.gain_db = round(dbOf(p.y), 1);
      store.setEq(kind, ch, dragBand, patch);
    }
  }
  function up(e: PointerEvent) {
    if (!pointers.delete(e.pointerId)) return;
    if (svg.hasPointerCapture(e.pointerId)) svg.releasePointerCapture(e.pointerId);
    const b = dragBand >= 0 ? dragBand : selected;
    const cur = bands?.[b];
    if (cur) store.setEq(kind, ch, b, { freq_hz: cur.freq_hz, gain_db: cur.gain_db, bw_oct: cur.bw_oct }, true);
    dragBand = -1;
  }
  function onwheel(e: WheelEvent, band: number) {
    if (off || !bands) return;
    e.preventDefault();
    selected = band;
    // Scrolling down widens the bell (lower Q), as before.
    store.setEq(kind, ch, band, { bw_oct: round(bands[band].bw_oct * Math.exp(-e.deltaY * 0.002), 2) }, true);
  }
  function key(e: KeyboardEvent, band: number) {
    if (off || !bands) return;
    const b = bands[band];
    const s = e.shiftKey ? 0.1 : 1;
    if (e.key === 'ArrowUp') store.setEq(kind, ch, band, { gain_db: round(b.gain_db + s, 1) }, true);
    else if (e.key === 'ArrowDown') store.setEq(kind, ch, band, { gain_db: round(b.gain_db - s, 1) }, true);
    else if (e.key === 'ArrowRight') store.setEq(kind, ch, band, { freq_hz: b.freq_hz * (1 + 0.02 * s) }, true);
    else if (e.key === 'ArrowLeft') store.setEq(kind, ch, band, { freq_hz: b.freq_hz / (1 + 0.02 * s) }, true);
    else return;
    e.preventDefault();
  }
  const round = (v: number, d: number) => Math.round(v * 10 ** d) / 10 ** d;
</script>

{#if bands}
  <svg
    bind:this={svg}
    viewBox="0 0 {W} {H}"
    class="eq"
    style:max-height="{H + 40}px"
    style:--k={k}
    bind:clientWidth={shownW}
    role="group"
    aria-label="EQ curve"
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
  >
    {#each GRID_DB as d (d)}
      <line x1="0" x2={W} y1={yOf(d)} y2={yOf(d)} class="grid" class:zero={d === 0} />
      {#if Math.abs(d) < EQ_RANGE_DB}<text x="4" y={yOf(d) - 3} class="axis">{d > 0 ? '+' : ''}{d}</text>{/if}
    {/each}
    {#each GRID_F as f (f)}
      <line x1={freqToX(f) * W} x2={freqToX(f) * W} y1="0" y2={H} class="grid" />
      <text x={freqToX(f) * W + 3} y={H - 4} class="axis">{fmtF(f)}</text>
    {/each}
    <path d={path} class="curve" />
    {#each bands as b, i (i)}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <circle
        class="handle"
        class:sel={selected === i}
        class:dim={b.bypass}
        cx={freqToX(b.freq_hz) * W}
        cy={yOf(handleDb(b))}
        r={8 * k}
        fill={EQ_COLORS[i % EQ_COLORS.length]}
        role="slider"
        tabindex="0"
        aria-label="EQ band {i + 1}: {Math.round(b.freq_hz)} Hz, {b.gain_db.toFixed(1)} dB, Q {b.bw_oct.toFixed(2)}"
        aria-valuenow={b.gain_db}
        aria-valuemin={-EQ_RANGE_DB}
        aria-valuemax={EQ_RANGE_DB}
        onpointerdown={(e) => down(e, i)}
        onkeydown={(e) => key(e, i)}
        onfocus={() => (selected = i)}
        use:wheel={(e) => onwheel(e, i)}
      />
      <text x={freqToX(b.freq_hz) * W} y={yOf(handleDb(b)) + 3.5 * k} class="num" text-anchor="middle">{i + 1}</text>
    {/each}
  </svg>
{/if}

<style>
  .eq {
    width: 100%;
    height: auto;
    background: var(--field);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    touch-action: none;
    user-select: none;
  }
  .grid {
    stroke: var(--grid);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .grid.zero {
    stroke: var(--grid-strong);
  }
  .axis {
    fill: var(--axis-text);
    font-size: calc(10px * var(--k, 1));
  }
  .curve {
    fill: none;
    stroke: var(--cyan);
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
  }
  .handle {
    cursor: grab;
    stroke: var(--field);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
  }
  .handle.sel {
    stroke: var(--text);
  }
  .handle.dim {
    opacity: 0.35;
  }
  .num {
    font-size: calc(10px * var(--k, 1));
    font-weight: 700;
    fill: var(--on-accent);
    pointer-events: none;
  }
</style>
