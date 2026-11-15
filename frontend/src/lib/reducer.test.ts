import { describe, expect, it } from 'vitest';
import type { EqBand, MotuState, ServerEvent } from './protocol';
import { applyEvent, applyParam, initialClientState } from './reducer';

const band = (): EqBand => ({ mode: 0, bypass: false, freq_hz: 1000, gain_db: 0, bw_oct: 1 });
const comp = () => ({ bypass: true, threshold_db: -10, attack_ms: 10, release_ms: 1000, ratio: 2, makeup: 1 });

function mkState(): MotuState {
  return {
    topology: { mixes: 2, mixer_inputs: 2, preamps: 2, mic_inputs: 1, routing_inputs: 2, routing_outputs: 2 },
    mixes: [0, 1].map((i) => ({ name: `Mix ${i + 1}`, channels: [0, 1].map(() => ({ gain: 1, pan: 0, mute: false, solo: false })) })),
    main: { db: 0 },
    preamps: [0, 1].map(() => ({ gain_db: 0, phantom: false, pad: false, phase: false })),
    input_eq: [0, 1].map(() => [band(), band(), band(), band()]),
    compressors: [comp(), comp()],
    gates: [{ bypass: true, threshold_db: -46, attack_ms: 10, release_ms: 1000 }],
    output_eq: [0, 1, 2].map(() => [band(), band(), band()]),
    reverb: { decay: 86, damping: 90, predelay_ms: 50, width: 50, preset: 1 },
    routing: { matrix: [[false, false], [false, false]] },
    control_room: { dim: false, mono: false, speaker_b: false, talkback: false, phones_db: -20 },
    output_trims: [0, 0],
    clock: { sample_rate: '48000', source: 'internal' },
  };
}

const withSnapshot = () => {
  const s = initialClientState();
  applyEvent(s, { event: 'snapshot', state: mkState(), session: { active_preset: 'a', modified: false, connection: 'online' } });
  return s;
};

describe('applyEvent', () => {
  it('snapshot', () => {
    const s = withSnapshot();
    expect(s.state?.topology.mixes).toBe(2);
    expect(s.session).toEqual({ active_preset: 'a', modified: false, connection: 'online' });
  });
  it('changed applies several params', () => {
    const s = withSnapshot();
    applyEvent(s, {
      event: 'changed',
      params: [
        { param: 'mix_vol', bus: 1, ch: 0, gain: 0.5 },
        { param: 'dim', on: true },
        { param: 'input_eq_freq', ch: 1, band: 2, hz: 500 },
        { param: 'input_eq_gain', ch: 1, band: 2, db: 3 },
        { param: 'input_eq_mode', ch: 1, band: 2, mode: 7 },
        { param: 'output_eq_bypass', ch: 2, band: 1, on: true },
        { param: 'output_eq_bw', ch: 2, band: 1, octaves: 0.5 },
        { param: 'comp_makeup', ch: 1, gain: 2 },
        { param: 'comp_bypass', ch: 0, on: false },
        { param: 'gate_threshold', ch: 0, db: -30 },
        { param: 'route', input: 1, output: 0, on: true },
        { param: 'reverb_decay', value: 40 },
        { param: 'reverb_preset', preset: 3 },
        { param: 'sample_rate', rate: '96000' },
        { param: 'clock_source', source: 'spdif' },
      ],
    });
    expect(s.state?.mixes[1].channels[0].gain).toBe(0.5);
    expect(s.state?.control_room.dim).toBe(true);
    expect(s.state?.input_eq[1][2]).toEqual({ mode: 7, bypass: false, freq_hz: 500, gain_db: 3, bw_oct: 1 });
    expect(s.state?.output_eq[2][1]).toMatchObject({ bypass: true, bw_oct: 0.5 });
    expect(s.state?.compressors[1].makeup).toBe(2);
    expect(s.state?.compressors[0].bypass).toBe(false);
    expect(s.state?.gates[0].threshold_db).toBe(-30);
    expect(s.state?.routing.matrix[1][0]).toBe(true);
    expect(s.state?.reverb).toMatchObject({ decay: 40, preset: 3 });
    expect(s.state?.clock).toEqual({ sample_rate: '96000', source: 'spdif' });
  });
  it('changed ignores out-of-range indices and missing state', () => {
    const s = initialClientState();
    expect(() => applyEvent(s, { event: 'changed', params: [{ param: 'dim', on: true }] })).not.toThrow();
    const t = withSnapshot();
    expect(() => applyParam(t.state!, { param: 'mix_vol', bus: 9, ch: 9, gain: 1 })).not.toThrow();
  });
  it('changed never treats non-integer keys from the wire as indices', () => {
    const t = withSnapshot();
    const before = JSON.stringify(t.state);
    // Malformed payloads (as JSON.parse would produce them) bypass the TS types.
    const evil = [
      { param: 'mix_vol', bus: '__proto__', ch: 'constructor', gain: 0 },
      { param: 'mix_mute', bus: 0, ch: '__proto__', on: true },
      { param: 'preamp_gain', ch: '__proto__', db: 1 },
      { param: 'input_eq_gain', ch: 0, band: '__proto__', db: 1 },
      { param: 'output_eq_mode', ch: '__proto__', band: 0, mode: 1 },
      { param: 'input_eq_freq', ch: 0, band: 9, hz: 1 },
      { param: 'gate_bypass', ch: 1, on: false },
      { param: 'comp_ratio', ch: '__proto__', ratio: 9 },
      { param: 'comp_makeup', ch: -1, gain: 9 },
      { param: 'route', input: 0, output: '__proto__', on: true },
      { param: 'route', input: 0.5, output: 0, on: true },
    ] as unknown as Parameters<typeof applyParam>[1][];
    for (const p of evil) applyParam(t.state!, p);
    expect(JSON.stringify(t.state)).toBe(before);
    expect(({} as Record<string, unknown>).gain).toBeUndefined();
    expect(([] as unknown as Record<string, unknown>).on).toBeUndefined();
  });
  it('status', () => {
    const s = withSnapshot();
    applyEvent(s, { event: 'status', connection: 'demo' });
    expect(s.session.connection).toBe('demo');
    expect(s.session.active_preset).toBe('a');
  });
  it('session', () => {
    const s = withSnapshot();
    applyEvent(s, { event: 'session', active_preset: 'b', modified: true, connection: 'online' });
    expect(s.session).toEqual({ active_preset: 'b', modified: true, connection: 'online', device: null });
  });
  it('output_trim updates in range and ignores out-of-range indices', () => {
    const s = withSnapshot();
    applyEvent(s, { event: 'changed', params: [{ param: 'output_trim', ch: 1, db: -6 }] });
    applyEvent(s, { event: 'changed', params: [{ param: 'output_trim', ch: 9, db: -6 }] });
    expect(s.state!.output_trims).toEqual([0, -6]);
  });
  it('session carries the identified device', () => {
    const s = withSnapshot();
    const device = {
      model: 'UL5E',
      firmware: '2.0.8',
      topology: s.state!.topology,
      input_names: ['Mic 1'],
      stereo_names: [''],
      mix_names: ['Main 1-2'],
      mix_order: [0],
      stereo_inputs: [false],
      stereo_buses: [true, false],
    };
    applyEvent(s, { event: 'session', active_preset: null, modified: false, connection: 'online', device });
    expect(s.session.device).toEqual(device);
  });
  it('presets / labels / meters / error', () => {
    const s = withSnapshot();
    applyEvent(s, { event: 'presets', names: ['x', 'y'] });
    applyEvent(s, { event: 'labels', channels: { '0': { name: 'Vox', color: '#06B6D4' } } });
    applyEvent(s, { event: 'meters', levels: [0.1, 0.2] });
    applyEvent(s, { event: 'error', message: 'boom' } satisfies ServerEvent);
    expect(s.presets).toEqual(['x', 'y']);
    expect(s.labels.channels['0'].name).toBe('Vox');
    expect(s.meters).toEqual([0.1, 0.2]);
    expect(s.toasts).toEqual([{ id: 1, message: 'boom' }]);
  });
});
