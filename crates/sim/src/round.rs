//! Round and match flow (Ikemen `stepRoundState` / `roundEndDecision`).

use rugen_cns::state::{MoveType, StateType};

use crate::player::assert;
use crate::world::{Event, World};

/// Timings from `fight.def [Round]` and `[Time]`, plus `mugen.cfg` rules.
#[derive(Clone, Debug, PartialEq)]
pub struct RoundConfig {
    pub start_waittime: i32,
    pub ctrl_time: i32,
    pub over_waittime: i32,
    pub over_hittime: i32,
    pub over_wintime: i32,
    pub over_time: i32,
    pub slow_time: i32,
    pub win_time: i32,
    pub match_wins: u8,
    pub max_drawgames: i32,
    /// Seconds on the clock; <= 0 = infinite.
    pub round_time: i32,
    pub frames_per_count: i32,
}

impl Default for RoundConfig {
    fn default() -> Self {
        Self {
            start_waittime: 30,
            ctrl_time: 30,
            over_waittime: 45,
            over_hittime: 10,
            over_wintime: 45,
            over_time: 210,
            slow_time: 60,
            win_time: 60,
            match_wins: 2,
            max_drawgames: 1,
            round_time: 99,
            frames_per_count: 60,
        }
    }
}

impl RoundConfig {
    pub fn from_fight_def(def: &rugen_formats::def::Def) -> Self {
        let mut c = Self::default();
        if let Some(r) = def.section("round") {
            let g = |k: &str, d: i32| r.i32(k).unwrap_or(d);
            c.start_waittime = g("start.waittime", c.start_waittime);
            c.ctrl_time = g("ctrl.time", c.ctrl_time);
            c.over_waittime = g("over.waittime", c.over_waittime);
            c.over_hittime = g("over.hittime", c.over_hittime);
            c.over_wintime = g("over.wintime", c.over_wintime);
            c.over_time = g("over.time", c.over_time);
            c.slow_time = g("slow.time", c.slow_time);
            c.win_time = g("win.time", c.win_time);
            c.match_wins = g("match.wins", c.match_wins as i32).clamp(1, 9) as u8;
            c.max_drawgames = g("match.maxdrawgames", c.max_drawgames);
        }
        if let Some(t) = def.section("time") {
            c.frames_per_count = t.i32("framespercount").unwrap_or(c.frames_per_count).max(1);
        }
        c
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Finish {
    #[default]
    NotYet,
    KO,
    DoubleKO,
    TimeOver,
    TimeOverDraw,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoundState {
    pub config: RoundConfig,
    pub round_no: i32,
    /// Countdown through the intro; 0 = fighting; negative = round over.
    pub intro: i32,
    /// Ticks left on the clock (-1 = infinite).
    pub time_left: i32,
    pub finish: Finish,
    /// Winning team (0/1) or -1.
    pub win_team: i32,
    pub wins: [u8; 2],
    /// Per team, how each round was won: `n` normal, `s` special, `h` hyper,
    /// `throw`, `t` time over, `perfect`, `suicide` (fight.def WinIcon keys).
    pub win_kinds: [Vec<String>; 2],
    pub draws: i32,
    pub winposetime: i32,
    pub match_over: bool,
    /// Players that already entered their win/lose state this round.
    over_flagged: Vec<bool>,
    pub wins_counted: bool,
}

impl RoundState {
    pub fn new(config: RoundConfig) -> Self {
        Self {
            intro: config.start_waittime + config.ctrl_time + 1,
            time_left: if config.round_time > 0 {
                config.round_time * config.frames_per_count
            } else {
                -1
            },
            winposetime: config.over_wintime,
            config,
            round_no: 1,
            finish: Finish::NotYet,
            win_team: -1,
            wins: [0, 0],
            win_kinds: [Vec::new(), Vec::new()],
            draws: 0,
            match_over: false,
            over_flagged: Vec::new(),
            wins_counted: false,
        }
    }

    /// MUGEN `RoundState`: 0 pre-intro, 1 intro, 2 fight, 3 over (pre-win), 4 over.
    pub fn round_state(&self) -> i32 {
        if self.intro > self.config.ctrl_time + 1 {
            0
        } else if self.intro > 0 {
            1
        } else if self.intro == 0 || self.finish == Finish::NotYet {
            2
        } else if self.intro < -self.config.over_waittime {
            4
        } else {
            3
        }
    }

    pub fn round_over(&self) -> bool {
        self.intro < -(self.config.over_waittime + self.config.over_time)
    }

    /// Seconds shown on the clock.
    pub fn clock(&self) -> i32 {
        if self.time_left < 0 {
            -1
        } else {
            (self.time_left + self.config.frames_per_count - 1) / self.config.frames_per_count
        }
    }
}

impl World {
    /// Classify a round win for the WinIcon display.
    fn win_kind(&self, roots: &[usize], team: usize) -> String {
        use crate::hit::attr;
        let winner = roots
            .iter()
            .copied()
            .find(|&i| self.players[i].team as usize == team);
        let loser = roots
            .iter()
            .copied()
            .find(|&i| self.players[i].team as usize != team);
        if self.round.finish == Finish::TimeOver {
            return "t".into();
        }
        if let Some(w) = winner
            && self.players[w].life >= self.players[w].life_max
        {
            return "perfect".into();
        }
        let Some(l) = loser else { return "n".into() };
        let lp = &self.players[l];
        if lp.ghv.playerid == lp.id {
            return "suicide".into();
        }
        let a = lp.ghv.attr;
        if a & attr::AT_AH != 0 {
            "h".into()
        } else if a & (attr::AT_SA | attr::AT_ST | attr::AT_SP) != 0 {
            "s".into()
        } else if a & attr::AT_NT != 0 {
            "throw".into()
        } else {
            "n".into()
        }
    }

    /// Reset players and timers for round `round.round_no` and run the 5900 init state.
    pub fn start_round(&mut self) {
        let cfg = self.round.config.clone();
        self.round.intro = cfg.start_waittime + cfg.ctrl_time + 1;
        self.round.time_left = if cfg.round_time > 0 {
            cfg.round_time * cfg.frames_per_count
        } else {
            -1
        };
        self.round.finish = Finish::NotYet;
        self.round.win_team = -1;
        self.round.winposetime = cfg.over_wintime;
        self.round.wins_counted = false;
        self.round.over_flagged = vec![false; self.players.len()];
        self.explods.clear();
        self.projs.clear();
        self.pausetime = 0;
        self.supertime = 0;
        self.pausetime_buffer = 0;
        self.supertime_buffer = 0;
        self.superbrightness = 1.0;
        // helpers go away between rounds
        for p in &mut self.players {
            if p.helper_index > 0 {
                p.removed = true;
            }
        }
        let roots: Vec<usize> = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0)
            .map(|(i, _)| i)
            .collect();
        for i in roots {
            let data = self.data[self.players[i].data].clone();
            let (x, facing) = if self.players[i].team == 0 {
                (self.stage.p1_start_x, 1.0)
            } else {
                (self.stage.p2_start_x, -1.0)
            };
            let x = x * data.localcoord[0] / self.stage.width;
            let int_persist = data.konst("data.intpersistindex").unwrap_or(60.0) as usize;
            let float_persist = data.konst("data.floatpersistindex").unwrap_or(40.0) as usize;
            let p = &mut self.players[i];
            p.pos = [x, 0.0];
            p.old_pos = p.pos;
            p.vel = [0.0, 0.0];
            p.facing = facing;
            p.life = p.life_max;
            p.ko = false;
            p.ctrl = false;
            p.hitdef = None;
            p.targets.clear();
            p.hitdef_targets.clear();
            p.ghv = Default::default();
            p.mhv = Default::default();
            p.mctype = Default::default();
            p.mctime = 0;
            p.hit_pause_time = 0;
            p.gethit = false;
            p.hittmp = 0;
            p.hit_count = 0;
            p.guard_count = 0;
            p.unhittable_time = 0;
            p.hitby = Default::default();
            p.attack_mul = 1.0;
            p.defence_mul = 1.0;
            p.fall_defence_mul = 1.0;
            p.super_defence_mul = 1.0;
            p.bind = None;
            p.pause_move_time = 0;
            p.super_move_time = 0;
            p.children.clear();
            p.pctime = -1;
            p.state_owner = p.data;
            p.state_type = StateType::S;
            p.move_type = MoveType::I;
            for v in p.vars.iter_mut().take(int_persist) {
                *v = 0;
            }
            for v in p.fvars.iter_mut().take(float_persist) {
                *v = 0.0;
            }
            p.sysvars.iter_mut().for_each(|v| *v = 0);
            p.sysfvars.iter_mut().for_each(|v| *v = 0.0);
            p.cmds.reset();
            p.input = Default::default();
            self.self_state(i, 5900, Some(0), Some(false));
            self.run_current_pub(i);
        }
        self.update_camera();
        let n = self.round.round_no;
        self.events.push(Event::RoundStart(n));
    }

    /// Advance the round timers; called at the start of every tick.
    pub(crate) fn step_round(&mut self) {
        let cfg = self.round.config.clone();
        let roots: Vec<usize> = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0 && !p.removed)
            .map(|(i, _)| i)
            .collect();
        if self.round.intro > cfg.ctrl_time {
            self.round.intro -= 1;
            let holding = roots
                .iter()
                .any(|&i| self.players[i].asserted(assert::INTRO));
            if holding && self.round.intro <= cfg.ctrl_time {
                self.round.intro = cfg.ctrl_time + 1;
            }
            if self.round.intro == cfg.ctrl_time {
                self.events.push(Event::Fight);
            }
        } else if self.round.intro > 0 {
            if self.round.intro == cfg.ctrl_time {
                for &i in &roots {
                    if !self.players[i].asserted(assert::NOINTRORESET) {
                        let data = self.data[self.players[i].data].clone();
                        let (x, facing) = if self.players[i].team == 0 {
                            (self.stage.p1_start_x, 1.0)
                        } else {
                            (self.stage.p2_start_x, -1.0)
                        };
                        let p = &mut self.players[i];
                        p.pos = [x * data.localcoord[0] / self.stage.width, 0.0];
                        p.old_pos = p.pos;
                        p.vel = [0.0, 0.0];
                        p.facing = facing;
                    }
                }
            }
            self.round.intro -= 1;
            if self.round.intro == 0 {
                for &i in &roots {
                    if self.players[i].alive() {
                        self.players[i].ctrl = true;
                        if self.players[i].state_no != 0
                            && !self.players[i].asserted(assert::NOINTRORESET)
                        {
                            self.self_state(i, 0, None, Some(true));
                            self.run_current_pub(i);
                        }
                    }
                }
            }
        }
        let timer_frozen = roots
            .iter()
            .any(|&i| self.players[i].asserted(assert::TIMERFREEZE));
        if self.round.intro == 0
            && !timer_frozen
            && self.supertime <= 0
            && self.pausetime <= 0
            && self.round.time_left > 0
        {
            self.round.time_left -= 1;
        }

        let decided = self.round.finish != Finish::NotYet || self.round_end_decision(&roots);
        if decided {
            let rs4t = -cfg.over_waittime;
            self.round.intro -= 1;
            if self.round.intro == -cfg.over_hittime && !self.round.wins_counted {
                self.round.wins_counted = true;
                match self.round.finish {
                    Finish::KO | Finish::TimeOver if self.round.win_team >= 0 => {
                        let t = self.round.win_team as usize;
                        self.round.wins[t] += 1;
                        let kind = self.win_kind(&roots, t);
                        self.round.win_kinds[t].push(kind);
                        self.events.push(Event::RoundWon(t as u8));
                    }
                    _ => {
                        self.round.draws += 1;
                    }
                }
            }
            let roundnotover = roots
                .iter()
                .any(|&i| self.players[i].asserted(assert::ROUNDNOTOVER));
            if self.round.winposetime > 0 && self.round.intro == rs4t - 1 {
                for &i in &roots {
                    self.players[i].ctrl = false;
                }
            }
            if self.round.intro < rs4t {
                self.round.winposetime -= 1;
            }
            if self.round.winposetime <= 0 {
                for &i in &roots {
                    if self.round.over_flagged.get(i).copied().unwrap_or(false) {
                        continue;
                    }
                    let p = &self.players[i];
                    if !p.alive() || p.hit_pause_time > 0 {
                        continue;
                    }
                    if self.round.over_flagged.len() <= i {
                        self.round.over_flagged.resize(self.players.len(), false);
                    }
                    self.round.over_flagged[i] = true;
                    let team = p.team as i32;
                    let no = if self.round.win_team == team {
                        180
                    } else if self.round.win_team >= 0 {
                        170
                    } else {
                        175
                    };
                    let data = self.data[self.players[i].data].clone();
                    if data.states.contains_key(&no) {
                        self.self_state(i, no, None, Some(false));
                        self.run_current_pub(i);
                    }
                }
            }
            // a win pose keeps the round open while asserted
            let fadeout_start = rs4t - 2 - cfg.over_time;
            if roundnotover && self.round.intro == fadeout_start {
                self.round.intro += 1;
            }
            if self.round.round_over() && !self.round.match_over {
                self.end_round();
            }
        } else if self.round.intro < 0 {
            self.round.intro = 0;
        }
    }

    /// KO / time-over check. Sets `finish` and `win_team`.
    fn round_end_decision(&mut self, roots: &[usize]) -> bool {
        if self.round.intro > 0 {
            return false;
        }
        let mut ko = [true, true];
        for &i in roots {
            let p = &self.players[i];
            if p.alive() {
                ko[p.team as usize] = false;
            }
        }
        if self.round.time_left == 0 {
            let mut life = [0.0f32, 0.0];
            for &i in roots {
                let p = &self.players[i];
                life[p.team as usize] += p.life as f32 / p.life_max.max(1) as f32;
            }
            let (a, b) = (
                libm::roundf(life[0] * 1000.0),
                libm::roundf(life[1] * 1000.0),
            );
            if a != b {
                self.round.finish = Finish::TimeOver;
                self.round.win_team = if a > b { 0 } else { 1 };
                self.events.push(Event::TimeOver);
            } else {
                self.round.finish = Finish::TimeOverDraw;
                self.round.win_team = -1;
                self.events.push(Event::TimeOver);
            }
        }
        if self.round.intro >= -1 && (ko[0] || ko[1]) {
            if ko[0] && ko[1] {
                self.round.finish = Finish::DoubleKO;
                self.round.win_team = -1;
            } else {
                self.round.finish = Finish::KO;
                self.round.win_team = if ko[0] { 1 } else { 0 };
            }
            if self.round.intro >= 0 {
                self.events.push(Event::KO);
            }
        }
        self.round.finish != Finish::NotYet
    }

    fn end_round(&mut self) {
        let w = self.round.config.match_wins;
        let over = self.round.wins[0] >= w
            || self.round.wins[1] >= w
            || (self.round.config.max_drawgames >= 0
                && self.round.draws > self.round.config.max_drawgames);
        if over {
            self.round.match_over = true;
            let winner = if self.round.wins[0] >= w {
                0
            } else if self.round.wins[1] >= w {
                1
            } else {
                -1
            };
            self.events.push(Event::MatchOver(winner));
            return;
        }
        self.round.round_no += 1;
        self.start_round();
    }
}
