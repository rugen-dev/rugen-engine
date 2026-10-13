//! Trigger evaluation: implements `rugen_cns::Context` over the world for one
//! player (with redirection to parent/root/p2/...).

use rugen_cns::state::{MoveType, StateType};
use rugen_cns::trigger::{Axis, NameWhich, TeamMode, Trigger, flags};
use rugen_cns::{Context, Redirect, Value, VarKind};

use crate::hit::{HitType, attr};
use crate::projectile::ProjContact;
use crate::round::Finish;
use crate::world::World;

pub struct Ctx<'a> {
    pub w: &'a mut World,
    /// Player whose state controllers are running.
    pub owner: usize,
    /// Player triggers currently read from (changes under redirection).
    pub cur: usize,
    stack: Vec<usize>,
}

impl<'a> Ctx<'a> {
    pub fn new(w: &'a mut World, player: usize) -> Self {
        Self {
            w,
            owner: player,
            cur: player,
            stack: Vec::new(),
        }
    }

    fn p(&self) -> &crate::Player {
        &self.w.players[self.cur]
    }

    fn p2(&self) -> Option<usize> {
        self.w.nearest_enemy(self.cur)
    }

    fn state_type_mask(t: StateType) -> u32 {
        match t {
            StateType::S => flags::ST_S,
            StateType::C => flags::ST_C,
            StateType::A => flags::ST_A,
            StateType::L => flags::ST_L,
            StateType::U => flags::ST_U,
        }
    }
    fn move_type_mask(t: MoveType) -> u32 {
        match t {
            MoveType::I => flags::MT_I,
            MoveType::A => flags::MT_A,
            MoveType::H => flags::MT_H,
            MoveType::U => flags::MT_U,
        }
    }

    fn f(v: f32) -> Value {
        Value::Float(v)
    }
    fn i(v: i32) -> Value {
        Value::Int(v)
    }
    fn b(v: bool) -> Value {
        Value::bool(v)
    }
}

impl Context for Ctx<'_> {
    fn trigger(&mut self, t: &Trigger, args: &[Value]) -> Value {
        use Trigger as T;
        let p = self.p();
        let data = &self.w.data[p.data];
        let arg = |n: usize| args.get(n).copied().unwrap_or(Value::Bottom);
        match t {
            T::AILevel => Self::i(p.ai_level),
            T::GroundY => Self::f(p.ground),
            T::OnPlatform => Self::b(p.ground < 0.0),
            T::Alive => Self::b(p.life > 0),
            T::Anim => Self::i(p.anim.action),
            T::AnimTime => match data.air.get(p.anim.action) {
                Some(a) => Self::i(p.anim.anim_time(a)),
                None => Self::i(0),
            },
            T::AnimElemTime => match data
                .air
                .get(p.anim.action)
                .and_then(|a| p.anim.anim_elem_time(a, arg(0).to_i()))
            {
                Some(v) => Self::i(v),
                None => Value::Bottom,
            },
            T::AnimElemNo => match data
                .air
                .get(p.anim.action)
                .and_then(|a| p.anim.anim_elem_no(a, arg(0).to_i()))
            {
                Some(v) => Self::i(v),
                None => Value::Bottom,
            },
            T::AnimExist | T::SelfAnimExist => Self::b(data.air.get(arg(0).to_i()).is_some()),
            T::BackEdge
            | T::FrontEdge
            | T::LeftEdge
            | T::RightEdge
            | T::BackEdgeDist
            | T::BackEdgeBodyDist
            | T::FrontEdgeDist
            | T::FrontEdgeBodyDist => {
                // edges are the visible screen; the *Dist forms measure to where
                // the player is actually held (the furthest zoom-out view)
                let dist = matches!(
                    t,
                    T::BackEdgeDist | T::BackEdgeBodyDist | T::FrontEdgeDist | T::FrontEdgeBodyDist
                );
                let (xmin, xmax) = if dist {
                    self.w.play_bounds()
                } else {
                    self.w.screen_bounds()
                };
                // screen values are stage units: in this fighter's own units
                let s = self.w.local_scale(self.cur);
                let (xmin, xmax, cam) = (xmin / s, xmax / s, self.w.cam_x / s);
                let x = p.pos[0];
                let front = if p.facing > 0.0 { xmax } else { xmin };
                let back = if p.facing > 0.0 { xmin } else { xmax };
                let (wf, wb) = self.w.player_width(self.cur);
                match t {
                    T::LeftEdge => Self::f(xmin - cam),
                    T::RightEdge => Self::f(xmax - cam),
                    T::FrontEdge => Self::f(front - cam),
                    T::BackEdge => Self::f(back - cam),
                    T::FrontEdgeDist => Self::i(((front - x) * p.facing) as i32),
                    T::BackEdgeDist => Self::i(((x - back) * p.facing) as i32),
                    T::FrontEdgeBodyDist => Self::i(((front - x) * p.facing - wf) as i32),
                    _ => Self::i(((x - back) * p.facing - wb) as i32),
                }
            }
            T::BottomEdge => Self::f(self.w.stage.height / self.w.local_scale(self.cur)),
            T::TopEdge => Self::f(0.0),
            T::CameraPos(Axis::X) => Self::f(self.w.cam_x / self.w.local_scale(self.cur)),
            T::CameraPos(Axis::Y) => Self::f(0.0),
            T::CameraZoom => Self::f(1.0),
            T::CanRecover => Self::b(
                p.ghv.fallflag && p.ghv.fall_recover && p.ghv.fall_time >= p.ghv.fall_recovertime,
            ),
            T::Ctrl => Self::b(p.ctrl),
            T::Facing => Self::i(if p.facing < 0.0 { -1 } else { 1 }),
            T::GameHeight => Self::f(self.w.stage.height),
            T::GameWidth => Self::f(self.w.stage.width),
            T::GameTime => Self::i(self.w.frame as i32),
            T::HitCount => Self::i(p.hit_count),
            T::UniqHitCount => Self::i(p.uniq_hit_count),
            T::HitFall => Self::b(p.ghv.fallflag),
            T::HitOver => Self::b(p.ghv.hittime < 0),
            T::HitPauseTime => Self::i(p.hit_pause_time),
            T::HitShakeOver => Self::b(p.ghv.hitshaketime <= 0),
            T::HitVel(Axis::X) => Self::f(p.ghv.xvel),
            T::HitVel(Axis::Y) => Self::f(p.ghv.yvel),
            T::Id => Self::i(p.id),
            T::InGuardDist => Self::b(p.inguarddist),
            T::IsHomeTeam => Self::b(p.team == 0),
            T::Life => Self::i(p.life),
            T::LifeMax => Self::i(p.life_max),
            T::Win
            | T::WinKO
            | T::WinTime
            | T::WinPerfect
            | T::Lose
            | T::LoseKO
            | T::LoseTime
            | T::MatchOver
            | T::DrawGame => {
                let r = &self.w.round;
                let decided = r.finish != Finish::NotYet;
                let team = p.team as i32;
                let won = decided && r.win_team == team;
                let lost = decided && r.win_team >= 0 && r.win_team != team;
                Self::b(match t {
                    T::Win => won,
                    T::WinKO => won && r.finish == Finish::KO,
                    T::WinTime => won && r.finish == Finish::TimeOver,
                    T::WinPerfect => {
                        won && self
                            .w
                            .players
                            .iter()
                            .filter(|o| o.helper_index == 0 && o.team == p.team)
                            .all(|o| o.life >= o.life_max)
                    }
                    T::Lose => lost,
                    T::LoseKO => lost && r.finish == Finish::KO,
                    T::LoseTime => lost && r.finish == Finish::TimeOver,
                    T::MatchOver => r.match_over,
                    _ => decided && r.win_team < 0,
                })
            }
            T::MatchNo => Self::i(1),
            T::RoundNo => Self::i(self.w.round.round_no),
            T::MoveContact => Self::i(p.move_contact()),
            T::MoveGuarded => Self::i(p.move_guarded()),
            T::MoveHit => Self::i(p.move_hit()),
            T::MoveReversed => Self::i(0),
            T::NumEnemy => {
                Self::i(self.w.players.iter().filter(|o| o.team != p.team).count() as i32)
            }
            T::NumPartner => Self::i(0),
            T::NumProj => Self::i(self.w.num_proj(self.w.players[p.root].id, -1)),
            T::NumProjId => Self::i(self.w.num_proj(self.w.players[p.root].id, arg(0).to_i())),
            T::NumHelper => {
                let id = arg(0).to_i();
                let n = self
                    .w
                    .players
                    .iter()
                    .filter(|o| {
                        !o.removed
                            && o.helper_index > 0
                            && o.root == p.root
                            && (id <= 0 || o.helper_id == id)
                    })
                    .count();
                Self::i(n as i32)
            }
            T::NumExplod => Self::i(self.w.num_explod(p.id, arg(0).to_i())),
            T::NumTarget => {
                let id = arg(0).to_i();
                let n = p
                    .targets
                    .iter()
                    .filter(|&&tid| {
                        self.w
                            .players
                            .iter()
                            .any(|o| o.id == tid && (id < 0 || o.ghv.hitid == id))
                    })
                    .count();
                Self::i(n as i32)
            }
            T::P2BodyDist(a) | T::P2Dist(a) => match self.p2() {
                Some(e) => {
                    let o = &self.w.players[e];
                    match a {
                        Axis::X => {
                            let mut d = (o.pos[0] - p.pos[0]) * p.facing;
                            if matches!(t, T::P2BodyDist(_)) {
                                let (wf, _) = self.w.player_width(self.cur);
                                let (of, ob) = self.w.player_width(e);
                                let ow = if (o.facing > 0.0) != (p.facing > 0.0) {
                                    of
                                } else {
                                    ob
                                };
                                d -= wf + ow;
                            }
                            Self::f(d)
                        }
                        Axis::Y => Self::f(o.pos[1] - p.pos[1]),
                    }
                }
                None => Value::Bottom,
            },
            T::P2Life => self
                .p2()
                .map_or(Value::Bottom, |e| Self::i(self.w.players[e].life)),
            T::P2StateNo => self
                .p2()
                .map_or(Value::Bottom, |e| Self::i(self.w.players[e].state_no)),
            T::PalNo => Self::i(p.pal_no),
            T::ParentDist(a) | T::RootDist(a) => {
                let who = if matches!(t, T::ParentDist(_)) {
                    self.w.player_by_id(p.parent_id)
                } else {
                    Some(p.root)
                };
                match who {
                    Some(o) if p.helper_index > 0 => {
                        let o = &self.w.players[o];
                        Self::f(match a {
                            Axis::X => (o.pos[0] - p.pos[0]) * p.facing,
                            Axis::Y => o.pos[1] - p.pos[1],
                        })
                    }
                    _ => Value::Bottom,
                }
            }
            T::Pos(Axis::X) => Self::f(p.pos[0] - self.w.cam_x / self.w.local_scale(self.cur)),
            T::Pos(Axis::Y) => Self::f(p.pos[1] - p.ground),
            T::Power => Self::i(p.power),
            T::PowerMax => Self::i(p.power_max),
            T::PrevStateNo => Self::i(p.prev_state_no),
            T::Random => {
                let r = self.w.rng.random();
                Self::i(r)
            }
            T::RoundsExisted => Self::i(self.w.round.round_no - 1),
            T::RoundState => Self::i(self.w.round.round_state()),
            T::ScreenPos(Axis::X) => {
                Self::f(p.pos[0] - self.w.screen_left() / self.w.local_scale(self.cur))
            }
            T::ScreenPos(Axis::Y) => {
                Self::f(p.pos[1] + self.w.stage.zoffset / self.w.local_scale(self.cur))
            }
            T::ScreenHeight => Self::f(self.w.stage.height),
            T::ScreenWidth => Self::f(self.w.stage.width),
            T::StateNo => Self::i(p.state_no),
            T::TeamSide => Self::i(p.team as i32 + 1),
            T::TicksPerSecond => Self::i(60),
            T::Time => Self::i(p.state_time),
            T::Vel(Axis::X) => Self::f(p.vel[0]),
            T::Vel(Axis::Y) => Self::f(p.vel[1]),
            T::StateType => Self::i(Self::state_type_mask(p.state_type) as i32),
            T::MoveType => Self::i(Self::move_type_mask(p.move_type) as i32),
            T::P2StateType => self.p2().map_or(Value::Bottom, |e| {
                Self::i(Self::state_type_mask(self.w.players[e].state_type) as i32)
            }),
            T::P2MoveType => self.p2().map_or(Value::Bottom, |e| {
                Self::i(Self::move_type_mask(self.w.players[e].move_type) as i32)
            }),
            T::TeamModeRaw => Self::i(0),
            T::StateTypeIs { p2, mask } => {
                let who = if *p2 { self.p2() } else { Some(self.cur) };
                match who {
                    Some(i) => {
                        Self::b(Self::state_type_mask(self.w.players[i].state_type) & mask != 0)
                    }
                    None => Value::Bottom,
                }
            }
            T::MoveTypeIs { p2, mask } => {
                let who = if *p2 { self.p2() } else { Some(self.cur) };
                match who {
                    Some(i) => {
                        Self::b(Self::move_type_mask(self.w.players[i].move_type) & mask != 0)
                    }
                    None => Value::Bottom,
                }
            }
            T::TeamModeIs(m) => Self::b(*m == TeamMode::Single),
            T::HitDefAttrIs { st, at } => {
                // cns flags: ST bits match ours; attack-class bits are 1<<0.. there and 1<<6.. here
                let want = st | (at << 6);
                Self::b(
                    p.move_type == MoveType::A
                        && p.hitdef.as_ref().is_some_and(|h| {
                            let a = h.attr & want;
                            a & attr::ST_MASK != 0 && a & !attr::ST_MASK != 0
                        }),
                )
            }
            T::NameIs { which, text } => {
                let who = match which {
                    NameWhich::Name | NameWhich::P1Name | NameWhich::AuthorName => Some(self.cur),
                    NameWhich::P2Name => self.p2(),
                    _ => None,
                };
                match who {
                    Some(i) => {
                        let d = &self.w.data[self.w.players[i].data];
                        let s = if *which == NameWhich::AuthorName {
                            &d.author
                        } else {
                            &d.name
                        };
                        Self::b(s.eq_ignore_ascii_case(text))
                    }
                    None => Self::b(false),
                }
            }
            T::StageVarIs { name, text } => Self::b(match name.as_str() {
                "info.name" => self.w.stage.name.eq_ignore_ascii_case(text),
                "info.displayname" => self.w.stage.display_name.eq_ignore_ascii_case(text),
                "info.author" => self.w.stage.author.eq_ignore_ascii_case(text),
                _ => false,
            }),
            // helpers without keyctrl read no command input (MUGEN Helper doc, Ikemen `command`)
            T::Command(name) => Self::b((p.helper_index == 0 || p.keyctrl) && p.cmds.active(name)),
            T::Const(name) => match p.size.konst(name).or_else(|| data.konst(name)) {
                Some(v) => {
                    // ints stay ints so `const(data.life)` compares like MUGEN
                    if name.starts_with("data.") && name != "data.fall.defence_mul"
                        || name.starts_with("size.") && !name.contains("scale")
                    {
                        Self::i(v as i32)
                    } else {
                        Self::f(v)
                    }
                }
                None => {
                    log::debug!("unknown const {name}");
                    Value::Bottom
                }
            },
            T::GetHitVar(name) => {
                let g = &p.ghv;
                let ht = |t: HitType| Self::i(t as i32);
                match name.as_str() {
                    "xveladd" => Self::f(g.xveladd),
                    "yveladd" => Self::f(g.yveladd),
                    "type" => ht(g.hit_type),
                    "animtype" => Self::i(g.animtype as i32),
                    "airtype" => ht(g.airtype),
                    "groundtype" => ht(g.groundtype),
                    "damage" => Self::i(g.damage),
                    "hitcount" => Self::i(g.hitcount),
                    "guardcount" => Self::i(g.guardcount),
                    "fallcount" => Self::i(g.fallcount),
                    "hitshaketime" => Self::i(g.hitshaketime),
                    "hittime" => Self::i(g.hittime),
                    "slidetime" => Self::i(g.slidetime),
                    "ctrltime" => Self::i(g.ctrltime),
                    "recovertime" => Self::i(g.down_recovertime),
                    "xoff" => Self::f(g.xoff),
                    "yoff" => Self::f(g.yoff),
                    "xvel" => Self::f(g.xvel),
                    "yvel" => Self::f(g.yvel),
                    "yaccel" => Self::f(g.yaccel),
                    "hitid" | "chainid" => Self::i(g.chain_id()),
                    "guarded" => Self::b(g.guarded),
                    "isbound" => Self::b(p.bind.is_some()),
                    "fall" => Self::b(g.fallflag),
                    "fall.damage" => Self::i(g.fall_damage),
                    "fall.xvel" => Self::f(g.fall_xvelocity.unwrap_or(0.0)),
                    "fall.yvel" => Self::f(g.fall_yvelocity),
                    "fall.recover" => Self::b(g.fall_recover),
                    "fall.time" => Self::i(g.fall_time),
                    "fall.recovertime" => Self::i(g.fall_recovertime),
                    "fall.kill" => Self::b(g.fall_kill),
                    "fall.envshake.time" => Self::i(g.fall_envshake_time),
                    "fall.envshake.freq" => Self::f(g.fall_envshake_freq),
                    "fall.envshake.ampl" => Self::i(g.fall_envshake_ampl),
                    "fall.envshake.phase" => Self::f(g.fall_envshake_phase.unwrap_or(0.0)),
                    "playerno" => Self::i(
                        self.w
                            .players
                            .iter()
                            .position(|o| o.id == g.playerid)
                            .map_or(0, |i| i as i32 + 1),
                    ),
                    "id" | "playerid" => Self::i(g.playerid),
                    "attr" => Self::i(g.attr as i32),
                    _ => {
                        log::debug!("unknown gethitvar {name}");
                        Value::Bottom
                    }
                }
            }
            T::PlayerIdExist => Self::b(self.w.players.iter().any(|o| o.id == arg(0).to_i())),
            T::ProjHitTime | T::ProjGuardedTime | T::ProjContactTime | T::ProjCancelTime => {
                let id = arg(0).to_i();
                let o = &self.w.players[p.root];
                let kind_ok = match t {
                    T::ProjHitTime => o.pctype == ProjContact::Hit,
                    T::ProjGuardedTime => o.pctype == ProjContact::Guarded,
                    T::ProjContactTime => {
                        matches!(o.pctype, ProjContact::Hit | ProjContact::Guarded)
                    }
                    _ => o.pctype == ProjContact::Cancel,
                };
                if kind_ok && o.pctime >= 0 && (id <= 0 || o.pcid == id) {
                    Self::i(o.pctime)
                } else {
                    Self::i(-1)
                }
            }
            T::IsHelper => {
                let id = arg(0).to_i();
                Self::b(p.helper_index > 0 && (id == i32::MIN || id <= 0 || p.helper_id == id))
            }
            T::ConstCoord(h) => Self::f(data.localcoord[1] / *h as f32 * arg(0).to_f()),
        }
    }

    fn get_var(&mut self, kind: VarKind, index: i32) -> Value {
        let p = self.p();
        let i = index as usize;
        match kind {
            VarKind::Var if index >= 0 => p.vars.get(i).map_or(Value::Bottom, |&v| Value::Int(v)),
            VarKind::FVar if index >= 0 => {
                p.fvars.get(i).map_or(Value::Bottom, |&v| Value::Float(v))
            }
            VarKind::SysVar if index >= 0 => {
                p.sysvars.get(i).map_or(Value::Bottom, |&v| Value::Int(v))
            }
            VarKind::SysFVar if index >= 0 => p
                .sysfvars
                .get(i)
                .map_or(Value::Bottom, |&v| Value::Float(v)),
            _ => Value::Bottom,
        }
    }

    fn set_var(&mut self, kind: VarKind, index: i32, value: Value) -> Value {
        if index < 0 {
            return Value::Bottom;
        }
        let i = index as usize;
        // assignment always targets the owner of the running state, never a redirect target
        let p = &mut self.w.players[self.owner];
        match kind {
            VarKind::Var => {
                if let Some(v) = p.vars.get_mut(i) {
                    *v = value.to_i();
                    return Value::Int(*v);
                }
            }
            VarKind::FVar => {
                if let Some(v) = p.fvars.get_mut(i) {
                    *v = value.to_f();
                    return Value::Float(*v);
                }
            }
            VarKind::SysVar => {
                if let Some(v) = p.sysvars.get_mut(i) {
                    *v = value.to_i();
                    return Value::Int(*v);
                }
            }
            VarKind::SysFVar => {
                if let Some(v) = p.sysfvars.get_mut(i) {
                    *v = value.to_f();
                    return Value::Float(*v);
                }
            }
        }
        Value::Bottom
    }

    fn redirect(&mut self, kind: Redirect, id: Option<i32>) -> bool {
        let target = match kind {
            Redirect::P2 | Redirect::Enemy | Redirect::EnemyNear => match id {
                None | Some(0) => self.p2(),
                Some(_) => None,
            },
            Redirect::PlayerId => id.and_then(|id| self.w.players.iter().position(|o| o.id == id)),
            Redirect::Target => {
                let p = self.p();
                let want = id.unwrap_or(-1);
                p.targets.iter().find_map(|&tid| {
                    self.w.players.iter().position(|o| {
                        !o.removed && o.id == tid && (want < 0 || o.ghv.hitid == want)
                    })
                })
            }
            Redirect::Root => {
                let p = self.p();
                if p.helper_index > 0 {
                    Some(p.root)
                } else {
                    None
                }
            }
            Redirect::Parent => {
                let p = self.p();
                if p.helper_index > 0 {
                    self.w.player_by_id(p.parent_id)
                } else {
                    None
                }
            }
            Redirect::Helper => {
                let p = self.p();
                let want = id.unwrap_or(0);
                self.w.players.iter().position(|o| {
                    !o.removed
                        && o.helper_index > 0
                        && o.root == p.root
                        && (want <= 0 || o.helper_id == want)
                })
            }
            Redirect::Partner => None,
        };
        match target {
            Some(t) => {
                self.stack.push(self.cur);
                self.cur = t;
                true
            }
            None => false,
        }
    }

    fn end_redirect(&mut self) {
        if let Some(c) = self.stack.pop() {
            self.cur = c;
        }
    }
}
