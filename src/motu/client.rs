//! MOTU WebSocket client.
//!
//! - connects to `ws://<motu_ip>:<port>` (binary frames from [`super::protocol`]);
//! - on (re)connect applies **Option A - hardware priority**: the device dump is
//!   collected during a sync window, then
//!   [`DeviceEvent::Synced`] carries every decoded param; the engine applies them
//!   on top of its state (unreported fields are kept). The device pushes its
//!   full state unprompted on connect (`docs/MOTU-PROTOCOL.md`): every frame
//!   is collected until `sync_window` passes without a non-meter frame (max 5 s);
//! - throttles outgoing commands **per parameter** ([`super::throttle`]) and
//!   always delivers the last value, then paces **every frame** through one
//!   aggregate [`Pacer`] (one frame per [`MIN_FRAME_SPACING`]) so the device
//!   never closes the socket on write bursts;
//! - tries every discovery candidate in turn ([`discovery::discover`]);
//! - falls back to [`ConnectionStatus::Demo`] when the device is absent and
//!   retries with exponential backoff (1 s up to 10 s); `Connecting` is only
//!   reported for the first attempt and after losing a connection.

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
use super::identify;
use super::protocol::{Frame, ProtocolError, ValueKind, HEADER_LEN};
pub use super::throttle::MIN_FRAME_SPACING;
use super::throttle::{Pacer, Throttle};
use crate::config::Config;
use crate::engine::{DeviceEvent, DeviceLink, Shutdown};
use crate::state::{DeviceInfo, Param, Topology};

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

/// Initial sync ends after this long without a non-meter frame.
pub const DEFAULT_SYNC_WINDOW: Duration = Duration::from_millis(500);
/// Upper bound of the initial sync (the device streams meters forever).
pub const SYNC_MAX: Duration = Duration::from_secs(5);
/// Meter stream property (motu-mk5-linux `METERS_ID`; format still unknown).
pub const METERS_PROP_ID: u16 = 6000;
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
    /// Identify the device from the raw frames of its initial dump (layout
    /// needed to decode the mix matrix). `None` = unknown, defaults stay.
    fn identify(&self, _frames: &[Vec<u8>]) -> Option<DeviceInfo> {
        None
    }
    /// Like [`PropCodec::decode`], with the identified device layout.
    fn decode_with(&self, frame: &Frame, _device: Option<&DeviceInfo>) -> Option<Param> {
        self.decode(frame)
    }
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
    fn identify(&self, frames: &[Vec<u8>]) -> Option<DeviceInfo> {
        super::identify::identify(frames)
    }
    fn decode_with(&self, frame: &Frame, device: Option<&DeviceInfo>) -> Option<Param> {
        match (frame.prop_id, device) {
            (
                identify::P_MIX_FADER
                | identify::P_MIX_PAN
                | identify::P_MIX_SOLO
                | identify::P_MIX_MUTE,
                Some(d),
            ) => identify::decode_mix(frame, d),
            _ => super::props::decode(frame),
        }
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
    // `Connecting` only for the first attempt and after losing a connection:
    // retries while offline stay `Demo` (no Demo <-> Connecting flapping).
    let mut announce_connecting = true;
    loop {
        if shutdown.is_triggered() {
            return;
        }
        if announce_connecting && !status.set(ConnectionStatus::Connecting).await {
            return;
        }
        announce_connecting = false;
        match connect(&cfg, &mut link, &mut shutdown).await {
            Guarded::Stop => return,
            Guarded::Done(Some(ws)) => {
                backoff = BACKOFF_START;
                match session(&cfg, &codec, ws, &mut link, &mut shutdown, &mut status).await {
                    SessionEnd::Stop => return,
                    SessionEnd::Lost => announce_connecting = true,
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

/// Client WebSocket type.
pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

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
        let candidates =
            discovery::discover(cfg.explicit_ip, cfg.port, cfg.interface.as_deref()).await;
        let addrs: Vec<SocketAddr> = candidates
            .into_iter()
            .map(|ip| SocketAddr::new(ip, cfg.port))
            .collect();
        connect_first(&addrs).await.map(|(_, ws)| ws)
    };
    guarded(attempt, link, shutdown).await
}

/// Try each address in order; the first successful WebSocket handshake wins,
/// failures fall through to the next candidate.
pub async fn connect_first(addrs: &[SocketAddr]) -> Option<(SocketAddr, Ws)> {
    for addr in addrs {
        let url = format!("ws://{addr}");
        match timeout(CONNECT_TIMEOUT, connect_async(url.as_str())).await {
            Ok(Ok((ws, _))) => {
                tracing::info!(%url, "connected to MOTU");
                return Some((*addr, ws));
            }
            Ok(Err(e)) => tracing::debug!(%url, error = %e, "MOTU connect failed"),
            Err(_) => tracing::debug!(%url, "MOTU connect timed out"),
        }
    }
    None
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
fn decode_bytes<C: PropCodec>(
    codec: &C,
    bytes: &[u8],
    device: Option<&DeviceInfo>,
) -> Option<Param> {
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
            let param = codec.decode_with(&frame, device);
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

/// Wire bytes of every frame of `param` (unencodable parts are logged and dropped).
fn encode_frames<C: PropCodec>(codec: &C, param: &Param) -> Vec<Vec<u8>> {
    let frames = match codec.encode(param) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(?param, error = %e, "cannot encode parameter, dropped");
            return Vec::new();
        }
    };
    frames
        .into_iter()
        .filter_map(|frame| match frame.encode_write() {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!(?frame, error = %e, "cannot encode frame, dropped");
                None
            }
        })
        .collect()
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
    // The device pushes its whole state unprompted on connect, then streams
    // meters forever (docs/MOTU-PROTOCOL.md): the sync ends after
    // `sync_window` without any non-meter frame, capped at SYNC_MAX.
    let mut dump: Vec<Vec<u8>> = Vec::new();
    let hard_deadline = Instant::now() + SYNC_MAX;
    let mut quiet_deadline = Instant::now() + cfg.sync_window;
    loop {
        tokio::select! {
            () = shutdown.wait() => return SessionEnd::Stop,
            () = sleep_until(quiet_deadline.min(hard_deadline)) => break,
            cmd = link.commands.recv() => {
                // State is about to be replaced by the hardware's: drop writes.
                if cmd.is_none() {
                    return SessionEnd::Stop;
                }
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    let is_meter = bytes.get(..2) == Some(&METERS_PROP_ID.to_be_bytes()[..]);
                    if !is_meter {
                        quiet_deadline = Instant::now() + cfg.sync_window;
                        dump.push(bytes.to_vec());
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return SessionEnd::Lost,
                Some(Ok(_)) => {}
            },
        }
    }
    if Instant::now() >= hard_deadline {
        tracing::warn!(
            "initial MOTU state dump did not settle within {SYNC_MAX:?}; using what was received"
        );
    }
    // Identify first: decoding the mix matrix needs the device layout.
    let device = codec.identify(&dump);
    if let Some(info) = &device {
        tracing::info!(
            model = %info.model,
            firmware = %info.firmware,
            mixes = info.topology.mixes,
            mixer_inputs = info.topology.mixer_inputs,
            "MOTU identified"
        );
        if link
            .events
            .send(DeviceEvent::Identified(info.clone()))
            .await
            .is_err()
        {
            return SessionEnd::Stop;
        }
    }
    let synced: Vec<Param> = dump
        .iter()
        .filter_map(|bytes| decode_bytes(codec, bytes, device.as_ref()))
        .collect();
    if link.events.send(DeviceEvent::Synced(synced)).await.is_err()
        || !status.set(ConnectionStatus::Online).await
    {
        return SessionEnd::Stop;
    }

    // --- steady state ------------------------------------------------------
    let mut throttle = Throttle::new(cfg.throttle_interval);
    let mut pacer = Pacer::new();
    loop {
        let throttle_deadline = throttle.next_deadline();
        let pacer_deadline = pacer.next_deadline(Instant::now());
        let throttle_due = async {
            match throttle_deadline {
                Some(d) => sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        let pacer_due = async {
            match pacer_deadline {
                Some(d) => sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = shutdown.wait() => return SessionEnd::Stop,
            () = throttle_due => {
                for param in throttle.flush_due(Instant::now()) {
                    pacer.push(param);
                }
            }
            () = pacer_due => {
                if let Some(bytes) = pacer.pop_due(Instant::now(), |p| encode_frames(codec, p)) {
                    let res = ws.send(Message::Binary(bytes.into())).await;
                    pacer.sent(Instant::now());
                    if let Err(e) = res {
                        tracing::warn!(error = %e, "send to MOTU failed");
                        return SessionEnd::Lost;
                    }
                }
            }
            cmd = link.commands.recv() => match cmd {
                None => return SessionEnd::Stop,
                Some(param) => {
                    if let Some(p) = throttle.push(Instant::now(), param) {
                        pacer.push(p);
                    }
                }
            },
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    if let Some(param) = decode_bytes(codec, &bytes, device.as_ref()) {
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
