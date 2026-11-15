//! Central state contract shared by every component.
//!
//! - [`MotuState`]   — full device state (serialised as the JSON snapshot and
//!   as the body of a preset file).
//! - [`Param`]       — one *set-style, idempotent* parameter change. The same
//!   type is used **in both directions**: a client asks for a change, and the
//!   server broadcasts the *effective* (clamped) change it applied. Web JSON
//!   and OSC are two encodings of `Param`.
//! - [`Command`]     — everything a client may ask (`Param` + preset load).
//! - [`ServerEvent`] — everything the core broadcasts to Web/OSC clients.
//!
//! Indexing: **0-based** everywhere inside the server and in JSON. The OSC
//! layer converts to/from **1-based** (see `osc::address`).
//!
//! Value policy at the boundary: non-finite numbers and out-of-bounds indices
//! are rejected ([`StateError`]); finite out-of-range values are clamped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::motu::client::ConnectionStatus;
use crate::motu::fader;

/// Hardware dimensions. Values for the UltraLite-mk5 are **provisional** until
/// confirmed against the device (open question Q2 in `docs/ARCHITECTURE.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    /// Independent stereo mixes (HANDOFF: 7).
    pub mixes: u8,
    /// Mixer input channels (strips per mix).
    pub mixer_inputs: u16,
    /// Mic/line preamps.
    pub preamps: u16,
    /// Routing matrix sources (rows).
    pub routing_inputs: u16,
    /// Routing matrix destinations (columns).
    pub routing_outputs: u16,
}

impl Topology {
    pub const ULTRALITE_MK5: Topology = Topology {
        mixes: 7,
        mixer_inputs: 18,
        preamps: 2,
        routing_inputs: 18,
        routing_outputs: 22,
    };
}

impl Default for Topology {
    fn default() -> Self {
        Self::ULTRALITE_MK5
    }
}

// ---------------------------------------------------------------------------
// State tree
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotuState {
    pub topology: Topology,
    pub mixes: Vec<Mix>,
    pub main: MainOut,
    pub preamps: Vec<Preamp>,
    /// Per mixer-input DSP (EQ, compressor, gate), indexed like mix channels.
    pub channels: Vec<ChannelDsp>,
    pub reverb: Reverb,
    pub routing: Routing,
    pub control_room: ControlRoom,
    pub clock: Clock,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mix {
    pub name: String,
    pub channels: Vec<MixChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MixChannel {
    /// Normalised fader position `0.0..=1.0` (see [`fader`]; `0.75` = 0 dB).
    pub vol: f64,
    /// `-1.0` (L) ..= `1.0` (R), `0.0` = centre.
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
}

impl Default for MixChannel {
    fn default() -> Self {
        Self {
            vol: fader::UNITY_POS,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MainOut {
    /// Normalised fader position `0.0..=1.0`.
    pub vol: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Preamp {
    /// `0.0..=60.0` dB.
    pub gain_db: f64,
    pub phantom: bool,
    pub pad: bool,
    pub phase: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ChannelDsp {
    pub eq: Equalizer,
    pub comp: Compressor,
    pub gate: Gate,
}

/// 4-band parametric EQ + HPF. Physical units here; their wire encoding is
/// open question Q1 (frequencies do not fit in `8.24`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Equalizer {
    /// Global bypass of the 4 bands (cuemix-web improvement over CueMix 5).
    pub bypass: bool,
    pub hpf: Hpf,
    pub bands: [EqBand; 4],
}

impl Default for Equalizer {
    fn default() -> Self {
        let band = |freq_hz| EqBand {
            enabled: false,
            freq_hz,
            gain_db: 0.0,
            q: 0.7,
        };
        Self {
            bypass: false,
            hpf: Hpf {
                enabled: false,
                freq_hz: 80.0,
            },
            bands: [band(100.0), band(500.0), band(2_000.0), band(8_000.0)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hpf {
    pub enabled: bool,
    pub freq_hz: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled: bool,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Compressor {
    pub enabled: bool,
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub gain_db: f64,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            gain_db: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub enabled: bool,
    pub threshold_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -50.0,
            attack_ms: 1.0,
            release_ms: 100.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reverb {
    pub enabled: bool,
    /// Normalised return fader position.
    pub return_vol: f64,
    pub time_s: f64,
    pub predelay_ms: f64,
    /// Per mixer-input send level (normalised fader position).
    pub sends: Vec<f64>,
}

/// Routing crosspoints, `matrix[input][output]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    pub matrix: Vec<Vec<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlRoom {
    /// −20 dB dim.
    pub dim: bool,
    pub mono: bool,
    /// `false` = speakers A, `true` = speakers B.
    pub speaker_b: bool,
    pub talkback: bool,
    /// Headphone fader position, normalised.
    pub phones_vol: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Clock {
    pub sample_rate: SampleRate,
    pub source: ClockSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SampleRate {
    #[serde(rename = "44100")]
    Hz44100,
    #[serde(rename = "48000")]
    Hz48000,
    #[serde(rename = "88200")]
    Hz88200,
    #[serde(rename = "96000")]
    Hz96000,
    #[serde(rename = "176400")]
    Hz176400,
    #[serde(rename = "192000")]
    Hz192000,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    Internal,
    Spdif,
    Optical,
}

impl MotuState {
    /// Neutral initial state (also the offline demo state).
    pub fn new(topology: Topology) -> Self {
        let inputs = usize::from(topology.mixer_inputs);
        Self {
            topology,
            mixes: (1..=topology.mixes)
                .map(|n| Mix {
                    name: format!("Mix {n}"),
                    channels: vec![MixChannel::default(); inputs],
                })
                .collect(),
            main: MainOut {
                vol: fader::UNITY_POS,
            },
            preamps: vec![Preamp::default(); usize::from(topology.preamps)],
            channels: vec![ChannelDsp::default(); inputs],
            reverb: Reverb {
                enabled: false,
                return_vol: 0.0,
                time_s: 2.0,
                predelay_ms: 20.0,
                sends: vec![0.0; inputs],
            },
            routing: Routing {
                matrix: vec![
                    vec![false; usize::from(topology.routing_outputs)];
                    usize::from(topology.routing_inputs)
                ],
            },
            control_room: ControlRoom {
                dim: false,
                mono: false,
                speaker_b: false,
                talkback: false,
                phones_vol: fader::UNITY_POS,
            },
            clock: Clock {
                sample_rate: SampleRate::Hz48000,
                source: ClockSource::Internal,
            },
        }
    }
}

impl Default for MotuState {
    fn default() -> Self {
        Self::new(Topology::default())
    }
}

// ---------------------------------------------------------------------------
// Commands & events
// ---------------------------------------------------------------------------

/// Preamp gain range in dB.
pub const PREAMP_GAIN_MAX_DB: f64 = 60.0;

/// One set-style parameter (0-based indices). JSON: `{"param":"mix_vol","bus":0,"ch":3,"value":0.75}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "param", rename_all = "snake_case")]
pub enum Param {
    MixVol { bus: u8, ch: u16, value: f64 },
    MixPan { bus: u8, ch: u16, value: f64 },
    MixMute { bus: u8, ch: u16, on: bool },
    MixSolo { bus: u8, ch: u16, on: bool },
    MainVol { value: f64 },
    PreampGain { ch: u16, db: f64 },
    Phantom { ch: u16, on: bool },
    Pad { ch: u16, on: bool },
    Phase { ch: u16, on: bool },
    EqBypass { ch: u16, on: bool },
    Route { input: u16, output: u16, on: bool },
    Dim { on: bool },
    Mono { on: bool },
    SpeakerB { on: bool },
    Talkback { on: bool },
    PhonesVol { value: f64 },
}

/// Everything a Web or OSC client may request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Set(Param),
    /// Recall `<presets_dir>/<name>.json`.
    LoadPreset {
        name: String,
    },
    /// Store the live state as `<presets_dir>/<name>.json` and make it active.
    SavePreset {
        name: String,
    },
    /// Ask for a [`ServerEvent::Presets`] listing.
    ListPresets,
    /// Rename / recolour a mixer input (`None` clears it) — persisted in `labels.json`.
    SetLabel {
        ch: u16,
        label: Option<ChannelLabel>,
    },
}

/// Everything the core broadcasts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Full state (on connect, after hardware sync, after preset load).
    Snapshot {
        state: Box<MotuState>,
        session: SessionInfo,
    },
    /// Effective changes, already clamped.
    Changed { params: Vec<Param> },
    /// Link status change.
    Status { connection: ConnectionStatus },
    /// Session metadata (active preset, `● Modifié` flag).
    Session(SessionInfo),
    /// Available preset names (sorted).
    Presets { names: Vec<String> },
    /// Full label set (sent on connect and after each change).
    Labels(Labels),
    /// Meter levels, linear peak `0.0..=1.0`, one per meter point (layout from
    /// the property table). High rate: bridges may drop/decimate.
    Meters { levels: Vec<f32> },
    /// Request rejected; delivered only to the requesting client.
    Error { message: String },
}

/// Top-bar data: active preset and whether the live state diverges from it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub active_preset: Option<String>,
    pub modified: bool,
    pub connection: ConnectionStatus,
}

#[derive(Debug, Error, PartialEq)]
pub enum StateError {
    #[error("non-finite value for {0}")]
    NonFinite(&'static str),
    #[error("{what} index {index} out of range (len {len})")]
    Index {
        what: &'static str,
        index: usize,
        len: usize,
    },
}

fn finite(value: f64, what: &'static str) -> Result<f64, StateError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(StateError::NonFinite(what))
    }
}

fn slot<'a, T>(
    items: &'a mut [T],
    index: usize,
    what: &'static str,
) -> Result<&'a mut T, StateError> {
    let len = items.len();
    items
        .get_mut(index)
        .ok_or(StateError::Index { what, index, len })
}

/// Write `new` into `field`; return whether it changed.
fn set<T: PartialEq>(field: &mut T, new: T) -> bool {
    if *field == new {
        false
    } else {
        *field = new;
        true
    }
}

impl MotuState {
    fn mix_channel(&mut self, bus: u8, ch: u16) -> Result<&mut MixChannel, StateError> {
        let mix = slot(&mut self.mixes, usize::from(bus), "mix")?;
        slot(&mut mix.channels, usize::from(ch), "mix channel")
    }

    /// Apply one change. Returns the *effective* param (clamped) when the state
    /// changed, `None` when it was already at that value (no feedback needed).
    pub fn apply(&mut self, param: &Param) -> Result<Option<Param>, StateError> {
        use Param as P;
        let (changed, effective) = match *param {
            P::MixVol { bus, ch, value } => {
                let v = finite(value, "mix_vol")?.clamp(0.0, 1.0);
                (
                    set(&mut self.mix_channel(bus, ch)?.vol, v),
                    P::MixVol { bus, ch, value: v },
                )
            }
            P::MixPan { bus, ch, value } => {
                let v = finite(value, "mix_pan")?.clamp(-1.0, 1.0);
                (
                    set(&mut self.mix_channel(bus, ch)?.pan, v),
                    P::MixPan { bus, ch, value: v },
                )
            }
            P::MixMute { bus, ch, on } => (set(&mut self.mix_channel(bus, ch)?.mute, on), *param),
            P::MixSolo { bus, ch, on } => (set(&mut self.mix_channel(bus, ch)?.solo, on), *param),
            P::MainVol { value } => {
                let v = finite(value, "main_vol")?.clamp(0.0, 1.0);
                (set(&mut self.main.vol, v), P::MainVol { value: v })
            }
            P::PreampGain { ch, db } => {
                let v = finite(db, "preamp_gain")?.clamp(0.0, PREAMP_GAIN_MAX_DB);
                (
                    set(
                        &mut slot(&mut self.preamps, usize::from(ch), "preamp")?.gain_db,
                        v,
                    ),
                    P::PreampGain { ch, db: v },
                )
            }
            P::Phantom { ch, on } => (
                set(
                    &mut slot(&mut self.preamps, usize::from(ch), "preamp")?.phantom,
                    on,
                ),
                *param,
            ),
            P::Pad { ch, on } => (
                set(
                    &mut slot(&mut self.preamps, usize::from(ch), "preamp")?.pad,
                    on,
                ),
                *param,
            ),
            P::Phase { ch, on } => (
                set(
                    &mut slot(&mut self.preamps, usize::from(ch), "preamp")?.phase,
                    on,
                ),
                *param,
            ),
            P::EqBypass { ch, on } => (
                set(
                    &mut slot(&mut self.channels, usize::from(ch), "channel")?
                        .eq
                        .bypass,
                    on,
                ),
                *param,
            ),
            P::Route { input, output, on } => {
                let row = slot(
                    &mut self.routing.matrix,
                    usize::from(input),
                    "routing input",
                )?;
                (
                    set(slot(row, usize::from(output), "routing output")?, on),
                    *param,
                )
            }
            P::Dim { on } => (set(&mut self.control_room.dim, on), *param),
            P::Mono { on } => (set(&mut self.control_room.mono, on), *param),
            P::SpeakerB { on } => (set(&mut self.control_room.speaker_b, on), *param),
            P::Talkback { on } => (set(&mut self.control_room.talkback, on), *param),
            P::PhonesVol { value } => {
                let v = finite(value, "phones_vol")?.clamp(0.0, 1.0);
                (
                    set(&mut self.control_room.phones_vol, v),
                    P::PhonesVol { value: v },
                )
            }
        };
        Ok(changed.then_some(effective))
    }

    /// Every [`Param`] needed to turn `self` into `target`, for the fields that
    /// `Param` can address (used for preset recall). Shapes are compared
    /// index-by-index over the overlap; extra rows/columns are ignored.
    pub fn diff(&self, target: &MotuState) -> Vec<Param> {
        self.params_between(target, false)
    }

    /// Every addressable [`Param`] with its current value (full OSC feedback
    /// dump after a snapshot).
    pub fn all_params(&self) -> Vec<Param> {
        self.params_between(self, true)
    }

    fn params_between(&self, target: &MotuState, all: bool) -> Vec<Param> {
        let mut out = Vec::new();
        for (bus, (a, b)) in self.mixes.iter().zip(&target.mixes).enumerate() {
            let Ok(bus) = u8::try_from(bus) else { break };
            for (ch, (x, y)) in a.channels.iter().zip(&b.channels).enumerate() {
                let Ok(ch) = u16::try_from(ch) else { break };
                if all || x.vol != y.vol {
                    out.push(Param::MixVol {
                        bus,
                        ch,
                        value: y.vol,
                    });
                }
                if all || x.pan != y.pan {
                    out.push(Param::MixPan {
                        bus,
                        ch,
                        value: y.pan,
                    });
                }
                if all || x.mute != y.mute {
                    out.push(Param::MixMute {
                        bus,
                        ch,
                        on: y.mute,
                    });
                }
                if all || x.solo != y.solo {
                    out.push(Param::MixSolo {
                        bus,
                        ch,
                        on: y.solo,
                    });
                }
            }
        }
        if all || self.main.vol != target.main.vol {
            out.push(Param::MainVol {
                value: target.main.vol,
            });
        }
        for (ch, (x, y)) in self.preamps.iter().zip(&target.preamps).enumerate() {
            let Ok(ch) = u16::try_from(ch) else { break };
            if all || x.gain_db != y.gain_db {
                out.push(Param::PreampGain { ch, db: y.gain_db });
            }
            if all || x.phantom != y.phantom {
                out.push(Param::Phantom { ch, on: y.phantom });
            }
            if all || x.pad != y.pad {
                out.push(Param::Pad { ch, on: y.pad });
            }
            if all || x.phase != y.phase {
                out.push(Param::Phase { ch, on: y.phase });
            }
        }
        for (ch, (x, y)) in self.channels.iter().zip(&target.channels).enumerate() {
            let Ok(ch) = u16::try_from(ch) else { break };
            if all || x.eq.bypass != y.eq.bypass {
                out.push(Param::EqBypass {
                    ch,
                    on: y.eq.bypass,
                });
            }
        }
        for (input, (ra, rb)) in self
            .routing
            .matrix
            .iter()
            .zip(&target.routing.matrix)
            .enumerate()
        {
            let Ok(input) = u16::try_from(input) else {
                break;
            };
            for (output, (x, y)) in ra.iter().zip(rb).enumerate() {
                let Ok(output) = u16::try_from(output) else {
                    break;
                };
                if all || x != y {
                    out.push(Param::Route {
                        input,
                        output,
                        on: *y,
                    });
                }
            }
        }
        let (a, b) = (&self.control_room, &target.control_room);
        if all || a.dim != b.dim {
            out.push(Param::Dim { on: b.dim });
        }
        if all || a.mono != b.mono {
            out.push(Param::Mono { on: b.mono });
        }
        if all || a.speaker_b != b.speaker_b {
            out.push(Param::SpeakerB { on: b.speaker_b });
        }
        if all || a.talkback != b.talkback {
            out.push(Param::Talkback { on: b.talkback });
        }
        if all || a.phones_vol != b.phones_vol {
            out.push(Param::PhonesVol {
                value: b.phones_vol,
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Presets & labels (persisted server-side, git-ignored data/)
// ---------------------------------------------------------------------------

/// A named snapshot stored as `<presets_dir>/<name>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub state: MotuState,
}

impl Preset {
    /// `● Modifié` indicator: live state diverges from this snapshot.
    pub fn is_modified(&self, live: &MotuState) -> bool {
        &self.state != live
    }
}

/// `labels.json`: user names and colours per mixer input (0-based keys).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Labels {
    pub channels: BTreeMap<u16, ChannelLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelLabel {
    pub name: String,
    /// CSS colour, e.g. `#06B6D4`.
    pub color: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_topology_shapes_state() {
        let s = MotuState::default();
        assert_eq!(s.mixes.len(), 7);
        assert!(s.mixes.iter().all(|m| m.channels.len() == 18));
        assert_eq!(s.preamps.len(), 2);
        assert_eq!(s.routing.matrix.len(), 18);
        assert_eq!(s.routing.matrix[0].len(), 22);
    }

    #[test]
    fn apply_clamps_and_reports_effective_value() {
        let mut s = MotuState::default();
        let r = s.apply(&Param::MixVol {
            bus: 1,
            ch: 2,
            value: 1.7,
        });
        assert_eq!(
            r,
            Ok(Some(Param::MixVol {
                bus: 1,
                ch: 2,
                value: 1.0
            }))
        );
        assert_eq!(s.mixes[1].channels[2].vol, 1.0);
        let r = s.apply(&Param::PreampGain { ch: 0, db: -5.0 });
        assert_eq!(r, Ok(None), "already 0 dB after clamping");
    }

    #[test]
    fn apply_is_idempotent() {
        let mut s = MotuState::default();
        let p = Param::Route {
            input: 3,
            output: 4,
            on: true,
        };
        assert_eq!(s.apply(&p), Ok(Some(p)));
        assert_eq!(s.apply(&p), Ok(None));
    }

    #[test]
    fn apply_rejects_bad_input() {
        let mut s = MotuState::default();
        assert_eq!(
            s.apply(&Param::MainVol { value: f64::NAN }),
            Err(StateError::NonFinite("main_vol"))
        );
        assert!(matches!(
            s.apply(&Param::MixMute {
                bus: 7,
                ch: 0,
                on: true
            }),
            Err(StateError::Index { what: "mix", .. })
        ));
        assert!(matches!(
            s.apply(&Param::Phantom { ch: 2, on: true }),
            Err(StateError::Index { .. })
        ));
        assert!(matches!(
            s.apply(&Param::Route {
                input: 0,
                output: 22,
                on: true
            }),
            Err(StateError::Index { .. })
        ));
        assert_eq!(
            s,
            MotuState::default(),
            "rejected input must not mutate state"
        );
    }

    #[test]
    fn preset_modified_flag() {
        let mut live = MotuState::default();
        let preset = Preset {
            name: "default".into(),
            state: live.clone(),
        };
        assert!(!preset.is_modified(&live));
        let _ = live.apply(&Param::Dim { on: true });
        assert!(preset.is_modified(&live));
    }

    #[test]
    fn json_contract_shapes() {
        let cmd = Command::Set(Param::MixVol {
            bus: 0,
            ch: 3,
            value: 0.75,
        });
        let json = serde_json::to_value(&cmd).unwrap_or_default();
        assert_eq!(
            json,
            serde_json::json!({"cmd":"set","param":"mix_vol","bus":0,"ch":3,"value":0.75})
        );
        let load: Command = serde_json::from_str(r#"{"cmd":"load_preset","name":"live"}"#)
            .unwrap_or(Command::LoadPreset {
                name: String::new(),
            });
        assert_eq!(
            load,
            Command::LoadPreset {
                name: "live".into()
            }
        );
        let ev = ServerEvent::Changed {
            params: vec![Param::Dim { on: true }],
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap_or_default(),
            serde_json::json!({"event":"changed","params":[{"param":"dim","on":true}]})
        );
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let state = MotuState::default();
        let ev = ServerEvent::Snapshot {
            state: Box::new(state),
            session: SessionInfo::default(),
        };
        let text = serde_json::to_string(&ev).unwrap_or_default();
        let back: Result<ServerEvent, _> = serde_json::from_str(&text);
        assert_eq!(back.ok(), Some(ev));
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn diff_replays_to_target() {
        let base = MotuState::default();
        let mut target = base.clone();
        for p in [
            Param::MixVol {
                bus: 3,
                ch: 5,
                value: 0.2,
            },
            Param::MixMute {
                bus: 0,
                ch: 17,
                on: true,
            },
            Param::Phantom { ch: 1, on: true },
            Param::EqBypass { ch: 4, on: true },
            Param::Route {
                input: 2,
                output: 9,
                on: true,
            },
            Param::Talkback { on: true },
            Param::PhonesVol { value: 0.1 },
        ] {
            assert!(target.apply(&p).is_ok());
        }
        let params = base.diff(&target);
        assert_eq!(params.len(), 7);
        let mut replay = base.clone();
        for p in &params {
            assert!(replay.apply(p).is_ok());
        }
        assert_eq!(replay, target);
        assert!(target.diff(&target).is_empty());
    }
}

#[cfg(test)]
mod all_params_tests {
    use super::*;

    #[test]
    fn all_params_covers_every_addressable_field() {
        let s = MotuState::default();
        let t = s.topology;
        let (mixes, inputs) = (usize::from(t.mixes), usize::from(t.mixer_inputs));
        let expected = mixes * inputs * 4 // vol, pan, mute, solo
            + 1 // main
            + usize::from(t.preamps) * 4
            + inputs // eq bypass
            + usize::from(t.routing_inputs) * usize::from(t.routing_outputs)
            + 5; // control room
        let params = s.all_params();
        assert_eq!(params.len(), expected);
        let mut replay = MotuState::default();
        for p in &params {
            assert_eq!(replay.apply(p), Ok(None), "default values replay as no-ops");
        }
    }
}
