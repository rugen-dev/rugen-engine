//! Latency handling around the rollback session: the hybrid input delay
//! policy, smooth time synchronisation (time dilation instead of frame
//! skips), round-trip statistics, misprediction accounting, rollback-safe
//! event (sound / announcer) filtering, and a connection quality tracker
//! for the HUD.

use std::collections::VecDeque;
use std::time::Duration;

use rugen_sim::world::Event;

/// One simulation tick at 60 Hz.
pub const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);
const FRAME_MS: f64 = 1000.0 / 60.0;

/// Hybrid delay policy (the "rollback first, delay only beyond it" model):
/// the rollback window hides the first `prediction_window - headroom`
/// frames of one-way latency; only latency beyond that becomes input
/// delay, never less than `min_delay` and never more than 8 frames.
///
/// `delay = clamp(ceil(one_way_ms / 16.67) - (window - headroom), min, 8)`
/// with `one_way_ms = p95 round trip / 2 + 1 frame` (an input is sent at
/// the end of the tick that sampled it and used at the next tick boundary).
///
/// Bands with the defaults (window 10, headroom 2, min 1), by p95 round
/// trip of the chosen path:
///
/// | p95 RTT        | delay | frames rolled back at most |
/// |----------------|-------|----------------------------|
/// | 0 - 266 ms     | 1     | up to 8 (180 ms: 6)        |
/// | 267 - 300 ms   | 2     | 8                          |
/// | 301 - 333 ms   | 3     | 8                          |
/// | 334 - 366 ms   | 4     | 8                          |
/// | each +33 ms    | +1    | 8 (delay capped at 8)      |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelayPolicy {
    pub min_delay: usize,
    /// ggrs prediction window (frames the session may run ahead of the
    /// last confirmed input).
    pub prediction_window: usize,
    /// Frames of the window kept free for jitter and tick quantisation, so
    /// the window is not hit (a hard stall) on every late packet.
    pub headroom: usize,
}

impl Default for DelayPolicy {
    fn default() -> Self {
        Self {
            min_delay: 1,
            prediction_window: 10,
            headroom: 2,
        }
    }
}

impl DelayPolicy {
    pub const MAX_DELAY: usize = 8;

    /// Input delay for a path whose round trip is `rtt_p95_ms` (95th
    /// percentile of the samples, which folds the jitter in).
    pub fn delay(&self, rtt_p95_ms: u32) -> usize {
        let one_way = f64::from(rtt_p95_ms) / 2.0 + FRAME_MS;
        let frames = (one_way / FRAME_MS - 1e-9).ceil() as i64;
        let covered = self.prediction_window.saturating_sub(self.headroom) as i64;
        (frames - covered).clamp(self.min_delay as i64, Self::MAX_DELAY as i64) as usize
    }

    /// Frames the session predicts at that round trip with that delay
    /// (the depth of a typical rollback).
    pub fn lead(&self, rtt_p95_ms: u32, delay: usize) -> usize {
        let one_way = f64::from(rtt_p95_ms) / 2.0 + FRAME_MS;
        ((one_way / FRAME_MS - 1e-9).ceil() as usize).saturating_sub(delay)
    }
}

/// Input delay for a measured round trip with the default [`DelayPolicy`]
/// (`[net] input_delay = "auto"`).
pub fn auto_input_delay(rtt_ms: u32) -> usize {
    DelayPolicy::default().delay(rtt_ms)
}

/// Round-trip summary of a path: median, 95th percentile and jitter (mean
/// absolute deviation from the median), all in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RttStats {
    pub median: u32,
    pub p95: u32,
    pub jitter: u32,
    pub samples: usize,
}

impl RttStats {
    /// From round-trip samples in microseconds (`None` without samples).
    pub fn from_us(samples: &[u32]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut s = samples.to_vec();
        s.sort_unstable();
        let ms = |us: u32| (us + 500) / 1000;
        let median = s[s.len() / 2];
        let p95 = s[((s.len() * 95).div_ceil(100)).clamp(1, s.len()) - 1];
        let dev = s
            .iter()
            .map(|&v| u64::from(v.abs_diff(median)))
            .sum::<u64>()
            / s.len() as u64;
        Some(Self {
            median: ms(median),
            p95: ms(p95),
            jitter: ms(dev as u32),
            samples: s.len(),
        })
    }
}

/// Smooth time synchronisation: when this side runs ahead of the peer it
/// slows its tick slightly (down to ~58-59 Hz for a frame or two of
/// advantage) for as long as it takes, instead of freezing whole frames
/// (ggrs' wait recommendation). Rendering keeps the display rate; only the
/// simulation clock stretches, so the peer catches up without a visible
/// stall.
#[derive(Clone, Debug)]
pub struct Pacer {
    /// Smoothed frame advantage (positive: we are ahead).
    ahead: f32,
    /// Slow-down per frame of advantage beyond the dead zone.
    gain: f32,
    dead_zone: f32,
    max_slowdown: f32,
    primed: bool,
}

impl Default for Pacer {
    fn default() -> Self {
        Self {
            ahead: 0.0,
            gain: 0.015,
            dead_zone: 0.25,
            max_slowdown: 0.08,
            primed: false,
        }
    }
}

impl Pacer {
    /// Feed ggrs' latest frame advantage estimates for the remote player:
    /// `local_behind` = frames we are behind the peer, `remote_behind` =
    /// frames the peer says it is behind us (`NetworkStats`).
    pub fn update(&mut self, local_behind: i32, remote_behind: i32) {
        let ahead = (remote_behind - local_behind) as f32 / 2.0;
        if self.primed {
            // ~0.5 s time constant at 60 Hz: reacts to drift, not to jitter
            self.ahead += (ahead - self.ahead) * 0.035;
        } else {
            self.ahead = ahead;
            self.primed = true;
        }
    }

    /// Smoothed advantage in frames (positive: ahead of the peer).
    pub fn ahead(&self) -> f32 {
        self.ahead
    }

    /// Tick length multiplier (1.0 = 60 Hz; 1.026 = 58.5 Hz, two frames ahead).
    pub fn scale(&self) -> f32 {
        1.0 + ((self.ahead - self.dead_zone) * self.gain).clamp(0.0, self.max_slowdown)
    }

    /// Current tick length.
    pub fn tick(&self) -> Duration {
        TICK.mul_f32(self.scale())
    }
}

/// Counts how often the first prediction of the remote player's input for
/// a frame turned out wrong (ggrs repeats the last received input).
#[derive(Clone, Debug, Default)]
pub struct Mispredictions {
    /// First-time predicted frames waiting for their confirmed input.
    pending: VecDeque<(u32, u16)>,
    pub predicted: u64,
    pub wrong: u64,
}

impl Mispredictions {
    /// A frame simulated with the remote input `input`: `first` when the
    /// frame is reached for the first time, `predicted` when the input is
    /// a prediction.
    pub fn simulated(&mut self, frame: u32, input: u16, first: bool, predicted: bool) {
        if first {
            if predicted {
                self.pending.push_back((frame, input));
                if self.pending.len() > 256 {
                    self.pending.pop_front();
                }
            }
        } else if !predicted && let Some(i) = self.pending.iter().position(|(f, _)| *f == frame) {
            let (_, guess) = self.pending.remove(i).unwrap_or_default();
            self.predicted += 1;
            if guess != input {
                self.wrong += 1;
            }
        }
    }

    /// Every frame up to `confirmed` has its real input now: predictions
    /// that were never corrected by a rollback were right.
    pub fn confirmed(&mut self, confirmed: i32) {
        while self
            .pending
            .front()
            .is_some_and(|(f, _)| (*f as i64) <= confirmed as i64)
        {
            self.pending.pop_front();
            self.predicted += 1;
        }
    }

    /// Wrong first predictions / first predictions.
    pub fn rate(&self) -> f32 {
        if self.predicted == 0 {
            0.0
        } else {
            self.wrong as f32 / self.predicted as f32
        }
    }
}

/// Same sound / spark regardless of volume or exact position: resimulated
/// frames may differ in such details but are still "that" event.
fn same_event(a: &Event, b: &Event) -> bool {
    match (a, b) {
        (
            Event::Sound {
                player: p1,
                own: o1,
                group: g1,
                number: n1,
                ..
            },
            Event::Sound {
                player: p2,
                own: o2,
                group: g2,
                number: n2,
                ..
            },
        ) => p1 == p2 && o1 == o2 && g1 == g2 && n1 == n2,
        (
            Event::Spark {
                player: p1,
                anim: a1,
                ..
            },
            Event::Spark {
                player: p2,
                anim: a2,
                ..
            },
        ) => p1 == p2 && a1 == a2,
        _ => a == b,
    }
}

/// Round flow calls (announcer, banners) are deduplicated across a window
/// rather than per frame: a rollback may move a KO by a frame or two.
fn is_round_call(e: &Event) -> bool {
    matches!(
        e,
        Event::RoundStart(_)
            | Event::Fight
            | Event::KO
            | Event::TimeOver
            | Event::RoundWon(_)
            | Event::MatchOver(_)
    )
}

/// Decides which simulation events reach audio / effects when ggrs replays
/// frames after a rollback. Every event is keyed by (frame, event identity):
///
/// * a frame reached for the first time emits all its events;
/// * a re-simulated frame emits only events that were not emitted for that
///   frame before (e.g. the hit sound of an attack the prediction missed);
///   nothing already playing is replayed or cut;
/// * `StopSound` only acts on first-time frames;
/// * round calls (FIGHT, KO, ...) are emitted once per `window` frames.
pub struct EventFilter {
    window: u32,
    /// (frame, events emitted for it), oldest first.
    seen: VecDeque<(u32, Vec<Event>)>,
    calls: VecDeque<(u32, Event)>,
    highest: u32,
}

impl EventFilter {
    pub fn new(window: u32) -> Self {
        Self {
            window: window.max(1),
            seen: VecDeque::new(),
            calls: VecDeque::new(),
            highest: 0,
        }
    }

    pub fn highest(&self) -> u32 {
        self.highest
    }

    /// Events of `frame` (just simulated) that should be played.
    pub fn filter(&mut self, frame: u32, events: &[Event]) -> Vec<Event> {
        let fresh = frame > self.highest;
        if fresh {
            self.highest = frame;
        }
        let horizon = self.highest.saturating_sub(self.window * 2);
        while self.seen.front().is_some_and(|(f, _)| *f < horizon) {
            self.seen.pop_front();
        }
        while self.calls.front().is_some_and(|(f, _)| *f < horizon) {
            self.calls.pop_front();
        }
        let idx = match self.seen.iter().position(|(f, _)| *f == frame) {
            Some(i) => i,
            None => {
                self.seen.push_back((frame, Vec::new()));
                self.seen.len() - 1
            }
        };
        let before = self.seen[idx].1.clone();
        let mut out = Vec::new();
        for (i, e) in events.iter().enumerate() {
            if is_round_call(e) {
                let dup = self
                    .calls
                    .iter()
                    .any(|(f, c)| c == e && frame.abs_diff(*f) <= self.window);
                if !dup {
                    self.calls.push_back((frame, e.clone()));
                    out.push(e.clone());
                }
                continue;
            }
            if matches!(e, Event::StopSound { .. }) {
                if fresh {
                    out.push(e.clone());
                }
                continue;
            }
            // the k-th identical sound of a frame plays only if fewer than k
            // were played for that frame before (two hits = two sounds)
            let k = events[..i].iter().filter(|p| same_event(p, e)).count();
            let already = before.iter().filter(|p| same_event(p, e)).count();
            if k >= already {
                self.seen[idx].1.push(e.clone());
                out.push(e.clone());
            }
        }
        out
    }
}

/// Rolling connection quality: ping, frame advantage, rollback depth.
#[derive(Clone, Debug, Default)]
pub struct Quality {
    /// Frames re-simulated per tick over the last second.
    depth: VecDeque<u32>,
    pub interrupted: bool,
}

impl Quality {
    pub fn push_tick(&mut self, resimulated: u32) {
        self.depth.push_back(resimulated);
        if self.depth.len() > 60 {
            self.depth.pop_front();
        }
    }

    /// Average rollback depth over the last second.
    pub fn avg_rollback(&self) -> f32 {
        if self.depth.is_empty() {
            return 0.0;
        }
        self.depth.iter().sum::<u32>() as f32 / self.depth.len() as f32
    }

    /// "Connection unstable": interrupted, far ahead of the peer, or deep rollbacks.
    pub fn unstable(&self, frames_ahead: i32) -> bool {
        self.interrupted || frames_ahead.abs() >= 4 || self.avg_rollback() >= 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snd(n: i32) -> Event {
        Event::Sound {
            player: 0,
            own: false,
            group: 5,
            number: n,
            channel: -1,
            volume: 100,
        }
    }

    #[test]
    fn delay_policy_bands() {
        let p = DelayPolicy::default();
        // the rollback window covers everything up to ~266 ms
        for rtt in [0, 12, 40, 80, 120, 180, 233, 266] {
            assert_eq!(p.delay(rtt), 1, "{rtt} ms");
        }
        assert_eq!(p.delay(267), 2);
        assert_eq!(p.delay(300), 2);
        assert_eq!(p.delay(301), 3);
        assert_eq!(p.delay(333), 3);
        assert_eq!(p.delay(350), 4);
        assert_eq!(p.delay(2000), DelayPolicy::MAX_DELAY);
        // what is left is predicted, never past window - headroom
        assert_eq!(p.lead(180, 1), 6);
        for rtt in (0..400).step_by(7) {
            let d = p.delay(rtt);
            assert!(p.lead(rtt, d) <= p.prediction_window - p.headroom, "{rtt}");
        }
        // a 0-frame minimum and a smaller window
        let lan = DelayPolicy {
            min_delay: 0,
            prediction_window: 8,
            headroom: 2,
        };
        assert_eq!(lan.delay(10), 0);
        assert_eq!(lan.delay(180), 1);
        assert_eq!(auto_input_delay(180), 1);
    }

    #[test]
    fn rtt_stats_fold_jitter_into_p95() {
        assert_eq!(RttStats::from_us(&[]), None);
        let mut v: Vec<u32> = (0..100).map(|i| 180_000 + i * 100).collect();
        v[99] = 400_000;
        let s = RttStats::from_us(&v).unwrap();
        assert_eq!(s.median, 185);
        assert_eq!(s.p95, 189);
        assert!(s.jitter <= 5, "{s:?}");
        assert_eq!(s.samples, 100);
        assert_eq!(RttStats::from_us(&[50_000]).unwrap().p95, 50);
    }

    #[test]
    fn pacer_slows_down_smoothly_when_ahead() {
        let mut p = Pacer::default();
        p.update(0, 0);
        assert_eq!(p.scale(), 1.0);
        assert_eq!(p.tick(), TICK);
        // two frames ahead (we are 2 ahead, the peer 2 behind)
        for _ in 0..300 {
            p.update(-2, 2);
        }
        let hz = 60.0 / p.scale();
        assert!((58.0..59.5).contains(&hz), "{hz} Hz");
        // never faster than 60 Hz, never slower than the cap
        let mut behind = Pacer::default();
        for _ in 0..300 {
            behind.update(3, -3);
        }
        assert_eq!(behind.scale(), 1.0);
        let mut far = Pacer::default();
        for _ in 0..300 {
            far.update(-40, 40);
        }
        assert!((far.scale() - 1.08).abs() < 1e-6);
        // one late packet barely moves it
        let mut j = Pacer::default();
        j.update(0, 0);
        j.update(-6, 6);
        assert!(j.scale() < 1.01);
    }

    #[test]
    fn mispredictions_count_first_guesses_only() {
        let mut m = Mispredictions::default();
        // frames 10..14 first simulated on predictions of input 5
        for f in 10..14 {
            m.simulated(f, 5, true, true);
        }
        // a rollback re-simulates 12, 13 with the real input 9
        m.simulated(12, 9, false, false);
        m.simulated(13, 9, false, false);
        // confirmed through 13: 10, 11 were right
        m.confirmed(13);
        assert_eq!((m.predicted, m.wrong), (4, 2));
        assert!((m.rate() - 0.5).abs() < 1e-6);
        // confirmed frames never count
        m.simulated(14, 1, true, false);
        m.confirmed(20);
        assert_eq!(m.predicted, 4);
    }

    #[test]
    fn rollback_does_not_replay_sounds() {
        let mut f = EventFilter::new(8);
        assert_eq!(f.filter(10, &[snd(1)]), vec![snd(1)]);
        assert_eq!(f.filter(11, &[]), vec![]);
        // rollback to 10: same sound again is not replayed
        assert_eq!(f.filter(10, &[snd(1)]), vec![]);
        // corrected input reveals a hit sound on frame 11: played once
        assert_eq!(f.filter(11, &[snd(2)]), vec![snd(2)]);
        assert_eq!(f.filter(11, &[snd(2)]), vec![]);
        // a louder variant of the same sound counts as the same
        let mut loud = snd(1);
        if let Event::Sound { volume, .. } = &mut loud {
            *volume = 255;
        }
        assert_eq!(f.filter(10, &[loud]), vec![]);
        // two identical sounds in one frame are both played
        assert_eq!(f.filter(12, &[snd(3), snd(3)]), vec![snd(3), snd(3)]);
        assert_eq!(f.filter(12, &[snd(3), snd(3), snd(3)]), vec![snd(3)]);
    }

    #[test]
    fn stop_sound_only_on_new_frames() {
        let mut f = EventFilter::new(8);
        let stop = Event::StopSound {
            player: 0,
            channel: -1,
        };
        assert_eq!(f.filter(5, std::slice::from_ref(&stop)), vec![stop.clone()]);
        assert_eq!(f.filter(5, std::slice::from_ref(&stop)), vec![]);
        assert_eq!(f.filter(4, std::slice::from_ref(&stop)), vec![]);
    }

    #[test]
    fn announcer_calls_survive_shifted_rollbacks() {
        let mut f = EventFilter::new(8);
        assert_eq!(f.filter(100, &[Event::KO]), vec![Event::KO]);
        // after a rollback the KO lands one frame later: still one call
        assert_eq!(f.filter(101, &[Event::KO]), vec![]);
        // much later (a new round) it is a new call
        assert_eq!(f.filter(400, &[Event::KO]), vec![Event::KO]);
    }

    #[test]
    fn quality_flags_spikes() {
        let mut q = Quality::default();
        for _ in 0..60 {
            q.push_tick(1);
        }
        assert!(!q.unstable(1));
        assert!(q.unstable(5));
        for _ in 0..60 {
            q.push_tick(6);
        }
        assert!(q.unstable(0));
    }
}
