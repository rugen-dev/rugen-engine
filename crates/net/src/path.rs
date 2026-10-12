//! Which path carries the match: the direct peer-to-peer path, TURN, or the
//! server relay (UDP or WebSocket), chosen by round trip and jitter with
//! hysteresis so a match does not flap between paths of similar quality.

use std::time::{Duration, Instant};

/// A player ↔ player path, best first when equally fast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PathKind {
    /// Peer to peer over UDP (host or server-reflexive candidates).
    Direct,
    /// Both players relayed by a TURN service.
    Turn,
    /// Through the rugen-server relay (UDP or WebSocket).
    #[default]
    Relay,
}

impl PathKind {
    /// HUD label.
    pub fn label(self) -> &'static str {
        match self {
            PathKind::Direct => "DIRECT",
            PathKind::Turn => "TURN",
            PathKind::Relay => "RELAY",
        }
    }

    /// Score bias in ms: on a tie the more direct path wins (fewer hops
    /// that can fail, TURN bytes are metered, and the server relay is the
    /// last resort: its bandwidth is the server's).
    fn bias(self) -> f32 {
        match self {
            PathKind::Direct => 0.0,
            PathKind::Turn => 5.0,
            PathKind::Relay => 30.0,
        }
    }
}

/// The path a match uses right now, for the HUD (shared with the ggrs
/// session's socket).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkInfo {
    pub kind: PathKind,
    /// `direct`, the TURN transport or the relay's (`UDP` / `WebSocket`).
    pub transport: &'static str,
    /// Smoothed round trip of that path (ms).
    pub rtt_ms: u32,
    pub switches: u32,
    /// Datagrams the direct path dropped as forged or replayed.
    pub forged: u64,
}

/// Latest measurement of one path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathSample {
    pub kind: PathKind,
    /// Smoothed round trip in ms (`None`: not measured yet).
    pub rtt_ms: Option<f32>,
    pub jitter_ms: f32,
    /// Carrying traffic works right now (heard from the peer recently).
    pub healthy: bool,
}

impl PathSample {
    fn score(&self) -> Option<f32> {
        self.rtt_ms
            .map(|rtt| rtt + 2.0 * self.jitter_ms + self.kind.bias())
    }
}

/// Path choice with hysteresis: switch at once away from a path that
/// stopped working; switch to a merely better path only when it is better
/// by a clear margin for a sustained time.
#[derive(Clone, Debug)]
pub struct PathSelector {
    current: PathKind,
    /// A better path and since when it has been better.
    better: Option<(PathKind, Instant)>,
    /// Required improvement: at least `margin_ms` and `margin_frac` of the
    /// current score.
    pub margin_ms: f32,
    pub margin_frac: f32,
    /// How long a path must stay better before switching to it.
    pub hold: Duration,
    pub switches: u32,
}

impl Default for PathSelector {
    fn default() -> Self {
        Self {
            current: PathKind::Relay,
            better: None,
            margin_ms: 15.0,
            margin_frac: 0.15,
            hold: Duration::from_secs(2),
            switches: 0,
        }
    }
}

impl PathSelector {
    pub fn current(&self) -> PathKind {
        self.current
    }

    /// Pick the best healthy, measured path now (match start: no hold).
    pub fn choose(&mut self, samples: &[PathSample]) -> PathKind {
        let best = Self::best(samples).unwrap_or(PathKind::Relay);
        if best != self.current {
            self.current = best;
            self.switches += 1;
        }
        self.better = None;
        self.current
    }

    fn best(samples: &[PathSample]) -> Option<PathKind> {
        samples
            .iter()
            .filter(|s| s.healthy)
            .filter_map(|s| s.score().map(|sc| (sc, s.kind)))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, k)| k)
    }

    /// Re-evaluate with the latest samples; returns the path to use.
    pub fn update(&mut self, now: Instant, samples: &[PathSample]) -> PathKind {
        let get = |k: PathKind| samples.iter().find(|s| s.kind == k);
        let cur = get(self.current);
        let cur_ok = self.current == PathKind::Relay || cur.is_some_and(|s| s.healthy);
        if !cur_ok {
            // the current path stopped working: best healthy one, now
            let next = Self::best(samples).unwrap_or(PathKind::Relay);
            if next != self.current {
                log::info!(
                    "net: {} path lost, switching to {}",
                    self.current.label(),
                    next.label()
                );
                self.current = next;
                self.switches += 1;
            }
            self.better = None;
            return self.current;
        }
        let cur_score = cur.and_then(PathSample::score).unwrap_or(f32::MAX);
        let threshold = cur_score - self.margin_ms.max(cur_score * self.margin_frac);
        let candidate = samples
            .iter()
            .filter(|s| s.healthy && s.kind != self.current)
            .filter_map(|s| s.score().map(|sc| (sc, s.kind)))
            .filter(|(sc, _)| *sc < threshold)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| k);
        match (candidate, self.better) {
            (None, _) => self.better = None,
            (Some(k), Some((b, since))) if k == b => {
                if now.duration_since(since) >= self.hold {
                    log::info!(
                        "net: switching from {} to {} (faster)",
                        self.current.label(),
                        k.label()
                    );
                    self.current = k;
                    self.switches += 1;
                    self.better = None;
                }
            }
            (Some(k), _) => self.better = Some((k, now)),
        }
        self.current
    }
}

/// Smoothed round trip and jitter (RFC 6298 style, in ms).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Srtt {
    pub rtt: Option<f32>,
    pub jitter: f32,
}

impl Srtt {
    pub fn sample(&mut self, ms: f32) {
        match self.rtt {
            None => {
                self.rtt = Some(ms);
                self.jitter = ms / 4.0;
            }
            Some(r) => {
                self.jitter += ((ms - r).abs() - self.jitter) / 4.0;
                self.rtt = Some(r + (ms - r) / 8.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(kind: PathKind, rtt: f32, healthy: bool) -> PathSample {
        PathSample {
            kind,
            rtt_ms: Some(rtt),
            jitter_ms: 2.0,
            healthy,
        }
    }

    #[test]
    fn start_picks_the_fastest_healthy_path() {
        let mut p = PathSelector::default();
        let all = [
            s(PathKind::Direct, 180.0, true),
            s(PathKind::Turn, 350.0, true),
            s(PathKind::Relay, 600.0, true),
        ];
        assert_eq!(p.choose(&all), PathKind::Direct);
        // direct not working: TURN
        let all = [
            s(PathKind::Direct, 180.0, false),
            s(PathKind::Turn, 350.0, true),
        ];
        assert_eq!(PathSelector::default().choose(&all), PathKind::Turn);
        // nothing measured: the relay
        assert_eq!(PathSelector::default().choose(&[]), PathKind::Relay);
        // a tie goes to the more direct path
        let tie = [
            s(PathKind::Turn, 100.0, true),
            s(PathKind::Direct, 103.0, true),
        ];
        assert_eq!(PathSelector::default().choose(&tie), PathKind::Direct);
    }

    #[test]
    fn hysteresis_needs_a_clear_and_sustained_win() {
        let t0 = Instant::now();
        let mut p = PathSelector::default();
        p.choose(&[s(PathKind::Turn, 200.0, true)]);
        assert_eq!(p.current(), PathKind::Turn);
        // direct shows up slightly faster: not worth a switch
        let close = [
            s(PathKind::Turn, 200.0, true),
            s(PathKind::Direct, 190.0, true),
        ];
        for i in 0..10 {
            assert_eq!(
                p.update(t0 + Duration::from_secs(i), &close),
                PathKind::Turn
            );
        }
        // clearly faster, but only briefly
        let fast = [
            s(PathKind::Turn, 200.0, true),
            s(PathKind::Direct, 120.0, true),
        ];
        let t1 = t0 + Duration::from_secs(20);
        assert_eq!(p.update(t1, &fast), PathKind::Turn);
        assert_eq!(
            p.update(t1 + Duration::from_secs(1), &close),
            PathKind::Turn
        );
        // sustained: switch after the hold time
        let t2 = t1 + Duration::from_secs(5);
        assert_eq!(p.update(t2, &fast), PathKind::Turn);
        assert_eq!(
            p.update(t2 + Duration::from_millis(1500), &fast),
            PathKind::Turn
        );
        assert_eq!(
            p.update(t2 + Duration::from_secs(2), &fast),
            PathKind::Direct
        );
        assert_eq!(p.switches, 2);
        // the direct path dies: back to TURN at once
        let dead = [
            s(PathKind::Turn, 200.0, true),
            s(PathKind::Direct, 120.0, false),
        ];
        assert_eq!(p.update(t2 + Duration::from_secs(3), &dead), PathKind::Turn);
        // everything dies: the relay
        assert_eq!(
            p.update(
                t2 + Duration::from_secs(4),
                &[s(PathKind::Turn, 200.0, false)]
            ),
            PathKind::Relay
        );
    }

    #[test]
    fn srtt_smooths_and_tracks_jitter() {
        let mut r = Srtt::default();
        r.sample(100.0);
        assert_eq!(r.rtt, Some(100.0));
        for _ in 0..50 {
            r.sample(100.0);
        }
        assert!(r.jitter < 1.0);
        r.sample(180.0);
        assert!((r.rtt.unwrap() - 110.0).abs() < 0.01);
        assert!(r.jitter > 15.0);
    }
}
