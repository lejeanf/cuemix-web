import { describe, expect, it } from 'vitest';
import { biquadMagDb, peakingBiquad } from './eq';
import type { EqBand } from './protocol';
import {
  BAND_COUNT,
  FIT,
  HW,
  MIN_SNR_DB,
  TRUSTED_RANGE,
  analyzeRoom,
  averagePower,
  bandCenters,
  bandPowers,
  bandWrites,
  dbToPower,
  powerToDb,
  qFromOctaves,
  type RoomBand,
} from './roomEq';

const FS = 48_000;
const N = 16_384; // bins of a 32768-point FFT
const BIN = FS / (2 * N);

interface Peak {
  f: number;
  db: number;
  q: number;
}

/** Pink-noise power spectrum through a room with the given peaks (+ dips as negative db), plus flat background power. */
function spectrum(peaks: Peak[], noiseFloor = 0, pink = true): Float64Array {
  const p = new Float64Array(N);
  for (let k = 1; k < N; k++) {
    const f = k * BIN;
    let db = 0;
    for (const pk of peaks) db += biquadMagDb(peakingBiquad(pk.f, pk.db, pk.q), f);
    p[k] = (pink ? (1 / f) * 10 ** (db / 10) : 0) + noiseFloor;
  }
  return p;
}
/** Quiet room: background 40 dB below the noise level in the middle of the spectrum. */
const BG_LEVEL = 1e-4 / 1000;
const bg = () => spectrum([], BG_LEVEL, false);
const room = (peaks: Peak[]) => spectrum(peaks, BG_LEVEL);

describe('band analysis helpers', () => {
  it('has 1/6-octave centres from 20 Hz to under 20 kHz', () => {
    const c = bandCenters();
    expect(c.length).toBe(BAND_COUNT);
    expect(c[0]).toBe(20);
    expect(c[c.length - 1]!).toBeLessThanOrEqual(20_000);
    expect(c[c.length - 1]! * 2 ** (1 / 6)).toBeGreaterThan(20_000);
    expect(c[6]).toBeCloseTo(40, 6);
  });

  it('turns pink noise into flat band powers', () => {
    const b = bandPowers(spectrum([]), FS).map(powerToDb);
    const mid = b.slice(4, 55);
    expect(Math.max(...mid) - Math.min(...mid)).toBeLessThan(0.6);
  });

  it('converts dB to power and averages frames', () => {
    expect(dbToPower(-10)).toBeCloseTo(0.1, 9);
    expect(dbToPower(Number.NEGATIVE_INFINITY)).toBe(0);
    expect(powerToDb(100)).toBeCloseTo(20, 9);
    expect(Array.from(averagePower([[1, 2], [3, 6]]))).toEqual([2, 4]);
    expect(averagePower([]).length).toBe(0);
  });

  it('converts a half-gain bandwidth to the RBJ Q', () => {
    expect(qFromOctaves(1)).toBeCloseTo(1.4142, 3);
    expect(qFromOctaves(0.5)).toBeGreaterThan(qFromOctaves(1));
  });
});

describe('analyzeRoom', () => {
  it('finds no bands in a flat response', () => {
    const r = analyzeRoom(room([]), bg(), FS);
    expect(r.bands).toEqual([]);
    expect(r.reliable).toBe(true);
    expect(r.snrDb).toBeGreaterThan(MIN_SNR_DB);
    expect(Math.max(...r.measuredDb.slice(4, 55).map(Math.abs))).toBeLessThan(0.7);
  });

  it('is deterministic', () => {
    const peaks = [{ f: 63, db: 8, q: 4 }];
    expect(analyzeRoom(room(peaks), bg(), FS)).toEqual(analyzeRoom(room(peaks), bg(), FS));
  });

  it('fits one cut for a +8 dB peak at 63 Hz', () => {
    const r = analyzeRoom(room([{ f: 63, db: 8, q: 4 }]), bg(), FS);
    expect(r.bands.length).toBe(1);
    const b = r.bands[0]!;
    expect(Math.abs(b.freq_hz - 63)).toBeLessThanOrEqual(5);
    expect(b.gain_db).toBeLessThan(-5.5);
    expect(b.gain_db).toBeGreaterThan(-9.5);
    expect(b.q).toBeGreaterThanOrEqual(FIT.Q[0]);
    expect(b.q).toBeLessThanOrEqual(FIT.Q[1]);
    // The predicted response is flat in the trusted range.
    const inRange = r.freqs.map((f, i) => (f >= TRUSTED_RANGE[0] && f <= TRUSTED_RANGE[1] ? r.predictedDb[i]! : 0));
    expect(Math.max(...inRange)).toBeLessThan(FIT.STOP_DB);
  });

  it('fits two peaks as two bands sorted by frequency', () => {
    const r = analyzeRoom(room([{ f: 160, db: 6, q: 3 }, { f: 63, db: 8, q: 4 }]), bg(), FS);
    expect(r.bands.length).toBe(2);
    expect(r.bands[0]!.freq_hz).toBeLessThan(r.bands[1]!.freq_hz);
    expect(Math.abs(r.bands[0]!.freq_hz - 63)).toBeLessThanOrEqual(6);
    expect(Math.abs(r.bands[1]!.freq_hz - 160)).toBeLessThanOrEqual(14);
  });

  it('never proposes a boost, and ignores dips', () => {
    expect(analyzeRoom(room([{ f: 100, db: -10, q: 3 }]), bg(), FS).bands).toEqual([]);
    const r = analyzeRoom(room([{ f: 100, db: -10, q: 3 }, { f: 50, db: 7, q: 3 }]), bg(), FS);
    expect(r.bands.length).toBeGreaterThan(0);
    for (const b of r.bands) expect(b.gain_db).toBeLessThanOrEqual(0);
  });

  it('respects the band, gain, Q and range limits', () => {
    const peaks = [
      { f: 35, db: 15, q: 30 },
      { f: 63, db: 9, q: 5 },
      { f: 125, db: 8, q: 5 },
      { f: 250, db: 9, q: 5 },
      { f: 3000, db: 12, q: 3 },
    ];
    const r = analyzeRoom(room(peaks), bg(), FS);
    expect(r.bands.length).toBeLessThanOrEqual(FIT.MAX_BANDS);
    for (const b of r.bands) {
      expect(b.gain_db).toBeGreaterThanOrEqual(FIT.GAIN_DB[0]);
      expect(b.gain_db).toBeLessThanOrEqual(0);
      expect(b.q).toBeGreaterThanOrEqual(FIT.Q[0]);
      expect(b.q).toBeLessThanOrEqual(FIT.Q[1]);
      expect(b.freq_hz).toBeGreaterThanOrEqual(TRUSTED_RANGE[0] - 3);
      expect(b.freq_hz).toBeLessThanOrEqual(TRUSTED_RANGE[1]);
      expect(Number.isInteger(b.freq_hz)).toBe(true);
    }
    // Sorted.
    for (let i = 1; i < r.bands.length; i++) expect(r.bands[i]!.freq_hz).toBeGreaterThan(r.bands[i - 1]!.freq_hz);
  });

  it('removes the background before fitting', () => {
    // Loud background: the room noise is only ~12 dB above it. Ignoring it would flatten the peak.
    const loud = 1e-4 / 1000 * 10 ** 2.8;
    const sig = spectrum([{ f: 80, db: 8, q: 4 }], loud);
    const background = spectrum([], loud, false);
    const withBg = analyzeRoom(sig, background, FS);
    expect(withBg.bands.length).toBe(1);
    expect(withBg.bands[0]!.gain_db).toBeLessThan(-6);
    const without = analyzeRoom(sig, new Float64Array(N), FS);
    expect(without.bands.length === 0 || without.bands[0]!.gain_db > withBg.bands[0]!.gain_db).toBe(true);
  });

  it('flags a recording that is not louder than the background', () => {
    const r = analyzeRoom(bg(), bg(), FS);
    expect(r.reliable).toBe(false);
    expect(r.snrDb).toBeLessThan(MIN_SNR_DB);
  });
});

describe('bandWrites', () => {
  const prev: EqBand[] = [
    { mode: 3, bypass: false, freq_hz: 80, gain_db: 0, bw_oct: 2 },
    { mode: 0, bypass: false, freq_hz: 500, gain_db: 3, bw_oct: 1 },
    { mode: 2, bypass: true, freq_hz: 9000, gain_db: 4, bw_oct: 1 },
  ];

  it('always returns 3 bell writes, unused bands bypassed and flat', () => {
    const w = bandWrites([{ freq_hz: 63, gain_db: -8, q: 4 }], prev);
    expect(w.length).toBe(3);
    expect(w[0]).toEqual({ mode: 0, bypass: false, freq_hz: 63, gain_db: -8, bw_oct: 4 });
    for (const x of w.slice(1)) {
      expect(x.mode).toBe(0);
      expect(x.bypass).toBe(true);
      expect(x.gain_db).toBe(0);
    }
    expect(w.every((x) => x.mode === 0)).toBe(true);
    expect(bandWrites([], prev).every((x) => x.bypass)).toBe(true);
  });

  it('keeps every value inside the hardware ranges, whole Hz, no boost', () => {
    const wild: RoomBand[] = [
      { freq_hz: 5.4, gain_db: 9, q: 99 },
      { freq_hz: 99_999, gain_db: -50, q: 0.01 },
      { freq_hz: 100.6, gain_db: -3, q: 3 },
      { freq_hz: 200, gain_db: -3, q: 3 },
    ];
    const w = bandWrites(wild, []);
    expect(w.length).toBe(3);
    for (const x of w) {
      expect(Number.isInteger(x.freq_hz)).toBe(true);
      expect(x.freq_hz).toBeGreaterThanOrEqual(HW.FREQ_HZ[0]);
      expect(x.freq_hz).toBeLessThanOrEqual(HW.FREQ_HZ[1]);
      expect(x.gain_db).toBeGreaterThanOrEqual(HW.GAIN_DB[0]);
      expect(x.gain_db).toBeLessThanOrEqual(0);
      expect(x.bw_oct).toBeGreaterThanOrEqual(HW.Q[0]);
      expect(x.bw_oct).toBeLessThanOrEqual(HW.Q[1]);
    }
  });
});
