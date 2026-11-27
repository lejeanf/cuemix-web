//! Optional admin token (`[web] admin_token = "auto"`, off by default).
//!
//! When it is on, only a *paired* device may change what the server runs or where it sends
//! requests: installs and the pre-release switch (`/api/update*`), the speaker plug and the OSC
//! settings (`set_speaker_plug`, `test_speaker_plug`, `set_osc_settings`). Faders, presets,
//! power and everything else stay open to the local network.
//!
//! - The token is 32 random bytes (64 hex characters), created on first start in
//!   `[storage] admin_token_file` with mode 0600, and never sent to a client that does not
//!   already hold it.
//! - Pairing link: `http://<mdns>.local:<port>/#pair=<token>`, logged once when the token is
//!   created and printed again by `cuemix-web --pair-link`. The token is in the fragment, so it
//!   never reaches a server log or a `Referer`. The page posts it to `POST /api/admin/pair`,
//!   which answers with an `HttpOnly`, `SameSite=Strict` cookie.
//! - Scripts can send `Authorization: Bearer <token>` instead of the cookie.
//! - Comparisons run in constant time.

use std::path::Path;

use anyhow::{bail, Context, Result};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::body::{check_json_content_type, parse_json};
use super::server::{origin_allowed, AppState, WebConfig};
use crate::state::Command;

/// Cookie that carries the token on a paired device.
pub const COOKIE: &str = "cuemix_admin";
/// Random bytes in a token.
const TOKEN_BYTES: usize = 32;
/// Cookie lifetime (one year; pairing again renews it).
const COOKIE_MAX_AGE: u32 = 365 * 24 * 3600;
/// Largest body of `POST /api/admin/pair`.
pub(super) const MAX_BODY_BYTES: usize = 256;

/// The admin token. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AdminToken(String);

impl std::fmt::Debug for AdminToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminToken(<redacted>)")
    }
}

impl AdminToken {
    /// A token as stored: exactly 64 lowercase hex characters.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        (t.len() == TOKEN_BYTES * 2 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(t.to_owned()))
    }

    /// Constant-time comparison with what a client sent.
    pub fn matches(&self, given: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), given.as_bytes());
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    /// The link that pairs a device (token in the fragment).
    pub fn pair_link(&self, mdns_hostname: &str, port: u16) -> String {
        format!("http://{mdns_hostname}.local:{port}/#pair={}", self.0)
    }
}

/// What `[web] admin_token` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminMode {
    Off,
    Auto,
}

impl AdminMode {
    pub fn from_config(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            _ => bail!(
                "[web] admin_token must be \"\" (off) or \"auto\" (a token generated in [storage] admin_token_file)"
            ),
        }
    }
}

/// Read the token file. `Ok(None)` when it does not exist.
pub async fn read_token(path: &Path) -> Result<Option<AdminToken>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => AdminToken::parse(&text).map(Some).with_context(|| {
            format!(
                "{} does not hold a valid admin token; delete it to generate a new one",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// The token in `path`, created (0600, never overwritten) when missing. The flag says whether
/// it was just created.
pub async fn load_or_create(path: &Path) -> Result<(AdminToken, bool)> {
    if let Some(t) = read_token(path).await? {
        return Ok((t, false));
    }
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .with_context(|| format!("cannot create {}", path.display()))?;
    use tokio::io::AsyncWriteExt;
    file.write_all(format!("{hex}\n").as_bytes()).await?;
    file.sync_all().await?;
    Ok((AdminToken(hex), true))
}

/// The token a request carries: `Authorization: Bearer`, else the cookie.
fn presented(headers: &HeaderMap) -> Option<&str> {
    if let Some(t) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(t.trim());
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

/// Whether the request may do admin things: always when the token is off.
pub(super) fn is_admin(headers: &HeaderMap, cfg: &WebConfig) -> bool {
    match &cfg.admin_token {
        None => true,
        Some(token) => presented(headers).is_some_and(|t| token.matches(t)),
    }
}

/// Commands that need a paired device: they change where the server sends requests.
pub(super) fn needs_admin(command: &Command) -> bool {
    matches!(
        command,
        Command::SetSpeakerPlug { .. }
            | Command::TestSpeakerPlug { .. }
            | Command::SetOscSettings { .. }
    )
}

/// The refusal of an admin-only request from a device that is not paired.
pub(super) const NOT_PAIRED: &str =
    "this device is not paired as admin: open the pairing link (cuemix-web --pair-link) on it first";

fn no_store(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// `GET /api/admin`: `{"required":bool,"paired":bool}`.
pub(super) async fn status(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    no_store(
        Json(json!({
            "required": app.cfg.admin_token.is_some(),
            "paired": app.cfg.admin_token.is_some() && is_admin(&headers, &app.cfg),
        }))
        .into_response(),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairBody {
    token: String,
}

/// `Set-Cookie` value: the token (or nothing, to unpair), `Secure` behind an HTTPS proxy.
fn cookie(value: &str, max_age: u32, secure: bool) -> Option<HeaderValue> {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    ))
    .ok()
}

fn behind_https(headers: &HeaderMap, cfg: &WebConfig) -> bool {
    cfg.trust_forwarded_headers
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|p| {
                p.rsplit(',')
                    .next()
                    .unwrap_or(p)
                    .trim()
                    .eq_ignore_ascii_case("https")
            })
}

/// `POST /api/admin/pair` with `{"token":"…"}`: on a match, the cookie that pairs this device.
pub(super) async fn pair(State(app): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let Some(token) = &app.cfg.admin_token else {
        return (StatusCode::NOT_FOUND, "the admin token is not enabled").into_response();
    };
    if let Err(refused) = check_json_content_type(&headers) {
        return refused.into_response();
    }
    let PairBody { token: given } =
        match parse_json(&body, MAX_BODY_BYTES, "expected {\"token\": \"…\"}") {
            Ok(b) => b,
            Err(refused) => return refused.into_response(),
        };
    if !token.matches(given.trim()) {
        tracing::warn!("refused an admin pairing attempt with a wrong token");
        return (StatusCode::FORBIDDEN, "wrong pairing token").into_response();
    }
    let Some(set) = cookie(
        given.trim(),
        COOKIE_MAX_AGE,
        behind_https(&headers, &app.cfg),
    ) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    tracing::info!("a device was paired as admin");
    let mut resp = Json(json!({"required": true, "paired": true})).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, set);
    no_store(resp)
}

/// `POST /api/admin/unpair`: forget the cookie on this device.
pub(super) async fn unpair(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers, &app.cfg) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let mut resp = Json(json!({
        "required": app.cfg.admin_token.is_some(),
        "paired": false,
    }))
    .into_response();
    if let Some(set) = cookie("", 0, behind_https(&headers, &app.cfg)) {
        resp.headers_mut().insert(header::SET_COOKIE, set);
    }
    no_store(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> AdminToken {
        AdminToken::parse(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn tokens_are_strict_and_never_printed() {
        assert!(AdminToken::parse(&"0f".repeat(32)).is_some());
        assert!(AdminToken::parse(&format!("{}\n", "0f".repeat(32))).is_some());
        for bad in [
            "",
            "abc",
            &"AB".repeat(32),
            &"0f".repeat(31),
            &"0g".repeat(32),
            &"0f".repeat(33),
        ] {
            assert!(AdminToken::parse(bad).is_none(), "{bad}");
        }
        assert!(!format!("{:?}", token()).contains("abab"));
        assert!(token().matches(&"ab".repeat(32)));
        assert!(!token().matches(&"ab".repeat(31)));
        assert!(!token().matches(&format!("{}ac", "ab".repeat(31))));
        assert_eq!(
            token().pair_link("cuemix-web", 8080),
            format!("http://cuemix-web.local:8080/#pair={}", "ab".repeat(32))
        );
    }

    #[test]
    fn the_token_comes_from_the_bearer_header_or_the_cookie() {
        let cfg = WebConfig {
            admin_token: Some(token()),
            ..WebConfig::default()
        };
        let with = |k: header::HeaderName, v: String| {
            let mut h = HeaderMap::new();
            h.insert(k, HeaderValue::from_str(&v).unwrap());
            h
        };
        let good = "ab".repeat(32);
        assert!(is_admin(
            &with(header::COOKIE, format!("a=1; {COOKIE}={good}; b=2")),
            &cfg
        ));
        assert!(is_admin(
            &with(header::AUTHORIZATION, format!("Bearer {good}")),
            &cfg
        ));
        assert!(!is_admin(&HeaderMap::new(), &cfg));
        assert!(!is_admin(
            &with(header::COOKIE, format!("{COOKIE}=00{}", &good[2..])),
            &cfg
        ));
        assert!(!is_admin(
            &with(header::COOKIE, format!("x{COOKIE}={good}")),
            &cfg
        ));
        // Off: everyone is admin.
        assert!(is_admin(&HeaderMap::new(), &WebConfig::default()));
    }

    #[test]
    fn config_values() {
        assert_eq!(AdminMode::from_config("").unwrap(), AdminMode::Off);
        assert_eq!(AdminMode::from_config("auto").unwrap(), AdminMode::Auto);
        assert!(AdminMode::from_config("hunter2").is_err());
    }

    #[tokio::test]
    async fn the_token_file_is_created_once_private_and_kept() {
        let dir = std::env::temp_dir().join(format!(
            "cuemix-admin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join("admin-token");
        let (first, created) = load_or_create(&path).await.unwrap();
        assert!(created);
        let (again, created) = load_or_create(&path).await.unwrap();
        assert!(!created);
        assert_eq!(first, again);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(&path, "garbage").unwrap();
        assert!(
            load_or_create(&path).await.is_err(),
            "a bad file is never replaced silently"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
