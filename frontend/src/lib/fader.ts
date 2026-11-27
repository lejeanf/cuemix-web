// Fader laws — exact TypeScript port of `src/motu/fader.rs`.
//
// The engine works in native units (linear gain, whole dB). These functions
// convert to/from the 0..1 *widget position* only; never send positions to
// the server. Any change here must be mirrored in Rust (and vice versa).
//
//   mix fader : gain(p) = (p / 0.75)^K, K = (12/20) / log10(1/0.75) ≈ 4.8024
//               0 → -inf · 0.5 → -16.9 dB · 0.75 → 0 dB · 1.0 → +12 dB
//   trims     : dB(p) = dB_mix(p · 0.75), 1.0 → 0 dB, floor at TRIM_MIN_DB

export const UNITY_POS = 0.75;
export const MAX_DB = 12;

export const EXPONENT = MAX_DB / 20 / Math.log10(1 / UNITY_POS);
export const MAX_GAIN = 10 ** (MAX_DB / 20);

const sanitize = (pos: number): number => (Number.isFinite(pos) ? Math.min(1, Math.max(0, pos)) : 0);

/** Position → linear gain (0..MAX_GAIN). */
export function posToGain(pos: number): number {
  const p = sanitize(pos);
  return p <= 0 ? 0 : (p / UNITY_POS) ** EXPONENT;
}

/** Linear gain → position (0..1). */
export function gainToPos(gain: number): number {
  if (Number.isNaN(gain) || gain <= 0) return 0;
  if (gain === Infinity) return 1;
  const g = Math.min(gain, MAX_GAIN);
  return Math.min(1, Math.max(0, UNITY_POS * g ** (1 / EXPONENT)));
}

/** Position → dB (-Infinity..MAX_DB). */
export function posToDb(pos: number): number {
  const p = sanitize(pos);
  return p <= 0 ? -Infinity : 20 * EXPONENT * Math.log10(p / UNITY_POS);
}

/** dB → position (0..1). */
export function dbToPos(db: number): number {
  if (Number.isNaN(db) || db === -Infinity) return 0;
  const d = Math.min(db, MAX_DB);
  return Math.min(1, Math.max(0, UNITY_POS * 10 ** (d / (20 * EXPONENT))));
}

/** Linear gain → dB. */
export const gainToDb = (gain: number): number => (gain <= 0 ? -Infinity : 20 * Math.log10(gain));
/** dB → linear gain. */
export const dbToGain = (db: number): number => (db === -Infinity ? 0 : 10 ** (db / 20));

/** Trim taper (main / phones): position → dB, never below `floorDb`. */
export function trimPosToDb(pos: number, floorDb: number): number {
  return Math.max(posToDb(sanitize(pos) * UNITY_POS), floorDb);
}

/** Trim taper inverse: dB (≤ 0) → position. */
export function trimDbToPos(db: number): number {
  return Math.min(1, Math.max(0, dbToPos(Math.min(db, 0)) / UNITY_POS));
}

/**
 * Volume knob curve (Monitor, Phones): the trim taper of the sliders, stretched so the
 * floor sits at the start of the arc. Knob position 0..1 ↔ dB (floorDb..0). Most of the
 * travel goes to the top 40 dB, where listening levels are. Shared with the iOS app
 * through `design/vectors/volume-knob.json`.
 */
export function volKnobToNorm(db: number, floorDb: number): number {
  if (!Number.isFinite(db) || db <= floorDb) return 0;
  const p0 = trimDbToPos(floorDb);
  return Math.min(1, Math.max(0, (trimDbToPos(db) - p0) / (1 - p0)));
}

/** Inverse of `volKnobToNorm` (not rounded). */
export function volKnobFromNorm(n: number, floorDb: number): number {
  const c = Math.min(1, Math.max(0, Number.isFinite(n) ? n : 0));
  if (c <= 0) return floorDb;
  const p0 = trimDbToPos(floorDb);
  return Math.min(0, trimPosToDb(p0 + c * (1 - p0), floorDb));
}

/** Human label: "-inf", "-6.0", "+3.5" (dB). */
export function formatDb(db: number, digits = 1): string {
  if (!Number.isFinite(db)) return db < 0 ? '-inf' : '+inf';
  const s = db.toFixed(digits);
  return db > 0 ? `+${s}` : s;
}
