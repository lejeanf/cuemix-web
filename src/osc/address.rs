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
use crate::state::{Command, Param, TRIM_MIN_DB};

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

fn index_u16(seg: &str, addr: &str) -> Result<u16, OscError> {
    match seg.parse::<u16>() {
        Ok(n) if n >= 1 => Ok(n - 1),
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
    let segs: Vec<&str> = addr.strip_prefix('/').unwrap_or(addr).split('/').collect();
    let unknown = || OscError::UnknownAddress(addr.to_owned());

    let param = match segs.as_slice() {
        ["mix", "main", "vol"] => Param::MainVol {
            db: fader::trim_pos_to_db(arg_f64(addr, args)?, TRIM_MIN_DB),
        },
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
        ["eq", ch, "bypass"] => Param::EqBypass {
            ch: index_u16(ch, addr)?,
            on: arg_bool(addr, args)?,
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
        ["preset", "load"] => match single(addr, args)? {
            OscType::String(name) => return Ok(Command::LoadPreset { name: name.clone() }),
            _ => return Err(OscError::ArgType(addr.to_owned())),
        },
        _ => return Err(unknown()),
    };
    Ok(Command::Set(param))
}

/// Encode an effective [`Param`] as OSC feedback (1-based addresses).
pub fn feedback(param: &Param) -> OscMessage {
    let n = |i: u16| u32::from(i) + 1;
    let b = |on: bool| OscType::Int(i32::from(on));
    // OSC floats are 32-bit; precision loss is expected and harmless here.
    let f = |v: f64| OscType::Float(v as f32);
    let (addr, arg) = match *param {
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
        Param::EqBypass { ch, on } => (format!("/eq/{}/bypass", n(ch)), b(on)),
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
    };
    OscMessage {
        addr,
        args: vec![arg],
    }
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
                msg("/eq/4/bypass", OscType::Int(1)),
                Param::EqBypass { ch: 3, on: true },
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
            assert_eq!(parse(&feedback(&p)), Ok(Command::Set(p)));
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
            let back = parse(&feedback(&Param::PhonesVol { db }));
            let Ok(Command::Set(Param::PhonesVol { db: got })) = back else {
                panic!()
            };
            assert!((got - db).abs() < 1e-3, "{db} -> {got}");
        }
        for gain in [0.1, 0.5, 2.0] {
            let back = parse(&feedback(&Param::MixVol {
                bus: 0,
                ch: 0,
                gain,
            }));
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
