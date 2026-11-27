// Speaker plug (docs/IOS-PLAN.md 5.2b): settings defaults, URL builder and validator.
// The server builds and checks the URLs too; this copy drives the live preview and
// the Save button. Both are tested against design/vectors/speaker-plug-urls.json.
import type { PlugKind, SpeakerPlugSettings } from './protocol';

export const PLUG_KINDS: readonly PlugKind[] = ['shelly_rpc', 'shelly_gen1', 'tasmota', 'custom'];
export const SWITCH_ID_MAX = 15;
export const WARMUP_MAX_MS = 10_000;
export const OFF_DELAY_MAX_MS = 5000;
export const TIMEOUT_MIN_MS = 500;
export const TIMEOUT_MAX_MS = 10_000;

export function defaultPlugSettings(): SpeakerPlugSettings {
  return {
    enabled: false,
    kind: 'shelly_rpc',
    host: '',
    switch_id: 0,
    on_url: '',
    off_url: '',
    warmup_ms: 2000,
    off_delay_ms: 300,
    timeout_ms: 2000,
  };
}

/** Why a form value is rejected; the message text lives in strings.ts (S.plug.errors). */
export type PlugError = 'host' | 'switchId' | 'url' | 'warmup' | 'offDelay' | 'timeout';

// `host`, `host:port`, `a.b.c.d[:port]` or `[v6][:port]`: no scheme, path, userinfo or spaces.
const HOST_RE = /^(?:\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?)(?::\d{1,5})?$/;

export const MAX_HOST_LEN = 253;
export const MAX_URL_LEN = 2048;

export function isValidHost(host: string): boolean {
  if (!HOST_RE.test(host)) return false;
  const rest = host.startsWith('[') ? host.slice(host.indexOf(']') + 1) : host;
  const port = /:(\d{1,5})$/.exec(rest);
  if (port && !(Number(port[1]) >= 1 && Number(port[1]) <= 65535)) return false;
  if (host.startsWith('[')) return true;
  // Host name: labels of 1-63 characters that neither start nor end with '-', 253 at most.
  const name = (port ? host.slice(0, port.index) : host).replace(/\.$/, '');
  return name.length <= MAX_HOST_LEN && name.split('.').every((l) => l.length >= 1 && l.length <= 63 && !l.startsWith('-') && !l.endsWith('-'));
}

// Path and query as the server allows them: unreserved characters, / ? & = : , + ; and %XX.
const PATH_RE = /^(?:[A-Za-z0-9._~/?&=:,+;-]|%[0-9A-Fa-f]{2})*$/;

/** `http://host[:port][/path][?query]` the way the server parses it: no login, no fragment, no spaces. */
export function isValidHttpUrl(url: string): boolean {
  if (!url.startsWith('http://') || url.length > MAX_URL_LEN) return false;
  const rest = url.slice('http://'.length);
  if (/[\s\p{Cc}#]/u.test(rest) || !/^[\x00-\x7f]*$/.test(rest)) return false;
  const end = rest.search(/[/?]/);
  const authority = end < 0 ? rest : rest.slice(0, end);
  const tail = end < 0 ? '' : rest.slice(end);
  return !authority.includes('@') && isValidHost(authority) && PATH_RE.test(tail);
}

/** Errors that arrived after `since` (an error seq noted when a command was sent), or null. */
export function errorSince(last: { seq: number; message: string } | null, since: number): string | null {
  return last && last.seq > since ? last.message : null;
}

const isInt = (n: unknown, lo: number, hi: number): boolean => typeof n === 'number' && Number.isInteger(n) && n >= lo && n <= hi;

type UrlFields = Pick<SpeakerPlugSettings, 'kind' | 'host' | 'switch_id' | 'on_url' | 'off_url'>;

/** First problem with the fields the URL preview depends on, or null. */
export function urlProblem(s: UrlFields): PlugError | null {
  if (s.kind === 'custom') return isValidHttpUrl(s.on_url) && isValidHttpUrl(s.off_url) ? null : 'url';
  if (!isInt(s.switch_id, 0, SWITCH_ID_MAX)) return 'switchId';
  return isValidHost(s.host) ? null : 'host';
}

/** The On and Off URLs the server will call, or null when the settings do not make valid ones. */
export function buildUrls(s: UrlFields): { on: string; off: string } | null {
  if (urlProblem(s)) return null;
  const h = s.host;
  const n = s.switch_id;
  switch (s.kind) {
    case 'shelly_rpc':
      return { on: `http://${h}/rpc/Switch.Set?id=${n}&on=true`, off: `http://${h}/rpc/Switch.Set?id=${n}&on=false` };
    case 'shelly_gen1':
      return { on: `http://${h}/relay/${n}?turn=on`, off: `http://${h}/relay/${n}?turn=off` };
    case 'tasmota': {
      const power = n > 0 ? `Power${n + 1}` : 'Power';
      return { on: `http://${h}/cm?cmnd=${power}%20On`, off: `http://${h}/cm?cmnd=${power}%20Off` };
    }
    case 'custom':
      return { on: s.on_url, off: s.off_url };
    default:
      return null;
  }
}

/** First problem with the whole form (what Save needs), or null. */
export function settingsProblem(input: SpeakerPlugSettings): PlugError | null {
  const s = normalizeSettings(input);
  // Ranges are always checked (the server does too); a hidden switch id of a custom plug is 0.
  if (!isInt(s.switch_id, 0, SWITCH_ID_MAX)) return 'switchId';
  if (!isInt(s.warmup_ms, 0, WARMUP_MAX_MS)) return 'warmup';
  if (!isInt(s.off_delay_ms, 0, OFF_DELAY_MAX_MS)) return 'offDelay';
  if (!isInt(s.timeout_ms, TIMEOUT_MIN_MS, TIMEOUT_MAX_MS)) return 'timeout';
  // A disabled plug may be saved half-filled; an enabled one needs working URLs.
  return s.enabled ? urlProblem(s) : null;
}

/** Settings as sent: fields that do not apply to the kind are blanked, like the server stores them. */
export function normalizeSettings(s: SpeakerPlugSettings): SpeakerPlugSettings {
  const custom = s.kind === 'custom';
  return {
    ...s,
    switch_id: custom ? 0 : s.switch_id,
    host: custom ? '' : s.host.trim(),
    on_url: custom ? s.on_url.trim() : '',
    off_url: custom ? s.off_url.trim() : '',
  };
}

export const sameSettings = (a: SpeakerPlugSettings, b: SpeakerPlugSettings): boolean =>
  JSON.stringify(normalizeSettings(a)) === JSON.stringify(normalizeSettings(b));
