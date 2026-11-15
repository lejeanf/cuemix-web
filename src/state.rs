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
    LoadPreset { name: String },
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
