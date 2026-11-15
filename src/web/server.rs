//! Axum HTTP + JSON WebSocket server.
//!
//! Contract (see `docs/ARCHITECTURE.md` §3.5):
//! - `GET /ws` upgrades to a WebSocket carrying JSON text frames:
//!   client → server: [`Command`]; server → client: [`ServerEvent`].
//!   `Snapshot`, `Labels`, `Presets` are always sent first after the upgrade.
//! - `GET /api/health` returns `{"status":"ok","version":…}`.
//! - every other path serves the embedded `frontend/dist/` bundle, falling
//!   back to `index.html` for client-side routes. Only embedded names are
//!   ever served, so path traversal is impossible.
//! - the client IP used in logs is the peer address, or the first
//!   `X-Forwarded-For` entry when `web.trust_forwarded_headers = true`.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::Context;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::RustEmbed;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::config::Config;
use crate::engine::{EngineHandle, Origin, Shutdown};
use crate::state::{Command, ServerEvent};

/// The compiled frontend (`frontend/dist/` must exist at build time).
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

/// Maximum inbound WebSocket message size.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// Keep-alive ping period.
const PING_EVERY: Duration = Duration::from_secs(20);
/// Minimum spacing between two forwarded `Meters` events (60 Hz).
const METER_MIN_INTERVAL: Duration = Duration::from_micros(16_667);

/// Web server settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebConfig {
    /// Trust `X-Forwarded-For` for the client IP (behind HAProxy / pfSense).
    pub trust_forwarded_headers: bool,
}

impl WebConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            trust_forwarded_headers: cfg.web.trust_forwarded_headers,
        }
    }
}

#[derive(Clone)]
struct AppState {
    engine: EngineHandle,
    cfg: WebConfig,
}

/// Build the router (HTTP + WebSocket).
pub fn router(engine: EngineHandle, cfg: WebConfig) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/health", get(health))
        .fallback(get(static_handler))
        .with_state(AppState { engine, cfg })
}

/// Serve until `shutdown` fires.
pub async fn serve(
    listener: tokio::net::TcpListener,
    engine: EngineHandle,
    cfg: WebConfig,
    mut shutdown: Shutdown,
) -> anyhow::Result<()> {
    let app = router(engine, cfg);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown.wait().await })
    .await
    .context("web server failed")
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}))
}

// ---------------------------------------------------------------- static ---

async fn static_handler(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let suspicious = raw.contains("..") || raw.contains('\\') || raw.contains('\0');
    let path = if raw.is_empty() || suspicious {
        "index.html"
    } else {
        raw
    };

    if let Some(resp) = embedded(path) {
        return resp;
    }
    // Missing build artefacts are a real 404; anything else is an SPA route.
    if path.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    embedded("index.html").unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

fn embedded(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=3600"
    };
    let mut resp = Response::new(axum::body::Body::from(file.data.into_owned()));
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(file.metadata.mimetype()) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Some(resp)
}

// ------------------------------------------------------------- websocket ---

fn client_ip(peer: SocketAddr, headers: &HeaderMap, trust: bool) -> IpAddr {
    if trust {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

async fn ws_handler(
    State(app): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let ip = client_ip(peer, &headers, app.cfg.trust_forwarded_headers);
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let id = app.engine.next_client_id();
            tracing::info!(client = id, %ip, "websocket connected");
            session(socket, app.engine, Origin::Web(id)).await;
            tracing::info!(client = id, %ip, "websocket disconnected");
        })
}

/// Send one event as a JSON text frame. `Err` means the socket is gone.
async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), ()> {
    match serde_json::to_string(event) {
        Ok(text) => socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| ()),
        Err(e) => {
            tracing::error!(error = %e, "cannot serialise event");
            Ok(())
        }
    }
}

async fn send_initial(socket: &mut WebSocket, engine: &EngineHandle) -> Result<(), ()> {
    let events = engine.initial_events().await.map_err(|_| ())?;
    for ev in &events {
        send_event(socket, ev).await?;
    }
    Ok(())
}

async fn session(mut socket: WebSocket, engine: EngineHandle, me: Origin) {
    // Subscribe before the snapshot so nothing is missed in between.
    let mut rx = engine.subscribe();
    if send_initial(&mut socket, &engine).await.is_err() {
        return;
    }

    let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let mut last_meter: Option<Instant> = None;

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                match msg {
                    Message::Text(text) => match serde_json::from_str::<Command>(text.as_str()) {
                        Ok(cmd) => {
                            if engine.command(me, cmd).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let err = ServerEvent::Error { message: format!("invalid command: {e}") };
                            if send_event(&mut socket, &err).await.is_err() {
                                break;
                            }
                        }
                    },
                    Message::Close(_) => break,
                    // Binary ignored; Ping is auto-answered by axum; Pong ignored.
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            ev = rx.recv() => match ev {
                Ok(env) => {
                    if !env.is_for(me) {
                        continue;
                    }
                    match &env.event {
                        ServerEvent::Changed { .. } if env.origin == me => continue,
                        ServerEvent::Meters { .. } => {
                            let now = Instant::now();
                            if last_meter.is_some_and(|t| now.duration_since(t) < METER_MIN_INTERVAL) {
                                continue;
                            }
                            last_meter = Some(now);
                        }
                        _ => {}
                    }
                    if send_event(&mut socket, &env.event).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "websocket client lagged, resyncing");
                    if send_initial(&mut socket, &engine).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 1000,
            reason: "bye".into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ip_prefers_forwarded_only_when_trusted() {
        let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("192.168.1.9, 10.0.0.1"),
        );
        assert_eq!(client_ip(peer, &h, true).to_string(), "192.168.1.9");
        assert_eq!(client_ip(peer, &h, false).to_string(), "10.0.0.1");
        h.insert("x-forwarded-for", HeaderValue::from_static("garbage"));
        assert_eq!(client_ip(peer, &h, true).to_string(), "10.0.0.1");
    }
}
