//! Server-side on/off of Monitor (Main) and Phones (docs/IOS-PLAN.md §5.2).
//!
//! The fade curve mirrors the browser's former one (`frontend/src/lib/fade.ts`): smoothstep,
//! linear in dB, 12 ms per dB clamped to 250–1200 ms, a step every 28 ms (under the 40 Hz
//! per-parameter write limit towards the MOTU), whole-dB writes. The engine owns one
//! [`Fade`] per channel and a timer branch in its loop; this module is pure (no I/O besides
//! the small remembered-level file) so the curve and its timing are tested on their own.

use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::plug::PlugState;
use crate::state::TRIM_MIN_DB;

/// Longest fade: a full sweep from the minimum to 0 dB (100 dB).
pub const FADE_MAX_MS: f64 = 1200.0;
pub const FADE_MIN_MS: f64 = 250.0;
/// Span (dB) that takes [`FADE_MAX_MS`].
pub const FADE_FULL_DB: f64 = -TRIM_MIN_DB;
/// Pace of the intermediate writes.
pub const FADE_STEP_MS: u64 = 28;
/// What "On" fades to when no level was remembered.
pub const FADE_SAFE_DB: f64 = -70.0;
/// A level further than this from a step the fade wrote is somebody else's move.
pub const CANCEL_TOLERANCE_DB: f64 = 0.5;
/// How many recent steps count as "ours" (a late hardware echo of an earlier step).
const RECENT_STEPS: usize = 8;

/// Which volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerChannel {
    /// Monitor (Main out).
    Main,
    Phones,
}

impl PowerChannel {
    pub const ALL: [Self; 2] = [Self::Main, Self::Phones];

    pub fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Phones => 1,
        }
    }
}

/// Direction of a running fade (`"in"` / `"out"` in JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FadeDir {
    In,
    Out,
}

/// One channel of [`crate::state::ServerEvent::Power`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelPower {
    /// Level above the minimum.
    pub on: bool,
    pub fading: Option<FadeDir>,
    /// Speaker plug state (Main only; `null` when no plug is enabled).
    pub plug: Option<PlugState>,
    /// Whole dB an On fades back to, while the channel is off or fading out (the remembered
    /// level, or [`FADE_SAFE_DB`] when none is); `null` while it is on or fading in. An older
    /// client ignores it.
    #[serde(default)]
    pub return_db: Option<f64>,
}

impl ChannelPower {
    /// What an OSC toggle should show: where the channel is heading.
    pub fn intent(&self) -> bool {
        match self.fading {
            Some(FadeDir::In) => true,
            Some(FadeDir::Out) => false,
            None => self.on,
        }
    }
}

/// A volume counts as off when it sits at (or within half a dB of) the minimum.
pub fn is_off(db: f64) -> bool {
    db <= TRIM_MIN_DB + 0.5
}

/// JavaScript's `Math.round` (halves go up), so the curve matches the browser's to the dB.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Smoothstep: gentle start and end, so the fade does not click in or out.
pub fn ease(t: f64) -> f64 {
    let c = t.clamp(0.0, 1.0);
    c * c * (3.0 - 2.0 * c)
}

/// dB at time `t` (0..1) of a fade: linear in dB with an ease.
pub fn fade_db(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * ease(t)
}

/// Duration (ms) of a fade over `span_db` decibels: 250 ms to 1.2 s (12 ms per dB).
pub fn fade_ms(span_db: f64) -> f64 {
    (span_db.abs() * FADE_MAX_MS / FADE_FULL_DB).clamp(FADE_MIN_MS, FADE_MAX_MS)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadePlan {
    pub from: f64,
    pub to: f64,
    pub ms: f64,
    /// Value of the last write: the minimum for a fade out, the target for a fade in.
    pub end: f64,
}

/// Fade out from `current` all the way to the minimum.
pub fn plan_out(current: f64) -> FadePlan {
    let from = current.clamp(TRIM_MIN_DB, 0.0);
    FadePlan {
        from,
        to: TRIM_MIN_DB,
        ms: fade_ms(from - TRIM_MIN_DB),
        end: TRIM_MIN_DB,
    }
}

/// Fade in from `current` up to the remembered level (never above it, nor above 0 dB;
/// [`FADE_SAFE_DB`] when nothing is remembered).
pub fn plan_in(current: f64, remembered: Option<f64>) -> FadePlan {
    let cap = remembered
        .filter(|r| r.is_finite())
        .map_or(FADE_SAFE_DB, js_round)
        .min(0.0);
    let target = cap.max(TRIM_MIN_DB);
    let from = target.min(current.max(TRIM_MIN_DB));
    FadePlan {
        from,
        to: target,
        ms: fade_ms(target - from),
        end: target,
    }
}

/// The level an On fades back to: what [`plan_in`] heads for.
pub fn return_level(remembered: Option<f64>) -> f64 {
    plan_in(TRIM_MIN_DB, remembered).to
}

/// Whole-dB value to write at time `t` (0..1), clamped to the plan's range.
pub fn value_at(plan: &FadePlan, t: f64) -> f64 {
    if t >= 1.0 {
        return plan.end;
    }
    let lo = plan.from.min(plan.to);
    let hi = plan.from.max(plan.to);
    js_round(fade_db(plan.from, plan.to, t)).clamp(lo, hi)
}

/// A remembered level is only valid when it is a real one.
pub fn valid_remembered(db: f64) -> bool {
    db.is_finite() && db > TRIM_MIN_DB + 0.5 && db <= 0.0
}

/// A running fade of one channel.
#[derive(Debug, Clone)]
pub struct Fade {
    pub dir: FadeDir,
    plan: FadePlan,
    start: Instant,
    last_sent: Option<f64>,
    /// The value as the fade last left it (a reversal starts from it, so it is continuous).
    level: f64,
    /// Values this fade wrote lately (plus the one it started from).
    recent: VecDeque<f64>,
    /// When the next step is due.
    due: Instant,
    /// Main only: switch the speaker plug off once the fade ends at the minimum.
    pub plug_off_after: bool,
}

/// Result of [`Fade::tick`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Whole-dB value to write now (`None` when it did not change).
    pub write: Option<f64>,
    /// The fade reached its end.
    pub done: bool,
}

impl Fade {
    pub fn new(
        dir: FadeDir,
        plan: FadePlan,
        now: Instant,
        current: f64,
        plug_off_after: bool,
    ) -> Self {
        Self {
            dir,
            plan,
            start: now,
            last_sent: None,
            level: current,
            recent: VecDeque::from([current]),
            due: now,
            plug_off_after,
        }
    }

    /// The last value written (where a reversal starts).
    pub fn level(&self) -> f64 {
        self.level
    }

    /// Advance to `now`.
    pub fn tick(&mut self, now: Instant) -> Tick {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f64() * 1000.0;
        let t = if self.plan.ms > 0.0 {
            elapsed / self.plan.ms
        } else {
            1.0
        };
        let v = value_at(&self.plan, t);
        self.level = v;
        let done = t >= 1.0;
        // The last step is always written: something else may have moved the knob meanwhile.
        let write = (done || self.last_sent != Some(v)).then_some(v);
        if write.is_some() {
            self.last_sent = Some(v);
            self.recent.push_back(v);
            if self.recent.len() > RECENT_STEPS {
                self.recent.pop_front();
            }
        }
        self.due = self.following_step(now);
        Tick { write, done }
    }

    /// When the next step is due: every [`FADE_STEP_MS`] on a grid from the start, and exactly
    /// at the end.
    pub fn next_deadline(&self) -> Instant {
        self.due
    }

    fn following_step(&self, now: Instant) -> Instant {
        let step = Duration::from_millis(FADE_STEP_MS);
        let elapsed = now.saturating_duration_since(self.start);
        let k = elapsed.as_millis() / u128::from(FADE_STEP_MS) + 1;
        let next = self.start + step.saturating_mul(u32::try_from(k).unwrap_or(u32::MAX));
        let end = self.start + Duration::from_secs_f64(self.plan.ms / 1000.0);
        next.min(end)
    }

    /// `true` when `db` is one of the values this fade wrote (the hardware's own echo).
    pub fn is_echo(&self, db: f64) -> bool {
        is_echo_of(self.recent.iter().copied(), db)
    }

    /// The values written lately, to carry into the next fade or to match late echoes.
    pub fn recent_steps(&self) -> Vec<f64> {
        self.recent.iter().copied().collect()
    }

    /// Add steps an earlier fade wrote (a reversal keeps matching their echoes).
    pub fn seed_recent(&mut self, steps: &[f64]) {
        let mut all: VecDeque<f64> = steps.iter().copied().collect();
        all.extend(self.recent.iter().copied());
        while all.len() > RECENT_STEPS {
            all.pop_front();
        }
        self.recent = all;
    }
}

/// `true` when `db` is within the tolerance of one of `steps`.
pub fn is_echo_of(steps: impl IntoIterator<Item = f64>, db: f64) -> bool {
    steps
        .into_iter()
        .any(|v| (db - v).abs() <= CANCEL_TOLERANCE_DB)
}

/// Remembered levels, `[main, phones]`, saved next to the labels file.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Remembered {
    pub main: Option<f64>,
    pub phones: Option<f64>,
}

impl Remembered {
    pub fn get(&self, ch: PowerChannel) -> Option<f64> {
        match ch {
            PowerChannel::Main => self.main,
            PowerChannel::Phones => self.phones,
        }
    }

    pub fn set(&mut self, ch: PowerChannel, v: Option<f64>) {
        match ch {
            PowerChannel::Main => self.main = v,
            PowerChannel::Phones => self.phones = v,
        }
    }

    fn sanitized(self) -> Self {
        let ok = |v: Option<f64>| v.filter(|d| valid_remembered(*d));
        Self {
            main: ok(self.main),
            phones: ok(self.phones),
        }
    }
}

/// Read the file; a missing file is "nothing remembered".
pub async fn load_remembered(path: &Path) -> Result<Remembered> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => serde_json::from_str::<Remembered>(&text)
            .map(Remembered::sanitized)
            .with_context(|| format!("invalid {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Remembered::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

pub async fn save_remembered(path: &Path, r: &Remembered) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    crate::presets::write_atomic(path, &serde_json::to_vec_pretty(r)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same checks as `frontend/src/lib/fade.test.ts`, point for point where it matters.
    #[test]
    fn ease_and_duration_match_the_browser() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(0.5), 0.5);
        assert_eq!(ease(-3.0), 0.0);
        assert_eq!(ease(7.0), 1.0);
        assert!(ease(0.25) < 0.25 && ease(0.75) > 0.75);
        assert_eq!(fade_ms(100.0), 1200.0);
        assert_eq!(fade_ms(-100.0), 1200.0);
        assert_eq!(fade_ms(50.0), 600.0);
        assert_eq!(fade_ms(1.0), 250.0, "short spans take the minimum");
        assert_eq!(fade_ms(0.0), 250.0);
        assert_eq!(fade_ms(500.0), 1200.0);
    }

    #[test]
    fn plans() {
        let out = plan_out(-20.0);
        assert_eq!((out.from, out.to, out.end), (-20.0, -100.0, -100.0));
        assert_eq!(out.ms, 960.0);
        // Outside the knob's range is clamped.
        assert_eq!(plan_out(12.0).from, 0.0);
        assert_eq!(plan_out(-500.0).from, -100.0);
        // In: up to the remembered level.
        let p = plan_in(-100.0, Some(-12.0));
        assert_eq!((p.from, p.to, p.end), (-100.0, -12.0, -12.0));
        assert_eq!(p.ms, 1056.0);
        // Nothing remembered: the safe level.
        assert_eq!(plan_in(-100.0, None).to, FADE_SAFE_DB);
        // Never above 0 dB, never above the remembered level when mid-fade.
        assert_eq!(plan_in(-100.0, Some(5.0)).to, 0.0);
        let mid = plan_in(-30.0, Some(-50.0));
        assert_eq!((mid.from, mid.to), (-50.0, -50.0));
        assert_eq!(plan_in(-100.0, Some(f64::NAN)).to, FADE_SAFE_DB);
        // Math.round(-12.5) is -12 in JavaScript.
        assert_eq!(plan_in(-100.0, Some(-12.5)).to, -12.0);
    }

    #[test]
    fn values_stay_in_range_and_end_exactly() {
        let plan = plan_out(-20.0);
        assert_eq!(value_at(&plan, 0.0), -20.0);
        assert_eq!(value_at(&plan, 1.0), -100.0);
        assert_eq!(value_at(&plan, 2.0), -100.0);
        assert_eq!(value_at(&plan, 0.5), -60.0);
        let mut last = f64::MAX;
        for i in 0..=100 {
            let v = value_at(&plan, f64::from(i) / 100.0);
            assert!((-100.0..=-20.0).contains(&v));
            assert!(v <= last, "a fade out never goes back up");
            last = v;
        }
        let up = plan_in(-100.0, Some(-10.0));
        let mut last = f64::MIN;
        for i in 0..=100 {
            let v = value_at(&up, f64::from(i) / 100.0);
            assert!((-100.0..=-10.0).contains(&v));
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn off_threshold() {
        assert!(is_off(-100.0));
        assert!(is_off(-99.6));
        assert!(!is_off(-99.0));
        assert!(!is_off(0.0));
        assert!(valid_remembered(-12.0));
        assert!(!valid_remembered(-100.0));
        assert!(!valid_remembered(3.0));
        assert!(!valid_remembered(f64::NAN));
    }

    #[tokio::test(start_paused = true)]
    async fn timing_steps_every_28_ms_and_ends_at_the_plan_duration() {
        let t0 = Instant::now();
        let mut fade = Fade::new(FadeDir::Out, plan_out(0.0), t0, 0.0, false);
        // 100 dB: 1200 ms.
        let mut now = t0;
        let mut writes = Vec::new();
        let mut ticks = 0;
        loop {
            let tick = fade.tick(now);
            ticks += 1;
            if let Some(v) = tick.write {
                writes.push((now - t0, v));
            }
            if tick.done {
                break;
            }
            let next = fade.next_deadline();
            assert!(next > now);
            assert!(
                next - now <= Duration::from_millis(FADE_STEP_MS),
                "never longer than a step"
            );
            now = next;
            assert!(ticks < 100, "must terminate");
        }
        assert_eq!(
            now - t0,
            Duration::from_millis(1200),
            "ends exactly on time"
        );
        assert_eq!(writes.first().map(|w| w.1), Some(0.0));
        assert_eq!(writes.last().map(|w| w.1), Some(-100.0));
        // Steps are on the 28 ms grid, apart from the last one.
        for (at, _) in &writes[..writes.len() - 1] {
            assert_eq!(at.as_millis() % u128::from(FADE_STEP_MS), 0);
        }
        // Whole dB, monotonic, no repeated value.
        for w in writes.windows(2) {
            assert!(w[1].1 <= w[0].1);
            assert_eq!(w[1].1.fract(), 0.0);
        }
        let (body, _) = writes.split_at(writes.len() - 1);
        assert!(
            body.windows(2).all(|w| w[1].1 < w[0].1),
            "no repeats before the last write"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_fade_takes_the_minimum_duration() {
        let t0 = Instant::now();
        let plan = plan_in(-100.0, Some(-95.0));
        let mut fade = Fade::new(FadeDir::In, plan, t0, -100.0, false);
        let at_end = t0 + Duration::from_millis(250);
        let mut now = t0;
        while now < at_end {
            assert!(!fade.tick(now).done);
            now = fade.next_deadline();
        }
        assert_eq!(now, at_end);
        let last = fade.tick(now);
        assert!(last.done);
        assert_eq!(fade.level(), -95.0);
        assert_eq!(last.write, Some(-95.0), "the end is always written");
    }

    #[tokio::test(start_paused = true)]
    async fn seeded_steps_keep_their_order_and_the_newest_survive() {
        let t0 = Instant::now();
        let mut fade = Fade::new(FadeDir::In, plan_in(-100.0, Some(-10.0)), t0, -100.0, false);
        let old: Vec<f64> = (0..10).map(|i| -50.0 + f64::from(i) * 3.0).collect();
        fade.seed_recent(&old);
        let kept = fade.recent_steps();
        assert_eq!(kept.len(), 8);
        assert_eq!(kept.last(), Some(&-100.0), "its own start stays last");
        assert!(
            fade.is_echo(old[9]),
            "the newest old step is still recognised"
        );
        assert!(!fade.is_echo(old[0]), "the oldest were dropped");
    }

    #[tokio::test(start_paused = true)]
    async fn echoes_are_recognised_but_other_levels_are_not() {
        let t0 = Instant::now();
        let mut fade = Fade::new(FadeDir::Out, plan_out(-10.0), t0, -10.0, false);
        fade.tick(t0 + Duration::from_millis(200));
        let level = fade.level();
        assert!(fade.is_echo(level));
        assert!(fade.is_echo(level + 0.4), "half a dB of tolerance");
        assert!(fade.is_echo(-10.0), "the level it started from");
        assert!(!fade.is_echo(level + 5.0));
        assert!(!fade.is_echo(-95.0));
    }

    #[tokio::test]
    async fn remembered_levels_persist_and_are_sanitised() {
        let dir = std::env::temp_dir().join(format!("cuemix-power-{}", std::process::id()));
        let path = dir.join("power.json");
        assert_eq!(load_remembered(&path).await.unwrap(), Remembered::default());
        let r = Remembered {
            main: Some(-12.0),
            phones: None,
        };
        save_remembered(&path, &r).await.unwrap();
        assert_eq!(load_remembered(&path).await.unwrap(), r);
        // A hand-edited file cannot smuggle in a level that is not a real one.
        tokio::fs::write(&path, r#"{"main":-100,"phones":7}"#)
            .await
            .unwrap();
        assert_eq!(load_remembered(&path).await.unwrap(), Remembered::default());
        tokio::fs::write(&path, "not json").await.unwrap();
        assert!(load_remembered(&path).await.is_err());
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[test]
    fn channel_power_intent_and_json() {
        let p = ChannelPower {
            on: true,
            fading: Some(FadeDir::Out),
            plug: Some(PlugState::Warming),
            return_db: Some(-20.0),
        };
        assert!(!p.intent());
        assert_eq!(
            serde_json::to_value(p).unwrap(),
            serde_json::json!({"on":true,"fading":"out","plug":"warming","return_db":-20.0})
        );
        let q = ChannelPower {
            on: false,
            fading: Some(FadeDir::In),
            plug: None,
            return_db: None,
        };
        assert!(q.intent());
        assert_eq!(
            serde_json::to_value(q).unwrap(),
            serde_json::json!({"on":false,"fading":"in","plug":null,"return_db":null})
        );
        // An older event without the field still reads.
        let old: ChannelPower =
            serde_json::from_value(serde_json::json!({"on":false,"fading":null,"plug":null}))
                .unwrap();
        assert_eq!(old.return_db, None);
    }

    #[test]
    fn return_level_is_where_on_fades_to() {
        assert_eq!(return_level(Some(-18.4)), -18.0);
        assert_eq!(return_level(None), FADE_SAFE_DB);
        assert_eq!(return_level(Some(5.0)), 0.0);
    }
}
