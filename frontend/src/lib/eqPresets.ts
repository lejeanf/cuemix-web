import { EQ_MODE, type EqBand } from './protocol';

/** One band of a preset: always a bell (the only output EQ type confirmed on a card).
 *  Wide bells (2-3 octaves) at the ends of the range stand in for shelves. */
type PBand = { freq_hz: number; gain_db: number; bw_oct: number };

export interface OutputEqPreset {
  id: string;
  name: string;
  /** What it is for, shown as a tooltip. */
  info: string;
  /** Low, mid, high: one per output EQ band. */
  bands: [PBand, PBand, PBand];
}

const b = (freq_hz: number, gain_db: number, bw_oct: number): PBand => ({ freq_hz, gain_db, bw_oct });

/** Starting points for the 3-band output EQ (home listening / home cinema). */
export const OUTPUT_EQ_PRESETS: readonly OutputEqPreset[] = [
  { id: 'flat', name: 'Flat', info: 'No correction', bands: [b(100, 0, 1), b(1000, 0, 1), b(10000, 0, 1)] },
  {
    id: 'voice',
    name: 'Voice clarity',
    info: 'Dialogue forward: less boom, more presence (2-3 kHz)',
    bands: [b(120, -3, 2), b(2500, 4, 1.5), b(8000, 1.5, 2)],
  },
  {
    id: 'night',
    name: 'Night / dialogue',
    info: 'Low volume listening: bass cut so speech stays intelligible without waking anyone',
    bands: [b(80, -6, 2.5), b(2000, 3, 1.5), b(10000, -2, 2)],
  },
  {
    id: 'loudness',
    name: 'Loudness',
    info: 'Low and high lift for quiet listening (Fletcher-Munson)',
    bands: [b(60, 4, 2), b(1000, 0, 1), b(12000, 3, 2)],
  },
  { id: 'bass', name: 'Bass boost', info: 'More low end', bands: [b(60, 5, 2), b(1000, 0, 1), b(10000, 0, 1)] },
  {
    id: 'soft',
    name: 'Tame harshness',
    info: 'Softer upper mids and treble for bright speakers or recordings',
    bands: [b(100, 0, 1), b(3500, -3, 1.2), b(10000, -1.5, 2)],
  },
];

/** Bands to apply for a preset (bells, not bypassed). */
export function presetBands(p: OutputEqPreset): EqBand[] {
  return p.bands.map((x) => ({ mode: EQ_MODE.BELL, bypass: false, ...x }));
}

/** The preset these bands match, if any (a bypassed band counts as 0 dB). */
export function matchPreset(bands: readonly EqBand[] | undefined): OutputEqPreset | undefined {
  if (!bands || bands.length !== 3) return undefined;
  const near = (a: number, c: number, tol: number) => Math.abs(a - c) <= tol;
  return OUTPUT_EQ_PRESETS.find((p) =>
    p.bands.every((pb, i) => {
      const x = bands[i];
      const gain = x.bypass ? 0 : x.gain_db;
      if (pb.gain_db === 0 && gain === 0) return true;
      return x.mode === EQ_MODE.BELL && !x.bypass && near(gain, pb.gain_db, 0.05) && near(x.freq_hz, pb.freq_hz, 1) && near(x.bw_oct, pb.bw_oct, 0.02);
    }),
  );
}
