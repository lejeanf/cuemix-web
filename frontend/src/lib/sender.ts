import type { Param } from './protocol';

/** Identity of a parameter for throttling: name + all index fields. */
export function paramKey(p: Param): string {
  const q = p as unknown as Record<string, unknown>;
  return [p.param, q.bus, q.ch, q.band, q.input, q.output].filter((v) => v !== undefined).join(':');
}

/**
 * Per-parameter send throttle: at most one message per `interval` ms per key,
 * latest value wins, and `final` always sends immediately (pointerup / toggles).
 * The first change of a burst is sent right away (leading edge).
 */
export class ParamSender {
  private pending = new Map<string, Param>();
  private timers = new Map<string, ReturnType<typeof setTimeout>>();

  constructor(
    private readonly send: (p: Param) => void,
    private readonly interval = 25,
  ) {}

  set(p: Param, final = false): void {
    const key = paramKey(p);
    if (final) {
      this.pending.delete(key);
      this.send(p);
      return;
    }
    if (this.timers.has(key)) {
      this.pending.set(key, p);
      return;
    }
    this.send(p);
    this.arm(key);
  }

  private arm(key: string): void {
    this.timers.set(
      key,
      setTimeout(() => {
        const p = this.pending.get(key);
        if (p) {
          this.pending.delete(key);
          this.send(p);
          this.arm(key);
        } else {
          this.timers.delete(key);
        }
      }, this.interval),
    );
  }

  dispose(): void {
    for (const t of this.timers.values()) clearTimeout(t);
    this.timers.clear();
    this.pending.clear();
  }
}
