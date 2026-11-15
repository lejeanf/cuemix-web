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
//! - `Int16`/`Array` properties of the device cannot be carried by
//!   [`ValueKind`] and are therefore absent from [`ENTRIES`].

use super::protocol::{Frame, ProtocolError, Value, ValueKind};
use crate::state::Param;

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

/// Number of analogue/line inputs with a gain and polarity control.
const N_INPUTS: u16 = 8;
/// Number of mic inputs (pad, phantom).
const N_MICS: u16 = 2;
/// Output trim indices of the headphone pair (left, right).
const PHONES_L: u16 = 10;
const PHONES_R: u16 = 11;
/// Mic input gain ceiling in dB (line inputs stop at 20 dB).
const GAIN_MAX_DB: f64 = 74.0;
/// Line input gain ceiling in dB.
const LINE_GAIN_MAX_DB: f64 = 20.0;
/// Output trim attenuation range: `0..=100` = `0..-100` dB.
const TRIM_MAX: u8 = 100;

use ValueKind::{Fixed as F, Str32 as S, U32 as L, U8 as B};

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
        "input_eq_mode: 32 entries (4 bands x 8 ch, order undocumented)",
        SRC_MK5,
    ),
    entry(1003, B, "input_eq_bypass: 32 entries", SRC_MK5),
    entry(
        1004,
        L,
        "input_eq_freq: plain integer Hz (Int32, not 8.24)",
        SRC_MK5,
    ),
    entry(1005, F, "input_eq_gain: dB (8.24)", SRC_MK5),
    entry(1006, F, "input_eq_bw: octaves (8.24)", SRC_MK5),
    entry(1007, B, "gate_bypass: 2 mic channels", SRC_MK5),
    entry(1008, F, "gate_threshold (8.24)", SRC_MK5),
    entry(1011, B, "comp_bypass: 8 channels", SRC_MK5),
    entry(1012, F, "comp_threshold (8.24)", SRC_MK5),
    entry(1015, F, "comp_ratio (8.24)", SRC_MK5),
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
    entry(1021, F, "comp_makeup (8.24)", SRC_MK5),
    entry(1022, B, "output_eq_mode: 36 entries", SRC_MK5),
    entry(1023, B, "output_eq_bypass: 36 entries", SRC_MK5),
    entry(1024, L, "output_eq_freq: plain integer Hz", SRC_MK5),
    entry(1025, F, "output_eq_gain: dB (8.24)", SRC_MK5),
    entry(1026, F, "output_eq_bw: octaves (8.24)", SRC_MK5),
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
        Param::Mono { on } => vec![bool_frame(P_MONO, 0, on)],
        Param::Talkback { on } => vec![bool_frame(P_TALKBACK_ENABLE, 0, on)],
        // Unmapped: index layout undocumented (MixVol/MixPan/MixMute/MixSolo,
        // Route, EqBypass) or no hardware counterpart (Dim, SpeakerB), or
        // channel out of the documented range.
        _ => Vec::new(),
    };
    Ok(frames)
}

/// Does `param` have a hardware mapping? Unmapped params never reach the
/// device queue (audit #2).
pub fn is_mapped(param: &Param) -> bool {
    encode(param).is_ok_and(|f| !f.is_empty())
}

/// Hardware frame → param. `None` = unmapped property.
pub fn decode(frame: &Frame) -> Option<Param> {
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
            Param::EqBypass { ch: 0, on: true },
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
        assert_eq!(kind_of(9999), None);
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
                let _ = encode(&Param::EqBypass { ch, on });
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
}
