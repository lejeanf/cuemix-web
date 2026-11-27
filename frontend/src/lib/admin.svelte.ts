// Admin pairing state shared by the Advanced dialog and the update badge.
import { apiUrl } from './update';
import { pairRequest, pairToken, parseAdmin, locked, type AdminInfo } from './admin';
import { S } from './strings';
import { store } from './store.svelte';

class Admin {
  info = $state<AdminInfo | null>(null);
  /** Outcome of a pairing link opened on this load (shown once in the dialog). */
  message = $state('');

  /** Admin-only controls are read-only on this device. */
  get locked(): boolean {
    return locked(this.info);
  }

  async refresh(): Promise<void> {
    try {
      const r = await fetch(apiUrl('api/admin'), { cache: 'no-store' });
      // An older server has no /api/admin: nothing is locked.
      this.info = r.ok ? parseAdmin(await r.json()) : null;
    } catch {
      this.info = null;
    }
  }

  /** On load: pair with a `#pair=…` link and read the state. A link opened while the page is already loaded only changes the hash, so that is watched too. */
  async start(): Promise<void> {
    // The UI also keeps its tab in the hash: only a pairing fragment matters here.
    window.addEventListener('hashchange', () => {
      if (pairToken(location.hash)) void this.consumeLink();
    });
    await this.consumeLink();
  }

  /** Pair with the token of a `#pair=…` fragment, if any (then drop it from the address bar and history entry), and refresh. */
  private async consumeLink(): Promise<void> {
    const token = pairToken(location.hash);
    if (token) {
      history.replaceState(null, '', location.pathname + location.search);
      try {
        const { url, init } = pairRequest(token);
        const r = await fetch(url, init);
        this.message = r.ok ? S.admin.paired : S.admin.pairFailed(r.status);
        // The WebSocket is checked when it opens: reopen it so it carries the new cookie.
        if (r.ok) store.renew();
      } catch {
        this.message = S.admin.pairFailed(0);
      }
    }
    await this.refresh();
  }

  async unpair(): Promise<void> {
    try {
      await fetch(apiUrl('api/admin/unpair'), { method: 'POST', credentials: 'same-origin' });
    } catch {
      // The state below says what happened.
    }
    this.message = '';
    store.renew();
    await this.refresh();
  }
}

export const admin = new Admin();
