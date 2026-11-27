// Night mode: the dark theme with every light dimmed (app.css `[data-theme='night']`), for a dark
// room or a stage. A manual choice, remembered per browser; off by default.
import { refreshMeterPalette } from './meters';

export const THEME_KEY = 'cuemix.theme';
export type Theme = 'dark' | 'night';

type Store = Pick<Storage, 'getItem' | 'setItem'> | null;

export function loadTheme(s: Store): Theme {
  try {
    return s?.getItem(THEME_KEY) === 'night' ? 'night' : 'dark';
  } catch {
    return 'dark';
  }
}

export function saveTheme(s: Store, t: Theme): void {
  try {
    s?.setItem(THEME_KEY, t);
  } catch {
    /* blocked storage: the choice lasts for this page only */
  }
}

/** Puts the theme on the page (and on the meters, which draw on canvases). */
export function applyTheme(t: Theme): void {
  if (typeof document === 'undefined') return;
  if (t === 'night') document.documentElement.dataset.theme = 'night';
  else delete document.documentElement.dataset.theme;
  refreshMeterPalette();
}
