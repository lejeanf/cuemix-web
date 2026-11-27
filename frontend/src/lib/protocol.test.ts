// protocol.ts and the reducer against design/vectors/protocol.json, which the server's own
// types write (tests/protocol_vectors.rs). The typed literals below are checked against
// protocol.ts by svelte-check and against the server's JSON by vitest: a field renamed on one
// side, or a message added to only one side, fails here instead of drifting.
import { describe, expect, it } from 'vitest';
import vectors from '../../../design/vectors/protocol.json';
import type { Command, MotuState, Param, ServerEvent } from './protocol';
import { applyEvent, applyParam, initialClientState } from './reducer';

const params = vectors.params as unknown as Param[];
const events = vectors.events as unknown as ServerEvent[];
const commands = vectors.commands as unknown as Command[];
const afterParams = vectors.after_params as unknown as MotuState;

function snapshotState(): MotuState {
  const ev = events.find((e) => e.event === 'snapshot');
  if (ev?.event !== 'snapshot') throw new Error('no snapshot vector');
  return structuredClone(ev.state);
}

// Exhaustive by type: a tag missing from (or unknown to) protocol.ts does not compile.
const PARAM_NAMES: Record<Param['param'], true> = {
  mix_vol: true, mix_pan: true, mix_mute: true, mix_solo: true, main_vol: true, preamp_gain: true,
  phantom: true, pad: true, phase: true, route: true, dim: true, mono: true, speaker_b: true,
  talkback: true, phones_vol: true, output_trim: true, input_eq_mode: true, input_eq_bypass: true,
  input_eq_freq: true, input_eq_gain: true, input_eq_bw: true, output_eq_mode: true,
  output_eq_bypass: true, output_eq_freq: true, output_eq_gain: true, output_eq_bw: true,
  gate_bypass: true, gate_threshold: true, gate_attack: true, gate_release: true, comp_bypass: true,
  comp_threshold: true, comp_attack: true, comp_release: true, comp_ratio: true, comp_makeup: true,
  reverb_decay: true, reverb_damping: true, reverb_predelay: true, reverb_width: true,
  reverb_preset: true, sample_rate: true, clock_source: true, optical_mode: true,
};
const EVENT_NAMES: Record<ServerEvent['event'], true> = {
  snapshot: true, changed: true, status: true, session: true, presets: true, labels: true,
  meters: true, meter_raw: true, patch: true, measure: true, power: true, speaker_plug: true,
  speaker_plug_result: true, osc_settings: true, notice: true, error: true,
};

/** Events the store consumes before the reducer (store.svelte.ts `handle`). */
const STORE_ONLY: ReadonlySet<ServerEvent['event']> = new Set(['meter_raw', 'patch', 'measure']);

const label = { name: 'Vocal', color: '#06B6D4' };
const plug = {
  enabled: true, kind: 'shelly_rpc', host: 'plug.example', switch_id: 0, on_url: '', off_url: '',
  warmup_ms: 2000, off_delay_ms: 300, timeout_ms: 2000,
} as const;
const osc = { listen: '127.0.0.1:9000', targets: ['192.0.2.10:9001'] };

/** The commands this client sends, typed by protocol.ts, in the vectors' order. */
const TYPED_COMMANDS: Command[] = [
  { cmd: 'set', param: 'mix_vol', bus: 0, ch: 1, gain: 0.5 },
  { cmd: 'load_preset', name: 'Live' },
  { cmd: 'save_preset', name: 'Live' },
  { cmd: 'import_preset', name: 'Imported', text: '{}' },
  { cmd: 'list_presets' },
  { cmd: 'set_label', ch: 0, label },
  { cmd: 'measure_start' },
  { cmd: 'measure_stop' },
  { cmd: 'power', channel: 'main', on: false },
  { cmd: 'set_speaker_plug', settings: plug },
  { cmd: 'test_speaker_plug', on: true },
  { cmd: 'set_osc_settings', settings: osc },
];

/** Every event but the snapshot, typed by protocol.ts, in the vectors' order. */
const TYPED_EVENTS: ServerEvent[] = [
  { event: 'changed', params: [params[0], params[1]] },
  { event: 'status', connection: 'demo' },
  {
    event: 'session', active_preset: 'Live', modified: true, connection: 'online',
    device: {
      model: 'UL5E', firmware: '2.0.0',
      topology: { mixes: 2, mixer_inputs: 4, preamps: 2, mic_inputs: 1, routing_inputs: 3, routing_outputs: 3 },
      input_names: [], stereo_names: [], mix_names: [], mix_order: [], stereo_inputs: [], stereo_buses: [],
    },
  },
  { event: 'presets', names: ['Live', 'Studio'] },
  { event: 'labels', channels: { 0: label } },
  { event: 'meters', levels: [0, 0.25, 0.5, 1] },
  { event: 'meter_raw', raw: [0, 64, 255] },
  { event: 'patch', sources: [0, 1, 2] },
  { event: 'measure', state: 'idle', source: [2, 3] },
  { event: 'power', main: { on: true, fading: 'out', plug: 'on' }, phones: { on: false, fading: null, plug: null } },
  {
    event: 'speaker_plug', settings: plug, state: 'off',
    urls: { on: 'http://plug.example/rpc/Switch.Set?id=0&on=true', off: 'http://plug.example/rpc/Switch.Set?id=0&on=false' },
  },
  { event: 'speaker_plug_result', on: true, ok: false, message: 'timed out' },
  { event: 'osc_settings', settings: osc, listening: true, error: null },
  { event: 'notice', level: 'warning', message: 'Link lost' },
  { event: 'error', message: 'unknown preset' },
];

describe('protocol vectors (server-generated)', () => {
  it('cover exactly the params, events and commands protocol.ts knows', () => {
    expect(params.map((p) => p.param)).toEqual(Object.keys(PARAM_NAMES));
    expect(events.map((e) => e.event).sort()).toEqual(Object.keys(EVENT_NAMES).sort());
    expect(commands.map((c) => c.cmd)).toEqual(TYPED_COMMANDS.map((c) => c.cmd));
  });

  it('commands built from protocol.ts types are what the server reads', () => {
    expect(TYPED_COMMANDS).toEqual(commands);
  });

  it('events typed by protocol.ts are what the server sends', () => {
    expect(TYPED_EVENTS).toEqual(events.filter((e) => e.event !== 'snapshot'));
  });

  it.each(params.map((p) => [p.param, p] as const))('applyParam handles %s', (_name, p) => {
    const st = snapshotState();
    applyParam(st, p);
    expect(st).not.toEqual(snapshotState());
  });

  it('applying every param gives the state the server computes', () => {
    const st = snapshotState();
    for (const p of params) applyParam(st, p);
    expect(st).toEqual(afterParams);
  });

  it.each(events.filter((e) => !STORE_ONLY.has(e.event)).map((e) => [e.event, e] as const))('applyEvent handles %s', (_name, ev) => {
    // After a snapshot, as on a live socket (`changed` needs a state to change).
    const s = initialClientState();
    if (ev.event !== 'snapshot') applyEvent(s, { event: 'snapshot', state: snapshotState(), session: { active_preset: null, modified: false, connection: 'online' } });
    const before = structuredClone(s);
    applyEvent(s, structuredClone(ev));
    expect(s).not.toEqual(before);
  });
});
