//! Protocol vectors: one sample of every `Param`, `Command` and `ServerEvent`, serialized by
//! the server's own types into `design/vectors/protocol.json`. The frontend's vitest suite
//! (`frontend/src/lib/protocol.test.ts`) checks `protocol.ts` and the reducer against the same
//! file, so a renamed field or a new message fails one side or the other instead of drifting.
//!
//! The file is generated, never hand-edited. After a protocol change:
//! `UPDATE_VECTORS=1 cargo test --test protocol_vectors`, then review the diff.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cuemix_web::motu::client::ConnectionStatus;
use cuemix_web::osc::settings::OscSettings;
use cuemix_web::plug::{PlugKind, PlugState, PlugUrls, SpeakerPlugSettings};
use cuemix_web::power::{ChannelPower, FadeDir, PowerChannel};
use cuemix_web::state::{
    ChannelLabel, ClockSource, Command, DeviceInfo, Labels, MeasureState, MeasureStatus, MotuState,
    NoticeLevel, OpticalMode, Param, SampleRate, ServerEvent, SessionInfo, Topology,
};
use serde_json::{json, Value};

/// A small device, so the snapshot stays readable (same shapes as the mk5, fewer channels).
const TOPOLOGY: Topology = Topology {
    mixes: 2,
    mixer_inputs: 4,
    preamps: 2,
    mic_inputs: 1,
    routing_inputs: 3,
    routing_outputs: 3,
};

/// Exhaustive on purpose: a new variant does not compile until it gets an index here and a
/// sample in `params()`.
fn param_kind(p: &Param) -> usize {
    use Param as P;
    match p {
        P::MixVol { .. } => 0,
        P::MixPan { .. } => 1,
        P::MixMute { .. } => 2,
        P::MixSolo { .. } => 3,
        P::MainVol { .. } => 4,
        P::PreampGain { .. } => 5,
        P::Phantom { .. } => 6,
        P::Pad { .. } => 7,
        P::Phase { .. } => 8,
        P::Route { .. } => 9,
        P::Dim { .. } => 10,
        P::Mono { .. } => 11,
        P::SpeakerB { .. } => 12,
        P::Talkback { .. } => 13,
        P::PhonesVol { .. } => 14,
        P::OutputTrim { .. } => 15,
        P::InputEqMode { .. } => 16,
        P::InputEqBypass { .. } => 17,
        P::InputEqFreq { .. } => 18,
        P::InputEqGain { .. } => 19,
        P::InputEqBw { .. } => 20,
        P::OutputEqMode { .. } => 21,
        P::OutputEqBypass { .. } => 22,
        P::OutputEqFreq { .. } => 23,
        P::OutputEqGain { .. } => 24,
        P::OutputEqBw { .. } => 25,
        P::GateBypass { .. } => 26,
        P::GateThreshold { .. } => 27,
        P::GateAttack { .. } => 28,
        P::GateRelease { .. } => 29,
        P::CompBypass { .. } => 30,
        P::CompThreshold { .. } => 31,
        P::CompAttack { .. } => 32,
        P::CompRelease { .. } => 33,
        P::CompRatio { .. } => 34,
        P::CompMakeup { .. } => 35,
        P::ReverbDecay { .. } => 36,
        P::ReverbDamping { .. } => 37,
        P::ReverbPredelay { .. } => 38,
        P::ReverbWidth { .. } => 39,
        P::ReverbPreset { .. } => 40,
        P::SampleRate { .. } => 41,
        P::ClockSource { .. } => 42,
        P::OpticalMode { .. } => 43,
    }
}
const PARAM_KINDS: usize = 44;

fn command_kind(c: &Command) -> usize {
    use Command as C;
    match c {
        C::Set(_) => 0,
        C::LoadPreset { .. } => 1,
        C::SavePreset { .. } => 2,
        C::ImportPreset { .. } => 3,
        C::ListPresets => 4,
        C::SetLabel { .. } => 5,
        C::MeasureStart => 6,
        C::MeasureStop => 7,
        C::Power { .. } => 8,
        C::SetSpeakerPlug { .. } => 9,
        C::TestSpeakerPlug { .. } => 10,
        C::SetOscSettings { .. } => 11,
    }
}
const COMMAND_KINDS: usize = 12;

fn event_kind(e: &ServerEvent) -> usize {
    use ServerEvent as E;
    match e {
        E::Snapshot { .. } => 0,
        E::Changed { .. } => 1,
        E::Status { .. } => 2,
        E::Session(_) => 3,
        E::Presets { .. } => 4,
        E::Labels(_) => 5,
        E::Meters { .. } => 6,
        E::MeterRaw { .. } => 7,
        E::Patch { .. } => 8,
        E::Measure(_) => 9,
        E::Power { .. } => 10,
        E::SpeakerPlug { .. } => 11,
        E::SpeakerPlugResult { .. } => 12,
        E::OscSettings { .. } => 13,
        E::Notice { .. } => 14,
        E::Error { .. } => 15,
    }
}
const EVENT_KINDS: usize = 16;

/// One sample per param, each a change from the neutral state and inside its range (so the
/// server applies it unclamped and the client can apply it as is).
fn params() -> Vec<Param> {
    use Param as P;
    vec![
        P::MixVol {
            bus: 0,
            ch: 1,
            gain: 0.5,
        },
        P::MixPan {
            bus: 1,
            ch: 2,
            value: -0.5,
        },
        P::MixMute {
            bus: 0,
            ch: 3,
            on: true,
        },
        P::MixSolo {
            bus: 1,
            ch: 0,
            on: true,
        },
        P::MainVol { db: -12.0 },
        P::PreampGain { ch: 0, db: 30.0 },
        P::Phantom { ch: 0, on: true },
        P::Pad { ch: 0, on: true },
        P::Phase { ch: 1, on: true },
        P::Route {
            input: 2,
            output: 1,
            on: true,
        },
        P::Dim { on: true },
        P::Mono { on: true },
        P::SpeakerB { on: true },
        P::Talkback { on: true },
        P::PhonesVol { db: -6.0 },
        P::OutputTrim { ch: 2, db: -3.0 },
        P::InputEqMode {
            ch: 1,
            band: 1,
            mode: 2,
        },
        P::InputEqBypass {
            ch: 1,
            band: 2,
            on: true,
        },
        P::InputEqFreq {
            ch: 0,
            band: 1,
            hz: 2500.0,
        },
        P::InputEqGain {
            ch: 0,
            band: 2,
            db: 4.5,
        },
        P::InputEqBw {
            ch: 0,
            band: 1,
            octaves: 0.5,
        },
        P::OutputEqMode {
            ch: 3,
            band: 1,
            mode: 2,
        },
        P::OutputEqBypass {
            ch: 3,
            band: 2,
            on: true,
        },
        P::OutputEqFreq {
            ch: 4,
            band: 1,
            hz: 1200.0,
        },
        P::OutputEqGain {
            ch: 4,
            band: 0,
            db: -2.5,
        },
        P::OutputEqBw {
            ch: 4,
            band: 1,
            octaves: 2.0,
        },
        P::GateBypass { ch: 0, on: false },
        P::GateThreshold { ch: 0, db: -30.0 },
        P::GateAttack { ch: 0, ms: 5.0 },
        P::GateRelease { ch: 0, ms: 200.0 },
        P::CompBypass { ch: 1, on: false },
        P::CompThreshold { ch: 1, db: -20.0 },
        P::CompAttack { ch: 1, ms: 20.0 },
        P::CompRelease { ch: 1, ms: 500.0 },
        P::CompRatio { ch: 1, ratio: 4.0 },
        P::CompMakeup { ch: 1, gain: 2.0 },
        P::ReverbDecay { value: 50.0 },
        P::ReverbDamping { value: 40.0 },
        P::ReverbPredelay { ms: 20.0 },
        P::ReverbWidth { value: 80.0 },
        P::ReverbPreset { preset: 3 },
        P::SampleRate {
            rate: SampleRate::Hz96000,
        },
        P::ClockSource {
            source: ClockSource::Spdif,
        },
        P::OpticalMode {
            output: true,
            mode: OpticalMode::Toslink,
        },
    ]
}

fn plug_settings() -> SpeakerPlugSettings {
    SpeakerPlugSettings {
        enabled: true,
        kind: PlugKind::ShellyRpc,
        host: "plug.example".into(),
        ..SpeakerPlugSettings::default()
    }
}

fn osc_settings() -> OscSettings {
    OscSettings {
        listen: "127.0.0.1:9000".into(),
        targets: vec!["192.0.2.10:9001".into()],
    }
}

fn label() -> ChannelLabel {
    ChannelLabel {
        name: "Vocal".into(),
        color: Some("#06B6D4".into()),
    }
}

fn commands() -> Vec<Command> {
    vec![
        Command::Set(Param::MixVol {
            bus: 0,
            ch: 1,
            gain: 0.5,
        }),
        Command::LoadPreset {
            name: "Live".into(),
        },
        Command::SavePreset {
            name: "Live".into(),
        },
        Command::ImportPreset {
            name: "Imported".into(),
            text: "{}".into(),
        },
        Command::ListPresets,
        Command::SetLabel {
            ch: 0,
            label: Some(label()),
        },
        Command::MeasureStart,
        Command::MeasureStop,
        Command::Power {
            channel: PowerChannel::Main,
            on: false,
        },
        Command::SetSpeakerPlug {
            settings: plug_settings(),
        },
        Command::TestSpeakerPlug { on: true },
        Command::SetOscSettings {
            settings: osc_settings(),
        },
    ]
}

fn session() -> SessionInfo {
    SessionInfo {
        active_preset: Some("Live".into()),
        modified: true,
        connection: ConnectionStatus::Online,
        device: Some(DeviceInfo {
            model: "UL5E".into(),
            firmware: "2.0.0".into(),
            topology: TOPOLOGY,
            ..DeviceInfo::default()
        }),
    }
}

fn events() -> Vec<ServerEvent> {
    let mut channels = BTreeMap::new();
    channels.insert(0, label());
    vec![
        ServerEvent::Snapshot {
            state: Box::new(MotuState::new(TOPOLOGY)),
            session: SessionInfo {
                device: None,
                modified: false,
                ..session()
            },
        },
        ServerEvent::Changed {
            params: params().into_iter().take(2).collect(),
        },
        ServerEvent::Status {
            connection: ConnectionStatus::Demo,
        },
        ServerEvent::Session(session()),
        ServerEvent::Presets {
            names: vec!["Live".into(), "Studio".into()],
        },
        ServerEvent::Labels(Labels { channels }),
        ServerEvent::Meters {
            levels: vec![0.0, 0.25, 0.5, 1.0],
        },
        ServerEvent::MeterRaw {
            raw: vec![0, 64, 255],
        },
        ServerEvent::Patch {
            sources: vec![0, 1, 2],
        },
        ServerEvent::Measure(MeasureStatus {
            state: MeasureState::Idle,
            source: vec![2, 3],
            message: None,
            until: None,
        }),
        ServerEvent::Power {
            main: ChannelPower {
                on: true,
                fading: Some(FadeDir::Out),
                plug: Some(PlugState::On),
                return_db: Some(-18.0),
            },
            phones: ChannelPower {
                on: false,
                fading: None,
                plug: None,
                return_db: Some(-70.0),
            },
        },
        ServerEvent::SpeakerPlug {
            settings: plug_settings(),
            urls: PlugUrls {
                on: "http://plug.example/rpc/Switch.Set?id=0&on=true".into(),
                off: "http://plug.example/rpc/Switch.Set?id=0&on=false".into(),
            },
            state: PlugState::Off,
        },
        ServerEvent::SpeakerPlugResult {
            on: true,
            ok: false,
            message: "timed out".into(),
        },
        ServerEvent::OscSettings {
            settings: osc_settings(),
            listening: true,
            error: None,
        },
        ServerEvent::Notice {
            level: NoticeLevel::Warning,
            message: "Link lost".into(),
        },
        ServerEvent::Error {
            message: "unknown preset".into(),
        },
    ]
}

/// Every kind has exactly one sample, in index order.
fn assert_covers<T>(items: &[T], kind: fn(&T) -> usize, kinds: usize, what: &str) {
    let got: Vec<usize> = items.iter().map(kind).collect();
    let want: Vec<usize> = (0..kinds).collect();
    assert_eq!(
        got, want,
        "{what}: one sample per variant, in declaration order"
    );
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|e| panic!("serialize: {e}"))
}

/// Serialize, then check the server reads it back to the same value.
fn round_trip<T>(items: &[T]) -> Vec<Value>
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    items
        .iter()
        .map(|item| {
            let v = to_value(item);
            let back: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(&back, item, "round trip of {v}");
            v
        })
        .collect()
}

fn build() -> Value {
    let params = params();
    assert_covers(&params, param_kind, PARAM_KINDS, "params");
    let commands = commands();
    assert_covers(&commands, command_kind, COMMAND_KINDS, "commands");
    let events = events();
    assert_covers(&events, event_kind, EVENT_KINDS, "events");

    // Each sample must be a real, unclamped change, so the client test can tell it applied.
    let mut state = MotuState::new(TOPOLOGY);
    for p in &params {
        let effective = state
            .apply(p)
            .unwrap_or_else(|e| panic!("{p:?} rejected: {e}"))
            .unwrap_or_else(|| panic!("{p:?} changed nothing: pick a non-default value"));
        assert_eq!(
            &effective, p,
            "{p:?} was clamped: pick a value inside its range"
        );
    }

    let tags: BTreeSet<String> = events
        .iter()
        .filter_map(|e| {
            to_value(e)
                .get("event")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(tags.len(), EVENT_KINDS, "event tags are unique");

    json!({
        "_comment": "Generated by tests/protocol_vectors.rs, do not edit. Regenerate: UPDATE_VECTORS=1 cargo test --test protocol_vectors",
        "params": round_trip(&params),
        "after_params": to_value(&state),
        "commands": round_trip(&commands),
        // Events only go server -> client: serialized, not read back (serde cannot read an
        // internally tagged enum holding integer map keys, as in `labels`).
        "events": events.iter().map(to_value).collect::<Vec<_>>(),
    })
}

#[test]
fn protocol_vectors_are_current() {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "design",
        "vectors",
        "protocol.json",
    ]
    .iter()
    .collect();
    let mut text = serde_json::to_string_pretty(&build()).unwrap_or_else(|e| panic!("{e}"));
    text.push('\n');
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return;
    }
    let current = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        current == text,
        "{} is out of date with src/state.rs. Run `UPDATE_VECTORS=1 cargo test --test protocol_vectors`, \
         review the diff, and update frontend/src/lib/protocol.ts if a message changed.",
        path.display()
    );
}
