import { describe, expect, it } from 'vitest';
import manifestRaw from '../../public/manifest.json?raw';
import indexHtml from '../../index.html?raw';
import { needsLivenessCheck, shouldDropSilentSocket, parseLaunchFlags, resolveSimple, shouldReconnectNow, WS_CLOSED, WS_CLOSING, WS_CONNECTING, WS_OPEN } from './launch';

describe('parseLaunchFlags', () => {
  it('reads bare and valued flags', () => {
    expect(parseLaunchFlags('')).toEqual({ simple: false, embed: false });
    expect(parseLaunchFlags('?simple')).toEqual({ simple: true, embed: false });
    expect(parseLaunchFlags('?embed&simple=1')).toEqual({ simple: true, embed: true });
    expect(parseLaunchFlags('?embed=true')).toEqual({ simple: false, embed: true });
  });
  it('treats 0 and false as off', () => {
    expect(parseLaunchFlags('?simple=0&embed=false')).toEqual({ simple: false, embed: false });
  });
});

describe('resolveSimple precedence', () => {
  const f = (simple: boolean) => ({ simple, embed: false });
  it('?simple beats the stored choice and the screen', () => {
    expect(resolveSimple(f(true), 'advanced', false)).toBe(true);
  });
  it('stored choice beats the screen size', () => {
    expect(resolveSimple(f(false), 'advanced', true)).toBe(false);
    expect(resolveSimple(f(false), 'simple', false)).toBe(true);
  });
  it('falls back to the screen size when nothing is stored or it is garbage', () => {
    expect(resolveSimple(f(false), null, true)).toBe(true);
    expect(resolveSimple(f(false), 'bogus', false)).toBe(false);
  });
  it('?embed alone means Advanced, whatever is stored or the screen size', () => {
    expect(resolveSimple({ simple: false, embed: true }, 'simple', true)).toBe(false);
    expect(resolveSimple({ simple: false, embed: true }, null, true)).toBe(false);
  });
  it('?embed&simple still means Simple', () => {
    expect(resolveSimple({ simple: true, embed: true }, 'advanced', false)).toBe(true);
  });
});

describe('shouldReconnectNow', () => {
  it('reconnects when there is no socket or it is closed', () => {
    expect(shouldReconnectNow(false, null)).toBe(true);
    expect(shouldReconnectNow(false, WS_CLOSED)).toBe(true);
  });
  it('never when open, connecting, closing or stopped (no storm)', () => {
    expect(shouldReconnectNow(false, WS_OPEN)).toBe(false);
    expect(shouldReconnectNow(false, WS_CONNECTING)).toBe(false);
    expect(shouldReconnectNow(false, WS_CLOSING)).toBe(false);
    expect(shouldReconnectNow(true, null)).toBe(false);
  });
});

describe('liveness of an OPEN socket after unlock', () => {
  it('only checks OPEN sockets of online/demo sessions', () => {
    expect(needsLivenessCheck(WS_OPEN, 'online')).toBe(true);
    expect(needsLivenessCheck(WS_OPEN, 'demo')).toBe(true);
    expect(needsLivenessCheck(WS_OPEN, 'offline')).toBe(false);
    expect(needsLivenessCheck(WS_OPEN, 'connecting')).toBe(false);
    expect(needsLivenessCheck(WS_CONNECTING, 'online')).toBe(false);
    expect(needsLivenessCheck(null, 'online')).toBe(false);
  });
  it('drops the socket only when nothing arrived since becoming visible', () => {
    expect(shouldDropSilentSocket(WS_OPEN, 'online', 900, 1000)).toBe(true);
    expect(shouldDropSilentSocket(WS_OPEN, 'online', 1500, 1000)).toBe(false);
    expect(shouldDropSilentSocket(WS_OPEN, 'offline', 900, 1000)).toBe(false);
    expect(shouldDropSilentSocket(WS_CLOSED, 'online', 900, 1000)).toBe(false);
  });
});

describe('index.html', () => {
  it('has no root-absolute href/src (proxy subpath safe)', () => {
    const urls = [...indexHtml.matchAll(/\b(?:href|src)="([^"]*)"/g)].map((x) => x[1]);
    expect(urls.length).toBeGreaterThan(0);
    for (const u of urls) expect(u).not.toMatch(/^\//);
  });
});

describe('web app manifest', () => {
  const m = JSON.parse(manifestRaw)
  const pngs = Object.keys(import.meta.glob('../../public/*.png')).map((p) => p.split('/').pop());
  const resolve = (rel: string) => new URL(rel, 'https://host.example/sub/').pathname;
  it('has a relative id and the top bar colour as theme', () => {
    expect(m.id).toBe('./');
    expect(m.theme_color).toBe('#16181d');
    expect(m.background_color).toBe('#121214');
  });
  it('icons have the expected pixel size (PNG IHDR)', async () => {
    // Specifier built at runtime: no @types/node here, and vite must not bundle it.
    const fs = (await import(/* @vite-ignore */ ['node', 'fs'].join(':'))) as { readFileSync(u: URL): Uint8Array };
    for (const name of pngs as string[]) {
      const b = fs.readFileSync(new URL(`../../public/${name}`, import.meta.url));
      const dv = new DataView(b.buffer, b.byteOffset);
      const want = name === 'apple-touch-icon.png' ? 180 : name.includes('192') ? 192 : 512;
      expect([name, dv.getUint32(16), dv.getUint32(20)]).toEqual([name, want, want]);
    }
  });
  it('is a standalone app named Cue Desk', () => {
    expect(m.name).toBe('Cue Desk');
    expect(m.short_name).toBe('Cue Desk');
    expect(m.display).toBe('standalone');
    expect(m.background_color).toBe('#121214');
  });
  it('uses relative URLs and a start_url inside the scope', () => {
    for (const u of [m.start_url, m.scope, ...m.icons.map((i: { src: string }) => i.src)]) {
      expect(u).not.toMatch(/^(\/|[a-z]+:)/i);
    }
    expect(resolve(m.start_url).startsWith(resolve(m.scope))).toBe(true);
    expect(parseLaunchFlags(new URL(m.start_url, 'https://host.example/sub/').search).simple).toBe(true);
  });
  it('lists 192 and 512 icons that exist', () => {
    const sizes = m.icons.map((i: { sizes: string }) => i.sizes);
    expect(sizes).toEqual(expect.arrayContaining(['192x192', '512x512']));
    for (const i of m.icons) expect(pngs).toContain(i.src);
    expect(pngs).toContain('apple-touch-icon.png');
  });
});
