import { describe, expect, it } from 'vitest';
import type { EqBand, MotuState, ServerEvent } from './protocol';
import { applyEvent, applyParam, initialClientState } from './reducer';

const band = (): EqBand => ({ enabled: false, freq_hz: 1000, gain_db: 0, q: 1 });
const dsp = () => ({
  eq: { bypass: false, hpf: { enabled: false, freq_hz: 80 }, bands: [band(), band(), band(), band()] as MotuState['channels'][0]['eq']['bands'] },
  comp: { enabled: false, threshold_db: -20, ratio: 2, attack_ms: 10, release_ms: 100, gain_db: 0 },
  gate: { enabled: false, threshold_db: -60, attack_ms: 1, release_ms: 100 },
});

function mkState(): MotuState {
  return {
    topology: { mixes: 2, mixer_inputs: 2, preamps: 2, mic_inputs: 1, routing_inputs: 2, routing_outputs: 2 },
    mixes: [0, 1].map((i) => ({ name: `Mix ${i + 1}`, channels: [0, 1].map(() => ({ gain: 1, pan: 0, mute: false, solo: false })) })),
    main: { db: 0 },
    preamps: [0, 1].map(() => ({ gain_db: 0, phantom: false, pad: false, phase: false })),
    channels: [dsp(), dsp()],
    reverb: { enabled: false, return_gain: 1, time_s: 2, predelay_ms: 0, sends: [0, 0] },
    routing: { matrix: [[false, false], [false, false]] },
    control_room: { dim: false, mono: false, speaker_b: false, talkback: false, phones_db: -20 },
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
        { param: 'eq_band', ch: 1, band: 2, enabled: true, freq_hz: 500, gain_db: 3, q: 2 },
        { param: 'route', input: 1, output: 0, on: true },
        { param: 'reverb_send', ch: 1, gain: 0.7 },
        { param: 'clock', sample_rate: '96000', source: 'spdif' },
      ],
    });
    expect(s.state?.mixes[1].channels[0].gain).toBe(0.5);
    expect(s.state?.control_room.dim).toBe(true);
    expect(s.state?.channels[1].eq.bands[2]).toEqual({ enabled: true, freq_hz: 500, gain_db: 3, q: 2 });
    expect(s.state?.routing.matrix[1][0]).toBe(true);
    expect(s.state?.reverb.sends[1]).toBe(0.7);
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
      { param: 'eq_band', ch: 0, band: '__proto__', enabled: true, freq_hz: 1, gain_db: 1, q: 1 },
      { param: 'reverb_send', ch: '__proto__', gain: 1 },
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
  it('session carries the identified device', () => {
    const s = withSnapshot();
    const device = {
      model: 'UL5E',
      firmware: '2.0.8',
      topology: s.state!.topology,
      input_names: ['Mic 1'],
      mix_names: ['Main 1-2'],
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
