//! Core engine: the single owner of [`MotuState`] (actor pattern).
//!
//! ```text
//!  Web / OSC ──EngineHandle::command()──►┐
//!                                        │  Engine task (sole writer of MotuState)
//!  Web / OSC ◄──broadcast<Envelope>──────┤
//!                                        │
//!  motu::client ◄──DeviceLink.commands───┤  effective Params to send (throttled there)
//!  motu::client ───DeviceLink.events────►┘  Synced / Changed / Meters / Status
//! ```
//!
//! Rules:
//! - Every request is validated by [`MotuState::apply`]; only the *effective*
//!   param is broadcast and forwarded to the device.
//! - Hardware-originated changes are applied and broadcast but never sent back
//!   to the device. `DeviceEvent::Synced` applies the hardware dump on top of
//!   the live state (Option A: hardware wins for every param it reports).
//! - Only params with a hardware mapping (`motu::props::is_mapped`) enter the
//!   device queue; a preset recall forwards every mapped param of the preset
//!   except trims and +48V (`Param::excluded_from_recall`).
//! - Broadcast events carry their [`Origin`] (bridges may skip echoing a
//!   fader back to the client that is dragging it) and a [`Target`]
//!   (`Only(origin)` for replies and errors).
//! - The engine never blocks on the device: forwarding uses `try_send`.
//! - Monitor / Phones on/off is a server-side fade (`power`, [`sequence`]): one
//!   `Option<Fade>` per channel and a timer branch in the loop; the speaker plug request
//!   (Main only) runs in a spawned task and comes back as an internal message.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::motu::client::ConnectionStatus;
use crate::motu::props;
use crate::osc::bridge::OscApply;
use crate::osc::settings::OscSettings;
use crate::plug::{self, PlugState, PlugTransport, SpeakerPlugSettings};
use crate::power::{self, FadeDir, PowerChannel, Remembered};
use crate::presets::{self, PresetStore};
use crate::state::{
    Command, DeviceInfo, Labels, MeasureState, MeasureStatus, MotuState, Param, Preset,
    ServerEvent, SessionInfo, Topology,
};

mod osc;
mod sequence;

/// Version of the short HTTP API (`/api/v1/*`), reported by `/api/health` as `api`.
pub const API_VERSION: u32 = 1;

/// Capacity of the client → engine request queue.
const REQUEST_QUEUE: usize = 1024;
/// Most presets kept on disk: a new name beyond this is refused (no client can fill the disk
/// with presets). Overwriting an existing preset is always allowed.
pub const MAX_PRESETS: usize = 200;
/// Capacity of the engine → device queue (the client drains it continuously).
/// Sized for a whole-state recall on an unidentified device: every mix param
/// (4 per input and bus) plus the output EQ (165 params) must fit.
const DEVICE_QUEUE: usize = 4096;
/// Capacity of the broadcast ring; slow receivers get `Lagged` and should
/// resynchronise with [`EngineHandle::initial_events`].
const EVENT_RING: usize = 4096;

/// Who caused an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A Web client, by connection id (allocate with [`EngineHandle::next_client_id`]).
    Web(u64),
    /// An OSC peer.
    Osc(SocketAddr),
    /// The MOTU itself.
    Hardware,
    /// The engine (preset recall, startup…).
    System,
}

/// Who should receive an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    All,
    Only(Origin),
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub origin: Origin,
    pub target: Target,
    pub event: ServerEvent,
}

impl Envelope {
    /// Should a client identified by `me` receive this envelope?
    pub fn is_for(&self, me: Origin) -> bool {
        match self.target {
            Target::All => true,
            Target::Only(o) => o == me,
        }
    }
}

/// Events produced by `motu::client`.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceEvent {
    /// Every param read during the initial dump after (re)connect — Option A:
    /// applied on top of the live state; fields the device did not report
    /// (unmapped ones) keep their value.
    Synced(Vec<Param>),
    /// What the device reported about itself, sent just before `Synced`.
    /// A different topology rebuilds the state to the device's dimensions.
    Identified(DeviceInfo),
    /// Changes made on the hardware side.
    Changed(Vec<Param>),
    Meters(Vec<f32>),
    /// Raw meter frame payload (see [`ServerEvent::MeterRaw`]).
    MeterRaw(Vec<u8>),
    /// Output patch table (property 5010, read-only), whole table.
    Patch(Vec<u8>),
    Status(ConnectionStatus),
}

/// The device side of the engine, owned by `motu::client`.
#[derive(Debug)]
pub struct DeviceLink {
    /// Effective params to write to the hardware (coalesce/throttle per param).
    pub commands: mpsc::Receiver<Param>,
    pub events: mpsc::Sender<DeviceEvent>,
}

#[derive(Debug, Error)]
#[error("engine stopped")]
pub struct EngineClosed;

/// Shared shutdown signal for every long-running task (`main` triggers it on
/// Ctrl-C / SIGTERM). Cloneable; `wait()` returns once shutdown is requested.
#[derive(Debug, Clone)]
pub struct Shutdown(tokio::sync::watch::Receiver<bool>);

/// Trigger side of [`Shutdown`].
#[derive(Debug)]
pub struct ShutdownTrigger(tokio::sync::watch::Sender<bool>);

pub fn shutdown_channel() -> (ShutdownTrigger, Shutdown) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (ShutdownTrigger(tx), Shutdown(rx))
}

impl ShutdownTrigger {
    pub fn trigger(&self) {
        let _ = self.0.send(true);
    }
}

impl Shutdown {
    /// Resolves when shutdown is requested (or the trigger is dropped).
    pub async fn wait(&mut self) {
        let _ = self.0.wait_for(|stop| *stop).await;
    }

    pub fn is_triggered(&self) -> bool {
        *self.0.borrow()
    }
}

#[derive(Debug)]
enum Request {
    Command {
        origin: Origin,
        command: Command,
    },
    Initial(oneshot::Sender<Vec<ServerEvent>>),
    /// From `measure`: register its control channel.
    MeasureAttach(mpsc::Sender<MeasureRequest>),
    /// From the OSC bridge: register the channel `OscApply` goes to.
    OscAttach(mpsc::Sender<OscApply>),
    /// From `measure`: the room-test state changed.
    MeasureStatus(MeasureStatus),
    /// A Web client disconnected.
    ClientGone(Origin),
    /// `GET /api/v1/summary`.
    Summary(oneshot::Sender<Summary>),
    /// `POST /api/v1/command`: runs like a client command, but the outcome goes back to the
    /// caller instead of to a WebSocket.
    Api {
        command: Command,
        reply: oneshot::Sender<Result<(), ApiError>>,
    },
}

/// A command refused through the HTTP API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// The thing asked for (a preset) does not exist.
    pub not_found: bool,
    pub message: String,
}

/// One channel of the API summary.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelSummary {
    /// Whole dB, `-100..=0`.
    pub db: f64,
    /// Level above the minimum.
    pub on: bool,
    pub fading: Option<FadeDir>,
    /// Speaker plug state (Main only, `null` when no plug is enabled).
    pub plug: Option<PlugState>,
    /// Whole dB an On fades back to while the channel is off or fading out; `null` while on.
    pub return_db: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PresetSummary {
    pub active: Option<String>,
    pub modified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MotuSummary {
    /// `online`, `connecting` or `demo`.
    pub status: ConnectionStatus,
    pub model: Option<String>,
    pub firmware: Option<String>,
}

/// `GET /api/v1/summary`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub api: u32,
    pub version: &'static str,
    pub main: ChannelSummary,
    pub phones: ChannelSummary,
    pub preset: PresetSummary,
    pub presets: Vec<String>,
    pub motu: MotuSummary,
}

/// What the engine forwards to the room-test task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureRequest {
    Start,
    Stop,
}

/// Cheap, cloneable handle used by the Web and OSC bridges.
#[derive(Debug, Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<Request>,
    events: broadcast::Sender<Envelope>,
    ids: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl EngineHandle {
    pub async fn command(&self, origin: Origin, command: Command) -> Result<(), EngineClosed> {
        self.tx
            .send(Request::Command { origin, command })
            .await
            .map_err(|_| EngineClosed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.events.subscribe()
    }

    /// What a newly connected client must receive first, in order:
    /// `Snapshot`, `Labels`, `Presets`.
    pub async fn initial_events(&self) -> Result<Vec<ServerEvent>, EngineClosed> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Request::Initial(tx))
            .await
            .map_err(|_| EngineClosed)?;
        rx.await.map_err(|_| EngineClosed)
    }

    /// Room test: register the channel `MeasureStart` / `MeasureStop` go to.
    pub async fn measure_attach(
        &self,
        tx: mpsc::Sender<MeasureRequest>,
    ) -> Result<(), EngineClosed> {
        self.tx
            .send(Request::MeasureAttach(tx))
            .await
            .map_err(|_| EngineClosed)
    }

    /// OSC bridge: register the channel live settings changes go to.
    pub async fn osc_attach(&self, tx: mpsc::Sender<OscApply>) -> Result<(), EngineClosed> {
        self.tx
            .send(Request::OscAttach(tx))
            .await
            .map_err(|_| EngineClosed)
    }

    /// Room test: publish its state to every client (and to later connects).
    pub async fn measure_status(&self, status: MeasureStatus) -> Result<(), EngineClosed> {
        self.tx
            .send(Request::MeasureStatus(status))
            .await
            .map_err(|_| EngineClosed)
    }

    /// A Web client's connection ended: a room test it started is stopped.
    pub async fn client_gone(&self, origin: Origin) -> Result<(), EngineClosed> {
        self.tx
            .send(Request::ClientGone(origin))
            .await
            .map_err(|_| EngineClosed)
    }

    /// Short-API summary (`GET /api/v1/summary`).
    pub async fn summary(&self) -> Result<Summary, EngineClosed> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Request::Summary(tx))
            .await
            .map_err(|_| EngineClosed)?;
        rx.await.map_err(|_| EngineClosed)
    }

    /// Short-API command (`POST /api/v1/command`): the real outcome, not an event.
    pub async fn api_command(
        &self,
        command: Command,
    ) -> Result<Result<(), ApiError>, EngineClosed> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request::Api { command, reply })
            .await
            .map_err(|_| EngineClosed)?;
        rx.await.map_err(|_| EngineClosed)
    }

    /// Unique id for a new Web connection.
    pub fn next_client_id(&self) -> u64 {
        self.ids.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub topology: Topology,
    pub presets_dir: PathBuf,
    pub labels_file: PathBuf,
    /// Remembered Monitor / Phones levels.
    pub power_file: PathBuf,
    /// Speaker plug settings saved from the UI (overrides `speaker_plug`).
    pub speaker_plug_file: PathBuf,
    /// Speaker plug defaults (`[speaker_plug]` in `config.toml`).
    pub speaker_plug: SpeakerPlugSettings,
    /// OSC settings saved from the UI (written when they change).
    pub osc_file: PathBuf,
    /// OSC settings in force at startup (what the bridge was bound with).
    pub osc: OscSettings,
    /// Why the saved OSC settings were not used, if so.
    pub osc_error: Option<String>,
    /// Target names of `osc` that did not resolve at startup (retried every minute).
    pub osc_unresolved: Vec<String>,
    /// How often unresolved target names are tried again.
    pub osc_retry_every: Duration,
}

impl EngineConfig {
    /// Every data file inside `dir` (tests, demos).
    pub fn for_dir(topology: Topology, dir: &Path) -> Self {
        Self {
            topology,
            presets_dir: dir.join("presets"),
            labels_file: dir.join("labels.json"),
            power_file: dir.join("power.json"),
            speaker_plug_file: dir.join("speaker-plug.json"),
            speaker_plug: SpeakerPlugSettings::default(),
            osc_file: dir.join("osc.json"),
            osc: OscSettings::default(),
            osc_error: None,
            osc_unresolved: Vec::new(),
            osc_retry_every: Duration::from_secs(60),
        }
    }
}

/// Messages the engine sends itself from the tasks it spawns (plug requests).
#[derive(Debug)]
enum Internal {
    /// Result of a plug request made by the power sequence.
    Switched {
        seq: u64,
        on: bool,
        result: Result<String, String>,
    },
    /// Result of `TestSpeakerPlug`.
    Tested {
        origin: Origin,
        on: bool,
        result: Result<String, String>,
    },
    /// Fence check of settings being saved.
    Checked {
        origin: Origin,
        settings: SpeakerPlugSettings,
        result: Result<(), String>,
        /// Order of the `set_speaker_plug` requests.
        ticket: u64,
    },
    /// The bridge applied (or refused) `set_osc_settings`, or a retry of unresolved targets
    /// (`Origin::System`).
    OscApplied {
        origin: Origin,
        done: Result<osc::OscDone, String>,
    },
    /// Once a minute: try the target names that did not resolve yet.
    OscTick,
}

/// Spawn the engine task.
pub async fn start(cfg: EngineConfig) -> (EngineHandle, DeviceLink, JoinHandle<()>) {
    start_with(cfg, Arc::new(plug::HttpPlug)).await
}

/// [`start`] with another way to reach the speaker plug (tests use a fake).
pub async fn start_with(
    cfg: EngineConfig,
    transport: Arc<dyn PlugTransport>,
) -> (EngineHandle, DeviceLink, JoinHandle<()>) {
    let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (dev_cmd_tx, dev_cmd_rx) = mpsc::channel(DEVICE_QUEUE);
    let (dev_ev_tx, dev_ev_rx) = mpsc::channel(DEVICE_QUEUE);
    let (events, _) = broadcast::channel(EVENT_RING);

    let labels = match presets::load_labels(&cfg.labels_file).await {
        Ok(mut l) => {
            // Keys outside the mixer's inputs can never be shown: dropped like invalid entries.
            l.channels.retain(|ch, _| *ch < cfg.topology.mixer_inputs);
            l
        }
        Err(e) => {
            tracing::warn!(error = %e, "labels not loaded, starting empty");
            Labels::default()
        }
    };
    let remembered = match power::load_remembered(&cfg.power_file).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "remembered levels not loaded, starting empty");
            Remembered::default()
        }
    };
    let plug_settings = initial_plug_settings(&cfg.speaker_plug_file, cfg.speaker_plug).await;
    let (internal_tx, internal_rx) = mpsc::unbounded_channel();
    let store = PresetStore::new(cfg.presets_dir);
    let preset_names = store.list().await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "cannot list presets");
        Vec::new()
    });

    let mut engine = Engine {
        state: MotuState::new(cfg.topology),
        session: SessionInfo::default(),
        active: None,
        labels,
        measure: MeasureStatus::default(),
        patch: Vec::new(),
        measure_tx: None,
        measure_owner: None,
        labels_file: cfg.labels_file,
        power: sequence::PowerState::new(remembered, cfg.power_file),
        plug: sequence::PlugRuntime::new(
            plug_settings,
            cfg.speaker_plug_file,
            transport,
            internal_tx.clone(),
        ),
        osc: osc::OscRuntime::new(
            cfg.osc,
            cfg.osc_file,
            cfg.osc_error,
            cfg.osc_unresolved,
            cfg.osc_retry_every,
        ),
        synced: false,
        internal: internal_tx,
        last_error: None,
        preset_names,
        store,
        events: events.clone(),
        to_device: dev_cmd_tx,
        _device_keepalive: dev_ev_tx.clone(),
    };
    // What a new client gets first is the baseline: only changes are broadcast after it.
    engine.remember_baseline_events();
    let task = tokio::spawn(engine.run(req_rx, dev_ev_rx, internal_rx));
    let handle = EngineHandle {
        tx: req_tx,
        events,
        ids: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
    };
    let link = DeviceLink {
        commands: dev_cmd_rx,
        events: dev_ev_tx,
    };
    (handle, link, task)
}

/// Saved settings win over `config.toml`; invalid ones are dropped with a warning.
async fn initial_plug_settings(
    file: &Path,
    from_config: SpeakerPlugSettings,
) -> SpeakerPlugSettings {
    let saved = match plug::load_settings(file).await {
        Ok(s) => s,
        // A file we cannot read must not silently bring back another plug: disabled.
        Err(e) => {
            tracing::warn!(error = %e, "speaker plug settings not loaded, plug disabled");
            return SpeakerPlugSettings::default();
        }
    };
    let (settings, origin) = match saved {
        Some(s) => (s, "speaker-plug.json"),
        None => (from_config, "config.toml"),
    };
    match settings.validate() {
        Ok(()) => settings,
        Err(e) => {
            tracing::warn!(error = %e, origin, "invalid speaker plug settings, plug disabled");
            SpeakerPlugSettings::default()
        }
    }
}

struct Engine {
    state: MotuState,
    session: SessionInfo,
    active: Option<Preset>,
    labels: Labels,
    /// Room-test state, replayed to every new client.
    measure: MeasureStatus,
    /// Last patch table read from the device (empty = unknown), replayed to new clients.
    patch: Vec<u8>,
    measure_tx: Option<mpsc::Sender<MeasureRequest>>,
    /// Web client that started the running room test.
    measure_owner: Option<Origin>,
    labels_file: PathBuf,
    /// Monitor / Phones fades, remembered levels, Main's plug sequence.
    power: sequence::PowerState,
    /// Speaker plug settings, state and transport.
    plug: sequence::PlugRuntime,
    /// OSC settings and the link to the bridge.
    osc: osc::OscRuntime,
    /// Results of the tasks the engine spawns (plug requests).
    internal: mpsc::UnboundedSender<Internal>,
    /// The state was read from the card since the link came up.
    synced: bool,
    /// Last rejection `reply_error` reported (read by the HTTP API path).
    last_error: Option<String>,
    /// Cached preset list (no disk I/O on every client connect, audit #12).
    preset_names: Vec<String>,
    store: PresetStore,
    events: broadcast::Sender<Envelope>,
    to_device: mpsc::Sender<Param>,
    /// Keeps the device-event channel open even if `motu::client` exits.
    _device_keepalive: mpsc::Sender<DeviceEvent>,
}

impl Engine {
    async fn run(
        mut self,
        mut requests: mpsc::Receiver<Request>,
        mut device: mpsc::Receiver<DeviceEvent>,
        mut internal: mpsc::UnboundedReceiver<Internal>,
    ) {
        loop {
            let deadline = self.next_deadline();
            tokio::select! {
                req = requests.recv() => match req {
                    Some(Request::Command { origin, command }) => self.on_command(origin, command).await,
                    Some(Request::Initial(reply)) => {
                        let _ = reply.send(self.initial_events().await);
                    }
                    Some(Request::Summary(reply)) => {
                        let _ = reply.send(self.summary());
                    }
                    Some(Request::Api { command, reply }) => {
                        let _ = reply.send(self.on_api(command).await);
                    }
                    Some(Request::MeasureAttach(tx)) => self.measure_tx = Some(tx),
                    Some(Request::OscAttach(tx)) => self.on_osc_attach(tx),
                    Some(Request::MeasureStatus(status)) => {
                        if matches!(status.state, MeasureState::Idle | MeasureState::Unavailable) {
                            self.measure_owner = None;
                        }
                        self.measure = status.clone();
                        self.emit(Origin::System, Target::All, ServerEvent::Measure(status));
                    }
                    Some(Request::ClientGone(origin)) => {
                        // Only the client that started the test ends it.
                        if self.measure_owner == Some(origin) {
                            self.measure_owner = None;
                            self.on_measure(origin, MeasureRequest::Stop);
                        }
                    }
                    // Every handle dropped: shut down.
                    None => break,
                },
                Some(ev) = device.recv() => self.on_device(ev),
                Some(msg) = internal.recv() => self.on_internal(msg).await,
                () = wait_until(deadline) => self.on_timer().await,
            }
            self.after_step().await;
        }
        tracing::debug!("engine stopped");
    }

    fn emit(&self, origin: Origin, target: Target, event: ServerEvent) {
        // No receivers is fine (e.g. no client connected yet).
        let _ = self.events.send(Envelope {
            origin,
            target,
            event,
        });
    }

    /// `SavePreset`: store the live state under `name` and make it active.
    async fn save_preset(&mut self, origin: Origin, name: &str) {
        match self.store.save(name, &self.state).await {
            Ok(preset) => {
                self.session.active_preset = Some(preset.name.clone());
                self.session.modified = false;
                if let Err(i) = self.preset_names.binary_search(&preset.name) {
                    self.preset_names.insert(i, preset.name.clone());
                }
                self.active = Some(preset);
                self.emit(
                    origin,
                    Target::All,
                    ServerEvent::Session(self.session.clone()),
                );
                self.emit(
                    origin,
                    Target::All,
                    ServerEvent::Presets {
                        names: self.preset_names.clone(),
                    },
                );
            }
            Err(e) => self.reply_error(origin, format!("{e:#}")),
        }
    }

    /// `Ok` when `name` exists already or there is room for one more preset.
    fn room_for(&self, name: &str) -> Result<(), String> {
        let exists = self
            .preset_names
            .binary_search_by(|n| n.as_str().cmp(name))
            .is_ok();
        if exists || self.preset_names.len() < MAX_PRESETS {
            Ok(())
        } else {
            Err(format!(
                "there are already {MAX_PRESETS} presets: delete or overwrite one first"
            ))
        }
    }

    fn reply_error(&mut self, origin: Origin, message: String) {
        tracing::debug!(?origin, %message, "request rejected");
        self.last_error = Some(message.clone());
        self.emit(origin, Target::Only(origin), ServerEvent::Error { message });
    }

    fn snapshot(&self) -> ServerEvent {
        ServerEvent::Snapshot {
            state: Box::new(self.state.clone()),
            session: self.session.clone(),
        }
    }

    async fn initial_events(&self) -> Vec<ServerEvent> {
        let mut events = vec![
            self.snapshot(),
            ServerEvent::Labels(self.labels.clone()),
            ServerEvent::Presets {
                names: self.preset_names.clone(),
            },
            ServerEvent::Measure(self.measure.clone()),
        ];
        events.push(self.power_event());
        events.push(self.plug_event());
        events.push(self.osc_event());
        if !self.patch.is_empty() {
            events.push(ServerEvent::Patch {
                sources: self.patch.clone(),
            });
        }
        events
    }

    fn forward(&self, param: Param) {
        if !props::is_mapped(&param) {
            return;
        }
        if let Err(e) = self.to_device.try_send(param) {
            tracing::warn!(error = %e, "device queue unavailable, param not forwarded");
        }
    }

    /// Recompute `● Modifié`; broadcast `Session` only when it flips.
    fn refresh_modified(&mut self, origin: Origin) {
        let modified = self
            .active
            .as_ref()
            .is_some_and(|p| p.is_modified(&self.state));
        if modified != self.session.modified {
            self.session.modified = modified;
            self.emit(
                origin,
                Target::All,
                ServerEvent::Session(self.session.clone()),
            );
        }
    }

    /// Recalls a preset: validates it, sends what differs to the device and
    /// makes it active.
    fn recall(&mut self, origin: Origin, preset: Preset) {
        let name = preset.name.clone();
        if preset.state.topology != self.state.topology {
            self.reply_error(
                origin,
                format!("preset {name:?} was saved for a different topology"),
            );
            return;
        }
        // Untrusted file: validate shape and clamp every value.
        let mut next = match preset.state.clone().normalized() {
            Ok(s) => s,
            Err(e) => {
                self.reply_error(origin, format!("preset {name:?} rejected: {e}"));
                return;
            }
        };
        // Trims and +48V are never recalled (maintainer ruling).
        next.keep_excluded_from(&self.state);
        // With an identified device the live state mirrors the
        // hardware (read back on connect): send only what differs.
        // Otherwise (demo, not yet synced) send every mapped param.
        let params = if self.session.device.is_some() {
            self.state.diff(&next)
        } else {
            next.all_params()
        };
        for p in params {
            if !p.excluded_from_recall() {
                self.forward(p);
            }
        }
        self.state = next;
        self.session.active_preset = Some(preset.name.clone());
        self.active = Some(preset);
        self.session.modified = false;
        self.emit(origin, Target::All, self.snapshot());
    }

    /// Imports a preset file sent by a client (same format as the files in
    /// the presets directory): checks it, stores it under `name`, recalls it.
    async fn import(&mut self, origin: Origin, name: &str, text: &str) {
        if let Err(e) = self.room_for(name) {
            return self.reply_error(origin, e);
        }
        let preset = match presets::parse_preset(name, text) {
            Ok(p) => p,
            Err(e) => return self.reply_error(origin, format!("cannot import {name:?}: {e:#}")),
        };
        if preset.state.topology != self.state.topology {
            return self.reply_error(
                origin,
                format!("preset {name:?} was saved for a different topology"),
            );
        }
        let state = match preset.state.normalized() {
            Ok(s) => s,
            Err(e) => return self.reply_error(origin, format!("preset {name:?} rejected: {e}")),
        };
        match self.store.save(name, &state).await {
            Ok(saved) => {
                if let Err(i) = self.preset_names.binary_search(&saved.name) {
                    self.preset_names.insert(i, saved.name.clone());
                }
                self.emit(
                    origin,
                    Target::All,
                    ServerEvent::Presets {
                        names: self.preset_names.clone(),
                    },
                );
                self.recall(origin, saved);
            }
            Err(e) => self.reply_error(origin, format!("{e:#}")),
        }
    }

    /// `MeasureStart` / `MeasureStop`: checked here, run by `measure`.
    fn on_measure(&mut self, origin: Origin, request: MeasureRequest) {
        if request == MeasureRequest::Start {
            match self.measure.state {
                MeasureState::Idle => {}
                MeasureState::Unavailable => {
                    return self.reply_error(origin, "room test is not available".into())
                }
                _ => return self.reply_error(origin, "a room test is already running".into()),
            }
        }
        let sent = self
            .measure_tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(request).is_ok());
        if !sent && request == MeasureRequest::Start {
            self.reply_error(origin, "room test is not available".into());
        } else if sent && request == MeasureRequest::Start && matches!(origin, Origin::Web(_)) {
            self.measure_owner = Some(origin);
        }
    }

    async fn on_command(&mut self, origin: Origin, command: Command) {
        // Last line of defence: whatever the entry point (WebSocket, HTTP API, OSC) already did.
        if let Err(e) = command.validate() {
            return self.reply_error(origin, e);
        }
        match command {
            Command::MeasureStart => self.on_measure(origin, MeasureRequest::Start),
            Command::MeasureStop => self.on_measure(origin, MeasureRequest::Stop),
            Command::Power { channel, on } => self.on_power(channel, on),
            Command::SetReturnLevel { channel, db } => self.on_return_level(channel, db),
            Command::SetSpeakerPlug { settings } => self.on_set_speaker_plug(origin, settings),
            Command::TestSpeakerPlug { on } => self.on_test_speaker_plug(origin, on),
            Command::SetOscSettings { settings } => self.on_set_osc_settings(origin, settings),
            Command::Set(param) => {
                let result =
                    crate::state::check_writable(&param).and_then(|()| self.state.apply(&param));
                // A volume set by a client (not by the fade) takes control of the knob.
                if result.is_ok() {
                    if let Some(channel) = volume_channel(&param) {
                        self.user_moved(channel);
                    }
                }
                match result {
                    Ok(Some(effective)) => {
                        self.forward(effective);
                        self.emit(
                            origin,
                            Target::All,
                            ServerEvent::Changed {
                                params: vec![effective],
                            },
                        );
                        self.refresh_modified(origin);
                    }
                    Ok(None) => {}
                    Err(e) => self.reply_error(origin, e.to_string()),
                }
            }
            Command::LoadPreset { name } => match self.store.load(&name).await {
                Ok(preset) => self.recall(origin, preset),
                Err(e) => self.reply_error(origin, format!("{e:#}")),
            },
            Command::ImportPreset { name, text } => self.import(origin, &name, &text).await,
            Command::SavePreset { name } => match self.room_for(&name) {
                Err(e) => self.reply_error(origin, e),
                Ok(()) => self.save_preset(origin, &name).await,
            },
            // Explicit request: re-read the directory and refresh the cache.
            Command::ListPresets => match self.store.list().await {
                Ok(names) => {
                    self.preset_names.clone_from(&names);
                    self.emit(origin, Target::Only(origin), ServerEvent::Presets { names })
                }
                Err(e) => self.reply_error(origin, format!("{e:#}")),
            },
            Command::SetLabel { ch, label } => {
                if ch >= self.state.topology.mixer_inputs {
                    self.reply_error(origin, format!("label channel {ch} out of range"));
                    return;
                }
                if let Some(Err(e)) = label.as_ref().map(|l| l.validate()) {
                    self.reply_error(origin, e);
                    return;
                }
                let previous = match label {
                    Some(l) => self.labels.channels.insert(ch, l),
                    None => self.labels.channels.remove(&ch),
                };
                if let Err(e) = presets::save_labels(&self.labels_file, &self.labels).await {
                    // Keep memory and disk consistent: roll back.
                    match previous {
                        Some(p) => self.labels.channels.insert(ch, p),
                        None => self.labels.channels.remove(&ch),
                    };
                    self.reply_error(origin, format!("labels not saved: {e:#}"));
                    return;
                }
                self.emit(
                    origin,
                    Target::All,
                    ServerEvent::Labels(self.labels.clone()),
                );
            }
        }
    }

    fn on_device(&mut self, event: DeviceEvent) {
        let origin = Origin::Hardware;
        match event {
            DeviceEvent::Identified(info) => {
                if info.topology != self.state.topology {
                    tracing::info!(
                        from = ?self.state.topology,
                        to = ?info.topology,
                        "device topology differs, state rebuilt"
                    );
                    self.state = MotuState::new(info.topology);
                }
                if self.session.device.as_ref() != Some(&info) {
                    self.session.device = Some(info);
                    self.emit(
                        origin,
                        Target::All,
                        ServerEvent::Session(self.session.clone()),
                    );
                }
            }
            DeviceEvent::Synced(params) => {
                self.synced = true;
                for p in &params {
                    match self.state.apply(p) {
                        Ok(Some(eff)) => self.hardware_moved(&eff),
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, ?p, "hardware param rejected during sync");
                        }
                    }
                }
                self.emit(origin, Target::All, self.snapshot());
                self.refresh_modified(origin);
            }
            DeviceEvent::Changed(params) => {
                let effective: Vec<Param> = params
                    .iter()
                    .filter_map(|p| match self.state.apply(p) {
                        Ok(eff) => eff,
                        Err(e) => {
                            tracing::warn!(error = %e, ?p, "hardware param rejected");
                            None
                        }
                    })
                    .collect();
                for p in &effective {
                    self.hardware_moved(p);
                }
                if !effective.is_empty() {
                    self.emit(
                        origin,
                        Target::All,
                        ServerEvent::Changed { params: effective },
                    );
                    self.refresh_modified(origin);
                }
            }
            DeviceEvent::Meters(levels) => {
                self.emit(origin, Target::All, ServerEvent::Meters { levels })
            }
            DeviceEvent::MeterRaw(raw) => {
                self.emit(origin, Target::All, ServerEvent::MeterRaw { raw })
            }
            DeviceEvent::Patch(sources) => {
                if self.patch != sources {
                    self.patch = sources.clone();
                    self.emit(origin, Target::All, ServerEvent::Patch { sources });
                }
            }
            DeviceEvent::Status(connection) => {
                if self.session.connection != connection {
                    self.session.connection = connection;
                    if connection != ConnectionStatus::Online {
                        self.link_lost();
                    }
                    self.emit(origin, Target::All, ServerEvent::Status { connection });
                    // Link lost (reconnecting) or demo mode: the patch read
                    // from the previous session may not describe the next
                    // device; the next session's dump re-sends it.
                    if connection != ConnectionStatus::Online && !self.patch.is_empty() {
                        self.patch.clear();
                        self.emit(
                            origin,
                            Target::All,
                            ServerEvent::Patch {
                                sources: Vec::new(),
                            },
                        );
                    }
                    if connection == ConnectionStatus::Demo && self.session.device.take().is_some()
                    {
                        self.emit(
                            origin,
                            Target::All,
                            ServerEvent::Session(self.session.clone()),
                        );
                    }
                }
            }
        }
    }
}

/// Sleep until `deadline` (never, when there is none).
async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// The power channel a volume param belongs to.
fn volume_channel(param: &Param) -> Option<PowerChannel> {
    match param {
        Param::MainVol { .. } => Some(PowerChannel::Main),
        Param::PhonesVol { .. } => Some(PowerChannel::Phones),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "cuemix-engine-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    async fn setup(tag: &str) -> (EngineHandle, DeviceLink, PathBuf) {
        let dir = tmp(tag);
        let cfg = EngineConfig::for_dir(Topology::default(), &dir);
        let (h, l, _task) = start(cfg).await;
        (h, l, dir)
    }

    async fn next(rx: &mut broadcast::Receiver<Envelope>) -> Envelope {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn set_broadcasts_and_forwards() {
        let (h, mut link, dir) = setup("set").await;
        let mut rx = h.subscribe();
        let o = Origin::Web(1);
        h.command(
            o,
            Command::Set(Param::MixVol {
                bus: 0,
                ch: 0,
                gain: 9.0,
            }),
        )
        .await
        .unwrap();
        let env = next(&mut rx).await;
        assert_eq!(env.origin, o);
        assert_eq!(env.target, Target::All);
        assert_eq!(
            env.event,
            ServerEvent::Changed {
                params: vec![Param::MixVol {
                    bus: 0,
                    ch: 0,
                    gain: crate::state::MIX_GAIN_MAX
                }]
            }
        );
        // Mix faders are mapped (the client encodes them with the device layout).
        assert_eq!(
            link.commands.try_recv().ok(),
            Some(Param::MixVol {
                bus: 0,
                ch: 0,
                gain: crate::state::MIX_GAIN_MAX
            })
        );
        // Params without a hardware counterpart are never queued (audit #2).
        h.command(o, Command::Set(Param::Dim { on: true }))
            .await
            .unwrap();
        let _ = next(&mut rx).await;
        assert!(link.commands.try_recv().is_err());
        h.command(o, Command::Set(Param::MainVol { db: -6.4 }))
            .await
            .unwrap();
        let _ = next(&mut rx).await;
        assert_eq!(
            link.commands.recv().await,
            Some(Param::MainVol { db: -6.0 }),
            "mapped params are forwarded with their effective value"
        );
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn new_presets_stop_at_the_cap_but_overwrites_still_work() {
        let (h, _link, dir) = setup("cap").await;
        tokio::fs::create_dir_all(dir.join("presets"))
            .await
            .unwrap();
        let state = MotuState::new(Topology::default());
        let text =
            serde_json::json!({ "schema_version": 2, "name": "x", "state": state }).to_string();
        for i in 0..MAX_PRESETS {
            tokio::fs::write(dir.join("presets").join(format!("p{i:03}.json")), &text)
                .await
                .unwrap();
        }
        let mut rx = h.subscribe();
        let o = Origin::Web(9);
        h.command(o, Command::ListPresets).await.unwrap();
        let ServerEvent::Presets { names } = next(&mut rx).await.event else {
            panic!("presets")
        };
        assert_eq!(names.len(), MAX_PRESETS);
        for cmd in [
            Command::SavePreset {
                name: "one-more".into(),
            },
            Command::ImportPreset {
                name: "one-more".into(),
                text: text.clone(),
            },
        ] {
            h.command(o, cmd).await.unwrap();
            let ServerEvent::Error { message } = next(&mut rx).await.event else {
                panic!("error")
            };
            assert!(message.contains("already 200 presets"), "{message}");
        }
        assert!(!dir.join("presets").join("one-more.json").exists());
        // Overwriting an existing name is fine.
        h.command(
            o,
            Command::SavePreset {
                name: "p000".into(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(next(&mut rx).await.event, ServerEvent::Session(_)));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn import_stores_and_recalls_a_preset_file() {
        let (h, _link, dir) = setup("import").await;
        let mut rx = h.subscribe();
        let o = Origin::Web(3);
        // Build a file the way the UI exports it: live state with Mono on.
        let mut state = MotuState::new(Topology::default());
        state.control_room.mono = true;
        let text =
            serde_json::json!({ "schema_version": 2, "name": "x", "state": state }).to_string();
        h.command(
            o,
            Command::ImportPreset {
                name: "imported".into(),
                text,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Presets {
                names: vec!["imported".into()]
            }
        );
        let ServerEvent::Snapshot { state, session } = next(&mut rx).await.event else {
            panic!("snapshot")
        };
        assert!(state.control_room.mono);
        assert_eq!(session.active_preset.as_deref(), Some("imported"));
        assert!(
            tokio::fs::metadata(dir.join("presets").join("imported.json"))
                .await
                .is_ok()
        );

        // Garbage is refused, to the sender only, and nothing is stored.
        h.command(
            o,
            Command::ImportPreset {
                name: "bad".into(),
                text: "{ nope".into(),
            },
        )
        .await
        .unwrap();
        let env = next(&mut rx).await;
        assert!(matches!(env.event, ServerEvent::Error { .. }));
        assert!(env.is_for(o) && !env.is_for(Origin::Web(4)));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn errors_go_only_to_origin() {
        let (h, _link, dir) = setup("err").await;
        let mut rx = h.subscribe();
        let o = Origin::Web(7);
        h.command(o, Command::Set(Param::Phantom { ch: 99, on: true }))
            .await
            .unwrap();
        let env = next(&mut rx).await;
        assert!(matches!(env.event, ServerEvent::Error { .. }));
        assert!(env.is_for(o));
        assert!(!env.is_for(Origin::Web(8)));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn preset_cycle_and_modified_flag() {
        let (h, mut link, dir) = setup("preset").await;
        let mut rx = h.subscribe();
        let o = Origin::System;
        h.command(
            o,
            Command::SavePreset {
                name: "base".into(),
            },
        )
        .await
        .unwrap();
        let ServerEvent::Session(s) = next(&mut rx).await.event else {
            panic!("session")
        };
        assert_eq!(s.active_preset.as_deref(), Some("base"));
        assert!(!s.modified);
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Presets {
                names: vec!["base".into()]
            }
        );

        h.command(o, Command::Set(Param::Mono { on: true }))
            .await
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            ServerEvent::Changed { .. }
        ));
        let ServerEvent::Session(s) = next(&mut rx).await.event else {
            panic!("session")
        };
        assert!(s.modified);
        assert_eq!(link.commands.recv().await, Some(Param::Mono { on: true }));
        // Trims and +48V changes never mark the preset as modified...
        h.command(o, Command::Set(Param::Phantom { ch: 0, on: true }))
            .await
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            ServerEvent::Changed { .. }
        ));
        assert_eq!(
            link.commands.recv().await,
            Some(Param::Phantom { ch: 0, on: true })
        );

        h.command(
            o,
            Command::LoadPreset {
                name: "base".into(),
            },
        )
        .await
        .unwrap();
        let ServerEvent::Snapshot { state, session } = next(&mut rx).await.event else {
            panic!("snapshot")
        };
        assert!(!state.control_room.mono);
        assert!(state.preamps[0].phantom, "...and are never recalled");
        assert!(!session.modified);
        // Every mapped, non-excluded param of the preset was sent.
        let mut sent = Vec::new();
        while let Ok(p) = link.commands.try_recv() {
            sent.push(p);
        }
        assert!(sent.contains(&Param::Mono { on: false }));
        assert!(sent.iter().all(|p| !p.excluded_from_recall()));
        assert!(sent.iter().all(props::is_mapped));
        // No device identified (demo): the whole mapped state is sent.
        assert!(sent.iter().any(|p| matches!(p, Param::MixVol { .. })));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn recall_with_an_identified_device_sends_only_the_difference() {
        let (h, mut link, dir) = setup("recall-diff").await;
        let mut rx = h.subscribe();
        let o = Origin::System;
        link.events
            .send(DeviceEvent::Identified(DeviceInfo {
                topology: Topology::ULTRALITE_MK5,
                ..DeviceInfo::default()
            }))
            .await
            .unwrap();
        let _ = next(&mut rx).await; // session with the device
        h.command(
            o,
            Command::SavePreset {
                name: "base".into(),
            },
        )
        .await
        .unwrap();
        let _ = next(&mut rx).await; // session
        let _ = next(&mut rx).await; // presets
        h.command(o, Command::Set(Param::Mono { on: true }))
            .await
            .unwrap();
        let _ = next(&mut rx).await; // changed
        let _ = next(&mut rx).await; // session (modified)
        while link.commands.try_recv().is_ok() {}
        h.command(
            o,
            Command::LoadPreset {
                name: "base".into(),
            },
        )
        .await
        .unwrap();
        loop {
            if matches!(next(&mut rx).await.event, ServerEvent::Snapshot { .. }) {
                break;
            }
        }
        let mut sent = Vec::new();
        while let Ok(p) = link.commands.try_recv() {
            sent.push(p);
        }
        assert_eq!(sent, vec![Param::Mono { on: false }]);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn identified_device_reshapes_state_and_reaches_the_session() {
        let (h, link, dir) = setup("ident").await;
        let mut rx = h.subscribe();
        let topology = Topology {
            mixes: 2,
            mixer_inputs: 4,
            ..Topology::ULTRALITE_MK5
        };
        let info = DeviceInfo {
            model: "TEST".into(),
            topology,
            ..DeviceInfo::default()
        };
        link.events
            .send(DeviceEvent::Identified(info.clone()))
            .await
            .unwrap();
        let ServerEvent::Session(session) = next(&mut rx).await.event else {
            panic!("session")
        };
        assert_eq!(session.device, Some(info));
        link.events.send(DeviceEvent::Synced(vec![])).await.unwrap();
        let ServerEvent::Snapshot { state, .. } = next(&mut rx).await.event else {
            panic!("snapshot")
        };
        assert_eq!(state.topology, topology);
        assert_eq!(state.mixes.len(), 2);
        assert_eq!(state.mixes[0].channels.len(), 4);
        // Back to demo: the device identity is dropped.
        link.events
            .send(DeviceEvent::Status(ConnectionStatus::Online))
            .await
            .unwrap();
        let _ = next(&mut rx).await;
        link.events
            .send(DeviceEvent::Status(ConnectionStatus::Demo))
            .await
            .unwrap();
        let _ = next(&mut rx).await;
        let ServerEvent::Session(session) = next(&mut rx).await.event else {
            panic!("session")
        };
        assert_eq!(session.device, None);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn hardware_sync_and_changes_are_not_echoed_to_device() {
        let (h, mut link, dir) = setup("hw").await;
        let mut rx = h.subscribe();
        h.command(Origin::System, Command::Set(Param::Dim { on: true }))
            .await
            .unwrap();
        let _ = next(&mut rx).await;
        link.events
            .send(DeviceEvent::Synced(vec![Param::Mono { on: true }]))
            .await
            .unwrap();
        let env = next(&mut rx).await;
        assert_eq!(env.origin, Origin::Hardware);
        let ServerEvent::Snapshot { state, .. } = env.event else {
            panic!("snapshot")
        };
        assert!(state.control_room.mono);
        assert!(
            state.control_room.dim,
            "fields the device did not report are kept"
        );

        link.events
            .send(DeviceEvent::Changed(vec![Param::Talkback { on: true }]))
            .await
            .unwrap();
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Changed {
                params: vec![Param::Talkback { on: true }]
            }
        );
        link.events
            .send(DeviceEvent::Status(ConnectionStatus::Online))
            .await
            .unwrap();
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Status {
                connection: ConnectionStatus::Online
            }
        );
        assert!(
            link.commands.try_recv().is_err(),
            "hardware changes must not loop back"
        );

        let init = h.initial_events().await.unwrap();
        // Snapshot, labels, presets, room-test state, power, speaker plug, OSC settings.
        assert_eq!(init.len(), 7);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn patch_is_broadcast_once_replayed_and_cleared_in_demo() {
        let (h, link, dir) = setup("patch").await;
        let mut rx = h.subscribe();
        let table = vec![46, 47, 78, 79];
        link.events
            .send(DeviceEvent::Patch(table.clone()))
            .await
            .unwrap();
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Patch {
                sources: table.clone()
            }
        );
        // Same table again: no new broadcast.
        link.events
            .send(DeviceEvent::Patch(table.clone()))
            .await
            .unwrap();
        let init = h.initial_events().await.unwrap();
        assert_eq!(init.len(), 8);
        assert_eq!(
            init[7],
            ServerEvent::Patch {
                sources: table.clone()
            }
        );
        link.events
            .send(DeviceEvent::Status(ConnectionStatus::Online))
            .await
            .unwrap();
        let _ = next(&mut rx).await; // status
                                     // Link lost: the patch is dropped until the next session re-reads it.
        link.events
            .send(DeviceEvent::Status(ConnectionStatus::Connecting))
            .await
            .unwrap();
        let _ = next(&mut rx).await; // status
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Patch {
                sources: Vec::new()
            }
        );
        link.events
            .send(DeviceEvent::Patch(table.clone()))
            .await
            .unwrap();
        let _ = next(&mut rx).await; // patch
        link.events
            .send(DeviceEvent::Status(ConnectionStatus::Demo))
            .await
            .unwrap();
        let _ = next(&mut rx).await; // status
        assert_eq!(
            next(&mut rx).await.event,
            ServerEvent::Patch {
                sources: Vec::new()
            }
        );
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn labels_are_persisted_and_bounded() {
        let (h, _link, dir) = setup("labels").await;
        let mut rx = h.subscribe();
        let label = crate::state::ChannelLabel {
            name: "Vox".into(),
            color: None,
        };
        h.command(
            Origin::System,
            Command::SetLabel {
                ch: 2,
                label: Some(label.clone()),
            },
        )
        .await
        .unwrap();
        let ServerEvent::Labels(l) = next(&mut rx).await.event else {
            panic!("labels")
        };
        assert_eq!(l.channels.get(&2), Some(&label));
        assert_eq!(
            presets::load_labels(&dir.join("labels.json")).await.ok(),
            Some(l)
        );
        h.command(
            Origin::System,
            Command::SetLabel {
                ch: 500,
                label: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            ServerEvent::Error { .. }
        ));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}
