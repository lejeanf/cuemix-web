// Shared state of GET /api/update for the update badge and the Version panel:
// one fetch/install/progress logic (see update.ts for the pure parts).
import { S } from './strings';
import { apiUrl, installRequest, prefsRequest, type UpdateInfo } from './update';

/** An install that was accepted and has not finished. */
export interface Waiting {
  /** `last_result.at` before the install: a different one means it finished. */
  baseAt: string | null;
  version: string | null;
  /** The socket dropped since (the service restarted). */
  sawDown: boolean;
}

export const WAIT_TIMEOUT_MS = 10 * 60 * 1000;
const POLL_MS = 5000;

/** True once the install that started with `w` is over: a newer result appeared, or the server came back after a restart. */
export function installDone(w: Waiting, info: UpdateInfo | null, socketOpen: boolean): boolean {
  if (info && (info.last_result?.at ?? null) !== w.baseAt) return true;
  return w.sawDown && socketOpen;
}

class Updates {
  info = $state<UpdateInfo | null>(null);
  /** The install request itself is in flight. */
  requesting = $state(false);
  /** Accepted, not yet finished. The root side removes the request file before it downloads, so `pending` alone is not enough. */
  waiting = $state<Waiting | null>(null);
  /** The last fetch failed (and there is no earlier answer). */
  loadFailed = $state(false);

  private timer: ReturnType<typeof setTimeout> | null = null;
  private poll: ReturnType<typeof setInterval> | null = null;

  get inProgress(): boolean {
    return this.requesting || this.waiting !== null || !!this.info?.pending;
  }
  /** Version being installed, when known ("newest" installs report it once the root side reads the request). */
  get installingVersion(): string | null {
    return this.waiting?.version ?? this.info?.pending_version ?? null;
  }

  async refresh(): Promise<void> {
    try {
      const r = await fetch(apiUrl('api/update'), { cache: 'no-store' });
      if (r.ok) {
        this.info = (await r.json()) as UpdateInfo;
        this.loadFailed = false;
      } else this.loadFailed = true;
    } catch {
      this.loadFailed = true; // offline: the last answer (if any) stays
    }
    if (this.waiting && installDone(this.waiting, this.info, false)) this.finish();
  }

  /** Call with the socket state: a drop and reconnect during an install means the new version runs. */
  noteSocket(open: boolean): void {
    const w = this.waiting;
    if (w) {
      if (!open) w.sawDown = true;
      else if (w.sawDown) {
        this.finish();
        void this.refresh();
      }
    }
  }

  private finish(): void {
    this.waiting = null;
    if (this.timer) clearTimeout(this.timer);
    if (this.poll) clearInterval(this.poll);
    this.timer = this.poll = null;
  }

  /** Installs `version` (a release tag), or the newest of the channel when omitted. Returns an error text, or null when accepted. */
  async install(version?: string): Promise<string | null> {
    this.requesting = true;
    // With per-version install, "install the latest" names the exact tag the UI showed, so
    // the root side never decides by itself which channel to follow.
    if (version === undefined && this.info?.version_pinning && this.info.latest?.tag) {
      version = this.info.latest.tag;
    }
    const { url, init } = installRequest(version);
    try {
      const r = await fetch(url, init);
      if (!r.ok) return (await r.text()) || S.update.requestFailed;
      this.waiting = { baseAt: this.info?.last_result?.at ?? null, version: version ?? null, sawDown: false };
      this.timer = setTimeout(() => this.finish(), WAIT_TIMEOUT_MS);
      this.poll = setInterval(() => void this.refresh(), POLL_MS);
      return null;
    } catch {
      return S.update.requestFailed;
    } finally {
      this.requesting = false;
    }
  }

  /** Persists the pre-release toggle on the server, then reloads the list. Returns an error text or null. */
  async setShowPrerelease(on: boolean): Promise<string | null> {
    if (this.info) this.info.show_prerelease = on;
    const { url, init } = prefsRequest(on);
    let err: string | null = null;
    try {
      const r = await fetch(url, init);
      if (!r.ok) err = (await r.text()) || S.update.requestFailed;
    } catch {
      err = S.update.requestFailed;
    }
    await this.refresh();
    return err;
  }
}

export const updates = new Updates();
