//! `/api/update`: release status, the pre-release switch and a user-triggered install.
//!
//! Privilege split: this process (unprivileged `cuemix` user) only reads the status written by
//! the root `cuemix-update-check` timer and writes two small files in the state directory:
//! the request file (empty, or one release tag) and `update-prefs.json`. The root
//! `cuemix-update-request.path` unit sees the request and runs `cuemix-update --run-request`,
//! which treats the file as untrusted input (64 bytes, strict tag syntax, never put in a shell
//! command), downloads the release, checks its sha256, swaps the binary, restarts the service
//! and rolls back if it is unhealthy.
//!
//! Choosing a version needs the new root side (`features` contains `pin` in the status file).
//! Older installs keep their old updater until `deploy/proxmox-setup.sh --update` (or the Linux
//! setup script) is run again; the answer then says `version_pinning: false` and a request
//! with a version is refused rather than silently installing something else.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use super::server::{AppState, WebConfig};

/// Header the UI sends with the install request. A plain cross-site form
/// cannot set it, and a page that tries gets a CORS preflight this server
/// never approves.
pub const INSTALL_HEADER: &str = "x-cuemix-update";

/// Largest body of `POST /api/update` and `PUT /api/update/prefs`.
pub(super) const MAX_BODY_BYTES: usize = 1024;
/// Most releases passed on to the UI (the root side writes at most as many).
const MAX_RELEASES: usize = 20;
/// Longest request file / tag the root side reads.
const MAX_TAG_BYTES: usize = 64;
/// Oldest release the UI may ask for (the root side enforces the same floor).
const MIN_PIN_VERSION: &str = "0.5.0";
/// Least time between two accepted install requests.
pub(super) const INSTALL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
/// `features` entry of a status file written by a root updater that reads the request file.
const FEATURE_PIN: &str = "pin";

async fn read_json(path: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&text).ok()
}

/// `cuemix-update` exit code of the last requested install, as text, and the tag that was
/// asked for when there was one.
async fn read_result(path: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let mut parts = text.split_whitespace();
    let code: i32 = parts.next()?.parse().ok()?;
    let at = parts.next().unwrap_or_default();
    let version = parts.next().filter(|t| valid_tag(t));
    let outcome = match code {
        0 => "ok",
        5 => "rolled_back",
        6 => "rollback_failed",
        4 => "busy",
        _ => "failed",
    };
    Some(json!({ "code": code, "outcome": outcome, "at": at, "version": version }))
}

// ------------------------------------------------------------- versions ---

/// `^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$`, at most 64 bytes: the one syntax every side
/// (this server, the root wrapper) accepts for a release tag.
pub(super) fn valid_tag(tag: &str) -> bool {
    if tag.len() > MAX_TAG_BYTES {
        return false;
    }
    let Some(rest) = tag.strip_prefix('v') else {
        return false;
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
        })
}

/// `(major, minor, patch, pre-release identifiers)` of `0.7.0` / `v0.7.0-alpha.1`.
fn parse_version(v: &str) -> Option<(u64, u64, u64, Vec<&str>)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, pre.split('.').collect()),
        None => (v, Vec::new()),
    };
    let mut nums = core.split('.').map(|p| p.parse::<u64>().ok());
    let parsed = (nums.next()??, nums.next()??, nums.next()??);
    if nums.next().is_some() {
        return None;
    }
    Some((parsed.0, parsed.1, parsed.2, pre))
}

/// Semantic-versioning precedence; unparsable versions sort lowest.
fn cmp_versions(a: &str, b: &str) -> Ordering {
    let (Some(a), Some(b)) = (parse_version(a), parse_version(b)) else {
        return parse_version(a).is_some().cmp(&parse_version(b).is_some());
    };
    (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)).then_with(|| {
        match (a.3.is_empty(), b.3.is_empty()) {
            (true, true) => return Ordering::Equal,
            // A release is newer than any of its pre-releases.
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (x, y) in a.3.iter().zip(&b.3) {
            let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                // Numeric identifiers sort below alphanumeric ones.
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        a.3.len().cmp(&b.3.len())
    })
}

/// Spacing between accepted install requests (a double click, or a script, cannot queue a
/// second install the moment the first one has been taken by the root side).
#[derive(Debug)]
pub struct Cooldown {
    last: std::sync::Mutex<Option<std::time::Instant>>,
    period: std::time::Duration,
}

impl Cooldown {
    pub fn new(period: std::time::Duration) -> Self {
        Self {
            last: std::sync::Mutex::new(None),
            period,
        }
    }

    /// Seconds still to wait, if any.
    fn remaining(&self) -> Option<u64> {
        let last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let wait = self
            .period
            .checked_sub(last.as_ref()?.elapsed())
            .filter(|d| !d.is_zero())?;
        Some(wait.as_secs() + 1)
    }

    fn mark(&self) {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
    }
}

/// One published release, as the status file lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub prerelease: bool,
    pub published_at: Option<String>,
}

fn valid_timestamp(t: &str) -> bool {
    t.len() <= 40
        && t.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'T' | b'Z' | b'.' | b'+'))
}

/// What this server reads from the status file, every field validated again here.
#[derive(Debug, Default, PartialEq, Eq)]
struct StatusView {
    releases: Vec<Release>,
    latest_stable: Option<String>,
    latest_pre: Option<String>,
    /// The root updater reads the request file (`features` has `pin`).
    pinning: bool,
    /// Written by an updater that lists releases (older ones only know `latest`).
    lists_releases: bool,
}

fn tag_field(status: &Value, key: &str) -> Option<String> {
    status
        .get(key)?
        .as_str()
        .filter(|t| valid_tag(t))
        .map(str::to_owned)
}

fn view(status: &Value) -> StatusView {
    let releases: Vec<Release> = status
        .get("releases")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|r| {
                    let tag = r.get("tag")?.as_str().filter(|t| valid_tag(t))?;
                    let published_at = r
                        .get("published_at")
                        .and_then(Value::as_str)
                        .filter(|t| valid_timestamp(t))
                        .map(str::to_owned);
                    Some(Release {
                        tag: tag.to_owned(),
                        version: tag.trim_start_matches('v').to_owned(),
                        prerelease: r
                            .get("prerelease")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                            || tag.contains('-'),
                        published_at,
                    })
                })
                .filter(|r| cmp_versions(&r.tag, MIN_PIN_VERSION) != Ordering::Less)
                .take(MAX_RELEASES)
                .collect()
        })
        .unwrap_or_default();
    StatusView {
        lists_releases: status.get("releases").is_some() || status.get("latest_stable").is_some(),
        pinning: status
            .get("features")
            .and_then(Value::as_array)
            .is_some_and(|f| f.iter().any(|x| x.as_str() == Some(FEATURE_PIN))),
        latest_stable: tag_field(status, "latest_stable"),
        latest_pre: tag_field(status, "latest_pre"),
        releases,
    }
}

/// The release the update notice is about: the newest of stable, and stable plus pre-release
/// when `show_prerelease`.
fn candidate(view: &StatusView, show_prerelease: bool) -> Option<&str> {
    let stable = view.latest_stable.as_deref();
    let pre = if show_prerelease {
        view.latest_pre.as_deref()
    } else {
        None
    };
    match (stable, pre) {
        (Some(s), Some(p)) => Some(if cmp_versions(p, s) == Ordering::Greater {
            p
        } else {
            s
        }),
        (s, p) => s.or(p),
    }
}

// ---------------------------------------------------------------- prefs ---

/// Whether the UI lists pre-releases: the saved switch, else `[update] channel`.
async fn show_prerelease(cfg: &WebConfig) -> bool {
    read_json(&cfg.update_prefs_file)
        .await
        .and_then(|v| v.get("show_prerelease").and_then(Value::as_bool))
        // The words cuemix-update accepts for the channel.
        .unwrap_or(matches!(
            cfg.update.channel.as_str(),
            "prerelease" | "pre" | "alpha" | "beta"
        ))
}

/// The tag waiting in the request file, if it holds a valid one.
async fn pending_version(path: &Path) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut buf = Vec::new();
    file.take(MAX_TAG_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .await
        .ok()?;
    let text = String::from_utf8(buf).ok()?;
    let tag = text.trim();
    valid_tag(tag).then(|| tag.to_owned())
}

/// `GET /api/update` body.
pub(super) async fn status_json(cfg: &WebConfig) -> Value {
    let u = &cfg.update;
    let status = read_json(&u.status_file).await;
    let show_pre = show_prerelease(cfg).await;
    let view = status.as_ref().map(view).unwrap_or_default();
    let installed = env!("CARGO_PKG_VERSION");

    // The newest release the notice is about, and whether it is newer than what runs.
    let (latest, update_available) = match candidate(&view, show_pre) {
        Some(tag) => (
            Some(tag.to_owned()),
            cmp_versions(tag, installed) == Ordering::Greater,
        ),
        // An older updater (or no list): what it computed for its own channel.
        None => {
            let tag = status.as_ref().and_then(|s| tag_field(s, "tag"));
            let flag = status
                .as_ref()
                .and_then(|s| s.get("update_available"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            (tag, flag)
        }
    };
    let latest = latest.map(|tag| {
        json!({
            "tag": tag,
            "version": tag.trim_start_matches('v'),
            "prerelease": tag.contains('-'),
        })
    });

    json!({
        "installed": installed,
        "channel": if show_pre { "prerelease" } else { "stable" },
        // No status file: the timer / units are not installed here (dev,
        // Docker), so the UI hides the update controls.
        "enabled": status.is_some(),
        // The root updater installs the version written in the request file.
        "version_pinning": view.pinning,
        "show_prerelease": show_pre,
        "update_available": update_available,
        "latest": latest,
        "latest_stable": view.latest_stable,
        "latest_pre": view.latest_pre,
        "releases": view.releases,
        "status": status,
        "pending": tokio::fs::try_exists(&u.request_file).await.unwrap_or(false),
        "pending_version": pending_version(&u.request_file).await,
        "last_result": read_result(&u.result_file).await,
    })
}

pub(super) async fn status(State(app): State<AppState>) -> Json<Value> {
    Json(status_json(&app.cfg).await)
}

fn refuse(status: StatusCode, message: &str) -> (StatusCode, String) {
    (status, message.to_owned())
}

/// Body of `PUT /api/update/prefs`: `{"show_prerelease":bool}`.
pub(super) async fn save_prefs(cfg: &WebConfig, body: &[u8]) -> (StatusCode, String) {
    let parsed: Result<Value, _> = serde_json::from_slice(body);
    let Some(show) = parsed
        .ok()
        .and_then(|v| v.get("show_prerelease").and_then(Value::as_bool))
    else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "expected {\"show_prerelease\": true|false}",
        );
    };
    let text = json!({ "show_prerelease": show }).to_string();
    if let Some(parent) = cfg.update_prefs_file.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match crate::presets::write_atomic(&cfg.update_prefs_file, text.as_bytes()).await {
        Ok(()) => (StatusCode::OK, text),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "cannot save the update preferences");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot save the update preferences",
            )
        }
    }
}

pub(super) async fn put_prefs(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !headers.contains_key(INSTALL_HEADER) {
        return (StatusCode::FORBIDDEN, "missing x-cuemix-update header").into_response();
    }
    let (code, text) = save_prefs(&app.cfg, &body).await;
    let mut response = (code, text).into_response();
    if code == StatusCode::OK {
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
    }
    response
}

// -------------------------------------------------------------- install ---

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Queue the request atomically: content written to a temporary file, then hard-linked to the
/// request path (which fails when one is already queued), so the root side never sees a
/// half-written request.
async fn queue_request(path: &Path, content: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "update-request".to_owned());
    let tmp: PathBuf = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .await?;
    let written = async {
        file.write_all(content.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::hard_link(&tmp, path).await
    }
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    written
}

/// `POST /api/update`: the status code and message. `body` is empty (newest of the channel,
/// as before 0.7.0) or `{"version":"v0.5.2"}`.
pub(super) async fn queue_install(
    cfg: &WebConfig,
    gate: &Cooldown,
    body: &[u8],
) -> (StatusCode, String) {
    let u = &cfg.update;
    let Some(status) = read_json(&u.status_file).await else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "updates are not set up on this machine (run the setup script's update units)",
        );
    };
    let version: Option<String> = if body.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        let parsed: Result<Value, _> = serde_json::from_slice(body);
        match parsed.ok().filter(Value::is_object) {
            Some(v) => match v.get("version") {
                None | Some(Value::Null) => None,
                Some(Value::String(tag)) => Some(tag.clone()),
                Some(_) => {
                    return refuse(StatusCode::BAD_REQUEST, "\"version\" must be a release tag")
                }
            },
            None => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    "expected {\"version\": \"vX.Y.Z\"} or no body",
                )
            }
        }
    };
    let content = match &version {
        None => String::new(),
        Some(tag) => {
            if !valid_tag(tag) {
                return refuse(StatusCode::BAD_REQUEST, "not a release tag (vX.Y.Z[-pre])");
            }
            let view = view(&status);
            if !view.pinning {
                return refuse(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "this machine's updater cannot install a chosen version yet: run the setup script's --update (deploy/proxmox-setup.sh --update, or deploy/linux-setup.sh) to refresh it",
                );
            }
            if cmp_versions(tag, MIN_PIN_VERSION) == Ordering::Less {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    "releases older than v0.5.0 cannot be installed from the UI",
                );
            }
            if !view.releases.iter().any(|r| &r.tag == tag) {
                return refuse(StatusCode::BAD_REQUEST, "that release is not in the list");
            }
            if cmp_versions(tag, env!("CARGO_PKG_VERSION")) == Ordering::Equal {
                return refuse(StatusCode::BAD_REQUEST, "that version is already installed");
            }
            tag.clone()
        }
    };
    if let Some(secs) = gate.remaining() {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("an install was just requested: try again in {secs} s"),
        );
    }
    match queue_request(&u.request_file, &content).await {
        Ok(()) => {
            gate.mark();
            tracing::info!(version = ?version, "update requested from the web UI");
            (StatusCode::ACCEPTED, "update requested".to_owned())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            refuse(StatusCode::CONFLICT, "an update is already in progress")
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %u.request_file.display(), "cannot create the update request");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot create the update request",
            )
        }
    }
}

pub(super) async fn install(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !headers.contains_key(INSTALL_HEADER) {
        return (StatusCode::FORBIDDEN, "missing x-cuemix-update header").into_response();
    }
    queue_install(&app.cfg, &app.gate, &body)
        .await
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Update;

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("cuemix-upd-{tag}-{}-{nanos}", std::process::id()))
    }

    fn cfg_in(dir: &Path) -> WebConfig {
        WebConfig {
            update: Update {
                channel: "stable".into(),
                status_file: dir.join("update-status.json"),
                request_file: dir.join("update-request"),
                result_file: dir.join("update-result"),
            },
            update_prefs_file: dir.join("update-prefs.json"),
            ..WebConfig::default()
        }
    }

    /// A status file as the new root updater writes it.
    fn status_text(installed: &str, pin: bool) -> String {
        json!({
            "installed": installed,
            "latest": "9.0.0", "tag": "v9.0.0", "channel": "stable",
            "update_available": true,
            "checked_at": "2026-11-25T10:00:00Z",
            "release_url": "https://example.test/r",
            "features": if pin { json!(["pin"]) } else { json!([]) },
            "latest_stable": "v9.0.0",
            "latest_pre": "v9.1.0-alpha.2",
            "releases": [
                {"tag":"v9.1.0-alpha.2","version":"9.1.0-alpha.2","prerelease":true,"published_at":"2026-11-25T09:00:00Z"},
                {"tag":"v9.0.0","version":"9.0.0","prerelease":false,"published_at":"2026-11-24T09:00:00Z"},
                {"tag":"v0.5.1","version":"0.5.1","prerelease":false,"published_at":"2026-11-01T09:00:00Z"},
            ],
        })
        .to_string()
    }

    /// `queue_install` without a cooldown.
    async fn request_install(cfg: &WebConfig, body: &[u8]) -> (StatusCode, String) {
        queue_install(cfg, &Cooldown::new(std::time::Duration::ZERO), body).await
    }

    async fn setup(tag: &str, status: Option<String>) -> (WebConfig, PathBuf) {
        let dir = tmp(tag);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let cfg = cfg_in(&dir);
        if let Some(s) = status {
            tokio::fs::write(&cfg.update.status_file, s).await.unwrap();
        }
        (cfg, dir)
    }

    #[tokio::test]
    async fn result_codes_map_to_outcomes() {
        let dir = tmp("result");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let p = dir.join("r");
        tokio::fs::write(&p, "5 2026-11-15T20:00:00Z\n")
            .await
            .unwrap();
        let v = read_result(&p).await.unwrap();
        assert_eq!(v["outcome"], "rolled_back");
        assert_eq!(v["at"], "2026-11-15T20:00:00Z");
        assert_eq!(v["version"], Value::Null);
        tokio::fs::write(&p, "0 2026-11-15T20:00:00Z v0.5.2\n")
            .await
            .unwrap();
        let v = read_result(&p).await.unwrap();
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["version"], "v0.5.2");
        tokio::fs::write(&p, "0 2026-11-15T20:00:00Z $(reboot)\n")
            .await
            .unwrap();
        assert_eq!(read_result(&p).await.unwrap()["version"], Value::Null);
        tokio::fs::write(&p, "garbage").await.unwrap();
        assert!(read_result(&p).await.is_none());
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[test]
    fn tag_syntax() {
        for ok in [
            "v0.5.2",
            "v10.20.30",
            "v0.7.0-alpha.1",
            "v1.0.0-rc1",
            "v1.2.3-A.b.9",
        ] {
            assert!(valid_tag(ok), "{ok}");
        }
        for bad in [
            "",
            "v",
            "0.5.2",
            "v0.5",
            "v0.5.2.1",
            "v0.5.x",
            "v0.5.2-",
            "v0.5.2-a-b",
            "v0.5.2 ",
            "v0.5.2\n",
            "v0.5.2;reboot",
            "v0.5.2-$(id)",
            "../v0.5.2",
            "v0.5.2-\u{e9}",
            &format!("v1.0.0-{}", "a".repeat(60)),
        ] {
            assert!(!valid_tag(bad), "{bad:?}");
        }
    }

    #[test]
    fn version_precedence() {
        use Ordering::*;
        for (a, b, want) in [
            ("0.7.0", "0.6.0", Greater),
            ("v0.10.0", "0.9.0", Greater),
            ("0.7.0", "0.7.0", Equal),
            ("0.7.0", "0.7.0-alpha.1", Greater),
            ("0.7.0-alpha.2", "0.7.0-alpha.10", Less),
            ("0.7.0-alpha.1", "0.7.0-beta.1", Less),
            ("0.7.0-alpha", "0.7.0-alpha.1", Less),
            ("0.7.0-1", "0.7.0-alpha", Less),
            ("0.6.9", "0.7.0-alpha.1", Less),
        ] {
            assert_eq!(cmp_versions(a, b), want, "{a} vs {b}");
            assert_eq!(cmp_versions(b, a), want.reverse(), "{b} vs {a}");
        }
        assert_eq!(cmp_versions("junk", "0.1.0"), Less);
    }

    #[test]
    fn status_view_validates_every_field() {
        let v = view(&serde_json::from_str(&status_text("0.6.0", true)).unwrap());
        assert!(v.pinning && v.lists_releases);
        assert_eq!(v.releases.len(), 3);
        assert_eq!(v.releases[0].tag, "v9.1.0-alpha.2");
        assert!(v.releases[0].prerelease);
        assert_eq!(v.releases[1].version, "9.0.0");
        assert_eq!(v.latest_stable.as_deref(), Some("v9.0.0"));
        assert_eq!(v.latest_pre.as_deref(), Some("v9.1.0-alpha.2"));

        let hostile = json!({
            "features": ["pin"],
            "latest_stable": "v1.0.0; reboot",
            "latest_pre": 5,
            "releases": [
                {"tag": "v1.0.0", "published_at": "<script>"},
                {"tag": "latest"},
                {"tag": "v2.0.0-x", "prerelease": false},
                "junk",
            ]
        });
        let v = view(&hostile);
        assert_eq!(v.latest_stable, None);
        assert_eq!(v.latest_pre, None);
        assert_eq!(v.releases.len(), 2);
        assert_eq!(v.releases[0].published_at, None);
        assert!(
            v.releases[1].prerelease,
            "a hyphenated tag is a pre-release"
        );

        let many: Vec<Value> = (0..40)
            .map(|i| json!({"tag": format!("v1.0.{i}"), "prerelease": false}))
            .collect();
        assert_eq!(
            view(&json!({"releases": many})).releases.len(),
            MAX_RELEASES
        );

        // The old status file has none of it.
        let old = view(
            &json!({"installed":"0.6.0","latest":"0.7.0","tag":"v0.7.0","update_available":true}),
        );
        assert!(!old.pinning && !old.lists_releases && old.releases.is_empty());
    }

    #[tokio::test]
    async fn get_reports_releases_candidate_and_pinning() {
        let installed = env!("CARGO_PKG_VERSION");
        let (cfg, dir) = setup("get", Some(status_text(installed, true))).await;
        let v = status_json(&cfg).await;
        assert_eq!(v["installed"], installed);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["version_pinning"], true);
        assert_eq!(v["show_prerelease"], false);
        assert_eq!(v["channel"], "stable");
        assert_eq!(v["update_available"], true);
        assert_eq!(v["latest"]["tag"], "v9.0.0");
        assert_eq!(v["releases"].as_array().unwrap().len(), 3);
        assert_eq!(v["latest_stable"], "v9.0.0");
        assert_eq!(v["pending"], false);
        assert_eq!(v["pending_version"], Value::Null);

        // The switch makes the pre-release the one the notice is about.
        let (code, _) = save_prefs(&cfg, br#"{"show_prerelease":true}"#).await;
        assert_eq!(code, StatusCode::OK);
        let v = status_json(&cfg).await;
        assert_eq!(v["show_prerelease"], true);
        assert_eq!(v["channel"], "prerelease");
        assert_eq!(v["latest"]["tag"], "v9.1.0-alpha.2");
        assert_eq!(v["latest"]["prerelease"], true);

        // Nothing newer than what runs: no badge, even when older releases are listed.
        let older = json!({
            "features": ["pin"], "latest_stable": "v0.5.1", "latest_pre": "v0.5.2-alpha.1",
            "releases": [{"tag": "v0.5.1", "prerelease": false}],
        });
        tokio::fs::write(&cfg.update.status_file, older.to_string())
            .await
            .unwrap();
        let v = status_json(&cfg).await;
        assert_eq!(v["update_available"], false);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn old_root_side_reports_no_pinning_and_keeps_its_own_verdict() {
        let old = json!({
            "installed": "0.6.0", "latest": "0.7.0", "tag": "v0.7.0",
            "channel": "stable", "update_available": true,
            "checked_at": "2026-11-25T10:00:00Z"
        })
        .to_string();
        let (cfg, dir) = setup("old", Some(old)).await;
        let v = status_json(&cfg).await;
        assert_eq!(v["version_pinning"], false);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["update_available"], true);
        assert_eq!(v["latest"]["tag"], "v0.7.0");
        assert_eq!(v["releases"], json!([]));
        // Asking for a version is refused, not turned into "newest".
        let (code, msg) = request_install(&cfg, br#"{"version":"v0.5.2"}"#).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{msg}");
        // No status at all: nothing is set up.
        let _ = tokio::fs::remove_file(&cfg.update.status_file).await;
        let v = status_json(&cfg).await;
        assert_eq!(v["enabled"], false);
        assert_eq!(v["version_pinning"], false);
        let (code, _) = request_install(&cfg, b"").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn a_new_status_without_the_pin_marker_refuses_a_version() {
        let (cfg, dir) = setup("nopin", Some(status_text("0.6.0", false))).await;
        let (code, msg) = request_install(&cfg, br#"{"version":"v0.5.1"}"#).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{msg}");
        assert!(!cfg.update.request_file.exists());
        // A plain request still works (the old root side installs the newest).
        let (code, _) = request_install(&cfg, b"").await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn install_body_is_validated_against_the_list() {
        let (cfg, dir) = setup("body", Some(status_text("0.6.0", true))).await;
        for (body, want) in [
            (&br#"{"version":"v7.7.7"}"#[..], StatusCode::BAD_REQUEST),
            (br#"{"version":"v0.5.1; reboot"}"#, StatusCode::BAD_REQUEST),
            (br#"{"version":"0.5.1"}"#, StatusCode::BAD_REQUEST),
            (br#"{"version":5}"#, StatusCode::BAD_REQUEST),
            (b"not json", StatusCode::BAD_REQUEST),
            (b"[]", StatusCode::BAD_REQUEST),
            (b"\"v0.5.1\"", StatusCode::BAD_REQUEST),
        ] {
            let (code, msg) = request_install(&cfg, body).await;
            assert_eq!(code, want, "{} -> {msg}", String::from_utf8_lossy(body));
            assert!(!cfg.update.request_file.exists());
        }
        // A tag from the list is written as is, atomically, and only once.
        let (code, _) = request_install(&cfg, br#"{"version":"v0.5.1"}"#).await;
        assert_eq!(code, StatusCode::ACCEPTED);
        assert_eq!(
            tokio::fs::read_to_string(&cfg.update.request_file)
                .await
                .unwrap(),
            "v0.5.1"
        );
        let v = status_json(&cfg).await;
        assert_eq!(v["pending"], true);
        assert_eq!(v["pending_version"], "v0.5.1");
        let (code, _) = request_install(&cfg, br#"{"version":"v9.0.0"}"#).await;
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(
            tokio::fs::read_to_string(&cfg.update.request_file)
                .await
                .unwrap(),
            "v0.5.1",
            "the queued request is untouched"
        );
        // No temporary file is left behind.
        let mut left = Vec::new();
        let mut rd = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(e) = rd.next_entry().await.unwrap() {
            left.push(e.file_name().to_string_lossy().into_owned());
        }
        assert!(left.iter().all(|n| !n.ends_with(".tmp")), "{left:?}");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn no_body_means_the_newest_of_the_channel_and_an_empty_file() {
        let (cfg, dir) = setup("empty", Some(status_text("0.6.0", true))).await;
        for body in [&b""[..], b"  \n", b"{}", br#"{"version":null}"#] {
            let (code, _) = request_install(&cfg, body).await;
            assert_eq!(code, StatusCode::ACCEPTED);
            assert_eq!(
                tokio::fs::metadata(&cfg.update.request_file)
                    .await
                    .unwrap()
                    .len(),
                0
            );
            tokio::fs::remove_file(&cfg.update.request_file)
                .await
                .unwrap();
        }
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn versions_below_the_floor_are_hidden_and_refused() {
        let status = json!({
            "features": ["pin"], "latest_stable": "v9.0.0",
            "releases": [
                {"tag": "v9.0.0", "prerelease": false},
                {"tag": "v0.5.0", "prerelease": false},
                {"tag": "v0.4.9", "prerelease": false},
                {"tag": "v0.3.0", "prerelease": false},
                {"tag": "v0.5.0-alpha.1", "prerelease": true},
            ],
        });
        let tags: Vec<String> = view(&status).releases.into_iter().map(|r| r.tag).collect();
        assert_eq!(tags, vec!["v9.0.0", "v0.5.0"]);
        let (cfg, dir) = setup("floor", Some(status.to_string())).await;
        let (code, msg) = request_install(&cfg, br#"{"version":"v0.4.9"}"#).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(msg.contains("older than v0.5.0"), "{msg}");
        assert!(!cfg.update.request_file.exists());
        let (code, _) = request_install(&cfg, br#"{"version":"v0.5.0"}"#).await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn pinning_is_checked_before_the_list() {
        // Not pinnable AND not in the list: 422 (the real problem), not 400.
        let (cfg, dir) = setup("n4", Some(status_text("0.6.0", false))).await;
        let (code, _) = request_install(&cfg, br#"{"version":"v7.7.7"}"#).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn a_second_install_within_the_cooldown_is_refused() {
        let (cfg, dir) = setup("cool", Some(status_text("0.6.0", true))).await;
        let gate = Cooldown::new(std::time::Duration::from_secs(30));
        let (code, _) = queue_install(&cfg, &gate, b"").await;
        assert_eq!(code, StatusCode::ACCEPTED);
        // The root side took it (the request is gone), a new click comes right after.
        tokio::fs::remove_file(&cfg.update.request_file)
            .await
            .unwrap();
        let (code, msg) = queue_install(&cfg, &gate, b"").await;
        assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "{msg}");
        assert!(!cfg.update.request_file.exists());
        // A refused request does not start the clock.
        let fresh = Cooldown::new(std::time::Duration::from_secs(30));
        let (code, _) = queue_install(&cfg, &fresh, br#"{"version":"v7.7.7"}"#).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        let (code, _) = queue_install(&cfg, &fresh, b"").await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn channel_words_match_the_root_side() {
        let (mut cfg, dir) = setup("words", None).await;
        for (word, want) in [
            ("prerelease", true),
            ("pre", true),
            ("alpha", true),
            ("beta", true),
            ("stable", false),
            ("", false),
        ] {
            cfg.update.channel = word.into();
            assert_eq!(show_prerelease(&cfg).await, want, "{word}");
        }
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn concurrent_prefs_writes_do_not_collide() {
        let (cfg, dir) = setup("n5", None).await;
        let mut tasks = Vec::new();
        for i in 0..16 {
            let cfg = cfg.clone();
            tasks.push(tokio::spawn(async move {
                let body = if i % 2 == 0 {
                    &br#"{"show_prerelease":true}"#[..]
                } else {
                    &br#"{"show_prerelease":false}"#[..]
                };
                save_prefs(&cfg, body).await.0
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap(), StatusCode::OK);
        }
        let mut left = Vec::new();
        let mut rd = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(e) = rd.next_entry().await.unwrap() {
            left.push(e.file_name().to_string_lossy().into_owned());
        }
        assert!(left.iter().all(|n| !n.ends_with(".tmp")), "{left:?}");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn installing_the_running_version_is_refused() {
        let installed = env!("CARGO_PKG_VERSION");
        let status = json!({
            "features": ["pin"], "latest_stable": format!("v{installed}"),
            "releases": [{"tag": format!("v{installed}"), "prerelease": false}],
        })
        .to_string();
        let (cfg, dir) = setup("same", Some(status)).await;
        let body = json!({"version": format!("v{installed}")}).to_string();
        let (code, msg) = request_install(&cfg, body.as_bytes()).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{msg}");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn prefs_persist_and_seed_from_the_config_channel() {
        let (mut cfg, dir) = setup("prefs", None).await;
        assert!(!show_prerelease(&cfg).await);
        cfg.update.channel = "prerelease".into();
        assert!(
            show_prerelease(&cfg).await,
            "config channel seeds the default"
        );
        // Saved prefs win over the config, and survive a "restart" (a fresh read).
        let (code, body) = save_prefs(&cfg, br#"{"show_prerelease":false}"#).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body, r#"{"show_prerelease":false}"#);
        assert!(!show_prerelease(&cfg).await);
        let (code, _) = save_prefs(&cfg, br#"{"show_prerelease":true}"#).await;
        assert_eq!(code, StatusCode::OK);
        cfg.update.channel = "stable".into();
        assert!(show_prerelease(&cfg).await);
        assert!(!dir.join("update-prefs.json.tmp").exists());
        for bad in [&b"{}"[..], br#"{"show_prerelease":"yes"}"#, b"true", b""] {
            let (code, _) = save_prefs(&cfg, bad).await;
            assert_eq!(code, StatusCode::BAD_REQUEST);
        }
        assert!(
            show_prerelease(&cfg).await,
            "a refused body changes nothing"
        );
        // A garbled file falls back to the config.
        tokio::fs::write(&cfg.update_prefs_file, "{").await.unwrap();
        assert!(!show_prerelease(&cfg).await);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    // ------------------------------------------------------------ HTTP ---

    async fn http(
        addr: std::net::SocketAddr,
        method: &str,
        path: &str,
        header: bool,
        body: &str,
    ) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let hdr = if header { "x-cuemix-update: 1\r\n" } else { "" };
        let req = format!(
            "{method} {path} HTTP/1.1\r\nhost: {addr}\r\n{hdr}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let code = out
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let body = out.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
        (code, body)
    }

    #[tokio::test]
    async fn routes_enforce_the_header_and_the_body_limit() {
        let (cfg, dir) = setup("http", Some(status_text("0.6.0", true))).await;
        let (engine, _link, _task) = crate::engine::start(crate::engine::EngineConfig::for_dir(
            crate::state::Topology::default(),
            &dir,
        ))
        .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_trigger, shutdown) = crate::engine::shutdown_channel();
        tokio::spawn(super::super::server::serve(
            listener,
            engine,
            cfg.clone(),
            shutdown,
        ));

        let (code, body) = http(addr, "GET", "/api/update", false, "").await;
        assert_eq!(code, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["version_pinning"], true);
        assert_eq!(v["releases"].as_array().unwrap().len(), 3);

        // Both writes need the header.
        assert_eq!(http(addr, "POST", "/api/update", false, "").await.0, 403);
        assert_eq!(
            http(
                addr,
                "PUT",
                "/api/update/prefs",
                false,
                r#"{"show_prerelease":true}"#
            )
            .await
            .0,
            403
        );
        assert!(!cfg.update.request_file.exists() && !cfg.update_prefs_file.exists());

        let big = format!(r#"{{"version":"v0.5.1","pad":"{}"}}"#, "x".repeat(2000));
        assert_eq!(http(addr, "POST", "/api/update", true, &big).await.0, 413);
        assert!(!cfg.update.request_file.exists());

        let (code, _) = http(
            addr,
            "PUT",
            "/api/update/prefs",
            true,
            r#"{"show_prerelease":true}"#,
        )
        .await;
        assert_eq!(code, 200);
        let (_, body) = http(addr, "GET", "/api/update", false, "").await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["show_prerelease"], true);

        assert_eq!(
            http(addr, "POST", "/api/update", true, r#"{"version":"v7.7.7"}"#)
                .await
                .0,
            400
        );
        assert_eq!(
            http(addr, "POST", "/api/update", true, r#"{"version":"v0.5.1"}"#)
                .await
                .0,
            202
        );
        assert_eq!(http(addr, "POST", "/api/update", true, "").await.0, 429);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}
