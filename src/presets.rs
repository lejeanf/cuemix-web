//! Server-side persistence: presets (`<presets_dir>/<name>.json`) and
//! `labels.json`. Both live in git-ignored, runtime data directories.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::state::{Labels, MotuState, Preset};

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
        let preset: Preset = serde_json::from_str(&text)
            .with_context(|| format!("invalid preset {}", path.display()))?;
        Ok(Preset {
            name: name.to_owned(),
            ..preset
        })
    }

    pub async fn save(&self, name: &str, state: &MotuState) -> Result<Preset> {
        let path = self.path(name)?;
        let preset = Preset {
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
