//! OSC address table (HANDOFF §3.1) ⇄ [`Command`] / [`Param`].
//!
//! OSC indices are **1-based** (`/mix/1/ch/1/vol` = first mix, first strip);
//! they are converted to the 0-based indices of [`crate::state`] here and
//! nowhere else. Boolean controls accept `int`, `float` (≥ 0.5 = on) and the
//! OSC `T`/`F` types; feedback is always sent as `int` 0/1. Continuous
//! controls accept `float`, `double` or `int` (NaN/inf rejected); feedback is `float`.
//!
//! Volumes: the engine works in MOTU native units (mix fader = linear gain,
//! main/phones = whole dB). OSC keeps the HANDOFF `0..1` fader position and
//! converts here with [`fader`]: mix faders `0.75` = 0 dB / `1.0` = +12 dB;
//! trims (main, phones) `1.0` = 0 dB, same taper, floor `TRIM_MIN_DB`.

use rosc::{OscMessage, OscType};
use thiserror::Error;

use crate::motu::fader;
use crate::power::{ChannelPower, PowerChannel};
use crate::state::{ClockSource, Command, OpticalMode, Param, SampleRate, TRIM_MIN_DB};

#[derive(Debug, Error, PartialEq)]
pub enum OscError {
    #[error("unknown OSC address {0}")]
    UnknownAddress(String),
    #[error("bad index {0:?} in {1} (expected an integer ≥ 1)")]
    BadIndex(String, String),
    #[error("{0} expects exactly one argument")]
    Arity(String),
    #[error("argument type not accepted for {0}")]
    ArgType(String),
}

/// Longest address pattern looked at (the longest real one is under 40 bytes).
const MAX_ADDRESS_LEN: usize = 128;
/// Most arguments a message may carry (every address takes one).
const MAX_ARGS: usize = 4;

fn index_u16(seg: &str, addr: &str) -> Result<u16, OscError> {
    match seg.parse::<u16>() {
        Ok(n) if n >= 1 && seg.bytes().all(|b| b.is_ascii_digit()) => Ok(n - 1),
        _ => Err(OscError::BadIndex(seg.to_owned(), addr.to_owned())),
    }
}

fn index_u8(seg: &str, addr: &str) -> Result<u8, OscError> {
    let i = index_u16(seg, addr)?;
    u8::try_from(i).map_err(|_| OscError::BadIndex(seg.to_owned(), addr.to_owned()))
}

fn single<'a>(addr: &str, args: &'a [OscType]) -> Result<&'a OscType, OscError> {
    match args {
        [a] => Ok(a),
        _ => Err(OscError::Arity(addr.to_owned())),
    }
}

fn arg_f64(addr: &str, args: &[OscType]) -> Result<f64, OscError> {
    match single(addr, args)? {
        OscType::Float(f) if f.is_finite() => Ok(f64::from(*f)),
        OscType::Double(d) if d.is_finite() => Ok(*d),
        OscType::Int(i) => Ok(f64::from(*i)),
        _ => Err(OscError::ArgType(addr.to_owned())),
    }
}

fn arg_bool(addr: &str, args: &[OscType]) -> Result<bool, OscError> {
    match single(addr, args)? {
        OscType::Int(i) => Ok(*i != 0),
        OscType::Float(f) => Ok(*f >= 0.5),
        OscType::Double(d) => Ok(*d >= 0.5),
        OscType::Bool(b) => Ok(*b),
        _ => Err(OscError::ArgType(addr.to_owned())),
    }
}

/// Whole number `0..=255` (int, or a float that rounds into the range).
fn arg_u8(addr: &str, args: &[OscType]) -> Result<u8, OscError> {
    let v = arg_f64(addr, args)?.round();
    if (0.0..=255.0).contains(&v) {
        // Integer in 0..=255: the cast is exact.
        Ok(v as u8)
    } else {
        Err(OscError::ArgType(addr.to_owned()))
    }
}

/// OSC name of a clock source (same as the JSON one).
fn clock_source_name(s: ClockSource) -> &'static str {
    match s {
        ClockSource::Internal => "internal",
        ClockSource::Spdif => "spdif",
        ClockSource::Optical => "optical",
    }
}

fn clock_source(name: &str) -> Option<ClockSource> {
    [
        ClockSource::Internal,
        ClockSource::Spdif,
        ClockSource::Optical,
    ]
    .into_iter()
    .find(|s| clock_source_name(*s).eq_ignore_ascii_case(name))
}

/// `/eq/{ch}/band/{b}/{leaf}` and `/output/{n}/eq/band/{b}/{leaf}`.
fn eq_param(
    output: bool,
    ch: u16,
    band: u8,
    leaf: &str,
    addr: &str,
    args: &[OscType],
) -> Result<Param, OscError> {
    use Param as P;
    Ok(match (leaf, output) {
        ("mode", false) => P::InputEqMode {
            ch,
            band,
            mode: arg_u8(addr, args)?,
        },
        ("bypass", false) => P::InputEqBypass {
            ch,
            band,
            on: arg_bool(addr, args)?,
        },
        ("freq", false) => P::InputEqFreq {
            ch,
            band,
            hz: arg_f64(addr, args)?,
        },
        ("gain", false) => P::InputEqGain {
            ch,
            band,
            db: arg_f64(addr, args)?,
        },
        ("bw", false) => P::InputEqBw {
            ch,
            band,
            octaves: arg_f64(addr, args)?,
        },
        ("mode", true) => P::OutputEqMode {
            ch,
            band,
            mode: arg_u8(addr, args)?,
        },
        ("bypass", true) => P::OutputEqBypass {
            ch,
            band,
            on: arg_bool(addr, args)?,
        },
        ("freq", true) => P::OutputEqFreq {
            ch,
            band,
            hz: arg_f64(addr, args)?,
        },
        ("gain", true) => P::OutputEqGain {
            ch,
            band,
            db: arg_f64(addr, args)?,
        },
        ("bw", true) => P::OutputEqBw {
            ch,
            band,
            octaves: arg_f64(addr, args)?,
        },
        _ => return Err(OscError::UnknownAddress(addr.to_owned())),
    })
}

/// Address of the explicit "send me the full state" request (no arguments).
pub const SYNC_ADDRESS: &str = "/sync";

/// `true` for a `/sync` message without arguments.
pub fn is_sync(msg: &OscMessage) -> bool {
    msg.addr == SYNC_ADDRESS && msg.args.is_empty()
}

/// Parse one incoming OSC message into a [`Command`].
pub fn parse(msg: &OscMessage) -> Result<Command, OscError> {
    let addr = msg.addr.as_str();
    let args = msg.args.as_slice();
    if addr.len() > MAX_ADDRESS_LEN || args.len() > MAX_ARGS {
        return Err(OscError::UnknownAddress(addr.chars().take(32).collect()));
    }
    let segs: Vec<&str> = addr.strip_prefix('/').unwrap_or(addr).split('/').collect();
    let unknown = || OscError::UnknownAddress(addr.to_owned());

    let param = match segs.as_slice() {
        ["mix", "main", "vol"] => Param::MainVol {
            db: fader::trim_pos_to_db(arg_f64(addr, args)?, TRIM_MIN_DB),
        },
        ["mix", "main", "power"] => {
            return Ok(Command::Power {
                channel: PowerChannel::Main,
                on: arg_bool(addr, args)?,
            })
        }
        ["controlroom", "phones", "power"] => {
            return Ok(Command::Power {
                channel: PowerChannel::Phones,
                on: arg_bool(addr, args)?,
            })
        }
        ["mix", bus, "ch", ch, leaf] => {
            let (bus, ch) = (index_u8(bus, addr)?, index_u16(ch, addr)?);
            match *leaf {
                "vol" => Param::MixVol {
                    bus,
                    ch,
                    gain: fader::pos_to_gain(arg_f64(addr, args)?),
                },
                "pan" => Param::MixPan {
                    bus,
                    ch,
                    value: arg_f64(addr, args)?,
                },
                "mute" => Param::MixMute {
                    bus,
                    ch,
                    on: arg_bool(addr, args)?,
                },
                "solo" => Param::MixSolo {
                    bus,
                    ch,
                    on: arg_bool(addr, args)?,
                },
                _ => return Err(unknown()),
            }
        }
        ["preamp", ch, leaf] => {
            let ch = index_u16(ch, addr)?;
            match *leaf {
                "gain" => Param::PreampGain {
                    ch,
                    db: arg_f64(addr, args)?,
                },
                "48v" => Param::Phantom {
                    ch,
                    on: arg_bool(addr, args)?,
                },
                "pad" => Param::Pad {
                    ch,
                    on: arg_bool(addr, args)?,
                },
                "phase" => Param::Phase {
                    ch,
                    on: arg_bool(addr, args)?,
                },
                _ => return Err(unknown()),
            }
        }
        ["eq", ch, "band", band, leaf] => eq_param(
            false,
            index_u16(ch, addr)?,
            index_u8(band, addr)?,
            leaf,
            addr,
            args,
        )?,
        ["output", ch, "eq", "band", band, leaf] => eq_param(
            true,
            index_u16(ch, addr)?,
            index_u8(band, addr)?,
            leaf,
            addr,
            args,
        )?,
        ["gate", ch, leaf] => {
            let ch = index_u16(ch, addr)?;
            match *leaf {
                "bypass" => Param::GateBypass {
                    ch,
                    on: arg_bool(addr, args)?,
                },
                "threshold" => Param::GateThreshold {
                    ch,
                    db: arg_f64(addr, args)?,
                },
                "attack" => Param::GateAttack {
                    ch,
                    ms: arg_f64(addr, args)?,
                },
                "release" => Param::GateRelease {
                    ch,
                    ms: arg_f64(addr, args)?,
                },
                _ => return Err(unknown()),
            }
        }
        ["comp", ch, leaf] => {
            let ch = index_u16(ch, addr)?;
            match *leaf {
                "bypass" => Param::CompBypass {
                    ch,
                    on: arg_bool(addr, args)?,
                },
                "threshold" => Param::CompThreshold {
                    ch,
                    db: arg_f64(addr, args)?,
                },
                "attack" => Param::CompAttack {
                    ch,
                    ms: arg_f64(addr, args)?,
                },
                "release" => Param::CompRelease {
                    ch,
                    ms: arg_f64(addr, args)?,
                },
                "ratio" => Param::CompRatio {
                    ch,
                    ratio: arg_f64(addr, args)?,
                },
                "makeup" => Param::CompMakeup {
                    ch,
                    gain: arg_f64(addr, args)?,
                },
                _ => return Err(unknown()),
            }
        }
        ["reverb", "decay"] => Param::ReverbDecay {
            value: arg_f64(addr, args)?,
        },
        ["reverb", "damping"] => Param::ReverbDamping {
            value: arg_f64(addr, args)?,
        },
        ["reverb", "predelay"] => Param::ReverbPredelay {
            ms: arg_f64(addr, args)?,
        },
        ["reverb", "width"] => Param::ReverbWidth {
            value: arg_f64(addr, args)?,
        },
        ["reverb", "preset"] => Param::ReverbPreset {
            preset: arg_u8(addr, args)?,
        },
        ["clock", "rate"] => {
            let hz = arg_f64(addr, args)?;
            let rate = SampleRate::ALL
                .into_iter()
                .find(|r| f64::from(r.hz()) == hz)
                .ok_or_else(|| OscError::ArgType(addr.to_owned()))?;
            Param::SampleRate { rate }
        }
        ["clock", "source"] => match single(addr, args)? {
            OscType::String(s) => Param::ClockSource {
                source: clock_source(s).ok_or_else(|| OscError::ArgType(addr.to_owned()))?,
            },
            _ => return Err(OscError::ArgType(addr.to_owned())),
        },
        ["optical", port @ ("in" | "out"), "mode"] => match single(addr, args)? {
            OscType::String(s) => Param::OpticalMode {
                output: *port == "out",
                mode: match s.as_str() {
                    "adat" => OpticalMode::Adat,
                    "toslink" | "tos" => OpticalMode::Toslink,
                    _ => return Err(OscError::ArgType(addr.to_owned())),
                },
            },
            _ => return Err(OscError::ArgType(addr.to_owned())),
        },
        ["routing", "in", input, "out", output] => Param::Route {
            input: index_u16(input, addr)?,
            output: index_u16(output, addr)?,
            on: arg_bool(addr, args)?,
        },
        ["controlroom", "dim"] => Param::Dim {
            on: arg_bool(addr, args)?,
        },
        ["controlroom", "mono"] => Param::Mono {
            on: arg_bool(addr, args)?,
        },
        ["controlroom", "ab"] => Param::SpeakerB {
            on: arg_bool(addr, args)?,
        },
        ["controlroom", "talkback"] => Param::Talkback {
            on: arg_bool(addr, args)?,
        },
        ["controlroom", "phones", "vol"] => Param::PhonesVol {
            db: fader::trim_pos_to_db(arg_f64(addr, args)?, TRIM_MIN_DB),
        },
        ["output", ch, "trim"] => Param::OutputTrim {
            ch: index_u16(ch, addr)?,
            db: fader::trim_pos_to_db(arg_f64(addr, args)?, TRIM_MIN_DB),
        },
        ["preset", "load"] => match single(addr, args)? {
            OscType::String(name) => {
                let command = Command::LoadPreset { name: name.clone() };
                // Names that cannot be a preset never reach the engine.
                command
                    .validate()
                    .map_err(|_| OscError::ArgType(addr.to_owned()))?;
                return Ok(command);
            }
            _ => return Err(OscError::ArgType(addr.to_owned())),
        },
        _ => return Err(unknown()),
    };
    Ok(Command::Set(param))
}

/// OSC address of a channel's on/off.
pub fn power_address(channel: PowerChannel) -> &'static str {
    match channel {
        PowerChannel::Main => "/mix/main/power",
        PowerChannel::Phones => "/controlroom/phones/power",
    }
}

/// Feedback of the on/off state: `int` 0/1, where the channel is heading (so a toggle flips
/// when a fade starts, not when it ends).
pub fn power_feedback(main: &ChannelPower, phones: &ChannelPower) -> Vec<OscMessage> {
    [(PowerChannel::Main, main), (PowerChannel::Phones, phones)]
        .into_iter()
        .map(|(channel, p)| OscMessage {
            addr: power_address(channel).to_owned(),
            args: vec![OscType::Int(i32::from(p.intent()))],
        })
        .collect()
}

/// Encode an effective [`Param`] as OSC feedback (1-based addresses).
/// Every [`Param`] has an address today; `None` is kept for future ones.
pub fn feedback(param: &Param) -> Option<OscMessage> {
    use Param as P;
    let n = |i: u16| u32::from(i) + 1;
    let nb = |i: u8| u32::from(i) + 1;
    let b = |on: bool| OscType::Int(i32::from(on));
    let i = |v: u8| OscType::Int(i32::from(v));
    // OSC floats are 32-bit; precision loss is expected and harmless here.
    let f = |v: f64| OscType::Float(v as f32);
    let ieq = |ch: u16, band: u8, leaf: &str| format!("/eq/{}/band/{}/{leaf}", n(ch), nb(band));
    let oeq =
        |ch: u16, band: u8, leaf: &str| format!("/output/{}/eq/band/{}/{leaf}", n(ch), nb(band));
    let (addr, arg) = match *param {
        P::InputEqMode { ch, band, mode } => (ieq(ch, band, "mode"), i(mode)),
        P::InputEqBypass { ch, band, on } => (ieq(ch, band, "bypass"), b(on)),
        P::InputEqFreq { ch, band, hz } => (ieq(ch, band, "freq"), f(hz)),
        P::InputEqGain { ch, band, db } => (ieq(ch, band, "gain"), f(db)),
        P::InputEqBw { ch, band, octaves } => (ieq(ch, band, "bw"), f(octaves)),
        P::OutputEqMode { ch, band, mode } => (oeq(ch, band, "mode"), i(mode)),
        P::OutputEqBypass { ch, band, on } => (oeq(ch, band, "bypass"), b(on)),
        P::OutputEqFreq { ch, band, hz } => (oeq(ch, band, "freq"), f(hz)),
        P::OutputEqGain { ch, band, db } => (oeq(ch, band, "gain"), f(db)),
        P::OutputEqBw { ch, band, octaves } => (oeq(ch, band, "bw"), f(octaves)),
        P::GateBypass { ch, on } => (format!("/gate/{}/bypass", n(ch)), b(on)),
        P::GateThreshold { ch, db } => (format!("/gate/{}/threshold", n(ch)), f(db)),
        P::GateAttack { ch, ms } => (format!("/gate/{}/attack", n(ch)), f(ms)),
        P::GateRelease { ch, ms } => (format!("/gate/{}/release", n(ch)), f(ms)),
        P::CompBypass { ch, on } => (format!("/comp/{}/bypass", n(ch)), b(on)),
        P::CompThreshold { ch, db } => (format!("/comp/{}/threshold", n(ch)), f(db)),
        P::CompAttack { ch, ms } => (format!("/comp/{}/attack", n(ch)), f(ms)),
        P::CompRelease { ch, ms } => (format!("/comp/{}/release", n(ch)), f(ms)),
        P::CompRatio { ch, ratio } => (format!("/comp/{}/ratio", n(ch)), f(ratio)),
        P::CompMakeup { ch, gain } => (format!("/comp/{}/makeup", n(ch)), f(gain)),
        P::ReverbDecay { value } => ("/reverb/decay".to_owned(), f(value)),
        P::ReverbDamping { value } => ("/reverb/damping".to_owned(), f(value)),
        P::ReverbPredelay { ms } => ("/reverb/predelay".to_owned(), f(ms)),
        P::ReverbWidth { value } => ("/reverb/width".to_owned(), f(value)),
        P::ReverbPreset { preset } => ("/reverb/preset".to_owned(), i(preset)),
        P::SampleRate { rate } => (
            "/clock/rate".to_owned(),
            OscType::Int(i32::try_from(rate.hz()).unwrap_or(i32::MAX)),
        ),
        P::ClockSource { source } => (
            "/clock/source".to_owned(),
            OscType::String(clock_source_name(source).to_owned()),
        ),
        P::OpticalMode { output, mode } => (
            format!("/optical/{}/mode", if output { "out" } else { "in" }),
            OscType::String(
                match mode {
                    OpticalMode::Adat => "adat",
                    OpticalMode::Toslink => "toslink",
                }
                .to_owned(),
            ),
        ),
        Param::MixVol { bus, ch, gain } => (
            format!("/mix/{}/ch/{}/vol", n(bus.into()), n(ch)),
            f(fader::gain_to_pos(gain)),
        ),
        Param::MixPan { bus, ch, value } => {
            (format!("/mix/{}/ch/{}/pan", n(bus.into()), n(ch)), f(value))
        }
        Param::MixMute { bus, ch, on } => {
            (format!("/mix/{}/ch/{}/mute", n(bus.into()), n(ch)), b(on))
        }
        Param::MixSolo { bus, ch, on } => {
            (format!("/mix/{}/ch/{}/solo", n(bus.into()), n(ch)), b(on))
        }
        Param::MainVol { db } => ("/mix/main/vol".to_owned(), f(fader::trim_db_to_pos(db))),
        Param::PreampGain { ch, db } => (format!("/preamp/{}/gain", n(ch)), f(db)),
        Param::Phantom { ch, on } => (format!("/preamp/{}/48v", n(ch)), b(on)),
        Param::Pad { ch, on } => (format!("/preamp/{}/pad", n(ch)), b(on)),
        Param::Phase { ch, on } => (format!("/preamp/{}/phase", n(ch)), b(on)),
        Param::Route { input, output, on } => {
            (format!("/routing/in/{}/out/{}", n(input), n(output)), b(on))
        }
        Param::Dim { on } => ("/controlroom/dim".to_owned(), b(on)),
        Param::Mono { on } => ("/controlroom/mono".to_owned(), b(on)),
        Param::SpeakerB { on } => ("/controlroom/ab".to_owned(), b(on)),
        Param::Talkback { on } => ("/controlroom/talkback".to_owned(), b(on)),
        Param::PhonesVol { db } => (
            "/controlroom/phones/vol".to_owned(),
            f(fader::trim_db_to_pos(db)),
        ),
        Param::OutputTrim { ch, db } => (
            format!("/output/{}/trim", n(ch)),
            f(fader::trim_db_to_pos(db)),
        ),
    };
    Some(OscMessage {
        addr,
        args: vec![arg],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(addr: &str, arg: OscType) -> OscMessage {
        OscMessage {
            addr: addr.to_owned(),
            args: vec![arg],
        }
    }

    #[test]
    fn parses_the_handoff_table() {
        let cases = [
            (
                msg("/mix/1/ch/3/vol", OscType::Float(0.75)),
                Param::MixVol {
                    bus: 0,
                    ch: 2,
                    gain: 1.0,
                },
            ),
            (
                msg("/mix/7/ch/1/pan", OscType::Float(-1.0)),
                Param::MixPan {
                    bus: 6,
                    ch: 0,
                    value: -1.0,
                },
            ),
            (
                msg("/mix/2/ch/2/mute", OscType::Int(1)),
                Param::MixMute {
                    bus: 1,
                    ch: 1,
                    on: true,
                },
            ),
            (
                msg("/mix/2/ch/2/solo", OscType::Float(0.0)),
                Param::MixSolo {
                    bus: 1,
                    ch: 1,
                    on: false,
                },
            ),
            (
                msg("/mix/main/vol", OscType::Float(1.0)),
                Param::MainVol { db: 0.0 },
            ),
            (
                msg("/preamp/1/gain", OscType::Float(30.0)),
                Param::PreampGain { ch: 0, db: 30.0 },
            ),
            (
                msg("/preamp/2/48v", OscType::Int(1)),
                Param::Phantom { ch: 1, on: true },
            ),
            (
                msg("/preamp/1/pad", OscType::Bool(true)),
                Param::Pad { ch: 0, on: true },
            ),
            (
                msg("/preamp/1/phase", OscType::Int(0)),
                Param::Phase { ch: 0, on: false },
            ),
            (
                msg("/eq/4/band/2/bypass", OscType::Int(1)),
                Param::InputEqBypass {
                    ch: 3,
                    band: 1,
                    on: true,
                },
            ),
            (
                msg("/routing/in/1/out/2", OscType::Int(1)),
                Param::Route {
                    input: 0,
                    output: 1,
                    on: true,
                },
            ),
            (
                msg("/controlroom/dim", OscType::Int(1)),
                Param::Dim { on: true },
            ),
            (
                msg("/controlroom/mono", OscType::Int(1)),
                Param::Mono { on: true },
            ),
            (
                msg("/controlroom/ab", OscType::Int(1)),
                Param::SpeakerB { on: true },
            ),
            (
                msg("/controlroom/talkback", OscType::Int(1)),
                Param::Talkback { on: true },
            ),
        ];
        for (m, p) in cases {
            assert_eq!(parse(&m), Ok(Command::Set(p)), "{}", m.addr);
        }
        assert_eq!(
            parse(&msg("/preset/load", OscType::String("live".into()))),
            Ok(Command::LoadPreset {
                name: "live".into()
            })
        );
    }

    #[test]
    fn feedback_round_trips_through_parse() {
        let params = [
            Param::MixVol {
                bus: 6,
                ch: 17,
                gain: 1.0,
            },
            Param::MixMute {
                bus: 0,
                ch: 0,
                on: true,
            },
            Param::Route {
                input: 17,
                output: 21,
                on: true,
            },
            Param::PreampGain { ch: 1, db: 42.0 },
            Param::SpeakerB { on: true },
            Param::PhonesVol { db: 0.0 },
        ];
        for p in params {
            assert_eq!(parse(&feedback(&p).unwrap()), Ok(Command::Set(p)));
        }
    }

    #[test]
    fn volume_edges_use_the_fader_laws() {
        let vol = |p: f32| parse(&msg("/mix/1/ch/1/vol", OscType::Float(p)));
        assert_eq!(
            vol(0.0),
            Ok(Command::Set(Param::MixVol {
                bus: 0,
                ch: 0,
                gain: 0.0
            }))
        );
        let Ok(Command::Set(Param::MixVol { gain, .. })) = vol(1.0) else {
            panic!()
        };
        assert!((gain - fader::max_gain()).abs() < 1e-9, "+12 dB at the top");
        let main = |p: f32| parse(&msg("/mix/main/vol", OscType::Float(p)));
        assert_eq!(
            main(0.0),
            Ok(Command::Set(Param::MainVol { db: TRIM_MIN_DB }))
        );
        let Ok(Command::Set(Param::MainVol { db })) = main(0.5) else {
            panic!()
        };
        assert!((-30.0..-25.0).contains(&db), "{db}");
        // Feedback: positions round-trip within f32 precision.
        for db in [-60.0, -24.0, -6.0, 0.0] {
            let back = parse(&feedback(&Param::PhonesVol { db }).unwrap());
            let Ok(Command::Set(Param::PhonesVol { db: got })) = back else {
                panic!()
            };
            assert!((got - db).abs() < 1e-3, "{db} -> {got}");
        }
        for gain in [0.1, 0.5, 2.0] {
            let back = parse(
                &feedback(&Param::MixVol {
                    bus: 0,
                    ch: 0,
                    gain,
                })
                .unwrap(),
            );
            let Ok(Command::Set(Param::MixVol { gain: got, .. })) = back else {
                panic!()
            };
            assert!((got - gain).abs() / gain < 1e-5, "{gain} -> {got}");
        }
        // NaN / inf are rejected, never turned into silence.
        assert!(matches!(vol(f32::NAN), Err(OscError::ArgType(_))));
        assert!(matches!(main(f32::INFINITY), Err(OscError::ArgType(_))));
    }

    #[test]
    fn power_addresses_parse_and_give_feedback() {
        use crate::power::FadeDir;
        let power = |a: &str, arg| parse(&msg(a, arg));
        assert_eq!(
            power("/mix/main/power", OscType::Int(1)),
            Ok(Command::Power {
                channel: PowerChannel::Main,
                on: true
            })
        );
        assert_eq!(
            power("/mix/main/power", OscType::Float(0.0)),
            Ok(Command::Power {
                channel: PowerChannel::Main,
                on: false
            })
        );
        assert_eq!(
            power("/controlroom/phones/power", OscType::Bool(true)),
            Ok(Command::Power {
                channel: PowerChannel::Phones,
                on: true
            })
        );
        assert!(matches!(
            power("/mix/main/power", OscType::String("on".into())),
            Err(OscError::ArgType(_))
        ));
        assert!(matches!(
            parse(&OscMessage {
                addr: "/mix/main/power".into(),
                args: vec![]
            }),
            Err(OscError::Arity(_))
        ));
        // Feedback follows where each channel is heading, as 0/1 ints.
        let main = ChannelPower {
            on: true,
            fading: Some(FadeDir::Out),
            plug: None,
            return_db: Some(-20.0),
        };
        let phones = ChannelPower {
            on: false,
            fading: Some(FadeDir::In),
            plug: None,
            return_db: None,
        };
        let fb = power_feedback(&main, &phones);
        assert_eq!(fb.len(), 2);
        assert_eq!(
            (fb[0].addr.as_str(), fb[0].args.as_slice()),
            ("/mix/main/power", &[OscType::Int(0)][..])
        );
        assert_eq!(
            (fb[1].addr.as_str(), fb[1].args.as_slice()),
            ("/controlroom/phones/power", &[OscType::Int(1)][..])
        );
        // What is sent can be parsed back by the same table.
        for m in &fb {
            assert!(matches!(parse(m), Ok(Command::Power { .. })));
        }
    }

    #[test]
    fn rejects_bad_messages() {
        assert!(matches!(
            parse(&msg("/mix/0/ch/1/vol", OscType::Float(0.1))),
            Err(OscError::BadIndex(..))
        ));
        assert!(matches!(
            parse(&msg("/mix/x/ch/1/vol", OscType::Float(0.1))),
            Err(OscError::BadIndex(..))
        ));
        assert!(matches!(
            parse(&msg("/mix/300/ch/1/vol", OscType::Float(0.1))),
            Err(OscError::BadIndex(..))
        ));
        assert!(matches!(
            parse(&msg("/nope", OscType::Int(1))),
            Err(OscError::UnknownAddress(_))
        ));
        assert!(matches!(
            parse(&msg("/controlroom/dim", OscType::String("x".into()))),
            Err(OscError::ArgType(_))
        ));
        let no_args = OscMessage {
            addr: "/controlroom/dim".into(),
            args: vec![],
        };
        assert!(matches!(parse(&no_args), Err(OscError::Arity(_))));
    }
}

#[cfg(test)]
mod dsp_osc_tests {
    use super::*;
    use crate::state::MotuState;

    fn msg(addr: &str, arg: OscType) -> OscMessage {
        OscMessage {
            addr: addr.to_owned(),
            args: vec![arg],
        }
    }

    #[test]
    fn dsp_addresses_parse() {
        let cases = [
            (
                msg("/eq/1/band/4/freq", OscType::Float(7000.0)),
                Param::InputEqFreq {
                    ch: 0,
                    band: 3,
                    hz: 7000.0,
                },
            ),
            (
                msg("/eq/8/band/1/mode", OscType::Int(3)),
                Param::InputEqMode {
                    ch: 7,
                    band: 0,
                    mode: 3,
                },
            ),
            (
                msg("/output/12/eq/band/3/gain", OscType::Float(-2.0)),
                Param::OutputEqGain {
                    ch: 11,
                    band: 2,
                    db: -2.0,
                },
            ),
            (
                msg("/output/1/eq/band/1/bw", OscType::Float(0.5)),
                Param::OutputEqBw {
                    ch: 0,
                    band: 0,
                    octaves: 0.5,
                },
            ),
            (
                msg("/gate/2/threshold", OscType::Float(-46.0)),
                Param::GateThreshold { ch: 1, db: -46.0 },
            ),
            (
                msg("/comp/3/makeup", OscType::Float(2.0)),
                Param::CompMakeup { ch: 2, gain: 2.0 },
            ),
            (
                msg("/comp/1/ratio", OscType::Int(4)),
                Param::CompRatio { ch: 0, ratio: 4.0 },
            ),
            (
                msg("/reverb/decay", OscType::Int(86)),
                Param::ReverbDecay { value: 86.0 },
            ),
            (
                msg("/reverb/preset", OscType::Float(2.0)),
                Param::ReverbPreset { preset: 2 },
            ),
            (
                msg("/clock/rate", OscType::Int(96_000)),
                Param::SampleRate {
                    rate: SampleRate::Hz96000,
                },
            ),
            (
                msg("/clock/source", OscType::String("Optical".into())),
                Param::ClockSource {
                    source: ClockSource::Optical,
                },
            ),
        ];
        for (m, p) in cases {
            assert_eq!(parse(&m), Ok(Command::Set(p)), "{}", m.addr);
        }
        for bad in [
            msg("/clock/rate", OscType::Int(32_000)),
            msg("/clock/source", OscType::String("adat".into())),
            msg("/clock/source", OscType::Int(3)),
            msg("/reverb/preset", OscType::Int(256)),
            msg("/eq/1/band/1/mode", OscType::Int(-1)),
        ] {
            assert!(matches!(parse(&bad), Err(OscError::ArgType(_))), "{bad:?}");
        }
        assert!(matches!(
            parse(&msg("/eq/1/band/1/q", OscType::Float(1.0))),
            Err(OscError::UnknownAddress(_))
        ));
        assert!(matches!(
            parse(&msg("/eq/1/band/0/freq", OscType::Float(1.0))),
            Err(OscError::BadIndex(..))
        ));
        assert!(matches!(
            parse(&msg("/eq/1/bypass", OscType::Int(1))),
            Err(OscError::UnknownAddress(_))
        ));
    }

    #[test]
    fn every_param_has_feedback_that_parses_back() {
        // All params of the default state, whole-number values: exact in f32.
        for p in MotuState::default().all_params() {
            let Some(m) = feedback(&p) else {
                panic!("no OSC address for {p:?}")
            };
            let Ok(Command::Set(back)) = parse(&m) else {
                panic!("{} does not parse", m.addr)
            };
            assert_eq!(
                crate::motu::throttle::key_of(&back),
                crate::motu::throttle::key_of(&p)
            );
            if !matches!(
                p,
                Param::MixVol { .. }
                    | Param::MainVol { .. }
                    | Param::PhonesVol { .. }
                    | Param::OutputTrim { .. }
            ) {
                assert_eq!(back, p, "{}", m.addr);
            }
        }
    }

    #[test]
    fn hostile_osc_messages_are_refused() {
        let m = |addr: &str, args: Vec<OscType>| OscMessage {
            addr: addr.to_owned(),
            args,
        };
        // Non-finite floats.
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for a in ["/mix/main/vol", "/preamp/1/gain", "/mix/1/ch/1/pan"] {
                assert!(parse(&m(a, vec![OscType::Float(v)])).is_err(), "{a} {v}");
            }
        }
        assert!(parse(&m("/mix/main/vol", vec![OscType::Double(f64::NAN)])).is_err());
        // Indices: digits only, in range.
        for a in [
            "/mix/+1/ch/1/vol",
            "/mix/1/ch/+1/vol",
            "/mix/1/ch/-1/vol",
            "/mix/1/ch/0/vol",
            "/mix/1/ch/65536/vol",
            "/mix/1/ch/1e1/vol",
            "/mix/1/ch/\u{ff11}/vol",
        ] {
            assert!(parse(&m(a, vec![OscType::Float(0.5)])).is_err(), "{a}");
        }
        // Address and argument count caps.
        let long = format!("/mix/1/ch/1/vol{}", "x".repeat(MAX_ADDRESS_LEN));
        assert!(parse(&m(&long, vec![OscType::Float(0.5)])).is_err());
        assert!(parse(&m("/mix/main/vol", vec![OscType::Float(0.5); 5])).is_err());
        // Preset names go through the same allowlist as the web side.
        for bad in ["../x", "a/b", "", ".x", "a\0b", &"x".repeat(100)] {
            assert!(
                parse(&m("/preset/load", vec![OscType::String(bad.into())])).is_err(),
                "{bad:?}"
            );
        }
        assert!(parse(&m("/preset/load", vec![OscType::String("live".into())])).is_ok());
        // Unknown strings in enum arguments are errors, not panics.
        assert!(parse(&m(
            "/clock/source",
            vec![OscType::String("x".repeat(10_000))]
        ))
        .is_err());
    }
}
