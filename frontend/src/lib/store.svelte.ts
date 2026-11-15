// Connection store: one WebSocket to `/ws`, reactive client state, optimistic
// edits with a per-parameter 25 ms send throttle.
import { meterBank } from './meters';
import type { ChannelLabel, Command, EqBand, Param, ServerEvent, Compressor, Gate, Hpf } from './protocol';
import { DSP_LIMITS } from './protocol';
import { applyEvent, applyParam, initialClientState, type ClientState } from './reducer';
import { ParamSender } from './sender';
import { clamp } from './validate';
import { isHwMapped } from './hwmap';
import { S } from './strings';

export type Link = 'offline' | 'connecting' | 'online' | 'demo';
export type Tab = 'mixer' | 'preamps' | 'routing' | 'reverb';

export const RECONNECT_MIN_MS = 500;
export const RECONNECT_MAX_MS = 8000;

/** Exponential reconnect delay for attempt n (0-based): 0.5 s → 8 s. */
export const reconnectDelay = (attempt: number): number =>
  Math.min(RECONNECT_MAX_MS, RECONNECT_MIN_MS * 2 ** attempt);

export function wsUrl(loc: Pick<Location, 'protocol' | 'host' | 'pathname'>): string {
  const scheme = loc.protocol === 'https:' ? 'wss:' : 'ws:';
  // Relative to the page so it works behind a reverse proxy subpath.
  const base = loc.pathname.endsWith('/') ? loc.pathname : loc.pathname.replace(/[^/]*$/, '');
  return `${scheme}//${loc.host}${base}ws`;
}

/** `/api/health` URL relative to the page (works behind a proxy subpath). */
export function healthUrl(loc: Pick<Location, 'pathname'>): string {
  const base = loc.pathname.endsWith('/') ? loc.pathname : loc.pathname.replace(/[^/]*$/, '');
  return `${base}api/health`;
}

class Store {
  c: ClientState = $state(initialClientState());
  socketOpen = $state(false);
  /** Running server version (from `/api/health`), shown in the top bar. No
   *  update check: the box never contacts GitHub (maintainer decision). */
  version: string | null = $state(null);
  tab: Tab = $state('mixer');
  bus = $state(0);
  dspChannel: number | null = $state(null);

  private ws: WebSocket | null = null;
  private attempt = 0;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private stopped = true;
  private sender = new ParamSender((p) => this.sendRaw({ cmd: 'set', ...p } as Command));
  private fakeTimer: ReturnType<typeof setInterval> | null = null;

  get state() {
    return this.c.state;
  }
  get ready(): boolean {
    return this.socketOpen && this.c.state !== null;
  }
  get link(): Link {
    if (!this.socketOpen) return this.attempt > 0 || this.stopped ? 'offline' : 'connecting';
    if (!this.c.state) return 'connecting';
    return this.c.session.connection;
  }
  get online(): boolean {
    return this.link === 'online';
  }

  /** Dashed "state only" hint for params the MOTU backend doesn't write yet. */
  unmapped(p: Param['param']): boolean {
    return this.online && !isHwMapped(p);
  }
  hint(p: Param['param']): string | undefined {
    return this.unmapped(p) ? S.unmapped : undefined;
  }

  label(ch: number): ChannelLabel | undefined {
    return this.c.labels.channels[String(ch)];
  }
  labelName(ch: number): string {
    return this.label(ch)?.name ?? `In ${ch + 1}`;
  }

  // ----------------------------------------------------------- lifecycle ---

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.connect();
    if (import.meta.env.DEV && new URLSearchParams(location.search).has('fakeMeters')) this.startFakeMeters();
  }

  stop(): void {
    this.stopped = true;
    if (this.retry) clearTimeout(this.retry);
    if (this.fakeTimer) clearInterval(this.fakeTimer);
    this.sender.dispose();
    this.ws?.close();
    this.ws = null;
    this.socketOpen = false;
  }

  private async refreshVersion(): Promise<void> {
    try {
      const r = await fetch(healthUrl(location), { cache: 'no-store' });
      const body = (await r.json()) as { version?: unknown };
      this.version = typeof body.version === 'string' ? body.version : null;
    } catch {
      this.version = null;
    }
  }

  private connect(): void {
    if (this.stopped) return;
    const ws = new WebSocket(wsUrl(location));
    this.ws = ws;
    ws.onopen = () => {
      this.attempt = 0;
      this.socketOpen = true;
      // Re-read on every (re)connect: the server may have just been updated.
      void this.refreshVersion();
    };
    ws.onmessage = (m) => {
      if (typeof m.data !== 'string') return;
      let ev: ServerEvent;
      try {
        ev = JSON.parse(m.data) as ServerEvent;
      } catch {
        return;
      }
      this.handle(ev);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.socketOpen = false;
      this.ws = null;
      if (this.stopped) return;
      const d = reconnectDelay(this.attempt++);
      this.retry = setTimeout(() => this.connect(), d);
    };
    ws.onerror = () => ws.close();
  }

  private handle(ev: ServerEvent): void {
    if (ev.event === 'meters') {
      // Hot path: bypass reactive state entirely, feed the canvas loop.
      meterBank.push(ev.levels);
      return;
    }
    applyEvent(this.c, ev);
    if (ev.event === 'error') {
      const id = this.c.toastSeq;
      setTimeout(() => this.dismissToast(id), 5000);
    }
    if (ev.event === 'snapshot' && this.bus >= ev.state.topology.mixes) this.bus = 0;
  }

  dismissToast(id: number): void {
    this.c.toasts = this.c.toasts.filter((t) => t.id !== id);
  }

  private startFakeMeters(): void {
    let t = 0;
    this.fakeTimer = setInterval(() => {
      t += 0.05;
      const n = this.c.state?.topology.mixer_inputs ?? 16;
      meterBank.push(Array.from({ length: n }, (_, i) => Math.max(0, 0.5 + 0.5 * Math.sin(t * (1 + i * 0.13) + i)) ** 2));
    }, 33);
  }

  // ------------------------------------------------------------- sending ---

  private sendRaw(cmd: Command): boolean {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(cmd));
      return true;
    }
    return false;
  }

  /**
   * Optimistic set: update local state now, send at most every 25 ms per
   * parameter (latest wins); `final` (pointerup, toggles) always sends at once.
   */
  set(p: Param, final = false): void {
    if (!this.ready || !this.c.state) return;
    applyParam(this.c.state, p);
    this.sender.set(p, final);
  }

  loadPreset(name: string): void {
    this.sendRaw({ cmd: 'load_preset', name });
  }
  savePreset(name: string): void {
    this.sendRaw({ cmd: 'save_preset', name });
  }
  listPresets(): void {
    this.sendRaw({ cmd: 'list_presets' });
  }
  setLabel(ch: number, label: ChannelLabel | null): void {
    if (label) this.c.labels.channels[String(ch)] = label;
    else delete this.c.labels.channels[String(ch)];
    this.sendRaw({ cmd: 'set_label', ch, label });
  }

  // ---------------------------------------------------- DSP block helpers ---

  setEqBand(ch: number, band: number, patch: Partial<EqBand>, final = false): void {
    const cur = this.c.state?.channels[ch]?.eq.bands[band];
    if (!cur) return;
    const n = { ...cur, ...patch };
    this.set(
      {
        param: 'eq_band',
        ch,
        band,
        enabled: n.enabled,
        freq_hz: clamp(n.freq_hz, DSP_LIMITS.FREQ_HZ),
        gain_db: clamp(n.gain_db, DSP_LIMITS.EQ_GAIN_DB),
        q: clamp(n.q, DSP_LIMITS.EQ_Q),
      },
      final,
    );
  }
  setHpf(ch: number, patch: Partial<Hpf>, final = false): void {
    const cur = this.c.state?.channels[ch]?.eq.hpf;
    if (!cur) return;
    const n = { ...cur, ...patch };
    this.set({ param: 'hpf', ch, enabled: n.enabled, freq_hz: clamp(n.freq_hz, DSP_LIMITS.FREQ_HZ) }, final);
  }
  setComp(ch: number, patch: Partial<Compressor>, final = false): void {
    const cur = this.c.state?.channels[ch]?.comp;
    if (!cur) return;
    const n = { ...cur, ...patch };
    this.set(
      {
        param: 'comp',
        ch,
        enabled: n.enabled,
        threshold_db: clamp(n.threshold_db, DSP_LIMITS.COMP_THRESHOLD_DB),
        ratio: clamp(n.ratio, DSP_LIMITS.COMP_RATIO),
        attack_ms: clamp(n.attack_ms, DSP_LIMITS.ATTACK_MS),
        release_ms: clamp(n.release_ms, DSP_LIMITS.RELEASE_MS),
        gain_db: clamp(n.gain_db, DSP_LIMITS.COMP_GAIN_DB),
      },
      final,
    );
  }
  setGate(ch: number, patch: Partial<Gate>, final = false): void {
    const cur = this.c.state?.channels[ch]?.gate;
    if (!cur) return;
    const n = { ...cur, ...patch };
    this.set(
      {
        param: 'gate',
        ch,
        enabled: n.enabled,
        threshold_db: clamp(n.threshold_db, DSP_LIMITS.GATE_THRESHOLD_DB),
        attack_ms: clamp(n.attack_ms, DSP_LIMITS.ATTACK_MS),
        release_ms: clamp(n.release_ms, DSP_LIMITS.RELEASE_MS),
      },
      final,
    );
  }
  setReverb(patch: Partial<{ enabled: boolean; return_gain: number; time_s: number; predelay_ms: number }>, final = false): void {
    const r = this.c.state?.reverb;
    if (!r) return;
    const n = { enabled: r.enabled, return_gain: r.return_gain, time_s: r.time_s, predelay_ms: r.predelay_ms, ...patch };
    this.set(
      {
        param: 'reverb',
        enabled: n.enabled,
        return_gain: n.return_gain,
        time_s: clamp(n.time_s, DSP_LIMITS.REVERB_TIME_S),
        predelay_ms: clamp(n.predelay_ms, DSP_LIMITS.REVERB_PREDELAY_MS),
      },
      final,
    );
  }
}

export const store = new Store();
