// Connection store: one WebSocket to `/ws`, reactive client state, optimistic
// edits with a per-parameter 25 ms send throttle.
import { meterBank } from './meters';
import { hasMk5Meters, mk5MeterLevels } from './meterMap';
import { handleStale } from './reload';
import { loadAdvancedTab, saveAdvancedTab, type AdvancedTab } from './advanced';
import type { ChannelLabel, Command, DeviceInfo, EqBand, MeasureInfo, Param, OscSettings, PowerChannel, ServerEvent, SpeakerPlugSettings } from './protocol';
import { DSP_LIMITS } from './protocol';
import { applyEvent, applyParam, initialClientState, type ClientState } from './reducer';
import { ParamSender } from './sender';
import { clamp } from './validate';
import { isHwMapped, isHwReadOnly, visibleTabs, type Tab } from './hwmap';
import { S } from './strings';
import { tabFromHash, toastTimeout } from './ui';
import { applyTheme, loadTheme, saveTheme, type Theme } from './theme';
import { LIVENESS_CHECK_MS, needsLivenessCheck, parseLaunchFlags, resolveSimple, shouldDropSilentSocket, shouldReconnectNow } from './launch';

export type Link = 'offline' | 'connecting' | 'online' | 'demo';
export type { Tab };

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

const TABS: readonly Tab[] = ['mixer', 'input', 'output', 'reverb', 'probe'];
const VIEW_KEY = 'cuemix.view';

/** localStorage, or null when the browser blocks even looking at it. */
export function safeStorage(): Storage | null {
  try {
    return localStorage;
  } catch {
    return null;
  }
}
/** Simple (phone remote) or advanced view: remembered per browser, phones start simple.
 *  `?simple` in the URL forces Simple for this load without saving it. */
function initialSimple(): boolean {
  let stored: string | null = null;
  try {
    stored = localStorage.getItem(VIEW_KEY);
  } catch {
    /* storage blocked: fall back to the screen size */
  }
  const narrow = typeof window !== 'undefined' && !!window.matchMedia?.('(max-width: 640px)').matches;
  return resolveSimple(launchFlags(), stored, narrow);
}

/** `?simple` / `?embed` of this page load (see lib/launch.ts). */
function launchFlags() {
  return parseLaunchFlags(typeof location === 'undefined' ? '' : location.search);
}

function initialTab(): Tab {
  return (typeof location === 'undefined' ? null : tabFromHash(location.hash, TABS)) ?? 'mixer';
}

class Store {
  c: ClientState = $state(initialClientState());
  socketOpen = $state(false);
  /** Running server version (from `/api/health`). The web server never
   *  contacts GitHub; release checks are a root timer (see UpdateBadge). */
  version: string | null = $state(null);
  /** Tabs to show: the Reverb tab disappears on a MOTU while reverb is not written. */
  tabList(probe: boolean): Tab[] {
    return visibleTabs({ online: this.online, reverbWritable: isHwMapped('reverb_decay'), probe });
  }
  /** Current view; `#mixer`, `#input`, `#output`, `#reverb` in the URL open it directly. */
  tab: Tab = $state(initialTab());
  /** Phone remote (Main / Phones volume) instead of the full console. */
  simple = $state(initialSimple());
  /** Night mode (dimmed theme), per browser. */
  theme: Theme = $state(loadTheme(safeStorage()));
  setTheme(t: Theme): void {
    this.theme = t;
    saveTheme(safeStorage(), t);
    applyTheme(t);
  }
  /** `?embed`: running inside the native app's web view (hides the view toggle and status dot). */
  embed = launchFlags().embed;
  setSimple(on: boolean): void {
    this.simple = on;
    try {
      localStorage.setItem(VIEW_KEY, on ? 'simple' : 'advanced');
    } catch {
      /* not remembered, still applied */
    }
  }
  /** Room measurement state from the server (`unavailable` until it says otherwise). */
  measure: MeasureInfo = $state({ state: 'unavailable', source: [] });
  /** The Advanced settings dialog (speaker plug, OSC, version tabs) is open. */
  advancedOpen = $state(false);
  /** The tab it shows. */
  advancedTab: AdvancedTab = $state('plug');
  /** The room test panel is open. */
  roomOpen = $state(false);
  /** Simple view preset picker (a sheet: recall takes a deliberate second tap). */
  presetsOpen = $state(false);
  /** Main output EQ bands (slot 0) from before the last applied room correction, for Undo. */
  roomUndo: EqBand[] | null = $state(null);
  /** The room test can be offered: the server has it configured and we are connected. */
  get roomTestAvailable(): boolean {
    return this.measure.state !== 'unavailable';
  }
  /** A `measure_stop` could not be sent (socket down): sent on the next connect. */
  private measureStopPending = false;
  measureStart(): void {
    this.measureStopPending = false;
    this.sendRaw({ cmd: 'measure_start' });
  }
  measureStop(): void {
    this.measureStopPending = !this.sendRaw({ cmd: 'measure_stop' });
  }
  /** Latest raw meter frame (meter probe); not deep-reactive. */
  meterRaw: number[] | null = $state.raw(null);
  /** Output patch table read from the card (prop 5010); null = unknown. */
  patch: number[] | null = $state.raw(null);
  bus = $state(0);
  dspChannel: number | null = $state(null);

  private ws: WebSocket | null = null;
  private attempt = 0;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private stopped = true;
  private lastMsgAt = 0;
  private liveTimer: ReturnType<typeof setTimeout> | null = null;
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

  /** True for params the MOTU backend doesn't write yet (online only; demo mode stays live). */
  unmapped(p: Param['param']): boolean {
    return this.online && !isHwMapped(p);
  }
  /** Controls for these params are greyed out and disabled so they don't look broken. */
  locked(p: Param['param']): boolean {
    return this.unmapped(p);
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
    document.addEventListener('visibilitychange', this.onVisible);
    window.addEventListener('pageshow', this.onPageShow);
    window.addEventListener('hashchange', this.onHash);
    if (import.meta.env.DEV && new URLSearchParams(location.search).has('fakeMeters')) this.startFakeMeters();
  }

  /** Back/forward or an edited `#tab` on an open page switches the view. */
  private onHash = (): void => {
    const t = tabFromHash(location.hash, TABS);
    if (t) this.tab = t;
  };

  private onVisible = (): void => {
    if (document.visibilityState !== 'visible') return;
    this.reconnectNow();
    this.checkLiveness();
  };

  /** After unlock a socket can still read OPEN while dead. While online/demo
   *  the server streams meters (~30/s): if nothing arrives within
   *  LIVENESS_CHECK_MS of becoming visible, drop it without waiting for a
   *  close handshake and reconnect at once. */
  private checkLiveness(): void {
    if (this.stopped || this.liveTimer) return;
    if (!needsLivenessCheck(this.ws ? this.ws.readyState : null, this.link)) return;
    const visibleAt = Date.now();
    this.liveTimer = setTimeout(() => {
      this.liveTimer = null;
      if (this.stopped) return;
      const ws = this.ws;
      if (!ws || !shouldDropSilentSocket(ws.readyState, this.link, this.lastMsgAt, visibleAt)) return;
      this.dropAndReconnect(ws);
    }, LIVENESS_CHECK_MS);
  }
  private dropAndReconnect(ws: WebSocket): void {
    ws.onclose = null;
    ws.onmessage = null;
    ws.onerror = null;
    this.ws = null;
    this.socketOpen = false;
    ws.close();
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
    this.attempt = 0;
    this.connect();
  }
  /** Open a new socket now (the server decides what a socket may do when it opens, e.g. after admin pairing). */
  renew(): void {
    if (this.stopped) return;
    if (this.ws) this.dropAndReconnect(this.ws);
    else this.reconnectNow();
  }
  private clearLiveness(): void {
    if (this.liveTimer) clearTimeout(this.liveTimer);
    this.liveTimer = null;
  }
  private onPageShow = (e: PageTransitionEvent): void => {
    if (e.persisted) this.reconnectNow();
  };

  /** Page back from lock/background: reconnect now instead of waiting for the
   *  backoff timer. One attempt at a time (a socket already connecting is left alone). */
  reconnectNow(): void {
    if (!shouldReconnectNow(this.stopped, this.ws ? this.ws.readyState : null)) return;
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
    // Foreground reconnect: show "connecting", not "offline".
    this.attempt = 0;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    document.removeEventListener('visibilitychange', this.onVisible);
    window.removeEventListener('pageshow', this.onPageShow);
    window.removeEventListener('hashchange', this.onHash);
    if (this.retry) clearTimeout(this.retry);
    this.clearLiveness();
    if (this.fakeTimer) clearInterval(this.fakeTimer);
    this.sender.dispose();
    this.ws?.close();
    this.ws = null;
    this.socketOpen = false;
  }

  /** Opens the Advanced settings dialog on `tab`, or on the last tab used in this browser. */
  openAdvanced(tab?: AdvancedTab): void {
    this.advancedTab = tab ?? loadAdvancedTab(safeStorage());
    this.advancedOpen = true;
  }
  /** Shows a tab of the open dialog and remembers it. */
  setAdvancedTab(tab: AdvancedTab): void {
    this.advancedTab = tab;
    saveAdvancedTab(safeStorage(), tab);
  }

  /** Build version of this page (overridable for tests). */
  buildVersion: string = __APP_VERSION__;
  /** A server update was seen while a panel was open: the reload waits for it to close. */
  reloadWaiting = false;
  /** Any modal panel is open (edits in it must not be lost to a reload). */
  get panelOpen(): boolean {
    return this.advancedOpen || this.roomOpen || this.presetsOpen;
  }

  async refreshVersion(): Promise<void> {
    try {
      const r = await fetch(healthUrl(location), { cache: 'no-store' });
      const body = (await r.json()) as { version?: unknown };
      this.version = typeof body.version === 'string' ? body.version : null;
    } catch {
      this.version = null;
    }
    this.checkStale();
  }

  /** A tab that outlived a server update runs old code: reload it, once per server version, never while a panel is open. */
  checkStale(): void {
    let storage: Storage | null = null;
    try {
      storage = sessionStorage;
    } catch {
      /* blocked */
    }
    const out = handleStale({
      build: this.buildVersion,
      server: this.version,
      storage,
      busy: this.panelOpen,
      reload: () => location.reload(),
      notify: () => {
        if (this.reloadWaiting) return;
        this.reloadWaiting = true;
        this.c.toasts.push({ id: ++this.c.toastSeq, message: S.update.reloadNotice, level: 'info' });
      },
    });
    if (out === 'reloaded') this.reloadWaiting = false;
  }

  private connect(): void {
    if (this.stopped) return;
    const ws = new WebSocket(wsUrl(location));
    this.ws = ws;
    ws.onopen = () => {
      this.attempt = 0;
      this.socketOpen = true;
      // The server replays the patch only when it knows one: drop ours.
      this.patch = null;
      if (this.measureStopPending) this.measureStop();
      // Re-read on every (re)connect: the server may have just been updated.
      void this.refreshVersion();
    };
    ws.onmessage = (m) => {
      this.lastMsgAt = Date.now();
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
      this.clearLiveness();
      this.socketOpen = false;
      this.ws = null;
      if (this.stopped) return;
      const d = reconnectDelay(this.attempt++);
      this.retry = setTimeout(() => this.connect(), d);
    };
    ws.onerror = () => ws.close();
  }

  private handle(ev: ServerEvent): void {
    if (ev.event === 'meter_raw') {
      this.meterRaw = ev.raw;
      if (hasMk5Meters(this.c.session.device?.model, ev.raw.length)) meterBank.push(mk5MeterLevels(ev.raw, this.patch));
      return;
    }
    if (ev.event === 'patch') {
      this.patch = ev.sources.length ? ev.sources : null;
      return;
    }
    if (ev.event === 'meters') {
      // Hot path: bypass reactive state entirely, feed the canvas loop.
      meterBank.push(ev.levels);
      return;
    }
    if (ev.event === 'measure') {
      this.measure = { state: ev.state, source: [...(ev.source ?? [])], message: ev.message, until: ev.until };
      return;
    }
    applyEvent(this.c, ev);
    if (ev.event === 'error' || ev.event === 'notice') {
      // Errors stay until tapped away; notices and warnings fade on their own.
      const t = this.c.toasts.find((x) => x.id === this.c.toastSeq);
      const ms = t ? toastTimeout(t) : null;
      if (t && ms !== null) setTimeout(() => this.dismissToast(t.id), ms);
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

  /** Drops a throttled `set` of this parameter that has not been sent yet. */
  cancelPending(p: Param): void {
    this.sender.cancel(p);
  }

  loadPreset(name: string): void {
    this.sendRaw({ cmd: 'load_preset', name });
  }
  /** Sends a preset file's JSON (any supported version) to be stored as `name` and recalled. */
  importPreset(name: string, text: string): void {
    this.sendRaw({ cmd: 'import_preset', name, text });
  }

  savePreset(name: string): void {
    this.sendRaw({ cmd: 'save_preset', name });
  }
  listPresets(): void {
    this.sendRaw({ cmd: 'list_presets' });
  }
  deletePreset(name: string): void {
    this.sendRaw({ cmd: 'delete_preset', name });
  }
  renamePreset(name: string, to: string): void {
    this.sendRaw({ cmd: 'rename_preset', name, to });
  }
  setPresetInSimple(name: string, show: boolean): void {
    this.sendRaw({ cmd: 'set_preset_in_simple', name, show });
  }
  /** Presets the Simple view lists (the Advanced view lists them all). */
  get simplePresets(): string[] {
    const hidden = new Set(this.c.presetsHidden);
    return this.c.presets.filter((p) => !hidden.has(p));
  }
  /** Switch Monitor or Phones on/off; the server fades and reports back with a `power` event. */
  power(channel: PowerChannel, on: boolean): void {
    if (!this.ready) return;
    this.sendRaw({ cmd: 'power', channel, on });
  }
  /** While a channel is off: choose the level its next On fades back to (it stays off). */
  setReturnLevel(channel: PowerChannel, db: number): void {
    if (!this.ready) return;
    this.sendRaw({ cmd: 'set_return_level', channel, db: Math.round(db) });
  }
  setSpeakerPlug(settings: SpeakerPlugSettings): boolean {
    return this.sendRaw({ cmd: 'set_speaker_plug', settings });
  }
  setOscSettings(settings: OscSettings): boolean {
    return this.sendRaw({ cmd: 'set_osc_settings', settings });
  }
  /** Fires the plug's On or Off URL from the *saved* settings. */
  testSpeakerPlug(on: boolean): boolean {
    this.c.plugResult = null;
    return this.sendRaw({ cmd: 'test_speaker_plug', on });
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
