//! Axum HTTP + JSON WebSocket server (Phase 2 — `agent-backend-bridges`).
//!
//! Contract:
//! - `GET /ws` upgrades to a WebSocket carrying JSON text frames:
//!   client → server: [`crate::state::Command`] (`{"cmd":"set","param":…}` /
//!   `{"cmd":"load_preset","name":…}`); server → client:
//!   [`crate::state::ServerEvent`] (`{"event":"snapshot"|"changed"|"status"|"session",…}`).
//!   A `snapshot` is always sent first after the upgrade.
//! - every other path serves the embedded `frontend/dist/` bundle (`rust-embed`),
//!   falling back to `index.html` for client-side routes.
//! - honours `X-Forwarded-For` / `X-Forwarded-Proto` when
//!   `web.trust_forwarded_headers = true` (HAProxy / pfSense).
