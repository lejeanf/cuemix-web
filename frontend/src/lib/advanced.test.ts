import { describe, expect, it } from 'vitest';
import { ADVANCED_TAB_KEY, ADVANCED_TABS, loadAdvancedTab, nextTab, saveAdvancedTab } from './advanced';

const mem = () => {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), m };
};
const blocked = {
  getItem: () => {
    throw new Error('blocked');
  },
  setItem: () => {
    throw new Error('blocked');
  },
};

describe('Advanced settings tab memory', () => {
  it('defaults to the speaker plug tab', () => {
    expect(loadAdvancedTab(mem())).toBe('plug');
    expect(loadAdvancedTab(null)).toBe('plug');
  });
  it('remembers the last tab used', () => {
    const s = mem();
    saveAdvancedTab(s, 'version');
    expect(s.m.get(ADVANCED_TAB_KEY)).toBe('version');
    expect(loadAdvancedTab(s)).toBe('version');
  });
  it('ignores unknown stored values and blocked storage', () => {
    const s = mem();
    for (const bad of ['', 'nope', '__proto__', 'PLUG', '<script>']) {
      s.setItem(ADVANCED_TAB_KEY, bad);
      expect(loadAdvancedTab(s)).toBe('plug');
    }
    expect(loadAdvancedTab(blocked)).toBe('plug');
    expect(() => saveAdvancedTab(blocked, 'osc')).not.toThrow();
  });
});

describe('tab keyboard navigation', () => {
  it('wraps with the arrow keys and jumps with Home and End', () => {
    expect(ADVANCED_TABS).toEqual(['plug', 'osc', 'version']);
    expect(nextTab('plug', 'ArrowRight')).toBe('osc');
    expect(nextTab('version', 'ArrowRight')).toBe('plug');
    expect(nextTab('plug', 'ArrowLeft')).toBe('version');
    expect(nextTab('osc', 'ArrowLeft')).toBe('plug');
    expect(nextTab('osc', 'Home')).toBe('plug');
    expect(nextTab('osc', 'End')).toBe('version');
    expect(nextTab('osc', 'Tab')).toBeNull();
    expect(nextTab('osc', 'a')).toBeNull();
  });
});
