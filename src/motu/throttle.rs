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

use std::collections::{HashMap, VecDeque};
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
        P::Route { input, output, .. } => ParamKey("route", input.into(), output.into()),
        P::Dim { .. } => ParamKey("dim", 0, 0),
        P::Mono { .. } => ParamKey("mono", 0, 0),
        P::SpeakerB { .. } => ParamKey("speaker_b", 0, 0),
        P::Talkback { .. } => ParamKey("talkback", 0, 0),
        P::PhonesVol { .. } => ParamKey("phones_vol", 0, 0),
        P::OutputTrim { ch, .. } => ParamKey("output_trim", ch.into(), 0),
        P::InputEqMode { ch, band, .. } => ParamKey("input_eq_mode", ch.into(), band.into()),
        P::InputEqBypass { ch, band, .. } => ParamKey("input_eq_bypass", ch.into(), band.into()),
        P::InputEqFreq { ch, band, .. } => ParamKey("input_eq_freq", ch.into(), band.into()),
        P::InputEqGain { ch, band, .. } => ParamKey("input_eq_gain", ch.into(), band.into()),
        P::InputEqBw { ch, band, .. } => ParamKey("input_eq_bw", ch.into(), band.into()),
        P::OutputEqMode { ch, band, .. } => ParamKey("output_eq_mode", ch.into(), band.into()),
        P::OutputEqBypass { ch, band, .. } => ParamKey("output_eq_bypass", ch.into(), band.into()),
        P::OutputEqFreq { ch, band, .. } => ParamKey("output_eq_freq", ch.into(), band.into()),
        P::OutputEqGain { ch, band, .. } => ParamKey("output_eq_gain", ch.into(), band.into()),
        P::OutputEqBw { ch, band, .. } => ParamKey("output_eq_bw", ch.into(), band.into()),
        P::GateBypass { ch, .. } => ParamKey("gate_bypass", ch.into(), 0),
        P::GateThreshold { ch, .. } => ParamKey("gate_threshold", ch.into(), 0),
        P::GateAttack { ch, .. } => ParamKey("gate_attack", ch.into(), 0),
        P::GateRelease { ch, .. } => ParamKey("gate_release", ch.into(), 0),
        P::CompBypass { ch, .. } => ParamKey("comp_bypass", ch.into(), 0),
        P::CompThreshold { ch, .. } => ParamKey("comp_threshold", ch.into(), 0),
        P::CompAttack { ch, .. } => ParamKey("comp_attack", ch.into(), 0),
        P::CompRelease { ch, .. } => ParamKey("comp_release", ch.into(), 0),
        P::CompRatio { ch, .. } => ParamKey("comp_ratio", ch.into(), 0),
        P::CompMakeup { ch, .. } => ParamKey("comp_makeup", ch.into(), 0),
        P::ReverbDecay { .. } => ParamKey("reverb_decay", 0, 0),
        P::ReverbDamping { .. } => ParamKey("reverb_damping", 0, 0),
        P::ReverbPredelay { .. } => ParamKey("reverb_predelay", 0, 0),
        P::ReverbWidth { .. } => ParamKey("reverb_width", 0, 0),
        P::ReverbPreset { .. } => ParamKey("reverb_preset", 0, 0),
        P::SampleRate { .. } => ParamKey("sample_rate", 0, 0),
        P::ClockSource { .. } => ParamKey("clock_source", 0, 0),
        P::OpticalMode { output, .. } => ParamKey("optical_mode", u32::from(output), 0),
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

/// Minimum spacing between two frames written to the MOTU, across **all**
/// parameters. The device closes the WebSocket when writes arrive faster than
/// it applies them (`docs/MOTU-PROTOCOL.md`; motu-mk5-linux spaces writes by
/// 20 ms). Applies to every frame, including each frame of a multi-frame
/// parameter and bursts after a preset recall.
pub const MIN_FRAME_SPACING: Duration = Duration::from_millis(20);

/// Aggregate output pacer: at most one frame per [`MIN_FRAME_SPACING`].
///
/// Sits behind the per-parameter [`Throttle`]. Queued parameters are coalesced
/// by [`key_of`] (latest value wins, original queue position kept), so the
/// queue never exceeds the number of distinct parameters (plus the remaining
/// frames of the one parameter being sent) and the final value is always
/// delivered. Pure state machine like [`Throttle`]: the caller owns time and
/// the socket.
#[derive(Debug)]
pub struct Pacer {
    order: VecDeque<ParamKey>,
    latest: HashMap<ParamKey, Param>,
    /// Remaining encoded frames of the parameter currently being sent.
    frames: VecDeque<Vec<u8>>,
    next_allowed: Option<Instant>,
}

impl Default for Pacer {
    fn default() -> Self {
        Self::new()
    }
}

impl Pacer {
    pub fn new() -> Self {
        Self {
            order: VecDeque::new(),
            latest: HashMap::new(),
            frames: VecDeque::new(),
            next_allowed: None,
        }
    }

    /// Queue a parameter, replacing a queued value of the same key.
    pub fn push(&mut self, param: Param) {
        let key = key_of(&param);
        if self.latest.insert(key, param).is_none() {
            self.order.push_back(key);
        }
    }

    /// Number of queued parameters (excluding frames already being sent).
    pub fn queued_params(&self) -> usize {
        self.order.len()
    }

    /// Nothing left to send.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.frames.is_empty()
    }

    /// Instant at which the next frame may be written (`None` = nothing queued).
    /// May be in the past: send right away.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        if self.is_empty() {
            None
        } else {
            Some(self.next_allowed.unwrap_or(now))
        }
    }

    /// Next frame if the spacing allows it at `now`. `encode` turns a
    /// parameter into wire frames (empty / unencodable = skipped).
    /// Call [`Pacer::sent`] after the write.
    pub fn pop_due(
        &mut self,
        now: Instant,
        mut encode: impl FnMut(&Param) -> Vec<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        if self.next_allowed.is_some_and(|t| now < t) {
            return None;
        }
        loop {
            if let Some(f) = self.frames.pop_front() {
                return Some(f);
            }
            let key = self.order.pop_front()?;
            if let Some(param) = self.latest.remove(&key) {
                self.frames = encode(&param).into();
            }
        }
    }

    /// Record that a frame was written at `at`.
    pub fn sent(&mut self, at: Instant) {
        self.next_allowed = Some(at + MIN_FRAME_SPACING);
    }

    /// Drop everything (e.g. on disconnect).
    pub fn clear(&mut self) {
        self.order.clear();
        self.latest.clear();
        self.frames.clear();
        self.next_allowed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IV: Duration = Duration::from_millis(25);

    fn enc(p: &Param) -> Vec<Vec<u8>> {
        match p {
            Param::PhonesVol { .. } => vec![vec![1], vec![2]],
            Param::MixVol { ch, .. } => vec![vec![u8::try_from(*ch).unwrap_or(255)]],
            _ => Vec::new(),
        }
    }

    #[test]
    fn pacer_spaces_every_frame_including_multi_frame_params() {
        let mut p = Pacer::new();
        let t0 = Instant::now();
        p.push(vol(1, 0.1));
        p.push(Param::PhonesVol { db: -3.0 });
        p.push(vol(2, 0.2));
        let mut sent = Vec::new();
        let mut now = t0;
        while !p.is_empty() {
            if let Some(f) = p.pop_due(now, enc) {
                sent.push((now, f));
                p.sent(now);
            } else {
                now += Duration::from_millis(1);
            }
        }
        assert_eq!(
            sent.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
            vec![vec![1], vec![1], vec![2], vec![2]]
        );
        for w in sent.windows(2) {
            assert!(w[1].0 - w[0].0 >= MIN_FRAME_SPACING);
        }
    }

    #[test]
    fn pacer_coalesces_by_key_and_stays_bounded() {
        let mut p = Pacer::new();
        for i in 0..1000u32 {
            p.push(vol(u16::try_from(i % 5).unwrap_or(0), f64::from(i)));
        }
        assert_eq!(p.queued_params(), 5);
        let mut last = std::collections::HashMap::new();
        let mut t = Instant::now();
        let mut n = 0;
        while !p.is_empty() {
            let f = p.pop_due(t, |q| {
                if let Param::MixVol { ch, gain, .. } = q {
                    last.insert(*ch, *gain);
                }
                enc(q)
            });
            assert_eq!(f.map(|f| f.len()), Some(1));
            p.sent(t);
            t += MIN_FRAME_SPACING;
            n += 1;
        }
        assert_eq!(n, 5);
        assert_eq!(last.len(), 5);
        assert_eq!(last.get(&0), Some(&995.0));
        assert_eq!(last.get(&4), Some(&999.0));
    }

    #[test]
    fn pacer_deadline_follows_last_send() {
        let mut p = Pacer::new();
        let t0 = Instant::now();
        assert_eq!(p.next_deadline(t0), None);
        p.push(vol(1, 0.1));
        p.push(vol(2, 0.2));
        assert_eq!(p.next_deadline(t0), Some(t0));
        assert!(p.pop_due(t0, enc).is_some());
        p.sent(t0);
        assert!(p.pop_due(t0 + Duration::from_millis(19), enc).is_none());
        assert_eq!(p.next_deadline(t0), Some(t0 + MIN_FRAME_SPACING));
        assert!(p.pop_due(t0 + MIN_FRAME_SPACING, enc).is_some());
    }

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
