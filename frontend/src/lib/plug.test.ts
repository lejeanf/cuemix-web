import { describe, expect, it } from 'vitest';
import vectors from '../../../design/vectors/speaker-plug-urls.json';
import type { SpeakerPlugSettings } from './protocol';
import { buildUrls, defaultPlugSettings, errorSince, isValidHttpUrl, isValidHost, normalizeSettings, sameSettings, settingsProblem, urlProblem } from './plug';

type Vec = { kind: SpeakerPlugSettings['kind']; host: string; switch_id: number; on_url?: string; off_url?: string };
const fields = (v: Vec) => ({ kind: v.kind, host: v.host, switch_id: v.switch_id, on_url: v.on_url ?? '', off_url: v.off_url ?? '' });

describe('speaker plug URL vectors', () => {
  for (const v of vectors.valid) {
    it(`builds ${v.kind} ${v.host || v.on}`, () => {
      expect(buildUrls(fields(v as Vec))).toEqual({ on: v.on, off: v.off });
      expect(urlProblem(fields(v as Vec))).toBeNull();
    });
  }
  for (const v of vectors.invalid) {
    it(`rejects: ${v.why}`, () => {
      expect(buildUrls(fields(v as Vec))).toBeNull();
      expect(urlProblem(fields(v as Vec))).not.toBeNull();
    });
  }
});

describe('speaker plug host and form validation', () => {
  it('hosts', () => {
    for (const ok of ['plug.example', '192.0.2.10', 'plug-1.example:80', '[2001:db8::1]:8080']) expect(isValidHost(ok)).toBe(true);
    for (const bad of ['', ' ', 'a b', 'a/b', 'http://x', 'u@h', 'h:99999', '-x', 'x:', '[zz]', 'x:0', '[zz']) expect(isValidHost(bad)).toBe(false);
  });
  it('defaults match the contract and are valid when enabled with a host', () => {
    const d = defaultPlugSettings();
    expect(d).toMatchObject({ enabled: false, kind: 'shelly_rpc', warmup_ms: 2000, off_delay_ms: 300, timeout_ms: 2000 });
    expect(settingsProblem(d)).toBeNull();
    expect(settingsProblem({ ...d, enabled: true })).toBe('host');
    expect(settingsProblem({ ...d, enabled: true, host: 'plug.example' })).toBeNull();
  });
  it('ranges', () => {
    const ok = { ...defaultPlugSettings(), enabled: true, host: 'plug.example' };
    expect(settingsProblem({ ...ok, warmup_ms: 10_000 })).toBeNull();
    expect(settingsProblem({ ...ok, warmup_ms: 10_500 })).toBe('warmup');
    expect(settingsProblem({ ...ok, warmup_ms: Number.NaN })).toBe('warmup');
    expect(settingsProblem({ ...ok, off_delay_ms: 5001 })).toBe('offDelay');
    expect(settingsProblem({ ...ok, timeout_ms: 499 })).toBe('timeout');
    expect(settingsProblem({ ...ok, switch_id: 16 })).toBe('switchId');
    expect(settingsProblem({ ...ok, switch_id: 0.5 })).toBe('switchId');
  });
  it('custom needs http:// URLs, other kinds ignore them', () => {
    const c = { ...defaultPlugSettings(), enabled: true, kind: 'custom' as const };
    expect(settingsProblem(c)).toBe('url');
    expect(settingsProblem({ ...c, on_url: 'http://plug.example/on', off_url: 'http://plug.example/off' })).toBeNull();
    expect(settingsProblem({ ...c, on_url: 'https://plug.example/on', off_url: 'http://plug.example/off' })).toBe('url');
  });
  it('normalize blanks the fields a kind does not use', () => {
    const s = { ...defaultPlugSettings(), host: ' plug.example ', on_url: 'http://x/on', off_url: 'http://x/off' };
    expect(normalizeSettings(s)).toMatchObject({ host: 'plug.example', on_url: '', off_url: '' });
    const c = normalizeSettings({ ...s, kind: 'custom' });
    expect(c).toMatchObject({ host: '', on_url: 'http://x/on' });
    expect(sameSettings(s, { ...s, on_url: '' })).toBe(true);
    expect(sameSettings(s, { ...s, switch_id: 1 })).toBe(false);
  });
  it('an empty switch id is a problem; a hidden one of a custom plug is not', () => {
    const ok = { ...defaultPlugSettings(), enabled: true, host: 'plug.example' };
    expect(settingsProblem({ ...ok, switch_id: null as unknown as number })).toBe('switchId');
    expect(settingsProblem({ ...ok, enabled: false, switch_id: 99 })).toBe('switchId');
    const c = { ...ok, kind: 'custom' as const, on_url: 'http://plug.local/on', off_url: 'http://plug.local/off', switch_id: null as unknown as number };
    expect(settingsProblem(c)).toBeNull();
    expect(normalizeSettings(c).switch_id).toBe(0);
  });
  it('the host is validated trimmed', () => {
    expect(settingsProblem({ ...defaultPlugSettings(), enabled: true, host: ' plug.local ' })).toBeNull();
  });
  it('custom URLs follow the server parser', () => {
    for (const ok of ['http://plug.local', 'http://plug.local?on=1', 'http://[2001:db8::1]:81/x', 'http://plug.local:8080/a/b?x=1']) expect(isValidHttpUrl(ok)).toBe(true);
    for (const bad of ['https://plug.local/', 'http://u:p@plug.local/', 'http://plug.local:0/', 'http://plug.local/#x', 'http://[zz/on', 'http:///x', 'plug.local', 'http://plug.local/a b']) expect(isValidHttpUrl(bad)).toBe(false);
  });
  it('only errors after the noted sequence count as a reply', () => {
    expect(errorSince(null, 0)).toBeNull();
    expect(errorSince({ seq: 3, message: 'old preset error' }, 3)).toBeNull();
    expect(errorSince({ seq: 4, message: 'bad host' }, 3)).toBe('bad host');
  });
});

describe('speaker plug hostile input', () => {
  const bs = String.fromCharCode(92);
  it('refuses framing characters, non-ASCII and over-long text in custom URLs', () => {
    const bad = [
      'http://192.0.2.1/on\r\nX-Evil: 1',
      'http://192.0.2.1/on\nGET /x',
      'http://192.0.2.1/on HTTP/1.1',
      'http://192.0.2.1/on\u0085x',
      'http://192.0.2.1/on x',
      'http://192.0.2.1/on‮x',
      'http://192.0.2.1/on／x',
      'http://192.0.2.1/café',
      'http://192.0.2.1/on@evil.example',
      `http://192.0.2.1/on${bs}x`,
      'http://192.0.2.1/on%zz',
      'http://192.0.2.1/on%0',
      'http://192.0.2.1/on"x',
      `http://192.0.2.1/${'a'.repeat(2100)}`,
      'http://192.0.2.1:+80/',
      'http://-x.example/',
      'http://a..example/',
      `http://${'a'.repeat(64)}.example/`,
    ];
    for (const u of bad) expect(isValidHttpUrl(u), JSON.stringify(u)).toBe(false);
    expect(isValidHttpUrl('http://192.0.2.1/cm?cmnd=Power%0D%0AOn')).toBe(true);
  });
  it('refuses odd hosts', () => {
    for (const h of ['a\nb', 'a\0', 'plug．example', 'plug.example‮', 'a..b', 'a.-b', 'a'.repeat(254)]) expect(isValidHost(h), JSON.stringify(h)).toBe(false);
  });
});
