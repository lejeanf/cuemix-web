//! The speaker plug sequence over real HTTP on loopback (the engine's own client, not a fake).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cuemix_web::engine::{self, DeviceEvent, EngineConfig, Envelope, Origin};
use cuemix_web::motu::client::ConnectionStatus;
use cuemix_web::plug::{PlugKind, PlugState, SpeakerPlugSettings};
use cuemix_web::power::PowerChannel;
use cuemix_web::state::{Command, Param, ServerEvent, Topology};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// A plug on loopback: records `(request line, when)`, answers 200.
async fn fake_plug() -> (SocketAddr, Arc<Mutex<Vec<(String, Instant)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut got = 0;
                while !buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf[got..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => got += n,
                    }
                }
                let text = String::from_utf8_lossy(&buf[..got]).into_owned();
                let line = text.lines().next().unwrap_or_default().to_owned();
                seen.lock().unwrap().push((line, Instant::now()));
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    (addr, log)
}

async fn until(rx: &mut broadcast::Receiver<Envelope>, pred: impl Fn(&ServerEvent) -> bool) {
    loop {
        let env = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timeout")
            .expect("recv");
        if pred(&env.event) {
            return;
        }
    }
}

#[tokio::test]
async fn the_engine_switches_a_real_plug_around_the_fade() {
    let (addr, log) = fake_plug().await;
    let dir = std::env::temp_dir().join(format!("cuemix-plug-e2e-{}", std::process::id()));
    let mut cfg = EngineConfig::for_dir(Topology::default(), &dir);
    cfg.speaker_plug = SpeakerPlugSettings {
        enabled: true,
        kind: PlugKind::ShellyGen1,
        host: addr.to_string(),
        switch_id: 0,
        warmup_ms: 300,
        off_delay_ms: 100,
        ..SpeakerPlugSettings::default()
    };
    let (h, link, _task) = engine::start(cfg).await;
    let mut rx = h.subscribe();
    // The plug is only switched while the MOTU link is up and its state has been read.
    link.events
        .send(DeviceEvent::Status(ConnectionStatus::Online))
        .await
        .unwrap();
    link.events.send(DeviceEvent::Synced(vec![])).await.unwrap();
    until(&mut rx, |e| matches!(e, ServerEvent::Snapshot { .. })).await;
    let me = Origin::Web(1);

    h.command(me, Command::Set(Param::MainVol { db: -20.0 }))
        .await
        .unwrap();
    h.command(
        me,
        Command::Power {
            channel: PowerChannel::Main,
            on: false,
        },
    )
    .await
    .unwrap();
    until(
        &mut rx,
        |e| matches!(e, ServerEvent::Power { main, .. } if main.plug == Some(PlugState::Off)),
    )
    .await;
    {
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, "GET /relay/0?turn=off HTTP/1.1");
    }
    let off_at = log.lock().unwrap()[0].1;

    h.command(
        me,
        Command::Power {
            channel: PowerChannel::Main,
            on: true,
        },
    )
    .await
    .unwrap();
    until(&mut rx, |e| {
        matches!(e, ServerEvent::Power { main, .. } if main.on && main.fading.is_none() && main.plug == Some(PlugState::On))
    })
    .await;
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[1].0, "GET /relay/0?turn=on HTTP/1.1");
    assert!(log[1].1 > off_at);
    let _ = std::fs::remove_dir_all(dir);
}
