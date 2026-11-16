import { TRIM_MIN_DB } from './protocol';

/**
 * Longest fade: a full sweep from the minimum to 0 dB (100 dB). The knob is linear in dB,
 * so it turns smoothly the whole way; shorter spans fade proportionally faster.
 */
export const FADE_MAX_MS = 1200;
export const FADE_MIN_MS = 250;
/** Span (dB) that takes `FADE_MAX_MS`: the whole range of the volume knobs. */
export const FADE_FULL_DB = -TRIM_MIN_DB;
/** Pace of the intermediate writes (the store coalesces to 25 ms per parameter anyway). */
export const FADE_STEP_MS = 28;
/** Used by "On" when no previous level was remembered: never louder than this. */
export const FADE_SAFE_DB = -70;
/** Remembered level per channel (localStorage keys). */
export const FADE_STORAGE_KEY = 'cuemix.mainBeforeOff';
export const PHONES_STORAGE_KEY = 'cuemix.phonesBeforeOff';

/** A volume counts as off when it sits at (or within half a dB of) the minimum. */
export const isOff = (db: number): boolean => db <= TRIM_MIN_DB + 0.5;

/** Smoothstep: gentle start and end, so the fade does not click in or out. */
export const ease = (t: number): number => {
  const c = Math.min(1, Math.max(0, t));
  return c * c * (3 - 2 * c);
};

/** dB at time `t` (0..1) of a fade, linear in dB with an ease. Never leaves [from, to]. */
export function fadeDb(from: number, to: number, t: number): number {
  return from + (to - from) * ease(t);
}

/** Duration for a fade over `spanDb` decibels: 250 ms to 1.2 s (12 ms per dB). */
export function fadeMs(spanDb: number): number {
  return Math.min(FADE_MAX_MS, Math.max(FADE_MIN_MS, (Math.abs(spanDb) * FADE_MAX_MS) / FADE_FULL_DB));
}

export interface FadePlan {
  from: number;
  to: number;
  ms: number;
  /** Value of the last write (final): the minimum for a fade out, the target for a fade in. */
  end: number;
}

/** Fade out from `current` all the way to the minimum (the knob sweeps its whole arc). */
export function planOut(current: number): FadePlan {
  const from = Math.max(TRIM_MIN_DB, Math.min(0, current));
  return { from, to: TRIM_MIN_DB, ms: fadeMs(from - TRIM_MIN_DB), end: TRIM_MIN_DB };
}

/**
 * Fade in from `current` (the minimum, or mid-fade) up to the remembered level.
 * Never goes above `remembered` (or `FADE_SAFE_DB` when there is none), nor above 0 dB.
 */
export function planIn(current: number, remembered: number | null): FadePlan {
  const cap = Math.min(0, remembered !== null && Number.isFinite(remembered) ? Math.round(remembered) : FADE_SAFE_DB);
  const target = Math.max(TRIM_MIN_DB, cap);
  const from = Math.min(target, Math.max(current, TRIM_MIN_DB));
  return { from, to: target, ms: fadeMs(target - from), end: target };
}

/** Whole-dB value to write at time `t` (0..1), clamped to the plan's range. */
export function valueAt(plan: FadePlan, t: number): number {
  if (t >= 1) return plan.end;
  const lo = Math.min(plan.from, plan.to);
  const hi = Math.max(plan.from, plan.to);
  return Math.min(hi, Math.max(lo, Math.round(fadeDb(plan.from, plan.to, t))));
}

export function loadRemembered(key: string): number | null {
  try {
    const v = localStorage.getItem(key);
    // Number('') is 0 (full level): an empty or blank value counts as nothing remembered.
    if (v === null || v.trim() === '') return null;
    const n = Number(v);
    return Number.isFinite(n) && n > TRIM_MIN_DB + 0.5 && n <= 0 ? n : null;
  } catch {
    return null;
  }
}

export function saveRemembered(key: string, db: number): void {
  try {
    localStorage.setItem(key, String(Math.round(db)));
  } catch {
    /* storage blocked: On falls back to the safe level */
  }
}

export function forgetRemembered(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* storage blocked: nothing was remembered */
  }
}
