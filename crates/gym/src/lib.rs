//! A match between two outside players (language models, bots), played
//! headless in real time.
//!
//! The clock never waits: the driver ticks the match 60 times a second. A
//! side is shown the state ([`Env::observation`]) as soon as its last answer
//! came in, and answers with a **plan** ([`brain`]): up to ten steps of its
//! fighter's cards ([`cards`]) with the branch to take on each outcome. The
//! match runs the plan by itself, at full speed, while the side thinks
//! about the next one: what is compared is how far ahead a player sees and
//! how well, as much as how fast it answers.
//!
//! A side is told everything that happened and nothing about what to do:
//! both fighters' state, every move each one made by its name with what it
//! did, how often each was used, and how its own plan went, step by step.
//!
//! Every tick's buttons are recorded: the match is saved as an ordinary
//! replay (`rugen replay file.json`), and replaying those inputs gives the
//! same world checksum, whatever the timing was.

pub mod brain;
pub mod cards;
pub mod lab;

use std::collections::VecDeque;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use rugen_cns::state::{MoveType, StateType};
use rugen_sim::command::{CommandDef, StepKey};
use rugen_sim::headless::{MatchSpec, build_world};
use rugen_sim::input::Key;
use rugen_sim::world::Movement;
use rugen_sim::{Buttons, World};
use serde_json::{Value, json};

/// The protocol of the JSON lines (`hello.protocol`).
pub const PROTOCOL: u32 = 2;

/// What a match is played with.
#[derive(Clone, Debug)]
pub struct Config {
    pub p1: PathBuf,
    pub p2: PathBuf,
    pub stage: Option<PathBuf>,
    pub data_dir: PathBuf,
    /// The motif whose `fight.def` gives the round timings. A replay plays
    /// in the game with the bundled default theme's (`motif/themes/ink`).
    pub motif: PathBuf,
    pub seed: u64,
    /// Round clock in seconds (`None`: the motif's).
    pub round_time: Option<i32>,
    /// Every fighter's life (`None`: its own).
    pub life: Option<i32>,
    /// Rounds to win (`None`: the motif's).
    pub match_wins: Option<u8>,
    /// Ticks one step of a plan is held when it gives no `*n`.
    pub step_ticks: u32,
    /// A plan is cut at this many ticks.
    pub max_plan: u32,
    /// The match is stopped (unfinished) at this many ticks.
    pub max_frames: u32,
    /// A side played by the game's own computer player at this level (1 to
    /// 8) instead of an outside one; 0: an outside player.
    pub cpu: [i32; 2],
    /// Measure every move of both fighters before the match ([`lab`]) and
    /// give each side its fighter's sheet.
    pub lab: bool,
}

impl Config {
    pub fn new(p1: PathBuf, p2: PathBuf, data_dir: PathBuf, motif: PathBuf) -> Self {
        Self {
            p1,
            p2,
            stage: None,
            data_dir,
            motif,
            seed: 1,
            round_time: None,
            life: None,
            match_wins: None,
            step_ticks: 2,
            max_plan: 90,
            max_frames: 60 * 60 * 10,
            cpu: [0, 0],
            lab: false,
        }
    }
}

/// The camera every gym match is simulated with: the widest view the
/// game allows, and no closer than 0.64 (the online camera closes in to
/// 1.25, which crops a fight that is watched rather than played). Recorded
/// in the replay, so it plays the same on every machine.
const ZOOM: (f32, f32) = (rugen_sim::headless::ONLINE_ZOOM.0, 0.64);

/// One tick of a plan: directions relative to the facing until applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rel {
    pub u: bool,
    pub d: bool,
    pub f: bool,
    pub b: bool,
    pub l: bool,
    pub r: bool,
    /// a b c x y z s
    pub btn: [bool; 7],
}

impl Rel {
    /// The buttons of this tick for a fighter facing right (`true`) or left.
    pub fn resolve(self, facing_right: bool) -> Buttons {
        let (fwd, back) = (self.f && !self.b, self.b && !self.f);
        let [a, b, c, x, y, z, s] = self.btn;
        Buttons {
            u: self.u,
            d: self.d,
            l: self.l || if facing_right { back } else { fwd },
            r: self.r || if facing_right { fwd } else { back },
            a,
            b,
            c,
            x,
            y,
            z,
            s,
            t: false,
        }
    }
}

/// Parse a plan into its ticks (see the crate docs for the notation).
/// `.cmd` decorations are accepted and dropped (`~D`, `/F`, `$B`, `>x`); a
/// charge time is a hold (`~30B` = `B*30`).
pub fn parse_plan(text: &str, step_ticks: u32, max_plan: u32) -> Result<Vec<Rel>, String> {
    let mut out = Vec::new();
    for raw in text.split(',') {
        let tok: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        if tok.is_empty() {
            continue;
        }
        let (keys, mut ticks) = match tok.split_once('*') {
            Some((k, n)) => (
                k,
                n.parse::<u32>()
                    .map_err(|_| format!("`{tok}`: `*` expects a number of ticks"))?,
            ),
            None => (tok.as_str(), step_ticks),
        };
        let mut rel = Rel::default();
        for key in keys.split('+') {
            let key = key.trim_start_matches(['~', '/', '$', '>']);
            let name = key.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Ok(charge) = key[..key.len() - name.len()].parse::<u32>() {
                ticks = ticks.max(charge);
            }
            match name {
                "N" => {}
                "U" => rel.u = true,
                "D" => rel.d = true,
                "F" => rel.f = true,
                "B" => rel.b = true,
                "L" => rel.l = true,
                "R" => rel.r = true,
                "UF" => (rel.u, rel.f) = (true, true),
                "UB" => (rel.u, rel.b) = (true, true),
                "DF" => (rel.d, rel.f) = (true, true),
                "DB" => (rel.d, rel.b) = (true, true),
                "UL" => (rel.u, rel.l) = (true, true),
                "UR" => (rel.u, rel.r) = (true, true),
                "DL" => (rel.d, rel.l) = (true, true),
                "DR" => (rel.d, rel.r) = (true, true),
                "a" => rel.btn[0] = true,
                "b" => rel.btn[1] = true,
                "c" => rel.btn[2] = true,
                "x" => rel.btn[3] = true,
                "y" => rel.btn[4] = true,
                "z" => rel.btn[5] = true,
                "s" => rel.btn[6] = true,
                other => return Err(format!("`{tok}`: unknown key `{other}`")),
            }
        }
        for _ in 0..ticks.clamp(1, max_plan.max(1)) {
            if out.len() as u32 >= max_plan {
                return Ok(out);
            }
            out.push(rel);
        }
    }
    if out.is_empty() {
        return Err("empty plan".into());
    }
    Ok(out)
}

fn key_name(k: Key) -> &'static str {
    match k {
        Key::U => "U",
        Key::D => "D",
        Key::B => "B",
        Key::F => "F",
        Key::UB => "UB",
        Key::UF => "UF",
        Key::DB => "DB",
        Key::DF => "DF",
        Key::N => "N",
        Key::A => "a",
        Key::Bt => "b",
        Key::C => "c",
        Key::X => "x",
        Key::Y => "y",
        Key::Z => "z",
        Key::S => "s",
    }
}

/// A whole number of stage units (`-0.0` is 0).
fn whole(v: f32) -> i32 {
    libm::roundf(v) as i32
}

/// One decimal, without the `f32` noise a JSON number would carry.
fn tenth(v: f32) -> f64 {
    f64::from(libm::roundf(v * 10.0) as i32) / 10.0
}

/// A command as a plan that performs it: a charged direction is held for
/// its charge time, a released button is followed by a neutral step, and a
/// direction pressed twice in a row is released in between (`F,N,F`).
pub(crate) fn plan_of(def: &CommandDef) -> String {
    let mut steps: Vec<String> = Vec::new();
    for step in &def.steps {
        let keys: Vec<&str> = step.keys.iter().map(|k| key_name(k.key)).collect();
        let charge = step.keys.iter().map(|k| k.charge).max().unwrap_or(0);
        let mut s = keys.join("+");
        // the release of the direction just held
        let release = step.keys.iter().all(|k| k.tilde && k.key.is_direction());
        if release && steps.last() == Some(&s) {
            s = "N".into();
        }
        if charge > 1 {
            s.push_str(&format!("*{}", charge + 2));
        }
        steps.push(s);
        let released = |k: &StepKey| k.tilde && !k.key.is_direction();
        if step.keys.iter().any(released) {
            steps.push("N".into());
        }
    }
    steps.join(",")
}

/// The fighter's motions worth telling a player about: every command of
/// more than one key, once per distinct input (a `.cmd` repeats names for
/// variants), at most `limit`.
pub fn moves(defs: &[CommandDef], limit: usize) -> Vec<Value> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for def in defs {
        let keys: usize = def.steps.iter().map(|s| s.keys.len()).sum();
        if keys < 2 {
            continue;
        }
        let plan = plan_of(def);
        if seen.contains(&plan) {
            continue;
        }
        seen.push(plan.clone());
        out.push(json!({ "name": def.name, "plan": plan }));
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Per-side counters of a match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Plans answered.
    pub plans: u32,
    /// Answers that were not a plan (the one running goes on).
    pub invalid: u32,
    /// Life taken from the other side.
    pub damage: i64,
    /// Ticks spent between an observation and its answer, summed.
    pub think_ticks: u64,
    /// Fight ticks with no plan running and the `wait` stance.
    pub idle_ticks: u32,
    /// Steps of its plans that ran to their end.
    pub steps: u32,
    /// Moves that ended (hit, blocked, missed)...
    pub judged: u32,
    /// ...and of those, the ones whose outcome the plan had a branch for.
    pub foreseen: u32,
    /// Plans a hit ended because no rule said what to do then.
    pub broken: u32,
    /// Plans that ran out of steps.
    pub ran_out: u32,
    /// The most hits it landed in one combo.
    pub combo: i32,
}

/// Something a fighter did, by the name its `.cmd` gives it.
#[derive(Clone, Debug)]
struct Action {
    frame: u32,
    label: String,
    /// Life it took from the other fighter.
    damage: i32,
    /// Power its fighter gained (or, negative, spent) with it.
    power: i32,
    /// The other fighter guarded (some of) it.
    guarded: bool,
}

/// Where a running plan is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The step is to be started.
    Start,
    /// A stance, held since `Run::since`.
    Stance,
    /// A move, played but not seen to come out yet.
    Waiting,
    /// A move that came out, since `Run::since`.
    Out,
}

/// A plan being run.
#[derive(Clone, Debug)]
struct Run {
    plan: brain::Plan,
    /// Its entry in `Env::plans`.
    id: usize,
    at: usize,
    since: u32,
    phase: Phase,
    /// The last tick a waited move may come out on.
    deadline: u32,
    /// The move in progress took life / was guarded.
    landed: bool,
    guarded: bool,
    /// The fighter is being hit (the plan waits for it to be over).
    hurt: bool,
    /// Each rule's condition last tick: a rule fires when it becomes true.
    edges: Vec<bool>,
    /// The step in hand was reached by a hit: its move is played at once,
    /// while the last one is still connecting (a cancel).
    chain: bool,
    /// The rule of the side's brain these moves are (a plan: none).
    rule: Option<usize>,
    /// That move did not come out yet: the times it is pressed again
    /// during the hit (a cancel has its moment, a player taps through it)...
    taps: u8,
    /// ...and it gets one last try, played normally (once the fighter is
    /// free).
    retry: bool,
}

pub struct Env {
    pub cfg: Config,
    pub world: World,
    queue: [VecDeque<Rel>; 2],
    /// Each side's deck and the stance it holds between moves.
    pub decks: [Vec<cards::Card>; 2],
    /// Per card of each deck: what the lab measured of it (`null`: not
    /// measured). Empty when the lab did not run.
    pub sheet: [Vec<Value>; 2],
    /// Per card of each deck: how far past the front of its fighter's body
    /// the move's hitbox goes, in stage units (the measure of `distance`).
    reach: [Vec<Option<i32>>; 2],
    stance: [cards::Stance; 2],
    /// `[p1 bits, p2 bits]` per tick, as a replay stores them.
    pub inputs: Vec<[u16; 2]>,
    pub tally: [Tally; 2],
    /// The tick each side was last shown the state on, until it answers.
    asked: [Option<u32>; 2],
    /// The plan each side is running, and the one that takes over at the
    /// next safe moment (with its entry in `plans`).
    run: [Option<Run>; 2],
    next: [Option<(brain::Plan, usize)>; 2],
    /// The brain each side wrote ([`brain::Book`], with its entry in
    /// `plans`): read every tick while no moves of it are playing.
    book: [Option<(brain::Book, usize)>; 2],
    /// Per rule of that brain: the rule that decides now, the tick each
    /// may fire again from, and how each has done (fired, hit, blocked,
    /// missed, did not come out).
    active: [Option<usize>; 2],
    ready_at: [Vec<u32>; 2],
    record: [Vec<[u32; 5]>; 2],
    /// Why a side's last answer was refused, until it is told.
    refused: [Option<String>; 2],
    /// Every answer, as the replay's viewer shows it: who, when it was
    /// asked for and came in, the steps, the player's own words.
    plans: Vec<Value>,
    /// Every step that ended: its plan, its number, the tick and how.
    steps: Vec<Value>,
    /// The latest of those per side, for its next observation.
    trace: [VecDeque<Value>; 2],
    /// What a live viewer is told as it happens ([`Env::take_feed`]).
    feed: Vec<Value>,
    /// A move waiting for its fighter to be free (it is still in another
    /// move, or being hit): its inputs, and the tick it is tried on anyway.
    /// A player buffers an input the same way.
    held: [Option<(Vec<Rel>, u32)>; 2],
    /// What each fighter did, by name, and how often each move.
    actions: [VecDeque<Action>; 2],
    counts: [Vec<(String, u32)>; 2],
    last_state: [i32; 2],
    /// The tick each fighter last entered a state on (a state entered
    /// again from itself counts: its clock went back), with that clock.
    entered: [u32; 2],
    last_state_time: [i32; 2],
    last_life: [i32; 2],
    last_power: [i32; 2],
    last_round: i32,
}

/// Ticks a move waits for its fighter to be free before it is tried anyway
/// (750 ms).
const HOLD: u32 = 45;
/// The times a move reached by a hit is pressed again while it has not
/// come out (about a third of a second of tapping).
const TAPS: u8 = 4;
/// The longest a move's last input is held (`for_ms` on a move: 5 s).
const HOLD_MAX: u32 = 300;
/// A stance with nothing to end it is held this long (500 ms)...
const STANCE: u32 = 30;
/// ...and one whose condition never comes, this long (8 s).
const STANCE_MAX: u32 = 480;
/// A fighter this much higher than the other is above it; a wall this
/// close behind a fighter corners it (stage units).
const ABOVE: i32 = 40;
const CORNER: i32 = 30;
/// How many actions and plan steps an observation recalls.
const RECALL: usize = 10;

impl Env {
    pub fn new(cfg: Config) -> Result<Self> {
        let spec = MatchSpec {
            p1: &cfg.p1,
            p2: &cfg.p2,
            stage: cfg.stage.as_deref(),
            data_dir: &cfg.data_dir,
            motif: &cfg.motif,
            zoom: ZOOM,
            movement: Movement::default(),
            round_time: cfg.round_time,
            ai: cfg.cpu,
            fighter_size: Default::default(),
            seed: cfg.seed,
            life: cfg.life,
        };
        let mut world = build_world(&spec)
            .with_context(|| format!("{} vs {}", cfg.p1.display(), cfg.p2.display()))?;
        if let Some(w) = cfg.match_wins {
            world.round.config.match_wins = w.max(1);
        }
        let last_life = [world.players[0].life, world.players[1].life];
        let last_state = [world.players[0].state_no, world.players[1].state_no];
        let last_round = world.round.round_no;
        let decks = [(0, &cfg.p1), (1, &cfg.p2)].map(|(i, def)| {
            let p = &world.players[i];
            cards::deck(def, &world.data[p.data], &p.cmds.table.defs, cfg.step_ticks)
        });
        let reach = [0, 1].map(|i| {
            let (front, sc) = (world.player_width(i).0, world.local_scale(i));
            decks[i]
                .iter()
                .map(|c| c.reach.map(|r| whole((r as f32 - front) * sc).max(0)))
                .collect()
        });
        let sheet = if cfg.lab {
            [lab::sheet(&cfg, 0), lab::sheet(&cfg, 1)]
        } else {
            Default::default()
        };
        // a move the lab saw give power while held is a charge
        let mut decks = decks;
        for (deck, sheet) in decks.iter_mut().zip(&sheet) {
            for (card, m) in deck.iter_mut().zip(sheet) {
                if m.get("held_1000_ms_power").is_some() {
                    card.kind = "charge";
                }
                // a move whose files name no price but which spent power
                // when tried: that is what it costs
                let spent = m.get("power").and_then(Value::as_i64).unwrap_or(0);
                if card.power == 0 && spent <= -100 {
                    card.power = -spent as i32;
                }
            }
        }
        Ok(Self {
            sheet,
            reach,
            decks,
            stance: Default::default(),
            cfg,
            world,
            queue: Default::default(),
            inputs: Vec::new(),
            tally: Default::default(),
            asked: [None; 2],
            run: [None, None],
            next: [None, None],
            book: [None, None],
            active: [None; 2],
            ready_at: Default::default(),
            record: Default::default(),
            refused: [None, None],
            plans: Vec::new(),
            steps: Vec::new(),
            trace: Default::default(),
            feed: Vec::new(),
            held: [None, None],
            actions: Default::default(),
            counts: Default::default(),
            last_state,
            entered: [0; 2],
            last_state_time: [0; 2],
            last_life,
            last_power: [0; 2],
            last_round,
        })
    }

    /// A side's position in stage units.
    fn pos(&self, i: usize) -> [f32; 2] {
        let (p, sc) = (&self.world.players[i], self.world.local_scale(i));
        [p.pos[0] * sc, p.pos[1] * sc]
    }

    /// The gap between the two bodies, in stage units (0: touching): the
    /// measure a fighter's own files use for range (`P2BodyDist`).
    fn distance(&self) -> i32 {
        let w = &self.world;
        let front = |i: usize| w.player_width(i).0 * w.local_scale(i);
        whole((self.pos(0)[0] - self.pos(1)[0]).abs() - front(0) - front(1)).max(0)
    }

    /// How far the wall is behind `side`'s fighter (stage units).
    fn wall_behind(&self, side: usize) -> i32 {
        let (left, right) = self.world.play_bounds();
        let x = self.pos(side)[0];
        whole(if self.world.players[side].facing > 0.0 {
            x - left
        } else {
            right - x
        })
        .max(0)
    }

    /// The deck with each move's reach, as the players are told it.
    fn deck_json(&self, side: usize) -> Vec<Value> {
        self.decks[side]
            .iter()
            .zip(&self.reach[side])
            .enumerate()
            .map(|(k, (c, reach))| {
                let mut v = c.json();
                if let Some(r) = reach {
                    v["reach"] = (*r).into();
                }
                // what the lab measured of it
                if let Some(m) = self.sheet[side].get(k).filter(|m| !m.is_null()) {
                    v["measured"] = m.clone();
                }
                v
            })
            .collect()
    }

    /// The first line of a match: the rules, the fighters and their decks.
    pub fn hello(&self) -> Value {
        let w = &self.world;
        let fighter = |i: usize| {
            let d = &w.data[w.players[i].data];
            json!({
                "name": d.display_name,
                "life": w.players[i].life_max,
                "power_max": w.players[i].power_max,
                "cards": self.deck_json(i),
            })
        };
        json!({
            "type": "hello",
            "protocol": PROTOCOL,
            "engine": env!("CARGO_PKG_VERSION"),
            "seed": self.cfg.seed.to_string(),
            "fighters": [fighter(0), fighter(1)],
            "stage": {
                "name": w.stage.display_name,
                "left": w.stage.bound_left,
                "right": w.stage.bound_right,
            },
            "rules": {
                "ticks_per_second": 60,
                "round_time": w.round.clock(),
                "rounds_to_win": w.round.config.match_wins,
                "max_steps": brain::MAX_STEPS,
                "conditions": brain::Cond::LIST,
            },
        })
    }

    /// What happened since the last call, for a viewer following the match
    /// live: `{"ask": side, "frame": n}`, `{"plan": {..}}` (an answer) and
    /// `{"step": {..}}` (a step that ended).
    pub fn take_feed(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.feed)
    }

    pub fn over(&self) -> bool {
        self.world.round.match_over || self.world.frame >= self.cfg.max_frames
    }

    /// The fighters are under the players' control (not an intro, a KO
    /// or a win pose).
    pub fn fighting(&self) -> bool {
        self.world.round.round_state() == 2
    }

    /// `side` is to be shown the state: the round is on and its last
    /// observation was answered.
    pub fn wants(&self, side: usize) -> bool {
        self.cfg.cpu[side] == 0 && !self.over() && self.fighting() && self.asked[side].is_none()
    }

    fn ready(&self, side: usize) -> Vec<&str> {
        let p = &self.world.players[side];
        let air = p.state_type == StateType::A;
        self.decks[side]
            .iter()
            .filter(|c| c.kind != "stance" && c.ready(p.power, air))
            .map(|c| c.id.as_str())
            .collect()
    }

    fn stance_id(&self, side: usize) -> &str {
        self.decks[side]
            .iter()
            .find(|c| c.play == cards::Play::Stance(self.stance[side]))
            .map_or("wait", |c| &c.id)
    }

    /// What `side` decides on, now. Facts only: the clock, both fighters,
    /// the projectiles, what each fighter did lately by name and what it
    /// did to the other, how often each used each move, and how this side's
    /// own plan is going. The side is then waited for ([`Env::submit`]).
    pub fn observation(&mut self, side: usize) -> Value {
        let w = &self.world;
        let player = |i: usize| {
            let p = &w.players[i];
            let sc = w.local_scale(i);
            let [x, y] = self.pos(i);
            json!({
                "x": whole(x),
                // up is positive, 0 on the ground
                "y": whole(-y),
                "vx": tenth(p.vel[0] * p.facing * sc),
                "vy": tenth(-p.vel[1] * sc),
                "facing": if p.facing > 0.0 { "right" } else { "left" },
                "life": p.life,
                "life_max": p.life_max,
                "power": p.power,
                "power_max": p.power_max,
                "posture": match p.state_type {
                    StateType::C => "crouch",
                    StateType::A => "air",
                    StateType::L => "down",
                    _ => "stand",
                },
                "doing": match p.move_type {
                    MoveType::A => "attack",
                    MoveType::H => "hit",
                    _ => "idle",
                },
                "can_act": p.ctrl,
                "guarding": p.guarding,
                "attack_live": p.hitdef.is_some(),
                // an attack: before its hit (startup), able to hit (active)
                // or past it and not free yet (recovery)
                "attack_phase": self.attack_phase(i),
                // what it did lately, oldest first, and what that did
                "actions": self.actions[i].iter().map(|a| json!({
                    "tick": a.frame,
                    "move": a.label,
                    "damage": a.damage,
                    "power": a.power,
                    "guarded": a.guarded,
                })).collect::<Vec<_>>(),
                // every move it has used this match, most used first
                "used": self.counts[i].iter().map(|(l, n)| json!([l, n])).collect::<Vec<_>>(),
            })
        };
        let projectiles: Vec<Value> = w
            .projs
            .iter()
            .map(|p| {
                json!({
                    "side": w.players.get(p.owner).map_or(0, |o| o.team),
                    "x": whole(p.pos[0]),
                    "y": whole(-p.pos[1]),
                    "vx": tenth(p.velocity[0] * p.facing),
                })
            })
            .collect();
        let plan = match &self.run[side] {
            Some(r) => json!({
                "running": true,
                "step": r.at + 1,
                "steps": r.plan.steps.len(),
            }),
            None => json!({ "running": false }),
        };
        let obs = json!({
            "type": "obs",
            "side": side,
            "frame": w.frame,
            "round": w.round.round_no,
            "time_left": w.round.clock(),
            "wins": w.round.wins,
            // the gap between the two bodies (0: touching), the measure a
            // card's reach is given in
            "distance": self.distance(),
            // how much higher the other fighter is than this side's
            "height": whole(self.pos(side)[1] - self.pos(1 - side)[1]),
            // how far the wall is behind each fighter
            "wall_behind": [self.wall_behind(0), self.wall_behind(1)],
            "players": [player(0), player(1)],
            "projectiles": projectiles,
            "stance": self.stance_id(side),
            // the moves this side's fighter can play now, as far as their
            // power cost and posture say
            "ready": self.ready(side),
            "plan": plan,
            // its brain: per rule, how often it fired and how its moves
            // ended (the rule that decides now is `active`)
            "brain": self.book[side].as_ref().map(|(book, _)| json!({
                "rules": book.rules.len(),
                "active": self.active[side].map(|k| k + 1),
                "record": self.record[side].iter().enumerate().map(|(k, r)| json!({
                    "rule": k + 1, "fired": r[0], "hit": r[1], "blocked": r[2],
                    "missed": r[3], "did_not_come_out": r[4],
                })).collect::<Vec<_>>(),
            })),
            // why the last answer was not taken, if it was not
            "refused": self.refused[side],
            // the steps of its plans that ended, oldest first
            "trace": self.trace[side],
        });
        self.refused[side] = None;
        self.asked[side] = Some(self.world.frame);
        self.feed
            .push(json!({ "ask": side, "frame": self.world.frame }));
        obs
    }

    /// What the player said with its answer, for the replay's viewer: one
    /// line, cut at 240 characters.
    fn say(v: &Value) -> String {
        let line: Vec<&str> = v.as_str().unwrap_or("").split_whitespace().collect();
        line.join(" ").chars().take(240).collect()
    }

    /// A side's answer: a plan ([`brain`]). It takes over from the running
    /// one at the next safe moment (at once while a stance is held, else
    /// when the moves in progress are over). An answer that is not a plan
    /// is counted and changes nothing; the reason comes back, and the side
    /// is told with its next observation.
    pub fn submit(&mut self, side: usize, answer: &Value) -> Option<String> {
        let t = &mut self.tally[side];
        t.plans += 1;
        let asked = self.asked[side].take().unwrap_or(self.world.frame);
        t.think_ticks += u64::from(self.world.frame.saturating_sub(asked));
        // a brain (`rules`) or a plan (`steps`)
        enum Parsed {
            Book(brain::Book),
            Plan(brain::Plan),
        }
        let parsed = if answer.get("rules").is_some() && answer.get("steps").is_none() {
            brain::Book::parse(answer, &self.decks[side]).map(Parsed::Book)
        } else {
            brain::Plan::parse(answer, &self.decks[side]).map(Parsed::Plan)
        };
        let mut entry = json!({
            "side": side,
            "asked": asked,
            "frame": self.world.frame,
            "ok": parsed.is_ok(),
            "say": Self::say(&answer["say"]),
            "steps": [],
        });
        let err = match parsed {
            Ok(Parsed::Plan(plan)) => {
                entry["steps"] = plan.listing(&self.decks[side]).into();
                self.book[side] = None;
                self.next[side] = Some((plan, self.plans.len()));
                None
            }
            Ok(Parsed::Book(book)) => {
                // the new brain decides from the next tick (moves in
                // progress play out first)
                entry["steps"] = book.listing(&self.decks[side]).into();
                entry["brain"] = true.into();
                let n = book.rules.len();
                self.book[side] = Some((book, self.plans.len()));
                self.next[side] = None;
                self.active[side] = None;
                self.ready_at[side] = vec![0; n];
                self.record[side] = vec![[0; 5]; n];
                None
            }
            Err(e) => {
                t.invalid += 1;
                entry["error"] = e.clone().into();
                self.refused[side] = Some(e.clone());
                Some(e)
            }
        };
        self.feed.push(json!({ "plan": entry }));
        self.plans.push(entry);
        err
    }

    /// What a plan's conditions are tested on, for `side`.
    fn view(&self, side: usize) -> brain::View {
        let (me, he) = (&self.world.players[side], &self.world.players[1 - side]);
        let distance = self.distance();
        // up is negative in the engine
        let higher = whole(self.pos(side)[1] - self.pos(1 - side)[1]);
        brain::View {
            distance,
            power: me.power,
            life_pct: me.life.max(0) * 100 / me.life_max.max(1),
            his_life_pct: he.life.max(0) * 100 / he.life_max.max(1),
            he_attacks: he.move_type == MoveType::A,
            he_jumps: he.state_type == StateType::A && he.move_type != MoveType::H,
            he_is_down: he.state_type == StateType::L,
            he_is_idle: he.move_type == MoveType::I && he.ctrl,
            he_is_hit: he.move_type == MoveType::H && !he.guarding,
            i_am_in_air: me.state_type == StateType::A,
            he_is_recovering: self.attack_phase(1 - side) == Some("recovery"),
            he_is_above: higher > ABOVE,
            he_is_below: higher < -ABOVE,
            projectile_incoming: self.world.projs.iter().any(|p| {
                self.world
                    .players
                    .get(p.owner)
                    .is_some_and(|o| o.team != me.team)
            }),
            i_am_cornered: self.wall_behind(side) < CORNER,
            he_is_cornered: self.wall_behind(1 - side) < CORNER,
            in_range: self.reach[side]
                .iter()
                .map(|r| r.is_none_or(|r| distance <= r))
                .collect(),
        }
    }

    /// A step ended: noted for the viewer (with the step the plan goes to
    /// next, 0 for its end), the side's next observation and the counters;
    /// then the plan goes there. `to`: where its branch leads (`None`: the
    /// next step). `stay`: the plan does not move yet (its fighter is
    /// being hit).
    fn ended(&mut self, side: usize, result: &str, to: Option<brain::To>, stay: bool) {
        let Some(r) = &self.run[side] else { return };
        let step = &r.plan.steps[r.at];
        let card = &self.decks[side][step.card];
        let dest = match to.unwrap_or(brain::To::Step(r.at + 1)) {
            brain::To::Step(k) if k < r.plan.steps.len() => Some(k),
            _ => None,
        };
        let t = &mut self.tally[side];
        t.steps += 1;
        if card.kind != "stance" && matches!(result, "hit" | "blocked" | "missed" | "failed") {
            t.judged += 1;
            let branch = match result {
                "hit" => step.hit,
                "blocked" => step.blocked,
                _ => step.missed,
            };
            if branch.or(step.otherwise).is_some() {
                t.foreseen += 1;
            }
        }
        let e = match r.rule {
            // a move of a brain's rule: said of the rule, which goes on
            // deciding until its moves are over
            Some(k) => json!({
                "plan": r.id,
                "side": side,
                "step": k + 1,
                "card": card.id,
                "frame": self.world.frame,
                "result": result,
                "to": k + 1,
            }),
            None => json!({
                "plan": r.id,
                "side": side,
                "step": r.at + 1,
                "card": card.id,
                "frame": self.world.frame,
                "result": result,
                "to": if stay { Value::Null } else { dest.map_or(0, |k| k + 1).into() },
            }),
        };
        if let Some(k) = r.rule {
            let col = match result {
                "hit" => Some(1),
                "blocked" => Some(2),
                "missed" => Some(3),
                "failed" => Some(4),
                _ => None,
            };
            if let (Some(col), Some(rec)) = (col, self.record[side].get_mut(k)) {
                rec[col] += 1;
            }
            // a rule whose move did not come out waits half a second, so
            // the rules under it get their turn
            if result == "failed"
                && let Some(at) = self.ready_at[side].get_mut(k)
            {
                *at = self.world.frame + 30;
            }
        }
        self.note_step(side, e);
        if !stay {
            self.go(side, dest);
            if let Some(r) = self.run[side].as_mut() {
                r.chain = result == "hit";
            }
        }
    }

    fn note_step(&mut self, side: usize, e: Value) {
        self.feed.push(json!({ "step": e }));
        if self.trace[side].len() >= RECALL {
            self.trace[side].pop_front();
        }
        self.trace[side].push_back(e.clone());
        self.steps.push(e);
    }

    /// Move the plan to a step, or end it (`None`).
    fn go(&mut self, side: usize, dest: Option<usize>) {
        match (self.run[side].as_mut(), dest) {
            (Some(r), Some(k)) => {
                r.at = k;
                r.phase = Phase::Start;
            }
            (Some(r), None) => {
                if r.rule.is_none() {
                    self.tally[side].ran_out += 1;
                }
                self.run[side] = None;
            }
            (None, _) => {}
        }
    }

    /// Run `side`'s plan for this tick: start, end and branch its steps.
    fn drive(&mut self, side: usize) {
        if self.book[side].is_none() {
            return self.drive_plan(side);
        }
        // a brain: its moves in progress play out through the plan machine
        if self.run[side].is_some() {
            // but a held move (a charge) gives way like a stance: the rules
            // above it are still read, and the first that holds stops it
            if self.outranked(side) {
                self.queue[side].clear();
                self.held[side] = None;
                self.ended(side, "stopped", Some(brain::To::End), false);
            } else {
                self.drive_plan(side);
            }
            if self.run[side].is_some() {
                return;
            }
        }
        let me = &self.world.players[side];
        // being hit: nothing to decide
        if me.move_type == MoveType::H && !me.guarding {
            return;
        }
        let free = me.ctrl;
        let frame = self.world.frame;
        let view = self.view(side);
        let Some((book, id)) = self.book[side].as_ref() else {
            return;
        };
        // the first rule whose conditions all hold: a stance at any time,
        // moves when the fighter can act and the rule has cooled down
        let stance_of = |law: &brain::Law| match self.decks[side][law.cards[0]].play {
            cards::Play::Stance(s) => Some(s),
            cards::Play::Move(_) => None,
        };
        let pick = book.rules.iter().enumerate().find(|(k, law)| {
            law.when.iter().all(|c| c.holds(&view))
                && (stance_of(law).is_some() || (free && frame >= self.ready_at[side][*k]))
        });
        let Some((k, law)) = pick else { return };
        let (id, first) = (*id, law.cards[0]);
        match stance_of(law) {
            Some(s) => {
                self.stance[side] = s;
                if self.active[side] != Some(k) {
                    self.active[side] = Some(k);
                    self.record[side][k][0] += 1;
                    self.note_rule(side, id, k, first);
                }
            }
            None => {
                let plan = book.sequence(k);
                self.ready_at[side][k] = frame + law.cooldown;
                self.active[side] = Some(k);
                self.record[side][k][0] += 1;
                self.note_rule(side, id, k, first);
                self.run[side] = Some(Run {
                    plan,
                    id,
                    at: 0,
                    since: frame,
                    phase: Phase::Start,
                    deadline: 0,
                    landed: false,
                    guarded: false,
                    hurt: false,
                    edges: Vec::new(),
                    chain: false,
                    rule: Some(k),
                    taps: 0,
                    retry: false,
                });
                self.drive_plan(side);
            }
        }
    }

    /// The moves in progress are a held move of a brain's rule, and a rule
    /// above that one holds now.
    fn outranked(&self, side: usize) -> bool {
        let (Some(r), Some((book, _))) = (&self.run[side], &self.book[side]) else {
            return false;
        };
        let Some(k) = r.rule else { return false };
        let held = book
            .rules
            .get(k)
            .is_some_and(|law| law.hold_ticks.is_some());
        if !held || r.hurt {
            return false;
        }
        let view = self.view(side);
        // its own conditions no longer hold (the power it was after is
        // there), or a rule above it does
        !book.rules[k].when.iter().all(|c| c.holds(&view))
            || book.rules[..k].iter().enumerate().any(|(j, law)| {
                law.when.iter().all(|c| c.holds(&view))
                    && self.world.frame >= self.ready_at[side][j]
            })
    }

    /// A rule of a brain took over: said to the viewer and the side.
    fn note_rule(&mut self, side: usize, plan: usize, rule: usize, card: usize) {
        let e = json!({
            "plan": plan,
            "side": side,
            "step": rule + 1,
            "card": self.decks[side][card].id,
            "frame": self.world.frame,
            "result": "fired",
            "to": rule + 1,
        });
        self.tally[side].steps += 1;
        self.note_step(side, e);
    }

    /// Run `side`'s plan (or the moves of its brain's rule) for this tick.
    fn drive_plan(&mut self, side: usize) {
        let frame = self.world.frame;
        // a few transitions at most in one tick (a plan that loops on
        // itself through skipped steps stops here until the next)
        for _ in 0..6 {
            // the next plan takes over when nothing is in the way: no plan,
            // a stance, or a stance about to start
            let free = match &self.run[side] {
                None => true,
                Some(r) => {
                    let stance = self.decks[side][r.plan.steps[r.at].card].kind == "stance";
                    !r.hurt && (r.phase == Phase::Stance || (r.phase == Phase::Start && stance))
                }
            };
            if free && let Some((plan, id)) = self.next[side].take() {
                let edges = vec![false; plan.rules.len()];
                self.run[side] = Some(Run {
                    plan,
                    id,
                    at: 0,
                    since: frame,
                    phase: Phase::Start,
                    deadline: 0,
                    landed: false,
                    guarded: false,
                    hurt: false,
                    edges,
                    chain: false,
                    rule: None,
                    taps: 0,
                    retry: false,
                });
                // the viewer is told which plan runs from now, at its step 1
                let e = json!({
                    "plan": id, "side": side, "step": 0, "card": "",
                    "frame": frame, "result": "start", "to": 1,
                });
                self.feed.push(json!({ "step": e }));
                self.steps.push(e);
            }
            let view = self.view(side);
            let me = &self.world.players[side];
            // hit: not guarding it (a guard is the plan working, not broken)
            let (hit_now, free_now, state) = (
                me.move_type == MoveType::H && !me.guarding,
                me.ctrl || me.move_type != MoveType::A,
                me.state_no,
            );
            let entered = self.entered[side];
            let Some(r) = self.run[side].as_mut() else {
                return;
            };
            // being hit: whatever was going on is over; the plan waits for
            // the hit to end, then follows its `i_am_hit` rule or stops
            if hit_now {
                if !r.hurt {
                    r.hurt = true;
                    let moving = matches!(r.phase, Phase::Waiting | Phase::Out);
                    self.queue[side].clear();
                    self.held[side] = None;
                    if moving {
                        self.ended(side, "interrupted", None, true);
                    }
                }
                return;
            }
            if r.hurt {
                r.hurt = false;
                let rule = r.plan.rules.iter().find(|x| x.cond == brain::Cond::IAmHit);
                match rule.map(|x| x.goto) {
                    Some(to) => self.ended(side, "i_am_hit", Some(to), false),
                    None if r.rule.is_some() => {
                        // a brain's moves were cut by the hit: its rules
                        // decide again once the fighter can act
                        self.run[side] = None;
                    }
                    None => {
                        // nothing in the plan says what to do after a hit
                        self.ended(side, "broken", Some(brain::To::End), false);
                        self.tally[side].broken += 1;
                        self.tally[side].ran_out = self.tally[side].ran_out.saturating_sub(1);
                    }
                }
                continue;
            }
            // the other rules fire when their condition becomes true, while
            // a stance is held (a move in progress is not cut)
            let mut fired = None;
            for (k, rule) in r.plan.rules.iter().enumerate() {
                let now = rule.cond.holds(&view);
                if now && !r.edges[k] && r.phase == Phase::Stance && fired.is_none() {
                    fired = Some(rule.goto);
                }
                r.edges[k] = now;
            }
            if let Some(to) = fired {
                self.ended(side, "rule", Some(to), false);
                continue;
            }
            let step = r.plan.steps[r.at].clone();
            match r.phase {
                Phase::Start => {
                    r.since = frame;
                    match self.decks[side][step.card].play.clone() {
                        cards::Play::Stance(s) => {
                            r.chain = false;
                            r.phase = Phase::Stance;
                            self.stance[side] = s;
                            self.queue[side].clear();
                            self.held[side] = None;
                            return;
                        }
                        cards::Play::Move(_) if step.gate.is_some_and(|c| !c.holds(&view)) => {
                            self.ended(side, "skipped", step.otherwise.or(step.next), false);
                        }
                        cards::Play::Move(mut ticks) => {
                            // `for_ms` on a move: its last input is held
                            // that long (a move that charges while held)
                            if let (Some(n), Some(last)) = (step.for_ticks, ticks.last().copied()) {
                                ticks.extend(std::iter::repeat_n(last, n.min(HOLD_MAX) as usize));
                            }
                            r.phase = Phase::Waiting;
                            r.landed = false;
                            r.guarded = false;
                            self.queue[side].clear();
                            if std::mem::take(&mut r.chain) {
                                // reached by a hit: played at once, inside
                                // the hit (a cancel), with one more try
                                // later if it does not come out
                                r.retry = true;
                                r.taps = TAPS;
                                r.deadline = frame + ticks.len() as u32 + 8;
                                self.held[side] = None;
                                self.queue[side] = ticks.into();
                            } else {
                                // played as soon as the fighter can act
                                r.retry = false;
                                r.taps = 0;
                                r.deadline = frame + HOLD + ticks.len() as u32 + 8;
                                self.held[side] = Some((ticks, frame + HOLD));
                            }
                            return;
                        }
                    }
                }
                Phase::Stance => {
                    let held = frame.saturating_sub(r.since);
                    let done = match (step.until, step.for_ticks) {
                        (None, None) => held >= STANCE,
                        (c, t) => c.is_some_and(|c| c.holds(&view)) || t.is_some_and(|t| held >= t),
                    };
                    if done {
                        self.ended(side, "done", step.next, false);
                    } else if held >= STANCE_MAX {
                        self.ended(side, "timeout", step.next, false);
                    } else {
                        return;
                    }
                }
                Phase::Waiting => {
                    let states = &self.decks[side][step.card].states;
                    // a move whose state is not known (a jump) is out once
                    // its inputs are in
                    let sent = self.held[side].is_none();
                    let out = if states.is_empty() {
                        sent && self.queue[side].is_empty()
                    } else {
                        // in one of its states, entered since the step
                        // began (a move chained into itself is in its
                        // state already)
                        states.contains(&state) && entered > r.since
                    };
                    if out {
                        r.phase = Phase::Out;
                        r.since = frame;
                    } else if sent && self.queue[side].is_empty() && r.taps > 0 {
                        // not out yet: released, then pressed again
                        r.taps -= 1;
                        if let cards::Play::Move(ticks) = self.decks[side][step.card].play.clone() {
                            r.deadline = frame + ticks.len() as u32 + 10;
                            let mut again = vec![Rel::default(); 2];
                            again.extend(ticks);
                            self.queue[side] = again.into();
                        }
                        return;
                    } else if sent && frame > r.deadline && std::mem::take(&mut r.retry) {
                        // the cancel did not come out: the same move again,
                        // as soon as the fighter is free
                        if let cards::Play::Move(ticks) = self.decks[side][step.card].play.clone() {
                            r.deadline = frame + HOLD + ticks.len() as u32 + 8;
                            self.held[side] = Some((ticks, frame + HOLD));
                        }
                        return;
                    } else if sent && frame > r.deadline {
                        let to = step.missed.or(step.otherwise).or(step.next);
                        self.ended(side, "failed", to, false);
                    } else {
                        return;
                    }
                }
                Phase::Out => {
                    // a hit the plan has a branch for is followed at once
                    // (the next move may cancel into it)
                    if r.landed && step.hit.is_some() {
                        self.ended(side, "hit", step.hit, false);
                        continue;
                    }
                    let over = self.queue[side].is_empty()
                        && frame.saturating_sub(r.since) >= 2
                        && free_now;
                    if !over {
                        return;
                    }
                    let (result, branch) = if r.landed {
                        ("hit", step.hit)
                    } else if r.guarded {
                        ("blocked", step.blocked)
                    } else if self.decks[side][step.card].reach.is_none()
                        && matches!(
                            self.decks[side][step.card].kind,
                            "charge" | "move" | "boost"
                        )
                    {
                        // a move with nothing to hit with (a charge, a
                        // jump, a run) is done, it did not miss
                        ("done", step.missed)
                    } else {
                        ("missed", step.missed)
                    };
                    self.ended(side, result, branch.or(step.otherwise).or(step.next), false);
                }
            }
        }
    }

    /// A fighter entered a state one of its cards leads to: it did that
    /// move, by the name its `.cmd` gives it.
    fn note_action(&mut self, side: usize, state: i32) {
        let label = self.decks[side]
            .iter()
            .flat_map(|c| c.names.iter())
            .find(|(s, _)| *s == state)
            .map(|(_, l)| l.clone());
        let Some(label) = label else { return };
        match self.counts[side].iter_mut().find(|(l, _)| *l == label) {
            Some((_, n)) => *n += 1,
            None => self.counts[side].push((label.clone(), 1)),
        }
        self.counts[side].sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        if self.actions[side].len() >= RECALL {
            self.actions[side].pop_front();
        }
        self.actions[side].push_back(Action {
            frame: self.world.frame,
            label,
            damage: 0,
            power: 0,
            guarded: false,
        });
    }

    /// Where `side`'s fighter is in its attack, by the attack boxes of its
    /// animation: before the first (`startup`), among them (`active`) or
    /// past the last (`recovery`). `None` when it is not attacking or its
    /// animation has no attack box.
    fn attack_phase(&self, side: usize) -> Option<&'static str> {
        let p = &self.world.players[side];
        if p.move_type != MoveType::A {
            return None;
        }
        let frames = &self.world.data[p.anim_owner].air.get(p.anim.action)?.frames;
        let first = frames.iter().position(|f| !f.clsn1.is_empty())?;
        let last = frames.iter().rposition(|f| !f.clsn1.is_empty())?;
        Some(match p.anim.elem {
            e if e < first => "startup",
            e if e > last => "recovery",
            _ => "active",
        })
    }

    /// One tick of the match: each side's plan decides what its fighter
    /// does (no plan: its stance). Intros and round ends play without the
    /// players.
    pub fn tick(&mut self) {
        let fight = self.fighting();
        if !fight {
            for i in 0..2 {
                self.queue[i].clear();
                self.held[i] = None;
                // a round's end ends the plans: a new round is a new fight
                self.run[i] = None;
            }
        }
        if fight {
            self.drive(0);
            self.drive(1);
        }
        let distance = self.distance();
        for i in 0..2 {
            let free = self.world.players[i].ctrl;
            match self.held[i].take() {
                Some((ticks, until)) if free || self.world.frame >= until => {
                    // it has until its inputs are over to come out
                    if let Some(r) = self.run[i].as_mut() {
                        r.deadline = self.world.frame + ticks.len() as u32 + 8;
                    }
                    self.queue[i] = ticks.into();
                }
                held => self.held[i] = held,
            }
        }
        let buttons: [Buttons; 2] = [0, 1].map(|i| {
            let right = self.world.players[i].facing > 0.0;
            let rel = match self.queue[i].pop_front() {
                Some(rel) => rel,
                // between moves the stance holds; a new round starts from it
                None if fight => {
                    if self.run[i].is_none() && self.stance[i] == cards::Stance::Wait {
                        self.tally[i].idle_ticks += 1;
                    }
                    self.stance[i].rel(distance)
                }
                None => Rel::default(),
            };
            rel.resolve(right)
        });
        self.world.step(&buttons);
        self.inputs.push([buttons[0].bits(), buttons[1].bits()]);
        let frame = self.world.frame;
        if self.world.round.round_no != self.last_round {
            self.last_round = self.world.round.round_no;
            self.last_life = [self.world.players[0].life, self.world.players[1].life];
        }
        for i in 0..2 {
            let p = &self.world.players[i];
            let (life, guarding, state) = (p.life, p.guarding, p.state_no);
            // hits it has taken in a row are the other side's combo
            let (power, taken) = (p.power, p.received_hits);
            let clock = p.state_time;
            if state != self.last_state[i] || clock < self.last_state_time[i] {
                self.entered[i] = frame;
            }
            self.last_state_time[i] = clock;
            if state != self.last_state[i] {
                self.last_state[i] = state;
                self.note_action(i, state);
            }
            // power gained or spent: to the move the fighter last made
            let gained = power - self.last_power[i];
            self.last_power[i] = power;
            if gained != 0
                && let Some(a) = self.actions[i]
                    .back_mut()
                    .filter(|a| frame.saturating_sub(a.frame) < 240)
            {
                a.power += gained;
            }
            self.tally[1 - i].combo = self.tally[1 - i].combo.max(taken);
            let lost = self.last_life[i] - life;
            self.last_life[i] = life;
            if lost <= 0 {
                continue;
            }
            // life lost: to the other side's count, its plan's move in
            // progress and the move it last made (within four seconds)
            let by = 1 - i;
            self.tally[by].damage += i64::from(lost);
            if let Some(r) = self.run[by].as_mut().filter(|r| r.phase == Phase::Out) {
                if guarding {
                    r.guarded = true;
                } else {
                    r.landed = true;
                }
            }
            match self.actions[by].back_mut() {
                Some(a) if frame.saturating_sub(a.frame) < 240 => {
                    a.damage += lost;
                    a.guarded |= guarding;
                }
                _ => {
                    if self.actions[by].len() >= RECALL {
                        self.actions[by].pop_front();
                    }
                    self.actions[by].push_back(Action {
                        frame,
                        label: "(an attack with no name in its files)".into(),
                        damage: lost,
                        power: 0,
                        guarded: guarding,
                    });
                }
            }
        }
        // a guard that took no life still stopped the move: said of the
        // plan's move in progress and of the move as the fighters see it
        for i in 0..2 {
            let he = &self.world.players[1 - i];
            if !(he.guarding && he.move_type == MoveType::H) {
                continue;
            }
            if let Some(r) = self.run[i].as_mut().filter(|r| r.phase == Phase::Out) {
                r.guarded = true;
            }
            if let Some(a) = self.actions[i]
                .back_mut()
                .filter(|a| frame.saturating_sub(a.frame) < 240)
            {
                a.guarded = true;
            }
        }
    }

    /// The winning side: by rounds, else (unfinished or drawn) by the life
    /// left, as a share of the fighter's own.
    pub fn winner(&self) -> Option<usize> {
        let w = self.world.round.wins;
        if w[0] != w[1] {
            return Some(usize::from(w[1] > w[0]));
        }
        let share = |i: usize| {
            let p = &self.world.players[i];
            i64::from(p.life.max(0)) * 1000 / i64::from(p.life_max.max(1))
        };
        match share(0).cmp(&share(1)) {
            std::cmp::Ordering::Greater => Some(0),
            std::cmp::Ordering::Less => Some(1),
            std::cmp::Ordering::Equal => None,
        }
    }

    /// The last line of a match.
    pub fn result(&self) -> Value {
        let w = &self.world;
        let t = &self.tally;
        let each = |f: &dyn Fn(&Tally) -> u64| [f(&t[0]), f(&t[1])];
        let used = |i: usize| self.record[i].iter().filter(|r| r[0] > 0).count();
        json!({
            "type": "result",
            "finished": w.round.match_over,
            "winner": self.winner(),
            "wins": w.round.wins,
            "frames": w.frame,
            "life": [w.players[0].life.max(0), w.players[1].life.max(0)],
            "damage": [t[0].damage, t[1].damage],
            "plans": each(&|t| u64::from(t.plans)),
            "invalid": each(&|t| u64::from(t.invalid)),
            // mean ticks from an observation to its answer
            "think_ticks": each(&|t| t.think_ticks / u64::from(t.plans.max(1))),
            "idle_ticks": each(&|t| u64::from(t.idle_ticks)),
            // how the plans held: steps that ran, moves that ended and how
            // many of those the plan had a branch for, plans a hit broke,
            // plans that ran out
            "steps": each(&|t| u64::from(t.steps)),
            "judged": each(&|t| u64::from(t.judged)),
            "foreseen": each(&|t| u64::from(t.foreseen)),
            "broken": each(&|t| u64::from(t.broken)),
            "ran_out": each(&|t| u64::from(t.ran_out)),
            // the most hits each landed in one combo
            "combo": [t[0].combo, t[1].combo],
            // of the brain each side ended with: its rules, and how many
            // of them fired
            "rules": [self.record[0].len(), self.record[1].len()],
            "rules_used": [used(0), used(1)],
            // u64: a string, JSON numbers lose bits past 2^53
            "checksum": w.checksum().to_string(),
        })
    }

    /// Let the win pose play out (neutral inputs), so the replay ends like
    /// a match does.
    pub fn finish(&mut self) {
        for _ in 0..300 {
            if !self.world.round.match_over || self.world.round.round_over() {
                break;
            }
            self.tick();
        }
    }

    /// The match as a replay file of the game (`rugen replay`): played
    /// under the online rules, so the round timings are the bundled
    /// theme's whatever the viewer's own.
    ///
    /// Paths under `root` (the game folder) are stored relative to it: a
    /// shared replay does not name the home folder it was recorded in.
    pub fn replay(&self, names: [&str; 2], root: &std::path::Path) -> Value {
        let m = self.world.movement;
        let rel = |p: &std::path::Path| p.strip_prefix(root).unwrap_or(p).to_path_buf();
        let decks = [0, 1].map(|i| self.deck_json(i));
        json!({
            "version": 1,
            "mode": "watch",
            "p1": rel(&self.cfg.p1),
            "p2": rel(&self.cfg.p2),
            "stage": self.cfg.stage.as_deref().map(rel),
            "ai": self.cfg.cpu,
            "fighter_size": self.world.fighter_size,
            "arena_ai": false,
            "round_time": self.cfg.round_time,
            "seed": self.cfg.seed,
            "life": self.cfg.life,
            "sim": {
                "zoom": [ZOOM.0, ZOOM.1],
                "movement": {
                    "air_jumps": m.air_jumps,
                    "air_dash": m.air_dash,
                    "fast_fall": m.fast_fall,
                },
                "online": true,
                "match_wins": self.world.round.config.match_wins,
            },
            // who played, with what deck, every plan and every step that
            // ended: the game draws it over the replay
            "gym": {
                "protocol": PROTOCOL,
                "players": names,
                "decks": decks,
                "plans": self.plans,
                "steps": self.steps,
            },
            "inputs": self.inputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(text: &str) -> Vec<Rel> {
        parse_plan(text, 2, 90).unwrap()
    }

    #[test]
    fn inputs_parse() {
        let p = raw("D,DF,F,x");
        assert_eq!(p.len(), 8);
        assert!(p[0].d && !p[0].f);
        assert!(p[2].d && p[2].f);
        assert!(p[6].btn[3] && !p[6].f);
        assert_eq!(raw("B*20").len(), 20);
        assert_eq!(raw("~D, DF, F, x"), raw("D,DF,F,x"));
        assert_eq!(raw("~30B,F+x").len(), 32);
        assert!(raw("B")[0].b && !raw("B")[0].btn[1]);
        assert!(raw("b")[0].btn[1] && !raw("b")[0].b);
        assert!(parse_plan("punch him", 2, 90).is_err());
        assert_eq!(parse_plan("F*500,x", 2, 90).unwrap().len(), 90);
        let f = raw("F+a")[0];
        assert!(f.resolve(true).r && f.resolve(false).l && f.resolve(true).a);
    }

    fn kfm() -> Option<Config> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        let def = root.join("chars/kfm/kfm.def");
        if !def.exists() {
            return None;
        }
        let mut cfg = Config::new(def.clone(), def, root.join("data"), root.join("data"));
        cfg.stage = Some(root.join("stages/kfm.def"));
        cfg.round_time = Some(20);
        cfg.match_wins = Some(1);
        cfg.seed = 7;
        Some(cfg)
    }

    fn pressure() -> Value {
        json!({
            "steps": [
                {"card": "approach", "until": "distance<40"},
                {"card": "stand_light_punch", "hit": 3, "blocked": 4, "missed": 1},
                {"card": "stand_strong_punch", "next": 1},
                {"card": "block", "for_ms": 300, "next": 1},
            ],
            "rules": [{"if": "i_am_hit", "goto": 4}],
            "say": "close in and punch",
        })
    }

    /// A match where each side answers `delay` ticks after it was shown
    /// the state, always with the same plan.
    fn play(cfg: Config, delay: [u32; 2], plans: [Value; 2]) -> Env {
        let mut env = Env::new(cfg).unwrap();
        let mut due: [Option<u32>; 2] = [None; 2];
        while !env.over() {
            for side in 0..2 {
                if env.wants(side) {
                    let obs = env.observation(side);
                    assert_eq!(obs["side"], side);
                    due[side] = Some(env.world.frame + delay[side]);
                }
                if due[side].is_some_and(|d| env.world.frame >= d) {
                    due[side] = None;
                    assert_eq!(env.submit(side, &plans[side]), None);
                }
            }
            env.tick();
        }
        env.finish();
        env
    }

    /// A plan runs by itself: the fighter walks in, punches, and takes the
    /// branch of what happened, with its side silent the whole time.
    #[test]
    fn a_plan_runs_ahead_of_its_player() {
        let Some(cfg) = kfm() else { return };
        let mut env = Env::new(cfg).unwrap();
        while !env.wants(0) {
            env.tick();
        }
        let x0 = env.observation(0)["players"][0]["x"].as_i64().unwrap();
        assert_eq!(env.submit(0, &pressure()), None);
        // not asked again, not answered again: 6 seconds on one plan
        for _ in 0..360 {
            env.tick();
        }
        let obs = env.observation(0);
        assert!(
            obs["players"][0]["x"].as_i64().unwrap() > x0 + 40,
            "walked in"
        );
        let trace = obs["trace"].as_array().unwrap();
        let ran = |card: &str, result: &str| {
            trace
                .iter()
                .any(|s| s["card"] == card && s["result"] == result)
        };
        assert!(ran("approach", "done"), "{trace:?}");
        assert!(
            ran("stand_light_punch", "hit"),
            "the punch landed: {trace:?}"
        );
        assert!(
            ran("stand_strong_punch", "hit") || ran("stand_strong_punch", "missed"),
            "{trace:?}"
        );
        let t = env.tally[0];
        assert_eq!((t.plans, t.invalid), (1, 0));
        assert!(t.steps >= 5 && t.judged >= 2, "{t:?}");
        assert!(t.foreseen >= 1, "the punch's hit had a branch: {t:?}");
        assert!(env.tally[0].damage > 0);
        // the other fighter is told what hit it, by name
        let his = env.observation(1);
        let did = his["players"][0]["actions"].as_array().unwrap();
        assert!(
            did.iter()
                .any(|a| a["move"] == "Stand Light Punch" && a["damage"].as_i64() > Some(0)),
            "{did:?}"
        );
        assert_eq!(his["players"][0]["used"][0][0], "Stand Light Punch");
    }

    /// Guarding an attack is not being hit: the plan of the side that
    /// guards goes on, its `i_am_hit` rule does not fire.
    #[test]
    fn a_guard_does_not_break_the_plan() {
        let Some(cfg) = kfm() else { return };
        let guard = json!({
            "steps": [{"card": "block", "for_ms": 60000}, {"card": "wait", "for_ms": 60000}],
            "rules": [{"if": "i_am_hit", "goto": 2}],
        });
        let mut env = Env::new(cfg).unwrap();
        while !env.wants(0) {
            env.tick();
        }
        env.observation(0);
        env.observation(1);
        assert_eq!(env.submit(0, &pressure()), None);
        assert_eq!(env.submit(1, &guard), None);
        for _ in 0..300 {
            env.tick();
        }
        let obs = env.observation(1);
        let blocked = obs["players"][0]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["guarded"] == true);
        assert!(
            blocked,
            "the punches were guarded: {}",
            obs["players"][0]["actions"]
        );
        assert_eq!(obs["stance"], "block", "still on its first step");
        assert_eq!(env.tally[1].broken, 0);
    }

    /// Being hit ends what the plan was doing; with an `i_am_hit` rule it
    /// goes on from there, without one it is broken.
    #[test]
    fn a_hit_follows_the_rule_or_breaks_the_plan() {
        let Some(cfg) = kfm() else { return };
        let wait = json!({"steps": [{"card": "wait", "for_ms": 60000}]});
        let ruled = json!({
            "steps": [{"card": "wait", "for_ms": 60000}, {"card": "block", "for_ms": 60000}],
            "rules": [{"if": "i_am_hit", "goto": 2}],
        });
        for (plan, broken) in [(wait, 1), (ruled, 0)] {
            let mut env = Env::new(cfg.clone()).unwrap();
            while !env.wants(0) {
                env.tick();
            }
            env.observation(0);
            env.observation(1);
            assert_eq!(env.submit(0, &pressure()), None);
            assert_eq!(env.submit(1, &plan), None);
            for _ in 0..300 {
                env.tick();
            }
            assert_eq!(env.tally[1].broken, broken);
            let obs = env.observation(1);
            assert_eq!(obs["plan"]["running"], broken == 0);
            if broken == 0 {
                assert_eq!(obs["stance"], "block", "the rule's step");
            }
        }
    }

    /// An answer that is not a plan is counted, changes nothing, and the
    /// side is told why with its next observation.
    #[test]
    fn a_bad_answer_is_refused_and_explained() {
        let Some(cfg) = kfm() else { return };
        let mut env = Env::new(cfg).unwrap();
        while !env.wants(0) {
            env.tick();
        }
        env.observation(0);
        assert_eq!(env.submit(0, &pressure()), None);
        env.tick();
        env.observation(0);
        let e = env
            .submit(0, &json!({"steps": [{"card": "hadoken"}]}))
            .unwrap();
        assert!(e.contains("hadoken"), "{e}");
        assert_eq!(env.tally[0].invalid, 1);
        for _ in 0..30 {
            env.tick();
        }
        let obs = env.observation(0);
        assert!(obs["refused"].as_str().unwrap().contains("hadoken"));
        assert_eq!(obs["plan"]["running"], true, "the first plan goes on");
        assert_eq!(env.observation(0)["refused"], Value::Null, "told once");
    }

    /// The clock does not wait, and a plan covers the wait: a side that
    /// answers late still fights. The recorded inputs replayed on a fresh
    /// world end on the same checksum.
    #[test]
    fn a_slow_side_still_fights_and_the_match_replays() {
        let Some(cfg) = kfm() else { return };
        let env = play(cfg.clone(), [2, 90], [pressure(), pressure()]);
        let t = env.tally;
        assert!(env.world.round.match_over, "a 20 s round ends");
        assert!(t[0].plans > 3 * t[1].plans, "{t:?}");
        assert!(t[1].steps > 10, "the slow side's plans ran: {t:?}");
        assert!(
            t[0].damage > 0 && t[1].damage > 0,
            "both landed hits: {t:?}"
        );
        assert_eq!(t[0].invalid + t[1].invalid, 0);

        let mut again = Env::new(cfg).unwrap();
        let bytes: Vec<u8> = env
            .inputs
            .iter()
            .flat_map(|[a, b]| [a.to_le_bytes(), b.to_le_bytes()].concat())
            .collect();
        let r = rugen_sim::headless::replay(&mut again.world, &bytes, None);
        assert_eq!(r.checksum, env.world.checksum());
        assert_eq!(r.frames, env.world.frame);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        let rep = env.replay(["a", "b"], &root);
        assert_eq!(rep["inputs"].as_array().unwrap().len(), env.inputs.len());
        assert_eq!(
            rep["p1"], "chars/kfm/kfm.def",
            "no absolute path in a replay"
        );
        let plans = rep["gym"]["plans"].as_array().unwrap();
        assert_eq!(plans[0]["steps"][0]["card"], "approach");
        assert_eq!(plans[0]["say"], "close in and punch");
        assert!(rep["gym"]["steps"].as_array().unwrap().len() > 20);
    }

    /// A fighter knows how far its moves reach: a bare `approach` walks
    /// until the next move of the plan is in range and stops there, and
    /// the move then lands. The other side is told the same senses.
    #[test]
    fn a_plan_uses_its_moves_reach() {
        let Some(cfg) = kfm() else { return };
        let mut env = Env::new(cfg).unwrap();
        let hello = env.hello();
        let reach = |id: &str| {
            hello["fighters"][0]["cards"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .and_then(|c| c["reach"].as_i64())
        };
        let (punch, kick) = (reach("stand_light_punch"), reach("standing_strong_kick"));
        assert!(punch.is_some_and(|r| (5..80).contains(&r)), "{punch:?}");
        assert!(
            kick > punch,
            "the strong kick reaches further: {kick:?} > {punch:?}"
        );
        assert_eq!(reach("approach"), None);
        assert_eq!(reach("run_fwd"), None, "no hitbox");
        while !env.wants(0) {
            env.tick();
        }
        let obs = env.observation(0);
        let gap = obs["distance"].as_i64().unwrap();
        assert!(gap > punch.unwrap(), "they start out of range: {gap}");
        assert!(obs["wall_behind"][0].as_i64() > Some(30));
        assert_eq!(obs["height"], 0);
        let plan = json!({"steps": [
            {"card": "approach"},
            {"card": "stand_light_punch", "if": "in_range", "else": 1, "hit": "end", "missed": "end"},
        ]});
        assert_eq!(env.submit(0, &plan), None);
        for _ in 0..240 {
            env.tick();
        }
        let obs = env.observation(0);
        let trace = obs["trace"].as_array().unwrap();
        assert_eq!(trace[0]["result"], "done", "walked into range: {trace:?}");
        assert_eq!(trace[1]["result"], "hit", "and the punch landed: {trace:?}");
        assert!(obs["distance"].as_i64().unwrap() <= punch.unwrap());
    }

    /// A move reached by a hit is played inside the hit, so it chains: a
    /// plan of punch into punch lands them as one combo. Cards carry their
    /// timing and damage, and the log says what power each move gave.
    #[test]
    fn hits_chain_into_combos_and_cards_carry_their_facts() {
        let Some(cfg) = kfm() else { return };
        let mut env = Env::new(cfg).unwrap();
        let hello = env.hello();
        let card = |id: &str| {
            hello["fighters"][0]["cards"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .cloned()
                .unwrap()
        };
        let (light, strong) = (card("stand_light_punch"), card("stand_strong_punch"));
        assert!(
            light["startup_ms"]
                .as_i64()
                .is_some_and(|t| (0..300).contains(&t)),
            "{light}"
        );
        assert!(
            strong["startup_ms"].as_i64() > light["startup_ms"].as_i64(),
            "{strong}"
        );
        assert!(
            strong["damage"].as_i64() > light["damage"].as_i64(),
            "{strong} {light}"
        );
        assert!(light["lasts_ms"].as_i64() > light["startup_ms"].as_i64());
        while !env.wants(0) {
            env.tick();
        }
        env.observation(0);
        let plan = json!({"steps": [
            {"card": "approach", "until": "distance<6"},
            {"card": "stand_light_punch", "if": "in_range", "else": 1, "hit": 3, "missed": 1},
            {"card": "stand_light_punch", "hit": 4, "missed": 1},
            {"card": "stand_strong_punch", "hit": 1, "missed": 1},
        ]});
        assert_eq!(env.submit(0, &plan), None);
        for _ in 0..420 {
            env.tick();
            if std::env::var("GYM_TRACE").is_ok() {
                let (a, b) = (&env.world.players[0], &env.world.players[1]);
                eprintln!(
                    "f{} st{} t{} ctrl{} q{} | he st{} {:?} life{} recv{} | {:?}",
                    env.world.frame,
                    a.state_no,
                    a.state_time,
                    a.ctrl as u8,
                    env.queue[0].len(),
                    b.state_no,
                    b.move_type,
                    b.life,
                    b.received_hits,
                    env.run[0]
                        .as_ref()
                        .map(|r| (r.at + 1, r.phase, r.landed, r.taps))
                );
            }
        }
        assert!(
            env.tally[0].combo >= 2,
            "punches chained: {:?} {:?}",
            env.tally[0],
            env.steps
                .iter()
                .take(14)
                .map(|e| format!(
                    "{}:{}@{}",
                    e["step"],
                    e["result"].as_str().unwrap_or(""),
                    e["frame"]
                ))
                .collect::<Vec<_>>()
        );
        let obs = env.observation(0);
        let did = obs["players"][0]["actions"].as_array().unwrap();
        assert!(
            did.iter().any(|a| a["power"].as_i64() > Some(0)),
            "hits give power: {did:?}"
        );
        assert_eq!(env.result()["combo"][0], env.tally[0].combo);
    }

    /// `for_ms` on a move holds its last input: the fighter stays in it.
    /// And a side is told when the other one is past its attack.
    #[test]
    fn a_move_can_be_held_and_recovery_is_seen() {
        let Some(cfg) = kfm() else { return };
        let mut env = Env::new(cfg).unwrap();
        while !env.wants(0) {
            env.tick();
        }
        env.observation(0);
        env.observation(1);
        // crouch-guard is a held direction: a stance; a held move here is
        // the strong kick's button kept down, which must not replay it
        let hold = json!({"steps": [{"card": "standing_strong_kick", "for_ms": 1000, "hit": "end", "missed": "end"}]});
        assert_eq!(env.submit(0, &hold), None);
        let mut phases = Vec::new();
        for _ in 0..90 {
            env.tick();
            if let Some(p) = env.attack_phase(0)
                && phases.last() != Some(&p)
            {
                phases.push(p);
            }
        }
        assert_eq!(
            phases[..3],
            ["startup", "active", "recovery"],
            "the kick's three phases"
        );
        let step = &env.steps.iter().find(|s| s["step"] == 1).unwrap();
        assert!(
            step["frame"].as_u64() > Some(u64::from(env.world.frame) - 40),
            "held: {step}"
        );
    }

    /// A brain is read every tick: with one answer for the whole fight,
    /// the fighter walks in when far, strikes in range, and guards the
    /// moment the other one attacks. The side is told how each rule did.
    #[test]
    fn a_brain_reacts_every_tick() {
        let Some(cfg) = kfm() else { return };
        let striker = json!({"rules": [
            {"when": ["in_range"], "do": ["stand_light_punch", "stand_light_punch", "stand_strong_punch"]},
            {"do": ["approach"]},
        ], "say": "walk in and hit"});
        let guard = json!({"rules": [
            {"when": ["he_attacks", "distance<60"], "do": ["block"]},
            {"do": ["wait"]},
        ]});
        let mut env = Env::new(cfg).unwrap();
        while !env.wants(0) {
            env.tick();
        }
        env.observation(0);
        env.observation(1);
        assert_eq!(env.submit(0, &striker), None);
        assert_eq!(env.submit(1, &guard), None);
        for _ in 0..600 {
            env.tick();
        }
        // one answer each, ten seconds of fight
        assert_eq!((env.tally[0].plans, env.tally[1].plans), (1, 1));
        let obs = env.observation(0);
        let rec = &obs["brain"]["record"];
        assert!(rec[1]["fired"].as_u64() >= Some(1), "it walked in: {rec}");
        assert!(
            rec[0]["fired"].as_u64() >= Some(5),
            "and kept striking: {rec}"
        );
        // the other side guards (and backs away while it does): few land
        let (fired, landed) = (
            rec[0]["fired"].as_u64().unwrap(),
            rec[0]["hit"].as_u64().unwrap(),
        );
        assert!(landed * 2 < fired, "the guard came up in time: {rec}");
        let his = env.observation(1);
        assert!(
            his["brain"]["record"][0]["fired"].as_u64() >= Some(3),
            "{}",
            his["brain"]
        );
        assert_eq!(env.result()["rules_used"], json!([2, 2]));
        // the plans' list holds the brain, rule by rule, for the viewer
        let rep = env.replay(["a", "b"], std::path::Path::new("/"));
        assert_eq!(rep["gym"]["plans"][0]["brain"], true);
        assert_eq!(
            rep["gym"]["plans"][0]["steps"][0]["note"],
            "when in range of stand_light_punch, then stand_light_punch > stand_strong_punch"
        );
        assert!(
            rep["gym"]["steps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["result"] == "fired")
        );
    }

    /// A held move gives way like a stance: a rule above it that comes to
    /// hold stops it at once, long before its hold is over.
    #[test]
    fn a_held_move_gives_way_to_a_rule_above_it() {
        let Some(cfg) = kfm() else { return };
        let holder = json!({"rules": [
            {"when": ["distance<60"], "do": ["block"]},
            {"do": ["taunt"], "hold_ms": 5000},
        ]});
        let walker = json!({"rules": [{"do": ["approach"]}]});
        let mut env = Env::new(cfg).unwrap();
        while !env.wants(0) {
            env.tick();
        }
        env.observation(0);
        env.observation(1);
        assert_eq!(env.submit(0, &holder), None);
        assert_eq!(env.submit(1, &walker), None);
        let start = env.world.frame;
        let mut stopped = None;
        for _ in 0..300 {
            env.tick();
            if stopped.is_none()
                && env
                    .steps
                    .iter()
                    .any(|s| s["side"] == 0 && s["result"] == "stopped")
            {
                stopped = Some(env.world.frame);
            }
        }
        let at = stopped.expect("the hold was stopped");
        assert!(
            at - start < 200,
            "well before its 5 s were over: {}",
            at - start
        );
        assert_eq!(
            env.observation(0)["stance"],
            "block",
            "the rule above took over"
        );
    }

    /// A side can be the game's own computer player: it is never asked,
    /// it fights, and the replay plays the same.
    #[test]
    fn a_side_can_be_the_built_in_cpu() {
        let Some(mut cfg) = kfm() else { return };
        cfg.cpu = [0, 6];
        let brain = json!({"rules": [
            {"when": ["he_attacks", "distance<50"], "do": ["block"]},
            {"when": ["in_range"], "do": ["stand_light_punch"]},
            {"do": ["approach"]},
        ]});
        let mut env = Env::new(cfg.clone()).unwrap();
        let mut asked = false;
        while !env.over() {
            assert!(!env.wants(1), "the cpu is not asked");
            if env.wants(0) && !asked {
                asked = true;
                env.observation(0);
                assert_eq!(env.submit(0, &brain), None);
            }
            env.tick();
        }
        env.finish();
        assert!(env.world.round.match_over);
        assert!(
            env.tally[1].damage > 0 || env.tally[0].damage > 0,
            "{:?}",
            env.tally
        );
        let rep = env.replay(["me", "cpu"], std::path::Path::new("/"));
        assert_eq!(rep["ai"], json!([0, 6]));
        let mut again = Env::new(cfg).unwrap();
        let bytes: Vec<u8> = env
            .inputs
            .iter()
            .flat_map(|[a, b]| [a.to_le_bytes(), b.to_le_bytes()].concat())
            .collect();
        let r = rugen_sim::headless::replay(&mut again.world, &bytes, None);
        assert_eq!(r.checksum, env.world.checksum());
    }

    /// KFM's deck has the common cards and its own moves by their .cmd
    /// labels, each of its kind.
    #[test]
    fn cards_come_from_the_fighter() {
        let Some(cfg) = kfm() else { return };
        let env = Env::new(cfg).unwrap();
        let card = |id: &str| env.decks[0].iter().find(|c| c.id == id).cloned();
        assert_eq!(card("approach").unwrap().kind, "stance");
        let palm = card("triple_kung_fu_palm").expect("the super, by its label");
        assert_eq!((palm.kind, palm.power), ("super", 1000));
        assert_eq!(card("stand_light_punch").unwrap().kind, "attack");
        assert_eq!(
            card("crouching_light_kick").unwrap().needs,
            cards::Needs::Ground
        );
        assert_eq!(card("jump_light_punch").unwrap().needs, cards::Needs::Air);
        assert_eq!(card("run_fwd").unwrap().kind, "move");
        let hello = env.hello();
        assert!(hello["fighters"][0]["cards"].as_array().unwrap().len() > 20);
        assert!(hello["rules"]["conditions"].as_array().unwrap().len() >= 8);
    }
}
