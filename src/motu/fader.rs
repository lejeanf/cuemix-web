//! Unified audio fader law shared by the Web UI and OSC.
//!
//! External controls use a normalised position `p ∈ [0.0, 1.0]`:
//!
//! | position | dB       | linear gain |
//! |----------|----------|-------------|
//! | `0.0`    | `-inf`   | `0.0`       |
//! | `0.75`   | `0.0`    | `1.0`       |
//! | `1.0`    | `+12.0`  | `3.98107…`  |
//!
//! The law is a single pure power curve `gain = (p / 0.75)^K` with
//! `K = (12 / 20) / log10(1 / 0.75) ≈ 4.8024`, i.e. `dB = 20·K·log10(p / 0.75)`.
//! It is continuous, strictly monotonic, exactly invertible, hits all three
//! anchor points exactly and gives ≈ −17 dB at `0.5`, ≈ −46 dB at `0.25`.
//!
//! All functions are total: non-finite input maps to silence (`0.0`) and
//! out-of-range input is clamped. Rejecting bad input is the job of
//! [`crate::state::Command::validate`], at the system boundary.

/// Position at which the fader reads exactly 0 dB (unity gain).
pub const UNITY_POS: f64 = 0.75;
/// Gain in dB at full travel (`p = 1.0`).
pub const MAX_DB: f64 = 12.0;

/// Exponent of the power law: `(MAX_DB / 20) / log10(1 / UNITY_POS)`.
#[inline]
pub fn exponent() -> f64 {
    (MAX_DB / 20.0) / (1.0 / UNITY_POS).log10()
}

/// Linear gain at full travel: `10^(MAX_DB / 20)` ≈ 3.98107.
#[inline]
pub fn max_gain() -> f64 {
    10f64.powf(MAX_DB / 20.0)
}

#[inline]
fn sanitize_pos(pos: f64) -> f64 {
    if pos.is_finite() {
        pos.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Normalised position → linear gain (`0.0 ..= max_gain()`).
pub fn pos_to_gain(pos: f64) -> f64 {
    let p = sanitize_pos(pos);
    if p <= 0.0 {
        return 0.0;
    }
    (p / UNITY_POS).powf(exponent())
}

/// Linear gain → normalised position (`0.0 ..= 1.0`).
pub fn gain_to_pos(gain: f64) -> f64 {
    if !gain.is_finite() || gain <= 0.0 {
        return if gain == f64::INFINITY { 1.0 } else { 0.0 };
    }
    let g = gain.min(max_gain());
    (UNITY_POS * g.powf(1.0 / exponent())).clamp(0.0, 1.0)
}

/// Normalised position → dB (`f64::NEG_INFINITY ..= MAX_DB`).
pub fn pos_to_db(pos: f64) -> f64 {
    let p = sanitize_pos(pos);
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    20.0 * exponent() * (p / UNITY_POS).log10()
}

/// dB → normalised position. `-inf` (or NaN) → `0.0`, above `MAX_DB` → `1.0`.
pub fn db_to_pos(db: f64) -> f64 {
    if db.is_nan() || db == f64::NEG_INFINITY {
        return 0.0;
    }
    let d = db.min(MAX_DB);
    (UNITY_POS * 10f64.powf(d / (20.0 * exponent()))).clamp(0.0, 1.0)
}

/// Trim taper (main / headphones): same curve as the mix faders but with
/// `1.0` = 0 dB (trims have no boost). Never returns less than `floor_db`
/// (so `0.0` maps to the hardware floor, not `-inf`).
pub fn trim_pos_to_db(pos: f64, floor_db: f64) -> f64 {
    pos_to_db(sanitize_pos(pos) * UNITY_POS).max(floor_db)
}

/// Inverse of [`trim_pos_to_db`]: dB (≤ 0) → position `0.0..=1.0`.
pub fn trim_db_to_pos(db: f64) -> f64 {
    (db_to_pos(db.min(0.0)) / UNITY_POS).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    #[test]
    fn anchor_points() {
        assert_eq!(pos_to_gain(0.0), 0.0);
        assert_eq!(pos_to_db(0.0), f64::NEG_INFINITY);
        assert!((pos_to_gain(0.75) - 1.0).abs() < EPS);
        assert!(pos_to_db(0.75).abs() < EPS);
        assert!((pos_to_gain(1.0) - 3.981_071_705_534_972).abs() < 1e-9);
        assert!((pos_to_db(1.0) - 12.0).abs() < EPS);
    }

    #[test]
    fn exponent_value() {
        assert!((exponent() - 4.802_4).abs() < 1e-3);
    }

    #[test]
    fn round_trip_gain_and_db() {
        for i in 1..=1000 {
            let p = f64::from(i) / 1000.0;
            assert!((gain_to_pos(pos_to_gain(p)) - p).abs() < 1e-9, "gain p={p}");
            assert!((db_to_pos(pos_to_db(p)) - p).abs() < 1e-9, "db p={p}");
        }
    }

    #[test]
    fn strictly_monotonic() {
        let mut prev = pos_to_gain(0.0);
        for i in 1..=1000 {
            let g = pos_to_gain(f64::from(i) / 1000.0);
            assert!(g > prev);
            prev = g;
        }
    }

    #[test]
    fn total_on_bad_input() {
        assert_eq!(pos_to_gain(f64::NAN), 0.0);
        assert_eq!(pos_to_gain(-1.0), 0.0);
        assert!((pos_to_gain(7.0) - max_gain()).abs() < EPS);
        assert_eq!(gain_to_pos(f64::NAN), 0.0);
        assert_eq!(gain_to_pos(-3.0), 0.0);
        assert_eq!(gain_to_pos(f64::INFINITY), 1.0);
        assert!((gain_to_pos(100.0) - 1.0).abs() < EPS);
        assert_eq!(db_to_pos(f64::NEG_INFINITY), 0.0);
        assert_eq!(db_to_pos(f64::NAN), 0.0);
        assert!((db_to_pos(40.0) - 1.0).abs() < EPS);
    }

    #[test]
    fn trim_law() {
        assert!(trim_pos_to_db(1.0, -100.0).abs() < EPS);
        assert_eq!(trim_pos_to_db(0.0, -100.0), -100.0);
        assert_eq!(trim_pos_to_db(f64::NAN, -100.0), -100.0);
        assert!((trim_db_to_pos(0.0) - 1.0).abs() < EPS);
        assert_eq!(trim_db_to_pos(f64::NEG_INFINITY), 0.0);
        assert!((trim_db_to_pos(12.0) - 1.0).abs() < EPS, "no boost");
        for i in 1..=100 {
            let p = f64::from(i) / 100.0;
            let db = trim_pos_to_db(p, -1000.0);
            assert!((trim_db_to_pos(db) - p).abs() < 1e-9, "p={p}");
        }
    }

    #[test]
    fn usable_midpoints() {
        assert!((pos_to_db(0.5) - -16.9).abs() < 0.1);
        assert!((pos_to_db(0.25) - -45.8).abs() < 0.1);
    }
}
