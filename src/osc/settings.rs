//! OSC settings saved from the UI (`osc.json`): where the bridge listens and who gets feedback.
//!
//! They override `[network] osc_listen` / `osc_feedback_targets`. This module is only data,
//! validation and persistence; the engine owns the current value and the bridge applies it
//! (see `engine/osc.rs` and [`super::bridge::OscBridge`]).
//!
//! Validation (the same function guards the UI and the file):
//! - `listen`: `ip:port` or `[v6]:port`; `0.0.0.0`, `::`, a loopback address or an address of
//!   this machine (the bind itself decides the last one); port `1024..=65535`.
//! - `targets`: `0..=8` entries, `ip:port`, `[v6]:port` or `name:port`; port `1..=65535`; every
//!   resolved address must pass the speaker plug's fence (private, link-local, loopback).

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::bridge::OscBridge;
use crate::config::Network;

/// Most feedback targets.
pub const MAX_TARGETS: usize = 8;
/// Lowest port the bridge may listen on (no privileged ports).
pub const MIN_LISTEN_PORT: u16 = 1024;
/// Time allowed for resolving every target name.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
/// Most addresses kept over all targets (a name can resolve to several).
const MAX_RESOLVED: usize = 16;

/// What the UI edits and `osc.json` stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OscSettings {
    /// `host:port` to bind (UDP).
    pub listen: String,
    /// Feedback targets, `host:port` each.
    pub targets: Vec<String>,
}

impl Default for OscSettings {
    fn default() -> Self {
        Self::from_config(&Network::default())
    }
}

/// Settings checked and resolved to socket addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub listen: SocketAddr,
    pub targets: Vec<SocketAddr>,
}

/// One target as written: a host (name or IP literal) and a port.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetSpec {
    host: String,
    port: u16,
}

impl OscSettings {
    /// What `config.toml` says.
    pub fn from_config(net: &Network) -> Self {
        Self {
            listen: net.osc_listen.to_string(),
            targets: net
                .osc_feedback_targets
                .iter()
                .map(SocketAddr::to_string)
                .collect(),
        }
    }

    /// Syntax check without any network access; returns the cleaned-up settings (trimmed,
    /// IP literals in canonical form) and the parsed pieces.
    fn parse(&self) -> Result<(Self, SocketAddr, Vec<TargetSpec>), String> {
        let listen = parse_listen(&self.listen)?;
        if self.targets.len() > MAX_TARGETS {
            return Err(format!("at most {MAX_TARGETS} feedback targets"));
        }
        let mut specs = Vec::with_capacity(self.targets.len());
        let mut clean = Vec::with_capacity(self.targets.len());
        for raw in &self.targets {
            let spec = parse_target(raw)?;
            clean.push(match spec.host.parse::<IpAddr>() {
                Ok(ip) => SocketAddr::new(ip, spec.port).to_string(),
                Err(_) => format!("{}:{}", spec.host, spec.port),
            });
            specs.push(spec);
        }
        Ok((
            Self {
                listen: listen.to_string(),
                targets: clean,
            },
            listen,
            specs,
        ))
    }

    /// Syntax check only (no name lookup): the cleaned-up settings.
    pub fn validate(&self) -> Result<Self, String> {
        self.parse().map(|(clean, _, _)| clean)
    }

    /// Full check: syntax, then every target name resolved and fenced. Returns the cleaned-up
    /// settings and the addresses to use.
    pub async fn resolve(&self) -> Result<(Self, Resolved), String> {
        let (clean, listen, specs) = self.parse()?;
        let mut targets: Vec<SocketAddr> = Vec::new();
        for spec in &specs {
            for a in resolve_spec(listen, spec).await? {
                if !targets.contains(&a) {
                    targets.push(a);
                }
            }
        }
        targets.truncate(MAX_RESOLVED);
        Ok((clean, Resolved { listen, targets }))
    }

    /// Like [`Self::resolve`] for settings that were saved earlier (startup, retries): a target
    /// that does not resolve, or is outside the fence, is left out instead of failing the lot.
    /// Only a syntax error is an error.
    pub async fn resolve_lenient(&self) -> Result<Lenient, String> {
        let (clean, listen, specs) = self.parse()?;
        let mut targets: Vec<SocketAddr> = Vec::new();
        let mut retry = Vec::new();
        let mut problems = Vec::new();
        for spec in &specs {
            match resolve_spec(listen, spec).await {
                Ok(addrs) => {
                    for a in addrs {
                        if !targets.contains(&a) {
                            targets.push(a);
                        }
                    }
                }
                Err(why) => {
                    problems.push(format!("{}:{} ({why})", spec.host, spec.port));
                    // A name may resolve later; an address that is not allowed never will.
                    if spec.host.parse::<IpAddr>().is_err() {
                        retry.push(format!("{}:{}", spec.host, spec.port));
                    }
                }
            }
        }
        targets.truncate(MAX_RESOLVED);
        Ok(Lenient {
            clean,
            resolved: Resolved { listen, targets },
            retry,
            problems,
        })
    }
}

/// Outcome of [`OscSettings::resolve_lenient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lenient {
    pub clean: OscSettings,
    pub resolved: Resolved,
    /// Targets that are names and did not resolve: tried again later.
    pub retry: Vec<String>,
    /// Every target left out, with the reason (for the UI's `error`).
    pub problems: Vec<String>,
}

impl Lenient {
    /// Text for `osc_settings.error`; `None` when nothing was left out.
    pub fn error_text(&self) -> Option<String> {
        if self.problems.is_empty() {
            return None;
        }
        let retry = if self.retry.is_empty() {
            ""
        } else {
            "; names are tried again every minute"
        };
        Some(format!(
            "feedback targets not used: {}{retry}",
            self.problems.join(", ")
        ))
    }
}

/// Can a socket bound to `listen` send to `to`? IPv4 sockets only reach IPv4; `[::]` reaches
/// both (IPv4 as mapped addresses); a specific IPv6 address only IPv6.
fn reachable_from(listen: SocketAddr, to: &SocketAddr) -> bool {
    match listen.ip() {
        IpAddr::V4(_) => to.is_ipv4(),
        IpAddr::V6(ip) if ip.is_unspecified() => true,
        IpAddr::V6(_) => to.is_ipv6(),
    }
}

/// Resolve one target through the fence, keeping the addresses the listener can reach.
async fn resolve_spec(listen: SocketAddr, spec: &TargetSpec) -> Result<Vec<SocketAddr>, String> {
    let addrs = tokio::time::timeout(
        RESOLVE_TIMEOUT,
        crate::plug::resolve_fenced(&spec.host, spec.port),
    )
    .await
    .map_err(|_| format!("cannot resolve {}: lookup timed out", spec.host))??;
    let usable: Vec<SocketAddr> = addrs
        .into_iter()
        .filter(|a| reachable_from(listen, a))
        .collect();
    if usable.is_empty() {
        let family = if listen.is_ipv4() { "IPv4" } else { "IPv6" };
        return Err(format!(
            "{}:{} has no address a {family} listener (listen {listen}) can send to",
            spec.host, spec.port
        ));
    }
    Ok(usable)
}

fn parse_listen(raw: &str) -> Result<SocketAddr, String> {
    let raw = raw.trim();
    let addr: SocketAddr = raw.parse().map_err(|_| {
        format!("listen address {raw:?} must be ip:port (for example 0.0.0.0:8000 or [::]:8000)")
    })?;
    if addr.port() < MIN_LISTEN_PORT {
        return Err(format!(
            "listen port must be {MIN_LISTEN_PORT}-65535 (got {})",
            addr.port()
        ));
    }
    let ip = addr.ip();
    let broadcast = matches!(ip, IpAddr::V4(a) if a.is_broadcast());
    if ip.is_multicast() || broadcast {
        return Err("listen address must be this machine's own address (0.0.0.0, ::, 127.0.0.1, ::1 or one of its addresses)".into());
    }
    Ok(addr)
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

fn parse_target(raw: &str) -> Result<TargetSpec, String> {
    let raw = raw.trim();
    let bad = |why: &str| format!("feedback target {raw:?}: {why}");
    let (host, port) = if let Some(rest) = raw.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| bad("missing ] in [v6]:port"))?;
        let port = tail
            .strip_prefix(':')
            .ok_or_else(|| bad("expected [v6]:port"))?;
        (host, port)
    } else {
        let (host, port) = raw
            .rsplit_once(':')
            .ok_or_else(|| bad("expected host:port"))?;
        if host.contains(':') {
            return Err(bad("an IPv6 address needs brackets: [addr]:port"));
        }
        (host, port)
    };
    let port: u16 = port
        .parse()
        .ok()
        .filter(|p| *p >= 1)
        .ok_or_else(|| bad("port must be 1-65535"))?;
    if host.parse::<IpAddr>().is_err() && !valid_hostname(host) {
        return Err(bad("not an IP address or a host name"));
    }
    Ok(TargetSpec {
        host: host.to_owned(),
        port,
    })
}

/// Saved settings (`osc.json`); `None` when the file does not exist.
pub async fn load(path: &Path) -> Result<Option<OscSettings>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("invalid {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Atomic write (temporary file, then rename).
pub async fn save(path: &Path, settings: &OscSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    crate::presets::write_atomic(path, &serde_json::to_vec_pretty(settings)?).await
}

/// The bridge to run at startup and what the engine reports about it.
#[derive(Debug)]
pub struct Startup {
    pub bridge: OscBridge,
    pub settings: OscSettings,
    /// Why the saved settings were not (all) used (shown in the UI), if so.
    pub error: Option<String>,
    /// Target names that did not resolve yet (the engine retries them).
    pub unresolved: Vec<String>,
}

/// Bind the bridge with the saved settings, or with `config.toml`'s when there are none, when
/// they cannot be read, are invalid or cannot be bound. A target name that does not resolve
/// (the network is often not up yet at boot) does not discard the saved settings: the listen
/// address and the other targets are kept, the reason goes to `error` and the name is retried.
/// Only the `config.toml` values failing to bind is an error (as before 0.7.0).
pub async fn start_bridge(file: &Path, net: &Network) -> Result<Startup> {
    let from_config = OscSettings::from_config(net);
    let mut error = None;
    match load(file).await {
        Ok(None) => {}
        Ok(Some(saved)) => match saved.resolve_lenient().await {
            Ok(l) => match OscBridge::bind(l.resolved.listen, l.resolved.targets.clone()).await {
                Ok(bridge) => {
                    let error = l.error_text();
                    if let Some(e) = &error {
                        tracing::warn!(error = %e, "some saved OSC feedback targets are not in use");
                    }
                    return Ok(Startup {
                        bridge,
                        settings: l.clean,
                        error,
                        unresolved: l.retry,
                    });
                }
                Err(e) => error = Some(format!("saved OSC settings not used: {e:#}")),
            },
            Err(e) => error = Some(format!("saved OSC settings not used: {e}")),
        },
        Err(e) => error = Some(format!("saved OSC settings not used: {e:#}")),
    }
    if let Some(e) = &error {
        tracing::warn!(file = %file.display(), error = %e, "using the OSC settings of config.toml");
    }
    let bridge = OscBridge::bind(net.osc_listen, net.osc_feedback_targets.clone()).await?;
    Ok(Startup {
        bridge,
        settings: from_config,
        error,
        unresolved: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn s(listen: &str, targets: &[&str]) -> OscSettings {
        OscSettings {
            listen: listen.into(),
            targets: targets.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    /// A private LAN address built without writing it as text.
    fn lan(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, last)
    }

    #[test]
    fn defaults_follow_the_config() {
        let d = OscSettings::default();
        assert_eq!(d.listen, "0.0.0.0:8000");
        assert!(d.targets.is_empty());
        let net = Network {
            osc_feedback_targets: vec!["127.0.0.1:9000".parse().unwrap()],
            ..Network::default()
        };
        assert_eq!(
            OscSettings::from_config(&net).targets,
            vec!["127.0.0.1:9000"]
        );
    }

    #[test]
    fn listen_rules() {
        for ok in [
            "0.0.0.0:8000",
            "127.0.0.1:1024",
            "[::]:65535",
            "[::1]:9000",
            " 0.0.0.0:8000 ",
        ] {
            assert!(s(ok, &[]).validate().is_ok(), "{ok}");
        }
        for bad in [
            "0.0.0.0:1023",
            "0.0.0.0:0",
            "0.0.0.0:80",
            "0.0.0.0",
            "localhost:8000",
            ":8000",
            "",
            "0.0.0.0:70000",
            "224.0.0.1:8000",
            "255.255.255.255:8000",
            "[ff02::1]:8000",
        ] {
            assert!(s(bad, &[]).validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn target_rules_and_cleanup() {
        let ok = s(
            "0.0.0.0:8000",
            &[
                &format!("  {}:9000 ", lan(20)),
                "[::1]:9001",
                "ipad.local:9002",
                "127.0.0.1:1",
                "host_1.example:65535",
            ],
        )
        .validate()
        .unwrap();
        assert_eq!(
            ok.targets,
            vec![
                format!("{}:9000", lan(20)),
                "[::1]:9001".to_owned(),
                "ipad.local:9002".to_owned(),
                "127.0.0.1:1".to_owned(),
                "host_1.example:65535".to_owned(),
            ]
        );
        for bad in [
            "127.0.0.1",
            "127.0.0.1:0",
            "127.0.0.1:65536",
            "127.0.0.1:x",
            ":9000",
            "::1:9000",
            "[::1]9000",
            "[::1:9000",
            "bad host:9000",
            "-.a:9000",
            "a..b:9000",
            "http://x:9000",
            "",
        ] {
            assert!(s("0.0.0.0:8000", &[bad]).validate().is_err(), "{bad:?}");
        }
        let nine: Vec<String> = (1..=9).map(|i| format!("127.0.0.1:{}", 9000 + i)).collect();
        let too_many = OscSettings {
            listen: "0.0.0.0:8000".into(),
            targets: nine.clone(),
        };
        assert!(too_many.validate().is_err());
        let eight = OscSettings {
            listen: "0.0.0.0:8000".into(),
            targets: nine[..MAX_TARGETS].to_vec(),
        };
        assert!(eight.validate().is_ok());
    }

    #[tokio::test]
    async fn fence_applies_to_every_target() {
        let local = s(
            "0.0.0.0:8000",
            &[
                &format!("{}:9000", lan(20)),
                "169.254.1.2:9000",
                "127.0.0.1:9000",
                "[::1]:9000",
                "[fe80::1]:9000",
                "[fd00::5]:9000",
            ],
        );
        // IPv6 targets are refused for an IPv4 listener, not by the fence.
        let err = local.resolve().await.unwrap_err();
        assert!(err.contains("IPv4"), "{err}");
        let local_v6 = OscSettings {
            listen: "[::]:8000".into(),
            ..local
        };
        let (_, r) = local_v6.resolve().await.unwrap();
        assert_eq!(r.targets.len(), 6);

        for public in ["203.0.113.5:9000", "[2001:db8::1]:9000"] {
            let err = s("[::]:8000", &["127.0.0.1:9000", public])
                .resolve()
                .await
                .unwrap_err();
            assert!(err.contains("not on the local network"), "{err}");
        }
    }

    #[tokio::test]
    async fn address_families_must_match_the_listener() {
        // IPv4 listener: IPv4 targets only.
        assert!(s("0.0.0.0:8000", &["127.0.0.1:9000"])
            .resolve()
            .await
            .is_ok());
        assert!(s("0.0.0.0:8000", &["[::1]:9000"]).resolve().await.is_err());
        // [::] reaches both.
        let (_, r) = s("[::]:8000", &["127.0.0.1:9000", "[::1]:9000"])
            .resolve()
            .await
            .unwrap();
        assert_eq!(r.targets.len(), 2);
        // A specific IPv6 listener only reaches IPv6.
        assert!(s("[::1]:8000", &["[::1]:9000"]).resolve().await.is_ok());
        let err = s("[::1]:8000", &["127.0.0.1:9000"])
            .resolve()
            .await
            .unwrap_err();
        assert!(err.contains("IPv6 listener"), "{err}");
    }

    #[tokio::test]
    async fn lenient_resolution_keeps_what_works_and_lists_the_rest() {
        let l = s(
            "127.0.0.1:9100",
            &[
                "127.0.0.1:9000",
                "no-such-host.invalid:9001",
                "203.0.113.5:9002",
            ],
        )
        .resolve_lenient()
        .await
        .unwrap();
        assert_eq!(l.resolved.listen, "127.0.0.1:9100".parse().unwrap());
        assert_eq!(l.resolved.targets, vec!["127.0.0.1:9000".parse().unwrap()]);
        // The name is retried; the address outside the fence never is.
        assert_eq!(l.retry, vec!["no-such-host.invalid:9001"]);
        assert_eq!(l.problems.len(), 2);
        let text = l.error_text().unwrap();
        assert!(
            text.contains("no-such-host.invalid") && text.contains("203.0.113.5"),
            "{text}"
        );
        assert!(text.contains("again every minute"));
        // Syntax errors are still errors, and a clean list has no error text.
        assert!(s("0.0.0.0:80", &[]).resolve_lenient().await.is_err());
        assert_eq!(
            s("127.0.0.1:9100", &["127.0.0.1:9000"])
                .resolve_lenient()
                .await
                .unwrap()
                .error_text(),
            None
        );
    }

    #[tokio::test]
    async fn startup_keeps_listen_and_literals_when_a_name_does_not_resolve() {
        let dir = tmp("lenient");
        let file = dir.join("osc.json");
        let net = Network {
            osc_listen: "127.0.0.1:0".parse().unwrap(),
            ..Network::default()
        };
        let free = free_udp_port();
        let saved = s(
            &format!("127.0.0.1:{free}"),
            &["127.0.0.1:9000", "no-such-host.invalid:9001"],
        );
        save(&file, &saved).await.unwrap();
        let st = start_bridge(&file, &net).await.unwrap();
        assert_eq!(
            st.settings, saved,
            "the saved settings are kept as they are"
        );
        assert_eq!(st.bridge.local_addr().unwrap().port(), free);
        assert_eq!(st.unresolved, vec!["no-such-host.invalid:9001"]);
        let e = st.error.unwrap_or_default();
        assert!(e.contains("no-such-host.invalid"), "{e}");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn resolve_dedupes_and_keeps_the_listen_address() {
        let (clean, r) = s("127.0.0.1:9100", &["127.0.0.1:9000", "127.0.0.1:9000"])
            .resolve()
            .await
            .unwrap();
        assert_eq!(clean.listen, "127.0.0.1:9100");
        assert_eq!(r.listen, "127.0.0.1:9100".parse().unwrap());
        assert_eq!(r.targets, vec!["127.0.0.1:9000".parse().unwrap()]);
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

    #[tokio::test]
    async fn save_and_load_roundtrip_atomically() {
        let dir = tmp("rt");
        let file = dir.join("osc.json");
        assert_eq!(load(&file).await.unwrap(), None);
        let v = s("127.0.0.1:9100", &["127.0.0.1:9000"]);
        save(&file, &v).await.unwrap();
        assert_eq!(load(&file).await.unwrap(), Some(v.clone()));
        assert!(!dir.join("osc.json.tmp").exists(), "no temporary file left");
        tokio::fs::write(&file, "not json").await.unwrap();
        assert!(load(&file).await.is_err());
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn startup_prefers_saved_settings_and_falls_back_loudly() {
        let dir = tmp("start");
        let file = dir.join("osc.json");
        let net = Network {
            osc_listen: "127.0.0.1:0".parse().unwrap(),
            ..Network::default()
        };
        // No file: config.toml's values, no error.
        let st = start_bridge(&file, &net).await.unwrap();
        assert_eq!(st.settings, OscSettings::from_config(&net));
        assert!(st.error.is_none());
        drop(st);
        // A saved file wins.
        let free = free_udp_port();
        let saved = s(&format!("127.0.0.1:{free}"), &["127.0.0.1:9000"]);
        save(&file, &saved).await.unwrap();
        let st = start_bridge(&file, &net).await.unwrap();
        assert_eq!(st.settings, saved);
        assert_eq!(st.bridge.local_addr().unwrap().port(), free);
        assert!(st.error.is_none());
        drop(st);
        // Garbage, a failed fence and a failed bind all fall back to the config, with a reason.
        tokio::fs::write(&file, "{").await.unwrap();
        let st = start_bridge(&file, &net).await.unwrap();
        assert_eq!(st.settings, OscSettings::from_config(&net));
        assert!(st.error.is_some());
        drop(st);
        save(&file, &s("127.0.0.1:9100", &["203.0.113.5:9000"]))
            .await
            .unwrap();
        let st = start_bridge(&file, &net).await.unwrap();
        assert!(st.error.unwrap_or_default().contains("not used"));
        let taken = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        save(&file, &s(&format!("127.0.0.1:{port}"), &[]))
            .await
            .unwrap();
        let st = start_bridge(&file, &net).await.unwrap();
        assert!(st.error.is_some());
        assert_eq!(st.settings, OscSettings::from_config(&net));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    /// A UDP port that was free a moment ago on loopback.
    fn free_udp_port() -> u16 {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .and_then(|s| s.local_addr())
            .map(|a| a.port())
            .unwrap_or(40_000)
    }
}
