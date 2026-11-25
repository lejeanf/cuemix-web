//! Short HTTP API for controls, widgets and shortcuts (`/api/v1/*`, docs/IOS-PLAN.md §5.1).
//!
//! A WebSocket plus a full snapshot is too slow for something that runs a few seconds in the
//! background, so:
//! - `GET /api/v1/summary`: levels, on/off and fading state, active preset, preset list, MOTU
//!   status and server version (see [`crate::engine::Summary`]).
//! - `POST /api/v1/command`: one [`Command`] in the same JSON as `/ws`, limited to
//!   `main_vol` / `phones_vol` (`{"cmd":"set","param":"main_vol","db":-12}`), `power` and
//!   `load_preset`. Anything else is refused with 400. The answer is the real outcome:
//!   `200 {"ok":true}`, or 4xx/5xx `{"ok":false,"error":"…"}`.
//!
//! Both use the same cross-site `Origin` check as `/ws` (no `Origin` header, as from an app,
//! is accepted). The change itself is broadcast to every client as usual.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::server::{origin_allowed, AppState};
use crate::state::{Command, Param};

/// Largest command body accepted.
pub(super) const MAX_BODY_BYTES: usize = 16 * 1024;

fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"ok": false, "error": message.into()})),
    )
        .into_response()
}

/// The only commands quick access needs. In particular the room test can never start from
/// here: a test is tied to the WebSocket client that started it.
pub(super) fn allowed(command: &Command) -> bool {
    matches!(
        command,
        Command::Set(Param::MainVol { .. } | Param::PhonesVol { .. })
            | Command::Power { .. }
            | Command::LoadPreset { .. }
    )
}

pub(super) async fn summary(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        return failure(StatusCode::FORBIDDEN, "origin not allowed");
    }
    match app.engine.summary().await {
        Ok(s) => ([(header::CACHE_CONTROL, "no-store")], Json(s)).into_response(),
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE, "engine stopped"),
    }
}

pub(super) async fn command(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        return failure(StatusCode::FORBIDDEN, "origin not allowed");
    }
    let command: Command = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => return failure(StatusCode::BAD_REQUEST, format!("invalid command: {e}")),
    };
    if !allowed(&command) {
        return failure(
            StatusCode::BAD_REQUEST,
            "command not available over the HTTP API (main_vol, phones_vol, power, load_preset only)",
        );
    }
    match app.engine.api_command(command).await {
        Ok(Ok(())) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"ok": true})),
        )
            .into_response(),
        Ok(Err(e)) if e.not_found => failure(StatusCode::NOT_FOUND, e.message),
        Ok(Err(e)) => failure(StatusCode::BAD_REQUEST, e.message),
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE, "engine stopped"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::PowerChannel;

    #[test]
    fn only_the_quick_access_commands_are_allowed() {
        assert!(allowed(&Command::Set(Param::MainVol { db: -10.0 })));
        assert!(allowed(&Command::Set(Param::PhonesVol { db: -10.0 })));
        assert!(allowed(&Command::Power {
            channel: PowerChannel::Main,
            on: false
        }));
        assert!(allowed(&Command::LoadPreset { name: "x".into() }));
        for refused in [
            Command::MeasureStart,
            Command::MeasureStop,
            Command::ListPresets,
            Command::SavePreset { name: "x".into() },
            Command::ImportPreset {
                name: "x".into(),
                text: String::new(),
            },
            Command::SetLabel { ch: 0, label: None },
            Command::Set(Param::Dim { on: true }),
            Command::Set(Param::Phantom { ch: 0, on: true }),
            Command::Set(Param::OutputTrim { ch: 0, db: 0.0 }),
            Command::TestSpeakerPlug { on: true },
            Command::SetSpeakerPlug {
                settings: Default::default(),
            },
        ] {
            assert!(!allowed(&refused), "{refused:?}");
        }
    }
}
