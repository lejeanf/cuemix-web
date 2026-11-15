//! MOTU WebSocket client.
//!
//! - connects to `ws://<motu_ip>:<port>` (binary frames from [`super::protocol`]);
//! - on (re)connect applies **Option A - hardware priority**: a full state is
//!   rebuilt from the frames the device sends during a sync window, then
//!   [`DeviceEvent::Synced`] replaces the engine state. PROVISIONAL: the real
//!   handshake/dump is not documented yet (see `docs/MOTU-PROTOCOL.md`);
//!   until then every frame received during `sync_window` is applied on top of
//!   `MotuState::new(topology)`;
//! - throttles outgoing commands **per parameter** ([`super::throttle`]) and
//!   always delivers the last value;
//! - falls back to [`ConnectionStatus::Demo`] when the device is absent and
//!   retries with exponential backoff (1 s up to 10 s).

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, sleep_until, timeout, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use super::discovery;
use super::protocol::{Frame, ProtocolError, ValueKind, HEADER_LEN};
use super::throttle::Throttle;
use crate::config::Config;
use crate::engine::{DeviceEvent, DeviceLink, Shutdown};
use crate::state::{MotuState, Param, Topology};

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

/// Default time during which frames are collected to build the initial state.
pub const DEFAULT_SYNC_WINDOW: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Property table seen by the client (injectable for tests).
pub trait PropCodec: Send + Sync + 'static {
    /// Payload kind of a property, `None` if unknown.
    fn kind_of(&self, prop_id: u16) -> Option<ValueKind>;
    /// Effective param to wire frames (empty = no hardware mapping).
    fn encode(&self, param: &Param) -> Result<Vec<Frame>, ProtocolError>;
    /// Hardware frame to param, `None` if unmapped.
    fn decode(&self, frame: &Frame) -> Option<Param>;
}

/// Codec backed by [`super::props`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultCodec;

impl PropCodec for DefaultCodec {
    fn kind_of(&self, prop_id: u16) -> Option<ValueKind> {
        super::props::kind_of(prop_id)
    }
    fn encode(&self, param: &Param) -> Result<Vec<Frame>, ProtocolError> {
        super::props::encode(param)
    }
    fn decode(&self, frame: &Frame) -> Option<Param> {
        super::props::decode(frame)
    }
}

/// Everything the client needs, detached from [`Config`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub explicit_ip: Option<IpAddr>,
    pub port: u16,
    /// Interface carrying the MOTU link (ARP filter); `None` = any.
    pub interface: Option<String>,
    pub topology: Topology,
    pub throttle_interval: Duration,
    pub demo_fallback: bool,
    pub sync_window: Duration,
}

impl ClientConfig {
    pub fn from_config(cfg: &Config, topology: Topology) -> Self {
        let iface = cfg.network.motu_interface.trim();
        Self {
            explicit_ip: cfg.motu.ip,
            port: cfg.motu.port,
            interface: (!iface.is_empty()).then(|| iface.to_owned()),
            topology,
            throttle_interval: cfg.throttle_interval(),
            demo_fallback: cfg.engine.offline_demo_fallback,
            sync_window: DEFAULT_SYNC_WINDOW,
        }
    }
}

/// Run the client with the real property table until `shutdown` triggers.
pub async fn run(cfg: ClientConfig, link: DeviceLink, shutdown: Shutdown) {
    run_with_codec(cfg, DefaultCodec, link, shutdown).await;
}

/// Run the client with an injected codec (tests).
pub async fn run_with_codec<C: PropCodec>(
    cfg: ClientConfig,
    codec: C,
    mut link: DeviceLink,
    mut shutdown: Shutdown,
) {
    let mut status = StatusTracker::new(link.events.clone());
    let mut backoff = BACKOFF_START;
    loop {
        if shutdown.is_triggered() {
            return;
        }
        if !status.set(ConnectionStatus::Connecting).await {
            return;
        }
        match connect(&cfg, &mut link, &mut shutdown).await {
            Guarded::Stop => return,
            Guarded::Done(Some(ws)) => {
                backoff = BACKOFF_START;
                match session(&cfg, &codec, ws, &mut link, &mut shutdown, &mut status).await {
                    SessionEnd::Stop => return,
                    SessionEnd::Lost => {}
                }
            }
            Guarded::Done(None) => {}
        }
        if cfg.demo_fallback && !status.set(ConnectionStatus::Demo).await {
            return;
        }
        if let Guarded::Stop = guarded(sleep(backoff), &mut link, &mut shutdown).await {
            return;
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

enum Guarded<T> {
    Done(T),
    Stop,
}

/// Await `fut` while discarding engine commands (the engine must never block)
/// and watching for shutdown / engine termination.
async fn guarded<T>(
    fut: impl Future<Output = T>,
    link: &mut DeviceLink,
    shutdown: &mut Shutdown,
) -> Guarded<T> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return Guarded::Done(out),
            () = shutdown.wait() => return Guarded::Stop,
            cmd = link.commands.recv() => {
                if cmd.is_none() {
                    return Guarded::Stop;
                }
            }
        }
    }
}

async fn connect(
    cfg: &ClientConfig,
    link: &mut DeviceLink,
    shutdown: &mut Shutdown,
) -> Guarded<Option<Ws>> {
    let attempt = async {
        let ip = discovery::discover(cfg.explicit_ip, cfg.port, cfg.interface.as_deref()).await?;
        let url = format!("ws://{}", SocketAddr::new(ip, cfg.port));
        match timeout(CONNECT_TIMEOUT, connect_async(url.as_str())).await {
            Ok(Ok((ws, _))) => {
                tracing::info!(%url, "connected to MOTU");
                Some(ws)
            }
            Ok(Err(e)) => {
                tracing::debug!(%url, error = %e, "MOTU connect failed");
                None
            }
            Err(_) => {
                tracing::debug!(%url, "MOTU connect timed out");
                None
            }
        }
    };
    guarded(attempt, link, shutdown).await
}

/// Emits `DeviceEvent::Status` only when the status changes.
struct StatusTracker {
    tx: mpsc::Sender<DeviceEvent>,
    last: Option<ConnectionStatus>,
}

impl StatusTracker {
    fn new(tx: mpsc::Sender<DeviceEvent>) -> Self {
        Self { tx, last: None }
    }

    /// `false` when the engine is gone.
    async fn set(&mut self, status: ConnectionStatus) -> bool {
        if self.last == Some(status) {
            return true;
        }
        self.last = Some(status);
        self.tx.send(DeviceEvent::Status(status)).await.is_ok()
    }
}

enum SessionEnd {
    /// Shutdown requested or engine gone: leave `run`.
    Stop,
    /// Connection lost: reconnect.
    Lost,
}

/// Decode one raw binary message (never panics; unknown/malformed gives `None`).
fn decode_bytes<C: PropCodec>(codec: &C, bytes: &[u8]) -> Option<Param> {
    let Some(head) = bytes.get(..HEADER_LEN) else {
        tracing::debug!(len = bytes.len(), "frame shorter than header, skipped");
        return None;
    };
    let prop_id = u16::from_be_bytes([head[0], head[1]]);
    let Some(kind) = codec.kind_of(prop_id) else {
        tracing::debug!(prop_id, "unknown property, skipped");
        return None;
    };
    match Frame::decode(bytes, |_| kind) {
        Ok(frame) => {
            let param = codec.decode(&frame);
            if param.is_none() {
                tracing::debug!(prop_id, index = frame.index, "unmapped property, skipped");
            }
            param
        }
        Err(e) => {
            tracing::debug!(prop_id, error = %e, "malformed frame, skipped");
            None
        }
    }
}

/// `Err(())` when the socket is broken.
async fn send_param<C: PropCodec>(codec: &C, ws: &mut Ws, param: &Param) -> Result<(), ()> {
    let frames = match codec.encode(param) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(?param, error = %e, "cannot encode parameter, dropped");
            return Ok(());
        }
    };
    for frame in frames {
        match frame.encode() {
            Ok(bytes) => {
                if let Err(e) = ws.send(Message::Binary(bytes)).await {
                    tracing::warn!(error = %e, "send to MOTU failed");
                    return Err(());
                }
            }
            Err(e) => tracing::warn!(?frame, error = %e, "cannot encode frame, dropped"),
        }
    }
    Ok(())
}

async fn session<C: PropCodec>(
    cfg: &ClientConfig,
    codec: &C,
    mut ws: Ws,
    link: &mut DeviceLink,
    shutdown: &mut Shutdown,
    status: &mut StatusTracker,
) -> SessionEnd {
    let end = session_inner(cfg, codec, &mut ws, link, shutdown, status).await;
    let _ = timeout(Duration::from_millis(500), ws.close(None)).await;
    end
}

async fn session_inner<C: PropCodec>(
    cfg: &ClientConfig,
    codec: &C,
    ws: &mut Ws,
    link: &mut DeviceLink,
    shutdown: &mut Shutdown,
    status: &mut StatusTracker,
) -> SessionEnd {
    // --- Option A: full sync (PROVISIONAL, see module docs) ---------------
    let mut state = MotuState::new(cfg.topology);
    let sync_deadline = Instant::now() + cfg.sync_window;
    loop {
        tokio::select! {
            () = shutdown.wait() => return SessionEnd::Stop,
            () = sleep_until(sync_deadline) => break,
            cmd = link.commands.recv() => {
                // State is about to be replaced by the hardware's: drop writes.
                if cmd.is_none() {
                    return SessionEnd::Stop;
                }
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    if let Some(param) = decode_bytes(codec, &bytes) {
                        if let Err(e) = state.apply(&param) {
                            tracing::debug!(?param, error = %e, "sync frame rejected by state");
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return SessionEnd::Lost,
                Some(Ok(_)) => {}
            },
        }
    }
    if link
        .events
        .send(DeviceEvent::Synced(Box::new(state)))
        .await
        .is_err()
        || !status.set(ConnectionStatus::Online).await
    {
        return SessionEnd::Stop;
    }

    // --- steady state ------------------------------------------------------
    let mut throttle = Throttle::new(cfg.throttle_interval);
    loop {
        let deadline = throttle.next_deadline();
        let due = async {
            match deadline {
                Some(d) => sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = shutdown.wait() => return SessionEnd::Stop,
            () = due => {
                for param in throttle.flush_due(Instant::now()) {
                    if send_param(codec, ws, &param).await.is_err() {
                        return SessionEnd::Lost;
                    }
                }
            }
            cmd = link.commands.recv() => match cmd {
                None => return SessionEnd::Stop,
                Some(param) => {
                    if let Some(p) = throttle.push(Instant::now(), param) {
                        if send_param(codec, ws, &p).await.is_err() {
                            return SessionEnd::Lost;
                        }
                    }
                }
            },
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    if let Some(param) = decode_bytes(codec, &bytes) {
                        if link.events.send(DeviceEvent::Changed(vec![param])).await.is_err() {
                            return SessionEnd::Stop;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return SessionEnd::Lost,
                Some(Ok(_)) => {}
            },
        }
    }
}
