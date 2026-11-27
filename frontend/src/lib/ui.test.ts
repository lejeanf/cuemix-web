import { describe, expect, it } from 'vitest';
import { TOAST_MS, tabFromHash, toastTimeout } from './ui';

describe('tabFromHash', () => {
  const tabs = ['mixer', 'input', 'output', 'reverb'] as const;
  it('reads a known view with or without the #', () => {
    expect(tabFromHash('#output', tabs)).toBe('output');
    expect(tabFromHash('input', tabs)).toBe('input');
  });
  it('ignores an unknown or hidden view', () => {
    expect(tabFromHash('#probe', tabs)).toBeNull();
    expect(tabFromHash('#nope', tabs)).toBeNull();
    expect(tabFromHash('', tabs)).toBeNull();
  });
});

describe('toastTimeout', () => {
  it('keeps errors until dismissed', () => {
    expect(toastTimeout({})).toBeNull();
  });
  it('fades notices and warnings', () => {
    expect(toastTimeout({ level: 'info' })).toBe(TOAST_MS);
    expect(toastTimeout({ level: 'warning' })).toBe(TOAST_MS);
  });
});
