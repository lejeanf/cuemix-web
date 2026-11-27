// Mixer strip groups (Mic · Line · S/PDIF · Optical · Computer · Reverb), so a long row of
// identical strips reads by kind. Mixer input order is CueMix 5's (docs/MOTU-PROTOCOL.md):
// analog inputs (mics first), S/PDIF 1-2, ADAT 1-8, USB returns (one pair per mix), reverb return.

export interface StripRef {
  ch: number;
  pair?: number;
}

export interface StripGroup {
  title: string;
  strips: StripRef[];
}

export interface MixerLayout {
  /** Mixer inputs (32 on the UltraLite-mk5). */
  inputs: number;
  mics: number;
  /** Analog inputs (preamps), mics included. */
  analog: number;
  /** Mixer input carrying the reverb return (left of its pair), if any. */
  reverbReturn: number | null;
}

/** Group title of one mixer input. */
export function groupOf(ch: number, l: MixerLayout): string {
  if (l.reverbReturn !== null && (ch === l.reverbReturn || ch === l.reverbReturn + 1)) return 'Reverb';
  if (ch < l.mics) return 'Mic';
  if (ch < l.analog) return 'Line';
  // The digital and USB blocks are known for the 32-input layout only.
  if (l.inputs === 32 && l.analog === 8) {
    if (ch < 10) return 'S/PDIF';
    if (ch < 18) return 'Optical';
    if (ch < 30) return 'Computer';
  }
  return 'Other';
}

/** Consecutive strips of the same kind, in mixer order. */
export function groupStrips(strips: StripRef[], l: MixerLayout): StripGroup[] {
  const out: StripGroup[] = [];
  for (const s of strips) {
    const title = groupOf(s.ch, l);
    const last = out.at(-1);
    if (last && last.title === title) last.strips.push(s);
    else out.push({ title, strips: [s] });
  }
  return out;
}
