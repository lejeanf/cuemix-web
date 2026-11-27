import { describe, expect, it } from 'vitest';
import { dbToPos, gainToPos, posToDb, posToGain, trimDbToPos, trimPosToDb, volKnobFromNorm, volKnobToNorm } from './fader';
import knob from '../../../design/vectors/volume-knob.json';

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

describe('volume knob curve (design/vectors/volume-knob.json)', () => {
  it('matches every shared point both ways', () => {
    for (const { db, pos } of knob.points) {
      expect(Math.abs(volKnobToNorm(db, knob.floor_db) - pos)).toBeLessThan(knob.tolerance);
      expect(Math.round(volKnobFromNorm(pos, knob.floor_db))).toBe(db);
    }
  });
  it('is monotonic and bounded', () => {
    let last = -Infinity;
    for (let i = 0; i <= 1000; i++) {
      const db = volKnobFromNorm(i / 1000, -100);
      expect(db).toBeGreaterThanOrEqual(last);
      expect(db).toBeLessThanOrEqual(0);
      expect(db).toBeGreaterThanOrEqual(-100);
      last = db;
    }
    expect(volKnobToNorm(-Infinity, -100)).toBe(0);
    expect(volKnobToNorm(6, -100)).toBe(1);
  });
});
