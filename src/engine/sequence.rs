//! Monitor / Phones on/off and the speaker plug sequence (docs/IOS-PLAN.md §5.2, §5.2b).
//!
//! Part of the engine actor (it is a child module so it can use the engine's private state).
//! Everything here runs inside the engine loop: the fades are stepped by its timer branch and
//! the plug requests go through one worker task (never two in flight, the latest wish wins) that
//! reports back with an [`Internal`] message, so the engine never waits on the network.
//!
//! ```text
//!  Off:  fade out ─► (end at the minimum) ─► off_delay ─► plug off
//!  On:   plug on ─► ok: warm-up ─► fade in        fail: notice + fade in at once
//! ```
//!
//! The fade runs in any mode (a demo UI is still a UI), but the plug is only ever switched, and
//! a level only ever remembered, while the MOTU link is `Online` and its state has been read.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::{
    ApiError, ChannelSummary, Engine, Internal, MotuSummary, Origin, PresetSummary, Summary,
    Target, API_VERSION,
};
use crate::motu::client::ConnectionStatus;
use crate::plug::{self, PlugState, PlugTransport, SpeakerPlugSettings};
use crate::power::{self, ChannelPower, Fade, FadeDir, PowerChannel, Remembered};
use crate::state::{Command, NoticeLevel, Param, ServerEvent, TRIM_MIN_DB};

use std::path::PathBuf;
use std::sync::Arc;

/// Shortest wait between the end of the fade out and switching the plug off.
const MIN_OFF_DELAY: Duration = Duration::from_millis(50);
/// How long the steps of a finished fade still count as the hardware's own echoes.
const ECHO_GRACE: Duration = Duration::from_secs(2);

/// Where Main's plug sequence stands (it precedes or follows the fade).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    None,
    /// Plug-on request in flight.
    PlugOn,
    /// The plug answered; the speakers warm up, then the fade starts.
    Warmup {
        until: Instant,
    },
    /// The fade out ended; the plug goes off at `until`.
    OffDelay {
        until: Instant,
    },
}

/// Fades, remembered levels and Main's plug sequence.
pub(super) struct PowerState {
    fades: [Option<Fade>; 2],
    /// Steps of the fade that just ended or was reversed, with the time they stop counting.
    echoes: [Option<(Vec<f64>, Instant)>; 2],
    stage: Stage,
    remembered: Remembered,
    /// Saves `remembered` in the background (latest value wins, writes stay atomic).
    writer: watch::Sender<Remembered>,
    /// `remembered` changed and is not handed to the writer yet.
    dirty: bool,
    /// Last `power` event sent (a new one goes out only when it differs).
    last_event: Option<ServerEvent>,
}

impl PowerState {
    pub(super) fn new(remembered: Remembered, file: PathBuf) -> Self {
        let (writer, mut rx) = watch::channel(remembered);
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let value = *rx.borrow_and_update();
                if let Err(e) = power::save_remembered(&file, &value).await {
                    tracing::warn!(error = %e, "remembered levels not saved");
                }
            }
        });
        Self {
            fades: [None, None],
            echoes: [None, None],
            stage: Stage::None,
            remembered,
            writer,
            dirty: false,
            last_event: None,
        }
    }
}

/// What the plug worker is asked to do.
#[derive(Debug)]
enum WorkerCmd {
    /// Part of the power sequence; only the latest one is kept while another request runs.
    Switch {
        seq: u64,
        on: bool,
        epoch: u64,
        settings: SpeakerPlugSettings,
    },
    /// A manual test (always sent, in order).
    Test {
        origin: Origin,
        on: bool,
        epoch: u64,
        settings: SpeakerPlugSettings,
    },
    /// Forget the queued sequence requests.
    Reset,
}

fn queue_cmd(queue: &mut VecDeque<WorkerCmd>, cmd: WorkerCmd) {
    match cmd {
        WorkerCmd::Reset => queue.retain(|c| !matches!(c, WorkerCmd::Switch { .. })),
        WorkerCmd::Switch { .. } => {
            queue.retain(|c| !matches!(c, WorkerCmd::Switch { .. }));
            queue.push_back(cmd);
        }
        WorkerCmd::Test { .. } => queue.push_back(cmd),
    }
}

/// The one task that talks to the plug: a request at a time, in order; of the sequence's
/// requests only the latest wish is kept, and it is not sent when the plug is already known to
/// be in that state (same settings epoch).
fn spawn_worker(
    transport: Arc<dyn PlugTransport>,
    internal: mpsc::UnboundedSender<Internal>,
) -> mpsc::UnboundedSender<WorkerCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel::<WorkerCmd>();
    tokio::spawn(async move {
        let mut queue: VecDeque<WorkerCmd> = VecDeque::new();
        // (settings epoch, state) the plug is known to be in after our last success.
        let mut applied: Option<(u64, bool)> = None;
        loop {
            if queue.is_empty() {
                match rx.recv().await {
                    Some(c) => queue_cmd(&mut queue, c),
                    None => break,
                }
            }
            while let Ok(c) = rx.try_recv() {
                queue_cmd(&mut queue, c);
            }
            let msg = match queue.pop_front() {
                None | Some(WorkerCmd::Reset) => continue,
                Some(WorkerCmd::Switch {
                    seq,
                    on,
                    epoch,
                    settings,
                }) => {
                    let result = if applied == Some((epoch, on)) {
                        Ok("already applied".to_owned())
                    } else {
                        let r = transport.send(settings, on).await;
                        applied = r.is_ok().then_some((epoch, on));
                        r
                    };
                    Internal::Switched { seq, on, result }
                }
                Some(WorkerCmd::Test {
                    origin,
                    on,
                    epoch,
                    settings,
                }) => {
                    let result = transport.send(settings, on).await;
                    applied = result.is_ok().then_some((epoch, on));
                    Internal::Tested { origin, on, result }
                }
            };
            if internal.send(msg).is_err() {
                break;
            }
        }
    });
    tx
}

/// Speaker plug settings, state and way to reach it.
pub(super) struct PlugRuntime {
    pub(super) settings: SpeakerPlugSettings,
    file: PathBuf,
    state: PlugState,
    /// Id of the latest plug request made by the sequence; older answers are ignored.
    seq: u64,
    /// Changes with the settings: what the plug was known to be no longer holds.
    epoch: u64,
    /// Counts `set_speaker_plug` requests; an older save never replaces a newer one.
    ticket: u64,
    applied_ticket: u64,
    test_running: bool,
    worker: mpsc::UnboundedSender<WorkerCmd>,
    transport: Arc<dyn PlugTransport>,
    last_event: Option<ServerEvent>,
}

impl PlugRuntime {
    pub(super) fn new(
        settings: SpeakerPlugSettings,
        file: PathBuf,
        transport: Arc<dyn PlugTransport>,
        internal: mpsc::UnboundedSender<Internal>,
    ) -> Self {
        Self {
            settings,
            file,
            state: PlugState::Unknown,
            seq: 0,
            epoch: 0,
            ticket: 0,
            applied_ticket: 0,
            test_running: false,
            worker: spawn_worker(Arc::clone(&transport), internal),
            transport,
            last_event: None,
        }
    }
}

fn vol_param(channel: PowerChannel, db: f64) -> Param {
    match channel {
        PowerChannel::Main => Param::MainVol { db },
        PowerChannel::Phones => Param::PhonesVol { db },
    }
}

impl Engine {
    fn level(&self, channel: PowerChannel) -> f64 {
        match channel {
            PowerChannel::Main => self.state.main.db,
            PowerChannel::Phones => self.state.control_room.phones_db,
        }
    }

    fn plug_enabled(&self) -> bool {
        self.plug.settings.enabled
    }

    /// The link is `Online` and the state was read from the card: what is shown is real.
    fn hw_ready(&self) -> bool {
        self.session.connection == ConnectionStatus::Online && self.synced
    }

    fn notice(&self, level: NoticeLevel, message: &str) {
        self.emit(
            Origin::System,
            Target::All,
            ServerEvent::Notice {
                level,
                message: message.to_owned(),
            },
        );
    }

    // ------------------------------------------------------------ events ---

    fn channel_power(&self, channel: PowerChannel) -> ChannelPower {
        let main = channel == PowerChannel::Main;
        let fading = self.power.fades[channel.index()]
            .as_ref()
            .map(|f| f.dir)
            .or_else(|| {
                (main && matches!(self.power.stage, Stage::PlugOn | Stage::Warmup { .. }))
                    .then_some(FadeDir::In)
            });
        ChannelPower {
            on: !power::is_off(self.level(channel)),
            fading,
            plug: (main && self.plug_enabled()).then_some(self.plug.state),
        }
    }

    pub(super) fn power_event(&self) -> ServerEvent {
        ServerEvent::Power {
            main: self.channel_power(PowerChannel::Main),
            phones: self.channel_power(PowerChannel::Phones),
        }
    }

    pub(super) fn plug_event(&self) -> ServerEvent {
        ServerEvent::SpeakerPlug {
            urls: self.plug.settings.urls_or_empty(),
            settings: self.plug.settings.clone(),
            state: self.plug.state,
        }
    }

    pub(super) fn summary(&self) -> Summary {
        let channel = |c: PowerChannel| {
            let p = self.channel_power(c);
            ChannelSummary {
                db: self.level(c),
                on: p.on,
                fading: p.fading,
                plug: p.plug,
            }
        };
        let device = self.session.device.as_ref();
        Summary {
            api: API_VERSION,
            version: env!("CARGO_PKG_VERSION"),
            main: channel(PowerChannel::Main),
            phones: channel(PowerChannel::Phones),
            preset: PresetSummary {
                active: self.session.active_preset.clone(),
                modified: self.session.modified,
            },
            presets: self.preset_names.clone(),
            motu: MotuSummary {
                status: self.session.connection,
                model: device.map(|d| d.model.clone()).filter(|m| !m.is_empty()),
                firmware: device.map(|d| d.firmware.clone()).filter(|f| !f.is_empty()),
            },
        }
    }

    /// The `power` / `speaker_plug` events a connecting client gets are the baseline for
    /// [`Self::after_step`].
    pub(super) fn remember_baseline_events(&mut self) {
        self.power.last_event = Some(self.power_event());
        self.plug.last_event = Some(self.plug_event());
    }

    /// End of every loop turn: send `power` / `speaker_plug` when they changed, hand the
    /// remembered levels to the writer when they did.
    pub(super) async fn after_step(&mut self) {
        let power = self.power_event();
        if self.power.last_event.as_ref() != Some(&power) {
            self.power.last_event = Some(power.clone());
            self.emit(Origin::System, Target::All, power);
        }
        let plug = self.plug_event();
        if self.plug.last_event.as_ref() != Some(&plug) {
            self.plug.last_event = Some(plug.clone());
            self.emit(Origin::System, Target::All, plug);
        }
        if self.power.dirty {
            self.power.dirty = false;
            let _ = self.power.writer.send(self.power.remembered);
        }
    }

    // ----------------------------------------------------------- API path ---

    /// `POST /api/v1/command`: run it like a client command and report its real outcome.
    pub(super) async fn on_api(&mut self, command: Command) -> Result<(), ApiError> {
        self.last_error = None;
        // An unknown (but well-formed) preset name is "not found"; a malformed one is the
        // caller's mistake.
        let missing = matches!(&command, Command::LoadPreset { name }
            if crate::presets::validate_name(name).is_ok() && !self.preset_names.contains(name));
        self.on_command(Origin::System, command).await;
        match self.last_error.take() {
            None => Ok(()),
            Some(message) => Err(ApiError {
                not_found: missing,
                message,
            }),
        }
    }

    // -------------------------------------------------------------- fades ---

    /// Write one fade step as a server-originated change, so every client sees the knob move.
    fn write_level(&mut self, channel: PowerChannel, db: f64) {
        match self.state.apply(&vol_param(channel, db)) {
            Ok(Some(effective)) => {
                self.forward(effective);
                self.emit(
                    Origin::System,
                    Target::All,
                    ServerEvent::Changed {
                        params: vec![effective],
                    },
                );
                self.refresh_modified(Origin::System);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "fade step rejected"),
        }
    }

    fn start_fade(&mut self, channel: PowerChannel, dir: FadeDir, from: f64, plug_off_after: bool) {
        let i = channel.index();
        let plan = match dir {
            FadeDir::Out => power::plan_out(from),
            FadeDir::In => power::plan_in(from, self.power.remembered.get(channel)),
        };
        let now = Instant::now();
        // A reversal keeps recognising the echoes of the steps already written.
        let carried = match (&self.power.fades[i], &self.power.echoes[i]) {
            (Some(f), _) => f.recent_steps(),
            (None, Some((steps, until))) if *until > now => steps.clone(),
            _ => Vec::new(),
        };
        let mut fade = Fade::new(dir, plan, now, self.level(channel), plug_off_after);
        fade.seed_recent(&carried);
        self.power.echoes[i] = None;
        self.power.fades[i] = Some(fade);
        self.tick_channel(channel, now);
    }

    fn tick_channel(&mut self, channel: PowerChannel, now: Instant) {
        let i = channel.index();
        let Some(mut fade) = self.power.fades[i].take() else {
            return;
        };
        let tick = fade.tick(now);
        if let Some(v) = tick.write {
            self.write_level(channel, v);
        }
        if !tick.done {
            self.power.fades[i] = Some(fade);
            return;
        }
        // Late echoes of its last steps are still the hardware's own.
        self.power.echoes[i] = Some((fade.recent_steps(), now + ECHO_GRACE));
        match fade.dir {
            // The remembered level is used once: a later Off remembers afresh.
            FadeDir::In => self.forget(channel),
            FadeDir::Out => {
                if channel == PowerChannel::Main
                    && fade.plug_off_after
                    && self.plug_enabled()
                    && self.hw_ready()
                {
                    self.begin_off_delay();
                }
            }
        }
    }

    /// Remember a level for the next On: only a level read from the card counts.
    fn remember(&mut self, channel: PowerChannel, db: f64) {
        if !self.hw_ready() {
            return;
        }
        self.power.remembered.set(channel, Some(db.round()));
        self.power.dirty = true;
    }

    fn forget(&mut self, channel: PowerChannel) {
        if self.hw_ready() && self.power.remembered.get(channel).is_some() {
            self.power.remembered.set(channel, None);
            self.power.dirty = true;
        }
    }

    /// When the engine must wake up next (a fade step or a plug timer).
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let stage = match self.power.stage {
            Stage::Warmup { until } | Stage::OffDelay { until } => Some(until),
            Stage::None | Stage::PlugOn => None,
        };
        self.power
            .fades
            .iter()
            .flatten()
            .map(Fade::next_deadline)
            .chain(stage)
            .min()
    }

    pub(super) async fn on_timer(&mut self) {
        let now = Instant::now();
        for channel in PowerChannel::ALL {
            let due = self.power.fades[channel.index()]
                .as_ref()
                .is_some_and(|f| f.next_deadline() <= now);
            if due {
                self.tick_channel(channel, now);
            }
        }
        match self.power.stage {
            Stage::Warmup { until } if until <= now => self.warmup_done(),
            Stage::OffDelay { until } if until <= now => {
                self.power.stage = Stage::None;
                self.send_plug(false);
            }
            _ => {}
        }
    }

    // --------------------------------------------------------- the link ---

    /// The MOTU link left `Online`: what is shown is no longer the card. Fades and the plug
    /// sequence stop and nothing is sent to the plug (it is left as it is).
    pub(super) fn link_lost(&mut self) {
        self.synced = false;
        self.power.fades = [None, None];
        self.power.echoes = [None, None];
        self.power.stage = Stage::None;
        // Anything queued or still in flight is dropped (its answer is ignored).
        self.plug.seq += 1;
        let _ = self.plug.worker.send(WorkerCmd::Reset);
        if self.plug.state == PlugState::Warming {
            self.plug.state = PlugState::Unknown;
        }
    }

    // ------------------------------------------------------------- power ---

    fn begin_plug_on(&mut self) {
        self.power.stage = Stage::PlugOn;
        self.plug.state = PlugState::Warming;
        self.send_plug(true);
    }

    /// The `power` command.
    pub(super) fn on_power(&mut self, channel: PowerChannel, on: bool) {
        let main = channel == PowerChannel::Main;
        let plug_wanted = main && self.plug_enabled();
        let plug = plug_wanted && self.hw_ready();
        let current = self.level(channel);
        let running = self.power.fades[channel.index()]
            .as_ref()
            .map(|f| (f.dir, f.level()));
        // The fade ran but the plug was left alone (the card is not connected).
        let mut skipped = false;
        if on {
            if main {
                match self.power.stage {
                    // Already on its way.
                    Stage::PlugOn | Stage::Warmup { .. } => return,
                    // The plug never went off: no need to wake the speakers again.
                    Stage::OffDelay { .. } => {
                        self.power.stage = Stage::None;
                        self.start_fade(channel, FadeDir::In, current, false);
                        return;
                    }
                    Stage::None => {}
                }
            }
            match running {
                Some((FadeDir::In, _)) => {}
                // Turned back before it ended: the plug is still on, only the fade reverses.
                Some((FadeDir::Out, level)) => self.start_fade(channel, FadeDir::In, level, false),
                None if power::is_off(current) => {
                    if plug {
                        self.begin_plug_on();
                    } else {
                        skipped = plug_wanted;
                        self.start_fade(channel, FadeDir::In, current, false);
                    }
                }
                // Audible but the plug is not known to be on: park the knob at the minimum
                // (the speakers are unpowered, so it is inaudible), then the normal sequence
                // brings it back to this level.
                None => {
                    if plug && self.plug.state != PlugState::On {
                        self.remember(channel, current);
                        self.write_level(channel, TRIM_MIN_DB);
                        self.begin_plug_on();
                    }
                }
            }
        } else {
            if main {
                match self.power.stage {
                    // Off during the warm-up cancels it and switches the plug off.
                    Stage::PlugOn | Stage::Warmup { .. } => {
                        if matches!(self.power.stage, Stage::Warmup { .. }) {
                            self.plug.state = PlugState::On;
                        }
                        self.power.stage = Stage::None;
                        self.send_plug(false);
                        return;
                    }
                    Stage::OffDelay { .. } => return,
                    Stage::None => {}
                }
            }
            match running {
                Some((FadeDir::Out, _)) => {}
                // Reversed mid-fade: the remembered level is the one the fade in was heading to.
                Some((FadeDir::In, level)) => {
                    skipped = plug_wanted && !plug;
                    self.start_fade(channel, FadeDir::Out, level, plug);
                }
                None if !power::is_off(current) => {
                    skipped = plug_wanted && !plug;
                    self.remember(channel, current);
                    self.start_fade(channel, FadeDir::Out, current, plug);
                }
                // Already off: make sure the plug is too.
                None => {
                    if plug && self.plug.state != PlugState::Off {
                        self.begin_off_delay();
                    }
                }
            }
        }
        if skipped {
            self.notice(
                NoticeLevel::Warning,
                "MOTU not connected: speaker plug left as is",
            );
        }
    }

    fn begin_off_delay(&mut self) {
        let delay =
            Duration::from_millis(u64::from(self.plug.settings.off_delay_ms)).max(MIN_OFF_DELAY);
        self.power.stage = Stage::OffDelay {
            until: Instant::now() + delay,
        };
    }

    /// The warm-up is over (or there was none): the speakers are on, fade in.
    fn warmup_done(&mut self) {
        self.power.stage = Stage::None;
        self.plug.state = PlugState::On;
        let current = self.level(PowerChannel::Main);
        self.start_fade(PowerChannel::Main, FadeDir::In, current, false);
    }

    // ------------------------------------------------- somebody moves a knob ---

    /// Stops what Main's plug sequence was about to do (a fade is stopped by the caller).
    fn cancel_stage(&mut self) {
        match self.power.stage {
            // The plug answered and is on; the user has the knob.
            Stage::Warmup { .. } => self.plug.state = PlugState::On,
            // Nothing to stop in flight: its answer is handled when it arrives.
            Stage::PlugOn | Stage::OffDelay { .. } | Stage::None => {}
        }
        self.power.stage = Stage::None;
    }

    /// A client or OSC set the volume: it takes the knob (stops the fade, the warm-up, the
    /// delayed plug-off; the plug stays as it is) and a real level replaces the remembered one.
    pub(super) fn user_moved(&mut self, channel: PowerChannel) {
        self.power.fades[channel.index()] = None;
        self.power.echoes[channel.index()] = None;
        if channel == PowerChannel::Main {
            self.cancel_stage();
        }
        if !power::is_off(self.level(channel)) {
            self.forget(channel);
        }
    }

    /// The hardware reported a volume. A fade's own echo (a level within half a dB of a step it
    /// wrote, also just after the fade ended or was reversed) is not a move; anything else is,
    /// like a client's.
    pub(super) fn hardware_moved(&mut self, param: &Param) {
        let (channel, db) = match *param {
            Param::MainVol { db } => (PowerChannel::Main, db),
            Param::PhonesVol { db } => (PowerChannel::Phones, db),
            _ => return,
        };
        let i = channel.index();
        let late_echo = self.power.echoes[i].as_ref().is_some_and(|(steps, until)| {
            *until > Instant::now() && power::is_echo_of(steps.iter().copied(), db)
        });
        if late_echo || self.power.fades[i].as_ref().is_some_and(|f| f.is_echo(db)) {
            return;
        }
        // During the plug sequence the level is parked at the minimum.
        if channel == PowerChannel::Main && self.power.stage != Stage::None && power::is_off(db) {
            return;
        }
        self.power.fades[i] = None;
        self.power.echoes[i] = None;
        if channel == PowerChannel::Main {
            self.cancel_stage();
        }
        if !power::is_off(db) {
            self.forget(channel);
        }
    }

    // --------------------------------------------------------- speaker plug ---

    /// Ask the plug (through the worker) to switch; the answer comes back as
    /// `Internal::Switched`.
    fn send_plug(&mut self, on: bool) {
        self.plug.seq += 1;
        let _ = self.plug.worker.send(WorkerCmd::Switch {
            seq: self.plug.seq,
            on,
            epoch: self.plug.epoch,
            settings: self.plug.settings.clone(),
        });
    }

    pub(super) async fn on_internal(&mut self, msg: Internal) {
        match msg {
            Internal::Switched { seq, on, result } => self.on_plug_result(seq, on, result),
            Internal::Tested { origin, on, result } => {
                self.plug.test_running = false;
                let (ok, message) = match &result {
                    Ok(m) => (true, m.clone()),
                    Err(e) => (false, e.clone()),
                };
                self.emit(
                    origin,
                    Target::Only(origin),
                    ServerEvent::SpeakerPlugResult { on, ok, message },
                );
                // What the test learned about the plug counts (tests only run when no
                // sequence does).
                if self.power.stage == Stage::None {
                    self.plug.state = match (ok, on) {
                        (false, _) => PlugState::Error,
                        (true, true) => PlugState::On,
                        (true, false) => PlugState::Off,
                    };
                }
            }
            Internal::Checked {
                origin,
                settings,
                result,
                ticket,
            } => match result {
                Ok(()) if ticket < self.plug.applied_ticket => {
                    self.reply_error(
                        origin,
                        "speaker plug settings superseded by a newer save".into(),
                    );
                }
                Ok(()) => {
                    self.plug.applied_ticket = ticket;
                    self.apply_plug_settings(origin, settings).await;
                }
                Err(e) => self.reply_error(origin, format!("speaker plug refused: {e}")),
            },
        }
    }

    fn on_plug_result(&mut self, seq: u64, on: bool, result: Result<String, String>) {
        if seq != self.plug.seq {
            tracing::debug!(seq, "stale speaker plug answer ignored");
            return;
        }
        match (on, result) {
            (true, Ok(_)) => {
                if self.power.stage == Stage::PlugOn {
                    let warmup = Duration::from_millis(u64::from(self.plug.settings.warmup_ms));
                    if warmup.is_zero() {
                        self.warmup_done();
                    } else {
                        self.power.stage = Stage::Warmup {
                            until: Instant::now() + warmup,
                        };
                    }
                } else {
                    // Cancelled while the request was in flight: the plug is on all the same.
                    self.plug.state = PlugState::On;
                }
            }
            (true, Err(e)) => {
                tracing::warn!(error = %e, "speaker plug did not switch on");
                self.plug.state = PlugState::Error;
                self.notice(NoticeLevel::Warning, "Speaker plug did not respond");
                if self.power.stage == Stage::PlugOn {
                    // Play on without it: the speakers may be on already.
                    self.power.stage = Stage::None;
                    let current = self.level(PowerChannel::Main);
                    self.start_fade(PowerChannel::Main, FadeDir::In, current, false);
                }
            }
            (false, Ok(_)) => self.plug.state = PlugState::Off,
            (false, Err(e)) => {
                tracing::warn!(error = %e, "speaker plug did not switch off");
                self.plug.state = PlugState::Error;
                self.notice(NoticeLevel::Warning, "Speaker plug did not switch off");
            }
        }
    }

    /// `set_speaker_plug`: validated here, fence-checked in a task, saved when that passes.
    pub(super) fn on_set_speaker_plug(&mut self, origin: Origin, settings: SpeakerPlugSettings) {
        if let Err(e) = settings.validate() {
            self.reply_error(origin, format!("invalid speaker plug settings: {e}"));
            return;
        }
        self.plug.ticket += 1;
        let ticket = self.plug.ticket;
        let transport = Arc::clone(&self.plug.transport);
        let tx = self.internal.clone();
        tokio::spawn(async move {
            let result = if settings.enabled {
                transport.check(settings.clone()).await
            } else {
                Ok(())
            };
            let _ = tx.send(Internal::Checked {
                origin,
                settings,
                result,
                ticket,
            });
        });
    }

    async fn apply_plug_settings(&mut self, origin: Origin, settings: SpeakerPlugSettings) {
        if let Err(e) = plug::save_settings(&self.plug.file, &settings).await {
            self.reply_error(origin, format!("speaker plug settings not saved: {e:#}"));
            return;
        }
        let target_changed = self.plug.settings.enabled != settings.enabled
            || self.plug.settings.urls_or_empty() != settings.urls_or_empty();
        let changed = self.plug.settings != settings;
        self.plug.settings = settings;
        if changed {
            // Answers still on their way come from the old settings: ignore them, and do not
            // trust what was known about the plug.
            self.plug.seq += 1;
            self.plug.epoch += 1;
            let _ = self.plug.worker.send(WorkerCmd::Reset);
        }
        if target_changed {
            // Another plug (or none): what was known about the old one no longer holds.
            self.plug.state = PlugState::Unknown;
            self.power.stage = Stage::None;
        }
    }

    /// `test_speaker_plug`: one request to the saved plug; the answer goes to the requester.
    pub(super) fn on_test_speaker_plug(&mut self, origin: Origin, on: bool) {
        let refuse = |this: &Self, message: &str| {
            this.emit(
                origin,
                Target::Only(origin),
                ServerEvent::SpeakerPlugResult {
                    on,
                    ok: false,
                    message: message.to_owned(),
                },
            );
        };
        if let Err(e) = plug::build_urls(&self.plug.settings) {
            refuse(self, &e);
            return;
        }
        if self.power.stage != Stage::None {
            refuse(
                self,
                "Monitor is switching on or off: try again in a moment",
            );
            return;
        }
        if on && !power::is_off(self.level(PowerChannel::Main)) {
            refuse(self, "Turn Monitor off first");
            return;
        }
        if self.plug.test_running {
            refuse(self, "a test is already running");
            return;
        }
        self.plug.test_running = true;
        let _ = self.plug.worker.send(WorkerCmd::Test {
            origin,
            on,
            epoch: self.plug.epoch,
            settings: self.plug.settings.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{start_with, DeviceEvent, DeviceLink, EngineConfig, EngineHandle, Envelope};
    use super::*;
    use crate::plug::{PlugFuture, PlugKind};
    use crate::state::Topology;
    use std::sync::Mutex;
    use tokio::sync::broadcast;

    /// A plug that records every request and answers as told.
    #[derive(Debug)]
    struct FakePlug {
        calls: Mutex<Vec<(bool, Instant)>>,
        fail_on: Mutex<bool>,
        fail_off: Mutex<bool>,
        delay: Mutex<Duration>,
        active: Arc<std::sync::atomic::AtomicUsize>,
        max_active: std::sync::atomic::AtomicUsize,
    }

    impl FakePlug {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                fail_on: Mutex::new(false),
                fail_off: Mutex::new(false),
                delay: Mutex::new(Duration::ZERO),
                active: Default::default(),
                max_active: Default::default(),
            })
        }

        fn calls(&self) -> Vec<(bool, Instant)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl PlugTransport for FakePlug {
        fn send(&self, _: SpeakerPlugSettings, on: bool) -> PlugFuture<Result<String, String>> {
            self.calls.lock().unwrap().push((on, Instant::now()));
            let fail = if on {
                *self.fail_on.lock().unwrap()
            } else {
                *self.fail_off.lock().unwrap()
            };
            let delay = *self.delay.lock().unwrap();
            let now_active = self
                .active
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            self.max_active
                .fetch_max(now_active, std::sync::atomic::Ordering::SeqCst);
            let active = Arc::clone(&self.active);
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                if fail {
                    Err("no answer (timeout)".to_owned())
                } else {
                    Ok("HTTP 200".to_owned())
                }
            })
        }

        fn check(&self, s: SpeakerPlugSettings) -> PlugFuture<Result<(), String>> {
            Box::pin(async move {
                if s.host.starts_with("slow") {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                // Same rule as the real fence, minus the network.
                if s.host.starts_with("public") {
                    Err("not on the local network".to_owned())
                } else {
                    Ok(())
                }
            })
        }
    }

    fn plug_settings() -> SpeakerPlugSettings {
        SpeakerPlugSettings {
            enabled: true,
            kind: PlugKind::ShellyRpc,
            host: "plug.example".into(),
            ..SpeakerPlugSettings::default()
        }
    }

    struct Rig {
        h: EngineHandle,
        link: DeviceLink,
        rx: broadcast::Receiver<Envelope>,
        dir: PathBuf,
        fake: Arc<FakePlug>,
    }

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("cuemix-power-{tag}-{}-{nanos}", std::process::id()))
    }

    async fn rig_in(
        dir: PathBuf,
        settings: Option<SpeakerPlugSettings>,
        fake: Arc<FakePlug>,
    ) -> Rig {
        let mut r = rig_raw(dir, settings, fake).await;
        r.go_online().await;
        r
    }

    /// An engine whose MOTU link is not up (demo mode).
    async fn rig_offline(tag: &str, settings: Option<SpeakerPlugSettings>) -> Rig {
        rig_raw(tmp(tag), settings, FakePlug::new()).await
    }

    async fn rig_raw(
        dir: PathBuf,
        settings: Option<SpeakerPlugSettings>,
        fake: Arc<FakePlug>,
    ) -> Rig {
        let mut cfg = EngineConfig::for_dir(Topology::default(), &dir);
        if let Some(s) = settings {
            cfg.speaker_plug = s;
        }
        let (h, link, _task) = start_with(cfg, fake.clone()).await;
        let rx = h.subscribe();
        Rig {
            h,
            link,
            rx,
            dir,
            fake,
        }
    }

    async fn rig(tag: &str, settings: Option<SpeakerPlugSettings>) -> Rig {
        rig_in(tmp(tag), settings, FakePlug::new()).await
    }

    impl Rig {
        /// The MOTU link comes up and its state is read.
        async fn go_online(&mut self) {
            self.link
                .events
                .send(DeviceEvent::Status(
                    crate::motu::client::ConnectionStatus::Online,
                ))
                .await
                .unwrap();
            self.link
                .events
                .send(DeviceEvent::Synced(vec![]))
                .await
                .unwrap();
            self.until(|e| matches!(e, ServerEvent::Snapshot { .. }))
                .await;
        }

        /// The remembered levels on disk (after the background writer had time to run).
        async fn saved(&self) -> Remembered {
            tokio::time::sleep(Duration::from_millis(100)).await;
            power::load_remembered(&self.dir.join("power.json"))
                .await
                .unwrap()
        }

        async fn power(&self, channel: PowerChannel, on: bool) {
            self.h
                .command(Origin::Web(1), Command::Power { channel, on })
                .await
                .unwrap();
        }

        async fn set(&self, param: Param) {
            self.h
                .command(Origin::Web(2), Command::Set(param))
                .await
                .unwrap();
        }

        /// Set the Main level and wait for the echo, so what follows starts from it.
        async fn set_main(&mut self, db: f64) {
            self.set(Param::MainVol { db }).await;
            self.until(|e| matches!(e, ServerEvent::Changed { .. }))
                .await;
        }

        /// Next envelope (virtual time advances while waiting).
        async fn next(&mut self) -> Envelope {
            tokio::time::timeout(Duration::from_secs(30), self.rx.recv())
                .await
                .unwrap()
                .unwrap()
        }

        /// Drain envelopes until `pred` matches one; returns that event.
        async fn until(&mut self, pred: impl Fn(&ServerEvent) -> bool) -> ServerEvent {
            loop {
                let env = self.next().await;
                if pred(&env.event) {
                    return env.event;
                }
            }
        }

        /// Everything broadcast until the Main power event satisfies `pred`.
        async fn collect_until_power(
            &mut self,
            pred: impl Fn(&ChannelPower, &ChannelPower) -> bool,
        ) -> Vec<(Instant, Envelope)> {
            let mut out = Vec::new();
            loop {
                let env = self.next().await;
                let hit =
                    matches!(&env.event, ServerEvent::Power { main, phones } if pred(main, phones));
                out.push((Instant::now(), env));
                if hit {
                    return out;
                }
            }
        }

        /// Let `d` of virtual time pass, returning what was broadcast meanwhile.
        async fn quiet_for(&mut self, d: Duration) -> Vec<Envelope> {
            let mut out = Vec::new();
            let end = Instant::now() + d;
            loop {
                match tokio::time::timeout_at(end, self.rx.recv()).await {
                    Ok(Ok(env)) => out.push(env),
                    Ok(Err(e)) => panic!("{e}"),
                    Err(_) => return out,
                }
            }
        }

        async fn main_db(&self) -> f64 {
            let ServerEvent::Snapshot { state, .. } =
                self.h.initial_events().await.unwrap().remove(0)
            else {
                panic!("snapshot")
            };
            state.main.db
        }

        async fn power_event(&self) -> (ChannelPower, ChannelPower) {
            let events = self.h.initial_events().await.unwrap();
            let Some(ServerEvent::Power { main, phones }) = events
                .into_iter()
                .find(|e| matches!(e, ServerEvent::Power { .. }))
            else {
                panic!("power event")
            };
            (main, phones)
        }

        async fn cleanup(self) {
            let _ = tokio::fs::remove_dir_all(self.dir).await;
        }
    }

    fn changed_main(e: &ServerEvent) -> Option<f64> {
        match e {
            ServerEvent::Changed { params } => params.iter().find_map(|p| match p {
                Param::MainVol { db } => Some(*db),
                _ => None,
            }),
            _ => None,
        }
    }

    // ---------------------------------------------------------- the fade ---

    #[tokio::test(start_paused = true)]
    async fn off_fades_out_with_system_steps_and_remembers_the_level() {
        let mut r = rig("off", None).await;
        r.set_main(-20.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, false).await;
        let events = r
            .collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        let (end, _) = events.last().unwrap();
        // 80 dB: 960 ms, ending exactly on time.
        assert_eq!(*end - t0, Duration::from_millis(960));
        // Steps are system-originated changes every client sees, monotonic, whole dB.
        let steps: Vec<f64> = events
            .iter()
            .filter_map(|(_, e)| {
                changed_main(&e.event).inspect(|_| {
                    assert_eq!(e.origin, Origin::System);
                    assert_eq!(e.target, Target::All);
                })
            })
            .collect();
        assert!(steps.len() > 10, "{steps:?}");
        assert_eq!(steps.last(), Some(&-100.0));
        assert!(steps.windows(2).all(|w| w[1] < w[0]));
        assert!(steps.iter().all(|v| v.fract() == 0.0));
        // The first power event says "fading out"; the fade never touches the Modified flag.
        let first_power = events
            .iter()
            .find_map(|(_, e)| match &e.event {
                ServerEvent::Power { main, .. } if main.fading.is_some() => Some(*main),
                _ => None,
            })
            .unwrap();
        assert_eq!(first_power.fading, Some(FadeDir::Out));
        assert!(first_power.on);
        assert!(events.iter().all(|(_, e)| !matches!(
            e.event,
            ServerEvent::Session(_) | ServerEvent::Snapshot { .. }
        )));
        // The remembered level reached the disk.
        let saved = r.saved().await;
        assert_eq!(saved.main, Some(-20.0));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn on_fades_back_to_the_remembered_level_then_forgets_it() {
        let mut r = rig("on", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        assert_eq!(r.main_db().await, -100.0);
        let t0 = Instant::now();
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        let (end, _) = events.last().unwrap();
        assert_eq!(*end - t0, Duration::from_millis(960));
        assert_eq!(r.main_db().await, -20.0, "never above the remembered level");
        let steps: Vec<f64> = events
            .iter()
            .filter_map(|(_, e)| changed_main(&e.event))
            .collect();
        assert!(steps.windows(2).all(|w| w[1] > w[0]));
        assert!(steps.iter().all(|v| *v <= -20.0));
        // Used once: the file no longer holds it, and the next On has nothing to go back to.
        let _ = r.quiet_for(Duration::from_millis(10)).await;
        let saved = r.saved().await;
        assert_eq!(saved, Remembered::default());
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn on_without_a_remembered_level_goes_to_minus_70() {
        let mut r = rig("safe", None).await;
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        r.collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert_eq!(r.main_db().await, power::FADE_SAFE_DB);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn off_during_a_fade_reverses_it_without_forgetting() {
        let mut r = rig("reverse", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.until(
            |e| matches!(e, ServerEvent::Power { main, .. } if main.fading == Some(FadeDir::Out)),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        // On while fading out turns around from where it is.
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert_eq!(r.main_db().await, -20.0, "back to the remembered level");
        assert!(events.iter().any(|(_, e)| matches!(
            &e.event,
            ServerEvent::Power { main, .. } if main.fading == Some(FadeDir::In)
        )));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_off_while_fading_keeps_the_first_remembered_level() {
        let mut r = rig("twice", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        let saved = r.saved().await;
        assert_eq!(saved.main, Some(-20.0), "not the mid-fade level");
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_set_from_a_client_cancels_the_fade_and_a_real_level_forgets() {
        let mut r = rig("cancel", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.until(|e| matches!(e, ServerEvent::Power { main, .. } if main.fading.is_some()))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        r.set(Param::MainVol { db: -30.0 }).await;
        r.until(|e| changed_main(e) == Some(-30.0)).await;
        let (main, _) = r.power_event().await;
        assert!(main.on && main.fading.is_none());
        // No more steps after the user's value.
        let later = r.quiet_for(Duration::from_secs(2)).await;
        assert!(
            later.iter().all(|e| changed_main(&e.event).is_none()),
            "{later:?}"
        );
        assert_eq!(r.main_db().await, -30.0);
        let saved = r.saved().await;
        assert_eq!(saved.main, None, "a knob move to a real level forgets");
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_move_to_the_minimum_keeps_the_remembered_level() {
        let mut r = rig("tomin", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        r.set(Param::MainVol { db: -100.0 }).await;
        let _ = r.quiet_for(Duration::from_millis(50)).await;
        let saved = r.saved().await;
        assert_eq!(saved.main, Some(-20.0));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn hardware_values_cancel_only_beyond_half_a_db_of_a_step() {
        let mut r = rig("hw", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.until(|e| matches!(e, ServerEvent::Power { main, .. } if main.fading.is_some()))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        // The hardware echoes an earlier step it was sent: the fade goes on.
        let _ = r.quiet_for(Duration::ZERO).await;
        let older = loop {
            let first = r.until(|e| changed_main(e).is_some()).await;
            let second = r.until(|e| changed_main(e).is_some()).await;
            if changed_main(&first) != changed_main(&second) {
                break changed_main(&first).unwrap();
            }
        };
        r.link
            .events
            .send(DeviceEvent::Changed(vec![Param::MainVol { db: older }]))
            .await
            .unwrap();
        let events = r
            .collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        assert_eq!(r.main_db().await, -100.0, "the fade ran to the end");
        assert!(events.len() > 3);

        // Now a hardware value far from the steps takes the knob.
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.until(|e| matches!(e, ServerEvent::Power { main, .. } if main.fading.is_some()))
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        r.link
            .events
            .send(DeviceEvent::Changed(vec![Param::MainVol { db: -8.0 }]))
            .await
            .unwrap();
        r.until(|e| changed_main(e) == Some(-8.0)).await;
        let later = r.quiet_for(Duration::from_secs(2)).await;
        assert!(later.iter().all(|e| changed_main(&e.event).is_none()));
        assert_eq!(r.main_db().await, -8.0);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn power_event_content_on_connect_and_for_phones() {
        let mut r = rig("event", None).await;
        let (main, phones) = r.power_event().await;
        assert_eq!(
            (main.fading, main.plug, phones.fading, phones.plug),
            (None, None, None, None)
        );
        assert_eq!(
            serde_json::to_value(ServerEvent::Power { main, phones }).unwrap(),
            serde_json::json!({
                "event": "power",
                "main": {"on": true, "fading": null, "plug": null},
                "phones": {"on": true, "fading": null, "plug": null}
            })
        );
        // Phones fade the same way and are independent of Main.
        r.set(Param::PhonesVol { db: -30.0 }).await;
        r.until(|e| matches!(e, ServerEvent::Changed { .. })).await;
        r.power(PowerChannel::Phones, false).await;
        let events = r
            .collect_until_power(|_, p| !p.on && p.fading.is_none())
            .await;
        assert!(events.iter().any(|(_, e)| matches!(
            &e.event,
            ServerEvent::Power { phones, main } if phones.fading == Some(FadeDir::Out) && main.fading.is_none()
        )));
        let saved = r.saved().await;
        assert_eq!(saved.phones, Some(-30.0));
        assert_eq!(saved.main, None);
        // Phones never use the plug.
        assert!(r.fake.calls().is_empty());
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_remembered_level_survives_a_restart() {
        let dir = tmp("restart");
        let mut r = rig_in(dir.clone(), None, FakePlug::new()).await;
        r.set_main(-14.0).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(r.h);
        // A new engine on the same files.
        let mut again = rig_in(dir, None, FakePlug::new()).await;
        again.set_main(-100.0).await;
        again.power(PowerChannel::Main, true).await;
        again
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert_eq!(again.main_db().await, -14.0);
        again.cleanup().await;
    }

    // ------------------------------------------------------ the speaker plug ---

    #[tokio::test(start_paused = true)]
    async fn plug_on_ok_waits_the_warmup_then_fades_in() {
        let mut r = rig("plug-on", Some(plug_settings())).await;
        r.set_main(-100.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        // The plug was asked once, at once.
        let calls = r.fake.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0);
        assert_eq!(calls[0].1 - t0, Duration::ZERO);
        // First power event: warming and fading in, still at the minimum.
        let first = events
            .iter()
            .find_map(|(_, e)| match &e.event {
                ServerEvent::Power { main, .. } if main.fading.is_some() => Some(*main),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            (first.on, first.fading, first.plug),
            (false, Some(FadeDir::In), Some(PlugState::Warming))
        );
        // No level change before the 2 s warm-up is over.
        let first_step = events
            .iter()
            .find(|(_, e)| changed_main(&e.event).is_some())
            .map(|(at, _)| *at - t0)
            .unwrap();
        // (the first step writes the level it starts from, which is no change: one step later)
        assert!(
            (Duration::from_millis(2000)..=Duration::from_millis(2000 + power::FADE_STEP_MS))
                .contains(&first_step),
            "{first_step:?}"
        );
        // Then the fade (30 dB, from the minimum up to -70: 360 ms) and the plug is "on".
        let (end, _) = events.last().unwrap();
        assert_eq!(*end - t0, Duration::from_millis(2000 + 360));
        let (main, _) = r.power_event().await;
        assert_eq!(main.plug, Some(PlugState::On));
        assert_eq!(r.main_db().await, power::FADE_SAFE_DB);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn plug_on_failure_warns_and_fades_in_at_once() {
        let mut r = rig("plug-fail", Some(plug_settings())).await;
        *r.fake.fail_on.lock().unwrap() = true;
        *r.fake.delay.lock().unwrap() = Duration::from_millis(1500);
        r.set_main(-100.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        let notice_at = events
            .iter()
            .find(|(_, e)| matches!(&e.event, ServerEvent::Notice { .. }))
            .map(|(at, e)| (*at - t0, e.clone()))
            .unwrap();
        assert_eq!(notice_at.0, Duration::from_millis(1500));
        assert_eq!(
            notice_at.1.event,
            ServerEvent::Notice {
                level: NoticeLevel::Warning,
                message: "Speaker plug did not respond".into()
            }
        );
        assert_eq!(notice_at.1.target, Target::All);
        // The fade starts as soon as the failure is known: no warm-up on top.
        let first_step = events
            .iter()
            .find(|(_, e)| changed_main(&e.event).is_some())
            .map(|(at, _)| *at - t0)
            .unwrap();
        assert!(
            (Duration::from_millis(1500)..=Duration::from_millis(1500 + power::FADE_STEP_MS))
                .contains(&first_step),
            "{first_step:?}"
        );
        let (main, _) = r.power_event().await;
        assert_eq!(main.plug, Some(PlugState::Error));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn plug_off_follows_the_fade_after_the_delay() {
        let mut r = rig("plug-off", Some(plug_settings())).await;
        r.set_main(-20.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, false).await;
        let events = r
            .collect_until_power(|m, _| m.plug == Some(PlugState::Off))
            .await;
        let calls = r.fake.calls();
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].0);
        // 960 ms of fade + 300 ms delay.
        assert_eq!(calls[0].1 - t0, Duration::from_millis(960 + 300));
        // The fade had ended (level at the minimum) before the plug was asked.
        assert!(events
            .iter()
            .any(|(_, e)| matches!(&e.event, ServerEvent::Power { main, .. } if !main.on && main.fading.is_none() && main.plug != Some(PlugState::Off))));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_fade_out_cancelled_by_a_knob_move_leaves_the_plug_on() {
        let mut r = rig("plug-keep", Some(plug_settings())).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.set(Param::MainVol { db: -25.0 }).await;
        let later = r.quiet_for(Duration::from_secs(3)).await;
        assert!(r.fake.calls().is_empty(), "the plug stays on");
        assert!(later
            .iter()
            .all(|e| !matches!(e.event, ServerEvent::Notice { .. })));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_knob_move_during_the_off_delay_leaves_the_plug_on() {
        let mut s = plug_settings();
        s.off_delay_ms = 1000;
        let mut r = rig("plug-delay", Some(s)).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.set(Param::MainVol { db: -30.0 }).await;
        let _ = r.quiet_for(Duration::from_secs(3)).await;
        assert!(r.fake.calls().is_empty());
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn off_during_the_warmup_cancels_it_and_switches_the_plug_off() {
        let mut r = rig("plug-cancel", Some(plug_settings())).await;
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        r.until(|e| matches!(e, ServerEvent::Power { main, .. } if main.plug == Some(PlugState::Warming)))
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let t_off = Instant::now();
        r.power(PowerChannel::Main, false).await;
        let events = r
            .collect_until_power(|m, _| m.plug == Some(PlugState::Off))
            .await;
        let calls = r.fake.calls();
        assert_eq!(
            calls.iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![true, false]
        );
        assert_eq!(calls[1].1 - t_off, Duration::ZERO, "switched off at once");
        // The fade in never started.
        assert!(events.iter().all(|(_, e)| changed_main(&e.event).is_none()));
        let later = r.quiet_for(Duration::from_secs(3)).await;
        assert!(later.iter().all(|e| changed_main(&e.event).is_none()));
        assert_eq!(r.main_db().await, -100.0);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn off_while_the_plug_request_is_in_flight_ignores_its_late_answer() {
        let mut r = rig("plug-late", Some(plug_settings())).await;
        *r.fake.delay.lock().unwrap() = Duration::from_millis(500);
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        r.power(PowerChannel::Main, false).await;
        let later = r.quiet_for(Duration::from_secs(5)).await;
        // The first request's answer arrives, but no fade ever starts.
        assert!(later.iter().all(|e| changed_main(&e.event).is_none()));
        let (main, _) = r.power_event().await;
        assert_eq!(main.plug, Some(PlugState::Off));
        assert_eq!(r.fake.calls().len(), 2);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn on_while_fading_out_reverses_without_touching_the_plug() {
        let mut r = rig("plug-reverse", Some(plug_settings())).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        r.power(PowerChannel::Main, true).await;
        r.collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert!(r.fake.calls().is_empty());
        assert_eq!(r.main_db().await, -20.0);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn no_plug_means_no_requests_and_null_plug_state() {
        let mut r = rig("no-plug", None).await;
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert!(events.iter().all(|(_, e)| !matches!(
            &e.event,
            ServerEvent::Power { main, .. } if main.plug.is_some()
        )));
        assert!(r.fake.calls().is_empty());
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn knob_moves_never_switch_the_plug() {
        let mut r = rig("plug-knob", Some(plug_settings())).await;
        r.set_main(-100.0).await;
        r.set_main(-10.0).await;
        r.set_main(-100.0).await;
        r.set(Param::PhonesVol { db: -100.0 }).await;
        let _ = r.quiet_for(Duration::from_secs(1)).await;
        assert!(r.fake.calls().is_empty());
        r.cleanup().await;
    }

    // ----------------------------------------------------- settings, test ---

    #[tokio::test(start_paused = true)]
    async fn set_speaker_plug_validates_saves_and_broadcasts() {
        let mut r = rig("set-plug", None).await;
        let me = Origin::Web(9);
        // Out of range: refused to the requester only.
        let mut bad = plug_settings();
        bad.warmup_ms = 99_999;
        r.h.command(me, Command::SetSpeakerPlug { settings: bad })
            .await
            .unwrap();
        let env = r.next().await;
        assert!(matches!(env.event, ServerEvent::Error { .. }));
        assert_eq!(env.target, Target::Only(me));
        // Fence: refused too.
        let mut public = plug_settings();
        public.host = "public.example".into();
        r.h.command(me, Command::SetSpeakerPlug { settings: public })
            .await
            .unwrap();
        let env = r.next().await;
        let ServerEvent::Error { message } = env.event else {
            panic!("error")
        };
        assert!(message.contains("not on the local network"), "{message}");
        assert_eq!(
            plug::load_settings(&r.dir.join("speaker-plug.json"))
                .await
                .unwrap(),
            None
        );
        // Valid: saved, broadcast with its URLs.
        let good = plug_settings();
        r.h.command(
            me,
            Command::SetSpeakerPlug {
                settings: good.clone(),
            },
        )
        .await
        .unwrap();
        let ev = r
            .until(|e| matches!(e, ServerEvent::SpeakerPlug { .. }))
            .await;
        let ServerEvent::SpeakerPlug {
            settings,
            urls,
            state,
        } = ev
        else {
            panic!()
        };
        assert_eq!(settings, good);
        assert_eq!(urls.on, "http://plug.example/rpc/Switch.Set?id=0&on=true");
        assert_eq!(urls.off, "http://plug.example/rpc/Switch.Set?id=0&on=false");
        assert_eq!(state, PlugState::Unknown);
        assert_eq!(
            plug::load_settings(&r.dir.join("speaker-plug.json"))
                .await
                .unwrap(),
            Some(good.clone())
        );
        // Now the Main power event carries the plug state, and a later start loads the file.
        let (main, phones) = r.power_event().await;
        assert_eq!(main.plug, Some(PlugState::Unknown));
        assert_eq!(phones.plug, None);
        let dir = r.dir.clone();
        drop(r.h);
        let again = rig_in(dir, None, FakePlug::new()).await;
        let events = again.h.initial_events().await.unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if *settings == good)));
        again.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn config_settings_are_the_defaults_and_the_file_overrides_them() {
        let dir = tmp("cfg-plug");
        let mut from_config = plug_settings();
        from_config.host = "config.example".into();
        let r = rig_in(dir.clone(), Some(from_config.clone()), FakePlug::new()).await;
        let init = r.h.initial_events().await.unwrap();
        assert!(init.iter().any(
            |e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if *settings == from_config)
        ));
        drop(r.h);
        let mut saved = plug_settings();
        saved.host = "saved.example".into();
        plug::save_settings(&dir.join("speaker-plug.json"), &saved)
            .await
            .unwrap();
        let r = rig_in(dir, Some(from_config), FakePlug::new()).await;
        let init = r.h.initial_events().await.unwrap();
        assert!(init
            .iter()
            .any(|e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if *settings == saved)));
        // An invalid saved file would be dropped (plug off), not trusted.
        r.cleanup().await;
        let bad = tmp("cfg-bad");
        let mut broken = plug_settings();
        broken.host = "bad host".into();
        plug::save_settings(&bad.join("speaker-plug.json"), &broken)
            .await
            .unwrap();
        let r = rig_in(bad, None, FakePlug::new()).await;
        let init = r.h.initial_events().await.unwrap();
        assert!(init
            .iter()
            .any(|e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if !settings.enabled)));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_speaker_plug_answers_the_requester_only() {
        let mut r = rig("test-plug", Some(plug_settings())).await;
        r.set_main(-100.0).await;
        let me = Origin::Web(5);
        r.h.command(me, Command::TestSpeakerPlug { on: true })
            .await
            .unwrap();
        let env = loop {
            let env = r.next().await;
            if matches!(env.event, ServerEvent::SpeakerPlugResult { .. }) {
                break env;
            }
        };
        assert_eq!(env.target, Target::Only(me));
        assert_eq!(
            env.event,
            ServerEvent::SpeakerPlugResult {
                on: true,
                ok: true,
                message: "HTTP 200".into()
            }
        );
        assert!(env.is_for(me) && !env.is_for(Origin::Web(6)));
        // The plug state follows what the test learned.
        let (main, _) = r.power_event().await;
        assert_eq!(main.plug, Some(PlugState::On));
        // A failing plug is reported, not hidden.
        *r.fake.fail_off.lock().unwrap() = true;
        r.h.command(me, Command::TestSpeakerPlug { on: false })
            .await
            .unwrap();
        let env = r
            .until(|e| matches!(e, ServerEvent::SpeakerPlugResult { .. }))
            .await;
        assert!(matches!(
            env,
            ServerEvent::SpeakerPlugResult {
                on: false,
                ok: false,
                ..
            }
        ));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_with_unbuildable_settings_fails_without_a_request() {
        let mut r = rig("test-bad", None).await;
        let me = Origin::Web(5);
        r.h.command(me, Command::TestSpeakerPlug { on: true })
            .await
            .unwrap();
        let env = r.next().await;
        assert!(matches!(
            env.event,
            ServerEvent::SpeakerPlugResult { ok: false, .. }
        ));
        assert!(r.fake.calls().is_empty());
        r.cleanup().await;
    }

    // ------------------------------------------------------------- the API ---

    #[tokio::test(start_paused = true)]
    async fn api_commands_report_their_real_outcome() {
        let mut r = rig("api", None).await;
        assert_eq!(
            r.h.api_command(Command::Set(Param::MainVol { db: -12.0 }))
                .await
                .unwrap(),
            Ok(())
        );
        // The change is broadcast like any other.
        let env = r.next().await;
        assert_eq!(changed_main(&env.event), Some(-12.0));
        let err =
            r.h.api_command(Command::Set(Param::MainVol { db: f64::NAN }))
                .await
                .unwrap()
                .unwrap_err();
        assert!(
            !err.not_found && err.message.contains("main_vol"),
            "{err:?}"
        );
        let err =
            r.h.api_command(Command::LoadPreset {
                name: "nope".into(),
            })
            .await
            .unwrap()
            .unwrap_err();
        assert!(err.not_found);
        // An error does not leak into the next call.
        assert_eq!(
            r.h.api_command(Command::Power {
                channel: PowerChannel::Phones,
                on: true
            })
            .await
            .unwrap(),
            Ok(())
        );
        let s = r.h.summary().await.unwrap();
        assert_eq!(s.api, API_VERSION);
        assert_eq!(s.main.db, -12.0);
        assert!(s.main.on && s.main.plug.is_none());
        assert_eq!(s.motu.status, crate::motu::client::ConnectionStatus::Online);
        r.cleanup().await;
    }

    // --------------------------------------------- the audit's follow-ups ---

    fn steps_of(events: &[(Instant, Envelope)]) -> Vec<f64> {
        events
            .iter()
            .filter_map(|(_, e)| changed_main(&e.event))
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn losing_the_link_stops_everything_and_never_touches_the_plug() {
        let mut r = rig("link", Some(plug_settings())).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.link
            .events
            .send(DeviceEvent::Status(
                crate::motu::client::ConnectionStatus::Demo,
            ))
            .await
            .unwrap();
        r.until(|e| matches!(e, ServerEvent::Status { .. })).await;
        let later = r.quiet_for(Duration::from_secs(3)).await;
        assert!(
            later.iter().all(|e| changed_main(&e.event).is_none()),
            "the fade stopped"
        );
        assert!(
            r.fake.calls().is_empty(),
            "no plug request when the link drops"
        );
        let (main, _) = r.power_event().await;
        assert_eq!((main.fading, main.plug), (None, Some(PlugState::Unknown)));

        // Pressing Off now still moves the (virtual) knob, but leaves the plug alone and says so.
        r.power(PowerChannel::Main, false).await;
        let events = r
            .collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        assert!(events.iter().any(|(_, e)| matches!(
            &e.event,
            ServerEvent::Notice { level: NoticeLevel::Warning, message }
                if message == "MOTU not connected: speaker plug left as is"
        )));
        let _ = r.quiet_for(Duration::from_secs(2)).await;
        assert!(r.fake.calls().is_empty(), "plug untouched while offline");
        // And On, offline: fade only, plus the notice.
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert!(events
            .iter()
            .any(|(_, e)| matches!(&e.event, ServerEvent::Notice { .. })));
        let _ = r.quiet_for(Duration::from_secs(3)).await;
        assert!(r.fake.calls().is_empty());
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_plug_wait_is_dropped_when_the_link_drops() {
        let mut r = rig("link-warm", Some(plug_settings())).await;
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        r.until(|e| matches!(e, ServerEvent::Power { main, .. } if main.plug == Some(PlugState::Warming)))
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        r.link
            .events
            .send(DeviceEvent::Status(
                crate::motu::client::ConnectionStatus::Connecting,
            ))
            .await
            .unwrap();
        let later = r.quiet_for(Duration::from_secs(5)).await;
        assert!(
            later.iter().all(|e| changed_main(&e.event).is_none()),
            "no fade after the warm-up"
        );
        assert_eq!(r.fake.calls().len(), 1, "only the request already made");
        let (main, _) = r.power_event().await;
        assert_eq!((main.fading, main.plug), (None, Some(PlugState::Unknown)));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_level_is_only_remembered_when_read_from_the_card() {
        let mut r = rig_offline("offline-mem", None).await;
        // Demo state: Main sits at 0 dB, which nobody read from hardware.
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        assert_eq!(
            r.saved().await,
            Remembered::default(),
            "nothing remembered, nothing saved"
        );
        assert!(!r.dir.join("power.json").exists());
        r.power(PowerChannel::Main, true).await;
        r.collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert_eq!(
            r.main_db().await,
            power::FADE_SAFE_DB,
            "never back up to the invented 0 dB"
        );
        // Online but not yet synced is the same.
        r.link
            .events
            .send(DeviceEvent::Status(
                crate::motu::client::ConnectionStatus::Online,
            ))
            .await
            .unwrap();
        r.until(|e| matches!(e, ServerEvent::Status { .. })).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        assert_eq!(r.saved().await, Remembered::default());
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn plug_requests_never_overlap_and_the_latest_wish_wins() {
        let mut r = rig("serial", Some(plug_settings())).await;
        *r.fake.delay.lock().unwrap() = Duration::from_millis(1000);
        r.set_main(-100.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        r.power(PowerChannel::Main, false).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        r.power(PowerChannel::Main, true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        r.power(PowerChannel::Main, false).await;
        let _ = r.quiet_for(Duration::from_secs(6)).await;
        let calls = r.fake.calls();
        assert_eq!(
            r.fake.max_active.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(calls[0].0);
        assert_eq!(calls[0].1 - t0, Duration::ZERO);
        // The second request waits for the first, and the wishes in between are skipped.
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(!calls[1].0);
        assert!(calls[1].1 - t0 >= Duration::from_millis(1000));
        let (main, _) = r.power_event().await;
        assert_eq!(main.plug, Some(PlugState::Off));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_plug_already_in_the_wished_state_is_not_asked_again() {
        let mut r = rig("applied", Some(plug_settings())).await;
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        r.collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        // Knob down by hand, then On again: the plug is known to be on already.
        r.set(Param::MainVol { db: -100.0 }).await;
        let _ = r.quiet_for(Duration::from_millis(10)).await;
        r.power(PowerChannel::Main, true).await;
        r.collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert_eq!(r.fake.calls().len(), 1);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn tests_are_refused_during_a_sequence_and_when_monitor_is_up() {
        let mut r = rig("test-refuse", Some(plug_settings())).await;
        let me = Origin::Web(5);
        // Monitor is up (0 dB): a test On would power the speakers at that level.
        r.h.command(me, Command::TestSpeakerPlug { on: true })
            .await
            .unwrap();
        let env = r.next().await;
        let ServerEvent::SpeakerPlugResult { ok, message, .. } = env.event else {
            panic!("result")
        };
        assert!(
            !ok && message.contains("Turn Monitor off first"),
            "{message}"
        );
        // A test Off is fine whatever the level.
        r.h.command(me, Command::TestSpeakerPlug { on: false })
            .await
            .unwrap();
        let ev = r
            .until(|e| matches!(e, ServerEvent::SpeakerPlugResult { .. }))
            .await;
        assert!(matches!(
            ev,
            ServerEvent::SpeakerPlugResult { ok: true, .. }
        ));
        assert_eq!(r.fake.calls().len(), 1);
        // During the warm-up nothing is tested.
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        for on in [true, false] {
            r.h.command(me, Command::TestSpeakerPlug { on })
                .await
                .unwrap();
            let ev = r
                .until(|e| matches!(e, ServerEvent::SpeakerPlugResult { .. }))
                .await;
            let ServerEvent::SpeakerPlugResult { ok, message, .. } = ev else {
                panic!()
            };
            assert!(!ok && message.contains("try again"), "{message}");
        }
        assert_eq!(r.fake.calls().len(), 2, "the sequence's own request only");
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn late_echoes_after_the_fade_ended_are_not_a_knob_move() {
        let mut r = rig("echo-end", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        let events = r
            .collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        let steps = steps_of(&events);
        let echo = *steps.iter().rev().find(|v| **v > -99.0).unwrap();
        // The hardware reports one of the last steps after the fade is over.
        r.link
            .events
            .send(DeviceEvent::Changed(vec![Param::MainVol { db: echo }]))
            .await
            .unwrap();
        r.until(|e| changed_main(e) == Some(echo)).await;
        // It did not count as a move to a real level: the remembered level is still there.
        assert_eq!(r.saved().await.main, Some(-20.0));
        // A level nowhere near the steps is a move.
        r.link
            .events
            .send(DeviceEvent::Changed(vec![Param::MainVol { db: -9.0 }]))
            .await
            .unwrap();
        r.until(|e| changed_main(e) == Some(-9.0)).await;
        assert_eq!(r.saved().await.main, None);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn echoes_of_the_old_steps_do_not_cancel_a_reversed_fade() {
        let mut r = rig("echo-reverse", None).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        let mut out = Vec::new();
        while out.len() < 6 {
            let ev = r.until(|e| changed_main(e).is_some()).await;
            out.extend(changed_main(&ev));
        }
        r.power(PowerChannel::Main, true).await;
        let older = out[out.len() - 2];
        assert!((out[out.len() - 1] - older).abs() > 1.0, "{out:?}");
        r.link
            .events
            .send(DeviceEvent::Changed(vec![Param::MainVol { db: older }]))
            .await
            .unwrap();
        r.collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        assert_eq!(
            r.main_db().await,
            -20.0,
            "the fade in ran to the remembered level"
        );
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn saves_apply_in_order_even_when_their_checks_finish_out_of_order() {
        let mut r = rig("tickets", None).await;
        let me = Origin::Web(9);
        let mut slow = plug_settings();
        slow.host = "slow.example".into();
        let mut fast = plug_settings();
        fast.host = "fast.example".into();
        r.h.command(me, Command::SetSpeakerPlug { settings: slow })
            .await
            .unwrap();
        r.h.command(me, Command::SetSpeakerPlug { settings: fast })
            .await
            .unwrap();
        let ev = r
            .until(|e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if settings.enabled))
            .await;
        let ServerEvent::SpeakerPlug { settings, .. } = ev else {
            panic!()
        };
        assert_eq!(settings.host, "fast.example");
        let ev = r.until(|e| matches!(e, ServerEvent::Error { .. })).await;
        let ServerEvent::Error { message } = ev else {
            panic!()
        };
        assert!(message.contains("superseded"), "{message}");
        let events = r.h.initial_events().await.unwrap();
        assert!(events.iter().any(|e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if settings.host == "fast.example")));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_from_the_old_plug_is_ignored_after_a_settings_change() {
        let mut r = rig("old-plug", Some(plug_settings())).await;
        *r.fake.delay.lock().unwrap() = Duration::from_millis(1000);
        r.set_main(-100.0).await;
        r.power(PowerChannel::Main, true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut other = plug_settings();
        other.host = "other.example".into();
        r.h.command(Origin::Web(9), Command::SetSpeakerPlug { settings: other })
            .await
            .unwrap();
        let later = r.quiet_for(Duration::from_secs(5)).await;
        assert!(
            later.iter().all(|e| changed_main(&e.event).is_none()),
            "no warm-up, no fade"
        );
        let (main, _) = r.power_event().await;
        assert_eq!((main.fading, main.plug), (None, Some(PlugState::Unknown)));
        assert_eq!(r.fake.calls().len(), 1);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn on_with_the_knob_up_and_the_plug_not_on_parks_the_knob_then_runs_the_sequence() {
        let mut r = rig("park", Some(plug_settings())).await;
        r.set_main(-20.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none() && m.plug == Some(PlugState::On))
            .await;
        let steps = steps_of(&events);
        assert_eq!(steps.first(), Some(&-100.0), "parked at the minimum first");
        assert_eq!(
            events
                .iter()
                .find(|(_, e)| changed_main(&e.event).is_some())
                .map(|(at, _)| *at - t0),
            Some(Duration::ZERO)
        );
        let calls = r.fake.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0);
        let (end, _) = events.last().unwrap();
        // warm-up 2 s, then 80 dB back up to the level it had.
        assert_eq!(*end - t0, Duration::from_millis(2000 + 960));
        assert_eq!(r.main_db().await, -20.0);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn on_during_the_off_delay_fades_in_without_touching_the_plug() {
        let mut s = plug_settings();
        s.off_delay_ms = 1000;
        let mut r = rig("delay-on", Some(s)).await;
        r.set_main(-20.0).await;
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| !m.on && m.fading.is_none())
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let t_on = Instant::now();
        r.power(PowerChannel::Main, true).await;
        let events = r
            .collect_until_power(|m, _| m.on && m.fading.is_none())
            .await;
        let first = events
            .iter()
            .find(|(_, e)| changed_main(&e.event).is_some())
            .map(|(at, _)| *at - t_on)
            .unwrap();
        assert!(
            first <= Duration::from_millis(3 * power::FADE_STEP_MS),
            "no warm-up: {first:?}"
        );
        let _ = r.quiet_for(Duration::from_secs(3)).await;
        assert!(r.fake.calls().is_empty(), "neither off nor on was sent");
        assert_eq!(r.main_db().await, -20.0);
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_off_delay_is_at_least_50_ms() {
        let mut s = plug_settings();
        s.off_delay_ms = 0;
        let mut r = rig("min-delay", Some(s)).await;
        r.set_main(-20.0).await;
        let t0 = Instant::now();
        r.power(PowerChannel::Main, false).await;
        r.collect_until_power(|m, _| m.plug == Some(PlugState::Off))
            .await;
        assert_eq!(r.fake.calls()[0].1 - t0, Duration::from_millis(960 + 50));
        r.cleanup().await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreadable_plug_file_disables_the_plug() {
        let dir = tmp("bad-file");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("speaker-plug.json"), "{ not json")
            .await
            .unwrap();
        let r = rig_in(dir, Some(plug_settings()), FakePlug::new()).await;
        let events = r.h.initial_events().await.unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, ServerEvent::SpeakerPlug { settings, .. } if !settings.enabled)));
        r.cleanup().await;
    }
}
