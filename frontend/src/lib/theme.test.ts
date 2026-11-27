import { describe, expect, it } from 'vitest';
import { THEME_KEY, loadTheme, saveTheme } from './theme';

describe('night mode choice', () => {
  it('defaults to the normal theme and survives blocked storage', () => {
    const m = new Map<string, string>();
    const s = { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
    expect(loadTheme(s)).toBe('dark');
    saveTheme(s, 'night');
    expect(m.get(THEME_KEY)).toBe('night');
    expect(loadTheme(s)).toBe('night');
    m.set(THEME_KEY, 'neon');
    expect(loadTheme(s)).toBe('dark');
    const blocked = { getItem: () => { throw new Error('x'); }, setItem: () => { throw new Error('x'); } };
    expect(loadTheme(blocked)).toBe('dark');
    expect(() => saveTheme(blocked, 'night')).not.toThrow();
  });
});
