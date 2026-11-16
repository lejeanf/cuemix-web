//! `/api/update`: release status and a user-triggered install.
//!
//! Privilege split: this process (unprivileged `cuemix` user) only reads the
//! status written by the root `cuemix-update-check` timer and creates an
//! empty request file. The root `cuemix-update-request.path` unit sees it and
//! runs `cuemix-update`, which downloads the release, checks its sha256,
//! swaps the binary, restarts the service and rolls back if it is unhealthy.

use std::path::Path;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::server::AppState;

/// Header the UI sends with the install request. A plain cross-site form
/// cannot set it, and a page that tries gets a CORS preflight this server
/// never approves.
pub const INSTALL_HEADER: &str = "x-cuemix-update";

async fn read_json(path: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&text).ok()
}

/// `cuemix-update` exit code of the last requested install, as text.
async fn read_result(path: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let mut parts = text.split_whitespace();
    let code: i32 = parts.next()?.parse().ok()?;
    let at = parts.next().unwrap_or_default();
    let outcome = match code {
        0 => "ok",
        5 => "rolled_back",
        6 => "rollback_failed",
        4 => "busy",
        _ => "failed",
    };
    Some(json!({ "code": code, "outcome": outcome, "at": at }))
}

pub(super) async fn status(State(app): State<AppState>) -> Json<Value> {
    let u = &app.cfg.update;
    let status = read_json(&u.status_file).await;
    Json(json!({
        "installed": env!("CARGO_PKG_VERSION"),
        "channel": u.channel,
        // No status file: the timer / units are not installed here (dev,
        // Docker), so the UI hides the update controls.
        "enabled": status.is_some(),
        "status": status,
        "pending": tokio::fs::try_exists(&u.request_file).await.unwrap_or(false),
        "last_result": read_result(&u.result_file).await,
    }))
}

pub(super) async fn install(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if !headers.contains_key(INSTALL_HEADER) {
        return (StatusCode::FORBIDDEN, "missing x-cuemix-update header").into_response();
    }
    let u = &app.cfg.update;
    if !tokio::fs::try_exists(&u.status_file).await.unwrap_or(false) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "updates are not set up on this machine (run the setup script's update units)",
        )
            .into_response();
    }
    // create_new: an install already queued is not queued twice.
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&u.request_file)
        .await
    {
        Ok(_) => {
            tracing::info!("update requested from the web UI");
            (StatusCode::ACCEPTED, "update requested").into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            (StatusCode::CONFLICT, "an update is already in progress").into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %u.request_file.display(), "cannot create the update request");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot create the update request",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn result_codes_map_to_outcomes() {
        let dir = std::env::temp_dir().join(format!("cuemix-upd-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let p = dir.join("r");
        tokio::fs::write(&p, "5 2026-11-15T20:00:00Z\n")
            .await
            .unwrap();
        let v = read_result(&p).await.unwrap();
        assert_eq!(v["outcome"], "rolled_back");
        assert_eq!(v["at"], "2026-11-15T20:00:00Z");
        tokio::fs::write(&p, "garbage").await.unwrap();
        assert!(read_result(&p).await.is_none());
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}
