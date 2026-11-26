// Stale tab reload (contract 0.7.0 C): a tab whose build differs from the running server
// reloads once per server version. The guard lives in sessionStorage so it can never loop:
// it keeps the last few server versions the tab already reloaded for (so A -> B -> A cannot
// ping-pong either), and a reload only happens when the guard was written and reads back.
export const DEV_BUILD = '0.0.0-dev';
export const RELOAD_KEY = 'cuemix.reloadedFor';
export const RELOAD_MEMORY = 5;

/** Normalizes "v0.7.0" / "0.7.0" for comparison. */
const norm = (v: string) => v.trim().replace(/^v/, '');

/** Pure decision: reload when both versions are known, differ, this is no dev build and we did not already reload for that server version. */
export function shouldReload(build: string, server: string | null, already: readonly string[]): boolean {
  if (!server || !build || build === DEV_BUILD) return false;
  if (norm(build) === norm(server)) return false;
  return !already.includes(norm(server));
}

const parseList = (raw: string | null): string[] => (raw ? raw.split(',').filter(Boolean) : []);

export type Guard = Pick<Storage, 'getItem' | 'setItem'>;

/** Whether a reload is due, without recording anything (a read that throws means no). */
export function reloadDue(build: string, server: string | null, storage: Guard | null): boolean {
  if (!storage || !server) return false;
  try {
    return shouldReload(build, server, parseList(storage.getItem(RELOAD_KEY)));
  } catch {
    return false;
  }
}

/** Records the guard and returns true when a reload may go ahead. Storage errors never throw and never allow a reload. */
export function claimReload(build: string, server: string | null, storage: Guard | null): boolean {
  if (!storage || !server || !reloadDue(build, server, storage)) return false;
  try {
    const list = [...parseList(storage.getItem(RELOAD_KEY)), norm(server)].slice(-RELOAD_MEMORY);
    storage.setItem(RELOAD_KEY, list.join(','));
    // Read back: a store that silently drops writes would otherwise loop.
    return parseList(storage.getItem(RELOAD_KEY)).includes(norm(server));
  } catch {
    return false;
  }
}

export type StaleOutcome = 'none' | 'deferred' | 'reloaded';

/** What to do when the server's version is known: reload now, wait for open panels, or nothing. */
export function handleStale(o: { build: string; server: string | null; storage: Guard | null; busy: boolean; reload: () => void; notify: () => void }): StaleOutcome {
  if (!reloadDue(o.build, o.server, o.storage)) return 'none';
  if (o.busy) {
    o.notify();
    return 'deferred';
  }
  if (!claimReload(o.build, o.server, o.storage)) return 'none';
  o.reload();
  return 'reloaded';
}
