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
//! - every request must name a known host (DNS-rebinding guard, [`host_allowed`]): an IP
//!   literal, `localhost`, a `.local` name, the mDNS name, or an entry of `web.allowed_hosts` /
//!   `web.allowed_origins`. Anything else gets 421 before any route runs.
//! - `/ws`, `/api/v1/*` and `/api/update*` reject cross-site browser requests (403): an
//!   `Origin` header must match the request host or an entry of `web.allowed_origins`.
//! - every response carries a Content-Security-Policy (no framing, scripts from this server
//!   only), `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`,
//!   `Referrer-Policy: no-referrer` and `Cross-Origin-Opener-Policy: same-origin`
//!   ([`security_headers`]).
//! - every WebSocket write has a [`SEND_TIMEOUT`]; a client that stops
//!   reading is disconnected so it can never pin memory or stall others.
//! - a client gets [`HEADER_READ_TIMEOUT`] to send its request headers (slow-header attacks), at
//!   most [`MAX_CONNECTIONS`] HTTP connections are served at once, and WebSockets are capped at
//!   [`MAX_WS_TOTAL`], [`MAX_WS_PER_IP`] per client address (503 beyond).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::DefaultBodyLimit;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
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
/// Time a client has to send a request's headers (also how long an idle keep-alive connection
/// stays open).
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP connections served at once (WebSockets are counted separately once upgraded).
pub const MAX_CONNECTIONS: usize = 256;
/// Open WebSockets, in total and per client address.
pub const MAX_WS_TOTAL: usize = 64;
pub const MAX_WS_PER_IP: usize = 16;

/// Web server settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebConfig {
    /// Trust `X-Forwarded-For` / `X-Forwarded-Host` (behind HAProxy / pfSense).
    pub trust_forwarded_headers: bool,
    /// Extra origins (`scheme://host[:port]`) allowed to open `/ws`.
    pub allowed_origins: Vec<String>,
    /// Extra host names the UI is reached by (`[web] allowed_hosts`), on top of IP literals,
    /// `localhost`, `.local` names and [`Self::mdns_hostname`].
    pub allowed_hosts: Vec<String>,
    /// `[network] mdns_hostname` (accepted bare and as `<name>.local`).
    pub mdns_hostname: String,
    /// The admin token when `[web] admin_token = "auto"` (see [`super::admin`]); `None` = off.
    pub admin_token: Option<super::admin::AdminToken>,
    pub update: crate::config::Update,
    /// `{"show_prerelease":bool}` saved from the UI (`[storage] update_prefs_file`).
    pub update_prefs_file: std::path::PathBuf,
}

impl WebConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            trust_forwarded_headers: cfg.web.trust_forwarded_headers,
            allowed_origins: cfg.web.allowed_origins.clone(),
            allowed_hosts: cfg.web.allowed_hosts.clone(),
            mdns_hostname: cfg.network.mdns_hostname.clone(),
            // Loaded by `main` (it may have to create the file).
            admin_token: None,
            update: cfg.update.clone(),
            update_prefs_file: cfg.storage.update_prefs_file.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct AppState {
    pub(super) engine: EngineHandle,
    pub(super) cfg: WebConfig,
    /// Spacing of accepted install requests.
    pub(super) gate: std::sync::Arc<super::update::Cooldown>,
    /// Open WebSockets per client address.
    ws_slots: std::sync::Arc<WsSlots>,
}

/// Counts open WebSockets, in total and per client address.
#[derive(Debug, Default)]
struct WsSlots {
    open: std::sync::Mutex<std::collections::HashMap<IpAddr, usize>>,
}

/// One open WebSocket; frees its slot when dropped.
struct WsSlot {
    slots: std::sync::Arc<WsSlots>,
    ip: IpAddr,
}

impl WsSlots {
    /// A slot for `ip`, or `None` when the total or the per-address limit is reached.
    fn take(self: &std::sync::Arc<Self>, ip: IpAddr) -> Option<WsSlot> {
        let mut open = self.open.lock().unwrap_or_else(|p| p.into_inner());
        let total: usize = open.values().sum();
        let mine = open.get(&ip).copied().unwrap_or(0);
        if total >= MAX_WS_TOTAL || mine >= MAX_WS_PER_IP {
            return None;
        }
        open.insert(ip, mine + 1);
        Some(WsSlot {
            slots: self.clone(),
            ip,
        })
    }
}

impl Drop for WsSlot {
    fn drop(&mut self) {
        let mut open = self.slots.open.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = open.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(&self.ip);
            }
        }
    }
}

/// Build the router (HTTP + WebSocket).
pub fn router(engine: EngineHandle, cfg: WebConfig) -> Router {
    let state = AppState {
        engine,
        cfg,
        gate: std::sync::Arc::new(super::update::Cooldown::new(
            super::update::INSTALL_COOLDOWN,
        )),
        ws_slots: std::sync::Arc::default(),
    };
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
        .route(
            "/api/update",
            get(super::update::status)
                .post(super::update::install)
                .layer(DefaultBodyLimit::max(super::update::MAX_BODY_BYTES)),
        )
        .route(
            "/api/update/install",
            post(super::update::install)
                .layer(DefaultBodyLimit::max(super::update::MAX_BODY_BYTES)),
        )
        .route(
            "/api/update/prefs",
            put(super::update::put_prefs)
                .layer(DefaultBodyLimit::max(super::update::MAX_BODY_BYTES)),
        )
        .route("/api/admin", get(super::admin::status))
        .route(
            "/api/admin/pair",
            post(super::admin::pair).layer(DefaultBodyLimit::max(super::admin::MAX_BODY_BYTES)),
        )
        .route("/api/admin/unpair", post(super::admin::unpair))
        .fallback(get(static_handler))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            host_guard,
        ))
        .with_state(state)
}

/// Serve until `shutdown` fires.
pub async fn serve(
    listener: tokio::net::TcpListener,
    engine: EngineHandle,
    cfg: WebConfig,
    mut shutdown: Shutdown,
) -> anyhow::Result<()> {
    tracing::info!(
        hosts = %accepted_hosts(&cfg),
        "web UI answers for these host names (others get 421; add yours to [web] allowed_hosts)"
    );
    let app = router(engine, cfg);
    // Each connection task holds a receiver; closing the sender asks them to finish.
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(());
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    // Out of file descriptors and the like: back off instead of spinning.
                    tracing::warn!(error = %e, "cannot accept a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = shutdown.wait() => break,
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            tracing::debug!(peer = %peer.ip(), "too many connections, closing a new one");
            continue;
        };
        let app = app.clone();
        let mut stop = stop_rx.clone();
        tokio::spawn(async move {
            serve_connection(stream, peer, app, &mut stop).await;
            drop(permit);
        });
    }
    drop(listener);
    drop(stop_rx);
    stop_tx.send_replace(());
    // Let open requests finish (upgraded WebSockets are not waited for).
    let _ = tokio::time::timeout(Duration::from_secs(5), stop_tx.closed()).await;
    Ok(())
}

/// One HTTP/1 connection: headers must arrive within [`HEADER_READ_TIMEOUT`], the peer address
/// is passed to handlers as `ConnectInfo`, and a change on `stop` starts a graceful close.
async fn serve_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    app: Router,
    stop: &mut tokio::sync::watch::Receiver<()>,
) {
    use hyper_util::rt::{TokioIo, TokioTimer};
    use hyper_util::service::TowerToHyperService;
    use tower::ServiceExt;

    let service = app.map_request(move |req: axum::http::Request<hyper::body::Incoming>| {
        let mut req = req.map(axum::body::Body::new);
        req.extensions_mut().insert(ConnectInfo(peer));
        req
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(stream), TowerToHyperService::new(service))
        .with_upgrades();
    let mut conn = std::pin::pin!(conn);
    let mut stopping = false;
    loop {
        tokio::select! {
            result = conn.as_mut() => {
                if let Err(e) = result {
                    tracing::trace!(error = %e, "connection ended with an error");
                }
                break;
            }
            changed = stop.changed(), if !stopping => {
                let _ = changed;
                stopping = true;
                conn.as_mut().graceful_shutdown();
            }
        }
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "api": crate::engine::API_VERSION,
    }))
}

// ------------------------------------------------------------ host guard ---

/// The host part of a `Host` / `X-Forwarded-Host` value (`name`, `name:port`, `[v6]:port`),
/// lowercase, without a trailing dot. IPv6 literals keep their brackets. `None` when the value
/// is not a plain authority.
pub(super) fn host_part(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 300
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/?#@\\,\"<>".contains(c))
    {
        return None;
    }
    let valid_port = |p: &str| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok()
    };
    if let Some(v6) = raw.strip_prefix('[') {
        let (addr, tail) = v6.split_once(']')?;
        if !(tail.is_empty() || tail.strip_prefix(':').is_some_and(valid_port)) {
            return None;
        }
        addr.parse::<Ipv6Addr>().ok()?;
        return Some(format!("[{}]", addr.to_ascii_lowercase()));
    }
    let host = match raw.split_once(':') {
        Some((h, p)) if valid_port(p) => h,
        Some(_) => return None,
        None => raw,
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether a request for `host` (from [`host_part`]) may be served. A DNS-rebinding page always
/// names its own public domain, so only names an outside attacker cannot own are accepted: IP
/// literals, `localhost`, `.local` names (mDNS, never served by public DNS), the configured mDNS
/// name, and the names listed in `allowed_hosts` / `allowed_origins`.
pub(super) fn host_allowed(host: &str, cfg: &WebConfig) -> bool {
    if host.starts_with('[') || host.parse::<Ipv4Addr>().is_ok() || host == "localhost" {
        return true;
    }
    if host.len() > ".local".len() && host.ends_with(".local") {
        return true;
    }
    let mdns = cfg.mdns_hostname.trim().to_ascii_lowercase();
    if !mdns.is_empty() && host == mdns {
        return true;
    }
    cfg.allowed_hosts
        .iter()
        .filter_map(|h| host_part(h))
        .chain(
            cfg.allowed_origins
                .iter()
                .filter_map(|o| parse_origin(o))
                .map(|(_, h, _)| h),
        )
        .any(|h| h == host)
}

/// The authority the client asked for: the rightmost `X-Forwarded-Host` behind a trusted
/// proxy, else `Host`, else the request URI's authority (HTTP/2). A header that is not text
/// comes back as `"\0"`, which [`host_part`] refuses.
fn requested_host<'a>(headers: &'a HeaderMap, uri: &'a Uri, cfg: &WebConfig) -> Option<&'a str> {
    let forwarded = cfg
        .trust_forwarded_headers
        .then(|| headers.get("x-forwarded-host"))
        .flatten();
    match forwarded.or_else(|| headers.get(header::HOST)) {
        Some(v) => Some(
            v.to_str()
                .map(|h| h.rsplit(',').next().unwrap_or(h).trim())
                .unwrap_or("\u{0}"),
        ),
        None => uri.authority().map(|a| a.as_str()),
    }
}

/// The accepted host names, for the startup log.
fn accepted_hosts(cfg: &WebConfig) -> String {
    let mut names = vec![
        "IP addresses".to_owned(),
        "localhost".to_owned(),
        "*.local".to_owned(),
    ];
    let mdns = cfg.mdns_hostname.trim().to_ascii_lowercase();
    if !mdns.is_empty() {
        names.push(mdns);
    }
    names.extend(cfg.allowed_hosts.iter().filter_map(|h| host_part(h)));
    names.extend(
        cfg.allowed_origins
            .iter()
            .filter_map(|o| parse_origin(o))
            .map(|(_, h, _)| h),
    );
    names.dedup();
    names.join(", ")
}

/// Security headers of every response. `host` is the authority the client used (already
/// checked by [`host_guard`]), so this server's own WebSocket is the only `connect-src` besides
/// `'self'` (older Safari does not count `ws:` as `'self'`).
pub(super) fn security_headers(headers: &mut HeaderMap, host: Option<&str>) {
    let ws = host
        .filter(|h| {
            !h.is_empty()
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.:[]_".contains(&b))
        })
        .map(|h| format!(" ws://{h} wss://{h}"))
        .unwrap_or_default();
    let csp = format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; font-src 'self' data:; media-src 'self' blob:; \
         connect-src 'self'{ws}; object-src 'none'; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'"
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        headers.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
}

/// Runs before every route: refuses unknown host names (421), then adds [`security_headers`].
async fn host_guard(State(app): State<AppState>, req: Request, next: Next) -> Response {
    let raw = requested_host(req.headers(), req.uri(), &app.cfg).map(str::to_owned);
    if let Some(raw) = &raw {
        if !host_part(raw).is_some_and(|h| host_allowed(&h, &app.cfg)) {
            // Untrusted text: cut and escaped before it reaches the log.
            let shown: String = raw
                .chars()
                .take(64)
                .flat_map(char::escape_default)
                .collect();
            tracing::info!(host = %shown, "refused a request for an unknown host name (add it to [web] allowed_hosts if it is yours)");
            let mut resp = (
                StatusCode::MISDIRECTED_REQUEST,
                "unknown host name: add it to [web] allowed_hosts in config.toml",
            )
                .into_response();
            security_headers(resp.headers_mut(), None);
            return resp;
        }
    }
    let mut resp = next.run(req).await;
    security_headers(resp.headers_mut(), raw.as_deref());
    resp
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
    // Decided once, at the upgrade (the cookie comes with it).
    let admin = super::admin::is_admin(&headers, &app.cfg);
    let Some(slot) = app.ws_slots.take(ip) else {
        tracing::info!(%ip, "refused websocket: too many open connections");
        return (StatusCode::SERVICE_UNAVAILABLE, "too many open connections").into_response();
    };
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let id = app.engine.next_client_id();
            tracing::info!(client = id, %ip, "websocket connected");
            session(socket, app.engine, Origin::Web(id), admin).await;
            drop(slot);
            tracing::info!(client = id, %ip, "websocket disconnected");
        })
}

/// One text frame to a checked [`Command`]: size, no control characters (NUL, escape, ... only
/// JSON whitespace is allowed), the typed parse, then [`Command::validate`]. The error is the
/// text of the `error` event.
fn parse_ws_command(text: &str) -> Result<Command, String> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(format!("message is larger than {MAX_MESSAGE_BYTES} bytes"));
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err("invalid command: control characters are not allowed".into());
    }
    let command: Command =
        serde_json::from_str(text).map_err(|e| format!("invalid command: {e}"))?;
    command.validate()?;
    Ok(command)
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

/// One WebSocket client. `admin`: the device is paired (or the admin token is off), so it may
/// send the commands of [`super::admin::needs_admin`].
async fn session(mut socket: WebSocket, engine: EngineHandle, me: Origin, admin: bool) {
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
                    Message::Text(text) => match parse_ws_command(text.as_str()) {
                        Ok(cmd) if !admin && super::admin::needs_admin(&cmd) => {
                            let err = ServerEvent::Error { message: super::admin::NOT_PAIRED.into() };
                            if send_event(&mut socket, &err).await.is_err() {
                                break;
                            }
                        }
                        Ok(cmd) => {
                            if engine.command(me, cmd).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let err = ServerEvent::Error { message: e };
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
            ..WebConfig::default()
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

    #[test]
    fn host_names_an_outside_attacker_could_own_are_refused() {
        let cfg = WebConfig {
            mdns_hostname: "Cuemix-Web".into(),
            allowed_hosts: vec!["mixer.example.net".into(), "Proxy.Example.org:443".into()],
            allowed_origins: vec!["https://studio.example.com:8443".into()],
            ..WebConfig::default()
        };
        let ok = |raw: &str| host_part(raw).is_some_and(|h| host_allowed(&h, &cfg));
        for good in [
            "192.0.2.10:8080",
            "127.0.0.1",
            "[::1]:8080",
            "[FE80::1]",
            "localhost:8080",
            "cuemix-web.local",
            "CUEMIX-WEB.LOCAL.:8080",
            "other-box.local",
            "cuemix-web",
            "mixer.example.net:8080",
            "proxy.example.org",
            "studio.example.com",
        ] {
            assert!(ok(good), "{good}");
        }
        for bad in [
            "rebind.attacker.example",
            "rebind.attacker.example:8080",
            "cuemix-web.local.attacker.example",
            "localhost.attacker.example",
            ".local",
            "local",
            "127.0.0.1.nip.io",
            "mixer.example.net.attacker.example",
            "",
            "a b",
            "x:y:z",
            "127.0.0.1:70000",
            "127.0.0.1:",
            "[::1",
            "[::1]x",
            "[nothex]",
            "u@127.0.0.1",
            "127.0.0.1/x",
            "\u{0}",
        ] {
            assert!(!ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn security_headers_name_only_this_server_for_websockets() {
        let csp_of = |host: Option<&str>| {
            let mut h = HeaderMap::new();
            security_headers(&mut h, host);
            assert_eq!(
                h.get(header::X_FRAME_OPTIONS).map(|v| v.as_bytes()),
                Some(&b"DENY"[..])
            );
            h.get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let csp = csp_of(Some("cuemix-web.local:8080"));
        assert!(
            csp.contains(
                "connect-src 'self' ws://cuemix-web.local:8080 wss://cuemix-web.local:8080;"
            ),
            "{csp}"
        );
        assert!(csp.contains("frame-ancestors 'none'"));
        // A strange value never reaches the policy.
        let csp = csp_of(Some("x; script-src *"));
        assert!(csp.contains("connect-src 'self';"), "{csp}");
        assert!(!csp.contains("script-src *"), "{csp}");
    }

    #[test]
    fn websocket_frames_are_checked_before_dispatch() {
        assert!(parse_ws_command(r#"{"cmd":"list_presets"}"#).is_ok());
        assert!(parse_ws_command("{\"cmd\":\n\"list_presets\"}").is_ok());
        for bad in [
            "",
            "null",
            "[]",
            "{}",
            r#"{"cmd":"list_presets"} x"#,
            r#"{"cmd":"load_preset","name":"../x"}"#,
            r#"{"cmd":"load_preset","name":"a/b"}"#,
            r#"{"cmd":"save_preset","name":"a\u0000b"}"#,
            r#"{"cmd":"save_preset","name":"\u202egnp"}"#,
            r#"{"cmd":"import_preset","name":"..","text":"{}"}"#,
            r#"{"cmd":"set_label","ch":0,"label":{"name":"a\u202eb","color":null}}"#,
            r#"{"cmd":"set_label","ch":0,"label":{"name":"ok","color":"red"}}"#,
            r#"{"cmd":"set","param":"main_vol","db":"x"}"#,
            "{\"cmd\":\"list_presets\"}\0",
            "{\"cmd\":\"list_\x1bpresets\"}",
        ] {
            assert!(parse_ws_command(bad).is_err(), "{bad:?}");
        }
        let big = format!(
            r#"{{"cmd":"import_preset","name":"x","text":"{}"}}"#,
            "a".repeat(MAX_MESSAGE_BYTES)
        );
        assert!(parse_ws_command(&big).is_err());
    }
}
