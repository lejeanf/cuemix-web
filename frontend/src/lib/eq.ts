// EQ response math (UI preview only). Standard RBJ cookbook biquads evaluated
// at a fixed 48 kHz reference rate. PROVISIONAL: the MOTU's real filter
// topology / Q definition may differ slightly.
import type { Equalizer } from './protocol';

export const EQ_FS = 48_000;
export const FREQ_MIN = 20;
export const FREQ_MAX = 20_000;
export const EQ_RANGE_DB = 20;

export type Biquad = [b0: number, b1: number, b2: number, a0: number, a1: number, a2: number];

export function peakingBiquad(f0: number, gainDb: number, q: number, fs = EQ_FS): Biquad {
  const A = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * f0) / fs;
  const alpha = Math.sin(w0) / (2 * q);
  const c = Math.cos(w0);
  return [1 + alpha * A, -2 * c, 1 - alpha * A, 1 + alpha / A, -2 * c, 1 - alpha / A];
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

/** Summed response (dB) of HPF + enabled bands at `f`; bands are skipped when the global bypass is on. */
export function eqResponseDb(eq: Equalizer, f: number): number {
  let db = 0;
  if (eq.hpf.enabled) db += biquadMagDb(hpfBiquad(eq.hpf.freq_hz), f);
  if (!eq.bypass) {
    for (const b of eq.bands) {
      if (b.enabled && b.gain_db !== 0) db += biquadMagDb(peakingBiquad(b.freq_hz, b.gain_db, b.q), f);
    }
  }
  return db;
}

const LOG_SPAN = Math.log(FREQ_MAX / FREQ_MIN);
export const freqToX = (f: number): number => Math.log(Math.max(f, FREQ_MIN) / FREQ_MIN) / LOG_SPAN;
export const xToFreq = (x: number): number => FREQ_MIN * Math.exp(Math.min(1, Math.max(0, x)) * LOG_SPAN);
