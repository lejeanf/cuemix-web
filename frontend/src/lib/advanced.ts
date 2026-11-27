// Advanced settings dialog (the ⋯ button): one dialog, three tabs. The last tab used is
// remembered per browser in localStorage, which may be blocked or throw: every access is guarded.
export const ADVANCED_TABS = ['plug', 'osc', 'version'] as const;
export type AdvancedTab = (typeof ADVANCED_TABS)[number];
export const ADVANCED_TAB_KEY = 'cuemix.advancedTab';
export const DEFAULT_ADVANCED_TAB: AdvancedTab = 'plug';

export type TabStorage = Pick<Storage, 'getItem' | 'setItem'>;

export const isAdvancedTab = (v: unknown): v is AdvancedTab => typeof v === 'string' && (ADVANCED_TABS as readonly string[]).includes(v);

/** The remembered tab, or the speaker plug tab when there is none, it is unknown or storage fails. */
export function loadAdvancedTab(storage: TabStorage | null): AdvancedTab {
  try {
    const v = storage?.getItem(ADVANCED_TAB_KEY);
    return isAdvancedTab(v) ? v : DEFAULT_ADVANCED_TAB;
  } catch {
    return DEFAULT_ADVANCED_TAB;
  }
}

export function saveAdvancedTab(storage: TabStorage | null, tab: AdvancedTab): void {
  try {
    storage?.setItem(ADVANCED_TAB_KEY, tab);
  } catch {
    /* blocked storage: the tab is just not remembered */
  }
}

/** The next tab for an arrow key / Home / End on `current`, or null for any other key. */
export function nextTab(current: AdvancedTab, key: string): AdvancedTab | null {
  const i = ADVANCED_TABS.indexOf(current);
  const n = ADVANCED_TABS.length;
  switch (key) {
    case 'ArrowRight':
    case 'ArrowDown':
      return ADVANCED_TABS[(i + 1) % n];
    case 'ArrowLeft':
    case 'ArrowUp':
      return ADVANCED_TABS[(i + n - 1) % n];
    case 'Home':
      return ADVANCED_TABS[0];
    case 'End':
      return ADVANCED_TABS[n - 1];
    default:
      return null;
  }
}
