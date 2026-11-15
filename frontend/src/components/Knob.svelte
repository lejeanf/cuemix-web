<script lang="ts">
  import { DoubleTap, wheel } from '../lib/gesture';

  interface Props {
    value: number;
    min: number;
    max: number;
    /** Value a double-click/tap resets to. */
    reset: number;
    label: string;
    /** Rounding step (e.g. 1 for whole dB). */
    step?: number;
    /** Logarithmic mapping (frequency, times). */
    log?: boolean;
    format?: (v: number) => string;
    disabled?: boolean;
    hint?: string;
    size?: number;
    onchange: (value: number, final: boolean) => void;
  }
  let {
    value,
    min,
    max,
    reset,
    label,
    step = 0,
    log = false,
    format = (v) => String(Math.round(v * 100) / 100),
    disabled = false,
    hint,
    size = 44,
    onchange,
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
  let startY = 0;
  let startNorm = 0;
  let moved = false;
  let fine = false;
  let last = 0;

  function down(e: PointerEvent) {
    if (disabled || e.button > 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragging = true;
    moved = false;
    fine = e.shiftKey;
    startY = e.clientY;
    startNorm = toNorm(value);
    dragNorm = startNorm;
    last = value;
  }
  function move(e: PointerEvent) {
    if (!dragging) return;
    if (e.shiftKey !== fine) {
      fine = e.shiftKey;
      startY = e.clientY;
      startNorm = dragNorm;
    }
    if (Math.abs(e.clientY - startY) > 2) moved = true;
    if (!moved) return;
    dragNorm = Math.min(1, Math.max(0, startNorm + ((startY - e.clientY) / 160) * (fine ? 0.1 : 1)));
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
    else if (tap.hit()) onchange(reset, true);
  }
  function onwheel(e: WheelEvent) {
    if (disabled) return;
    e.preventDefault();
    const d = (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 0.002 : 0.02);
    let v = fromNorm(toNorm(value) + d);
    if (step > 0 && v === value) v = Math.min(max, Math.max(min, value + (d > 0 ? step : -step)));
    onchange(v, true);
  }
  function key(e: KeyboardEvent) {
    if (disabled) return;
    const d = e.key === 'ArrowUp' || e.key === 'ArrowRight' ? 1 : e.key === 'ArrowDown' || e.key === 'ArrowLeft' ? -1 : 0;
    if (d) {
      e.preventDefault();
      let v = fromNorm(toNorm(value) + d * (e.shiftKey ? 0.002 : 0.02));
      if (step > 0 && v === value) v = Math.min(max, Math.max(min, value + d * step));
      onchange(v, true);
    } else if (e.key === 'Enter') onchange(reset, true);
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

<div class="knob" class:disabled data-unmapped={hint ? '' : undefined} title={hint}>
  <div
    class="dial"
    style:width="{size}px"
    style:height="{size}px"
    role="slider"
    tabindex={disabled ? -1 : 0}
    aria-label={label}
    aria-valuemin={min}
    aria-valuemax={max}
    aria-valuenow={Math.round(value * 100) / 100}
    aria-valuetext={format(value)}
    aria-disabled={disabled}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
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
