//! Request bodies: everything is checked before a single byte is parsed, and the parse goes
//! into a small typed struct (`#[serde(deny_unknown_fields)]`), never into a free-form value.
//!
//! Order: size, `Content-Type`, UTF-8, control characters, then serde. Each step has its own
//! status code so a client (and the tests) can tell what was wrong.

use axum::http::{header, HeaderMap, StatusCode};
use serde::de::DeserializeOwned;

/// A refused body: status code and a short reason.
pub(super) type Refusal = (StatusCode, String);

fn refuse(status: StatusCode, message: impl Into<String>) -> Refusal {
    (status, message.into())
}

/// `Content-Type` must be `application/json` (parameters such as `charset=utf-8` allowed,
/// case-insensitive). Call it only when a body is present.
pub(super) fn check_json_content_type(headers: &HeaderMap) -> Result<(), Refusal> {
    let value = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let media = value.split(';').next().unwrap_or_default().trim();
    if !media.eq_ignore_ascii_case("application/json") {
        return Err(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        ));
    }
    Ok(())
}

/// The body as text: at most `max` bytes, valid UTF-8, and no control character other than the
/// JSON whitespace (`\t`, `\n`, `\r`). NUL, escape, DEL and C1 controls are all refused.
pub(super) fn json_text(body: &[u8], max: usize) -> Result<&str, Refusal> {
    if body.len() > max {
        return Err(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("body is larger than {max} bytes"),
        ));
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| refuse(StatusCode::BAD_REQUEST, "body must be valid UTF-8"))?;
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "body must not contain control characters",
        ));
    }
    Ok(text)
}

/// [`json_text`], then the typed parse. `hint` is what the error says was expected.
pub(super) fn parse_json<T: DeserializeOwned>(
    body: &[u8],
    max: usize,
    hint: &str,
) -> Result<T, Refusal> {
    let text = json_text(body, max)?;
    serde_json::from_str(text).map_err(|_| refuse(StatusCode::BAD_REQUEST, hint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        on: bool,
    }

    fn ct(v: &'static str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(v));
        h
    }

    #[test]
    fn only_json_content_types_pass() {
        for ok in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            " application/json ;charset=UTF-8",
        ] {
            assert!(check_json_content_type(&ct(ok)).is_ok(), "{ok}");
        }
        for bad in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "application/jsonp",
            "application/json5",
            "text/json",
            "",
            "application/",
        ] {
            let e = check_json_content_type(&ct(bad)).unwrap_err();
            assert_eq!(e.0, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{bad}");
        }
        let none = check_json_content_type(&HeaderMap::new()).unwrap_err();
        assert_eq!(none.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn size_utf8_and_control_characters_are_refused_before_parsing() {
        let code = |b: &[u8]| parse_json::<Probe>(b, 64, "hint").map_err(|e| e.0);
        assert_eq!(code(br#"{"on":true}"#), Ok(Probe { on: true }));
        assert_eq!(code(b"{\"on\":\n\ttrue\r\n}"), Ok(Probe { on: true }));
        let big = format!("{{\"on\":true,\"pad\":\"{}\"}}", "x".repeat(100));
        assert_eq!(code(big.as_bytes()), Err(StatusCode::PAYLOAD_TOO_LARGE));
        // Exactly the limit passes, one more does not.
        let edge = format!("{{\"on\":true}}{}", " ".repeat(64 - 11));
        assert_eq!(edge.len(), 64);
        assert!(code(edge.as_bytes()).is_ok());
        assert_eq!(
            code(format!("{edge} ").as_bytes()),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
        for bad in [
            &b"\xff\xfe{\"on\":true}"[..],
            b"{\"on\":true}\xc3",
            b"{\"on\":\0true}",
            b"{\"on\":true}\0",
            b"\x1b{\"on\":true}",
            b"{\"on\":true}\x7f",
            "{\"on\":true}\u{85}".as_bytes(),
        ] {
            assert_eq!(code(bad), Err(StatusCode::BAD_REQUEST), "{bad:?}");
        }
    }

    #[test]
    fn the_typed_parse_refuses_unknown_fields_wrong_types_and_non_objects() {
        let code = |b: &str| parse_json::<Probe>(b.as_bytes(), 256, "hint").map_err(|e| e.0);
        assert!(code(r#"{"on":true}"#).is_ok());
        for bad in [
            "",
            "null",
            "[]",
            "true",
            "\"on\"",
            "{}",
            r#"{"on":1}"#,
            r#"{"on":"true"}"#,
            r#"{"on":true,"extra":1}"#,
            r#"{"On":true}"#,
            r#"{"on":true} trailing"#,
            r#"{"on":true}{"on":false}"#,
        ] {
            assert_eq!(code(bad), Err(StatusCode::BAD_REQUEST), "{bad}");
        }
    }
}
