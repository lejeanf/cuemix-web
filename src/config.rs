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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub network: Network,
    pub motu: Motu,
    pub engine: Engine,
    pub storage: Storage,
    pub web: Web,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    pub presets_dir: PathBuf,
    pub labels_file: PathBuf,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            presets_dir: PathBuf::from("/var/lib/cuemix-web/presets"),
            labels_file: PathBuf::from("/var/lib/cuemix-web/labels.json"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    pub trust_forwarded_headers: bool,
}

impl Default for Web {
    fn default() -> Self {
        Self {
            trust_forwarded_headers: true,
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
        toml::from_str(text).context("invalid configuration TOML")
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
                    config = Self::from_toml_str(&text)
                        .with_context(|| format!("in {}", path.display()))?;
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
    fn empty_config_is_defaults() {
        assert_eq!(Config::from_toml_str("").ok(), Some(Config::default()));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::from_toml_str("[network]\nhttp_bnd = \"0.0.0.0:1\"").is_err());
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
