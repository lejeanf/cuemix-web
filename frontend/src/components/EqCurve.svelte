<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { wheel } from '../lib/gesture';
  import { EQ_RANGE_DB, eqResponseDb, freqToX, xToFreq } from '../lib/eq';

  let { ch, selected = $bindable(0) }: { ch: number; selected?: number } = $props();

  const W = 640;
  const H = 260;
  const eq = $derived(store.state?.channels[ch]?.eq);
  const off = $derived(!store.ready);
  let svg = $state<SVGSVGElement>()!;

  const COLORS = ['#06B6D4', '#10B981', '#F59E0B', '#EC4899'];
  const GRID_F = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
  const GRID_DB = [-20, -10, 0, 10, 20];

  const yOf = (db: number) => H / 2 - (db / EQ_RANGE_DB) * (H / 2);
  const dbOf = (y: number) => ((H / 2 - y) / (H / 2)) * EQ_RANGE_DB;
  const fmtF = (f: number) => (f >= 1000 ? `${f / 1000}k` : String(f));

  const path = $derived.by(() => {
    if (!eq) return '';
    const pts: string[] = [];
    for (let i = 0; i <= 160; i++) {
      const x = i / 160;
      const db = Math.max(-EQ_RANGE_DB, Math.min(EQ_RANGE_DB, eqResponseDb(eq, xToFreq(x))));
      pts.push(`${i === 0 ? 'M' : 'L'}${(x * W).toFixed(1)} ${yOf(db).toFixed(1)}`);
    }
    return pts.join(' ');
  });

  // Pointer tracking for drag + pinch.
  const pointers = new Map<number, { x: number; y: number }>();
  let dragBand = -1;
  let pinchStart = 0;
  let pinchQ = 0;

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
    else if (pointers.size === 2 && eq) {
      const [a, b] = [...pointers.values()];
      pinchStart = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      pinchQ = eq.bands[selected].q;
      dragBand = -1;
    }
  }
  function move(e: PointerEvent) {
    if (!pointers.has(e.pointerId)) return;
    const p = local(e);
    pointers.set(e.pointerId, p);
    if (pointers.size >= 2 && eq) {
      const [a, b] = [...pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      store.setEqBand(ch, selected, { q: round(pinchQ * (d / pinchStart), 2) });
    } else if (dragBand >= 0) {
      store.setEqBand(ch, dragBand, {
        freq_hz: round(xToFreq(p.x / W), 1),
        gain_db: round(dbOf(p.y), 1),
      });
    }
  }
  function up(e: PointerEvent) {
    if (!pointers.delete(e.pointerId)) return;
    if (svg.hasPointerCapture(e.pointerId)) svg.releasePointerCapture(e.pointerId);
    if (dragBand >= 0 && eq) store.setEqBand(ch, dragBand, {}, true);
    else if (pointers.size < 2 && eq) store.setEqBand(ch, selected, {}, true);
    dragBand = -1;
  }
  function onwheel(e: WheelEvent, band: number) {
    if (off || !eq) return;
    e.preventDefault();
    selected = band;
    const q = eq.bands[band].q * Math.exp(-e.deltaY * 0.002);
    store.setEqBand(ch, band, { q: round(q, 2) }, true);
  }
  function key(e: KeyboardEvent, band: number) {
    if (off || !eq) return;
    const b = eq.bands[band];
    const s = e.shiftKey ? 0.1 : 1;
    if (e.key === 'ArrowUp') store.setEqBand(ch, band, { gain_db: round(b.gain_db + s, 1) }, true);
    else if (e.key === 'ArrowDown') store.setEqBand(ch, band, { gain_db: round(b.gain_db - s, 1) }, true);
    else if (e.key === 'ArrowRight') store.setEqBand(ch, band, { freq_hz: round(b.freq_hz * (1 + 0.02 * s), 1) }, true);
    else if (e.key === 'ArrowLeft') store.setEqBand(ch, band, { freq_hz: round(b.freq_hz / (1 + 0.02 * s), 1) }, true);
    else return;
    e.preventDefault();
  }
  const round = (v: number, d: number) => Math.round(v * 10 ** d) / 10 ** d;
</script>

{#if eq}
  <svg
    bind:this={svg}
    viewBox="0 0 {W} {H}"
    class="eq"
    class:bypass={eq.bypass}
    role="group"
    aria-label="EQ curve"
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
  >
    {#each GRID_DB as d (d)}
      <line x1="0" x2={W} y1={yOf(d)} y2={yOf(d)} class="grid" class:zero={d === 0} />
      <text x="4" y={yOf(d) - 3} class="axis">{d > 0 ? '+' : ''}{d}</text>
    {/each}
    {#each GRID_F as f (f)}
      <line x1={freqToX(f) * W} x2={freqToX(f) * W} y1="0" y2={H} class="grid" />
      <text x={freqToX(f) * W + 3} y={H - 4} class="axis">{fmtF(f)}</text>
    {/each}
    <path d={path} class="curve" />
    {#if eq.hpf.enabled}
      <line x1={freqToX(eq.hpf.freq_hz) * W} x2={freqToX(eq.hpf.freq_hz) * W} y1="0" y2={H} class="hpf" />
    {/if}
    {#each eq.bands as b, i (i)}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <circle
        class="handle"
        class:sel={selected === i}
        class:dim={!b.enabled || eq.bypass}
        cx={freqToX(b.freq_hz) * W}
        cy={yOf(b.gain_db)}
        r="11"
        fill={COLORS[i]}
        role="slider"
        tabindex="0"
        aria-label="EQ band {i + 1}: {Math.round(b.freq_hz)} Hz, {b.gain_db.toFixed(1)} dB, Q {b.q}"
        aria-valuenow={b.gain_db}
        aria-valuemin={-EQ_RANGE_DB}
        aria-valuemax={EQ_RANGE_DB}
        onpointerdown={(e) => down(e, i)}
        onkeydown={(e) => key(e, i)}
        onfocus={() => (selected = i)}
        use:wheel={(e) => onwheel(e, i)}
      />
      <text x={freqToX(b.freq_hz) * W} y={yOf(b.gain_db) + 4} class="num" text-anchor="middle">{i + 1}</text>
    {/each}
  </svg>
{/if}

<style>
  .eq {
    width: 100%;
    height: auto;
    max-height: 300px;
    background: #0d0e11;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    touch-action: none;
    user-select: none;
  }
  .grid {
    stroke: #1d2027;
    stroke-width: 1;
  }
  .grid.zero {
    stroke: #343947;
  }
  .axis {
    fill: #6b7280;
    font-size: 10px;
  }
  .curve {
    fill: none;
    stroke: var(--cyan);
    stroke-width: 2.5;
  }
  .bypass .curve {
    stroke: #4b5563;
  }
  .hpf {
    stroke: var(--cyan);
    stroke-dasharray: 4 4;
    opacity: 0.6;
  }
  .handle {
    cursor: grab;
    stroke: #0d0e11;
    stroke-width: 2;
  }
  .handle.sel {
    stroke: var(--text);
  }
  .handle.dim {
    opacity: 0.35;
  }
  .num {
    font-size: 11px;
    font-weight: 700;
    fill: #111;
    pointer-events: none;
  }
</style>
