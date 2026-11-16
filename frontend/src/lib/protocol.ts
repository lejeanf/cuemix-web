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

/** One EQ band as the MOTU stores it (input EQ props 1002-1006, output 1022-1026). */
export interface EqBand {
  /** Filter type, raw byte (see EQ_MODE); unknown values are kept. */
  mode: number;
  /** true = bypassed. */
  bypass: boolean;
  /** Whole Hz. */
  freq_hz: number;
  gain_db: number;
  /** Bandwidth in octaves. */
  bw_oct: number;
}

export interface Compressor {
  /** true = bypassed. */
  bypass: boolean;
  threshold_db: number;
  /** Whole ms. */
  attack_ms: number;
  /** Whole ms. */
  release_ms: number;
  ratio: number;
  /** Make-up gain, linear (1.0 = 0 dB). */
  makeup: number;
}

export interface Gate {
  /** true = bypassed. */
  bypass: boolean;
  threshold_db: number;
  attack_ms: number;
  release_ms: number;
}

/** Reverb processor. Sends = faders of the Reverb mix; return = a mixer input in every mix. */
export interface Reverb {
  /** Whole 0..100. */
  decay: number;
  damping: number;
  /** Whole ms 0..100. */
  predelay_ms: number;
  width: number;
  /** Raw preset number. */
  preset: number;
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
  /** input_eq[ch][band]: analog inputs (topology.preamps) x INPUT_EQ_BANDS. */
  input_eq: EqBand[][];
  /** One per analog input. */
  compressors: Compressor[];
  /** One per mic input. */
  gates: Gate[];
  /** output_eq[out][band]: OUTPUT_EQ_OUTPUTS x OUTPUT_EQ_BANDS. */
  output_eq: EqBand[][];
  reverb: Reverb;
  routing: { /** matrix[input][output] */ matrix: boolean[][] };
  control_room: ControlRoom;
  /** Analog line output trims, whole dB -100..0 (index 0 = output 1). */
  output_trims: number[];
  clock: { sample_rate: SampleRate; source: ClockSource };
}

export interface SessionInfo {
  active_preset: string | null;
  modified: boolean;
  connection: ConnectionStatus;
  /** What the connected MOTU reported about itself (null in demo mode). */
  device?: DeviceInfo | null;
}

/** Identity and layout read from the MOTU's state dump on connect. */
export interface DeviceInfo {
  model: string;
  firmware: string;
  topology: Topology;
  /** Per mixer input; empty string = no name known. */
  input_names: string[];
  /** Name when the input is the left channel of a stereo pair; empty = use input_names. */
  stereo_names: string[];
  /** Per stereo mix; empty string = no name known. */
  mix_names: string[];
  /** Display order of the mixes (empty = index order). */
  mix_order: number[];
  /** Per mixer input: true on the left channel of a stereo pair. */
  stereo_inputs: boolean[];
  stereo_buses: boolean[];
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
  | { param: 'route'; input: number; output: number; on: boolean }
  | { param: 'dim'; on: boolean }
  | { param: 'mono'; on: boolean }
  | { param: 'speaker_b'; on: boolean }
  | { param: 'talkback'; on: boolean }
  | { param: 'phones_vol'; db: number }
  | { param: 'output_trim'; ch: number; db: number }
  | { param: 'input_eq_mode'; ch: number; band: number; mode: number }
  | { param: 'input_eq_bypass'; ch: number; band: number; on: boolean }
  | { param: 'input_eq_freq'; ch: number; band: number; hz: number }
  | { param: 'input_eq_gain'; ch: number; band: number; db: number }
  | { param: 'input_eq_bw'; ch: number; band: number; octaves: number }
  | { param: 'output_eq_mode'; ch: number; band: number; mode: number }
  | { param: 'output_eq_bypass'; ch: number; band: number; on: boolean }
  | { param: 'output_eq_freq'; ch: number; band: number; hz: number }
  | { param: 'output_eq_gain'; ch: number; band: number; db: number }
  | { param: 'output_eq_bw'; ch: number; band: number; octaves: number }
  | { param: 'gate_bypass'; ch: number; on: boolean }
  | { param: 'gate_threshold'; ch: number; db: number }
  | { param: 'gate_attack'; ch: number; ms: number }
  | { param: 'gate_release'; ch: number; ms: number }
  | { param: 'comp_bypass'; ch: number; on: boolean }
  | { param: 'comp_threshold'; ch: number; db: number }
  | { param: 'comp_attack'; ch: number; ms: number }
  | { param: 'comp_release'; ch: number; ms: number }
  | { param: 'comp_ratio'; ch: number; ratio: number }
  | { param: 'comp_makeup'; ch: number; gain: number }
  | { param: 'reverb_decay'; value: number }
  | { param: 'reverb_damping'; value: number }
  | { param: 'reverb_predelay'; ms: number }
  | { param: 'reverb_width'; value: number }
  | { param: 'reverb_preset'; preset: number }
  | { param: 'sample_rate'; rate: SampleRate }
  | { param: 'clock_source'; source: ClockSource };

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
  /** Raw meter frame from the MOTU (prop 6000), for the meter probe. */
  | { event: 'meter_raw'; raw: number[] }
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

/** Provisional ranges (src/state.rs dsp_limits), wide so MOTU values are never distorted. */
export const DSP_LIMITS = {
  FREQ_HZ: [20, 20_000],
  EQ_GAIN_DB: [-24, 24],
  EQ_BW_OCT: [0.01, 4],
  THRESHOLD_DB: [-100, 0],
  COMP_RATIO: [1, 100],
  /** Linear make-up gain. */
  COMP_MAKEUP: [0, 16],
  ATTACK_MS: [0, 1_000],
  RELEASE_MS: [0, 10_000],
  REVERB_PERCENT: [0, 100],
  REVERB_PREDELAY_MS: [0, 100],
} as const satisfies Record<string, readonly [number, number]>;

export const INPUT_EQ_BANDS = 4;
export const OUTPUT_EQ_OUTPUTS = 12;
export const OUTPUT_EQ_BANDS = 3;

/** EQ band filter types seen in the MOTU dump (props 1002 / 1022). */
export const EQ_MODE = { BELL: 0, SHELF: 2, HIGH_PASS: 3 } as const;

/** Short name of an EQ mode byte; unknown values are shown as their number. */
export function eqModeName(mode: number): string {
  switch (mode) {
    case EQ_MODE.BELL:
      return 'Bell';
    case EQ_MODE.SHELF:
      return 'Shelf';
    case EQ_MODE.HIGH_PASS:
      return 'HPF';
    default:
      return `Type ${mode}`;
  }
}

export const SAMPLE_RATES: readonly SampleRate[] = ['44100', '48000', '88200', '96000', '176400', '192000'];
export const CLOCK_SOURCES: readonly ClockSource[] = ['internal', 'spdif', 'optical'];
