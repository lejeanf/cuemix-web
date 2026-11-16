import type { ParamName } from './protocol';

export type Tab = 'mixer' | 'input' | 'output' | 'reverb' | 'probe';

/** Params actually written to the MOTU today (ARCHITECTURE §8 Q3). Everything else is state-only. */
export const HW_MAPPED_PARAMS: ReadonlySet<ParamName> = new Set<ParamName>([
  'preamp_gain',
  'phase',
  'pad',
  'phantom',
  'main_vol',
  'phones_vol',
  'output_trim',
  'mono',
  'talkback',
  // Mix matrix: written with the layout read from the device on connect.
  'mix_vol',
  'mix_pan',
  'mix_mute',
  'mix_solo',
  // Prop 5006, from CueMix 5's property table.
  'optical_mode',
  // Input EQ, 8 channels x 4 bands (props 1002-1006).
  'input_eq_mode',
  'input_eq_bypass',
  'input_eq_freq',
  'input_eq_gain',
  'input_eq_bw',
  // Output EQ, 12 slots x 3 bands (props 1022-1026); slot 1 is unused and never written.
  'output_eq_mode',
  'output_eq_bypass',
  'output_eq_freq',
  'output_eq_gain',
  'output_eq_bw',
]);

/** Tab list for the header: Reverb is hidden online while its params are not written. */
export function visibleTabs(o: { online: boolean; reverbWritable: boolean; probe: boolean }): Tab[] {
  const t: Tab[] = ['mixer', 'input', 'output'];
  if (!o.online || o.reverbWritable) t.push('reverb');
  if (o.probe) t.push('probe');
  return t;
}

export const isHwMapped = (p: ParamName): boolean => HW_MAPPED_PARAMS.has(p);

/**
 * Params read from the MOTU (shown as the hardware has them) but not written yet:
 * the dynamics / reverb / clock params whose encoders exist in
 * `src/motu/props.rs` but are not active (writes get enabled one feature at a time).
 */
export const HW_READ_ONLY_PARAMS: ReadonlySet<ParamName> = new Set<ParamName>([
  'gate_bypass',
  'gate_threshold',
  'gate_attack',
  'gate_release',
  'comp_bypass',
  'comp_threshold',
  'comp_attack',
  'comp_release',
  'comp_ratio',
  'comp_makeup',
  'reverb_decay',
  'reverb_damping',
  'reverb_predelay',
  'reverb_width',
  'reverb_preset',
  'sample_rate',
  'clock_source',
]);

export const isHwReadOnly = (p: ParamName): boolean => HW_READ_ONLY_PARAMS.has(p);
