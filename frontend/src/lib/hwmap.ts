import type { ParamName } from './protocol';

/** Params actually written to the MOTU today (ARCHITECTURE §8 Q3). Everything else is state-only. */
export const HW_MAPPED_PARAMS: ReadonlySet<ParamName> = new Set<ParamName>([
  'preamp_gain',
  'phase',
  'pad',
  'phantom',
  'main_vol',
  'phones_vol',
  'mono',
  'talkback',
]);

export const isHwMapped = (p: ParamName): boolean => HW_MAPPED_PARAMS.has(p);
