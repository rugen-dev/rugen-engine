//! fight.def HUD: life/power bars, faces, names, clock, round/fight/KO
//! announcements and win icons, drawn from fight.sff with the fight fonts.
//!
//! Coordinates are fight.def's 320x240 space, centred horizontally when the
//! window is wider; callers map them through a `hud` closure.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use rugen_formats::air::Air;
use rugen_formats::def::{Def, Section};
use rugen_formats::fnt::Font;
use rugen_formats::sff::Sff;
use rugen_render::{Draw, PaletteId, Renderer, TextureId};
use rugen_sim::World;
use rugen_sim::round::Finish;
use rugen_sim::world::Event;

use crate::sprites::{Anim, FontGpu, FontRef, SpriteSheet};

/// Uploaded player portrait: texture, palette, axis.
pub type Portrait = (TextureId, Option<PaletteId>, [f32; 2]);

/// One drawable HUD piece: a sprite or an animation, with offset/facing/scale.
#[derive(Clone, Debug, Default)]
struct Elem {
    spr: Option<(i32, i32)>,
    anim: Option<Anim>,
    offset: [f32; 2],
    facing: f32,
    scale: [f32; 2],
}

impl Elem {
    fn parse(s: &Section, prefix: &str) -> Self {
        let k = |n: &str| format!("{prefix}.{n}");
        let spr = s
            .i32s(&k("spr"))
            .filter(|v| v.len() >= 2)
            .map(|v| (v[0], v[1]));
        let anim = s.i32(&k("anim")).map(Anim::new);
        let offset = s
            .f32s(&k("offset"))
            .filter(|v| v.len() >= 2)
            .map_or([0.0, 0.0], |v| [v[0], v[1]]);
        let facing = s.f32(&k("facing")).unwrap_or(1.0);
        let scale = s
            .f32s(&k("scale"))
            .filter(|v| v.len() >= 2)
            .map_or([1.0, 1.0], |v| [v[0], v[1]]);
        Self {
            spr,
            anim,
            offset,
            facing,
            scale,
        }
    }
    fn present(&self) -> bool {
        self.spr.is_some() || self.anim.is_some()
    }
}

#[derive(Clone, Debug, Default)]
struct Bar {
    pos: [f32; 2],
    bg0: Elem,
    bg1: Elem,
    mid: Elem,
    front: Elem,
    /// `range.x = start, end`: pixels covered at full value (negative = leftward).
    range: [f32; 2],
}

impl Bar {
    fn parse(s: &Section, p: &str) -> Self {
        let pos = s
            .f32s(&format!("{p}.pos"))
            .filter(|v| v.len() >= 2)
            .map_or([0.0, 0.0], |v| [v[0], v[1]]);
        Self {
            pos,
            bg0: Elem::parse(s, &format!("{p}.bg0")),
            bg1: Elem::parse(s, &format!("{p}.bg1")),
            mid: Elem::parse(s, &format!("{p}.mid")),
            front: Elem::parse(s, &format!("{p}.front")),
            range: s
                .f32s(&format!("{p}.range.x"))
                .filter(|v| v.len() >= 2)
                .map_or([0.0, 0.0], |v| [v[0], v[1]]),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Face {
    pos: [f32; 2],
    bg: Elem,
    face: Elem,
}

#[derive(Clone, Debug)]
struct Text {
    offset: [f32; 2],
    font: Option<FontRef>,
    text: String,
    displaytime: i32,
}

impl Text {
    fn parse(s: &Section, p: &str, default: &str, time: i32) -> Self {
        Self {
            offset: s
                .f32s(&format!("{p}.offset"))
                .filter(|v| v.len() >= 2)
                .map_or([160.0, 70.0], |v| [v[0], v[1]]),
            font: FontRef::parse(s, &format!("{p}.font")),
            text: s
                .raw(&format!("{p}.text"))
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| default.to_string()),
            displaytime: s.i32(&format!("{p}.displaytime")).unwrap_or(time),
        }
    }
}

/// A transient announcement being shown.
#[derive(Clone, Debug)]
enum Shown {
    Text { text: Text, line: String },
    Anim { elem: Elem, pos: [f32; 2] },
}

pub struct Lifebar {
    sheet: SpriteSheet,
    air: Air,
    fonts: HashMap<i32, FontGpu>,
    pub localcoord: [f32; 2],
    life: [Bar; 2],
    power: [Bar; 2],
    power_counter: [(Option<FontRef>, [f32; 2]); 2],
    faces: [Face; 2],
    names: [([f32; 2], Option<FontRef>); 2],
    time_pos: [f32; 2],
    time_counter: ([f32; 2], Option<FontRef>),
    round_text: Text,
    round_offset_time: i32,
    fight: Elem,
    fight_offset: [f32; 2],
    ko: Elem,
    ko_offset: [f32; 2],
    dko: Text,
    to: Text,
    win: Text,
    draw: Text,
    winicon_pos: [[f32; 2]; 2],
    winicon_offset: [[f32; 2]; 2],
    winicon: [HashMap<String, Elem>; 2],
    shown: Vec<(Shown, i32)>,
    tick: i32,
}

impl Lifebar {
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let fight_path = rugen_sim::chardata::resolve(data_dir, "fight.def")?;
        let def = Def::open(&fight_path).with_context(|| fight_path.display().to_string())?;
        let text = rugen_formats::text::decode(&std::fs::read(&fight_path)?);
        let air = Air::parse(&text);
        let files = def.section("files").context("fight.def has no [Files]")?;
        let spr = files.raw("sff").unwrap_or("fight.sff");
        let sff = Sff::open(rugen_sim::chardata::resolve(data_dir, spr)?, false)
            .with_context(|| spr.to_string())?;
        let mut fonts = HashMap::new();
        let root = data_dir.parent().unwrap_or(data_dir);
        for i in 1..=10 {
            let Some(name) = files.raw(&format!("font{i}")) else {
                continue;
            };
            let name = name.trim();
            let candidates = [
                data_dir.join(name),
                root.join("font").join(name),
                root.join(name),
            ];
            let Some(path) = candidates.iter().find(|p| p.exists()) else {
                log::warn!("fight.def: font{i} = {name} not found");
                continue;
            };
            match Font::open(path) {
                Ok(f) => {
                    fonts.insert(i, FontGpu::new(f));
                }
                Err(e) => log::warn!("font {name}: {e}"),
            }
        }
        let localcoord = def
            .section("info")
            .and_then(|s| s.f32s("localcoord"))
            .filter(|v| v.len() >= 2)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let empty = Section {
            name: String::new(),
            rest: String::new(),
            line: 0,
            entries: Default::default(),
        };
        let sec = |n: &str| def.section(n).unwrap_or(&empty);
        let lb = sec("lifebar");
        let pb = sec("powerbar");
        let fc = sec("face");
        let nm = sec("name");
        let tm = sec("time");
        let rd = sec("round");
        let wi = sec("winicon");
        let pos2 = |s: &Section, k: &str, d: [f32; 2]| {
            s.f32s(k)
                .filter(|v| v.len() >= 2)
                .map_or(d, |v| [v[0], v[1]])
        };
        let face = |p: &str| Face {
            pos: pos2(fc, &format!("{p}.pos"), [0.0, 0.0]),
            bg: Elem::parse(fc, &format!("{p}.bg")),
            face: Elem::parse(fc, &format!("{p}.face")),
        };
        let icon_kinds = [
            "n", "s", "h", "throw", "c", "t", "suicide", "teammate", "perfect",
        ];
        let icons = |p: &str| -> HashMap<String, Elem> {
            icon_kinds
                .iter()
                .map(|k| (k.to_string(), Elem::parse(wi, &format!("{p}.{k}"))))
                .collect()
        };
        Ok(Self {
            sheet: SpriteSheet::new(sff),
            air,
            fonts,
            localcoord,
            life: [Bar::parse(lb, "p1"), Bar::parse(lb, "p2")],
            power: [Bar::parse(pb, "p1"), Bar::parse(pb, "p2")],
            power_counter: [
                (
                    FontRef::parse(pb, "p1.counter.font"),
                    pos2(pb, "p1.counter.offset", [0.0, 0.0]),
                ),
                (
                    FontRef::parse(pb, "p2.counter.font"),
                    pos2(pb, "p2.counter.offset", [0.0, 0.0]),
                ),
            ],
            faces: [face("p1"), face("p2")],
            names: [
                (
                    pos2(nm, "p1.pos", [14.0, 10.0]),
                    FontRef::parse(nm, "p1.name.font"),
                ),
                (
                    pos2(nm, "p2.pos", [305.0, 10.0]),
                    FontRef::parse(nm, "p2.name.font"),
                ),
            ],
            time_pos: pos2(tm, "pos", [160.0, 23.0]),
            time_counter: (
                pos2(tm, "counter.offset", [0.0, 0.0]),
                FontRef::parse(tm, "counter.font"),
            ),
            round_text: Text::parse(rd, "round.default", "Round %i", 60),
            round_offset_time: rd.i32("round.time").unwrap_or(0),
            fight: Elem::parse(rd, "fight"),
            fight_offset: pos2(rd, "fight.offset", [160.0, 110.0]),
            ko: Elem::parse(rd, "ko"),
            ko_offset: pos2(rd, "ko.offset", [160.0, 70.0]),
            dko: Text::parse(rd, "dko", "Double K.O.", 60),
            to: Text::parse(rd, "to", "Time Over", 60),
            win: Text::parse(rd, "win", "%s Wins", 540),
            draw: Text::parse(rd, "draw", "Draw Game", 540),
            winicon_pos: [
                pos2(wi, "p1.pos", [34.0, 43.0]),
                pos2(wi, "p2.pos", [284.0, 43.0]),
            ],
            winicon_offset: [
                pos2(wi, "p1.iconoffset", [12.0, 0.0]),
                pos2(wi, "p2.iconoffset", [-12.0, 0.0]),
            ],
            winicon: [icons("p1"), icons("p2")],
            shown: Vec::new(),
            tick: 0,
        })
    }

    /// React to round events: queue announcements.
    pub fn event(&mut self, w: &World, e: &Event) {
        match e {
            Event::RoundStart(n) => {
                let t = self.round_text.clone();
                let line = t.text.replace("%i", &n.to_string());
                let time = t.displaytime;
                self.shown.push((Shown::Text { text: t, line }, time));
            }
            Event::Fight => {
                if self.fight.present() {
                    let mut e = self.fight.clone();
                    if let Some(a) = e.anim.as_mut() {
                        *a = Anim::new(a.action);
                    }
                    let pos = self.fight_offset;
                    self.shown.push((Shown::Anim { elem: e, pos }, 60));
                }
            }
            Event::KO => {
                if self.ko.present() {
                    let mut e = self.ko.clone();
                    if let Some(a) = e.anim.as_mut() {
                        *a = Anim::new(a.action);
                    }
                    let pos = self.ko_offset;
                    self.shown.push((Shown::Anim { elem: e, pos }, 120));
                }
            }
            Event::TimeOver => {
                let t = self.to.clone();
                let line = t.text.clone();
                let time = t.displaytime;
                self.shown.push((Shown::Text { text: t, line }, time));
            }
            Event::RoundWon(team) => {
                let name = w
                    .players
                    .iter()
                    .find(|p| p.helper_index == 0 && p.team == *team)
                    .map(|p| w.data[p.data].display_name.clone())
                    .unwrap_or_default();
                let (t, line) = if w.round.finish == Finish::DoubleKO {
                    (self.dko.clone(), self.dko.text.clone())
                } else {
                    (self.win.clone(), self.win.text.replace("%s", &name))
                };
                let time = t.displaytime;
                self.shown.push((Shown::Text { text: t, line }, time));
            }
            Event::MatchOver(team) if *team < 0 => {
                let t = self.draw.clone();
                let line = t.text.clone();
                let time = t.displaytime;
                self.shown.push((Shown::Text { text: t, line }, time));
            }
            _ => {}
        }
    }

    pub fn step(&mut self) {
        self.tick += 1;
        for bar in self.life.iter_mut().chain(self.power.iter_mut()) {
            for e in [&mut bar.bg0, &mut bar.bg1, &mut bar.mid, &mut bar.front] {
                if let Some(a) = e.anim.as_mut() {
                    a.step(&self.air);
                }
            }
        }
        for (s, t) in &mut self.shown {
            *t -= 1;
            if let Shown::Anim { elem, .. } = s
                && let Some(a) = elem.anim.as_mut()
            {
                a.step(&self.air);
            }
        }
        self.shown.retain(|(_, t)| *t > 0);
    }

    fn draw_elem(
        &mut self,
        r: &mut Renderer,
        e: &Elem,
        base: [f32; 2],
        scale: f32,
        clip: Option<f32>,
    ) {
        let pos = [base[0] + e.offset[0] * scale, base[1] + e.offset[1] * scale];
        let sc = [e.scale[0] * scale, e.scale[1] * scale];
        if let Some(a) = &e.anim {
            if let Some(f) = a.frame(&self.air).cloned() {
                self.sheet.draw_frame(r, &f, pos, sc, e.facing);
            }
            return;
        }
        let Some((g, n)) = e.spr else { return };
        let Some(p) = self.sheet.get(r, g, n) else {
            return;
        };
        let mut d = Draw::new(p.tex, p.pal, pos, p.axis);
        d.scale = sc;
        d.hflip = e.facing < 0.0;
        if let Some(frac) = clip {
            // keep only the part of the sprite that lies within `frac` of its width,
            // measured from the axis in the bar's growth direction
            let w = p.size[0];
            let keep = libm::roundf(w * frac).max(0.0) as u32;
            if keep == 0 {
                return;
            }
            d.src = Some([0, 0, keep.min(w as u32), p.size[1] as u32]);
        }
        r.draw(d);
    }

    fn draw_bar(
        &mut self,
        r: &mut Renderer,
        bar: &Bar,
        frac: f32,
        mid_frac: f32,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
    ) {
        let base = hud(bar.pos[0], bar.pos[1]);
        self.draw_elem(r, &bar.bg0, base, u, None);
        self.draw_elem(r, &bar.bg1, base, u, None);
        let span = (bar.range[1] - bar.range[0]).abs();
        if span > 0.0 {
            // the sprite spans the range; crop it to the fraction
            self.draw_elem(r, &bar.mid, base, u, Some(mid_frac.clamp(0.0, 1.0)));
            self.draw_elem(r, &bar.front, base, u, Some(frac.clamp(0.0, 1.0)));
        } else {
            self.draw_elem(r, &bar.mid, base, u, None);
            self.draw_elem(r, &bar.front, base, u, None);
        }
    }

    fn text(&mut self, r: &mut Renderer, f: Option<FontRef>, line: &str, pos: [f32; 2], u: f32) {
        let Some(f) = f else { return };
        if let Some(font) = self.fonts.get_mut(&f.index) {
            font.draw(r, line, pos, f.bank, f.align, u);
        }
    }

    /// Draw everything. `hud(x, y)` maps fight.def coordinates to world
    /// space, `u` is world units per HUD pixel. `portraits[i]` is player i's
    /// 9000,0 sprite if the app uploaded it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        r: &mut Renderer,
        w: &World,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        portraits: [Option<Portrait>; 2],
    ) {
        let roots: Vec<usize> = w
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0)
            .map(|(i, _)| i)
            .take(2)
            .collect();
        for (side, &pi) in roots.iter().enumerate() {
            let p = &w.players[pi];
            let life = p.life.max(0) as f32 / p.life_max.max(1) as f32;
            let bar = self.life[side].clone();
            self.draw_bar(r, &bar, life, life, hud, u);
            let power = p.power.max(0) as f32 / p.power_max.max(1) as f32;
            let bar = self.power[side].clone();
            self.draw_bar(r, &bar, power, power, hud, u);
            let (font, off) = self.power_counter[side];
            let base = hud(
                self.power[side].pos[0] + off[0],
                self.power[side].pos[1] + off[1],
            );
            let level = (p.power / 1000).max(0);
            self.text(r, font, &level.to_string(), base, u);
            // face
            let face = self.faces[side].clone();
            let base = hud(face.pos[0], face.pos[1]);
            self.draw_elem(r, &face.bg, base, u, None);
            if let Some((tex, pal, axis)) = portraits[side] {
                let pos = [
                    base[0] + face.face.offset[0] * u,
                    base[1] + face.face.offset[1] * u,
                ];
                let mut d = Draw::new(tex, pal, pos, axis);
                d.scale = [face.face.scale[0] * u, face.face.scale[1] * u];
                d.hflip = face.face.facing < 0.0;
                r.draw(d);
            }
            // name
            let (pos, font) = self.names[side];
            let name = w.data[p.data].display_name.clone();
            self.text(r, font, &name, hud(pos[0], pos[1]), u);
            // win icons
            let wins = w.round.wins[p.team as usize] as usize;
            let kinds: Vec<String> = w
                .round
                .win_kinds
                .get(p.team as usize)
                .cloned()
                .unwrap_or_default();
            for k in 0..wins {
                let kind = kinds.get(k).cloned().unwrap_or_else(|| "n".into());
                let Some(e) = self.winicon[side].get(&kind).cloned() else {
                    continue;
                };
                let pos = [
                    self.winicon_pos[side][0] + self.winicon_offset[side][0] * k as f32,
                    self.winicon_pos[side][1] + self.winicon_offset[side][1] * k as f32,
                ];
                let base = hud(pos[0], pos[1]);
                self.draw_elem(r, &e, base, u, None);
            }
        }
        // clock
        let clock = w.round.clock();
        let (off, font) = self.time_counter;
        let base = hud(self.time_pos[0] + off[0], self.time_pos[1] + off[1]);
        let line = if clock < 0 {
            "o".to_string()
        } else {
            format!("{clock:02}")
        };
        self.text(r, font, &line, base, u);
        // announcements
        let shown = self.shown.clone();
        for (s, _) in &shown {
            match s {
                Shown::Text { text, line } => {
                    let base = hud(text.offset[0], text.offset[1]);
                    self.text(r, text.font, line, base, u);
                }
                Shown::Anim { elem, pos } => {
                    let base = hud(pos[0], pos[1]);
                    self.draw_elem(r, elem, base, u, None);
                }
            }
        }
        let _ = self.round_offset_time;
    }
}
