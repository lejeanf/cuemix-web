//! Configuration: `config.toml` (git-ignored) + environment overrides.
//!
//! Resolution order for the file: `--config <path>` → `$CUEMIX_CONFIG` →
//! `./config.toml` → `/etc/cuemix-web/config.toml`. A missing file is not an
//! error (zero-config start with defaults); an unreadable or invalid one is.
//! Environment overrides (applied last): `CUEMIX_HTTP_BIND`,
//! `CUEMIX_OSC_LISTEN`, `CUEMIX_MOTU_IP`.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::plug::SpeakerPlugSettings;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub network: Network,
    pub motu: Motu,
    pub engine: Engine,
    pub storage: Storage,
    pub web: Web,
    pub update: Update,
    pub measure: Measure,
    /// Defaults of the speaker plug; `speaker_plug_file` (saved from the UI) overrides them.
    pub speaker_plug: SpeakerPlugSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
    pub http_bind: SocketAddr,
    pub osc_listen: SocketAddr,
    pub osc_feedback_targets: Vec<SocketAddr>,
    /// Interface carrying the MOTU link-local subnet; empty = auto-detect.
    pub motu_interface: String,
    pub mdns_hostname: String,
}

impl Default for Network {
    fn default() -> Self {
        Self {
            http_bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            osc_listen: SocketAddr::from(([0, 0, 0, 0], 8000)),
            osc_feedback_targets: Vec::new(),
            motu_interface: String::new(),
            mdns_hostname: "cuemix-web".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Motu {
    /// Explicit device IP; `""` (→ `None`) = auto-discovery.
    #[serde(with = "optional_ip")]
    pub ip: Option<IpAddr>,
    pub port: u16,
}

impl Default for Motu {
    fn default() -> Self {
        Self {
            ip: None,
            port: crate::motu::MOTU_WS_PORT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Option A: on (re)connect the hardware state wins.
    #[default]
    HardwarePriority,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Engine {
    /// Max messages/s per parameter towards the MOTU (40 → 25 ms).
    pub throttle_hz: u32,
    pub sync_mode: SyncMode,
    pub offline_demo_fallback: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            throttle_hz: 40,
            sync_mode: SyncMode::HardwarePriority,
            offline_demo_fallback: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    pub presets_dir: PathBuf,
    pub labels_file: PathBuf,
    /// Remembered Monitor / Phones levels (what "On" fades back to).
    pub power_file: PathBuf,
    /// Speaker plug settings saved from the UI; overrides `[speaker_plug]`.
    pub speaker_plug_file: PathBuf,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            presets_dir: PathBuf::from("/var/lib/cuemix-web/presets"),
            labels_file: PathBuf::from("/var/lib/cuemix-web/labels.json"),
            power_file: PathBuf::from("/var/lib/cuemix-web/power.json"),
            speaker_plug_file: PathBuf::from("/var/lib/cuemix-web/speaker-plug.json"),
        }
    }
}

/// Updates from GitHub releases (see `deploy/cuemix-update.sh`). The web
/// server never downloads or installs anything itself: a root systemd timer
/// writes `status_file`, and the UI's Install button only creates
/// `request_file`, which a root systemd path unit turns into `cuemix-update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Update {
    /// `stable` (published releases) or `prerelease` (newest release,
    /// alphas included). Read by `cuemix-update`.
    pub channel: String,
    /// Written by `cuemix-update --check --status-file` (daily timer).
    pub status_file: PathBuf,
    /// Created by the UI to ask for an install (content ignored).
    pub request_file: PathBuf,
    /// Exit status of the last requested install.
    pub result_file: PathBuf,
}

impl Default for Update {
    fn default() -> Self {
        Self {
            channel: "stable".into(),
            status_file: PathBuf::from("/var/lib/cuemix-web/update-status.json"),
            request_file: PathBuf::from("/var/lib/cuemix-web/update-request"),
            result_file: PathBuf::from("/var/lib/cuemix-web/update-result"),
        }
    }
}

/// Room test: noise played by the user's MPD (see `src/measure`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Measure {
    /// MPD protocol address (`host:port`, plain TCP). Empty = feature disabled.
    pub mpd: String,
    /// MPD password; keep it in `config.toml` only. Never logged.
    pub mpd_password: String,
    /// URL MPD fetches the noise from; empty = this host as seen from MPD.
    pub noise_url: String,
    /// Mixer inputs (0-based) that carry the MPD signal; default Line 7-8.
    pub source_inputs: Vec<u16>,
    /// The test ends and everything is restored after this many seconds.
    pub max_seconds: u64,
}

impl Default for Measure {
    fn default() -> Self {
        Self {
            mpd: String::new(),
            mpd_password: String::new(),
            noise_url: String::new(),
            source_inputs: vec![6, 7],
            max_seconds: 60,
        }
    }
}

impl std::fmt::Debug for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Measure")
            .field("mpd", &self.mpd)
            .field("mpd_password", &"<hidden>")
            .field("noise_url", &self.noise_url)
            .field("source_inputs", &self.source_inputs)
            .field("max_seconds", &self.max_seconds)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Web {
    /// Use `X-Forwarded-For` (rightmost entry, appended by the proxy) for the
    /// client IP in logs. Only enable behind a reverse proxy.
    pub trust_forwarded_headers: bool,
    /// Extra origins allowed to open `/ws` (e.g. `https://mixer.example.com`).
    /// Same-origin requests (`Origin` host = `Host`) are always allowed.
    pub allowed_origins: Vec<String>,
}

/// Keys present in `raw` but absent from the serialised known config.
fn collect_unknown(raw: &toml::Value, known: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    if let (toml::Value::Table(r), toml::Value::Table(k)) = (raw, known) {
        for (key, value) in r {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match k.get(key) {
                Some(known_value) => collect_unknown(value, known_value, &path, out),
                None => out.push(path),
            }
        }
    }
}

mod optional_ip {
    use std::net::IpAddr;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(ip: &Option<IpAddr>, s: S) -> Result<S::Ok, S::Error> {
        match ip {
            Some(ip) => s.collect_str(ip),
            None => s.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<IpAddr>, D::Error> {
        let raw = String::deserialize(d)?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        trimmed.parse().map(Some).map_err(serde::de::Error::custom)
    }
}

impl Config {
    /// Throttle interval derived from `throttle_hz` (0 is treated as 1 Hz).
    pub fn throttle_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / f64::from(self.engine.throttle_hz.max(1)))
    }

    pub fn from_toml_str(text: &str) -> Result<Self> {
        Self::parse_with_warnings(text).map(|(cfg, _)| cfg)
    }

    /// Parse and list keys this version does not know (`section.key`).
    ///
    /// Unknown keys are **warnings, not errors** (D15): a typo is still
    /// reported loudly, but a config written for a newer version does not stop
    /// an older binary from starting (e.g. after `cuemix-update --rollback`).
    pub fn parse_with_warnings(text: &str) -> Result<(Self, Vec<String>)> {
        let raw: toml::Value = toml::from_str(text).context("invalid configuration TOML")?;
        let cfg: Config = raw.clone().try_into().context("invalid configuration")?;
        let known = toml::Value::try_from(&cfg).context("cannot re-serialise configuration")?;
        let mut unknown = Vec::new();
        collect_unknown(&raw, &known, "", &mut unknown);
        Ok((cfg, unknown))
    }

    /// Load from an explicit path, or search the default locations.
    pub fn load(explicit: Option<&Path>) -> Result<(Self, Option<PathBuf>)> {
        let candidates: Vec<PathBuf> = match explicit {
            Some(p) => vec![p.to_path_buf()],
            None => std::env::var_os("CUEMIX_CONFIG")
                .map(PathBuf::from)
                .into_iter()
                .chain([
                    PathBuf::from("config.toml"),
                    PathBuf::from("/etc/cuemix-web/config.toml"),
                ])
                .collect(),
        };
        let mut config = Config::default();
        let mut used = None;
        for path in candidates {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let (cfg, unknown) = Self::parse_with_warnings(&text)
                        .with_context(|| format!("in {}", path.display()))?;
                    for key in unknown {
                        tracing::warn!(%key, path = %path.display(), "unknown configuration key ignored (typo, or written for a newer cuemix-web?)");
                    }
                    config = cfg;
                    used = Some(path);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
            }
        }
        config.apply_env(|k| std::env::var(k).ok())?;
        Ok((config, used))
    }

    /// Apply `CUEMIX_*` overrides from a lookup function (testable).
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<()> {
        if let Some(v) = get("CUEMIX_HTTP_BIND") {
            self.network.http_bind = v.parse().with_context(|| format!("CUEMIX_HTTP_BIND={v}"))?;
        }
        if let Some(v) = get("CUEMIX_OSC_LISTEN") {
            self.network.osc_listen = v
                .parse()
                .with_context(|| format!("CUEMIX_OSC_LISTEN={v}"))?;
        }
        if let Some(v) = get("CUEMIX_MOTU_IP") {
            let t = v.trim();
            self.motu.ip = if t.is_empty() {
                None
            } else {
                Some(t.parse().with_context(|| format!("CUEMIX_MOTU_IP={v}"))?)
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let text = include_str!("../config.example.toml");
        let cfg = Config::from_toml_str(text);
        assert!(cfg.is_ok(), "{cfg:?}");
        let cfg = cfg.unwrap_or_default();
        assert_eq!(cfg.network.http_bind.port(), 8080);
        assert_eq!(cfg.motu.ip, None);
        assert_eq!(cfg.motu.port, 1280);
        assert_eq!(cfg.engine.throttle_hz, 40);
        assert_eq!(
            cfg.throttle_interval(),
            std::time::Duration::from_millis(25)
        );
    }

    #[test]
    fn speaker_plug_section_and_storage_keys() {
        let (cfg, unknown) = Config::parse_with_warnings(
            "[speaker_plug]
enabled = true
kind = \"tasmota\"
host = \"plug.example:8080\"
switch_id = 1
warmup_ms = 3000
             [storage]
power_file = \"/tmp/p.json\"
speaker_plug_file = \"/tmp/s.json\"
",
        )
        .unwrap_or_default();
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(cfg.speaker_plug.enabled);
        assert_eq!(cfg.speaker_plug.kind, crate::plug::PlugKind::Tasmota);
        assert_eq!(cfg.speaker_plug.host, "plug.example:8080");
        assert_eq!(cfg.speaker_plug.switch_id, 1);
        assert_eq!(cfg.speaker_plug.warmup_ms, 3000);
        assert_eq!(
            cfg.speaker_plug.off_delay_ms, 300,
            "other keys keep their defaults"
        );
        assert_eq!(cfg.storage.power_file, PathBuf::from("/tmp/p.json"));
        assert_eq!(cfg.storage.speaker_plug_file, PathBuf::from("/tmp/s.json"));
        // Defaults: no plug, files next to the labels file.
        let d = Config::default();
        assert!(!d.speaker_plug.enabled);
        assert_eq!(d.speaker_plug.warmup_ms, 2000);
        assert_eq!(d.speaker_plug.timeout_ms, 2000);
        assert_eq!(
            d.storage.power_file,
            PathBuf::from("/var/lib/cuemix-web/power.json")
        );
        assert_eq!(
            d.storage.speaker_plug_file,
            PathBuf::from("/var/lib/cuemix-web/speaker-plug.json")
        );
    }

    #[test]
    fn empty_config_is_defaults() {
        assert_eq!(Config::from_toml_str("").ok(), Some(Config::default()));
    }

    #[test]
    fn unknown_keys_are_reported_not_fatal() {
        let text = "[network]\nhttp_bnd = \"0.0.0.0:1\"\n[future]\nflag = true\n[web]\ntrust_forwarded_headers = true";
        let parsed = Config::parse_with_warnings(text);
        assert!(parsed.is_ok(), "{parsed:?}");
        let (cfg, mut unknown) = parsed.unwrap_or_default();
        unknown.sort();
        assert_eq!(
            unknown,
            vec!["future".to_owned(), "network.http_bnd".to_owned()]
        );
        assert!(cfg.web.trust_forwarded_headers, "known keys still apply");
        assert_eq!(
            cfg.network.http_bind.port(),
            8080,
            "a typo falls back to the default"
        );
        // Wrong *types* on known keys are still errors.
        assert!(Config::from_toml_str("[engine]\nthrottle_hz = \"fast\"").is_err());
        // The shipped example has no unknown keys.
        let example = Config::parse_with_warnings(include_str!("../config.example.toml"));
        assert_eq!(example.map(|(_, u)| u).ok(), Some(vec![]));
    }

    #[test]
    fn measure_defaults_and_hidden_password() {
        let cfg = Config::from_toml_str(
            "[measure]
mpd = \"192.0.2.20:6600\"
mpd_password = \"s3cret\"",
        )
        .unwrap_or_default();
        assert_eq!(cfg.measure.mpd, "192.0.2.20:6600");
        assert_eq!(cfg.measure.source_inputs, vec![6, 7]);
        assert_eq!(cfg.measure.max_seconds, 60);
        assert!(!format!("{cfg:?}").contains("s3cret"));
        assert!(Config::default().measure.mpd.is_empty());
    }

    #[test]
    fn env_overrides() {
        let mut cfg = Config::default();
        let env = |k: &str| match k {
            "CUEMIX_HTTP_BIND" => Some("127.0.0.1:9090".to_owned()),
            "CUEMIX_MOTU_IP" => Some("169.254.10.20".to_owned()),
            _ => None,
        };
        assert!(cfg.apply_env(env).is_ok());
        assert_eq!(
            cfg.network.http_bind,
            SocketAddr::from(([127, 0, 0, 1], 9090))
        );
        assert_eq!(cfg.motu.ip, Some(IpAddr::from([169, 254, 10, 20])));
        assert!(cfg.apply_env(|_| Some("not-an-addr".into())).is_err());
    }
}
