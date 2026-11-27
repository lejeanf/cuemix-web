// Software update / version selection (contract 0.7.0 B): types, release filtering and
// request shapes for GET/POST /api/update and PUT /api/update/prefs. Pure; the fetches
// live in updates.svelte.ts.
import { compareVersions } from './semver';

export interface Release {
  tag: string;
  version: string;
  prerelease: boolean;
  /** Null when GitHub gave no date. */
  published_at: string | null;
}

export interface UpdateInfo {
  installed: string;
  channel: string;
  enabled: boolean;
  /** An install request is waiting for the root side (goes false within about a second of its start). */
  pending: boolean;
  /** Tag the pending request asked for, null for "newest". */
  pending_version?: string | null;
  /** Raw status file of the root checker; fields may be missing. */
  status: { latest?: string; tag?: string; update_available?: boolean; checked_at?: string; release_url?: string } | null;
  last_result: { code: number; outcome: string; at: string; version?: string | null } | null;
  /** Newer than installed, following `show_prerelease` (use this, not the raw status). */
  update_available?: boolean;
  latest?: { tag: string; version: string; prerelease: boolean } | null;
  latest_stable?: string | null;
  latest_pre?: string | null;
  /** Absent or false on a root side without per-version install. */
  version_pinning?: boolean;
  releases?: Release[];
  show_prerelease?: boolean;
}

/** Version the classic one-click install would install, or null when nothing newer exists. */
export function classicLatest(info: UpdateInfo | null): string | null {
  return info?.enabled && info.update_available && info.latest ? info.latest.version : null;
}

/** Last install result when it was not a success. */
export function failedResult(info: UpdateInfo | null): NonNullable<UpdateInfo['last_result']> | null {
  const r = info?.last_result;
  return r && r.outcome !== 'ok' ? r : null;
}

export const TAG_RE = /^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$/;
export const UPDATE_HEADER = { 'x-cuemix-update': '1' } as const;

/** True when the server can install a chosen version (needs the host update). */
export function canPin(info: UpdateInfo | null): boolean {
  return !!info && info.enabled && info.version_pinning !== false && Array.isArray(info.releases);
}

/** `api/...` relative to the page, so it works behind a proxy subpath. */
export function apiUrl(path: string, loc: Pick<Location, 'pathname'> = location): string {
  const base = loc.pathname.endsWith('/') ? loc.pathname : loc.pathname.replace(/[^/]*$/, '');
  return `${base}${path}`;
}

/** Releases to list: newest first, pre-releases only when asked for (the installed one always stays). */
export function visibleReleases(releases: readonly Release[], showPre: boolean, installed: string): Release[] {
  return releases
    .filter((r) => TAG_RE.test(r.tag) && (showPre || !r.prerelease || compareVersions(r.version, installed) === 0))
    .sort((a, b) => compareVersions(b.version, a.version) ?? 0);
}

/** Tag of the newest release in the (already filtered) list, or null. */
export function latestTag(list: readonly Release[]): string | null {
  return list.reduce<Release | null>((m, r) => (!m || (compareVersions(r.version, m.version) ?? 0) > 0 ? r : m), null)?.tag ?? null;
}

export type Rel = 'installed' | 'older' | 'newer';

/** How a release relates to the running version ('newer' when unknown). */
export function relation(r: Release, installed: string): Rel {
  const c = compareVersions(r.version, installed);
  return c === 0 ? 'installed' : c !== null && c < 0 ? 'older' : 'newer';
}

/** Request for installing `version` (a tag), or the newest of the channel when undefined. */
export function installRequest(version?: string, loc?: Pick<Location, 'pathname'>): { url: string; init: RequestInit } {
  return {
    url: apiUrl('api/update', loc),
    init: version
      ? { method: 'POST', headers: { ...UPDATE_HEADER, 'content-type': 'application/json' }, body: JSON.stringify({ version }) }
      : { method: 'POST', headers: { ...UPDATE_HEADER } },
  };
}

export function prefsRequest(showPre: boolean, loc?: Pick<Location, 'pathname'>): { url: string; init: RequestInit } {
  return {
    url: apiUrl('api/update/prefs', loc),
    init: { method: 'PUT', headers: { ...UPDATE_HEADER, 'content-type': 'application/json' }, body: JSON.stringify({ show_prerelease: showPre }) },
  };
}
