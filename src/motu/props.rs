//! MOTU UltraLite-mk5 property table: `prop_id` ⇄ [`Param`] and payload kind.
//!
//! **Owner: `agent-motu-protocol` (props).** Source: the open-source
//! `motu-mk5-linux` project (maintainer ruling Q3). Every entry stays
//! `verified: false` until tested on real hardware by the maintainer.
//!
//! Stable interface (used by `motu::client` — do not change signatures
//! without updating `docs/ARCHITECTURE.md`):
//! - [`kind_of`]     — payload kind of a property, `None` if unknown.
//! - [`encode`]      — effective [`Param`] → wire frames (may be several, or none
//!   if the parameter has no known hardware mapping yet).
//! - [`decode`]      — hardware frame → [`Param`], `None` if unmapped.
//! - [`ENTRIES`]     — the table itself, with provenance, for docs/tests.
//!
//! Honesty notes (details in `docs/MOTU-PROTOCOL.md`):
//! - IDs/types are facts taken from the sources; no code was copied
//!   (`motu-mk5-linux` is GPL-2.0-or-later).
//! - Several `Param`s have **no mapping** because the sources do not document
//!   their index layout (mix matrix, routing, per-channel EQ bypass) or the
//!   hardware has no such parameter (dim, speaker B): `encode` returns no
//!   frame and `decode` returns `None` for them.
//! - `Array` properties of the device cannot be carried by [`ValueKind`] and
//!   are therefore absent from [`ENTRIES`] (`Int16` ones are listed since
//!   [`ValueKind::I16`] exists).
//! - DSP / reverb / clock (input EQ 8 x 4 bands, output EQ 12 x 3 bands,
//!   compressor 8, gate 2, reverb, sample rate, clock source) are **decoded**
//!   from the device (layouts read from a connect dump, 2026-11-15). Their
//!   encoders exist ([`encode_unverified`], unit-tested) but are **not active**:
//!   [`encode`] returns no frame for them, so nothing is written until each one
//!   is checked on the card.

use super::protocol::{Frame, ProtocolError, Value, ValueKind};
use crate::state::{dsp_limits, ClockSource, Param, SampleRate};

/// One row of the property table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropEntry {
    pub prop_id: u16,
    pub kind: ValueKind,
    /// Human description, e.g. `"mix channel fader (index = bus*64+ch)"`.
    pub name: &'static str,
    /// Where the mapping comes from (file/commit in motu-mk5-linux).
    pub source: &'static str,
    /// `true` only once confirmed on hardware by the maintainer.
    pub verified: bool,
}

const SRC_MK5: &str = "motu-mk5-linux src/protocol/properties.rs @bf43e08 \
    (https://github.com/MarkWind85/motu-mk5-linux, GPL-2.0-or-later; id/type facts only)";
const SRC_SD: &str = "licanccc/motu-monitor-control com.local.motu-monitor.sdPlugin/bin/plugin.js \
    @70b4969 (MIT; id/index observed on hardware by that project)";

const fn entry(
    prop_id: u16,
    kind: ValueKind,
    name: &'static str,
    source: &'static str,
) -> PropEntry {
    PropEntry {
        prop_id,
        kind,
        name,
        source,
        verified: false,
    }
}

// Property ids used by the encoders/decoders below.
const P_OUTPUT_TRIM: u16 = 5000;
const P_INPUT_GAIN: u16 = 5001;
const P_INPUT_PHASE: u16 = 5002;
const P_INPUT_PAD: u16 = 5003;
const P_INPUT_48V: u16 = 5004;
const P_MAIN_TRIM: u16 = 5011;
const P_MONO: u16 = 5018;
const P_TALKBACK_ENABLE: u16 = 5027;
const P_SAMPLE_RATE: u16 = 10;
const P_CLOCK_SOURCE: u16 = 11;
/// Input EQ: mode, bypass, freq, gain, bw (`1002..=1006`), index = ch * 4 + band.
const P_IN_EQ_MODE: u16 = 1002;
const P_IN_EQ_BW: u16 = 1006;
const P_GATE_BYPASS: u16 = 1007;
const P_GATE_THRESHOLD: u16 = 1008;
const P_GATE_ATTACK: u16 = 1009;
const P_GATE_RELEASE: u16 = 1010;
const P_COMP_BYPASS: u16 = 1011;
const P_COMP_THRESHOLD: u16 = 1012;
const P_COMP_ATTACK: u16 = 1013;
const P_COMP_RELEASE: u16 = 1014;
const P_COMP_RATIO: u16 = 1015;
const P_COMP_MAKEUP: u16 = 1021;
/// Output EQ: mode, bypass, freq, gain, bw (`1022..=1026`), index = out * 3 + band.
const P_OUT_EQ_MODE: u16 = 1022;
const P_OUT_EQ_BW: u16 = 1026;
const P_REVERB_DECAY: u16 = 1030;
const P_REVERB_DAMPING: u16 = 1031;
const P_REVERB_PREDELAY: u16 = 1032;
const P_REVERB_WIDTH: u16 = 1033;
const P_REVERB_PRESET: u16 = 1034;

/// Input EQ bands per channel (channels = [`N_INPUTS`]).
const N_IN_EQ_BANDS: u16 = 4;
/// Outputs with an EQ.
const N_OUT_EQ: u16 = 12;
/// Output EQ bands per output.
const N_OUT_EQ_BANDS: u16 = 3;
/// Inputs with a compressor.
const N_COMPS: u16 = 8;
/// Inputs with a gate (the mic inputs).
const N_GATES: u16 = 2;

/// Number of analogue/line inputs with a gain and polarity control.
const N_INPUTS: u16 = 8;
/// Number of mic inputs (pad, phantom).
const N_MICS: u16 = 2;
/// Output trim indices of the headphone pair (left, right).
const PHONES_L: u16 = 10;
/// Analog line outputs with a trim, before the phones pair (indices 0..=9).
const N_LINE_OUTPUTS: u16 = 10;
const PHONES_R: u16 = 11;
/// Mic input gain ceiling in dB (line inputs stop at 20 dB).
const GAIN_MAX_DB: f64 = 74.0;
/// Line input gain ceiling in dB.
const LINE_GAIN_MAX_DB: f64 = 20.0;
/// Output trim attenuation range: `0..=100` = `0..-100` dB.
const TRIM_MAX: u8 = 100;

use ValueKind::{Fixed as F, Str32 as S, I16 as W, U32 as L, U8 as B};

/// Property table. Only kinds representable by [`ValueKind`] are listed.
pub const ENTRIES: &[PropEntry] = &[
    entry(3, S, "device_name: user device name", SRC_MK5),
    entry(
        6,
        S,
        "input_names: 18 x 32-byte names (index = channel)",
        SRC_MK5,
    ),
    entry(
        7,
        S,
        "output_names: 22 x 32-byte names (index = channel)",
        SRC_MK5,
    ),
    entry(10, L, "sample_rate: Hz (Int32)", SRC_MK5),
    entry(
        11,
        B,
        "clock_source: 0=S/PDIF 2=Optical 3=Internal",
        SRC_MK5,
    ),
    entry(15, B, "fpga_locked (ro)", SRC_MK5),
    entry(16, B, "pll_locked (ro)", SRC_MK5),
    entry(19, B, "midi_thru", SRC_MK5),
    entry(20, B, "loopback_first", SRC_MK5),
    entry(21, S, "model_id (ro)", SRC_MK5),
    entry(
        P_OUTPUT_TRIM,
        B,
        "output_trim: 0..100 = 0..-100 dB, 12 outputs (phones = idx 10/11)",
        SRC_MK5,
    ),
    entry(
        P_INPUT_GAIN,
        B,
        "input_gain: whole dB (mic 0-74, line 0-20), 8 inputs",
        SRC_MK5,
    ),
    entry(
        P_INPUT_PHASE,
        B,
        "input_phase: polarity invert, 8 inputs",
        SRC_MK5,
    ),
    entry(P_INPUT_PAD, B, "input_pad: mic pad, 2 mic inputs", SRC_MK5),
    entry(
        P_INPUT_48V,
        B,
        "input_48v: phantom power, 2 mic inputs",
        SRC_MK5,
    ),
    entry(5005, B, "jack_detect (ro)", SRC_MK5),
    entry(5006, B, "optical_mode", SRC_MK5),
    entry(
        5010,
        B,
        "fpga_patch: routing table, 80 entries (semantics undocumented)",
        SRC_MK5,
    ),
    entry(P_MAIN_TRIM, B, "main_trim: 0..100 = 0..-100 dB", SRC_MK5),
    entry(5014, B, "loopback_source", SRC_MK5),
    entry(5015, B, "ab_enable", SRC_MK5),
    entry(5016, B, "a_enable", SRC_MK5),
    entry(5017, B, "b_enable", SRC_MK5),
    entry(
        P_MONO,
        B,
        "mono_enable (hidden in CueMix UI, works per licanccc)",
        SRC_MK5,
    ),
    entry(
        5019,
        B,
        "mute_enable (reported NOT to cut audio, unused)",
        SRC_MK5,
    ),
    entry(5020, B, "optical_expander_patch: 80 entries", SRC_MK5),
    entry(5024, B, "word_clock_out", SRC_MK5),
    entry(5025, B, "optical_expander", SRC_MK5),
    entry(5026, B, "talkback_latch", SRC_MK5),
    entry(P_TALKBACK_ENABLE, B, "talkback_enable", SRC_MK5),
    entry(5028, F, "talkback_level (8.24)", SRC_MK5),
    entry(5029, F, "talkback_dim (8.24)", SRC_MK5),
    entry(5030, B, "talkback_source", SRC_MK5),
    entry(5031, L, "talkback_destination bitmask", SRC_MK5),
    entry(5032, B, "footswitch_enabled", SRC_MK5),
    entry(5033, S, "footswitch_down_key", SRC_MK5),
    entry(5034, S, "footswitch_up_key", SRC_MK5),
    entry(1000, B, "mix_input_stereo: 32 inputs", SRC_MK5),
    entry(1001, B, "mix_output_stereo: 14 buses", SRC_MK5),
    entry(
        1002,
        B,
        "input_eq_mode: 32 = 8 ch x 4 bands, index = ch*4+band (dump); 0 bell, 2 shelf?, 3 high-pass",
        SRC_MK5,
    ),
    entry(1003, B, "input_eq_bypass: 32, index = ch*4+band", SRC_MK5),
    entry(
        1004,
        L,
        "input_eq_freq: plain integer Hz (Int32, not 8.24), index = ch*4+band",
        SRC_MK5,
    ),
    entry(1005, F, "input_eq_gain: dB (8.24), index = ch*4+band", SRC_MK5),
    entry(1006, F, "input_eq_bw: octaves (8.24), index = ch*4+band", SRC_MK5),
    entry(1007, B, "gate_bypass: 2 mic channels, 1 = bypassed", SRC_MK5),
    entry(1008, F, "gate_threshold: dB (8.24; dump -46)", SRC_MK5),
    entry(P_GATE_ATTACK, W, "gate_attack: ms (Int16)", SRC_MK5),
    entry(P_GATE_RELEASE, W, "gate_release: ms (Int16)", SRC_MK5),
    entry(1011, B, "comp_bypass: 8 channels, 1 = bypassed", SRC_MK5),
    entry(1012, F, "comp_threshold: dB (8.24)", SRC_MK5),
    entry(P_COMP_ATTACK, W, "comp_attack: ms (Int16)", SRC_MK5),
    entry(P_COMP_RELEASE, W, "comp_release: ms (Int16)", SRC_MK5),
    entry(1015, F, "comp_ratio: n in n:1 (8.24)", SRC_MK5),
    entry(
        1016,
        F,
        "mix_fader: 448 = 32 inputs x 14 buses, index layout undocumented",
        SRC_MK5,
    ),
    entry(
        1017,
        F,
        "mix_pan: 448 entries, index layout undocumented",
        SRC_MK5,
    ),
    entry(
        1018,
        B,
        "mix_solo: 448 entries, index layout undocumented",
        SRC_MK5,
    ),
    entry(
        1019,
        B,
        "mix_mute: 448 entries, index layout undocumented",
        SRC_MK5,
    ),
    entry(1021, F, "comp_makeup: linear gain (8.24, 1.0 = 0 dB)", SRC_MK5),
    entry(
        1022,
        B,
        "output_eq_mode: 36 = 12 out x 3 bands, index = out*3+band (dump)",
        SRC_MK5,
    ),
    entry(1023, B, "output_eq_bypass: 36, index = out*3+band", SRC_MK5),
    entry(1024, L, "output_eq_freq: plain integer Hz, index = out*3+band", SRC_MK5),
    entry(1025, F, "output_eq_gain: dB (8.24), index = out*3+band", SRC_MK5),
    entry(1026, F, "output_eq_bw: octaves (8.24), index = out*3+band", SRC_MK5),
    entry(
        1028,
        B,
        "bus_mute: per mix bus (idx 0 = main, 10 = phones); not in motu-mk5-linux",
        SRC_SD,
    ),
    entry(1030, B, "reverb_decay 0-100", SRC_MK5),
    entry(1031, B, "reverb_damping 0-100", SRC_MK5),
    entry(1032, B, "reverb_predelay 0-100 ms", SRC_MK5),
    entry(1033, B, "reverb_width 0-100", SRC_MK5),
    entry(1034, B, "reverb_preset", SRC_MK5),
];

pub fn kind_of(prop_id: u16) -> Option<ValueKind> {
    ENTRIES
        .iter()
        .find(|e| e.prop_id == prop_id)
        .map(|e| e.kind)
}

fn bool_frame(prop_id: u16, index: u16, on: bool) -> Frame {
    Frame {
        prop_id,
        index,
        value: Value::U8(u8::from(on)),
    }
}

fn require_finite(v: f64) -> Result<f64, ProtocolError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(ProtocolError::NonFinite(v))
    }
}

/// Trim in dB (≤ 0, native unit of the state) → attenuation byte
/// (`0` = 0 dB, `100` = -100 dB).
fn db_to_trim(db: f64) -> Result<u8, ProtocolError> {
    let att = (-require_finite(db)?.min(0.0))
        .round()
        .clamp(0.0, f64::from(TRIM_MAX));
    // `att` is an integer in 0..=100: the cast is exact.
    Ok(att as u8)
}

fn trim_to_db(trim: u8) -> f64 {
    -f64::from(trim.min(TRIM_MAX))
}

/// Effective param → frames. Empty = no known hardware mapping (yet).
pub fn encode(param: &Param) -> Result<Vec<Frame>, ProtocolError> {
    let frames = match *param {
        Param::PreampGain { ch, db } if ch < N_INPUTS => {
            let max = if ch < N_MICS {
                GAIN_MAX_DB
            } else {
                LINE_GAIN_MAX_DB
            };
            let gain = require_finite(db)?.clamp(0.0, max).round();
            // Integer in 0..=74: the cast is exact.
            vec![Frame {
                prop_id: P_INPUT_GAIN,
                index: ch,
                value: Value::U8(gain as u8),
            }]
        }
        Param::Phase { ch, on } if ch < N_INPUTS => vec![bool_frame(P_INPUT_PHASE, ch, on)],
        Param::Pad { ch, on } if ch < N_MICS => vec![bool_frame(P_INPUT_PAD, ch, on)],
        Param::Phantom { ch, on } if ch < N_MICS => vec![bool_frame(P_INPUT_48V, ch, on)],
        Param::MainVol { db } => vec![Frame {
            prop_id: P_MAIN_TRIM,
            index: 0,
            value: Value::U8(db_to_trim(db)?),
        }],
        Param::PhonesVol { db } => {
            let trim = db_to_trim(db)?;
            [PHONES_L, PHONES_R]
                .into_iter()
                .map(|index| Frame {
                    prop_id: P_OUTPUT_TRIM,
                    index,
                    value: Value::U8(trim),
                })
                .collect()
        }
        // Same property as the phones trim (5000), analog outputs 0..=9.
        Param::OutputTrim { ch, db } if ch < N_LINE_OUTPUTS => vec![Frame {
            prop_id: P_OUTPUT_TRIM,
            index: ch,
            value: Value::U8(db_to_trim(db)?),
        }],
        Param::Mono { on } => vec![bool_frame(P_MONO, 0, on)],
        Param::Talkback { on } => vec![bool_frame(P_TALKBACK_ENABLE, 0, on)],
        // Unmapped: write not verified (MixVol/MixPan/MixMute/MixSolo, and the
        // DSP / reverb / clock params whose encoders exist but are NOT ACTIVE,
        // see `encode_unverified`), semantics unknown (Route), no hardware
        // counterpart (Dim, SpeakerB), or channel out of the documented range.
        _ => Vec::new(),
    };
    Ok(frames)
}

/// Does `param` have a hardware mapping? Unmapped params never reach the
/// device queue (audit #2).
pub fn is_mapped(param: &Param) -> bool {
    encode(param).is_ok_and(|f| !f.is_empty())
}

// ---------------------------------------------------------------------------
// DSP / reverb / clock: decoded from the device, encoders NOT ACTIVE yet.
// ---------------------------------------------------------------------------

/// `(ch, band)` of a per-band EQ index `ch * bands + band`, if in range.
fn eq_slot(index: u16, channels: u16, bands: u16) -> Option<(u16, u8)> {
    let (ch, band) = (index / bands, index % bands);
    (ch < channels).then_some(())?;
    Some((ch, u8::try_from(band).ok()?))
}

/// Per-band EQ index, if `(ch, band)` is in range.
fn eq_index(ch: u16, band: u8, channels: u16, bands: u16) -> Option<u16> {
    let band = u16::from(band);
    (ch < channels && band < bands).then(|| ch * bands + band)
}

/// Int32 payload carried as `Value::U32` (same 4 bytes, signed reading).
fn int32(v: u32) -> i32 {
    i32::from_be_bytes(v.to_be_bytes())
}

/// Decode an EQ frame (`base` = mode prop of the input or output EQ).
fn decode_eq(frame: &Frame, output: bool) -> Option<Param> {
    use Param as P;
    let (base, channels, bands) = if output {
        (P_OUT_EQ_MODE, N_OUT_EQ, N_OUT_EQ_BANDS)
    } else {
        (P_IN_EQ_MODE, N_INPUTS, N_IN_EQ_BANDS)
    };
    let (ch, band) = eq_slot(frame.index, channels, bands)?;
    let p = match (frame.prop_id - base, &frame.value, output) {
        (0, Value::U8(mode), false) => P::InputEqMode {
            ch,
            band,
            mode: *mode,
        },
        (1, Value::U8(b), false) => P::InputEqBypass {
            ch,
            band,
            on: *b != 0,
        },
        (2, Value::U32(hz), false) => P::InputEqFreq {
            ch,
            band,
            hz: f64::from(int32(*hz)),
        },
        (3, Value::Fixed(db), false) => P::InputEqGain { ch, band, db: *db },
        (4, Value::Fixed(o), false) => P::InputEqBw {
            ch,
            band,
            octaves: *o,
        },
        (0, Value::U8(mode), true) => P::OutputEqMode {
            ch,
            band,
            mode: *mode,
        },
        (1, Value::U8(b), true) => P::OutputEqBypass {
            ch,
            band,
            on: *b != 0,
        },
        (2, Value::U32(hz), true) => P::OutputEqFreq {
            ch,
            band,
            hz: f64::from(int32(*hz)),
        },
        (3, Value::Fixed(db), true) => P::OutputEqGain { ch, band, db: *db },
        (4, Value::Fixed(o), true) => P::OutputEqBw {
            ch,
            band,
            octaves: *o,
        },
        _ => return None,
    };
    Some(p)
}

/// Hardware frame → DSP / reverb / clock param (`None` = not one of them).
fn decode_dsp(frame: &Frame) -> Option<Param> {
    use Param as P;
    let ch = frame.index;
    let gate = ch < N_GATES;
    let comp = ch < N_COMPS;
    let one = ch == 0;
    Some(match (frame.prop_id, &frame.value) {
        (P_IN_EQ_MODE..=P_IN_EQ_BW, _) => return decode_eq(frame, false),
        (P_OUT_EQ_MODE..=P_OUT_EQ_BW, _) => return decode_eq(frame, true),
        (P_GATE_BYPASS, Value::U8(b)) if gate => P::GateBypass { ch, on: *b != 0 },
        (P_GATE_THRESHOLD, Value::Fixed(db)) if gate => P::GateThreshold { ch, db: *db },
        (P_GATE_ATTACK, Value::I16(ms)) if gate => P::GateAttack {
            ch,
            ms: f64::from(*ms),
        },
        (P_GATE_RELEASE, Value::I16(ms)) if gate => P::GateRelease {
            ch,
            ms: f64::from(*ms),
        },
        (P_COMP_BYPASS, Value::U8(b)) if comp => P::CompBypass { ch, on: *b != 0 },
        (P_COMP_THRESHOLD, Value::Fixed(db)) if comp => P::CompThreshold { ch, db: *db },
        (P_COMP_ATTACK, Value::I16(ms)) if comp => P::CompAttack {
            ch,
            ms: f64::from(*ms),
        },
        (P_COMP_RELEASE, Value::I16(ms)) if comp => P::CompRelease {
            ch,
            ms: f64::from(*ms),
        },
        (P_COMP_RATIO, Value::Fixed(r)) if comp => P::CompRatio { ch, ratio: *r },
        (P_COMP_MAKEUP, Value::Fixed(g)) if comp => P::CompMakeup { ch, gain: *g },
        (P_REVERB_DECAY, Value::U8(v)) if one => P::ReverbDecay {
            value: f64::from(*v),
        },
        (P_REVERB_DAMPING, Value::U8(v)) if one => P::ReverbDamping {
            value: f64::from(*v),
        },
        (P_REVERB_PREDELAY, Value::U8(v)) if one => P::ReverbPredelay { ms: f64::from(*v) },
        (P_REVERB_WIDTH, Value::U8(v)) if one => P::ReverbWidth {
            value: f64::from(*v),
        },
        (P_REVERB_PRESET, Value::U8(v)) if one => P::ReverbPreset { preset: *v },
        (P_SAMPLE_RATE, Value::U32(hz)) if one => P::SampleRate {
            rate: SampleRate::from_hz(*hz)?,
        },
        (P_CLOCK_SOURCE, Value::U8(c)) if one => P::ClockSource {
            source: ClockSource::from_code(*c)?,
        },
        _ => return None,
    })
}

/// Clamp into `(min, max)` and round to a whole number (finite input only).
fn whole_in(v: f64, (min, max): (f64, f64)) -> Result<f64, ProtocolError> {
    Ok(require_finite(v)?.round().clamp(min, max))
}

fn frame(prop_id: u16, index: u16, value: Value) -> Vec<Frame> {
    vec![Frame {
        prop_id,
        index,
        value,
    }]
}

/// EQ frame for `field` (0 mode .. 4 bw) of the input or output EQ.
fn encode_eq(output: bool, ch: u16, band: u8, field: u16, value: Value) -> Vec<Frame> {
    let (base, channels, bands) = if output {
        (P_OUT_EQ_MODE, N_OUT_EQ, N_OUT_EQ_BANDS)
    } else {
        (P_IN_EQ_MODE, N_INPUTS, N_IN_EQ_BANDS)
    };
    match eq_index(ch, band, channels, bands) {
        Some(index) => frame(base + field, index, value),
        None => Vec::new(),
    }
}

/// Whole Hz for the Int32 frequency props.
fn freq_value(hz: f64) -> Result<Value, ProtocolError> {
    let hz = whole_in(hz, dsp_limits::FREQ_HZ)?;
    // Integer in 20..=20000: the cast is exact.
    Ok(Value::U32(hz as u32))
}

/// Whole ms for the Int16 time props.
fn ms_value(ms: f64, limits: (f64, f64)) -> Result<Value, ProtocolError> {
    // Integer within the limits (≤ 10000 < i16::MAX): the cast is exact.
    Ok(Value::I16(whole_in(ms, limits)? as i16))
}

/// Whole `0..=100` for the reverb U8 props.
fn percent_value(v: f64, limits: (f64, f64)) -> Result<Value, ProtocolError> {
    // Integer in 0..=100: the cast is exact.
    Ok(Value::U8(whole_in(v, limits)? as u8))
}

fn fixed_value(v: f64, limits: (f64, f64)) -> Result<Value, ProtocolError> {
    Ok(Value::Fixed(require_finite(v)?.clamp(limits.0, limits.1)))
}

/// Frames for the DSP / reverb / clock params.
///
/// **NOT ACTIVE YET.** The layouts were read from a connect dump, but no write
/// has been checked on the hardware, so [`encode`] does not call this: these
/// params stay unmapped ([`is_mapped`] is `false`) and nothing is sent. Each
/// feature is enabled on its own after a test on the card, by routing its
/// variants from [`encode`] to here. Empty for any other param or index.
pub fn encode_unverified(param: &Param) -> Result<Vec<Frame>, ProtocolError> {
    use dsp_limits as L;
    use Param as P;
    let b = |on: bool| Value::U8(u8::from(on));
    let frames = match *param {
        P::InputEqMode { ch, band, mode } => encode_eq(false, ch, band, 0, Value::U8(mode)),
        P::InputEqBypass { ch, band, on } => encode_eq(false, ch, band, 1, b(on)),
        P::InputEqFreq { ch, band, hz } => encode_eq(false, ch, band, 2, freq_value(hz)?),
        P::InputEqGain { ch, band, db } => {
            encode_eq(false, ch, band, 3, fixed_value(db, L::EQ_GAIN_DB)?)
        }
        P::InputEqBw { ch, band, octaves } => {
            encode_eq(false, ch, band, 4, fixed_value(octaves, L::EQ_BW_OCT)?)
        }
        P::OutputEqMode { ch, band, mode } => encode_eq(true, ch, band, 0, Value::U8(mode)),
        P::OutputEqBypass { ch, band, on } => encode_eq(true, ch, band, 1, b(on)),
        P::OutputEqFreq { ch, band, hz } => encode_eq(true, ch, band, 2, freq_value(hz)?),
        P::OutputEqGain { ch, band, db } => {
            encode_eq(true, ch, band, 3, fixed_value(db, L::EQ_GAIN_DB)?)
        }
        P::OutputEqBw { ch, band, octaves } => {
            encode_eq(true, ch, band, 4, fixed_value(octaves, L::EQ_BW_OCT)?)
        }
        P::GateBypass { ch, on } if ch < N_GATES => frame(P_GATE_BYPASS, ch, b(on)),
        P::GateThreshold { ch, db } if ch < N_GATES => {
            frame(P_GATE_THRESHOLD, ch, fixed_value(db, L::THRESHOLD_DB)?)
        }
        P::GateAttack { ch, ms } if ch < N_GATES => {
            frame(P_GATE_ATTACK, ch, ms_value(ms, L::ATTACK_MS)?)
        }
        P::GateRelease { ch, ms } if ch < N_GATES => {
            frame(P_GATE_RELEASE, ch, ms_value(ms, L::RELEASE_MS)?)
        }
        P::CompBypass { ch, on } if ch < N_COMPS => frame(P_COMP_BYPASS, ch, b(on)),
        P::CompThreshold { ch, db } if ch < N_COMPS => {
            frame(P_COMP_THRESHOLD, ch, fixed_value(db, L::THRESHOLD_DB)?)
        }
        P::CompAttack { ch, ms } if ch < N_COMPS => {
            frame(P_COMP_ATTACK, ch, ms_value(ms, L::ATTACK_MS)?)
        }
        P::CompRelease { ch, ms } if ch < N_COMPS => {
            frame(P_COMP_RELEASE, ch, ms_value(ms, L::RELEASE_MS)?)
        }
        P::CompRatio { ch, ratio } if ch < N_COMPS => {
            frame(P_COMP_RATIO, ch, fixed_value(ratio, L::COMP_RATIO)?)
        }
        P::CompMakeup { ch, gain } if ch < N_COMPS => {
            frame(P_COMP_MAKEUP, ch, fixed_value(gain, L::COMP_MAKEUP)?)
        }
        P::ReverbDecay { value } => {
            frame(P_REVERB_DECAY, 0, percent_value(value, L::REVERB_PERCENT)?)
        }
        P::ReverbDamping { value } => frame(
            P_REVERB_DAMPING,
            0,
            percent_value(value, L::REVERB_PERCENT)?,
        ),
        P::ReverbPredelay { ms } => frame(
            P_REVERB_PREDELAY,
            0,
            percent_value(ms, L::REVERB_PREDELAY_MS)?,
        ),
        P::ReverbWidth { value } => {
            frame(P_REVERB_WIDTH, 0, percent_value(value, L::REVERB_PERCENT)?)
        }
        P::ReverbPreset { preset } => frame(P_REVERB_PRESET, 0, Value::U8(preset)),
        P::SampleRate { rate } => frame(P_SAMPLE_RATE, 0, Value::U32(rate.hz())),
        P::ClockSource { source } => frame(P_CLOCK_SOURCE, 0, Value::U8(source.code())),
        _ => Vec::new(),
    };
    Ok(frames)
}

/// Hardware frame → param. `None` = unmapped property.
pub fn decode(frame: &Frame) -> Option<Param> {
    if let Some(p) = decode_dsp(frame) {
        return Some(p);
    }
    let Value::U8(byte) = frame.value else {
        return None;
    };
    let on = byte != 0;
    let ch = frame.index;
    match frame.prop_id {
        P_INPUT_GAIN if ch < N_INPUTS => Some(Param::PreampGain {
            ch,
            db: f64::from(byte),
        }),
        P_INPUT_PHASE if ch < N_INPUTS => Some(Param::Phase { ch, on }),
        P_INPUT_PAD if ch < N_MICS => Some(Param::Pad { ch, on }),
        P_INPUT_48V if ch < N_MICS => Some(Param::Phantom { ch, on }),
        P_MAIN_TRIM if ch == 0 => Some(Param::MainVol {
            db: trim_to_db(byte),
        }),
        // Only the left channel drives PhonesVol: L and R are written together, and
        // reading both would let an unbalanced pair overwrite each other.
        P_OUTPUT_TRIM if ch == PHONES_L => Some(Param::PhonesVol {
            db: trim_to_db(byte),
        }),
        P_OUTPUT_TRIM if ch < N_LINE_OUTPUTS => Some(Param::OutputTrim {
            ch,
            db: trim_to_db(byte),
        }),
        P_MONO if ch == 0 => Some(Param::Mono { on }),
        P_TALKBACK_ENABLE if ch == 0 => Some(Param::Talkback { on }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn mapped_params() -> Vec<Param> {
        let mut v = Vec::new();
        for ch in 0..N_INPUTS {
            let max = if ch < N_MICS {
                GAIN_MAX_DB
            } else {
                LINE_GAIN_MAX_DB
            };
            for db in [0.0, 1.0, 13.0, 20.0, 37.0, 60.0, 74.0] {
                if db <= max {
                    v.push(Param::PreampGain { ch, db });
                }
            }
            for on in [false, true] {
                v.push(Param::Phase { ch, on });
            }
        }
        for ch in 0..N_MICS {
            for on in [false, true] {
                v.push(Param::Pad { ch, on });
                v.push(Param::Phantom { ch, on });
            }
        }
        for on in [false, true] {
            v.push(Param::Mono { on });
            v.push(Param::Talkback { on });
        }
        for ch in 0..N_LINE_OUTPUTS {
            for db in [0.0, -12.0, -100.0] {
                v.push(Param::OutputTrim { ch, db });
            }
        }
        v
    }

    #[test]
    fn mapped_params_round_trip() {
        for p in mapped_params() {
            let frames = encode(&p).unwrap();
            assert_eq!(frames.len(), 1, "{p:?}");
            assert_eq!(decode(&frames[0]), Some(p), "{p:?}");
            // And through the byte codec.
            let bytes = frames[0].encode().unwrap();
            let back = Frame::decode(&bytes, |id| kind_of(id).unwrap_or(ValueKind::U8)).unwrap();
            assert_eq!(decode(&back), Some(p));
        }
    }

    #[test]
    fn trim_volumes_round_trip_on_whole_db() {
        // Native dB in, native byte out: every whole-dB attenuation is exact.
        for att in 0..=100u8 {
            let db = trim_to_db(att);
            assert_eq!(db_to_trim(db), Ok(att), "att {att}");
            let main = encode(&Param::MainVol { db }).unwrap();
            assert_eq!(main.len(), 1);
            assert_eq!(main[0].value, Value::U8(att));
            assert_eq!(decode(&main[0]), Some(Param::MainVol { db }));
            let ph = encode(&Param::PhonesVol { db }).unwrap();
            assert_eq!(ph.len(), 2);
            assert_eq!((ph[0].index, ph[1].index), (PHONES_L, PHONES_R));
            assert_eq!(decode(&ph[0]), Some(Param::PhonesVol { db }));
            assert_eq!(decode(&ph[1]), None, "right channel is write-only");
        }
        assert_eq!(db_to_trim(6.0), Ok(0), "no boost: saturates at 0 dB");
        assert_eq!(db_to_trim(-250.0), Ok(100));
        assert_eq!(db_to_trim(-6.4), Ok(6));
    }

    #[test]
    fn unmapped_params_encode_to_nothing() {
        let none = [
            Param::MixVol {
                bus: 0,
                ch: 0,
                gain: 0.5,
            },
            Param::MixPan {
                bus: 0,
                ch: 0,
                value: 0.0,
            },
            Param::MixMute {
                bus: 0,
                ch: 0,
                on: true,
            },
            Param::MixSolo {
                bus: 0,
                ch: 0,
                on: true,
            },
            Param::Route {
                input: 0,
                output: 0,
                on: true,
            },
            Param::Dim { on: true },
            Param::SpeakerB { on: true },
            // Out of the documented hardware range.
            Param::PreampGain { ch: 8, db: 10.0 },
            Param::Phase { ch: 8, on: true },
            Param::Pad { ch: 2, on: true },
            Param::Phantom { ch: 2, on: true },
        ];
        for p in none {
            assert_eq!(encode(&p), Ok(Vec::new()), "{p:?}");
        }
    }

    #[test]
    fn bad_values_are_errors() {
        assert!(matches!(
            encode(&Param::PreampGain {
                ch: 0,
                db: f64::NAN
            }),
            Err(ProtocolError::NonFinite(_))
        ));
        assert!(matches!(
            encode(&Param::MainVol { db: f64::INFINITY }),
            Err(ProtocolError::NonFinite(_))
        ));
        // Out-of-range finite values are clamped, not rejected.
        let f = encode(&Param::PreampGain { ch: 0, db: 500.0 }).unwrap();
        assert_eq!(f[0].value, Value::U8(74));
        let f = encode(&Param::PreampGain { ch: 0, db: -5.0 }).unwrap();
        assert_eq!(f[0].value, Value::U8(0));
    }

    #[test]
    fn table_is_consistent() {
        let mut seen = HashSet::new();
        for e in ENTRIES {
            assert!(seen.insert(e.prop_id), "duplicate id {}", e.prop_id);
            assert!(!e.verified, "{} must stay unverified", e.prop_id);
            assert!(!e.source.is_empty() && !e.name.is_empty());
            assert_eq!(kind_of(e.prop_id), Some(e.kind));
        }
        // Every id used by encode() is in the table with a U8 payload.
        for p in mapped_params() {
            for f in encode(&p).unwrap() {
                assert_eq!(kind_of(f.prop_id), Some(ValueKind::U8));
            }
        }
        assert_eq!(kind_of(1004), Some(ValueKind::U32));
        assert_eq!(kind_of(1009), Some(ValueKind::I16));
        assert_eq!(kind_of(9999), None);
        // The DSP encoders only emit table ids, with the table's payload kind.
        for p in dsp_params() {
            for f in encode_unverified(&p).unwrap() {
                assert_eq!(kind_of(f.prop_id), Some(f.value.kind()), "{p:?}");
            }
        }
    }

    #[test]
    fn unknown_props_and_wrong_kinds_decode_to_none() {
        for prop_id in [0u16, 1, 9999, 0xFFFF, 0xFFFE, 6000, 1016, 1019, 5019] {
            let f = Frame {
                prop_id,
                index: 0,
                value: Value::U8(1),
            };
            assert_eq!(decode(&f), None, "{prop_id}");
        }
        // Right id, wrong payload kind.
        for value in [Value::Fixed(1.0), Value::U32(1), Value::Str32("x".into())] {
            let f = Frame {
                prop_id: P_INPUT_GAIN,
                index: 0,
                value,
            };
            assert_eq!(decode(&f), None);
        }
    }

    #[test]
    fn no_panic_on_any_index() {
        let ids: Vec<u16> = ENTRIES
            .iter()
            .map(|e| e.prop_id)
            .chain([0, 9999, 0xFFFF])
            .collect();
        for id in ids {
            for index in 0..=u16::MAX {
                let _ = decode(&Frame {
                    prop_id: id,
                    index,
                    value: Value::U8(255),
                });
            }
        }
        for ch in [0u16, 7, 8, 100, u16::MAX] {
            for on in [false, true] {
                let _ = encode(&Param::Phase { ch, on });
                let _ = encode(&Param::Pad { ch, on });
                let _ = encode(&Param::Phantom { ch, on });
                let _ = encode_unverified(&Param::GateBypass { ch, on });
                let _ = encode_unverified(&Param::InputEqBypass {
                    ch,
                    band: u8::MAX,
                    on,
                });
                let _ = encode(&Param::MixMute {
                    bus: u8::MAX,
                    ch,
                    on,
                });
                let _ = encode(&Param::Route {
                    input: ch,
                    output: ch,
                    on,
                });
            }
            let _ = encode(&Param::PreampGain { ch, db: 12.0 });
            let _ = encode(&Param::MixVol {
                bus: u8::MAX,
                ch,
                gain: 1.0,
            });
        }
    }

    /// One sample of every DSP / reverb / clock param, at each index edge,
    /// with values already in the state's normalised form.
    fn dsp_params() -> Vec<Param> {
        let mut v = Vec::new();
        for (ch, band) in [(0u16, 0u8), (3, 2), (7, 3)] {
            v.extend([
                Param::InputEqMode { ch, band, mode: 3 },
                Param::InputEqMode { ch, band, mode: 9 },
                Param::InputEqBypass { ch, band, on: true },
                Param::InputEqFreq {
                    ch,
                    band,
                    hz: 3_500.0,
                },
                Param::InputEqGain {
                    ch,
                    band,
                    db: -12.7,
                },
                Param::InputEqBw {
                    ch,
                    band,
                    octaves: 1.0,
                },
            ]);
        }
        for (ch, band) in [(0u16, 0u8), (5, 1), (11, 2)] {
            v.extend([
                Param::OutputEqMode { ch, band, mode: 0 },
                Param::OutputEqBypass {
                    ch,
                    band,
                    on: false,
                },
                Param::OutputEqFreq { ch, band, hz: 45.0 },
                Param::OutputEqGain { ch, band, db: 2.0 },
                Param::OutputEqBw {
                    ch,
                    band,
                    octaves: 0.5,
                },
            ]);
        }
        for ch in 0..N_GATES {
            v.extend([
                Param::GateBypass { ch, on: true },
                Param::GateThreshold { ch, db: -46.0 },
                Param::GateAttack { ch, ms: 10.0 },
                Param::GateRelease { ch, ms: 1_000.0 },
            ]);
        }
        for ch in [0, 4, N_COMPS - 1] {
            v.extend([
                Param::CompBypass { ch, on: true },
                Param::CompThreshold { ch, db: -10.0 },
                Param::CompAttack { ch, ms: 10.0 },
                Param::CompRelease { ch, ms: 1_000.0 },
                Param::CompRatio { ch, ratio: 2.0 },
                Param::CompMakeup { ch, gain: 9.638 },
            ]);
        }
        v.extend([
            Param::ReverbDecay { value: 86.0 },
            Param::ReverbDamping { value: 90.0 },
            Param::ReverbPredelay { ms: 50.0 },
            Param::ReverbWidth { value: 50.0 },
            Param::ReverbPreset { preset: 1 },
            Param::ClockSource {
                source: ClockSource::Spdif,
            },
            Param::ClockSource {
                source: ClockSource::Optical,
            },
            Param::ClockSource {
                source: ClockSource::Internal,
            },
        ]);
        v.extend(SampleRate::ALL.map(|rate| Param::SampleRate { rate }));
        v
    }

    /// Equal, numbers within 8.24 resolution.
    fn close(a: &Param, b: &Param) -> bool {
        let (ja, jb) = (
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap(),
        );
        let (Some(oa), Some(ob)) = (ja.as_object(), jb.as_object()) else {
            return false;
        };
        oa.len() == ob.len()
            && oa
                .iter()
                .all(|(k, va)| match (va.as_f64(), ob[k].as_f64()) {
                    (Some(x), Some(y)) => (x - y).abs() <= 1.0 / 16_777_216.0,
                    _ => *va == ob[k],
                })
    }

    #[test]
    fn dsp_encoders_round_trip_but_are_not_active() {
        for p in dsp_params() {
            // Not wired: nothing reaches the device.
            assert_eq!(encode(&p), Ok(Vec::new()), "{p:?} must stay inactive");
            assert!(!is_mapped(&p), "{p:?}");
            let frames = encode_unverified(&p).unwrap();
            assert_eq!(frames.len(), 1, "{p:?}");
            let bytes = frames[0].encode().unwrap();
            let back = Frame::decode(&bytes, |id| kind_of(id).unwrap_or(ValueKind::U8)).unwrap();
            let got = decode(&back).unwrap_or_else(|| panic!("no decode for {p:?}"));
            assert!(close(&got, &p), "{p:?} -> {got:?}");
            // The write layout carries the same payload.
            let w = frames[0].encode_write().unwrap();
            let back = Frame::decode_write(&w, |id| kind_of(id).unwrap_or(ValueKind::U8)).unwrap();
            assert_eq!(back.encode_write().unwrap(), w);
        }
    }

    #[test]
    fn dsp_index_layouts_match_the_dump() {
        let f = |prop_id, index, value| Frame {
            prop_id,
            index,
            value,
        };
        // Input EQ index = ch * 4 + band: 13 = ch 3, band 1.
        assert_eq!(
            decode(&f(1004, 13, Value::U32(350))),
            Some(Param::InputEqFreq {
                ch: 3,
                band: 1,
                hz: 350.0
            })
        );
        assert_eq!(decode(&f(1004, 32, Value::U32(350))), None, "9th channel");
        // Output EQ index = out * 3 + band: 35 = out 11, band 2.
        assert_eq!(
            decode(&f(1025, 35, Value::Fixed(2.0))),
            Some(Param::OutputEqGain {
                ch: 11,
                band: 2,
                db: 2.0
            })
        );
        assert_eq!(decode(&f(1025, 36, Value::Fixed(2.0))), None);
        let out = encode_unverified(&Param::OutputEqMode {
            ch: 11,
            band: 2,
            mode: 0,
        })
        .unwrap();
        assert_eq!(out[0].index, 35);
        assert_eq!(
            encode_unverified(&Param::InputEqMode {
                ch: 1,
                band: 4,
                mode: 0
            }),
            Ok(Vec::new()),
            "band out of range"
        );
        // Gate only on the 2 mic inputs, compressor on 8.
        assert_eq!(decode(&f(1007, 2, Value::U8(1))), None);
        assert_eq!(
            decode(&f(1007, 1, Value::U8(1))),
            Some(Param::GateBypass { ch: 1, on: true })
        );
        assert_eq!(decode(&f(1011, 8, Value::U8(1))), None);
        assert_eq!(
            decode(&f(1013, 7, Value::I16(10))),
            Some(Param::CompAttack { ch: 7, ms: 10.0 })
        );
        // Wrong payload kinds are ignored.
        assert_eq!(decode(&f(1013, 0, Value::U8(10))), None);
        assert_eq!(decode(&f(1004, 0, Value::Fixed(1.0))), None);
        // Clock: unknown rates / sources are not guessed.
        assert_eq!(
            decode(&f(10, 0, Value::U32(48_000))),
            Some(Param::SampleRate {
                rate: SampleRate::Hz48000
            })
        );
        assert_eq!(decode(&f(10, 0, Value::U32(32_000))), None);
        assert_eq!(decode(&f(11, 0, Value::U8(1))), None);
        assert_eq!(
            decode(&f(11, 0, Value::U8(3))),
            Some(Param::ClockSource {
                source: ClockSource::Internal
            })
        );
        // Reverb has a single index.
        assert_eq!(decode(&f(1030, 1, Value::U8(86))), None);
        // A negative Int32 frequency decodes as is (the state clamps it).
        assert_eq!(
            decode(&f(1004, 0, Value::U32(u32::MAX))),
            Some(Param::InputEqFreq {
                ch: 0,
                band: 0,
                hz: -1.0
            })
        );
    }

    #[test]
    fn dsp_encoders_clamp_and_reject_non_finite() {
        let one = |p: Param| encode_unverified(&p).unwrap()[0].value.clone();
        assert_eq!(
            one(Param::InputEqFreq {
                ch: 0,
                band: 0,
                hz: 99_999.0
            }),
            Value::U32(20_000)
        );
        assert_eq!(
            one(Param::CompRelease { ch: 0, ms: 1e9 }),
            Value::I16(10_000)
        );
        assert_eq!(one(Param::ReverbWidth { value: -5.0 }), Value::U8(0));
        assert!(matches!(
            encode_unverified(&Param::GateThreshold {
                ch: 0,
                db: f64::NAN
            }),
            Err(ProtocolError::NonFinite(_))
        ));
    }
}
