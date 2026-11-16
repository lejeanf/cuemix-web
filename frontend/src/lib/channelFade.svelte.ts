import { TRIM_MIN_DB } from './protocol';
import { FADE_STEP_MS, forgetRemembered, isOff, loadRemembered, planIn, planOut, saveRemembered, valueAt, type FadePlan } from './fade';

export type FadeDir = 'out' | 'in';

export interface ChannelFadeOptions {
  /** localStorage key of this channel's remembered level. */
  key: string;
  /** Current value in the store (undefined before the first snapshot). */
  read: () => number | undefined;
  /** Writes a whole-dB value to the device; `final` on the last write. */
  write: (db: number, final: boolean) => void;
  /** False while the connection or device is not ready: a running fade stops. */
  ready: () => boolean;
  /**
   * Drops a fade value still queued in the send throttle. Called when the fade aborts because
   * the value changed elsewhere, so a stale fade level is not sent after (and over) that change.
   */
  cancel?: () => void;
}

/**
 * On/off fade of one volume (Main, Phones). Off sweeps from the current level to the
 * minimum and remembers the level; On sweeps back up, never above the remembered level.
 * A fade stops when the user touches the knob, when the value is changed elsewhere
 * (another client, OSC, a preset, a reconnect) or when the device stops being ready.
 */
export class ChannelFade {
  fading = $state<FadeDir | null>(null);
  private timer: ReturnType<typeof setInterval> | undefined;
  /** Last value written by the fade; a reversal starts from it, so it is continuous. */
  private level = TRIM_MIN_DB;
  private o: ChannelFadeOptions;

  constructor(o: ChannelFadeOptions) {
    this.o = o;
  }

  private get current(): number {
    return this.o.read() ?? TRIM_MIN_DB;
  }

  /** True while the channel is audible, or fading toward it. */
  get on(): boolean {
    return this.fading === 'in' || (this.fading === null && !isOff(this.current));
  }

  stop = (): void => {
    if (this.timer !== undefined) clearInterval(this.timer);
    this.timer = undefined;
    this.fading = null;
  };

  private run(dir: FadeDir, plan: FadePlan): void {
    this.stop();
    this.fading = dir;
    const t0 = performance.now();
    let lastSent = Number.NaN;
    // The value as this fade last left it; anything else means someone else moved it.
    let expect = this.current;
    const tick = () => {
      const now = this.o.read();
      if (!this.o.ready() || now === undefined || Math.abs(now - expect) > 0.5) {
        this.stop();
        this.o.cancel?.();
        return;
      }
      const t = plan.ms > 0 ? (performance.now() - t0) / plan.ms : 1;
      const v = valueAt(plan, t);
      this.level = v;
      if (t >= 1) {
        this.stop();
        this.o.write(v, true);
        // The remembered level is used once: a later Off remembers afresh.
        if (dir === 'in') forgetRemembered(this.o.key);
      } else if (v !== lastSent) {
        lastSent = v;
        this.o.write(v, false);
        expect = v;
      }
    };
    this.timer = setInterval(tick, FADE_STEP_MS);
    tick();
  }

  toggle(): void {
    if (!this.o.ready()) return;
    const from = this.fading ? this.level : this.current;
    if (this.on) {
      // Remember the level only when it is a real one (not mid-fade).
      if (this.fading === null) saveRemembered(this.o.key, this.current);
      this.run('out', planOut(from));
    } else {
      this.run('in', planIn(from, loadRemembered(this.o.key)));
    }
  }

  /** A knob drag: stops any fade; a level the user sets replaces the remembered one. */
  user(v: number, final: boolean): void {
    this.stop();
    this.o.write(v, final);
    if (final && !isOff(v)) forgetRemembered(this.o.key);
  }
}
