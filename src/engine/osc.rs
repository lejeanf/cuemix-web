//! OSC settings (`set_osc_settings`, `osc_settings` event): part of the engine actor so it can
//! use the engine's private state, like `sequence`.
//!
//! The engine owns the current settings and the answer to "is the bridge listening". A change
//! is checked here (syntax), resolved and fenced in a task (names, same fence as the speaker
//! plug), applied by the bridge (rebind first, old socket kept on failure) and only then saved
//! to `osc.json` and broadcast. One change runs at a time.
//!
//! Target names that did not resolve at startup are tried again (every minute) by the
//! same path (a tick from a small task), and newly resolved targets get a full dump.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use super::{Engine, Internal, Origin, Target};
use crate::osc::bridge::{ApplyOutcome, OscApply};
use crate::osc::settings::{self, OscSettings, Resolved};
use crate::state::ServerEvent;

/// Current OSC settings and the link to the running bridge.
pub(super) struct OscRuntime {
    settings: OscSettings,
    file: PathBuf,
    /// The bridge answers `OscApply` here once it runs.
    ctl: Option<mpsc::Sender<OscApply>>,
    /// The bridge has a real UDP socket bound (and, with `ctl` open, still runs).
    listening: bool,
    /// Last problem with the settings (saved ones not used, bind or resolve failure of a
    /// change); `None` when fine, cleared by the next change that works.
    error: Option<String>,
    /// Target names that did not resolve yet.
    unresolved: Vec<String>,
    /// A change is being checked or applied.
    busy: bool,
    retry_every: Duration,
}

impl OscRuntime {
    pub(super) fn new(
        settings: OscSettings,
        file: PathBuf,
        error: Option<String>,
        unresolved: Vec<String>,
        retry_every: Duration,
    ) -> Self {
        Self {
            settings,
            file,
            ctl: None,
            listening: false,
            error,
            unresolved,
            busy: false,
            retry_every,
        }
    }
}

/// What a finished change tells the engine.
#[derive(Debug)]
pub(super) struct OscDone {
    /// Cleaned-up settings (a user change; `None` for a retry, which changes none).
    clean: Option<OscSettings>,
    outcome: ApplyOutcome,
    unresolved: Vec<String>,
    /// `error` to show when everything worked except some targets.
    note: Option<String>,
}

impl Engine {
    pub(super) fn osc_event(&self) -> ServerEvent {
        let running = self.osc.ctl.as_ref().is_some_and(|c| !c.is_closed());
        ServerEvent::OscSettings {
            settings: self.osc.settings.clone(),
            listening: self.osc.listening && running,
            error: self.osc.error.clone(),
        }
    }

    pub(super) fn on_osc_attach(&mut self, ctl: mpsc::Sender<OscApply>) {
        self.osc.ctl = Some(ctl);
        self.osc.listening = true;
        let tx = self.internal.clone();
        let every = self.osc.retry_every;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.tick().await;
            loop {
                tick.tick().await;
                if tx.send(Internal::OscTick).is_err() {
                    break;
                }
            }
        });
        self.emit(Origin::System, Target::All, self.osc_event());
    }

    /// `set_osc_settings`: syntax here, names and fence in a task, then the bridge.
    pub(super) fn on_set_osc_settings(&mut self, origin: Origin, requested: OscSettings) {
        let Some(ctl) = self.osc.ctl.clone() else {
            self.reply_error(origin, "OSC is not running".into());
            return;
        };
        if self.osc.busy {
            self.reply_error(origin, "another OSC change is in progress".into());
            return;
        }
        if let Err(e) = requested.validate() {
            self.reply_error(origin, format!("invalid OSC settings: {e}"));
            return;
        }
        self.osc.busy = true;
        let tx = self.internal.clone();
        tokio::spawn(async move {
            let done = async {
                let (clean, resolved) = requested.resolve().await?;
                let outcome = push(&ctl, resolved).await?;
                Ok(OscDone {
                    clean: Some(clean),
                    outcome,
                    unresolved: Vec::new(),
                    note: None,
                })
            }
            .await;
            let _ = tx.send(Internal::OscApplied { origin, done });
        });
    }

    /// Once a minute: try the target names that did not resolve yet.
    pub(super) fn on_osc_tick(&mut self) {
        let Some(ctl) = self.osc.ctl.clone() else {
            return;
        };
        if self.osc.unresolved.is_empty() || self.osc.busy {
            return;
        }
        self.osc.busy = true;
        let current = self.osc.settings.clone();
        let tx = self.internal.clone();
        tokio::spawn(async move {
            let done = async {
                let l = current.resolve_lenient().await?;
                let outcome = push(&ctl, l.resolved.clone()).await?;
                Ok(OscDone {
                    clean: None,
                    outcome,
                    note: l.error_text(),
                    unresolved: l.retry,
                })
            }
            .await;
            let _ = tx.send(Internal::OscApplied {
                origin: Origin::System,
                done,
            });
        });
    }

    pub(super) async fn on_osc_applied(&mut self, origin: Origin, done: Result<OscDone, String>) {
        self.osc.busy = false;
        let retry = origin == Origin::System;
        let failure = match done {
            Err(e) => Some(e),
            Ok(d) => {
                self.osc.listening = d.outcome.listening;
                match d.outcome.error {
                    Some(e) => Some(e),
                    None => {
                        self.finish_osc(origin, d.clean, d.unresolved, d.note).await;
                        return;
                    }
                }
            }
        };
        if let Some(e) = failure {
            // Nothing changed, but what went wrong is part of the state the UI shows.
            self.osc.error = Some(e.clone());
            self.emit(Origin::System, Target::All, self.osc_event());
            if retry {
                tracing::debug!(error = %e, "OSC target retry failed");
            } else {
                self.reply_error(origin, format!("OSC settings refused: {e}"));
            }
        }
    }

    async fn finish_osc(
        &mut self,
        origin: Origin,
        clean: Option<OscSettings>,
        unresolved: Vec<String>,
        note: Option<String>,
    ) {
        // The bridge already runs with the settings: they are the current ones even if saving
        // fails. Saved before the broadcast, so a client that sees them knows they are kept.
        let saved = match &clean {
            Some(c) => Some(settings::save(&self.osc.file, c).await),
            None => None,
        };
        if let Some(c) = clean {
            self.osc.settings = c;
        }
        self.osc.unresolved = unresolved;
        self.osc.error = note;
        self.emit(Origin::System, Target::All, self.osc_event());
        if let Some(Err(e)) = saved {
            self.reply_error(
                origin,
                format!("OSC settings applied but not saved (they are lost at restart): {e:#}"),
            );
        }
    }
}

/// Hand resolved settings to the bridge and wait for its answer.
async fn push(ctl: &mpsc::Sender<OscApply>, resolved: Resolved) -> Result<ApplyOutcome, String> {
    let stopped = || "the OSC bridge is not running".to_owned();
    let (reply, answer) = oneshot::channel();
    ctl.send(OscApply {
        listen: resolved.listen,
        targets: resolved.targets,
        reply,
    })
    .await
    .map_err(|_| stopped())?;
    answer.await.map_err(|_| stopped())
}
