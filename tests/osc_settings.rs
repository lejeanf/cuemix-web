//! Live OSC settings (`set_osc_settings`): rebind, targets, fence and persistence, over real
//! UDP on loopback.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use cuemix_web::engine::{self, EngineConfig, EngineHandle, Envelope, Origin, Shutdown, Target};
use cuemix_web::osc::bridge::OscBridge;
use cuemix_web::osc::settings::{self, OscSettings};
use cuemix_web::state::{Command, ServerEvent, Topology};
use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tokio::sync::broadcast;

struct Rig {
    engine: EngineHandle,
    listen: SocketAddr,
    dir: std::path::PathBuf,
    _trigger: engine::ShutdownTrigger,
    _link: engine::DeviceLink,
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "cuemix-oscset-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

/// A UDP port that was free a moment ago on loopback.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
        .map(|a| a.port())
        .unwrap()
}

fn lo(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

async fn rig(tag: &str, targets: Vec<SocketAddr>) -> Rig {
    let dir = tmp(tag);
    let listen = lo(free_port());
    let mut cfg = EngineConfig::for_dir(Topology::default(), &dir);
    cfg.osc = OscSettings {
        listen: listen.to_string(),
        targets: targets.iter().map(SocketAddr::to_string).collect(),
    };
    let (engine, link, _task) = engine::start(cfg).await;
    let bridge = OscBridge::bind(listen, targets).await.unwrap();
    let (trigger, shutdown): (_, Shutdown) = engine::shutdown_channel();
    tokio::spawn(bridge.run(engine.clone(), shutdown));
    let r = Rig {
        engine,
        listen,
        dir,
        _trigger: trigger,
        _link: link,
    };
    // The bridge announces itself once it runs.
    for _ in 0..100 {
        if osc_state(&r.engine).await.1 {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the bridge never attached");
}

/// What a new client would get: settings and `listening`.
async fn osc_state(engine: &EngineHandle) -> (OscSettings, bool, Option<String>) {
    for e in engine.initial_events().await.unwrap() {
        if let ServerEvent::OscSettings {
            settings,
            listening,
            error,
        } = e
        {
            return (settings, listening, error);
        }
    }
    panic!("no osc_settings event on connect");
}

enum Outcome {
    Applied(OscSettings),
    Refused(String),
}

const ME: Origin = Origin::Web(7);

/// Send `set_osc_settings` and wait for the broadcast or for the error.
async fn set(engine: &EngineHandle, settings: OscSettings) -> Outcome {
    let mut rx = engine.subscribe();
    engine
        .command(ME, Command::SetOscSettings { settings })
        .await
        .unwrap();
    loop {
        let env = next(&mut rx).await;
        match env.event {
            // A failure is broadcast with its reason, then answered with an `error`.
            ServerEvent::OscSettings {
                settings,
                error: None,
                ..
            } => return Outcome::Applied(settings),
            ServerEvent::Error { message } if env.is_for(ME) => return Outcome::Refused(message),
            _ => {}
        }
    }
}

async fn next(rx: &mut broadcast::Receiver<Envelope>) -> Envelope {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timeout")
        .expect("recv")
}

async fn sock() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.unwrap()
}

async fn send(s: &UdpSocket, to: SocketAddr, addr: &str, arg: Option<OscType>) {
    let m = OscMessage {
        addr: addr.into(),
        args: arg.into_iter().collect(),
    };
    let bytes = rosc::encoder::encode(&OscPacket::Message(m)).unwrap();
    s.send_to(&bytes, to).await.unwrap();
}

fn flatten(p: OscPacket, out: &mut Vec<OscMessage>) {
    match p {
        OscPacket::Message(m) => out.push(m),
        OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| flatten(p, out)),
    }
}

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

const Q: Duration = Duration::from_millis(300);

fn settings_of(listen: SocketAddr, targets: &[SocketAddr]) -> OscSettings {
    OscSettings {
        listen: listen.to_string(),
        targets: targets.iter().map(SocketAddr::to_string).collect(),
    }
}

#[tokio::test]
async fn a_a_new_client_gets_the_settings_and_listening() {
    let t = lo(9000);
    let r = rig("a", vec![t]).await;
    let (s, listening, error) = osc_state(&r.engine).await;
    assert_eq!(s, settings_of(r.listen, &[t]));
    assert!(listening);
    assert_eq!(error, None);
}

#[tokio::test]
async fn b_listen_change_rebinds_live_and_saves() {
    let r = rig("b", vec![]).await;
    let new = lo(free_port());
    let Outcome::Applied(s) = set(&r.engine, settings_of(new, &[])).await else {
        panic!("refused");
    };
    assert_eq!(s.listen, new.to_string());
    // The new port answers a sync with a dump; the old one is closed.
    let c = sock().await;
    send(&c, new, "/sync", None).await;
    let got = collect(&c, Duration::from_millis(1500)).await;
    assert!(
        got.iter().any(|m| m.addr.starts_with("/mix/")),
        "dump from the new port"
    );
    let old = sock().await;
    send(&old, r.listen, "/sync", None).await;
    assert!(
        collect(&old, Q).await.is_empty(),
        "old port no longer served"
    );
    // Saved, and reported to the next client.
    let saved = settings::load(&r.dir.join("osc.json")).await.unwrap();
    assert_eq!(saved, Some(settings_of(new, &[])));
    assert_eq!(osc_state(&r.engine).await.0, settings_of(new, &[]));
}

#[tokio::test]
async fn c_failed_bind_keeps_the_old_socket_and_changes_nothing() {
    let r = rig("c", vec![]).await;
    let taken = sock().await;
    let busy = taken.local_addr().unwrap();
    let Outcome::Refused(msg) = set(&r.engine, settings_of(busy, &[])).await else {
        panic!("applied");
    };
    assert!(msg.contains("cannot listen on"), "{msg}");
    let c = sock().await;
    send(&c, r.listen, "/sync", None).await;
    let got = collect(&c, Duration::from_millis(1500)).await;
    assert!(!got.is_empty(), "old socket still answers");
    assert_eq!(osc_state(&r.engine).await.0, settings_of(r.listen, &[]));
    assert!(!r.dir.join("osc.json").exists(), "nothing saved");
}

#[tokio::test]
async fn d_targets_change_immediately_with_a_dump_to_the_new_ones() {
    let (old_t, new_t) = (sock().await, sock().await);
    let (old_a, new_a) = (old_t.local_addr().unwrap(), new_t.local_addr().unwrap());
    let r = rig("d", vec![old_a]).await;
    let Outcome::Applied(_) = set(&r.engine, settings_of(r.listen, &[new_a])).await else {
        panic!("refused");
    };
    // Same listen address: no rebind. The new target got a full dump, the old one is dropped.
    let dump = collect(&new_t, Duration::from_millis(500)).await;
    assert!(
        dump.len() > 20,
        "full dump to the added target ({})",
        dump.len()
    );
    let c = sock().await;
    send(&c, r.listen, "/mix/1/ch/2/vol", Some(OscType::Float(0.25))).await;
    let live = collect(&new_t, Q).await;
    assert!(live.iter().any(|m| m.addr == "/mix/1/ch/2/vol"));
    assert!(
        collect(&old_t, Q).await.is_empty(),
        "removed target gets nothing"
    );
    // A target kept across a change is not dumped again.
    let Outcome::Applied(_) = set(&r.engine, settings_of(r.listen, &[new_a])).await else {
        panic!("refused");
    };
    assert!(collect(&new_t, Q).await.is_empty());
}

#[tokio::test]
async fn e_fence_and_syntax_refuse_without_touching_anything() {
    let r = rig("e", vec![]).await;
    let public = "203.0.113.5:9000";
    let mut s = settings_of(r.listen, &[]);
    s.targets = vec![public.into()];
    let Outcome::Refused(msg) = set(&r.engine, s).await else {
        panic!("applied");
    };
    assert!(msg.contains("not on the local network"), "{msg}");
    for bad in [
        OscSettings {
            listen: "0.0.0.0:80".into(),
            targets: vec![],
        },
        OscSettings {
            listen: r.listen.to_string(),
            targets: vec!["127.0.0.1".into()],
        },
        OscSettings {
            listen: r.listen.to_string(),
            targets: (1..=9).map(|i| format!("127.0.0.1:{}", 9000 + i)).collect(),
        },
    ] {
        assert!(matches!(set(&r.engine, bad).await, Outcome::Refused(_)));
    }
    assert_eq!(osc_state(&r.engine).await.0, settings_of(r.listen, &[]));
    assert!(!r.dir.join("osc.json").exists());
}

#[tokio::test]
async fn f_same_port_on_another_address_rebinds() {
    // Wildcard to loopback on the same port: refused by the system while the old socket is open.
    let port = free_port();
    let dir = tmp("f");
    let wild = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let mut cfg = EngineConfig::for_dir(Topology::default(), &dir);
    cfg.osc = settings_of(wild, &[]);
    let (engine, _link, _task) = engine::start(cfg).await;
    let bridge = OscBridge::bind(wild, vec![]).await.unwrap();
    let (_trigger, shutdown): (_, Shutdown) = engine::shutdown_channel();
    tokio::spawn(bridge.run(engine.clone(), shutdown));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let Outcome::Applied(s) = set(&engine, settings_of(lo(port), &[])).await else {
        panic!("refused");
    };
    assert_eq!(s.listen, lo(port).to_string());
    let c = sock().await;
    send(&c, lo(port), "/sync", None).await;
    assert!(!collect(&c, Duration::from_millis(1500)).await.is_empty());
}

#[tokio::test]
async fn g_every_client_sees_the_change_and_it_survives_a_restart() {
    let r = rig("g", vec![]).await;
    let mut other = r.engine.subscribe();
    let new = lo(free_port());
    let t = lo(9100);
    let want = settings_of(new, &[t]);
    let Outcome::Applied(_) = set(&r.engine, want.clone()).await else {
        panic!("refused");
    };
    let env = next(&mut other).await;
    assert_eq!(env.target, Target::All);
    assert!(
        matches!(env.event, ServerEvent::OscSettings { settings, listening: true, .. } if settings == want)
    );
    // A restart reads osc.json: it wins over the config values.
    let net = cuemix_web::config::Network {
        osc_listen: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    let st = settings::start_bridge(&r.dir.join("osc.json"), &net)
        .await
        .unwrap();
    // The first bridge still holds the port; the saved one cannot bind and falls back, loudly.
    assert!(st.error.is_some());
    drop(st);
}

#[tokio::test]
async fn h_failures_are_reported_in_the_state_and_cleared_by_the_next_success() {
    let r = rig("h", vec![]).await;
    let taken = sock().await;
    let Outcome::Refused(_) = set(&r.engine, settings_of(taken.local_addr().unwrap(), &[])).await
    else {
        panic!("applied");
    };
    let (s, listening, error) = osc_state(&r.engine).await;
    assert_eq!(s, settings_of(r.listen, &[]), "nothing changed");
    assert!(listening, "the old socket is still bound");
    assert!(error.unwrap_or_default().contains("cannot listen on"));
    // A refusal by the fence is reported the same way.
    let mut bad = settings_of(r.listen, &[]);
    bad.targets = vec!["203.0.113.5:9000".into()];
    assert!(matches!(set(&r.engine, bad).await, Outcome::Refused(_)));
    assert!(osc_state(&r.engine).await.2.is_some());
    // The next change that works clears it.
    let Outcome::Applied(_) = set(&r.engine, settings_of(r.listen, &[lo(9000)])).await else {
        panic!("refused");
    };
    assert_eq!(osc_state(&r.engine).await.2, None);
}

#[tokio::test]
async fn i_listening_is_false_once_the_bridge_task_has_stopped() {
    let dir = tmp("i");
    let listen = lo(free_port());
    let mut cfg = EngineConfig::for_dir(Topology::default(), &dir);
    cfg.osc = settings_of(listen, &[]);
    let (engine, _link, _task) = engine::start(cfg).await;
    let bridge = OscBridge::bind(listen, vec![]).await.unwrap();
    let (trigger, shutdown): (_, Shutdown) = engine::shutdown_channel();
    let handle = tokio::spawn(bridge.run(engine.clone(), shutdown));
    for _ in 0..100 {
        if osc_state(&engine).await.1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(osc_state(&engine).await.1);
    trigger.trigger();
    handle.await.unwrap().unwrap();
    assert!(!osc_state(&engine).await.1, "no bridge, not listening");
    let Outcome::Refused(m) = set(&engine, settings_of(listen, &[])).await else {
        panic!("applied");
    };
    assert!(m.contains("not running"), "{m}");
}

#[tokio::test]
async fn j_unresolved_targets_are_retried_and_get_a_dump_once_they_resolve() {
    let target = sock().await;
    let port = target.local_addr().unwrap().port();
    let dir = tmp("j");
    let listen = lo(free_port());
    let mut cfg = EngineConfig::for_dir(Topology::default(), &dir);
    // As after a boot where the name did not resolve yet: saved settings kept, error shown,
    // nothing bound as a target.
    cfg.osc = settings_of(listen, &[]);
    cfg.osc.targets = vec![
        format!("localhost:{port}"),
        "no-such-host.invalid:9001".into(),
    ];
    cfg.osc_error = Some("feedback targets not used: ...".into());
    cfg.osc_unresolved = cfg.osc.targets.clone();
    cfg.osc_retry_every = Duration::from_millis(200);
    let (engine, _link, _task) = engine::start(cfg).await;
    let bridge = OscBridge::bind(listen, vec![]).await.unwrap();
    let (_trigger, shutdown): (_, Shutdown) = engine::shutdown_channel();
    tokio::spawn(bridge.run(engine.clone(), shutdown));
    // localhost resolves on the first retry: it gets the dump; the other name keeps being tried.
    let dump = collect(&target, Duration::from_millis(1500)).await;
    assert!(
        dump.len() > 20,
        "dump to the target that resolved ({})",
        dump.len()
    );
    let (s, listening, error) = osc_state(&engine).await;
    assert_eq!(s.listen, listen.to_string(), "settings untouched");
    assert!(listening);
    let e = error.unwrap_or_default();
    assert!(
        e.contains("no-such-host.invalid") && !e.contains("localhost"),
        "{e}"
    );
    assert!(
        !dir.join("osc.json").exists(),
        "a retry changes nothing, so saves nothing"
    );
}
