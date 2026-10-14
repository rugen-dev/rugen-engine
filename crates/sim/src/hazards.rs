//! Stage hazards on `[Arena]` stages, all driven by the fight clock
//! ([`World::hazard_t`], fight ticks this round), so netplay, replays and
//! rollback reproduce them with nothing but that counter:
//!
//! - **Moving platforms**: `[Platform] path = dx, dy, ...` offsets from the
//!   platform's place, visited in order and back (a closed loop), at
//!   `speed` stage units per tick. Fighters and items on it ride along.
//! - **Dropping platforms**: `[Platform] drop = every, gone` (seconds): every
//!   `every` seconds the platform falls away for `gone` seconds, after
//!   [`WARN_TICKS`] of warning. Whoever stands on it falls.
//! - **Wind**: `[Arena] wind = every, length, strength`: a gust every
//!   `every` seconds for `length` seconds, pushing the fighters `strength`
//!   stage units per tick, the direction alternating (first gust to the
//!   right), with [`WARN_TICKS`] of warning.

use rugen_cns::state::StateType;

use crate::world::{Platform, World};

/// Warning before a platform drops or a gust starts (3 s).
pub const WARN_TICKS: i32 = 180;
/// A gust eases in and out over this many ticks.
const GUST_EASE: i32 = 30;

/// A platform that moves and / or drops (`Stage::motions`).
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    /// Index into `Stage::platforms`.
    pub platform: usize,
    /// The platform as declared.
    pub rest: Platform,
    /// Offsets from `rest` visited in order, then back to it.
    pub path: Vec<[f32; 2]>,
    /// Stage units per tick along the path.
    pub speed: f32,
    /// `[every, gone]` in ticks.
    pub drop: Option<[i32; 2]>,
}

/// Where a dropping platform is in its cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPhase {
    Solid,
    /// Drops in this many ticks.
    Warn(i32),
    /// Dropped this many ticks ago.
    Gone(i32),
}

impl Motion {
    /// Offset along the path at fight tick `t` (stage units).
    pub fn offset_at(&self, t: i32) -> [f32; 2] {
        if self.path.is_empty() || self.speed <= 0.0 {
            return [0.0, 0.0];
        }
        let mut pts = Vec::with_capacity(self.path.len() + 2);
        pts.push([0.0, 0.0]);
        pts.extend_from_slice(&self.path);
        pts.push([0.0, 0.0]);
        let len = |a: [f32; 2], b: [f32; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            libm::sqrtf(dx * dx + dy * dy)
        };
        let total: f32 = pts.windows(2).map(|w| len(w[0], w[1])).sum();
        if total <= 0.0 {
            return [0.0, 0.0];
        }
        // whole ticks per loop keep the phase an integer (no float modulo)
        let period = (libm::ceilf(total / self.speed) as i32).max(1);
        let mut s = (t.rem_euclid(period)) as f32 * self.speed;
        for w in pts.windows(2) {
            let l = len(w[0], w[1]);
            if s <= l && l > 0.0 {
                let f = s / l;
                return [
                    w[0][0] + (w[1][0] - w[0][0]) * f,
                    w[0][1] + (w[1][1] - w[0][1]) * f,
                ];
            }
            s -= l;
        }
        [0.0, 0.0]
    }

    /// The drop cycle at fight tick `t`: solid, then warned, then gone for
    /// the last `gone` ticks of every `every`.
    pub fn drop_phase(&self, t: i32) -> DropPhase {
        let Some([every, gone]) = self.drop else {
            return DropPhase::Solid;
        };
        let at = t.rem_euclid(every.max(1));
        let falls = every - gone;
        if at >= falls {
            DropPhase::Gone(at - falls)
        } else if at >= falls - WARN_TICKS {
            DropPhase::Warn(falls - at)
        } else {
            DropPhase::Solid
        }
    }
}

/// The gust at fight tick `t` of `wind = [every, length, strength]` (ticks,
/// ticks, units per tick): the push now (signed stage units per tick) and,
/// before a gust, the ticks until it starts with its direction.
pub fn gust_at(wind: [f32; 3], t: i32) -> (f32, Option<(i32, f32)>) {
    let every = (wind[0] as i32).max(1);
    let length = (wind[1] as i32).clamp(1, every);
    let k = t / every; // gust k starts at (k + 1) * every
    let dir = |k: i32| if k % 2 == 0 { 1.0 } else { -1.0 };
    let into = t - k * every; // ticks since the start of gust k - 1
    let mut push = 0.0;
    if k >= 1 && into < length {
        let ease = (into.min(length - into) as f32 / GUST_EASE as f32).min(1.0);
        push = dir(k - 1) * wind[2] * ease;
    }
    let next = every - into;
    let warn = (push == 0.0 && next <= WARN_TICKS).then_some((next, dir(k)));
    (push, warn)
}

/// What the renderer needs about a hazard platform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlatformView {
    pub platform: usize,
    /// Where its art goes, from where the stage draws it (stage units).
    pub offset: [f32; 2],
    pub phase: DropPhase,
}

/// The hazard platforms now.
pub fn platform_views(w: &World) -> Vec<PlatformView> {
    let t = w.hazard_t;
    w.stage
        .motions
        .iter()
        .map(|m| {
            let mut offset = m.offset_at(t);
            let phase = m.drop_phase(t);
            if let DropPhase::Gone(k) = phase {
                // falls away out of sight
                let k = k.min(120) as f32;
                offset[1] += 0.25 * k * k;
            }
            PlatformView {
                platform: m.platform,
                offset,
                phase,
            }
        })
        .collect()
}

/// The stage has hazards?
pub fn any(w: &World) -> bool {
    !w.stage.motions.is_empty() || w.stage.arena.wind.is_some()
}

/// The wind now: push (signed stage units per tick) and an upcoming gust.
pub fn wind(w: &World) -> (f32, Option<(i32, f32)>) {
    match w.stage.arena.wind {
        Some(wd) if w.round.round_state() == 2 => gust_at(wd, w.hazard_t),
        _ => (0.0, None),
    }
}

impl World {
    /// Before the states run: move the platforms (carrying whoever stands
    /// on them), drop and restore them, and blow the wind.
    pub(crate) fn step_hazards(&mut self) {
        if !any(self) {
            return;
        }
        if self.round.round_state() == 2 && self.pausetime <= 0 && self.supertime <= 0 {
            self.hazard_t += 1;
        }
        let t = self.hazard_t;
        for mi in 0..self.stage.motions.len() {
            let m = &self.stage.motions[mi];
            let k = m.platform;
            let off = m.offset_at(t);
            let gone = matches!(m.drop_phase(t), DropPhase::Gone(_));
            let rest = m.rest;
            let Some(&old) = self.stage.platforms.get(k) else {
                continue;
            };
            let new = Platform {
                x1: rest.x1 + off[0],
                x2: rest.x2 + off[0],
                y: rest.y + off[1],
                gone,
            };
            let d = [new.x1 - old.x1, new.y - old.y];
            if d != [0.0, 0.0] && !old.gone {
                self.carry_riders(&old, d);
                for it in &mut self.items.list {
                    if it.state == (crate::items::ItemState::Ground { surface: k as i32 }) {
                        it.pos[0] += d[0];
                    }
                }
            }
            self.stage.platforms[k] = new;
        }
        let (push, _) = wind(self);
        if push != 0.0 {
            for i in 0..self.players.len() {
                let p = &self.players[i];
                if p.helper_index != 0
                    || p.removed
                    || p.standby
                    || !p.alive()
                    || p.ledge.is_some()
                    || p.bind.is_some()
                {
                    continue;
                }
                // a fighter on the ground leans into it: half the push
                let k = if p.state_type == StateType::A {
                    1.0
                } else {
                    0.5
                };
                let s = self.local_scale(i);
                self.players[i].pos[0] += push * k / s;
            }
        }
    }

    /// Fighters standing on `pl` move by `d` (stage units) with it.
    fn carry_riders(&mut self, pl: &Platform, d: [f32; 2]) {
        for i in 0..self.players.len() {
            let s = self.local_scale(i);
            let p = &mut self.players[i];
            if p.removed || !p.player_type || p.state_type == StateType::A || p.ledge.is_some() {
                continue;
            }
            let x = p.pos[0] * s;
            if p.ground != pl.y / s || x < pl.x1 || x > pl.x2 {
                continue;
            }
            p.pos[0] += d[0] / s;
            p.pos[1] += d[1] / s;
            p.old_pos[0] += d[0] / s;
            p.old_pos[1] += d[1] / s;
            p.ground = (pl.y + d[1]) / s;
        }
    }

    /// A new round: platforms back in place, the clock at zero.
    pub(crate) fn reset_hazards(&mut self) {
        self.hazard_t = 0;
        for m in &self.stage.motions {
            if let Some(p) = self.stage.platforms.get_mut(m.platform) {
                *p = m.rest;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Buttons;
    use crate::world::Arena;
    use crate::{CharData, Stage};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn mover() -> Motion {
        Motion {
            platform: 0,
            rest: Platform {
                x1: -50.0,
                x2: 50.0,
                y: -80.0,
                gone: false,
            },
            path: vec![[100.0, 0.0]],
            speed: 2.0,
            drop: None,
        }
    }

    #[test]
    fn a_path_loops_out_and_back() {
        let m = mover();
        assert_eq!(m.offset_at(0), [0.0, 0.0]);
        assert_eq!(m.offset_at(25), [50.0, 0.0]);
        assert_eq!(m.offset_at(50), [100.0, 0.0]);
        assert_eq!(m.offset_at(75), [50.0, 0.0]);
        assert_eq!(m.offset_at(100), [0.0, 0.0]);
        assert_eq!(m.offset_at(125), [50.0, 0.0]);
    }

    #[test]
    fn a_drop_warns_then_goes_then_comes_back() {
        let m = Motion {
            drop: Some([600, 240]),
            ..mover()
        };
        assert_eq!(m.drop_phase(0), DropPhase::Solid);
        assert_eq!(m.drop_phase(359 - WARN_TICKS), DropPhase::Solid);
        assert_eq!(m.drop_phase(360 - WARN_TICKS), DropPhase::Warn(WARN_TICKS));
        assert_eq!(m.drop_phase(359), DropPhase::Warn(1));
        assert_eq!(m.drop_phase(360), DropPhase::Gone(0));
        assert_eq!(m.drop_phase(599), DropPhase::Gone(239));
        assert_eq!(m.drop_phase(600), DropPhase::Solid);
    }

    #[test]
    fn gusts_alternate_with_a_warning() {
        let wind = [600.0, 120.0, 1.0];
        assert_eq!(gust_at(wind, 0).0, 0.0);
        assert_eq!(
            gust_at(wind, 600 - WARN_TICKS),
            (0.0, Some((WARN_TICKS, 1.0)))
        );
        assert_eq!(gust_at(wind, 660).0, 1.0, "full strength mid gust");
        assert!(
            gust_at(wind, 605).0 > 0.0 && gust_at(wind, 605).0 < 1.0,
            "eases in"
        );
        assert_eq!(gust_at(wind, 720).0, 0.0, "over");
        assert_eq!(
            gust_at(wind, 1260).0,
            -1.0,
            "the next one blows the other way"
        );
        assert_eq!(gust_at(wind, 1200 - 10).1, Some((10, -1.0)));
    }

    fn kfm() -> Option<Arc<CharData>> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1/chars/kfm/kfm.def");
        p.exists()
            .then(|| CharData::load(&p).unwrap_or_else(|e| panic!("load kfm: {e}")))
    }

    /// Two human KFMs on an island with one hazard platform over the deck.
    fn island(motion: Motion, wind: Option<[f32; 3]>) -> Option<World> {
        let d = kfm()?;
        let stage = Stage {
            arena: Arena {
                blastzone: Some([-900.0, -500.0, 900.0, 300.0]),
                walls: false,
                stocks: 3,
                floor: Some([-400.0, 400.0]),
                wind,
                ..Default::default()
            },
            platforms: vec![motion.rest],
            motions: vec![motion],
            left_bound: -2000.0,
            right_bound: 2000.0,
            ..Default::default()
        };
        let mut w = World::new(stage, 5);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        w.round.time_left = -1;
        w.start_round();
        w.round.time_left = -1;
        while w.round.round_state() != 2 {
            w.step(&[]);
        }
        Some(w)
    }

    fn stand_on_platform(w: &mut World) {
        let y = w.stage.platforms[0].y;
        let x = (w.stage.platforms[0].x1 + w.stage.platforms[0].x2) / 2.0;
        w.change_state(0, 0, None, Some(true));
        let p = &mut w.players[0];
        p.pos = [x, y];
        p.old_pos = p.pos;
        p.ground = y;
        p.vel = [0.0, 0.0];
        w.players[1].pos = [-300.0, 0.0];
    }

    #[test]
    fn a_fighter_rides_a_moving_platform() {
        let Some(mut w) = island(mover(), None) else {
            return;
        };
        stand_on_platform(&mut w);
        let x0 = w.players[0].pos[0] - (w.stage.platforms[0].x1 + 50.0);
        for _ in 0..40 {
            w.step(&[]);
            let p = &w.players[0];
            assert_eq!(p.ground, w.stage.platforms[0].y, "still on it");
            let rel = p.pos[0] - (w.stage.platforms[0].x1 + 50.0);
            assert!((rel - x0).abs() < 0.01, "moves with it: {rel} vs {x0}");
        }
        assert!(w.stage.platforms[0].x1 > -50.0, "it moved");
    }

    #[test]
    fn a_dropping_platform_drops_its_rider_and_comes_back() {
        let m = Motion {
            path: Vec::new(),
            drop: Some([600, 240]),
            ..mover()
        };
        let Some(mut w) = island(m, None) else {
            return;
        };
        stand_on_platform(&mut w);
        for _ in 0..(360 - w.hazard_t - 2) {
            w.step(&[]);
        }
        assert_eq!(w.players[0].ground, -80.0, "on it through the warning");
        for _ in 0..60 {
            w.step(&[]);
        }
        assert!(w.stage.platforms[0].gone);
        let p = &w.players[0];
        assert_eq!(p.ground, 0.0, "fell to the deck: at {:?}", p.pos);
        for _ in 0..240 {
            w.step(&[]);
        }
        assert!(!w.stage.platforms[0].gone, "back");
        assert_eq!(w.stage.platforms[0], w.stage.motions[0].rest);
    }

    /// A CPU on a platform that is about to drop gets off in the warning.
    #[test]
    fn the_cpu_leaves_a_platform_before_it_drops() {
        let m = Motion {
            path: Vec::new(),
            drop: Some([900, 240]),
            ..mover()
        };
        let Some(mut w) = island(m, None) else {
            return;
        };
        w.arena_ai = true;
        // the deck under the platform, the opponent far away on it
        let warn_at = 900 - 240 - WARN_TICKS;
        while w.hazard_t < warn_at - 5 {
            w.step(&[]);
        }
        stand_on_platform(&mut w);
        let mut left = false;
        while w.hazard_t < 900 - 240 {
            w.players[1].pos = [-300.0, 0.0];
            // the plugin's buttons alone (an idle character AI)
            w.players[0].ai_level = 4;
            let b = crate::arena_ai::filter(&mut w, 0, Buttons::default());
            w.players[0].ai_level = 0;
            w.step(&[b, Buttons::default()]);
            let p = &w.players[0];
            if p.ground != -80.0 || p.state_type == StateType::A {
                left = true;
            }
        }
        assert!(left, "still on it at {:?}", w.players[0].pos);
        assert!(w.players[0].arena.hazards > 0);
    }

    #[test]
    fn wind_pushes_and_hazards_replay_from_a_copy() {
        let m = Motion {
            drop: Some([500, 120]),
            ..mover()
        };
        let Some(mut w) = island(m, Some([300.0, 120.0, 1.0])) else {
            return;
        };
        w.players[1].pos = [-300.0, 0.0];
        let x0 = w.players[0].pos[0];
        for _ in 0..(300 + 90 - w.hazard_t) {
            w.step(&[]);
        }
        assert!(
            w.players[0].pos[0] > x0 + 20.0,
            "blown right: {x0} -> {}",
            w.players[0].pos[0]
        );
        // determinism from a mid-match copy, CPUs fighting through it all
        w.players[0].ai_level = 5;
        w.players[1].ai_level = 5;
        w.arena_ai = true;
        let mut copy = w.clone();
        for _ in 0..1500 {
            w.step(&[]);
            copy.step(&[Buttons::default(); 2]);
        }
        assert_eq!(w.checksum(), copy.checksum());
        assert_eq!(w.stage, copy.stage);
        let mut other = copy.clone();
        other.hazard_t += 1;
        assert_ne!(other.checksum(), copy.checksum());
    }
}
