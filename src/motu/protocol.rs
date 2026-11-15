//! MOTU UltraLite-mk5 binary frame codec (Big-Endian) and `8.24` fixed point.
//!
//! Wire format of one property frame: `[prop_id: u16][index: u16][payload]`.
//! The payload layout depends on the property kind ([`ValueKind`]):
//!
//! | kind      | bytes | encoding                                      |
//! |-----------|-------|-----------------------------------------------|
//! | `Fixed`   | 4     | signed `8.24` fixed point, BE (`i32 / 2^24`)  |
//! | `U32`     | 4     | `u32` BE (booleans/enums)                     |
//! | `U8`      | 1     | `u8` (booleans/enums)                         |
//! | `Str32`   | 32    | UTF-8, NUL-terminated / NUL-padded            |
//!
//! The concrete `prop_id` table is **not yet confirmed against hardware**
//! (sources: `motu-mk5-linux`, CueMix 5 `app.asar`); it lives in Phase 2.

use thiserror::Error;

/// Scale factor of the signed `8.24` fixed-point format (`2^24`).
pub const FIXED_8_24_SCALE: f64 = 16_777_216.0;
/// Smallest representable `8.24` value.
pub const FIXED_8_24_MIN: f64 = -128.0;
/// Largest representable `8.24` value (`(2^31 - 1) / 2^24`).
pub const FIXED_8_24_MAX: f64 = i32::MAX as f64 / FIXED_8_24_SCALE;
/// Header length: `prop_id` + `index`.
pub const HEADER_LEN: usize = 4;
/// Length of a fixed-size string payload.
pub const STR32_LEN: usize = 32;

/// Codec errors. Malformed hardware frames must never panic the server.
#[derive(Debug, Error, PartialEq)]
pub enum ProtocolError {
    #[error("value {0} is not finite")]
    NonFinite(f64),
    #[error("value {0} is outside the 8.24 range [-128, 128)")]
    OutOfRange(f64),
    #[error("frame too short: {len} bytes, need at least {need}")]
    Truncated { len: usize, need: usize },
    #[error("payload length {len} does not match kind {kind:?} ({expected} bytes)")]
    PayloadLength {
        kind: ValueKind,
        len: usize,
        expected: usize,
    },
    #[error("string of {0} bytes does not fit in 31 bytes + NUL")]
    StringTooLong(usize),
    #[error("string payload is not valid UTF-8")]
    InvalidUtf8,
}

/// `f64` → signed `8.24` (round-to-nearest). Rejects NaN/inf and out-of-range.
pub fn to_fixed_8_24(value: f64) -> Result<i32, ProtocolError> {
    if !value.is_finite() {
        return Err(ProtocolError::NonFinite(value));
    }
    let scaled = (value * FIXED_8_24_SCALE).round();
    if scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
        return Err(ProtocolError::OutOfRange(value));
    }
    // In range and integral: the cast is exact.
    Ok(scaled as i32)
}

/// Signed `8.24` → `f64` (exact: every `i32 / 2^24` is representable).
pub fn from_fixed_8_24(raw: i32) -> f64 {
    f64::from(raw) / FIXED_8_24_SCALE
}

/// Kind of payload carried by a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Fixed,
    U32,
    U8,
    Str32,
}

impl ValueKind {
    /// Payload size in bytes.
    pub const fn payload_len(self) -> usize {
        match self {
            ValueKind::Fixed | ValueKind::U32 => 4,
            ValueKind::U8 => 1,
            ValueKind::Str32 => STR32_LEN,
        }
    }
}

/// Decoded payload value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Fixed(f64),
    U32(u32),
    U8(u8),
    Str32(String),
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Fixed(_) => ValueKind::Fixed,
            Value::U32(_) => ValueKind::U32,
            Value::U8(_) => ValueKind::U8,
            Value::Str32(_) => ValueKind::Str32,
        }
    }

    /// Append the BE payload bytes to `out`.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
        match self {
            Value::Fixed(v) => out.extend_from_slice(&to_fixed_8_24(*v)?.to_be_bytes()),
            Value::U32(v) => out.extend_from_slice(&v.to_be_bytes()),
            Value::U8(v) => out.push(*v),
            Value::Str32(s) => {
                let bytes = s.as_bytes();
                if bytes.len() >= STR32_LEN {
                    return Err(ProtocolError::StringTooLong(bytes.len()));
                }
                let mut buf = [0u8; STR32_LEN];
                buf[..bytes.len()].copy_from_slice(bytes);
                out.extend_from_slice(&buf);
            }
        }
        Ok(())
    }

    /// Decode a payload of the given kind. `bytes` must have exactly `kind.payload_len()` bytes.
    pub fn decode(kind: ValueKind, bytes: &[u8]) -> Result<Self, ProtocolError> {
        let payload_err = || ProtocolError::PayloadLength {
            kind,
            len: bytes.len(),
            expected: kind.payload_len(),
        };
        match kind {
            ValueKind::Fixed => {
                let arr: [u8; 4] = bytes.try_into().map_err(|_| payload_err())?;
                Ok(Value::Fixed(from_fixed_8_24(i32::from_be_bytes(arr))))
            }
            ValueKind::U32 => {
                let arr: [u8; 4] = bytes.try_into().map_err(|_| payload_err())?;
                Ok(Value::U32(u32::from_be_bytes(arr)))
            }
            ValueKind::U8 => match bytes {
                [b] => Ok(Value::U8(*b)),
                _ => Err(payload_err()),
            },
            ValueKind::Str32 => {
                if bytes.len() != STR32_LEN {
                    return Err(payload_err());
                }
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                let s =
                    std::str::from_utf8(&bytes[..end]).map_err(|_| ProtocolError::InvalidUtf8)?;
                Ok(Value::Str32(s.to_owned()))
            }
        }
    }
}

/// One property frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub prop_id: u16,
    pub index: u16,
    pub value: Value,
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.value.kind().payload_len());
        out.extend_from_slice(&self.prop_id.to_be_bytes());
        out.extend_from_slice(&self.index.to_be_bytes());
        self.value.encode_into(&mut out)?;
        Ok(out)
    }

    /// Decode one frame. `kind_of` maps a `prop_id` to its payload kind
    /// (the property table, Phase 2). Trailing bytes are rejected.
    pub fn decode(bytes: &[u8], kind_of: impl Fn(u16) -> ValueKind) -> Result<Self, ProtocolError> {
        let (header, payload) = match bytes.split_first_chunk::<HEADER_LEN>() {
            Some(split) => split,
            None => {
                return Err(ProtocolError::Truncated {
                    len: bytes.len(),
                    need: HEADER_LEN,
                })
            }
        };
        let prop_id = u16::from_be_bytes([header[0], header[1]]);
        let index = u16::from_be_bytes([header[2], header[3]]);
        let value = Value::decode(kind_of(prop_id), payload)?;
        Ok(Frame {
            prop_id,
            index,
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_known_values() {
        assert_eq!(to_fixed_8_24(1.0), Ok(0x0100_0000));
        assert_eq!(to_fixed_8_24(-1.0), Ok(-0x0100_0000));
        assert_eq!(to_fixed_8_24(0.5), Ok(0x0080_0000));
        assert_eq!(to_fixed_8_24(0.0), Ok(0));
        assert_eq!(to_fixed_8_24(-128.0), Ok(i32::MIN));
        assert_eq!(from_fixed_8_24(i32::MIN), -128.0);
        assert_eq!(from_fixed_8_24(0x0100_0000), 1.0);
    }

    #[test]
    fn fixed_rejects_bad_values() {
        assert!(matches!(
            to_fixed_8_24(f64::NAN),
            Err(ProtocolError::NonFinite(_))
        ));
        assert!(matches!(
            to_fixed_8_24(f64::INFINITY),
            Err(ProtocolError::NonFinite(_))
        ));
        assert_eq!(to_fixed_8_24(128.0), Err(ProtocolError::OutOfRange(128.0)));
        assert!(to_fixed_8_24(-128.1).is_err());
        // 20 kHz cannot be expressed in 8.24 — see ARCHITECTURE "open questions".
        assert!(to_fixed_8_24(20_000.0).is_err());
    }

    #[test]
    fn fixed_round_trip_precision() {
        for v in [3.981_071_705_534_972, 60.0, -0.000_123, 127.9, 1e-7] {
            let back = from_fixed_8_24(to_fixed_8_24(v).unwrap_or_default());
            assert!((back - v).abs() <= 0.5 / FIXED_8_24_SCALE, "{v}");
        }
    }

    #[test]
    fn frame_round_trip_all_kinds() {
        let kinds = |p: u16| match p {
            1 => ValueKind::Fixed,
            2 => ValueKind::U32,
            3 => ValueKind::U8,
            _ => ValueKind::Str32,
        };
        let frames = [
            Frame {
                prop_id: 1,
                index: 7,
                value: Value::Fixed(1.0),
            },
            Frame {
                prop_id: 2,
                index: 0,
                value: Value::U32(0xDEAD_BEEF),
            },
            Frame {
                prop_id: 3,
                index: 65535,
                value: Value::U8(1),
            },
            Frame {
                prop_id: 4,
                index: 2,
                value: Value::Str32("Mic 1".into()),
            },
        ];
        for f in frames {
            let bytes = f.encode().unwrap_or_default();
            assert_eq!(Frame::decode(&bytes, kinds), Ok(f));
        }
    }

    #[test]
    fn frame_wire_layout_is_big_endian() {
        let f = Frame {
            prop_id: 0x0102,
            index: 0x0304,
            value: Value::Fixed(1.0),
        };
        assert_eq!(
            f.encode(),
            Ok(vec![0x01, 0x02, 0x03, 0x04, 0x01, 0x00, 0x00, 0x00])
        );
    }

    #[test]
    fn malformed_frames_are_errors_not_panics() {
        let k = |_| ValueKind::Fixed;
        assert!(matches!(
            Frame::decode(&[], k),
            Err(ProtocolError::Truncated { .. })
        ));
        assert!(matches!(
            Frame::decode(&[0, 1, 0], k),
            Err(ProtocolError::Truncated { .. })
        ));
        assert!(matches!(
            Frame::decode(&[0, 1, 0, 0, 1], k),
            Err(ProtocolError::PayloadLength { .. })
        ));
        assert!(matches!(
            Frame::decode(&[0; 9], k),
            Err(ProtocolError::PayloadLength { .. })
        ));
        let mut bad_utf8 = vec![0, 4, 0, 0, 0xFF, 0xFE];
        bad_utf8.resize(HEADER_LEN + STR32_LEN, 0);
        assert_eq!(
            Frame::decode(&bad_utf8, |_| ValueKind::Str32),
            Err(ProtocolError::InvalidUtf8)
        );
        // Exhaustive short-input sweep.
        for len in 0..64usize {
            let buf = vec![0xAB; len];
            for kind in [
                ValueKind::Fixed,
                ValueKind::U32,
                ValueKind::U8,
                ValueKind::Str32,
            ] {
                let _ = Frame::decode(&buf, |_| kind);
            }
        }
    }

    #[test]
    fn string_too_long() {
        let f = Frame {
            prop_id: 4,
            index: 0,
            value: Value::Str32("x".repeat(32)),
        };
        assert_eq!(f.encode(), Err(ProtocolError::StringTooLong(32)));
    }
}
