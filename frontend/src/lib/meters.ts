// Meter rendering: one requestAnimationFrame loop for all canvases.
//
// The bank holds linear peaks (0..1) per meter slot (see ./meterMap:
// mixer inputs, outputs, preamp inputs), fed from the MOTU's raw meter frame.

export const PEAK_HOLD_MS = 1500;
export const DECAY_PER_S = 1.2; // position units per second
export const METER_FLOOR_DB = -60;

/** Linear peak (0..1) → meter position (0..1) on a -60 dB..0 dB scale. */
export function levelToPos(level: number): number {
  if (!Number.isFinite(level) || level <= 0) return 0;
  const db = 20 * Math.log10(Math.min(level, 1));
  return Math.min(1, Math.max(0, (db - METER_FLOOR_DB) / -METER_FLOOR_DB));
}

/** Scale ticks drawn on every meter (dBFS). */
export const METER_TICKS_DB = [-40, -18, -6];
/** A frame at or above this position (about -0.3 dBFS: the card's top steps) latches the clip mark. */
export const CLIP_POS = levelToPos(10 ** (-0.3 / 20));

/** Meter position (0..1) of a dBFS value on the meter scale. */
export function dbToMeterPos(db: number): number {
  return Math.min(1, Math.max(0, (db - METER_FLOOR_DB) / -METER_FLOOR_DB));
}

export interface MeterTrack {
  value: number;
  peak: number;
  peakAt: number;
}

/** Advance one meter: instant attack, smooth decay, 1.5 s peak hold. Returns true if it changed. */
export function stepMeter(m: MeterTrack, target: number, now: number, dtMs: number): boolean {
  const before = m.value * 1000 + m.peak * 1e6;
  m.value = target >= m.value ? target : Math.max(target, m.value - (DECAY_PER_S * dtMs) / 1000);
  if (m.value >= m.peak) {
    m.peak = m.value;
    m.peakAt = now;
  } else if (now - m.peakAt > PEAK_HOLD_MS) {
    m.peak = Math.max(m.value, m.peak - (DECAY_PER_S * dtMs) / 1000);
  }
  return m.value * 1000 + m.peak * 1e6 !== before;
}

interface Target extends MeterTrack {
  canvas: HTMLCanvasElement;
  index: number;
  horizontal: boolean;
  drawn: boolean;
  /** Clip latched: stays until the meter is tapped. */
  clip: boolean;
}

class MeterBank {
  private levels: number[] = [];
  private targets = new Set<Target>();
  private raf = 0;
  private last = 0;

  constructor() {
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', () => {
        if (!document.hidden) this.wake();
      });
    }
  }

  push(levels: number[]): void {
    this.levels = levels;
    this.wake();
  }

  register(canvas: HTMLCanvasElement, index: number, horizontal = false): () => void {
    const t: Target = { canvas, index, horizontal, value: 0, peak: 0, peakAt: 0, drawn: false, clip: false };
    this.targets.add(t);
    this.wake();
    return () => this.targets.delete(t);
  }

  /** Draws every meter again (new colours). */
  redraw(): void {
    for (const t of this.targets) draw(t);
  }

  /** Clears the clip mark of the meter drawn on `canvas`. */
  clearClip(canvas: HTMLCanvasElement): void {
    for (const t of this.targets) {
      if (t.canvas === canvas && t.clip) {
        t.clip = false;
        draw(t);
      }
    }
  }

  private wake(): void {
    if (this.raf === 0 && typeof requestAnimationFrame !== 'undefined' && !document.hidden) {
      this.last = 0;
      this.raf = requestAnimationFrame((t) => this.frame(t));
    }
  }

  private frame(now: number): void {
    this.raf = 0;
    if (document.hidden) return; // resumed by visibilitychange
    const dt = this.last ? Math.min(100, now - this.last) : 16;
    this.last = now;
    let active = false;
    for (const t of this.targets) {
      const target = levelToPos(this.levels[t.index] ?? 0);
      let changed = stepMeter(t, target, now, dt);
      if (target >= CLIP_POS && !t.clip) {
        t.clip = true;
        changed = true;
      }
      if (changed || !t.drawn) {
        draw(t);
        t.drawn = true;
      }
      if (t.value > 0 || t.peak > 0) active = true;
    }
    if (active) this.raf = requestAnimationFrame((n) => this.frame(n));
  }
}

/** Meter colours, from the theme tokens (app.css); refreshed when the theme changes. */
interface Palette {
  track: string;
  green: string;
  yellow: string;
  red: string;
  tick: string;
}
const FALLBACK: Palette = {
  track: '#0d0e11',
  green: '#10B981',
  yellow: '#EAB308',
  red: '#EF4444',
  tick: 'rgba(255, 255, 255, 0.22)',
};
let palette: Palette = FALLBACK;

/** Reads the meter colours from the page's CSS variables (call after a theme change). */
export function refreshMeterPalette(): void {
  if (typeof document === 'undefined') return;
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string, d: string) => cs.getPropertyValue(name).trim() || d;
  palette = {
    track: v('--field', FALLBACK.track),
    green: v('--meter-green', FALLBACK.green),
    yellow: v('--meter-yellow', FALLBACK.yellow),
    red: v('--meter-red', FALLBACK.red),
    tick: v('--meter-tick', FALLBACK.tick),
  };
  meterBank.redraw();
}

function colorAt(pos: number): string {
  return pos > 0.92 ? palette.red : pos > 0.75 ? palette.yellow : palette.green;
}

function draw(t: Target): void {
  const c = t.canvas;
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(c.clientWidth * dpr));
  const h = Math.max(1, Math.round(c.clientHeight * dpr));
  if (c.width !== w) c.width = w;
  if (c.height !== h) c.height = h;
  const g = c.getContext('2d');
  if (!g) return;
  g.clearRect(0, 0, w, h);
  g.fillStyle = palette.track;
  g.fillRect(0, 0, w, h);
  const len = t.horizontal ? w : h;
  const fill = Math.round(len * t.value);
  const grad = t.horizontal ? g.createLinearGradient(0, 0, w, 0) : g.createLinearGradient(0, h, 0, 0);
  grad.addColorStop(0, palette.green);
  grad.addColorStop(0.75, palette.yellow);
  grad.addColorStop(1, palette.red);
  g.fillStyle = grad;
  if (t.horizontal) g.fillRect(0, 0, fill, h);
  else g.fillRect(0, h - fill, w, fill);
  if (t.peak > 0.01) {
    g.fillStyle = colorAt(t.peak);
    const p = Math.round(len * t.peak);
    if (t.horizontal) g.fillRect(Math.max(0, p - 2 * dpr), 0, 2 * dpr, h);
    else g.fillRect(0, Math.max(0, h - p), w, 2 * dpr);
  }
  // Scale ticks (-40 / -18 / -6 dBFS): faint lines across, readable on the track and the fill.
  g.fillStyle = palette.tick;
  for (const db of METER_TICKS_DB) {
    const p = Math.round(len * dbToMeterPos(db));
    if (t.horizontal) g.fillRect(p, 0, Math.max(1, dpr), h);
    else g.fillRect(0, h - p, w, Math.max(1, dpr));
  }
  // Clip latch: a solid red cap at the 0 dBFS end until the meter is tapped.
  if (t.clip) {
    g.fillStyle = palette.red;
    const c = Math.max(3, Math.round(4 * dpr));
    if (t.horizontal) g.fillRect(w - c, 0, c, h);
    else g.fillRect(0, 0, w, c);
  }
}

export const meterBank = new MeterBank();
