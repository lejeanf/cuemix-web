//! Device identification and the mix-matrix layout, both read from the state
//! dump the device pushes on connect.
//!
//! Layout facts (UltraLite-mk5 firmware 2.0.8, dump captured 2026-11-15; see
//! `docs/MOTU-PROTOCOL.md`):
//! - the number of entries of a per-channel property is the channel count:
//!   `1000` mixer inputs, `1001` mono buses, `5001` preamps, `5003` mic
//!   inputs, `6` physical inputs, `7` physical outputs;
//! - `1000` / `1001` flag the left channel of each stereo input / bus;
//! - the mix matrix (`1016` fader, `1017` pan, `1018` solo, `1019` mute) is
//!   indexed `bus * mixer_inputs + input`, mono buses in pairs per mix. A mono
//!   input sits on the left bus of the mix; a stereo pair sits on the left
//!   bus (left input) and the right bus (right input). **Read-only for now**:
//!   the layout matches the dump, but no write has been checked on hardware.

use std::collections::BTreeMap;

use super::protocol::{Frame, Value, HEADER_LEN};
use crate::state::{DeviceInfo, Param, Topology, MIX_GAIN_MAX};

const P_FIRMWARE: u16 = 4;
const P_INPUT_NAMES: u16 = 6;
const P_OUTPUT_NAMES: u16 = 7;
const P_MODEL: u16 = 21;
const P_MIX_INPUT_STEREO: u16 = 1000;
const P_MIX_BUS_STEREO: u16 = 1001;
const P_INPUT_GAIN: u16 = 5001;
const P_INPUT_PAD: u16 = 5003;
pub(crate) const P_MIX_FADER: u16 = 1016;
pub(crate) const P_MIX_PAN: u16 = 1017;
pub(crate) const P_MIX_SOLO: u16 = 1018;
pub(crate) const P_MIX_MUTE: u16 = 1019;

/// Model code of the UltraLite-mk5.
pub const MODEL_ULTRALITE_MK5: &str = "UL5E";

/// Text payload up to its first NUL (lossy: names are display-only).
fn text(payload: &[u8]) -> String {
    let end = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    String::from_utf8_lossy(&payload[..end]).trim().to_owned()
}

/// Mixes of the UltraLite-mk5 in device order (bus pairs 0-1 … 12-13).
const MK5_MIXES: [&str; 7] = [
    "Main 1-2",
    "Line 3-4",
    "Line 5-6",
    "Line 7-8",
    "Line 9-10",
    "Phones",
    "Reverb",
];
/// CueMix 5 lists Phones right after Main.
const MK5_MIX_ORDER: [u8; 7] = [0, 5, 1, 2, 3, 4, 6];

/// Default names of the UltraLite-mk5 mixer inputs, in device order (as CueMix
/// 5 shows them): 8 analog, S/PDIF 1-2, ADAT 1-8, then the computer (USB)
/// return of each mix but Reverb, then the reverb return.
/// Returns (mono names, stereo-pair names on the left channel).
fn mk5_input_names(mixer_inputs: usize) -> (Vec<String>, Vec<String>) {
    let mut mono: Vec<String> = vec!["Mic 1".into(), "Mic 2".into()];
    mono.extend((3..=8).map(|n| format!("Line {n}")));
    mono.extend(["S/PDIF 1".into(), "S/PDIF 2".into()]);
    mono.extend((1..=8).map(|n| format!("ADAT {n}")));
    let mut stereo = vec![String::new(); mono.len()];
    stereo[8] = "S/PDIF 1-2".into();
    for n in (1..=8).step_by(2) {
        stereo[9 + n] = format!("ADAT {n}-{}", n + 1);
    }
    for mix in &MK5_MIXES[..6] {
        mono.extend([format!("USB {mix} L"), format!("USB {mix} R")]);
        stereo.extend([format!("USB {mix}"), String::new()]);
    }
    mono.extend(["Reverb L".into(), "Reverb R".into()]);
    stereo.extend(["Reverb".into(), String::new()]);
    mono.resize(mixer_inputs, String::new());
    stereo.resize(mixer_inputs, String::new());
    (mono, stereo)
}

/// Identify the device from the raw frames of its state dump. `None` when a
/// count needed for the topology is missing (the defaults then stay).
pub fn identify(frames: &[Vec<u8>]) -> Option<DeviceInfo> {
    // prop_id -> index -> payload (last one wins).
    let mut props: BTreeMap<u16, BTreeMap<u16, &[u8]>> = BTreeMap::new();
    for bytes in frames {
        let Some((head, payload)) = bytes.split_first_chunk::<HEADER_LEN>() else {
            continue;
        };
        let prop_id = u16::from_be_bytes([head[0], head[1]]);
        let index = u16::from_be_bytes([head[2], head[3]]);
        props.entry(prop_id).or_default().insert(index, payload);
    }
    let count = |p: u16| -> Option<u16> {
        let n = props.get(&p)?.len();
        u16::try_from(n).ok().filter(|&n| n > 0)
    };
    let flags = |p: u16| -> Vec<bool> {
        props
            .get(&p)
            .map(|m| {
                m.values()
                    .map(|v| v.first().is_some_and(|&b| b != 0))
                    .collect()
            })
            .unwrap_or_default()
    };
    let one_text = |p: u16| -> String {
        props
            .get(&p)
            .and_then(|m| m.get(&0))
            .map(|v| text(v))
            .unwrap_or_default()
    };

    let mixer_inputs = count(P_MIX_INPUT_STEREO)?;
    let buses = count(P_MIX_BUS_STEREO)?;
    let topology = Topology {
        mixes: u8::try_from(buses / 2).ok().filter(|&m| m > 0)?,
        mixer_inputs,
        preamps: count(P_INPUT_GAIN)?,
        mic_inputs: count(P_INPUT_PAD)?,
        routing_inputs: count(P_INPUT_NAMES)?,
        routing_outputs: count(P_OUTPUT_NAMES)?,
    };
    let model = one_text(P_MODEL);
    let inputs = usize::from(topology.mixer_inputs);
    let mixes = usize::from(topology.mixes);
    let mk5 = model == MODEL_ULTRALITE_MK5 && mixes == MK5_MIXES.len();
    let (mut input_names, stereo_names) = if mk5 {
        mk5_input_names(inputs)
    } else {
        (vec![String::new(); inputs], vec![String::new(); inputs])
    };
    let (mix_names, mix_order) = if mk5 {
        (
            MK5_MIXES.iter().map(|m| (*m).to_owned()).collect(),
            MK5_MIX_ORDER.to_vec(),
        )
    } else {
        (vec![String::new(); mixes], Vec::new())
    };
    // Names set on the device (physical inputs) win over the defaults.
    if let Some(names) = props.get(&P_INPUT_NAMES) {
        for (&i, payload) in names {
            let name = text(payload);
            if let Some(slot) = input_names.get_mut(usize::from(i)) {
                if !name.is_empty() {
                    *slot = name;
                }
            }
        }
    }
    Some(DeviceInfo {
        model,
        firmware: one_text(P_FIRMWARE),
        topology,
        input_names,
        stereo_names,
        mix_names,
        mix_order,
        stereo_inputs: flags(P_MIX_INPUT_STEREO),
        stereo_buses: flags(P_MIX_BUS_STEREO),
    })
}

/// Is `input` the right channel of a stereo pair?
fn is_right_of_pair(stereo_inputs: &[bool], input: usize) -> bool {
    input % 2 == 1 && stereo_inputs.get(input - 1).copied().unwrap_or(false)
}

/// Mix-matrix frame (`1016`..=`1019`) → param, given the device layout.
/// `None` for frames that do not drive a strip (e.g. a mono input on the
/// right bus) or that fall outside the layout.
pub fn decode_mix(frame: &Frame, device: &DeviceInfo) -> Option<Param> {
    let inputs = usize::from(device.topology.mixer_inputs);
    if inputs == 0 {
        return None;
    }
    let idx = usize::from(frame.index);
    let (bus, input) = (idx / inputs, idx % inputs);
    let left_bus = bus - bus % 2;
    if !device.stereo_buses.get(left_bus).copied().unwrap_or(false) {
        return None;
    }
    // Each strip is read from one bus: the right bus for the right channel of
    // a stereo pair, the left bus otherwise.
    if (bus % 2 == 1) != is_right_of_pair(&device.stereo_inputs, input) {
        return None;
    }
    let mix = u8::try_from(bus / 2).ok()?;
    if mix >= device.topology.mixes {
        return None;
    }
    let ch = u16::try_from(input).ok()?;
    match (frame.prop_id, &frame.value) {
        (P_MIX_FADER, Value::Fixed(g)) if g.is_finite() => Some(Param::MixVol {
            bus: mix,
            ch,
            gain: g.clamp(0.0, MIX_GAIN_MAX),
        }),
        // Device pan is 0..1 with 0.5 = centre (all strips at 0.5 in the dump).
        (P_MIX_PAN, Value::Fixed(v)) if v.is_finite() => Some(Param::MixPan {
            bus: mix,
            ch,
            value: (v * 2.0 - 1.0).clamp(-1.0, 1.0),
        }),
        (P_MIX_SOLO, Value::U8(b)) => Some(Param::MixSolo {
            bus: mix,
            ch,
            on: *b != 0,
        }),
        (P_MIX_MUTE, Value::U8(b)) => Some(Param::MixMute {
            bus: mix,
            ch,
            on: *b != 0,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(prop_id: u16, index: u16, payload: &[u8]) -> Vec<u8> {
        let mut v = prop_id.to_be_bytes().to_vec();
        v.extend_from_slice(&index.to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// A dump shaped like the UltraLite-mk5 one (counts and stereo flags).
    fn mk5_dump() -> Vec<Vec<u8>> {
        let mut d = vec![
            frame(P_MODEL, 0, b"UL5E\0"),
            frame(P_FIRMWARE, 0, b"2.0.8+2568\0\0\0\0"),
        ];
        for i in 0..32u16 {
            let stereo = [8, 10, 18, 20, 22, 24, 26, 28, 30].contains(&i);
            d.push(frame(P_MIX_INPUT_STEREO, i, &[u8::from(stereo)]));
        }
        for i in 0..14u16 {
            d.push(frame(P_MIX_BUS_STEREO, i, &[u8::from(i % 2 == 0)]));
        }
        for i in 0..8u16 {
            d.push(frame(P_INPUT_GAIN, i, &[0]));
        }
        for i in 0..2u16 {
            d.push(frame(P_INPUT_PAD, i, &[0]));
        }
        for i in 0..18u16 {
            d.push(frame(P_INPUT_NAMES, i, &[0; 32]));
        }
        for i in 0..22u16 {
            d.push(frame(P_OUTPUT_NAMES, i, &[0; 32]));
        }
        d
    }

    #[test]
    fn identifies_the_mk5_topology_and_names() {
        let info = identify(&mk5_dump()).expect("identified");
        assert_eq!(info.model, "UL5E");
        assert_eq!(info.firmware, "2.0.8+2568");
        assert_eq!(info.topology, Topology::ULTRALITE_MK5);
        assert_eq!(info.input_names.len(), 32);
        assert_eq!(info.input_names[0], "Mic 1");
        assert_eq!(info.input_names[2], "Line 3");
        assert_eq!(info.input_names[8], "S/PDIF 1");
        assert_eq!(info.input_names[10], "ADAT 1");
        assert_eq!(info.input_names[17], "ADAT 8");
        assert_eq!(info.input_names[18], "USB Main 1-2 L");
        assert_eq!(info.input_names[29], "USB Phones R");
        assert_eq!(info.input_names[30], "Reverb L");
        assert_eq!(info.stereo_names[8], "S/PDIF 1-2");
        assert_eq!(info.stereo_names[10], "ADAT 1-2");
        assert_eq!(info.stereo_names[16], "ADAT 7-8");
        assert_eq!(info.stereo_names[18], "USB Main 1-2");
        assert_eq!(info.stereo_names[28], "USB Phones");
        assert_eq!(info.stereo_names[30], "Reverb");
        assert_eq!(info.mix_order, vec![0, 5, 1, 2, 3, 4, 6]);
        assert_eq!(info.mix_names[0], "Main 1-2");
        assert_eq!(info.mix_names[5], "Phones");
        assert_eq!(info.mix_names[6], "Reverb");
        assert!(info.stereo_inputs[18] && !info.stereo_inputs[19]);
    }

    #[test]
    fn device_names_win_over_defaults() {
        let mut d = mk5_dump();
        let mut name = b"Synth".to_vec();
        name.resize(32, 0);
        d.push(frame(P_INPUT_NAMES, 2, &name));
        let info = identify(&d).expect("identified");
        assert_eq!(info.input_names[2], "Synth");
    }

    #[test]
    fn missing_counts_give_no_identity() {
        assert_eq!(identify(&[frame(P_MODEL, 0, b"UL5E\0")]), None);
        assert_eq!(identify(&[]), None);
    }

    #[test]
    fn unknown_model_counts_only() {
        let mut d = mk5_dump();
        d[0] = frame(P_MODEL, 0, b"XXXX\0");
        let info = identify(&d).expect("identified");
        assert_eq!(info.topology, Topology::ULTRALITE_MK5);
        assert!(info.input_names.iter().all(String::is_empty));
        assert!(info.mix_order.is_empty());
    }

    fn fixed(prop_id: u16, index: u16, v: f64) -> Frame {
        Frame {
            prop_id,
            index,
            value: Value::Fixed(v),
        }
    }

    #[test]
    fn mix_layout_is_bus_major_with_stereo_pairs_on_both_buses() {
        let info = identify(&mk5_dump()).expect("identified");
        // Mono strip Line 3 (input 2) on Main L (bus 0).
        assert_eq!(
            decode_mix(&fixed(P_MIX_FADER, 2, 1.0), &info),
            Some(Param::MixVol {
                bus: 0,
                ch: 2,
                gain: 1.0
            })
        );
        // Mono strip on the right bus: not a strip value.
        assert_eq!(decode_mix(&fixed(P_MIX_FADER, 32 + 2, 1.0), &info), None);
        // USB 1-2 pair (inputs 18/19): left on bus 0, right on bus 1.
        assert_eq!(
            decode_mix(&fixed(P_MIX_FADER, 18, 0.5), &info),
            Some(Param::MixVol {
                bus: 0,
                ch: 18,
                gain: 0.5
            })
        );
        assert_eq!(decode_mix(&fixed(P_MIX_FADER, 19, 0.5), &info), None);
        assert_eq!(
            decode_mix(&fixed(P_MIX_FADER, 32 + 19, 1.0), &info),
            Some(Param::MixVol {
                bus: 0,
                ch: 19,
                gain: 1.0
            })
        );
        // Phones = buses 10/11 = mix 5 (USB 11-12 = inputs 28/29 in the dump).
        assert_eq!(
            decode_mix(&fixed(P_MIX_FADER, 10 * 32 + 28, 1.0), &info),
            Some(Param::MixVol {
                bus: 5,
                ch: 28,
                gain: 1.0
            })
        );
        // Out of the matrix.
        assert_eq!(decode_mix(&fixed(P_MIX_FADER, 14 * 32, 1.0), &info), None);
    }

    #[test]
    fn mix_pan_mute_solo_and_clamping() {
        let info = identify(&mk5_dump()).expect("identified");
        assert_eq!(
            decode_mix(&fixed(P_MIX_PAN, 3, 0.5), &info),
            Some(Param::MixPan {
                bus: 0,
                ch: 3,
                value: 0.0
            })
        );
        assert_eq!(
            decode_mix(&fixed(P_MIX_FADER, 3, 9.0), &info),
            Some(Param::MixVol {
                bus: 0,
                ch: 3,
                gain: MIX_GAIN_MAX
            })
        );
        let b = |p, on: u8| Frame {
            prop_id: p,
            index: 4,
            value: Value::U8(on),
        };
        assert_eq!(
            decode_mix(&b(P_MIX_MUTE, 1), &info),
            Some(Param::MixMute {
                bus: 0,
                ch: 4,
                on: true
            })
        );
        assert_eq!(
            decode_mix(&b(P_MIX_SOLO, 0), &info),
            Some(Param::MixSolo {
                bus: 0,
                ch: 4,
                on: false
            })
        );
    }
}
