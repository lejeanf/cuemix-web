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
//!   to the device. `DeviceEvent::Synced` replaces the live state (Option A).
//! - Broadcast events carry their [`Origin`] (bridges may skip echoing a
//!   fader back to the client that is dragging it) and a [`Target`]
//!   (`Only(origin)` for replies and errors).
//! - The engine never blocks on the device: forwarding uses `try_send`.

use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::motu::client::ConnectionStatus;
use crate::presets::{self, PresetStore};
use crate::state::{Command, Labels, MotuState, Param, Preset, ServerEvent, SessionInfo, Topology};

/// Capacity of the client → engine request queue.
const REQUEST_QUEUE: usize = 1024;
/// Capacity of the engine → device queue (the client drains it continuously).
const DEVICE_QUEUE: usize = 1024;
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
    /// Full hardware state read after (re)connect — Option A, hardware wins.
    Synced(Box<MotuState>),
    /// Changes made on the hardware side.
    Changed(Vec<Param>),
    Meters(Vec<f32>),
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
    Command { origin: Origin, command: Command },
    Initial(oneshot::Sender<Vec<ServerEvent>>),
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
}

/// Spawn the engine task.
pub async fn start(cfg: EngineConfig) -> (EngineHandle, DeviceLink, JoinHandle<()>) {
    let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE);
    let (dev_cmd_tx, dev_cmd_rx) = mpsc::channel(DEVICE_QUEUE);
    let (dev_ev_tx, dev_ev_rx) = mpsc::channel(DEVICE_QUEUE);
    let (events, _) = broadcast::channel(EVENT_RING);

    let labels = match presets::load_labels(&cfg.labels_file).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "labels not loaded, starting empty");
            Labels::default()
        }
    };

    let engine = Engine {
        state: MotuState::new(cfg.topology),
        session: SessionInfo::default(),
        active: None,
        labels,
        labels_file: cfg.labels_file,
        store: PresetStore::new(cfg.presets_dir),
        events: events.clone(),
        to_device: dev_cmd_tx,
        _device_keepalive: dev_ev_tx.clone(),
    };
    let task = tokio::spawn(engine.run(req_rx, dev_ev_rx));
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

struct Engine {
    state: MotuState,
    session: SessionInfo,
    active: Option<Preset>,
    labels: Labels,
    labels_file: PathBuf,
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
    ) {
        loop {
            tokio::select! {
                req = requests.recv() => match req {
                    Some(Request::Command { origin, command }) => self.on_command(origin, command).await,
                    Some(Request::Initial(reply)) => {
                        let _ = reply.send(self.initial_events().await);
                    }
                    // Every handle dropped: shut down.
                    None => break,
                },
                Some(ev) = device.recv() => self.on_device(ev),
            }
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

    fn reply_error(&self, origin: Origin, message: String) {
        tracing::debug!(?origin, %message, "request rejected");
        self.emit(origin, Target::Only(origin), ServerEvent::Error { message });
    }

    fn snapshot(&self) -> ServerEvent {
        ServerEvent::Snapshot {
            state: Box::new(self.state.clone()),
            session: self.session.clone(),
        }
    }

    async fn initial_events(&self) -> Vec<ServerEvent> {
        let names = self.store.list().await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "cannot list presets");
            Vec::new()
        });
        vec![
            self.snapshot(),
            ServerEvent::Labels(self.labels.clone()),
            ServerEvent::Presets { names },
        ]
    }

    fn forward(&self, param: Param) {
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

    async fn on_command(&mut self, origin: Origin, command: Command) {
        match command {
            Command::Set(param) => match self.state.apply(&param) {
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
            },
            Command::LoadPreset { name } => match self.store.load(&name).await {
                Ok(preset) if preset.state.topology != self.state.topology => {
                    self.reply_error(
                        origin,
                        format!("preset {name:?} was saved for a different topology"),
                    );
                }
                Ok(preset) => {
                    for p in self.state.diff(&preset.state) {
                        self.forward(p);
                    }
                    self.state = preset.state.clone();
                    self.session.active_preset = Some(preset.name.clone());
                    self.session.modified = false;
                    self.active = Some(preset);
                    self.emit(origin, Target::All, self.snapshot());
                }
                Err(e) => self.reply_error(origin, format!("{e:#}")),
            },
            Command::SavePreset { name } => match self.store.save(&name, &self.state).await {
                Ok(preset) => {
                    self.session.active_preset = Some(preset.name.clone());
                    self.session.modified = false;
                    self.active = Some(preset);
                    self.emit(
                        origin,
                        Target::All,
                        ServerEvent::Session(self.session.clone()),
                    );
                    match self.store.list().await {
                        Ok(names) => self.emit(origin, Target::All, ServerEvent::Presets { names }),
                        Err(e) => tracing::warn!(error = %e, "cannot list presets"),
                    }
                }
                Err(e) => self.reply_error(origin, format!("{e:#}")),
            },
            Command::ListPresets => match self.store.list().await {
                Ok(names) => {
                    self.emit(origin, Target::Only(origin), ServerEvent::Presets { names })
                }
                Err(e) => self.reply_error(origin, format!("{e:#}")),
            },
            Command::SetLabel { ch, label } => {
                if ch >= self.state.topology.mixer_inputs {
                    self.reply_error(origin, format!("label channel {ch} out of range"));
                    return;
                }
                match label {
                    Some(l) => self.labels.channels.insert(ch, l),
                    None => self.labels.channels.remove(&ch),
                };
                if let Err(e) = presets::save_labels(&self.labels_file, &self.labels).await {
                    self.reply_error(origin, format!("labels not saved: {e:#}"));
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
            DeviceEvent::Synced(state) => {
                self.state = *state;
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
            DeviceEvent::Status(connection) => {
                if self.session.connection != connection {
                    self.session.connection = connection;
                    self.emit(origin, Target::All, ServerEvent::Status { connection });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        let cfg = EngineConfig {
            topology: Topology::default(),
            presets_dir: dir.join("presets"),
            labels_file: dir.join("labels.json"),
        };
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
        assert_eq!(
            link.commands.recv().await,
            Some(Param::MixVol {
                bus: 0,
                ch: 0,
                gain: crate::state::MIX_GAIN_MAX
            })
        );
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

        h.command(o, Command::Set(Param::Dim { on: true }))
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
        assert_eq!(link.commands.recv().await, Some(Param::Dim { on: true }));

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
        assert!(!state.control_room.dim);
        assert!(!session.modified);
        // The recall diff was forwarded to the device.
        assert_eq!(link.commands.recv().await, Some(Param::Dim { on: false }));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn hardware_sync_and_changes_are_not_echoed_to_device() {
        let (h, mut link, dir) = setup("hw").await;
        let mut rx = h.subscribe();
        let mut hw = MotuState::default();
        let _ = hw.apply(&Param::Mono { on: true });
        link.events
            .send(DeviceEvent::Synced(Box::new(hw)))
            .await
            .unwrap();
        let env = next(&mut rx).await;
        assert_eq!(env.origin, Origin::Hardware);
        let ServerEvent::Snapshot { state, .. } = env.event else {
            panic!("snapshot")
        };
        assert!(state.control_room.mono);

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
        assert_eq!(init.len(), 3);
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
