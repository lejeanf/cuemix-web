<script lang="ts">
  import { DoubleTap, wheel } from '../lib/gesture';

  interface Props {
    value: number;
    min: number;
    max: number;
    /** Value a double-click/tap resets to; omit to disable the reset (volumes: it must never jump up). */
    reset?: number;
    label: string;
    /** Accessible name when it must say more than the visible caption. */
    ariaLabel?: string;
    /** Rounding step (e.g. 1 for whole dB). */
    step?: number;
    /** Logarithmic mapping (frequency, times). */
    log?: boolean;
    format?: (v: number) => string;
    disabled?: boolean;
    hint?: string;
    size?: number;
    /** Drag distance in px for the full range (bigger knob, bigger travel, less twitchy). */
    travel?: number;
    /** Called when a drag starts (a touch or click on the dial). */
    onstart?: () => void;
    onchange: (value: number, final: boolean) => void;
    /**
     * Optional larger drag surface (e.g. the whole card around the knob). A pointerdown
     * anywhere in it starts the drag like one on the dial, except on a button. The dial then
     * only keeps keyboard and wheel; the parent should set `touch-action: none` on the surface.
     */
    surface?: HTMLElement | null;
  }
  let {
    value,
    min,
    max,
    reset,
    label,
    ariaLabel,
    step = 0,
    log = false,
    format = (v) => String(Math.round(v * 100) / 100),
    disabled = false,
    hint,
    size = 44,
    travel = 160,
    onstart,
    onchange,
    surface = null,
  }: Props = $props();

  const toNorm = (v: number) =>
    log ? Math.log(Math.max(v, min) / min) / Math.log(max / min) : (v - min) / (max - min);
  const fromNorm = (n: number) => {
    const c = Math.min(1, Math.max(0, n));
    let v = log ? min * (max / min) ** c : min + c * (max - min);
    if (step > 0) v = Math.round(v / step) * step;
    return Math.min(max, Math.max(min, v));
  };

  let dragging = $state(false);
  let dragNorm = $state(0);
  const norm = $derived(dragging ? dragNorm : Math.min(1, Math.max(0, toNorm(value))));
  const tap = new DoubleTap();
  let startX = 0;
  let startY = 0;
  let startNorm = 0;
  let moved = false;
  /** A hint means the MOTU doesn't take this param yet: locked like a disabled control. */
  const off = $derived(disabled || !!hint);
  /** Axis chosen when the drag starts moving; locked so a diagonal drag never jumps. */
  let axis: 'x' | 'y' = 'y';
  let fine = false;
  let last = 0;

  function down(e: PointerEvent) {
    if (off || e.button > 0) return;
    // Buttons inside a shared surface stay plain buttons.
    if (surface && (e.target as Element).closest('button, a, select, input, textarea')) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragging = true;
    moved = false;
    fine = e.shiftKey;
    startX = e.clientX;
    startY = e.clientY;
    startNorm = toNorm(value);
    dragNorm = startNorm;
    last = value;
    onstart?.();
  }
  /** Drag distance along the locked axis: up or right = more. */
  function delta(e: PointerEvent): number {
    return axis === 'x' ? e.clientX - startX : startY - e.clientY;
  }
  function move(e: PointerEvent) {
    if (!dragging) return;
    if (e.shiftKey !== fine) {
      fine = e.shiftKey;
      startX = e.clientX;
      startY = e.clientY;
      startNorm = dragNorm;
    }
    if (!moved) {
      const dx = e.clientX - startX;
      const dy = startY - e.clientY;
      if (Math.max(Math.abs(dx), Math.abs(dy)) <= 2) return;
      // The axis that moved more wins for the whole drag.
      axis = Math.abs(dx) > Math.abs(dy) ? 'x' : 'y';
      moved = true;
    }
    dragNorm = Math.min(1, Math.max(0, startNorm + (delta(e) / travel) * (fine ? 0.1 : 1)));
    const v = fromNorm(dragNorm);
    if (v !== last) {
      last = v;
      onchange(v, false);
    }
  }
  function up(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    const el = e.currentTarget as HTMLElement;
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
    if (moved) onchange(fromNorm(dragNorm), true);
    else if (reset !== undefined && tap.hit()) onchange(reset, true);
  }
  // With a shared surface the pointer handlers live on it (and pointer capture goes to it)
  // instead of the dial.
  $effect(() => {
    const el = surface;
    if (!el) return;
    const h = [
      ['pointerdown', down],
      ['pointermove', move],
      ['pointerup', up],
      ['pointercancel', up],
    ] as const;
    for (const [n, f] of h) el.addEventListener(n, f as EventListener);
    return () => {
      for (const [n, f] of h) el.removeEventListener(n, f as EventListener);
    };
  });
  function onwheel(e: WheelEvent) {
    if (off) return;
    e.preventDefault();
    const d = (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 0.002 : 0.02);
    let v = fromNorm(toNorm(value) + d);
    if (step > 0 && v === value) v = Math.min(max, Math.max(min, value + (d > 0 ? step : -step)));
    onchange(v, true);
  }
  function key(e: KeyboardEvent) {
    if (off) return;
    const d = e.key === 'ArrowUp' || e.key === 'ArrowRight' ? 1 : e.key === 'ArrowDown' || e.key === 'ArrowLeft' ? -1 : 0;
    if (d) {
      e.preventDefault();
      let v = fromNorm(toNorm(value) + d * (e.shiftKey ? 0.002 : 0.02));
      if (step > 0 && v === value) v = Math.min(max, Math.max(min, value + d * step));
      onchange(v, true);
    } else if (e.key === 'Enter' && reset !== undefined) onchange(reset, true);
  }

  // Arc geometry: 270° sweep, starting at 135° (lower-left).
  const R = 16;
  const pt = (n: number) => {
    const a = ((135 + 270 * n) * Math.PI) / 180;
    return [20 + R * Math.cos(a), 20 + R * Math.sin(a)];
  };
  const arc = $derived.by(() => {
    const [x0, y0] = pt(0);
    const [x1, y1] = pt(norm);
    return `M${x0} ${y0} A${R} ${R} 0 ${norm * 270 > 180 ? 1 : 0} 1 ${x1} ${y1}`;
  });
  const track = (() => {
    const [x0, y0] = pt(0);
    const [x1, y1] = pt(1);
    return `M${x0} ${y0} A${R} ${R} 0 1 1 ${x1} ${y1}`;
  })();
  const ptr = $derived(pt(norm));
</script>

<div class="knob" class:disabled={off} data-unmapped={hint ? '' : undefined} title={hint}>
  <div
    class="dial"
    style:width="{size}px"
    style:height="{size}px"
    role="slider"
    tabindex={off ? -1 : 0}
    aria-label={ariaLabel ?? label}
    aria-valuemin={min}
    aria-valuemax={max}
    aria-valuenow={Math.round(value * 100) / 100}
    aria-valuetext={format(value)}
    aria-disabled={off}
    onpointerdown={surface ? undefined : down}
    onpointermove={surface ? undefined : move}
    onpointerup={surface ? undefined : up}
    onpointercancel={surface ? undefined : up}
    onkeydown={key}
    use:wheel={onwheel}
  >
    <svg viewBox="0 0 40 40" aria-hidden="true">
      <path d={track} class="track" />
      <path d={arc} class="arc" />
      <line x1="20" y1="20" x2={ptr[0]} y2={ptr[1]} class="needle" />
    </svg>
  </div>
  <div class="val">{format(value)}</div>
  <div class="lbl">{label}</div>
</div>

<style>
  .knob {
    display: inline-flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    border-radius: 6px;
  }
  .knob.disabled {
    opacity: 0.45;
  }
  .dial {
    touch-action: none;
    cursor: ns-resize;
    border-radius: 50%;
    user-select: none;
    -webkit-user-select: none;
    -webkit-touch-callout: none;
    -webkit-tap-highlight-color: transparent;
  }
  svg {
    width: 100%;
    height: 100%;
    fill: none;
    stroke-linecap: round;
  }
  .track {
    stroke: #0d0e11;
    stroke-width: 4;
  }
  .arc {
    stroke: var(--cyan);
    stroke-width: 3;
  }
  .needle {
    stroke: var(--text);
    stroke-width: 2;
  }
  .val {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .lbl {
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
