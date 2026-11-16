//! Integration tests for the Axum server (`/ws`, `/api/health`, embedded UI).

use std::net::SocketAddr;
use std::time::Duration;

use cuemix_web::engine::{self, shutdown_channel, DeviceLink, EngineConfig, ShutdownTrigger};
use cuemix_web::state::Topology;
use cuemix_web::web::server::{serve, WebConfig};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const T: Duration = Duration::from_secs(3);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Harness {
    addr: SocketAddr,
    dir: std::path::PathBuf,
    _link: DeviceLink,
    _trigger: ShutdownTrigger,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn harness(tag: &str) -> Result<Harness, Box<dyn std::error::Error>> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cuemix-web-{tag}-{}-{nanos}", std::process::id()));
    let (handle, link, _task) = engine::start(EngineConfig {
        topology: Topology::default(),
        presets_dir: dir.join("presets"),
        labels_file: dir.join("labels.json"),
    })
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (trigger, shutdown) = shutdown_channel();
    tokio::spawn(serve(
        listener,
        handle,
        WebConfig {
            trust_forwarded_headers: true,
            allowed_origins: vec!["http://mixer.example.com:8443".to_owned()],
            update: Default::default(),
        },
        shutdown,
    ));
    Ok(Harness {
        addr,
        dir,
        _link: link,
        _trigger: trigger,
    })
}

async fn connect(h: &Harness) -> Result<Ws, Box<dyn std::error::Error>> {
    let (ws, _) = tokio::time::timeout(T, connect_async(format!("ws://{}/ws", h.addr))).await??;
    Ok(ws)
}

/// Connect with an optional `Origin` header; returns the handshake status.
async fn connect_with_origin(
    h: &Harness,
    origin: Option<&str>,
) -> Result<u16, Box<dyn std::error::Error>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Error as WsErr;
    let mut req = format!("ws://{}/ws", h.addr).into_client_request()?;
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse()?);
    }
    match tokio::time::timeout(T, connect_async(req)).await? {
        Ok((_, resp)) => Ok(resp.status().as_u16()),
        Err(WsErr::Http(resp)) => Ok(resp.status().as_u16()),
        Err(e) => Err(e.into()),
    }
}

/// Next JSON text frame (pings etc. skipped).
async fn next_json(ws: &mut Ws) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let msg = tokio::time::timeout(T, ws.next())
            .await?
            .ok_or("socket closed")??;
        if let Message::Text(t) = msg {
            return Ok(serde_json::from_str(t.as_ref())?);
        }
    }
}

async fn expect_silence(ws: &mut Ws) -> Result<(), Box<dyn std::error::Error>> {
    match tokio::time::timeout(Duration::from_millis(300), next_json(ws)).await {
        Err(_) => Ok(()),
        Ok(Ok(v)) => Err(format!("unexpected frame: {v}").into()),
        Ok(Err(e)) => Err(e),
    }
}

async fn drain_initial(ws: &mut Ws) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..4 {
        next_json(ws).await?;
    }
    Ok(())
}

struct HttpResp {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

impl HttpResp {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }
}

async fn http_get(addr: SocketAddr, path: &str) -> Result<HttpResp, Box<dyn std::error::Error>> {
    let mut s = TcpStream::connect(addr).await?;
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await?;
    let mut raw = Vec::new();
    tokio::time::timeout(T, s.read_to_end(&mut raw)).await??;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header end")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .ok_or("no status")?
        .parse()?;
    let headers = head.lines().skip(1).collect::<Vec<_>>().join("\n");
    let mut body = raw[split + 4..].to_vec();
    // Connection: close with a chunked body would need decoding; axum sends
    // content-length for these in-memory bodies, so the raw tail is the body.
    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body = Vec::new();
    }
    Ok(HttpResp {
        status,
        headers,
        body,
    })
}

#[tokio::test]
async fn ws_connect_receives_snapshot_labels_presets_in_order(
) -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("order").await?;
    let mut ws = connect(&h).await?;
    assert_eq!(next_json(&mut ws).await?["event"], "snapshot");
    assert_eq!(next_json(&mut ws).await?["event"], "labels");
    assert_eq!(next_json(&mut ws).await?["event"], "presets");
    let m = next_json(&mut ws).await?;
    assert_eq!(m["event"], "measure");
    assert_eq!(m["state"], "unavailable", "no MPD configured here");
    Ok(())
}

#[tokio::test]
async fn noise_wav_streams_with_a_correct_header() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("noise").await?;
    let mut s = TcpStream::connect(h.addr).await?;
    s.write_all(
        b"GET /api/measure/noise.wav HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await?;
    // Read only the start: the body is streamed, no need to take all 17 MB.
    let mut raw = vec![0u8; 4096];
    let mut got = 0;
    while got < 1500 {
        let n = tokio::time::timeout(T, s.read(&mut raw[got..])).await??;
        if n == 0 {
            break;
        }
        got += n;
    }
    let raw = &raw[..got];
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header end")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: audio/wav"), "{head}");
    assert!(head.contains("content-length: 17280044"), "{head}");
    let body = &raw[split + 4..];
    assert_eq!(&body[0..4], b"RIFF");
    assert_eq!(&body[8..12], b"WAVE");
    assert_eq!(&body[36..40], b"data");
    assert!(body.len() > 44, "samples follow the header");
    Ok(())
}

#[tokio::test]
async fn set_is_broadcast_to_others_but_not_echoed() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("set").await?;
    let mut a = connect(&h).await?;
    let mut b = connect(&h).await?;
    drain_initial(&mut a).await?;
    drain_initial(&mut b).await?;

    a.send(Message::text(
        r#"{"cmd":"set","param":"mix_vol","bus":0,"ch":3,"gain":0.5}"#,
    ))
    .await?;
    let ev = next_json(&mut b).await?;
    assert_eq!(ev["event"], "changed");
    assert_eq!(ev["params"][0]["param"], "mix_vol");
    assert_eq!(ev["params"][0]["ch"], 3);
    expect_silence(&mut a).await?;
    Ok(())
}

#[tokio::test]
async fn invalid_json_errors_only_to_sender() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("bad").await?;
    let mut a = connect(&h).await?;
    let mut b = connect(&h).await?;
    drain_initial(&mut a).await?;
    drain_initial(&mut b).await?;

    a.send(Message::text("{not json")).await?;
    let ev = next_json(&mut a).await?;
    assert_eq!(ev["event"], "error");
    assert!(ev["message"].is_string());
    expect_silence(&mut b).await?;
    // Binary frames are ignored.
    a.send(Message::binary(vec![1u8, 2, 3])).await?;
    expect_silence(&mut a).await?;
    Ok(())
}

#[tokio::test]
async fn save_preset_notifies_other_clients() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("preset").await?;
    let mut a = connect(&h).await?;
    let mut b = connect(&h).await?;
    drain_initial(&mut a).await?;
    drain_initial(&mut b).await?;

    a.send(Message::text(r#"{"cmd":"save_preset","name":"base"}"#))
        .await?;
    let s = next_json(&mut b).await?;
    assert_eq!(s["event"], "session");
    assert_eq!(s["active_preset"], "base");
    let p = next_json(&mut b).await?;
    assert_eq!(p["event"], "presets");
    assert_eq!(p["names"][0], "base");
    Ok(())
}

#[tokio::test]
async fn http_static_and_health() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("http").await?;

    let root = http_get(h.addr, "/").await?;
    assert_eq!(root.status, 200);
    assert!(root
        .header("content-type")
        .is_some_and(|c| c.starts_with("text/html")));
    assert_eq!(root.header("cache-control").as_deref(), Some("no-cache"));
    assert!(!root.body.is_empty());

    let spa = http_get(h.addr, "/some/spa/route").await?;
    assert_eq!(spa.status, 200);
    assert_eq!(spa.body, root.body);

    let health = http_get(h.addr, "/api/health").await?;
    assert_eq!(health.status, 200);
    let v: Value = serde_json::from_slice(&health.body)?;
    assert_eq!(v["status"], "ok");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));

    // Find a real asset referenced by index.html.
    let html = String::from_utf8_lossy(&root.body).to_string();
    let start = html
        .find("/assets/")
        .ok_or("no asset reference in index.html")?;
    let rest = &html[start..];
    let end = rest.find('"').ok_or("unterminated asset url")?;
    let asset = http_get(h.addr, &rest[..end]).await?;
    assert_eq!(asset.status, 200);
    let ct = asset.header("content-type").unwrap_or_default();
    assert!(
        ct.contains("javascript") || ct.contains("css"),
        "content-type {ct}"
    );
    assert_eq!(
        asset.header("cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );

    let missing = http_get(h.addr, "/assets/nope.js").await?;
    assert_eq!(missing.status, 404);
    Ok(())
}

#[tokio::test]
async fn path_traversal_never_serves_files() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("trav").await?;
    let index = http_get(h.addr, "/").await?;
    for p in [
        "/../Cargo.toml",
        "/..%2fCargo.toml",
        "/assets/../../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "/..\\Cargo.toml",
    ] {
        let r = http_get(h.addr, p).await?;
        let body = String::from_utf8_lossy(&r.body);
        assert!(!body.contains("[package]"), "{p} leaked a file");
        assert!(
            r.status == 404 || (r.status == 200 && r.body == index.body),
            "{p}: {}",
            r.status
        );
    }
    Ok(())
}

#[tokio::test]
async fn ws_origin_policy() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("origin").await?;
    let same = format!("http://{}", h.addr);
    assert_eq!(connect_with_origin(&h, None).await?, 101);
    assert_eq!(connect_with_origin(&h, Some(&same)).await?, 101);
    assert_eq!(
        connect_with_origin(&h, Some("http://mixer.example.com:8443")).await?,
        101
    );
    assert_eq!(
        connect_with_origin(&h, Some("http://evil.example")).await?,
        403
    );
    assert_eq!(
        connect_with_origin(&h, Some("http://mixer.example.com")).await?,
        403
    );
    assert_eq!(connect_with_origin(&h, Some("null")).await?, 403);
    assert_eq!(
        connect_with_origin(&h, Some("http://127.0.0.1/evil")).await?,
        403
    );
    Ok(())
}
