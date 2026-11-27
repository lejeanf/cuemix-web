import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RELOAD_KEY } from './reload';
import { store } from './store.svelte';

// refreshVersion wiring: /api/health version -> stale check -> reload guard, with fetch,
// location and sessionStorage stubbed.
describe('stale tab reload wiring', () => {
  let reload: ReturnType<typeof vi.fn>;
  let mem: Map<string, string>;
  const serve = (version: unknown) =>
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ version }) })));

  beforeEach(() => {
    reload = vi.fn();
    mem = new Map();
    vi.stubGlobal('location', { pathname: '/', reload });
    vi.stubGlobal('sessionStorage', { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v) });
    store.buildVersion = '0.6.0';
    store.version = null;
    store.reloadWaiting = false;
    store.advancedOpen = store.roomOpen = false;
    store.c.toasts.length = 0;
  });
  afterEach(() => vi.unstubAllGlobals());

  it('reads /api/health relative to the page and reloads once for a new server version', async () => {
    serve('0.7.0');
    await store.refreshVersion();
    expect(fetch).toHaveBeenCalledWith('/api/health', { cache: 'no-store' });
    expect(store.version).toBe('0.7.0');
    expect(reload).toHaveBeenCalledTimes(1);
    expect(mem.get(RELOAD_KEY)).toBe('0.7.0');
    await store.refreshVersion(); // reconnect: the guard holds
    expect(reload).toHaveBeenCalledTimes(1);
  });
  it('does nothing when the versions match or the build is the dev build', async () => {
    serve('0.6.0');
    await store.refreshVersion();
    store.buildVersion = '0.0.0-dev';
    serve('0.9.0');
    await store.refreshVersion();
    expect(reload).not.toHaveBeenCalled();
  });
  it('does nothing when the health answer has no version or fails', async () => {
    serve(undefined);
    await store.refreshVersion();
    vi.stubGlobal('fetch', vi.fn(async () => {
      throw new Error('down');
    }));
    await store.refreshVersion();
    expect(reload).not.toHaveBeenCalled();
  });
  it('waits for an open panel, tells the user, and reloads once it is closed', async () => {
    serve('0.7.0');
    store.advancedOpen = true;
    await store.refreshVersion();
    await store.refreshVersion();
    expect(reload).not.toHaveBeenCalled();
    expect(store.c.toasts.filter((t) => t.level === 'info')).toHaveLength(1);
    expect(store.reloadWaiting).toBe(true);
    store.advancedOpen = false;
    store.checkStale();
    expect(reload).toHaveBeenCalledTimes(1);
    expect(store.reloadWaiting).toBe(false);
  });
  it('never reloads when sessionStorage is unusable', async () => {
    vi.stubGlobal('sessionStorage', {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {},
    });
    serve('0.7.0');
    await store.refreshVersion();
    expect(reload).not.toHaveBeenCalled();
  });
});

describe('Advanced settings dialog state', () => {
  it('counts as an open panel (no stale reload) and opens on the asked or remembered tab', () => {
    const mem = new Map<string, string>();
    vi.stubGlobal('localStorage', { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v) });
    store.advancedOpen = false;
    expect(store.panelOpen).toBe(false);
    store.openAdvanced();
    expect(store.panelOpen).toBe(true);
    expect(store.advancedTab).toBe('plug');
    store.setAdvancedTab('osc');
    store.advancedOpen = false;
    store.openAdvanced();
    expect(store.advancedTab).toBe('osc');
    store.openAdvanced('version');
    expect(store.advancedTab).toBe('version');
    store.advancedOpen = false;
    vi.unstubAllGlobals();
  });
});
