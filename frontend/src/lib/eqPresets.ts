import { EQ_MODE, type EqBand } from './protocol';

/** One band of a preset. Shelves only on the first (low) and last (high)
 *  band, as the hardware allows; `q` only matters for bells. */
type PBand = { mode: number; freq_hz: number; gain_db: number; q: number };

export interface OutputEqPreset {
  id: string;
  name: string;
  /** What it is for, shown as a tooltip. */
  info: string;
  /** Low, mid, high: one per output EQ band. */
  bands: [PBand, PBand, PBand];
}

const bell = (freq_hz: number, gain_db: number, q = 1): PBand => ({ mode: EQ_MODE.BELL, freq_hz, gain_db, q });
const low = (freq_hz: number, gain_db: number): PBand => ({ mode: EQ_MODE.LOW_SHELF, freq_hz, gain_db, q: 1 });
const high = (freq_hz: number, gain_db: number): PBand => ({ mode: EQ_MODE.HIGH_SHELF, freq_hz, gain_db, q: 1 });

/** Starting points for the 3-band output EQ (home listening / home cinema). */
export const OUTPUT_EQ_PRESETS: readonly OutputEqPreset[] = [
  { id: 'flat', name: 'Flat', info: 'No correction', bands: [bell(100, 0), bell(1000, 0), bell(10000, 0)] },
  {
    id: 'voice',
    name: 'Voice clarity',
    info: 'Dialogue forward: less boom, more presence (2-3 kHz)',
    bands: [low(120, -3), bell(2500, 4, 0.9), high(8000, 1.5)],
  },
  {
    id: 'night',
    name: 'Night / dialogue',
    info: 'Low volume listening: bass cut so speech stays intelligible without waking anyone',
    bands: [low(100, -6), bell(2000, 3, 0.9), high(10000, -2)],
  },
  {
    id: 'loudness',
    name: 'Loudness',
    info: 'Low and high lift for quiet listening (Fletcher-Munson)',
    bands: [low(80, 4), bell(1000, 0), high(10000, 3)],
  },
  { id: 'bass', name: 'Bass boost', info: 'More low end', bands: [low(80, 5), bell(1000, 0), bell(10000, 0)] },
  {
    id: 'soft',
    name: 'Tame harshness',
    info: 'Softer upper mids and treble for bright speakers or recordings',
    bands: [bell(100, 0), bell(3500, -3, 1.2), high(10000, -1.5)],
  },
];

/** Bands to apply for a preset (not bypassed). */
export function presetBands(p: OutputEqPreset): EqBand[] {
  return p.bands.map((x) => ({ mode: x.mode, bypass: false, freq_hz: x.freq_hz, gain_db: x.gain_db, bw_oct: x.q }));
}

/** The preset these bands match, if any (a bypassed or 0 dB band matches any 0 dB band). */
export function matchPreset(bands: readonly EqBand[] | undefined): OutputEqPreset | undefined {
  if (!bands || bands.length !== 3) return undefined;
  const near = (a: number, c: number, tol: number) => Math.abs(a - c) <= tol;
  return OUTPUT_EQ_PRESETS.find((p) =>
    p.bands.every((pb, i) => {
      const x = bands[i];
      const gain = x.bypass ? 0 : x.gain_db;
      if (pb.gain_db === 0 && gain === 0) return true;
      if (x.bypass || x.mode !== pb.mode || !near(gain, pb.gain_db, 0.05) || !near(x.freq_hz, pb.freq_hz, 1)) return false;
      return pb.mode !== EQ_MODE.BELL || near(x.bw_oct, pb.q, 0.02);
    }),
  );
}
