// OSC settings (contract 0.7.0 A): form helpers mirroring src/osc/settings.rs. The server
// validates too (and canonicalises IPs, resolves names and applies the local-address fence);
// this copy drives the Save button and the inline messages.
import type { OscSettings } from './protocol';

export const OSC_MAX_TARGETS = 8;
export const OSC_LISTEN_MIN_PORT = 1024;

/** Why a value is rejected; the text lives in strings.ts (S.osc.errors). */
export type OscError =
  | 'listenEmpty'
  | 'listenFormat'
  | 'listenPort'
  | 'listenAddr'
  | 'format'
  | 'targetPort'
  | 'v6target'
  | 'tooMany';

/** Dotted quad like Rust's Ipv4Addr: four numbers 0..255 without leading zeros. */
export function parseIpv4(s: string): number[] | null {
  const m = /^(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})$/.exec(s);
  if (!m) return null;
  const o = m.slice(1).map(Number);
  return o.every((n) => n <= 255) ? o : null;
}

/** IPv6 text (with `::` compression and an optional IPv4 tail) to its 8 groups, or null. */
export function parseIpv6(s: string): number[] | null {
  if (!/^[0-9A-Fa-f:.]+$/.test(s) || !s.includes(':')) return null;
  const halves = s.split('::');
  if (halves.length > 2) return null;
  const side = (t: string, last: boolean): number[] | null => {
    if (t === '') return [];
    const out: number[] = [];
    const parts = t.split(':');
    for (let i = 0; i < parts.length; i++) {
      const p = parts[i];
      if (i === parts.length - 1 && last && p.includes('.')) {
        const v4 = parseIpv4(p);
        if (!v4) return null;
        out.push(v4[0] * 256 + v4[1], v4[2] * 256 + v4[3]);
      } else if (/^[0-9A-Fa-f]{1,4}$/.test(p)) out.push(parseInt(p, 16));
      else return null;
    }
    return out;
  };
  if (halves.length === 1) {
    const g = side(s, true);
    return g && g.length === 8 ? g : null;
  }
  const head = side(halves[0], false);
  const tail = side(halves[1], true);
  if (!head || !tail || head.length + tail.length > 7) return null;
  return [...head, ...Array<number>(8 - head.length - tail.length).fill(0), ...tail];
}

/** A host name by the server's rules: dot separated labels of 1..63 letters, digits, `-` or `_`, none starting or ending with `-`. */
export function validHostname(h: string): boolean {
  return (
    h.length > 0 &&
    h.length <= 253 &&
    h.split('.').every((l) => l.length > 0 && l.length <= 63 && !l.startsWith('-') && !l.endsWith('-') && /^[A-Za-z0-9_-]+$/.test(l))
  );
}

const parsePort = (s: string): number | null => (/^\d{1,5}$/.test(s) && Number(s) <= 65535 ? Number(s) : null);

/** Splits `host:port` or `[v6]:port` at the last colon the way the server does; the host keeps no brackets. */
export function splitHostPort(s: string): { host: string; port: string; bracketed: boolean } | null {
  if (s.startsWith('[')) {
    const m = /^\[([^\]]*)\]:(.*)$/.exec(s);
    return m ? { host: m[1], port: m[2], bracketed: true } : null;
  }
  const i = s.lastIndexOf(':');
  if (i < 0) return null;
  const host = s.slice(0, i);
  return host.includes(':') ? null : { host, port: s.slice(i + 1), bracketed: false };
}

/** First problem with the "ip:port" the server listens on, or null. */
export function listenProblem(raw: string): OscError | null {
  const s = raw.trim();
  if (s === '') return 'listenEmpty';
  const hp = splitHostPort(s);
  if (!hp) return 'listenFormat';
  const v4 = hp.bracketed ? null : parseIpv4(hp.host);
  const v6 = hp.bracketed ? parseIpv6(hp.host) : null;
  if (!v4 && !v6) return 'listenFormat';
  const port = parsePort(hp.port);
  if (port === null) return 'listenFormat';
  if (port < OSC_LISTEN_MIN_PORT) return 'listenPort';
  const multicast = v4 ? v4[0] >= 224 && v4[0] <= 239 : v6![0] >= 0xff00;
  const broadcast = !!v4 && v4.every((n) => n === 255);
  return multicast || broadcast ? 'listenAddr' : null;
}

/** The address family of a valid listen string, or null. */
export function listenFamily(raw: string): 4 | 6 | null {
  const hp = splitHostPort(raw.trim());
  if (!hp || listenProblem(raw)) return null;
  return hp.bracketed ? 6 : 4;
}

/** First problem with one feedback target "host:port", or null. */
export function targetProblem(raw: string, listen4 = false): OscError | null {
  const hp = splitHostPort(raw.trim());
  if (!hp) return 'format';
  const port = parsePort(hp.port);
  if (port === null || port < 1) return hp.port !== '' && /^\d+$/.test(hp.port) ? 'targetPort' : 'format';
  const v6 = hp.bracketed ? parseIpv6(hp.host) : null;
  const v4 = hp.bracketed ? parseIpv4(hp.host) : null;
  if (hp.bracketed ? !v6 && !v4 : !validHostname(hp.host)) return 'format';
  return listen4 && v6 ? 'v6target' : null;
}

/** Settings as sent: trimmed, empty target rows dropped. */
export function normalizeOsc(s: OscSettings): OscSettings {
  return { listen: s.listen.trim(), targets: s.targets.map((t) => t.trim()).filter((t) => t !== '') };
}

export interface OscProblems {
  listen: OscError | null;
  /** One entry per normalized target. */
  targets: (OscError | null)[];
  /** Per normalized target: index of the earlier identical target, or null. */
  dupOf: (number | null)[];
  /** Whole-list problem (too many). */
  list: OscError | null;
}

const canon = (t: string) => t.toLowerCase();

export function oscProblems(input: OscSettings): OscProblems {
  const s = normalizeOsc(input);
  const listen4 = listenFamily(s.listen) === 4;
  const seen = new Map<string, number>();
  const dupOf = s.targets.map((t, i) => {
    const k = canon(t);
    const first = seen.get(k);
    if (first === undefined) seen.set(k, i);
    return first ?? null;
  });
  return {
    listen: listenProblem(s.listen),
    targets: s.targets.map((t) => targetProblem(t, listen4)),
    dupOf,
    list: s.targets.length > OSC_MAX_TARGETS ? 'tooMany' : null,
  };
}

/** True when the form can be saved. */
export function oscValid(s: OscSettings): boolean {
  const p = oscProblems(s);
  return !p.listen && !p.list && p.targets.every((e) => !e) && p.dupOf.every((d) => d === null);
}

export const sameOsc = (a: OscSettings, b: OscSettings): boolean => JSON.stringify(normalizeOsc(a)) === JSON.stringify(normalizeOsc(b));
