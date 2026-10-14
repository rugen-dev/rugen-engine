//! Arena items (`[Arena] items = 1`): pickups that drop onto the deck and
//! the platforms every 10–20 s (`item_every = min, max` seconds).
//!
//! - **Throwable**: picked up and carried; the next B throws it forward, a
//!   projectile that knocks down the first opponent it touches.
//! - **Heal**: 30 % of the life bar back at once.
//! - **Power**: one power bar (1000) at once.
//!
//! B picks up an item next to a standing fighter with control (or throws the
//! one carried); that press, held, never reaches the character, so it does
//! not also start a kick. Spawns come from an rng seeded by the match seed,
//! the round number and the round's first frame, never from `World::rng` (the characters' `Random`
//! stays as it was), so netplay, replays and rollback reproduce them. The
//! whole state is plain data in [`World::items`], cloned and checksummed.

use indexmap::IndexMap;
use rugen_cns::state::{MoveType, StateType};

use crate::Rng;
use crate::collision::{self, HitSrc};
use crate::hit::{HitDefSpec, HitType, Reaction, Trade, attr, hf};
use crate::input::Buttons;
use crate::world::World;

/// Items lying around at once.
pub const MAX_ITEMS: usize = 3;
/// Ticks an item lies on the ground before it goes (blinking for the last 2 s).
pub const LIFETIME: i32 = 20 * 60;
/// Half the size of an item (stage units).
pub const HALF: f32 = 8.0;
/// Reach of a pickup: horizontal and vertical distance (stage units).
pub const REACH: [f32; 2] = [34.0, 30.0];
/// A thrown item: speed, gravity, damage and how long it flies.
pub const THROW_VEL: [f32; 2] = [9.0, -2.5];
pub const THROW_GRAVITY: f32 = 0.18;
pub const THROW_DAMAGE: i32 = 70;
pub const THROW_TICKS: i32 = 150;
/// Life a heal gives back (percent of the bar) and power a power item adds.
pub const HEAL_PERCENT: i32 = 30;
pub const POWER_ADD: i32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Throwable,
    Heal,
    Power,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemState {
    /// Dropping onto (or lying on) surface `surface` (-1 = the deck,
    /// k = platform k).
    Ground { surface: i32 },
    /// Carried by player `holder`.
    Held { holder: usize },
    /// In flight, thrown by `owner` toward `facing`.
    Thrown { owner: usize, facing: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub id: u32,
    pub kind: ItemKind,
    pub state: ItemState,
    /// Centre, stage units.
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    /// Ticks in its current state.
    pub age: i32,
}

impl Item {
    /// Lying on the ground, about to go: drawn blinking.
    pub fn expiring(&self) -> bool {
        matches!(self.state, ItemState::Ground { .. }) && self.age > LIFETIME - 120
    }
}

/// The items of a match (part of the world state).
#[derive(Clone, Debug, PartialEq)]
pub struct Items {
    pub list: Vec<Item>,
    /// Seeded from the match seed and the round number.
    pub rng: Rng,
    /// The match seed.
    pub seed: u64,
    /// Ticks until the next drop.
    pub timer: i32,
    pub next_id: u32,
}

impl Items {
    pub fn new(seed: u64) -> Self {
        Self {
            list: Vec::new(),
            rng: Rng::new(seed ^ 0x17E5_A12E_4A11_D00D),
            seed,
            timer: 0,
            next_id: 1,
        }
    }

    /// The item player `i` carries, if any.
    pub fn held_by(&self, i: usize) -> Option<&Item> {
        self.list
            .iter()
            .find(|it| it.state == ItemState::Held { holder: i })
    }

    fn hash(&self) -> u64 {
        let mut h = self.rng.state() ^ (self.timer as u64) << 40 ^ u64::from(self.next_id);
        for it in &self.list {
            let st = match it.state {
                ItemState::Ground { surface } => surface as u64,
                ItemState::Held { holder } => 1000 + holder as u64,
                ItemState::Thrown { owner, .. } => 2000 + owner as u64,
            };
            h = h.rotate_left(7)
                ^ u64::from(it.pos[0].to_bits())
                ^ u64::from(it.pos[1].to_bits()) << 32
                ^ st << 3
                ^ it.kind as u64
                ^ (it.age as u64) << 20;
        }
        h
    }
}

/// Items on this stage?
pub fn enabled(w: &World) -> bool {
    w.stage.arena.items
}

/// Surfaces items drop onto: (surface id, x1, x2, y), stage units.
fn surfaces(w: &World) -> Vec<(i32, f32, f32, f32)> {
    let mut out = Vec::new();
    if let Some([l, r]) = w.stage.arena.floor
        && r - l > 140.0
    {
        out.push((-1, l + 60.0, r - 60.0, 0.0));
    }
    for (k, pl) in w.stage.platforms.iter().enumerate() {
        if pl.x2 - pl.x1 > 40.0 && !pl.gone {
            out.push((k as i32, pl.x1 + 15.0, pl.x2 - 15.0, pl.y));
        }
    }
    out
}

/// Height of surface `surface` now (stage units).
fn surface_y(w: &World, surface: i32) -> Option<f32> {
    if surface < 0 {
        Some(0.0)
    } else {
        w.stage
            .platforms
            .get(surface as usize)
            .filter(|p| !p.gone)
            .map(|p| p.y)
    }
}

impl World {
    /// A new round: no items, a fresh drop timer and rng.
    pub(crate) fn reset_items(&mut self) {
        // the round seed: the match seed, the round number and the frame the
        // round starts on (later rounds start wherever the last one ended)
        let round = u64::try_from(self.round.round_no).unwrap_or(0);
        let frame = u64::from(self.frame);
        let it = &mut self.items;
        it.list.clear();
        it.rng = Rng::new(
            it.seed ^ 0x17E5_A12E_4A11_D00D ^ round.wrapping_mul(0x9E37_79B9) ^ frame << 24,
        );
        it.next_id = 1;
        it.timer = 0;
        if enabled(self) {
            self.items.timer = self.next_drop();
        }
    }

    fn next_drop(&mut self) -> i32 {
        let [lo, hi] = self.stage.arena.item_every;
        let (lo, hi) = (lo.max(1) * 60, hi.max(lo.max(1)) * 60);
        lo + self.items.rng.below((hi - lo + 1) as u32) as i32
    }

    /// Phase 1, after the buttons are known: B picks up or throws an item.
    /// The press is kept from the character until B is released.
    pub(crate) fn item_buttons(&mut self, i: usize, mut b: Buttons) -> Buttons {
        if !enabled(self) {
            return b;
        }
        let p = &mut self.players[i];
        let fresh = b.b && !p.item_b_last;
        p.item_b_last = b.b;
        if p.item_b_mask {
            if b.b {
                b.b = false;
                return b;
            }
            p.item_b_mask = false;
        }
        if !fresh || self.round.round_state() != 2 {
            return b;
        }
        let p = &self.players[i];
        let free = p.ctrl
            && !p.pause_bool
            && p.move_type != MoveType::H
            && p.ledge.is_none()
            && p.bind.is_none()
            && p.state_owner == p.data
            && p.alive();
        if !free {
            return b;
        }
        if let Some(k) = self
            .items
            .list
            .iter()
            .position(|it| it.state == ItemState::Held { holder: i })
        {
            self.throw_item(i, k);
        } else if p.state_type != StateType::A
            && let Some(k) = self.item_in_reach(i)
        {
            self.pick_up(i, k);
        } else {
            return b;
        }
        self.players[i].item_b_mask = true;
        b.b = false;
        b
    }

    /// The nearest item lying within reach of player `i`.
    pub fn item_in_reach(&self, i: usize) -> Option<usize> {
        let p = &self.players[i];
        let s = self.local_scale(i);
        let (x, y) = (p.pos[0] * s, p.pos[1] * s - HALF);
        let carrying = self.items.held_by(i).is_some();
        self.items
            .list
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it.state, ItemState::Ground { .. }))
            .filter(|(_, it)| !(carrying && it.kind == ItemKind::Throwable))
            .filter(|(_, it)| {
                (it.pos[0] - x).abs() <= REACH[0] && (it.pos[1] - y).abs() <= REACH[1]
            })
            .min_by(|a, b| {
                (a.1.pos[0] - x)
                    .abs()
                    .total_cmp(&(b.1.pos[0] - x).abs())
                    .then(a.1.id.cmp(&b.1.id))
            })
            .map(|(k, _)| k)
    }

    fn pick_up(&mut self, i: usize, k: usize) {
        let kind = self.items.list[k].kind;
        match kind {
            ItemKind::Throwable => {
                let it = &mut self.items.list[k];
                it.state = ItemState::Held { holder: i };
                it.age = 0;
                it.vel = [0.0, 0.0];
            }
            ItemKind::Heal => {
                self.items.list.remove(k);
                let p = &mut self.players[i];
                p.life = (p.life + p.life_max * HEAL_PERCENT / 100).min(p.life_max);
            }
            ItemKind::Power => {
                self.items.list.remove(k);
                self.power_add_pub(i, POWER_ADD);
            }
        }
        self.push_sound(crate::world::Event::Sound {
            player: i,
            own: false,
            group: 5,
            number: 0,
            channel: -1,
            volume: 60,
        });
    }

    fn throw_item(&mut self, i: usize, k: usize) {
        let p = &self.players[i];
        let s = self.local_scale(i);
        let facing = p.facing;
        let h = self
            .data_height
            .get(p.data)
            .copied()
            .filter(|h| *h > 0.0)
            .unwrap_or(60.0);
        let at = [p.pos[0] * s + facing * 20.0, p.pos[1] * s - h * 0.6];
        let it = &mut self.items.list[k];
        it.state = ItemState::Thrown { owner: i, facing };
        it.pos = at;
        it.vel = [THROW_VEL[0] * facing, THROW_VEL[1]];
        it.age = 0;
    }

    /// End of the bounds phase: drops, falls, carried items and throws.
    pub(crate) fn step_items(&mut self) {
        if !enabled(self) {
            return;
        }
        if self.round.round_state() == 2 && self.pausetime <= 0 && self.supertime <= 0 {
            self.items.timer -= 1;
            if self.items.timer <= 0 {
                self.items.timer = self.next_drop();
                let lying = self
                    .items
                    .list
                    .iter()
                    .filter(|it| matches!(it.state, ItemState::Ground { .. }))
                    .count();
                if lying < MAX_ITEMS {
                    self.drop_item();
                }
            }
        }
        let mut k = 0;
        while k < self.items.list.len() {
            if self.step_item(k) {
                k += 1;
            } else {
                self.items.list.remove(k);
            }
        }
    }

    fn drop_item(&mut self) {
        let surf = surfaces(self);
        if surf.is_empty() {
            return;
        }
        let rng = &mut self.items.rng;
        let kind = match rng.below(4) {
            0 | 1 => ItemKind::Throwable,
            2 => ItemKind::Heal,
            _ => ItemKind::Power,
        };
        let (surface, x1, x2, y) = surf[rng.below(surf.len() as u32) as usize];
        let x = x1 + rng.below((x2 - x1).max(1.0) as u32) as f32;
        let id = self.items.next_id;
        self.items.next_id += 1;
        self.items.list.push(Item {
            id,
            kind,
            state: ItemState::Ground { surface },
            pos: [x, y - HALF - 80.0],
            vel: [0.0, 0.0],
            age: 0,
        });
    }

    /// One tick of item `k`; false = it is gone.
    fn step_item(&mut self, k: usize) -> bool {
        let mut it = self.items.list[k];
        it.age += 1;
        let keep = match it.state {
            ItemState::Ground { surface } => {
                let Some(y) = surface_y(self, surface) else {
                    return false;
                };
                let rest = y - HALF;
                if it.pos[1] < rest {
                    it.vel[1] = (it.vel[1] + 0.35).min(8.0);
                    it.pos[1] = (it.pos[1] + it.vel[1]).min(rest);
                } else {
                    it.pos[1] = rest;
                    it.vel = [0.0, 0.0];
                }
                it.age < LIFETIME
            }
            ItemState::Held { holder } => {
                let p = &self.players[holder];
                if p.removed || !p.alive() || p.move_type == MoveType::H {
                    // knocked loose: it drops where the holder is
                    let s = self.local_scale(holder);
                    let x = p.pos[0] * s;
                    match self.item_surface_under(x, p.pos[1] * s) {
                        Some(surface) if p.alive() && !p.removed => {
                            it.state = ItemState::Ground { surface };
                            it.age = 0;
                            true
                        }
                        _ => false,
                    }
                } else {
                    let s = self.local_scale(holder);
                    let h = self
                        .data_height
                        .get(p.data)
                        .copied()
                        .filter(|h| *h > 0.0)
                        .unwrap_or(60.0);
                    it.pos = [p.pos[0] * s + p.facing * 14.0, p.pos[1] * s - h * 0.55];
                    true
                }
            }
            ItemState::Thrown { owner, facing } => {
                it.vel[1] += THROW_GRAVITY;
                it.pos = [it.pos[0] + it.vel[0], it.pos[1] + it.vel[1]];
                if let Some(g) = self.item_hits(owner, &it) {
                    self.item_hit(owner, g, it.pos, facing);
                    false
                } else {
                    let below = self
                        .item_surface_under(it.pos[0], it.pos[1] - it.vel[1])
                        .and_then(|s| surface_y(self, s))
                        .is_some_and(|y| it.pos[1] + HALF >= y && it.vel[1] > 0.0);
                    let out =
                        self.stage.arena.blastzone.is_some_and(|b| {
                            it.pos[0] < b[0] || it.pos[0] > b[2] || it.pos[1] > b[3]
                        });
                    !below && !out && it.age < THROW_TICKS
                }
            }
        };
        self.items.list[k] = it;
        keep
    }

    /// The surface right under (or at) `(x, y)`, if any.
    fn item_surface_under(&self, x: f32, y: f32) -> Option<i32> {
        let mut best: Option<(i32, f32)> = None;
        if let Some([l, r]) = self.stage.arena.floor {
            if x >= l && x <= r && y <= 0.5 {
                best = Some((-1, 0.0));
            }
        } else if y <= 0.5 {
            best = Some((-1, 0.0));
        }
        for (k, pl) in self
            .stage
            .platforms
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.gone)
        {
            if x >= pl.x1 && x <= pl.x2 && pl.y >= y - 0.5 && best.is_none_or(|b| pl.y < b.1) {
                best = Some((k as i32, pl.y));
            }
        }
        best.map(|b| b.0)
    }

    /// The first opponent of `owner` a thrown item touches.
    fn item_hits(&self, owner: usize, it: &Item) -> Option<usize> {
        let team = self.players[owner].team;
        let me = [[-HALF, -HALF, HALF, HALF]];
        (0..self.players.len()).find(|&g| {
            let p = &self.players[g];
            if p.helper_index != 0 || p.removed || p.standby || !p.alive() || p.team == team {
                return false;
            }
            let s = self.data_scale(p.data);
            let boxes = collision::clsn_world(self, g, 2);
            crate::hit::boxes_overlap(
                &me,
                it.pos,
                1.0,
                &boxes,
                [p.pos[0] * s, p.pos[1] * s],
                p.facing,
            )
            .is_some()
        })
    }

    /// A thrown item hits `g`: a hard, knock-down hit from `owner`.
    fn item_hit(&mut self, owner: usize, g: usize, pos: [f32; 2], facing: f32) {
        if self.players[g].unhittable_time > 0 {
            return;
        }
        let spec = HitDefSpec {
            attr: attr::ST_S | attr::AT_SP,
            reversal_attr: 0,
            hitflag: hf::H | hf::L | hf::A | hf::F,
            guardflag: hf::H | hf::L | hf::A,
            affectteam: 1,
            animtype: Reaction::Hard,
            air_animtype: None,
            fall_animtype: None,
            ground_type: HitType::High,
            air_type: None,
            trade: Trade::Hit,
            exprs: IndexMap::new(),
        };
        let mut hd = collision::eval_hitdef(self, owner, &spec);
        // velocities are in the thrower's local units
        let ls = 1.0 / self.local_scale(owner);
        hd.damage = THROW_DAMAGE;
        hd.guard_damage = THROW_DAMAGE / 7;
        hd.pausetime = [0, 12];
        hd.guard_pausetime = [0, 8];
        hd.ground_hittime = 18;
        hd.air_hittime = 20;
        hd.ground_velocity = [-5.0 * ls, -4.0 * ls];
        hd.air_velocity = [-4.5 * ls, -5.0 * ls];
        hd.guard_velocity = -4.0 * ls;
        hd.ground_fall = true;
        hd.air_fall = true;
        hd.fall_recover = true;
        let _ = collision::hit_result(self, owner, g, &hd, HitSrc::Item { pos, facing });
    }

    pub(crate) fn items_checksum(&self) -> Option<u64> {
        enabled(self).then(|| self.items.hash())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Two human KFMs on an island with items, round on.
    fn island(seed: u64) -> Option<World> {
        let d = kfm()?;
        let stage = Stage {
            arena: Arena {
                blastzone: Some([-700.0, -500.0, 700.0, 300.0]),
                walls: false,
                stocks: 3,
                floor: Some([-300.0, 300.0]),
                items: true,
                ..Default::default()
            },
            platforms: vec![crate::world::Platform {
                x1: -80.0,
                x2: 80.0,
                y: -80.0,
                ..Default::default()
            }],
            left_bound: -2000.0,
            right_bound: 2000.0,
            ..Default::default()
        };
        let mut w = World::new(stage, seed);
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

    fn put(w: &mut World, kind: ItemKind, x: f32) {
        let id = w.items.next_id;
        w.items.next_id += 1;
        w.items.list.push(Item {
            id,
            kind,
            state: ItemState::Ground { surface: -1 },
            pos: [x, -HALF],
            vel: [0.0, 0.0],
            age: 0,
        });
    }

    const B: Buttons = Buttons {
        u: false,
        d: false,
        l: false,
        r: false,
        a: false,
        b: true,
        c: false,
        x: false,
        y: false,
        z: false,
        s: false,
        t: false,
    };

    fn idle(w: &mut World, n: usize) {
        for _ in 0..n {
            w.step(&[]);
        }
    }

    #[test]
    fn items_drop_every_ten_to_twenty_seconds_from_the_seed() {
        let Some(mut w) = island(3) else { return };
        let t = w.items.timer;
        assert!((600..=1200).contains(&t), "first drop in {t}");
        let mut drops = Vec::new();
        for f in 0..3600 {
            let n = w.items.next_id;
            w.step(&[]);
            if w.items.next_id != n {
                drops.push(f);
            }
        }
        assert!(drops.len() >= 3 && drops.len() <= 6, "{drops:?}");
        for d in drops.windows(2) {
            assert!((600..=1200).contains(&(d[1] - d[0])), "{drops:?}");
        }
        assert!(w.items.list.len() <= MAX_ITEMS);
        // every item came down onto the deck or the platform
        for it in &w.items.list {
            let ItemState::Ground { surface } = it.state else {
                panic!("{it:?}")
            };
            let y = if surface < 0 { 0.0 } else { -80.0 };
            assert_eq!(it.pos[1], y - HALF, "{it:?}");
        }
        // the same seed drops the same items; another seed others
        let Some(mut a) = island(3) else { return };
        let Some(mut b) = island(4) else { return };
        idle(&mut a, 3600);
        idle(&mut b, 3600);
        assert_eq!(a.items, w.items);
        assert_ne!(a.items.list, b.items.list);
    }

    #[test]
    fn classic_stages_have_no_items() {
        let Some(mut w) = island(3) else { return };
        w.stage.arena.items = false;
        w.items.list.clear();
        let before = w.items.clone();
        idle(&mut w, 2000);
        assert!(w.items.list.is_empty());
        assert_eq!(w.items.rng, before.rng);
        assert_eq!(w.items_checksum(), None);
    }

    #[test]
    fn heal_and_power_apply_at_once_and_eat_the_press() {
        let Some(mut w) = island(3) else { return };
        w.players[0].life = 300;
        let x = w.players[0].pos[0];
        put(&mut w, ItemKind::Heal, x + 10.0);
        w.step(&[B, Buttons::default()]);
        assert_eq!(w.players[0].life, 300 + 1000 * HEAL_PERCENT / 100);
        assert!(w.items.list.is_empty());
        // the held B never started a kick
        for _ in 0..10 {
            w.step(&[B, Buttons::default()]);
            assert_eq!(w.players[0].state_no, 0, "B reached the character");
        }
        w.step(&[]);
        put(&mut w, ItemKind::Power, x - 10.0);
        let p0 = w.players[0].power;
        w.step(&[B, Buttons::default()]);
        assert_eq!(w.players[0].power, p0 + POWER_ADD);
        // out of reach: B is a kick again
        w.step(&[]);
        put(&mut w, ItemKind::Heal, x + 200.0);
        w.step(&[B, Buttons::default()]);
        w.step(&[]);
        assert_ne!(w.players[0].state_no, 0, "B kicks when nothing is near");
        assert_eq!(w.items.list.len(), 1);
    }

    #[test]
    fn a_thrown_item_knocks_the_opponent_down() {
        let Some(mut w) = island(3) else { return };
        let x = w.players[0].pos[0];
        put(&mut w, ItemKind::Throwable, x + 10.0);
        w.step(&[B, Buttons::default()]);
        assert!(w.items.held_by(0).is_some(), "carried");
        idle(&mut w, 5);
        let life = w.players[1].life;
        w.step(&[B, Buttons::default()]);
        assert!(matches!(w.items.list[0].state, ItemState::Thrown { .. }));
        let mut hit = false;
        for _ in 0..60 {
            w.step(&[]);
            if w.players[1].move_type == MoveType::H {
                hit = true;
                break;
            }
        }
        assert!(hit, "never hit: {:?}", w.items.list);
        assert!(w.items.list.is_empty(), "spent on the hit");
        idle(&mut w, 2);
        assert!(w.players[1].life < life);
    }

    #[test]
    fn a_hit_knocks_the_item_loose() {
        let Some(mut w) = island(3) else { return };
        let x = w.players[1].pos[0];
        put(&mut w, ItemKind::Throwable, x - 10.0);
        w.step(&[Buttons::default(), B]);
        assert!(w.items.held_by(1).is_some());
        // P1 walks over and kicks
        let mut loose = false;
        for k in 0..240 {
            let b = Buttons {
                r: true,
                b: k % 10 == 0,
                ..Default::default()
            };
            w.step(&[b, Buttons::default()]);
            if w.players[1].move_type == MoveType::H {
                w.step(&[]);
                loose = w.items.held_by(1).is_none();
                break;
            }
        }
        assert!(loose, "still carried");
    }

    #[test]
    fn deterministic_from_a_mid_match_copy() {
        let Some(mut w) = island(9) else { return };
        w.players[0].ai_level = 6;
        w.players[1].ai_level = 6;
        w.arena_ai = true;
        idle(&mut w, 900);
        let mut copy = w.clone();
        for _ in 0..3000 {
            w.step(&[]);
            copy.step(&[]);
        }
        assert_eq!(w.checksum(), copy.checksum());
        assert_eq!(w.items, copy.items);
        // the items are part of the hash
        let mut other = copy.clone();
        other.items.timer += 1;
        assert_ne!(other.checksum(), copy.checksum());
    }
}

#[cfg(test)]
mod bundled {
    use crate::headless::{MatchSpec, build_world};
    use std::path::PathBuf;

    /// The bundled Ink Arena drops items within its first 20 s of fight.
    #[test]
    fn ink_arena_drops_items() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = here.join("../../reference/mugen-1.1b1");
        let kfm = root.join("chars/kfm/kfm.def");
        let stage = here.join("../../motif/stages/ink_arena/ink_arena.def");
        if !kfm.exists() {
            return;
        }
        let spec = MatchSpec {
            life: None,
            p1: &kfm,
            p2: &kfm,
            stage: Some(&stage),
            data_dir: &root.join("data"),
            motif: &root.join("data/system.def"),
            zoom: crate::headless::ONLINE_ZOOM,
            movement: Default::default(),
            round_time: None,
            ai: [0, 0],
            fighter_size: Default::default(),
            seed: crate::headless::LEGACY_SEED,
        };
        let mut w = build_world(&spec).expect("world");
        assert!(w.stage.arena.items);
        let mut seen = 0;
        for _ in 0..1500 {
            w.step(&[]);
            seen = seen.max(w.items.list.len());
        }
        assert!(seen > 0, "timer {}", w.items.timer);
    }

    /// Per-match seed: two matches with different seeds drop different
    /// items (where and what), the same seed drops the same ones.
    #[test]
    fn the_match_seed_shapes_the_drops() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = here.join("../../reference/mugen-1.1b1");
        let kfm = root.join("chars/kfm/kfm.def");
        let stage = here.join("../../motif/stages/ink_arena/ink_arena.def");
        if !kfm.exists() {
            return;
        }
        let data = root.join("data");
        let motif = root.join("data/system.def");
        let drops = |seed: u64| {
            let spec = MatchSpec {
                life: None,
                p1: &kfm,
                p2: &kfm,
                stage: Some(&stage),
                data_dir: &data,
                motif: &motif,
                zoom: crate::headless::ONLINE_ZOOM,
                movement: Default::default(),
                round_time: None,
                ai: [0, 0],
                fighter_size: Default::default(),
                seed,
            };
            let mut w = build_world(&spec).expect("world");
            assert_eq!(w.seed(), seed);
            let mut seen: Vec<(u32, super::ItemKind, i32, u32)> = Vec::new();
            for _ in 0..3000 {
                w.step(&[]);
                for it in &w.items.list {
                    if !seen.iter().any(|(id, ..)| *id == it.id) {
                        seen.push((it.id, it.kind, it.pos[0] as i32, w.frame));
                    }
                }
            }
            seen
        };
        let a = drops(crate::headless::LEGACY_SEED);
        assert!(!a.is_empty());
        assert_eq!(a, drops(crate::headless::LEGACY_SEED));
        let others: Vec<_> = (2..6)
            .map(|m| drops(crate::headless::online_seed(m)))
            .collect();
        assert!(others.iter().any(|b| *b != a), "{a:?} vs {others:?}");
    }
}
