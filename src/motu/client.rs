//! MOTU WebSocket client (Phase 2 — `agent-motu-protocol`).
//!
//! Contract for Phase 2:
//! - connects to `ws://<motu_ip>:1280`, binary frames from [`super::protocol`];
//! - on (re)connect applies **Option A — hardware priority**: full state read,
//!   then a [`crate::state::ServerEvent::Snapshot`] is broadcast to Web & OSC;
//! - throttles outgoing commands **per parameter** to at most `throttle_hz`
//!   (default 40 Hz / 25 ms) and always delivers the last value;
//! - falls back to [`ConnectionStatus::Demo`] when the device is absent.

use serde::{Deserialize, Serialize};

/// Link state reported to every client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    /// Talking to real hardware.
    Online,
    /// Discovery / handshake in progress.
    Connecting,
    /// No hardware: simulated in-memory state (offline demo mode).
    #[default]
    Demo,
}
