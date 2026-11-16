import { TRIM_MIN_DB } from './protocol';

/** Where the audible part of a master fade starts and ends (below it the card is near silence). */
export const FADE_FLOOR_DB = -60;
/** Longest fade; shorter spans fade proportionally faster (about 40 dB per second). */
export const FADE_MAX_MS = 1000;
export const FADE_MIN_MS = 250;
/** Pace of the intermediate writes (the store coalesces to 25 ms per parameter anyway). */
export const FADE_STEP_MS = 28;
/** Used by "On" when no previous level was remembered: never louder than this. */
export const FADE_SAFE_DB = -70;
export const FADE_STORAGE_KEY = 'cuemix.mainBeforeOff';

/** Main counts as off when it sits at (or within half a dB of) the minimum. */
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

/** Duration for a fade over `spanDb` decibels, 250 ms to 1 s. */
export function fadeMs(spanDb: number): number {
  return Math.min(FADE_MAX_MS, Math.max(FADE_MIN_MS, (Math.abs(spanDb) * FADE_MAX_MS) / 40));
}

export interface FadePlan {
  from: number;
  to: number;
  ms: number;
  /** Value of the last write (final): the minimum for a fade out, the target for a fade in. */
  end: number;
}

/** Fade out from `current` to silence. Starts audible ramp at `current`, ends on the minimum. */
export function planOut(current: number): FadePlan {
  const from = Math.min(0, current);
  const to = Math.min(from, FADE_FLOOR_DB);
  return { from, to, ms: fadeMs(from - to), end: TRIM_MIN_DB };
}

/**
 * Fade in from `current` (the minimum, or mid-fade) to the remembered level.
 * Never goes above `remembered` (or `FADE_SAFE_DB` when there is none), nor above 0 dB.
 */
export function planIn(current: number, remembered: number | null): FadePlan {
  const cap = Math.min(0, remembered !== null && Number.isFinite(remembered) ? Math.round(remembered) : FADE_SAFE_DB);
  const target = Math.max(TRIM_MIN_DB, cap);
  const from = Math.min(target, Math.max(current, Math.min(FADE_FLOOR_DB, target)));
  return { from, to: target, ms: fadeMs(target - from), end: target };
}

/** Whole-dB value to write at time `t` (0..1), clamped to the plan's range. */
export function valueAt(plan: FadePlan, t: number): number {
  if (t >= 1) return plan.end;
  const lo = Math.min(plan.from, plan.to);
  const hi = Math.max(plan.from, plan.to);
  return Math.min(hi, Math.max(lo, Math.round(fadeDb(plan.from, plan.to, t))));
}

export function loadRemembered(): number | null {
  try {
    const v = localStorage.getItem(FADE_STORAGE_KEY);
    // Number('') is 0 (full level): an empty or blank value counts as nothing remembered.
    if (v === null || v.trim() === '') return null;
    const n = Number(v);
    return Number.isFinite(n) && n > TRIM_MIN_DB + 0.5 && n <= 0 ? n : null;
  } catch {
    return null;
  }
}

export function saveRemembered(db: number): void {
  try {
    localStorage.setItem(FADE_STORAGE_KEY, String(Math.round(db)));
  } catch {
    /* storage blocked: On falls back to the safe level */
  }
}

export function forgetRemembered(): void {
  try {
    localStorage.removeItem(FADE_STORAGE_KEY);
  } catch {
    /* storage blocked: nothing was remembered */
  }
}
