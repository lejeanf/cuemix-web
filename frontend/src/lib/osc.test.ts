import { afterEach, describe, expect, it, vi } from 'vitest';
import { listenFamily, listenProblem, normalizeOsc, oscProblems, oscValid, parseIpv4, parseIpv6, sameOsc, splitHostPort, targetProblem, validHostname } from './osc';
import { applyEvent, initialClientState } from './reducer';
import { store } from './store.svelte';

describe('OSC address parsing (mirrors src/osc/settings.rs)', () => {
  it('IPv4', () => {
    expect(parseIpv4('0.0.0.0')).toEqual([0, 0, 0, 0]);
    for (const bad of ['1.2.3', '1.2.3.256', '01.2.3.4', '1.2.3.4.5', 'a.b.c.d', '']) expect(parseIpv4(bad)).toBeNull();
  });
  it('IPv6', () => {
    expect(parseIpv6('::')).toEqual([0, 0, 0, 0, 0, 0, 0, 0]);
    expect(parseIpv6('::1')).toEqual([0, 0, 0, 0, 0, 0, 0, 1]);
    expect(parseIpv6('FE80::0001')?.[0]).toBe(0xfe80);
    expect(parseIpv6('::ffff:1.2.3.4')).toEqual([0, 0, 0, 0, 0, 0xffff, 0x0102, 0x0304]);
    expect(parseIpv6('1:2:3:4:5:6:7:8')).toHaveLength(8);
    for (const bad of ['', ':', '1::2::3', '1:2:3:4:5:6:7', '1:2:3:4:5:6:7:8:9', 'zz::1', '12345::1', '1.2.3.4']) expect(parseIpv6(bad)).toBeNull();
  });
  it('host names use RFC label rules, underscore allowed like the server', () => {
    for (const ok of ['tablet.local', 'a', 'my_tablet.lan', 'a-b.c', '1.2.3.4']) expect(validHostname(ok)).toBe(true);
    for (const bad of ['', '-a', 'a-', 'a..b', '.a', 'a.', 'a b', 'a'.repeat(64), `${'a.'.repeat(130)}a`]) expect(validHostname(bad)).toBe(false);
  });
  it('splits host and port', () => {
    expect(splitHostPort('tablet.local:9000')).toEqual({ host: 'tablet.local', port: '9000', bracketed: false });
    expect(splitHostPort('[::1]:8000')).toEqual({ host: '::1', port: '8000', bracketed: true });
    expect(splitHostPort('::1:8000')).toBeNull();
    expect(splitHostPort('nohost')).toBeNull();
  });
});

describe('OSC listen validation', () => {
  it('needs an IP literal with a port of 1024..65535', () => {
    for (const ok of ['0.0.0.0:8000', '127.0.0.1:1024', '[::]:65535', '[::1]:8000']) expect(listenProblem(ok)).toBeNull();
    expect(listenProblem('')).toBe('listenEmpty');
    expect(listenProblem('  ')).toBe('listenEmpty');
    expect(listenProblem('tablet.local:8000')).toBe('listenFormat');
    expect(listenProblem('0.0.0.0')).toBe('listenFormat');
    expect(listenProblem('::1:8000')).toBe('listenFormat');
    expect(listenProblem('0.0.0.0:99999')).toBe('listenFormat');
    expect(listenProblem('0.0.0.0:1023')).toBe('listenPort');
    expect(listenProblem('0.0.0.0:0')).toBe('listenPort');
  });
  it('refuses multicast and broadcast', () => {
    for (const bad of ['224.0.0.1:8000', '239.255.255.255:8000', '255.255.255.255:8000', '[ff02::1]:8000']) expect(listenProblem(bad)).toBe('listenAddr');
    expect(listenProblem('223.255.255.255:8000')).toBeNull();
    expect(listenProblem('240.0.0.1:8000')).toBeNull();
  });
  it('knows the family', () => {
    expect(listenFamily('0.0.0.0:8000')).toBe(4);
    expect(listenFamily('[::]:8000')).toBe(6);
    expect(listenFamily('nope')).toBeNull();
  });
});

describe('OSC target validation', () => {
  it('accepts names, IPv4 and bracketed IPv6 with port 1..65535', () => {
    for (const ok of ['tablet.local:9000', 'my_tablet:1', '[fe80::1]:9000', '[FE80::0001]:9000', 'tablet.local:65535']) expect(targetProblem(ok)).toBeNull();
  });
  it('rejects bad shapes and ports', () => {
    expect(targetProblem('tablet.local')).toBe('format');
    expect(targetProblem('fe80::1:9000')).toBe('format');
    expect(targetProblem('http://x:1')).toBe('format');
    expect(targetProblem('-a.local:1')).toBe('format');
    expect(targetProblem('[zz]:1')).toBe('format');
    expect(targetProblem('tablet.local:0')).toBe('targetPort');
    expect(targetProblem('tablet.local:65536')).toBe('targetPort');
    expect(targetProblem('tablet.local:x')).toBe('format');
  });
  it('flags an IPv6 target when the listener is IPv4', () => {
    expect(targetProblem('[fe80::1]:9000', true)).toBe('v6target');
    expect(targetProblem('tablet.local:9000', true)).toBeNull();
    expect(oscProblems({ listen: '0.0.0.0:8000', targets: ['[fe80::1]:9000'] }).targets).toEqual(['v6target']);
    expect(oscProblems({ listen: '[::]:8000', targets: ['[fe80::1]:9000'] }).targets).toEqual([null]);
  });
});

describe('OSC form', () => {
  it('normalizes: trims and drops empty rows', () => {
    expect(normalizeOsc({ listen: ' 0.0.0.0:8000 ', targets: [' tablet.local:9000', '', '  '] })).toEqual({ listen: '0.0.0.0:8000', targets: ['tablet.local:9000'] });
  });
  it('limits to 8 targets', () => {
    const t = (n: number) => Array.from({ length: n }, (_, i) => `t${i}.local:9000`);
    expect(oscValid({ listen: '0.0.0.0:8000', targets: t(8) })).toBe(true);
    expect(oscProblems({ listen: '0.0.0.0:8000', targets: t(9) }).list).toBe('tooMany');
    expect(oscValid({ listen: '0.0.0.0:8000', targets: [] })).toBe(true);
  });
  it('names the earlier row of a duplicate', () => {
    const p = oscProblems({ listen: '0.0.0.0:8000', targets: ['a.local:1', 'b.local:1', 'A.local:1'] });
    expect(p.dupOf).toEqual([null, null, 0]);
    expect(oscValid({ listen: '0.0.0.0:8000', targets: ['a.local:1', 'A.local:1'] })).toBe(false);
  });
  it('compares settings ignoring blanks', () => {
    expect(sameOsc({ listen: '0.0.0.0:8025', targets: ['b:1', ''] }, { listen: '0.0.0.0:8025 ', targets: ['b:1'] })).toBe(true);
    expect(sameOsc({ listen: '0.0.0.0:8025', targets: [] }, { listen: '0.0.0.0:8026', targets: [] })).toBe(false);
  });
});

describe('osc_settings event', () => {
  it('reducer stores settings, listening and error without aliasing the payload', () => {
    const s = initialClientState();
    expect(s.osc).toBeNull();
    const settings = { listen: '0.0.0.0:8000', targets: ['tablet.local:9000'] };
    applyEvent(s, { event: 'osc_settings', settings, listening: true, error: null });
    expect(s.osc).toEqual({ settings, listening: true, error: null });
    settings.targets.push('x:1');
    expect(s.osc?.settings.targets).toEqual(['tablet.local:9000']);
    applyEvent(s, { event: 'osc_settings', settings: { listen: '0.0.0.0:8001', targets: [] }, listening: false, error: 'address in use' });
    expect(s.osc).toEqual({ settings: { listen: '0.0.0.0:8001', targets: [] }, listening: false, error: 'address in use' });
  });
});

describe('set_osc_settings command', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    (store as unknown as { ws: unknown }).ws = null;
  });
  it('sends {cmd, settings} on an open socket and reports false on a closed one', () => {
    vi.stubGlobal('WebSocket', { OPEN: 1 });
    const send = vi.fn();
    const settings = normalizeOsc({ listen: '0.0.0.0:8000', targets: ['tablet.local:9000', ''] });
    expect(store.setOscSettings(settings)).toBe(false);
    (store as unknown as { ws: unknown }).ws = { readyState: 1, send };
    expect(store.setOscSettings(settings)).toBe(true);
    expect(JSON.parse(send.mock.calls[0][0])).toEqual({ cmd: 'set_osc_settings', settings: { listen: '0.0.0.0:8000', targets: ['tablet.local:9000'] } });
    (store as unknown as { ws: unknown }).ws = { readyState: 3, send };
    expect(store.setOscSettings(settings)).toBe(false);
    expect(send).toHaveBeenCalledTimes(1);
  });
});
