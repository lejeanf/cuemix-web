//! Per-parameter rate limiter towards the MOTU (≤ `throttle_hz`, default 40 Hz),
//! always delivering the final value.
//!
//! The limiter is a pure state machine: callers pass the current
//! [`tokio::time::Instant`] explicitly, so it is deterministic and works
//! with `tokio::time::pause()`/`advance()`. Key = parameter identity (same
//! [`Param`] variant + same indices, value ignored).
//!
//! Semantics: the first update of an idle key is sent immediately. Updates
//! arriving within `interval` of the previous send are coalesced (latest
//! wins) and released by [`Throttle::flush_due`] once the interval elapsed.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::state::Param;

/// Parameter identity: variant name and up to two indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamKey(&'static str, u32, u32);

/// Identity of a parameter (value and `on` flag ignored).
pub fn key_of(p: &Param) -> ParamKey {
    use Param as P;
    match *p {
        P::MixVol { bus, ch, .. } => ParamKey("mix_vol", bus.into(), ch.into()),
        P::MixPan { bus, ch, .. } => ParamKey("mix_pan", bus.into(), ch.into()),
        P::MixMute { bus, ch, .. } => ParamKey("mix_mute", bus.into(), ch.into()),
        P::MixSolo { bus, ch, .. } => ParamKey("mix_solo", bus.into(), ch.into()),
        P::MainVol { .. } => ParamKey("main_vol", 0, 0),
        P::PreampGain { ch, .. } => ParamKey("preamp_gain", ch.into(), 0),
        P::Phantom { ch, .. } => ParamKey("phantom", ch.into(), 0),
        P::Pad { ch, .. } => ParamKey("pad", ch.into(), 0),
        P::Phase { ch, .. } => ParamKey("phase", ch.into(), 0),
        P::EqBypass { ch, .. } => ParamKey("eq_bypass", ch.into(), 0),
        P::Route { input, output, .. } => ParamKey("route", input.into(), output.into()),
        P::Dim { .. } => ParamKey("dim", 0, 0),
        P::Mono { .. } => ParamKey("mono", 0, 0),
        P::SpeakerB { .. } => ParamKey("speaker_b", 0, 0),
        P::Talkback { .. } => ParamKey("talkback", 0, 0),
        P::PhonesVol { .. } => ParamKey("phones_vol", 0, 0),
    }
}

#[derive(Debug)]
struct Slot {
    last_sent: Instant,
    pending: Option<Param>,
}

/// Per-key rate limiter with latest-value coalescing.
#[derive(Debug)]
pub struct Throttle {
    interval: Duration,
    slots: HashMap<ParamKey, Slot>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            slots: HashMap::new(),
        }
    }

    /// Submit an update. Returns `Some(param)` when it may be sent right now;
    /// otherwise it is stored as the pending value of its key.
    pub fn push(&mut self, now: Instant, param: Param) -> Option<Param> {
        let key = key_of(&param);
        match self.slots.get_mut(&key) {
            Some(slot) if now.saturating_duration_since(slot.last_sent) < self.interval => {
                slot.pending = Some(param);
                None
            }
            Some(slot) => {
                slot.last_sent = now;
                slot.pending = None;
                Some(param)
            }
            None => {
                self.slots.insert(
                    key,
                    Slot {
                        last_sent: now,
                        pending: None,
                    },
                );
                Some(param)
            }
        }
    }

    /// Earliest instant at which a pending value becomes sendable.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.slots
            .values()
            .filter(|s| s.pending.is_some())
            .map(|s| s.last_sent + self.interval)
            .min()
    }

    /// Release every pending value whose interval has elapsed, and forget idle keys.
    pub fn flush_due(&mut self, now: Instant) -> Vec<Param> {
        let interval = self.interval;
        let mut out = Vec::new();
        for slot in self.slots.values_mut() {
            if now.saturating_duration_since(slot.last_sent) >= interval {
                if let Some(p) = slot.pending.take() {
                    slot.last_sent = now;
                    out.push(p);
                }
            }
        }
        self.slots.retain(|_, s| {
            s.pending.is_some() || now.saturating_duration_since(s.last_sent) < interval
        });
        out
    }

    /// Drop all pending values (e.g. on disconnect).
    pub fn clear(&mut self) {
        self.slots.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IV: Duration = Duration::from_millis(25);

    fn vol(ch: u16, value: f64) -> Param {
        Param::MixVol {
            bus: 0,
            ch,
            gain: value,
        }
    }

    fn value_of(p: &Param) -> f64 {
        match p {
            Param::MixVol { gain, .. } => *gain,
            _ => f64::NAN,
        }
    }

    #[test]
    fn burst_is_coalesced_and_final_value_delivered() {
        let mut t = Throttle::new(IV);
        let mut sent = Vec::new();
        let start = Instant::now();
        for i in 0..100u32 {
            let now = start + Duration::from_micros(u64::from(i) * 240); // 100 in 24 ms
            sent.extend(t.push(now, vol(1, f64::from(i) / 100.0)));
        }
        assert_eq!(sent.len(), 1, "first goes out immediately");
        assert_eq!(t.next_deadline(), Some(start + IV));
        assert!(t.flush_due(start + Duration::from_millis(24)).is_empty());
        sent.extend(t.flush_due(start + IV));
        assert!(sent.len() <= 2);
        assert_eq!(sent.last().map(value_of), Some(0.99));
        assert_eq!(t.next_deadline(), None);
    }

    #[test]
    fn independent_keys_do_not_block_each_other() {
        let mut t = Throttle::new(IV);
        let now = Instant::now();
        assert!(t.push(now, vol(1, 0.1)).is_some());
        assert!(t.push(now, vol(2, 0.2)).is_some());
        assert!(t
            .push(
                now,
                Param::MixVol {
                    bus: 1,
                    ch: 1,
                    gain: 0.3
                }
            )
            .is_some());
        assert!(t.push(now, Param::MainVol { db: -6.0 }).is_some());
        assert!(t.push(now, vol(1, 0.9)).is_none());
    }

    #[test]
    fn idle_key_sends_immediately_again_after_interval() {
        let mut t = Throttle::new(IV);
        let now = Instant::now();
        assert!(t.push(now, vol(1, 0.1)).is_some());
        assert!(t.flush_due(now + IV).is_empty());
        assert!(t.push(now + IV, vol(1, 0.2)).is_some());
    }

    #[test]
    fn pending_release_restarts_the_interval() {
        let mut t = Throttle::new(IV);
        let now = Instant::now();
        t.push(now, vol(1, 0.1));
        t.push(now, vol(1, 0.2));
        assert_eq!(t.flush_due(now + IV).len(), 1);
        assert!(t.push(now + IV, vol(1, 0.3)).is_none());
        assert_eq!(t.next_deadline(), Some(now + IV + IV));
    }
}
