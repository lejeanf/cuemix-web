import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { biquadMagDb, bwToQ, eqResponseDb, freqToX, peakingBiquad, xToFreq } from './eq';
import { levelToPos, stepMeter } from './meters';
import { EQ_MODE, eqModeName, type EqBand, type Param } from './protocol';
import { ParamSender, paramKey } from './sender';
import { isHexColor, isValidLabelName, isValidPresetName } from './validate';
import { isHwMapped, isHwReadOnly } from './hwmap';

describe('ParamSender throttle', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());
  const vol = (gain: number): Param => ({ param: 'mix_vol', bus: 0, ch: 1, gain });

  it('sends leading, then at most every 25 ms, latest wins', () => {
    const sent: Param[] = [];
    const s = new ParamSender((p) => sent.push(p), 25);
    s.set(vol(0.1));
    s.set(vol(0.2));
    s.set(vol(0.3));
    expect(sent).toEqual([vol(0.1)]);
    vi.advanceTimersByTime(25);
    expect(sent).toEqual([vol(0.1), vol(0.3)]);
    s.set(vol(0.4));
    vi.advanceTimersByTime(10);
    expect(sent.length).toBe(2);
    vi.advanceTimersByTime(20);
    expect(sent[2]).toEqual(vol(0.4));
    vi.advanceTimersByTime(100);
    expect(sent.length).toBe(3);
  });
  it('final value is sent immediately and supersedes pending', () => {
    const sent: Param[] = [];
    const s = new ParamSender((p) => sent.push(p), 25);
    s.set(vol(0.1));
    s.set(vol(0.2));
    s.set(vol(0.9), true);
    expect(sent).toEqual([vol(0.1), vol(0.9)]);
    vi.advanceTimersByTime(100);
    expect(sent.length).toBe(2);
  });
  it('keys are independent per parameter', () => {
    const sent: Param[] = [];
    const s = new ParamSender((p) => sent.push(p), 25);
    s.set({ param: 'mix_vol', bus: 0, ch: 0, gain: 1 });
    s.set({ param: 'mix_vol', bus: 0, ch: 1, gain: 1 });
    expect(sent.length).toBe(2);
    expect(paramKey({ param: 'main_vol', db: 0 })).toBe('main_vol');
  });
});

describe('validation', () => {
  it('preset names', () => {
    expect(isValidPresetName('live set_1.v2-a')).toBe(true);
    expect(isValidPresetName('')).toBe(false);
    expect(isValidPresetName('a/b')).toBe(false);
    expect(isValidPresetName('x'.repeat(65))).toBe(false);
  });
  it('label names and colors', () => {
    expect(isValidLabelName('Vox')).toBe(true);
    expect(isValidLabelName('')).toBe(false);
    expect(isValidLabelName('   ')).toBe(false);
    expect(isValidLabelName('a\nb')).toBe(false);
    expect(isValidLabelName('é'.repeat(64))).toBe(true);
    expect(isValidLabelName('x'.repeat(65))).toBe(false);
    expect(isHexColor('#06B6D4')).toBe(true);
    expect(isHexColor('06B6D4')).toBe(false);
    expect(isHexColor('#fff')).toBe(false);
  });
});

describe('EQ math', () => {
  const flat = (): EqBand[] => [0, 1, 2, 3].map(() => ({ mode: EQ_MODE.BELL, bypass: false, freq_hz: 1000, gain_db: 0, bw_oct: 1 }));
  it('flat when every band is at 0 dB or bypassed', () => {
    const eq = flat();
    eq[0] = { ...eq[0], mode: EQ_MODE.HIGH_PASS, bypass: true };
    for (const f of [20, 100, 1000, 10000, 20000]) expect(eqResponseDb(eq, f)).toBeCloseTo(0, 9);
  });
  it('+6 dB bell gives +6 dB at f0 and ~0 far away; bypass removes it', () => {
    expect(biquadMagDb(peakingBiquad(1000, 6, 1), 1000)).toBeCloseTo(6, 6);
    const eq = flat();
    eq[1].gain_db = 6;
    expect(eqResponseDb(eq, 1000)).toBeCloseTo(6, 6);
    expect(Math.abs(eqResponseDb(eq, 20))).toBeLessThan(0.2);
    eq[1].bypass = true;
    expect(eqResponseDb(eq, 1000)).toBeCloseTo(0, 9);
  });
  it('high-pass band is -3 dB at fc and 12 dB/oct below', () => {
    const eq = flat();
    eq[0] = { ...eq[0], mode: EQ_MODE.HIGH_PASS };
    expect(eqResponseDb(eq, 1000)).toBeCloseTo(-3, 0);
    expect(eqResponseDb(eq, 250)).toBeLessThan(-23);
  });
  it('shelf reaches its gain far past the corner; unknown modes are not drawn', () => {
    const eq = flat();
    eq[3] = { ...eq[3], mode: EQ_MODE.SHELF, freq_hz: 5000, gain_db: 6 };
    expect(eqResponseDb(eq, 20000)).toBeCloseTo(6, 0);
    expect(Math.abs(eqResponseDb(eq, 100))).toBeLessThan(0.1);
    eq[3].mode = 9;
    expect(eqResponseDb(eq, 20000)).toBeCloseTo(0, 9);
    expect(eqModeName(9)).toBe('Type 9');
    expect(eqModeName(EQ_MODE.HIGH_PASS)).toBe('HPF');
  });
  it('bandwidth 1 octave is Q ~1.41', () => {
    expect(bwToQ(1)).toBeCloseTo(Math.SQRT2, 6);
  });
  it('log axis round trip', () => {
    expect(freqToX(20)).toBeCloseTo(0, 12);
    expect(freqToX(20000)).toBeCloseTo(1, 12);
    expect(xToFreq(freqToX(1234))).toBeCloseTo(1234, 6);
  });
});

describe('meters', () => {
  it('level mapping', () => {
    expect(levelToPos(0)).toBe(0);
    expect(levelToPos(1)).toBe(1);
    expect(levelToPos(0.001)).toBeCloseTo(0, 6);
  });
  it('peak holds 1.5 s then decays; value decays smoothly', () => {
    const m = { value: 0, peak: 0, peakAt: 0 };
    stepMeter(m, 0.8, 0, 16);
    expect(m.peak).toBe(0.8);
    stepMeter(m, 0, 500, 16);
    expect(m.value).toBeLessThan(0.8);
    expect(m.peak).toBe(0.8);
    stepMeter(m, 0, 2000, 16);
    expect(m.peak).toBeLessThan(0.8);
  });
});

describe('hardware mapping list', () => {
  it('matches ARCHITECTURE §8', () => {
    for (const p of ['preamp_gain', 'phase', 'pad', 'phantom', 'main_vol', 'phones_vol', 'mono', 'talkback'] as const) expect(isHwMapped(p)).toBe(true);
    expect(isHwMapped('mix_vol')).toBe(false);
  });
  it('DSP, reverb and clock are read from the MOTU but not written yet', () => {
    for (const p of ['input_eq_freq', 'output_eq_gain', 'comp_makeup', 'gate_bypass', 'reverb_decay', 'sample_rate', 'clock_source'] as const) {
      expect(isHwMapped(p)).toBe(false);
      expect(isHwReadOnly(p)).toBe(true);
    }
  });
});
