// Launch-time flags and the lock-screen reconnect decision (pure, testable).

export interface LaunchFlags {
  /** `?simple`: open the Simple view for this page load, without saving it. */
  simple: boolean;
  /** `?embed`: embedded in the native app's web view; hides the Simple/Advanced
   *  toggle and the connection dot (the app has its own chrome). */
  embed: boolean;
}

/** Reads `?simple` / `?embed` (bare or with any value except `0` / `false`). */
export function parseLaunchFlags(search: string): LaunchFlags {
  const q = new URLSearchParams(search);
  const on = (k: string): boolean => q.has(k) && !['0', 'false'].includes((q.get(k) ?? '').toLowerCase());
  return { simple: on('simple'), embed: on('embed') };
}

/** Initial view: `?simple` wins, then `?embed` alone (Advanced: in the native
 *  app Simple is native, the web view shows the full pages), then the
 *  remembered choice, then the screen size. Nothing here is saved. */
export function resolveSimple(flags: LaunchFlags, stored: string | null, narrowScreen: boolean): boolean {
  if (flags.simple) return true;
  if (flags.embed) return false;
  if (stored === 'simple' || stored === 'advanced') return stored === 'simple';
  return narrowScreen;
}

/** WebSocket.readyState values (the global is not available under node tests). */
export const WS_CONNECTING = 0;
export const WS_OPEN = 1;
export const WS_CLOSING = 2;
export const WS_CLOSED = 3;

/** On return from lock/background: reconnect now unless stopped, already open,
 *  or an attempt (or a clean close) is already in flight. `null` = no socket
 *  (waiting on the backoff timer). */
export function shouldReconnectNow(stopped: boolean, readyState: number | null): boolean {
  if (stopped) return false;
  return readyState === null || readyState === WS_CLOSED;
}

/** Wait after becoming visible before judging an OPEN socket dead. */
export const LIVENESS_CHECK_MS = 2000;

/** Only online/demo sessions stream meters, so only they can be judged by silence. */
export function needsLivenessCheck(readyState: number | null, link: string): boolean {
  return readyState === WS_OPEN && (link === 'online' || link === 'demo');
}

/** True when an OPEN socket in an online/demo session delivered nothing since
 *  the page became visible: treat it as dead. */
export function shouldDropSilentSocket(readyState: number | null, link: string, lastMsgAt: number, visibleAt: number): boolean {
  return needsLivenessCheck(readyState, link) && lastMsgAt < visibleAt;
}
