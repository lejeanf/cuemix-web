import { describe, expect, it } from 'vitest';
import { visibleTabs } from './hwmap';

describe('visibleTabs', () => {
  it('hides Reverb on a MOTU while reverb is not written', () => {
    expect(visibleTabs({ online: true, reverbWritable: false, probe: false })).toEqual(['mixer', 'input', 'output']);
  });
  it('keeps Reverb in demo mode and once reverb is written', () => {
    expect(visibleTabs({ online: false, reverbWritable: false, probe: false })).toContain('reverb');
    expect(visibleTabs({ online: true, reverbWritable: true, probe: false })).toContain('reverb');
  });
  it('appends the probe tab last', () => {
    expect(visibleTabs({ online: true, reverbWritable: false, probe: true }).at(-1)).toBe('probe');
  });
});
