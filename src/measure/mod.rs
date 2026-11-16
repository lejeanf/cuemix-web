//! Room test: pink noise from the user's MPD, mixer made neutral, then put back.
//!
//! The card has no signal generator, so the noise comes from an MPD server
//! whose analog output is wired to a MOTU line input (`[measure]`). A phone
//! in the room records it; the analysis runs in the browser.
//!
//! Flow, driven by [`run`] (one test at a time):
//! 1. snapshot what will change: Main mix (bus 0) mute / solo of every input,
//!    Main output EQ bypass, input EQ bypass of the source inputs ([`plan`]);
//! 2. mute every other input, unmute the sources, clear solos, bypass those
//!    EQs - as ordinary `Set` commands, so the card and every client follow;
//! 3. MPD: save its status, pause the user's music (state `quiet`, 3.5 s: the
//!    phone records the background), queue the noise, play it, wait until it
//!    plays;
//! 4. hold until stop, the watchdog (`max_seconds`), an MPD failure or a
//!    device loss or the disconnection of the client that started the test;
//!    5. stop and remove the noise, restore the mixer values,
//!    then resume what MPD was playing.
//!
//! Any failure ends in the same restore and an `error` then `idle` state.
//!
//! The mixer restore list is written to `measure-restore.json` (next to
//! `labels.json`) before anything is changed, and kept until the restore has
//! been sent to an online MOTU and the link stayed up for [`CONFIRM`]. A
//! restore that could not reach the card (link lost, device unplugged,
//! service stopped or crashed mid-test) is therefore re-applied as soon as the
//! MOTU is online again, also after a restart. Values are written in a
//! "quiet first" order (mutes and solos that silence, then the rest, then
//! unmutes), so no input is briefly heard that should not be.

pub mod mpd;
pub mod noise;

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::config;
use crate::engine::{EngineHandle, MeasureRequest, Origin, Shutdown};
use crate::motu::client::ConnectionStatus;
use crate::state::{
    Command, MeasureState, MeasureStatus, MotuState, Param, ServerEvent, OUTPUT_EQ_BANDS,
};
use mpd::{Mpd, PlayState, Status};

/// Main mix.
const MAIN_BUS: u8 = 0;
/// Output EQ slot of Main L-R.
const MAIN_EQ_SLOT: u16 = 0;
/// MPD must confirm the noise plays within this time.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// Silence before the noise (the phone records the background).
const QUIET: Duration = Duration::from_millis(3_500);
/// While playing, MPD is asked for its status this often.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// The noise lasts [`noise::SECONDS`]; the watchdog stays safely below.
const MAX_SECONDS: u64 = 80;
/// A test needs about 17 s of noise (MPD start, settle, 10 s recording).
const MIN_SECONDS: u64 = 20;
/// A restore counts as delivered once the link stayed online this long after
/// it was sent (the card receives one frame per 20 ms).
const CONFIRM: Duration = Duration::from_secs(3);

/// Runtime settings derived from `[measure]` and `[network]`.
#[derive(Debug, Clone)]
pub struct Settings {
    pub mpd: String,
    pub password: String,
    pub noise_url: String,
    pub sources: Vec<u16>,
    pub max: Duration,
    pub http_port: u16,
    /// How long the room stays quiet before the noise starts.
    pub quiet: Duration,
    /// Where the pending mixer restore is kept across restarts (`None`: memory only).
    pub restore_file: Option<PathBuf>,
}

impl Settings {
    /// `None` when the feature is off (no MPD address, or no source input).
    pub fn from_config(cfg: &config::Measure, http_port: u16) -> Option<Self> {
        let mpd = cfg.mpd.trim();
        if mpd.is_empty() {
            return None;
        }
        if cfg.source_inputs.is_empty() {
            tracing::warn!("[measure] source_inputs is empty: room test disabled");
            return None;
        }
        Some(Self {
            mpd: mpd.to_owned(),
            password: cfg.mpd_password.clone(),
            noise_url: cfg.noise_url.trim().to_owned(),
            sources: cfg.source_inputs.clone(),
            max: Duration::from_secs(cfg.max_seconds.clamp(MIN_SECONDS, MAX_SECONDS)),
            http_port,
            quiet: QUIET,
            restore_file: None,
        })
    }

    /// Keep the pending mixer restore in `path` (survives a restart mid-test).
    #[must_use]
    pub fn with_restore_file(mut self, path: PathBuf) -> Self {
        self.restore_file = Some(path);
        self
    }

    /// Where MPD fetches the noise; `local` is our address on the MPD connection.
    fn noise_url(&self, local: IpAddr) -> String {
        if !self.noise_url.is_empty() {
            return self.noise_url.clone();
        }
        let host = match local {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        format!("http://{host}:{}/api/measure/noise.wav", self.http_port)
    }
}

/// What a test changes, and the values that put it back.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub apply: Vec<Param>,
    pub restore: Vec<Param>,
}

/// Compare `state` with the test setup. Only values that differ are listed,
/// so an input that was already muted is left alone and stays muted.
pub fn plan(state: &MotuState, sources: &[u16]) -> Plan {
    let mut plan = Plan::default();
    let mut change = |now: Param, then: Param, differs: bool| {
        if differs {
            plan.apply.push(now);
            plan.restore.push(then);
        }
    };
    if let Some(mix) = state.mixes.get(usize::from(MAIN_BUS)) {
        for (i, c) in mix.channels.iter().enumerate() {
            let Ok(ch) = u16::try_from(i) else { break };
            let want_mute = !sources.contains(&ch);
            let mute = |on| Param::MixMute {
                bus: MAIN_BUS,
                ch,
                on,
            };
            change(mute(want_mute), mute(c.mute), c.mute != want_mute);
            let solo = |on| Param::MixSolo {
                bus: MAIN_BUS,
                ch,
                on,
            };
            change(solo(false), solo(c.solo), c.solo);
        }
    }
    if let Some(bands) = state.output_eq.get(usize::from(MAIN_EQ_SLOT)) {
        for (band, b) in bands.iter().enumerate().take(OUTPUT_EQ_BANDS) {
            let Ok(band) = u8::try_from(band) else { break };
            let by = |on| Param::OutputEqBypass {
                ch: MAIN_EQ_SLOT,
                band,
                on,
            };
            change(by(true), by(b.bypass), !b.bypass);
        }
    }
    // Only analog inputs have an input EQ.
    for &ch in sources {
        let Some(bands) = state.input_eq.get(usize::from(ch)) else {
            continue;
        };
        for (band, b) in bands.iter().enumerate() {
            let Ok(band) = u8::try_from(band) else { break };
            let by = |on| Param::InputEqBypass { ch, band, on };
            change(by(true), by(b.bypass), !b.bypass);
        }
    }
    plan.apply.sort_by_key(loudness_rank);
    plan.restore.sort_by_key(loudness_rank);
    plan
}

/// Write order: what silences first, what makes something audible last, so
/// the card (one frame per 20 ms) never briefly plays an input it should not.
fn loudness_rank(p: &Param) -> u8 {
    match p {
        Param::MixMute { on: true, .. } | Param::MixSolo { on: true, .. } => 0,
        Param::MixSolo { on: false, .. } => 2,
        Param::MixMute { on: false, .. } => 3,
        _ => 1,
    }
}

/// RFC 3339 UTC, whole seconds (civil-from-days, H. Hinnant).
fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let rem = secs % 86_400;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// What to undo on the MPD side.
#[derive(Debug)]
struct MpdUndo {
    saved: Status,
    noise_id: Option<u32>,
    /// We paused the user's music for the quiet phase.
    paused_by_us: bool,
}

#[derive(Debug, Default)]
struct Undo {
    mixer: Vec<Param>,
    mpd: Option<MpdUndo>,
}

/// How [`Controller::wait_until`] ended (without error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waited {
    Elapsed,
    Stopped,
}

struct Controller {
    cfg: Settings,
    engine: EngineHandle,
    /// A device is identified.
    available: bool,
    /// The MOTU link is online (writes reach the card).
    online: bool,
    /// Mixer restore not yet known to have reached the card (also on disk).
    pending: Vec<Param>,
    /// When `pending` counts as delivered (sent while online, link still up).
    confirm_at: Option<Instant>,
}

/// Room-test task: register with the engine, then serve start / stop requests
/// until shutdown.
pub async fn run(cfg: Settings, engine: EngineHandle, mut shutdown: Shutdown) {
    let (tx, mut rx) = mpsc::channel(8);
    // Subscribe first so no device change is missed.
    let mut events = engine.subscribe();
    if engine.measure_attach(tx).await.is_err() {
        return;
    }
    let pending = load_pending(cfg.restore_file.as_ref()).await;
    if !pending.is_empty() {
        tracing::warn!(
            values = pending.len(),
            "room test: a previous test was not fully restored; restoring when the MOTU is online"
        );
    }
    let (available, online) = device_status(&engine).await;
    let mut ctl = Controller {
        available,
        online,
        pending,
        confirm_at: None,
        cfg,
        engine,
    };
    ctl.publish(ctl.resting(), None, None).await;
    ctl.flush_pending().await;
    loop {
        let confirm_at = ctl.confirm_at;
        let confirmed = async move {
            match confirm_at {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = shutdown.wait() => break,
            () = confirmed => ctl.confirmed().await,
            req = rx.recv() => match req {
                None => break,
                Some(MeasureRequest::Start) if ctl.available && ctl.online => {
                    ctl.test(&mut rx, &mut events, &mut shutdown).await;
                }
                Some(_) => {}
            },
            ev = events.recv() => match ev {
                Ok(env) => match env.event {
                    ServerEvent::Session(info) => ctl.set_available(info.device.is_some()).await,
                    ServerEvent::Status { connection } => {
                        ctl.set_online(connection == ConnectionStatus::Online).await;
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (present, online) = device_status(&ctl.engine).await;
                    ctl.set_available(present).await;
                    ctl.set_online(online).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

/// (a device is identified, the link is online).
async fn device_status(engine: &EngineHandle) -> (bool, bool) {
    let Ok(events) = engine.initial_events().await else {
        return (false, false);
    };
    events
        .iter()
        .find_map(|e| match e {
            ServerEvent::Snapshot { session, .. } => Some((
                session.device.is_some(),
                session.connection == ConnectionStatus::Online,
            )),
            _ => None,
        })
        .unwrap_or((false, false))
}

async fn load_pending(path: Option<&PathBuf>) -> Vec<Param> {
    let Some(path) = path else {
        return Vec::new();
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!(error = %e, path = %path.display(), "room test: unreadable restore file ignored");
            Vec::new()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "room test: restore file not read");
            Vec::new()
        }
    }
}

impl Controller {
    fn resting(&self) -> MeasureState {
        if self.available && self.online {
            MeasureState::Idle
        } else {
            MeasureState::Unavailable
        }
    }

    async fn set_available(&mut self, available: bool) {
        if self.available != available {
            let before = self.resting();
            self.available = available;
            if self.resting() != before {
                self.publish(self.resting(), None, None).await;
            }
        }
    }

    async fn set_online(&mut self, online: bool) {
        if self.online == online {
            return;
        }
        let before = self.resting();
        self.online = online;
        if online {
            self.flush_pending().await;
        } else {
            // Not confirmed: sent again when the link is back.
            self.confirm_at = None;
        }
        if self.resting() != before {
            self.publish(self.resting(), None, None).await;
        }
    }

    /// Send the pending restore when the card can receive it.
    async fn flush_pending(&mut self) {
        if self.pending.is_empty() || !self.online {
            return;
        }
        tracing::info!(
            values = self.pending.len(),
            "room test: restoring the mixer"
        );
        for p in self.pending.clone() {
            if self.set(p).await.is_err() {
                return;
            }
        }
        self.confirm_at = Some(Instant::now() + CONFIRM);
    }

    /// The link stayed up after the restore: forget it.
    async fn confirmed(&mut self) {
        self.confirm_at = None;
        if !self.online {
            return;
        }
        self.pending.clear();
        if let Some(path) = &self.cfg.restore_file {
            if let Err(e) = tokio::fs::remove_file(path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(error = %e, path = %path.display(), "room test: restore file not removed");
                }
            }
        }
    }

    /// Keep `restore` on disk before the mixer is touched.
    async fn save_pending(&self, restore: &[Param]) {
        let Some(path) = &self.cfg.restore_file else {
            return;
        };
        let written = match serde_json::to_vec(restore) {
            Ok(bytes) => tokio::fs::write(path, bytes)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = written {
            tracing::warn!(error = %e, path = %path.display(), "room test: restore file not written (kept in memory only)");
        }
    }

    async fn publish(&self, state: MeasureState, message: Option<String>, until: Option<String>) {
        let status = MeasureStatus {
            state,
            source: self.cfg.sources.clone(),
            message,
            until,
        };
        // Only fails when the engine is gone (shutdown).
        let _ = self.engine.measure_status(status).await;
    }

    async fn set(&self, param: Param) -> Result<(), String> {
        self.engine
            .command(Origin::System, Command::Set(param))
            .await
            .map_err(|_| "engine stopped".to_owned())
    }

    /// One whole test: setup, hold, restore. Always ends in `idle` (or
    /// `unavailable`), with an `error` first when something failed.
    async fn test(
        &mut self,
        rx: &mut mpsc::Receiver<MeasureRequest>,
        events: &mut broadcast::Receiver<crate::engine::Envelope>,
        shutdown: &mut Shutdown,
    ) {
        self.publish(MeasureState::Starting, None, None).await;
        // This test's snapshot is taken from the live state, which already
        // holds any earlier pending restore: it supersedes it.
        self.pending.clear();
        self.confirm_at = None;
        let mut undo = Undo::default();
        let outcome = self.hold(&mut undo, rx, events, shutdown).await;
        self.publish(MeasureState::Restoring, None, None).await;
        let shutting_down = shutdown.is_triggered();
        self.restore(undo, shutting_down).await;
        if let Err(message) = outcome {
            tracing::warn!(%message, "room test failed");
            self.publish(MeasureState::Error, Some(message), None).await;
        }
        self.publish(self.resting(), None, None).await;
    }

    async fn hold(
        &mut self,
        undo: &mut Undo,
        rx: &mut mpsc::Receiver<MeasureRequest>,
        events: &mut broadcast::Receiver<crate::engine::Envelope>,
        shutdown: &mut Shutdown,
    ) -> Result<(), String> {
        // 1-2. snapshot, then make the mixer neutral.
        let state = self.mixer_state().await?;
        let plan = plan(&state, &self.cfg.sources);
        self.save_pending(&plan.restore).await;
        undo.mixer = plan.restore;
        for p in plan.apply {
            self.set(p).await?;
        }
        // 3. the noise.
        let mut mpd = Mpd::connect(&self.cfg.mpd, &self.cfg.password)
            .await
            .map_err(|e| format!("cannot reach MPD: {e}"))?;
        let saved = mpd
            .status()
            .await
            .map_err(|e| format!("MPD status failed: {e}"))?;
        undo.mpd = Some(MpdUndo {
            saved,
            noise_id: None,
            paused_by_us: false,
        });
        let url = self.cfg.noise_url(mpd.local_ip());
        // A consumed song would vanish from the queue when interrupted; with
        // `single` on, the noise could loop if the restore cannot reach MPD.
        mpd.run("consume 0").await.map_err(|e| e.to_string())?;
        mpd.run("single 0").await.map_err(|e| e.to_string())?;
        // Quiet phase: the user's music out of the way, the phone records the room.
        if undo
            .mpd
            .as_ref()
            .is_some_and(|m| m.saved.state == PlayState::Play)
        {
            mpd.run("pause 1").await.map_err(|e| e.to_string())?;
            if let Some(m) = undo.mpd.as_mut() {
                m.paused_by_us = true;
            }
        }
        let until = rfc3339(SystemTime::now() + self.cfg.quiet);
        self.publish(MeasureState::Quiet, None, Some(until)).await;
        let quiet_end = Instant::now() + self.cfg.quiet;
        if self
            .wait_until(quiet_end, None, rx, events, shutdown)
            .await?
            == Waited::Stopped
        {
            return Ok(());
        }
        let id = mpd
            .addid(&url)
            .await
            .map_err(|e| format!("MPD could not load the noise from {url}: {e}"))?;
        if let Some(m) = undo.mpd.as_mut() {
            m.noise_id = Some(id);
        }
        mpd.run(&format!("playid {id}"))
            .await
            .map_err(|e| format!("MPD could not play the noise: {e}"))?;
        wait_playing(&mut mpd, id).await?;
        let until = rfc3339(SystemTime::now() + self.cfg.max);
        self.publish(MeasureState::Playing, None, Some(until)).await;

        // 4. hold; the watchdog ends it.
        let deadline = Instant::now() + self.cfg.max;
        if self
            .wait_until(deadline, Some((&mut mpd, id)), rx, events, shutdown)
            .await?
            == Waited::Elapsed
        {
            tracing::info!("room test: watchdog ended the test");
        }
        Ok(())
    }

    /// Wait until `deadline`, or until something ends the test: a stop
    /// request, shutdown or the loss of the MOTU (an `Err`). With `playing`,
    /// MPD is also polled and must keep playing that song.
    async fn wait_until(
        &mut self,
        deadline: Instant,
        mut playing: Option<(&mut Mpd, u32)>,
        rx: &mut mpsc::Receiver<MeasureRequest>,
        events: &mut broadcast::Receiver<crate::engine::Envelope>,
        shutdown: &mut Shutdown,
    ) -> Result<Waited, String> {
        let mut poll = tokio::time::interval_at(Instant::now() + POLL_EVERY, POLL_EVERY);
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => return Ok(Waited::Elapsed),
                () = shutdown.wait() => return Ok(Waited::Stopped),
                req = rx.recv() => match req {
                    Some(MeasureRequest::Stop) | None => return Ok(Waited::Stopped),
                    Some(MeasureRequest::Start) => {}
                },
                ev = events.recv() => match ev {
                    Ok(env) => match env.event {
                        ServerEvent::Session(info) if info.device.is_none() => {
                            self.available = false;
                            return Err("the MOTU disconnected".into());
                        }
                        ServerEvent::Status { connection } if connection != ConnectionStatus::Online => {
                            self.online = false;
                            return Err("the MOTU link was lost".into());
                        }
                        _ => {}
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(Waited::Stopped),
                },
                _ = poll.tick() => {
                    if let Some((mpd, id)) = playing.as_mut() {
                        let s = mpd.status().await.map_err(|e| format!("lost MPD: {e}"))?;
                        if s.state != PlayState::Play || s.songid != Some(*id) {
                            return Err("MPD stopped playing the noise".into());
                        }
                    }
                }
            }
        }
    }

    async fn mixer_state(&self) -> Result<MotuState, String> {
        let events = self
            .engine
            .initial_events()
            .await
            .map_err(|_| "engine stopped".to_owned())?;
        events
            .into_iter()
            .find_map(|e| match e {
                ServerEvent::Snapshot { state, .. } => Some(*state),
                _ => None,
            })
            .ok_or_else(|| "no mixer state".to_owned())
    }

    /// Stop the noise, put the mixer back, then resume what MPD was doing.
    /// Failures are logged: the test must still end. The mixer values stay
    /// pending (and on disk) until they are known to have reached the card.
    async fn restore(&mut self, undo: Undo, shutting_down: bool) {
        let mut conn = None;
        if let Some(m) = &undo.mpd {
            match Mpd::connect(&self.cfg.mpd, &self.cfg.password).await {
                Ok(mut c) => {
                    if let Some(id) = m.noise_id {
                        quiet(&mut c, "stop").await;
                        quiet(&mut c, &format!("deleteid {id}")).await;
                    }
                    restore_flags(&mut c, &m.saved).await;
                    conn = Some(c);
                }
                Err(e) => tracing::warn!(error = %e, "room test: MPD not restored"),
            }
        }
        for p in &undo.mixer {
            if let Err(e) = self.set(*p).await {
                tracing::warn!(error = %e, ?p, "room test: mixer not restored");
                break;
            }
        }
        if undo.mixer.is_empty() {
            // Nothing to put back: just drop the (empty) file.
            self.confirm_at = Some(Instant::now());
        } else {
            self.pending = undo.mixer;
            if self.online && !shutting_down {
                self.confirm_at = Some(Instant::now() + CONFIRM);
            } else {
                tracing::warn!(
                    "room test: the MOTU is not reachable; the mixer will be restored when it is back"
                );
            }
        }
        if let (Some(c), Some(m)) = (conn.as_mut(), &undo.mpd) {
            if m.noise_id.is_some() {
                resume(c, &m.saved).await;
            } else if m.paused_by_us {
                // Aborted in the quiet phase: the music is only paused.
                quiet(c, "pause 0").await;
            }
        }
    }
}

async fn wait_playing(mpd: &mut Mpd, id: u32) -> Result<(), String> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let s = mpd
            .status()
            .await
            .map_err(|e| format!("MPD status failed: {e}"))?;
        if s.state == PlayState::Play && s.songid == Some(id) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("MPD did not start the noise (can it reach noise_url?)".into());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Run a command whose failure is only worth a log line.
async fn quiet(mpd: &mut Mpd, command: &str) {
    if let Err(e) = mpd.run(command).await {
        tracing::warn!(%command, error = %e, "room test: MPD restore step failed");
    }
}

async fn restore_flags(mpd: &mut Mpd, saved: &Status) {
    for (name, value) in [
        ("consume", &saved.consume),
        ("single", &saved.single),
        ("repeat", &saved.repeat),
        ("random", &saved.random),
    ] {
        if let Ok(arg) = mpd::quote(value) {
            if !value.is_empty() {
                quiet(mpd, &format!("{name} {arg}")).await;
            }
        }
    }
}

/// Play the saved song again from where it was, paused if it was paused.
async fn resume(mpd: &mut Mpd, saved: &Status) {
    if saved.state == PlayState::Stop {
        return;
    }
    match (saved.songid, saved.song) {
        (Some(id), _) => quiet(mpd, &format!("playid {id}")).await,
        (None, Some(pos)) => quiet(mpd, &format!("play {pos}")).await,
        (None, None) => return,
    }
    if let (Some(id), Some(elapsed)) = (saved.songid, saved.elapsed) {
        quiet(mpd, &format!("seekid {id} {elapsed:.3}")).await;
    }
    if saved.state == PlayState::Pause {
        quiet(mpd, "pause 1").await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{self, DeviceEvent, EngineConfig, Envelope};
    use crate::state::{DeviceInfo, Topology};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    #[test]
    fn rfc3339_formats_utc() {
        let at = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(rfc3339(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(at(951_782_400 + 86_399)), "2000-02-29T23:59:59Z");
        assert_eq!(rfc3339(at(1_700_000_000)), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn noise_url_defaults_to_our_address_and_port() {
        let mut s = Settings {
            mpd: "192.0.2.20:6600".into(),
            password: String::new(),
            noise_url: String::new(),
            sources: vec![6, 7],
            max: Duration::from_secs(60),
            http_port: 8080,
            quiet: QUIET,
            restore_file: None,
        };
        let v4: IpAddr = "192.0.2.10".parse().unwrap();
        let v6: IpAddr = "2001:db8::10".parse().unwrap();
        assert_eq!(
            s.noise_url(v4),
            "http://192.0.2.10:8080/api/measure/noise.wav"
        );
        assert_eq!(
            s.noise_url(v6),
            "http://[2001:db8::10]:8080/api/measure/noise.wav"
        );
        s.noise_url = "http://noise.example.com/n.wav".into();
        assert_eq!(s.noise_url(v4), "http://noise.example.com/n.wav");
    }

    #[test]
    fn settings_need_an_mpd_and_clamp_the_watchdog() {
        let mut c = config::Measure::default();
        assert!(Settings::from_config(&c, 8080).is_none());
        c.mpd = "192.0.2.20:6600".into();
        c.max_seconds = 1000;
        assert_eq!(
            Settings::from_config(&c, 8080).map(|s| s.max),
            Some(Duration::from_secs(80))
        );
        c.source_inputs.clear();
        assert!(Settings::from_config(&c, 8080).is_none());
    }

    #[test]
    fn plan_mutes_others_and_restores_exactly() {
        let mut s = MotuState::new(Topology::default());
        // Input 2 already muted, source 6 muted, input 3 soloed, a source EQ band bypassed.
        s.mixes[0].channels[2].mute = true;
        s.mixes[0].channels[6].mute = true;
        s.mixes[0].channels[3].solo = true;
        s.input_eq[7][1].bypass = true;
        s.output_eq[0][2].bypass = true;
        let p = plan(&s, &[6, 7]);

        let mute = |ch, on| Param::MixMute { bus: 0, ch, on };
        assert!(
            !p.apply.contains(&mute(2, true)),
            "already muted: untouched"
        );
        assert!(p.apply.contains(&mute(0, true)) && p.apply.contains(&mute(31, true)));
        assert!(p.apply.contains(&mute(6, false)), "source unmuted");
        assert!(!p
            .apply
            .iter()
            .any(|x| matches!(x, Param::MixMute { ch: 7, .. })));
        assert!(p.apply.contains(&Param::MixSolo {
            bus: 0,
            ch: 3,
            on: false
        }));
        let by = |ch, band, on| Param::OutputEqBypass { ch, band, on };
        assert!(p.apply.contains(&by(0, 0, true)) && p.apply.contains(&by(0, 1, true)));
        assert!(!p
            .apply
            .iter()
            .any(|x| matches!(x, Param::OutputEqBypass { band: 2, .. })));
        assert!(p.apply.contains(&Param::InputEqBypass {
            ch: 6,
            band: 0,
            on: true
        }));
        assert!(!p.apply.contains(&Param::InputEqBypass {
            ch: 7,
            band: 1,
            on: true
        }));

        // Applying then restoring gives the original state back.
        let mut t = s.clone();
        for x in &p.apply {
            assert!(t.apply(x).is_ok());
        }
        assert_ne!(t, s);
        for x in &p.restore {
            assert!(t.apply(x).is_ok());
        }
        assert_eq!(t, s);
    }

    #[test]
    fn plan_ignores_sources_without_an_input_eq() {
        let s = MotuState::new(Topology::default());
        let p = plan(&s, &[20]); // USB: mixer input, not analog
        assert!(!p
            .apply
            .iter()
            .any(|x| matches!(x, Param::InputEqBypass { .. })));
        assert!(p.apply.contains(&Param::MixMute {
            bus: 0,
            ch: 6,
            on: true
        }));
    }

    #[test]
    fn plan_writes_what_silences_first() {
        let mut s = MotuState::new(Topology::default());
        // Solo on 3 (so 0-2 are silent), source 6 muted by the user.
        s.mixes[0].channels[3].solo = true;
        s.mixes[0].channels[6].mute = true;
        let p = plan(&s, &[6, 7]);
        let ranks = |v: &[Param]| v.iter().map(loudness_rank).collect::<Vec<_>>();
        for list in [&p.apply, &p.restore] {
            let r = ranks(list);
            assert!(r.windows(2).all(|w| w[0] <= w[1]), "{list:?}");
        }
        // Setup: every mute before the solo is cleared, the source unmuted last.
        assert_eq!(
            p.apply.last(),
            Some(&Param::MixMute {
                bus: 0,
                ch: 6,
                on: false
            })
        );
        // Restore: the source re-muted and the solo back before anything is unmuted.
        let first_unmute = p
            .restore
            .iter()
            .position(|x| matches!(x, Param::MixMute { on: false, .. }));
        let solo = p.restore.iter().position(|x| {
            *x == Param::MixSolo {
                bus: 0,
                ch: 3,
                on: true,
            }
        });
        let remute = p.restore.iter().position(|x| {
            *x == Param::MixMute {
                bus: 0,
                ch: 6,
                on: true,
            }
        });
        assert!(solo < first_unmute && remute < first_unmute);
    }

    // ---- fake MPD + engine ------------------------------------------------

    type Log = Arc<Mutex<Vec<String>>>;

    /// Serves one MPD session at a time: playing song 5 at 12.5 s, repeat on.
    async fn fake_mpd(log: Log) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            // Shared across connections, like a real server.
            let state = Arc::new(Mutex::new((
                "play".to_owned(),
                5u32,
                false, // noise queued
            )));
            while let Ok((sock, _)) = listener.accept().await {
                let (log, state) = (log.clone(), state.clone());
                tokio::spawn(async move {
                    let (r, mut w) = sock.into_split();
                    let _ = w.write_all(b"OK MPD 0.23.5\n").await;
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(cmd)) = lines.next_line().await {
                        log.lock().unwrap().push(cmd.clone());
                        let verb = cmd.split(' ').next().unwrap_or("").to_owned();
                        let reply = match verb.as_str() {
                            "status" => {
                                let (st, id, _) = state.lock().unwrap().clone();
                                format!("repeat: 1\nrandom: 0\nsingle: oneshot\nconsume: 1\nstate: {st}\nsong: 0\nsongid: {id}\nelapsed: 12.500\nOK\n")
                            }
                            "addid" => "Id: 99\nOK\n".to_owned(),
                            "playid" => {
                                let id = cmd
                                    .split(' ')
                                    .nth(1)
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0);
                                *state.lock().unwrap() = ("play".into(), id, id == 99);
                                "OK\n".to_owned()
                            }
                            "pause" => {
                                let on = cmd.ends_with(" 1");
                                state.lock().unwrap().0 = if on { "pause" } else { "play" }.into();
                                "OK\n".to_owned()
                            }
                            "stop" => {
                                state.lock().unwrap().0 = "stop".into();
                                "OK\n".to_owned()
                            }
                            _ => "OK\n".to_owned(),
                        };
                        if w.write_all(reply.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    struct Rig {
        engine: EngineHandle,
        rx: broadcast::Receiver<Envelope>,
        link: engine::DeviceLink,
        dir: std::path::PathBuf,
    }

    async fn rig(cfg: Settings) -> Rig {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("cuemix-measure-{}-{nanos}", std::process::id()));
        let (engine, link, _task) = engine::start(EngineConfig {
            topology: Topology::default(),
            presets_dir: dir.join("presets"),
            labels_file: dir.join("labels.json"),
        })
        .await;
        let info = DeviceInfo {
            topology: Topology::default(),
            ..Default::default()
        };
        let mut rx = engine.subscribe();
        assert!(link
            .events
            .send(DeviceEvent::Identified(info))
            .await
            .is_ok());
        assert!(link
            .events
            .send(DeviceEvent::Status(ConnectionStatus::Online))
            .await
            .is_ok());
        // Wait until the engine has processed both.
        loop {
            let env = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
            if let Ok(Ok(Envelope {
                event: ServerEvent::Status { connection },
                ..
            })) = env
            {
                if connection == ConnectionStatus::Online {
                    break;
                }
            }
        }
        let (_trigger, shutdown) = engine::shutdown_channel();
        std::mem::forget(_trigger); // keep the task alive for the whole test
        tokio::spawn(run(cfg, engine.clone(), shutdown));
        Rig {
            engine,
            rx,
            link,
            dir,
        }
    }

    fn settings(mpd: String, max: Duration) -> Settings {
        Settings {
            mpd,
            password: String::new(),
            noise_url: "http://noise.example.com/n.wav".into(),
            sources: vec![6, 7],
            max,
            http_port: 8080,
            quiet: Duration::from_millis(100),
            restore_file: None,
        }
    }

    /// Measure events until one has `until`; returns every status seen.
    async fn until_state(
        rx: &mut broadcast::Receiver<Envelope>,
        until: MeasureState,
    ) -> Vec<MeasureStatus> {
        let mut seen = Vec::new();
        loop {
            let env = tokio::time::timeout(Duration::from_secs(8), rx.recv())
                .await
                .expect("timeout")
                .expect("closed");
            if let ServerEvent::Measure(m) = env.event {
                let done = m.state == until;
                seen.push(m);
                if done {
                    return seen;
                }
            }
        }
    }

    async fn state_of(engine: &EngineHandle) -> MotuState {
        let events = engine.initial_events().await.unwrap();
        events
            .into_iter()
            .find_map(|e| match e {
                ServerEvent::Snapshot { state, .. } => Some(*state),
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn full_test_restores_mixer_and_mpd() {
        let log: Log = Arc::default();
        let addr = fake_mpd(log.clone()).await;
        let mut rig = rig(settings(addr, Duration::from_secs(30))).await;
        let o = Origin::Web(1);
        // Initial setup: input 2 muted, solo on 3, a bypassed band.
        for p in [
            Param::MixMute {
                bus: 0,
                ch: 2,
                on: true,
            },
            Param::MixSolo {
                bus: 0,
                ch: 3,
                on: true,
            },
            Param::InputEqBypass {
                ch: 6,
                band: 2,
                on: true,
            },
        ] {
            rig.engine.command(o, Command::Set(p)).await.unwrap();
        }
        let before = state_of(&rig.engine).await;
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await; // initial status

        rig.engine.command(o, Command::MeasureStart).await.unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Playing).await;
        let states: Vec<_> = seen.iter().map(|m| m.state).collect();
        assert_eq!(
            states,
            vec![
                MeasureState::Starting,
                MeasureState::Quiet,
                MeasureState::Playing
            ]
        );
        assert!(seen[1].until.as_deref().is_some_and(|u| u.ends_with('Z')));
        let playing = seen.last().unwrap();
        assert_eq!(playing.source, vec![6, 7]);
        assert!(playing.until.as_deref().is_some_and(|u| u.ends_with('Z')));

        let during = state_of(&rig.engine).await;
        assert!(during.mixes[0].channels[0].mute && !during.mixes[0].channels[6].mute);
        assert!(!during.mixes[0].channels[3].solo);
        assert!(during.output_eq[0].iter().all(|b| b.bypass));

        // A second start is refused to that client only.
        rig.engine
            .command(Origin::Web(2), Command::MeasureStart)
            .await
            .unwrap();
        rig.engine.command(o, Command::MeasureStop).await.unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Idle).await;
        let states: Vec<_> = seen.iter().map(|m| m.state).collect();
        assert_eq!(states, vec![MeasureState::Restoring, MeasureState::Idle]);

        assert_eq!(
            state_of(&rig.engine).await,
            before,
            "exactly what was there"
        );
        let log = log.lock().unwrap().clone();
        let pos = |needle: &str| log.iter().position(|l| l == needle);
        assert!(
            pos("pause 1") < pos("addid \"http://noise.example.com/n.wav\""),
            "music paused before the noise is queued: {log:?}"
        );
        assert!(pos("pause 1").is_some());
        assert!(
            pos("consume 0").is_some() && pos("addid \"http://noise.example.com/n.wav\"").is_some()
        );
        assert!(pos("deleteid 99").is_some());
        assert!(pos("consume \"1\"").is_some() && pos("single \"oneshot\"").is_some());
        let resume = log.iter().rposition(|l| l == "playid 5");
        assert!(
            resume.is_some() && pos("seekid 5 12.500").is_some(),
            "{log:?}"
        );
        assert!(
            pos("stop") < resume,
            "noise stopped before the song resumes"
        );
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[tokio::test]
    async fn stop_during_the_quiet_phase_unpauses_the_music() {
        let log: Log = Arc::default();
        let addr = fake_mpd(log.clone()).await;
        let mut cfg = settings(addr, Duration::from_secs(30));
        cfg.quiet = Duration::from_secs(10);
        let mut rig = rig(cfg).await;
        let before = state_of(&rig.engine).await;
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await;
        let o = Origin::Web(1);
        rig.engine.command(o, Command::MeasureStart).await.unwrap();
        let _ = until_state(&mut rig.rx, MeasureState::Quiet).await;
        rig.engine.command(o, Command::MeasureStop).await.unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Idle).await;
        let states: Vec<_> = seen.iter().map(|m| m.state).collect();
        assert_eq!(states, vec![MeasureState::Restoring, MeasureState::Idle]);
        assert_eq!(state_of(&rig.engine).await, before);
        let log = log.lock().unwrap().clone();
        assert!(log.iter().any(|l| l == "pause 1"), "{log:?}");
        assert!(log.iter().any(|l| l == "pause 0"), "unpaused: {log:?}");
        assert!(!log.iter().any(|l| l.starts_with("addid")), "{log:?}");
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[tokio::test]
    async fn only_the_starting_client_disconnecting_stops_the_test() {
        let log: Log = Arc::default();
        let addr = fake_mpd(log).await;
        let mut rig = rig(settings(addr, Duration::from_secs(30))).await;
        let before = state_of(&rig.engine).await;
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await;
        rig.engine
            .command(Origin::Web(1), Command::MeasureStart)
            .await
            .unwrap();
        let _ = until_state(&mut rig.rx, MeasureState::Playing).await;

        // Another client leaving changes nothing.
        rig.engine.client_gone(Origin::Web(2)).await.unwrap();
        let quiet = tokio::time::timeout(Duration::from_millis(400), async {
            loop {
                if let Ok(Envelope {
                    event: ServerEvent::Measure(m),
                    ..
                }) = rig.rx.recv().await
                {
                    return m;
                }
            }
        })
        .await;
        assert!(quiet.is_err(), "no measure event: {quiet:?}");

        // The starter leaving ends it.
        rig.engine.client_gone(Origin::Web(1)).await.unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Idle).await;
        let states: Vec<_> = seen.iter().map(|m| m.state).collect();
        assert_eq!(states, vec![MeasureState::Restoring, MeasureState::Idle]);
        assert_eq!(state_of(&rig.engine).await, before);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[tokio::test]
    async fn watchdog_ends_the_test() {
        let log: Log = Arc::default();
        let addr = fake_mpd(log).await;
        let mut rig = rig(settings(addr, Duration::from_millis(300))).await;
        let before = state_of(&rig.engine).await;
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await;
        rig.engine
            .command(Origin::Web(1), Command::MeasureStart)
            .await
            .unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Idle).await;
        let states: Vec<_> = seen.iter().map(|m| m.state).collect();
        assert_eq!(
            states,
            vec![
                MeasureState::Starting,
                MeasureState::Quiet,
                MeasureState::Playing,
                MeasureState::Restoring,
                MeasureState::Idle
            ]
        );
        assert_eq!(state_of(&rig.engine).await, before);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[tokio::test]
    async fn unreachable_mpd_ends_in_error_then_idle_with_everything_restored() {
        // A port nobody listens on.
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().to_string()
        };
        let mut rig = rig(settings(closed, Duration::from_secs(30))).await;
        let before = state_of(&rig.engine).await;
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await;
        rig.engine
            .command(Origin::Web(1), Command::MeasureStart)
            .await
            .unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Idle).await;
        let states: Vec<_> = seen.iter().map(|m| m.state).collect();
        assert_eq!(
            states,
            vec![
                MeasureState::Starting,
                MeasureState::Restoring,
                MeasureState::Error,
                MeasureState::Idle
            ]
        );
        let err = seen
            .iter()
            .find(|m| m.state == MeasureState::Error)
            .unwrap();
        assert!(
            err.message.as_deref().is_some_and(|m| m.contains("MPD")),
            "{err:?}"
        );
        assert_eq!(state_of(&rig.engine).await, before);
        let _ = std::fs::remove_dir_all(&rig.dir);
    }

    #[tokio::test]
    async fn link_lost_mid_test_restores_the_card_when_it_is_back() {
        let log: Log = Arc::default();
        let addr = fake_mpd(log).await;
        let file = std::env::temp_dir().join(format!(
            "cuemix-measure-restore-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let cfg = settings(addr, Duration::from_secs(30)).with_restore_file(file.clone());
        let mut rig = rig(cfg).await;
        let before = state_of(&rig.engine).await;
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await;
        rig.engine
            .command(Origin::Web(1), Command::MeasureStart)
            .await
            .unwrap();
        let _ = until_state(&mut rig.rx, MeasureState::Playing).await;
        assert!(file.exists(), "restore list on disk while the test runs");

        // The link drops: the restore cannot reach the card.
        rig.link
            .events
            .send(DeviceEvent::Status(ConnectionStatus::Connecting))
            .await
            .unwrap();
        let seen = until_state(&mut rig.rx, MeasureState::Unavailable).await;
        assert!(seen.iter().any(|m| m.state == MeasureState::Error));
        assert!(file.exists(), "kept until the card has it");
        while rig.link.commands.try_recv().is_ok() {}

        // Back: the card still has the test values ("hardware wins")...
        let muted = Param::MixMute {
            bus: 0,
            ch: 0,
            on: true,
        };
        rig.link
            .events
            .send(DeviceEvent::Synced(vec![muted]))
            .await
            .unwrap();
        rig.link
            .events
            .send(DeviceEvent::Status(ConnectionStatus::Online))
            .await
            .unwrap();
        let _ = until_state(&mut rig.rx, MeasureState::Idle).await;
        // ...and the restore is written again.
        let unmute = Param::MixMute {
            bus: 0,
            ch: 0,
            on: false,
        };
        let mut sent = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !sent.contains(&unmute) && Instant::now() < deadline {
            let next =
                tokio::time::timeout(Duration::from_millis(200), rig.link.commands.recv()).await;
            if let Ok(Some(p)) = next {
                sent.push(p);
            }
        }
        assert!(sent.contains(&unmute), "{sent:?}");
        assert_eq!(state_of(&rig.engine).await, before);
        // Confirmed after CONFIRM online: the file goes.
        tokio::time::sleep(CONFIRM + Duration::from_millis(500)).await;
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&rig.dir);
    }
}
