//! Server-side persistence: presets (`<presets_dir>/<name>.json`) and
//! `labels.json`. Both live in git-ignored, runtime data directories.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::state::{Labels, MotuState, Preset, PRESET_SCHEMA_VERSION};

/// Preset names: 1–64 chars of `[A-Za-z0-9 _.-]`, not starting with `.`
/// (no path separators → no traversal outside `presets_dir`).
pub fn validate_name(name: &str) -> Result<()> {
    let ok_len = (1..=64).contains(&name.chars().count());
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'));
    if !ok_len || !ok_chars || name.starts_with('.') || name.trim() != name {
        bail!("invalid preset name {name:?} (1-64 chars: letters, digits, space, _ - .)");
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PresetStore {
    dir: PathBuf,
}

impl PresetStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir.join(format!("{name}.json")))
    }

    pub async fn load(&self, name: &str) -> Result<Preset> {
        let path = self.path(name)?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("cannot read preset {}", path.display()))?;
        parse_preset(name, &text).with_context(|| format!("invalid preset {}", path.display()))
    }

    pub async fn save(&self, name: &str, state: &MotuState) -> Result<Preset> {
        let path = self.path(name)?;
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: name.to_owned(),
            state: state.clone(),
        };
        tokio::fs::create_dir_all(&self.dir)
            .await
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        write_atomic(&path, &serde_json::to_vec_pretty(&preset)?).await?;
        Ok(preset)
    }

    /// Sorted preset names; a missing directory is an empty list.
    pub async fn list(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut dir = match tokio::fs::read_dir(&self.dir).await {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(names),
            Err(e) => return Err(e).with_context(|| format!("cannot list {}", self.dir.display())),
        };
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if validate_name(stem).is_ok() {
                        names.push(stem.to_owned());
                    }
                }
            }
        }
        names.sort();
        Ok(names)
    }
}

/// Parse a preset file (D15).
///
/// - Older files (lower or missing `schema_version`) load: missing fields take
///   their defaults, unknown fields are ignored, the result is upgraded to the
///   current version in memory (and on disk at the next save).
/// - Files written by a *newer* cuemix-web are refused with a clear message
///   (e.g. after a rollback) instead of being silently misread.
pub fn parse_preset(name: &str, text: &str) -> Result<Preset> {
    let mut raw: serde_json::Value = serde_json::from_str(text)?;
    // Missing = written before versioning (v0, same layout as v1).
    let version = raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version > u64::from(PRESET_SCHEMA_VERSION) {
        bail!(
            "preset {name:?} uses format v{version} but this cuemix-web reads up to v{PRESET_SCHEMA_VERSION}; \
             update cuemix-web (cuemix-update) to load it"
        );
    }
    let old_dsp = version < 2;
    if old_dsp {
        drop_v1_dsp(&mut raw);
    }
    let mut preset: Preset = serde_json::from_value(raw)?;
    if old_dsp {
        // Defaults shaped for the preset's own topology.
        preset.state.reset_dsp();
    }
    Ok(Preset {
        schema_version: PRESET_SCHEMA_VERSION,
        name: name.to_owned(),
        ..preset
    })
}

/// v1 → v2: the per-mixer-input DSP (`channels`) and the old reverb block
/// (with its separate sends) do not match the hardware and cannot be mapped
/// onto it, so they are dropped. Mixes, preamps, trims, control room,
/// routing and clock keep their v1 layout.
fn drop_v1_dsp(raw: &mut serde_json::Value) {
    if let Some(state) = raw
        .get_mut("state")
        .and_then(serde_json::Value::as_object_mut)
    {
        state.remove("channels");
        state.remove("reverb");
    }
}

/// Load `labels.json`; missing file = no labels.
pub async fn load_labels(path: &Path) -> Result<Labels> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("invalid {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Labels::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

pub async fn save_labels(path: &Path, labels: &Labels) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    write_atomic(path, &serde_json::to_vec_pretty(labels)?).await
}

/// Write to `<path>.tmp` then rename, so a crash never leaves a half file.
async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let tmp = path.with_extension("json.tmp");
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .with_context(|| format!("cannot create {}", tmp.display()))?;
    file.write_all(bytes)
        .await
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    // Data on disk before the rename, so power loss never leaves a torn file.
    file.sync_all()
        .await
        .with_context(|| format!("cannot sync {}", tmp.display()))?;
    drop(file);
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ChannelLabel, Param};

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("cuemix-test-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn names() {
        for ok in ["default", "Live Set 2", "a.b_c-d"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "../etc",
            "a/b",
            "a\\b",
            ".hidden",
            " pad",
            "x".repeat(65).as_str(),
            "é",
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn save_load_list() {
        let dir = tmp_dir("presets");
        let store = PresetStore::new(&dir);
        assert_eq!(store.list().await.ok(), Some(vec![]));
        let mut state = MotuState::default();
        let _ = state.apply(&Param::Dim { on: true });
        assert!(store.save("b", &state).await.is_ok());
        assert!(store.save("a", &MotuState::default()).await.is_ok());
        assert_eq!(store.list().await.ok(), Some(vec!["a".into(), "b".into()]));
        let loaded = store.load("b").await.ok();
        assert_eq!(loaded.map(|p| p.state), Some(state));
        assert!(store.load("missing").await.is_err());
        assert!(store.load("../x").await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn labels_round_trip() {
        let dir = tmp_dir("labels");
        let path = dir.join("labels.json");
        assert_eq!(load_labels(&path).await.ok(), Some(Labels::default()));
        let mut labels = Labels::default();
        labels.channels.insert(
            3,
            ChannelLabel {
                name: "Kick".into(),
                color: Some("#EF4444".into()),
            },
        );
        assert!(save_labels(&path, &labels).await.is_ok());
        assert_eq!(load_labels(&path).await.ok(), Some(labels));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}

#[cfg(test)]
mod compat_tests {
    use super::*;
    use crate::state::{Param, Reverb, Topology};

    /// Frozen v1 preset (written by cuemix-web 0.3). Never edit it: it guards
    /// that later versions keep reading presets saved then.
    const V1_FIXTURE: &str = include_str!("../tests/fixtures/preset-v1.json");
    /// Frozen v2 preset (hardware-shaped DSP). Never edit it either.
    const V2_FIXTURE: &str = include_str!("../tests/fixtures/preset-v2.json");

    #[test]
    fn frozen_v1_fixture_still_loads() {
        let p = parse_preset("Live set", V1_FIXTURE);
        assert!(p.is_ok(), "{p:?}");
        let p = p.unwrap_or_else(|_| unreachable!());
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        assert_eq!(p.state.topology, Topology::ULTRALITE_MK5);
        assert!(p.state.control_room.mono);
        assert_eq!(p.state.main.db, -8.0);
        assert_eq!(p.state.mixes[0].channels[0].gain, 0.5, "mixes migrate");
        assert!(p.state.clone().normalized().is_ok());
    }

    #[test]
    fn v1_dsp_and_reverb_are_replaced_by_hardware_defaults() {
        // The v1 fixture has a non-default EQ band (ch 1, band 2) and the old
        // reverb block with 32 sends: neither survives the migration.
        let p = parse_preset("Live set", V1_FIXTURE).unwrap_or_else(|e| panic!("{e}"));
        let fresh = MotuState::new(p.state.topology);
        assert_eq!(p.state.input_eq, fresh.input_eq);
        assert_eq!(p.state.compressors, fresh.compressors);
        assert_eq!(p.state.gates, fresh.gates);
        assert_eq!(p.state.output_eq, fresh.output_eq);
        assert_eq!(p.state.reverb, fresh.reverb);
        assert_eq!(p.state.clock, fresh.clock, "v1 clock has the v2 layout");

        // Same for an old file whose reverb fields overlap the new names.
        let text = r#"{"schema_version":1,"name":"r","state":{
            "reverb":{"enabled":true,"predelay_ms":7.0,"sends":[1.0]},
            "control_room":{"talkback":true}}}"#;
        let p = parse_preset("r", text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(p.state.reverb, Reverb::default());
        assert!(p.state.control_room.talkback);
        assert!(p.state.normalized().is_ok());
    }

    #[test]
    fn v1_migration_follows_the_preset_topology() {
        let text = r#"{"schema_version":1,"name":"t","state":{
            "topology":{"mixes":1,"mixer_inputs":2,"preamps":4,"mic_inputs":1,
                        "routing_inputs":1,"routing_outputs":1},
            "mixes":[{"name":"A","channels":[{},{}]}],
            "preamps":[{},{},{},{}],
            "routing":{"matrix":[[false]]}}}"#;
        let p = parse_preset("t", text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(p.state.input_eq.len(), 4);
        assert_eq!(p.state.compressors.len(), 4);
        assert_eq!(p.state.gates.len(), 1);
        assert!(p.state.normalized().is_ok());
    }

    #[test]
    fn frozen_v2_fixture_loads_unchanged() {
        let p = parse_preset("Live set", V2_FIXTURE).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(p.state.input_eq[1][2].gain_db, 4.0);
        assert_eq!(p.state.compressors[0].makeup, 2.0);
        assert!(!p.state.gates[1].bypass);
        assert_eq!(p.state.reverb.decay, 40.0);
        assert!(p.state.normalized().is_ok());
    }

    #[test]
    fn legacy_file_without_version_or_sections_loads_with_defaults() {
        // As written before schema_version existed, with whole sections and
        // individual fields missing.
        let text = r#"{"name":"old","state":{"control_room":{"dim":true},"main":{}}}"#;
        let p = parse_preset("old", text);
        assert!(p.is_ok(), "{p:?}");
        let p = p.unwrap_or_else(|_| unreachable!());
        assert!(p.state.control_room.dim);
        assert_eq!(p.state.control_room.phones_db, 0.0);
        assert_eq!(
            p.state.mixes.len(),
            7,
            "missing sections come from the default state"
        );
        assert!(p.state.normalized().is_ok());
    }

    #[test]
    fn unknown_fields_from_newer_minor_versions_are_ignored() {
        let text = r#"{"schema_version":1,"name":"n","future_field":42,
                       "state":{"main":{"db":-3,"future_knob":1}}}"#;
        let p = parse_preset("n", text);
        assert!(p.is_ok(), "{p:?}");
        assert_eq!(p.map(|p| p.state.main.db).ok(), Some(-3.0));
    }

    #[test]
    fn newer_major_format_is_refused_clearly() {
        let text = format!(
            r#"{{"schema_version":{},"name":"x","state":{{}}}}"#,
            PRESET_SCHEMA_VERSION + 1
        );
        let err = parse_preset("x", &text)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("update cuemix-web"), "{err}");
    }

    /// Regenerate the fixture only when deliberately introducing a new format
    /// (then add preset-v3.json and keep v1/v2): `cargo test write_v2_fixture -- --ignored`.
    #[test]
    #[ignore]
    fn write_v2_fixture() {
        let mut s = MotuState::default();
        for p in [
            Param::Mono { on: true },
            Param::MainVol { db: -8.0 },
            Param::MixVol {
                bus: 0,
                ch: 0,
                gain: 0.5,
            },
            Param::InputEqGain {
                ch: 1,
                band: 2,
                db: 4.0,
            },
            Param::CompMakeup { ch: 0, gain: 2.0 },
            Param::GateBypass { ch: 1, on: false },
            Param::ReverbDecay { value: 40.0 },
        ] {
            let _ = s.apply(&p);
        }
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: "Live set".into(),
            state: s,
        };
        let json = serde_json::to_string_pretty(&preset).unwrap_or_default();
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/preset-v2.json"),
            json + "\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
}
