// Room measurement analysis (pure, deterministic, no browser APIs).
//
// Input: power spectra (linear power per FFT bin, bins 0..fftSize/2-1, bin k at
// k * sampleRate / fftSize Hz) of the microphone, averaged over the pink-noise
// recording and over the background recording that came before it.
//
// Method: bins -> 1/6-octave band powers (20 Hz-20 kHz; pink noise has the same
// power in every such band, so a flat room reads flat), background power
// subtracted, light smoothing, level normalised to the mean of 200-2000 Hz.
// Then a greedy, cut-only fit of up to 3 bells in 30-500 Hz on the Main output
// EQ: take the highest remaining peak, cut it by its excess (max 12 dB), set Q
// from the peak's width, subtract the bell's own response, repeat until nothing
// sticks out by 2 dB. Never a boost: a dip (cancellation) cannot be fixed by EQ.
import { biquadMagDb, peakingBiquad } from './eq';
import { DSP_LIMITS, type EqBand } from './protocol';

/** Band resolution: 6 bands per octave. */
export const BANDS_PER_OCTAVE = 6;
export const BAND_F_MIN = 20;
export const BAND_F_MAX = 20_000;
/** Number of bands whose centre is <= 20 kHz. */
export const BAND_COUNT = Math.floor(BANDS_PER_OCTAVE * Math.log2(BAND_F_MAX / BAND_F_MIN)) + 1;

/** Level reference: the mean of the bands inside this range is 0 dB. */
export const REF_RANGE = [200, 2000] as const;
/** The part of the spectrum a phone microphone and a living room let us trust. */
export const TRUSTED_RANGE = [30, 500] as const;

export const FIT = {
  /** Maximum number of corrective bands (the Main output EQ has 3). */
  MAX_BANDS: 3,
  /** Stop when the highest remaining peak is below this many dB. */
  STOP_DB: 2,
  /** Cut range, dB (cuts only). */
  GAIN_DB: [-12, 0],
  Q: [0.7, 8],
} as const;

/** Hardware ranges the written values must stay in (CueMix 5 / MOTU). */
export const HW = {
  FREQ_HZ: [20, 20_000],
  GAIN_DB: [-20, 0],
  Q: [0.45, 10],
} as const;

/** Below this signal-to-background ratio (dB, mean over 200-2000 Hz) the result is not trustworthy. */
export const MIN_SNR_DB = 10;

export interface RoomBand {
  freq_hz: number;
  /** Always <= 0. */
  gain_db: number;
  q: number;
}

export interface RoomAnalysis {
  /** Band centres, Hz. */
  freqs: number[];
  /** Measured response, dB relative to the 200-2000 Hz mean. */
  measuredDb: number[];
  /** Measured + the proposed bands. */
  predictedDb: number[];
  /** At most 3 bells, sorted by frequency. */
  bands: RoomBand[];
  /** Mean level of the noise over the background in 200-2000 Hz, dB. */
  snrDb: number;
  /** snrDb >= MIN_SNR_DB. */
  reliable: boolean;
}

const clamp = (v: number, [lo, hi]: readonly [number, number] | readonly number[]): number => Math.min(hi, Math.max(lo, v));

export const bandCenter = (i: number): number => BAND_F_MIN * 2 ** (i / BANDS_PER_OCTAVE);
export const bandCenters = (): number[] => Array.from({ length: BAND_COUNT }, (_, i) => bandCenter(i));

export const dbToPower = (db: number): number => (Number.isFinite(db) ? 10 ** (db / 10) : 0);
export const powerToDb = (p: number): number => 10 * Math.log10(Math.max(p, 1e-20));

/** Mean of several power spectra (same length), bin by bin. */
export function averagePower(frames: readonly ArrayLike<number>[]): Float64Array {
  const n = frames[0]?.length ?? 0;
  const out = new Float64Array(n);
  if (frames.length === 0) return out;
  for (const f of frames) for (let k = 0; k < n; k++) out[k] += f[k] ?? 0;
  for (let k = 0; k < n; k++) out[k] /= frames.length;
  return out;
}

/**
 * Sums bin power into the 1/6-octave bands. A bin counts for the fraction of
 * its width that lies inside the band, so narrow low bands (a couple of bins)
 * do not jitter with the bin count.
 */
export function bandPowers(power: ArrayLike<number>, sampleRate: number): number[] {
  const n = power.length;
  const binHz = sampleRate / (2 * n);
  const half = 2 ** (1 / (2 * BANDS_PER_OCTAVE));
  const out: number[] = [];
  for (let i = 0; i < BAND_COUNT; i++) {
    const fc = bandCenter(i);
    const lo = fc / half;
    const hi = fc * half;
    let sum = 0;
    const kStart = Math.max(0, Math.floor(lo / binHz));
    const kEnd = Math.min(n - 1, Math.ceil(hi / binHz));
    for (let k = kStart; k <= kEnd; k++) {
      const a = Math.max(lo, (k - 0.5) * binHz);
      const b = Math.min(hi, (k + 0.5) * binHz);
      if (b > a) sum += (power[k] ?? 0) * ((b - a) / binHz);
    }
    out.push(sum);
  }
  return out;
}

/** Signal minus background (linear power), never below 0.1 % of the signal so the dB value stays finite. */
export function subtractBackground(signal: readonly number[], background: readonly number[]): number[] {
  return signal.map((s, i) => Math.max(s - (background[i] ?? 0), s * 1e-3, 1e-20));
}

/** [1 2 1]/4 smoothing in the power domain (about 1/3 octave), edges replicated. */
export function smoothBands(p: readonly number[]): number[] {
  return p.map((v, i) => (0.25 * (p[Math.max(0, i - 1)] ?? v) + 0.5 * v + 0.25 * (p[Math.min(p.length - 1, i + 1)] ?? v)));
}

function meanDbIn(db: readonly number[], freqs: readonly number[], [lo, hi]: readonly [number, number]): number {
  let s = 0;
  let c = 0;
  freqs.forEach((f, i) => {
    if (f >= lo && f <= hi) {
      s += db[i] ?? 0;
      c++;
    }
  });
  return c ? s / c : 0;
}

/** Response of a bell (cut) at the band centres, dB. */
const bellDb = (b: RoomBand, freqs: readonly number[]): number[] => {
  const bq = peakingBiquad(b.freq_hz, b.gain_db, b.q);
  return freqs.map((f) => biquadMagDb(bq, f));
};

/** Q of an RBJ bell whose half-gain bandwidth is `octaves`. */
export const qFromOctaves = (octaves: number): number => 1 / (2 * Math.sinh((Math.LN2 / 2) * octaves));

/** Width (octaves) of the peak at index `k` where the curve is above `level`. */
function peakWidthOctaves(db: readonly number[], freqs: readonly number[], k: number, level: number): number {
  const cross = (dir: -1 | 1): number => {
    let i = k;
    while (i + dir >= 0 && i + dir < db.length && (db[i + dir] ?? -Infinity) > level) i += dir;
    const j = i + dir;
    if (j < 0 || j >= db.length) return Math.log2(freqs[i] ?? 1);
    const a = db[i] ?? level;
    const b = db[j] ?? level;
    const t = a === b ? 0 : (a - level) / (a - b);
    return Math.log2(freqs[i] ?? 1) + t * (Math.log2(freqs[j] ?? 1) - Math.log2(freqs[i] ?? 1));
  };
  return cross(1) - cross(-1);
}

/** Greedy cut-only fit on a normalised response (dB, target 0). Result sorted by frequency. */
export function fitCuts(freqs: readonly number[], responseDb: readonly number[]): RoomBand[] {
  const work = [...responseDb];
  const bands: RoomBand[] = [];
  const inRange = freqs.map((f) => f >= TRUSTED_RANGE[0] && f <= TRUSTED_RANGE[1]);
  while (bands.length < FIT.MAX_BANDS) {
    let k = -1;
    for (let i = 0; i < work.length; i++) {
      if (inRange[i] && (k < 0 || (work[i] ?? -Infinity) > (work[k] ?? -Infinity))) k = i;
    }
    const excess = k >= 0 ? (work[k] ?? 0) : 0;
    if (k < 0 || excess < FIT.STOP_DB) break;

    // Peak frequency: parabola through the peak and its neighbours (in log frequency).
    let pos = Math.log2(freqs[k] ?? 1);
    const l = work[k - 1];
    const r = work[k + 1];
    if (l !== undefined && r !== undefined) {
      const d = l - 2 * excess + r;
      if (d < 0) pos += clamp((0.5 * (l - r)) / d, [-0.5, 0.5]) / BANDS_PER_OCTAVE;
    }
    const width = peakWidthOctaves(work, freqs, k, excess / 2);
    const band: RoomBand = {
      freq_hz: Math.round(clamp(2 ** pos, HW.FREQ_HZ)),
      gain_db: Math.round(clamp(-excess, FIT.GAIN_DB) * 10) / 10,
      q: Math.round(clamp(width > 0 ? qFromOctaves(width) : FIT.Q[1], FIT.Q) * 100) / 100,
    };
    bands.push(band);
    const resp = bellDb(band, freqs);
    for (let i = 0; i < work.length; i++) work[i] = (work[i] ?? 0) + (resp[i] ?? 0);
  }
  return bands.sort((a, b) => a.freq_hz - b.freq_hz);
}

/**
 * Whole analysis. `signal` and `background` are averaged power spectra of the
 * same length (bins), `sampleRate` the AudioContext's.
 */
export function analyzeRoom(signal: ArrayLike<number>, background: ArrayLike<number>, sampleRate: number): RoomAnalysis {
  const freqs = bandCenters();
  const sig = bandPowers(signal, sampleRate);
  const bg = bandPowers(background, sampleRate);
  const snrDb = meanDbIn(sig.map(powerToDb), freqs, REF_RANGE) - meanDbIn(bg.map(powerToDb), freqs, REF_RANGE);
  const clean = smoothBands(subtractBackground(sig, bg)).map(powerToDb);
  const ref = meanDbIn(clean, freqs, REF_RANGE);
  const measuredDb = clean.map((v) => v - ref);
  const bands = fitCuts(freqs, measuredDb);
  const predictedDb = [...measuredDb];
  for (const b of bands) {
    const resp = bellDb(b, freqs);
    for (let i = 0; i < predictedDb.length; i++) predictedDb[i] = (predictedDb[i] ?? 0) + (resp[i] ?? 0);
  }
  return { freqs, measuredDb, predictedDb, bands, snrDb, reliable: Number.isFinite(snrDb) && snrDb >= MIN_SNR_DB };
}

/** What gets written to one Main output EQ band. */
export type BandWrite = Pick<EqBand, 'mode' | 'bypass' | 'freq_hz' | 'gain_db' | 'bw_oct'>;

/**
 * The 3 band writes for `bands` (bells only: allowed on all 3 hardware bands).
 * Unused bands are bypassed, flat, and keep their frequency and Q.
 */
export function bandWrites(bands: readonly RoomBand[], previous: readonly EqBand[]): BandWrite[] {
  const sorted = [...bands].sort((a, b) => a.freq_hz - b.freq_hz).slice(0, FIT.MAX_BANDS);
  return Array.from({ length: FIT.MAX_BANDS }, (_, i) => {
    const b = sorted[i];
    if (b) {
      return {
        mode: 0,
        bypass: false,
        freq_hz: Math.round(clamp(b.freq_hz, HW.FREQ_HZ)),
        gain_db: clamp(Math.min(0, b.gain_db), HW.GAIN_DB),
        bw_oct: clamp(b.q, HW.Q),
      };
    }
    const p = previous[i];
    return {
      mode: 0,
      bypass: true,
      freq_hz: Math.round(clamp(p?.freq_hz ?? 1000, DSP_LIMITS.FREQ_HZ)),
      gain_db: 0,
      bw_oct: clamp(p?.bw_oct ?? 1, HW.Q),
    };
  });
}
