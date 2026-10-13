//! Character and stage select: system.def `[Select Info]` grid over `[SelectBG]`,
//! fed by the content library (portraits come from the cache manifests).

use std::path::PathBuf;

use rugen_content::{Entry, Library, Manifest};
use rugen_render::{Camera, Draw, Renderer, TextureId};
use rugen_sim::Buttons;

use crate::game::{Mode, Transition};
use crate::play::FightSetup;
use crate::stage::StageVisual;
use crate::system::SystemData;
use crate::versus::Portrait;

struct Cell {
    dir: PathBuf,
    manifest: Manifest,
    small: Option<(u32, u32, Vec<u8>, [i32; 2])>,
    big: Option<(u32, u32, Vec<u8>, [i32; 2])>,
    small_tex: Option<TextureId>,
    big_tex: Option<TextureId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Chars,
    Stage,
    Done,
}

pub struct SelectScreen {
    mode: Mode,
    cells: Vec<Cell>,
    stages: Vec<Entry>,
    lib: Library,
    cursor: [[i32; 2]; 2],
    done: [bool; 2],
    /// Index into `stages` (+1), 0 = random.
    stage_idx: usize,
    phase: Phase,
    bg: Option<StageVisual>,
    tick: i32,
    /// Which sides a human controls.
    human: [bool; 2],
    random_pick: [Option<usize>; 2],
    seed: u32,
    ai_level: i32,
}

impl SelectScreen {
    pub fn new(mode: Mode, sys: &SystemData, lib: &Library, ai_level: i32) -> Self {
        let bg = match StageVisual::load_prefixed(&sys.def_path, "selectbg", Some(&sys.sff_path)) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("select background: {e:#}");
                None
            }
        };
        let mut cells = Vec::new();
        for e in lib.chars() {
            match crate::verify::prepare_verified(lib, &e) {
                Ok((dir, m)) => {
                    let load = |p: &Option<rugen_content::PortraitInfo>| {
                        let p = p.as_ref()?;
                        let (w, h, rgba) = rugen_content::read_png(&dir.join(&p.file)).ok()?;
                        Some((w, h, rgba, p.axis))
                    };
                    let small = load(&m.portrait_small);
                    let big = load(&m.portrait_big);
                    cells.push(Cell {
                        dir,
                        manifest: m,
                        small,
                        big,
                        small_tex: None,
                        big_tex: None,
                    });
                }
                Err(err) => log::warn!("{}: {err}", e.id),
            }
        }
        let human = match mode {
            Mode::Versus => [true, true],
            Mode::Arcade | Mode::Training | Mode::Tutorial | Mode::Arena | Mode::Online => {
                [true, false]
            }
            Mode::Watch | Mode::Netplay | Mode::Options | Mode::Exit => [false, false],
        };
        let s = &sys.select;
        let n = cells.len() as i32;
        let cols = s.columns as i32;
        let clamp = |c: [i32; 2]| -> [i32; 2] {
            let i = (c[0] * cols + c[1]).clamp(0, (n - 1).max(0));
            [i / cols.max(1), i % cols.max(1)]
        };
        Self {
            mode,
            cells,
            stages: lib.stages(),
            lib: lib.clone(),
            cursor: [clamp(s.cursor_start[0]), clamp(s.cursor_start[1])],
            done: [false; 2],
            stage_idx: 0,
            phase: Phase::Chars,
            bg,
            tick: 0,
            human,
            random_pick: [None, None],
            seed: 0x2545_f491,
            ai_level,
        }
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        if n == 0 { 0 } else { (self.seed as usize) % n }
    }

    fn cell_index(&self, sys: &SystemData, c: [i32; 2]) -> Option<usize> {
        let cols = sys.select.columns as i32;
        if c[0] < 0 || c[1] < 0 || c[1] >= cols {
            return None;
        }
        let i = (c[0] * cols + c[1]) as usize;
        (i < self.cells.len()).then_some(i)
    }

    fn move_cursor(&mut self, sys: &SystemData, side: usize, d: [i32; 2]) {
        let s = &sys.select;
        let (rows, cols) = (s.rows as i32, s.columns as i32);
        if rows <= 0 || cols <= 0 {
            return;
        }
        // every slot of the grid is reachable, empty ones included (they
        // show no face and cannot be picked)
        let mut c = [self.cursor[side][0] + d[0], self.cursor[side][1] + d[1]];
        if s.wrapping {
            c = [(c[0] + rows) % rows, (c[1] + cols) % cols];
        } else if c[0] < 0 || c[0] >= rows || c[1] < 0 || c[1] >= cols {
            return;
        }
        self.cursor[side] = c;
    }

    fn setup(&mut self) -> Option<Transition> {
        let pick = |me: &Self, side: usize| -> Option<PathBuf> {
            let i = me.random_pick[side]?;
            let c = me.cells.get(i)?;
            Some(c.manifest.def_path(&c.dir))
        };
        let p1 = pick(self, 0)?;
        let p2 = pick(self, 1)?;
        let stage = if self.stage_idx == 0 {
            let n = self.stages.len();
            if n == 0 {
                None
            } else {
                let k = self.rand(n);
                self.stages.get(k).cloned()
            }
        } else {
            self.stages.get(self.stage_idx - 1).cloned()
        };
        let stage = stage.and_then(|e| {
            self.lib
                .prepare_stage(&e)
                .map(|(d, m)| m.def_path(&d))
                .map_err(|err| log::warn!("stage {}: {err}", e.id))
                .ok()
        });
        // no stages installed: fall back to the bundled training stage if present
        let stage = stage.or_else(|| self.lib.default_stage());
        let ai = |human: bool| if human { 0 } else { self.ai_level };
        let tutorial = self.mode == Mode::Tutorial;
        let setup = FightSetup {
            p1: p1.clone(),
            p2: if tutorial { p1 } else { p2 },
            stage,
            ai: match self.mode {
                Mode::Training | Mode::Tutorial => [0, 0],
                _ => [ai(self.human[0]), ai(self.human[1])],
            },
            net: None,
            tutorial,
            ids: [String::new(), String::new()],
            mode: Some(self.mode),
            fighter_size: None,
        };
        if tutorial {
            return Some(Transition::Fight(setup));
        }
        let portrait = |me: &Self, side: usize| {
            let c = me.random_pick[side].and_then(|i| me.cells.get(i));
            Portrait::new(
                c.map(|c| c.manifest.display_name.clone())
                    .unwrap_or_default(),
                c.and_then(|c| c.big.clone()),
            )
        };
        Some(Transition::Versus(
            setup,
            [portrait(self, 0), portrait(self, 1)],
        ))
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn tick(
        &mut self,
        pressed: [Buttons; 2],
        sys: &SystemData,
        sfx: &mut dyn FnMut(Option<(i32, i32)>),
    ) -> Option<Transition> {
        self.tick += 1;
        if let Some(b) = self.bg.as_mut() {
            b.step();
        }
        if self.cells.is_empty() {
            return Some(Transition::Title);
        }
        match self.phase {
            Phase::Chars => {
                for (side, &b) in pressed.iter().enumerate() {
                    if self.done[side] {
                        continue;
                    }
                    if !self.human[side] {
                        // computer side: pick once P1 is done (or immediately in watch mode)
                        if side == 0 || self.done[0] {
                            // computer picks among characters that passed verification
                            let ok: Vec<usize> = (0..self.cells.len())
                                .filter(|&i| {
                                    !self.cells[i]
                                        .manifest
                                        .verify
                                        .as_ref()
                                        .is_some_and(|v| v.is_broken())
                                })
                                .collect();
                            let pool = if ok.is_empty() {
                                (0..self.cells.len()).collect()
                            } else {
                                ok
                            };
                            let k = pool[self.rand(pool.len())];
                            self.random_pick[side] = Some(k);
                            self.done[side] = true;
                        }
                        continue;
                    }
                    if b.u {
                        self.move_cursor(sys, side, [-1, 0]);
                        sfx(sys.select.move_snd[side]);
                    }
                    if b.d {
                        self.move_cursor(sys, side, [1, 0]);
                        sfx(sys.select.move_snd[side]);
                    }
                    if b.l {
                        self.move_cursor(sys, side, [0, -1]);
                        sfx(sys.select.move_snd[side]);
                    }
                    if b.r {
                        self.move_cursor(sys, side, [0, 1]);
                        sfx(sys.select.move_snd[side]);
                    }
                    let accept = b.a || b.b || b.c || b.x || b.y || b.z || b.s;
                    if accept && self.cell_index(sys, self.cursor[side]).is_none() {
                        // an empty slot: nothing to pick
                        sfx(sys.select.cancel_snd);
                    }
                    if accept && let Some(i) = self.cell_index(sys, self.cursor[side]) {
                        let broken = self.cells[i]
                            .manifest
                            .verify
                            .as_ref()
                            .is_some_and(|v| v.is_broken());
                        if broken {
                            // failed verification: cannot be picked
                            sfx(sys.select.cancel_snd);
                        } else {
                            self.random_pick[side] = Some(i);
                            self.done[side] = true;
                            sfx(sys.select.done_snd[side]);
                        }
                    }
                }
                if self.done[0] && self.done[1] {
                    self.phase = Phase::Stage;
                    if (!self.human[0] && !self.human[1]) || self.mode == Mode::Tutorial {
                        self.phase = Phase::Done;
                    }
                }
            }
            Phase::Stage => {
                let n = self.stages.len() + 1;
                let b = if self.human[0] {
                    pressed[0]
                } else {
                    pressed[1]
                };
                if b.l {
                    self.stage_idx = (self.stage_idx + n - 1) % n;
                    sfx(sys.select.stage_move_snd);
                }
                if b.r {
                    self.stage_idx = (self.stage_idx + 1) % n;
                    sfx(sys.select.stage_move_snd);
                }
                if b.a || b.b || b.c || b.x || b.y || b.z || b.s {
                    sfx(sys.select.stage_done_snd);
                    self.phase = Phase::Done;
                }
            }
            Phase::Done => {}
        }
        if self.phase == Phase::Done {
            return self.setup().or(Some(Transition::Title));
        }
        None
    }

    fn upload(r: &mut Renderer, src: &Option<(u32, u32, Vec<u8>, [i32; 2])>) -> Option<TextureId> {
        let (w, h, rgba, _) = src.as_ref()?;
        Some(r.add_rgba(*w, *h, rgba))
    }

    pub fn render(&mut self, r: &mut Renderer, sys: &mut SystemData) -> (Camera, [f64; 4]) {
        let lc = sys.localcoord;
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let sw = lc[1] * aspect;
        let clear = self
            .bg
            .as_ref()
            .map_or([0.0, 0.0, 0.0, 1.0], |b| b.clear_color);
        if let Some(b) = self.bg.as_mut() {
            b.draw(r, 0, [0.0, 0.0], 1.0, sw, Default::default());
        }
        let ox = -lc[0] / 2.0;
        let s = sys.select.clone();
        // title
        let title = match self.mode {
            Mode::Arcade => "Arcade",
            Mode::Versus => "VS Mode",
            Mode::Training => "Training",
            Mode::Watch => "Watch",
            Mode::Arena => "Arena",
            Mode::Tutorial => "Tutorial",
            Mode::Online => "Online",
            Mode::Netplay | Mode::Options | Mode::Exit => "",
        };
        sys.text(r, s.title_font, title, s.title_offset);
        // grid
        for row in 0..s.rows {
            for col in 0..s.columns {
                let x = s.pos[0] + col as f32 * (s.cell_size[0] + s.cell_spacing);
                let y = s.pos[1] + row as f32 * (s.cell_size[1] + s.cell_spacing);
                let idx = row * s.columns + col;
                let present = idx < self.cells.len();
                if !present {
                    continue;
                }
                if let Some((g, n)) = s.cell_bg
                    && let Some(p) = sys.sheet.get(r, g, n)
                {
                    let d = Draw::new(p.tex, p.pal, [x + ox, y], p.axis);
                    r.draw(d);
                }
                let cell = &mut self.cells[idx];
                if cell.small_tex.is_none() {
                    cell.small_tex = Self::upload(r, &cell.small);
                }
                if let Some(tex) = cell.small_tex
                    && let Some((_, _, _, axis)) = cell.small
                {
                    let mut d = Draw::new(
                        tex,
                        None,
                        [x + ox + s.portrait_offset[0], y + s.portrait_offset[1]],
                        [axis[0] as f32, axis[1] as f32],
                    );
                    d.scale = s.portrait_scale;
                    r.draw(d);
                }
                // verification badge: green verified, yellow warnings, red broken
                let badge = match cell.manifest.verify.as_ref().map(|v| v.verdict.as_str()) {
                    Some("verified") => Some([0.2, 0.9, 0.3, 1.0]),
                    Some("warn") => Some([1.0, 0.8, 0.1, 1.0]),
                    Some("broken") => Some([1.0, 0.15, 0.15, 1.0]),
                    _ => None,
                };
                if let Some(c) = badge {
                    let bx = x + ox + s.cell_size[0] - 5.0;
                    let by = y + s.cell_size[1] - 5.0;
                    r.rect(
                        [bx - 1.0, by - 1.0],
                        [bx + 4.0, by + 4.0],
                        [0.0, 0.0, 0.0, 0.8],
                    );
                    r.rect([bx, by], [bx + 3.0, by + 3.0], c);
                }
                if cell.manifest.verify.as_ref().is_some_and(|v| v.is_broken()) {
                    // dim broken characters
                    r.rect(
                        [x + ox, y],
                        [x + ox + s.cell_size[0], y + s.cell_size[1]],
                        [0.0, 0.0, 0.0, 0.55],
                    );
                }
            }
        }
        // cursors
        for side in 0..2 {
            if !self.human[side] {
                continue;
            }
            let c = self.cursor[side];
            let x = s.pos[0] + c[1] as f32 * (s.cell_size[0] + s.cell_spacing);
            let y = s.pos[1] + c[0] as f32 * (s.cell_size[1] + s.cell_spacing);
            let spr = if self.done[side] {
                s.cursor_done[side]
            } else {
                s.cursor_active[side]
            };
            if let Some((g, n)) = spr
                && let Some(p) = sys.sheet.get(r, g, n)
            {
                r.draw(Draw::new(p.tex, p.pal, [x + ox, y], p.axis));
            }
        }
        // big faces + names
        for side in 0..2 {
            let idx = match (self.random_pick[side], self.human[side]) {
                (Some(i), _) => Some(i),
                (None, true) => self.cell_index(sys, self.cursor[side]),
                (None, false) => None,
            };
            let Some(i) = idx else { continue };
            let cell = &mut self.cells[i];
            if cell.big_tex.is_none() {
                cell.big_tex = Self::upload(r, &cell.big);
            }
            if let Some(tex) = cell.big_tex
                && let Some((_, _, _, axis)) = cell.big
            {
                let mut d = Draw::new(
                    tex,
                    None,
                    [s.face_offset[side][0] + ox, s.face_offset[side][1]],
                    [axis[0] as f32, axis[1] as f32],
                );
                d.scale = s.face_scale[side];
                d.hflip = s.face_facing[side] < 0.0;
                r.draw(d);
            }
            let name = cell.manifest.display_name.clone();
            sys.text(r, s.name_font[side], &name, s.name_offset[side]);
        }
        // stage line
        if self.phase != Phase::Chars || (self.done[0] && self.done[1]) {
            let name = if self.stage_idx == 0 {
                "Random".to_string()
            } else {
                self.stages
                    .get(self.stage_idx - 1)
                    .map(|e| e.id.clone())
                    .unwrap_or_default()
            };
            let font = if self.phase == Phase::Done {
                s.stage_done_font
            } else if (self.tick / 4) % 2 == 0 {
                s.stage_active_font
            } else {
                s.stage_active2_font.or(s.stage_active_font)
            };
            sys.text(r, font, &format!("Stage: {name}"), s.stage_pos);
        }
        if let Some(b) = self.bg.as_mut() {
            b.draw(r, 1, [0.0, 0.0], 1.0, sw, Default::default());
        }
        (
            Camera {
                screen: [sw, lc[1]],
                center: [0.0, lc[1] / 2.0],
                zoom: (sw / lc[0]).min(1.0),
            },
            clear,
        )
    }
}
