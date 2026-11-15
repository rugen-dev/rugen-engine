//! The lab: before a match, every move of a fighter is tried by itself on
//! the other fighter standing still, and what happened is written down.
//!
//! A card's files say little of a move whose hit comes from something it
//! creates (a projectile, a clone, a summoned toad), nothing of what a
//! second of charging gives, and nothing of which move can follow which.
//! The fighter's author knew all that; a player has to be told, and told
//! the same thing whoever it is. So the gym measures:
//!
//! - whether the move comes out, with full power, on the ground;
//! - the life it takes, in how many hits, and whether it knocks down;
//! - the power it costs or gives, and what holding it a second gives;
//! - how long the fighter is busy with it;
//! - from how far it still hits (tried close, then further and further);
//! - which moves link after it: played the moment it hits, they come out
//!   and hit before the other fighter has recovered.
//!
//! It is a measure, not a promise: a move tried on a standing fighter with
//! full power, in the fighter's first form. The sheet says so.

use serde_json::{Value, json};

use rugen_cns::state::{MoveType, StateType};
use rugen_sim::World;

use crate::brain::{Plan, Step, To};
use crate::{Env, cards};

/// The gaps a move is tried from (stage units), nearest first.
const GAPS: [i32; 4] = [4, 60, 130, 220];
/// A try is given up after this many ticks.
const PATIENCE: u32 = 480;
/// The most moves tried as the first, and as the second, of a link.
const LINKS: usize = 10;

/// What one try showed.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    out: bool,
    damage: i32,
    hits: i32,
    power: i32,
    busy: u32,
    down: bool,
    /// The second move of a link hit while the other fighter had not
    /// recovered from the first.
    linked: bool,
}

struct Lab {
    env: Env,
    side: usize,
    /// The fight at each gap of [`GAPS`] it could be brought to.
    at: Vec<(i32, World)>,
}

impl Lab {
    /// Put the env back on a snapshot, with nothing in progress.
    fn reset(&mut self, world: &World) {
        let e = &mut self.env;
        e.world = world.clone();
        e.run = [None, None];
        e.next = [None, None];
        e.book = [None, None];
        e.held = [None, None];
        e.queue[0].clear();
        e.queue[1].clear();
        e.stance = Default::default();
        e.steps.clear();
        e.feed.clear();
        e.trace = Default::default();
        e.actions = Default::default();
        for i in 0..2 {
            let p = &e.world.players[i];
            e.last_life[i] = p.life;
            e.last_power[i] = p.power;
            e.last_state[i] = p.state_no;
            e.last_state_time[i] = p.state_time;
        }
    }

    /// Bring the fighters to a gap of about `gap` and keep the picture.
    fn place(&mut self, gap: i32) -> Option<World> {
        let side = self.side;
        for _ in 0..600 {
            let d = self.env.distance();
            let stance = if d > gap + 3 {
                cards::Stance::Approach
            } else if d < gap - 3 {
                cards::Stance::BackOff
            } else {
                break;
            };
            self.env.stance[side] = stance;
            if !self.env.fighting() {
                return None;
            }
            self.env.tick();
        }
        self.env.stance[side] = cards::Stance::Wait;
        for _ in 0..40 {
            self.env.tick();
        }
        if (self.env.distance() - gap).abs() > 12 || !self.env.fighting() {
            return None;
        }
        let mut world = self.env.world.clone();
        // full power, full life, and the other fighter cannot be knocked out
        for (i, p) in world.players.iter_mut().enumerate().take(2) {
            p.life = p.life_max;
            if i == side {
                p.power = p.power_max;
            } else {
                p.no_ko = true;
            }
        }
        // the clock does not run out on a long test
        world.round.time_left = -1;
        Some(world)
    }

    /// Play `first` (held `hold` ticks), then `second` the moment it hits.
    fn play(
        &mut self,
        world: &World,
        first: usize,
        hold: Option<u32>,
        second: Option<usize>,
    ) -> Seen {
        self.reset(world);
        let (side, other) = (self.side, 1 - self.side);
        let step = |card, for_ticks, hit| Step {
            card,
            until: None,
            for_ticks,
            gate: None,
            next: Some(To::End),
            hit,
            blocked: Some(To::End),
            missed: Some(To::End),
            otherwise: None,
        };
        let mut steps = vec![step(
            first,
            hold,
            Some(second.map_or(To::End, |_| To::Step(1))),
        )];
        if let Some(b) = second {
            steps.push(step(b, None, Some(To::End)));
        }
        let last = steps.len();
        self.env.next[side] = Some((
            Plan {
                steps,
                rules: Vec::new(),
            },
            0,
        ));
        let (life, power) = {
            let w = &self.env.world;
            (w.players[other].life, w.players[side].power)
        };
        let start = self.env.world.frame;
        let mut seen = Seen::default();
        let mut first_hit = None;
        let mut recovered = false;
        for _ in 0..PATIENCE {
            self.env.tick();
            let w = &self.env.world;
            let (me, he) = (&w.players[side], &w.players[other]);
            seen.hits = seen.hits.max(he.received_hits);
            seen.down |= he.state_type == StateType::L;
            if he.life < life && first_hit.is_none() {
                first_hit = Some(w.frame);
            }
            // between the first hit and the second move's, did he recover?
            if first_hit.is_some() && he.move_type != MoveType::H && !seen.linked {
                recovered = true;
            }
            for s in self.env.steps.drain(..) {
                let (n, result) = (
                    s["step"].as_u64().unwrap_or(0) as usize,
                    s["result"].as_str().unwrap_or(""),
                );
                if n == 1 && matches!(result, "hit" | "blocked" | "missed" | "done") {
                    seen.out = true;
                }
                if n == 2 && n == last && result == "hit" && !recovered {
                    seen.linked = true;
                }
            }
            let done = self.env.run[side].is_none() && self.env.next[side].is_none();
            let calm = he.move_type == MoveType::I && he.state_type != StateType::L;
            if done && me.ctrl && w.frame > start + 12 && (calm || !seen.out) {
                break;
            }
        }
        let w = &self.env.world;
        seen.damage = life - w.players[other].life;
        seen.power = w.players[side].power - power;
        seen.busy = w.frame - start;
        seen
    }
}

/// The move sheet of `side`'s fighter in a match of `cfg`: one entry per
/// card of its deck (`null` for a stance, a move that needs the air, or
/// when the fight could not be set up).
pub fn sheet(cfg: &crate::Config, side: usize) -> Vec<Value> {
    let mut cfg = cfg.clone();
    cfg.cpu = [0, 0];
    cfg.lab = false;
    let Ok(mut env) = Env::new(cfg) else {
        return Vec::new();
    };
    for _ in 0..600 {
        if env.fighting() {
            break;
        }
        env.tick();
    }
    let deck = env.decks[side].clone();
    let mut lab = Lab {
        env,
        side,
        at: Vec::new(),
    };
    for gap in GAPS {
        if let Some(w) = lab.place(gap) {
            lab.at.push((gap, w));
        }
    }
    let Some((_, close)) = lab.at.first().cloned() else {
        return vec![Value::Null; deck.len()];
    };
    let ms = |ticks: u32| ticks * 1000 / 60;
    let mut out: Vec<Value> = Vec::new();
    let mut hitters: Vec<usize> = Vec::new();
    for (k, card) in deck.iter().enumerate() {
        if card.kind == "stance" || card.needs == cards::Needs::Air {
            out.push(Value::Null);
            continue;
        }
        let near = lab.play(&close, k, None, None);
        // from how far it still hits: further and further, until it does not
        let mut far = (near.damage > 0).then_some(GAPS[0]);
        let mut best = near;
        for (gap, world) in lab.at.clone().iter().skip(1) {
            let s = lab.play(world, k, None, None);
            if s.damage <= 0 {
                if far.is_some() {
                    break;
                }
                continue;
            }
            far = Some(*gap);
            if best.damage <= 0 {
                best = s;
            }
        }
        let mut v = json!({ "comes_out": best.out || best.damage > 0 });
        if best.damage > 0 {
            hitters.push(k);
            v["damage"] = best.damage.into();
            v["hits"] = best.hits.max(1).into();
            v["knocks_down"] = best.down.into();
            v["hits_from"] = far.unwrap_or(GAPS[0]).into();
        }
        if best.power != 0 {
            v["power"] = best.power.into();
        }
        if best.out || best.damage > 0 {
            v["busy_ms"] = ms(best.busy).into();
        }
        // a move that takes no life and costs nothing: what holding it gives
        if best.damage <= 0 && best.power >= 0 {
            let held = lab.play(&drained(&close, side), k, Some(60), None);
            if held.power > best.power.max(0) + 20 {
                v["held_1000_ms_power"] = held.power.into();
                v["comes_out"] = true.into();
            }
        }
        out.push(v);
    }
    // which moves link: the first of a pair hits close, the second is
    // played the moment it does
    let firsts: Vec<usize> = hitters
        .iter()
        .copied()
        .filter(|k| out[*k]["hits_from"] == GAPS[0])
        .take(LINKS)
        .collect();
    for &a in &firsts {
        let mut links = Vec::new();
        for &b in hitters.iter().take(LINKS + 6) {
            if lab.play(&close, a, None, Some(b)).linked {
                links.push(deck[b].id.clone());
            }
        }
        if !links.is_empty() {
            out[a]["links_into"] = links.into();
        }
    }
    out
}

/// `world` with the fighter's power at zero (to see what a move gives).
fn drained(world: &World, side: usize) -> World {
    let mut w = world.clone();
    w.players[side].power = 0;
    w
}
