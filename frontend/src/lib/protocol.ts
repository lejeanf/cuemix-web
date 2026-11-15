// JSON WebSocket contract (`/ws`) — TypeScript mirror of `src/state.rs`.
//
// Source of truth: the Rust types and docs/ARCHITECTURE.md §3.5. Keep this file
// in sync with them (field names, units, 0-based indices). Units are the MOTU's
// native ones (decision D8): mix faders and sends are *linear gain*, trims and
// preamps are *whole dB*. The 0..1 fader position only exists in the UI widget
// (see ./fader.ts).

// ---------------------------------------------------------------- state ---

export type ConnectionStatus = 'online' | 'connecting' | 'demo';
export type SampleRate = '44100' | '48000' | '88200' | '96000' | '176400' | '192000';
export type ClockSource = 'internal' | 'spdif' | 'optical';

export interface Topology {
  mixes: number;
  mixer_inputs: number;
  /** Inputs with gain + phase. */
  preamps: number;
  /** Leading preamps that are mic inputs (only these have +48V / Pad). */
  mic_inputs: number;
  routing_inputs: number;
  routing_outputs: number;
}

export interface MixChannel {
  /** Linear gain 0..MIX_GAIN_MAX, 1.0 = 0 dB. */
  gain: number;
  /** -1 (L) .. 1 (R). */
  pan: number;
  mute: boolean;
  solo: boolean;
}

export interface Mix {
  name: string;
  channels: MixChannel[];
}

export interface Preamp {
  /** Whole dB, 0..74 on mic inputs, 0..20 on line inputs. */
  gain_db: number;
  phantom: boolean;
  pad: boolean;
  phase: boolean;
}

export interface EqBand {
  enabled: boolean;
  freq_hz: number;
  gain_db: number;
  q: number;
}

export interface Hpf {
  enabled: boolean;
  freq_hz: number;
}

export interface Equalizer {
  /** Global bypass of the 4 bands (cuemix-web improvement). */
  bypass: boolean;
  hpf: Hpf;
  bands: [EqBand, EqBand, EqBand, EqBand];
}

export interface Compressor {
  enabled: boolean;
  threshold_db: number;
  ratio: number;
  attack_ms: number;
  release_ms: number;
  gain_db: number;
}

export interface Gate {
  enabled: boolean;
  threshold_db: number;
  attack_ms: number;
  release_ms: number;
}

export interface ChannelDsp {
  eq: Equalizer;
  comp: Compressor;
  gate: Gate;
}

export interface Reverb {
  enabled: boolean;
  /** Linear gain. */
  return_gain: number;
  time_s: number;
  predelay_ms: number;
  /** Per mixer input, linear gain. */
  sends: number[];
}

export interface ControlRoom {
  dim: boolean;
  mono: boolean;
  /** false = speakers A, true = speakers B. */
  speaker_b: boolean;
  talkback: boolean;
  /** Whole dB, -100..0. */
  phones_db: number;
}

export interface MotuState {
  topology: Topology;
  mixes: Mix[];
  main: { /** Whole dB, -100..0. */ db: number };
  preamps: Preamp[];
  channels: ChannelDsp[];
  reverb: Reverb;
  routing: { /** matrix[input][output] */ matrix: boolean[][] };
  control_room: ControlRoom;
  clock: { sample_rate: SampleRate; source: ClockSource };
}

export interface SessionInfo {
  active_preset: string | null;
  modified: boolean;
  connection: ConnectionStatus;
}

export interface ChannelLabel {
  /** 1..64 printable characters. */
  name: string;
  /** `#RRGGBB` or null. */
  color: string | null;
}

/** Keys are 0-based mixer input indices serialised as strings. */
export interface Labels {
  channels: Record<string, ChannelLabel>;
}

// ---------------------------------------------------------------- params ---

export type Param =
  | { param: 'mix_vol'; bus: number; ch: number; gain: number }
  | { param: 'mix_pan'; bus: number; ch: number; value: number }
  | { param: 'mix_mute'; bus: number; ch: number; on: boolean }
  | { param: 'mix_solo'; bus: number; ch: number; on: boolean }
  | { param: 'main_vol'; db: number }
  | { param: 'preamp_gain'; ch: number; db: number }
  | { param: 'phantom'; ch: number; on: boolean }
  | { param: 'pad'; ch: number; on: boolean }
  | { param: 'phase'; ch: number; on: boolean }
  | { param: 'eq_bypass'; ch: number; on: boolean }
  | { param: 'route'; input: number; output: number; on: boolean }
  | { param: 'dim'; on: boolean }
  | { param: 'mono'; on: boolean }
  | { param: 'speaker_b'; on: boolean }
  | { param: 'talkback'; on: boolean }
  | { param: 'phones_vol'; db: number }
  | ({ param: 'eq_band'; ch: number; band: number } & EqBand)
  | ({ param: 'hpf'; ch: number } & Hpf)
  | ({ param: 'comp'; ch: number } & Compressor)
  | ({ param: 'gate'; ch: number } & Gate)
  | { param: 'reverb'; enabled: boolean; return_gain: number; time_s: number; predelay_ms: number }
  | { param: 'reverb_send'; ch: number; gain: number }
  | { param: 'clock'; sample_rate: SampleRate; source: ClockSource };

export type ParamName = Param['param'];

// ------------------------------------------------------------- messages ---

/** Client → server. */
export type Command =
  | ({ cmd: 'set' } & Param)
  | { cmd: 'load_preset'; name: string }
  | { cmd: 'save_preset'; name: string }
  | { cmd: 'list_presets' }
  | { cmd: 'set_label'; ch: number; label: ChannelLabel | null };

/** Server → client. On connect: `snapshot`, then `labels`, then `presets`. */
export type ServerEvent =
  | { event: 'snapshot'; state: MotuState; session: SessionInfo }
  | { event: 'changed'; params: Param[] }
  | { event: 'status'; connection: ConnectionStatus }
  | ({ event: 'session' } & SessionInfo)
  | { event: 'presets'; names: string[] }
  | ({ event: 'labels' } & Labels)
  | { event: 'meters'; levels: number[] }
  | { event: 'error'; message: string };

// --------------------------------------------------------------- limits ---
// Mirrors of the Rust constants (src/state.rs). Values are clamped server-side;
// the UI uses them for widget ranges.

export const MIX_GAIN_MAX = 3.981071705534972; // +12 dB
export const TRIM_MIN_DB = -100;
export const PREAMP_GAIN_MAX_DB = 74;
export const LINE_GAIN_MAX_DB = 20;
export const LABEL_NAME_MAX = 64;
export const PRESET_NAME_RE = /^[A-Za-z0-9 _.-]{1,64}$/;

export const DSP_LIMITS = {
  FREQ_HZ: [20, 20_000],
  EQ_GAIN_DB: [-20, 20],
  EQ_Q: [0.1, 10],
  COMP_THRESHOLD_DB: [-60, 0],
  COMP_RATIO: [1, 20],
  COMP_GAIN_DB: [0, 24],
  GATE_THRESHOLD_DB: [-80, 0],
  ATTACK_MS: [0.1, 200],
  RELEASE_MS: [10, 2_000],
  REVERB_TIME_S: [0.1, 10],
  REVERB_PREDELAY_MS: [0, 200],
} as const satisfies Record<string, readonly [number, number]>;

export const SAMPLE_RATES: readonly SampleRate[] = ['44100', '48000', '88200', '96000', '176400', '192000'];
export const CLOCK_SOURCES: readonly ClockSource[] = ['internal', 'spdif', 'optical'];
