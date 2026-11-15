//! Bidirectional OSC/UDP bridge (Phase 2 — `agent-backend-bridges`).
//!
//! Contract: bind `network.osc_listen` (default `0.0.0.0:8000`), decode
//! packets/bundles with `rosc`, map them with [`super::address::parse`],
//! forward [`crate::state::Command`]s to the core, and send every
//! [`crate::state::ServerEvent::Changed`] param back as
//! [`super::address::feedback`] to `network.osc_feedback_targets` plus every
//! client that recently sent us a message (registered peers).
