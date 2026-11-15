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

/// Hardware dimensions. Values for the UltraLite-mk5 are **provisional** until
/// confirmed against the device (open question Q2 in `docs/ARCHITECTURE.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Topology {
    /// Independent stereo mixes (HANDOFF: 7).
    pub mixes: u8,
    /// Mixer input channels (strips per mix).
    pub mixer_inputs: u16,
    /// Inputs with gain + phase (mic and line preamps).
    pub preamps: u16,
    /// Leading preamps that are mic inputs (only these have +48V and Pad).
    pub mic_inputs: u16,
    /// Routing matrix sources (rows).
    pub routing_inputs: u16,
    /// Routing matrix destinations (columns).
    pub routing_outputs: u16,
}

impl Topology {
    pub const ULTRALITE_MK5: Topology = Topology {
        mixes: 7,
        mixer_inputs: 32,
        preamps: 8,
        mic_inputs: 2,
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
#[serde(default)]
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
    /// Analog line output trims in whole dB (`OUTPUT_TRIMS` entries, 0 dB =
    /// no attenuation). Calibration, not recalled by presets.
    pub output_trims: Vec<f64>,
    pub clock: Clock,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Mix {
    pub name: String,
    pub channels: Vec<MixChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixChannel {
    /// Linear gain, `0.0..=MIX_GAIN_MAX` (`1.0` = 0 dB) — MOTU native (8.24).
    /// The `0..1` fader position exists only at the edges (OSC, UI widget).
    pub gain: f64,
    /// `-1.0` (L) ..= `1.0` (R), `0.0` = centre.
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
}

impl Default for MixChannel {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MainOut {
    /// Main trim, whole dB `TRIM_MIN_DB..=0.0` — MOTU native (U8 attenuation).
    pub db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preamp {
    /// Whole dB `0.0..=PREAMP_GAIN_MAX_DB` — MOTU native (U8).
    pub gain_db: f64,
    pub phantom: bool,
    pub pad: bool,
    pub phase: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelDsp {
    pub eq: Equalizer,
    pub comp: Compressor,
    pub gate: Gate,
}

/// 4-band parametric EQ + HPF. Physical units here; their wire encoding is
/// open question Q1 (frequencies do not fit in `8.24`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
#[serde(default)]
pub struct Hpf {
    pub enabled: bool,
    pub freq_hz: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqBand {
    pub enabled: bool,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
#[serde(default)]
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
#[serde(default)]
pub struct Reverb {
    pub enabled: bool,
    /// Return level, linear gain `0.0..=MIX_GAIN_MAX` (native unit, D8).
    pub return_gain: f64,
    pub time_s: f64,
    pub predelay_ms: f64,
    /// Per mixer-input send level, linear gain `0.0..=MIX_GAIN_MAX`.
    pub sends: Vec<f64>,
}

/// Routing crosspoints, `matrix[input][output]`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Routing {
    pub matrix: Vec<Vec<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlRoom {
    /// −20 dB dim.
    pub dim: bool,
    pub mono: bool,
    /// `false` = speakers A, `true` = speakers B.
    pub speaker_b: bool,
    pub talkback: bool,
    /// Headphone trim, whole dB `TRIM_MIN_DB..=0.0` — MOTU native (U8 attenuation).
    pub phones_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clock {
    pub sample_rate: SampleRate,
    pub source: ClockSource,
}

// Defaults used when an older/partial preset file lacks a field (tolerant
// reading, D15). They match `MotuState::new`.
impl Default for Clock {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::Hz48000,
            source: ClockSource::Internal,
        }
    }
}

impl Default for Hpf {
    fn default() -> Self {
        Self {
            enabled: false,
            freq_hz: 80.0,
        }
    }
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            enabled: false,
            freq_hz: 1_000.0,
            gain_db: 0.0,
            q: 0.7,
        }
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self {
            enabled: false,
            return_gain: 0.0,
            time_s: 2.0,
            predelay_ms: 20.0,
            sends: Vec::new(),
        }
    }
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
            main: MainOut { db: 0.0 },
            preamps: vec![Preamp::default(); usize::from(topology.preamps)],
            channels: vec![ChannelDsp::default(); inputs],
            reverb: Reverb {
                enabled: false,
                return_gain: 0.0,
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
            output_trims: vec![0.0; OUTPUT_TRIMS],
            control_room: ControlRoom {
                dim: false,
                mono: false,
                speaker_b: false,
                talkback: false,
                phones_db: 0.0,
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

/// Preamp gain range in dB (UltraLite-mk5 hardware: 0..74, U8 whole dB).
pub const PREAMP_GAIN_MAX_DB: f64 = 74.0;
/// Analog line outputs with a trim (UltraLite-mk5: Main 1-2 and Line 3-10).
pub const OUTPUT_TRIMS: usize = 10;
/// Lowest trim (main / phones) in dB (U8 attenuation 0..100).
pub const TRIM_MIN_DB: f64 = -100.0;
/// Highest mix fader gain: +12 dB (`10^(12/20)`), the top of the fader law.
pub const MIX_GAIN_MAX: f64 = 3.981_071_705_534_972;

/// One set-style parameter (0-based indices). JSON: `{"param":"mix_vol","bus":0,"ch":3,"gain":1.0}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "param", rename_all = "snake_case")]
pub enum Param {
    /// Linear gain, clamped to `0.0..=MIX_GAIN_MAX`.
    MixVol {
        bus: u8,
        ch: u16,
        gain: f64,
    },
    MixPan {
        bus: u8,
        ch: u16,
        value: f64,
    },
    MixMute {
        bus: u8,
        ch: u16,
        on: bool,
    },
    MixSolo {
        bus: u8,
        ch: u16,
        on: bool,
    },
    /// Whole dB, clamped to `TRIM_MIN_DB..=0.0`.
    MainVol {
        db: f64,
    },
    /// Whole dB, clamped to `0.0..=PREAMP_GAIN_MAX_DB`.
    PreampGain {
        ch: u16,
        db: f64,
    },
    Phantom {
        ch: u16,
        on: bool,
    },
    Pad {
        ch: u16,
        on: bool,
    },
    Phase {
        ch: u16,
        on: bool,
    },
    EqBypass {
        ch: u16,
        on: bool,
    },
    Route {
        input: u16,
        output: u16,
        on: bool,
    },
    Dim {
        on: bool,
    },
    Mono {
        on: bool,
    },
    SpeakerB {
        on: bool,
    },
    Talkback {
        on: bool,
    },
    /// Whole dB, clamped to `TRIM_MIN_DB..=0.0`.
    PhonesVol {
        db: f64,
    },
    /// Trim of one analog line output (`ch` 0 = output 1), whole dB clamped
    /// to `TRIM_MIN_DB..=0.0`.
    OutputTrim {
        ch: u16,
        db: f64,
    },
    // --- DSP, reverb, clock (Phase 3). Set-style: each variant carries the
    // whole block so one message fully describes it. No hardware mapping yet
    // (state, presets and UI only); ranges in `dsp_limits`.
    /// One EQ band (`band` 0..=3).
    EqBand {
        ch: u16,
        band: u8,
        enabled: bool,
        freq_hz: f64,
        gain_db: f64,
        q: f64,
    },
    Hpf {
        ch: u16,
        enabled: bool,
        freq_hz: f64,
    },
    Comp {
        ch: u16,
        enabled: bool,
        threshold_db: f64,
        ratio: f64,
        attack_ms: f64,
        release_ms: f64,
        gain_db: f64,
    },
    Gate {
        ch: u16,
        enabled: bool,
        threshold_db: f64,
        attack_ms: f64,
        release_ms: f64,
    },
    /// Reverb block; `return_gain` linear (`0.0..=MIX_GAIN_MAX`).
    Reverb {
        enabled: bool,
        return_gain: f64,
        time_s: f64,
        predelay_ms: f64,
    },
    /// Per-input reverb send, linear gain.
    ReverbSend {
        ch: u16,
        gain: f64,
    },
    Clock {
        sample_rate: SampleRate,
        source: ClockSource,
    },
}

/// Accepted ranges for the DSP / reverb params (values are clamped into them).
pub mod dsp_limits {
    pub const FREQ_HZ: (f64, f64) = (20.0, 20_000.0);
    pub const EQ_GAIN_DB: (f64, f64) = (-20.0, 20.0);
    pub const EQ_Q: (f64, f64) = (0.1, 10.0);
    pub const COMP_THRESHOLD_DB: (f64, f64) = (-60.0, 0.0);
    pub const COMP_RATIO: (f64, f64) = (1.0, 20.0);
    pub const COMP_GAIN_DB: (f64, f64) = (0.0, 24.0);
    pub const GATE_THRESHOLD_DB: (f64, f64) = (-80.0, 0.0);
    pub const ATTACK_MS: (f64, f64) = (0.1, 200.0);
    pub const RELEASE_MS: (f64, f64) = (10.0, 2_000.0);
    pub const REVERB_TIME_S: (f64, f64) = (0.1, 10.0);
    pub const REVERB_PREDELAY_MS: (f64, f64) = (0.0, 200.0);
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
    /// What the connected device reported about itself; `None` in demo mode
    /// or before the first sync.
    #[serde(default)]
    pub device: Option<DeviceInfo>,
}

/// Identity and layout of the connected device, read from its state dump on
/// connect (`motu::identify`). Not part of presets: it describes the hardware.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceInfo {
    /// Model code reported by the device (e.g. `UL5E`).
    pub model: String,
    /// Firmware version string.
    pub firmware: String,
    /// Dimensions counted from the dump.
    pub topology: Topology,
    /// Display name per mixer input (device names, else the model's defaults;
    /// empty string = no name known).
    pub input_names: Vec<String>,
    /// Display name per mixer input when it is the left channel of a stereo
    /// pair (e.g. `S/PDIF 1-2`); empty = use `input_names`.
    pub stereo_names: Vec<String>,
    /// Display name per stereo mix.
    pub mix_names: Vec<String>,
    /// Order in which to list the mixes (CueMix 5: Main, Phones, Line 3-4…);
    /// empty = index order.
    pub mix_order: Vec<u8>,
    /// Per mixer input: `true` on the left channel of a stereo pair.
    pub stereo_inputs: Vec<bool>,
    /// Per mono bus: `true` on the left bus of a stereo mix.
    pub stereo_buses: Vec<bool>,
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
    #[error("state shape does not match its topology: {0}")]
    Shape(&'static str),
}

/// Line-input (non-mic) preamp gain ceiling in dB (hardware, motu-mk5-linux).
pub const LINE_GAIN_MAX_DB: f64 = 20.0;

impl Param {
    /// Parameters a preset recall never writes to the hardware (maintainer
    /// ruling on audit #9): listening trims and +48V phantom power. They are
    /// still stored in preset files.
    pub fn excluded_from_recall(&self) -> bool {
        matches!(
            self,
            Param::MainVol { .. }
                | Param::PhonesVol { .. }
                | Param::OutputTrim { .. }
                | Param::Phantom { .. }
        )
    }
}

impl MotuState {
    /// Vector lengths must match `topology` (presets are untrusted files).
    pub fn check_shape(&self) -> Result<(), StateError> {
        let t = self.topology;
        let inputs = usize::from(t.mixer_inputs);
        let ok = |cond: bool, what| {
            if cond {
                Ok(())
            } else {
                Err(StateError::Shape(what))
            }
        };
        ok(self.mixes.len() == usize::from(t.mixes), "mixes")?;
        ok(
            self.mixes.iter().all(|m| m.channels.len() == inputs),
            "mix channels",
        )?;
        ok(self.preamps.len() == usize::from(t.preamps), "preamps")?;
        ok(self.output_trims.len() == OUTPUT_TRIMS, "output trims")?;
        ok(t.mic_inputs <= t.preamps, "mic_inputs")?;
        ok(self.channels.len() == inputs, "channels")?;
        ok(self.reverb.sends.len() == inputs, "reverb sends")?;
        ok(
            self.routing.matrix.len() == usize::from(t.routing_inputs),
            "routing inputs",
        )?;
        ok(
            self.routing
                .matrix
                .iter()
                .all(|r| r.len() == usize::from(t.routing_outputs)),
            "routing outputs",
        )?;
        Ok(())
    }

    /// Validate shape and clamp every addressable value through [`Self::apply`].
    pub fn normalized(mut self) -> Result<Self, StateError> {
        // Presets saved before output trims existed.
        if self.output_trims.is_empty() {
            self.output_trims = vec![0.0; OUTPUT_TRIMS];
        }
        self.check_shape()?;
        for p in self.all_params() {
            self.apply(&p)?;
        }
        Ok(self)
    }

    /// Copy the recall-excluded fields (trims, +48V) from `live` into `self`.
    pub fn keep_excluded_from(&mut self, live: &MotuState) {
        self.main.db = live.main.db;
        self.control_room.phones_db = live.control_room.phones_db;
        self.output_trims.clone_from(&live.output_trims);
        for (mine, theirs) in self.preamps.iter_mut().zip(&live.preamps) {
            mine.phantom = theirs.phantom;
        }
    }
}

/// Reject non-finite, then clamp into `(min, max)`.
fn range(value: f64, what: &'static str, (min, max): (f64, f64)) -> Result<f64, StateError> {
    Ok(finite(value, what)?.clamp(min, max))
}

fn comp_param(ch: u16, c: &Compressor) -> Param {
    Param::Comp {
        ch,
        enabled: c.enabled,
        threshold_db: c.threshold_db,
        ratio: c.ratio,
        attack_ms: c.attack_ms,
        release_ms: c.release_ms,
        gain_db: c.gain_db,
    }
}

fn gate_param(ch: u16, g: &Gate) -> Param {
    Param::Gate {
        ch,
        enabled: g.enabled,
        threshold_db: g.threshold_db,
        attack_ms: g.attack_ms,
        release_ms: g.release_ms,
    }
}

/// Round to the hardware's whole-dB resolution, then clamp.
fn whole_db(db: f64, min: f64, max: f64) -> f64 {
    db.round().clamp(min, max)
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
    /// Preamps that have +48V / Pad (the first `topology.mic_inputs`).
    fn mic_preamps(&mut self) -> &mut [Preamp] {
        let n = usize::from(self.topology.mic_inputs).min(self.preamps.len());
        self.preamps.get_mut(..n).unwrap_or_default()
    }

    fn mix_channel(&mut self, bus: u8, ch: u16) -> Result<&mut MixChannel, StateError> {
        let mix = slot(&mut self.mixes, usize::from(bus), "mix")?;
        slot(&mut mix.channels, usize::from(ch), "mix channel")
    }

    /// Apply one change. Returns the *effective* param (clamped) when the state
    /// changed, `None` when it was already at that value (no feedback needed).
    pub fn apply(&mut self, param: &Param) -> Result<Option<Param>, StateError> {
        use Param as P;
        let (changed, effective) = match *param {
            P::MixVol { bus, ch, gain } => {
                let v = finite(gain, "mix_vol")?.clamp(0.0, MIX_GAIN_MAX);
                (
                    set(&mut self.mix_channel(bus, ch)?.gain, v),
                    P::MixVol { bus, ch, gain: v },
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
            P::MainVol { db } => {
                let v = whole_db(finite(db, "main_vol")?, TRIM_MIN_DB, 0.0);
                (set(&mut self.main.db, v), P::MainVol { db: v })
            }
            P::PreampGain { ch, db } => {
                let max = if ch < self.topology.mic_inputs {
                    PREAMP_GAIN_MAX_DB
                } else {
                    LINE_GAIN_MAX_DB
                };
                let v = whole_db(finite(db, "preamp_gain")?, 0.0, max);
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
                    &mut slot(self.mic_preamps(), usize::from(ch), "mic preamp")?.phantom,
                    on,
                ),
                *param,
            ),
            P::Pad { ch, on } => (
                set(
                    &mut slot(self.mic_preamps(), usize::from(ch), "mic preamp")?.pad,
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
            P::PhonesVol { db } => {
                let v = whole_db(finite(db, "phones_vol")?, TRIM_MIN_DB, 0.0);
                (
                    set(&mut self.control_room.phones_db, v),
                    P::PhonesVol { db: v },
                )
            }
            P::OutputTrim { ch, db } => {
                let v = whole_db(finite(db, "output_trim")?, TRIM_MIN_DB, 0.0);
                (
                    set(
                        slot(&mut self.output_trims, usize::from(ch), "output trim")?,
                        v,
                    ),
                    P::OutputTrim { ch, db: v },
                )
            }
            P::EqBand {
                ch,
                band,
                enabled,
                freq_hz,
                gain_db,
                q,
            } => {
                let new = EqBand {
                    enabled,
                    freq_hz: range(freq_hz, "eq_band", dsp_limits::FREQ_HZ)?,
                    gain_db: range(gain_db, "eq_band", dsp_limits::EQ_GAIN_DB)?,
                    q: range(q, "eq_band", dsp_limits::EQ_Q)?,
                };
                let dsp = slot(&mut self.channels, usize::from(ch), "channel")?;
                let slot_band = slot(&mut dsp.eq.bands, usize::from(band), "eq band")?;
                (
                    set(slot_band, new),
                    P::EqBand {
                        ch,
                        band,
                        enabled,
                        freq_hz: new.freq_hz,
                        gain_db: new.gain_db,
                        q: new.q,
                    },
                )
            }
            P::Hpf {
                ch,
                enabled,
                freq_hz,
            } => {
                let new = Hpf {
                    enabled,
                    freq_hz: range(freq_hz, "hpf", dsp_limits::FREQ_HZ)?,
                };
                let dsp = slot(&mut self.channels, usize::from(ch), "channel")?;
                (
                    set(&mut dsp.eq.hpf, new),
                    P::Hpf {
                        ch,
                        enabled,
                        freq_hz: new.freq_hz,
                    },
                )
            }
            P::Comp {
                ch,
                enabled,
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                gain_db,
            } => {
                let new = Compressor {
                    enabled,
                    threshold_db: range(threshold_db, "comp", dsp_limits::COMP_THRESHOLD_DB)?,
                    ratio: range(ratio, "comp", dsp_limits::COMP_RATIO)?,
                    attack_ms: range(attack_ms, "comp", dsp_limits::ATTACK_MS)?,
                    release_ms: range(release_ms, "comp", dsp_limits::RELEASE_MS)?,
                    gain_db: range(gain_db, "comp", dsp_limits::COMP_GAIN_DB)?,
                };
                let dsp = slot(&mut self.channels, usize::from(ch), "channel")?;
                (set(&mut dsp.comp, new), comp_param(ch, &new))
            }
            P::Gate {
                ch,
                enabled,
                threshold_db,
                attack_ms,
                release_ms,
            } => {
                let new = Gate {
                    enabled,
                    threshold_db: range(threshold_db, "gate", dsp_limits::GATE_THRESHOLD_DB)?,
                    attack_ms: range(attack_ms, "gate", dsp_limits::ATTACK_MS)?,
                    release_ms: range(release_ms, "gate", dsp_limits::RELEASE_MS)?,
                };
                let dsp = slot(&mut self.channels, usize::from(ch), "channel")?;
                (set(&mut dsp.gate, new), gate_param(ch, &new))
            }
            P::Reverb {
                enabled,
                return_gain,
                time_s,
                predelay_ms,
            } => {
                let return_gain = range(return_gain, "reverb", (0.0, MIX_GAIN_MAX))?;
                let time_s = range(time_s, "reverb", dsp_limits::REVERB_TIME_S)?;
                let predelay_ms = range(predelay_ms, "reverb", dsp_limits::REVERB_PREDELAY_MS)?;
                let r = &mut self.reverb;
                let changed = set(&mut r.enabled, enabled)
                    | set(&mut r.return_gain, return_gain)
                    | set(&mut r.time_s, time_s)
                    | set(&mut r.predelay_ms, predelay_ms);
                (
                    changed,
                    P::Reverb {
                        enabled,
                        return_gain,
                        time_s,
                        predelay_ms,
                    },
                )
            }
            P::ReverbSend { ch, gain } => {
                let v = range(gain, "reverb_send", (0.0, MIX_GAIN_MAX))?;
                (
                    set(
                        slot(&mut self.reverb.sends, usize::from(ch), "reverb send")?,
                        v,
                    ),
                    P::ReverbSend { ch, gain: v },
                )
            }
            P::Clock {
                sample_rate,
                source,
            } => {
                let changed = set(&mut self.clock.sample_rate, sample_rate)
                    | set(&mut self.clock.source, source);
                (changed, *param)
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
                if all || x.gain != y.gain {
                    out.push(Param::MixVol {
                        bus,
                        ch,
                        gain: y.gain,
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
        if all || self.main.db != target.main.db {
            out.push(Param::MainVol { db: target.main.db });
        }
        for (ch, (x, y)) in self.preamps.iter().zip(&target.preamps).enumerate() {
            let Ok(ch) = u16::try_from(ch) else { break };
            if all || x.gain_db != y.gain_db {
                out.push(Param::PreampGain { ch, db: y.gain_db });
            }
            if ch < target.topology.mic_inputs && (all || x.phantom != y.phantom) {
                out.push(Param::Phantom { ch, on: y.phantom });
            }
            if ch < target.topology.mic_inputs && (all || x.pad != y.pad) {
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
            for (band, (bx, by)) in x.eq.bands.iter().zip(&y.eq.bands).enumerate() {
                let Ok(band) = u8::try_from(band) else { break };
                if all || bx != by {
                    out.push(Param::EqBand {
                        ch,
                        band,
                        enabled: by.enabled,
                        freq_hz: by.freq_hz,
                        gain_db: by.gain_db,
                        q: by.q,
                    });
                }
            }
            if all || x.eq.hpf != y.eq.hpf {
                out.push(Param::Hpf {
                    ch,
                    enabled: y.eq.hpf.enabled,
                    freq_hz: y.eq.hpf.freq_hz,
                });
            }
            if all || x.comp != y.comp {
                out.push(comp_param(ch, &y.comp));
            }
            if all || x.gate != y.gate {
                out.push(gate_param(ch, &y.gate));
            }
        }
        let (ra, rb) = (&self.reverb, &target.reverb);
        if all
            || ra.enabled != rb.enabled
            || ra.return_gain != rb.return_gain
            || ra.time_s != rb.time_s
            || ra.predelay_ms != rb.predelay_ms
        {
            out.push(Param::Reverb {
                enabled: rb.enabled,
                return_gain: rb.return_gain,
                time_s: rb.time_s,
                predelay_ms: rb.predelay_ms,
            });
        }
        for (ch, (x, y)) in ra.sends.iter().zip(&rb.sends).enumerate() {
            let Ok(ch) = u16::try_from(ch) else { break };
            if all || x != y {
                out.push(Param::ReverbSend { ch, gain: *y });
            }
        }
        if all || self.clock != target.clock {
            out.push(Param::Clock {
                sample_rate: target.clock.sample_rate,
                source: target.clock.source,
            });
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
        if all || a.phones_db != b.phones_db {
            out.push(Param::PhonesVol { db: b.phones_db });
        }
        for (ch, (x, y)) in self
            .output_trims
            .iter()
            .zip(&target.output_trims)
            .enumerate()
        {
            let Ok(ch) = u16::try_from(ch) else { break };
            if all || x != y {
                out.push(Param::OutputTrim { ch, db: *y });
            }
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
    /// Preset file format version (D15). Missing in files written before it
    /// existed → `0`, read as the current format (fields default when absent).
    #[serde(default)]
    pub schema_version: u32,
    pub name: String,
    pub state: MotuState,
}

/// Current preset file format. Bump on any *incompatible* change to
/// `MotuState` (renamed field, changed unit) and add a migration + fixture.
/// Additive changes (new fields with defaults) do not need a bump.
pub const PRESET_SCHEMA_VERSION: u32 = 1;

impl Preset {
    /// `● Modifié` indicator: live state diverges from this snapshot, ignoring
    /// the fields a recall never restores (trims, +48V).
    pub fn is_modified(&self, live: &MotuState) -> bool {
        let mut expected = self.state.clone();
        expected.keep_excluded_from(live);
        &expected != live
    }
}

/// Maximum label name length (characters).
pub const LABEL_NAME_MAX: usize = 64;

impl ChannelLabel {
    /// Name 1–64 chars without control characters; colour `#RRGGBB` if set.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.chars().count();
        if n == 0 || n > LABEL_NAME_MAX || self.name.chars().any(char::is_control) {
            return Err(format!(
                "label name must be 1-{LABEL_NAME_MAX} printable characters"
            ));
        }
        if let Some(c) = &self.color {
            let hex = c.strip_prefix('#').unwrap_or("");
            if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(format!("label colour {c:?} must be #RRGGBB"));
            }
        }
        Ok(())
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
        assert!(s.mixes.iter().all(|m| m.channels.len() == 32));
        assert_eq!(s.preamps.len(), 8);
        assert_eq!(s.routing.matrix.len(), 18);
        assert_eq!(s.routing.matrix[0].len(), 22);
    }

    #[test]
    fn apply_clamps_and_reports_effective_value() {
        let mut s = MotuState::default();
        let r = s.apply(&Param::MixVol {
            bus: 1,
            ch: 2,
            gain: 9.0,
        });
        assert_eq!(
            r,
            Ok(Some(Param::MixVol {
                bus: 1,
                ch: 2,
                gain: MIX_GAIN_MAX
            }))
        );
        assert_eq!(s.mixes[1].channels[2].gain, MIX_GAIN_MAX);
        assert_eq!(
            s.apply(&Param::MainVol { db: -12.4 }),
            Ok(Some(Param::MainVol { db: -12.0 })),
            "trims are whole dB"
        );
        assert_eq!(
            s.apply(&Param::PhonesVol { db: -300.0 }),
            Ok(Some(Param::PhonesVol { db: TRIM_MIN_DB }))
        );
        assert_eq!(
            s.apply(&Param::PreampGain { ch: 1, db: 80.0 }),
            Ok(Some(Param::PreampGain {
                ch: 1,
                db: PREAMP_GAIN_MAX_DB
            }))
        );
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
            s.apply(&Param::MainVol { db: f64::NAN }),
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
            s.apply(&Param::Phantom { ch: 8, on: true }),
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
            schema_version: PRESET_SCHEMA_VERSION,
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
            gain: 0.5,
        });
        let json = serde_json::to_value(&cmd).unwrap_or_default();
        assert_eq!(
            json,
            serde_json::json!({"cmd":"set","param":"mix_vol","bus":0,"ch":3,"gain":0.5})
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
                gain: 0.2,
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
            Param::PhonesVol { db: -12.0 },
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
mod output_trim_tests {
    use super::*;

    #[test]
    fn output_trims_clamp_and_are_not_recalled() {
        let mut s = MotuState::default();
        assert_eq!(
            s.apply(&Param::OutputTrim { ch: 2, db: -7.4 }),
            Ok(Some(Param::OutputTrim { ch: 2, db: -7.0 }))
        );
        assert_eq!(
            s.apply(&Param::OutputTrim { ch: 3, db: 5.0 }),
            Ok(None),
            "trims only attenuate: +5 dB clamps to the current 0 dB"
        );
        assert!(s.apply(&Param::OutputTrim { ch: 99, db: 0.0 }).is_err());
        assert!(Param::OutputTrim { ch: 0, db: 0.0 }.excluded_from_recall());
        let mut preset = MotuState::default();
        preset.keep_excluded_from(&s);
        assert_eq!(preset.output_trims[2], -7.0);
    }

    #[test]
    fn presets_without_output_trims_still_load() {
        let mut old = MotuState::default();
        old.output_trims.clear();
        let n = old.normalized().unwrap_or_default();
        assert_eq!(n.output_trims, vec![0.0; OUTPUT_TRIMS]);
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
            + usize::from(t.preamps) * 2 // gain, phase
            + usize::from(t.mic_inputs) * 2 // phantom, pad
            + inputs * (1 + 4 + 1 + 1 + 1) // eq bypass, 4 bands, hpf, comp, gate
            + 1 + inputs // reverb block + sends
            + 1 // clock
            + usize::from(t.routing_inputs) * usize::from(t.routing_outputs)
            + 5 // control room
            + OUTPUT_TRIMS;
        let params = s.all_params();
        assert_eq!(params.len(), expected);
        let mut replay = MotuState::default();
        for p in &params {
            assert_eq!(replay.apply(p), Ok(None), "default values replay as no-ops");
        }
    }
}

#[cfg(test)]
mod native_unit_tests {
    use super::*;
    use crate::motu::fader;

    #[test]
    fn mix_gain_max_matches_fader_law() {
        assert!((MIX_GAIN_MAX - fader::max_gain()).abs() < 1e-12);
    }

    #[test]
    fn native_json_shapes() {
        let main = serde_json::to_value(Param::MainVol { db: -6.0 }).unwrap_or_default();
        assert_eq!(main, serde_json::json!({"param":"main_vol","db":-6.0}));
        let phones = serde_json::to_value(Param::PhonesVol { db: 0.0 }).unwrap_or_default();
        assert_eq!(phones, serde_json::json!({"param":"phones_vol","db":0.0}));
        let mix = serde_json::to_value(Param::MixVol {
            bus: 0,
            ch: 0,
            gain: 1.0,
        })
        .unwrap_or_default();
        assert_eq!(
            mix,
            serde_json::json!({"param":"mix_vol","bus":0,"ch":0,"gain":1.0})
        );
    }
}

#[cfg(test)]
mod recall_tests {
    use super::*;

    #[test]
    fn line_inputs_cap_gain_at_20_db() {
        let mut s = MotuState::default();
        assert_eq!(
            s.apply(&Param::PreampGain { ch: 1, db: 70.0 }),
            Ok(Some(Param::PreampGain { ch: 1, db: 70.0 }))
        );
        assert_eq!(
            s.apply(&Param::PreampGain { ch: 2, db: 70.0 }),
            Ok(Some(Param::PreampGain {
                ch: 2,
                db: LINE_GAIN_MAX_DB
            }))
        );
    }

    #[test]
    fn normalized_clamps_and_rejects_bad_shapes() {
        let mut s = MotuState::default();
        s.main.db = 30.0; // impossible on hardware
        s.mixes[0].channels[0].gain = 99.0;
        let n = s.normalized();
        assert!(n.is_ok());
        let n = n.unwrap_or_default();
        assert_eq!(n.main.db, 0.0);
        assert_eq!(n.mixes[0].channels[0].gain, MIX_GAIN_MAX);

        let mut bad = MotuState::default();
        bad.preamps.pop();
        assert_eq!(bad.normalized(), Err(StateError::Shape("preamps")));
        let mut bad = MotuState::default();
        bad.routing.matrix[3].push(true);
        assert_eq!(bad.check_shape(), Err(StateError::Shape("routing outputs")));
    }

    #[test]
    fn recall_excludes_trims_and_phantom() {
        let mut live = MotuState::default();
        let _ = live.apply(&Param::Phantom { ch: 0, on: true });
        let _ = live.apply(&Param::MainVol { db: -20.0 });
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: "p".into(),
            state: MotuState::default(),
        };
        assert!(
            !preset.is_modified(&live),
            "trims/48V never count as modified"
        );
        let _ = live.apply(&Param::Mono { on: true });
        assert!(preset.is_modified(&live));

        let mut next = preset.state.clone();
        next.keep_excluded_from(&live);
        assert!(next.preamps[0].phantom);
        assert_eq!(next.main.db, -20.0);
        assert!(Param::Phantom { ch: 0, on: true }.excluded_from_recall());
        assert!(!Param::Mono { on: true }.excluded_from_recall());
    }

    #[test]
    fn label_validation() {
        let ok = ChannelLabel {
            name: "Kick".into(),
            color: Some("#06B6D4".into()),
        };
        assert!(ok.validate().is_ok());
        for bad in [
            ChannelLabel {
                name: String::new(),
                color: None,
            },
            ChannelLabel {
                name: "x".repeat(65),
                color: None,
            },
            ChannelLabel {
                name: "a\nb".into(),
                color: None,
            },
            ChannelLabel {
                name: "ok".into(),
                color: Some("red".into()),
            },
            ChannelLabel {
                name: "ok".into(),
                color: Some("#12345".into()),
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }
}

#[cfg(test)]
mod dsp_param_tests {
    use super::*;

    #[test]
    fn dsp_params_clamp_validate_and_replay() {
        let mut s = MotuState::default();
        let r = s.apply(&Param::EqBand {
            ch: 3,
            band: 2,
            enabled: true,
            freq_hz: 50_000.0,
            gain_db: -40.0,
            q: 1.5,
        });
        assert_eq!(
            r,
            Ok(Some(Param::EqBand {
                ch: 3,
                band: 2,
                enabled: true,
                freq_hz: 20_000.0,
                gain_db: -20.0,
                q: 1.5
            }))
        );
        assert!(matches!(
            s.apply(&Param::EqBand {
                ch: 0,
                band: 4,
                enabled: true,
                freq_hz: 100.0,
                gain_db: 0.0,
                q: 1.0
            }),
            Err(StateError::Index {
                what: "eq band",
                ..
            })
        ));
        assert_eq!(
            s.apply(&Param::Comp {
                ch: 0,
                enabled: true,
                threshold_db: f64::NAN,
                ratio: 2.0,
                attack_ms: 5.0,
                release_ms: 50.0,
                gain_db: 0.0
            }),
            Err(StateError::NonFinite("comp"))
        );
        let _ = s.apply(&Param::Reverb {
            enabled: true,
            return_gain: 0.5,
            time_s: 99.0,
            predelay_ms: 10.0,
        });
        assert_eq!(s.reverb.time_s, 10.0);
        let _ = s.apply(&Param::ReverbSend { ch: 31, gain: 0.25 });
        let _ = s.apply(&Param::Clock {
            sample_rate: SampleRate::Hz96000,
            source: ClockSource::Optical,
        });

        // A preset recall replays every DSP change exactly.
        let mut replay = MotuState::default();
        for p in MotuState::default().diff(&s) {
            assert!(replay.apply(&p).is_ok());
        }
        assert_eq!(replay, s);
        let json = serde_json::to_value(Param::Clock {
            sample_rate: SampleRate::Hz48000,
            source: ClockSource::Internal,
        })
        .unwrap_or_default();
        assert_eq!(
            json,
            serde_json::json!({"param":"clock","sample_rate":"48000","source":"internal"})
        );
    }
}
