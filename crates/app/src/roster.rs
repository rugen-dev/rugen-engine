//! Roster state behind the TOML-driven character select (`motif/ui/select.toml`):
//! the character list with search/sort, both players' picks, the stage strip
//! and the data context the screen binds to. Drawing is the UI engine's job.

use std::path::PathBuf;

use rugen_content::{Entry, Library, Manifest};
use serde_json::{Value, json};

use crate::game::{Mode, Transition};
use crate::play::FightSetup;
use crate::uiscreen::Profile;
use crate::versus::Portrait;

pub struct RosterCell {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

struct StageItem {
    entry: Entry,
    dir: Option<PathBuf>,
    manifest: Option<Manifest>,
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
    human: [bool; 2],
    picks: [Option<usize>; 2],
    /// Which side the next pick goes to (2 = everyone picked).
    picking: usize,
    /// 0 = random, else `stages[i - 1]`.
    stage_idx: usize,
    pub query: String,
    sort: Sort,
    /// Cell under the cursor / focus (index into `cells`).
    hover: Option<usize>,
    ai_level: i32,
    seed: u32,
}

fn verdict_of(m: &Manifest) -> (&'static str, String) {
    match m.verify.as_ref() {
        Some(v) if v.verdict == "verified" => ("ui/badge_ok.webp", "VERIFIED".into()),
        Some(v) if v.verdict == "warn" => (
            "ui/badge_warn.webp",
            format!("RUNS WITH {} ISSUES", v.issues.len()),
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
        let stages = lib
            .stages()
            .into_iter()
            .map(|entry| {
                let prepared = lib
                    .prepare_stage(&entry)
                    .map_err(|e| log::warn!("stage {}: {e}", entry.id))
                    .ok();
                StageItem {
                    entry,
                    dir: prepared.as_ref().map(|(d, _)| d.clone()),
                    manifest: prepared.map(|(_, m)| m),
                }
            })
            .collect();
        let human = match mode {
            Mode::Versus => [true, true],
            Mode::Arcade | Mode::Training | Mode::Tutorial => [true, false],
            Mode::Watch | Mode::Netplay | Mode::Options | Mode::Exit => [false, false],
        };
        let mut r = Self {
            mode,
            cells,
            order: Vec::new(),
            stages,
            lib: lib.clone(),
            human,
            picks: [None, None],
            picking: if human[0] { 0 } else { 1 },
            stage_idx: 0,
            query: String::new(),
            sort: Sort::Name,
            hover: None,
            ai_level,
            seed: 0x2545_f491,
        };
        r.reorder();
        // computer sides pick up front so the plates fill in
        for side in 0..2 {
            if !r.human[side] {
                r.picks[side] = r.random_ok();
            }
        }
        if !human[0] && !human[1] {
            r.picking = 2;
        }
        r.hover = r.order.first().copied();
        r
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

    /// Focus moved onto `cells[i]` of the filtered list: remember which character to show.
    pub fn hover_order(&mut self, i: usize) -> bool {
        let new = self.order.get(i).copied();
        if new.is_some() && new != self.hover {
            self.hover = new;
            return true;
        }
        false
    }

    fn all_picked(&self) -> bool {
        (0..2).all(|s| self.picks[s].is_some())
    }

    fn next_picking(&self) -> usize {
        (0..2)
            .find(|&s| self.human[s] && self.picks[s].is_none())
            .unwrap_or(2)
    }

    /// Both humans pick at the same time (each with their own cursor).
    pub fn dual(&self) -> bool {
        self.human[0] && self.human[1]
    }

    /// Search text from the input widget.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_ascii_lowercase();
        self.reorder();
    }

    /// `side` picks the `i`th cell of the filtered list (None: whoever is next).
    fn pick(&mut self, i: usize, side: Option<usize>) -> Outcome {
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
            Some(s) if self.human[s] => {
                if self.picks[s].is_some() {
                    // pressing again on a picked side = unpick
                    self.picks[s] = None;
                    self.picking = self.next_picking();
                    return Outcome::Refresh;
                }
                Some(s)
            }
            Some(_) => None,
            None if self.picking < 2 => Some(self.picking),
            None => None,
        };
        match target {
            Some(s) => {
                self.picks[s] = Some(cell);
                self.picking = self.next_picking();
                Outcome::Refresh
            }
            None if self.all_picked() => match self.setup() {
                Some(t) => Outcome::Go(Box::new(t)),
                None => Outcome::Refresh,
            },
            None => Outcome::Refresh,
        }
    }

    pub fn action(&mut self, a: &str) -> Outcome {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match head {
            "pick" => match arg.parse::<usize>() {
                Ok(i) => self.pick(i, if self.dual() { Some(0) } else { None }),
                Err(_) => Outcome::None,
            },
            "pick2" => match arg.parse::<usize>() {
                Ok(i) if self.human[1] => self.pick(i, Some(1)),
                _ => Outcome::None,
            },
            "unpick" => {
                if let Some(s) = (0..2)
                    .rev()
                    .find(|&s| self.human[s] && self.picks[s].is_some())
                {
                    self.picks[s] = None;
                    self.picking = s;
                    return Outcome::Refresh;
                }
                Outcome::None
            }
            "query" => {
                self.set_query(arg);
                Outcome::Refresh
            }
            "confirm" => {
                if self.all_picked() {
                    match self.setup() {
                        Some(t) => Outcome::Go(Box::new(t)),
                        None => Outcome::None,
                    }
                } else if let Some(h) = self.hover {
                    let i = self.order.iter().position(|&c| c == h).unwrap_or(0);
                    self.action(&format!("pick:{i}"))
                } else {
                    Outcome::None
                }
            }
            "random" => {
                if self.picking < 2 {
                    self.picks[self.picking] = self.random_ok();
                    self.hover = self.picks[self.picking];
                    self.picking = self.next_picking();
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
            "search" => Outcome::None,
            "sort" => {
                self.sort = match self.sort {
                    Sort::Name => Sort::Author,
                    Sort::Author => Sort::Recent,
                    Sort::Recent => Sort::Name,
                };
                self.reorder();
                Outcome::Refresh
            }
            "back" | "title" => Outcome::Go(Box::new(Transition::Title)),
            _ => Outcome::None,
        }
    }

    fn stage_path(&mut self) -> Option<PathBuf> {
        let chosen = if self.stage_idx == 0 {
            let n = self.stages.len();
            if n == 0 { None } else { Some(self.rand(n)) }
        } else {
            Some(self.stage_idx - 1)
        };
        let path = chosen.and_then(|i| {
            let s = self.stages.get(i)?;
            Some(s.dir.as_ref()?.join(&s.manifest.as_ref()?.def))
        });
        path.or_else(|| {
            let p = self
                .lib
                .root
                .join("reference/mugen-1.1b1/stages/stage0.def");
            p.exists().then_some(p)
        })
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
        let def = |me: &Self, side: usize| -> Option<(PathBuf, String)> {
            let c = me.cells.get(me.picks[side]?)?;
            Some((c.dir.join(&c.manifest.def), c.manifest.id.clone()))
        };
        let (p1, id1) = def(self, 0)?;
        let (p2, id2) = def(self, 1)?;
        let stage = self.stage_path();
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
        };
        if tutorial {
            return Some(Transition::Fight(setup));
        }
        Some(Transition::Versus(
            setup,
            [self.portrait(0), self.portrait(1)],
        ))
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
            "stats": format!("{plays} PLAYS   {wins} WINS"),
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
            Mode::Netplay | Mode::Options | Mode::Exit => "",
        };
        let prompt = if self.all_picked() {
            "READY  -  CONFIRM TO FIGHT".to_string()
        } else if self.dual() {
            match (self.picks[0].is_some(), self.picks[1].is_some()) {
                (false, false) => "P1 AND P2 : CHOOSE YOUR FIGHTERS".to_string(),
                (true, false) => "P2 : CHOOSE YOUR FIGHTER".to_string(),
                _ => "P1 : CHOOSE YOUR FIGHTER".to_string(),
            }
        } else if self.picking < 2 {
            format!("P{} : CHOOSE YOUR FIGHTER", self.picking + 1)
        } else {
            "READY  -  CONFIRM TO FIGHT".to_string()
        };
        let shown = self.char_json(self.hover.or(self.picks[0]).or(self.picks[1]), profile);
        json!({
            "title": title,
            "prompt": prompt,
            "room": room,
            "cells": cells,
            "count": format!("{} / {} FIGHTERS", self.order.len(), self.cells.len()),
            "shown": shown,
            "p1": self.char_json(self.picks[0], profile),
            "p2": self.char_json(self.picks[1], profile),
            "versus": self.human[1] || self.picks[1].is_some(),
            "ready": self.all_picked(),
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
