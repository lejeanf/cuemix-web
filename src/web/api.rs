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

use super::body::{check_json_content_type, parse_json};
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

/// Checks, in order: size, `Content-Type: application/json`, UTF-8, control characters, then
/// the typed [`Command`] parse and [`Command::validate`] (names, text, settings). The body is
/// required: an empty one is a 400.
pub(super) fn parse_command(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Command, (StatusCode, String)> {
    if body.len() > MAX_BODY_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("body is larger than {MAX_BODY_BYTES} bytes"),
        ));
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid command: a JSON body is required".into(),
        ));
    }
    check_json_content_type(headers)?;
    let command: Command = parse_json(body, MAX_BODY_BYTES, "invalid command")?;
    command
        .validate()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(command)
}

pub(super) async fn command(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        return failure(StatusCode::FORBIDDEN, "origin not allowed");
    }
    let command = match parse_command(&headers, &body) {
        Ok(c) => c,
        Err((status, message)) => return failure(status, message),
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

    fn json_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        h
    }

    #[test]
    fn command_bodies_are_checked_before_and_while_parsing() {
        let ok = br#"{"cmd":"set","param":"main_vol","db":-12}"#;
        assert!(parse_command(&json_headers(), ok).is_ok());
        let code = |h: &HeaderMap, b: &[u8]| parse_command(h, b).map_err(|e| e.0);
        // No content type, a wrong one, a form type.
        assert_eq!(
            code(&HeaderMap::new(), ok),
            Err(StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
        let mut text = HeaderMap::new();
        text.insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/plain"),
        );
        assert_eq!(code(&text, ok), Err(StatusCode::UNSUPPORTED_MEDIA_TYPE));
        // Empty body.
        assert_eq!(code(&json_headers(), b""), Err(StatusCode::BAD_REQUEST));
        assert_eq!(code(&json_headers(), b"  \n"), Err(StatusCode::BAD_REQUEST));
        // Oversized.
        let big = format!(
            r#"{{"cmd":"list_presets","pad":"{}"}}"#,
            "x".repeat(MAX_BODY_BYTES)
        );
        assert_eq!(
            code(&json_headers(), big.as_bytes()),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
        for bad in [
            &b"\xff{}"[..],
            b"{\"cmd\":\"list_presets\"}\0",
            b"{\"cmd\":\"list_presets\x1b\"}",
            b"[]",
            b"null",
            b"\"set\"",
            br#"{"cmd":"set","param":"main_vol","db":"-12"}"#,
            br#"{"cmd":"set","param":"main_vol"}"#,
            br#"{"cmd":"set","param":"nope","db":1}"#,
            br#"{"cmd":["set"]}"#,
        ] {
            assert_eq!(
                code(&json_headers(), bad),
                Err(StatusCode::BAD_REQUEST),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        // Names are validated before the engine sees them.
        for name in ["../x", "a/b", "", ".x", "a\\u0000b", "\\u202ex", "\\uff0f"] {
            let body = format!(r#"{{"cmd":"load_preset","name":"{name}"}}"#);
            assert_eq!(
                code(&json_headers(), body.as_bytes()),
                Err(StatusCode::BAD_REQUEST),
                "{name}"
            );
        }
        let live = br#"{"cmd":"load_preset","name":"Live 1"}"#;
        assert!(matches!(
            parse_command(&json_headers(), live),
            Ok(Command::LoadPreset { .. })
        ));
    }

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
            Command::SetOscSettings {
                settings: Default::default(),
            },
        ] {
            assert!(!allowed(&refused), "{refused:?}");
        }
    }
}
