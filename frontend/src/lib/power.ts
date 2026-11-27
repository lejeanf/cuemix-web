import { TRIM_MIN_DB, type FadeDirection, type PlugState, type PowerInfo } from './protocol';

export interface PowerView {
  on: boolean;
  fading: FadeDirection | null;
  plug: PlugState | null;
  /** Level an On brings the channel back to, while it is off or fading out (null otherwise or unknown). */
  returnDb: number | null;
}

/**
 * What a power button shows: the server's `power` state. Until the server has sent one
 * (older server), On follows the level (above the minimum) and nothing fades.
 */
export function powerView(info: PowerInfo | null, channel: 'main' | 'phones', db: number): PowerView {
  const c = info?.[channel];
  if (!c) return { on: db > TRIM_MIN_DB + 0.5, fading: null, plug: null, returnDb: null };
  // Where the channel is heading: a fade in (including the plug warm-up, while the level is
  // still at the minimum) shows On, a fade out shows Off.
  const on = c.fading === 'in' ? true : c.fading === 'out' ? false : c.on;
  return { on, fading: c.fading, plug: channel === 'main' ? (info?.main.plug ?? null) : null, returnDb: on ? null : (c.return_db ?? null) };
}
