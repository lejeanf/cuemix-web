//! OSC bridge over real UDP on loopback.

use cuemix_web::motu::fader::pos_to_gain;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use cuemix_web::engine::{self, EngineConfig, EngineHandle, Envelope, Origin, Shutdown};
use cuemix_web::osc::bridge::OscBridge;
use cuemix_web::state::{Param, ServerEvent, Topology};
use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tokio::sync::broadcast;

struct Rig {
    engine: EngineHandle,
    bridge: SocketAddr,
    _trigger: engine::ShutdownTrigger,
    _dir: std::path::PathBuf,
    _link: engine::DeviceLink,
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("cuemix-osc-{tag}-{}-{nanos}", std::process::id()))
}

async fn rig(tag: &str, targets: Vec<SocketAddr>) -> Rig {
    let dir = tmp(tag);
    let (engine, link, _task) = engine::start(EngineConfig {
        topology: Topology::default(),
        presets_dir: dir.join("presets"),
        labels_file: dir.join("labels.json"),
    })
    .await;
    let bridge = OscBridge::bind("127.0.0.1:0".parse().unwrap(), targets)
        .await
        .unwrap();
    let addr = bridge.local_addr().unwrap();
    let (trigger, shutdown): (_, Shutdown) = engine::shutdown_channel();
    tokio::spawn(bridge.run(engine.clone(), shutdown));
    Rig {
        engine,
        bridge: addr,
        _trigger: trigger,
        _dir: dir,
        _link: link,
    }
}

async fn sock() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.unwrap()
}

fn msg(addr: &str, arg: OscType) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args: vec![arg],
    }
}

async fn send(s: &UdpSocket, to: SocketAddr, m: OscMessage) {
    let bytes = rosc::encoder::encode(&OscPacket::Message(m)).unwrap();
    s.send_to(&bytes, to).await.unwrap();
}

fn flatten(p: OscPacket, out: &mut Vec<OscMessage>) {
    match p {
        OscPacket::Message(m) => out.push(m),
        OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| flatten(p, out)),
    }
}

/// Receive everything until the socket has been quiet for `quiet`.
async fn collect(s: &UdpSocket, quiet: Duration) -> Vec<OscMessage> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65536];
    while let Ok(Ok(n)) = tokio::time::timeout(quiet, s.recv(&mut buf)).await {
        if let Ok((_, p)) = rosc::decoder::decode_udp(&buf[..n]) {
            flatten(p, &mut out);
        }
    }
    out
}

fn float_of(m: &OscMessage) -> f32 {
    match m.args.first() {
        Some(OscType::Float(f)) => *f,
        _ => f32::NAN,
    }
}

async fn next_changed(rx: &mut broadcast::Receiver<Envelope>) -> (Origin, Vec<Param>) {
    loop {
        let env = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timeout")
            .expect("recv");
        if let ServerEvent::Changed { params } = env.event {
            return (env.origin, params);
        }
    }
}

const Q: Duration = Duration::from_millis(300);

#[tokio::test]
async fn a_osc_message_reaches_engine() {
    let r = rig("a", vec![]).await;
    let mut rx = r.engine.subscribe();
    let c = sock().await;
    send(&c, r.bridge, msg("/mix/1/ch/1/vol", OscType::Float(0.5))).await;
    let (origin, params) = next_changed(&mut rx).await;
    assert_eq!(origin, Origin::Osc(c.local_addr().unwrap()));
    assert_eq!(
        params,
        vec![Param::MixVol {
            bus: 0,
            ch: 0,
            gain: pos_to_gain(0.5)
        }]
    );
}

#[tokio::test]
async fn b_feedback_to_other_peer_but_no_echo() {
    let r = rig("b", vec![]).await;
    let (a, b) = (sock().await, sock().await);
    // A registers first and swallows its initial dump.
    send(&a, r.bridge, msg("/controlroom/mono", OscType::Int(0))).await;
    let _ = collect(&a, Q).await;
    // B registers right after: its dump is rate-limited, wait for it.
    send(&b, r.bridge, msg("/controlroom/mono", OscType::Int(0))).await;
    let _ = collect(&b, Duration::from_millis(1500)).await;

    send(&a, r.bridge, msg("/mix/1/ch/1/vol", OscType::Float(0.37))).await;
    let got_b = collect(&b, Q).await;
    assert!(got_b
        .iter()
        .any(|m| m.addr == "/mix/1/ch/1/vol" && (float_of(m) - 0.37).abs() < 1e-4));
    let got_a = collect(&a, Q).await;
    assert!(!got_a
        .iter()
        .any(|m| m.addr == "/mix/1/ch/1/vol" && (float_of(m) - 0.37).abs() < 1e-4));
}

#[tokio::test]
async fn c_configured_target_receives_feedback() {
    let target = sock().await;
    let r = rig("c", vec![target.local_addr().unwrap()]).await;
    let c = sock().await;
    send(&c, r.bridge, msg("/mix/1/ch/2/vol", OscType::Float(0.25))).await;
    let got = collect(&target, Q).await;
    assert!(got
        .iter()
        .any(|m| m.addr == "/mix/1/ch/2/vol" && (float_of(m) - 0.25).abs() < 1e-4));
}

#[tokio::test]
async fn d_bundle_applies_both() {
    let r = rig("d", vec![]).await;
    let mut rx = r.engine.subscribe();
    let c = sock().await;
    let imm = rosc::OscTime {
        seconds: 0,
        fractional: 1,
    };
    let bundle = OscPacket::Bundle(rosc::OscBundle {
        timetag: imm,
        content: vec![
            OscPacket::Message(msg("/mix/1/ch/1/vol", OscType::Float(0.25))),
            OscPacket::Bundle(rosc::OscBundle {
                timetag: imm,
                content: vec![OscPacket::Message(msg(
                    "/mix/1/ch/2/vol",
                    OscType::Float(0.5),
                ))],
            }),
        ],
    });
    c.send_to(&rosc::encoder::encode(&bundle).unwrap(), r.bridge)
        .await
        .unwrap();
    let mut seen = Vec::new();
    while seen.len() < 2 {
        seen.extend(next_changed(&mut rx).await.1);
    }
    assert!(seen.contains(&Param::MixVol {
        bus: 0,
        ch: 0,
        gain: pos_to_gain(0.25)
    }));
    assert!(seen.contains(&Param::MixVol {
        bus: 0,
        ch: 1,
        gain: pos_to_gain(0.5)
    }));
}

#[tokio::test]
async fn e_garbage_and_unknown_do_not_break_bridge() {
    let r = rig("e", vec![]).await;
    let mut rx = r.engine.subscribe();
    let c = sock().await;
    c.send_to(&[0xff, 0x00, 0x13, 0x37, 0x42], r.bridge)
        .await
        .unwrap();
    send(&c, r.bridge, msg("/nope/nothing", OscType::Int(1))).await;
    send(&c, r.bridge, msg("/mix/1/ch/1/vol", OscType::Float(0.6))).await;
    let (_, params) = next_changed(&mut rx).await;
    assert_eq!(
        params,
        vec![Param::MixVol {
            bus: 0,
            ch: 0,
            gain: pos_to_gain(f64::from(0.6_f32))
        }]
    );
}

#[tokio::test]
async fn f_missing_preset_does_not_crash() {
    let r = rig("f", vec![]).await;
    let mut rx = r.engine.subscribe();
    let c = sock().await;
    send(
        &c,
        r.bridge,
        msg("/preset/load", OscType::String("missing".into())),
    )
    .await;
    let env = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let e = rx.recv().await.unwrap();
            if matches!(e.event, ServerEvent::Error { .. }) {
                break e;
            }
        }
    })
    .await
    .expect("error event");
    assert_eq!(env.target, engine::Target::Only(env.origin));
    // Still alive.
    send(&c, r.bridge, msg("/mix/1/ch/1/vol", OscType::Float(0.4))).await;
    let (_, params) = next_changed(&mut rx).await;
    assert_eq!(params.len(), 1);
}

#[tokio::test]
async fn g_new_peer_gets_full_dump() {
    let r = rig("g", vec![]).await;
    let c = sock().await;
    send(&c, r.bridge, msg("/controlroom/dim", OscType::Int(0))).await;
    let got = collect(&c, Q).await;
    let expected = cuemix_web::state::MotuState::default().all_params().len();
    assert!(got.len() >= expected, "{} < {expected}", got.len());
    assert!(got.iter().any(|m| m.addr == "/controlroom/dim"));
    assert!(got.iter().any(|m| m.addr == "/mix/main/vol"));
}

async fn first_arrival(s: &UdpSocket, t0: Instant) -> Option<Duration> {
    let mut buf = vec![0u8; 65536];
    tokio::time::timeout(Duration::from_millis(2500), s.recv(&mut buf))
        .await
        .ok()
        .and_then(Result::ok)
        .map(|_| t0.elapsed())
}

#[tokio::test]
async fn h_dumps_are_rate_limited_and_coalesced() {
    let r = rig("h", vec![]).await;
    let (a, b) = (sock().await, sock().await);
    let t0 = Instant::now();
    send(&a, r.bridge, msg("/controlroom/dim", OscType::Int(0))).await;
    send(&b, r.bridge, msg("/controlroom/dim", OscType::Int(0))).await;
    let (ta, tb) = tokio::join!(first_arrival(&a, t0), first_arrival(&b, t0));
    let (ta, tb) = (ta.expect("a dump"), tb.expect("b dump (coalesced)"));
    assert!(ta < Duration::from_millis(500), "{ta:?}");
    assert!(tb >= Duration::from_millis(800), "{tb:?}");
    // B eventually gets the full dump.
    let got = collect(&b, Q).await;
    assert!(got.iter().any(|m| m.addr == "/mix/main/vol"));
}

#[tokio::test]
async fn i_sync_requests_dump() {
    let r = rig("i", vec![]).await;
    let c = sock().await;
    // /sync from an unknown peer enrols it and dumps.
    let sync = OscMessage {
        addr: "/sync".into(),
        args: vec![],
    };
    send(&c, r.bridge, sync.clone()).await;
    let expected = cuemix_web::state::MotuState::default().all_params().len();
    assert!(collect(&c, Q).await.len() >= expected);
    // Known peer: another /sync after the window yields another dump.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    send(&c, r.bridge, sync).await;
    assert!(collect(&c, Q).await.len() >= expected);
}

#[tokio::test]
async fn j_unparseable_message_does_not_enrol() {
    let r = rig("j", vec![]).await;
    let c = sock().await;
    send(&c, r.bridge, msg("/nope/nothing", OscType::Int(1))).await;
    send(
        &c,
        r.bridge,
        msg("/mix/1/ch/1/vol", OscType::Float(f32::NAN)),
    )
    .await;
    assert!(collect(&c, Q).await.is_empty());
}

fn nested(depth: usize) -> Vec<u8> {
    let mut cur = rosc::encoder::encode(&OscPacket::Message(msg(
        "/mix/1/ch/1/vol",
        OscType::Float(0.5),
    )))
    .unwrap();
    for _ in 0..depth {
        let mut b = b"#bundle ".to_vec();
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        b.extend_from_slice(&(cur.len() as u32).to_be_bytes());
        b.extend_from_slice(&cur);
        cur = b;
    }
    cur
}

#[tokio::test]
async fn k_nested_bundles_limits() {
    let r = rig("k", vec![]).await;
    let mut rx = r.engine.subscribe();
    let c = sock().await;

    let deep = nested(3000);
    assert!(deep.len() > 32 * 1024 && deep.len() <= 64 * 1024);
    c.send_to(&deep, r.bridge).await.unwrap();
    c.send_to(&nested(9), r.bridge).await.unwrap();
    // Neither produced a change; an 8-deep one does.
    c.send_to(&nested(8), r.bridge).await.unwrap();
    let (_, params) = next_changed(&mut rx).await;
    assert_eq!(params.len(), 1);
    // Bridge still works afterwards.
    send(&c, r.bridge, msg("/mix/1/ch/2/vol", OscType::Float(0.4))).await;
    let (_, params) = next_changed(&mut rx).await;
    assert!(matches!(params[0], Param::MixVol { ch: 1, .. }));
}

#[tokio::test]
async fn l_random_datagrams_do_not_break_bridge() {
    let r = rig("l", vec![]).await;
    let mut rx = r.engine.subscribe();
    let c = sock().await;
    let mut x: u64 = 0x1234_5678_9ABC_DEF1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let valid = nested(3);
    for i in 0..300 {
        let mut d = if i % 2 == 0 {
            valid.clone()
        } else {
            (0..(next() % 120) as usize).map(|_| next() as u8).collect()
        };
        if i % 2 == 0 {
            let cut = (next() as usize) % (d.len() + 1);
            d.truncate(cut);
        }
        let _ = c.send_to(&d, r.bridge).await;
    }
    // Drain any changes caused by intact packets, then prove liveness.
    send(&c, r.bridge, msg("/mix/1/ch/3/vol", OscType::Float(0.9))).await;
    loop {
        let (_, params) = next_changed(&mut rx).await;
        if params
            .iter()
            .any(|p| matches!(p, Param::MixVol { ch: 2, .. }))
        {
            break;
        }
    }
}
