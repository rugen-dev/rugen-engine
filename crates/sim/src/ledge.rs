//! Ledge grab on `[Arena]` stages (`[Arena] ledge`, on by default when
//! the stage has a `floor` span).
//!
//! A falling fighter whose hand reaches an edge of the deck while facing it
//! (or drifting toward it) catches it and hangs for up to [`HANG_TICKS`],
//! invulnerable for a moment ([`INVULN`]). From the ledge: up climbs on,
//! forward rolls in, X jumps, down or back lets go; the time running out
//! lets go as well. [`REGRABS`] grabs per airtime, then a fighter must touch
//! the ground again; letting go keeps it off the ledge for [`COOLDOWN`].
//!
//! While hanging the engine owns the fighter (its states do not run, like a
//! Tag switch): the position is pinned, the action is read from the input
//! buffer, so humans, the CPU and replays all go through the same path.
//! Everything is plain player state, cloned for rollback and checksummed.

use rugen_cns::state::{MoveType, Physics, StateType};

use crate::world::World;

/// Longest hang (2 s).
pub const HANG_TICKS: i32 = 120;
/// Grabs per airtime.
pub const REGRABS: u8 = 2;
/// Ticks off the ledge after letting go (or climbing).
pub const COOLDOWN: i32 = 30;
/// Invulnerability on the first grab of an airtime, then on a regrab.
pub const INVULN: [i32; 2] = [40, 16];
/// Ticks on the ledge before an action is read.
pub const ACT_AFTER: i32 = 6;
/// How far outside the edge a body still reaches it (stage units).
pub const REACH: f32 = 36.0;
/// Length of a climb and of a roll.
pub const CLIMB_TICKS: i32 = 14;
pub const ROLL_TICKS: i32 = 26;
/// Where a climb and a roll end, inside the edge (stage units).
pub const CLIMB_IN: f32 = 22.0;
pub const ROLL_IN: f32 = 90.0;

/// What a fighter on the ledge is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgeAction {
    Hang,
    Climb,
    Roll,
}

/// A fighter on the ledge (`Player::ledge`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hang {
    /// -1 = the deck's left edge, +1 = its right edge.
    pub side: f32,
    /// The edge's x (stage units).
    pub edge: f32,
    /// Ticks since the grab.
    pub t: i32,
    pub action: LedgeAction,
    /// Ticks into a climb or a roll.
    pub at: i32,
}

impl Hang {
    /// Ticks left before the fighter lets go.
    pub fn left(&self) -> i32 {
        (HANG_TICKS - self.t).max(0)
    }

    pub fn hash(&self) -> u64 {
        (u64::from(self.side.to_bits()) << 32)
            ^ (self.t as u64) << 8
            ^ (self.at as u64) << 20
            ^ self.action as u64
            ^ u64::from(self.edge.to_bits()).rotate_left(17)
    }
}

/// The deck's edges (stage units) when the stage has a ledge.
pub fn edges(w: &World) -> Option<[f32; 2]> {
    let a = &w.stage.arena;
    if a.ledge { a.floor } else { None }
}

/// How far below the deck the feet hang (stage units): most of the
/// fighter's standing height.
pub fn hang_height(w: &World, i: usize) -> f32 {
    let h = w.data_height.get(w.players[i].data).copied().unwrap_or(0.0);
    if h > 0.0 {
        h * 0.8
    } else {
        56.0 * w.stage.width / 320.0
    }
}

/// How far outside the edge the body hangs (stage units).
fn body_offset(w: &World, i: usize) -> f32 {
    (w.players[i].size.air_front * w.local_scale(i)).clamp(6.0, 30.0)
}

/// Is the hand of a body at `(x, y)` (stage units) at an edge? Returns the
/// edge's side.
pub fn window(w: &World, i: usize, x: f32, y: f32) -> Option<f32> {
    let [l, r] = edges(w)?;
    let hand = y - hang_height(w, i);
    if !(-10.0..=14.0).contains(&hand) {
        return None;
    }
    [(-1.0, l), (1.0, r)]
        .into_iter()
        .find(|&(side, edge)| (-6.0..=REACH).contains(&((x - edge) * side)))
        .map(|(side, _)| side)
}

/// May player `i` grab a ledge now (its state, not the geometry)?
pub fn can_grab(w: &World, i: usize) -> bool {
    let p = &w.players[i];
    p.helper_index == 0
        && p.player_type
        && !p.removed
        && !p.broken
        && !p.standby
        && p.tag == crate::team::TagPhase::None
        && p.alive()
        && p.ledge.is_none()
        && p.ledge_cool == 0
        && p.ledge_grabs < REGRABS
        && p.state_type == StateType::A
        && p.vel[1] >= 0.0
        && p.bind.is_none()
        && !p.pause_bool
        && p.state_owner == p.data
        && (p.move_type != MoveType::H || p.ghv.hittime < 0)
        && w.round.round_state() == 2
}

/// Facing the edge on `side`, or drifting toward it (`vx` in stage units).
pub fn faces(facing: f32, vx: f32, side: f32) -> bool {
    facing * side < 0.0 || vx * -side > 0.5
}

impl World {
    /// End of the bounds phase: fighters that reach an edge catch it.
    pub(crate) fn check_ledges(&mut self) {
        let Some([l, r]) = edges(self) else {
            return;
        };
        for i in 0..self.players.len() {
            let p = &mut self.players[i];
            if p.helper_index != 0 || p.removed {
                continue;
            }
            if p.ledge_cool > 0 {
                p.ledge_cool -= 1;
            }
            if p.state_type != StateType::A && p.ledge.is_none() {
                p.ledge_grabs = 0;
            }
            if !can_grab(self, i) {
                continue;
            }
            let s = self.local_scale(i);
            let p = &self.players[i];
            let (x, y) = (p.pos[0] * s, p.pos[1] * s);
            let vx = p.vel[0] * p.facing * s;
            let Some(side) = window(self, i, x, y) else {
                continue;
            };
            if !faces(p.facing, vx, side) {
                continue;
            }
            let taken = self
                .players
                .iter()
                .any(|q| q.ledge.is_some_and(|h| h.side == side));
            if taken {
                continue;
            }
            let edge = if side < 0.0 { l } else { r };
            self.grab_ledge(i, side, edge);
        }
    }

    fn hang_pos(&self, i: usize, h: &Hang) -> [f32; 2] {
        [h.edge + h.side * body_offset(self, i), hang_height(self, i)]
    }

    fn set_stage_pos(&mut self, i: usize, at: [f32; 2]) {
        let s = self.local_scale(i);
        let p = &mut self.players[i];
        p.pos = [at[0] / s, at[1] / s];
        p.old_pos = p.pos;
        p.vel = [0.0, 0.0];
    }

    /// The first animation of `list` the fighter has.
    fn ledge_anim(&mut self, i: usize, list: &[i32]) {
        let d = self.players[i].data;
        if let Some(&a) = list.iter().find(|&&a| self.data[d].air.get(a).is_some()) {
            self.change_anim(i, a, None);
        }
    }

    fn grab_ledge(&mut self, i: usize, side: f32, edge: f32) {
        let h = Hang {
            side,
            edge,
            t: 0,
            action: LedgeAction::Hang,
            at: 0,
        };
        let n = self.data[self.players[i].data]
            .states
            .get(&50)
            .map_or(0, |s| s.ctrls.len());
        let p = &mut self.players[i];
        p.ledge = Some(h);
        p.ledge_grabs += 1;
        p.unhittable_time = p
            .unhittable_time
            .max(INVULN[usize::from(p.ledge_grabs > 1)]);
        p.prev_state_no = p.state_no;
        p.state_no = 50;
        p.state_time = 0;
        p.pending_state = None;
        p.persist = vec![0; n];
        p.state_type = StateType::A;
        p.move_type = MoveType::I;
        p.physics = Physics::N;
        p.ctrl = false;
        p.hitdef = None;
        p.ghv.fallflag = false;
        p.air_jump_count = 0;
        p.air_dash_used = false;
        p.air_dash_time = 0;
        p.set_facing(-side);
        let at = self.hang_pos(i, &h);
        self.set_stage_pos(i, at);
        self.ledge_anim(i, &[41, 44, 0]);
    }

    /// Phase 3 for a fighter on the ledge. `false`: it left the ledge (or
    /// never was on it) and its states run as usual this tick.
    pub(crate) fn ledge_step(&mut self, i: usize) -> bool {
        let Some(mut h) = self.players[i].ledge else {
            return false;
        };
        let p = &self.players[i];
        if p.pause_bool {
            return true;
        }
        if p.move_type == MoveType::H || p.gethit || !p.alive() {
            self.players[i].ledge = None;
            return false;
        }
        h.t += 1;
        let hang = self.hang_pos(i, &h);
        match h.action {
            LedgeAction::Hang => {
                if self.round.round_state() != 2 {
                    let x = h.edge - h.side * CLIMB_IN;
                    return self.leave_standing(i, x);
                }
                let inp = &self.players[i].input;
                let fresh = |b: i32| b > 0 && b <= h.t;
                let ready = h.t >= ACT_AFTER;
                if ready && fresh(inp.xb) {
                    return self.leave_jumping(i, &h);
                }
                if ready && fresh(inp.ub) {
                    h.action = LedgeAction::Climb;
                    self.players[i].unhittable_time = CLIMB_TICKS + 4;
                } else if ready && fresh(inp.fb) {
                    h.action = LedgeAction::Roll;
                    self.players[i].unhittable_time = ROLL_TICKS + 4;
                    self.ledge_anim(i, &[20, 0]);
                } else if (ready && (fresh(inp.db) || fresh(inp.bb))) || h.t >= HANG_TICKS {
                    return self.leave_falling(i, &h);
                }
                self.set_stage_pos(i, hang);
            }
            LedgeAction::Climb | LedgeAction::Roll => {
                let (len, inside) = if h.action == LedgeAction::Climb {
                    (CLIMB_TICKS, CLIMB_IN)
                } else {
                    (ROLL_TICKS, ROLL_IN)
                };
                h.at += 1;
                let end_x = h.edge - h.side * inside;
                if h.at >= len {
                    return self.leave_standing(i, end_x);
                }
                // up first, then in
                let f = h.at as f32 / len as f32;
                let up = (f * 2.0).min(1.0);
                let across = (f * 2.0 - 1.0).max(0.0);
                let at = [hang[0] + (end_x - hang[0]) * across, hang[1] - hang[1] * up];
                self.set_stage_pos(i, at);
            }
        }
        let p = &mut self.players[i];
        p.ledge = Some(h);
        p.state_time += 1;
        true
    }

    /// Off the ledge onto the deck at `x`, standing with control.
    fn leave_standing(&mut self, i: usize, x: f32) -> bool {
        self.set_stage_pos(i, [x, 0.0]);
        let p = &mut self.players[i];
        p.ledge = None;
        p.ledge_cool = COOLDOWN;
        p.ground = 0.0;
        self.change_state(i, 0, Some(0), Some(true));
        false
    }

    /// A jump from the ledge, up and toward the deck.
    fn leave_jumping(&mut self, i: usize, h: &Hang) -> bool {
        let at = [h.edge + h.side * body_offset(self, i), 0.0];
        self.set_stage_pos(i, at);
        let d = &self.data[self.players[i].data];
        let vx = d.konst("velocity.jump.fwd.x").unwrap_or(2.5).abs();
        let vy = d.konst("velocity.jump.y").unwrap_or(-8.4);
        let p = &mut self.players[i];
        p.ledge = None;
        p.ledge_cool = COOLDOWN;
        p.vel = [vx, vy];
        self.change_state(i, 50, None, Some(true));
        false
    }

    /// Let go: fall from the ledge.
    fn leave_falling(&mut self, i: usize, h: &Hang) -> bool {
        let at = self.hang_pos(i, h);
        self.set_stage_pos(i, [at[0] + h.side * 4.0, at[1]]);
        let p = &mut self.players[i];
        p.ledge = None;
        p.ledge_cool = COOLDOWN + 10;
        p.vel = [0.0, 1.0];
        self.change_state(i, 50, None, Some(true));
        false
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

    fn kfm() -> Option<Arc<CharData>> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1/chars/kfm/kfm.def");
        p.exists()
            .then(|| CharData::load(&p).unwrap_or_else(|e| panic!("load kfm: {e}")))
    }

    /// Two human KFMs on an island (deck -300..300) over a pit, round on.
    fn island() -> Option<World> {
        let d = kfm()?;
        let stage = Stage {
            arena: Arena {
                blastzone: Some([-700.0, -500.0, 700.0, 300.0]),
                walls: false,
                stocks: 3,
                floor: Some([-300.0, 300.0]),
                ..Default::default()
            },
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

    /// P1 falling just outside the right edge, hand a little above the
    /// deck, facing `facing`.
    fn falling_at_right_edge(w: &mut World, facing: f32) {
        let y = hang_height(w, 0) - 16.0;
        w.change_state(0, 50, None, Some(true));
        let p = &mut w.players[0];
        p.pos = [312.0, y];
        p.old_pos = p.pos;
        p.vel = [0.0, 0.0];
        p.set_facing(facing);
        w.players[1].pos = [-200.0, 0.0];
    }

    fn step(w: &mut World, n: usize, b: Buttons) {
        for _ in 0..n {
            w.players[1].pos = [-200.0, 0.0];
            w.step(&[b, Buttons::default()]);
        }
    }

    fn grabbed(w: &mut World) -> bool {
        for _ in 0..30 {
            step(w, 1, Buttons::default());
            if w.players[0].ledge.is_some() {
                return true;
            }
        }
        false
    }

    #[test]
    fn a_falling_fighter_catches_the_edge_and_hangs() {
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, -1.0);
        assert!(grabbed(&mut w), "no grab at {:?}", w.players[0].pos);
        let p = &w.players[0];
        let h = p.ledge.unwrap();
        assert_eq!((h.side, h.edge), (1.0, 300.0));
        assert!(p.unhittable_time > 30, "brief invulnerability");
        assert!(!p.ctrl);
        let pinned = p.pos;
        step(&mut w, 60, Buttons::default());
        assert_eq!(w.players[0].pos, pinned, "hangs still");
        assert!(w.players[0].unhittable_time == 0, "invulnerability ends");
        // two seconds, then it lets go and falls
        step(&mut w, (HANG_TICKS - 60) as usize, Buttons::default());
        let p = &w.players[0];
        assert!(
            p.ledge.is_none(),
            "let go after {HANG_TICKS} ticks: {:?}",
            p.ledge
        );
        step(&mut w, 15, Buttons::default());
        assert!(w.players[0].pos[1] > pinned[1] + 20.0, "falls");
        assert!(w.players[0].ledge.is_none(), "no regrab while falling away");
    }

    #[test]
    fn up_climbs_forward_rolls_and_x_jumps_onto_the_deck() {
        let up = Buttons {
            u: true,
            ..Default::default()
        };
        // facing left at the right edge: forward is left
        let fwd = Buttons {
            l: true,
            ..Default::default()
        };
        let x = Buttons {
            x: true,
            ..Default::default()
        };
        for (b, inside) in [(up, CLIMB_IN), (fwd, ROLL_IN), (x, 0.0)] {
            let Some(mut w) = island() else { return };
            falling_at_right_edge(&mut w, -1.0);
            assert!(grabbed(&mut w));
            step(&mut w, ACT_AFTER as usize, Buttons::default());
            step(&mut w, 2, b);
            step(&mut w, 70, Buttons::default());
            let p = &w.players[0];
            assert!(p.ledge.is_none(), "{b:?}: still on the ledge");
            assert!(
                p.state_type != StateType::A && p.ground == 0.0 && p.pos[0] < 300.0,
                "{b:?}: not on the deck: {:?} state {}",
                p.pos,
                p.state_no
            );
            if inside > 0.0 {
                assert!(
                    (p.pos[0] - (300.0 - inside)).abs() < 1.0,
                    "{b:?}: {:?}",
                    p.pos
                );
            }
            assert!(p.ctrl, "{b:?}: control back");
            assert_eq!(p.stocks, 3);
        }
    }

    #[test]
    fn down_lets_go() {
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, -1.0);
        assert!(grabbed(&mut w));
        step(&mut w, ACT_AFTER as usize, Buttons::default());
        let down = Buttons {
            d: true,
            ..Default::default()
        };
        step(&mut w, 1, down);
        let p = &w.players[0];
        assert!(p.ledge.is_none() && p.ctrl && p.state_type == StateType::A);
        assert!(p.ledge_cool > 0);
    }

    #[test]
    fn no_grab_facing_away_spent_or_turned_off() {
        // facing away and not drifting toward the deck
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, 1.0);
        assert!(!grabbed(&mut w));
        // facing away but drifting back toward it: catches it
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, 1.0);
        w.players[0].vel[0] = -1.5;
        assert!(grabbed(&mut w));
        assert_eq!(w.players[0].facing, -1.0, "turned to the deck");
        // grabs spent this airtime
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, -1.0);
        w.players[0].ledge_grabs = REGRABS;
        assert!(!grabbed(&mut w));
        // `[Arena] ledge = 0`
        let Some(mut w) = island() else { return };
        w.stage.arena.ledge = false;
        falling_at_right_edge(&mut w, -1.0);
        assert!(!grabbed(&mut w));
    }

    #[test]
    fn a_hit_knocks_the_hanging_fighter_off() {
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, -1.0);
        assert!(grabbed(&mut w));
        step(&mut w, 50, Buttons::default());
        assert_eq!(w.players[0].unhittable_time, 0);
        // P2 crouching light kick from the deck at the edge
        let p0 = w.players[0].pos;
        let mut hit = false;
        for k in 0..40 {
            let p2 = &mut w.players[1];
            if k == 0 {
                p2.pos = [p0[0] - 40.0, 0.0];
                p2.set_facing(1.0);
            }
            let b = Buttons {
                b: k % 8 == 0,
                d: true,
                ..Default::default()
            };
            w.step(&[Buttons::default(), b]);
            if w.players[0].ledge.is_none() {
                hit = true;
                break;
            }
        }
        assert!(hit, "still hanging");
        assert_eq!(w.players[0].move_type, MoveType::H);
    }

    #[test]
    fn deterministic_from_a_copy_on_the_ledge() {
        let Some(mut w) = island() else { return };
        falling_at_right_edge(&mut w, -1.0);
        assert!(grabbed(&mut w));
        step(&mut w, 10, Buttons::default());
        let mut copy = w.clone();
        assert_eq!(copy.checksum(), w.checksum());
        let up = Buttons {
            u: true,
            ..Default::default()
        };
        for k in 0..200 {
            let b = if k == 20 { up } else { Buttons::default() };
            w.step(&[b, Buttons::default()]);
            copy.step(&[b, Buttons::default()]);
            assert_eq!(w.checksum(), copy.checksum(), "tick {k}");
        }
        assert_eq!(w.players[0], copy.players[0]);
        // the hang itself is part of the hash
        let mut other = copy.clone();
        other.players[0].ledge = Some(Hang {
            side: 1.0,
            edge: 300.0,
            t: 3,
            action: LedgeAction::Hang,
            at: 0,
        });
        assert_ne!(other.checksum(), copy.checksum());
    }
}
