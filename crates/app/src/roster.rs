//! Roster state behind the TOML-driven character select (`motif/ui/select.toml`):
//! the character list with search/sort, both players' picks, the stage strip
//! and the data context the screen binds to. Drawing is the UI engine's job.

use std::path::PathBuf;

use rugen_content::{Library, Manifest};
use serde_json::{Value, json};

use crate::game::{Mode, Transition};
use crate::play::FightSetup;
use crate::stagesel::{StageItem, StagePlan};
use crate::uiscreen::Profile;
use crate::versus::Portrait;

/// Cells per page of the select grid (12 x 5).
pub const GRID_PAGE: usize = 60;

pub struct RosterCell {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Ticks between the last pick and leaving for the stage select / VS screen.
const READY_TICKS: u32 = 30;

/// What a grid slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Char(usize),
    Random,
    Empty,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Author,
    Recent,
}

pub enum Outcome {
    None,
    /// Data changed; rebuild the bind context.
    Refresh,
    Go(Box<Transition>),
}

pub struct Roster {
    pub mode: Mode,
    cells: Vec<RosterCell>,
    /// Indices into `cells` after filter + sort.
    order: Vec<usize>,
    stages: Vec<StageItem>,
    lib: Library,
    /// Sides a player controls in the fight (the others are the CPU).
    human: [bool; 2],
    /// Sides whose fighter is chosen on this screen; a computer side that is
    /// not chosen gets a random fighter up front (the arcade opponent).
    chooses: [bool; 2],
    picks: [Option<usize>; 2],
    /// Which side the next pick goes to (2 = everyone picked).
    picking: usize,
    /// 0 = random, else `stages[i - 1]`.
    stage_idx: usize,
    pub query: String,
    sort: Sort,
    /// Cell under the cursor / focus (index into `cells`).
    hover: Option<usize>,
    /// Cell under the second player's cursor (dual select).
    hover2: Option<usize>,
    /// The slot under each cursor (an empty slot or the random cell show a
    /// blank card instead of the last fighter).
    hover_slot: [Slot; 2],
    ai_level: i32,
    seed: u32,
    /// Everyone picked: ticks left before the screen moves on (cleared by
    /// an unpick; not started when the picks came back from the stage select).
    ready_in: Option<u32>,
    /// Online: the opponent's name and their fighter once picked.
    foe: (String, Option<String>),
}

/// Why the official server blocks this fighter (cached, verified feed).
fn official_block(m: &Manifest) -> Option<String> {
    m.fingerprint.as_deref().and_then(crate::official::blocked)
}

fn verdict_of(m: &Manifest) -> (&'static str, String) {
    if official_block(m).is_some() {
        return ("ui/badge_bad.webp", "BLOCKED BY THE OFFICIAL SERVER".into());
    }
    match m.verify.as_ref() {
        Some(v) if v.verdict == "verified" => ("ui/badge_ok.webp", "VERIFIED".into()),
        Some(v) if v.verdict == "warn" => (
            "ui/badge_warn.webp",
            format!(
                "RUNS WITH {}",
                crate::uiscreen::plural(v.issues.len(), "ISSUE", "ISSUES")
            ),
        ),
        Some(v) if v.verdict == "broken" => ("ui/badge_bad.webp", "BROKEN".into()),
        _ => ("", "NOT CHECKED".into()),
    }
}

fn is_broken(m: &Manifest) -> bool {
    m.verify.as_ref().is_some_and(|v| v.is_broken())
}

impl Roster {
    pub fn new(mode: Mode, lib: &Library, ai_level: i32) -> Self {
        let mut cells = Vec::new();
        for e in lib.chars() {
            match crate::verify::prepare_verified(lib, &e) {
                Ok((dir, m)) => cells.push(RosterCell { dir, manifest: m }),
                Err(err) => log::warn!("{}: {err}", e.id),
            }
        }
        // the stage is picked on its own screen; only the online pick (the
        // host's stage travels with the fighter) keeps a stage strip here
        let stages = if mode == Mode::Online {
            crate::stagesel::load_stages(lib, mode)
        } else {
            Vec::new()
        };
        let human = match mode {
            Mode::Versus => [true, true],
            Mode::Arcade | Mode::Training | Mode::Tutorial | Mode::Arena | Mode::Online => {
                [true, false]
            }
            Mode::Watch | Mode::Netplay | Mode::Options | Mode::Exit => [false, false],
        };
        // training, arena and watch: the cursor also picks the other side
        let chooses = match mode {
            Mode::Versus | Mode::Training | Mode::Watch | Mode::Arena => [true, true],
            Mode::Arcade | Mode::Tutorial | Mode::Online => [true, false],
            Mode::Netplay | Mode::Options | Mode::Exit => [false, false],
        };
        let mut r = Self {
            mode,
            cells,
            order: Vec::new(),
            stages,
            lib: lib.clone(),
            human,
            chooses,
            picks: [None, None],
            picking: if chooses[0] { 0 } else { 1 },
            stage_idx: 0,
            query: String::new(),
            sort: Sort::Name,
            hover: None,
            hover2: None,
            hover_slot: [Slot::Empty; 2],
            ai_level,
            ready_in: None,
            foe: (String::new(), None),
            // a fresh CPU opponent / random stage every visit
            seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x2545_f491, |d| d.subsec_nanos() | 1),
        };
        r.reorder();
        // computer sides pick up front so the plates fill in (online: the
        // opponent brings their own, unknown until the matchup)
        for side in 0..2 {
            if !r.chooses[side] && mode != Mode::Online {
                r.picks[side] = r.random_ok();
            }
        }
        r.picking = r.next_picking();
        r.hover = r.order.first().copied();
        r.hover_slot = [r.hover.map_or(Slot::Random, Slot::Char); 2];
        r
    }

    /// The last pick went in: leave for the stage select after a beat.
    fn arm_ready(&mut self) {
        if self.all_picked() && self.mode != Mode::Online {
            self.ready_in = Some(READY_TICKS);
        }
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        if n == 0 { 0 } else { (self.seed as usize) % n }
    }

    fn random_ok(&mut self) -> Option<usize> {
        let ok: Vec<usize> = (0..self.cells.len())
            .filter(|&i| !is_broken(&self.cells[i].manifest))
            .collect();
        let pool = if ok.is_empty() {
            (0..self.cells.len()).collect()
        } else {
            ok
        };
        if pool.is_empty() {
            return None;
        }
        let k = self.rand(pool.len());
        Some(pool[k])
    }

    fn reorder(&mut self) {
        let q = self.query.to_ascii_lowercase();
        let mut order: Vec<usize> = (0..self.cells.len())
            .filter(|&i| {
                if q.is_empty() {
                    return true;
                }
                let m = &self.cells[i].manifest;
                m.display_name.to_ascii_lowercase().contains(&q)
                    || m.id.to_ascii_lowercase().contains(&q)
                    || m.author.to_ascii_lowercase().contains(&q)
            })
            .collect();
        match self.sort {
            Sort::Name => {
                order.sort_by_key(|&i| self.cells[i].manifest.display_name.to_ascii_lowercase())
            }
            Sort::Author => order.sort_by_key(|&i| {
                (
                    self.cells[i].manifest.author.to_ascii_lowercase(),
                    self.cells[i].manifest.display_name.to_ascii_lowercase(),
                )
            }),
            Sort::Recent => order.sort_by_key(|&i| {
                std::cmp::Reverse(self.cells[i].manifest.verify.as_ref().map_or(0, |v| v.when))
            }),
        }
        self.order = order;
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// What grid slot `i` holds: a fighter (`cells` index), the random
    /// cell or an empty filler.
    fn slot_of(&self, i: usize) -> Slot {
        match self.order.get(i) {
            Some(&c) => Slot::Char(c),
            None if i == self.order.len() => Slot::Random,
            None => Slot::Empty,
        }
    }

    /// Focus moved onto grid slot `i`: remember what to show on the card.
    pub fn hover_order(&mut self, i: usize) -> bool {
        self.set_hover(0, i)
    }

    /// The second player's focus moved onto grid slot `i`.
    pub fn hover2_order(&mut self, i: usize) -> bool {
        self.set_hover(1, i)
    }

    fn set_hover(&mut self, side: usize, i: usize) -> bool {
        let slot = self.slot_of(i);
        let cell = match slot {
            Slot::Char(c) => Some(c),
            Slot::Random | Slot::Empty => None,
        };
        let (hover, at) = if side == 0 {
            (&mut self.hover, &mut self.hover_slot[0])
        } else {
            (&mut self.hover2, &mut self.hover_slot[1])
        };
        let changed = *hover != cell || *at != slot;
        *hover = cell;
        *at = slot;
        changed
    }

    fn all_picked(&self) -> bool {
        if self.mode == Mode::Online {
            return self.picks[0].is_some();
        }
        (0..2).all(|s| self.picks[s].is_some())
    }

    fn next_picking(&self) -> usize {
        (0..2)
            .find(|&s| self.chooses[s] && self.picks[s].is_none())
            .unwrap_or(2)
    }

    /// Sides whose fighter is still to be picked on this screen.
    pub fn open_picks(&self) -> [bool; 2] {
        [0, 1].map(|s| self.chooses[s] && self.picks[s].is_none())
    }

    /// Both humans pick at the same time (each with their own cursor).
    pub fn dual(&self) -> bool {
        self.human[0] && self.human[1]
    }

    /// Back / Esc: take back `side`'s pick (None: the last pick made on this
    /// screen). False when there was nothing to take back (leave the screen).
    pub fn cancel(&mut self, side: Option<usize>) -> bool {
        let side = match side {
            // two cursors: each player's back only touches their own pick
            Some(s) if self.dual() => Some(s).filter(|&s| self.picks[s].is_some()),
            // one cursor picks everything: back always undoes the last pick
            _ => (0..2)
                .rev()
                .find(|&s| self.chooses[s] && self.picks[s].is_some()),
        };
        match side {
            Some(s) => {
                self.picks[s] = None;
                self.picking = self.next_picking();
                self.ready_in = None;
                true
            }
            // nothing of theirs locked: leave
            None => false,
        }
    }

    /// Picks handed back by the stage select (manifest ids): locked again,
    /// no countdown; one more back changes just that fighter.
    pub fn restore(&mut self, picks: &[Option<String>; 2]) {
        for (side, id) in picks.iter().enumerate() {
            let Some(id) = id else { continue };
            if let Some(i) = self.cells.iter().position(|c| &c.manifest.id == id) {
                self.picks[side] = Some(i);
                if side == 0 || !self.dual() {
                    self.hover = Some(i);
                } else {
                    self.hover2 = Some(i);
                }
            }
        }
        self.picking = self.next_picking();
        self.ready_in = None;
    }

    /// Everyone picked: count down, then move on. `Go` when it is time.
    pub fn tick(&mut self) -> Outcome {
        let Some(left) = self.ready_in.as_mut() else {
            return Outcome::None;
        };
        *left = left.saturating_sub(1);
        if *left > 0 {
            return Outcome::None;
        }
        self.ready_in = None;
        if self.all_picked() {
            self.go()
        } else {
            Outcome::Refresh
        }
    }

    /// Online: who the opponent card shows.
    pub fn set_foe(&mut self, name: String, fighter: Option<String>) {
        self.foe = (name, fighter);
    }

    /// Where a stage is chosen. Online keeps the strip on this screen.
    pub fn stage_strip(&self) -> bool {
        self.mode == Mode::Online
    }

    /// Everyone picked: start (VS screen or straight into the fight).
    fn go(&mut self) -> Outcome {
        match self.setup() {
            Some(t) => Outcome::Go(Box::new(t)),
            None => Outcome::Refresh,
        }
    }

    /// Search text from the input widget.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_ascii_lowercase();
        self.reorder();
    }

    /// `side` picks the `i`th cell of the filtered list (None: whoever is next).
    /// Once everyone has picked, a pick press from either side starts the fight;
    /// a side that already picked keeps its fighter until it backs out.
    fn pick(&mut self, i: usize, side: Option<usize>) -> Outcome {
        if self.all_picked() {
            return self.go();
        }
        let Some(&cell) = self.order.get(i) else {
            return Outcome::None;
        };
        if is_broken(&self.cells[cell].manifest) {
            return Outcome::None;
        }
        if side.is_none_or(|s| s == 0) {
            self.hover = Some(cell);
        }
        let target = match side {
            Some(s) if self.chooses[s] && self.picks[s].is_none() => Some(s),
            Some(_) => None,
            None if self.picking < 2 => Some(self.picking),
            None => None,
        };
        if let Some(s) = target {
            self.picks[s] = Some(cell);
            self.picking = self.next_picking();
            self.arm_ready();
        }
        // online: one pick and back to the matchup
        if self.mode == Mode::Online && self.picks[0].is_some() {
            return self.go();
        }
        Outcome::Refresh
    }

    pub fn action(&mut self, a: &str) -> Outcome {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match head {
            "pick" => match arg.parse::<usize>() {
                Ok(i) => self.pick(i, if self.dual() { Some(0) } else { None }),
                Err(_) => Outcome::None,
            },
            "pick2" => match arg.parse::<usize>() {
                Ok(i) if self.dual() => self.pick(i, Some(1)),
                _ => Outcome::None,
            },
            "unpick" => {
                if self.cancel(None) {
                    Outcome::Refresh
                } else {
                    Outcome::None
                }
            }
            "query" => {
                self.set_query(arg);
                Outcome::Refresh
            }
            "confirm" => {
                if self.all_picked() {
                    self.go()
                } else if let Some(h) = self.hover {
                    let i = self.order.iter().position(|&c| c == h).unwrap_or(0);
                    self.action(&format!("pick:{i}"))
                } else {
                    Outcome::None
                }
            }
            _ if head.starts_with("random") && self.all_picked() => self.go(),
            "random2" => {
                if self.dual() && self.picks[1].is_none() {
                    self.picks[1] = self.random_ok();
                    self.hover2 = self.picks[1];
                    self.picking = self.next_picking();
                    self.arm_ready();
                }
                Outcome::Refresh
            }
            "random" if self.dual() => {
                if self.picks[0].is_none() {
                    self.picks[0] = self.random_ok();
                    self.hover = self.picks[0];
                    self.picking = self.next_picking();
                    self.arm_ready();
                }
                Outcome::Refresh
            }
            "random" => {
                if self.picking < 2 {
                    self.picks[self.picking] = self.random_ok();
                    self.hover = self.picks[self.picking];
                    self.picking = self.next_picking();
                    self.arm_ready();
                }
                Outcome::Refresh
            }
            "stage" => {
                let n = self.stages.len() + 1;
                match arg {
                    "prev" => self.stage_idx = (self.stage_idx + n - 1) % n,
                    "next" => self.stage_idx = (self.stage_idx + 1) % n,
                    "random" => self.stage_idx = 0,
                    other => {
                        if let Ok(i) = other.parse::<usize>() {
                            self.stage_idx = i.min(n - 1);
                        }
                    }
                }
                Outcome::Refresh
            }
            // an empty slot: nothing to pick
            "search" | "empty" => Outcome::None,
            "sort" => {
                self.sort = match self.sort {
                    Sort::Name => Sort::Author,
                    Sort::Author => Sort::Recent,
                    Sort::Recent => Sort::Name,
                };
                self.reorder();
                Outcome::Refresh
            }
            "back" | "title" if self.mode == Mode::Online => {
                Outcome::Go(Box::new(Transition::Online))
            }
            "back" | "title" => Outcome::Go(Box::new(Transition::Title)),
            _ => Outcome::None,
        }
    }

    fn portrait(&self, side: usize) -> Portrait {
        let c = self.picks[side].and_then(|i| self.cells.get(i));
        let img = c.and_then(|c| {
            let p = c.manifest.portrait_big.as_ref()?;
            let (w, h, rgba) = rugen_content::read_png(&c.dir.join(&p.file)).ok()?;
            Some((w, h, rgba, p.axis))
        });
        Portrait::new(
            c.map(|c| c.manifest.display_name.clone())
                .unwrap_or_default(),
            img,
        )
    }

    fn setup(&mut self) -> Option<Transition> {
        if self.mode == Mode::Online {
            let id = self.cells.get(self.picks[0]?)?.manifest.id.clone();
            // RANDOM stage: settle it here so it travels as a real stage
            let stage = if self.stage_idx == 0 {
                let n = self.stages.len();
                (n > 0).then(|| self.rand(n))
            } else {
                Some(self.stage_idx - 1)
            }
            .and_then(|i| self.stages.get(i))
            .map(|s| s.entry.id.clone());
            return Some(Transition::OnlinePicked(id, stage));
        }
        let def = |me: &Self, side: usize| -> Option<(PathBuf, String)> {
            let c = me.cells.get(me.picks[side]?)?;
            Some((c.manifest.def_path(&c.dir), c.manifest.id.clone()))
        };
        let (p1, id1) = def(self, 0)?;
        let (p2, id2) = def(self, 1)?;
        // arcade: the ladder decides (a random first stage), no stage select
        let stage = if self.mode == Mode::Arcade {
            self.arcade_stage()
        } else {
            None
        };
        let ai = |human: bool, lvl: i32| if human { 0 } else { lvl };
        let tutorial = self.mode == Mode::Tutorial;
        let setup = FightSetup {
            p1: p1.clone(),
            p2: if tutorial { p1 } else { p2 },
            stage,
            ai: match self.mode {
                Mode::Training | Mode::Tutorial => [0, 0],
                _ => [
                    ai(self.human[0], self.ai_level),
                    ai(self.human[1], self.ai_level),
                ],
            },
            net: None,
            tutorial,
            ids: [id1.clone(), if tutorial { id1 } else { id2 }],
            mode: Some(self.mode),
            fighter_size: None,
            arena_ai: None,
        };
        if tutorial {
            let mut setup = setup;
            setup.stage = self.lib.default_stage();
            return Some(Transition::Fight(setup));
        }
        let portraits = [self.portrait(0), self.portrait(1)];
        if self.mode == Mode::Arcade {
            return Some(Transition::Versus(setup, portraits));
        }
        let name = |me: &Self, s: usize| {
            me.picks[s]
                .and_then(|i| me.cells.get(i))
                .map(|c| c.manifest.display_name.clone())
                .unwrap_or_default()
        };
        Some(Transition::StageSelect(Box::new(StagePlan {
            mode: self.mode,
            names: [name(self, 0), name(self, 1)],
            picks: [setup.ids[0].clone(), setup.ids[1].clone()],
            setup,
            portraits,
        })))
    }

    /// Arcade's first stage: any installed one (only that one is prepared).
    fn arcade_stage(&mut self) -> Option<PathBuf> {
        let all = self.lib.stages();
        let pick = (!all.is_empty()).then(|| self.rand(all.len()));
        pick.and_then(|i| all.get(i))
            .and_then(|e| self.lib.prepare_stage(e).ok())
            .map(|(d, m)| m.def_path(&d))
            .or_else(|| self.lib.default_stage())
    }

    fn cell_json(&self, order_i: usize, cell: usize) -> Value {
        let c = &self.cells[cell];
        let (badge, _) = verdict_of(&c.manifest);
        let thumb = c
            .manifest
            .portrait_small
            .as_ref()
            .map(|p| c.dir.join(&p.file).display().to_string())
            .unwrap_or_default();
        json!({
            "index": order_i,
            "id": c.manifest.id,
            "name": c.manifest.display_name,
            "thumb": thumb,
            "badge": badge,
            "broken": is_broken(&c.manifest),
            "p1": self.picks[0] == Some(cell),
            "p2": self.picks[1] == Some(cell),
            "hover": self.hover == Some(cell),
        })
    }

    /// The select grid: one cell per filtered character, a random cell, then
    /// empty fillers up to a whole page of `GRID_PAGE` cells.
    fn grid_json(&self) -> Vec<Value> {
        let mut grid: Vec<Value> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let mut v = self.cell_json(i, c);
                let m = &self.cells[c].manifest;
                let face = m
                    .portrait_big
                    .as_ref()
                    .or(m.portrait_small.as_ref())
                    .map(|p| self.cells[c].dir.join(&p.file).display().to_string())
                    .unwrap_or_default();
                v["face"] = Value::from(face.clone());
                v["has_face"] = Value::from(!face.is_empty());
                v["face_ok"] = Value::from(!face.is_empty() && !is_broken(m));
                v["face_grey"] = Value::from(!face.is_empty() && is_broken(m));
                v["warn"] = Value::from(m.verify.as_ref().is_some_and(|v| v.verdict == "warn"));
                v["press"] = Value::from(format!("pick:{i}"));
                v["kind"] = Value::from(if is_broken(m) { "broken" } else { "char" });
                v
            })
            .collect();
        let n = grid.len();
        grid.push(json!({
            "index": n, "kind": "random", "press": "random", "name": "RANDOM",
            "face": "", "has_face": false, "warn": false, "broken": false,
        }));
        let total = grid.len().div_ceil(GRID_PAGE).max(1) * GRID_PAGE;
        while grid.len() < total {
            let i = grid.len();
            grid.push(json!({
                "index": i, "kind": "empty", "press": "empty", "name": "",
                "face": "", "has_face": false, "warn": false, "broken": false,
            }));
        }
        grid
    }

    /// A player's card: their pick, else what their cursor is on.
    fn card_json(&self, side: usize, profile: &Profile) -> Value {
        // one cursor picks every side; two cursors each fill their own card
        let (hover, slot) = if self.dual() && side == 1 {
            (self.hover2, self.hover_slot[1])
        } else {
            (self.hover, self.hover_slot[0])
        };
        let cursor = if self.dual() {
            true
        } else {
            self.picking == side
        };
        let cell = self.picks[side].or(if cursor { hover } else { None });
        let mut v = self.char_json(cell, profile);
        // a cursor on an empty slot / the random cell: a blank card
        let blank = match (self.picks[side], cursor, slot) {
            (None, true, Slot::Empty) => "NO FIGHTER",
            (None, true, Slot::Random) => "RANDOM",
            _ => "",
        };
        v["blank"] = Value::from(!blank.is_empty());
        v["blank_text"] = Value::from(blank);
        let Some(c) = cell.and_then(|i| self.cells.get(i)) else {
            let online_foe = self.mode == Mode::Online && side == 1;
            v["status"] = Value::from(if self.human[side] {
                String::new()
            } else if online_foe {
                // their fighter once they picked, else a waiting line
                match &self.foe.1 {
                    Some(f) => format!("BRINGS {f}"),
                    None => "WAITING FOR THEIR PICK...".to_string(),
                }
            } else {
                "CPU".to_string()
            });
            if online_foe {
                let name = if self.foe.0.is_empty() {
                    "OPPONENT".to_string()
                } else {
                    self.foe.0.to_uppercase()
                };
                let long = name.chars().count() > 12;
                v["name_up"] = Value::from(name);
                v["name_big"] = Value::from(!long);
                v["name_small"] = Value::from(long);
                v["ok"] = Value::from(true);
                v["bad"] = Value::from(false);
            }
            v["picked"] = Value::from(false);
            v["cpu"] = Value::from(!self.human[side] && !online_foe);
            v["portrait"] = Value::from("");
            return v;
        };
        let m = &c.manifest;
        let portrait = m
            .art
            .as_deref()
            .or(m.portrait_big.as_ref().map(|p| p.file.as_str()))
            .map(|f| c.dir.join(f).display().to_string())
            .unwrap_or_default();
        let (status, ok) = match m.verify.as_ref() {
            Some(v) if v.verdict == "verified" => ("✓ VERIFIED".to_string(), true),
            Some(v) if v.verdict == "warn" => (
                format!(
                    "⚠ {}",
                    crate::uiscreen::plural(v.issues.len(), "KNOWN ISSUE", "KNOWN ISSUES")
                ),
                false,
            ),
            Some(v) if v.verdict == "broken" => ("⚠ BROKEN".to_string(), false),
            _ => ("NOT CHECKED".to_string(), true),
        };
        // the official blocklist wins: this fighter is refused online everywhere
        let (status, ok) = match official_block(m) {
            Some(why) => (format!("⚠ OFFICIAL BLOCK: {why}"), false),
            None => (status, ok),
        };
        let st = profile.chars.get(&m.id);
        let (plays, wins) = st.map_or((0, 0), |s| (s.plays, s.wins));
        v["portrait"] = Value::from(portrait);
        v["name_up"] = Value::from(m.display_name.to_uppercase());
        // long names drop a size so they stay on the card
        let long = m.display_name.chars().count() > 12;
        v["name_big"] = Value::from(!long);
        v["name_small"] = Value::from(long);
        v["status"] = Value::from(status);
        v["ok"] = Value::from(ok);
        v["bad"] = Value::from(!ok);
        v["record"] = Value::from(format!(
            "{} · {}",
            crate::uiscreen::plural(plays as usize, "PLAY", "PLAYS"),
            crate::uiscreen::plural(wins as usize, "WIN", "WINS")
        ));
        v["picked"] = Value::from(self.picks[side].is_some());
        v["cpu"] = Value::from(!self.human[side]);
        v
    }

    fn char_json(&self, cell: Option<usize>, profile: &Profile) -> Value {
        let Some(c) = cell.and_then(|i| self.cells.get(i)) else {
            return json!({
                "name": "", "author": "", "art": "", "verdict": "",
                "badge": "", "stats": "", "present": false
            });
        };
        let (badge, verdict) = verdict_of(&c.manifest);
        let art = c
            .manifest
            .art
            .as_deref()
            .or(c.manifest.portrait_big.as_ref().map(|p| p.file.as_str()))
            .map(|f| c.dir.join(f).display().to_string())
            .unwrap_or_default();
        let st = profile.chars.get(&c.manifest.id);
        let (plays, wins) = st.map_or((0, 0), |s| (s.plays, s.wins));
        let author = if c.manifest.author.is_empty() {
            "UNKNOWN".to_string()
        } else {
            c.manifest.author.to_uppercase()
        };
        json!({
            "name": c.manifest.display_name,
            "author": author,
            "art": art,
            "verdict": verdict,
            "badge": badge,
            "stats": format!(
                "{}   {}",
                crate::uiscreen::plural(plays as usize, "PLAY", "PLAYS"),
                crate::uiscreen::plural(wins as usize, "WIN", "WINS")
            ),
            "present": true,
        })
    }

    pub fn data(&self, profile: &Profile, room: &str) -> Value {
        let cells: Vec<Value> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, &c)| self.cell_json(i, c))
            .collect();
        let stage_item = self
            .stage_idx
            .checked_sub(1)
            .and_then(|i| self.stages.get(i));
        let stage_name = match stage_item {
            None => "RANDOM".to_string(),
            Some(s) => s
                .manifest
                .as_ref()
                .map(|m| m.display_name.clone())
                .unwrap_or_else(|| s.entry.id.clone())
                .to_uppercase(),
        };
        let stage_thumb = stage_item
            .and_then(|s| {
                let t = s.manifest.as_ref()?.thumb.as_ref()?;
                Some(s.dir.as_ref()?.join(&t.file).display().to_string())
            })
            .unwrap_or_default();
        let title = match self.mode {
            Mode::Arcade => "ARCADE",
            Mode::Versus => "VS MODE",
            Mode::Training => "TRAINING",
            Mode::Tutorial => "TUTORIAL",
            Mode::Watch => "WATCH",
            Mode::Arena => "ARENA",
            Mode::Online => "ONLINE",
            Mode::Netplay | Mode::Options | Mode::Exit => "",
        };
        let next = if self.mode == Mode::Arcade {
            "CONFIRM TO FIGHT"
        } else {
            "CONFIRM FOR THE STAGE"
        };
        let prompt = if self.mode == Mode::Online {
            "BRING YOUR FIGHTER".to_string()
        } else if self.all_picked() && self.ready_in.is_some() {
            "READY!".to_string()
        } else if self.all_picked() {
            format!("READY  -  {next}")
        } else if self.dual() {
            match (self.picks[0].is_some(), self.picks[1].is_some()) {
                (false, false) => "P1 AND P2 : CHOOSE YOUR FIGHTERS".to_string(),
                (true, false) => "P2 : CHOOSE YOUR FIGHTER".to_string(),
                _ => "P1 : CHOOSE YOUR FIGHTER".to_string(),
            }
        } else if self.picking < 2 && !self.human[self.picking] {
            match (self.mode, self.picking) {
                (Mode::Watch, 0) => "CHOOSE THE 1P FIGHTER (CPU)".to_string(),
                (Mode::Watch, _) => "CHOOSE THE 2P FIGHTER (CPU)".to_string(),
                (Mode::Training, _) => "P1 : CHOOSE YOUR SPARRING PARTNER".to_string(),
                _ => "P1 : CHOOSE YOUR OPPONENT".to_string(),
            }
        } else if self.picking < 2 {
            format!("P{} : CHOOSE YOUR FIGHTER", self.picking + 1)
        } else {
            format!("READY  -  {next}")
        };
        let shown = self.char_json(self.hover.or(self.picks[0]).or(self.picks[1]), profile);
        let warns = self
            .cells
            .iter()
            .filter(|c| {
                c.manifest
                    .verify
                    .as_ref()
                    .is_some_and(|v| v.verdict == "warn")
            })
            .count();
        let broken = self.cells.iter().filter(|c| is_broken(&c.manifest)).count();
        json!({
            "title": title,
            "prompt": prompt,
            "room": room,
            "cells": cells,
            "count": format!("{} / {} FIGHTERS", self.order.len(), self.cells.len()),
            "grid": self.grid_json(),
            "roster_line": format!(
                "{} FIGHTER{} · {} WITH WARNINGS · {} BROKEN",
                self.cells.len(),
                if self.cells.len() == 1 { "" } else { "S" },
                warns,
                broken
            ),
            "filter": if self.query.is_empty() { String::new() } else { format!("SEARCH: {}", self.query.to_uppercase()) },
            "card1": self.card_json(0, profile),
            "card2": self.card_json(1, profile),
            "shown": shown,
            "p1": self.char_json(self.picks[0], profile),
            "p2": self.char_json(self.picks[1], profile),
            "versus": self.human[1] || self.picks[1].is_some(),
            "ready": self.all_picked(),
            "stage_strip": self.stage_strip(),
            "dual": self.dual(),
            "query": self.query,
            "sort": match self.sort {
                Sort::Name => "NAME",
                Sort::Author => "AUTHOR",
                Sort::Recent => "RECENT",
            },
            "stage": {
                "name": stage_name,
                "thumb": stage_thumb,
                "has_thumb": !stage_thumb.is_empty(),
                "index": self.stage_idx,
                "count": self.stages.len(),
            },
            "profile": serde_json::to_value(profile).unwrap_or(Value::Null),
            "rating": profile.rating_text(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap().flatten() {
            if e.path().is_dir() {
                copy_dir(&e.path(), &dst.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
            }
        }
    }

    /// Cursors may rest on empty slots and the random cell: blank card, no pick.
    #[test]
    fn empty_slots_show_a_blank_card() {
        let reference =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        if !reference.join("chars/kfm/kfm.def").exists() {
            return;
        }
        let root = std::env::temp_dir().join(format!("rugen-roster-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_dir(&reference.join("chars/kfm"), &root.join("chars/kfm"));
        copy_dir(&reference.join("data"), &root.join("data"));
        let lib = Library::discover(Some(&root));
        let mut r = Roster::new(Mode::Versus, &lib, 4);
        let p = Profile::default();
        assert_eq!(r.data(&p, "")["card1"]["present"], true);
        // slot 0 = kfm, 1 = random, 2.. = empty
        assert!(r.hover_order(5));
        assert!(r.hover2_order(1));
        let d = r.data(&p, "");
        assert_eq!(d["card1"]["present"], false);
        assert_eq!(d["card1"]["blank_text"], "NO FIGHTER");
        assert_eq!(d["card2"]["blank_text"], "RANDOM");
        assert_eq!(d["grid"][5]["press"], "empty");
        assert!(matches!(r.action("empty"), Outcome::None));
        assert!(r.picks == [None, None], "an empty slot picks nobody");
        // back onto the fighter: the card fills in again
        assert!(r.hover_order(0));
        assert_eq!(r.data(&p, "")["card1"]["present"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Online pick: one cursor, no CPU opponent, one press goes back to the matchup.
    #[test]
    fn online_mode_picks_one_fighter_and_a_stage() {
        let reference =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        if !reference.join("chars/kfm/kfm.def").exists() {
            return;
        }
        let root = std::env::temp_dir().join(format!("rugen-roster-online-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_dir(&reference.join("chars/kfm"), &root.join("chars/kfm"));
        copy_dir(&reference.join("data"), &root.join("data"));
        std::fs::create_dir_all(root.join("stages/stage0")).unwrap();
        for f in ["stage0.def", "stage0.sff"] {
            std::fs::copy(
                reference.join("stages").join(f),
                root.join("stages/stage0").join(f),
            )
            .unwrap();
        }
        let lib = Library::discover(Some(&root));
        let mut r = Roster::new(Mode::Online, &lib, 4);
        assert!(!r.dual());
        assert!(r.picks[1].is_none(), "no CPU opponent online");
        let d = r.data(&Profile::default(), "ONLINE");
        assert_eq!(d["title"], "ONLINE");
        assert_eq!(d["card2"]["cpu"], false);
        // the opponent card names them and waits for their pick
        assert_eq!(d["card2"]["name_up"], "OPPONENT");
        assert_eq!(d["card2"]["status"], "WAITING FOR THEIR PICK...");
        r.set_foe("Ayame".into(), Some("SASUKE".into()));
        let d = r.data(&Profile::default(), "ONLINE");
        assert_eq!(d["card2"]["name_up"], "AYAME");
        assert_eq!(d["card2"]["name_big"], true);
        assert_eq!(d["card2"]["status"], "BRINGS SASUKE");
        match r.action("stage:next") {
            Outcome::Refresh => {}
            _ => panic!("stage strip"),
        }
        match r.action("pick:0") {
            Outcome::Go(t) => match *t {
                Transition::OnlinePicked(id, stage) => {
                    assert_eq!(id, "kfm");
                    assert_eq!(stage.as_deref(), Some("stage0"));
                }
                other => panic!("{other:?}"),
            },
            _ => panic!("one pick goes back to the matchup"),
        }
        match r.action("back") {
            Outcome::Go(t) => assert!(matches!(*t, Transition::Online)),
            _ => panic!("back returns to the online screen"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
