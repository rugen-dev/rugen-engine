//! The match: players, stage bounds, camera, and the per-tick update in
//! MUGEN's order (commands → prepare → run → bounds → animate).

use std::sync::Arc;

use rugen_cns::state::{MoveType, Physics, StateType};
use rugen_cns::{Code, Context, Value};

use crate::Rng;
use crate::anim::AnimState;
use crate::chardata::{CharData, Ctrl};
use crate::command::CommandList;
use crate::context::Ctx;
use crate::input::Buttons;
use crate::player::{Player, assert};
use crate::sctrl::Sctrl;

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
}

#[derive(Clone)]
pub struct World {
    pub data: Vec<Arc<CharData>>,
    pub players: Vec<Player>,
    pub stage: Stage,
    pub frame: u32,
    pub rng: Rng,
    pub cam_x: f32,
    pub events: Vec<Event>,
    pub warnings: Vec<String>,
}

const MAX_STATE_LOOP: usize = 2500;

impl World {
    pub fn new(stage: Stage, seed: u64) -> Self {
        Self {
            data: Vec::new(),
            players: Vec::new(),
            stage,
            frame: 0,
            rng: Rng::new(seed),
            cam_x: 0.0,
            events: Vec::new(),
            warnings: Vec::new(),
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
        self.players.push(p);
        // round start: MUGEN runs state 5900 then 0 and grants control once the round begins
        self.change_state(idx, 5900, None, None);
        self.apply_pending(idx);
        self.change_state(idx, 0, None, Some(true));
        self.apply_pending(idx);
        idx
    }

    pub fn nearest_enemy(&self, me: usize) -> Option<usize> {
        let p = &self.players[me];
        self.players
            .iter()
            .enumerate()
            .filter(|(i, o)| *i != me && o.team != p.team)
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
        self.cam_x - self.stage.width / 2.0
    }
    /// Stage x range players may occupy on screen.
    pub fn screen_bounds(&self) -> (f32, f32) {
        (
            self.screen_left() + self.stage.screen_left,
            self.cam_x + self.stage.width / 2.0 - self.stage.screen_right,
        )
    }

    // ------------------------------------------------------------ state changes

    fn eval(&mut self, i: usize, code: &Code) -> Value {
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
        let n = self.data[p.data]
            .states
            .get(&p.state_no)
            .map_or(0, |s| s.ctrls.len());
        p.persist = vec![0; n];
        p.pending_state = Some(p.state_no);
    }

    /// `stateChange2`: apply the statedef of the buffered state.
    fn apply_pending(&mut self, i: usize) -> bool {
        let Some(no) = self.players[i].pending_state.take() else {
            return false;
        };
        let data = self.data[self.players[i].data].clone();
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

    pub fn change_anim(&mut self, i: usize, mut no: i32, elem: Option<(i32, i32)>) {
        if no < 0 {
            self.warn(format!("P{} attempted change to negative anim {no}", i + 1));
            no = 0;
        }
        let data = self.data[self.players[i].data].clone();
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
        if let Some((e, t)) = elem {
            p.anim.set_elem(action, e, t);
        }
    }

    fn power_add(&mut self, i: usize, v: i32) {
        let p = &mut self.players[i];
        p.power = (p.power + v).clamp(0, p.power_max);
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

        // 1. commands
        for i in 0..n {
            let p = &self.players[i];
            if p.ctrl && matches!(p.state_no, 0 | 11 | 20) {
                self.auto_turn(i);
            }
            let p = &mut self.players[i];
            let b = inputs.get(i).copied().unwrap_or_default();
            let (back, fwd) = if p.facing < 0.0 {
                (b.r, b.l)
            } else {
                (b.l, b.r)
            };
            p.input
                .update(b.u, b.d, back, fwd, b.a, b.b, b.c, b.x, b.y, b.z, b.s);
            let input = p.input.clone();
            p.cmds.step(&input);
        }

        // 2. prepare: hardcoded key behaviour and per-tick resets
        for i in 0..n {
            self.prepare(i);
        }

        // 3. run states, physics, landing
        for i in 0..n {
            self.run(i);
        }

        // 4. bounds
        self.update_camera();
        for i in 0..n {
            self.screen_bound(i);
        }

        // 5. animate
        for i in 0..n {
            let data = self.data[self.players[i].data].clone();
            let p = &mut self.players[i];
            if p.hit_pause == 0
                && let Some(a) = data.air.get(p.anim.action)
            {
                p.anim.step(a);
            }
            p.old_pos = p.pos;
        }
        self.frame += 1;
    }

    fn prepare(&mut self, i: usize) {
        let p = &self.players[i];
        let data = self.data[p.data].clone();
        let (ub, db, fb, bb) = (p.input.ub, p.input.db, p.input.fb, p.input.bb);
        if p.ctrl {
            let st = p.state_type;
            let no = p.state_no;
            if st == StateType::S && ub > 0 {
                if no != 40 {
                    self.change_state(i, 40, None, None);
                }
            } else if st == StateType::A
                && ub == 1
                && p.pos[1] <= -data.konst("movement.airjump.height").unwrap_or(35.0)
                && p.air_jump_count < data.konst("movement.airjump.num").unwrap_or(1.0) as i32
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
        if p.state_no == 20 && (bb > 0) == (fb > 0) {
            self.change_state(i, 0, None, None);
        }
        let p = &mut self.players[i];
        if p.state_type != StateType::A {
            p.air_jump_count = 0;
        }
        p.assert_flags = 0;
        p.pos_freeze = false;
        p.width_edge = [0.0, 0.0];
        p.width_player = [0.0, 0.0];
        p.screen_bound = true;
        p.move_camera = [true, true];
    }

    fn run(&mut self, i: usize) {
        let data = self.data[self.players[i].data].clone();
        for neg in [-3, -2, -1] {
            if let Some(state) = data.states.get(&neg) {
                if neg == -1 && !self.players[i].ctrl && self.players[i].state_type != StateType::U
                {
                    // [State -1] still runs in MUGEN; its controllers gate on ctrl themselves
                }
                self.players[i].minus = neg as i8;
                let mut persist = vec![0; state.ctrls.len()];
                self.run_ctrls(i, &state.ctrls, &mut persist);
            }
        }
        self.players[i].minus = 0;
        self.run_current(i);
        if self.players[i].hit_pause == 0 {
            self.pos_update(i);
            let p = &self.players[i];
            if p.physics == Physics::A && p.vel[1] > 0.0 && p.pos[1] >= 0.0 && p.state_no != 105 {
                // engine-side ChangeState outside a state body: the new state runs right away
                self.change_state(i, 52, None, None);
                self.run_current(i);
            }
            self.players[i].state_time += 1;
        }
    }

    /// Apply a buffered state change and run the current state's controllers,
    /// following ChangeStates from inside the body immediately (Ikemen `changeStateEx`).
    fn run_current(&mut self, i: usize) {
        let data = self.data[self.players[i].data].clone();
        self.apply_pending(i);
        let mut loops = 0;
        loop {
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
                self.warn(format!(
                    "P{} state machine stuck in a loop at state {no}",
                    i + 1
                ));
                break;
            }
        }
    }

    /// Run a controller list. Returns true if the state changed (stop processing).
    fn run_ctrls(&mut self, i: usize, ctrls: &[Ctrl], persist: &mut [i32]) -> bool {
        for (ci, c) in ctrls.iter().enumerate() {
            if self.players[i].hit_pause > 0 && !c.ignorehitpause {
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
            Sctrl::ChangeState { value, ctrl, anim } | Sctrl::SelfState { value, ctrl, anim } => {
                let v = self.eval(i, value).to_i();
                let a = anim.as_ref().map(|c| self.eval(i, c).to_i());
                let c = ctrl.as_ref().map(|c| self.eval(i, c).to_b());
                self.change_state(i, v, a, c);
                return true;
            }
            Sctrl::ChangeAnim { value, elem } | Sctrl::ChangeAnim2 { value, elem } => {
                let v = self.eval(i, value).to_i();
                let e = elem.as_ref().map(|c| (self.eval(i, c).to_i(), 0));
                self.change_anim(i, v, e);
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
                    p.pos[1] = v;
                    p.old_pos[1] = v;
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
                self.events.push(Event::Sound {
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
                p.life = (p.life + v).min(p.life_max).max(if kill { 0 } else { 1 });
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

    fn pos_update(&mut self, i: usize) {
        let data = self.data[self.players[i].data].clone();
        let p = &mut self.players[i];
        p.old_pos = p.pos;
        if !p.pos_freeze {
            p.pos[0] += p.vel[0] * p.facing;
            p.pos[1] += p.vel[1];
        }
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
    }

    fn update_camera(&mut self) {
        if self.players.is_empty() {
            return;
        }
        let (min, max) = self
            .players
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                (lo.min(p.pos[0]), hi.max(p.pos[0]))
            });
        let mid = (min + max) / 2.0;
        self.cam_x = mid.clamp(self.stage.bound_left, self.stage.bound_right);
    }

    fn screen_bound(&mut self, i: usize) {
        let (xmin, xmax) = self.screen_bounds();
        let p = &self.players[i];
        let mut x = p.pos[0];
        if p.screen_bound {
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

    /// Hash of the gameplay state, for desync detection.
    pub fn checksum(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        mix(self.frame as u64);
        mix(self.rng.state());
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
