// Small semver helper for release tags like "v0.7.0" or "0.7.0-rc.1" (build metadata ignored).
export interface Semver {
  major: number;
  minor: number;
  patch: number;
  pre: string[];
}

const RE = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

export function parseSemver(s: string): Semver | null {
  const m = RE.exec(s.trim());
  if (!m) return null;
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre: m[4] ? m[4].split('.') : [] };
}

const isNum = (x: string) => /^\d+$/.test(x);

/** Negative if a < b, positive if a > b, 0 if equal. A pre-release sorts below its release. */
export function compareSemver(a: Semver, b: Semver): number {
  for (const k of ['major', 'minor', 'patch'] as const) if (a[k] !== b[k]) return a[k] < b[k] ? -1 : 1;
  if (!a.pre.length || !b.pre.length) return a.pre.length === b.pre.length ? 0 : a.pre.length ? -1 : 1;
  for (let i = 0; i < Math.min(a.pre.length, b.pre.length); i++) {
    const x = a.pre[i];
    const y = b.pre[i];
    if (x === y) continue;
    if (isNum(x) && isNum(y)) return Number(x) < Number(y) ? -1 : 1;
    if (isNum(x) !== isNum(y)) return isNum(x) ? -1 : 1; // numeric identifiers sort below alphanumeric
    return x < y ? -1 : 1;
  }
  return a.pre.length === b.pre.length ? 0 : a.pre.length < b.pre.length ? -1 : 1;
}

/** Compares two version strings; null when either is not a version. */
export function compareVersions(a: string, b: string): number | null {
  const x = parseSemver(a);
  const y = parseSemver(b);
  return x && y ? compareSemver(x, y) : null;
}
