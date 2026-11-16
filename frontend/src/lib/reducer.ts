// Client reducer: applies server events (and optimistic local edits) to the
// client state. It mutates `s` in place (so Svelte's deep $state proxy keeps
// fine-grained reactivity) and returns it.
import type { EqBand, Labels, MotuState, Param, ServerEvent, SessionInfo } from './protocol';

export interface Toast {
  id: number;
  message: string;
}

export interface ClientState {
  state: MotuState | null;
  session: SessionInfo;
  presets: string[];
  labels: Labels;
  meters: number[];
  toasts: Toast[];
  toastSeq: number;
}

export function initialClientState(): ClientState {
  return {
    state: null,
    session: { active_preset: null, modified: false, connection: 'connecting' },
    presets: [],
    labels: { channels: {} },
    meters: [],
    toasts: [],
    toastSeq: 0,
  };
}

/**
 * Element `i` of `arr`, or undefined unless `i` is an in-range integer index.
 * Server payloads are untrusted data: a non-numeric key such as "__proto__"
 * must never select (and then mutate) a prototype object.
 */
function at<T>(arr: readonly T[] | undefined, i: unknown): T | undefined {
  if (!arr || typeof i !== 'number' || !Number.isInteger(i) || i < 0 || i >= arr.length) return undefined;
  return arr[i];
}

/**
 * Replace element `i` of `arr` with `v` when `i` is an in-range integer index.
 * Uses `splice` rather than `arr[i] = v`, so no property write keyed by
 * payload data is ever performed.
 */
function setAt<T>(arr: T[], i: unknown, v: T): void {
  if (typeof i === 'number' && Number.isInteger(i) && i >= 0 && i < arr.length) arr.splice(i, 1, v);
}

/** EQ band `band` of row `ch` of `rows` (input or output EQ), index-guarded. */
function eqBand(rows: EqBand[][] | undefined, ch: unknown, band: unknown): EqBand | undefined {
  return at(at(rows, ch), band);
}

/** Apply one parameter to the state (used for `changed` and optimistic edits). Out-of-range indices are ignored. */
export function applyParam(st: MotuState, p: Param): void {
  switch (p.param) {
    case 'mix_vol': {
      const c = at(at(st.mixes, p.bus)?.channels, p.ch);
      if (c) c.gain = p.gain;
      break;
    }
    case 'mix_pan': {
      const c = at(at(st.mixes, p.bus)?.channels, p.ch);
      if (c) c.pan = p.value;
      break;
    }
    case 'mix_mute': {
      const c = at(at(st.mixes, p.bus)?.channels, p.ch);
      if (c) c.mute = p.on;
      break;
    }
    case 'mix_solo': {
      const c = at(at(st.mixes, p.bus)?.channels, p.ch);
      if (c) c.solo = p.on;
      break;
    }
    case 'main_vol':
      st.main.db = p.db;
      break;
    case 'preamp_gain': {
      const a = at(st.preamps, p.ch);
      if (a) a.gain_db = p.db;
      break;
    }
    case 'phantom': {
      const a = at(st.preamps, p.ch);
      if (a) a.phantom = p.on;
      break;
    }
    case 'pad': {
      const a = at(st.preamps, p.ch);
      if (a) a.pad = p.on;
      break;
    }
    case 'phase': {
      const a = at(st.preamps, p.ch);
      if (a) a.phase = p.on;
      break;
    }
    case 'input_eq_mode':
    case 'output_eq_mode': {
      const b = eqBand(p.param === 'input_eq_mode' ? st.input_eq : st.output_eq, p.ch, p.band);
      if (b) b.mode = p.mode;
      break;
    }
    case 'input_eq_bypass':
    case 'output_eq_bypass': {
      const b = eqBand(p.param === 'input_eq_bypass' ? st.input_eq : st.output_eq, p.ch, p.band);
      if (b) b.bypass = p.on;
      break;
    }
    case 'input_eq_freq':
    case 'output_eq_freq': {
      const b = eqBand(p.param === 'input_eq_freq' ? st.input_eq : st.output_eq, p.ch, p.band);
      if (b) b.freq_hz = p.hz;
      break;
    }
    case 'input_eq_gain':
    case 'output_eq_gain': {
      const b = eqBand(p.param === 'input_eq_gain' ? st.input_eq : st.output_eq, p.ch, p.band);
      if (b) b.gain_db = p.db;
      break;
    }
    case 'input_eq_bw':
    case 'output_eq_bw': {
      const b = eqBand(p.param === 'input_eq_bw' ? st.input_eq : st.output_eq, p.ch, p.band);
      if (b) b.bw_oct = p.octaves;
      break;
    }
    case 'gate_bypass': {
      const g = at(st.gates, p.ch);
      if (g) g.bypass = p.on;
      break;
    }
    case 'gate_threshold': {
      const g = at(st.gates, p.ch);
      if (g) g.threshold_db = p.db;
      break;
    }
    case 'gate_attack': {
      const g = at(st.gates, p.ch);
      if (g) g.attack_ms = p.ms;
      break;
    }
    case 'gate_release': {
      const g = at(st.gates, p.ch);
      if (g) g.release_ms = p.ms;
      break;
    }
    case 'comp_bypass': {
      const c = at(st.compressors, p.ch);
      if (c) c.bypass = p.on;
      break;
    }
    case 'comp_threshold': {
      const c = at(st.compressors, p.ch);
      if (c) c.threshold_db = p.db;
      break;
    }
    case 'comp_attack': {
      const c = at(st.compressors, p.ch);
      if (c) c.attack_ms = p.ms;
      break;
    }
    case 'comp_release': {
      const c = at(st.compressors, p.ch);
      if (c) c.release_ms = p.ms;
      break;
    }
    case 'comp_ratio': {
      const c = at(st.compressors, p.ch);
      if (c) c.ratio = p.ratio;
      break;
    }
    case 'comp_makeup': {
      const c = at(st.compressors, p.ch);
      if (c) c.makeup = p.gain;
      break;
    }
    case 'reverb_decay':
      st.reverb.decay = p.value;
      break;
    case 'reverb_damping':
      st.reverb.damping = p.value;
      break;
    case 'reverb_predelay':
      st.reverb.predelay_ms = p.ms;
      break;
    case 'reverb_width':
      st.reverb.width = p.value;
      break;
    case 'reverb_preset':
      st.reverb.preset = p.preset;
      break;
    case 'route': {
      const row = at(st.routing.matrix, p.input);
      if (row) setAt(row, p.output, p.on);
      break;
    }
    case 'dim':
      st.control_room.dim = p.on;
      break;
    case 'mono':
      st.control_room.mono = p.on;
      break;
    case 'speaker_b':
      st.control_room.speaker_b = p.on;
      break;
    case 'talkback':
      st.control_room.talkback = p.on;
      break;
    case 'phones_vol':
      st.control_room.phones_db = p.db;
      break;
    case 'output_trim':
      if (st.output_trims) setAt(st.output_trims, p.ch, p.db);
      break;
    case 'sample_rate':
      st.clock.sample_rate = p.rate;
      break;
    case 'clock_source':
      st.clock.source = p.source;
      break;
    case 'optical_mode':
      if (p.output) st.clock.optical_out = p.mode;
      else st.clock.optical_in = p.mode;
      break;
  }
}

export function applyEvent(s: ClientState, ev: ServerEvent): ClientState {
  switch (ev.event) {
    case 'snapshot':
      s.state = ev.state;
      s.session = { ...ev.session };
      break;
    case 'changed':
      if (s.state) for (const p of ev.params) applyParam(s.state, p);
      break;
    case 'status':
      s.session.connection = ev.connection;
      break;
    case 'session':
      s.session = {
        active_preset: ev.active_preset,
        modified: ev.modified,
        connection: ev.connection,
        device: ev.device ?? null,
      };
      break;
    case 'presets':
      s.presets = [...ev.names];
      break;
    case 'labels':
      s.labels = { channels: { ...ev.channels } };
      break;
    case 'meter_raw':
      break;
    case 'meters':
      s.meters = ev.levels;
      break;
    case 'error':
      s.toasts.push({ id: ++s.toastSeq, message: ev.message });
      break;
  }
  return s;
}
