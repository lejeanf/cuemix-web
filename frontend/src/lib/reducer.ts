// Client reducer: applies server events (and optimistic local edits) to the
// client state. It mutates `s` in place (so Svelte's deep $state proxy keeps
// fine-grained reactivity) and returns it.
import type { Labels, MotuState, Param, ServerEvent, SessionInfo } from './protocol';

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

/** Apply one parameter to the state (used for `changed` and optimistic edits). Out-of-range indices are ignored. */
export function applyParam(st: MotuState, p: Param): void {
  switch (p.param) {
    case 'mix_vol': {
      const c = st.mixes[p.bus]?.channels[p.ch];
      if (c) c.gain = p.gain;
      break;
    }
    case 'mix_pan': {
      const c = st.mixes[p.bus]?.channels[p.ch];
      if (c) c.pan = p.value;
      break;
    }
    case 'mix_mute': {
      const c = st.mixes[p.bus]?.channels[p.ch];
      if (c) c.mute = p.on;
      break;
    }
    case 'mix_solo': {
      const c = st.mixes[p.bus]?.channels[p.ch];
      if (c) c.solo = p.on;
      break;
    }
    case 'main_vol':
      st.main.db = p.db;
      break;
    case 'preamp_gain': {
      const a = st.preamps[p.ch];
      if (a) a.gain_db = p.db;
      break;
    }
    case 'phantom': {
      const a = st.preamps[p.ch];
      if (a) a.phantom = p.on;
      break;
    }
    case 'pad': {
      const a = st.preamps[p.ch];
      if (a) a.pad = p.on;
      break;
    }
    case 'phase': {
      const a = st.preamps[p.ch];
      if (a) a.phase = p.on;
      break;
    }
    case 'eq_bypass': {
      const c = st.channels[p.ch];
      if (c) c.eq.bypass = p.on;
      break;
    }
    case 'eq_band': {
      const b = st.channels[p.ch]?.eq.bands[p.band];
      if (b) {
        b.enabled = p.enabled;
        b.freq_hz = p.freq_hz;
        b.gain_db = p.gain_db;
        b.q = p.q;
      }
      break;
    }
    case 'hpf': {
      const h = st.channels[p.ch]?.eq.hpf;
      if (h) {
        h.enabled = p.enabled;
        h.freq_hz = p.freq_hz;
      }
      break;
    }
    case 'comp': {
      const c = st.channels[p.ch]?.comp;
      if (c) {
        c.enabled = p.enabled;
        c.threshold_db = p.threshold_db;
        c.ratio = p.ratio;
        c.attack_ms = p.attack_ms;
        c.release_ms = p.release_ms;
        c.gain_db = p.gain_db;
      }
      break;
    }
    case 'gate': {
      const g = st.channels[p.ch]?.gate;
      if (g) {
        g.enabled = p.enabled;
        g.threshold_db = p.threshold_db;
        g.attack_ms = p.attack_ms;
        g.release_ms = p.release_ms;
      }
      break;
    }
    case 'reverb':
      st.reverb.enabled = p.enabled;
      st.reverb.return_gain = p.return_gain;
      st.reverb.time_s = p.time_s;
      st.reverb.predelay_ms = p.predelay_ms;
      break;
    case 'reverb_send':
      if (p.ch >= 0 && p.ch < st.reverb.sends.length) st.reverb.sends[p.ch] = p.gain;
      break;
    case 'route': {
      const row = st.routing.matrix[p.input];
      if (row && p.output >= 0 && p.output < row.length) row[p.output] = p.on;
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
    case 'clock':
      st.clock.sample_rate = p.sample_rate;
      st.clock.source = p.source;
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
      };
      break;
    case 'presets':
      s.presets = [...ev.names];
      break;
    case 'labels':
      s.labels = { channels: { ...ev.channels } };
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
