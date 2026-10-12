//! The match: players, stage bounds, camera, and the per-tick update in
//! MUGEN's order (commands → prepare → run → bounds → animate).

use std::sync::Arc;

use rugen_cns::state::{MoveType, Physics, StateType};
use rugen_cns::{Code, Context, Value, VarKind};

use crate::Rng;
use crate::anim::AnimState;
use crate::chardata::{CharData, Ctrl, FxData};
use crate::collision;
use crate::command::CommandList;
use crate::context::Ctx;
use crate::explod::{AnimSrc, Explod, PosType, Space};
use crate::fx::{AfterImage, AfterImg, EnvShake, PalFx};
use crate::hit::{HitType, MoveContact};
use crate::input::Buttons;
use crate::player::{Bind, HitBySlot, HitOverride, Player, assert};
use crate::projectile::{ProjStatus, Projectile, ProjectileSpec};
use crate::round::{RoundConfig, RoundState};
use crate::sctrl::{ExplodSpec, PalFxSpec, Sctrl};

/// Stage geometry in the stage's own coordinate space (320 wide by default).
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub name: String,
    pub display_name: String,
    pub author: String,
    pub width: f32,
    pub height: f32,
    /// Ground y measured from the top of the screen.
    pub zoffset: f32,
    /// Camera limits.
    pub bound_left: f32,
    pub bound_right: f32,
    /// Players can't go past these stage coordinates.
    pub left_bound: f32,
    pub right_bound: f32,
    /// Distance from the screen edge players can't cross.
    pub screen_left: f32,
    pub screen_right: f32,
    pub p1_start_x: f32,
    pub p2_start_x: f32,
    pub tension: f32,
    pub start_x: f32,
    pub bound_high: f32,
    pub bound_low: f32,
    pub vertical_follow: f32,
    pub floor_tension: f32,
    /// Zoom limits: `zoom_out` ≤ 1 lets the camera pull back, `zoom_in` ≥ 1 lets it close in.
    pub zoom_out: f32,
    pub zoom_in: f32,
    /// rugen extension: raised ground segments players can stand on.
    pub platforms: Vec<Platform>,
    /// rugen extension: Smash-style arena rules (`[Arena]` section).
    pub arena: Arena,
}

/// `[Arena]` section of a stage .def (all optional):
/// `blastzone = left, top, right, bottom` KOs players that cross it,
/// `walls = 0` lets players leave the screen edges, `stocks = N` gives lives
/// per round (respawn at `spawn_y`), `floor = x1, x2` limits the ground to a
/// span (outside it players fall), and `air_jumps` / `air_dash` /
/// `fast_fall` override the `[game]` movement options for this stage.
#[derive(Clone, Debug, PartialEq)]
pub struct Arena {
    pub blastzone: Option<[f32; 4]>,
    pub walls: bool,
    pub stocks: i32,
    pub floor: Option<[f32; 2]>,
    pub spawn_y: f32,
    pub air_jumps: Option<i32>,
    pub air_dash: Option<bool>,
    pub fast_fall: Option<bool>,
}

impl Default for Arena {
    fn default() -> Self {
        Self {
            blastzone: None,
            walls: true,
            stocks: 0,
            floor: None,
            spawn_y: -120.0,
            air_jumps: None,
            air_dash: None,
            fast_fall: None,
        }
    }
}

/// Engine movement options for legacy characters (no .cns changes needed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Movement {
    /// Extra air jumps on top of `movement.airjump.num`.
    pub air_jumps: i32,
    /// Double-tap forward/back in the air for a short dash.
    pub air_dash: bool,
    /// Hold down while falling to fall faster.
    pub fast_fall: bool,
}

/// Ground value for a player over a pit: far below everything.
pub const NO_GROUND: f32 = 1.0e6;

/// A horizontal surface at height `y` (negative = above the floor) spanning `x1..=x2`
/// in stage coordinates. Declared in a stage .def as
/// `[Platform]` sections with `x = x1, x2` and `y = height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Platform {
    pub x1: f32,
    pub x2: f32,
    pub y: f32,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            name: "Training Room".into(),
            display_name: "Training Room".into(),
            author: String::new(),
            width: 320.0,
            height: 240.0,
            zoffset: 210.0,
            bound_left: -150.0,
            bound_right: 150.0,
            left_bound: -1000.0,
            right_bound: 1000.0,
            screen_left: 15.0,
            screen_right: 15.0,
            p1_start_x: -70.0,
            p2_start_x: 70.0,
            tension: 50.0,
            start_x: 0.0,
            bound_high: -25.0,
            bound_low: 0.0,
            vertical_follow: 0.2,
            floor_tension: 0.0,
            zoom_out: 1.0,
            zoom_in: 1.0,
            platforms: Vec::new(),
            arena: Arena::default(),
        }
    }
}

impl Stage {
    pub fn from_def(def: &rugen_formats::def::Def) -> Self {
        let mut s = Stage::default();
        let g = |sec: &str, key: &str| def.section(sec).and_then(|s| s.f32(key));
        if let Some(i) = def.section("info") {
            s.name = i.str("name").unwrap_or("").to_string();
            s.display_name = i.str("displayname").unwrap_or(&s.name).to_string();
            s.author = i.str("author").unwrap_or("").to_string();
        }
        if let Some(lc) = def
            .section("stageinfo")
            .and_then(|s| s.f32s("localcoord"))
            .filter(|v| v.len() >= 2)
        {
            s.width = lc[0];
            s.height = lc[1];
        }
        if let Some(v) = g("stageinfo", "zoffset") {
            s.zoffset = v;
        }
        if let Some(v) = g("camera", "boundleft") {
            s.bound_left = v;
        }
        if let Some(v) = g("camera", "boundright") {
            s.bound_right = v;
        }
        if let Some(v) = g("camera", "tension") {
            s.tension = v;
        }
        if let Some(v) = g("camera", "startx") {
            s.start_x = v;
        }
        if let Some(v) = g("camera", "boundhigh") {
            s.bound_high = v;
        }
        if let Some(v) = g("camera", "boundlow") {
            s.bound_low = v;
        }
        if let Some(v) = g("camera", "verticalfollow") {
            s.vertical_follow = v.clamp(0.0, 1.0);
        }
        if let Some(v) = g("camera", "floortension") {
            s.floor_tension = v;
        }
        if let Some(v) = g("camera", "zoomout") {
            s.zoom_out = v.clamp(0.1, 1.0);
        }
        if let Some(v) = g("camera", "zoomin") {
            s.zoom_in = v.max(s.zoom_out);
        }
        if let Some(a) = def.section("arena") {
            if let Some(v) = a.f32s("blastzone").filter(|v| v.len() >= 4) {
                s.arena.blastzone = Some([
                    v[0].min(v[2]),
                    v[1].min(v[3]),
                    v[0].max(v[2]),
                    v[1].max(v[3]),
                ]);
            }
            if let Some(v) = a.i32("walls") {
                s.arena.walls = v != 0;
            }
            if let Some(v) = a.i32("stocks") {
                s.arena.stocks = v.max(0);
            }
            if let Some(v) = a.f32s("floor").filter(|v| v.len() >= 2) {
                s.arena.floor = Some([v[0].min(v[1]), v[0].max(v[1])]);
            }
            if let Some(v) = a.f32("spawn_y") {
                s.arena.spawn_y = v.min(0.0);
            }
            if let Some(v) = a.i32("air_jumps") {
                s.arena.air_jumps = Some(v.max(0));
            }
            if let Some(v) = a.i32("air_dash") {
                s.arena.air_dash = Some(v != 0);
            }
            if let Some(v) = a.i32("fast_fall") {
                s.arena.fast_fall = Some(v != 0);
            }
        }
        for sec in def.sections_named("platform") {
            let x = sec.f32s("x").unwrap_or_default();
            let (Some(&x1), Some(&x2), Some(y)) = (x.first(), x.get(1), sec.f32("y")) else {
                continue;
            };
            s.platforms.push(Platform {
                x1: x1.min(x2),
                x2: x1.max(x2),
                y: y.min(0.0),
            });
        }
        if let Some(v) = g("playerinfo", "leftbound") {
            s.left_bound = v;
        }
        if let Some(v) = g("playerinfo", "rightbound") {
            s.right_bound = v;
        }
        if let Some(v) = g("playerinfo", "p1startx") {
            s.p1_start_x = v;
        }
        if let Some(v) = g("playerinfo", "p2startx") {
            s.p2_start_x = v;
        }
        if let Some(v) = g("bound", "screenleft") {
            s.screen_left = v;
        }
        if let Some(v) = g("bound", "screenright") {
            s.screen_right = v;
        }
        s
    }
}

/// Side effects the renderer/audio pick up after a step.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Sound {
        player: usize,
        own: bool,
        group: i32,
        number: i32,
        channel: i32,
        volume: i32,
    },
    /// Stop sounds on `channel` of `player` (-1 = all channels).
    StopSound {
        player: usize,
        channel: i32,
    },
    RoundStart(i32),
    Fight,
    KO,
    TimeOver,
    RoundWon(u8),
    /// Winning team or -1 for a draw.
    MatchOver(i32),
    /// Hit/guard spark at a stage position. `own` = from the player's AIR, else fightfx.
    Spark {
        player: usize,
        own: bool,
        anim: i32,
        pos: [f32; 2],
        facing: f32,
    },
}

/// Hard caps that keep one character's content from flooding the simulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Helpers alive per root player (MUGEN: 56).
    pub helpers: usize,
    pub explods: usize,
    pub projectiles: usize,
    /// PlaySnd events accepted per tick.
    pub sounds_per_tick: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            helpers: 56,
            explods: 512,
            projectiles: 128,
            sounds_per_tick: 32,
        }
    }
}

#[derive(Clone)]
pub struct World {
    pub data: Vec<Arc<CharData>>,
    pub fx: Option<Arc<FxData>>,
    pub players: Vec<Player>,
    pub explods: Vec<Explod>,
    pub projs: Vec<Projectile>,
    pub stage: Stage,
    pub frame: u32,
    pub rng: Rng,
    pub cam_x: f32,
    /// Vertical camera offset (negative = up), after verticalfollow.
    pub cam_y: f32,
    /// Camera zoom: 1 = native, < 1 pulled back, > 1 closed in.
    pub cam_zoom: f32,
    /// Pause / SuperPause ticks remaining (buffered: start next tick).
    pub pausetime: i32,
    pub supertime: i32,
    pub pausetime_buffer: i32,
    pub supertime_buffer: i32,
    pub pause_player: i32,
    pub super_player: i32,
    /// 0..1 screen brightness during super pause.
    pub superbrightness: f32,
    pub envshake: EnvShake,
    /// AllPalFX: applied on top of every player's own PalFX.
    pub allpalfx: PalFx,
    /// BGPalFX: applied to the stage.
    pub bgpalfx: PalFx,
    /// Engine movement options (config merged with the stage's `[Arena]`).
    pub movement: Movement,
    pub round: RoundState,
    pub events: Vec<Event>,
    pub warnings: Vec<String>,
    pub limits: Limits,
    /// Player index whose states are being run right now (`step_checked` blames it on panic).
    pub running: Option<usize>,
    /// Test hook: panic when this player's states run (simulates a content crash).
    #[cfg(test)]
    pub panic_on_run: Option<usize>,
}

const MAX_STATE_LOOP: usize = 2500;

impl World {
    pub fn new(stage: Stage, seed: u64) -> Self {
        Self {
            data: Vec::new(),
            fx: None,
            players: Vec::new(),
            explods: Vec::new(),
            projs: Vec::new(),
            stage,
            frame: 0,
            rng: Rng::new(seed),
            cam_x: 0.0,
            cam_y: 0.0,
            cam_zoom: 1.0,
            pausetime: 0,
            supertime: 0,
            pausetime_buffer: 0,
            supertime_buffer: 0,
            pause_player: -1,
            super_player: -1,
            superbrightness: 1.0,
            envshake: EnvShake::default(),
            allpalfx: PalFx::default(),
            bgpalfx: PalFx::default(),
            movement: Movement::default(),
            round: RoundState::new(RoundConfig::default()),
            events: Vec::new(),
            warnings: Vec::new(),
            limits: Limits::default(),
            running: None,
            #[cfg(test)]
            panic_on_run: None,
        }
    }

    /// Stage units per player-local unit.
    pub fn local_scale(&self, i: usize) -> f32 {
        self.stage.width / self.data[self.players[i].data].localcoord[0]
    }

    /// Does `i` act this tick, given a running Pause / SuperPause?
    fn compute_pause_bool(&self, i: usize) -> bool {
        let p = &self.players[i];
        if self.supertime > 0 {
            p.super_move_time == 0
        } else if self.pausetime > 0 {
            p.pause_move_time == 0
        } else {
            false
        }
    }

    /// Add a player of `data` on `team` (0 = left, 1 = right).
    pub fn add_player(&mut self, data: Arc<CharData>, team: u8, pal_no: i32) -> usize {
        let idx = self.players.len();
        let life = data.konst("data.life").unwrap_or(1000.0) as i32;
        let power_max = data.konst("data.power").unwrap_or(3000.0) as i32;
        let (x, facing) = if team == 0 {
            (self.stage.p1_start_x, 1.0)
        } else {
            (self.stage.p2_start_x, -1.0)
        };
        let x = x * data.localcoord[0] / self.stage.width;
        self.data.push(data.clone());
        let mut p = Player::new(
            self.data.len() - 1,
            team,
            idx as i32 + 1,
            life,
            power_max,
            x,
            facing,
            pal_no,
        );
        p.cmds = CommandList::compile(&data.cmd);
        // every root player is its own root (helpers inherit it)
        p.root = idx;
        self.players.push(p);
        // tests and tools can fight right away; `start_round` does the real intro sequence
        self.change_state(idx, 5900, None, None);
        self.apply_pending(idx);
        self.change_state(idx, 0, None, Some(true));
        self.apply_pending(idx);
        self.round.intro = 0;
        idx
    }

    pub fn nearest_enemy(&self, me: usize) -> Option<usize> {
        let p = &self.players[me];
        self.players
            .iter()
            .enumerate()
            .filter(|(i, o)| *i != me && !o.removed && o.player_type && o.team != p.team)
            .min_by(|(_, a), (_, b)| {
                let da = (a.pos[0] - p.pos[0]).abs();
                let db = (b.pos[0] - p.pos[0]).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    /// (front, back) widths in effect this tick.
    pub fn player_width(&self, i: usize) -> (f32, f32) {
        let p = &self.players[i];
        let d = &self.data[p.data];
        let (f, b) = if p.state_type == StateType::A {
            (
                d.konst("size.air.front").unwrap_or(12.0),
                d.konst("size.air.back").unwrap_or(12.0),
            )
        } else {
            (
                d.konst("size.ground.front").unwrap_or(16.0),
                d.konst("size.ground.back").unwrap_or(15.0),
            )
        };
        (f + p.width_player[0], b + p.width_player[1])
    }

    pub fn screen_left(&self) -> f32 {
        self.cam_x - self.stage.width / (2.0 * self.cam_zoom)
    }
    pub fn screen_right(&self) -> f32 {
        self.cam_x + self.stage.width / (2.0 * self.cam_zoom)
    }
    /// Stage x range players may occupy on screen.
    pub fn screen_bounds(&self) -> (f32, f32) {
        (
            self.screen_left() + self.stage.screen_left,
            self.screen_right() - self.stage.screen_right,
        )
    }

    /// Stage x range players are held inside: the view at the furthest zoom
    /// out the stage allows, around the camera. A hard hit sends the target
    /// flying while the camera pulls back instead of bouncing it off the
    /// current screen edge; stages without zoom keep MUGEN's screen bounds.
    pub fn play_bounds(&self) -> (f32, f32) {
        let zoom = self.stage.zoom_out.min(self.cam_zoom).max(0.05);
        let half = self.stage.width / (2.0 * zoom);
        (
            self.cam_x - half + self.stage.screen_left,
            self.cam_x + half - self.stage.screen_right,
        )
    }

    // ------------------------------------------------------------ state changes

    pub(crate) fn eval(&mut self, i: usize, code: &Code) -> Value {
        let mut ctx = Ctx::new(self, i);
        code.eval(&mut ctx)
    }

    /// `ChangeState`: buffer the change; it applies before the next state body runs
    /// (immediately when called from the current state body).
    pub fn change_state(&mut self, i: usize, no: i32, anim: Option<i32>, ctrl: Option<bool>) {
        if let Some(a) = anim
            && a != -1
        {
            self.change_anim(i, a, None);
        }
        let p = &mut self.players[i];
        if let Some(c) = ctrl {
            p.ctrl = c;
        }
        p.prev_state_no = p.state_no;
        p.state_no = no.max(0);
        p.state_time = 0;
        let n = self.data[p.state_owner]
            .states
            .get(&p.state_no)
            .map_or(0, |s| s.ctrls.len());
        p.persist = vec![0; n];
        p.pending_state = Some(p.state_no);
    }

    /// `SelfState`: back to the player's own states.
    pub fn self_state(&mut self, i: usize, no: i32, anim: Option<i32>, ctrl: Option<bool>) {
        let own = self.players[i].data;
        self.players[i].state_owner = own;
        self.change_state(i, no, anim, ctrl);
    }

    /// `stateChange2`: apply the statedef of the buffered state.
    fn apply_pending(&mut self, i: usize) -> bool {
        let Some(no) = self.players[i].pending_state.take() else {
            return false;
        };
        let owner_id = self.players[i].id;
        for e in &mut self.explods {
            if e.owner_id == owner_id && e.removeonchangestate {
                e.removed = true;
            }
        }
        let data = self.data[self.players[i].state_owner].clone();
        let Some(state) = data.states.get(&no) else {
            self.warn(format!("P{} changed to invalid state {no}", i + 1));
            let p = &mut self.players[i];
            p.state_type = StateType::U;
            return true;
        };
        let def = &state.def;
        {
            let p = &mut self.players[i];
            if def.state_type != StateType::U {
                p.state_type = def.state_type;
            }
            if def.move_type != MoveType::U {
                p.move_type = def.move_type;
            }
            if def.physics != Physics::U {
                p.physics = def.physics;
            }
            if p.move_type != MoveType::A {
                p.juggle = 0;
            }
        }
        if let Some(c) = &def.sprpriority {
            let v = self.eval(i, c).to_i();
            self.players[i].spr_priority = v;
        }
        if let Some(c) = &def.facep2
            && self.eval(i, c).to_b()
            && self.should_face_p2(i)
        {
            let f = -self.players[i].facing;
            self.players[i].set_facing(f);
        }
        if let Some(c) = &def.juggle {
            let v = self.eval(i, c).to_i();
            self.players[i].juggle = v;
        }
        if let Some(vs) = &def.velset {
            if let Some(x) = vs.first() {
                let v = self.eval(i, x).to_f();
                self.players[i].vel[0] = v;
            }
            if let Some(y) = vs.get(1) {
                let v = self.eval(i, y).to_f();
                self.players[i].vel[1] = v;
            }
        }
        if let Some(c) = &def.anim {
            let a = self.eval(i, c).to_i();
            if a != -1 {
                self.change_anim(i, a, None);
            }
        }
        if def
            .hitdefpersist
            .as_ref()
            .is_none_or(|c| !self.eval(i, c).to_b())
        {
            self.players[i].hitdef = None;
        }
        if def
            .movehitpersist
            .as_ref()
            .is_none_or(|c| !self.eval(i, c).to_b())
        {
            self.players[i].mctype = MoveContact::None;
            self.players[i].mctime = 0;
        }
        if def
            .hitcountpersist
            .as_ref()
            .is_none_or(|c| !self.eval(i, c).to_b())
        {
            self.players[i].hit_count = 0;
            self.players[i].guard_count = 0;
        }
        if let Some(c) = &def.ctrl {
            let v = self.eval(i, c).to_b();
            self.players[i].ctrl = v;
        }
        if let Some(c) = &def.poweradd {
            let v = self.eval(i, c).to_i();
            self.power_add(i, v);
        }
        true
    }

    pub fn change_anim(&mut self, i: usize, no: i32, elem: Option<(i32, i32)>) {
        let own = self.players[i].data;
        self.change_anim_from(i, own, no, elem);
    }

    /// `ChangeAnim2`: animation from the state owner's AIR.
    pub fn change_anim2(&mut self, i: usize, no: i32, elem: Option<(i32, i32)>) {
        let owner = self.players[i].state_owner;
        self.change_anim_from(i, owner, no, elem);
    }

    fn change_anim_from(&mut self, i: usize, owner: usize, mut no: i32, elem: Option<(i32, i32)>) {
        if no < 0 {
            self.warn(format!("P{} attempted change to negative anim {no}", i + 1));
            no = 0;
        }
        let data = self.data[owner].clone();
        let Some(action) = data.air.get(no) else {
            self.warn(format!(
                "P{} attempted to change to invalid action {no}",
                i + 1
            ));
            return;
        };
        let p = &mut self.players[i];
        p.prev_anim_no = p.anim.action;
        p.anim = AnimState::start(no);
        p.anim_owner = owner;
        if let Some((e, t)) = elem {
            p.anim.set_elem(action, e, t);
        }
    }

    fn power_add(&mut self, i: usize, v: i32) {
        let p = &mut self.players[i];
        p.power = p.power.saturating_add(v).clamp(0, p.power_max);
    }

    /// Indices of `i`'s targets, optionally filtered by hit id.
    fn target_indices(&self, i: usize, hit_id: i32) -> Vec<usize> {
        let p = &self.players[i];
        p.targets
            .iter()
            .filter_map(|&tid| self.players.iter().position(|o| o.id == tid))
            .filter(|&t| hit_id < 0 || self.players[t].ghv.hitid == hit_id)
            .collect()
    }

    fn apply_bind(&mut self, i: usize) {
        let Some(b) = self.players[i].bind else {
            return;
        };
        let Some(to) = self.players.iter().position(|o| o.id == b.to) else {
            self.players[i].bind = None;
            return;
        };
        let (tx, ty, tf) = (
            self.players[to].pos[0],
            self.players[to].pos[1],
            self.players[to].facing,
        );
        let p = &mut self.players[i];
        p.pos[0] = tx + b.pos[0] * tf;
        p.pos[1] = ty + b.pos[1];
        p.old_pos = p.pos;
        if b.facing != 0.0 {
            p.set_facing(b.facing);
        }
    }

    /// Spawn a fightfx explod at a position relative to player `i` (GameMakeAnim / MakeDust).
    fn spawn_fx_anim(&mut self, i: usize, anim: i32, rel: [f32; 2], ontop: bool) {
        let owner_id = self.players[i].id;
        let mut e = Explod::new(i, owner_id, AnimSrc::Fx, anim);
        if e.air(self).and_then(|a| a.get(anim)).is_none() {
            return;
        }
        let ls = self.local_scale(i);
        e.id = 0;
        e.rel = [rel[0] * ls, rel[1] * ls];
        e.ontop = ontop;
        e.sprpriority = i32::MIN;
        e.facing = self.players[i].facing;
        self.explod_set_pos(&mut e, i);
        self.explods.push(e);
    }

    /// Write a variable on `target` directly (ParentVarSet / ParentVarAdd).
    fn write_var(&mut self, target: usize, kind: VarKind, idx: i32, v: Value, add: bool) {
        if idx < 0 {
            return;
        }
        let p = &mut self.players[target];
        let k = idx as usize;
        match kind {
            VarKind::Var | VarKind::SysVar => {
                let slot = if kind == VarKind::Var {
                    p.vars.get_mut(k)
                } else {
                    p.sysvars.get_mut(k)
                };
                if let Some(s) = slot {
                    *s = if add { *s + v.to_i() } else { v.to_i() };
                }
            }
            VarKind::FVar | VarKind::SysFVar => {
                let slot = if kind == VarKind::FVar {
                    p.fvars.get_mut(k)
                } else {
                    p.sysfvars.get_mut(k)
                };
                if let Some(s) = slot {
                    *s = if add { *s + v.to_f() } else { v.to_f() };
                }
            }
        }
    }

    fn should_face_p2(&self, i: usize) -> bool {
        let Some(e) = self.nearest_enemy(i) else {
            return false;
        };
        let (p, o) = (&self.players[i], &self.players[e]);
        (o.pos[0] - p.pos[0]) * p.facing < 0.0
    }

    fn auto_turn(&mut self, i: usize) {
        if self.players[i].asserted(assert::NOAUTOTURN) || !self.should_face_p2(i) {
            return;
        }
        let p = &self.players[i];
        let turn_anim = match p.state_type {
            StateType::S => Some(5),
            StateType::C => Some(6),
            _ => None,
        };
        if let Some(a) = turn_anim
            && p.anim.action != a
        {
            self.change_anim(i, a, None);
        }
        let f = -self.players[i].facing;
        self.players[i].set_facing(f);
    }

    fn warn(&mut self, msg: String) {
        if self.warnings.len() < 200 {
            self.warnings.push(msg);
        }
    }

    // ------------------------------------------------------------ frame

    /// Advance one tick. `inputs[i]` drives player `i` (missing = no input).
    pub fn step(&mut self, inputs: &[Buttons]) {
        self.events.clear();
        let n = self.players.len();

        // 0. pause timers (a pause requested last tick starts now)
        if self.supertime > 0 {
            self.supertime -= 1;
        } else if self.pausetime > 0 {
            self.pausetime -= 1;
        }
        if self.supertime_buffer < 0 {
            self.supertime = !self.supertime_buffer;
            self.supertime_buffer = 0;
        }
        if self.pausetime_buffer < 0 {
            self.pausetime = !self.pausetime_buffer;
            self.pausetime_buffer = 0;
        }
        if self.supertime <= 0 && self.pausetime <= 0 {
            self.superbrightness = 1.0;
        }
        for i in 0..n {
            let pb = self.compute_pause_bool(i);
            self.players[i].pause_bool = pb;
        }
        self.step_round();
        if self.round.match_over {
            self.frame += 1;
            return;
        }

        // 1. commands
        let mut i = 0;
        while i < self.players.len() {
            if self.players[i].removed {
                i += 1;
                continue;
            }
            let p = &self.players[i];
            if p.helper_index == 0 && p.ctrl && !p.pause_bool && matches!(p.state_no, 0 | 11 | 20) {
                self.auto_turn(i);
            }
            let p = &self.players[i];
            if p.helper_index > 0 {
                // helpers with keyctrl read their root's buffer and commands
                if p.keyctrl {
                    let root = p.root;
                    let (input, cmds) = (
                        self.players[root].input.clone(),
                        self.players[root].cmds.clone(),
                    );
                    self.players[i].input = input;
                    self.players[i].cmds = cmds;
                }
                i += 1;
                continue;
            }
            let b = if self.players[i].ai_level > 0 {
                let level = self.players[i].ai_level;
                let active = self.round.round_state() == 2;
                let mut ai = self.players[i].ai;
                let b = ai.update(level, active, &mut self.rng);
                self.players[i].ai = ai;
                b
            } else {
                inputs.get(i).copied().unwrap_or_default()
            };
            let p = &mut self.players[i];
            let (back, fwd) = if p.facing < 0.0 {
                (b.r, b.l)
            } else {
                (b.l, b.r)
            };
            p.input
                .update(b.u, b.d, back, fwd, b.a, b.b, b.c, b.x, b.y, b.z, b.s);
            let input = p.input.clone();
            p.cmds.step(&input);
            i += 1;
        }

        // 2. prepare: hardcoded key behaviour and per-tick resets
        let mut i = 0;
        while i < self.players.len() {
            if !self.players[i].removed {
                self.prepare(i);
            }
            i += 1;
        }

        // 3. run states, physics, landing (helpers spawned mid-loop run this tick too)
        let mut i = 0;
        while i < self.players.len() {
            if !self.players[i].removed && !self.players[i].broken {
                self.running = Some(i);
                self.run(i);
                self.sanitize(i);
            }
            i += 1;
        }
        self.running = None;

        // 4. finish: KO
        for i in 0..self.players.len() {
            if !self.players[i].removed {
                self.finish(i);
            }
        }
        self.update_projectiles();

        // 5. collision
        collision::push_detection(self);
        collision::hit_detection(self);
        collision::projectile_hit_detection(self);

        // 6. bounds
        self.update_camera();
        for i in 0..self.players.len() {
            if !self.players[i].removed {
                self.screen_bound(i);
            }
        }
        self.check_blastzone();

        // 7. tick: animate, hit bookkeeping, get-hit state entry
        for i in 0..self.players.len() {
            if !self.players[i].removed {
                self.tick(i);
            }
        }
        self.tick_projectiles();
        self.update_explods();
        self.envshake.update();
        if self.pausetime <= 0 && self.supertime <= 0 {
            self.allpalfx.step();
            self.bgpalfx.step();
        }
        let _ = n;
        self.frame += 1;
    }

    /// Fill a PalFX from controller parameters (Ikemen `palFX.runSub`).
    fn eval_palfx(&mut self, i: usize, spec: &PalFxSpec) -> PalFx {
        let mut pf = PalFx::default();
        if let Some(t) = &spec.time {
            pf.time = self.eval(i, t).to_i();
        }
        if let Some(c) = &spec.color {
            pf.color = self.eval(i, c).to_f() / 256.0;
        }
        if let Some(v) = &spec.invertall {
            pf.invertall = self.eval(i, v).to_b();
        }
        let rgb = |w: &mut Self, codes: &Option<Vec<Code>>| -> Option<([i32; 3], Option<i32>)> {
            let codes = codes.as_ref()?;
            let mut out = [0; 3];
            for (k, c) in codes.iter().take(3).enumerate() {
                out[k] = w.eval(i, c).to_i();
            }
            let period = codes.get(3).map(|c| w.eval(i, c).to_i());
            Some((out, period))
        };
        if let Some((v, _)) = rgb(self, &spec.add) {
            pf.add = v;
        }
        if let Some((v, _)) = rgb(self, &spec.mul) {
            pf.mul = v;
        }
        for (slot, codes, dst) in [(0usize, &spec.sinadd, 0usize), (1, &spec.sinmul, 1)] {
            if let Some((mut v, period)) = rgb(self, codes) {
                let mut side = 1;
                if let Some(p) = period {
                    pf.cycletime[slot] = p.abs();
                    if p < 0 {
                        side = -1;
                    }
                }
                for x in &mut v {
                    *x *= side;
                }
                if dst == 0 {
                    pf.sinadd = v;
                } else {
                    pf.sinmul = v;
                }
            }
        }
        if let Some(codes) = &spec.sincolor {
            let mut side = 1;
            if let Some(p) = codes.get(1) {
                let p = self.eval(i, p).to_i();
                pf.cycletime[2] = p.abs();
                if p < 0 {
                    side = -1;
                }
            }
            if let Some(c) = codes.first() {
                pf.sincolor = self.eval(i, c).to_i() * side;
            }
        }
        pf
    }

    /// Snapshot the player's sprite for its AfterImage (runs after animation stepped).
    fn record_afterimage(&mut self, i: usize) {
        let p = &self.players[i];
        let Some(ai) = p.afterimage.as_ref() else {
            return;
        };
        if !ai.is_active() {
            return;
        }
        if p.hit_pause_time > 0 && !ai.ignorehitpause {
            return;
        }
        let img = AfterImg {
            pos: p.pos,
            facing: p.facing,
            scale: p.angle_scale,
            angle: if p.angle_drawn { p.angle } else { 0.0 },
            anim_owner: p.anim_owner,
            sprite_data: p.data,
            action: p.anim.action,
            elem: p.anim.elem,
            priority: p.spr_priority,
        };
        let p = &mut self.players[i];
        if let Some(ai) = p.afterimage.as_mut() {
            ai.record(img);
        }
    }

    /// Position for a helper / projectile relative to player `i` (Ikemen `helperPos`).
    fn spawn_pos(
        &self,
        i: usize,
        pt: PosType,
        off: [f32; 2],
        facing_param: i32,
        is_proj: bool,
    ) -> ([f32; 2], f32) {
        let p = &self.players[i];
        let mut facing = if facing_param < 0 { -1.0 } else { 1.0 };
        let (xmin, xmax) = (self.screen_left(), self.screen_left() + self.stage.width);
        let ls = self.local_scale(i);
        let pos = match pt {
            PosType::P1 => {
                facing *= p.facing;
                [p.pos[0] + off[0] * p.facing, p.pos[1] + off[1]]
            }
            PosType::P2 => match self.nearest_enemy(i) {
                Some(e) => {
                    let o = &self.players[e];
                    facing *= if is_proj { p.facing } else { o.facing };
                    let os = self.local_scale(e) / ls;
                    [o.pos[0] * os + off[0] * o.facing, o.pos[1] * os + off[1]]
                }
                None => [p.pos[0], p.pos[1]],
            },
            PosType::Front | PosType::Back => {
                facing *= p.facing;
                let right = (p.facing > 0.0 && pt == PosType::Front)
                    || (p.facing < 0.0 && pt == PosType::Back);
                let edge = if right { xmax } else { xmin } / ls;
                [edge + off[0] * p.facing, off[1]]
            }
            PosType::Left => {
                if is_proj {
                    facing *= p.facing;
                }
                [xmin / ls + off[0], off[1]]
            }
            PosType::Right => {
                if is_proj {
                    facing *= p.facing;
                }
                [xmax / ls + off[0], off[1]]
            }
            PosType::None => {
                if is_proj {
                    facing *= p.facing;
                }
                off
            }
        };
        (pos, facing)
    }

    /// Spawn a projectile for player `i`.
    pub fn spawn_projectile(&mut self, i: usize, spec: &ProjectileSpec) {
        if self.projs.iter().filter(|p| !p.removed).count() >= self.limits.projectiles {
            self.warn(format!("P{} reached the projectile limit", i + 1));
            return;
        }
        let hd = collision::eval_hitdef(self, i, &spec.hitdef);
        let ls = self.local_scale(i);
        let data = self.data[self.players[i].data].clone();
        let one = |w: &mut World, c: &Option<Code>| c.as_ref().map(|c| w.eval(i, c));
        let two = |w: &mut World, v: &Option<Vec<Code>>| -> Option<[f32; 2]> {
            let v = v.as_ref()?;
            let a = v.first().map_or(0.0, |c| w.eval(i, c).to_f());
            let b = v.get(1).map_or(0.0, |c| w.eval(i, c).to_f());
            Some([a, b])
        };
        let anim_no = one(self, &spec.projanim).map_or(0, |v| v.to_i());
        if data.air.get(anim_no).is_none() {
            self.warn(format!(
                "P{} projectile with invalid action {anim_no}",
                i + 1
            ));
            return;
        }
        let off = two(self, &spec.offset).unwrap_or([0.0, 0.0]);
        let pt = spec.postype.unwrap_or(PosType::P1);
        let (pos, facing) = self.spawn_pos(i, pt, off, 1, true);
        let root = self.players[i].root;
        let owner = if self.players[i].helper_index == 0 {
            i
        } else {
            root
        };
        let mut p = Projectile {
            owner,
            owner_id: self.players[owner].id,
            id: one(self, &spec.projid).map_or(0, |v| v.to_i()),
            status: ProjStatus::Active,
            hitdef: hd,
            anim: AnimState::start(anim_no),
            anim_no,
            hitanim: one(self, &spec.projhitanim).map_or(-1, |v| v.to_i()),
            remanim: one(self, &spec.projremanim).map_or(i32::MIN, |v| v.to_i().max(-2)),
            cancelanim: one(self, &spec.projcancelanim).map_or(i32::MIN, |v| v.to_i().max(-1)),
            scale: two(self, &spec.projscale).unwrap_or([1.0, 1.0]),
            remove: one(self, &spec.projremove).is_none_or(|v| v.to_b()),
            removetime: one(self, &spec.projremovetime).map_or(-1, |v| v.to_i()),
            velocity: two(self, &spec.velocity).map_or([0.0, 0.0], |v| [v[0] * ls, v[1] * ls]),
            remvelocity: two(self, &spec.remvelocity)
                .map_or([0.0, 0.0], |v| [v[0] * ls, v[1] * ls]),
            accel: two(self, &spec.accel).map_or([0.0, 0.0], |v| [v[0] * ls, v[1] * ls]),
            velmul: two(self, &spec.velmul).unwrap_or([1.0, 1.0]),
            hits: one(self, &spec.projhits).map_or(1, |v| v.to_i()),
            misstime: one(self, &spec.projmisstime).map_or(0, |v| v.to_i()),
            priority: one(self, &spec.projpriority).map_or(1, |v| v.to_i()),
            sprpriority: one(self, &spec.projsprpriority).map_or(3, |v| v.to_i()),
            edgebound: one(self, &spec.projedgebound).map_or(40.0, |v| v.to_f()) * ls,
            stagebound: one(self, &spec.projstagebound).map_or(40.0, |v| v.to_f()) * ls,
            heightbound: two(self, &spec.projheightbound)
                .map_or([-240.0 * ls, 1.0 * ls], |v| [v[0] * ls, v[1] * ls]),
            pos: [pos[0] * ls, pos[1] * ls],
            facing,
            removefacing: facing,
            supermovetime: one(self, &spec.supermovetime).map_or(0, |v| {
                if v.to_i() >= 0 {
                    v.to_i().saturating_add(1)
                } else {
                    v.to_i()
                }
            }),
            pausemovetime: one(self, &spec.pausemovetime).map_or(0, |v| {
                if v.to_i() >= 0 {
                    v.to_i().saturating_add(1)
                } else {
                    v.to_i()
                }
            }),
            curmisstime: 0,
            hitpause: 0,
            contactflag: false,
            freezeflag: false,
            time: 0,
            remove_done: false,
            removed: false,
        };
        if p.hitdef.hitonce < 0 {
            p.hitdef.hitonce = 0;
        }
        p.hitdef.attr |= 0; // keep as evaluated
        self.projs.push(p);
    }

    /// Spawn a helper of player `i`. Returns its index.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_helper(
        &mut self,
        i: usize,
        stateno: i32,
        helper_id: i32,
        pos: [f32; 2],
        pt: PosType,
        facing_param: i32,
        keyctrl: bool,
        player_type: bool,
    ) -> Option<usize> {
        let root = self.players[i].root;
        let helpers = self
            .players
            .iter()
            .filter(|p| !p.removed && p.helper_index > 0 && p.root == root)
            .count();
        if helpers >= self.limits.helpers {
            self.warn(format!("P{} reached the helper limit", i + 1));
            return None;
        }
        let parent = self.players[i].clone();
        let (p0, facing) = self.spawn_pos(i, pt, pos, facing_param, false);
        let id = self.players.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        let mut h = Player::new(
            parent.data,
            parent.team,
            id,
            parent.life_max,
            parent.power_max,
            p0[0],
            facing,
            parent.pal_no,
        );
        h.pos[1] = p0[1];
        h.old_pos = h.pos;
        h.helper_index = helpers as i32 + 1;
        h.helper_id = helper_id.max(0);
        h.parent_id = parent.id;
        h.root = parent.root;
        h.player_type = player_type;
        h.keyctrl = keyctrl;
        h.life = parent.life_max;
        h.cmds = parent.cmds.clone();
        h.input = parent.input.clone();
        h.ctrl = false;
        let roots = self.players.iter().filter(|p| p.helper_index == 0).count();
        let slot = self
            .players
            .iter()
            .enumerate()
            .skip(roots)
            .find(|(_, p)| p.removed)
            .map(|(k, _)| k);
        let idx = match slot {
            Some(k) => {
                self.players[k] = h;
                k
            }
            None => {
                self.players.push(h);
                self.players.len() - 1
            }
        };
        self.players[i].children.push(id);
        self.change_state(idx, stateno, Some(0), Some(true));
        // Ikemen `helperInit`: the statedef applies now, but the state body runs once,
        // at the helper's turn in this tick's run loop. Running it here as well made
        // `time = 0` controllers (Turn, VarAdd...) fire twice on the first tick.
        self.apply_pending(idx);
        if self.running.is_some_and(|r| idx < r) {
            // reused a slot the run loop already passed: take this tick's turn now
            self.run(idx);
            self.sanitize(idx);
        }
        Some(idx)
    }

    /// Remove helper `i` (and optionally its explods / children).
    pub fn destroy_helper(&mut self, i: usize, recursive: bool, remove_explods: bool) {
        if self.players[i].helper_index == 0 || self.players[i].removed {
            return;
        }
        let id = self.players[i].id;
        self.players[i].removed = true;
        for o in &mut self.players {
            o.targets.retain(|&t| t != id);
            o.hitdef_targets.retain(|&t| t != id);
            o.ghv.targeted_by.retain(|t| t.0 != id);
            o.children.retain(|&c| c != id);
            if o.parent_id == id {
                o.parent_id = -1;
            }
            if o.bind.is_some_and(|b| b.to == id) {
                o.bind = None;
            }
        }
        if remove_explods {
            self.remove_explods(id, -1);
        }
        if recursive {
            let kids: Vec<usize> = self
                .players
                .iter()
                .enumerate()
                .filter(|(_, o)| !o.removed && o.parent_id == id)
                .map(|(k, _)| k)
                .collect();
            for k in kids {
                self.destroy_helper(k, true, remove_explods);
            }
        }
    }

    pub fn player_by_id(&self, id: i32) -> Option<usize> {
        self.players.iter().position(|p| !p.removed && p.id == id)
    }

    pub fn set_pause_time(&mut self, i: usize, pausetime: i32, movetime: i32) {
        if !pausetime < self.pausetime_buffer || self.pause_player == i as i32 {
            self.pausetime_buffer = !pausetime;
            self.pause_player = i as i32;
        }
        let p = &mut self.players[i];
        p.pause_move_time = movetime.max(0);
        if p.pause_move_time > pausetime {
            p.pause_move_time = 0;
        } else if self.pausetime > 0 && p.pause_move_time > 0 {
            p.pause_move_time -= 1;
        }
    }

    pub fn set_super_pause_time(
        &mut self,
        i: usize,
        pausetime: i32,
        movetime: i32,
        unhittable: bool,
        p2defmul: f32,
    ) {
        if !pausetime < self.supertime_buffer || self.super_player == i as i32 {
            self.supertime_buffer = !pausetime;
            self.super_player = i as i32;
        }
        let p = &mut self.players[i];
        p.super_move_time = movetime.max(0);
        if p.super_move_time > pausetime {
            p.super_move_time = 0;
        } else if self.supertime > 0 && p.super_move_time > 0 {
            p.super_move_time -= 1;
        }
        if unhittable {
            p.unhittable_time = pausetime + (pausetime > 0) as i32;
        }
        if p2defmul != 1.0 {
            let team = p.team;
            for o in &mut self.players {
                if o.team != team {
                    o.super_defence_mul_buffer *= p2defmul;
                }
            }
        }
    }

    /// Spawn an explod for player `i` from a compiled spec. Returns its index.
    pub fn spawn_explod(&mut self, i: usize, spec: &ExplodSpec) -> Option<usize> {
        let (fx, anim_code) = spec.anim.as_ref()?;
        if self.explods.iter().filter(|e| !e.removed).count() >= self.limits.explods {
            self.warn(format!("P{} reached the explod limit", i + 1));
            return None;
        }
        let anim_no = self.eval(i, anim_code).to_i();
        let src = if *fx {
            AnimSrc::Fx
        } else {
            AnimSrc::Player(self.players[i].data)
        };
        let owner_id = self.players[i].id;
        let mut e = Explod::new(i, owner_id, src, anim_no);
        if e.air(self).and_then(|a| a.get(anim_no)).is_none() {
            self.warn(format!(
                "P{} attempted to create explod with invalid action {}{anim_no}",
                i + 1,
                if *fx { "F" } else { "" }
            ));
            return None;
        }
        e.id = spec
            .id
            .as_ref()
            .map_or(0, |c| self.eval(i, c).to_i().max(0));
        self.apply_explod_spec(i, &mut e, spec, true);
        self.explod_set_pos(&mut e, i);
        self.explods.push(e);
        Some(self.explods.len() - 1)
    }

    fn apply_explod_spec(&mut self, i: usize, e: &mut Explod, spec: &ExplodSpec, fresh: bool) {
        let ls = self.local_scale(i);
        let two = |w: &mut World, v: &Option<Vec<Code>>| -> Option<[f32; 2]> {
            let v = v.as_ref()?;
            let a = v.first().map_or(0.0, |c| w.eval(i, c).to_f());
            let b = v.get(1).map_or(0.0, |c| w.eval(i, c).to_f());
            Some([a, b])
        };
        if let Some(pt) = spec.postype {
            e.postype = pt;
        }
        if let Some(sp) = spec.space {
            e.space = sp;
        }
        if let Some(v) = two(self, &spec.pos) {
            e.rel = [v[0] * ls, v[1] * ls];
        }
        if let Some(v) = two(self, &spec.random) {
            let rx = self.rng.below(1000) as f32 / 1000.0 - 0.5;
            let ry = self.rng.below(1000) as f32 / 1000.0 - 0.5;
            e.rel[0] += v[0] * rx * ls;
            e.rel[1] += v[1] * ry * ls;
        }
        if let Some(c) = &spec.facing {
            e.facing = if self.eval(i, c).to_i() < 0 {
                -1.0
            } else {
                1.0
            };
        }
        if let Some(c) = &spec.vfacing {
            e.vfacing = if self.eval(i, c).to_i() < 0 {
                -1.0
            } else {
                1.0
            };
        }
        if let Some(c) = &spec.bindtime {
            e.bindtime = self.eval(i, c).to_i();
        }
        if let Some(c) = &spec.bindid {
            let id = self.eval(i, c).to_i();
            e.bind_id = if id == -1 { self.players[i].id } else { id };
        }
        if let Some(v) = two(self, &spec.vel) {
            e.velocity = [v[0] * ls, v[1] * ls];
        }
        if let Some(v) = two(self, &spec.accel) {
            e.accel = [v[0] * ls, v[1] * ls];
        }
        if let Some(v) = two(self, &spec.friction) {
            e.friction = v;
        }
        if let Some(c) = &spec.removetime {
            e.removetime = self.eval(i, c).to_i();
        }
        if let Some(c) = &spec.supermove {
            e.supermovetime = if self.eval(i, c).to_b() { -1 } else { 0 };
        }
        if let Some(c) = &spec.supermovetime {
            e.supermovetime = self.eval(i, c).to_i();
        }
        if let Some(c) = &spec.pausemovetime {
            e.pausemovetime = self.eval(i, c).to_i();
        }
        if let Some(v) = two(self, &spec.scale) {
            e.scale = v;
        }
        if let Some(c) = &spec.sprpriority {
            e.sprpriority = self.eval(i, c).to_i();
        }
        if let Some(c) = &spec.ontop {
            e.ontop = self.eval(i, c).to_b();
            if e.ontop {
                e.sprpriority = 0;
            }
        }
        if let Some(c) = &spec.under {
            e.under = self.eval(i, c).to_b();
        }
        if let Some(c) = &spec.removeongethit {
            e.removeongethit = self.eval(i, c).to_b();
        }
        if let Some(c) = &spec.removeonchangestate {
            e.removeonchangestate = self.eval(i, c).to_b();
        }
        if let Some(c) = &spec.ignorehitpause {
            e.ignorehitpause = self.eval(i, c).to_b();
        }
        if let Some(t) = spec.trans {
            e.trans = t;
        }
        if let Some(v) = two(self, &spec.alpha) {
            e.alpha = [v[0] as i32, v[1] as i32];
        }
        if let Some(c) = &spec.angle {
            e.angle = self.eval(i, c).to_f();
        }
        if let Some(c) = &spec.animfreeze {
            e.animfreeze = self.eval(i, c).to_b();
        }
        if fresh && let Some(c) = &spec.animelem {
            let el = self.eval(i, c).to_i();
            if let Some(a) = e.air(self).and_then(|a| a.get(e.anim.action)).cloned() {
                e.anim.set_elem(&a, el, 0);
            }
        }
    }

    /// Spawn a hit/guard spark (`F` fightfx or the player's own AIR) at a stage position.
    pub(crate) fn spawn_spark(
        &mut self,
        i: usize,
        own: bool,
        anim_no: i32,
        stage_pos: [f32; 2],
        facing: f32,
    ) {
        let src = if own {
            AnimSrc::Player(self.players[i].data)
        } else {
            AnimSrc::Fx
        };
        let owner_id = self.players[i].id;
        let mut e = Explod::new(i, owner_id, src, anim_no);
        if e.air(self).and_then(|a| a.get(anim_no)).is_none() {
            return;
        }
        e.id = -1; // sparks are anonymous: not found by NumExplod / RemoveExplod
        e.postype = PosType::None;
        e.space = Space::Stage;
        e.bindtime = 0;
        e.pos = stage_pos;
        e.facing = facing;
        e.ontop = true;
        e.sprpriority = i32::MAX;
        e.supermovetime = -1;
        e.pausemovetime = -1;
        e.removetime = -2;
        self.explods.push(e);
    }

    fn finish(&mut self, i: usize) {
        self.players[i].inguarddist = false;
        self.players[i].prev_fallflag = self.players[i].ghv.fallflag;
        let p = &self.players[i];
        if p.hit_pause_time == 0
            && p.alive()
            && p.life <= 0
            && !p.asserted(assert::NOKO)
            && (!p.ghv.guarded)
        {
            self.push_sound(Event::Sound {
                player: i,
                own: false,
                group: 11,
                number: 0,
                channel: -1,
                volume: 100,
            });
            let p = &mut self.players[i];
            p.ko = true;
            p.ctrl = false;
            if p.pending_state.is_none() && p.move_type != MoveType::H {
                p.ghv.fallflag = true;
                self.self_state(i, 5030, None, Some(false));
                self.run_current(i);
                self.players[i].state_time = 1;
            }
        }
    }

    fn tick(&mut self, i: usize) {
        if self.players[i].pause_bool {
            return;
        }
        // animate (not during hit pause)
        let data = self.data[self.players[i].anim_owner].clone();
        {
            let p = &mut self.players[i];
            if p.hit_pause_time == 0
                && let Some(a) = data.air.get(p.anim.action)
            {
                p.anim.step(a);
            }
            p.old_pos = p.pos;
            p.pushed = false;
            if p.hit_pause_time == 0 {
                p.palfx.step();
            }
        }
        self.record_afterimage(i);
        // contact bookkeeping
        {
            let p = &mut self.players[i];
            if p.mctime < 0 {
                p.mctime = 1;
                let numhits = p.hitdef.as_ref().map_or(1, |h| h.numhits);
                match p.mctype {
                    MoveContact::Hit => p.hit_count = p.hit_count.saturating_add(numhits),
                    MoveContact::Guarded => p.guard_count = p.guard_count.saturating_add(numhits),
                    _ => {}
                }
            }
            if !p.hitdef_targets.is_empty() {
                let add = std::mem::take(&mut p.hitdef_targets);
                for id in add {
                    if !p.targets.contains(&id) {
                        p.targets.push(id);
                    }
                }
                // keep them as "already hit by this HitDef" too
                p.hitdef_targets = p.targets.clone();
            }
        }
        // enter the get-hit state
        if self.players[i].gethit {
            let p = &mut self.players[i];
            p.move_type = MoveType::H;
            p.hit_pause_time = 0;
            let custom = p.pending_state.is_some();
            let st = p.state_type;
            let on_ground_down = st == StateType::L && p.pos[1] == p.ground;
            let trip = p.ghv.hit_type == HitType::Trip;
            if custom {
                p.prev_state_no = if on_ground_down {
                    5080
                } else if trip {
                    5070
                } else {
                    match st {
                        StateType::S => 5000,
                        StateType::C => 5010,
                        _ => 5020,
                    }
                };
            } else if p.ghv.guarded && (p.ghv.damage < p.life || p.asserted(assert::NOKO)) {
                let no = match st {
                    StateType::S => 150,
                    StateType::C => 152,
                    _ => 154,
                };
                self.self_state(i, no, None, Some(false));
            } else if on_ground_down {
                self.change_state(i, 5080, None, Some(false));
            } else if trip {
                self.change_state(i, 5070, None, Some(false));
            } else {
                if p.ghv.forcestand && st == StateType::C {
                    p.state_type = StateType::S;
                }
                let no = match p.state_type {
                    StateType::S => 5000,
                    StateType::C => {
                        if p.ghv.damage >= p.life && !p.asserted(assert::NOKO) {
                            5000
                        } else {
                            5010
                        }
                    }
                    _ => 5020,
                };
                self.change_state(i, no, None, Some(false));
            }
            let p = &mut self.players[i];
            if p.state_type == StateType::L && p.pos[1] == p.ground && p.ghv.yvel != 0.0 {
                p.down_hit_offset = true;
            }
        }
        // HitOverride state (does not require actually getting hit)
        let hi = self.players[i].hover_idx;
        if hi >= 0 {
            let ho = self.players[i].hover[hi as usize];
            self.players[i].hover_idx = -1;
            if ho.stateno >= 0 {
                let own = self.players[i].data;
                self.players[i].state_owner = own;
                self.change_state(i, ho.stateno, None, None);
            }
        }
        let p = &mut self.players[i];
        if p.hit_pause_time > 0 {
            p.hit_pause_time -= 1;
        }
    }

    fn prepare(&mut self, i: usize) {
        if self.players[i].pause_bool {
            return;
        }
        let p = &self.players[i];
        let data = self.data[p.data].clone();
        let (ub, db, fb, bb) = (p.input.ub, p.input.db, p.input.fb, p.input.bb);
        // Ikemen `actionPrepare`: the hardcoded basic actions read command input,
        // so helpers only take them with keyctrl
        let keys = p.helper_index == 0 || p.keyctrl;
        if keys && p.ctrl {
            let st = p.state_type;
            let no = p.state_no;
            if p.guarding && p.inguarddist && !p.in_guard_state() && st != StateType::L && bb > 0 {
                self.change_state(i, 120, None, None);
            } else if st == StateType::S && ub > 0 {
                if no != 40 {
                    self.change_state(i, 40, None, None);
                }
            } else if st == StateType::A
                && ub == 1
                && p.pos[1] - p.ground <= -data.konst("movement.airjump.height").unwrap_or(35.0)
                && p.air_jump_count
                    < data.konst("movement.airjump.num").unwrap_or(1.0) as i32
                        + self.movement.air_jumps
            {
                if no != 45 || p.state_time > 0 {
                    self.players[i].air_jump_count += 1;
                    self.change_state(i, 45, None, None);
                }
            } else if st == StateType::S && db > 0 {
                if no != 10 {
                    if no != 100 {
                        self.players[i].vel[0] = 0.0;
                    }
                    self.change_state(i, 10, None, None);
                }
            } else if st == StateType::C && db <= 0 {
                if no != 12 {
                    self.change_state(i, 12, None, None);
                }
            } else if !p.asserted(assert::NOWALK)
                && st == StateType::S
                && ((fb > 0) != (bb > 0))
                && no != 20
            {
                self.change_state(i, 20, None, None);
            }
        }
        let p = &self.players[i];
        if keys && p.state_no == 20 && (bb > 0) == (fb > 0) {
            self.change_state(i, 0, None, None);
        }
        if keys {
            self.engine_air_moves(i, &data, fb, bb, db);
        }
        let p = &mut self.players[i];
        if p.state_type != StateType::A {
            p.air_jump_count = 0;
            p.air_dash_used = false;
            p.air_dash_time = 0;
        }
        if p.hit_pause_time == 0 {
            for slot in &mut p.hitby {
                if slot.time > 0 {
                    slot.time -= 1;
                    if slot.time == 0 {
                        *slot = HitBySlot::default();
                    }
                }
            }
            for ho in &mut p.hover {
                if ho.time > 0 {
                    ho.time -= 1;
                    if ho.time == 0 {
                        *ho = HitOverride::default();
                    }
                }
            }
            p.width_edge = [0.0, 0.0];
            p.width_player = [0.0, 0.0];
            p.screen_bound = true;
            p.move_camera = [true, true];
            p.player_push = p.alive() || p.state_no != 5150;
        }
        if p.unhittable_time > 0 {
            p.unhittable_time -= 1;
        }
        if self.supertime > 0 {
            if p.super_move_time > 0 {
                p.super_move_time -= 1;
            }
        } else if self.pausetime > 0 && p.pause_move_time > 0 {
            p.pause_move_time -= 1;
        }
        if p.super_defence_mul_buffer != 1.0 {
            if p.move_type == MoveType::H {
                p.super_defence_mul *= p.super_defence_mul_buffer;
            }
            p.super_defence_mul_buffer = 1.0;
        }
        p.assert_flags = 0;
        p.pos_freeze = false;
        p.angle_drawn = false;
        p.angle_scale = [1.0, 1.0];
        p.trans = crate::explod::Trans::Default;
        p.alpha = [255, 0];
        // step binds
        if let Some(b) = p.bind {
            if b.time > 0 {
                p.bind = Some(Bind {
                    time: b.time - 1,
                    facing: b.facing,
                    ..b
                });
            }
            if p.bind.is_some_and(|b| b.time == 0) {
                p.bind = None;
            }
        }
    }

    fn run(&mut self, i: usize) {
        #[cfg(test)]
        if self.panic_on_run == Some(i) {
            panic!("simulated content crash in P{}", i + 1);
        }
        if self.players[i].pause_bool {
            return;
        }
        let own = self.data[self.players[i].data].clone();
        let p = &self.players[i];
        // Ikemen `actionRun`: the special states always come from the player's own
        // files. -3 is skipped in a custom state (MUGEN cns docs), -2 always runs,
        // -1 only in its own states. Helpers run -3 and -2 only as player-type
        // helpers and -1 only with keyctrl (MUGEN Helper doc).
        let in_own = p.state_owner == p.data;
        let root = p.helper_index == 0;
        let runs = [
            in_own && (root || p.player_type),
            root || p.player_type,
            in_own && (root || p.keyctrl),
        ];
        for (neg, run) in [-3, -2, -1].into_iter().zip(runs) {
            if !run {
                continue;
            }
            if let Some(state) = own.states.get(&neg) {
                self.players[i].minus = neg as i8;
                let mut persist = vec![0; state.ctrls.len()];
                self.run_ctrls(i, &state.ctrls, &mut persist);
            }
        }
        self.players[i].minus = 0;
        self.run_current(i);
        // guarding flag for next tick's hit checks and guard entry
        let guarding = collision::compute_guard_flag(&self.players[i]);
        self.players[i].guarding = guarding;
        if self.players[i].hit_pause_time == 0 {
            // forced stay in 5110 then get up
            let p = &self.players[i];
            if p.state_no == 5110 && p.state_time >= 1 && p.ghv.down_recovertime <= 0 && p.alive() {
                self.change_state(i, 5120, None, None);
                self.run_current(i);
            }
            if self.pos_update(i) {
                self.change_state(i, 50, None, None);
                self.run_current(i);
            }
            let p = &self.players[i];
            if p.physics == Physics::A
                && p.vel[1] > 0.0
                && p.pos[1] >= p.ground
                && p.state_no != 105
            {
                // engine-side ChangeState outside a state body: the new state runs right away
                self.change_state(i, 52, None, None);
                self.run_current(i);
            }
            let f = self.players[i].p1facing;
            self.players[i].set_facing(f);
            self.players[i].p1facing = 0.0;
            self.players[i].state_time += 1;
            if self.players[i].mctime > 0 {
                self.players[i].mctime += 1;
            }
            if self.players[i].pctime >= 0 {
                self.players[i].pctime += 1;
            }
        }
        // pending damage from hits lands here
        let liedown = own.konst("data.liedown.time").unwrap_or(60.0) as i32;
        let p = &mut self.players[i];
        if p.ghv.damage != 0 {
            if p.move_type == MoveType::H {
                let d = p.ghv.damage;
                p.life = (p.life - d).max(0);
            }
            p.ghv.damage = 0;
        }
        if p.hit_pause_time == 0 {
            if p.move_type == MoveType::H {
                if p.ghv.guarded {
                    p.received_hits = 0;
                }
                if p.ghv.hitshaketime > 0 {
                    p.ghv.hitshaketime -= 1;
                }
                if p.ghv.fallflag {
                    p.ghv.fall_time += 1;
                }
                p.hittmp = p.ghv.fallflag as i8 + 1;
            } else {
                if p.hittmp > 0 {
                    p.hittmp = 0;
                }
                p.ghv.hitshaketime = 0;
                p.fall_defence_mul = 1.0;
                p.super_defence_mul = 1.0;
                p.ghv.fallflag = false;
                p.ghv.fallcount = 0;
                p.ghv.hitid >>= 31;
                p.ghv.guardcount = 0;
                p.received_hits = 0;
                p.ghv.down_recovertime = liedown;
                p.ghv.fall_envshake_time = 0;
            }
            if p.ghv.hitshaketime <= 0 && p.ghv.hittime >= 0 {
                p.ghv.hittime -= 1;
            }
            if p.ghv.down_recovertime > 0 && p.state_no == 5110 {
                p.ghv.down_recovertime -= 1;
            }
        }
        let gp = std::mem::take(&mut p.ghv.power);
        let mp = std::mem::take(&mut p.mhv.power);
        if gp != 0 {
            self.power_add(i, gp);
        }
        if mp != 0 {
            self.power_add(i, mp);
        }
        // drop targets that left their hit state
        let my_id = self.players[i].id;
        let keep: Vec<i32> = self.players[i]
            .targets
            .iter()
            .copied()
            .filter(|&tid| {
                self.players.iter().any(|o| {
                    o.id == tid
                        && (o.move_type == MoveType::H || o.gethit)
                        && o.ghv.targeted_by.iter().any(|t| t.0 == my_id)
                })
            })
            .collect();
        self.players[i].targets = keep;
    }

    pub(crate) fn run_current_pub(&mut self, i: usize) {
        self.run_current(i);
    }

    /// Apply a buffered state change and run the current state's controllers,
    /// following ChangeStates from inside the body immediately (Ikemen `changeStateEx`).
    fn run_current(&mut self, i: usize) {
        self.apply_pending(i);
        let mut loops = 0;
        loop {
            let data = self.data[self.players[i].state_owner].clone();
            let no = self.players[i].state_no;
            let Some(state) = data.states.get(&no) else {
                break;
            };
            let mut persist = std::mem::take(&mut self.players[i].persist);
            let changed = self.run_ctrls(i, &state.ctrls, &mut persist);
            if self.players[i].persist.is_empty() {
                self.players[i].persist = persist;
            }
            if !changed || !self.apply_pending(i) {
                break;
            }
            loops += 1;
            if loops >= MAX_STATE_LOOP {
                if !self.players[i].loop_warned {
                    self.players[i].loop_warned = true;
                    self.warn(format!(
                        "P{} state machine stuck in a loop at state {no}",
                        i + 1
                    ));
                }
                break;
            }
        }
    }

    /// Run a controller list. Returns true if the state changed (stop processing).
    fn run_ctrls(&mut self, i: usize, ctrls: &[Ctrl], persist: &mut [i32]) -> bool {
        for (ci, c) in ctrls.iter().enumerate() {
            if self.players[i].hit_pause_time > 0 && !c.ignorehitpause {
                continue;
            }
            if let Some(ps) = persist.get_mut(ci) {
                if *ps != i32::MAX {
                    *ps -= 1;
                }
                if *ps > 0 {
                    continue;
                }
            }
            let mut fired = true;
            for t in &c.triggerall {
                if !self.eval(i, t).to_b() {
                    fired = false;
                    break;
                }
            }
            if fired {
                fired = false;
                for group in &c.triggers {
                    let mut all = true;
                    for t in group {
                        if !self.eval(i, t).to_b() {
                            all = false;
                            break;
                        }
                    }
                    if all {
                        fired = true;
                        break;
                    }
                }
            }
            if !fired {
                continue;
            }
            if let Some(ps) = persist.get_mut(ci) {
                *ps = c.persistent;
            }
            if self.exec(i, &c.op) {
                return true;
            }
        }
        false
    }

    fn evf(&mut self, i: usize, c: &Option<Code>) -> Option<f32> {
        c.as_ref().map(|c| self.eval(i, c).to_f())
    }

    /// Execute one controller. Returns true when it changed the state of the running player.
    fn exec(&mut self, i: usize, op: &Sctrl) -> bool {
        match op {
            Sctrl::Null | Sctrl::Unsupported(_) => {}
            Sctrl::ChangeState { value, ctrl, anim } => {
                let v = self.eval(i, value).to_i();
                let a = anim.as_ref().map(|c| self.eval(i, c).to_i());
                let c = ctrl.as_ref().map(|c| self.eval(i, c).to_b());
                self.change_state(i, v, a, c);
                return true;
            }
            Sctrl::SelfState { value, ctrl, anim } => {
                let v = self.eval(i, value).to_i();
                let a = anim.as_ref().map(|c| self.eval(i, c).to_i());
                let c = ctrl.as_ref().map(|c| self.eval(i, c).to_b());
                self.self_state(i, v, a, c);
                return true;
            }
            Sctrl::ChangeAnim { value, elem } => {
                let v = self.eval(i, value).to_i();
                let e = elem.as_ref().map(|c| (self.eval(i, c).to_i(), 0));
                self.change_anim(i, v, e);
            }
            Sctrl::ChangeAnim2 { value, elem } => {
                let v = self.eval(i, value).to_i();
                let e = elem.as_ref().map(|c| (self.eval(i, c).to_i(), 0));
                self.change_anim2(i, v, e);
            }
            Sctrl::HitDef(spec) => {
                let hd = collision::eval_hitdef(self, i, spec);
                let p = &mut self.players[i];
                p.hitdef = Some(hd);
                p.hitdef_targets.clear();
            }
            Sctrl::HitVelSet { x, y } => {
                let sx = x.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                let sy = y.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                let p = &mut self.players[i];
                if sx {
                    p.vel[0] = p.ghv.xvel * p.facing;
                }
                if sy {
                    p.vel[1] = p.ghv.yvel;
                }
            }
            Sctrl::HitFallSet { value, xvel, yvel } => {
                let v = value.as_ref().map(|c| self.eval(i, c).to_i());
                let xv = xvel.as_ref().map(|c| self.eval(i, c).to_f());
                let yv = yvel.as_ref().map(|c| self.eval(i, c).to_f());
                let p = &mut self.players[i];
                if let Some(f) = v
                    && f >= 0
                {
                    p.ghv.fallflag = f != 0;
                }
                if let Some(x) = xv {
                    p.ghv.fall_xvelocity = Some(x);
                }
                if let Some(y) = yv {
                    p.ghv.fall_yvelocity = y;
                }
            }
            Sctrl::HitFallVel => {
                let p = &mut self.players[i];
                if p.move_type == MoveType::H {
                    if let Some(x) = p.ghv.fall_xvelocity {
                        p.vel[0] = x;
                    }
                    p.vel[1] = p.ghv.fall_yvelocity;
                }
            }
            Sctrl::HitFallDamage => {
                let p = &mut self.players[i];
                if p.move_type == MoveType::H {
                    let d = p.ghv.fall_damage;
                    p.ghv.fall_damage = 0;
                    let kill = p.ghv.fall_kill;
                    p.life = (p.life - d).max(if kill { 0 } else { 1 });
                }
            }
            Sctrl::HitAdd { value } => {
                let v = self.eval(i, value).to_i();
                let p = &mut self.players[i];
                p.hit_count = p.hit_count.saturating_add(v);
                p.uniq_hit_count = p.uniq_hit_count.saturating_add(v);
            }
            Sctrl::MoveHitReset => {
                let p = &mut self.players[i];
                p.mctype = MoveContact::None;
                p.mctime = 0;
            }
            Sctrl::HitBy {
                not,
                slot,
                attr,
                time,
            } => {
                let t = time.as_ref().map_or(1, |c| self.eval(i, c).to_i());
                self.players[i].hitby[*slot] = HitBySlot {
                    attr: *attr,
                    time: t,
                    not: *not,
                };
            }
            Sctrl::TargetState { value, id } => {
                let v = self.eval(i, value).to_i();
                let tid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let owner = self.players[i].state_owner;
                for t in self.target_indices(i, tid) {
                    self.players[t].state_owner = owner;
                    self.change_state(t, v, None, Some(false));
                    self.players[t].move_type = MoveType::H;
                }
            }
            Sctrl::TargetLifeAdd {
                value,
                kill,
                absolute,
                id,
            } => {
                let v = self.eval(i, value).to_i();
                let kill = kill.as_ref().is_none_or(|c| self.eval(i, c).to_b());
                let abs = absolute.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                let tid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                for t in self.target_indices(i, tid) {
                    let d = if abs {
                        v
                    } else {
                        -collision_damage(self, t, i, -v, kill)
                    };
                    let p = &mut self.players[t];
                    p.life = p
                        .life
                        .saturating_add(d)
                        .min(p.life_max)
                        .max(if kill { 0 } else { 1 });
                }
            }
            Sctrl::TargetPowerAdd { value, id } => {
                let v = self.eval(i, value).to_i();
                let tid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                for t in self.target_indices(i, tid) {
                    self.power_add(t, v);
                }
            }
            Sctrl::TargetVelSet { x, y, id } | Sctrl::TargetVelAdd { x, y, id } => {
                let add = matches!(op, Sctrl::TargetVelAdd { .. });
                let xv = self.evf(i, x);
                let yv = self.evf(i, y);
                let tid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let f = self.players[i].facing;
                for t in self.target_indices(i, tid) {
                    let p = &mut self.players[t];
                    if let Some(v) = xv {
                        let v = if add { v * p.facing } else { v * f * p.facing };
                        if add {
                            p.vel[0] += v;
                        } else {
                            p.vel[0] = v;
                        }
                    }
                    if let Some(v) = yv {
                        if add {
                            p.vel[1] += v;
                        } else {
                            p.vel[1] = v;
                        }
                    }
                }
            }
            Sctrl::TargetFacing { value, id } => {
                let v = self.eval(i, value).to_i();
                let tid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let f = self.players[i].facing;
                for t in self.target_indices(i, tid) {
                    let nf = if v > 0 { f } else { -f };
                    self.players[t].set_facing(nf);
                }
            }
            Sctrl::TargetBind { time, pos, id } => {
                let t = time.as_ref().map_or(1, |c| self.eval(i, c).to_i());
                let px = pos
                    .as_ref()
                    .and_then(|p| p.first())
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let py = pos
                    .as_ref()
                    .and_then(|p| p.get(1))
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let tid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let my = self.players[i].id;
                for tg in self.target_indices(i, tid) {
                    self.players[tg].bind = Some(Bind {
                        to: my,
                        time: t,
                        pos: [px, py],
                        facing: 0.0,
                    });
                    self.apply_bind(tg);
                }
            }
            Sctrl::TargetDrop { excludeid, keepone } => {
                let ex = excludeid.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let keep = keepone.as_ref().is_none_or(|c| self.eval(i, c).to_b());
                let my = self.players[i].id;
                let mut kept = Vec::new();
                for &tid in &self.players[i].targets.clone() {
                    let hid = self
                        .players
                        .iter()
                        .find(|o| o.id == tid)
                        .map_or(-1, |o| o.ghv.hitid);
                    if ex >= 0 && hid == ex && (!keep || kept.is_empty()) {
                        kept.push(tid);
                    } else if let Some(o) = self.players.iter_mut().find(|o| o.id == tid) {
                        o.ghv.targeted_by.retain(|t| t.0 != my);
                    }
                }
                self.players[i].targets = kept;
            }
            Sctrl::PlayerPush { value } => {
                let v = value.as_ref().is_none_or(|c| self.eval(i, c).to_b());
                self.players[i].player_push = v;
            }
            Sctrl::AttackMulSet { value } => {
                let v = self.eval(i, value).to_f();
                self.players[i].attack_mul = v;
            }
            Sctrl::DefenceMulSet { value } => {
                let v = self.eval(i, value).to_f();
                self.players[i].defence_mul = v;
            }
            Sctrl::HitOverride {
                attr,
                slot,
                stateno,
                time,
                forceair,
                forceguard,
            } => {
                let mut sl = slot.as_ref().map_or(0, |c| self.eval(i, c).to_i()).max(0);
                if sl > 7 {
                    sl = 0;
                }
                let ho = HitOverride {
                    attr: *attr,
                    stateno: stateno.as_ref().map_or(-1, |c| self.eval(i, c).to_i()),
                    time: time.as_ref().map_or(1, |c| self.eval(i, c).to_i()),
                    forceair: forceair.as_ref().is_some_and(|c| self.eval(i, c).to_b()),
                    forceguard: forceguard.as_ref().is_some_and(|c| self.eval(i, c).to_b()),
                };
                self.players[i].hover[sl as usize] = ho;
            }
            Sctrl::BindToParent {
                root,
                time,
                facing,
                pos,
            } => {
                let t = time.as_ref().map_or(1, |c| self.eval(i, c).to_i());
                let f = facing.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                let px = pos
                    .as_ref()
                    .and_then(|p| p.first())
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let py = pos
                    .as_ref()
                    .and_then(|p| p.get(1))
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let p = &self.players[i];
                let to = if *root {
                    self.players.get(p.root).map(|r| r.id)
                } else if p.parent_id >= 0 {
                    Some(p.parent_id)
                } else {
                    None
                };
                if let Some(to) = to
                    && to != p.id
                {
                    if t == 0 {
                        self.players[i].bind = None;
                    } else {
                        self.players[i].bind = Some(Bind {
                            to,
                            time: t,
                            pos: [px, py],
                            facing: f.signum() as f32,
                        });
                        self.apply_bind(i);
                    }
                }
            }
            Sctrl::ParentVarSet {
                kind,
                index,
                value,
                add,
            } => {
                let idx = self.eval(i, index).to_i();
                let v = self.eval(i, value);
                let pid = self.players[i].parent_id;
                if let Some(t) = self.players.iter().position(|o| o.id == pid && pid >= 0) {
                    self.write_var(t, *kind, idx, v, *add);
                }
            }
            Sctrl::StopSnd { channel } => {
                let ch = channel.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                if ch < 0 {
                    for p in 0..self.players.len() {
                        self.events.push(Event::StopSound {
                            player: p,
                            channel: -1,
                        });
                    }
                } else {
                    self.events.push(Event::StopSound {
                        player: i,
                        channel: ch,
                    });
                }
            }
            Sctrl::VarRandom { v, range } => {
                let idx = self.eval(i, v).to_i();
                let (mut lo, mut hi) = (0, 1000);
                if let Some(r) = range {
                    if let Some(c) = r.first() {
                        hi = self.eval(i, c).to_i();
                    }
                    if let Some(c) = r.get(1) {
                        lo = hi;
                        hi = self.eval(i, c).to_i();
                    }
                }
                let (lo, hi) = (lo.min(hi), lo.max(hi));
                let val = lo + self.rng.below((hi - lo + 1) as u32) as i32;
                self.write_var(i, VarKind::Var, idx, Value::Int(val), false);
            }
            Sctrl::GameMakeAnim {
                value,
                under,
                pos,
                random,
            } => {
                let anim = value.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                let under = under.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                let mut px = pos
                    .as_ref()
                    .and_then(|p| p.first())
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let py = pos
                    .as_ref()
                    .and_then(|p| p.get(1))
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                if let Some(c) = random {
                    let r = self.eval(i, c).to_f() / 2.0;
                    if r > 0.0 {
                        let u = self.rng.below(1000) as f32 / 999.0;
                        px += -r + 2.0 * r * u;
                    }
                }
                self.spawn_fx_anim(i, anim, [px, py], !under);
            }
            Sctrl::MakeDust { pos, pos2 } => {
                for p in [pos, pos2].into_iter().flatten() {
                    let px = p.first().map_or(0.0, |c| self.eval(i, c).to_f());
                    let py = p.get(1).map_or(0.0, |c| self.eval(i, c).to_f());
                    self.spawn_fx_anim(i, 120, [px, py], false);
                }
                if pos.is_none() && pos2.is_none() {
                    self.spawn_fx_anim(i, 120, [0.0, 0.0], false);
                }
            }
            Sctrl::FallEnvShake => {
                let g = &self.players[i].ghv;
                if g.fall_envshake_time > 0 {
                    self.envshake = EnvShake {
                        time: g.fall_envshake_time,
                        freq: g.fall_envshake_freq,
                        ampl: g.fall_envshake_ampl as f32,
                        phase: g.fall_envshake_phase,
                        ..Default::default()
                    };
                    self.envshake.start();
                }
                self.players[i].ghv.fall_envshake_time = 0;
            }
            Sctrl::EnvShake {
                time,
                freq,
                ampl,
                phase,
            } => {
                let scale = self.local_scale(i);
                let mut e = EnvShake {
                    time: self.eval(i, time).to_i(),
                    ..Default::default()
                };
                if let Some(c) = freq {
                    e.freq = self.eval(i, c).to_f().max(0.0);
                }
                if let Some(c) = ampl {
                    e.ampl = (self.eval(i, c).to_f() * scale) as i32 as f32;
                }
                if let Some(c) = phase {
                    e.phase = Some(self.eval(i, c).to_f().clamp(-180.0, 180.0));
                }
                e.start();
                self.envshake = e;
            }
            Sctrl::Trans { trans, alpha } => {
                let mut a = [255, 0];
                if let Some(codes) = alpha {
                    for (k, c) in codes.iter().take(2).enumerate() {
                        a[k] = self.eval(i, c).to_i().clamp(0, 255);
                    }
                }
                let p = &mut self.players[i];
                p.trans = *trans;
                p.alpha = a;
            }
            Sctrl::AngleDraw { value, scale } => {
                if let Some(c) = value {
                    let v = self.eval(i, c).to_f();
                    self.players[i].angle = v;
                }
                if let Some(codes) = scale {
                    for (k, c) in codes.iter().take(2).enumerate() {
                        let v = self.eval(i, c).to_f();
                        self.players[i].angle_scale[k] *= v;
                    }
                }
                self.players[i].angle_drawn = true;
            }
            Sctrl::AngleSet { value } => {
                let v = value.as_ref().map_or(0.0, |c| self.eval(i, c).to_f());
                self.players[i].angle = v;
            }
            Sctrl::AngleAdd { value } => {
                let v = value.as_ref().map_or(0.0, |c| self.eval(i, c).to_f());
                self.players[i].angle += v;
            }
            Sctrl::AngleMul { value } => {
                let v = value.as_ref().map_or(0.0, |c| self.eval(i, c).to_f());
                self.players[i].angle *= v;
            }
            Sctrl::PalFx(spec) => {
                let pf = self.eval_palfx(i, spec);
                self.players[i].palfx = pf;
            }
            Sctrl::AllPalFx(spec) => {
                self.allpalfx = self.eval_palfx(i, spec);
            }
            Sctrl::BgPalFx(spec) => {
                self.bgpalfx = self.eval_palfx(i, spec);
            }
            Sctrl::AfterImage(spec) => {
                let mut ai = AfterImage {
                    time: 1,
                    ..Default::default()
                };
                if let Some(c) = &spec.time {
                    ai.time = self.eval(i, c).to_i();
                }
                if let Some(c) = &spec.length {
                    ai.length = self.eval(i, c).to_i().clamp(0, crate::fx::MAX_AIMG_LENGTH);
                }
                if let Some(c) = &spec.timegap {
                    let v = self.eval(i, c).to_i();
                    if v >= 1 {
                        ai.timegap = v;
                    }
                }
                if let Some(c) = &spec.framegap {
                    let v = self.eval(i, c).to_i();
                    if v >= 1 {
                        ai.framegap = v;
                    }
                }
                if let Some(c) = &spec.palcolor {
                    let v = self.eval(i, c).to_i();
                    ai.set_pal_color(v);
                }
                if let Some(c) = &spec.palinvertall {
                    let v = self.eval(i, c).to_b();
                    ai.set_pal_invertall(v);
                }
                if let Some(codes) = &spec.palbright {
                    for (k, c) in codes.iter().take(3).enumerate() {
                        let v = self.eval(i, c).to_i();
                        ai.set_pal_bright(k, v);
                    }
                }
                if let Some(codes) = &spec.palcontrast {
                    for (k, c) in codes.iter().take(3).enumerate() {
                        let v = self.eval(i, c).to_i();
                        ai.set_pal_contrast(k, v);
                    }
                }
                if let Some(codes) = &spec.palpostbright {
                    for (k, c) in codes.iter().take(3).enumerate() {
                        ai.postbright[k] = self.eval(i, c).to_i();
                    }
                }
                if let Some(codes) = &spec.paladd {
                    for (k, c) in codes.iter().take(3).enumerate() {
                        ai.add[k] = self.eval(i, c).to_i();
                    }
                }
                if let Some(codes) = &spec.palmul {
                    for (k, c) in codes.iter().take(3).enumerate() {
                        ai.mul[k] = self.eval(i, c).to_f();
                    }
                }
                if let Some(t) = spec.trans {
                    ai.trans = t;
                }
                if let Some(codes) = &spec.alpha {
                    for (k, c) in codes.iter().take(2).enumerate() {
                        ai.alpha[k] = self.eval(i, c).to_i().clamp(0, 255);
                    }
                }
                if let Some(c) = &spec.ignorehitpause {
                    ai.ignorehitpause = self.eval(i, c).to_b();
                }
                ai.setup();
                self.players[i].afterimage = Some(Box::new(ai));
            }
            Sctrl::AfterImageTime { time } => {
                let mut t = time.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                if t == 1 {
                    t = 0;
                }
                if let Some(ai) = self.players[i].afterimage.as_mut()
                    && ai.timegap > 0
                {
                    ai.time = t;
                    ai.timecount = 0;
                }
            }
            Sctrl::Explod(spec) => {
                self.spawn_explod(i, spec);
            }
            Sctrl::Projectile(spec) => {
                self.spawn_projectile(i, spec);
            }
            Sctrl::Helper {
                stateno,
                id,
                pos,
                postype,
                facing,
                keyctrl,
                player_type,
                supermovetime,
                pausemovetime,
            } => {
                let st = stateno.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                let hid = id.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                let px = pos
                    .as_ref()
                    .and_then(|p| p.first())
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let py = pos
                    .as_ref()
                    .and_then(|p| p.get(1))
                    .map_or(0.0, |c| self.eval(i, c).to_f());
                let f = facing.as_ref().map_or(1, |c| self.eval(i, c).to_i());
                let kc = keyctrl.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                let smt = supermovetime.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                let pmt = pausemovetime.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                let pt = postype.unwrap_or(PosType::P1);
                if let Some(h) = self.spawn_helper(i, st, hid, [px, py], pt, f, kc, *player_type) {
                    self.players[h].super_move_time = smt;
                    self.players[h].pause_move_time = pmt;
                }
            }
            Sctrl::DestroySelf {
                recursive,
                removeexplods,
            } => {
                let rec = recursive.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                let rem = removeexplods
                    .as_ref()
                    .is_some_and(|c| self.eval(i, c).to_b());
                if self.players[i].helper_index > 0 {
                    self.destroy_helper(i, rec, rem);
                    return true;
                }
            }
            Sctrl::ModifyExplod { id, spec } => {
                let eid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let owner_id = self.players[i].id;
                let idxs: Vec<usize> = self
                    .explods
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| {
                        !e.removed
                            && e.owner_id == owner_id
                            && e.id >= 0
                            && (eid < 0 || e.id == eid)
                    })
                    .map(|(k, _)| k)
                    .collect();
                for k in idxs {
                    let mut e = self.explods[k].clone();
                    self.apply_explod_spec(i, &mut e, spec, false);
                    if spec.postype.is_some() || spec.pos.is_some() {
                        self.explod_set_pos(&mut e, i);
                    }
                    self.explods[k] = e;
                }
            }
            Sctrl::RemoveExplod { id } => {
                let eid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let owner_id = self.players[i].id;
                self.remove_explods(owner_id, eid);
            }
            Sctrl::ExplodBindTime { id, time } => {
                let eid = id.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let t = time.as_ref().map_or(1, |c| self.eval(i, c).to_i());
                let owner_id = self.players[i].id;
                for e in &mut self.explods {
                    if e.owner_id == owner_id && e.id >= 0 && (eid < 0 || e.id == eid) {
                        e.bindtime = t;
                    }
                }
            }
            Sctrl::Pause { time, movetime } => {
                let t = self.eval(i, time).to_i();
                let mt = movetime.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                self.set_pause_time(i, t, mt);
            }
            Sctrl::SuperPause {
                time,
                movetime,
                anim,
                pos,
                sound,
                darken,
                p2defmul,
                poweradd,
                unhittable,
            } => {
                let t = time.as_ref().map_or(30, |c| self.eval(i, c).to_i());
                let mt = movetime.as_ref().map_or(0, |c| self.eval(i, c).to_i());
                self.superbrightness = if darken.as_ref().is_none_or(|c| self.eval(i, c).to_b()) {
                    0.5
                } else {
                    1.0
                };
                let p2d = p2defmul.as_ref().map_or(1.5, |c| self.eval(i, c).to_f());
                let p2d = if p2d > 0.0 { p2d } else { 1.5 };
                if let Some(c) = poweradd {
                    let v = self.eval(i, c).to_i();
                    self.power_add(i, v);
                }
                let uh = unhittable.as_ref().is_none_or(|c| self.eval(i, c).to_b());
                if let Some((fx, codes)) = sound
                    && codes.len() >= 2
                {
                    let g = self.eval(i, &codes[0]).to_i();
                    let n = self.eval(i, &codes[1]).to_i();
                    self.push_sound(Event::Sound {
                        player: i,
                        own: !fx,
                        group: g,
                        number: n,
                        channel: -1,
                        volume: 100,
                    });
                }
                // the flash: fightfx 100 by default
                let (fx, anim_no) = match anim {
                    Some((fx, c)) => (*fx, self.eval(i, c).to_i()),
                    None => (true, 100),
                };
                if anim_no >= 0 {
                    let px = pos
                        .as_ref()
                        .and_then(|p| p.first())
                        .map_or(0.0, |c| self.eval(i, c).to_f());
                    let py = pos
                        .as_ref()
                        .and_then(|p| p.get(1))
                        .map_or(0.0, |c| self.eval(i, c).to_f());
                    let ls = self.local_scale(i);
                    let p = &self.players[i];
                    let stage_pos = [p.pos[0] * ls + px * ls * p.facing, p.pos[1] * ls + py * ls];
                    let f = p.facing;
                    self.spawn_spark(i, !fx, anim_no, stage_pos, f);
                }
                self.set_super_pause_time(i, t, mt, uh, p2d);
            }
            Sctrl::VelSet { x, y } => {
                if let Some(v) = self.evf(i, x) {
                    self.players[i].vel[0] = v;
                }
                if let Some(v) = self.evf(i, y) {
                    self.players[i].vel[1] = v;
                }
            }
            Sctrl::VelAdd { x, y } => {
                if let Some(v) = self.evf(i, x) {
                    self.players[i].vel[0] += v;
                }
                if let Some(v) = self.evf(i, y) {
                    self.players[i].vel[1] += v;
                }
            }
            Sctrl::VelMul { x, y } => {
                if let Some(v) = self.evf(i, x) {
                    self.players[i].vel[0] *= v;
                }
                if let Some(v) = self.evf(i, y) {
                    self.players[i].vel[1] *= v;
                }
            }
            Sctrl::PosAdd { x, y } => {
                if let Some(v) = self.evf(i, x) {
                    let p = &mut self.players[i];
                    p.pos[0] += v * p.facing;
                }
                if let Some(v) = self.evf(i, y) {
                    self.players[i].pos[1] += v;
                }
            }
            Sctrl::PosSet { x, y } => {
                if let Some(v) = self.evf(i, x) {
                    let cx = self.cam_x;
                    let p = &mut self.players[i];
                    p.pos[0] = cx + v;
                    p.old_pos[0] = p.pos[0];
                }
                if let Some(v) = self.evf(i, y) {
                    let p = &mut self.players[i];
                    p.pos[1] = p.ground + v;
                    p.old_pos[1] = p.pos[1];
                }
            }
            Sctrl::PosFreeze { value } => {
                let v = value.as_ref().is_none_or(|c| self.eval(i, c).to_b());
                self.players[i].pos_freeze = v;
            }
            Sctrl::CtrlSet { value } => {
                let v = self.eval(i, value).to_b();
                self.players[i].ctrl = v;
            }
            Sctrl::StateTypeSet {
                statetype,
                movetype,
                physics,
            } => {
                let p = &mut self.players[i];
                match statetype {
                    Some('s') => p.state_type = StateType::S,
                    Some('c') => p.state_type = StateType::C,
                    Some('a') => p.state_type = StateType::A,
                    Some('l') => p.state_type = StateType::L,
                    _ => {}
                }
                match movetype {
                    Some('i') => p.move_type = MoveType::I,
                    Some('a') => p.move_type = MoveType::A,
                    Some('h') => p.move_type = MoveType::H,
                    _ => {}
                }
                match physics {
                    Some('s') => p.physics = Physics::S,
                    Some('c') => p.physics = Physics::C,
                    Some('a') => p.physics = Physics::A,
                    Some('n') => p.physics = Physics::N,
                    _ => {}
                }
            }
            Sctrl::VarSet { kind, index, value } | Sctrl::VarAdd { kind, index, value } => {
                let idx = self.eval(i, index).to_i();
                let v = self.eval(i, value);
                let add = matches!(op, Sctrl::VarAdd { .. });
                let mut ctx = Ctx::new(self, i);
                let cur = if add {
                    ctx.get_var(*kind, idx)
                } else {
                    Value::Int(0)
                };
                let nv = if add { cur.add(v) } else { v };
                ctx.set_var(*kind, idx, nv);
            }
            Sctrl::VarRangeSet {
                float,
                value,
                first,
                last,
            } => {
                let v = self.eval(i, value);
                let (lo, hi) = (
                    first.as_ref().map_or(0, |c| self.eval(i, c).to_i()),
                    last.as_ref()
                        .map_or(if *float { 39 } else { 59 }, |c| self.eval(i, c).to_i()),
                );
                let p = &mut self.players[i];
                for k in lo.max(0)..=hi {
                    if *float {
                        if let Some(s) = p.fvars.get_mut(k as usize) {
                            *s = v.to_f();
                        }
                    } else if let Some(s) = p.vars.get_mut(k as usize) {
                        *s = v.to_i();
                    }
                }
            }
            Sctrl::Turn => {
                let f = -self.players[i].facing;
                self.players[i].set_facing(f);
            }
            Sctrl::PlaySnd {
                own,
                group,
                number,
                channel,
                volume,
            } => {
                let g = self.eval(i, group).to_i();
                let n = self.eval(i, number).to_i();
                let ch = channel.as_ref().map_or(-1, |c| self.eval(i, c).to_i());
                let vol = volume.as_ref().map_or(100, |c| self.eval(i, c).to_i());
                self.push_sound(Event::Sound {
                    player: i,
                    own: *own,
                    group: g,
                    number: n,
                    channel: ch,
                    volume: vol,
                });
            }
            Sctrl::SprPriority { value } => {
                let v = self.eval(i, value).to_i();
                self.players[i].spr_priority = v;
            }
            Sctrl::AssertSpecial { flags } => {
                for f in flags {
                    if let Some(bit) = assert::parse(f) {
                        self.players[i].assert_flags |= bit;
                    }
                }
            }
            Sctrl::Width {
                edge,
                player,
                value,
            } => {
                let two = |w: &mut World, v: &Option<Vec<Code>>| -> Option<[f32; 2]> {
                    let v = v.as_ref()?;
                    let a = v.first().map_or(0.0, |c| w.eval(i, c).to_f());
                    let b = v.get(1).map_or(0.0, |c| w.eval(i, c).to_f());
                    Some([a, b])
                };
                if let Some(v) = two(self, value) {
                    self.players[i].width_edge = v;
                    self.players[i].width_player = v;
                }
                if let Some(v) = two(self, edge) {
                    self.players[i].width_edge = v;
                }
                if let Some(v) = two(self, player) {
                    self.players[i].width_player = v;
                }
            }
            Sctrl::ScreenBound { value, movecamera } => {
                let v = value.as_ref().is_some_and(|c| self.eval(i, c).to_b());
                self.players[i].screen_bound = v;
                if let Some(mc) = movecamera {
                    let x = mc.first().is_some_and(|c| self.eval(i, c).to_b());
                    let y = mc.get(1).is_some_and(|c| self.eval(i, c).to_b());
                    self.players[i].move_camera = [x, y];
                } else {
                    self.players[i].move_camera = [false, false];
                }
            }
            Sctrl::PowerAdd { value } => {
                let v = self.eval(i, value).to_i();
                self.power_add(i, v);
            }
            Sctrl::PowerSet { value } => {
                let v = self.eval(i, value).to_i();
                let p = &mut self.players[i];
                p.power = v.clamp(0, p.power_max);
            }
            Sctrl::LifeAdd { value, kill, .. } => {
                let v = self.eval(i, value).to_i();
                let kill = kill.as_ref().is_none_or(|c| self.eval(i, c).to_b());
                let p = &mut self.players[i];
                p.life = p
                    .life
                    .saturating_add(v)
                    .min(p.life_max)
                    .max(if kill { 0 } else { 1 });
            }
            Sctrl::LifeSet { value } => {
                let v = self.eval(i, value).to_i();
                let p = &mut self.players[i];
                p.life = v.clamp(0, p.life_max);
            }
            Sctrl::Gravity => {
                let y = self.data[self.players[i].data]
                    .konst("movement.yaccel")
                    .unwrap_or(0.44);
                self.players[i].vel[1] += y;
            }
        }
        false
    }

    /// Corner push: when the opponent we hit is stuck on the screen edge, we move back instead.
    fn corner_push(&self, i: usize) -> f32 {
        let p = &self.players[i];
        if p.mhv.cornerpush_veloff == 0.0 {
            return 0.0;
        }
        let (xmin, xmax) = self.play_bounds();
        for o in &self.players {
            if o.move_type != MoveType::H || o.ghv.playerid != p.id {
                continue;
            }
            if o.ghv.hitshaketime > 0 {
                continue;
            }
            let next = o.pos[0] + o.vel[0] * o.facing;
            if next > xmin && next < xmax || !o.screen_bound {
                continue;
            }
            let mut push = p.mhv.cornerpush_veloff;
            let mut mv = next - o.pos[0];
            if next > xmax {
                mv = xmax - o.pos[0];
            } else if next < xmin {
                mv = xmin - o.pos[0];
            }
            if mv.abs() < push.abs() {
                push -= push.signum() * mv.abs();
            } else {
                push = 0.0;
            }
            return push;
        }
        0.0
    }

    /// Returns true when the player just walked off a platform edge.
    fn pos_update(&mut self, i: usize) -> bool {
        let data = self.data[self.players[i].data].clone();
        let push = self.corner_push(i);
        let bound = self.players[i].bind.is_some();
        if bound {
            self.apply_bind(i);
        }
        let p = &mut self.players[i];
        p.old_pos = p.pos;
        if p.down_hit_offset {
            let ox = data.konst("movement.down.bounce.offset.x").unwrap_or(0.0);
            let oy = data.konst("movement.down.bounce.offset.y").unwrap_or(20.0);
            p.pos[0] += ox * p.facing;
            p.pos[1] += oy;
            p.down_hit_offset = false;
        }
        if p.move_type == MoveType::H && (p.ghv.xoff != 0.0 || p.ghv.yoff != 0.0) {
            p.pos[0] += p.ghv.xoff;
            p.pos[1] += p.ghv.yoff;
            p.ghv.xoff = 0.0;
            p.ghv.yoff = 0.0;
        }
        if p.pos_freeze {
            if !bound {
                p.pos[0] = p.old_pos[0] + push;
            }
        } else if !bound {
            p.pos[0] += p.vel[0] * p.facing + push;
            p.pos[1] += p.vel[1];
        }
        let fell_off = self.update_ground(i);
        let p = &mut self.players[i];
        match p.physics {
            Physics::S => {
                p.vel[0] *= data.konst("movement.stand.friction").unwrap_or(0.85);
                if p.vel[0].abs() < 1.0 {
                    p.vel[0] = 0.0;
                }
            }
            Physics::C => {
                p.vel[0] *= data.konst("movement.crouch.friction").unwrap_or(0.82);
            }
            Physics::A => {
                p.vel[1] += data.konst("movement.yaccel").unwrap_or(0.44);
            }
            Physics::N | Physics::U => {}
        }
        if p.mhv.cornerpush_veloff != 0.0 {
            let fr = if matches!(p.state_type, StateType::C | StateType::L) {
                data.konst("movement.crouch.friction").unwrap_or(0.82)
            } else {
                data.konst("movement.stand.friction").unwrap_or(0.85)
            };
            p.mhv.cornerpush_veloff *= fr;
            if p.mhv.cornerpush_veloff.abs() < 1.0 {
                p.mhv.cornerpush_veloff = 0.0;
            }
        }
        fell_off
    }

    /// Air dash (double tap forward/back) and fast fall (hold down), engine side.
    fn engine_air_moves(&mut self, i: usize, data: &CharData, fb: i32, bb: i32, db: i32) {
        let frame = self.frame;
        let mv = self.movement;
        let p = &mut self.players[i];
        // remember tap times for the double-tap window
        if fb == 1 {
            let prev = p.tap_tick[0];
            p.tap_tick[0] = frame + 1;
            p.tap_pending[0] = prev != 0 && frame + 1 - prev <= 12;
        }
        if bb == 1 {
            let prev = p.tap_tick[1];
            p.tap_tick[1] = frame + 1;
            p.tap_pending[1] = prev != 0 && frame + 1 - prev <= 12;
        }
        let airborne = p.state_type == StateType::A && p.ctrl && p.move_type != MoveType::H;
        if mv.air_dash && airborne && !p.air_dash_used && (fb == 1 || bb == 1) {
            let dir = if fb == 1 { 0 } else { 1 };
            if p.tap_pending[dir] {
                let speed = data
                    .konst("velocity.run.fwd.x")
                    .unwrap_or(4.6)
                    .abs()
                    .max(2.0);
                p.vel[0] = if dir == 0 { speed } else { -speed };
                p.vel[1] = 0.0;
                p.air_dash_used = true;
                p.air_dash_time = 10;
                p.tap_pending = [false, false];
            }
        }
        if p.air_dash_time > 0 {
            p.air_dash_time -= 1;
            // hover through the dash, then let the character fall again
            if p.state_type == StateType::A {
                p.vel[1] = p.vel[1].min(0.0);
                if p.air_dash_time == 0 {
                    p.vel[0] *= 0.35;
                }
            } else {
                p.air_dash_time = 0;
            }
        }
        if mv.fast_fall && airborne && db > 0 && p.vel[1] > 0.0 && p.air_dash_time == 0 {
            let fast = data.konst("movement.yaccel").unwrap_or(0.44) * 18.0;
            p.vel[1] = p.vel[1].max(fast);
        }
    }

    /// Ring-outs: a root player past the arena blast zone loses a stock
    /// (respawn) or is KO'd. Runs after the bounds phase.
    fn check_blastzone(&mut self) {
        let Some(bz) = self.stage.arena.blastzone else {
            return;
        };
        if self.round.round_state() != 2 {
            return;
        }
        for i in 0..self.players.len() {
            let p = &self.players[i];
            if p.removed || p.helper_index != 0 || !p.alive() {
                continue;
            }
            let s = self.local_scale(i);
            let (x, y) = (p.pos[0] * s, p.pos[1] * s);
            if x >= bz[0] && x <= bz[2] && y >= bz[1] && y <= bz[3] {
                continue;
            }
            // pull the body back to the edge so the KO / explosion is on screen
            let cx = x.clamp(bz[0], bz[2]) / s;
            let cy = y.clamp(bz[1], bz[3]) / s;
            self.players[i].pos = [cx, cy];
            self.players[i].old_pos = [cx, cy];
            self.players[i].vel = [0.0, 0.0];
            self.spawn_fx_anim(i, 2, [0.0, 0.0], true);
            self.push_sound(Event::Sound {
                player: i,
                own: false,
                group: 5,
                number: 2,
                channel: -1,
                volume: 100,
            });
            if self.players[i].stocks > 1 {
                self.players[i].stocks -= 1;
                self.respawn(i);
            } else {
                let p = &mut self.players[i];
                p.stocks = 0;
                p.life = 0;
            }
        }
    }

    /// Put a player back at its start position above the stage with full life.
    fn respawn(&mut self, i: usize) {
        let s = self.local_scale(i);
        let (x, facing) = if self.players[i].team == 0 {
            (self.stage.p1_start_x, 1.0)
        } else {
            (self.stage.p2_start_x, -1.0)
        };
        let spawn_y = self.stage.arena.spawn_y;
        let p = &mut self.players[i];
        p.pos = [x / s, spawn_y / s];
        p.old_pos = p.pos;
        p.vel = [0.0, 0.0];
        p.facing = facing;
        p.life = p.life_max;
        p.ground = 0.0;
        p.ghv = Default::default();
        p.hitdef = None;
        p.targets.clear();
        p.hit_pause_time = 0;
        p.move_type = MoveType::I;
        p.unhittable_time = 90;
        p.air_jump_count = 0;
        p.air_dash_used = false;
        p.air_dash_time = 0;
        p.state_owner = p.data;
        self.change_state(i, 50, None, Some(true));
        self.run_current(i);
    }

    /// Platforms: land on a surface crossed from above this tick, keep the
    /// surface while standing within its span, fall off when leaving it.
    /// Queue a sound event unless this tick already hit the PlaySnd cap.
    pub(crate) fn push_sound(&mut self, ev: Event) {
        let n = self
            .events
            .iter()
            .filter(|e| matches!(e, Event::Sound { .. }))
            .count();
        if n < self.limits.sounds_per_tick {
            self.events.push(ev);
        }
    }

    /// Replace NaN / infinite positions and velocities written by content with
    /// zero, so one bad expression cannot poison the match.
    fn sanitize(&mut self, i: usize) {
        let p = &mut self.players[i];
        let mut bad = false;
        for v in p.pos.iter_mut().chain(p.vel.iter_mut()) {
            if !v.is_finite() {
                *v = 0.0;
                bad = true;
            }
        }
        if !p.ground.is_finite() {
            p.ground = 0.0;
            bad = true;
        }
        if bad {
            self.warn(format!(
                "P{} produced a non-finite position or velocity",
                i + 1
            ));
        }
    }

    /// `step` guarded against content panics: if a player's states panic, the
    /// world is rolled back to the pre-step snapshot, that player (and its
    /// helpers) is marked `broken` (never runs again, life 0) and the match
    /// continues. Returns the blamed root player index when a panic was caught.
    pub fn step_checked(&mut self, inputs: &[Buttons]) -> Option<usize> {
        let snapshot = self.clone();
        let inputs: Vec<Buttons> = inputs.to_vec();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.step(&inputs);
        }));
        match result {
            Ok(()) => None,
            Err(_) => {
                let culprit = self
                    .running
                    .or_else(|| self.players.iter().position(|p| !p.removed && !p.broken));
                *self = snapshot;
                self.running = None;
                let root = culprit.map(|c| self.players.get(c).map_or(c, |p| p.root));
                if let Some(root) = root {
                    let name = self
                        .players
                        .get(root)
                        .and_then(|p| self.data.get(p.data))
                        .map(|d| d.name.clone())
                        .unwrap_or_default();
                    self.warn(format!(
                        "P{} ({name}) crashed the simulation and was disabled",
                        root + 1
                    ));
                    for p in &mut self.players {
                        if p.root == root {
                            p.broken = true;
                            p.life = 0;
                            p.ctrl = false;
                        }
                    }
                }
                root
            }
        }
    }

    fn update_ground(&mut self, i: usize) -> bool {
        if self.stage.platforms.is_empty() && self.stage.arena.floor.is_none() {
            return false;
        }
        // normal helpers (projectiles, effects, trackers) keep MUGEN's flat
        // ground: pits and platforms are for the fighters
        if !self.players[i].player_type {
            self.players[i].ground = 0.0;
            return false;
        }
        let (x, old_y, new_y, vy, ground) = {
            let p = &self.players[i];
            (p.pos[0], p.old_pos[1], p.pos[1], p.vel[1], p.ground)
        };
        let scale = self.local_scale(i);
        let mut landed: Option<f32> = None;
        // the floor exists only inside the arena's span (a pit elsewhere)
        let on_floor_span = self
            .stage
            .arena
            .floor
            .is_none_or(|f| x * scale >= f[0] && x * scale <= f[1]);
        let mut below: f32 = if on_floor_span { 0.0 } else { NO_GROUND };
        if on_floor_span && vy >= 0.0 && old_y <= 0.01 && new_y >= 0.0 && ground >= NO_GROUND {
            landed = Some(0.0);
        }
        // the surface already stood on, kept while inside its span even when the
        // feet sink below it (as on the floor, where Pos Y may go positive: the
        // common fall states wait for `Pos Y >= 25` before bouncing)
        let mut keep: Option<f32> = None;
        for pl in &self.stage.platforms {
            let (x1, x2, y) = (pl.x1 / scale, pl.x2 / scale, pl.y / scale);
            if x < x1 || x > x2 {
                continue;
            }
            if ground == y {
                keep = Some(y);
                continue;
            }
            // crossed the surface from above while falling
            if vy >= 0.0 && old_y <= y + 0.01 && new_y >= y && landed.is_none_or(|l| y < l) {
                landed = Some(y);
            }
            // highest surface at or below the feet (for walk-offs)
            if y >= new_y - 0.01 && y < below {
                below = y;
            }
        }
        if let Some(y) = landed.filter(|&l| keep.is_none_or(|k| l < k)) {
            let p = &mut self.players[i];
            p.pos[1] = y;
            p.ground = y;
            return false;
        }
        let p = &mut self.players[i];
        p.ground = match keep {
            Some(k) if new_y >= k - 0.01 || k <= below => k,
            _ => below,
        };
        // walked off the surface it stood on (the ground under the feet dropped
        // away this tick): the caller drops the player into the air-fall state.
        // A body content placed in the air (PosSet, binds) is left alone.
        if p.physics != Physics::A
            && p.state_type != StateType::A
            && p.move_type != MoveType::H
            && p.bind.is_none()
            && p.ground > ground
            && old_y >= ground - 0.5
            && p.pos[1] < p.ground - 0.5
        {
            p.vel[1] = 0.0;
            return true;
        }
        false
    }

    /// MUGEN camera: the view only moves when a player crosses the tension
    /// zone near a screen edge; it zooms out (down to `zoomout`) when players
    /// drift apart and back in (up to `zoomin`) when they close.
    pub(crate) fn update_camera(&mut self) {
        let st = &self.stage;
        let (mut leftest, mut rightest, mut highest, mut lowest) =
            (f32::MAX, f32::MIN, 0.0f32, 0.0f32);
        for p in self
            .players
            .iter()
            .filter(|p| !p.removed && p.helper_index == 0 && p.player_type)
        {
            leftest = leftest.min(p.pos[0]);
            rightest = rightest.max(p.pos[0]);
            highest = highest.min(p.pos[1]);
            lowest = lowest.max(p.pos[1].min(500.0));
        }
        if leftest > rightest {
            return;
        }
        let half = st.width / 2.0;
        let tension = st.tension.max(0.0);
        let min_left = st.bound_left - half;
        let max_right = st.bound_right + half;
        let scale = self.cam_zoom;
        let (old_left, old_right) = (self.cam_x - half / scale, self.cam_x + half / scale);
        let (mut tl, mut tr) = (old_left, old_right);
        if leftest < tl + tension {
            let nl = (leftest - tension).max(min_left);
            let diff = tl - nl;
            tl = nl;
            tr = (old_right - diff).max((rightest + tension).min(max_right));
        } else if rightest > tr - tension {
            let nr = (rightest + tension).min(max_right);
            let diff = tr - nr;
            tr = nr;
            tl = (old_left - diff).min((leftest - tension).max(min_left));
        }
        // too wide for the zoom-out limit: pull the edges back in
        if half * 2.0 / (tr - tl) < st.zoom_out {
            let r_left = (tl + tension - leftest).max(0.0);
            let r_right = (rightest - (tr - tension)).max(0.0);
            let mut diff = 2.0 * ((tr - tl) / 2.0 - half / st.zoom_out);
            if r_left > r_right {
                let d2 = (r_left - r_right).min(diff);
                tr -= d2;
                diff -= d2;
            } else if r_right > r_left {
                let d2 = (r_right - r_left).min(diff);
                tl += d2;
                diff -= d2;
            }
            tl += diff / 2.0;
            tr -= diff / 2.0;
        }
        // room to zoom in: tighten the edges toward the players
        let max_scale = st.zoom_in;
        if half * 2.0 / (tr - tl) < max_scale {
            let mut dl = (leftest - tension - tl).max(0.0);
            let mut dr = (rightest + tension - tr).min(0.0);
            if dl - dr > 0.0 && half * 2.0 / ((tr + dr) - (tl + dl)) > max_scale {
                let span = (tr + dr) - (tl + dl) - half * 2.0 / max_scale;
                let t1 = dl / (dl - dr) * span;
                let t2 = dr / (dl - dr) * span;
                dl += t1;
                dr += t2;
            }
            tl += dl;
            tr += dr;
        }
        let target_x = (tl + tr) / 2.0;
        let mut target_scale = (half * 2.0 / (tr - tl)).clamp(st.zoom_out, max_scale);
        // vertical: zoom out when the highest player would leave the top even
        // with the camera raised to boundhigh, or the lowest one falls below the floor
        let vmargin = 40.0;
        let top_need = st.bound_high.min(st.bound_low) - highest + vmargin;
        if top_need > 0.0 {
            target_scale = target_scale.min(st.zoffset / top_need);
        }
        let bottom_room = (st.height - st.zoffset).max(1.0);
        if lowest + vmargin > bottom_room {
            target_scale = target_scale.min(bottom_room / (lowest + vmargin));
        }
        let target_scale = target_scale.max(st.zoom_out);
        // smooth zoom, immediate pan
        let new_scale = if (target_scale - scale).abs() < 0.0005 {
            target_scale
        } else {
            scale + (target_scale - scale) * 0.1
        };
        let hw = half / new_scale;
        self.cam_zoom = new_scale;
        self.cam_x = target_x.clamp(min_left + hw, (max_right - hw).max(min_left + hw));
        // vertical follow toward the highest player; never let it leave the top
        let follow = (highest + st.floor_tension).min(0.0) * st.vertical_follow;
        let keep_visible = highest + st.zoffset / new_scale - vmargin;
        let target_y = follow
            .min(keep_visible)
            .clamp(st.bound_high.min(st.bound_low), st.bound_low);
        self.cam_y += (target_y - self.cam_y) * 0.2;
        if (target_y - self.cam_y).abs() < 0.01 {
            self.cam_y = target_y;
        }
    }

    pub(crate) fn screen_bound(&mut self, i: usize) {
        let (xmin, xmax) = self.play_bounds();
        let p = &self.players[i];
        let mut x = p.pos[0];
        if p.screen_bound && p.player_type && self.stage.arena.walls {
            let (mut lo, mut hi) = (p.width_edge[0], -p.width_edge[1]);
            if p.facing > 0.0 {
                (lo, hi) = (-hi, -lo);
            }
            x = x.clamp(lo + xmin, hi + xmax);
        }
        x = x.clamp(self.stage.left_bound, self.stage.right_bound);
        let p = &mut self.players[i];
        if x != p.pos[0] {
            p.pos[0] = x;
            p.old_pos[0] = x;
        }
    }
}

/// Damage scaled like a hit (`TargetLifeAdd` with absolute = 0).
fn collision_damage(w: &World, getter: usize, attacker: usize, damage: i32, kill: bool) -> i32 {
    let a = &w.players[attacker];
    let g = &w.players[getter];
    let atk = a.attack_mul * w.data[a.data].konst("data.attack").unwrap_or(100.0) / 100.0;
    let def = w.data[g.data].konst("data.defence").unwrap_or(100.0) / 100.0 * g.defence_mul;
    let mut d = damage as f32 * atk / def.max(0.0001);
    if d > 0.0 && d < 1.0 {
        d = 1.0;
    }
    if !kill && d >= g.life as f32 {
        d = (g.life - 1) as f32;
    }
    libm::roundf(d) as i32
}

impl World {
    /// Hash of the gameplay state, for desync detection.
    pub fn checksum(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        mix(self.frame as u64);
        mix(self.rng.state());
        mix(self.cam_x.to_bits() as u64);
        mix(self.cam_zoom.to_bits() as u64);
        for p in &self.players {
            mix(p.pos[0].to_bits() as u64);
            mix(p.pos[1].to_bits() as u64);
            mix(p.vel[0].to_bits() as u64);
            mix(p.state_no as u64);
            mix(p.state_time as u64);
            mix(p.anim.action as u64);
            mix(p.anim.elem as u64);
            mix(p.life as u64);
            mix(p.power as u64);
            for v in &p.vars {
                mix(*v as u64);
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Finish, RoundConfig, RoundState};
    use std::path::PathBuf;

    fn kfm() -> Option<Arc<CharData>> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1/chars/kfm/kfm.def");
        if !p.exists() {
            return None;
        }
        let d = CharData::load(&p).unwrap_or_else(|e| panic!("load kfm: {e}"));
        for w in &d.warnings {
            eprintln!("warning: {w}");
        }
        Some(d)
    }

    fn world() -> Option<World> {
        let d = kfm()?;
        let mut w = World::new(Stage::default(), 1);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        Some(w)
    }

    fn run(w: &mut World, n: usize, p1: Buttons) {
        for _ in 0..n {
            w.step(&[p1, Buttons::default()]);
        }
    }

    #[test]
    fn loads_with_states() {
        let Some(d) = kfm() else { return };
        assert_eq!(d.name, "Kung Fu Man");
        assert!(d.states.contains_key(&0), "state 0 from common1");
        assert!(d.states.contains_key(&200), "state 200 from kfm.cns");
        assert!(d.states.contains_key(&-1), "state -1 from kfm.cmd");
        assert_eq!(d.konst("velocity.walk.fwd.x"), Some(2.4));
        assert_eq!(d.konst("data.life"), Some(1000.0));
        assert_eq!(d.konst("movement.yaccel"), Some(0.44));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    }

    #[test]
    fn idle_stays_in_state_zero() {
        let Some(mut w) = world() else { return };
        run(&mut w, 120, Buttons::default());
        let p = &w.players[0];
        assert_eq!(p.state_no, 0);
        assert_eq!(p.anim.action, 0);
        assert!(p.ctrl);
        assert_eq!(p.pos[1], 0.0);
        assert_eq!(w.players[1].facing, -1.0);
        assert!(w.warnings.is_empty(), "{:?}", w.warnings);
    }

    #[test]
    fn walking() {
        let Some(mut w) = world() else { return };
        run(&mut w, 5, Buttons::default());
        let x0 = w.players[0].pos[0];
        run(
            &mut w,
            30,
            Buttons {
                r: true,
                ..Default::default()
            },
        );
        let p = &w.players[0];
        assert_eq!(p.state_no, 20, "walk state");
        assert_eq!(p.anim.action, 20);
        assert!(
            p.pos[0] > x0 + 50.0,
            "moved forward: {} -> {}",
            x0,
            p.pos[0]
        );
        assert!(
            (p.vel[0] - 2.4 * 0.85).abs() < 1e-4,
            "VelSet then stand friction, like MUGEN: {}",
            p.vel[0]
        );
        run(&mut w, 1, Buttons::default());
        assert_eq!(w.players[0].state_no, 0, "released: back to stand");
        let x1 = w.players[0].pos[0];
        run(
            &mut w,
            30,
            Buttons {
                l: true,
                ..Default::default()
            },
        );
        assert!(w.players[0].pos[0] < x1, "walked back");
        assert_eq!(w.players[0].anim.action, 21);
    }

    #[test]
    fn light_punch() {
        let Some(mut w) = world() else { return };
        run(&mut w, 5, Buttons::default());
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        let p = &w.players[0];
        assert_eq!(p.state_no, 200, "x → standing light punch");
        assert_eq!(p.anim.action, 200);
        assert!(!p.ctrl);
        run(&mut w, 60, Buttons::default());
        assert_eq!(w.players[0].state_no, 0, "attack finished");
        assert!(w.players[0].ctrl);
    }

    /// A platform above the floor: jump onto it, stand on it, walk off and fall.
    #[test]
    fn platform_landing_and_walk_off() {
        let Some(mut w) = world() else { return };
        w.stage.platforms.push(Platform {
            x1: -120.0,
            x2: -20.0,
            y: -40.0,
        });
        run(&mut w, 5, Buttons::default());
        assert_eq!(w.players[0].pos[0], -70.0);
        run(
            &mut w,
            1,
            Buttons {
                u: true,
                ..Default::default()
            },
        );
        let mut landed = false;
        for _ in 0..120 {
            w.step(&[Buttons::default(), Buttons::default()]);
            if w.players[0].state_no == 0 && w.players[0].ground < 0.0 {
                landed = true;
                break;
            }
        }
        assert!(landed, "should land on the platform");
        let p = &w.players[0];
        assert_eq!(p.pos[1], -40.0);
        assert_eq!(p.ground, -40.0);
        // Pos Y reads relative to the surface
        let mut ctx = Ctx::new(&mut w, 0);
        assert_eq!(
            ctx.trigger(&rugen_cns::Trigger::Pos(rugen_cns::trigger::Axis::Y), &[]),
            Value::Float(0.0)
        );
        // walk right past the platform's edge and fall to the floor
        let fwd = Buttons {
            r: true,
            ..Default::default()
        };
        let mut fell = false;
        for t in 0..200 {
            w.step(&[fwd, Buttons::default()]);
            if std::env::var_os("RUGEN_TEST_TRACE").is_some() {
                let p = &w.players[0];
                eprintln!(
                    "t={t} state {} type {:?} phys {:?} pos {:?} vel {:?} ground {} ctrl {}",
                    p.state_no, p.state_type, p.physics, p.pos, p.vel, p.ground, p.ctrl
                );
            }
            if w.players[0].ground == 0.0
                && w.players[0].pos[1] == 0.0
                && w.players[0].pos[0] > -20.0
            {
                fell = true;
                break;
            }
        }
        assert!(fell, "should fall off the edge, pos {:?}", w.players[0].pos);
    }

    /// A fall that waits for `Pos Y >= 25` (common1's 5050 without a 5060
    /// anim) must still bounce on a platform: the feet sink below the surface
    /// as they do below the floor, instead of being clamped onto it every tick.
    #[test]
    fn platform_fall_reaches_bounce_threshold() {
        let Some(mut w) = world() else { return };
        w.stage.platforms.push(Platform {
            x1: -200.0,
            x2: 0.0,
            y: -80.0,
        });
        let text = "[Statedef 777]\ntype = A\nmovetype = H\nphysics = N\nctrl = 0\n\n\
                    [State 777, g]\ntype = VelAdd\ntrigger1 = 1\ny = 0.44\n\n\
                    [State 777, land]\ntype = ChangeState\n\
                    trigger1 = Vel Y > 0 && Pos Y >= 25\nvalue = 778\n\n\
                    [Statedef 778]\ntype = L\nmovetype = H\nphysics = N\nvelset = 0,0\nctrl = 0\n\n\
                    [State 778, set]\ntype = PosSet\ntrigger1 = 1\ny = 0\n";
        let d = w.data[0].with_states("test", text).expect("states compile");
        w.data[0] = d;
        run(&mut w, 5, Buttons::default());
        w.players[0].pos = [-70.0, -80.0];
        w.players[0].old_pos = w.players[0].pos;
        w.players[0].ground = -80.0;
        run(&mut w, 2, Buttons::default());
        w.players[0].vel = [0.0, -4.0];
        w.change_state(0, 777, None, Some(false));
        let mut bounced = false;
        for _ in 0..120 {
            w.step(&[Buttons::default(), Buttons::default()]);
            if w.players[0].state_no == 778 {
                bounced = true;
                break;
            }
        }
        let p = &w.players[0];
        assert!(
            bounced,
            "stuck falling: state {} pos {:?} vel {:?} ground {}",
            p.state_no, p.pos, p.vel, p.ground
        );
        w.step(&[Buttons::default(), Buttons::default()]);
        let p = &w.players[0];
        assert_eq!(p.ground, -80.0, "still on the platform");
        assert_eq!(p.pos[1], -80.0, "PosSet y = 0 is relative to the platform");
    }

    /// Arena pits / platforms only drop fighters that walk off their surface.
    /// G6 Sasuke's thrown kunai (a normal helper flying with physics S) left the
    /// island, was forced into state 50 and never destroyed itself, so his throw
    /// state (which waits for the kunai) never ended; a stand-type state held
    /// in the air by PosSet (his Susanoo) was dropped into the fall state too.
    #[test]
    fn arena_floor_drops_only_fighters_that_walk_off() {
        let Some(d) = kfm() else { return };
        let d = d
            .with_states(
                "test.cns",
                "[Statedef 1001]\ntype = S\nphysics = S\nctrl = 0\nanim = 0\n\
                 [State 1001, h]\ntype = Helper\ntrigger1 = time = 0\nstateno = 1002\nid = 80\n\
                 [State 1001, back]\ntype = ChangeState\ntrigger1 = time = 2\nvalue = 1003\n\
                 [Statedef 1002]\ntype = S\nphysics = S\nanim = 0\n\
                 [State 1002, v]\ntype = VelSet\ntrigger1 = 1\nx = 15\n\
                 [Statedef 1003]\ntype = S\nphysics = N\nanim = 0\nctrl = 0\n\
                 [State 1003, p]\ntype = PosSet\ntrigger1 = 1\ny = -50\n",
            )
            .unwrap();
        let mut w = World::new(Stage::default(), 1);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        w.stage.arena.floor = Some([-150.0, 150.0]);
        run(&mut w, 5, Buttons::default());
        w.change_state(0, 1001, None, None);
        run(&mut w, 40, Buttons::default());
        let h = w
            .players
            .iter()
            .position(|p| !p.removed && p.helper_id == 80)
            .expect("helper exists");
        let h = &w.players[h];
        assert!(
            h.pos[0].abs() > 150.0,
            "helper flew past the floor: {:?}",
            h.pos
        );
        assert_eq!(
            h.state_no, 1002,
            "normal helpers are not dropped into a fall"
        );
        assert_eq!((h.pos[1], h.ground), (0.0, 0.0));
        let p = &w.players[0];
        assert_eq!(
            p.state_no, 1003,
            "held in the air by PosSet, not walked off"
        );
        assert_eq!(p.pos[1], -50.0);
    }

    fn arena_world() -> Option<World> {
        let mut w = world()?;
        w.stage.arena = Arena {
            blastzone: Some([-400.0, -400.0, 400.0, 150.0]),
            walls: false,
            stocks: 0,
            floor: None,
            ..Default::default()
        };
        w.stage.left_bound = -1000.0;
        w.stage.right_bound = 1000.0;
        w.start_round();
        while w.round.round_state() != 2 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        Some(w)
    }

    #[test]
    fn blastzone_ring_out_kos() {
        let Some(mut w) = arena_world() else { return };
        w.players[0].pos = [-450.0, 0.0];
        w.players[0].old_pos = w.players[0].pos;
        for _ in 0..3 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        assert_eq!(w.players[0].life, 0, "ring-out empties life");
        assert!(!w.players[0].alive(), "ring-out KOs");
        assert!(
            w.players[0].pos[0] >= -400.0,
            "body pulled back to the edge"
        );
        for _ in 0..200 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        assert_eq!(w.round.finish, Finish::KO);
        assert_eq!(w.round.win_team, 1);
    }

    #[test]
    fn stocks_respawn_then_ko() {
        let Some(mut w) = arena_world() else { return };
        w.stage.arena.stocks = 2;
        w.start_round();
        while w.round.round_state() != 2 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        assert_eq!(w.players[0].stocks, 2);
        w.players[0].life = 300;
        w.players[0].pos = [-450.0, 0.0];
        w.step(&[Buttons::default(), Buttons::default()]);
        let p = &w.players[0];
        assert_eq!(p.stocks, 1, "one life spent");
        assert!(p.alive());
        assert_eq!(p.life, p.life_max, "respawn restores life");
        assert!(
            p.pos[1] < 0.0 && p.pos[0].abs() < 100.0,
            "respawned above the start spot, got {:?}",
            p.pos
        );
        for _ in 0..200 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        assert_eq!(w.players[0].pos[1], 0.0, "landed after respawn");
        w.players[0].pos = [-450.0, 0.0];
        w.step(&[Buttons::default(), Buttons::default()]);
        w.step(&[Buttons::default(), Buttons::default()]);
        assert!(!w.players[0].alive(), "last stock lost = KO");
    }

    #[test]
    fn floor_span_makes_a_pit() {
        let Some(mut w) = arena_world() else { return };
        w.stage.arena.floor = Some([-100.0, 100.0]);
        let fwd = Buttons {
            l: true,
            ..Default::default()
        };
        // P1 starts at -70 facing right; walk back (left) past the floor's edge
        let mut fell = false;
        for _ in 0..300 {
            w.step(&[fwd, Buttons::default()]);
            if w.players[0].pos[1] > 50.0 {
                fell = true;
                break;
            }
        }
        assert!(
            fell,
            "walked off the floor into the pit, pos {:?}",
            w.players[0].pos
        );
        for _ in 0..200 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        assert!(
            !w.players[0].alive(),
            "falling past the bottom blast zone KOs"
        );
    }

    #[test]
    fn extra_air_jumps() {
        let Some(mut w) = world() else { return };
        w.movement.air_jumps = 2;
        run(&mut w, 5, Buttons::default());
        let up = Buttons {
            u: true,
            ..Default::default()
        };
        run(&mut w, 1, up);
        run(&mut w, 20, Buttons::default());
        let mut jumps = 0;
        for _ in 0..6 {
            run(&mut w, 1, up);
            run(&mut w, 18, Buttons::default());
            jumps = jumps.max(w.players[0].air_jump_count);
        }
        assert_eq!(jumps, 3, "KFM's one air jump plus two extra");
    }

    #[test]
    fn air_dash_double_tap() {
        let Some(mut w) = world() else { return };
        w.movement.air_dash = true;
        run(&mut w, 5, Buttons::default());
        let up = Buttons {
            u: true,
            ..Default::default()
        };
        run(&mut w, 1, up);
        run(&mut w, 12, Buttons::default());
        let fwd = Buttons {
            r: true,
            ..Default::default()
        };
        run(&mut w, 1, fwd);
        run(&mut w, 2, Buttons::default());
        run(&mut w, 1, fwd);
        let p = &w.players[0];
        assert!(p.air_dash_used, "double tap in the air dashes");
        assert!(p.vel[0] >= 4.0, "dash speed, got {}", p.vel[0]);
        assert!(p.vel[1] <= 0.5, "dash hovers, vel y {}", p.vel[1]);
    }

    #[test]
    fn fast_fall_when_holding_down() {
        let Some(mut a) = world() else { return };
        let mut b = a.clone();
        b.movement.fast_fall = true;
        let up = Buttons {
            u: true,
            ..Default::default()
        };
        let down = Buttons {
            d: true,
            ..Default::default()
        };
        for w in [&mut a, &mut b] {
            run(w, 5, Buttons::default());
            run(w, 1, up);
            run(w, 30, Buttons::default());
        }
        assert!(a.players[0].vel[1] > 0.0, "falling by now");
        run(&mut a, 5, down);
        run(&mut b, 5, down);
        assert!(
            b.players[0].vel[1] > a.players[0].vel[1] + 2.0,
            "fast fall {} vs normal {}",
            b.players[0].vel[1],
            a.players[0].vel[1]
        );
    }

    #[test]
    fn camera_zooms_out_for_height() {
        let Some(mut w) = world() else { return };
        w.stage.zoom_out = 0.5;
        w.stage.platforms.push(Platform {
            x1: -200.0,
            x2: 200.0,
            y: -260.0,
        });
        run(&mut w, 5, Buttons::default());
        w.players[0].pos = [-70.0, -260.0];
        w.players[0].old_pos = w.players[0].pos;
        w.players[0].ground = -260.0;
        for _ in 0..120 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
        assert!(
            w.cam_zoom < 0.95,
            "zoomed out for a high player, zoom {}",
            w.cam_zoom
        );
        assert!(w.cam_y < -20.0, "camera raised, y {}", w.cam_y);
    }

    #[test]
    fn jump_and_land() {
        let Some(mut w) = world() else { return };
        run(&mut w, 5, Buttons::default());
        run(
            &mut w,
            1,
            Buttons {
                u: true,
                ..Default::default()
            },
        );
        assert_eq!(w.players[0].state_no, 40, "jump start");
        run(&mut w, 3, Buttons::default());
        let p = &w.players[0];
        assert_eq!(p.state_no, 50, "airborne");
        assert_eq!(p.state_type, StateType::A);
        assert!(p.vel[1] < 0.0);
        let mut min_y = 0.0f32;
        let mut landed = false;
        for _ in 0..120 {
            w.step(&[Buttons::default(), Buttons::default()]);
            min_y = min_y.min(w.players[0].pos[1]);
            if w.players[0].state_no == 52 {
                landed = true;
            }
        }
        assert!(min_y < -40.0, "jump height {min_y}");
        assert!(landed, "went through state 52");
        let p = &w.players[0];
        assert_eq!(p.state_no, 0);
        assert_eq!(p.pos[1], 0.0);
        assert!(w.warnings.is_empty(), "{:?}", w.warnings);
    }

    fn walk_until_close(w: &mut World) {
        // walk P1 toward P2 until push contact
        for _ in 0..200 {
            w.step(&[
                Buttons {
                    r: true,
                    ..Default::default()
                },
                Buttons::default(),
            ]);
            if w.players[0].pushed {
                break;
            }
        }
        for _ in 0..10 {
            w.step(&[Buttons::default(), Buttons::default()]);
        }
    }

    #[test]
    fn push_keeps_players_apart() {
        let Some(mut w) = world() else { return };
        walk_until_close(&mut w);
        let (a, b) = (&w.players[0], &w.players[1]);
        assert!(
            w.players[0].pushed || (b.pos[0] - a.pos[0]) >= 25.0,
            "bodies do not overlap: {} {}",
            a.pos[0],
            b.pos[0]
        );
        assert!(b.pos[0] > a.pos[0]);
    }

    #[test]
    fn light_punch_connects() {
        let Some(mut w) = world() else { return };
        walk_until_close(&mut w);
        let life0 = w.players[1].life;
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        let mut hit_frame = None;
        for f in 0..30 {
            run(&mut w, 1, Buttons::default());
            if w.players[1].state_no == 5000 && hit_frame.is_none() {
                hit_frame = Some(f);
            }
        }
        assert!(
            hit_frame.is_some(),
            "P2 entered state 5000; P2 state {} life {}",
            w.players[1].state_no,
            w.players[1].life
        );
        assert!(
            w.players[1].life < life0,
            "damage applied: {} -> {}",
            life0,
            w.players[1].life
        );
        assert!(
            w.players[0].move_hit() > 0 || w.players[0].hit_count > 0,
            "MoveHit / HitCount on attacker"
        );
        run(&mut w, 90, Buttons::default());
        assert_eq!(w.players[1].state_no, 0, "P2 recovered to stand");
        assert!(w.players[1].ctrl);
        assert_eq!(w.players[1].move_type, MoveType::I);
        assert!(w.warnings.is_empty(), "{:?}", w.warnings);
    }

    #[test]
    fn guard_blocks_damage() {
        let Some(mut w) = world() else { return };
        walk_until_close(&mut w);
        let life0 = w.players[1].life;
        // P2 holds back (P2 faces left, so back = right)
        let hold = Buttons {
            r: true,
            ..Default::default()
        };
        for _ in 0..5 {
            w.step(&[Buttons::default(), hold]);
        }
        w.step(&[
            Buttons {
                x: true,
                ..Default::default()
            },
            hold,
        ]);
        let mut guarded = false;
        for _ in 0..40 {
            w.step(&[Buttons::default(), hold]);
            if (150..=155).contains(&w.players[1].state_no) {
                guarded = true;
            }
        }
        assert!(
            guarded,
            "P2 went through a guard-hit state, was in {}",
            w.players[1].state_no
        );
        assert_eq!(w.players[1].life, life0, "no chip damage from a normal");
        assert!(w.players[0].move_guarded() > 0 || w.players[0].guard_count > 0);
    }

    /// P2 overrides P1's standing normal with a custom state via HitOverride.
    #[test]
    fn hitoverride_redirects_to_state() {
        let Some(mut w) = world() else { return };
        walk_until_close(&mut w);
        w.players[1].hover[0] = HitOverride {
            attr: crate::hit::attr::parse("SCA, NA", false).unwrap(),
            stateno: 11,
            time: -1,
            forceair: false,
            forceguard: false,
        };
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        let mut seen = false;
        for _ in 0..30 {
            w.step(&[Buttons::default(), Buttons::default()]);
            if w.players[1].state_no == 11 {
                seen = true;
            }
            assert!(
                !(5000..=5020).contains(&w.players[1].state_no),
                "override must replace the get-hit state"
            );
        }
        assert!(seen, "P2 never entered the HitOverride state");
    }

    /// A ReversalDef on P2 catches P1's punch: P1 is the one who gets hit.
    #[test]
    fn reversaldef_catches_attack() {
        let Some(mut w) = world() else { return };
        walk_until_close(&mut w);
        let text = "[Statedef 900]\ntype = S\nmovetype = A\nphysics = S\nanim = 200\nctrl = 0\n\n\
                    [State 900, r]\ntype = ReversalDef\ntrigger1 = 1\nreversal.attr = SCA, NA\n\
                    pausetime = 0, 0\np2stateno = 5000\n";
        let d = w.data[0]
            .with_states("test", text)
            .expect("reversal state compiles");
        // give the reversal anim a Clsn1 on every frame so timing can't miss
        let mut d = (*d).clone();
        if let Some(a) = d.air.actions.get_mut(&200) {
            for f in &mut a.frames {
                f.clsn1 = vec![[-10, -80, 40, 0]];
            }
        }
        let d = Arc::new(d);
        w.data[0] = d.clone();
        w.data[1] = d;
        w.change_state(1, 900, None, Some(false));
        let life1 = w.players[1].life;
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        let mut p1_hit = false;
        for _ in 0..20 {
            w.step(&[Buttons::default(), Buttons::default()]);
            if w.players[0].move_type == MoveType::H {
                p1_hit = true;
            }
        }
        assert!(p1_hit, "P1 should be reversed into a hit state");
        assert_eq!(w.players[1].life, life1, "the reversed attack never lands");
    }

    #[test]
    fn knockout() {
        let Some(mut w) = world() else { return };
        w.players[1].life = 5;
        w.players[1].life_max = 1000;
        walk_until_close(&mut w);
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        run(&mut w, 40, Buttons::default());
        assert_eq!(w.players[1].life, 0);
        assert!(w.players[1].ko);
        run(&mut w, 200, Buttons::default());
        assert_eq!(
            w.players[1].state_no, 5150,
            "lying defeated: in {}",
            w.players[1].state_no
        );
    }

    /// Community characters from reference/testchars: random inputs must never panic or warn.
    #[test]
    fn soak_test_chars() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/testchars");
        let Ok(rd) = std::fs::read_dir(&root) else {
            return;
        };
        let mut defs = Vec::new();
        for e in rd.flatten() {
            let d = e.path();
            if let Ok(inner) = std::fs::read_dir(&d) {
                for f in inner.flatten() {
                    let sub = f.path();
                    if sub.is_dir() {
                        let name = sub.file_name().unwrap().to_string_lossy().to_string();
                        let def = sub.join(format!("{name}.def"));
                        if def.exists() {
                            defs.push(def);
                        }
                    }
                }
            }
        }
        if defs.len() < 2 {
            return;
        }
        let d1 = CharData::load(&defs[0]).unwrap();
        let d2 = CharData::load(&defs[1]).unwrap();
        let mut w = World::new(Stage::default(), 7);
        w.add_player(d1, 0, 1);
        w.add_player(d2, 1, 2);
        let mut rng = Rng::new(99);
        for _ in 0..1800 {
            let r = |rng: &mut Rng| Buttons::from_bits((rng.next_u64() & 0x7ff) as u16);
            let a = r(&mut rng);
            let b = r(&mut rng);
            w.step(&[a, b]);
        }
        for p in &w.players {
            assert!(p.pos[0].is_finite() && p.pos[1].is_finite());
        }
        // missing actions are the content's problem; engine trouble is "stuck" / "crashed"
        let bad: Vec<&String> = w
            .warnings
            .iter()
            .filter(|m| m.contains("stuck") || m.contains("crashed") || m.contains("non-finite"))
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
    }

    #[test]
    fn hit_spawns_spark_explod() {
        let Some(mut w) = world() else { return };
        let data_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1/data");
        w.fx = FxData::load(&data_dir).ok();
        assert!(w.fx.is_some(), "fightfx loads");
        walk_until_close(&mut w);
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        let mut seen = false;
        for _ in 0..20 {
            run(&mut w, 1, Buttons::default());
            if w.explods.iter().any(|e| matches!(e.src, AnimSrc::Fx)) {
                seen = true;
            }
        }
        assert!(seen, "a fightfx spark explod existed after the hit");
        run(&mut w, 120, Buttons::default());
        assert!(
            w.explods.is_empty(),
            "sparks remove themselves when their animation ends"
        );
    }

    #[test]
    fn pause_freezes_opponent() {
        let Some(mut w) = world() else { return };
        run(&mut w, 5, Buttons::default());
        // P1 pauses for 20 ticks with 20 move ticks for itself
        w.set_pause_time(0, 20, 20);
        let t2 = w.players[1].state_time;
        run(&mut w, 10, Buttons::default());
        assert!(w.pausetime > 0, "pause running");
        assert_eq!(w.players[1].state_time, t2, "P2 frozen");
        assert!(
            w.players[0].state_time > 0,
            "P1 keeps moving during its own pause"
        );
        run(&mut w, 30, Buttons::default());
        assert_eq!(w.pausetime, 0);
        assert!(w.players[1].state_time > t2, "P2 resumes");
    }

    #[test]
    fn projectile_hits_and_is_counted() {
        let Some(d) = kfm() else { return };
        let d = d
            .with_states(
                "test.cns",
                "[Statedef 1000]\ntype = S\nphysics = S\nctrl = 0\nanim = 0\n[State 1000, p]\ntype = Projectile\ntrigger1 = time = 0\nprojanim = 200\nprojid = 5\noffset = 20, 0\nvelocity = 4\nattr = S, NP\ndamage = 30\npausetime = 2, 8\nground.velocity = -4\nhitflag = MAF\n[State 1000, back]\ntype = ChangeState\ntrigger1 = time > 60\nvalue = 0\nctrl = 1\n",
            )
            .unwrap();
        let mut w = World::new(Stage::default(), 1);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        run(&mut w, 5, Buttons::default());
        w.change_state(0, 1000, None, None);
        run(&mut w, 1, Buttons::default());
        assert_eq!(w.projs.len(), 1, "projectile spawned");
        assert_eq!(w.num_proj(1, 5), 1);
        assert_eq!(w.num_proj(1, 6), 0);
        let x0 = w.projs[0].pos[0];
        run(&mut w, 3, Buttons::default());
        assert!(w.projs[0].pos[0] > x0, "it moves forward");
        let life0 = w.players[1].life;
        let mut hit = false;
        for _ in 0..90 {
            run(&mut w, 1, Buttons::default());
            if w.players[1].state_no == 5000 {
                hit = true;
                break;
            }
        }
        assert!(hit, "projectile hit P2 (state {})", w.players[1].state_no);
        run(&mut w, 5, Buttons::default());
        assert!(w.players[1].life < life0);
        assert!(w.players[0].pctime >= 0, "ProjHitTime ticks on the owner");
        assert!(
            w.projs.is_empty() || !w.projs[0].is_active(),
            "single-hit projectile is spent"
        );
    }

    #[test]
    fn helper_spawns_runs_and_destroys_itself() {
        let Some(d) = kfm() else { return };
        let d = d
            .with_states(
                "test.cns",
                "[Statedef 1001]\ntype = S\nphysics = S\nctrl = 0\nanim = 0\n[State 1001, h]\ntype = Helper\ntrigger1 = time = 0\nstateno = 1002\nid = 77\npos = 30, 0\n[State 1001, back]\ntype = ChangeState\ntrigger1 = time > 60\nvalue = 0\nctrl = 1\n[Statedef 1002]\ntype = S\nphysics = N\nanim = 0\n[State 1002, v]\ntype = VarSet\ntrigger1 = 1\nvar(5) = parentdist x\n[State 1002, d]\ntype = DestroySelf\ntrigger1 = time >= 10\n",
            )
            .unwrap();
        let mut w = World::new(Stage::default(), 1);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        run(&mut w, 5, Buttons::default());
        w.change_state(0, 1001, None, None);
        run(&mut w, 2, Buttons::default());
        let h = w
            .players
            .iter()
            .position(|p| !p.removed && p.helper_index > 0)
            .expect("helper exists");
        assert_eq!(w.players[h].helper_id, 77);
        assert_eq!(w.players[h].state_no, 1002);
        assert_eq!(w.players[h].parent_id, w.players[0].id);
        assert!(
            (w.players[h].pos[0] - (w.players[0].pos[0] + 30.0)).abs() < 0.01,
            "placed 30 in front of the parent"
        );
        assert_eq!(
            w.players[h].vars[5], -30,
            "parentdist x from the helper's view"
        );
        assert_eq!(w.players.len(), 3);
        run(&mut w, 15, Buttons::default());
        assert!(w.players[h].removed, "DestroySelf after 10 ticks");
        assert!(w.players.iter().filter(|p| !p.removed).count() == 2);
    }

    #[test]
    fn new_helper_runs_its_first_tick_once() {
        // G6 Naruto's Rendan clones `Turn` at time = 0; running the body twice on the
        // spawn tick turned them back around and they dashed away from P2
        let Some(d) = kfm() else { return };
        let d = d
            .with_states(
                "test.cns",
                "[Statedef 1001]\ntype = S\nphysics = S\nctrl = 0\nanim = 0\n[State 1001, h]\ntype = Helper\ntrigger1 = time = 0\nstateno = 1002\nid = 78\npos = 30, 0\n[State 1001, back]\ntype = ChangeState\ntrigger1 = time > 60\nvalue = 0\nctrl = 1\n[Statedef 1002]\ntype = S\nphysics = N\nanim = 0\n[State 1002, t]\ntype = Turn\ntrigger1 = time = 0\n[State 1002, v]\ntype = VarAdd\ntrigger1 = time = 0\nvar(1) = 1\n",
            )
            .unwrap();
        let mut w = World::new(Stage::default(), 1);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        run(&mut w, 5, Buttons::default());
        w.change_state(0, 1001, None, None);
        run(&mut w, 3, Buttons::default());
        let h = w
            .players
            .iter()
            .position(|p| !p.removed && p.helper_id == 78)
            .expect("helper exists");
        assert_eq!(w.players[h].vars[1], 1, "time = 0 controllers fire once");
        assert_eq!(
            w.players[h].facing, -w.players[0].facing,
            "a single Turn on the first tick"
        );
    }

    /// Helpers run no special states, hardcoded actions or commands unless keyctrl = 1
    /// (-1 only) or helpertype = player (-3 / -2). G6 Sasuke's Amaterasu FX
    /// helper (keyctrl = 0, missing statedef) kept the spawn tick's buffered
    /// "holddown" + "b" forever and looped his state -1 combo moves.
    #[test]
    fn helpers_run_special_states_only_with_keyctrl() {
        let Some(d) = kfm() else { return };
        let d = d
            .with_states(
                "test.cns",
                "[Statedef -3]\n[State -3, v]\ntype = VarAdd\ntrigger1 = IsHelper\nvar(2) = 1\n\
                 [Statedef -2]\n[State -2, v]\ntype = VarAdd\ntrigger1 = IsHelper\nvar(3) = 1\n\
                 [Statedef -1]\n[State -1, c]\ntype = ChangeState\ntrigger1 = IsHelper && command = \"x\"\nvalue = 1003\n\
                 [Statedef 1001]\ntype = S\nphysics = S\nctrl = 0\nanim = 0\n\
                 [State 1001, a]\ntype = Helper\ntrigger1 = time = 0\nstateno = 1002\nid = 77\n\
                 [State 1001, b]\ntype = Helper\ntrigger1 = time = 0\nstateno = 1002\nid = 79\nkeyctrl = 1\n\
                 [Statedef 1002]\ntype = S\nphysics = N\nanim = 0\nctrl = 1\n\
                 [State 1002, c]\ntype = VarSet\ntrigger1 = command = \"x\"\nvar(4) = 1\n\
                 [Statedef 1003]\ntype = S\nphysics = N\nanim = 0\nctrl = 0\n",
            )
            .unwrap();
        let mut w = World::new(Stage::default(), 1);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        run(&mut w, 5, Buttons::default());
        let x = Buttons {
            x: true,
            d: true,
            ..Default::default()
        };
        w.change_state(0, 1001, None, Some(false));
        run(&mut w, 1, x);
        run(&mut w, 10, Buttons::default());
        run(&mut w, 1, x);
        run(&mut w, 2, Buttons::default());
        let find = |w: &World, id| {
            w.players
                .iter()
                .position(|p| !p.removed && p.helper_id == id)
                .expect("helper exists")
        };
        let plain = &w.players[find(&w, 77)];
        assert_eq!(
            plain.state_no, 1002,
            "no state -1 or hardcoded crouch without keyctrl"
        );
        assert_eq!(plain.vars[4], 0, "no command input without keyctrl");
        assert_eq!(
            (plain.vars[2], plain.vars[3]),
            (0, 0),
            "no -3 / -2 for a normal helper"
        );
        let keyed = &w.players[find(&w, 79)];
        assert_eq!(keyed.state_no, 1003, "keyctrl helpers run state -1");
    }

    /// In another character's states (thrown via p2stateno) a player skips its
    /// state -3 and still runs its own -2, never the state owner's.
    #[test]
    fn custom_state_runs_own_minus_two_and_skips_minus_three() {
        let Some(d) = kfm() else { return };
        let mine = d
            .with_states(
                "a.cns",
                "[Statedef -3]\n[State -3, v]\ntype = VarAdd\ntrigger1 = 1\nvar(5) = 1\n\
                 [Statedef -2]\n[State -2, v]\ntype = VarAdd\ntrigger1 = 1\nvar(6) = 1\n",
            )
            .unwrap();
        let theirs = d
            .with_states(
                "b.cns",
                "[Statedef -2]\n[State -2, v]\ntype = VarAdd\ntrigger1 = 1\nvar(7) = 1\n\
                 [Statedef 888]\ntype = S\nmovetype = H\nphysics = N\nanim = 0\nctrl = 0\n",
            )
            .unwrap();
        let mut w = World::new(Stage::default(), 1);
        w.add_player(mine, 0, 1);
        w.add_player(theirs, 1, 2);
        run(&mut w, 5, Buttons::default());
        w.players[0].vars[5] = 0;
        w.players[0].vars[6] = 0;
        w.players[0].vars[7] = 0;
        w.players[0].state_owner = w.players[1].data;
        w.change_state(0, 888, None, Some(false));
        run(&mut w, 10, Buttons::default());
        let p = &w.players[0];
        assert_eq!(p.state_no, 888);
        assert_eq!(p.vars[5], 0, "-3 skipped in a custom state");
        assert_eq!(p.vars[6], 10, "own -2 runs every tick");
        assert_eq!(p.vars[7], 0, "the state owner's -2 is not run");
    }

    #[test]
    fn full_round_flow_to_match_over() {
        let Some(mut w) = world() else { return };
        w.round = RoundState::new(RoundConfig {
            round_time: 99,
            ..Default::default()
        });
        w.start_round();
        assert_eq!(w.round.round_state(), 0, "pre-intro");
        assert!(!w.players[0].ctrl);
        run(&mut w, 40, Buttons::default());
        assert_eq!(
            w.round.round_state(),
            1,
            "KFM's intro pose holds the round (AssertSpecial intro)"
        );
        let mut waited = 0;
        while w.round.round_state() != 2 && waited < 1200 {
            run(&mut w, 1, Buttons::default());
            waited += 1;
        }
        assert_eq!(w.round.round_state(), 2, "fighting once the intro ends");
        assert!(w.players[0].ctrl && w.players[1].ctrl);
        assert_eq!(w.round.clock(), 99);
        // P2 dies to one punch
        w.players[1].life = 1;
        walk_until_close(&mut w);
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        run(&mut w, 30, Buttons::default());
        assert!(w.players[1].ko);
        assert_eq!(w.round.finish, Finish::KO);
        assert_eq!(w.round.win_team, 0);
        assert_eq!(w.round.round_state(), 3);
        run(&mut w, 120, Buttons::default());
        assert_eq!(w.round.round_state(), 4);
        assert!(
            (180..=189).contains(&w.players[0].state_no),
            "winner in a win pose: {}",
            w.players[0].state_no
        );
        assert_eq!(w.round.wins, [1, 0]);
        run(&mut w, 250, Buttons::default());
        assert_eq!(w.round.round_no, 2, "second round started");
        assert_eq!(w.players[1].life, w.players[1].life_max, "life restored");
        assert!(!w.players[1].ko);
        assert!(w.explods.is_empty());
        // round 2: no intro pose, straight to the fight
        let mut waited = 0;
        while w.round.round_state() != 2 && waited < 1200 {
            run(&mut w, 1, Buttons::default());
            waited += 1;
        }
        w.players[1].life = 1;
        walk_until_close(&mut w);
        run(
            &mut w,
            1,
            Buttons {
                x: true,
                ..Default::default()
            },
        );
        run(&mut w, 400, Buttons::default());
        assert!(w.round.match_over, "two wins end the match");
        assert_eq!(w.round.wins, [2, 0]);
        assert!(w.events.iter().any(|e| matches!(e, Event::MatchOver(0))) || w.round.match_over);
    }

    #[test]
    fn time_over_decides_by_life() {
        let Some(mut w) = world() else { return };
        w.round = RoundState::new(RoundConfig {
            round_time: 1,
            ..Default::default()
        });
        w.start_round();
        let mut waited = 0;
        while w.round.round_state() != 2 && waited < 1200 {
            run(&mut w, 1, Buttons::default());
            waited += 1;
        }
        w.players[1].life = 500;
        run(&mut w, 70, Buttons::default());
        assert_eq!(w.round.finish, Finish::TimeOver);
        assert_eq!(w.round.win_team, 0);
        assert_eq!(w.round.clock(), 0);
    }

    #[test]
    fn deterministic_and_cloneable() {
        let Some(mut a) = world() else { return };
        let mut b = a.clone();
        let seq = [
            Buttons {
                r: true,
                ..Default::default()
            },
            Buttons {
                x: true,
                ..Default::default()
            },
            Buttons::default(),
        ];
        for k in 0..200 {
            let i = seq[k % 3];
            a.step(&[i, Buttons::default()]);
            b.step(&[i, Buttons::default()]);
        }
        assert_eq!(a.checksum(), b.checksum());
        assert_eq!(a.players, b.players);
    }
}
