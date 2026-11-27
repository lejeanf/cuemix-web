import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PowerInfo, SpeakerPlugSettings } from './protocol';
import { powerView } from './power';
import { applyEvent, initialClientState } from './reducer';
import { defaultPlugSettings } from './plug';
import { store } from './store.svelte';

const info = (): PowerInfo => ({
  main: { on: true, fading: 'in', plug: 'warming' },
  phones: { on: false, fading: null, plug: null },
});

describe('powerView', () => {
  it('shows what the server reports', () => {
    expect(powerView(info(), 'main', -100)).toEqual({ on: true, fading: 'in', plug: 'warming', returnDb: null });
    expect(powerView(info(), 'phones', -20)).toEqual({ on: false, fading: null, plug: null, returnDb: null });
  });
  it('shows where the channel is heading while it fades', () => {
    // Plug warm-up: the server still reports on=false (level at the minimum).
    const warm: PowerInfo = { main: { on: false, fading: 'in', plug: 'warming' }, phones: { on: true, fading: 'out', plug: null } };
    expect(powerView(warm, 'main', -100).on).toBe(true);
    expect(powerView(warm, 'phones', -20).on).toBe(false);
  });
  it('says where On goes back to while off', () => {
    const off: PowerInfo = { main: { on: false, fading: null, plug: null, return_db: -18 }, phones: { on: true, fading: 'out', plug: null, return_db: -30 } };
    expect(powerView(off, 'main', -100).returnDb).toBe(-18);
    expect(powerView(off, 'phones', -50).returnDb).toBe(-30);
    // Heading on: nothing to return to.
    const warm: PowerInfo = { main: { on: false, fading: 'in', plug: 'warming', return_db: -18 }, phones: { on: true, fading: null, plug: null } };
    expect(powerView(warm, 'main', -100).returnDb).toBeNull();
  });
  it('falls back to the level before the first power event', () => {
    expect(powerView(null, 'main', -30)).toEqual({ on: true, fading: null, plug: null, returnDb: null });
    expect(powerView(null, 'main', -100)).toEqual({ on: false, fading: null, plug: null, returnDb: null });
  });
});

describe('power / plug commands', () => {
  const sent: string[] = [];
  afterEach(() => {
    vi.unstubAllGlobals();
    sent.length = 0;
    store.socketOpen = false;
    store.c.state = null;
    (store as unknown as { ws: unknown }).ws = null;
  });
  function connect() {
    vi.stubGlobal('WebSocket', { OPEN: 1 });
    (store as unknown as { ws: unknown }).ws = { readyState: 1, send: (m: string) => sent.push(m) };
    store.socketOpen = true;
  }
  const json = () => sent.map((m) => JSON.parse(m));

  it('power sends the contract shape, only when ready', () => {
    connect();
    store.power('main', true); // no snapshot yet: not ready
    expect(sent).toEqual([]);
    store.c.state = {} as never;
    store.power('main', true);
    store.power('phones', false);
    expect(json()).toEqual([
      { cmd: 'power', channel: 'main', on: true },
      { cmd: 'power', channel: 'phones', on: false },
    ]);
  });
  it('set_speaker_plug and test_speaker_plug shapes', () => {
    connect();
    const settings: SpeakerPlugSettings = { ...defaultPlugSettings(), enabled: true, host: 'plug.example' };
    expect(store.setSpeakerPlug(settings)).toBe(true);
    store.c.plugResult = { on: true, ok: true, message: 'x' };
    expect(store.testSpeakerPlug(false)).toBe(true);
    expect(store.c.plugResult).toBeNull();
    expect(json()).toEqual([
      { cmd: 'set_speaker_plug', settings },
      { cmd: 'test_speaker_plug', on: false },
    ]);
  });
  it('commands report failure when the socket is down', () => {
    expect(store.setSpeakerPlug(defaultPlugSettings())).toBe(false);
    expect(store.testSpeakerPlug(true)).toBe(false);
  });
});

describe('reducer: power and plug events', () => {
  it('power, speaker_plug, result, notice, error', () => {
    const s = initialClientState();
    applyEvent(s, { event: 'power', ...info() });
    expect(s.power?.main).toEqual({ on: true, fading: 'in', plug: 'warming' });
    const settings = { ...defaultPlugSettings(), enabled: true, host: 'plug.example' };
    applyEvent(s, { event: 'speaker_plug', settings, urls: { on: 'a', off: 'b' }, state: 'on' });
    expect(s.plug).toEqual({ settings, urls: { on: 'a', off: 'b' }, state: 'on' });
    applyEvent(s, { event: 'speaker_plug_result', on: true, ok: false, message: 'timeout' });
    expect(s.plugResult).toEqual({ on: true, ok: false, message: 'timeout' });
    applyEvent(s, { event: 'notice', level: 'warning', message: 'Speaker plug did not respond' });
    expect(s.toasts.at(-1)).toMatchObject({ message: 'Speaker plug did not respond', level: 'warning' });
    applyEvent(s, { event: 'error', message: 'bad host' });
    expect(s.toasts.at(-1)?.level).toBeUndefined();
    expect(s.lastError).toEqual({ seq: s.toastSeq, message: 'bad host' });
    // unknown events and extra fields are ignored
    applyEvent(s, { event: 'future_thing', x: 1 } as never);
    applyEvent(s, { event: 'power', ...info(), extra: 1 } as never);
    expect(s.power?.phones.on).toBe(false);
  });
});
