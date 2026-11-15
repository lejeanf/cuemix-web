import { describe, expect, it } from 'vitest';
import { healthUrl, wsUrl } from './store.svelte';

describe('relative URLs (reverse-proxy subpaths)', () => {
  it('health URL follows the page path like the WebSocket URL', () => {
    expect(healthUrl({ pathname: '/' })).toBe('/api/health');
    expect(healthUrl({ pathname: '/mixer/' })).toBe('/mixer/api/health');
    expect(healthUrl({ pathname: '/mixer/index.html' })).toBe('/mixer/api/health');
    expect(wsUrl({ protocol: 'https:', host: 'example.com', pathname: '/mixer/' })).toBe(
      'wss://example.com/mixer/ws',
    );
  });
});
