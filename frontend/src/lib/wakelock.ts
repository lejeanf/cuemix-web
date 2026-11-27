// "Keep screen on" for the Simple view (stage use): a per-browser choice, off by default.
// The browser drops a wake lock whenever the page is hidden, so it is asked for again each
// time the page comes back.

export const KEEP_AWAKE_KEY = 'cuemix.keepAwake';

type Store = Pick<Storage, 'getItem' | 'setItem'> | null;

export function loadKeepAwake(s: Store): boolean {
  try {
    return s?.getItem(KEEP_AWAKE_KEY) === '1';
  } catch {
    return false;
  }
}

export function saveKeepAwake(s: Store, on: boolean): void {
  try {
    s?.setItem(KEEP_AWAKE_KEY, on ? '1' : '0');
  } catch {
    /* blocked storage: the choice lasts for this page only */
  }
}

/** True when this browser can keep the screen on. */
export function canKeepAwake(): boolean {
  return typeof navigator !== 'undefined' && 'wakeLock' in navigator;
}

/** Holds a screen wake lock while the page is visible; returns the release function. */
export function holdScreenAwake(): () => void {
  let lock: WakeLockSentinel | null = null;
  let stopped = false;
  const ask = async () => {
    if (stopped || document.visibilityState !== 'visible') return;
    try {
      const l = await navigator.wakeLock.request('screen');
      if (stopped) void l.release();
      else lock = l;
    } catch {
      /* refused (battery saver, no user gesture yet): try again on the next return */
    }
  };
  const onVisible = () => void ask();
  document.addEventListener('visibilitychange', onVisible);
  void ask();
  return () => {
    stopped = true;
    document.removeEventListener('visibilitychange', onVisible);
    void lock?.release();
    lock = null;
  };
}
