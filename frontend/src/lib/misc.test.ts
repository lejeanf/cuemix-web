import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { biquadMagDb, bwToQ, eqResponseDb, freqToX, peakingBiquad, xToFreq } from './eq';
import { levelToPos, stepMeter } from './meters';
import { EQ_MODE, TRIM_MIN_DB, eqModeName, type EqBand, type Param } from './protocol';
import { ParamSender, paramKey } from './sender';
import { isHexColor, isValidLabelName, isValidPresetName } from './validate';
import { isHwMapped, isHwReadOnly } from './hwmap';
import { FADE_SAFE_DB, FADE_STORAGE_KEY, PHONES_STORAGE_KEY, fadeMs, forgetRemembered, isOff, loadRemembered, planIn, planOut, saveRemembered, valueAt } from './fade';

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
  it('cancel drops the queued value of that parameter only', () => {
    const sent: Param[] = [];
    const s = new ParamSender((p) => sent.push(p), 25);
    const other: Param = { param: 'mix_vol', bus: 0, ch: 2, gain: 0.5 };
    s.set(vol(0.1));
    s.set(vol(0.2));
    s.set(other);
    s.set({ ...other, gain: 0.6 });
    s.cancel(vol(0));
    vi.advanceTimersByTime(100);
    expect(sent).toEqual([vol(0.1), other, { ...other, gain: 0.6 }]);
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
    eq[3] = { ...eq[3], mode: EQ_MODE.HIGH_SHELF, freq_hz: 5000, gain_db: 6 };
    expect(eqResponseDb(eq, 20000)).toBeCloseTo(6, 0);
    expect(Math.abs(eqResponseDb(eq, 100))).toBeLessThan(0.1);
    eq[3].mode = 9;
    expect(eqResponseDb(eq, 20000)).toBeCloseTo(0, 9);
    expect(eqModeName(9)).toBe('Type 9');
    expect(eqModeName(EQ_MODE.HIGH_PASS)).toBe('HPF');
  });
  it('band types follow CueMix: low shelf / HPF first, high shelf last, bells between', async () => {
    const { eqModesForBand } = await import('./protocol');
    expect(eqModesForBand(0, 4)).toEqual([EQ_MODE.BELL, EQ_MODE.LOW_SHELF, EQ_MODE.HIGH_PASS]);
    expect(eqModesForBand(1, 4)).toEqual([EQ_MODE.BELL]);
    expect(eqModesForBand(2, 4)).toEqual([EQ_MODE.BELL]);
    expect(eqModesForBand(3, 4)).toEqual([EQ_MODE.BELL, EQ_MODE.HIGH_SHELF]);
    expect(eqModesForBand(2, 3)).toEqual([EQ_MODE.BELL, EQ_MODE.HIGH_SHELF]);
  });
  it('the stored value is the bell Q', () => {
    const eq = flat();
    eq[1] = { ...eq[1], gain_db: 6, bw_oct: 4 };
    // A narrow bell (Q 4) is ~0 dB one octave away; a wide one (Q 0.5) is not.
    expect(Math.abs(eqResponseDb(eq, 2000))).toBeLessThan(1);
    eq[1].bw_oct = 0.5;
    expect(eqResponseDb(eq, 2000)).toBeGreaterThan(3);
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
    for (const p of ['preamp_gain', 'phase', 'pad', 'phantom', 'main_vol', 'phones_vol', 'output_trim', 'mono', 'talkback', 'mix_vol', 'mix_pan', 'mix_mute', 'mix_solo'] as const)
      expect(isHwMapped(p)).toBe(true);
    expect(isHwMapped('dim')).toBe(false);
    expect(isHwReadOnly('mix_vol')).toBe(false);
  });
  it('input and output EQ are written to the MOTU', () => {
    for (const p of ['input_eq_mode', 'input_eq_bypass', 'input_eq_freq', 'input_eq_gain', 'input_eq_bw', 'output_eq_mode', 'output_eq_bypass', 'output_eq_freq', 'output_eq_gain', 'output_eq_bw'] as const) {
      expect(isHwMapped(p)).toBe(true);
      expect(isHwReadOnly(p)).toBe(false);
    }
  });
  it('dynamics, reverb and clock are read from the MOTU but not written yet', () => {
    for (const p of ['comp_makeup', 'gate_bypass', 'reverb_decay', 'sample_rate', 'clock_source'] as const) {
      expect(isHwMapped(p)).toBe(false);
      expect(isHwReadOnly(p)).toBe(true);
    }
  });
});

describe('output EQ slots', () => {
  it('names the 12 device slots: Main, unused, Line 3-10, Phones, Reverb', async () => {
    const { OUTPUT_EQ_SLOTS, visibleEqSlots, eqSlotName } = await import('./outputEq');
    expect(OUTPUT_EQ_SLOTS).toHaveLength(12);
    expect(OUTPUT_EQ_SLOTS.map((s) => s.slot)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    expect(OUTPUT_EQ_SLOTS.filter((s) => s.hidden).map((s) => s.slot)).toEqual([1]);
    expect(visibleEqSlots(12).map((s) => s.name)).toEqual([
      'Main', 'Line 3', 'Line 4', 'Line 5', 'Line 6', 'Line 7', 'Line 8', 'Line 9', 'Line 10', 'Phones', 'Reverb',
    ]);
    expect(visibleEqSlots(3).map((s) => s.slot)).toEqual([0, 2]);
    expect(visibleEqSlots(0)).toEqual([]);
    expect(eqSlotName(10)).toBe('Phones');
    expect(eqSlotName(99)).toBe('Slot 99');
  });
});

describe('output EQ presets', () => {
  it('every preset round-trips through matchPreset and stays in range', async () => {
    const { OUTPUT_EQ_PRESETS, presetBands, matchPreset, presetWrites } = await import('./eqPresets');
    const { DSP_LIMITS, eqModesForBand } = await import('./protocol');
    for (const p of OUTPUT_EQ_PRESETS) {
      const bands = presetBands(p);
      expect(bands).toHaveLength(3);
      expect(matchPreset(bands)?.id).toBe(p.id);
      bands.forEach((x, band) => {
        expect(x.freq_hz).toBeGreaterThanOrEqual(DSP_LIMITS.FREQ_HZ[0]);
        expect(x.freq_hz).toBeLessThanOrEqual(DSP_LIMITS.FREQ_HZ[1]);
        expect(Math.abs(x.gain_db)).toBeLessThanOrEqual(DSP_LIMITS.EQ_GAIN_DB[1]);
        expect(x.bw_oct).toBeGreaterThanOrEqual(DSP_LIMITS.EQ_BW_OCT[0]);
        expect(x.bw_oct).toBeLessThanOrEqual(DSP_LIMITS.EQ_BW_OCT[1]);
        expect(eqModesForBand(band, 3)).toContain(x.mode);
      });
      // Applying a preset writes the 3 bands of the selected slot only.
      for (const slot of [0, 2, 10, 11]) {
        const w = presetWrites(p, slot);
        expect(w.map((x) => x.slot)).toEqual([slot, slot, slot]);
        expect(w.map((x) => x.band)).toEqual([0, 1, 2]);
      }
    }
    const custom = presetBands(OUTPUT_EQ_PRESETS[1]);
    custom[1].gain_db = 2;
    expect(matchPreset(custom)).toBeUndefined();
  });
});

describe('meter probe', () => {
  it('rolling stats and baseline diff', async () => {
    const { ProbeWindow, changedFrom } = await import('./probe');
    const w = new ProbeWindow(2);
    w.push([255, 10]);
    w.push([255, 20]);
    w.push([255, 30]);
    expect(w.count).toBe(2);
    expect(w.stats()).toEqual({ mean: [255, 25], min: [255, 20], max: [255, 30] });
    expect(changedFrom([255, 10], [255, 25])).toEqual([1]);
  });
});

describe('mk5 meter map', () => {
  it('maps the points measured on a card', async () => {
    const { mk5MeterLevels, METER_SLOT, rawToLinear, hasMk5Meters } = await import('./meterMap');
    const raw = new Array(100).fill(255);
    raw[37] = 63; // Mic 1 hiss at the preamp
    raw[50] = 66; // Mic 1 after DSP
    raw[33] = 20; // Line 7 at the preamp
    raw[24] = 9; // ADAT 1
    raw[46] = 0; // Main L, full scale
    const lv = mk5MeterLevels(raw);
    expect(lv[METER_SLOT.preamp(0)]).toBeCloseTo(rawToLinear(63), 9);
    expect(lv[METER_SLOT.mixerInput(0)]).toBeCloseTo(rawToLinear(66), 9);
    expect(lv[METER_SLOT.preamp(6)]).toBeCloseTo(rawToLinear(20), 9);
    expect(lv[METER_SLOT.mixerInput(10)]).toBeCloseTo(rawToLinear(9), 9);
    expect(lv[METER_SLOT.output(0)]).toBe(1);
    expect(lv[METER_SLOT.preamp(1)]).toBe(0);
    expect(rawToLinear(40)).toBeCloseTo(0.1, 9); // -20 dB
    expect(hasMk5Meters('UL5E', 100)).toBe(true);
    expect(hasMk5Meters(undefined, 100)).toBe(false);
  });
});

describe('output patch', () => {
  it('follows the patch only for known destinations', async () => {
    const { outputSourcePoint, mk5MeterLevels, METER_SLOT, rawToLinear, MK5_PATCH_DEST } = await import('./meterMap');
    expect(MK5_PATCH_DEST.every((d) => d === null)).toBe(true);
    const patch = new Array(80).fill(255);
    patch[10] = 46;
    patch[11] = 78;
    const dests = [null, null, null, null, null, null, null, null, null, null, 10, 11];
    expect(outputSourcePoint(10, patch, dests)).toBe(46); // Phones L patched to Main
    expect(outputSourcePoint(11, patch, dests)).toBe(78);
    expect(outputSourcePoint(0, patch, dests)).toBe(46); // unknown dest: own point
    expect(outputSourcePoint(10, null, dests)).toBe(64); // unknown patch
    expect(outputSourcePoint(10, [], dests)).toBe(64); // destination beyond the table
    expect(outputSourcePoint(11, [...patch.slice(0, 11), 255], dests)).toBe(65); // nothing patched
    expect(outputSourcePoint(99, patch, dests)).toBe(255);
    // Unknown destinations: levels identical with or without a patch.
    const raw = new Array(100).fill(255);
    raw[64] = 10;
    raw[78] = 4;
    expect(mk5MeterLevels(raw, patch)).toEqual(mk5MeterLevels(raw));
    expect(mk5MeterLevels(raw)[METER_SLOT.output(10)]).toBeCloseTo(rawToLinear(10), 9);
  });
});

describe('preset files', () => {
  it('derives valid names', async () => {
    const { presetNameFromFile, exportFileName } = await import('./presetFile');
    expect(presetNameFromFile('My mix (2).json')).toBe('My mix 2');
    expect(presetNameFromFile('???.json')).toBe('imported');
    expect(presetNameFromFile('x'.repeat(80) + '.json')).toHaveLength(64);
    expect(exportFileName('Live set', new Date('2026-11-15T10:00:00Z'))).toBe('cuemix-Live_set-2026-11-15.json');
  });
});

describe('master fade', () => {
  it('fades out from the current level all the way to the minimum', () => {
    const p = planOut(-20);
    expect(p).toMatchObject({ from: -20, to: TRIM_MIN_DB, end: TRIM_MIN_DB });
    expect(valueAt(p, 0)).toBe(-20);
    expect(valueAt(p, 0.5)).toBe(-60);
    expect(valueAt(p, 1)).toBe(TRIM_MIN_DB);
    const seq = [0, 0.2, 0.4, 0.6, 0.8, 0.99].map((t) => valueAt(p, t));
    expect([...seq].sort((a, b) => b - a)).toEqual(seq);
    // passes through values below -60 (the old floor), so the knob never jumps
    expect(seq.some((v) => v < -60 && v > TRIM_MIN_DB)).toBe(true);
  });
  it('a quiet level still sweeps to the minimum', () => {
    const p = planOut(-65);
    expect(p.from).toBe(-65);
    expect(p.to).toBe(TRIM_MIN_DB);
    expect(valueAt(p, 0.5)).toBeLessThan(-65);
    expect(valueAt(p, 1)).toBe(TRIM_MIN_DB);
  });
  it('fades in from the minimum to the remembered level and never above it', () => {
    const p = planIn(TRIM_MIN_DB, -30);
    expect(p).toMatchObject({ from: TRIM_MIN_DB, to: -30, end: -30 });
    for (let t = 0; t <= 1.2; t += 0.05) expect(valueAt(p, t)).toBeLessThanOrEqual(-30);
    expect(valueAt(p, 0)).toBe(TRIM_MIN_DB);
    expect(valueAt(p, 0.5)).toBeLessThan(-60);
    expect(valueAt(p, 1)).toBe(-30);
  });
  it('a remembered level below -60 is reached by a continuous sweep', () => {
    const p = planIn(TRIM_MIN_DB, -65);
    for (let t = 0; t <= 1; t += 0.1) expect(valueAt(p, t)).toBeLessThanOrEqual(-65);
    expect(valueAt(p, 1)).toBe(-65);
  });
  it('without a remembered level, fades up to -70 dB at most; never above 0', () => {
    expect(planIn(TRIM_MIN_DB, null).end).toBe(FADE_SAFE_DB);
    expect(planIn(TRIM_MIN_DB, 12).end).toBe(0);
    expect(planIn(TRIM_MIN_DB, Number.NaN).end).toBe(FADE_SAFE_DB);
  });
  it('a fade in starting above the remembered level never raises it', () => {
    const p = planIn(-10, -30);
    for (let t = 0; t <= 1; t += 0.05) expect(valueAt(p, t)).toBeLessThanOrEqual(-30);
  });
  it('reversing mid-fade starts from the current level', () => {
    const p = planIn(-45, -20);
    expect(p.from).toBe(-45);
    expect(valueAt(p, 0)).toBe(-45);
    expect(p.ms).toBeLessThan(1200);
    expect(planOut(-45).from).toBe(-45);
  });
  it('a full sweep takes 1.2 s, shorter spans less, never under 250 ms', () => {
    expect(planOut(0).ms).toBe(1200);
    expect(planIn(TRIM_MIN_DB, 0).ms).toBe(1200);
    expect(planOut(-80).ms).toBeLessThan(planOut(0).ms);
    expect(planOut(TRIM_MIN_DB + 1).ms).toBe(250);
  });
  it('detects off, durations are bounded, and storage failures are harmless', () => {
    expect(isOff(TRIM_MIN_DB)).toBe(true);
    expect(isOff(TRIM_MIN_DB + 0.5)).toBe(true);
    expect(isOff(-65)).toBe(false);
    expect(fadeMs(100)).toBe(1200);
    expect(fadeMs(1)).toBe(250);
    expect(fadeMs(-50)).toBe(600);
    expect(loadRemembered(FADE_STORAGE_KEY)).toBeNull(); // no storage in node: harmless
    const m = new Map<string, string>();
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => m.get(k) ?? null,
      setItem: (k: string, v: string) => void m.set(k, v),
      removeItem: (k: string) => void m.delete(k),
    });
    expect(loadRemembered(FADE_STORAGE_KEY)).toBeNull();
    saveRemembered(FADE_STORAGE_KEY, -30);
    expect(loadRemembered(FADE_STORAGE_KEY)).toBe(-30);
    forgetRemembered(FADE_STORAGE_KEY);
    expect(loadRemembered(FADE_STORAGE_KEY)).toBeNull();
    // each channel keeps its own level
    expect(FADE_STORAGE_KEY).not.toBe(PHONES_STORAGE_KEY);
    saveRemembered(FADE_STORAGE_KEY, -30);
    saveRemembered(PHONES_STORAGE_KEY, -12);
    expect(loadRemembered(FADE_STORAGE_KEY)).toBe(-30);
    expect(loadRemembered(PHONES_STORAGE_KEY)).toBe(-12);
    forgetRemembered(FADE_STORAGE_KEY);
    expect(loadRemembered(FADE_STORAGE_KEY)).toBeNull();
    expect(loadRemembered(PHONES_STORAGE_KEY)).toBe(-12);
    forgetRemembered(PHONES_STORAGE_KEY);
    for (const bad of [String(TRIM_MIN_DB), '5', 'abc', '', ' ', 'Infinity']) {
      m.set(FADE_STORAGE_KEY, bad);
      expect(loadRemembered(FADE_STORAGE_KEY)).toBeNull();
    }
    vi.unstubAllGlobals();
  });
});
