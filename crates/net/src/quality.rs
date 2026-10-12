//! Latency handling around the rollback session: adaptive input delay,
//! rollback-safe event (sound / announcer) filtering, and a connection
//! quality tracker for the HUD.

use std::collections::VecDeque;

use rugen_sim::world::Event;

/// Input delay for a measured round trip: one-way latency in frames minus
/// one (rollback hides the rest), clamped to 0..=3. Roughly 0-1 frames on a
/// LAN or within a city, 2-3 across a continent.
pub fn auto_input_delay(rtt_ms: u32) -> usize {
    // ceil(rtt / 2 / 16.67 ms) in integers: frames = ceil(rtt * 60 / 2000)
    let one_way_frames = (rtt_ms as i64 * 60 + 1999) / 2000;
    (one_way_frames - 1).clamp(0, 3) as usize
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
    fn delay_follows_round_trip() {
        assert_eq!(auto_input_delay(0), 0);
        assert_eq!(auto_input_delay(12), 0);
        assert_eq!(auto_input_delay(40), 1);
        assert_eq!(auto_input_delay(80), 2);
        assert_eq!(auto_input_delay(120), 3);
        assert_eq!(auto_input_delay(400), 3);
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
