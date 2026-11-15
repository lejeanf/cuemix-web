/** Detects a double tap/click from pointer-up events (works for touch, where `dblclick` is unreliable). */
export class DoubleTap {
  private last = 0;
  hit(now = performance.now(), windowMs = 350): boolean {
    const d = this.last > 0 && now - this.last < windowMs;
    this.last = d ? 0 : now;
    return d;
  }
}

/** Svelte action: non-passive wheel listener (so we can preventDefault). */
export function wheel(node: HTMLElement | SVGElement, handler: (e: WheelEvent) => void) {
  let h = handler;
  const l = (e: Event) => h(e as WheelEvent);
  node.addEventListener('wheel', l, { passive: false });
  return {
    update(n: (e: WheelEvent) => void) {
      h = n;
    },
    destroy() {
      node.removeEventListener('wheel', l);
    },
  };
}
