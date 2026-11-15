import { describe, expect, it } from 'vitest';
import { dbToPos, gainToPos, posToDb, posToGain, trimDbToPos, trimPosToDb } from './fader';

describe('fader law parity with Rust', () => {
  it('anchors', () => {
    expect(posToGain(0.75)).toBeCloseTo(1, 12);
    expect(posToDb(1)).toBeCloseTo(12, 9);
    expect(posToDb(0.5)).toBeCloseTo(-16.9, 1);
    expect(posToGain(0)).toBe(0);
    expect(trimPosToDb(0, -100)).toBe(-100);
    expect(trimPosToDb(1, -100)).toBeCloseTo(0, 9);
  });
  it('round trips over 1000 points', () => {
    for (let i = 1; i <= 1000; i++) {
      const p = i / 1000;
      expect(Math.abs(gainToPos(posToGain(p)) - p)).toBeLessThan(1e-9);
      expect(Math.abs(dbToPos(posToDb(p)) - p)).toBeLessThan(1e-9);
    }
    for (let i = 100; i <= 1000; i++) {
      const p = i / 1000;
      expect(Math.abs(trimDbToPos(trimPosToDb(p, -1000)) - p)).toBeLessThan(1e-9);
    }
  });
});
