//! Speaker plug (docs/IOS-PLAN.md §5.2b): a smart plug that powers the speakers,
//! switched by the engine around Monitor on/off.
//!
//! - [`SpeakerPlugSettings`]: what the UI edits (`speaker-plug.json`, defaults from
//!   `[speaker_plug]` in `config.toml`).
//! - [`build_urls`]: the on/off URLs of each plug type (pure; vectors shared with the web side
//!   in `design/vectors/speaker-plug-urls.json`).
//! - The request itself is a hand-written `GET` over a `TcpStream` ([`http_get`]): plain
//!   `http://` only, no redirect, the answer is read up to its status line and headers (8 KiB at
//!   most). The server is told where to send requests, so every resolved address must be on the
//!   local network ([`is_local_addr`], the *fence*), at save time and again before each send.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Largest answer read (status line + headers).
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024;
/// Highest switch id.
pub const SWITCH_ID_MAX: u8 = 15;
/// Allowed `warmup_ms`.
pub const WARMUP_MS_MAX: u32 = 10_000;
/// Allowed `off_delay_ms`.
pub const OFF_DELAY_MS_MAX: u32 = 5_000;
/// Allowed `timeout_ms`.
pub const TIMEOUT_MS_RANGE: (u32, u32) = (500, 10_000);
/// Longest URL accepted (custom plugs).
pub const MAX_URL_LEN: usize = 2048;
/// Longest host name (RFC 1035).
pub const MAX_HOST_LEN: usize = 253;
/// Longest label of a host name.
const MAX_LABEL_LEN: usize = 63;

/// Plug type (a preset of URLs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlugKind {
    /// Shelly Gen2, Plus, Pro, Gen3, Gen4.
    #[default]
    ShellyRpc,
    ShellyGen1,
    Tasmota,
    /// Two full `http://` URLs.
    Custom,
}

/// Speaker plug settings (JSON contract: IOS-PLAN §5.2b, ARCHITECTURE §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeakerPlugSettings {
    pub enabled: bool,
    pub kind: PlugKind,
    /// IP or hostname, optional `:port`; IPv6 literal in brackets. `""` for `custom`.
    pub host: String,
    pub switch_id: u8,
    /// `custom` only.
    pub on_url: String,
    /// `custom` only.
    pub off_url: String,
    pub warmup_ms: u32,
    pub off_delay_ms: u32,
    pub timeout_ms: u32,
}

impl Default for SpeakerPlugSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: PlugKind::ShellyRpc,
            host: String::new(),
            switch_id: 0,
            on_url: String::new(),
            off_url: String::new(),
            warmup_ms: 2000,
            off_delay_ms: 300,
            timeout_ms: 2000,
        }
    }
}

/// What the engine last learned about the plug (it is never polled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlugState {
    Off,
    On,
    /// Switch-on requested, speakers warming up.
    Warming,
    /// The last request failed.
    Error,
    #[default]
    Unknown,
}

/// The two URLs of a plug (`""` when not buildable).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlugUrls {
    pub on: String,
    pub off: String,
}

impl SpeakerPlugSettings {
    /// Range checks (always applied, enabled or not).
    pub fn validate_ranges(&self) -> Result<(), String> {
        if self.switch_id > SWITCH_ID_MAX {
            return Err(format!("switch id must be 0-{SWITCH_ID_MAX}"));
        }
        if self.warmup_ms > WARMUP_MS_MAX {
            return Err(format!("warm-up must be 0-{WARMUP_MS_MAX} ms"));
        }
        if self.off_delay_ms > OFF_DELAY_MS_MAX {
            return Err(format!("off delay must be 0-{OFF_DELAY_MS_MAX} ms"));
        }
        let (lo, hi) = TIMEOUT_MS_RANGE;
        if !(lo..=hi).contains(&self.timeout_ms) {
            return Err(format!("timeout must be {lo}-{hi} ms"));
        }
        Ok(())
    }

    /// Text fields (always applied, enabled or not): bounded, no control character. Whether they
    /// make working URLs is checked by [`build_urls`] when the plug is enabled.
    pub fn validate_text(&self) -> Result<(), String> {
        for (text, max, what) in [
            (&self.host, MAX_HOST_LEN + 8, "host"),
            (&self.on_url, MAX_URL_LEN, "on URL"),
            (&self.off_url, MAX_URL_LEN, "off URL"),
        ] {
            if text.len() > max {
                return Err(format!("{what} is longer than {max} characters"));
            }
            if text.chars().any(char::is_control) {
                return Err(format!("{what} must not contain control characters"));
            }
        }
        Ok(())
    }

    /// Full validation for saving: text, ranges, and buildable URLs when enabled.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_text()?;
        self.validate_ranges()?;
        if self.enabled {
            build_urls(self)?;
        }
        Ok(())
    }

    /// URLs for the event; empty when not buildable.
    pub fn urls_or_empty(&self) -> PlugUrls {
        build_urls(self).unwrap_or_default()
    }
}

/// A parsed `http://` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Host without the IPv6 brackets.
    pub host: String,
    pub port: u16,
    /// Authority as written (`host[:port]`), used for the `Host` header.
    pub authority: String,
    /// Path and query, starting with `/`.
    pub path: String,
}

/// `host[:port]` with an optional bracketed IPv6 literal: `(host, port, port_was_given)`.
fn parse_authority(a: &str) -> Result<(String, u16, bool), String> {
    if a.is_empty() {
        return Err("host is empty".into());
    }
    if a.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("host must not contain spaces".into());
    }
    if a.len() > MAX_HOST_LEN + 8 {
        return Err("host is too long".into());
    }
    let bad_port = || "port must be 1-65535".to_owned();
    // Digits only: `u16::from_str` would also take a leading `+`.
    let port = |s: &str| -> Result<u16, String> {
        match s.parse::<u16>() {
            Ok(p) if p != 0 && s.bytes().all(|b| b.is_ascii_digit()) => Ok(p),
            _ => Err(bad_port()),
        }
    };
    if let Some(rest) = a.strip_prefix('[') {
        let (addr, tail) = rest.split_once(']').ok_or("missing ] in IPv6 address")?;
        addr.parse::<std::net::Ipv6Addr>()
            .map_err(|_| "invalid IPv6 address".to_owned())?;
        return match tail {
            "" => Ok((addr.to_owned(), 80, false)),
            t => Ok((
                addr.to_owned(),
                port(t.strip_prefix(':').ok_or("bad host")?)?,
                true,
            )),
        };
    }
    let (host, p) = match a.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (a, None),
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        return Err("host must be an IP address or a name, without scheme, path or login".into());
    }
    if !valid_host_name(host) {
        return Err(format!(
            "host name must be at most {MAX_HOST_LEN} characters, in labels of 1-{MAX_LABEL_LEN} that do not start or end with '-'"
        ));
    }
    match p {
        None => Ok((host.to_owned(), 80, false)),
        Some(p) => Ok((host.to_owned(), port(p)?, true)),
    }
}

/// Host name shape (characters were checked): dot-separated labels of 1-63 characters that do
/// not start or end with `-`, 253 characters at most; one trailing dot is allowed.
fn valid_host_name(host: &str) -> bool {
    let name = host.strip_suffix('.').unwrap_or(host);
    name.len() <= MAX_HOST_LEN
        && name.split('.').all(|l| {
            (1..=MAX_LABEL_LEN).contains(&l.len()) && !l.starts_with('-') && !l.ends_with('-')
        })
}

/// Path and query: RFC 3986 unreserved characters, `/ ? & = : , + ;` and well-formed `%XX`.
/// Nothing else ever reaches the request line (no space, `@`, `#`, `\`, control or non-ASCII).
fn valid_path_and_query(tail: &str) -> bool {
    let bytes = tail.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'%' => {
                let hex = |k: usize| bytes.get(i + k).is_some_and(u8::is_ascii_hexdigit);
                if !(hex(1) && hex(2)) {
                    return false;
                }
                i += 3;
                continue;
            }
            b if b.is_ascii_alphanumeric() => {}
            b'-' | b'.' | b'_' | b'~' | b'/' | b'?' | b'&' | b'=' | b':' | b',' | b'+' | b';' => {}
            _ => return false,
        }
        i += 1;
    }
    true
}

/// Parse a plain `http://host[:port][/path][?query]` URL (no login, no fragment). Strict
/// allowlist: this text ends up verbatim in a hand-written request line.
pub fn parse_url(url: &str) -> Result<Target, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("URL must start with http://")?;
    if url.len() > MAX_URL_LEN {
        return Err(format!("URL is longer than {MAX_URL_LEN} characters"));
    }
    if rest
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '#')
    {
        return Err("URL must not contain spaces or a fragment".into());
    }
    if !rest.is_ascii() {
        return Err("URL must be plain ASCII (write other characters as %XX)".into());
    }
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return Err("URL must not contain a login".into());
    }
    let (host, port, _) = parse_authority(authority)?;
    if !valid_path_and_query(tail) {
        return Err(
            "path and query may only contain letters, digits and - . _ ~ / ? & = : , + ; %XX"
                .into(),
        );
    }
    let path = match tail {
        "" => "/".to_owned(),
        t if t.starts_with('?') => format!("/{t}"),
        t => t.to_owned(),
    };
    Ok(Target {
        host,
        port,
        authority: authority.to_owned(),
        path,
    })
}

/// The on/off URLs of a plug (the contract shared with the web side).
pub fn build_urls(s: &SpeakerPlugSettings) -> Result<PlugUrls, String> {
    if s.switch_id > SWITCH_ID_MAX {
        return Err(format!("switch id must be 0-{SWITCH_ID_MAX}"));
    }
    if s.kind == PlugKind::Custom {
        parse_url(&s.on_url).map_err(|e| format!("on URL: {e}"))?;
        parse_url(&s.off_url).map_err(|e| format!("off URL: {e}"))?;
        return Ok(PlugUrls {
            on: s.on_url.clone(),
            off: s.off_url.clone(),
        });
    }
    parse_authority(&s.host)?;
    let (h, n) = (&s.host, s.switch_id);
    Ok(match s.kind {
        PlugKind::ShellyRpc => PlugUrls {
            on: format!("http://{h}/rpc/Switch.Set?id={n}&on=true"),
            off: format!("http://{h}/rpc/Switch.Set?id={n}&on=false"),
        },
        PlugKind::ShellyGen1 => PlugUrls {
            on: format!("http://{h}/relay/{n}?turn=on"),
            off: format!("http://{h}/relay/{n}?turn=off"),
        },
        PlugKind::Tasmota => {
            let power = if n == 0 {
                "Power".to_owned()
            } else {
                format!("Power{}", u16::from(n) + 1)
            };
            PlugUrls {
                on: format!("http://{h}/cm?cmnd={power}%20On"),
                off: format!("http://{h}/cm?cmnd={power}%20Off"),
            }
        }
        PlugKind::Custom => PlugUrls::default(),
    })
}

/// The fence: private (RFC 1918), link-local, loopback and IPv6 unique-local addresses only.
pub fn is_local_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_link_local() || a.is_loopback(),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local_addr(IpAddr::V4(v4));
            }
            let first = a.segments()[0];
            a.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Resolve a host and apply the fence: every address must be local.
pub async fn resolve_fenced(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("cannot resolve {host}"));
    }
    if addrs.iter().any(|a| !is_local_addr(a.ip())) {
        return Err(format!(
            "{host} is not on the local network (only private, link-local and loopback addresses are allowed)"
        ));
    }
    Ok(addrs)
}

/// Fence check of every URL of the settings (what saving does).
pub async fn check_fence(s: &SpeakerPlugSettings) -> Result<(), String> {
    let urls = build_urls(s)?;
    let all = async {
        for url in [&urls.on, &urls.off] {
            let t = parse_url(url)?;
            resolve_fenced(&t.host, t.port).await?;
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_millis(u64::from(s.timeout_ms)), all)
        .await
        .map_err(|_| "name lookup timed out".to_owned())?
}

/// Status code of an HTTP/1.x answer (only its first line is read).
fn parse_status(buf: &[u8]) -> Result<u16, String> {
    let line_end = buf
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("no HTTP status line in the answer")?;
    let line = String::from_utf8_lossy(buf.get(..line_end).unwrap_or_default());
    let mut parts = line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    match (
        version.starts_with("HTTP/1."),
        parts.next().map(str::parse::<u16>),
    ) {
        (true, Some(Ok(code))) if (100..=599).contains(&code) => Ok(code),
        _ => Err("the answer is not HTTP".into()),
    }
}

/// `GET url`: `Ok(status)` for a 2xx answer, an explanation otherwise. Plain HTTP, no redirect,
/// body ignored, fenced, `timeout` for everything (name lookup included).
pub async fn http_get(url: &str, timeout: Duration) -> Result<u16, String> {
    tokio::time::timeout(timeout, http_get_inner(url))
        .await
        .map_err(|_| "no answer (timeout)".to_owned())?
}

async fn http_get_inner(url: &str) -> Result<u16, String> {
    let t = parse_url(url)?;
    // Belt and braces: nothing but visible ASCII may reach the hand-written request.
    if !t
        .path
        .bytes()
        .chain(t.authority.bytes())
        .all(|b| b.is_ascii_graphic())
    {
        return Err("URL contains characters that cannot be sent".into());
    }
    let addrs = resolve_fenced(&t.host, t.port).await?;
    let mut stream = None;
    let mut last = String::new();
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let mut stream = stream.ok_or_else(|| format!("cannot connect: {last}"))?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: cuemix-web/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        t.path,
        t.authority,
        env!("CARGO_PKG_VERSION")
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("cannot send the request: {e}"))?;
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    let mut chunk = [0u8; 1024];
    while buf.len() < MAX_RESPONSE_BYTES && !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("cannot read the answer: {e}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
    buf.truncate(MAX_RESPONSE_BYTES);
    let status = parse_status(&buf)?;
    if (200..300).contains(&status) {
        Ok(status)
    } else if (300..400).contains(&status) {
        Err(format!("HTTP {status} (redirects are not followed)"))
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// Switch the plug: `Ok("HTTP 200")` or the reason it failed.
pub async fn send_switch(settings: &SpeakerPlugSettings, on: bool) -> Result<String, String> {
    let urls = build_urls(settings)?;
    let url = if on { &urls.on } else { &urls.off };
    let status = http_get(url, Duration::from_millis(u64::from(settings.timeout_ms))).await?;
    Ok(format!("HTTP {status}"))
}

/// Boxed future of a plug request.
pub type PlugFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// How the engine reaches the plug (real HTTP, or a fake in tests).
pub trait PlugTransport: Send + Sync + std::fmt::Debug + 'static {
    /// Switch the plug on or off (fence enforced).
    fn send(&self, settings: SpeakerPlugSettings, on: bool) -> PlugFuture<Result<String, String>>;
    /// Fence check done when settings are saved.
    fn check(&self, settings: SpeakerPlugSettings) -> PlugFuture<Result<(), String>>;
}

/// The real transport: plain HTTP, fenced.
#[derive(Debug, Clone, Copy, Default)]
pub struct HttpPlug;

impl PlugTransport for HttpPlug {
    fn send(&self, settings: SpeakerPlugSettings, on: bool) -> PlugFuture<Result<String, String>> {
        Box::pin(async move { send_switch(&settings, on).await })
    }

    fn check(&self, settings: SpeakerPlugSettings) -> PlugFuture<Result<(), String>> {
        Box::pin(async move { check_fence(&settings).await })
    }
}

/// Saved settings (`speaker-plug.json`); `None` when the file does not exist.
pub async fn load_settings(path: &Path) -> Result<Option<SpeakerPlugSettings>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("invalid {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

pub async fn save_settings(path: &Path, settings: &SpeakerPlugSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    crate::presets::write_atomic(path, &serde_json::to_vec_pretty(settings)?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::net::TcpListener;

    const VECTORS: &str = include_str!("../design/vectors/speaker-plug-urls.json");

    fn kind(name: &str) -> PlugKind {
        match name {
            "shelly_rpc" => PlugKind::ShellyRpc,
            "shelly_gen1" => PlugKind::ShellyGen1,
            "tasmota" => PlugKind::Tasmota,
            "custom" => PlugKind::Custom,
            other => panic!("unknown kind {other}"),
        }
    }

    fn settings_of(v: &serde_json::Value) -> SpeakerPlugSettings {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
        SpeakerPlugSettings {
            enabled: true,
            kind: kind(v["kind"].as_str().unwrap()),
            host: s("host"),
            switch_id: u8::try_from(v["switch_id"].as_u64().unwrap()).unwrap(),
            on_url: s("on_url"),
            off_url: s("off_url"),
            ..SpeakerPlugSettings::default()
        }
    }

    #[test]
    fn valid_vectors_build_exactly_those_urls() {
        let doc: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        let valid = doc["valid"].as_array().unwrap();
        assert!(valid.len() >= 7);
        for v in valid {
            let urls = build_urls(&settings_of(v)).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(urls.on, v["on"].as_str().unwrap(), "{v}");
            assert_eq!(urls.off, v["off"].as_str().unwrap(), "{v}");
            // Every built URL is itself a valid request target.
            assert!(
                parse_url(&urls.on).is_ok() && parse_url(&urls.off).is_ok(),
                "{v}"
            );
        }
    }

    #[test]
    fn invalid_vectors_are_rejected() {
        let doc: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        let invalid = doc["invalid"].as_array().unwrap();
        assert!(invalid.len() >= 8);
        for v in invalid {
            let s = settings_of(v);
            assert!(build_urls(&s).is_err(), "{v} must be rejected");
            assert!(s.validate().is_err(), "{v} must be rejected on save");
        }
    }

    #[test]
    fn url_parsing() {
        let t = parse_url("http://plug.example:8080/a/b?x=1").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str()),
            ("plug.example", 8080, "/a/b?x=1")
        );
        assert_eq!(parse_url("http://plug.example").unwrap().path, "/");
        assert_eq!(
            parse_url("http://plug.example?on=1").unwrap().path,
            "/?on=1"
        );
        let v6 = parse_url("http://[2001:db8::1]:81/x").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("2001:db8::1", 81));
        assert_eq!(v6.authority, "[2001:db8::1]:81");
        for bad in [
            "https://plug.example/",
            "http://u:p@plug.example/",
            "http://plug.example:0/",
            "http://plug.example:99999/",
            "http://plug.example/a b",
            "http://plug.example/#x",
            "http://2001:db8::1/",
            "http://[nope]/",
            "http:///x",
            "plug.example",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ranges_are_validated() {
        let ok = SpeakerPlugSettings::default();
        assert!(ok.validate().is_ok(), "disabled and empty is fine");
        let bad = |f: fn(&mut SpeakerPlugSettings)| {
            let mut s = SpeakerPlugSettings::default();
            f(&mut s);
            s.validate_ranges().is_err()
        };
        assert!(bad(|s| s.switch_id = 16));
        assert!(bad(|s| s.warmup_ms = 10_001));
        assert!(bad(|s| s.off_delay_ms = 5_001));
        assert!(bad(|s| s.timeout_ms = 499));
        assert!(bad(|s| s.timeout_ms = 10_001));
        let mut s = SpeakerPlugSettings {
            enabled: true,
            ..SpeakerPlugSettings::default()
        };
        assert!(s.validate().is_err(), "enabled needs a host");
        s.host = "plug.example".into();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn the_fence_allows_only_the_local_network() {
        let v4 = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        // Private addresses are built numerically, never written as dotted text.
        for ok in [
            v4(10, 0, 0, 1),
            v4(10, 255, 255, 254),
            v4(172, 16, 0, 1),
            v4(172, 31, 255, 1),
            v4(192, 168, 0, 1),
            v4(169, 254, 1, 1),
            v4(127, 0, 0, 1),
            v4(127, 9, 9, 9),
        ] {
            assert!(is_local_addr(ok), "{ok}");
        }
        for bad in [
            v4(8, 8, 8, 8),
            v4(172, 15, 0, 1),
            v4(172, 32, 0, 1),
            v4(192, 167, 1, 1),
            v4(192, 0, 2, 10),
            v4(100, 64, 0, 1),
            v4(0, 0, 0, 0),
            v4(169, 253, 1, 1),
        ] {
            assert!(!is_local_addr(bad), "{bad}");
        }
        let v6 = |s: [u16; 8]| {
            IpAddr::V6(Ipv6Addr::new(
                s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
            ))
        };
        assert!(is_local_addr(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_local_addr(v6([0xfc00, 0, 0, 0, 0, 0, 0, 1])));
        assert!(is_local_addr(v6([0xfd12, 0x3456, 0, 0, 0, 0, 0, 1])));
        assert!(is_local_addr(v6([0xfe80, 0, 0, 0, 0, 0, 0, 1])));
        assert!(!is_local_addr(v6([0xfec0, 0, 0, 0, 0, 0, 0, 1])));
        assert!(!is_local_addr(v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 1])));
        assert!(!is_local_addr(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        // IPv4-mapped addresses follow the IPv4 rule.
        assert!(is_local_addr(v6([0, 0, 0, 0, 0, 0xffff, 0x0a00, 0x0001])));
        assert!(!is_local_addr(v6([0, 0, 0, 0, 0, 0xffff, 0x0808, 0x0808])));
    }

    #[tokio::test]
    async fn resolve_refuses_public_and_accepts_loopback() {
        assert!(resolve_fenced("127.0.0.1", 80).await.is_ok());
        assert!(resolve_fenced("::1", 80).await.is_ok());
        let err = resolve_fenced("8.8.8.8", 80).await.unwrap_err();
        assert!(err.contains("not on the local network"), "{err}");
        // Saving is fenced too.
        let s = SpeakerPlugSettings {
            enabled: true,
            host: "8.8.8.8".into(),
            ..SpeakerPlugSettings::default()
        };
        assert!(check_fence(&s).await.is_err());
        let local = SpeakerPlugSettings {
            host: "127.0.0.1:8080".into(),
            ..s
        };
        assert!(check_fence(&local).await.is_ok());
        // Custom URLs: both are checked.
        let custom = SpeakerPlugSettings {
            kind: PlugKind::Custom,
            on_url: "http://127.0.0.1/on".into(),
            off_url: "http://8.8.8.8/off".into(),
            enabled: true,
            ..SpeakerPlugSettings::default()
        };
        assert!(check_fence(&custom).await.is_err());
    }

    /// One-shot HTTP server: answers every connection with `reply` (after `delay`) and sends the
    /// received request text on the channel.
    async fn serve(
        reply: Vec<u8>,
        delay: Duration,
    ) -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let (reply, tx) = (reply.clone(), tx.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let mut got = 0;
                    while !buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf[got..]).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => got += n,
                        }
                    }
                    let _ = tx.send(String::from_utf8_lossy(&buf[..got]).into_owned());
                    tokio::time::sleep(delay).await;
                    let _ = s.write_all(&reply).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        (addr, rx)
    }

    const T: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn http_get_sends_a_plain_get_and_accepts_2xx() {
        let (addr, mut rx) = serve(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
            Duration::ZERO,
        )
        .await;
        let url = format!(
            "http://127.0.0.1:{}/rpc/Switch.Set?id=0&on=true",
            addr.port()
        );
        assert_eq!(http_get(&url, T).await, Ok(200));
        let req = rx.recv().await.unwrap();
        assert!(
            req.starts_with("GET /rpc/Switch.Set?id=0&on=true HTTP/1.1\r\n"),
            "{req}"
        );
        assert!(
            req.contains(&format!("Host: 127.0.0.1:{}\r\n", addr.port())),
            "{req}"
        );
        assert!(req.contains("Connection: close\r\n"), "{req}");
    }

    #[tokio::test]
    async fn http_get_reports_errors() {
        let get = |addr: SocketAddr| format!("http://127.0.0.1:{}/x", addr.port());
        let (addr, _rx) = serve(b"HTTP/1.1 500 Oops\r\n\r\n".to_vec(), Duration::ZERO).await;
        assert_eq!(http_get(&get(addr), T).await, Err("HTTP 500".to_owned()));
        let (addr, _rx) = serve(
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/\r\n\r\n".to_vec(),
            Duration::ZERO,
        )
        .await;
        let err = http_get(&get(addr), T).await.unwrap_err();
        assert!(err.contains("302") && err.contains("redirect"), "{err}");
        let (addr, _rx) = serve(b"\x00\x01garbage with no line".to_vec(), Duration::ZERO).await;
        assert!(http_get(&get(addr), T).await.is_err());
        let (addr, _rx) = serve(b"SSH-2.0-nope\r\n\r\n".to_vec(), Duration::ZERO).await;
        let err = http_get(&get(addr), T).await.unwrap_err();
        assert!(err.contains("not HTTP"), "{err}");
        // Slow plug: the timeout wins.
        let (addr, _rx) = serve(
            b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
            Duration::from_millis(1500),
        )
        .await;
        let err = http_get(&get(addr), Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(err.contains("timeout"), "{err}");
        // Nothing listening.
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        assert!(http_get(&get(addr), T).await.is_err());
        // Fenced: a public address is never contacted.
        let err = http_get("http://8.8.8.8/x", T).await.unwrap_err();
        assert!(err.contains("not on the local network"), "{err}");
    }

    #[tokio::test]
    async fn an_endless_answer_is_capped() {
        let mut long = b"HTTP/1.1 200 OK\r\nX-Pad: ".to_vec();
        long.extend(std::iter::repeat_n(b'a', 64 * 1024));
        let (addr, _rx) = serve(long, Duration::ZERO).await;
        let url = format!("http://127.0.0.1:{}/x", addr.port());
        // Headers never end: only the first 8 KiB are read, the status line is enough.
        assert_eq!(http_get(&url, T).await, Ok(200));
    }

    #[tokio::test]
    async fn send_switch_uses_the_built_url() {
        let (addr, mut rx) =
            serve(b"HTTP/1.0 204 No Content\r\n\r\n".to_vec(), Duration::ZERO).await;
        let s = SpeakerPlugSettings {
            enabled: true,
            kind: PlugKind::Tasmota,
            host: format!("127.0.0.1:{}", addr.port()),
            switch_id: 1,
            ..SpeakerPlugSettings::default()
        };
        assert_eq!(send_switch(&s, false).await, Ok("HTTP 204".to_owned()));
        let req = rx.recv().await.unwrap();
        assert!(
            req.starts_with("GET /cm?cmnd=Power2%20Off HTTP/1.1\r\n"),
            "{req}"
        );
    }

    #[tokio::test]
    async fn settings_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!("cuemix-plug-{}", std::process::id()));
        let path = dir.join("speaker-plug.json");
        assert_eq!(load_settings(&path).await.unwrap(), None);
        let s = SpeakerPlugSettings {
            enabled: true,
            host: "plug.example".into(),
            warmup_ms: 1234,
            ..SpeakerPlugSettings::default()
        };
        save_settings(&path, &s).await.unwrap();
        assert_eq!(load_settings(&path).await.unwrap(), Some(s));
        // Missing fields take their defaults.
        tokio::fs::write(&path, r#"{"enabled":true}"#)
            .await
            .unwrap();
        let got = load_settings(&path).await.unwrap().unwrap();
        assert!(got.enabled && got.warmup_ms == 2000);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[test]
    fn hostile_text_never_makes_a_url() {
        for bad in [
            "http://192.0.2.1/on\r\nHost: evil.example",
            "http://192.0.2.1/on\nX: 1",
            "http://192.0.2.1/on\rX: 1",
            "http://192.0.2.1/on HTTP/1.1\r\n\r\nGET /x",
            "http://192.0.2.1/on\u{85}x",
            "http://192.0.2.1/on\u{2028}x",
            "http://192.0.2.1/on\u{202e}x",
            "http://192.0.2.1/on\u{ff0f}x",
            "http://192.0.2.1\u{ff0f}on",
            "http://192.0.2.1/caf\u{e9}",
            "http://192.0.2.1/on@evil.example",
            "http://192.0.2.1/on\\x",
            "http://192.0.2.1/on%",
            "http://192.0.2.1/on%g0",
            "http://192.0.2.1/on|x",
            "http://192.0.2.1/on`x`",
            "http://192.0.2.1/on{x}",
            "http://192.0.2.1/on\0",
            "http://192.0.2.1:+80/",
            "http://192.0.2.1:8 0/",
            "http://-x.example/",
            "http://x-.example/",
            "http://a..example/",
            "http://plug.example\u{202e}/",
            "http://pl\0ug.example/",
        ] {
            assert!(parse_url(bad).is_err(), "{bad:?}");
        }
        let long_path = format!("http://192.0.2.1/{}", "a".repeat(MAX_URL_LEN));
        assert!(parse_url(&long_path).is_err());
        let long_label = format!("http://{}.example/", "a".repeat(64));
        assert!(parse_url(&long_label).is_err(), "label of 64");
        let huge_name = format!("http://{}/", vec!["a".repeat(60); 5].join("."));
        assert!(parse_url(&huge_name).is_err(), "name above 253");
        // Percent-encoded text is data, not framing: it is allowed and stays encoded.
        let t = parse_url("http://192.0.2.1/cm?cmnd=Power%0D%0AOn").unwrap();
        assert_eq!(t.path, "/cm?cmnd=Power%0D%0AOn");
        assert!(t.path.bytes().all(|b| b.is_ascii_graphic()));
    }

    #[test]
    fn text_fields_are_bounded_even_when_the_plug_is_disabled() {
        let with = |f: fn(&mut SpeakerPlugSettings)| {
            let mut s = SpeakerPlugSettings::default();
            f(&mut s);
            s.validate_text().is_err() && s.validate().is_err()
        };
        assert!(with(|s| s.host = "a\r\nb".into()));
        assert!(with(|s| s.host = "a\0".into()));
        assert!(with(|s| s.on_url = "http://x/\u{7f}".into()));
        assert!(with(|s| s.off_url = "http://x/\u{1b}[2J".into()));
        assert!(with(|s| s.host = "h".repeat(300)));
        assert!(with(|s| s.on_url = format!("http://x/{}", "a".repeat(3000))));
        assert!(SpeakerPlugSettings::default().validate_text().is_ok());
    }

    #[tokio::test]
    async fn a_hostile_url_is_refused_before_any_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/on\r\nX-Evil: 1");
        assert!(http_get(&url, Duration::from_secs(2)).await.is_err());
        let accepted = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(accepted.is_err(), "nothing may reach the plug");
    }
}
