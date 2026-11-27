// Optional admin token (server `[web] admin_token = "auto"`, see src/web/admin.rs): pure parts.
// A pairing link carries the token in the fragment (`#pair=<64 hex>`), so it never reaches a
// server log; the page posts it once and the server answers with an HttpOnly cookie.
import { apiUrl } from './update';

export interface AdminInfo {
  /** The server has an admin token. */
  required: boolean;
  /** This device holds it (always false when not required). */
  paired: boolean;
}

const PAIR_RE = /^#pair=([0-9a-f]{64})$/;

/** The token of a pairing link's fragment, or null. */
export function pairToken(hash: string): string | null {
  return PAIR_RE.exec(hash)?.[1] ?? null;
}

/** Admin-only controls are read-only on this device. */
export function locked(info: AdminInfo | null): boolean {
  return !!info?.required && !info.paired;
}

/** Shape check of a GET /api/admin answer. */
export function parseAdmin(v: unknown): AdminInfo | null {
  if (!v || typeof v !== 'object') return null;
  const o = v as Record<string, unknown>;
  if (typeof o.required !== 'boolean' || typeof o.paired !== 'boolean') return null;
  return { required: o.required, paired: o.paired };
}

export function pairRequest(token: string, loc?: Pick<Location, 'pathname'>): { url: string; init: RequestInit } {
  return {
    url: apiUrl('api/admin/pair', loc),
    init: { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token }), credentials: 'same-origin' },
  };
}
