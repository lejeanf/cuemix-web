// EQ response math (UI preview only). Standard RBJ cookbook biquads evaluated
// at a fixed 48 kHz reference rate. PROVISIONAL: the MOTU's real filter
// topology / bandwidth definition may differ slightly.
import { EQ_MODE, type EqBand } from './protocol';

export const EQ_FS = 48_000;
export const FREQ_MIN = 20;
export const FREQ_MAX = 20_000;
export const EQ_RANGE_DB = 24;

export type Biquad = [b0: number, b1: number, b2: number, a0: number, a1: number, a2: number];

/** Bandwidth in octaves → Q (RBJ). */
export function bwToQ(octaves: number): number {
  const p = 2 ** Math.max(octaves, 0.01);
  return Math.sqrt(p) / (p - 1);
}

export function peakingBiquad(f0: number, gainDb: number, q: number, fs = EQ_FS): Biquad {
  const A = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * f0) / fs;
  const alpha = Math.sin(w0) / (2 * q);
  const c = Math.cos(w0);
  return [1 + alpha * A, -2 * c, 1 - alpha * A, 1 + alpha / A, -2 * c, 1 - alpha / A];
}

/** RBJ shelf (slope 1); `high` = high shelf, else low shelf. */
export function shelfBiquad(f0: number, gainDb: number, high: boolean, fs = EQ_FS): Biquad {
  const A = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * f0) / fs;
  const c = Math.cos(w0);
  const alpha = (Math.sin(w0) / 2) * Math.SQRT2;
  const k = 2 * Math.sqrt(A) * alpha;
  const s = high ? -1 : 1;
  return [
    A * (A + 1 - s * (A - 1) * c + k),
    2 * s * A * (A - 1 - s * (A + 1) * c),
    A * (A + 1 - s * (A - 1) * c - k),
    A + 1 + s * (A - 1) * c + k,
    -2 * s * (A - 1 + s * (A + 1) * c),
    A + 1 + s * (A - 1) * c - k,
  ];
}

/** 2nd-order Butterworth high-pass = 12 dB/oct. */
export function hpfBiquad(f0: number, fs = EQ_FS): Biquad {
  const w0 = (2 * Math.PI * f0) / fs;
  const alpha = Math.sin(w0) / (2 * Math.SQRT1_2);
  const c = Math.cos(w0);
  return [(1 + c) / 2, -(1 + c), (1 + c) / 2, 1 + alpha, -2 * c, 1 - alpha];
}

/** Magnitude in dB at frequency `f`. */
export function biquadMagDb([b0, b1, b2, a0, a1, a2]: Biquad, f: number, fs = EQ_FS): number {
  const w = (2 * Math.PI * Math.min(f, fs / 2 - 1)) / fs;
  const cw = Math.cos(w);
  const c2w = Math.cos(2 * w);
  const sw = Math.sin(w);
  const s2w = Math.sin(2 * w);
  const nr = b0 + b1 * cw + b2 * c2w;
  const ni = -(b1 * sw + b2 * s2w);
  const dr = a0 + a1 * cw + a2 * c2w;
  const di = -(a1 * sw + a2 * s2w);
  const mag2 = (nr * nr + ni * ni) / (dr * dr + di * di);
  return 10 * Math.log10(Math.max(mag2, 1e-30));
}

/** Filter of one band, or null when it adds nothing (bypassed, flat, unknown mode). */
export function bandBiquad(b: EqBand): Biquad | null {
  if (b.bypass) return null;
  switch (b.mode) {
    case EQ_MODE.HIGH_PASS:
      return hpfBiquad(b.freq_hz);
    case EQ_MODE.LOW_SHELF:
    case EQ_MODE.HIGH_SHELF:
      return b.gain_db === 0 ? null : shelfBiquad(b.freq_hz, b.gain_db, b.mode === EQ_MODE.HIGH_SHELF);
    case EQ_MODE.BELL:
      return b.gain_db === 0 ? null : peakingBiquad(b.freq_hz, b.gain_db, bwToQ(b.bw_oct));
    default:
      return null;
  }
}

/** Summed response (dB) of the non-bypassed bands at `f`. */
export function eqResponseDb(bands: readonly EqBand[], f: number): number {
  let db = 0;
  for (const b of bands) {
    const q = bandBiquad(b);
    if (q) db += biquadMagDb(q, f);
  }
  return db;
}

const LOG_SPAN = Math.log(FREQ_MAX / FREQ_MIN);
export const freqToX = (f: number): number => Math.log(Math.max(f, FREQ_MIN) / FREQ_MIN) / LOG_SPAN;
export const xToFreq = (x: number): number => FREQ_MIN * Math.exp(Math.min(1, Math.max(0, x)) * LOG_SPAN);
