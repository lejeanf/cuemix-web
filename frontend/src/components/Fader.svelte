<script lang="ts">
  import { DoubleTap, wheel } from '../lib/gesture';

  interface Props {
    /** Widget position 0..1 (use the fader laws to convert). */
    pos: number;
    label: string;
    valueText?: string;
    /** Position a double-click/tap resets to. */
    resetPos?: number;
    disabled?: boolean;
    hint?: string;
    /** Marker for the unity / 0 dB position. */
    mark?: number;
    onchange: (pos: number, final: boolean) => void;
  }
  let { pos, label, valueText, resetPos = 0.75, disabled = false, hint, mark, onchange }: Props = $props();

  let track: HTMLDivElement;
  let dragging = $state(false);
  let dragPos = $state(0);
  const shown = $derived(dragging ? dragPos : pos);
  const tap = new DoubleTap();

  let startY = 0;
  let startPos = 0;
  let fine = false;
  let moved = false;

  const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
  /** Half the thumb height: the thumb's centre travels between HALF and height - HALF,
   *  so the thumb never sticks out of the fader and covers the labels around it. */
  const HALF = 11;
  const travel = (r: DOMRect) => Math.max(1, r.height - 2 * HALF);
  const at = (p: number) => `calc(${HALF}px + ${p} * (100% - ${2 * HALF}px))`;

  function down(e: PointerEvent) {
    if (disabled || e.button > 0) return;
    track.setPointerCapture(e.pointerId);
    const r = track.getBoundingClientRect();
    fine = e.shiftKey;
    moved = false;
    dragging = true;
    startY = e.clientY;
    startPos = pos;
    dragPos = pos;
    // Coarse drag jumps to the pointer once the user actually moves; a plain tap keeps the value.
    jumpPending = fine ? null : clamp01(1 - (e.clientY - r.top - HALF) / travel(r));
  }
  let jumpPending: number | null = null;

  function move(e: PointerEvent) {
    if (!dragging) return;
    const r = track.getBoundingClientRect();
    if (e.shiftKey !== fine) {
      // Rebase when Shift is pressed/released mid-drag.
      fine = e.shiftKey;
      startY = e.clientY;
      startPos = dragPos;
      jumpPending = null;
    }
    if (Math.abs(e.clientY - startY) > 3) moved = true;
    if (!moved) return;
    if (jumpPending !== null) {
      startPos = jumpPending;
      startY = e.clientY;
      jumpPending = null;
    }
    const next = clamp01(startPos + ((startY - e.clientY) / travel(r)) * (fine ? 0.1 : 1));
    dragPos = next;
    onchange(next, false);
  }

  function up(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    if (track.hasPointerCapture(e.pointerId)) track.releasePointerCapture(e.pointerId);
    if (moved) {
      onchange(dragPos, true);
    } else if (tap.hit()) {
      onchange(resetPos, true);
    } else if (jumpPending !== null) {
      onchange(jumpPending, true);
    }
    jumpPending = null;
  }

  function onwheel(e: WheelEvent) {
    if (disabled) return;
    e.preventDefault();
    const step = e.shiftKey ? 0.002 : 0.01;
    onchange(clamp01(pos + (e.deltaY < 0 ? step : -step)), true);
  }

  function key(e: KeyboardEvent) {
    if (disabled) return;
    const step = e.shiftKey ? 0.002 : 0.01;
    let n: number | null = null;
    if (e.key === 'ArrowUp' || e.key === 'ArrowRight') n = pos + step;
    else if (e.key === 'ArrowDown' || e.key === 'ArrowLeft') n = pos - step;
    else if (e.key === 'PageUp') n = pos + 0.1;
    else if (e.key === 'PageDown') n = pos - 0.1;
    else if (e.key === 'Home') n = 1;
    else if (e.key === 'End') n = 0;
    else if (e.key === 'Enter' || e.key === '0') n = resetPos;
    if (n !== null) {
      e.preventDefault();
      onchange(clamp01(n), true);
    }
  }
</script>

<div
  class="fader"
  class:disabled
  data-unmapped={hint ? '' : undefined}
  title={hint}
  role="slider"
  tabindex={disabled ? -1 : 0}
  aria-label={label}
  aria-orientation="vertical"
  aria-valuemin={0}
  aria-valuemax={1}
  aria-valuenow={Number(shown.toFixed(3))}
  aria-valuetext={valueText}
  aria-disabled={disabled}
  bind:this={track}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onkeydown={key}
  use:wheel={onwheel}
>
  <div class="rail"></div>
  {#if mark !== undefined}<div class="mark" style:bottom={at(mark)}></div>{/if}
  <div class="fill" style:height={at(shown)}></div>
  <div class="thumb" style:bottom={at(shown)}></div>
</div>

<style>
  .fader {
    position: relative;
    width: 40px;
    min-height: 140px;
    flex: 1;
    touch-action: none;
    cursor: ns-resize;
    user-select: none;
    margin: 0 auto;
    border-radius: 6px;
  }
  .fader.disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .rail {
    position: absolute;
    left: 50%;
    top: 0;
    bottom: 0;
    width: 6px;
    margin-left: -3px;
    border-radius: 3px;
    background: #0d0e11;
    border: 1px solid var(--border);
  }
  .fill {
    position: absolute;
    left: 50%;
    bottom: 0;
    width: 6px;
    margin-left: -3px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--cyan) 55%, transparent);
    pointer-events: none;
  }
  .mark {
    position: absolute;
    left: 6px;
    right: 6px;
    height: 1px;
    background: var(--text-dim);
    opacity: 0.6;
  }
  .thumb {
    position: absolute;
    left: 2px;
    right: 2px;
    height: 22px;
    margin-bottom: -11px;
    border-radius: 4px;
    background: linear-gradient(#4b5160, #2f333d);
    border: 1px solid #5b6273;
    box-shadow: 0 2px 4px #0008;
    pointer-events: none;
  }
  .thumb::after {
    content: '';
    position: absolute;
    left: 4px;
    right: 4px;
    top: 50%;
    height: 2px;
    margin-top: -1px;
    background: var(--text);
  }
</style>
