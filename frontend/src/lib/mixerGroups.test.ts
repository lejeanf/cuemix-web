import { describe, expect, it } from 'vitest';
import { groupOf, groupStrips } from './mixerGroups';

const mk5 = { inputs: 32, mics: 2, analog: 8, reverbReturn: 30 };

describe('mixer groups', () => {
  it('names every mk5 mixer input by kind', () => {
    const names = Array.from({ length: 32 }, (_, ch) => groupOf(ch, mk5));
    expect(names.slice(0, 2)).toEqual(['Mic', 'Mic']);
    expect(new Set(names.slice(2, 8))).toEqual(new Set(['Line']));
    expect(names.slice(8, 10)).toEqual(['S/PDIF', 'S/PDIF']);
    expect(new Set(names.slice(10, 18))).toEqual(new Set(['Optical']));
    expect(new Set(names.slice(18, 30))).toEqual(new Set(['Computer']));
    expect(names.slice(30)).toEqual(['Reverb', 'Reverb']);
  });
  it('groups consecutive strips, stereo pairs included', () => {
    const strips = [{ ch: 0 }, { ch: 1 }, { ch: 2, pair: 3 }, { ch: 8, pair: 9 }, { ch: 30, pair: 31 }];
    expect(groupStrips(strips, mk5).map((g) => [g.title, g.strips.length])).toEqual([
      ['Mic', 2],
      ['Line', 1],
      ['S/PDIF', 1],
      ['Reverb', 1],
    ]);
  });
  it('does not guess the digital blocks of another layout', () => {
    const other = { inputs: 24, mics: 4, analog: 8, reverbReturn: null };
    expect(groupOf(12, other)).toBe('Other');
    expect(groupOf(5, other)).toBe('Line');
  });
});
