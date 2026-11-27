import { describe, expect, it } from 'vitest';
import { locked, pairRequest, pairToken, parseAdmin } from './admin';

describe('admin pairing', () => {
  const tok = 'ab'.repeat(32);

  it('reads only a well-formed pairing fragment', () => {
    expect(pairToken(`#pair=${tok}`)).toBe(tok);
    for (const bad of ['', '#pair=', `#pair=${tok}x`, `#pair=${tok.toUpperCase()}`, `#pair=${'ab'.repeat(31)}`, `#x=${tok}`, `#pair=${tok}&a=1`]) {
      expect(pairToken(bad)).toBeNull();
    }
  });

  it('locks admin controls only when the server requires a token this device lacks', () => {
    expect(locked(null)).toBe(false);
    expect(locked({ required: false, paired: false })).toBe(false);
    expect(locked({ required: true, paired: true })).toBe(false);
    expect(locked({ required: true, paired: false })).toBe(true);
  });

  it('checks the shape of the status answer', () => {
    expect(parseAdmin({ required: true, paired: false })).toEqual({ required: true, paired: false });
    for (const bad of [null, 1, 'x', {}, { required: 'yes', paired: false }, { required: true }]) {
      expect(parseAdmin(bad)).toBeNull();
    }
  });

  it('posts the token as JSON to a path relative to the page', () => {
    const { url, init } = pairRequest(tok, { pathname: '/mixer/' });
    expect(url).toBe('/mixer/api/admin/pair');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ token: tok });
    expect((init.headers as Record<string, string>)['content-type']).toBe('application/json');
  });
});
