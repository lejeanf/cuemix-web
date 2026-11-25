//! Axum HTTP + JSON WebSocket server.
//!
//! Contract (see `docs/ARCHITECTURE.md` §3.5):
//! - `GET /ws` upgrades to a WebSocket carrying JSON text frames:
//!   client → server: [`Command`]; server → client: [`ServerEvent`].
//!   `Snapshot`, `Labels`, `Presets` are always sent first after the upgrade.
//! - `GET /api/health` returns `{"status":"ok","version":…,"api":1}`.
//! - `GET /api/v1/summary` and `POST /api/v1/command`: the short HTTP API (`super::api`).
//!   Same `Origin` rule as `/ws`.
//! - `GET /api/measure/noise.wav` streams the room-test pink noise (`crate::measure`).
//! - every other path serves the embedded `frontend/dist/` bundle, falling
//!   back to `index.html` for client-side routes. Only embedded names are
//!   ever served, so path traversal is impossible.
//! - the client IP used in logs is the peer address, or the rightmost
//!   `X-Forwarded-For` entry (appended by the trusted proxy) when
//!   `web.trust_forwarded_headers = true`.
//! - `/ws` and `/api/v1/*` reject cross-site browser requests (403): an `Origin` header
//!   must match the request host or an entry of `web.allowed_origins`.
//! - every WebSocket write has a [`SEND_TIMEOUT`]; a client that stops
//!   reading is disconnected so it can never pin memory or stall others.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::Context;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::DefaultBodyLimit;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{Sink, SinkExt};
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
const MAX_MESSAGE_BYTES: usize = 256 * 1024;
/// Keep-alive ping period.
const PING_EVERY: Duration = Duration::from_secs(20);
/// Maximum time a single WebSocket write may take before the client is dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(2);
/// Minimum spacing between two forwarded `Meters` events (60 Hz).
const METER_MIN_INTERVAL: Duration = Duration::from_micros(16_667);

/// Web server settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebConfig {
    /// Trust `X-Forwarded-For` / `X-Forwarded-Host` (behind HAProxy / pfSense).
    pub trust_forwarded_headers: bool,
    /// Extra origins (`scheme://host[:port]`) allowed to open `/ws`.
    pub allowed_origins: Vec<String>,
    pub update: crate::config::Update,
}

impl WebConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            trust_forwarded_headers: cfg.web.trust_forwarded_headers,
            allowed_origins: cfg.web.allowed_origins.clone(),
            update: cfg.update.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct AppState {
    pub(super) engine: EngineHandle,
    pub(super) cfg: WebConfig,
}

/// Build the router (HTTP + WebSocket).
pub fn router(engine: EngineHandle, cfg: WebConfig) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/health", get(health))
        .route("/api/v1/summary", get(super::api::summary))
        .route(
            "/api/v1/command",
            post(super::api::command).layer(DefaultBodyLimit::max(super::api::MAX_BODY_BYTES)),
        )
        .route(
            "/api/measure/noise.wav",
            get(crate::measure::noise::handler),
        )
        .route("/api/update", get(super::update::status))
        .route("/api/update/install", post(super::update::install))
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
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "api": crate::engine::API_VERSION,
    }))
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
            .and_then(|v| v.rsplit(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

/// Canonical `(scheme, host, port)` of a strict `scheme://host[:port]` origin.
fn parse_origin(raw: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    if rest.is_empty()
        || rest
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/?#@\\".contains(c))
    {
        return None;
    }
    let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
        let (addr, tail) = v6.split_once(']')?;
        let port = match tail {
            "" => default_port,
            t => t.strip_prefix(':')?.parse().ok()?,
        };
        (format!("[{}]", addr.to_ascii_lowercase()), port)
    } else {
        match rest.split_once(':') {
            Some((h, p)) => (h.to_ascii_lowercase(), p.parse().ok()?),
            None => (rest.to_ascii_lowercase(), default_port),
        }
    };
    if host.is_empty() || host == "[]" {
        return None;
    }
    Some((scheme, host, port))
}

/// Whether a WebSocket upgrade with these headers may proceed.
pub(super) fn origin_allowed(headers: &HeaderMap, cfg: &WebConfig) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true; // non-browser client
    };
    let Some(parsed) = origin.to_str().ok().and_then(parse_origin) else {
        return false;
    };
    let (scheme, host, port) = &parsed;
    let request_host = if cfg.trust_forwarded_headers {
        headers
            .get("x-forwarded-host")
            .or_else(|| headers.get(header::HOST))
    } else {
        headers.get(header::HOST)
    }
    .and_then(|v| v.to_str().ok())
    .map(|h| {
        // Only the rightmost forwarded host (appended by our proxy) counts.
        h.rsplit(',')
            .next()
            .unwrap_or(h)
            .trim()
            .to_ascii_lowercase()
    });
    if let Some(h) = request_host {
        let default_port = matches!((scheme.as_str(), *port), ("http", 80) | ("https", 443));
        if h == format!("{host}:{port}") || (default_port && h == *host) {
            return true;
        }
    }
    cfg.allowed_origins
        .iter()
        .filter_map(|o| parse_origin(o))
        .any(|allowed| allowed == parsed)
}

async fn ws_handler(
    State(app): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        let origin = headers
            .get(header::ORIGIN)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .unwrap_or_default();
        tracing::info!(%origin, peer = %peer.ip(), "rejected websocket: origin not allowed");
        return StatusCode::FORBIDDEN.into_response();
    }
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

/// A WebSocket write failed or timed out; the session must end.
#[derive(Debug, PartialEq, Eq)]
struct SendFailed;

/// Send one message, giving up after `limit` (client not reading).
async fn send_msg<S>(sink: &mut S, msg: Message, limit: Duration) -> Result<(), SendFailed>
where
    S: Sink<Message> + Unpin,
{
    match tokio::time::timeout(limit, sink.send(msg)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(SendFailed),
        Err(_) => {
            tracing::info!("websocket send timed out, closing slow client");
            Err(SendFailed)
        }
    }
}

/// Send one event as a JSON text frame.
async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), SendFailed> {
    match serde_json::to_string(event) {
        Ok(text) => send_msg(socket, Message::Text(text.into()), SEND_TIMEOUT).await,
        Err(e) => {
            tracing::error!(error = %e, "cannot serialise event");
            Ok(())
        }
    }
}

async fn send_initial(socket: &mut WebSocket, engine: &EngineHandle) -> Result<(), SendFailed> {
    let events = engine.initial_events().await.map_err(|_| SendFailed)?;
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
                        ServerEvent::Meters { .. } | ServerEvent::MeterRaw { .. } => {
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
                if send_msg(&mut socket, Message::Ping(Vec::new().into()), SEND_TIMEOUT).await.is_err() {
                    break;
                }
            }
        }
    }
    // A room test started from this connection ends with it.
    let _ = engine.client_gone(me).await;
    let close = Message::Close(Some(CloseFrame {
        code: 1000,
        reason: "bye".into(),
    }));
    let _ = send_msg(&mut socket, close, SEND_TIMEOUT).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ip_prefers_forwarded_only_when_trusted() {
        let peer: SocketAddr = "192.0.2.1:5000".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.9, 198.51.100.7"),
        );
        assert_eq!(client_ip(peer, &h, true).to_string(), "198.51.100.7");
        assert_eq!(client_ip(peer, &h, false).to_string(), "192.0.2.1");
        h.insert("x-forwarded-for", HeaderValue::from_static("garbage"));
        assert_eq!(client_ip(peer, &h, true).to_string(), "192.0.2.1");
    }

    fn hm(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn origin_rules() {
        let cfg = WebConfig {
            trust_forwarded_headers: false,
            allowed_origins: vec!["https://Mixer.Example.com".into()],
            update: Default::default(),
        };
        let ok = |p: &[(&'static str, &'static str)]| origin_allowed(&hm(p), &cfg);
        assert!(ok(&[("host", "pi.local:8080")]));
        assert!(ok(&[
            ("host", "pi.local:8080"),
            ("origin", "http://pi.local:8080")
        ]));
        assert!(ok(&[
            ("host", "PI.local"),
            ("origin", "http://pi.LOCAL:80")
        ]));
        assert!(!ok(&[
            ("host", "pi.local:8080"),
            ("origin", "http://evil.example")
        ]));
        assert!(!ok(&[
            ("host", "pi.local:8080"),
            ("origin", "http://pi.local")
        ]));
        assert!(ok(&[
            ("host", "x"),
            ("origin", "https://mixer.example.com")
        ]));
        assert!(!ok(&[
            ("host", "x"),
            ("origin", "http://mixer.example.com")
        ]));
        assert!(!ok(&[("host", "x"), ("origin", "null")]));
        assert!(!ok(&[("host", "x"), ("origin", "http://x/path")]));
        assert!(!ok(&[("host", "x"), ("origin", "ftp://x")]));
        assert!(!ok(&[("host", "x"), ("origin", "http://")]));
        // X-Forwarded-Host is ignored unless trusted.
        let fwd = [
            ("host", "internal"),
            ("x-forwarded-host", "pub.example"),
            ("origin", "https://pub.example"),
        ];
        assert!(!ok(&fwd));
        let trusted = WebConfig {
            trust_forwarded_headers: true,
            ..cfg
        };
        assert!(origin_allowed(&hm(&fwd), &trusted));
    }

    /// A sink that never accepts anything, like a client that never reads.
    struct Stalled;
    impl Sink<Message> for Stalled {
        type Error = ();
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), ()>> {
            std::task::Poll::Pending
        }
        fn start_send(self: std::pin::Pin<&mut Self>, _: Message) -> Result<(), ()> {
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), ()>> {
            std::task::Poll::Pending
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), ()>> {
            std::task::Poll::Pending
        }
    }

    #[tokio::test]
    async fn send_to_stalled_sink_times_out() {
        let mut sink = Stalled;
        let r = send_msg(
            &mut sink,
            Message::Text("x".into()),
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(r, Err(SendFailed));
    }
}
