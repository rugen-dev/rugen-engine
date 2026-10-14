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
