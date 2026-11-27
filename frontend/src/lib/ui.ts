// Small UI rules kept out of the components so they can be tested.
import type { Tab } from './hwmap';
import type { Toast } from './reducer';

/** The view a URL fragment (`#mixer`, `#input`, …) names among `tabs`, or `null` when it names none. */
export function tabFromHash(hash: string, tabs: readonly Tab[]): Tab | null {
  const h = hash.replace(/^#/, '');
  return (tabs as readonly string[]).includes(h) ? (h as Tab) : null;
}

/** Notices and warnings disappear after this long. */
export const TOAST_MS = 5000;

/**
 * How long a toast stays on screen: notices and warnings go after `TOAST_MS`,
 * errors (no level) stay until they are tapped away, so none is missed.
 */
export function toastTimeout(t: Pick<Toast, 'level'>): number | null {
  return t.level ? TOAST_MS : null;
}
