import { LABEL_NAME_MAX, PRESET_NAME_RE } from './protocol';

export const isValidPresetName = (n: string): boolean => PRESET_NAME_RE.test(n);

/** 1..LABEL_NAME_MAX printable characters (no control, bidi-override or zero-width characters, like src/state.rs), counted by code point. */
export function isValidLabelName(n: string): boolean {
  const len = [...n].length;
  return len >= 1 && len <= LABEL_NAME_MAX && !/[\u0000-\u001f\u007f-\u009f؜​‎‏‪-‮⁠⁦-⁩﻿]/.test(n) && n.trim().length > 0;
}

export const isHexColor = (c: string): boolean => /^#[0-9A-Fa-f]{6}$/.test(c);

export const clamp = (v: number, [lo, hi]: readonly [number, number]): number =>
  Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : lo;
