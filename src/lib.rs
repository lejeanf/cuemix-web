//! `cuemix-web` — standalone web & OSC control bridge for the MOTU UltraLite-mk5.
//!
//! Module map (see `docs/ARCHITECTURE.md`, the living source of truth):
//! - [`config`]  — `config.toml` loading + environment overrides.
//! - [`state`]   — central [`state::MotuState`], the canonical [`state::Command`] set
//!   shared by Web and OSC, and the resulting [`state::Change`] events.
//! - [`motu`]    — hardware side: fader law, `8.24` fixed point, binary frames,
//!   discovery and the WebSocket client.
//! - [`osc`]     — OSC address table and the UDP bridge.
//! - [`web`]     — JSON WebSocket contract and the Axum server.

// Production code must never panic on bad input: no unwrap/expect outside tests.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![warn(missing_debug_implementations)]

pub mod config;
pub mod motu;
pub mod osc;
pub mod state;
pub mod web;
