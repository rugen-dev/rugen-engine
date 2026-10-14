//! Team modes, with Ikemen GO's rules (its `system.go` / `char.go` are the
//! spec): Simul (partners on screen together), Turns (one fighter at a time,
//! the next comes in when one falls) and Tag (see the tag states below).
//!
//! Every root player of a team match is added up front, in Ikemen's player
//! order: P1 and P2 (first pick of each side), then P3 / P4 (second picks)
//! and so on. A Turns member waiting for its turn is `removed` (absent from
//! everything), a Tag partner outside the screen is `standby`.
//!
//! A classic match (both sides Single, one fighter each) never reaches the
//! team paths: replays and `rugen-server`'s re-simulation stay bit-identical
//! (`golden_tests`).

use std::sync::Arc;

use crate::CharData;
use crate::world::World;

/// How a side fights (MUGEN / Ikemen `TeamMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TeamMode {
    #[default]
    Single,
    Simul,
    Turns,
    Tag,
}

impl TeamMode {
    pub const ALL: [TeamMode; 4] = [
        TeamMode::Single,
        TeamMode::Turns,
        TeamMode::Simul,
        TeamMode::Tag,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TeamMode::Single => "single",
            TeamMode::Simul => "simul",
            TeamMode::Turns => "turns",
            TeamMode::Tag => "tag",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// Ikemen's numbering (`TM_Single` = 0, `TM_Simul`, `TM_Turns`, `TM_Tag`).
    pub fn raw(self) -> i32 {
        match self {
            TeamMode::Single => 0,
            TeamMode::Simul => 1,
            TeamMode::Turns => 2,
            TeamMode::Tag => 3,
        }
    }

    /// Fighters a side of this mode brings (Ikemen's defaults: 2..=4).
    pub fn sizes(self) -> std::ops::RangeInclusive<usize> {
        match self {
            TeamMode::Single => 1..=1,
            _ => 2..=MAX_MEMBERS,
        }
    }

    /// Several fighters on the side at once (`NumPartner` counts them).
    pub fn has_partners(self) -> bool {
        matches!(self, TeamMode::Simul | TeamMode::Tag)
    }
}

/// Largest team.
pub const MAX_MEMBERS: usize = 4;

/// The engine's Tag movements (Ikemen GO `data/tag.zss` states). While in
/// one the player's own states do not run: the engine moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TagPhase {
    #[default]
    None,
    /// Tagged out: runs off its side of the screen (`StateTagLeavingScreen`).
    Leaving,
    /// Waits outside the screen, invisible (`StateTagWaitingOutside`).
    Waiting,
    /// Tagged in: runs on from behind the partner to where it stood
    /// (`StateTagEnteringScreen`).
    Entering,
    /// Jumps in after the partner fell, or for the win pose
    /// (`StateTagJumpingIn`).
    JumpingIn,
    /// Lands from the jump (`StateTagLanding`).
    Landing,
}

/// Ikemen `common.const` tag numbers, in 240p units / ticks.
mod tag_const {
    /// Max run-in speed.
    pub const IN_VEL_X: f32 = 12.5;
    /// Ticks the run-in takes at most.
    pub const IN_TIME: f32 = 20.0;
    /// How far behind the back edge a partner starts.
    pub const START_BEHIND: f32 = 90.0;
    /// Off-screen distance where a leaving partner starts to wait.
    pub const GONE: f32 = 160.0;
    /// Where a waiting partner stands (x, facing the centre).
    pub const WAIT_X: f32 = 480.0;
    /// Red life regeneration: every 30 ticks, 0.5 % of max life.
    pub const REGEN_TICKS: u32 = 30;
    pub const REGEN_SHARE: f32 = 0.005;
    /// Red life lost per point of damage (`Default.LifeToRedLifeMul`).
    pub const RED_MUL: f32 = 0.75;
    /// A computer leader's chance per eligible tick to tag out (1 in N).
    pub const AI_CHANCE: u32 = 240;
}

/// The team setup of a match. `Default` is a classic 1v1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Teams {
    pub mode: [TeamMode; 2],
    /// Root players of each side, in pick order (empty in a classic match).
    pub members: [Vec<usize>; 2],
    /// Simul: a KO of a human-controlled member loses the round
    /// (Ikemen `Simul.LoseOnKO`, on by default).
    pub simul_lose_on_ko: bool,
    /// Tag: a KO of any member loses the round (`Tag.LoseOnKO`, off).
    pub tag_lose_on_ko: bool,
    /// Turns: a member fell last round, so the fighters still standing keep
    /// their life into the next one.
    pub(crate) keep_life: bool,
    /// Tag: each side's fighter in the fight (player index).
    pub leader: [usize; 2],
    /// Tag: the last leader, until it is outside or down (no switch before).
    pub(crate) last_out: [Option<usize>; 2],
}

impl Teams {
    /// Anything but a classic 1v1.
    pub fn active(&self) -> bool {
        self.members.iter().any(|m| !m.is_empty())
    }
}

impl World {
    /// Make this a team match: `mode` per side. Call before adding the
    /// partners with [`World::add_member`]; P1 / P2 already added are the
    /// first member of their side.
    pub fn set_teams(&mut self, mode: [TeamMode; 2]) {
        self.teams.mode = mode;
        self.teams.simul_lose_on_ko = true;
        self.teams.tag_lose_on_ko = false;
        for side in 0..2 {
            self.teams.members[side].clear();
            self.teams.last_out[side] = None;
        }
        let roots: Vec<usize> = (0..self.players.len())
            .filter(|&i| self.players[i].helper_index == 0)
            .collect();
        for i in roots {
            let side = self.players[i].team as usize;
            self.enlist(i, side);
        }
    }

    /// Add a team partner on `side` (after P1 / P2, before the first round).
    pub fn add_member(&mut self, data: Arc<CharData>, side: u8, pal_no: i32) -> usize {
        let i = self.add_player(data, side, pal_no);
        self.enlist(i, side as usize);
        i
    }

    fn enlist(&mut self, i: usize, side: usize) {
        let member = self.teams.members[side].len();
        self.teams.members[side].push(i);
        if member == 0 {
            self.teams.leader[side] = i;
        }
        let mode = self.teams.mode[side];
        let p = &mut self.players[i];
        p.member = member as u8;
        match mode {
            // one fighter at a time, all driven by the side's controls
            TeamMode::Turns => {
                p.input_slot = side;
                p.removed = member > 0;
            }
            TeamMode::Tag => p.input_slot = side,
            TeamMode::Single | TeamMode::Simul => {}
        }
    }

    /// The side's mode (Single in a classic match).
    pub fn team_mode(&self, side: usize) -> TeamMode {
        self.teams.mode.get(side).copied().unwrap_or_default()
    }

    /// Rounds `side` must win: the other side's team size against Turns,
    /// else `match.wins` (Ikemen `setMatchWins`).
    pub fn wins_needed(&self, side: usize) -> u8 {
        let other = 1 - side.min(1);
        if self.teams.active() && self.teams.mode[other] == TeamMode::Turns {
            self.teams.members[other].len() as u8
        } else {
            self.round.config.match_wins
        }
    }

    /// Start position and facing of root `i` (stage units).
    pub(crate) fn start_pos(&self, i: usize) -> (f32, f32) {
        let p = &self.players[i];
        let side = p.team as usize;
        let facing = if side == 0 { 1.0 } else { -1.0 };
        let first = |side: usize| {
            if side == 0 {
                self.stage.p1_start_x
            } else {
                self.stage.p2_start_x
            }
        };
        // partners on screen take P3, P4...: Turns members all start as the first
        if !self.teams.active() || !self.teams.mode[side].has_partners() || p.member == 0 {
            return (first(side), facing);
        }
        let mut x = first(side);
        for m in 1..=p.member as usize {
            let slot = m * 2 + side; // P3 = 2, P4 = 3, ...
            x = self
                .stage
                .partner_start_x
                .get(slot - 2)
                .copied()
                .flatten()
                .unwrap_or(x - facing * self.stage.partner_spacing);
        }
        (x, facing)
    }

    /// The fighters `i` teams up with (Simul / Tag), in pick order.
    pub fn partners(&self, i: usize) -> Vec<usize> {
        let root = self.players[i].root;
        let side = self.players[root].team as usize;
        if !self.teams.active() || !self.teams.mode[side].has_partners() {
            return Vec::new();
        }
        self.teams.members[side]
            .iter()
            .copied()
            .filter(|&m| m != root && !self.players[m].removed)
            .collect()
    }

    /// The fighter a side's main life bar follows: in a team match the
    /// first member in the fight (Turns: the one fighting), else the side's
    /// first root player.
    pub fn side_leader(&self, side: usize) -> Option<usize> {
        if self.teams.active() && self.teams.mode[side] == TeamMode::Tag {
            return self.teams.members[side]
                .contains(&self.teams.leader[side])
                .then_some(self.teams.leader[side]);
        }
        if self.teams.active() {
            return self.teams.members[side]
                .iter()
                .copied()
                .find(|&m| !self.players[m].removed);
        }
        (0..self.players.len())
            .find(|&i| self.players[i].helper_index == 0 && self.players[i].team as usize == side)
    }

    /// The match winner (0 / 1) once it is over: the side that reached its
    /// [`wins_needed`](Self::wins_needed); `None` for a draw or unfinished.
    pub fn match_winner(&self) -> Option<usize> {
        if !self.round.match_over {
            return None;
        }
        (0..2).find(|&s| self.round.wins[s] >= self.wins_needed(s))
    }

    /// Enemy root players still in the match, in player order.
    pub fn enemy_roots(&self, i: usize) -> Vec<usize> {
        let team = self.players[i].team;
        (0..self.players.len())
            .filter(|&k| {
                let o = &self.players[k];
                o.helper_index == 0 && !o.removed && o.team != team
            })
            .collect()
    }

    /// 240p units in stage units (Ikemen `const240p`).
    fn s240(&self) -> f32 {
        self.stage.width / 320.0
    }

    /// Screen edge behind / in front of a fighter facing `f` (stage units).
    fn edges(&self, f: f32) -> (f32, f32) {
        let (l, r) = (self.screen_left(), self.screen_right());
        if f > 0.0 { (l, r) } else { (r, l) }
    }

    /// `BackEdgeDist` of root `i` in stage units (negative: behind the edge).
    fn back_dist(&self, i: usize) -> f32 {
        let p = &self.players[i];
        let x = p.pos[0] * self.local_scale(i);
        (x - self.edges(p.facing).0) * p.facing
    }

    fn being_hit(&self, i: usize) -> bool {
        let p = &self.players[i];
        p.move_type == rugen_cns::state::MoveType::H
            || p.bind.is_some()
            || matches!(p.state_no, 5120 | 5200 | 5210)
    }

    /// The next living member after `from` (`dir` 1 forward, -1 back), as
    /// Ikemen `TagGetPartnerId`.
    fn tag_partner(&self, side: usize, from: usize, dir: i32) -> Option<usize> {
        let m = &self.teams.members[side];
        let n = m.len() as i32;
        let at = m.iter().position(|&k| k == from)? as i32;
        (1..n)
            .map(|k| m[((at + dir * k) % n + n) as usize % n as usize])
            .find(|&k| self.players[k].alive())
    }

    /// Ikemen `tag.zss` `[StateDef -2]` for every Tag side, once per tick
    /// before the states run: partners leave at the start of the fight,
    /// the leader tags a waiting partner in (TAG button, or the computer
    /// now and then), a fallen leader's partner jumps in, waiting partners
    /// win their red life back and jump in for the win pose.
    pub(crate) fn tag_step(&mut self) {
        let rs = self.round.round_state();
        for side in 0..2 {
            if self.teams.mode[side] != TeamMode::Tag {
                continue;
            }
            let members = self.teams.members[side].clone();
            // red life follows the damage taken
            for &i in &members {
                let p = &mut self.players[i];
                if p.life < p.last_life {
                    let lost = (p.last_life - p.life) as f32 * tag_const::RED_MUL;
                    p.red_life -= libm::roundf(lost) as i32;
                }
                p.red_life = p.red_life.clamp(p.life.max(0), p.life_max);
                p.last_life = p.life;
            }
            if rs == 2 {
                self.tag_fight(side, &members);
            } else if rs == 3 {
                // the round is decided: the partners outside jump in to pose
                for &i in &members {
                    if self.players[i].tag == TagPhase::Waiting && self.players[i].alive() {
                        self.tag_jump_in(side, i);
                    }
                }
            }
            // waiting outside wins red life back
            if self.frame % tag_const::REGEN_TICKS == 0 {
                for &i in &members {
                    let p = &mut self.players[i];
                    if p.tag == TagPhase::Waiting && p.alive() && p.life < p.red_life {
                        let step = libm::ceilf(p.life_max as f32 * tag_const::REGEN_SHARE) as i32;
                        p.life += step.min(p.red_life - p.life);
                        p.last_life = p.life;
                    }
                }
            }
        }
    }

    fn tag_fight(&mut self, side: usize, members: &[usize]) {
        let leader = self.teams.leader[side];
        // partners on screen (after the intro) leave
        for &i in members {
            let p = &self.players[i];
            if i != leader
                && p.alive()
                && p.tag == TagPhase::None
                && !self.being_hit(i)
                && (p.state_no == 0 || p.pos[1] >= p.ground)
            {
                self.tag_out(i);
            }
        }
        let lp = &self.players[leader];
        // the leader fell and hit the ground: the next partner jumps in
        if !lp.alive() {
            if matches!(lp.state_no, 5110 | 5150) && !lp.standby {
                self.players[leader].standby = true;
                if let Some(next) = self.tag_partner(side, leader, 1) {
                    self.teams.last_out[side] = Some(leader);
                    self.teams.leader[side] = next;
                    self.tag_jump_in(side, next);
                }
            }
            return;
        }
        // the last leader must be outside (or down) before the next switch
        if let Some(out) = self.teams.last_out[side] {
            let o = &self.players[out];
            if o.tag != TagPhase::Waiting && o.alive() {
                return;
            }
            self.teams.last_out[side] = None;
        }
        let lp = &self.players[leader];
        if !lp.ctrl || self.being_hit(leader) || !(lp.state_no == 0 || lp.pos[1] >= lp.ground) {
            return;
        }
        let want = if lp.ai_level > 0 {
            // the computer tags out now and then, more often when hurt
            let hurt = self.tag_partner(side, leader, 1).is_some_and(|n| {
                let (a, b) = (&self.players[leader], &self.players[n]);
                a.life * b.life_max < b.life * a.life_max
            });
            let n = if hurt {
                tag_const::AI_CHANCE / 3
            } else {
                tag_const::AI_CHANCE
            };
            (self.rng.below(n) == 0)
                .then(|| self.tag_partner(side, leader, 1))
                .flatten()
        } else if lp.tag_press {
            self.tag_partner(side, leader, 1)
        } else {
            None
        };
        let Some(next) = want else { return };
        if self.players[next].tag != TagPhase::Waiting {
            return;
        }
        // switch: the leader runs off, the partner runs on from behind it
        let (lx, lf) = {
            let p = &self.players[leader];
            (p.pos[0] * self.local_scale(leader), p.facing)
        };
        self.tag_out(leader);
        self.teams.last_out[side] = Some(leader);
        self.teams.leader[side] = next;
        let s = self.local_scale(next);
        let back = self.edges(lf).0 - lf * tag_const::START_BEHIND * self.s240();
        let dist = (lx - back).abs();
        let vel = (tag_const::IN_VEL_X * self.s240()).min(dist / tag_const::IN_TIME);
        let anim = self.tag_anim(next, &[100, 20, 0]);
        let p = &mut self.players[next];
        p.standby = false;
        p.tag = TagPhase::Entering;
        p.facing = lf;
        p.pos = [back / s, 0.0];
        p.old_pos = p.pos;
        p.vel = [vel / s, 0.0];
        p.tag_dist = dist;
        p.ctrl = false;
        p.state_no = 6565600;
        p.state_time = 0;
        self.change_anim(next, anim, None);
        self.players[leader].tag_press = false;
    }

    /// The first of `anims` the player has (Ikemen falls back to its own).
    fn tag_anim(&self, i: usize, anims: &[i32]) -> i32 {
        let d = &self.data[self.players[i].data];
        anims
            .iter()
            .copied()
            .find(|&a| d.air.get(a).is_some())
            .unwrap_or(0)
    }

    /// `tagOut{stateno: StateTagLeavingScreen}`.
    fn tag_out(&mut self, i: usize) {
        // run away from the fight: turn from the nearest enemy
        let away = match self.nearest_enemy(i) {
            Some(e) => {
                if self.rel_pos(i, e)[0] > 0.0 {
                    -1.0
                } else {
                    1.0
                }
            }
            None => -self.players[i].facing,
        };
        let d = self.data[self.players[i].data].clone();
        let run = d
            .konst("velocity.run.fwd.x")
            .filter(|v| *v != 0.0)
            .unwrap_or_else(|| d.konst("velocity.walk.fwd.x").unwrap_or(2.4) * 2.0);
        let cap = tag_const::IN_VEL_X * self.s240() / self.local_scale(i);
        let anim = self.tag_anim(i, &[100, 20, 0]);
        let p = &mut self.players[i];
        p.standby = true;
        p.tag = TagPhase::Leaving;
        p.facing = away;
        p.vel = [run.max(5.0).min(cap), 0.0];
        p.ctrl = false;
        p.hitdef = None;
        p.state_no = 6565610;
        p.state_time = 0;
        self.change_anim(i, anim, None);
    }

    /// `StateTagJumpingIn`: from behind the leader's back edge, in an arc.
    fn tag_jump_in(&mut self, side: usize, i: usize) {
        let leader = self.teams.leader[side];
        let lf = if leader == i {
            self.players[self.teams.last_out[side].unwrap_or(i)].facing
        } else {
            self.players[leader].facing
        };
        let s = self.local_scale(i);
        let k = self.s240() / s;
        let back = self.edges(lf).0 - lf * tag_const::START_BEHIND * self.s240();
        let vx = if self.round.round_state() == 2 {
            4.0
        } else {
            8.0 - 0.75 * f32::from(self.players[i].member)
        };
        let anim = self.tag_anim(i, &[42, 41, 0]);
        let p = &mut self.players[i];
        p.standby = false;
        p.tag = TagPhase::JumpingIn;
        p.facing = lf;
        p.pos = [back / s, 0.0];
        p.old_pos = p.pos;
        p.vel = [vx * k, -10.0 * k];
        p.ctrl = false;
        p.state_no = 6565620;
        p.state_time = 0;
        self.change_anim(i, anim, None);
    }

    /// One tick of a player in a [`TagPhase`] (instead of its states).
    pub(crate) fn tag_phase_step(&mut self, i: usize) {
        use crate::player::assert;
        if self.players[i].pause_bool {
            return;
        }
        let s = self.local_scale(i);
        let k = self.s240() / s;
        {
            let p = &mut self.players[i];
            p.screen_bound = false;
            p.player_push = false;
            p.assert_flags |= assert::NOAUTOTURN;
            p.unhittable_time = p.unhittable_time.max(2);
            p.state_time += 1;
        }
        match self.players[i].tag {
            TagPhase::None => {}
            TagPhase::Leaving => {
                let p = &mut self.players[i];
                p.pos[0] += p.vel[0] * p.facing;
                let x = p.pos[0] * s;
                let gone = tag_const::GONE * self.s240();
                let (l, r) = (self.screen_left(), self.screen_right());
                if x < l - gone || x > r + gone {
                    self.tag_wait(i);
                }
            }
            TagPhase::Waiting => self.tag_wait(i),
            TagPhase::Entering => {
                let stop = {
                    let p = &mut self.players[i];
                    p.pos[0] += p.vel[0] * p.facing;
                    p.tag_dist -= p.vel[0] * s;
                    p.tag_dist <= 0.0
                };
                let near = self.nearest_enemy(i).is_some_and(|e| {
                    let dx = self.rel_pos(i, e)[0] * self.players[i].facing;
                    dx * s < 30.0 * self.s240() + 30.0
                });
                if self.back_dist(i) >= 0.0 && (stop || near) {
                    self.tag_land(i, true);
                }
            }
            TagPhase::JumpingIn => {
                let p = &mut self.players[i];
                p.pos[0] += p.vel[0] * p.facing;
                p.pos[1] += p.vel[1];
                p.vel[1] += 0.5 * k;
                if p.vel[1] > 0.0 && p.pos[1] >= 0.0 {
                    p.pos[1] = 0.0;
                    p.vel = [0.0, 0.0];
                    p.tag = TagPhase::Landing;
                    p.state_no = 6565621;
                    p.state_time = 0;
                    let anim = self.tag_anim(i, &[47, 0]);
                    self.change_anim(i, anim, None);
                }
            }
            TagPhase::Landing => {
                let d = self.data[self.players[i].anim_owner].clone();
                let p = &self.players[i];
                let done = d
                    .air
                    .get(p.anim.action)
                    .is_none_or(|a| p.anim.anim_time(a) >= 0);
                if done {
                    // control only while the round is still on
                    let fight = self.round.round_state() == 2;
                    self.tag_land(i, fight);
                }
            }
        }
    }

    /// `StateTagWaitingOutside`: invisible, far outside its side.
    fn tag_wait(&mut self, i: usize) {
        use crate::player::assert;
        let s = self.local_scale(i);
        let zoom = self.cam_zoom.clamp(0.05, 1.0);
        let p = &mut self.players[i];
        if p.tag != TagPhase::Waiting {
            p.tag = TagPhase::Waiting;
            p.state_no = 6565611;
            p.state_time = 0;
        }
        // face the centre, stand outside on the side behind it
        if p.pos[0] * p.facing > 0.0 {
            p.facing = -p.facing;
        }
        p.vel = [0.0, 0.0];
        p.pos = [
            -tag_const::WAIT_X * (self.stage.width / 320.0) * p.facing / zoom / s,
            0.0,
        ];
        p.old_pos = p.pos;
        p.assert_flags |= assert::INVISIBLE | assert::NOSHADOW;
    }

    /// Back on its feet in its own states (state 0, `ctrl`).
    fn tag_land(&mut self, i: usize, ctrl: bool) {
        let p = &mut self.players[i];
        p.tag = TagPhase::None;
        p.vel = [0.0, 0.0];
        p.pos[1] = 0.0;
        self.self_state(i, 0, None, Some(ctrl));
        self.run_current_pub(i);
    }

    /// Round start in a Tag team: everybody on screen for the intros.
    pub(crate) fn tag_reset(&mut self, i: usize) {
        let p = &mut self.players[i];
        p.standby = false;
        p.tag = TagPhase::None;
        p.red_life = p.life;
        p.last_life = p.life;
        p.tag_press = false;
    }

    /// Round over in a team match: the Turns side that lost sends its next
    /// fighter (the `n`th member after `n` lost rounds), who brings the
    /// fallen one's power; the winners keep their life.
    pub(crate) fn turns_advance(&mut self) {
        self.teams.keep_life = false;
        let win = self.round.win_team;
        if win < 0 {
            return;
        }
        let lost = 1 - win as usize;
        if self.teams.mode[lost] != TeamMode::Turns {
            return;
        }
        let members = self.teams.members[lost].clone();
        let Some(&out) = members.iter().find(|&&m| !self.players[m].removed) else {
            return;
        };
        let next = self.round.wins[win as usize] as usize;
        let Some(&inc) = members.get(next) else {
            return;
        };
        if inc == out {
            return;
        }
        let power = self.players[out].power;
        self.players[out].removed = true;
        let p = &mut self.players[inc];
        p.removed = false;
        p.life = p.life_max;
        p.power = power.min(p.power_max);
        self.teams.keep_life = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::{MatchSpec, TeamSpec, build_team_world};
    use std::path::PathBuf;

    fn reference() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1")
    }

    /// KFM team match on the KFM stage (None when the reference is absent).
    fn team_world(mode: [TeamMode; 2], sizes: [usize; 2], ai: [i32; 2]) -> Option<World> {
        let root = reference();
        let kfm = root.join("chars/kfm/kfm.def");
        let stage = root.join("stages/kfm.def");
        if !kfm.exists() {
            return None;
        }
        let spec = MatchSpec {
            p1: &kfm,
            p2: &kfm,
            stage: Some(&stage),
            data_dir: &root.join("data"),
            motif: &root.join("data/system.def"),
            zoom: crate::headless::ONLINE_ZOOM,
            movement: Default::default(),
            round_time: None,
            ai,
            fighter_size: Default::default(),
        };
        let partners = sizes.map(|n| vec![kfm.as_path(); n.saturating_sub(1)]);
        let t = TeamSpec {
            mode,
            partners,
            partner_ai: [4, 4],
        };
        Some(build_team_world(&spec, &t).expect("team world"))
    }

    fn eval(w: &mut World, i: usize, src: &str) -> i32 {
        let code = rugen_cns::compile(src).expect(src);
        w.eval(i, &code).to_i()
    }

    fn until(w: &mut World, max: u32, done: impl Fn(&World) -> bool) {
        for _ in 0..max {
            if done(w) {
                return;
            }
            w.step(&[]);
        }
        panic!("condition not reached in {max} ticks (frame {})", w.frame);
    }

    /// Fight, then knock `side`'s fighter out and wait for the next round.
    fn ko(w: &mut World, side: usize) {
        until(w, 400, |w| w.round.round_state() == 2);
        let i = w.teams.members[side]
            .iter()
            .copied()
            .find(|&m| !w.players[m].removed)
            .unwrap();
        w.players[i].life = 0;
        let (round, over) = (w.round.round_no, w.round.match_over);
        assert!(!over);
        until(w, 2000, |w| w.round.round_no > round || w.round.match_over);
    }

    fn active(w: &World, side: usize) -> Vec<usize> {
        w.teams.members[side]
            .iter()
            .copied()
            .filter(|&m| !w.players[m].removed)
            .collect()
    }

    /// Turns: one fighter per side at a time; the loser's next fighter comes
    /// in with the fallen one's power, the winner stays with the life it
    /// had; the match ends when a side has nobody left (Ikemen rules).
    #[test]
    fn turns_bring_the_next_fighter_and_the_winner_keeps_its_life() {
        let Some(mut w) = team_world([TeamMode::Turns, TeamMode::Single], [3, 1], [0, 0]) else {
            return;
        };
        // Ikemen's player order: P1, P2, then P1's partners
        assert_eq!(w.teams.members, [vec![0, 2, 3], vec![1]]);
        assert_eq!(active(&w, 0), vec![0]);
        assert_eq!(
            w.players[2].input_slot, 0,
            "the side's controls drive every member"
        );
        // the single fighter must beat all three, the team two rounds
        assert_eq!(w.wins_needed(1), 3);
        assert_eq!(w.wins_needed(0), 2);
        assert_eq!(eval(&mut w, 0, "TeamMode = Turns"), 1);
        assert_eq!(eval(&mut w, 1, "TeamMode = Single"), 1);
        assert_eq!(eval(&mut w, 0, "NumPartner"), 0);
        assert_eq!(eval(&mut w, 0, "NumEnemy"), 1);

        until(&mut w, 400, |w| w.round.round_state() == 2);
        w.players[1].life = 640;
        w.players[0].power = 1500;
        ko(&mut w, 0);
        assert_eq!(w.round.wins, [0, 1]);
        assert_eq!(active(&w, 0), vec![2], "the second pick comes in");
        let next = &w.players[2];
        assert_eq!(next.life, next.life_max);
        assert_eq!(next.power, 1500, "with the fallen fighter's power");
        assert_eq!(w.players[1].life, 640, "the winner keeps its life");
        // the newcomer starts where P1 starts
        let (x, _) = w.start_pos(2);
        assert_eq!(w.players[2].pos[0], x / w.local_scale(2));
        assert_eq!(w.start_pos(2), w.start_pos(0));

        // the team wins a round: a normal round, everyone back to full life
        w.players[2].life = 300;
        ko(&mut w, 1);
        assert_eq!(w.round.wins, [1, 1]);
        assert_eq!(active(&w, 0), vec![2], "a winner stays");
        assert_eq!(w.players[2].life, w.players[2].life_max);

        ko(&mut w, 0);
        assert_eq!(active(&w, 0), vec![3], "the last pick");
        ko(&mut w, 0);
        assert!(w.round.match_over);
        assert_eq!(w.round.wins, [1, 3]);
    }

    /// Turns against Turns: each side needs the other's team size.
    #[test]
    fn turns_against_turns_need_the_whole_other_team() {
        let Some(mut w) = team_world([TeamMode::Turns, TeamMode::Turns], [2, 3], [0, 0]) else {
            return;
        };
        assert_eq!(w.teams.members, [vec![0, 2], vec![1, 3, 4]]);
        assert_eq!((w.wins_needed(0), w.wins_needed(1)), (3, 2));
        ko(&mut w, 1);
        assert_eq!(active(&w, 1), vec![3]);
        ko(&mut w, 1);
        assert_eq!(active(&w, 1), vec![4]);
        ko(&mut w, 0);
        assert_eq!(active(&w, 0), vec![2]);
        assert_eq!(active(&w, 1), vec![4]);
        ko(&mut w, 1);
        assert!(w.round.match_over);
        assert_eq!(w.round.wins, [3, 1]);
    }

    /// Simul: partners fight on screen together, take P3 / P4's start
    /// positions, see each other through `Partner` / `NumPartner` and the
    /// enemies through `EnemyNear(n)`; a side is down when every fighter
    /// is, or at once when its human fighter is (Ikemen `Simul.LoseOnKO`).
    #[test]
    fn simul_partners_share_the_screen() {
        let Some(mut w) = team_world([TeamMode::Simul, TeamMode::Simul], [2, 2], [0, 4]) else {
            return;
        };
        assert_eq!(w.teams.members, [vec![0, 2], vec![1, 3]]);
        assert_eq!(
            w.players[2].ai_level, 4,
            "a human side's partner is a computer player"
        );
        assert_eq!(w.players[3].ai_level, 4);
        let x: Vec<f32> = (0..4)
            .map(|i| w.players[i].pos[0] * w.local_scale(i))
            .collect();
        assert_eq!(x, vec![-70.0, 70.0, -95.0, 95.0], "Ikemen partner spacing");
        for i in 0..4 {
            assert_eq!(eval(&mut w, i, "NumPartner"), 1);
            assert_eq!(eval(&mut w, i, "NumEnemy"), 2);
            assert_eq!(eval(&mut w, i, "TeamMode = Simul"), 1);
            assert_eq!(eval(&mut w, i, "TeamMode"), 1);
        }
        assert_eq!(eval(&mut w, 0, "Partner, ID"), w.players[2].id);
        assert_eq!(eval(&mut w, 3, "Partner, ID"), w.players[1].id);
        assert_eq!(eval(&mut w, 0, "EnemyNear, ID"), w.players[1].id);
        assert_eq!(eval(&mut w, 0, "EnemyNear(1), ID"), w.players[3].id);
        assert_eq!(eval(&mut w, 0, "Enemy(1), ID"), w.players[3].id);
        assert_eq!(eval(&mut w, 0, "TeamSide"), 1);
        assert_eq!(eval(&mut w, 3, "TeamSide"), 2);

        // the computer partner falls: the round goes on
        until(&mut w, 400, |w| w.round.round_state() == 2);
        w.players[2].life = 0;
        for _ in 0..60 {
            w.step(&[]);
        }
        assert_eq!(w.round.finish, crate::Finish::NotYet);
        // one CPU of the other side falls: still on
        w.players[3].life = 0;
        for _ in 0..60 {
            w.step(&[]);
        }
        assert_eq!(w.round.finish, crate::Finish::NotYet);
        // the human fighter falls: the side loses at once
        w.players[0].life = 0;
        w.step(&[]);
        w.step(&[]);
        assert_eq!(w.round.finish, crate::Finish::KO);
        assert_eq!(w.round.win_team, 1);
    }

    /// Simul time over: each fighter counts `1 / team size` of its side.
    #[test]
    fn simul_time_over_weighs_each_partner() {
        let Some(mut w) = team_world([TeamMode::Simul, TeamMode::Single], [2, 1], [4, 4]) else {
            return;
        };
        until(&mut w, 400, |w| w.round.round_state() == 2);
        let max = w.players[0].life_max;
        // 1v2 at 60 %: the pair at 100 % + 10 % has 55 %
        w.players[1].life = max * 6 / 10;
        w.players[0].life = max;
        w.players[2].life = max / 10;
        for p in &mut w.players {
            p.ai_level = 0;
        }
        w.round.time_left = 1;
        w.step(&[]);
        w.step(&[]);
        assert_eq!(w.round.finish, crate::Finish::TimeOver);
        assert_eq!(w.round.win_team, 1);
    }

    /// Team matches replay exactly: the same inputs give the same world,
    /// also from a copy taken mid-match (rollback / replays).
    #[test]
    fn team_matches_are_deterministic() {
        let modes = [
            ([TeamMode::Turns, TeamMode::Turns], [3, 2]),
            ([TeamMode::Simul, TeamMode::Simul], [2, 2]),
            ([TeamMode::Simul, TeamMode::Turns], [2, 3]),
            ([TeamMode::Tag, TeamMode::Tag], [3, 2]),
            ([TeamMode::Tag, TeamMode::Simul], [2, 2]),
        ];
        for (mode, sizes) in modes {
            let Some(mut a) = team_world(mode, sizes, [6, 6]) else {
                return;
            };
            let mut b = team_world(mode, sizes, [6, 6]).unwrap();
            for _ in 0..2500 {
                a.step(&[]);
                b.step(&[]);
            }
            let mut c = a.clone();
            for _ in 0..2500 {
                a.step(&[]);
                c.step(&[]);
            }
            for _ in 0..2500 {
                b.step(&[]);
            }
            assert_eq!(a.checksum(), b.checksum(), "{mode:?}");
            assert_eq!(a.checksum(), c.checksum(), "{mode:?} from a copy");
        }
    }

    const TAG: crate::Buttons = crate::Buttons {
        u: false,
        d: false,
        l: false,
        r: false,
        a: false,
        b: false,
        c: false,
        x: false,
        y: false,
        z: false,
        s: false,
        t: true,
    };

    fn ticks(w: &mut World, n: u32) {
        for _ in 0..n {
            w.step(&[]);
        }
    }

    /// Tag (Ikemen GO `tag.zss`): the partner runs off once the fight
    /// starts and waits outside, out of reach; TAG sends the leader off and
    /// brings the partner on from behind; a fallen leader's partner jumps in.
    #[test]
    fn tag_partners_switch_in_and_out() {
        let Some(mut w) = team_world([TeamMode::Tag, TeamMode::Single], [2, 1], [0, 4]) else {
            return;
        };
        assert_eq!(w.teams.members[0], vec![0, 2]);
        assert_eq!(
            w.players[2].input_slot, 0,
            "the side's controls drive whoever is in"
        );
        assert_eq!(w.side_leader(0), Some(0));
        assert_eq!(eval(&mut w, 0, "TeamMode = Tag"), 1);
        assert_eq!(eval(&mut w, 0, "NumPartner"), 1);
        until(&mut w, 400, |w| w.round.round_state() == 2);
        until(&mut w, 300, |w| w.players[2].tag == TagPhase::Waiting);
        let p = &w.players[2];
        assert!(p.standby);
        assert!(p.asserted(crate::player::assert::INVISIBLE));
        assert_eq!(
            w.nearest_enemy(1),
            Some(0),
            "a partner outside is nobody's P2"
        );
        // TAG: the leader runs off, the partner runs on
        until(&mut w, 300, |w| {
            w.players[0].ctrl && w.players[0].state_no == 0
        });
        w.players[1].ai_level = 0;
        w.step(&[TAG]);
        assert_eq!(w.players[0].tag, TagPhase::Leaving);
        assert!(w.players[0].standby);
        assert_eq!(w.players[2].tag, TagPhase::Entering);
        assert_eq!(w.side_leader(0), Some(2));
        assert_eq!(eval(&mut w, 2, "StateNo"), 6565600);
        until(&mut w, 200, |w| w.players[2].tag == TagPhase::None);
        let p = &w.players[2];
        assert!(!p.standby && p.ctrl);
        assert_eq!(p.state_no, 0);
        // no switch back before the last leader is outside
        w.step(&[]);
        w.step(&[TAG]);
        assert_eq!(w.side_leader(0), Some(2));
        until(&mut w, 300, |w| w.players[0].tag == TagPhase::Waiting);
        until(&mut w, 300, |w| {
            w.players[2].ctrl && w.players[2].state_no == 0
        });
        // a held TAG switches once
        for _ in 0..90 {
            w.step(&[TAG]);
        }
        assert_eq!(w.side_leader(0), Some(0));
        assert_eq!(
            w.players[2].tag,
            TagPhase::Waiting,
            "out again, and staying out"
        );
        w.step(&[]);
        until(&mut w, 200, |w| w.players[0].tag == TagPhase::None);

        // the leader falls: the partner jumps in, the round goes on
        until(&mut w, 300, |w| w.players[2].tag == TagPhase::Waiting);
        w.players[0].life = 0;
        until(&mut w, 400, |w| w.players[2].tag == TagPhase::JumpingIn);
        assert_eq!(w.side_leader(0), Some(2));
        assert_eq!(w.round.finish, crate::Finish::NotYet);
        until(&mut w, 300, |w| w.players[2].tag == TagPhase::None);
        assert!(w.players[2].ctrl);
        // the last one falls: the round is over
        w.players[2].life = 0;
        until(&mut w, 30, |w| w.round.finish != crate::Finish::NotYet);
        assert_eq!(w.round.win_team, 1);
    }

    /// Red life: a Tag fighter keeps a quarter of the damage as red life
    /// and wins it back, 0.5 % of its life every 30 ticks, while outside.
    #[test]
    fn tag_partners_win_red_life_back_outside() {
        let Some(mut w) = team_world([TeamMode::Tag, TeamMode::Single], [2, 1], [0, 0]) else {
            return;
        };
        until(&mut w, 400, |w| w.round.round_state() == 2);
        until(&mut w, 300, |w| w.players[2].tag == TagPhase::Waiting);
        until(&mut w, 300, |w| {
            w.players[0].ctrl && w.players[0].state_no == 0
        });
        let max = w.players[0].life_max;
        w.players[0].life = max - 400;
        w.step(&[]);
        assert_eq!(
            w.players[0].red_life,
            max - 300,
            "a quarter of the damage stays red"
        );
        w.step(&[TAG]);
        until(&mut w, 400, |w| w.players[0].tag == TagPhase::Waiting);
        let before = w.players[0].life;
        ticks(&mut w, 600);
        let after = w.players[0].life;
        assert!(after > before, "{before} -> {after}");
        ticks(&mut w, 3000);
        assert_eq!(
            w.players[0].life,
            max - 300,
            "up to the red life, no further"
        );
    }

    #[test]
    fn modes_parse_and_number_like_ikemen() {
        for m in TeamMode::ALL {
            assert_eq!(TeamMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(TeamMode::parse("TAG"), Some(TeamMode::Tag));
        assert_eq!(TeamMode::parse("duo"), None);
        assert_eq!(TeamMode::Turns.raw(), 2);
        assert_eq!(TeamMode::Single.sizes(), 1..=1);
        assert_eq!(TeamMode::Simul.sizes(), 2..=4);
    }
}
