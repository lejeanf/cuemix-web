//! `motu::client` against the in-process mock MOTU (127.0.0.1 only).
//!
//! The codec below follows the mock's PROVISIONAL kind map and a test-only
//! Param <-> Frame mapping (NOT the real hardware property table).

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cuemix_web::engine::{shutdown_channel, DeviceEvent, DeviceLink, ShutdownTrigger};
use cuemix_web::motu::client::{run_with_codec, ClientConfig, ConnectionStatus, PropCodec};
use cuemix_web::motu::protocol::{Frame, ProtocolError, Value, ValueKind};
use cuemix_web::state::{Param, Topology};
use futures_util::{SinkExt, StreamExt};
use support::{kind_of, MockMotu};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const T: Duration = Duration::from_secs(5);
const PROP_MIX_VOL: u16 = 0x0001; // index = bus * 64 + ch, Fixed
const PROP_MAIN_VOL: u16 = 0x0002; // index 0, Fixed carrying dB (test-only codec)
const PROP_TALKBACK: u16 = 0x2001; // index 0, U8

#[derive(Debug, Clone, Copy)]
struct TestCodec;

impl PropCodec for TestCodec {
    fn kind_of(&self, prop_id: u16) -> Option<ValueKind> {
        (prop_id < 0x4000).then(|| kind_of(prop_id))
    }

    fn encode(&self, param: &Param) -> Result<Vec<Frame>, ProtocolError> {
        Ok(match *param {
            Param::MixVol { bus, ch, gain } => vec![Frame {
                prop_id: PROP_MIX_VOL,
                index: u16::from(bus) * 64 + ch,
                value: Value::Fixed(gain),
            }],
            Param::MainVol { db } => vec![Frame {
                prop_id: PROP_MAIN_VOL,
                index: 0,
                value: Value::Fixed(db),
            }],
            Param::Talkback { on } => vec![Frame {
                prop_id: PROP_TALKBACK,
                index: 0,
                value: Value::U8(u8::from(on)),
            }],
            _ => Vec::new(),
        })
    }

    fn decode(&self, frame: &Frame) -> Option<Param> {
        match (frame.prop_id, &frame.value) {
            (PROP_MIX_VOL, Value::Fixed(v)) => Some(Param::MixVol {
                bus: u8::try_from(frame.index / 64).ok()?,
                ch: frame.index % 64,
                gain: *v,
            }),
            (PROP_MAIN_VOL, Value::Fixed(v)) => Some(Param::MainVol { db: *v }),
            (PROP_TALKBACK, Value::U8(v)) => Some(Param::Talkback { on: *v != 0 }),
            _ => None,
        }
    }
}

struct Harness {
    cmd: mpsc::Sender<Param>,
    events: mpsc::Receiver<DeviceEvent>,
    trigger: ShutdownTrigger,
    task: JoinHandle<()>,
}

fn client_cfg(port: u16) -> ClientConfig {
    ClientConfig {
        explicit_ip: Some("127.0.0.1".parse().expect("ip")),
        port,
        interface: None,
        topology: Topology::default(),
        throttle_interval: Duration::from_millis(25),
        demo_fallback: true,
        sync_window: Duration::from_millis(100),
    }
}

fn start_client(port: u16) -> Harness {
    let (cmd, commands) = mpsc::channel(1024);
    let (events_tx, events) = mpsc::channel(4096);
    let (trigger, shutdown) = shutdown_channel();
    let link = DeviceLink {
        commands,
        events: events_tx,
    };
    let task = tokio::spawn(run_with_codec(client_cfg(port), TestCodec, link, shutdown));
    Harness {
        cmd,
        events,
        trigger,
        task,
    }
}

impl Harness {
    async fn next_event(&mut self) -> DeviceEvent {
        timeout(T, self.events.recv())
            .await
            .expect("event timeout")
            .expect("event channel closed")
    }

    /// Consume events until one satisfies `pred`; returns it.
    async fn wait_for(&mut self, pred: impl Fn(&DeviceEvent) -> bool) -> DeviceEvent {
        loop {
            let ev = self.next_event().await;
            if pred(&ev) {
                return ev;
            }
        }
    }

    async fn stop(self) {
        self.trigger.trigger();
        timeout(T, self.task)
            .await
            .expect("client did not exit on shutdown")
            .expect("client task panicked");
    }
}

fn port_of(mock: &MockMotu) -> u16 {
    mock.url()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("port")
}

fn quantized(v: f64) -> f64 {
    cuemix_web::motu::protocol::from_fixed_8_24(
        cuemix_web::motu::protocol::to_fixed_8_24(v).expect("range"),
    )
}

async fn wait_online(h: &mut Harness) {
    h.wait_for(|e| matches!(e, DeviceEvent::Status(ConnectionStatus::Online)))
        .await;
}

#[tokio::test]
async fn connects_syncs_and_goes_online() {
    let mock = MockMotu::start().await.expect("mock");
    let mut h = start_client(port_of(&mock));
    assert_eq!(
        h.next_event().await,
        DeviceEvent::Status(ConnectionStatus::Connecting)
    );
    assert!(matches!(h.next_event().await, DeviceEvent::Synced(_)));
    assert_eq!(
        h.next_event().await,
        DeviceEvent::Status(ConnectionStatus::Online)
    );
    h.stop().await;
}

#[tokio::test]
async fn engine_param_reaches_the_device() {
    let mock = MockMotu::start().await.expect("mock");
    let mut h = start_client(port_of(&mock));
    wait_online(&mut h).await;
    h.cmd
        .send(Param::MixVol {
            bus: 2,
            ch: 5,
            gain: 0.5,
        })
        .await
        .expect("send");
    h.cmd
        .send(Param::Talkback { on: true })
        .await
        .expect("send");
    let deadline = tokio::time::Instant::now() + T;
    loop {
        let a = mock.get(PROP_MIX_VOL, 2 * 64 + 5).await;
        let b = mock.get(PROP_TALKBACK, 0).await;
        if a == Some(Value::Fixed(quantized(0.5))) && b == Some(Value::U8(1)) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "not delivered");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.stop().await;
}

#[tokio::test]
async fn device_frame_becomes_changed_event() {
    let mock = MockMotu::start().await.expect("mock");
    let mut h = start_client(port_of(&mock));
    wait_online(&mut h).await;

    let (mut other, _) = timeout(T, tokio_tungstenite::connect_async(mock.url()))
        .await
        .expect("timeout")
        .expect("connect");
    let frame = Frame {
        prop_id: PROP_MAIN_VOL,
        index: 0,
        value: Value::Fixed(-6.0),
    };
    other
        .send(Message::Binary(frame.encode().expect("encode")))
        .await
        .expect("send");
    let ev = h.wait_for(|e| matches!(e, DeviceEvent::Changed(_))).await;
    assert_eq!(ev, DeviceEvent::Changed(vec![Param::MainVol { db: -6.0 }]));
    h.stop().await;
}

#[tokio::test]
async fn rapid_fader_updates_are_throttled_and_final_value_wins() {
    let mock = MockMotu::start().await.expect("mock");

    // Observer: counts every valid frame the mock broadcasts.
    let (mut observer, _) = timeout(T, tokio_tungstenite::connect_async(mock.url()))
        .await
        .expect("timeout")
        .expect("connect");
    let seen = Arc::new(AtomicUsize::new(0));
    let seen2 = seen.clone();
    let obs = tokio::spawn(async move {
        while let Some(Ok(msg)) = observer.next().await {
            if matches!(msg, Message::Binary(_)) {
                seen2.fetch_add(1, Ordering::SeqCst);
            }
        }
    });

    let mut h = start_client(port_of(&mock));
    wait_online(&mut h).await;
    for i in 1..=200u32 {
        h.cmd
            .send(Param::MixVol {
                bus: 0,
                ch: 3,
                gain: f64::from(i) / 200.0,
            })
            .await
            .expect("send");
    }
    let deadline = tokio::time::Instant::now() + T;
    loop {
        if mock.get(PROP_MIX_VOL, 3).await == Some(Value::Fixed(quantized(1.0))) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "final value lost");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let n = seen.load(Ordering::SeqCst);
    assert!(
        (1..=40).contains(&n),
        "expected heavy coalescing, got {n} frames"
    );
    obs.abort();
    h.stop().await;
}

#[tokio::test]
async fn unreachable_device_reports_demo() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("addr").port()
    };
    let mut h = start_client(port);
    assert_eq!(
        h.next_event().await,
        DeviceEvent::Status(ConnectionStatus::Connecting)
    );
    assert_eq!(
        h.next_event().await,
        DeviceEvent::Status(ConnectionStatus::Demo)
    );
    // Commands are drained while offline: the engine side never blocks.
    for _ in 0..2000 {
        h.cmd.send(Param::Dim { on: true }).await.expect("drained");
    }
    h.stop().await;
}

#[tokio::test]
async fn reconnects_after_drop_and_survives_garbage_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    let server = tokio::spawn(async move {
        // 1st connection: dropped right after the sync window.
        let (s, _) = listener.accept().await.expect("accept");
        let ws = tokio_tungstenite::accept_async(s).await.expect("ws");
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(ws);
        // 2nd connection: garbage first, then a valid frame.
        let (s, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(s).await.expect("ws");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let valid = Frame {
            prop_id: PROP_MAIN_VOL,
            index: 0,
            value: Value::Fixed(-12.0),
        }
        .encode()
        .expect("encode");
        let mut wrong_len = vec![0, 2, 0, 0, 1, 2];
        wrong_len.push(3);
        for bytes in [
            vec![1u8, 2],
            vec![0x50, 0x00, 0, 0, 0, 0, 0, 0],
            wrong_len,
            vec![],
            valid,
        ] {
            ws.send(Message::Binary(bytes)).await.expect("send");
        }
        let _ = timeout(Duration::from_secs(3), ws.next()).await;
    });

    let mut h = start_client(port);
    wait_online(&mut h).await;
    h.wait_for(|e| matches!(e, DeviceEvent::Status(ConnectionStatus::Demo)))
        .await;
    h.wait_for(|e| matches!(e, DeviceEvent::Status(ConnectionStatus::Online)))
        .await;
    let ev = h.wait_for(|e| matches!(e, DeviceEvent::Changed(_))).await;
    assert_eq!(ev, DeviceEvent::Changed(vec![Param::MainVol { db: -12.0 }]));
    h.stop().await;
    server.abort();
}

#[tokio::test]
async fn offline_retries_do_not_flap_between_demo_and_connecting() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("addr").port()
    };
    let mut h = start_client(port);
    // Spans the first retry (backoff 1 s) and part of the second (2 s).
    tokio::time::sleep(Duration::from_millis(3300)).await;
    let mut statuses = Vec::new();
    while let Ok(ev) = h.events.try_recv() {
        if let DeviceEvent::Status(s) = ev {
            statuses.push(s);
        }
    }
    assert_eq!(
        statuses,
        vec![ConnectionStatus::Connecting, ConnectionStatus::Demo]
    );
    h.stop().await;
}

#[tokio::test]
async fn connect_first_falls_through_failed_candidates() {
    use cuemix_web::motu::client::connect_first;
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("addr")
    };
    let live = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let live_addr = live.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (s, _) = live.accept().await.expect("accept");
        let _ws = tokio_tungstenite::accept_async(s).await.expect("ws");
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    let got = timeout(T, connect_first(&[dead, live_addr]))
        .await
        .expect("timeout");
    assert_eq!(got.map(|(a, _)| a), Some(live_addr));
    assert!(connect_first(&[dead]).await.is_none());
    server.abort();
}

#[tokio::test]
async fn aggregate_pacer_spaces_a_burst_of_distinct_params() {
    use cuemix_web::motu::client::MIN_FRAME_SPACING;
    const N: u16 = 50;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = tokio::spawn(async move {
        let (s, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(s).await.expect("ws");
        let mut got: Vec<(tokio::time::Instant, Vec<u8>)> = Vec::new();
        while got.len() < usize::from(N) {
            match ws.next().await {
                Some(Ok(Message::Binary(b))) => got.push((tokio::time::Instant::now(), b)),
                Some(Ok(_)) => {}
                _ => break,
            }
        }
        got
    });

    let mut h = start_client(port);
    wait_online(&mut h).await;
    for ch in 0..N {
        h.cmd
            .send(Param::MixVol {
                bus: 0,
                ch,
                gain: 0.25,
            })
            .await
            .expect("send");
    }
    let got = timeout(Duration::from_secs(5), server)
        .await
        .expect("frames not delivered in time")
        .expect("server task");
    assert_eq!(got.len(), usize::from(N));
    let total = got[got.len() - 1].0 - got[0].0;
    assert!(
        total >= (MIN_FRAME_SPACING - Duration::from_millis(2)) * u32::from(N - 1),
        "burst not paced: {total:?}"
    );
    // Every channel arrived with its value.
    let mut seen = std::collections::HashSet::new();
    for (_, bytes) in &got {
        let frame = Frame::decode(bytes, |_| ValueKind::Fixed).expect("decode");
        assert_eq!(frame.value, Value::Fixed(quantized(0.25)));
        seen.insert(frame.index);
    }
    assert_eq!(seen.len(), usize::from(N));
    h.stop().await;
}

#[tokio::test]
async fn shutdown_while_waiting_for_backoff_exits_promptly() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("addr").port()
    };
    let mut h = start_client(port);
    h.wait_for(|e| matches!(e, DeviceEvent::Status(ConnectionStatus::Demo)))
        .await;
    h.stop().await;
}
