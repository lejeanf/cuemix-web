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

use crate::osc::settings::OscSettings;
use crate::plug::{PlugState, PlugUrls, SpeakerPlugSettings};
use crate::power::{ChannelPower, PowerChannel};

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
    /// Input EQ, `input_eq[ch][band]`: one row per analog input (`topology.preamps`,
    /// 8 on the mk5), [`INPUT_EQ_BANDS`] bands each.
    pub input_eq: Vec<Vec<EqBand>>,
    /// Compressor per analog input (`topology.preamps` entries).
    pub compressors: Vec<Compressor>,
    /// Gate per mic input (`topology.mic_inputs` entries).
    pub gates: Vec<Gate>,
    /// Output EQ, `output_eq[out][band]`: [`OUTPUT_EQ_OUTPUTS`] x [`OUTPUT_EQ_BANDS`].
    pub output_eq: Vec<Vec<EqBand>>,
    /// Reverb processor. Its sends are the faders of the "Reverb" mix and its
    /// return is a mixer input (30-31 on the mk5): both live in `mixes`.
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

/// EQ band filter type, the raw byte of props `1002` / `1022`. Kept as a
/// number so values the dump has not explained yet survive a round trip.
pub mod eq_mode {
    /// Peak / bell (most bands in the dump).
    pub const BELL: u8 = 0;
    /// Low shelf.
    pub const LOW_SHELF: u8 = 1;
    /// High shelf.
    pub const HIGH_SHELF: u8 = 2;
    /// High-pass (band 0 of every input in the dump).
    pub const HIGH_PASS: u8 = 3;
}

/// Output EQ slot that does not exist on the device (slot 0 = Main L-R,
/// 1 = unused, 2..=9 = Line 3..10, 10 = Phones L-R, 11 = Reverb).
pub const OUTPUT_EQ_UNUSED_SLOT: u16 = 1;

/// Is `mode` a filter type CueMix 5 offers on band `band` of an EQ with
/// `bands` bands (input: 4, output: 3)? The first band can be a bell, low
/// shelf or high-pass, the last a bell or high shelf, the ones in between
/// bells only. Single source of truth for the state (`check_writable`,
/// `normalized`) and the device encoder.
pub fn eq_mode_allowed(band: u8, bands: usize, mode: u8) -> bool {
    let band = usize::from(band);
    if band >= bands {
        false
    } else if band == 0 {
        matches!(
            mode,
            eq_mode::BELL | eq_mode::LOW_SHELF | eq_mode::HIGH_PASS
        )
    } else if band == bands - 1 {
        matches!(mode, eq_mode::BELL | eq_mode::HIGH_SHELF)
    } else {
        mode == eq_mode::BELL
    }
}

/// [`eq_mode_allowed`] for the input (`output == false`) or output EQ.
pub fn eq_band_mode_allowed(output: bool, band: u8, mode: u8) -> bool {
    let bands = if output {
        OUTPUT_EQ_BANDS
    } else {
        INPUT_EQ_BANDS
    };
    eq_mode_allowed(band, bands, mode)
}

/// Refuses a change asked by a client (web UI, OSC) that the device would
/// never receive: the unused output EQ slot, or a filter type the EQ band
/// does not offer. Checked on the command path only: values reported by
/// the device itself always go into the state, the card being the reference.
pub fn check_writable(param: &Param) -> Result<(), StateError> {
    let Some((output, ch, band, field)) = eq_parts(param) else {
        return Ok(());
    };
    if output && ch == OUTPUT_EQ_UNUSED_SLOT {
        return Err(StateError::Index {
            what: "unused output eq slot",
            index: usize::from(ch),
            len: OUTPUT_EQ_OUTPUTS,
        });
    }
    if let EqField::Mode(mode) = field {
        if !eq_band_mode_allowed(output, band, mode) {
            return Err(StateError::Index {
                what: "eq mode for this band",
                index: usize::from(mode),
                len: 4,
            });
        }
    }
    Ok(())
}

/// One EQ band, as the hardware stores it (props `1002..=1006` for the
/// inputs, `1022..=1026` for the outputs).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqBand {
    /// Filter type, see [`eq_mode`]; unknown values are kept as is.
    pub mode: u8,
    /// Band bypass (`true` = bypassed).
    pub bypass: bool,
    /// Whole Hz (hardware: Int32).
    pub freq_hz: f64,
    /// dB (hardware: 8.24).
    pub gain_db: f64,
    /// Q of a bell band (hardware: 8.24, CueMix 5 range 0.45-10; ignored by
    /// shelves and the high-pass). Named `bw_oct` for preset compatibility:
    /// it was first read as a bandwidth in octaves.
    pub bw_oct: f64,
}

impl Default for EqBand {
    fn default() -> Self {
        Self {
            mode: eq_mode::BELL,
            bypass: false,
            freq_hz: 1_000.0,
            gain_db: 0.0,
            bw_oct: 1.0,
        }
    }
}

impl EqBand {
    fn at(mode: u8, freq_hz: f64) -> Self {
        Self {
            mode,
            freq_hz,
            ..Self::default()
        }
    }

    /// Input EQ defaults of every channel (CueMix 5 values): low shelf 60 Hz,
    /// bell 400 Hz, bell 2 kHz, high shelf 14 kHz, 0 dB, Q 1, not bypassed.
    fn input_defaults() -> Vec<EqBand> {
        vec![
            Self::at(eq_mode::LOW_SHELF, 60.0),
            Self::at(eq_mode::BELL, 400.0),
            Self::at(eq_mode::BELL, 2_000.0),
            Self::at(eq_mode::HIGH_SHELF, 14_000.0),
        ]
    }

    /// Output EQ defaults of every slot (CueMix 5 values): low shelf 60 Hz,
    /// bell 800 Hz, high shelf 14 kHz, 0 dB, Q 1, not bypassed.
    fn output_defaults() -> Vec<EqBand> {
        vec![
            Self::at(eq_mode::LOW_SHELF, 60.0),
            Self::at(eq_mode::BELL, 800.0),
            Self::at(eq_mode::HIGH_SHELF, 14_000.0),
        ]
    }
}

/// Compressor of one analog input (props `1011..=1015`, `1021`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compressor {
    /// `true` = bypassed (hardware `1` = bypassed).
    pub bypass: bool,
    pub threshold_db: f64,
    /// Whole ms (hardware: Int16).
    pub attack_ms: f64,
    /// Whole ms (hardware: Int16).
    pub release_ms: f64,
    /// `n` in `n:1`.
    pub ratio: f64,
    /// Make-up gain, **linear** (`1.0` = 0 dB), hardware 8.24.
    pub makeup: f64,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            bypass: true,
            threshold_db: -10.0,
            attack_ms: 10.0,
            release_ms: 1_000.0,
            ratio: 2.0,
            makeup: 1.0,
        }
    }
}

/// Gate of one mic input (props `1007..=1010`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gate {
    /// `true` = bypassed (hardware `1` = bypassed).
    pub bypass: bool,
    pub threshold_db: f64,
    /// Whole ms (hardware: Int16).
    pub attack_ms: f64,
    /// Whole ms (hardware: Int16).
    pub release_ms: f64,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            bypass: true,
            threshold_db: -46.0,
            attack_ms: 10.0,
            release_ms: 1_000.0,
        }
    }
}

/// Reverb processor (props `1030..=1034`, all U8). Sends and return are mix
/// faders (Reverb mix, reverb-return input), not part of this block.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reverb {
    /// Whole `0..=100`.
    pub decay: f64,
    /// Whole `0..=100`.
    pub damping: f64,
    /// Whole ms `0..=100`.
    pub predelay_ms: f64,
    /// Whole `0..=100`.
    pub width: f64,
    /// Preset number (raw byte, meaning of each value not documented).
    pub preset: u8,
}

impl Default for Reverb {
    fn default() -> Self {
        Self {
            decay: 86.0,
            damping: 90.0,
            predelay_ms: 50.0,
            width: 50.0,
            preset: 1,
        }
    }
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
    /// Format of the optical input (prop 5006 index 0).
    #[serde(default)]
    pub optical_in: OpticalMode,
    /// Format of the optical output (prop 5006 index 1).
    #[serde(default)]
    pub optical_out: OpticalMode,
}

/// Optical port format (prop 5006: `0` = ADAT, 8 channels; `1` = TOSLINK
/// stereo S/PDIF), per CueMix 5's property table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpticalMode {
    #[default]
    Adat,
    Toslink,
}

impl OpticalMode {
    pub const fn code(self) -> u8 {
        match self {
            OpticalMode::Adat => 0,
            OpticalMode::Toslink => 1,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(OpticalMode::Adat),
            1 => Some(OpticalMode::Toslink),
            _ => None,
        }
    }
}

// Defaults used when an older/partial preset file lacks a field (tolerant
// reading, D15). They match `MotuState::new`.
impl Default for Clock {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::Hz48000,
            source: ClockSource::Internal,
            optical_in: OpticalMode::Adat,
            optical_out: OpticalMode::Adat,
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

impl SampleRate {
    pub const ALL: [SampleRate; 6] = [
        SampleRate::Hz44100,
        SampleRate::Hz48000,
        SampleRate::Hz88200,
        SampleRate::Hz96000,
        SampleRate::Hz176400,
        SampleRate::Hz192000,
    ];

    /// Rate in Hz (prop `10`).
    pub const fn hz(self) -> u32 {
        match self {
            SampleRate::Hz44100 => 44_100,
            SampleRate::Hz48000 => 48_000,
            SampleRate::Hz88200 => 88_200,
            SampleRate::Hz96000 => 96_000,
            SampleRate::Hz176400 => 176_400,
            SampleRate::Hz192000 => 192_000,
        }
    }

    /// `None` for a rate the UltraLite-mk5 does not list.
    pub fn from_hz(hz: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.hz() == hz)
    }
}

impl ClockSource {
    /// Hardware code of prop `11` (`0` = S/PDIF, `2` = Optical, `3` = Internal).
    pub const fn code(self) -> u8 {
        match self {
            ClockSource::Spdif => 0,
            ClockSource::Optical => 2,
            ClockSource::Internal => 3,
        }
    }

    /// `None` for a code not seen yet (e.g. `1`).
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(ClockSource::Spdif),
            2 => Some(ClockSource::Optical),
            3 => Some(ClockSource::Internal),
            _ => None,
        }
    }
}

impl MotuState {
    /// Hardware-shaped DSP defaults for `topology` (input EQ / compressor per
    /// preamp, gate per mic input, fixed output EQ), plus the reverb block.
    /// Also used to migrate presets older than schema v2.
    pub fn reset_dsp(&mut self) {
        let t = self.topology;
        self.input_eq = vec![EqBand::input_defaults(); usize::from(t.preamps)];
        self.compressors = vec![Compressor::default(); usize::from(t.preamps)];
        self.gates = vec![Gate::default(); usize::from(t.mic_inputs)];
        self.output_eq = vec![EqBand::output_defaults(); OUTPUT_EQ_OUTPUTS];
        self.reverb = Reverb::default();
    }

    /// Neutral initial state (also the offline demo state).
    pub fn new(topology: Topology) -> Self {
        let inputs = usize::from(topology.mixer_inputs);
        let mut s = Self {
            topology,
            mixes: (1..=topology.mixes)
                .map(|n| Mix {
                    name: format!("Mix {n}"),
                    channels: vec![MixChannel::default(); inputs],
                })
                .collect(),
            main: MainOut { db: 0.0 },
            preamps: vec![Preamp::default(); usize::from(topology.preamps)],
            input_eq: Vec::new(),
            compressors: Vec::new(),
            gates: Vec::new(),
            output_eq: Vec::new(),
            reverb: Reverb::default(),
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
            clock: Clock::default(),
        };
        s.reset_dsp();
        s
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
/// Bands of each input EQ (UltraLite-mk5; the channel count is `topology.preamps`).
pub const INPUT_EQ_BANDS: usize = 4;
/// Outputs with an EQ (UltraLite-mk5).
pub const OUTPUT_EQ_OUTPUTS: usize = 12;
/// Bands of each output EQ (UltraLite-mk5).
pub const OUTPUT_EQ_BANDS: usize = 3;

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
    // --- DSP, reverb, clock: one variant per hardware property (props
    // 1002-1015, 1021-1026, 1030-1034, 10, 11), so a frame read from the MOTU
    // maps to exactly one param. Read from the device; **not written yet**
    // (`motu::props::encode` returns no frame). Ranges in `dsp_limits`.
    /// Input EQ band filter type (raw, see [`eq_mode`]). `ch` 0..preamps, `band` 0..4.
    InputEqMode {
        ch: u16,
        band: u8,
        mode: u8,
    },
    /// Input EQ band bypass (`on` = bypassed).
    InputEqBypass {
        ch: u16,
        band: u8,
        on: bool,
    },
    /// Input EQ band frequency, whole Hz.
    InputEqFreq {
        ch: u16,
        band: u8,
        hz: f64,
    },
    /// Input EQ band gain, dB.
    InputEqGain {
        ch: u16,
        band: u8,
        db: f64,
    },
    /// Input EQ band bandwidth, octaves.
    InputEqBw {
        ch: u16,
        band: u8,
        octaves: f64,
    },
    /// Output EQ band filter type. `ch` = output 0..12, `band` 0..3.
    OutputEqMode {
        ch: u16,
        band: u8,
        mode: u8,
    },
    OutputEqBypass {
        ch: u16,
        band: u8,
        on: bool,
    },
    OutputEqFreq {
        ch: u16,
        band: u8,
        hz: f64,
    },
    OutputEqGain {
        ch: u16,
        band: u8,
        db: f64,
    },
    OutputEqBw {
        ch: u16,
        band: u8,
        octaves: f64,
    },
    /// Gate bypass (`on` = bypassed), mic inputs only.
    GateBypass {
        ch: u16,
        on: bool,
    },
    GateThreshold {
        ch: u16,
        db: f64,
    },
    /// Whole ms.
    GateAttack {
        ch: u16,
        ms: f64,
    },
    /// Whole ms.
    GateRelease {
        ch: u16,
        ms: f64,
    },
    /// Compressor bypass (`on` = bypassed), analog inputs.
    CompBypass {
        ch: u16,
        on: bool,
    },
    CompThreshold {
        ch: u16,
        db: f64,
    },
    /// Whole ms.
    CompAttack {
        ch: u16,
        ms: f64,
    },
    /// Whole ms.
    CompRelease {
        ch: u16,
        ms: f64,
    },
    CompRatio {
        ch: u16,
        ratio: f64,
    },
    /// Make-up gain, linear (`1.0` = 0 dB).
    CompMakeup {
        ch: u16,
        gain: f64,
    },
    /// Whole `0..=100`.
    ReverbDecay {
        value: f64,
    },
    /// Whole `0..=100`.
    ReverbDamping {
        value: f64,
    },
    /// Whole ms `0..=100`.
    ReverbPredelay {
        ms: f64,
    },
    /// Whole `0..=100`.
    ReverbWidth {
        value: f64,
    },
    /// Raw preset number.
    ReverbPreset {
        preset: u8,
    },
    SampleRate {
        rate: SampleRate,
    },
    ClockSource {
        source: ClockSource,
    },
    /// Optical port format; `output` = the optical output, else the input.
    OpticalMode {
        output: bool,
        mode: OpticalMode,
    },
}

/// Accepted ranges for the DSP / reverb params (values are clamped into
/// them). **Provisional**: the hardware ranges are not documented, so they are
/// chosen wide enough that a value read from the MOTU is never distorted.
pub mod dsp_limits {
    pub const FREQ_HZ: (f64, f64) = (20.0, 20_000.0);
    /// CueMix 5 ranges for the EQ: gain ±20 dB, Q 0.45-10.
    pub const EQ_GAIN_DB: (f64, f64) = (-20.0, 20.0);
    pub const EQ_BW_OCT: (f64, f64) = (0.45, 10.0);
    pub const THRESHOLD_DB: (f64, f64) = (-100.0, 0.0);
    pub const COMP_RATIO: (f64, f64) = (1.0, 100.0);
    /// Linear make-up gain (`16.0` ≈ +24 dB).
    pub const COMP_MAKEUP: (f64, f64) = (0.0, 16.0);
    pub const ATTACK_MS: (f64, f64) = (0.0, 1_000.0);
    pub const RELEASE_MS: (f64, f64) = (0.0, 10_000.0);
    /// Decay, damping, width.
    pub const REVERB_PERCENT: (f64, f64) = (0.0, 100.0);
    pub const REVERB_PREDELAY_MS: (f64, f64) = (0.0, 100.0);
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
    /// Store a preset file sent by the client (same JSON as the files in the
    /// presets directory, any supported `schema_version`) as `<name>.json`,
    /// then recall it.
    ImportPreset {
        name: String,
        text: String,
    },
    /// Ask for a [`ServerEvent::Presets`] listing.
    ListPresets,
    /// Delete `<presets_dir>/<name>.json` (and its Simple view flag).
    DeletePreset {
        name: String,
    },
    /// Rename a preset; refused when `to` is taken. Its Simple view flag follows it.
    RenamePreset {
        name: String,
        to: String,
    },
    /// Show or hide a preset in the Simple view's list (the Advanced view lists every preset).
    SetPresetInSimple {
        name: String,
        show: bool,
    },
    /// Rename / recolour a mixer input (`None` clears it) — persisted in `labels.json`.
    SetLabel {
        ch: u16,
        label: Option<ChannelLabel>,
    },
    /// Start the room test (`[measure]` in the config): noise from the user's
    /// MPD, other inputs muted, EQ flat, everything restored afterwards.
    MeasureStart,
    /// End the room test early (idempotent).
    MeasureStop,
    /// Monitor / Phones on or off: a fade run by the engine (and, for Main, the speaker
    /// plug when one is set up), see `crate::power`.
    Power {
        channel: PowerChannel,
        on: bool,
    },
    /// While a channel is off (or fading out): choose the level its next On fades back to
    /// (a drag on the off channel's knob). Whole dB; at the minimum it clears the choice (On
    /// then goes to the safe level). Ignored while the channel is on or fading in.
    SetReturnLevel {
        channel: PowerChannel,
        db: f64,
    },
    /// Validate, save and broadcast the speaker plug settings.
    SetSpeakerPlug {
        settings: SpeakerPlugSettings,
    },
    /// Send one on/off request to the saved plug; answered by `SpeakerPlugResult`.
    TestSpeakerPlug {
        on: bool,
    },
    /// Validate, apply live (rebind, new feedback targets), save and broadcast the OSC settings.
    SetOscSettings {
        settings: OscSettings,
    },
}

/// Largest preset file text a client may import (a little under the 256 KiB WebSocket message limit, which also holds the JSON envelope and escapes).
pub const IMPORT_TEXT_MAX: usize = 240 * 1024;

impl Command {
    /// Checks every string a client sent, before anything is done with it (WebSocket, short
    /// HTTP API and OSC all go through here, then again inside the engine). Numbers and
    /// indices of [`Command::Set`] are checked by [`MotuState::apply`]: non-finite values and
    /// out-of-range indices are refused, finite values are clamped.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::LoadPreset { name }
            | Self::SavePreset { name }
            | Self::DeletePreset { name }
            | Self::SetPresetInSimple { name, .. } => {
                crate::presets::validate_name(name).map_err(|e| format!("{e:#}"))
            }
            Self::RenamePreset { name, to } => {
                crate::presets::validate_name(name).map_err(|e| format!("{e:#}"))?;
                crate::presets::validate_name(to).map_err(|e| format!("{e:#}"))
            }
            Self::ImportPreset { name, text } => {
                crate::presets::validate_name(name).map_err(|e| format!("{e:#}"))?;
                if text.len() > IMPORT_TEXT_MAX {
                    return Err(format!(
                        "preset file is larger than {} KiB",
                        IMPORT_TEXT_MAX / 1024
                    ));
                }
                if text.contains('\0') {
                    return Err("preset file must not contain NUL characters".into());
                }
                Ok(())
            }
            Self::SetLabel { label, .. } => label.as_ref().map_or(Ok(()), ChannelLabel::validate),
            Self::SetSpeakerPlug { settings } => settings.validate_text(),
            Self::SetOscSettings { settings } => settings.validate().map(|_| ()),
            Self::Set(_)
            | Self::ListPresets
            | Self::MeasureStart
            | Self::MeasureStop
            | Self::Power { .. }
            | Self::TestSpeakerPlug { .. } => Ok(()),
            Self::SetReturnLevel { db, .. } => {
                if db.is_finite() && (TRIM_MIN_DB..=0.0).contains(db) {
                    Ok(())
                } else {
                    Err(format!(
                        "return level must be between {TRIM_MIN_DB} and 0 dB"
                    ))
                }
            }
        }
    }
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
    Presets {
        names: Vec<String>,
        /// Presets the Simple view leaves out (a subset of `names`). An older client ignores it.
        #[serde(default)]
        hidden: Vec<String>,
    },
    /// Full label set (sent on connect and after each change).
    Labels(Labels),
    /// Meter levels, linear peak `0.0..=1.0`, one per meter point (layout from
    /// the property table). High rate: bridges may drop/decimate.
    Meters { levels: Vec<f32> },
    /// Raw, undecoded meter frame from the MOTU (property 6000, one byte per
    /// meter point), for the meter probe used to map the layout. Rate-limited
    /// by the device client (~30 Hz, `METER_RAW_INTERVAL` = 33 ms).
    MeterRaw { raw: Vec<u8> },
    /// Output patch read from the MOTU (property 5010, read-only): index =
    /// destination, value = patch-bus source point (same numbering as the
    /// meter points). Sent on connect when known and on every change; an
    /// empty list means "unknown" (demo mode, link lost).
    Patch { sources: Vec<u8> },
    /// Room-test state (sent on connect and at every change).
    Measure(MeasureStatus),
    /// On/off and fading state of Monitor and Phones (on connect and on every change; not part
    /// of the device state, so it never touches the "Modified" flag).
    Power {
        main: ChannelPower,
        phones: ChannelPower,
    },
    /// Speaker plug settings, the URLs they build and the last known plug state.
    SpeakerPlug {
        settings: SpeakerPlugSettings,
        urls: PlugUrls,
        state: PlugState,
    },
    /// Answer to `TestSpeakerPlug`, to the requesting client only.
    SpeakerPlugResult { on: bool, ok: bool, message: String },
    /// OSC listen address and feedback targets in force (on connect and on every change).
    /// `listening`: the UDP socket is bound. `error`: why the saved settings were not used
    /// at startup, `null` when fine.
    OscSettings {
        settings: OscSettings,
        listening: bool,
        error: Option<String>,
    },
    /// Something the user should know about (broadcast).
    Notice { level: NoticeLevel, message: String },
    /// Request rejected; delivered only to the requesting client.
    Error { message: String },
}

/// Severity of a [`ServerEvent::Notice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
}

/// Where the room test stands (`ServerEvent::Measure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasureState {
    /// `[measure] mpd` not configured, no device identified, or link not online.
    #[default]
    Unavailable,
    Idle,
    /// Mixer snapshot taken; muting, flattening, asking MPD to play.
    Starting,
    /// The user's music is paused and the room is quiet: the phone records its
    /// background now (a few seconds, then `Playing`).
    Quiet,
    /// The noise plays (MPD confirmed): the phone records now.
    Playing,
    /// Undoing everything.
    Restoring,
    /// Failed (see `message`); what could be restored has been. `Idle` follows.
    Error,
}

/// Payload of `ServerEvent::Measure`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MeasureStatus {
    pub state: MeasureState,
    /// Mixer inputs that carry the MPD signal (`[measure] source_inputs`).
    #[serde(default)]
    pub source: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// RFC 3339 UTC time at which the watchdog ends a running test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
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
        let preamps = usize::from(t.preamps);
        ok(
            self.input_eq.len() == preamps
                && self.input_eq.iter().all(|c| c.len() == INPUT_EQ_BANDS),
            "input eq",
        )?;
        ok(self.compressors.len() == preamps, "compressors")?;
        ok(self.gates.len() == usize::from(t.mic_inputs), "gates")?;
        ok(
            self.output_eq.len() == OUTPUT_EQ_OUTPUTS
                && self.output_eq.iter().all(|c| c.len() == OUTPUT_EQ_BANDS),
            "output eq",
        )?;
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
        // EQ values a client could not write (output EQ unused slot, filter
        // type the band does not offer) are reset to the defaults instead of
        // failing the whole preset.
        for (output, rows, defaults) in [
            (false, &mut self.input_eq, EqBand::input_defaults()),
            (true, &mut self.output_eq, EqBand::output_defaults()),
        ] {
            for (ch, bands) in rows.iter_mut().enumerate() {
                let unused = output && ch == usize::from(OUTPUT_EQ_UNUSED_SLOT);
                for (band, (b, d)) in bands.iter_mut().zip(&defaults).enumerate() {
                    let allowed =
                        u8::try_from(band).is_ok_and(|n| eq_band_mode_allowed(output, n, b.mode));
                    if unused {
                        *b = *d;
                    } else if !allowed {
                        b.mode = d.mode;
                    }
                }
            }
        }
        for p in self.all_params() {
            match self.apply(&p) {
                Ok(_) => {}
                // Never written by the device, so never part of a preset.
                Err(StateError::Index {
                    what: "unused output eq slot",
                    ..
                }) => {}
                Err(e) => return Err(e),
            }
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

/// Reject non-finite, round to a whole number, then clamp (hardware integers).
fn whole(value: f64, what: &'static str, (min, max): (f64, f64)) -> Result<f64, StateError> {
    Ok(finite(value, what)?.round().clamp(min, max))
}

/// One field of an EQ band (shared by the input and output EQ params).
#[derive(Debug, Clone, Copy, PartialEq)]
enum EqField {
    Mode(u8),
    Bypass(bool),
    Freq(f64),
    Gain(f64),
    Bw(f64),
}

impl EqField {
    /// Clamp/round into the accepted range.
    fn checked(self) -> Result<Self, StateError> {
        use dsp_limits as L;
        Ok(match self {
            EqField::Freq(hz) => EqField::Freq(whole(hz, "eq_freq", L::FREQ_HZ)?),
            EqField::Gain(db) => EqField::Gain(range(db, "eq_gain", L::EQ_GAIN_DB)?),
            EqField::Bw(o) => EqField::Bw(range(o, "eq_bw", L::EQ_BW_OCT)?),
            other => other,
        })
    }

    /// Write into `band`; return whether it changed.
    fn write(self, band: &mut EqBand) -> bool {
        match self {
            EqField::Mode(m) => set(&mut band.mode, m),
            EqField::Bypass(on) => set(&mut band.bypass, on),
            EqField::Freq(hz) => set(&mut band.freq_hz, hz),
            EqField::Gain(db) => set(&mut band.gain_db, db),
            EqField::Bw(o) => set(&mut band.bw_oct, o),
        }
    }

    /// The param carrying this field for the input (`output == false`) or output EQ.
    fn param(self, output: bool, ch: u16, band: u8) -> Param {
        use Param as P;
        match (self, output) {
            (EqField::Mode(mode), false) => P::InputEqMode { ch, band, mode },
            (EqField::Bypass(on), false) => P::InputEqBypass { ch, band, on },
            (EqField::Freq(hz), false) => P::InputEqFreq { ch, band, hz },
            (EqField::Gain(db), false) => P::InputEqGain { ch, band, db },
            (EqField::Bw(octaves), false) => P::InputEqBw { ch, band, octaves },
            (EqField::Mode(mode), true) => P::OutputEqMode { ch, band, mode },
            (EqField::Bypass(on), true) => P::OutputEqBypass { ch, band, on },
            (EqField::Freq(hz), true) => P::OutputEqFreq { ch, band, hz },
            (EqField::Gain(db), true) => P::OutputEqGain { ch, band, db },
            (EqField::Bw(octaves), true) => P::OutputEqBw { ch, band, octaves },
        }
    }

    /// Every field of `b`, in a stable order.
    fn all(b: &EqBand) -> [EqField; 5] {
        [
            EqField::Mode(b.mode),
            EqField::Bypass(b.bypass),
            EqField::Freq(b.freq_hz),
            EqField::Gain(b.gain_db),
            EqField::Bw(b.bw_oct),
        ]
    }
}

/// Split an EQ param into (is output EQ, channel, band, field).
fn eq_parts(p: &Param) -> Option<(bool, u16, u8, EqField)> {
    use Param as P;
    Some(match *p {
        P::InputEqMode { ch, band, mode } => (false, ch, band, EqField::Mode(mode)),
        P::InputEqBypass { ch, band, on } => (false, ch, band, EqField::Bypass(on)),
        P::InputEqFreq { ch, band, hz } => (false, ch, band, EqField::Freq(hz)),
        P::InputEqGain { ch, band, db } => (false, ch, band, EqField::Gain(db)),
        P::InputEqBw { ch, band, octaves } => (false, ch, band, EqField::Bw(octaves)),
        P::OutputEqMode { ch, band, mode } => (true, ch, band, EqField::Mode(mode)),
        P::OutputEqBypass { ch, band, on } => (true, ch, band, EqField::Bypass(on)),
        P::OutputEqFreq { ch, band, hz } => (true, ch, band, EqField::Freq(hz)),
        P::OutputEqGain { ch, band, db } => (true, ch, band, EqField::Gain(db)),
        P::OutputEqBw { ch, band, octaves } => (true, ch, band, EqField::Bw(octaves)),
        _ => return None,
    })
}

/// Push the params for every field of `ys` that differs from `xs` (or all).
fn eq_params(
    out: &mut Vec<Param>,
    output: bool,
    xs: &[Vec<EqBand>],
    ys: &[Vec<EqBand>],
    all: bool,
) {
    for (ch, (a, b)) in xs.iter().zip(ys).enumerate() {
        let Ok(ch) = u16::try_from(ch) else { break };
        for (band, (x, y)) in a.iter().zip(b).enumerate() {
            let Ok(band) = u8::try_from(band) else { break };
            for (fx, fy) in EqField::all(x).into_iter().zip(EqField::all(y)) {
                if all || fx != fy {
                    out.push(fy.param(output, ch, band));
                }
            }
        }
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

    fn gate(&mut self, ch: u16) -> Result<&mut Gate, StateError> {
        slot(&mut self.gates, usize::from(ch), "gate")
    }

    fn comp(&mut self, ch: u16) -> Result<&mut Compressor, StateError> {
        slot(&mut self.compressors, usize::from(ch), "compressor")
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
            P::InputEqMode { .. }
            | P::InputEqBypass { .. }
            | P::InputEqFreq { .. }
            | P::InputEqGain { .. }
            | P::InputEqBw { .. }
            | P::OutputEqMode { .. }
            | P::OutputEqBypass { .. }
            | P::OutputEqFreq { .. }
            | P::OutputEqGain { .. }
            | P::OutputEqBw { .. } => {
                let Some((output, ch, band, field)) = eq_parts(param) else {
                    return Ok(None);
                };
                let field = field.checked()?;
                let (rows, what) = if output {
                    (&mut self.output_eq, "output eq")
                } else {
                    (&mut self.input_eq, "input eq")
                };
                let row = slot(rows, usize::from(ch), what)?;
                let b = slot(row, usize::from(band), "eq band")?;
                (field.write(b), field.param(output, ch, band))
            }
            P::GateBypass { ch, on } => (set(&mut self.gate(ch)?.bypass, on), *param),
            P::GateThreshold { ch, db } => {
                let v = range(db, "gate_threshold", dsp_limits::THRESHOLD_DB)?;
                (
                    set(&mut self.gate(ch)?.threshold_db, v),
                    P::GateThreshold { ch, db: v },
                )
            }
            P::GateAttack { ch, ms } => {
                let v = whole(ms, "gate_attack", dsp_limits::ATTACK_MS)?;
                (
                    set(&mut self.gate(ch)?.attack_ms, v),
                    P::GateAttack { ch, ms: v },
                )
            }
            P::GateRelease { ch, ms } => {
                let v = whole(ms, "gate_release", dsp_limits::RELEASE_MS)?;
                (
                    set(&mut self.gate(ch)?.release_ms, v),
                    P::GateRelease { ch, ms: v },
                )
            }
            P::CompBypass { ch, on } => (set(&mut self.comp(ch)?.bypass, on), *param),
            P::CompThreshold { ch, db } => {
                let v = range(db, "comp_threshold", dsp_limits::THRESHOLD_DB)?;
                (
                    set(&mut self.comp(ch)?.threshold_db, v),
                    P::CompThreshold { ch, db: v },
                )
            }
            P::CompAttack { ch, ms } => {
                let v = whole(ms, "comp_attack", dsp_limits::ATTACK_MS)?;
                (
                    set(&mut self.comp(ch)?.attack_ms, v),
                    P::CompAttack { ch, ms: v },
                )
            }
            P::CompRelease { ch, ms } => {
                let v = whole(ms, "comp_release", dsp_limits::RELEASE_MS)?;
                (
                    set(&mut self.comp(ch)?.release_ms, v),
                    P::CompRelease { ch, ms: v },
                )
            }
            P::CompRatio { ch, ratio } => {
                let v = range(ratio, "comp_ratio", dsp_limits::COMP_RATIO)?;
                (
                    set(&mut self.comp(ch)?.ratio, v),
                    P::CompRatio { ch, ratio: v },
                )
            }
            P::CompMakeup { ch, gain } => {
                let v = range(gain, "comp_makeup", dsp_limits::COMP_MAKEUP)?;
                (
                    set(&mut self.comp(ch)?.makeup, v),
                    P::CompMakeup { ch, gain: v },
                )
            }
            P::ReverbDecay { value } => {
                let v = whole(value, "reverb_decay", dsp_limits::REVERB_PERCENT)?;
                (set(&mut self.reverb.decay, v), P::ReverbDecay { value: v })
            }
            P::ReverbDamping { value } => {
                let v = whole(value, "reverb_damping", dsp_limits::REVERB_PERCENT)?;
                (
                    set(&mut self.reverb.damping, v),
                    P::ReverbDamping { value: v },
                )
            }
            P::ReverbPredelay { ms } => {
                let v = whole(ms, "reverb_predelay", dsp_limits::REVERB_PREDELAY_MS)?;
                (
                    set(&mut self.reverb.predelay_ms, v),
                    P::ReverbPredelay { ms: v },
                )
            }
            P::ReverbWidth { value } => {
                let v = whole(value, "reverb_width", dsp_limits::REVERB_PERCENT)?;
                (set(&mut self.reverb.width, v), P::ReverbWidth { value: v })
            }
            P::ReverbPreset { preset } => (set(&mut self.reverb.preset, preset), *param),
            P::SampleRate { rate } => (set(&mut self.clock.sample_rate, rate), *param),
            P::ClockSource { source } => (set(&mut self.clock.source, source), *param),
            P::OpticalMode { output, mode } => {
                let slot = if output {
                    &mut self.clock.optical_out
                } else {
                    &mut self.clock.optical_in
                };
                (set(slot, mode), *param)
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
        eq_params(&mut out, false, &self.input_eq, &target.input_eq, all);
        for (ch, (x, y)) in self.compressors.iter().zip(&target.compressors).enumerate() {
            let Ok(ch) = u16::try_from(ch) else { break };
            for (differs, p) in [
                (x.bypass != y.bypass, Param::CompBypass { ch, on: y.bypass }),
                (
                    x.threshold_db != y.threshold_db,
                    Param::CompThreshold {
                        ch,
                        db: y.threshold_db,
                    },
                ),
                (
                    x.attack_ms != y.attack_ms,
                    Param::CompAttack {
                        ch,
                        ms: y.attack_ms,
                    },
                ),
                (
                    x.release_ms != y.release_ms,
                    Param::CompRelease {
                        ch,
                        ms: y.release_ms,
                    },
                ),
                (x.ratio != y.ratio, Param::CompRatio { ch, ratio: y.ratio }),
                (
                    x.makeup != y.makeup,
                    Param::CompMakeup { ch, gain: y.makeup },
                ),
            ] {
                if all || differs {
                    out.push(p);
                }
            }
        }
        for (ch, (x, y)) in self.gates.iter().zip(&target.gates).enumerate() {
            let Ok(ch) = u16::try_from(ch) else { break };
            for (differs, p) in [
                (x.bypass != y.bypass, Param::GateBypass { ch, on: y.bypass }),
                (
                    x.threshold_db != y.threshold_db,
                    Param::GateThreshold {
                        ch,
                        db: y.threshold_db,
                    },
                ),
                (
                    x.attack_ms != y.attack_ms,
                    Param::GateAttack {
                        ch,
                        ms: y.attack_ms,
                    },
                ),
                (
                    x.release_ms != y.release_ms,
                    Param::GateRelease {
                        ch,
                        ms: y.release_ms,
                    },
                ),
            ] {
                if all || differs {
                    out.push(p);
                }
            }
        }
        eq_params(&mut out, true, &self.output_eq, &target.output_eq, all);
        let (ra, rb) = (&self.reverb, &target.reverb);
        for (differs, p) in [
            (ra.decay != rb.decay, Param::ReverbDecay { value: rb.decay }),
            (
                ra.damping != rb.damping,
                Param::ReverbDamping { value: rb.damping },
            ),
            (
                ra.predelay_ms != rb.predelay_ms,
                Param::ReverbPredelay { ms: rb.predelay_ms },
            ),
            (ra.width != rb.width, Param::ReverbWidth { value: rb.width }),
            (
                ra.preset != rb.preset,
                Param::ReverbPreset { preset: rb.preset },
            ),
        ] {
            if all || differs {
                out.push(p);
            }
        }
        if all || self.clock.sample_rate != target.clock.sample_rate {
            out.push(Param::SampleRate {
                rate: target.clock.sample_rate,
            });
        }
        if all || self.clock.source != target.clock.source {
            out.push(Param::ClockSource {
                source: target.clock.source,
            });
        }
        if all || self.clock.optical_in != target.clock.optical_in {
            out.push(Param::OpticalMode {
                output: false,
                mode: target.clock.optical_in,
            });
        }
        if all || self.clock.optical_out != target.clock.optical_out {
            out.push(Param::OpticalMode {
                output: true,
                mode: target.clock.optical_out,
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
///
/// - v1: per-mixer-input `channels` DSP and a `reverb` block with sends.
/// - v2: hardware-shaped DSP (`input_eq`, `compressors`, `gates`,
///   `output_eq`) and reverb (`decay`, `damping`, `predelay_ms`, `width`,
///   `preset`); reverb sends/return are mix faders. Older files drop their
///   DSP/reverb sections and take the defaults (`presets::parse_preset`).
pub const PRESET_SCHEMA_VERSION: u32 = 2;

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

/// Invisible characters that reorder or hide text (bidi overrides, isolates and marks,
/// zero-width space, byte-order mark): refused in names shown to other people.
fn is_spoofing_char(c: char) -> bool {
    matches!(c,
        '\u{061C}' | '\u{200B}' | '\u{200E}' | '\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

impl ChannelLabel {
    /// Name 1–64 chars without control characters; colour `#RRGGBB` if set.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.chars().count();
        if n == 0
            || self.name.trim().is_empty()
            || n > LABEL_NAME_MAX
            || self
                .name
                .chars()
                .any(|c| c.is_control() || is_spoofing_char(c))
        {
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
            Param::InputEqBypass {
                ch: 4,
                band: 1,
                on: true,
            },
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
            + usize::from(t.preamps) * INPUT_EQ_BANDS * 5 // mode, bypass, freq, gain, bw
            + usize::from(t.preamps) * 6 // compressor
            + usize::from(t.mic_inputs) * 4 // gate
            + OUTPUT_EQ_OUTPUTS * OUTPUT_EQ_BANDS * 5
            + 5 // reverb
            + 4 // sample rate, clock source, optical in/out format
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
    fn default_dsp_is_hardware_shaped() {
        let s = MotuState::default();
        assert_eq!(s.input_eq.len(), 8);
        assert!(s.input_eq.iter().all(|c| c.len() == INPUT_EQ_BANDS));
        let modes: Vec<u8> = s.input_eq[0].iter().map(|b| b.mode).collect();
        assert_eq!(
            modes,
            [
                eq_mode::LOW_SHELF,
                eq_mode::BELL,
                eq_mode::BELL,
                eq_mode::HIGH_SHELF
            ]
        );
        let freqs: Vec<f64> = s.input_eq[3].iter().map(|b| b.freq_hz).collect();
        assert_eq!(freqs, [60.0, 400.0, 2_000.0, 14_000.0]);
        assert_eq!(s.compressors.len(), 8);
        assert_eq!(s.gates.len(), 2);
        assert_eq!(s.output_eq.len(), OUTPUT_EQ_OUTPUTS);
        assert!(s.output_eq.iter().all(|c| c.len() == OUTPUT_EQ_BANDS));
        let modes: Vec<u8> = s.output_eq[0].iter().map(|b| b.mode).collect();
        assert_eq!(
            modes,
            [eq_mode::LOW_SHELF, eq_mode::BELL, eq_mode::HIGH_SHELF]
        );
        let freqs: Vec<f64> = s.output_eq[7].iter().map(|b| b.freq_hz).collect();
        assert_eq!(freqs, [60.0, 800.0, 14_000.0]);
        assert_eq!(s.reverb.decay, 86.0);
        assert!(s.check_shape().is_ok());
    }

    #[test]
    fn output_eq_rejects_what_the_device_never_gets() {
        let mut s = MotuState::default();
        // Unused slot 1: rejected, state untouched.
        for p in [
            Param::OutputEqGain {
                ch: 1,
                band: 0,
                db: 3.0,
            },
            Param::OutputEqBypass {
                ch: 1,
                band: 2,
                on: true,
            },
        ] {
            assert!(
                matches!(check_writable(&p), Err(StateError::Index { .. })),
                "{p:?}"
            );
        }
        // Filter type the band does not offer.
        for (band, mode) in [(0u8, 2u8), (1, 1), (1, 3), (2, 1), (2, 3), (0, 255)] {
            let p = Param::OutputEqMode { ch: 0, band, mode };
            assert!(
                matches!(check_writable(&p), Err(StateError::Index { .. })),
                "{p:?}"
            );
        }
        assert_eq!(s, MotuState::default());
        // The device itself is the reference: what it reports is kept.
        let dev = Param::OutputEqGain {
            ch: 1,
            band: 0,
            db: 3.0,
        };
        assert_eq!(s.apply(&dev), Ok(Some(dev)));
        s = MotuState::default();
        // Allowed ones go through.
        let ok = Param::OutputEqMode {
            ch: 0,
            band: 0,
            mode: eq_mode::HIGH_PASS,
        };
        assert_eq!(check_writable(&ok), Ok(()));
        assert_eq!(s.apply(&ok), Ok(Some(ok)));
    }

    #[test]
    fn input_eq_modes_follow_the_band_rule_on_the_command_path() {
        // 4 bands: first bell / low shelf / high-pass, middle bell, last bell / high shelf.
        let allowed: [&[u8]; 4] = [&[0, 1, 3], &[0], &[0], &[0, 2]];
        for (band, ok) in allowed.iter().enumerate() {
            let band = band as u8;
            for mode in 0..=u8::MAX {
                let p = Param::InputEqMode { ch: 0, band, mode };
                assert_eq!(check_writable(&p).is_ok(), ok.contains(&mode), "{p:?}");
            }
        }
        // Same rule for the 3-band output EQ, from one function.
        assert!(eq_mode_allowed(2, OUTPUT_EQ_BANDS, eq_mode::HIGH_SHELF));
        assert!(!eq_mode_allowed(3, OUTPUT_EQ_BANDS, eq_mode::BELL));
        // The device reference is always applied, whatever the mode.
        let mut s = MotuState::default();
        let dev = Param::InputEqMode {
            ch: 0,
            band: 1,
            mode: 7,
        };
        assert_eq!(s.apply(&dev), Ok(Some(dev)));
        // Non-mode fields and other params are not restricted.
        assert_eq!(
            check_writable(&Param::InputEqGain {
                ch: 0,
                band: 1,
                db: 3.0
            }),
            Ok(())
        );
    }

    #[test]
    fn normalized_repairs_invalid_input_eq_instead_of_failing() {
        let mut s = MotuState::default();
        s.input_eq[2][1].mode = eq_mode::HIGH_PASS;
        s.input_eq[2][3].mode = eq_mode::LOW_SHELF;
        s.input_eq[2][3].gain_db = 4.0;
        s.input_eq[5][0].mode = 200;
        let n = s.normalized().unwrap_or_default();
        let fresh = EqBand::input_defaults();
        assert_eq!(n.input_eq[2][1].mode, fresh[1].mode);
        assert_eq!(n.input_eq[2][3].mode, fresh[3].mode);
        assert_eq!(n.input_eq[2][3].gain_db, 4.0, "other fields kept");
        assert_eq!(n.input_eq[5][0].mode, fresh[0].mode);
    }

    #[test]
    fn normalized_repairs_invalid_output_eq_instead_of_failing() {
        let mut s = MotuState::default();
        s.output_eq[1][0].gain_db = 9.0;
        s.output_eq[1][1].bypass = true;
        s.output_eq[4][1].mode = eq_mode::HIGH_PASS;
        s.output_eq[4][2].mode = 200;
        s.output_eq[4][2].gain_db = 5.0;
        let n = s.normalized();
        let n = n.unwrap_or_default();
        let fresh = EqBand::output_defaults();
        assert_eq!(n.output_eq[1], fresh, "unused slot reset");
        assert_eq!(n.output_eq[4][1].mode, fresh[1].mode);
        assert_eq!(n.output_eq[4][2].mode, fresh[2].mode);
        assert_eq!(n.output_eq[4][2].gain_db, 5.0, "other fields kept");
    }

    #[test]
    fn dsp_params_clamp_validate_and_replay() {
        let mut s = MotuState::default();
        assert_eq!(
            s.apply(&Param::InputEqFreq {
                ch: 3,
                band: 2,
                hz: 50_000.4
            }),
            Ok(Some(Param::InputEqFreq {
                ch: 3,
                band: 2,
                hz: 20_000.0
            }))
        );
        assert_eq!(
            s.apply(&Param::OutputEqGain {
                ch: 11,
                band: 2,
                db: -12.7
            }),
            Ok(Some(Param::OutputEqGain {
                ch: 11,
                band: 2,
                db: -12.7
            }))
        );
        // Unknown filter types are kept.
        assert_eq!(
            s.apply(&Param::InputEqMode {
                ch: 0,
                band: 1,
                mode: 7
            }),
            Ok(Some(Param::InputEqMode {
                ch: 0,
                band: 1,
                mode: 7
            }))
        );
        assert!(matches!(
            s.apply(&Param::InputEqBw {
                ch: 0,
                band: 4,
                octaves: 1.0
            }),
            Err(StateError::Index {
                what: "eq band",
                ..
            })
        ));
        assert!(matches!(
            s.apply(&Param::OutputEqBypass {
                ch: 12,
                band: 0,
                on: true
            }),
            Err(StateError::Index {
                what: "output eq",
                ..
            })
        ));
        assert!(matches!(
            s.apply(&Param::GateBypass { ch: 2, on: false }),
            Err(StateError::Index { what: "gate", .. })
        ));
        assert!(matches!(
            s.apply(&Param::CompRatio { ch: 8, ratio: 2.0 }),
            Err(StateError::Index {
                what: "compressor",
                ..
            })
        ));
        assert_eq!(
            s.apply(&Param::CompThreshold {
                ch: 0,
                db: f64::NAN
            }),
            Err(StateError::NonFinite("comp_threshold"))
        );
        assert_eq!(
            s.apply(&Param::CompAttack { ch: 1, ms: 12.6 }),
            Ok(Some(Param::CompAttack { ch: 1, ms: 13.0 })),
            "Int16 ms are whole"
        );
        assert_eq!(
            s.apply(&Param::CompMakeup { ch: 7, gain: 9.638 }),
            Ok(Some(Param::CompMakeup { ch: 7, gain: 9.638 }))
        );
        assert_eq!(
            s.apply(&Param::GateRelease { ch: 1, ms: 1e9 }),
            Ok(Some(Param::GateRelease {
                ch: 1,
                ms: dsp_limits::RELEASE_MS.1
            }))
        );
        assert_eq!(
            s.apply(&Param::ReverbDecay { value: 140.0 }),
            Ok(Some(Param::ReverbDecay { value: 100.0 }))
        );
        let _ = s.apply(&Param::ReverbPreset { preset: 4 });
        let _ = s.apply(&Param::SampleRate {
            rate: SampleRate::Hz96000,
        });
        let _ = s.apply(&Param::ClockSource {
            source: ClockSource::Optical,
        });

        // A preset recall replays every DSP change exactly.
        let mut replay = MotuState::default();
        for p in MotuState::default().diff(&s) {
            assert!(replay.apply(&p).is_ok());
        }
        assert_eq!(replay, s);
    }

    #[test]
    fn dsp_json_shapes() {
        let j = |p: Param| serde_json::to_value(p).unwrap_or_default();
        assert_eq!(
            j(Param::InputEqFreq {
                ch: 0,
                band: 1,
                hz: 350.0
            }),
            serde_json::json!({"param":"input_eq_freq","ch":0,"band":1,"hz":350.0})
        );
        assert_eq!(
            j(Param::CompMakeup { ch: 2, gain: 1.0 }),
            serde_json::json!({"param":"comp_makeup","ch":2,"gain":1.0})
        );
        assert_eq!(
            j(Param::SampleRate {
                rate: SampleRate::Hz48000
            }),
            serde_json::json!({"param":"sample_rate","rate":"48000"})
        );
        assert_eq!(
            j(Param::ClockSource {
                source: ClockSource::Internal
            }),
            serde_json::json!({"param":"clock_source","source":"internal"})
        );
    }

    #[test]
    fn clock_hardware_codes() {
        for r in SampleRate::ALL {
            assert_eq!(SampleRate::from_hz(r.hz()), Some(r));
        }
        assert_eq!(SampleRate::from_hz(12_345), None);
        for c in [
            ClockSource::Internal,
            ClockSource::Spdif,
            ClockSource::Optical,
        ] {
            assert_eq!(ClockSource::from_code(c.code()), Some(c));
        }
        assert_eq!(ClockSource::from_code(1), None);
    }

    #[test]
    fn shape_checks_cover_dsp() {
        let mut bad = MotuState::default();
        bad.gates.pop();
        assert_eq!(bad.check_shape(), Err(StateError::Shape("gates")));
        let mut bad = MotuState::default();
        bad.input_eq[2].pop();
        assert_eq!(bad.check_shape(), Err(StateError::Shape("input eq")));
        let mut bad = MotuState::default();
        bad.output_eq.pop();
        assert_eq!(bad.check_shape(), Err(StateError::Shape("output eq")));
        let mut fixed = MotuState::default();
        fixed.gates.clear();
        fixed.reset_dsp();
        assert!(fixed.check_shape().is_ok());
    }

    #[test]
    fn command_validation_refuses_hostile_strings() {
        let name = |n: &str| Command::LoadPreset { name: n.into() };
        assert!(name("Live set 1").validate().is_ok());
        for bad in [
            "",
            "../x",
            "a/b",
            "a\0b",
            ".hidden",
            "\u{ff0f}",
            "\u{202e}x",
        ] {
            assert!(name(bad).validate().is_err(), "{bad:?}");
            assert!(
                Command::SavePreset { name: bad.into() }.validate().is_err(),
                "{bad:?}"
            );
            assert!(
                Command::ImportPreset {
                    name: bad.into(),
                    text: "{}".into()
                }
                .validate()
                .is_err(),
                "{bad:?}"
            );
        }
        let import = |text: String| Command::ImportPreset {
            name: "ok".into(),
            text,
        };
        assert!(import("{}".into()).validate().is_ok());
        assert!(import("a\0b".into()).validate().is_err());
        assert!(import("x".repeat(IMPORT_TEXT_MAX + 1)).validate().is_err());
        assert!(import("x".repeat(IMPORT_TEXT_MAX)).validate().is_ok());

        let label = |n: &str, c: Option<&str>| Command::SetLabel {
            ch: 0,
            label: Some(ChannelLabel {
                name: n.into(),
                color: c.map(str::to_owned),
            }),
        };
        assert!(label("Kick", Some("#06B6D4")).validate().is_ok());
        assert!(label("Gu\u{ee}tare", None).validate().is_ok());
        for bad in [
            "",
            "   ",
            "\t",
            "a\nb",
            "a\0",
            "\u{202e}cba",
            "a\u{200b}b",
            "\u{feff}x",
            "\u{2066}x",
        ] {
            assert!(label(bad, None).validate().is_err(), "{bad:?}");
        }
        assert!(label(&"x".repeat(65), None).validate().is_err());
        for bad in [
            "red",
            "#12345",
            "#GGGGGG",
            "#06B6D4\n",
            "url(x)",
            "#06B6D4;x",
        ] {
            assert!(label("ok", Some(bad)).validate().is_err(), "{bad:?}");
        }
        assert!(Command::SetLabel { ch: 0, label: None }.validate().is_ok());

        let plug = |f: fn(&mut SpeakerPlugSettings)| {
            let mut s = SpeakerPlugSettings::default();
            f(&mut s);
            Command::SetSpeakerPlug { settings: s }.validate()
        };
        assert!(plug(|_| {}).is_ok());
        assert!(plug(|s| s.host = "a\r\nX: 1".into()).is_err());
        assert!(plug(|s| s.on_url = "x".repeat(5000)).is_err());
        let osc = |listen: &str| Command::SetOscSettings {
            settings: OscSettings {
                listen: listen.into(),
                targets: vec![],
            },
        };
        assert!(osc("0.0.0.0:8000").validate().is_ok());
        assert!(osc("0.0.0.0:8000\r\n").validate().is_err());
    }

    #[test]
    fn numbers_from_clients_are_finite_and_in_range() {
        let mut st = MotuState::default();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for p in [
                Param::MainVol { db: bad },
                Param::PhonesVol { db: bad },
                Param::PreampGain { ch: 0, db: bad },
                Param::OutputTrim { ch: 0, db: bad },
                Param::MixPan {
                    bus: 0,
                    ch: 0,
                    value: bad,
                },
            ] {
                assert!(
                    matches!(st.apply(&p), Err(StateError::NonFinite(_))),
                    "{p:?}"
                );
            }
        }
        // Huge finite values are clamped, never stored as is.
        let huge = st.apply(&Param::MainVol { db: 1e308 }).unwrap();
        assert!(matches!(huge, Some(Param::MainVol { db }) if db <= 0.0) || huge.is_none());
        let low = st.apply(&Param::PreampGain { ch: 0, db: -1e308 }).unwrap();
        assert!(matches!(low, Some(Param::PreampGain { db, .. }) if db >= 0.0) || low.is_none());
        // Huge indices are refused; the JSON types refuse negatives and wrong types.
        assert!(st
            .apply(&Param::Phantom {
                ch: u16::MAX,
                on: true
            })
            .is_err());
        for bad in [
            r#"{"cmd":"set","param":"phantom","ch":-1,"on":true}"#,
            r#"{"cmd":"set","param":"phantom","ch":70000,"on":true}"#,
            r#"{"cmd":"set","param":"phantom","ch":1.5,"on":true}"#,
            r#"{"cmd":"set","param":"main_vol","db":"loud"}"#,
            r#"{"cmd":"set","param":"main_vol","db":null}"#,
            r#"{"cmd":"nope"}"#,
            r#"{"cmd":["set"]}"#,
            "[]",
            "null",
        ] {
            assert!(serde_json::from_str::<Command>(bad).is_err(), "{bad}");
        }
    }
}
