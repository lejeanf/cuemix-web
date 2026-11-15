// Connection store: one WebSocket to `/ws`, reactive client state, optimistic
// edits with a per-parameter 25 ms send throttle.
import { meterBank } from './meters';
import type { ChannelLabel, Command, DeviceInfo, EqBand, Param, ServerEvent } from './protocol';
import { DSP_LIMITS } from './protocol';
import { applyEvent, applyParam, initialClientState, type ClientState } from './reducer';
import { ParamSender } from './sender';
import { clamp } from './validate';
import { isHwMapped, isHwReadOnly } from './hwmap';
import { S } from './strings';

export type Link = 'offline' | 'connecting' | 'online' | 'demo';
export type Tab = 'mixer' | 'input' | 'output' | 'routing' | 'reverb';

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
    if (!this.unmapped(p)) return undefined;
    return this.online && this.device && isHwReadOnly(p) ? S.readOnly : S.unmapped;
  }

  /** What the connected MOTU reported about itself, or null. */
  get device(): DeviceInfo | null {
    return this.c.session.device ?? null;
  }

  /** Strip name: user label, else the device's (pair) name, else `In N`. */
  stripName(ch: number, stereo = false): string {
    const own = this.label(ch)?.name;
    if (own) return own;
    const d = this.device;
    return (stereo && d?.stereo_names[ch]) || d?.input_names[ch] || `In ${ch + 1}`;
  }

  /** Mixer strips: stereo pairs (left channel flagged by the device) share one strip. */
  get strips(): { ch: number; pair?: number }[] {
    const n = this.c.state?.topology.mixer_inputs ?? 0;
    const stereo = this.device?.stereo_inputs ?? [];
    const out: { ch: number; pair?: number }[] = [];
    for (let ch = 0; ch < n; ch++) {
      if (stereo[ch] && ch + 1 < n) {
        out.push({ ch, pair: ch + 1 });
        ch++;
      } else out.push({ ch });
    }
    return out;
  }

  /** Mix indices in display order (device order if it gives one). */
  get mixOrder(): number[] {
    const n = this.c.state?.mixes.length ?? 0;
    const order = (this.device?.mix_order ?? []).filter((i) => i < n);
    if (order.length !== n || new Set(order).size !== n) return Array.from({ length: n }, (_, i) => i);
    return order;
  }

  /** Mix tab name: user-set name, else the device's name, else `Mix N`. */
  mixName(i: number): string {
    const own = this.c.state?.mixes[i]?.name ?? '';
    if (own && own !== `Mix ${i + 1}`) return own;
    return this.device?.mix_names[i] || own || `Mix ${i + 1}`;
  }

  label(ch: number): ChannelLabel | undefined {
    return this.c.labels.channels[String(ch)];
  }
  labelName(ch: number): string {
    return this.label(ch)?.name ?? (this.device?.input_names[ch] || `In ${ch + 1}`);
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

  // ------------------------------------------------------------- DSP ---

  /** Strips 0..n-1 with an input EQ / compressor (the analog inputs). */
  hasDsp(ch: number): boolean {
    return ch >= 0 && ch < (this.c.state?.input_eq.length ?? 0);
  }
  /** Inputs with a gate (the mic inputs). */
  hasGate(ch: number): boolean {
    return ch >= 0 && ch < (this.c.state?.gates.length ?? 0);
  }

  /** The Reverb mix (its faders are the reverb sends): by device name, else the last mix. */
  get reverbMix(): number | null {
    const n = this.c.state?.mixes.length ?? 0;
    if (n === 0) return null;
    const i = this.device?.mix_names.indexOf('Reverb') ?? -1;
    return i >= 0 && i < n ? i : n - 1;
  }
  /** Mixer input carrying the reverb return (left of its pair): by device name, else the last pair. */
  get reverbReturn(): number | null {
    const n = this.c.state?.topology.mixer_inputs ?? 0;
    const i = this.device?.stereo_names.indexOf('Reverb') ?? -1;
    if (i >= 0 && i < n) return i;
    return n >= 2 ? n - 2 : null;
  }

  /** Set fields of an input or output EQ band (one param per field, clamped like the server). */
  setEq(
    kind: 'input' | 'output',
    ch: number,
    band: number,
    patch: Partial<Pick<EqBand, 'mode' | 'bypass' | 'freq_hz' | 'gain_db' | 'bw_oct'>>,
    final = false,
  ): void {
    const L = DSP_LIMITS;
    const send = (p: Param) => this.set(p, final);
    if (kind === 'input') {
      if (patch.mode !== undefined) send({ param: 'input_eq_mode', ch, band, mode: patch.mode });
      if (patch.bypass !== undefined) send({ param: 'input_eq_bypass', ch, band, on: patch.bypass });
      if (patch.freq_hz !== undefined) send({ param: 'input_eq_freq', ch, band, hz: Math.round(clamp(patch.freq_hz, L.FREQ_HZ)) });
      if (patch.gain_db !== undefined) send({ param: 'input_eq_gain', ch, band, db: clamp(patch.gain_db, L.EQ_GAIN_DB) });
      if (patch.bw_oct !== undefined) send({ param: 'input_eq_bw', ch, band, octaves: clamp(patch.bw_oct, L.EQ_BW_OCT) });
    } else {
      if (patch.mode !== undefined) send({ param: 'output_eq_mode', ch, band, mode: patch.mode });
      if (patch.bypass !== undefined) send({ param: 'output_eq_bypass', ch, band, on: patch.bypass });
      if (patch.freq_hz !== undefined) send({ param: 'output_eq_freq', ch, band, hz: Math.round(clamp(patch.freq_hz, L.FREQ_HZ)) });
      if (patch.gain_db !== undefined) send({ param: 'output_eq_gain', ch, band, db: clamp(patch.gain_db, L.EQ_GAIN_DB) });
      if (patch.bw_oct !== undefined) send({ param: 'output_eq_bw', ch, band, octaves: clamp(patch.bw_oct, L.EQ_BW_OCT) });
    }
  }
}

export const store = new Store();
