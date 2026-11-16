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
    eq[3] = { ...eq[3], mode: EQ_MODE.HIGH_SHELF, freq_hz: 5000, gain_db: 6 };
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
    for (const p of ['preamp_gain', 'phase', 'pad', 'phantom', 'main_vol', 'phones_vol', 'output_trim', 'mono', 'talkback', 'mix_vol', 'mix_pan', 'mix_mute', 'mix_solo'] as const)
      expect(isHwMapped(p)).toBe(true);
    expect(isHwMapped('dim')).toBe(false);
    expect(isHwReadOnly('mix_vol')).toBe(false);
  });
  it('DSP, reverb and clock are read from the MOTU but not written yet', () => {
    for (const p of ['input_eq_freq', 'output_eq_gain', 'comp_makeup', 'gate_bypass', 'reverb_decay', 'sample_rate', 'clock_source'] as const) {
      expect(isHwMapped(p)).toBe(false);
      expect(isHwReadOnly(p)).toBe(true);
    }
  });
});

describe('output EQ presets', () => {
  it('every preset round-trips through matchPreset and stays in range', async () => {
    const { OUTPUT_EQ_PRESETS, presetBands, matchPreset } = await import('./eqPresets');
    for (const p of OUTPUT_EQ_PRESETS) {
      const bands = presetBands(p);
      expect(bands).toHaveLength(3);
      expect(matchPreset(bands)?.id).toBe(p.id);
      for (const x of bands) {
        expect(x.freq_hz).toBeGreaterThanOrEqual(20);
        expect(Math.abs(x.gain_db)).toBeLessThanOrEqual(24);
      }
    }
    const custom = presetBands(OUTPUT_EQ_PRESETS[1]);
    custom[1].gain_db = 2;
    expect(matchPreset(custom)).toBeUndefined();
  });
});

describe('routing names', () => {
  it('names mk5 inputs and outputs and groups them', async () => {
    const { routingInputs, routingOutputs, groupSpans } = await import('./io');
    const dev = { model: 'UL5E', input_names: ['Mic 1', 'Mic 2', 'Line 3', 'Line 4', 'Line 5', 'Line 6', 'Line 7', 'Line 8', 'S/PDIF 1', 'S/PDIF 2', ...Array.from({ length: 8 }, (_, i) => `ADAT ${i + 1}`)] } as never;
    const outs = routingOutputs(22, dev);
    expect(outs[0].name).toBe('Main L');
    expect(outs[21].name).toBe('ADAT 8');
    expect(groupSpans(outs).map((g) => [g.group, g.span])).toEqual([['Analog', 10], ['Phones', 2], ['S/PDIF', 2], ['ADAT', 8]]);
    expect(groupSpans(routingInputs(18, dev)).map((g) => g.span)).toEqual([8, 2, 8]);
    expect(routingOutputs(4, null)[3].name).toBe('Out 4');
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

describe('preset files', () => {
  it('derives valid names', async () => {
    const { presetNameFromFile, exportFileName } = await import('./presetFile');
    expect(presetNameFromFile('My mix (2).json')).toBe('My mix 2');
    expect(presetNameFromFile('???.json')).toBe('imported');
    expect(presetNameFromFile('x'.repeat(80) + '.json')).toHaveLength(64);
    expect(exportFileName('Live set', new Date('2026-11-15T10:00:00Z'))).toBe('cuemix-Live_set-2026-11-15.json');
  });
});
