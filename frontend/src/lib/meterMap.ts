// UltraLite-mk5 meter layout. The device streams property 6000: one byte per
// point of its internal patch bus, level = -byte / 2 dB (255 = nothing).
// Point numbering follows the patch-bus sources: USB playback 0-22, ADAT in
// 24-31, analog in 32-43 (hardware order, see MK5_ANALOG_IN), S/PDIF in 44-45,
// DSP outputs from 46 (output processing, input post-FX, reverb). Verified on
// a card with the meter probe (mic hiss, optical and line sources).
//
// The app-side meter bank is indexed by "meter slots" (see METER_SLOT).

/** Hardware position of Mic 1, Mic 2, Line 3…8 on the analog input bus (offset from 32). */
const MK5_ANALOG_IN = [5, 0, 6, 7, 3, 4, 1, 2];

/** Raw point for each mixer input (0-31), measured post input DSP like CueMix's mix-input meters. */
const MK5_MIXER_IN: readonly number[] = [
  50, 51, 52, 53, 58, 59, 60, 61, // Mic 1-2, Line 3-8, after EQ/dynamics
  44, 45, // S/PDIF 1-2
  24, 25, 26, 27, 28, 29, 30, 31, // ADAT 1-8
  0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, // USB playback, one pair per mix
  68, 69, // reverb return (wet)
];

/** Raw point for each output: Main L/R, Line 3-10, Phones L/R (output processing, before the trim). */
const MK5_OUTPUT: readonly number[] = [46, 47, 48, 49, 54, 55, 56, 57, 62, 63, 64, 65];

/**
 * Destination index in the device patch table (property 5010) of each output,
 * same order as MK5_OUTPUT: Main L/R, Line 3-10, Phones L/R. null = not known
 * yet. Fill this in after reading the "Patch (5010)" table of the meter probe
 * (`/?probe`) on the card; until then the outputs are metered at MK5_OUTPUT.
 */
export const MK5_PATCH_DEST: readonly (number | null)[] = [
  null, null, null, null, null, null, null, null, null, null, null, null,
];

/**
 * Meter point of the source really patched to `output`'s destination, or the
 * output's own processing point (MK5_OUTPUT) when the patch or the destination
 * index is unknown.
 */
export function outputSourcePoint(
  output: number,
  patch: readonly number[] | null,
  dests: readonly (number | null)[] = MK5_PATCH_DEST,
): number {
  const fallback = MK5_OUTPUT[output] ?? 255;
  const dest = dests[output];
  if (!patch || dest === null || dest === undefined) return fallback;
  const src = patch[dest];
  return src === undefined || src >= 255 ? fallback : src;
}

export const MIXER_INPUTS = 32;
export const OUTPUTS = MK5_OUTPUT.length;
export const PREAMPS = MK5_ANALOG_IN.length;

/** Slots in the meter bank: mixer inputs, then outputs, then preamp inputs (before DSP). */
export const METER_SLOT = {
  mixerInput: (ch: number) => ch,
  output: (o: number) => MIXER_INPUTS + o,
  preamp: (ch: number) => MIXER_INPUTS + OUTPUTS + ch,
};

/** One raw byte → linear peak 0..1. */
export function rawToLinear(b: number | undefined): number {
  if (b === undefined || !Number.isFinite(b) || b >= 255) return 0;
  return Math.pow(10, -Math.max(0, b) / 40);
}

/** Whether a device uses this layout. */
export function hasMk5Meters(model: string | undefined, rawLength: number): boolean {
  return !!model?.startsWith('UL5') && rawLength >= 70;
}

/** Raw mk5 meter frame → linear levels per meter slot. */
export function mk5MeterLevels(raw: readonly number[], patch: readonly number[] | null = null): number[] {
  const out: number[] = [];
  MK5_MIXER_IN.forEach((p, i) => (out[METER_SLOT.mixerInput(i)] = rawToLinear(raw[p])));
  MK5_OUTPUT.forEach((_, i) => (out[METER_SLOT.output(i)] = rawToLinear(raw[outputSourcePoint(i, patch)])));
  MK5_ANALOG_IN.forEach((p, i) => (out[METER_SLOT.preamp(i)] = rawToLinear(raw[32 + p])));
  return out;
}
