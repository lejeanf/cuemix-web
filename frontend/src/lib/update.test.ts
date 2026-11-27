import { describe, expect, it } from 'vitest';
import { compareVersions, parseSemver } from './semver';
import { canPin, classicLatest, failedResult, installRequest, latestTag, prefsRequest, relation, visibleReleases, type Release, type UpdateInfo } from './update';
import { DEV_BUILD, RELOAD_KEY, RELOAD_MEMORY, claimReload, handleStale, shouldReload } from './reload';
import { installDone } from './updates.svelte';

describe('semver', () => {
  it('parses tags with or without v', () => {
    expect(parseSemver('v0.7.0')).toEqual({ major: 0, minor: 7, patch: 0, pre: [] });
    expect(parseSemver('1.2.3-rc.1+build')).toEqual({ major: 1, minor: 2, patch: 3, pre: ['rc', '1'] });
    for (const bad of ['', 'v1', '1.2', 'latest', '1.2.3.4']) expect(parseSemver(bad)).toBeNull();
  });
  it('orders numerically, not lexically', () => {
    expect(compareVersions('0.10.0', '0.9.0')).toBeGreaterThan(0);
    expect(compareVersions('v0.6.0', '0.7.0')).toBeLessThan(0);
    expect(compareVersions('v0.7.0', '0.7.0')).toBe(0);
  });
  it('orders pre-releases below their release (semver 2 precedence)', () => {
    const asc = ['0.7.0-alpha', '0.7.0-alpha.1', '0.7.0-alpha.beta', '0.7.0-beta', '0.7.0-beta.2', '0.7.0-beta.11', '0.7.0-rc.1', '0.7.0'];
    for (let i = 0; i < asc.length - 1; i++) {
      expect(compareVersions(asc[i], asc[i + 1])).toBeLessThan(0);
      expect(compareVersions(asc[i + 1], asc[i])).toBeGreaterThan(0);
    }
    expect(compareVersions('0.6.9', '0.7.0-rc.1')).toBeLessThan(0);
  });
  it('is null for non-versions', () => {
    expect(compareVersions('x', '1.0.0')).toBeNull();
  });
});

const rel = (version: string, prerelease = false): Release => ({ tag: `v${version}`, version, prerelease, published_at: '2026-11-25T10:00:00Z' });
const releases = [rel('0.5.2'), rel('0.7.0-rc.1', true), rel('0.6.0'), rel('0.7.0'), rel('0.6.1-beta.1', true)];

describe('release list', () => {
  it('hides pre-releases unless asked, newest first', () => {
    expect(visibleReleases(releases, false, '0.6.0').map((r) => r.tag)).toEqual(['v0.7.0', 'v0.6.0', 'v0.5.2']);
    expect(visibleReleases(releases, true, '0.6.0').map((r) => r.tag)).toEqual(['v0.7.0', 'v0.7.0-rc.1', 'v0.6.1-beta.1', 'v0.6.0', 'v0.5.2']);
  });
  it('keeps the installed pre-release visible', () => {
    expect(visibleReleases(releases, false, '0.7.0-rc.1').map((r) => r.tag)).toContain('v0.7.0-rc.1');
  });
  it('drops entries whose tag the server would refuse', () => {
    expect(visibleReleases([{ ...rel('1.0.0'), tag: 'latest' }], true, '0.1.0')).toEqual([]);
  });
  it('marks the latest of the visible list', () => {
    expect(latestTag(visibleReleases(releases, false, '0.6.0'))).toBe('v0.7.0');
    expect(latestTag(visibleReleases([rel('0.7.0-rc.1', true), rel('0.6.0')], true, '0.6.0'))).toBe('v0.7.0-rc.1');
    expect(latestTag([])).toBeNull();
  });
  it('relates a release to the installed version', () => {
    expect(relation(rel('0.6.0'), '0.6.0')).toBe('installed');
    expect(relation(rel('0.5.2'), '0.6.0')).toBe('older');
    expect(relation(rel('0.7.0'), '0.6.0')).toBe('newer');
    expect(relation(rel('0.7.0-rc.1', true), '0.7.0')).toBe('older');
  });
});

describe('pinning support', () => {
  const base: UpdateInfo = { installed: '0.6.0', channel: 'stable', enabled: true, pending: false, status: null, last_result: null };
  it('needs releases and version_pinning not false', () => {
    expect(canPin({ ...base, releases: [], version_pinning: true })).toBe(true);
    expect(canPin({ ...base, releases: [] })).toBe(true);
    expect(canPin({ ...base, releases: [], version_pinning: false })).toBe(false);
    expect(canPin(base)).toBe(false);
    expect(canPin({ ...base, releases: [], enabled: false })).toBe(false);
    expect(canPin(null)).toBe(false);
  });
});

describe('request shapes', () => {
  it('install latest: POST /api/update, header, no body', () => {
    const { url, init } = installRequest(undefined, { pathname: '/' });
    expect(url).toBe('/api/update');
    expect(init).toEqual({ method: 'POST', headers: { 'x-cuemix-update': '1' } });
  });
  it('install a tag: JSON body {"version":tag}', () => {
    const { url, init } = installRequest('v0.5.2', { pathname: '/mixer/' });
    expect(url).toBe('/mixer/api/update');
    expect(init.method).toBe('POST');
    expect(init.headers).toEqual({ 'x-cuemix-update': '1', 'content-type': 'application/json' });
    expect(JSON.parse(init.body as string)).toEqual({ version: 'v0.5.2' });
  });
  it('prefs: PUT /api/update/prefs {"show_prerelease":bool}', () => {
    const { url, init } = prefsRequest(true, { pathname: '/index.html' });
    expect(url).toBe('/api/update/prefs');
    expect(init.method).toBe('PUT');
    expect(init.headers).toMatchObject({ 'x-cuemix-update': '1' });
    expect(JSON.parse(init.body as string)).toEqual({ show_prerelease: true });
  });
});

describe('classic install and last result', () => {
  const base: UpdateInfo = { installed: '0.6.0', channel: 'stable', enabled: true, pending: false, status: null, last_result: null };
  it('follows the top-level update_available and latest (not the raw status)', () => {
    const latest = { tag: 'v0.7.0', version: '0.7.0', prerelease: false };
    expect(classicLatest({ ...base, update_available: true, latest })).toBe('0.7.0');
    expect(classicLatest({ ...base, update_available: false, latest })).toBeNull();
    expect(classicLatest({ ...base, update_available: true, latest: null })).toBeNull();
    expect(classicLatest({ ...base, enabled: false, update_available: true, latest })).toBeNull();
    expect(classicLatest({ ...base, status: { latest: '9.9.9', update_available: true } })).toBeNull();
  });
  it('failedResult ignores ok', () => {
    expect(failedResult({ ...base, last_result: { code: 0, outcome: 'ok', at: 'x' } })).toBeNull();
    expect(failedResult({ ...base, last_result: { code: 3, outcome: 'rolled_back', at: 'x', version: 'v0.7.0' } })?.outcome).toBe('rolled_back');
  });
  it('published_at may be null', () => {
    const r: Release = { tag: 'v1.0.0', version: '1.0.0', prerelease: false, published_at: null };
    expect(visibleReleases([r], false, '0.1.0')).toEqual([r]);
  });
});

describe('install progress', () => {
  const info = (at: string | null): UpdateInfo => ({
    installed: '0.6.0',
    channel: 'stable',
    enabled: true,
    pending: false,
    status: null,
    last_result: at ? { code: 0, outcome: 'ok', at } : null,
  });
  it('waits while pending is already false and the result is unchanged', () => {
    const w = { baseAt: 'a', version: 'v0.5.2', sawDown: false };
    expect(installDone(w, info('a'), true)).toBe(false);
  });
  it('is done when last_result.at changes, also from no result at all', () => {
    expect(installDone({ baseAt: 'a', version: null, sawDown: false }, info('b'), true)).toBe(true);
    expect(installDone({ baseAt: null, version: null, sawDown: false }, info('b'), true)).toBe(true);
  });
  it('is done when the socket dropped and came back', () => {
    expect(installDone({ baseAt: 'a', version: null, sawDown: true }, info('a'), true)).toBe(true);
    expect(installDone({ baseAt: 'a', version: null, sawDown: true }, info('a'), false)).toBe(false);
  });
});

describe('stale tab reload guard', () => {
  it('reloads only when the server differs', () => {
    expect(shouldReload('0.6.0', '0.7.0', [])).toBe(true);
    expect(shouldReload('0.7.0', '0.7.0', [])).toBe(false);
    expect(shouldReload('0.7.0', 'v0.7.0', [])).toBe(false);
  });
  it('never reloads in dev, or with an unknown server version', () => {
    expect(shouldReload(DEV_BUILD, '0.7.0', [])).toBe(false);
    expect(shouldReload('0.6.0', null, [])).toBe(false);
  });
  it('at most once per server version', () => {
    expect(shouldReload('0.6.0', '0.7.0', ['0.7.0'])).toBe(false);
    expect(shouldReload('0.6.0', '0.8.0', ['0.7.0'])).toBe(true);
  });
  const mem = () => {
    const m = new Map<string, string>();
    return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), m };
  };
  it('claimReload records the guard so a second call does not reload', () => {
    const st = mem();
    expect(claimReload('0.6.0', '0.7.0', st)).toBe(true);
    expect(st.m.get(RELOAD_KEY)).toBe('0.7.0');
    expect(claimReload('0.6.0', '0.7.0', st)).toBe(false);
  });
  it('A -> B -> A cannot loop: earlier versions are remembered', () => {
    const st = mem();
    expect(claimReload('0.6.0', '0.7.0', st)).toBe(true);
    expect(claimReload('0.6.0', '0.8.0', st)).toBe(true);
    expect(claimReload('0.6.0', '0.7.0', st)).toBe(false);
    expect(st.m.get(RELOAD_KEY)).toBe('0.7.0,0.8.0');
  });
  it('keeps at most RELOAD_MEMORY versions', () => {
    const st = mem();
    for (let i = 1; i <= RELOAD_MEMORY + 2; i++) claimReload('0.0.1', `1.0.${i}`, st);
    expect(st.m.get(RELOAD_KEY)?.split(',')).toHaveLength(RELOAD_MEMORY);
  });
  it('storage failures never allow a reload', () => {
    const noWrite = {
      getItem: () => null,
      setItem: () => {
        throw new Error('quota');
      },
    };
    expect(claimReload('0.6.0', '0.7.0', noWrite)).toBe(false);
    const noRead = {
      getItem: (): string | null => {
        throw new Error('blocked');
      },
      setItem: () => {},
    };
    expect(claimReload('0.6.0', '0.7.0', noRead)).toBe(false);
    expect(claimReload('0.6.0', '0.7.0', null)).toBe(false);
    // a store that drops writes silently: the read-back catches it
    const dropping = { getItem: () => null, setItem: () => {} };
    expect(claimReload('0.6.0', '0.7.0', dropping)).toBe(false);
  });
  it('handleStale reloads once, defers while a panel is open, and is silent otherwise', () => {
    const st = mem();
    const calls = { reload: 0, notify: 0 };
    const run = (busy: boolean, build = '0.6.0') =>
      handleStale({ build, server: '0.7.0', storage: st, busy, reload: () => void calls.reload++, notify: () => void calls.notify++ });
    expect(run(true)).toBe('deferred');
    expect(calls).toEqual({ reload: 0, notify: 1 });
    expect(st.m.size).toBe(0); // nothing recorded while deferred
    expect(run(false)).toBe('reloaded');
    expect(calls.reload).toBe(1);
    expect(run(false)).toBe('none');
    expect(run(false, '0.7.0')).toBe('none');
    expect(calls.reload).toBe(1);
  });
});
