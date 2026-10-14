//! Arena AI plugin: keeps computer fighters alive on `[Arena]` stages.
//!
//! MUGEN characters' own AI knows nothing about pits, blast zones or
//! platforms, so on an arena it walks, dashes and flies off the island. This
//! layer sits between a CPU fighter's own AI and the simulation and only acts
//! on arena stages:
//!
//! - [`filter`] edits the CPU's buttons each tick: it drops directions that
//!   would carry the fighter off the floor, drives recovery (air jumps, air
//!   dash, hit-fall tech) when off-stage, and adds platform navigation and
//!   corner escapes.
//! - [`allow_change`] vets the character AI's `ChangeState`s: a move learned
//!   (this match) to travel past the edge from here is vetoed.
//! - [`observe`] measures every AI-started move's travel and remembers the
//!   risky ones per fighter.
//!
//! The character AI still picks every attack; the plugin filters movement.
//! Everything is computed from sim state (no `world.rng`: its rolls hash the
//! frame), so replays and synctest reproduce it exactly.

use rugen_cns::state::{MoveType, StateType};

use crate::input::Buttons;
use crate::world::{NO_GROUND, World};

/// Moves remembered per fighter.
const MEMO: usize = 24;

/// What a fighter learned about one of its moves (stage units).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveMemo {
    pub state: i32,
    /// Furthest travel along the facing at the move's start (forward > 0).
    pub fwd: f32,
    /// Furthest travel backward (≤ 0).
    pub back: f32,
    pub uses: u16,
    /// Uses that left the stage (or ended in a ring-out).
    pub bad: u16,
    /// Jumped more than 60 units in one tick (a teleport to who knows where).
    pub teleport: bool,
    /// Started in the air at least once (usable for recovery).
    pub air: bool,
    /// Furthest drop below the start height (≥ 0).
    pub sink: f32,
}

/// A move in progress, opened by an AI `ChangeState` from a neutral state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Episode {
    state: i32,
    x: f32,
    facing: f32,
    fwd: f32,
    back: f32,
    off: bool,
    teleport: bool,
    air: bool,
    y: f32,
    sink: f32,
    ticks: i32,
}

/// Per-fighter plugin state (plain data, part of the world).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArenaAiState {
    pub memo: [MoveMemo; MEMO],
    memo_next: u8,
    episode: Option<Episode>,
    /// Ticks the fighter has been in danger (off-stage) in a row.
    danger: i32,
    /// Ticks the fighter has been sliding toward a drop in a row.
    edge: i32,
    /// Buttons sent last tick (for taps).
    last: Buttons,
    /// Scripted directions (bits of `Buttons`) played one per tick.
    script: [u16; 6],
    script_len: u8,
    script_pos: u8,
    prev_stocks: i32,
    /// Position at the end of the last tick and the move since the one
    /// before (stage units): catches PosAdd teleports that velocity misses.
    last_x: Option<f32>,
    dx: f32,
    /// State a chain move was vetoed in, to let it through if it gets stuck.
    veto_state: i32,
    /// A recovery move was used this time off-stage.
    used_move: bool,
    /// Where a navigation jump is headed (the opponent's spot on a higher tier).
    climb: Option<[f32; 2]>,
    /// Counters for tests and tools.
    pub vetoes: u32,
    pub cancels: u32,
    pub recoveries: u32,
    pub navs: u32,
    pub ledges: u32,
    /// Ticks spent walking to an item.
    pub items: u32,
    /// Walking to an item this tick (a smaller edge margin).
    item_walk: bool,
    /// Ticks spent dodging a hazard (leaving a dropping platform, leaning
    /// into the wind).
    pub hazards: u32,
}

impl ArenaAiState {
    /// Buttons the plugin sent last tick (tools, tests).
    pub fn last_buttons(&self) -> Buttons {
        self.last
    }

    fn memo(&self, state: i32) -> Option<&MoveMemo> {
        self.memo.iter().find(|m| m.uses > 0 && m.state == state)
    }

    fn learn(&mut self, e: &Episode, bad: bool) {
        let k = match self
            .memo
            .iter()
            .position(|m| m.uses > 0 && m.state == e.state)
        {
            Some(k) => k,
            None => {
                let k = usize::from(self.memo_next) % MEMO;
                self.memo_next = ((k + 1) % MEMO) as u8;
                self.memo[k] = MoveMemo {
                    state: e.state,
                    ..Default::default()
                };
                k
            }
        };
        let m = &mut self.memo[k];
        m.fwd = m.fwd.max(e.fwd);
        m.back = m.back.min(e.back);
        m.uses = m.uses.saturating_add(1);
        m.teleport |= e.teleport;
        m.air |= e.air;
        m.sink = m.sink.max(e.sink);
        if bad {
            m.bad = m.bad.saturating_add(1);
        }
    }

    fn play(&mut self, steps: &[Buttons]) {
        let n = steps.len().min(self.script.len());
        for (k, b) in steps.iter().take(n).enumerate() {
            self.script[k] = b.bits();
        }
        self.script_len = n as u8;
        self.script_pos = 0;
    }

    fn scripted(&self) -> bool {
        self.script_pos < self.script_len
    }
}

/// Engine basic states the plugin never vetoes or tracks as moves.
fn basic(state: i32) -> bool {
    matches!(state, 0 | 10 | 11 | 12 | 20 | 40..=52 | 120..=155) || state >= 5000
}

/// Does the plugin drive player `i` this tick?
pub fn active(w: &World, i: usize) -> bool {
    let Some(p) = w.players.get(i) else {
        return false;
    };
    let a = &w.stage.arena;
    w.arena_ai
        && p.ai_level > 0
        && p.helper_index == 0
        && !p.removed
        && !p.broken
        && p.alive()
        && (a.floor.is_some() || a.blastzone.is_some())
        && w.round.round_state() == 2
}

/// Per-tick view of one fighter and the stage, in stage units.
struct View {
    level: i32,
    x: f32,
    y: f32,
    /// Velocity in stage units, absolute direction.
    vx: f32,
    vy: f32,
    span: [f32; 2],
    airborne: bool,
    /// Standing on the main floor.
    on_deck: bool,
    /// Off the stage: airborne with nothing below.
    off: bool,
    /// The opponent: position and whether it stands on something.
    opp: Option<([f32; 2], bool)>,
    run: f32,
    /// How far a forward jump carries (stage units).
    jump: f32,
    /// Gravity (stage units per tick²).
    g: f32,
}

impl View {
    fn new(w: &World, i: usize) -> Self {
        let p = &w.players[i];
        let s = w.local_scale(i);
        let (x, y) = (p.pos[0] * s, p.pos[1] * s);
        let span = floor_span(w);
        let airborne = p.state_type == StateType::A;
        let vel = p.vel[0] * p.facing * s;
        // measured travel wins when larger (PosAdd, teleports), but not a respawn
        let moved = p.arena.dx;
        let vx = if moved.abs() > vel.abs() && moved.abs() < 80.0 {
            moved
        } else {
            vel
        };
        let g = w.data[p.data].konst("movement.yaccel").unwrap_or(0.44) * s;
        // off-stage: the fall from here (as it goes now) lands on nothing
        // and catches no ledge
        let fall = [vx, p.vel[1] * s];
        let off =
            airborne && !lands(w, [x, y], fall, g, 0, 150) && !grabs(w, i, [x, y], fall, g, 150);
        let opp = w
            .players
            .iter()
            .enumerate()
            .find(|(k, q)| {
                *k != i && q.helper_index == 0 && !q.removed && q.alive() && q.team != p.team
            })
            .map(|(k, q)| {
                let qs = w.local_scale(k);
                (
                    [q.pos[0] * qs, q.pos[1] * qs],
                    q.state_type != StateType::A && q.ground < NO_GROUND,
                )
            });
        let run = w.data[p.data]
            .konst("velocity.run.fwd.x")
            .unwrap_or(4.6)
            .abs()
            * s;
        let data = &w.data[p.data];
        let k = |n: &str, d: f32| data.konst(n).unwrap_or(d);
        let air_ticks =
            2.0 * k("velocity.jump.y", -8.4).abs() / k("movement.yaccel", 0.44).max(0.05);
        let jump = k("velocity.jump.fwd.x", 2.5).abs() * s * air_ticks;
        Self {
            level: p.ai_level.clamp(1, 8),
            jump,
            x,
            y,
            vx,
            vy: p.vel[1] * s,
            span,
            airborne,
            on_deck: !airborne && p.ground == 0.0,
            off,
            g,
            opp,
            run,
        }
    }

    /// Ticks before the plugin reacts to danger.
    fn react(&self) -> i32 {
        2 * (9 - self.level)
    }

    /// Safety distance kept from the edge.
    fn margin(&self) -> f32 {
        40.0 + 8.0 * self.level as f32 + self.run * 8.0
    }

    /// Percent chance a veto or cancel happens.
    fn chance(&self) -> u32 {
        (70 + 8 * self.level).clamp(0, 100) as u32
    }
}

/// The main floor's span; without `floor`, the ground runs to just short
/// of the blast zone.
fn floor_span(w: &World) -> [f32; 2] {
    let a = &w.stage.arena;
    match (a.floor, a.blastzone) {
        (Some(f), _) => f,
        (None, Some(b)) => [b[0] + 120.0, b[2] - 120.0],
        (None, None) => [w.stage.left_bound, w.stage.right_bound],
    }
}

/// Direction (+1 / -1) from the centre of `span` toward its nearer edge.
fn outward(span: [f32; 2], x: f32) -> f32 {
    if x >= (span[0] + span[1]) * 0.5 {
        1.0
    } else {
        -1.0
    }
}

/// Highest surface at or below `y` at `x` (stage units), if any.
pub fn surface_below(w: &World, x: f32, y: f32) -> Option<f32> {
    let span = floor_span(w);
    let mut best: Option<f32> = (x >= span[0] && x <= span[1] && y <= 0.5).then_some(0.0);
    for pl in w.stage.platforms.iter().filter(|p| !p.gone) {
        if x >= pl.x1 && x <= pl.x2 && pl.y >= y - 0.5 && best.is_none_or(|b| pl.y < b) {
            best = Some(pl.y);
        }
    }
    best
}

/// Does a body at `(x, y)` moving at `(vx, vy)` come down on a surface
/// within `ticks`? It slides along the surface it stands on (still there
/// after `slide` ticks = safe), then falls ballistically (stage units).
/// Undecided after `ticks` counts as landing.
pub fn lands(
    w: &World,
    [mut x, mut y]: [f32; 2],
    [vx, mut vy]: [f32; 2],
    g: f32,
    slide: u32,
    ticks: u32,
) -> bool {
    let bottom = w.stage.arena.blastzone.map_or(400.0, |b| b[3]);
    // a gust blowing now keeps blowing (half on the ground)
    let wind = crate::hazards::wind(w).0;
    let mut on = surface_below(w, x, y).filter(|s| (s - y).abs() < 1.0 && vy >= 0.0);
    for t in 0..ticks {
        if let Some(sy) = on {
            if t >= slide {
                return true;
            }
            x += vx + wind * 0.5;
            if surface_below(w, x, sy - 0.5).is_some_and(|s| (s - sy).abs() < 1.0) {
                continue;
            }
            on = None;
            vy = 0.0;
            continue;
        }
        vy += g;
        let (nx, ny) = (x + vx + wind, y + vy);
        if vy >= 0.0 && surface_below(w, nx, y).is_some_and(|s| s <= ny) {
            return true;
        }
        x = nx;
        y = ny;
        if y > bottom {
            return false;
        }
    }
    true
}

/// Does player `i`'s body at `(x, y)` moving at `(vx, vy)` catch a ledge
/// within `ticks` (stage units, ballistic)? Its facing stays as it is.
pub fn grabs(
    w: &World,
    i: usize,
    [mut x, mut y]: [f32; 2],
    [vx, mut vy]: [f32; 2],
    g: f32,
    ticks: u32,
) -> bool {
    let p = &w.players[i];
    if crate::ledge::edges(w).is_none() || p.ledge_grabs >= crate::ledge::REGRABS {
        return false;
    }
    let bottom = w.stage.arena.blastzone.map_or(400.0, |b| b[3]);
    let wind = crate::hazards::wind(w).0;
    for t in 0..ticks {
        vy += g;
        x += vx + wind;
        y += vy;
        if vy >= 0.0
            && t as i32 >= p.ledge_cool
            && crate::ledge::window(w, i, x, y)
                .is_some_and(|side| crate::ledge::faces(p.facing, vx, side))
        {
            return true;
        }
        if y > bottom {
            return false;
        }
    }
    false
}

/// On the ledge: wait a moment (shorter at higher levels), then climb, or
/// roll / jump past an opponent waiting at the edge. A fresh press each
/// time (the ledge reads presses made after the grab).
fn ledge_choice(w: &World, i: usize, h: &crate::ledge::Hang) -> Buttons {
    use crate::ledge::{ACT_AFTER, LedgeAction};
    if h.action != LedgeAction::Hang {
        return Buttons::default();
    }
    let p = &w.players[i];
    let level = p.ai_level.clamp(1, 8);
    let grabbed_at = w.frame.wrapping_sub(h.t as u32);
    let wait = ACT_AFTER + 2 * (9 - level) + roll_at(grabbed_at, i, 6) as i32 % 12;
    if h.t < wait && h.left() > 10 {
        return Buttons::default();
    }
    let toward = -h.side;
    // an opponent standing near this edge: get past it
    let guarded = w.players.iter().enumerate().any(|(k, q)| {
        k != i
            && q.helper_index == 0
            && !q.removed
            && q.alive()
            && q.team != p.team
            && q.state_type != StateType::A
            && (-20.0..90.0).contains(&((q.pos[0] * w.local_scale(k) - h.edge) * toward))
    });
    let mut b = Buttons::default();
    if guarded && level >= 3 {
        if roll_at(grabbed_at, i, 7) < 50 {
            b.x = true;
        } else {
            b = dir_buttons(toward);
        }
    } else {
        b.u = true;
    }
    b
}

/// Deterministic 0..100 roll for player `i` (no `world.rng`, so the
/// character's own `Random` stream is untouched).
fn roll(w: &World, i: usize, salt: u32) -> u32 {
    roll_at(w.frame, i, salt)
}

/// A roll that holds for 32 ticks: a veto decided once is not retried
/// every tick by an AI that asks again every tick.
fn roll_slow(w: &World, i: usize, salt: u32) -> u32 {
    roll_at(w.frame / 32, i, salt)
}

fn roll_at(t: u32, i: usize, salt: u32) -> u32 {
    let mut z = (u64::from(t) << 8) ^ (i as u64) ^ (u64::from(salt) << 40);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) % 100) as u32
}

fn dir_buttons(dir: f32) -> Buttons {
    Buttons {
        l: dir < 0.0,
        r: dir > 0.0,
        ..Default::default()
    }
}

fn with_dirs(mut b: Buttons, d: Buttons) -> Buttons {
    b.u = d.u;
    b.d = d.d;
    b.l = d.l;
    b.r = d.r;
    b
}

/// Where an off-stage fighter should head: the nearest safe point.
fn recovery_target(w: &World, v: &View) -> [f32; 2] {
    let [l, r] = v.span;
    let mut tx = v.x.clamp(l + 30.0, r - 30.0);
    // under the island: get out from under the deck before climbing
    if v.y > 2.0 && v.x > l - 20.0 && v.x < r + 20.0 {
        tx = if v.x < (l + r) * 0.5 {
            l - 45.0
        } else {
            r + 45.0
        };
        return [tx, v.y];
    }
    let mut best = [tx, 0.0];
    let mut bd = (tx - v.x).abs() + (0.0 - v.y).max(0.0) * 0.5;
    for pl in w.stage.platforms.iter().filter(|p| !p.gone) {
        if pl.y < v.y - 60.0 {
            continue; // too high to reach
        }
        let px = v.x.clamp(pl.x1 + 10.0, pl.x2 - 10.0);
        let d = (px - v.x).abs() + (pl.y - v.y).abs() * 0.5;
        if d < bd {
            bd = d;
            best = [px, pl.y];
        }
    }
    best
}

/// Edit CPU player `i`'s buttons for this tick (phase 1 of `World::step`).
pub fn filter(w: &mut World, i: usize, b: Buttons) -> Buttons {
    if !active(w, i) {
        w.players[i].arena.danger = 0;
        w.players[i].arena.script_len = 0;
        w.players[i].arena.last = b;
        return b;
    }
    if let Some(h) = w.players[i].ledge {
        let out = ledge_choice(w, i, &h);
        let st = &mut w.players[i].arena;
        st.danger = 0;
        st.script_len = 0;
        st.last = out;
        if h.t == 0 {
            st.ledges += 1;
        }
        return out;
    }
    let v = View::new(w, i);
    let st = &mut w.players[i].arena;
    st.item_walk = false;
    st.danger = if v.off { st.danger + 1 } else { 0 };
    if !v.airborne {
        st.used_move = false;
    }
    let reacted = st.danger > v.react();
    let mut out = b;

    // a move carrying the fighter away from the stage: cancel it into a fall
    let cancel = {
        let p = &w.players[i];
        v.off
            && w.players[i].arena.danger > v.react() / 2
            && p.arena.episode.is_some()
            && p.move_type != MoveType::H
            && p.bind.is_none()
            && !p.pause_bool
            && p.state_owner == p.data
            && v.vx * outward(v.span, v.x) > 0.3
    };
    if cancel && roll(w, i, 1) < v.chance() {
        cancel_move(w, i, &v, 50, true);
    }

    let p = &w.players[i];
    if v.off {
        let [tx, ty] = recovery_target(w, &v);
        let dx = tx - v.x;
        let toward = if dx.abs() < 4.0 { 0.0 } else { dx.signum() };
        if p.move_type == MoveType::H {
            // tech out of the hit fall as soon as the character allows
            let t = w.frame % 6;
            out = dir_buttons(toward);
            out.x = t == 0;
            out.y = t == 0;
            out.a = t == 3;
            out.b = t == 3;
        } else if reacted {
            out = recover(w, i, &v, tx, ty, toward);
            if w.players[i].arena.danger == v.react() + 1 {
                w.players[i].arena.recoveries += 1;
            }
        } else {
            // before reacting: at least never hold away or fast fall
            if outward(v.span, v.x) > 0.0 {
                out.r = false;
            } else {
                out.l = false;
            }
            out.d = false;
        }
    } else if !v.airborne {
        out = ground(w, i, &v, b);
    } else {
        out = air_filter(w, i, &v, b);
        out = climb(w, i, &v, out);
    }
    if !v.airborne && w.players[i].state_no != 40 {
        w.players[i].arena.climb = None;
    }

    let st = &mut w.players[i].arena;
    if st.scripted() {
        let d = Buttons::from_bits(st.script[usize::from(st.script_pos)]);
        st.script_pos += 1;
        out = with_dirs(out, d);
        if v.off {
            out.d = false;
        }
    }
    if !v.airborne {
        // a walk to an item may go closer to the edge (a walk, not a run)
        let margin = if w.players[i].arena.item_walk {
            40.0
        } else {
            v.margin()
        };
        out = edge_filter_with(w, i, &v, out, margin);
    }
    let st = &mut w.players[i].arena;
    st.last = out;
    out
}

/// Velocity (stage units, absolute) an air jump gives player `i` now when
/// holding `hold` (-1 / 0 / +1): the common state 45 rules.
fn air_jump_vel(w: &World, i: usize, hold: f32) -> [f32; 2] {
    let p = &w.players[i];
    let data = &w.data[p.data];
    let k = |n: &str, d: f32| data.konst(n).unwrap_or(d);
    let s = w.local_scale(i);
    let x = if hold == 0.0 {
        k("velocity.airjump.neu.x", 0.0)
    } else if hold == p.facing {
        k("velocity.airjump.fwd.x", 2.5)
    } else {
        k("velocity.airjump.back.x", -2.55)
    };
    [x * p.facing * s, k("velocity.airjump.y", -8.1) * s]
}

/// Can player `i` air jump this tick (control, jumps left, up released)?
fn can_air_jump(w: &World, i: usize) -> bool {
    let p = &w.players[i];
    let jumps =
        w.data[p.data].konst("movement.airjump.num").unwrap_or(1.0) as i32 + w.movement.air_jumps;
    p.ctrl && p.air_jump_count < jumps && !p.arena.last.u
}

/// Does an engine air dash toward `dir` now (hover 10 ticks, then a third
/// of the speed) come down on something?
fn dash_lands(w: &World, i: usize, v: &View, dir: f32) -> bool {
    let p = &w.players[i];
    let speed = w.data[p.data]
        .konst("velocity.run.fwd.x")
        .unwrap_or(4.6)
        .abs()
        .max(2.0)
        * w.local_scale(i);
    let after = [v.x + dir * speed * 10.0, v.y];
    lands(w, after, [dir * speed * 0.35, 0.0], v.g, 0, 150)
}

/// In the air over the stage: drop directions whose air jump or air dash
/// would carry the fighter off (MUGEN has no air steering, so holding a
/// direction does nothing else up here).
fn air_filter(w: &World, i: usize, v: &View, mut out: Buttons) -> Buttons {
    let p = &w.players[i];
    // common state 45 reads the held direction until its velocity is set
    // (time 2): a direction held then still picks the jump
    let airjump_start = p.state_no == 45 && p.state_time <= 2 && p.state_owner == p.data;
    let jump = (out.u && can_air_jump(w, i)) || airjump_start;
    let dash = w.movement.air_dash && p.ctrl && !p.air_dash_used;
    for dir in [-1.0f32, 1.0] {
        let held = if dir < 0.0 { out.l } else { out.r };
        if !held {
            continue;
        }
        let vel = air_jump_vel(w, i, dir);
        let bad = (jump
            && !lands(w, [v.x, v.y], vel, v.g, 0, 150)
            && !grabs(w, i, [v.x, v.y], vel, v.g, 150))
            || (dash && !dash_lands(w, i, v, dir));
        if bad {
            if dir < 0.0 {
                out.l = false;
            } else {
                out.r = false;
            }
        }
    }
    if jump
        && !airjump_start
        && !out.l
        && !out.r
        && !lands(w, [v.x, v.y], air_jump_vel(w, i, 0.0), v.g, 0, 150)
    {
        out.u = false;
    }
    out
}

/// A navigation jump that will not reach its platform: air jump near the
/// top of the arc, toward the opponent.
fn climb(w: &World, i: usize, v: &View, out: Buttons) -> Buttons {
    let Some([ox, oy]) = w.players[i].arena.climb else {
        return out;
    };
    if v.vy < -1.5 || v.y < oy - 10.0 || !can_air_jump(w, i) {
        return out;
    }
    let dx = ox - v.x;
    let hold = if dx.abs() < 20.0 { 0.0 } else { dx.signum() };
    let vel = air_jump_vel(w, i, hold);
    if !lands(w, [v.x, v.y], vel, v.g, 0, 150) {
        return out;
    }
    let mut j = with_dirs(out, dir_buttons(hold));
    j.u = true;
    j
}

/// Recovery inputs: steer to the target, air jump, air dash.
fn recover(w: &mut World, i: usize, v: &View, tx: f32, ty: f32, toward: f32) -> Buttons {
    let p = &w.players[i];
    let data = &w.data[p.data];
    let k = |n: &str, d: f32| data.konst(n).unwrap_or(d);
    let mut out = dir_buttons(toward);
    let jumps = k("movement.airjump.num", 1.0) as i32 + w.movement.air_jumps;
    let left = jumps - p.air_jump_count;
    let dx = (tx - v.x).abs();
    let below = v.y - ty; // > 0: under the target level
    let falling = v.vy > 0.0;
    let last = p.arena.last;
    let st_free = !p.arena.scripted();
    let dash_ok = w.movement.air_dash && p.ctrl && !p.air_dash_used && st_free && v.level >= 2;
    let pos = [v.x, v.y];
    if can_air_jump(w, i) {
        // an air jump that brings the fighter down on something, or to
        // the ledge: take it
        for hold in [toward, 0.0] {
            let vel = air_jump_vel(w, i, hold);
            if lands(w, pos, vel, v.g, 0, 150) || grabs(w, i, pos, vel, v.g, 150) {
                let mut j = dir_buttons(hold);
                j.u = true;
                return j;
            }
        }
    }
    if dash_ok && toward != 0.0 && dash_lands(w, i, v, toward) {
        let d = dir_buttons(toward);
        w.players[i]
            .arena
            .play(&[Buttons::default(), d, Buttons::default(), d]);
        return out;
    }
    // the fighter's own air moves that carried it far: fly home with one
    if p.ctrl && !p.arena.used_move && v.level >= 2 && toward != 0.0 {
        let best = p
            .arena
            .memo
            .iter()
            .filter(|m| {
                m.uses > 0
                    && m.air
                    && !m.teleport
                    && m.fwd > 60.0
                    && m.sink < 60.0
                    && w.data[p.data].states.contains_key(&m.state)
            })
            .max_by(|a, b| a.fwd.total_cmp(&b.fwd))
            .copied();
        if let Some(m) = best
            && m.fwd >= dx * 0.6
        {
            let st = &mut w.players[i].arena;
            st.used_move = true;
            st.recoveries += 1;
            if w.players[i].facing != toward {
                w.players[i].set_facing(toward);
            }
            if let Some(e) = w.players[i].arena.episode.take() {
                w.players[i].arena.learn(&e, e.off);
            }
            w.change_state(i, m.state, None, None);
            w.players[i].arena.episode = Some(Episode {
                state: m.state,
                x: v.x,
                y: v.y,
                air: true,
                facing: toward,
                ..Default::default()
            });
            return out;
        }
    }
    // nothing lands yet: climb when falling low, dash when far out
    if p.ctrl && left > 0 && !last.u {
        // keep the last jump for when it is needed
        let need = if left > 1 {
            falling && (below > -40.0 || dx > 140.0)
        } else {
            falling && (below > -10.0 || dx > 200.0 && below > -60.0)
        };
        if need {
            out.u = true;
            return out;
        }
    }
    if dash_ok && dx > 90.0 && below < 60.0 {
        let d = dir_buttons(toward);
        w.players[i]
            .arena
            .play(&[Buttons::default(), d, Buttons::default(), d]);
    }
    out
}

/// Edge safety: no walking, running or jumping toward a drop into the pit.
fn edge_filter(w: &World, i: usize, v: &View, out: Buttons) -> Buttons {
    edge_filter_with(w, i, v, out, v.margin())
}

/// [`edge_filter`] keeping `margin` from the drop.
fn edge_filter_with(w: &World, i: usize, v: &View, mut out: Buttons, margin: f32) -> Buttons {
    // a jump's direction is read through the squat (state 40)
    let jumping = out.u || w.players[i].state_no == 40;
    let reach = if jumping {
        v.jump + margin * 0.5
    } else {
        margin
    };
    for dir in [-1.0f32, 1.0] {
        let held = if dir < 0.0 { out.l } else { out.r };
        if held && surface_below(w, v.x + dir * reach, v.y).is_none() {
            if dir < 0.0 {
                out.l = false;
            } else {
                out.r = false;
            }
        }
    }
    out
}

/// Grounded filtering: edge safety, platform navigation, corner escape.
fn ground(w: &mut World, i: usize, v: &View, b: Buttons) -> Buttons {
    let out = edge_filter(w, i, v, b);
    let p = &w.players[i];
    // already sliding outward near the drop (a run, a moving move, a
    // landing with momentum): stop
    let horizon = 8 + v.react() as u32;
    let stop = p.move_type != MoveType::H
        && p.bind.is_none()
        && !p.pause_bool
        && p.state_owner == p.data
        && v.vx.abs() > 1.0
        && !lands(w, [v.x, v.y], [v.vx, 0.0], v.g, horizon, horizon + 60);
    if !stop {
        w.players[i].arena.edge = 0;
    } else {
        let imminent = !lands(w, [v.x, v.y], [v.vx, 0.0], v.g, 4, 64);
        let st = &mut w.players[i].arena;
        st.edge += 1;
        if (imminent || st.edge > v.react() / 3) && roll(w, i, 2) < v.chance() {
            cancel_move(w, i, v, 0, false);
            w.players[i].arena.edge = 0;
        }
        return out;
    }
    let p = &w.players[i];
    if !p.ctrl || !matches!(p.state_no, 0 | 20) || p.arena.scripted() {
        return out;
    }
    // a platform about to drop: off it, down through it onto what is below
    if let Some(k) = platform_at(w, v.x, v.y)
        && platform_unsafe(w, k)
        && surface_below(w, v.x, v.y + 1.0).is_some()
    {
        let down = Buttons {
            d: true,
            ..Default::default()
        };
        w.players[i].arena.hazards += 1;
        w.players[i]
            .arena
            .play(&[Buttons::default(), down, Buttons::default(), down]);
        return Buttons::default();
    }
    // a gust toward a nearby drop: lean into it
    let (push, _) = crate::hazards::wind(w);
    if push != 0.0 && v.on_deck {
        let edge = if push > 0.0 { v.span[1] } else { v.span[0] };
        if (edge - v.x).abs() < 260.0 {
            w.players[i].arena.hazards += 1;
            return with_dirs(out, dir_buttons(-push.signum()));
        }
    }
    let Some(([ox, oy], opp_grounded)) = v.opp else {
        return out;
    };
    if let Some(b) = item_moves(w, i, v, out, [ox, oy]) {
        if b.l || b.r {
            let st = &mut w.players[i].arena;
            st.items += 1;
            st.item_walk = true;
        }
        return edge_filter_with(w, i, v, b, 40.0);
    }
    let dx = ox - v.x;
    let toward = if dx.abs() < 25.0 { 0.0 } else { dx.signum() };
    // platform navigation: follow the opponent to its tier
    if v.level >= 3 && opp_grounded && roll(w, i, 3) < (v.level as u32 - 1) {
        // never onto a platform about to drop
        let target_ok = platform_at(w, ox, oy).is_none_or(|k| !platform_unsafe(w, k));
        let jump_up = oy < v.y - 40.0 && dx.abs() < 260.0 && oy > v.y - 170.0 && target_ok;
        let drop = oy > v.y + 40.0 && !v.on_deck;
        if jump_up {
            let mut up = dir_buttons(toward);
            up.u = true;
            w.players[i].arena.play(&[up, up, up]);
            w.players[i].arena.climb = Some([ox, oy]);
            w.players[i].arena.navs += 1;
            return with_dirs(out, up);
        }
        if drop {
            let down = Buttons {
                d: true,
                ..Default::default()
            };
            let walk = dir_buttons(toward);
            w.players[i].arena.navs += 1;
            w.players[i].arena.play(&[
                Buttons::default(),
                down,
                Buttons::default(),
                down,
                walk,
                walk,
            ]);
            return out;
        }
    }
    // cornered: the opponent sits between the fighter and the centre
    let [l, r] = v.span;
    let centre = (l + r) * 0.5;
    let cdir = (centre - v.x).signum();
    let near_edge = (v.x - l).min(r - v.x) < 220.0;
    if v.level >= 2
        && v.on_deck
        && near_edge
        && dx * cdir > 0.0
        && dx.abs() < 150.0
        && roll(w, i, 4) < (v.level as u32 / 2)
    {
        let mut up = dir_buttons(cdir);
        up.u = true;
        w.players[i].arena.play(&[up, up, up]);
        return with_dirs(out, up);
    }
    out
}

/// Arena items, for a grounded CPU with control: throw a carried item at an
/// opponent in front on its level, pick up a useful item in reach, or walk
/// to one lying on its surface while the opponent is not close.
fn item_moves(w: &World, i: usize, v: &View, out: Buttons, [ox, oy]: [f32; 2]) -> Option<Buttons> {
    use crate::items::{ItemKind, ItemState};
    if !crate::items::enabled(w) || v.level < 2 {
        return None;
    }
    let p = &w.players[i];
    // B must be a fresh press: release it for a tick first
    let press = |mut b: Buttons| {
        b.b = !p.item_b_last;
        b
    };
    let dx = ox - v.x;
    if w.items.held_by(i).is_some() {
        let in_front = dx * p.facing > 40.0 && dx.abs() < 320.0 && (oy - v.y).abs() < 50.0;
        return (in_front && roll(w, i, 8) < 10 + 5 * v.level as u32)
            .then(|| press(Buttons::default()));
    }
    let useful = |k: ItemKind| match k {
        ItemKind::Heal => p.life < p.life_max * 9 / 10,
        ItemKind::Power => p.power < p.power_max,
        ItemKind::Throwable => true,
    };
    if let Some(k) = w.item_in_reach(i)
        && useful(w.items.list[k].kind)
    {
        return Some(press(Buttons::default()));
    }
    let hurt = p.life < p.life_max / 2;
    let target = w
        .items
        .list
        .iter()
        .filter(|it| matches!(it.state, ItemState::Ground { .. }) && useful(it.kind))
        .filter(|it| (it.pos[1] + crate::items::HALF - v.y).abs() < 4.0)
        .filter(|it| (it.pos[0] - v.x).abs() < 260.0)
        .filter(|_| dx.abs() > 110.0 || hurt)
        .min_by(|a, b| (a.pos[0] - v.x).abs().total_cmp(&(b.pos[0] - v.x).abs()))?;
    let dir = (target.pos[0] - v.x).signum();
    Some(with_dirs(out, dir_buttons(dir)))
}

/// The platform a body at `(x, y)` stands on (stage units), if any.
fn platform_at(w: &World, x: f32, y: f32) -> Option<usize> {
    w.stage
        .platforms
        .iter()
        .position(|pl| !pl.gone && (pl.y - y).abs() < 1.0 && x >= pl.x1 && x <= pl.x2)
}

/// Platform `k` drops soon (or is gone).
fn platform_unsafe(w: &World, k: usize) -> bool {
    use crate::hazards::DropPhase;
    w.stage
        .motions
        .iter()
        .filter(|m| m.platform == k)
        .any(|m| !matches!(m.drop_phase(w.hazard_t), DropPhase::Solid))
}

/// Cancel the running move into `state` with control, killing outward drift.
fn cancel_move(w: &mut World, i: usize, v: &View, state: i32, air: bool) {
    let own = w.players[i].data;
    if !w.data[own].states.contains_key(&state) {
        return;
    }
    // drift home at half the jump's speed (local units)
    let drift = w.data[own]
        .konst("velocity.jump.fwd.x")
        .unwrap_or(2.5)
        .abs()
        * 0.5;

    let ep = w.players[i].arena.episode.take();
    if let Some(e) = ep {
        w.players[i].arena.learn(&e, true);
    }
    w.change_state(i, state, None, Some(true));
    let p = &mut w.players[i];
    // a small drift back toward the stage (velocity is facing-relative)
    let back = -outward(v.span, v.x) * p.facing;
    p.vel[0] = if air { back * drift } else { 0.0 };
    if air {
        p.vel[1] = p.vel[1].min(0.0);
    }
    p.arena.cancels += 1;
    p.arena.danger = 0;
}

/// May the character's AI `ChangeState` player `i` to `to`? Vetoes moves
/// known to carry the fighter off the stage from here. A move started from
/// a neutral state (`ctrl`) or chained from an attack opens a segment that
/// [`observe`] measures.
pub fn allow_change(w: &mut World, i: usize, to: i32) -> bool {
    if !active(w, i) {
        return true;
    }
    let p = &w.players[i];
    if p.move_type == MoveType::H || p.state_owner != p.data {
        return true;
    }
    if basic(to) || to == p.state_no {
        return true;
    }
    let neutral = p.ctrl;
    if !neutral && p.move_type != MoveType::A && p.arena.episode.is_none() {
        return true;
    }
    let v = View::new(w, i);
    let facing = p.facing;
    let memo = p.arena.memo(to).copied();
    let risky = if v.off {
        // recovering: only moves known to travel back toward the stage
        let home = -outward(v.span, v.x);
        neutral && !memo.is_some_and(|m| m.fwd * facing * home > 30.0)
    } else if memo.is_some_and(|m| m.teleport && m.bad > 0) {
        // a teleport that has dropped the fighter in the pit: its target is
        // unknowable, so not again this match
        true
    } else if let Some(m) = memo {
        let margin = v.margin() * if m.bad > 0 { 2.0 } else { 1.0 } * 0.5;
        // long dashes often home in on the opponent: they end past it
        let mut fwd = m.fwd;
        if let Some(([ox, _], _)) = v.opp
            && fwd > 120.0
            && (ox - v.x) * facing > 0.0
        {
            fwd = fwd.max((ox - v.x).abs() + 60.0);
        }
        [fwd, m.back].iter().any(|&d| {
            if d.abs() < 8.0 {
                return false;
            }
            let end = v.x + facing * (d + d.signum() * margin);
            surface_below(w, end, v.y.min(0.0)).is_none()
        })
    } else {
        false
    };
    // a chain step held back too long would leave the move stuck: let it go
    let stuck = !neutral && p.arena.veto_state == p.state_no && p.state_time > 45;
    if risky && !stuck && roll_slow(w, i, 5) < v.chance() {
        let from = w.players[i].state_no;
        let st = &mut w.players[i].arena;
        st.vetoes += 1;
        st.veto_state = if neutral { -1 } else { from };
        return false;
    }
    let st = &mut w.players[i].arena;
    if let Some(e) = st.episode.take() {
        st.learn(&e, e.off);
    }
    st.episode = Some(Episode {
        state: to,
        x: v.x,
        y: v.y,
        air: v.airborne,
        facing,
        ..Default::default()
    });
    true
}

/// End of tick: track open move episodes and learn from them.
pub fn observe(w: &mut World) {
    for i in 0..w.players.len() {
        let p = &w.players[i];
        if p.helper_index != 0 || p.removed || p.ai_level <= 0 || !w.arena_ai {
            continue;
        }
        let stocks = p.stocks;
        let ringout = stocks < p.arena.prev_stocks || (!p.alive() && p.arena.episode.is_some());
        let x = p.pos[0] * w.local_scale(i);
        let st = &mut w.players[i].arena;
        st.dx = st.last_x.map_or(0.0, |l| x - l);
        st.last_x = Some(x);
        let Some(mut e) = st.episode else {
            st.prev_stocks = stocks;
            continue;
        };
        let v = View::new(w, i);
        let p = &w.players[i];
        let d = (v.x - e.x) * e.facing;
        e.ticks += 1;
        e.teleport |= !ringout && w.players[i].arena.dx.abs() > 60.0;
        if !ringout {
            e.sink = e.sink.max(v.y - e.y);
        }
        if !ringout {
            e.fwd = e.fwd.max(d);
            e.back = e.back.min(d);
        }
        e.off |= v.off;
        // the drift after an air move counts toward its travel: close on landing
        let settled = p.state_type != StateType::A;
        let done = ringout
            || (e.ticks > 1 && (p.ctrl || basic(p.state_no)) && settled)
            || p.move_type == MoveType::H
            || e.ticks > 600;
        let st = &mut w.players[i].arena;
        if done {
            st.episode = None;
            st.learn(&e, e.off || ringout);
        } else {
            st.episode = Some(e);
        }
        st.prev_stocks = stocks;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Arena, Platform};
    use crate::{CharData, Stage};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn kfm() -> Option<Arc<CharData>> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1/chars/kfm/kfm.def");
        p.exists()
            .then(|| CharData::load(&p).unwrap_or_else(|e| panic!("load kfm: {e}")))
    }

    /// A small island (floor -300..300) over a pit, KFM CPU (P1, level
    /// `level`) against an idle human P2 parked on the far side.
    fn island(level: i32, plugin: bool, seed: u64) -> Option<World> {
        let d = kfm()?;
        let stage = Stage {
            arena: Arena {
                blastzone: Some([-700.0, -500.0, 700.0, 200.0]),
                walls: false,
                stocks: 3,
                floor: Some([-300.0, 300.0]),
                air_jumps: Some(1),
                ..Default::default()
            },
            left_bound: -2000.0,
            right_bound: 2000.0,
            ..Default::default()
        };
        let mut w = World::new(stage, seed);
        w.add_player(d.clone(), 0, 1);
        w.add_player(d, 1, 2);
        w.movement.air_jumps = 1;
        w.arena_ai = plugin;
        w.players[0].ai_level = level;
        w.round.time_left = -1;
        w.start_round();
        w.round.time_left = -1;
        while w.round.round_state() != 2 {
            w.step(&[]);
        }
        Some(w)
    }

    fn place(w: &mut World, i: usize, x: f32, y: f32, facing: f32) {
        let p = &mut w.players[i];
        p.pos = [x, y];
        p.old_pos = p.pos;
        p.vel = [0.0, 0.0];
        p.set_facing(facing);
        p.arena.last_x = None;
    }

    /// Steps until `n` ticks pass; returns the number of stocks P1 lost.
    fn run(w: &mut World, n: usize) -> i32 {
        let s0 = w.players[0].stocks;
        for _ in 0..n {
            // P2 stays put far away so nobody hits P1
            w.players[1].pos = [-250.0, 0.0];
            w.players[1].vel = [0.0, 0.0];
            w.step(&[]);
            if w.round.round_state() != 2 {
                break;
            }
        }
        s0 - w.players[0].stocks
    }

    #[test]
    fn cpu_near_the_edge_does_not_walk_off() {
        for seed in [3, 11, 29] {
            let Some(mut w) = island(4, true, seed) else {
                return;
            };
            place(&mut w, 0, 250.0, 0.0, 1.0);
            let lost = run(&mut w, 1800);
            assert_eq!(lost, 0, "seed {seed}: the CPU ran off the island");
        }
        // control: the same random button jamming falls off without the plugin
        let mut fell = 0;
        for seed in [3, 11, 29] {
            let Some(mut w) = island(4, false, seed) else {
                return;
            };
            place(&mut w, 0, 250.0, 0.0, 1.0);
            fell += run(&mut w, 1800);
        }
        assert!(fell > 0, "the control CPUs should fall without the plugin");
    }

    #[test]
    fn level_one_still_does_not_walk_off() {
        let Some(mut w) = island(1, true, 5) else {
            return;
        };
        place(&mut w, 0, -260.0, 0.0, -1.0);
        assert_eq!(run(&mut w, 1800), 0);
    }

    #[test]
    fn off_stage_cpu_recovers_onto_the_floor() {
        // out past the right edge, facing away and toward the stage
        for (x, facing) in [(340.0, 1.0), (340.0, -1.0), (-345.0, 1.0)] {
            let Some(mut w) = island(4, true, 7) else {
                return;
            };
            place(&mut w, 0, x, -60.0, facing);
            w.change_state(0, 50, None, Some(true));
            let mut landed = false;
            for _ in 0..240 {
                w.players[1].pos = [0.0, 0.0];
                w.step(&[]);
                let p = &w.players[0];
                if p.state_type != StateType::A && p.ground == 0.0 && p.stocks == 3 {
                    landed = true;
                    break;
                }
            }
            let p = &w.players[0];
            assert!(
                landed,
                "x {x} facing {facing}: still at {:?} state {} stocks {}",
                p.pos, p.state_no, p.stocks
            );
            assert!(p.arena.recoveries > 0);
        }
    }

    /// Out of air jumps below the deck: the CPU catches the ledge and
    /// climbs back on.
    #[test]
    fn spent_cpu_recovers_through_the_ledge() {
        for (x, facing) in [(330.0, -1.0), (-330.0, 1.0)] {
            let Some(mut w) = island(4, true, 7) else {
                return;
            };
            place(&mut w, 0, x, 20.0, facing);
            w.change_state(0, 50, None, Some(true));
            w.players[0].air_jump_count = 9;
            w.players[0].air_dash_used = true;
            let mut back = false;
            for _ in 0..300 {
                w.players[1].pos = [0.0, 0.0];
                w.step(&[]);
                let p = &w.players[0];
                if p.state_type != StateType::A && p.ground == 0.0 && p.stocks == 3 {
                    back = true;
                    break;
                }
            }
            let p = &w.players[0];
            assert!(
                back,
                "x {x}: at {:?} state {} stocks {}",
                p.pos, p.state_no, p.stocks
            );
            assert_eq!(p.arena.ledges, 1, "x {x}: through the ledge");
            assert!(p.pos[0].abs() < 300.0);
        }
    }

    /// A hurt CPU walks to a heal and takes it; one carrying a throwable
    /// throws it at the opponent in front.
    #[test]
    fn cpu_takes_a_heal_and_throws_items() {
        use crate::items::{HALF, Item, ItemKind, ItemState};
        let Some(mut w) = island(6, true, 7) else {
            return;
        };
        w.stage.arena.items = true;
        w.items.timer = 100_000;
        place(&mut w, 0, 100.0, 0.0, -1.0);
        w.players[0].life = 300;
        w.items.list.push(Item {
            id: 1,
            kind: ItemKind::Heal,
            state: ItemState::Ground { surface: -1 },
            pos: [220.0, -HALF],
            vel: [0.0, 0.0],
            age: 0,
        });
        let mut healed = false;
        for _ in 0..400 {
            w.players[1].pos = [-250.0, 0.0];
            w.step(&[]);
            if w.items.list.is_empty() {
                healed = true;
                break;
            }
        }
        assert!(healed, "P1 at {:?}", w.players[0].pos);
        assert!(w.players[0].life > 300);
        // a carried throwable flies at the opponent
        let x = w.players[0].pos[0];
        w.items.list.push(Item {
            id: 2,
            kind: ItemKind::Throwable,
            state: ItemState::Held { holder: 0 },
            pos: [x, -40.0],
            vel: [0.0, 0.0],
            age: 0,
        });
        let mut thrown = false;
        for _ in 0..600 {
            w.players[1].pos = [x - 150.0, 0.0];
            w.step(&[]);
            if w.items.held_by(0).is_none() {
                thrown = true;
                break;
            }
        }
        assert!(thrown, "never thrown");
    }

    #[test]
    fn off_stage_cpu_falls_without_the_plugin() {
        let Some(mut w) = island(4, false, 7) else {
            return;
        };
        place(&mut w, 0, 340.0, -60.0, 1.0);
        w.change_state(0, 50, None, Some(true));
        let lost = run(&mut w, 240);
        assert_eq!(lost, 1);
    }

    #[test]
    fn humans_and_classic_stages_are_untouched() {
        let Some(mut w) = island(4, true, 7) else {
            return;
        };
        assert!(active(&w, 0));
        assert!(!active(&w, 1), "P2 is human");
        w.stage.arena = Arena::default();
        assert!(!active(&w, 0), "no [Arena]: classic stage");
    }

    #[test]
    fn deterministic_across_rollback() {
        let Some(mut w) = island(6, true, 13) else {
            return;
        };
        w.players[1].ai_level = 3;
        for _ in 0..400 {
            w.step(&[]);
        }
        let mut copy = w.clone();
        for _ in 0..800 {
            w.step(&[]);
            copy.step(&[]);
        }
        assert_eq!(w.checksum(), copy.checksum());
        assert_eq!(w.players[0].arena, copy.players[0].arena);
    }

    /// P1 driven by the plugin alone (no button jamming) with the opponent
    /// parked on a platform one tier up: does P1 get up there?
    fn reaches_platform(level: i32) -> Option<bool> {
        let mut w = island(level, true, 21)?;
        w.stage.platforms.push(Platform {
            x1: -60.0,
            x2: 60.0,
            y: -80.0,
            ..Default::default()
        });
        place(&mut w, 0, -140.0, 0.0, 1.0);
        w.players[0].ai_level = 0;
        for _ in 0..900 {
            let p2 = &mut w.players[1];
            p2.pos = [20.0, -80.0];
            p2.vel = [0.0, 0.0];
            p2.ground = -80.0;
            // the plugin's buttons on top of an idle "character AI"
            w.players[0].ai_level = level;
            let b = filter(&mut w, 0, Buttons::default());
            w.players[0].ai_level = 0;
            w.step(&[b, Buttons::default()]);
            let p = &w.players[0];
            if p.state_type != StateType::A && p.ground == -80.0 {
                return Some(true);
            }
        }
        Some(false)
    }

    #[test]
    fn follows_the_opponent_up_a_platform() {
        let Some(high) = reaches_platform(8) else {
            return;
        };
        assert!(high, "level 8 climbs to the opponent's tier");
        assert_eq!(
            reaches_platform(2),
            Some(false),
            "below level 3: no navigation"
        );
    }

    #[test]
    fn landing_prediction() {
        let Some(mut w) = island(4, true, 7) else {
            return;
        };
        w.stage.platforms.push(Platform {
            x1: 100.0,
            x2: 200.0,
            y: -80.0,
            ..Default::default()
        });
        let g = 0.44;
        assert!(lands(&w, [0.0, 0.0], [0.0, 0.0], g, 10, 100), "standing");
        assert!(!lands(&w, [290.0, 0.0], [4.0, 0.0], g, 10, 200), "runs off");
        assert!(
            lands(&w, [190.0, -80.0], [3.0, 0.0], g, 30, 200),
            "platform to floor"
        );
        assert!(
            !lands(&w, [400.0, -50.0], [1.0, -2.0], g, 0, 200),
            "over the pit"
        );
        assert!(
            lands(&w, [320.0, -50.0], [-4.0, -6.0], g, 0, 200),
            "jumps back"
        );
    }
}
