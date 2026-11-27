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
    let (handle, link, _task) =
        engine::start(EngineConfig::for_dir(Topology::default(), &dir)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (trigger, shutdown) = shutdown_channel();
    tokio::spawn(serve(
        listener,
        handle,
        WebConfig {
            trust_forwarded_headers: true,
            allowed_origins: vec!["http://mixer.example.com:8443".to_owned()],
            allowed_hosts: vec!["mixer.example.net".to_owned()],
            mdns_hostname: "cuemix-web".to_owned(),
            ..WebConfig::default()
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
    // Snapshot, labels, presets, measure, power, speaker plug, OSC settings.
    for _ in 0..7 {
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
    http_request(addr, "GET", path, &[], None).await
}

async fn http_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<HttpResp, Box<dyn std::error::Error>> {
    let mut s = TcpStream::connect(addr).await?;
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    // `Host: localhost` unless the test names its own.
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        req.push_str("Host: localhost\r\n");
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    req.push_str(body.unwrap_or_default());
    s.write_all(req.as_bytes()).await?;
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
    let p = next_json(&mut ws).await?;
    assert_eq!(p["event"], "power");
    assert_eq!(p["main"]["fading"], Value::Null);
    assert_eq!(p["main"]["plug"], Value::Null, "no plug set up");
    assert_eq!(p["phones"]["plug"], Value::Null);
    let sp = next_json(&mut ws).await?;
    assert_eq!(sp["event"], "speaker_plug");
    assert_eq!(sp["settings"]["enabled"], false);
    assert_eq!(sp["settings"]["kind"], "shelly_rpc");
    assert_eq!(sp["settings"]["warmup_ms"], 2000);
    assert_eq!(sp["urls"]["on"], "");
    assert_eq!(sp["state"], "unknown");
    let osc = next_json(&mut ws).await?;
    assert_eq!(osc["event"], "osc_settings");
    assert_eq!(osc["settings"]["listen"], "0.0.0.0:8000");
    assert_eq!(osc["settings"]["targets"], serde_json::json!([]));
    assert_eq!(osc["listening"], false, "no OSC bridge in this harness");
    assert_eq!(osc["error"], Value::Null);
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
    assert_eq!(v["api"], 1, "the short HTTP API version");

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

#[tokio::test]
async fn api_summary_reports_levels_presets_and_motu_status(
) -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("summary").await?;
    let r = http_get(h.addr, "/api/v1/summary").await?;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    let v: Value = serde_json::from_slice(&r.body)?;
    assert_eq!(v["api"], 1);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    for ch in ["main", "phones"] {
        assert!(v[ch]["db"].is_number(), "{v}");
        assert_eq!(v[ch]["on"], true);
        assert_eq!(v[ch]["fading"], Value::Null);
        assert_eq!(v[ch]["plug"], Value::Null);
    }
    assert_eq!(v["preset"]["active"], Value::Null);
    assert_eq!(v["preset"]["modified"], false);
    assert_eq!(v["presets"], serde_json::json!([]));
    assert_eq!(v["motu"]["status"], "demo");
    assert_eq!(v["motu"]["model"], Value::Null);
    Ok(())
}

async fn post(
    h: &Harness,
    body: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, Value), Box<dyn std::error::Error>> {
    let mut all: Vec<(&str, &str)> = headers.to_vec();
    if !all
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        all.push(("Content-Type", "application/json"));
    }
    let r = http_request(h.addr, "POST", "/api/v1/command", &all, Some(body)).await?;
    let v = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    Ok((r.status, v))
}

#[tokio::test]
async fn api_command_runs_the_allowed_commands_and_broadcasts(
) -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("api-ok").await?;
    let mut ws = connect(&h).await?;
    drain_initial(&mut ws).await?;

    let (status, v) = post(&h, r#"{"cmd":"set","param":"main_vol","db":-12}"#, &[]).await?;
    assert_eq!((status, &v["ok"]), (200, &Value::Bool(true)), "{v}");
    let ev = next_json(&mut ws).await?;
    assert_eq!(ev["event"], "changed");
    assert_eq!(ev["params"][0]["param"], "main_vol");
    assert_eq!(ev["params"][0]["db"], -12.0);
    let s: Value = serde_json::from_slice(&http_get(h.addr, "/api/v1/summary").await?.body)?;
    assert_eq!(s["main"]["db"], -12.0);

    let (status, _) = post(&h, r#"{"cmd":"set","param":"phones_vol","db":-30}"#, &[]).await?;
    assert_eq!(status, 200);
    assert_eq!(next_json(&mut ws).await?["event"], "changed");

    // Power: the fade is visible to every client, step by step, and ends with the power event.
    let (status, v) = post(&h, r#"{"cmd":"power","channel":"main","on":false}"#, &[]).await?;
    assert_eq!((status, &v["ok"]), (200, &Value::Bool(true)));
    let mut saw_fading = false;
    let mut last_db = None;
    loop {
        let ev = next_json(&mut ws).await?;
        match ev["event"].as_str() {
            Some("changed") if ev["params"][0]["param"] == "main_vol" => {
                last_db = ev["params"][0]["db"].as_f64();
            }
            Some("power") if ev["main"]["fading"] == "out" => saw_fading = true,
            Some("power") if ev["main"]["on"] == false && ev["main"]["fading"].is_null() => break,
            _ => {}
        }
    }
    assert!(saw_fading);
    assert_eq!(last_db, Some(-100.0));
    Ok(())
}

#[tokio::test]
async fn api_command_refuses_everything_else_and_reports_failures(
) -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("api-refuse").await?;
    for body in [
        r#"{"cmd":"measure_start"}"#,
        r#"{"cmd":"measure_stop"}"#,
        r#"{"cmd":"set","param":"dim","on":true}"#,
        r#"{"cmd":"set","param":"phantom","ch":0,"on":true}"#,
        r#"{"cmd":"save_preset","name":"x"}"#,
        r#"{"cmd":"list_presets"}"#,
        r#"{"cmd":"test_speaker_plug","on":true}"#,
        r#"{"cmd":"set_speaker_plug","settings":{}}"#,
        r#"{"cmd":"set_osc_settings","settings":{"listen":"0.0.0.0:8000","targets":[]}}"#,
        r#"{"cmd":"set_label","ch":0,"label":null}"#,
    ] {
        let (status, v) = post(&h, body, &[]).await?;
        assert_eq!(status, 400, "{body}");
        assert_eq!(v["ok"], false, "{body}");
        assert!(
            v["error"]
                .as_str()
                .is_some_and(|e| e.contains("not available")),
            "{v}"
        );
    }
    // Not even JSON, or not a command.
    for body in [
        "",
        "{nope",
        r#"{"cmd":"nope"}"#,
        r#"{"cmd":"power","channel":"x","on":true}"#,
    ] {
        let (status, v) = post(&h, body, &[]).await?;
        assert_eq!(status, 400, "{body:?}");
        assert!(
            v["error"]
                .as_str()
                .is_some_and(|e| e.contains("invalid command")),
            "{v}"
        );
    }
    // The real outcome: an unknown preset is a 404 with the reason.
    let (status, v) = post(&h, r#"{"cmd":"load_preset","name":"missing"}"#, &[]).await?;
    assert_eq!(status, 404);
    assert_eq!(v["ok"], false);
    assert!(v["error"].is_string());
    // An invalid name is the caller's mistake too.
    let (status, _) = post(&h, r#"{"cmd":"load_preset","name":"../x"}"#, &[]).await?;
    assert_eq!(status, 400);
    // Too large.
    let big = format!(
        r#"{{"cmd":"load_preset","name":"{}"}}"#,
        "a".repeat(32 * 1024)
    );
    let (status, _) = post(&h, &big, &[]).await?;
    assert_eq!(status, 413);
    Ok(())
}

#[tokio::test]
async fn api_loads_an_existing_preset() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("api-preset").await?;
    let mut ws = connect(&h).await?;
    drain_initial(&mut ws).await?;
    ws.send(Message::text(r#"{"cmd":"save_preset","name":"base"}"#))
        .await?;
    next_json(&mut ws).await?; // session
    next_json(&mut ws).await?; // presets
    let (status, v) = post(&h, r#"{"cmd":"load_preset","name":"base"}"#, &[]).await?;
    assert_eq!((status, &v["ok"]), (200, &Value::Bool(true)), "{v}");
    assert_eq!(next_json(&mut ws).await?["event"], "snapshot");
    let s: Value = serde_json::from_slice(&http_get(h.addr, "/api/v1/summary").await?.body)?;
    assert_eq!(s["preset"]["active"], "base");
    assert_eq!(s["presets"], serde_json::json!(["base"]));
    Ok(())
}

#[tokio::test]
async fn api_applies_the_same_origin_rule_as_ws() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("api-origin").await?;
    let cmd = r#"{"cmd":"set","param":"main_vol","db":-3}"#;
    // No Origin (an app), the same origin, and a configured one: accepted.
    for origin in [
        None,
        Some("http://localhost"),
        Some("http://mixer.example.com:8443"),
    ] {
        let headers: Vec<(&str, &str)> = origin.map(|o| ("Origin", o)).into_iter().collect();
        assert_eq!(post(&h, cmd, &headers).await?.0, 200, "{origin:?}");
        let r = http_request(h.addr, "GET", "/api/v1/summary", &headers, None).await?;
        assert_eq!(r.status, 200, "{origin:?}");
    }
    // A page from another site: refused, and nothing happens.
    for origin in ["http://evil.example", "null", "https://localhost:9999"] {
        let (status, v) = post(
            &h,
            r#"{"cmd":"power","channel":"main","on":false}"#,
            &[("Origin", origin)],
        )
        .await?;
        assert_eq!(status, 403, "{origin}");
        assert_eq!(v["ok"], false);
        let r = http_request(
            h.addr,
            "GET",
            "/api/v1/summary",
            &[("Origin", origin)],
            None,
        )
        .await?;
        assert_eq!(r.status, 403, "{origin}");
    }
    let s: Value = serde_json::from_slice(&http_get(h.addr, "/api/v1/summary").await?.body)?;
    assert_eq!(
        s["main"]["on"], true,
        "the refused power command did nothing"
    );
    assert_eq!(s["main"]["db"], -3.0);
    Ok(())
}

#[tokio::test]
async fn ws_power_and_speaker_plug_commands() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("ws-power").await?;
    let mut a = connect(&h).await?;
    let mut b = connect(&h).await?;
    drain_initial(&mut a).await?;
    drain_initial(&mut b).await?;
    // The sender sees the knob move too (fade steps are server changes).
    a.send(Message::text(
        r#"{"cmd":"power","channel":"phones","on":false}"#,
    ))
    .await?;
    let mut steps_a = 0;
    loop {
        let ev = next_json(&mut a).await?;
        if ev["event"] == "changed" {
            steps_a += 1;
        }
        if ev["event"] == "power" && ev["phones"]["on"] == false && ev["phones"]["fading"].is_null()
        {
            break;
        }
    }
    assert!(steps_a > 3);
    // Settings the fence refuses come back as an error to the sender only.
    a.send(Message::text(
        r#"{"cmd":"set_speaker_plug","settings":{"enabled":true,"kind":"shelly_rpc","host":"8.8.8.8"}}"#,
    ))
    .await?;
    loop {
        let ev = next_json(&mut a).await?;
        if ev["event"] == "error" {
            assert!(
                ev["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("not on the local network")),
                "{ev}"
            );
            break;
        }
    }
    // A local one is saved and everybody gets the event with its URLs.
    a.send(Message::text(
        r#"{"cmd":"set_speaker_plug","settings":{"enabled":true,"kind":"tasmota","host":"127.0.0.1:9","switch_id":1}}"#,
    ))
    .await?;
    loop {
        let ev = next_json(&mut b).await?;
        if ev["event"] == "speaker_plug" && ev["settings"]["enabled"] == true {
            assert_eq!(ev["urls"]["on"], "http://127.0.0.1:9/cm?cmnd=Power2%20On");
            assert_eq!(ev["state"], "unknown");
            break;
        }
    }
    // Test: nothing listens on port 9, so the answer is a clean failure, to the requester only.
    a.send(Message::text(r#"{"cmd":"test_speaker_plug","on":true}"#))
        .await?;
    loop {
        let ev = next_json(&mut a).await?;
        if ev["event"] == "speaker_plug_result" {
            assert_eq!(
                (&ev["on"], &ev["ok"]),
                (&Value::Bool(true), &Value::Bool(false))
            );
            assert!(ev["message"].is_string());
            break;
        }
    }
    Ok(())
}

#[tokio::test]
async fn api_command_refuses_hostile_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("api-hostile").await?;
    let ok = r#"{"cmd":"set","param":"main_vol","db":-3}"#;
    // Wrong or missing content type.
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
    ] {
        assert_eq!(post(&h, ok, &[("Content-Type", ct)]).await?.0, 415, "{ct}");
    }
    let r = http_request(h.addr, "POST", "/api/v1/command", &[], Some(ok)).await?;
    assert_eq!(r.status, 415);
    // Empty body, oversized body, non-JSON, wrong types, hostile preset names.
    assert_eq!(post(&h, "", &[]).await?.0, 400);
    let big = format!(r#"{{"cmd":"list_presets","pad":"{}"}}"#, "x".repeat(20_000));
    assert_eq!(post(&h, &big, &[]).await?.0, 413);
    for body in [
        "not json",
        "[]",
        r#"{"cmd":"set","param":"main_vol","db":"-3"}"#,
        r#"{"cmd":"load_preset","name":"../../etc/passwd"}"#,
        r#"{"cmd":"load_preset","name":"a\u0000b"}"#,
        r#"{"cmd":"load_preset","name":"\u202egnp"}"#,
        r#"{"cmd":"load_preset","name":""}"#,
    ] {
        let (status, v) = post(&h, body, &[]).await?;
        assert_eq!(status, 400, "{body}");
        assert_eq!(v["ok"], false, "{body}");
    }
    // The state did not move.
    let s: Value = serde_json::from_slice(&http_get(h.addr, "/api/v1/summary").await?.body)?;
    assert_ne!(s["main"]["db"], -3.0);
    Ok(())
}

/// DNS rebinding: the attacker's page names its own domain in both `Host` and `Origin`, which
/// match each other. The host guard refuses it on every route before anything runs.
#[tokio::test]
async fn requests_for_an_unknown_host_name_are_refused_everywhere(
) -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("rebind").await?;
    let before: Value = serde_json::from_slice(&http_get(h.addr, "/api/v1/summary").await?.body)?;
    let evil = [
        ("Host", "rebind.attacker.example:8080"),
        ("Origin", "http://rebind.attacker.example:8080"),
    ];
    let set = r#"{"cmd":"set","param":"main_vol","db":-7}"#;
    assert_eq!(post(&h, set, &evil).await?.0, 421);
    for (method, path, body) in [
        ("GET", "/", None),
        ("GET", "/api/health", None),
        ("GET", "/api/v1/summary", None),
        ("GET", "/api/update", None),
        ("POST", "/api/update", Some(r#"{"version":"v9.9.9"}"#)),
        ("GET", "/ws", None),
    ] {
        let mut headers = evil.to_vec();
        headers.push(("x-cuemix-update", "1"));
        headers.push(("Content-Type", "application/json"));
        let r = http_request(h.addr, method, path, &headers, body).await?;
        assert_eq!(r.status, 421, "{method} {path}");
        assert_eq!(r.header("x-frame-options").as_deref(), Some("DENY"));
    }
    // The WebSocket upgrade itself.
    let upgrade = [
        ("Host", "rebind.attacker.example"),
        ("Origin", "http://rebind.attacker.example"),
        ("Connection", "Upgrade"),
        ("Upgrade", "websocket"),
        ("Sec-WebSocket-Version", "13"),
        ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    let r = http_request(h.addr, "GET", "/ws", &upgrade, None).await?;
    assert_eq!(r.status, 421);
    // Behind the trusted proxy, the forwarded host is the one that counts.
    let r = http_request(
        h.addr,
        "GET",
        "/api/health",
        &[("X-Forwarded-Host", "rebind.attacker.example")],
        None,
    )
    .await?;
    assert_eq!(r.status, 421);
    // Nothing moved.
    let s: Value = serde_json::from_slice(&http_get(h.addr, "/api/v1/summary").await?.body)?;
    assert_eq!(s["main"]["db"], before["main"]["db"]);
    assert_ne!(s["main"]["db"], -7.0);

    // Names an outside attacker cannot own are served: IP literals, localhost, .local, the
    // mDNS name, allowed_hosts and the hosts of allowed_origins.
    for host in [
        h.addr.to_string(),
        "127.0.0.1".to_owned(),
        "[::1]:8080".to_owned(),
        "LOCALHOST".to_owned(),
        "cuemix-web.local:8080".to_owned(),
        "studio-pi.local.".to_owned(),
        "cuemix-web".to_owned(),
        "Mixer.Example.NET".to_owned(),
        "mixer.example.com:8443".to_owned(),
    ] {
        let r = http_request(h.addr, "GET", "/api/health", &[("Host", &host)], None).await?;
        assert_eq!(r.status, 200, "{host}");
    }
    for host in ["x.example", "127.0.0.1:99999", "user@127.0.0.1"] {
        let r = http_request(h.addr, "GET", "/api/health", &[("Host", host)], None).await?;
        assert_ne!(r.status, 200, "{host:?}");
    }
    Ok(())
}

#[tokio::test]
async fn every_response_carries_the_security_headers() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("headers").await?;
    for path in [
        "/",
        "/index.html",
        "/api/health",
        "/api/v1/summary",
        "/some/spa/route",
    ] {
        let r = http_get(h.addr, path).await?;
        let csp = r.header("content-security-policy").ok_or("no CSP")?;
        for part in [
            "default-src 'self'",
            "script-src 'self';",
            "frame-ancestors 'none'",
            "object-src 'none'",
            "connect-src 'self' ws://localhost wss://localhost;",
        ] {
            assert!(csp.contains(part), "{path}: {csp}");
        }
        assert_eq!(
            r.header("x-frame-options").as_deref(),
            Some("DENY"),
            "{path}"
        );
        assert_eq!(
            r.header("x-content-type-options").as_deref(),
            Some("nosniff")
        );
        assert_eq!(r.header("referrer-policy").as_deref(), Some("no-referrer"));
    }
    Ok(())
}

/// The update endpoints apply the same cross-site Origin rule as `/ws` and `/api/v1`.
#[tokio::test]
async fn update_endpoints_refuse_cross_site_origins() -> Result<(), Box<dyn std::error::Error>> {
    let h = harness("update-origin").await?;
    for (method, path, body) in [
        ("GET", "/api/update", None),
        ("POST", "/api/update", Some("")),
        (
            "PUT",
            "/api/update/prefs",
            Some(r#"{"show_prerelease":true}"#),
        ),
    ] {
        let headers = [
            ("Origin", "http://evil.example"),
            ("x-cuemix-update", "1"),
            ("Content-Type", "application/json"),
        ];
        let r = http_request(h.addr, method, path, &headers, body).await?;
        assert_eq!(r.status, 403, "{method} {path}");
    }
    Ok(())
}

/// One client address cannot hold more than `MAX_WS_PER_IP` WebSockets; closing one frees a slot.
#[tokio::test]
async fn websockets_are_capped_per_client_address() -> Result<(), Box<dyn std::error::Error>> {
    use cuemix_web::web::server::MAX_WS_PER_IP;
    let h = harness("ws-cap").await?;
    let mut open = Vec::new();
    for _ in 0..MAX_WS_PER_IP {
        open.push(connect(&h).await?);
    }
    assert_eq!(connect_with_origin(&h, None).await?, 503);
    let mut first = open.remove(0);
    first.close(None).await?;
    drop(first);
    // The slot comes back once the server has seen the close.
    let mut status = 0;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        status = connect_with_origin(&h, None).await?;
        if status == 101 {
            break;
        }
    }
    assert_eq!(status, 101);
    Ok(())
}

/// A client that never finishes its request headers is disconnected (slow-header attack).
#[tokio::test]
async fn a_client_that_never_finishes_its_headers_is_dropped(
) -> Result<(), Box<dyn std::error::Error>> {
    use cuemix_web::web::server::HEADER_READ_TIMEOUT;
    let h = harness("slow-header").await?;
    let mut s = TcpStream::connect(h.addr).await?;
    s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .await?;
    let started = std::time::Instant::now();
    let mut buf = [0u8; 256];
    // Closed (0 bytes, or an error answer then close) well before twice the timeout.
    let read = tokio::time::timeout(HEADER_READ_TIMEOUT * 2, async {
        loop {
            match s.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(read.is_ok(), "still open after {:?}", started.elapsed());
    assert!(started.elapsed() >= HEADER_READ_TIMEOUT / 2);
    Ok(())
}
