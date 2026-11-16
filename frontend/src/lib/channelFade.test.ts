import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChannelFade } from './channelFade.svelte';
import { FADE_SAFE_DB, PHONES_STORAGE_KEY, FADE_STORAGE_KEY, loadRemembered, saveRemembered } from './fade';
import { TRIM_MIN_DB } from './protocol';

/** One simulated channel: optimistic store value, the writes sent, readiness. */
function channel(key: string, start: number) {
  const ch = {
    value: start as number | undefined,
    ready: true,
    writes: [] as { v: number; final: boolean }[],
    cancels: 0,
    fade: undefined as unknown as ChannelFade,
  };
  ch.fade = new ChannelFade({
    key,
    read: () => ch.value,
    write: (v, final) => {
      ch.value = v;
      ch.writes.push({ v, final });
    },
    ready: () => ch.ready,
    cancel: () => ch.cancels++,
  });
  return ch;
}

describe('ChannelFade', () => {
  const m = new Map<string, string>();
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout', 'performance', 'Date'] });
    m.clear();
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => m.get(k) ?? null,
      setItem: (k: string, v: string) => void m.set(k, v),
      removeItem: (k: string) => void m.delete(k),
    });
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('Off sweeps down to the minimum, On returns to the remembered level and never above it', () => {
    const c = channel(FADE_STORAGE_KEY, -30);
    expect(c.fade.on).toBe(true);
    c.fade.toggle();
    expect(c.fade.fading).toBe('out');
    expect(c.fade.on).toBe(false);
    expect(loadRemembered(FADE_STORAGE_KEY)).toBe(-30);
    vi.advanceTimersByTime(2000);
    expect(c.fade.fading).toBeNull();
    expect(c.value).toBe(TRIM_MIN_DB);
    expect(c.writes.at(-1)).toEqual({ v: TRIM_MIN_DB, final: true });
    for (const w of c.writes) expect(w.v).toBeLessThanOrEqual(-30);
    const outVals = c.writes.map((w) => w.v);
    expect([...outVals].sort((a, b) => b - a)).toEqual(outVals);

    c.writes.length = 0;
    c.fade.toggle();
    expect(c.fade.fading).toBe('in');
    vi.advanceTimersByTime(2000);
    expect(c.value).toBe(-30);
    expect(c.writes.at(-1)).toEqual({ v: -30, final: true });
    for (const w of c.writes) expect(w.v).toBeLessThanOrEqual(-30);
    // used once
    expect(loadRemembered(FADE_STORAGE_KEY)).toBeNull();
    // only one timer ever runs
    expect(vi.getTimerCount()).toBe(0);
  });

  it('On with nothing remembered stops at the safe level', () => {
    const c = channel(FADE_STORAGE_KEY, TRIM_MIN_DB);
    expect(c.fade.on).toBe(false);
    c.fade.toggle();
    vi.advanceTimersByTime(2000);
    expect(c.value).toBe(FADE_SAFE_DB);
    for (const w of c.writes) expect(w.v).toBeLessThanOrEqual(FADE_SAFE_DB);
  });

  it('a press mid-fade reverses from where it is, without a jump, and never above the remembered level', () => {
    const c = channel(FADE_STORAGE_KEY, -20);
    c.fade.toggle();
    vi.advanceTimersByTime(400);
    const mid = c.value!;
    expect(mid).toBeLessThan(-20);
    expect(mid).toBeGreaterThan(TRIM_MIN_DB);
    c.writes.length = 0;
    c.fade.toggle();
    expect(c.fade.fading).toBe('in');
    expect(Math.abs(c.writes[0].v - mid)).toBeLessThanOrEqual(1);
    // reverse again mid-way: the level in progress is not remembered
    vi.advanceTimersByTime(100);
    c.fade.toggle();
    expect(loadRemembered(FADE_STORAGE_KEY)).toBe(-20);
    vi.advanceTimersByTime(2000);
    expect(c.value).toBe(TRIM_MIN_DB);
    c.writes.length = 0;
    c.fade.toggle();
    vi.advanceTimersByTime(2000);
    expect(c.value).toBe(-20);
    for (const w of c.writes) expect(w.v).toBeLessThanOrEqual(-20);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('a change made elsewhere aborts the fade and drops its queued value', () => {
    saveRemembered(FADE_STORAGE_KEY, -10);
    const c = channel(FADE_STORAGE_KEY, TRIM_MIN_DB);
    c.fade.toggle();
    vi.advanceTimersByTime(200);
    c.value = -90; // another client turns it down
    const n = c.writes.length;
    vi.advanceTimersByTime(100);
    expect(c.fade.fading).toBeNull();
    expect(c.writes.length).toBe(n);
    expect(c.value).toBe(-90);
    expect(c.cancels).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('losing readiness aborts; stop() and a knob drag clear the timer', () => {
    const c = channel(FADE_STORAGE_KEY, -30);
    c.fade.toggle();
    vi.advanceTimersByTime(100);
    c.ready = false;
    vi.advanceTimersByTime(100);
    expect(c.fade.fading).toBeNull();
    expect(vi.getTimerCount()).toBe(0);

    c.ready = true;
    c.fade.toggle();
    c.fade.stop();
    expect(vi.getTimerCount()).toBe(0);

    c.fade.toggle();
    c.fade.user(-12, true);
    expect(c.fade.fading).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
    expect(c.value).toBe(-12);
  });

  it('toggle does nothing while not ready', () => {
    const c = channel(FADE_STORAGE_KEY, -30);
    c.ready = false;
    c.fade.toggle();
    expect(c.fade.fading).toBeNull();
    expect(c.writes).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('Monitor and Phones fade and remember independently', () => {
    const a = channel(FADE_STORAGE_KEY, -30);
    const b = channel(PHONES_STORAGE_KEY, -12);
    a.fade.toggle();
    b.fade.toggle();
    vi.advanceTimersByTime(300);
    a.fade.toggle(); // reverse Monitor only
    vi.advanceTimersByTime(2000);
    expect(a.value).toBe(-30);
    expect(b.value).toBe(TRIM_MIN_DB);
    expect(loadRemembered(PHONES_STORAGE_KEY)).toBe(-12);
    b.fade.toggle();
    vi.advanceTimersByTime(2000);
    expect(b.value).toBe(-12);
    for (const w of b.writes) expect(w.v).toBeLessThanOrEqual(-12);
  });
});
