//! fight.def HUD: life/power bars, faces, names, clock, round/fight/KO
//! announcements and win icons, drawn from fight.sff with the fight fonts.
//!
//! Coordinates are fight.def's `[Info] localcoord` space (320x240 when
//! absent), centred horizontally when the window is wider; callers map them
//! through a `hud` closure.
//!
//! The life bar's `mid` element is the damage trail: it holds where the life
//! was for `pN.mid.delay` ticks after a hit, then drains down to `front`.
//!
//! rugen extensions (all optional; plain MUGEN screenpacks ignore them):
//!
//! - any element (`spr`/`anim` prefix): `.skew = px` shears the sprite (top
//!   edge shifted right), `.angle = deg` rotates it counter-clockwise.
//! - any text font (`<prefix>.font`): `<prefix>.scale = s` multiplies its size,
//!   e.g. `p1.name.scale = 2`, `counter.scale = 5`, `team1.counter.scale = 3`.
//! - `[Lifebar]`
//!   - `pN.fg0.*`: element drawn over `front` (tick marks, gloss).
//!   - `pN.mid.delay = 30`: ticks the damage trail waits before draining.
//!   - `pN.flash.low = 25`: below this life percentage the bar flashes;
//!     `pN.flash.spr` / `pN.flash.anim` replaces `front` on the lit phase
//!     (without it `front` fades), `pN.flash.period = 16` ticks per cycle.
//! - `[Powerbar]`
//!   - `pN.segments = 3`: draw the bar as N segments instead of `mid`/`front`:
//!     `pN.segment.bg.*` behind each, `pN.segment.full.*` for a charged one,
//!     `pN.segment.part.*` cropped to the charge of the filling one,
//!     `pN.segment.step = x,y` between segments.
//!   - `pN.stamp.*` drawn behind the level counter, `pN.stamp.lit.*` instead
//!     when the level is 1 or more; `pN.counter.lit.font` likewise.
//! - `[Face]` `pN.face.fit = w,h`: scale and crop the portrait to cover a
//!   w x h box centred on `pN.pos + pN.face.offset` (with `pN.face.angle`).
//! - `[Name]` `pN.bg.*`: plate behind the name, stretched to the text:
//!   `pN.bg.slice = left,right` caps kept unstretched, `pN.bg.pad = px` of
//!   plate beyond the text on each side, `pN.bg.offset` y = plate top;
//!   `pN.name.upper = 1` shows the name in capitals.
//! - `[WinIcon]` `pN.follow = 1`: `pN.pos` x is measured from the name
//!   plate's outer end (P1 right end, P2 left end) instead of the screen.
//! - `[Time]` round tag: `round.text = ROUND %i`, `round.offset`,
//!   `round.font`, `round.scale`, `round.bg.*` (all relative to `pos`).
//! - `[Combo]` `teamN.angle = deg`: tilt counter and label counter-clockwise
//!   around `teamN.pos`.

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
    /// rugen: horizontal shear (top edge shifted right), HUD pixels.
    skew: f32,
    /// rugen: rotation, degrees counter-clockwise.
    angle: f32,
    /// Transparency, 0 = opaque (low-life flash without a flash sprite).
    fade: f32,
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
            skew: s.f32(&k("skew")).unwrap_or(0.0),
            angle: s.f32(&k("angle")).unwrap_or(0.0),
            fade: 0.0,
        }
    }
    fn present(&self) -> bool {
        self.spr.is_some() || self.anim.is_some()
    }
}

/// A font reference plus the rugen `<prefix>.scale` size multiplier.
#[derive(Clone, Copy, Debug)]
struct Fnt {
    f: FontRef,
    scale: f32,
}

impl Fnt {
    /// `<prefix>.font` and `<prefix>.scale`.
    fn parse(s: &Section, prefix: &str) -> Option<Self> {
        let f = FontRef::parse(s, &format!("{prefix}.font"))?;
        let scale = s
            .f32s(&format!("{prefix}.scale"))
            .and_then(|v| v.first().copied())
            .filter(|v| *v > 0.0)
            .unwrap_or(1.0);
        Some(Self { f, scale })
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
    fg0: Elem,
    /// Ticks the damage trail (`mid`) holds before draining.
    mid_delay: i32,
    /// Flash below this life percentage (0 = never).
    flash_low: f32,
    flash: Elem,
    flash_period: i32,
    segments: i32,
    seg_bg: Elem,
    seg_full: Elem,
    seg_part: Elem,
    seg_step: [f32; 2],
    stamp: Elem,
    stamp_lit: Elem,
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
            fg0: Elem::parse(s, &format!("{p}.fg0")),
            mid_delay: s.i32(&format!("{p}.mid.delay")).unwrap_or(30).max(0),
            flash_low: s.f32(&format!("{p}.flash.low")).unwrap_or(0.0),
            flash: Elem::parse(s, &format!("{p}.flash")),
            flash_period: s.i32(&format!("{p}.flash.period")).unwrap_or(16).max(2),
            segments: s.i32(&format!("{p}.segments")).unwrap_or(0).max(0),
            seg_bg: Elem::parse(s, &format!("{p}.segment.bg")),
            seg_full: Elem::parse(s, &format!("{p}.segment.full")),
            seg_part: Elem::parse(s, &format!("{p}.segment.part")),
            seg_step: s
                .f32s(&format!("{p}.segment.step"))
                .filter(|v| v.len() >= 2)
                .map_or([0.0, 0.0], |v| [v[0], v[1]]),
            stamp: Elem::parse(s, &format!("{p}.stamp")),
            stamp_lit: Elem::parse(s, &format!("{p}.stamp.lit")),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Face {
    pos: [f32; 2],
    bg: Elem,
    face: Elem,
    /// rugen: cover-fit the portrait into this box.
    fit: Option<[f32; 2]>,
}

/// `[Name]` per player: position, font and the optional stretched plate.
#[derive(Clone, Debug)]
struct NameTag {
    pos: [f32; 2],
    font: Option<Fnt>,
    bg: Elem,
    slice: [f32; 2],
    pad: f32,
    /// rugen `pN.name.upper = 1`: show the name in capitals.
    upper: bool,
}

/// `[Time] round.*`: "ROUND n" tag under the clock.
#[derive(Clone, Debug)]
struct RoundTag {
    offset: [f32; 2],
    font: Option<Fnt>,
    text: String,
    bg: Elem,
}

#[derive(Clone, Debug)]
struct Text {
    offset: [f32; 2],
    font: Option<Fnt>,
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
            font: Fnt::parse(s, p),
            text: s
                .raw(&format!("{p}.text"))
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| default.to_string()),
            displaytime: s.i32(&format!("{p}.displaytime")).unwrap_or(time),
        }
    }
}

/// `[Combo]` per team: counter font, label text and where to show them.
#[derive(Clone, Debug)]
struct Combo {
    pos: [f32; 2],
    counter_font: Option<Fnt>,
    text: String,
    text_font: Option<Fnt>,
    text_offset: [f32; 2],
    displaytime: i32,
    angle: f32,
}

impl Combo {
    fn parse(s: &Section, p: &str) -> Self {
        let pos2 = |k: &str, d: [f32; 2]| {
            s.f32s(k)
                .filter(|v| v.len() >= 2)
                .map_or(d, |v| [v[0], v[1]])
        };
        Self {
            pos: pos2(&format!("{p}.pos"), [10.0, 80.0]),
            counter_font: Fnt::parse(s, &format!("{p}.counter")),
            text: s
                .raw(&format!("{p}.text.text"))
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| "Hit".into()),
            text_font: Fnt::parse(s, &format!("{p}.text")),
            text_offset: pos2(&format!("{p}.text.offset"), [3.0, 0.0]),
            displaytime: s.i32(&format!("{p}.displaytime")).unwrap_or(90),
            angle: s.f32(&format!("{p}.angle")).unwrap_or(0.0),
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
    power_counter: [(Option<Fnt>, Option<Fnt>, [f32; 2]); 2],
    faces: [Face; 2],
    names: [NameTag; 2],
    time_pos: [f32; 2],
    time_counter: ([f32; 2], Option<Fnt>),
    round_tag: RoundTag,
    /// `[WinIcon] pN.follow`: icons start at the name plate's outer end.
    winicon_follow: [bool; 2],
    /// Damage trail per side: shown fraction, hold ticks left, current life.
    trail: [(f32, i32, f32); 2],
    trail_seen: [bool; 2],
    /// rugen / Ikemen: `[Time] bg.spr` plate behind the clock.
    time_bg: Elem,
    /// rugen: `pN.empty.spr` placeholder for rounds not yet won.
    winicon_empty: [Elem; 2],
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
    /// Text versions of FIGHT / KO for screenpacks without animations.
    fight_text: Text,
    ko_text: Text,
    combo: [Combo; 2],
    /// Per team: hits shown and ticks left to keep showing them.
    combo_state: [(i32, i32); 2],
}

impl Lifebar {
    /// `motif` holds the screenpack (fight.def, fight.sff, fonts); anything
    /// missing there is looked up in `data_dir`.
    pub fn load(motif: &Path, data_dir: &Path) -> anyhow::Result<Self> {
        let chain = rugen_sim::chardata::motif_chain(motif, data_dir);
        let dirs: Vec<&Path> = chain.iter().map(std::path::PathBuf::as_path).collect();
        let fight_path = rugen_sim::chardata::resolve_in(&dirs, "fight.def")?;
        let def = Def::open(&fight_path).with_context(|| fight_path.display().to_string())?;
        let text = rugen_formats::text::decode(&std::fs::read(&fight_path)?);
        let air = Air::parse(&text);
        let files = def.section("files").context("fight.def has no [Files]")?;
        let spr = files.raw("sff").unwrap_or("fight.sff");
        let sff = Sff::open(rugen_sim::chardata::resolve_in(&dirs, spr)?, false)
            .with_context(|| spr.to_string())?;
        let mut fonts = HashMap::new();
        let root = data_dir.parent().unwrap_or(data_dir);
        for i in 1..=10 {
            let Some(name) = files.raw(&format!("font{i}")) else {
                continue;
            };
            let name = name.trim();
            let mut candidates: Vec<std::path::PathBuf> = Vec::new();
            for d in &chain {
                candidates.push(d.join(name));
                candidates.push(d.join("font").join(name));
            }
            candidates.push(root.join("font").join(name));
            candidates.push(root.join(name));
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
            fit: fc
                .f32s(&format!("{p}.face.fit"))
                .filter(|v| v.len() >= 2)
                .map(|v| [v[0], v[1]]),
        };
        let name = |p: &str, d: [f32; 2]| NameTag {
            pos: pos2(nm, &format!("{p}.pos"), d),
            font: Fnt::parse(nm, &format!("{p}.name")),
            bg: Elem::parse(nm, &format!("{p}.bg")),
            slice: pos2(nm, &format!("{p}.bg.slice"), [0.0, 0.0]),
            pad: nm.f32(&format!("{p}.bg.pad")).unwrap_or(0.0),
            upper: nm.i32(&format!("{p}.name.upper")).unwrap_or(0) != 0,
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
                    Fnt::parse(pb, "p1.counter"),
                    Fnt::parse(pb, "p1.counter.lit"),
                    pos2(pb, "p1.counter.offset", [0.0, 0.0]),
                ),
                (
                    Fnt::parse(pb, "p2.counter"),
                    Fnt::parse(pb, "p2.counter.lit"),
                    pos2(pb, "p2.counter.offset", [0.0, 0.0]),
                ),
            ],
            faces: [face("p1"), face("p2")],
            names: [name("p1", [14.0, 10.0]), name("p2", [305.0, 10.0])],
            round_tag: RoundTag {
                offset: pos2(tm, "round.offset", [0.0, 0.0]),
                font: Fnt::parse(tm, "round"),
                text: tm
                    .raw("round.text")
                    .map(|t| t.trim().to_string())
                    .unwrap_or_else(|| "Round %i".into()),
                bg: Elem::parse(tm, "round.bg"),
            },
            winicon_follow: [
                wi.i32("p1.follow").unwrap_or(0) != 0,
                wi.i32("p2.follow").unwrap_or(0) != 0,
            ],
            trail: [(1.0, 0, 1.0); 2],
            trail_seen: [false; 2],
            time_pos: pos2(tm, "pos", [160.0, 23.0]),
            time_bg: Elem::parse(tm, "bg"),
            winicon_empty: [Elem::parse(wi, "p1.empty"), Elem::parse(wi, "p2.empty")],
            time_counter: (
                pos2(tm, "counter.offset", [0.0, 0.0]),
                Fnt::parse(tm, "counter"),
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
            fight_text: Text::parse(rd, "fight", "FIGHT!", 45),
            ko_text: Text::parse(rd, "ko", "K.O.", 90),
            combo: [
                Combo::parse(sec("combo"), "team1"),
                Combo::parse(sec("combo"), "team2"),
            ],
            combo_state: [(0, 0); 2],
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
                } else if self.fight_text.font.is_some() {
                    let t = self.fight_text.clone();
                    let line = t.text.clone();
                    let time = t.displaytime;
                    self.shown.push((Shown::Text { text: t, line }, time));
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
                } else if self.ko_text.font.is_some() {
                    let t = self.ko_text.clone();
                    let line = t.text.clone();
                    let time = t.displaytime;
                    self.shown.push((Shown::Text { text: t, line }, time));
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
        for (trail, hold, life) in &mut self.trail {
            if *life >= *trail {
                *trail = *life;
                *hold = 0;
            } else if *hold > 0 {
                *hold -= 1;
            } else {
                // ease out: fast for big chunks, never slower than ~4 s for a full bar
                let step = ((*trail - *life) * 0.08).max(0.004);
                *trail = (*trail - step).max(*life);
            }
        }
        for bar in self.life.iter_mut().chain(self.power.iter_mut()) {
            for e in [
                &mut bar.bg0,
                &mut bar.bg1,
                &mut bar.mid,
                &mut bar.front,
                &mut bar.fg0,
                &mut bar.flash,
            ] {
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
        for (_, timer) in &mut self.combo_state {
            if *timer > 0 {
                *timer -= 1;
            }
        }
    }

    /// Draw `line` with fight font `index` at a world position (used by the
    /// tutorial and other engine prompts). Returns false when the font is missing.
    #[allow(clippy::too_many_arguments)]
    pub fn text_at(
        &mut self,
        r: &mut Renderer,
        index: i32,
        line: &str,
        pos: [f32; 2],
        bank: i32,
        align: i32,
        scale: f32,
    ) -> bool {
        match self.fonts.get_mut(&index) {
            Some(font) => {
                font.draw(r, line, pos, bank, align, scale);
                true
            }
            None => false,
        }
    }

    /// Unscaled width and height of `line` in fight.def font `index`.
    pub fn text_size(&self, index: i32, line: &str) -> Option<[f32; 2]> {
        let f = self.fonts.get(&index)?;
        Some([f.width(line, 1.0), f32::from(f.font.size[1])])
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
        d.skew = e.skew * scale;
        d.angle = e.angle;
        d.color[3] = 1.0 - e.fade.clamp(0.0, 1.0);
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

    /// Draw a bar's layers. `frac` is the value, `mid_frac` the damage trail,
    /// `lit` the flash phase of a low bar.
    #[allow(clippy::too_many_arguments)]
    fn draw_bar(
        &mut self,
        r: &mut Renderer,
        bar: &Bar,
        frac: f32,
        mid_frac: f32,
        lit: bool,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
    ) {
        let base = hud(bar.pos[0], bar.pos[1]);
        self.draw_elem(r, &bar.bg0, base, u, None);
        self.draw_elem(r, &bar.bg1, base, u, None);
        let front = if lit && bar.flash.present() {
            bar.flash.clone()
        } else if lit {
            Elem {
                fade: 0.6,
                ..bar.front.clone()
            }
        } else {
            bar.front.clone()
        };
        let span = (bar.range[1] - bar.range[0]).abs();
        if span > 0.0 {
            // the sprite spans the range; crop it to the fraction
            self.draw_elem(r, &bar.mid, base, u, Some(mid_frac.clamp(0.0, 1.0)));
            self.draw_elem(r, &front, base, u, Some(frac.clamp(0.0, 1.0)));
        } else {
            self.draw_elem(r, &bar.mid, base, u, None);
            self.draw_elem(r, &front, base, u, None);
        }
        self.draw_elem(r, &bar.fg0, base, u, None);
    }

    /// rugen `pN.segments`: one cell per power level, the filling one cropped.
    fn draw_segments(
        &mut self,
        r: &mut Renderer,
        bar: &Bar,
        levels: f32,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
    ) {
        for k in 0..bar.segments {
            let base = hud(
                bar.pos[0] + bar.seg_step[0] * k as f32,
                bar.pos[1] + bar.seg_step[1] * k as f32,
            );
            self.draw_elem(r, &bar.seg_bg, base, u, None);
            let v = (levels - k as f32).clamp(0.0, 1.0);
            if v >= 1.0 {
                self.draw_elem(r, &bar.seg_full, base, u, None);
            } else if v > 0.0 {
                self.draw_elem(r, &bar.seg_part, base, u, Some(v));
            }
        }
    }

    fn text(&mut self, r: &mut Renderer, f: Option<Fnt>, line: &str, pos: [f32; 2], u: f32) {
        let Some(Fnt { f, scale }) = f else { return };
        if let Some(font) = self.fonts.get_mut(&f.index) {
            font.draw(r, line, pos, f.bank, f.align, u * scale);
        }
    }

    /// Width of `line` in HUD pixels (0 when the font is missing).
    fn text_width(&self, f: Option<Fnt>, line: &str) -> f32 {
        f.and_then(|Fnt { f, scale }| self.fonts.get(&f.index).map(|g| g.width(line, scale)))
            .unwrap_or(0.0)
    }

    /// Draw a sprite stretched horizontally over `[x0, x1]` (HUD pixels) with
    /// its `slice` caps kept at their size. `y` is the sprite's top.
    #[allow(clippy::too_many_arguments)]
    fn draw_sliced(
        &mut self,
        r: &mut Renderer,
        e: &Elem,
        slice: [f32; 2],
        x0: f32,
        x1: f32,
        y: f32,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
    ) {
        let Some((g, n)) = e.spr else { return };
        let Some(p) = self.sheet.get(r, g, n) else {
            return;
        };
        let (w, h) = (p.size[0], p.size[1]);
        let l = slice[0].clamp(0.0, w);
        let rc = slice[1].clamp(0.0, w - l);
        let mid_src = (w - l - rc).max(1.0);
        let width = (x1 - x0).max(l + rc);
        let mid_w = width - l - rc;
        let parts = [
            (0.0, l, x0, 1.0),
            (l, mid_src, x0 + l, mid_w / mid_src),
            (w - rc, rc, x0 + width - rc, 1.0),
        ];
        for (sx, sw, dx, sc) in parts {
            if sw <= 0.0 || sc <= 0.0 {
                continue;
            }
            let mut d = Draw::new(p.tex, p.pal, hud(dx, y), [0.0, 0.0]);
            d.scale = [sc * u, u];
            d.src = Some([sx as u32, 0, sw as u32, h as u32]);
            r.draw(d);
        }
    }

    /// Draw everything. `hud(x, y)` maps fight.def coordinates to world
    /// space, `u` is world units per HUD pixel. `portraits[i]` is player i's
    /// 9000,0 sprite if the app uploaded it. `spread` (fight.def units) moves
    /// P1's elements left and P2's right, onto the edges of a window wider
    /// than the fight.def; the clock and announcements stay centred. Only
    /// widescreen fight.defs spread: 4:3 ones often draw both bars as one frame.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        r: &mut Renderer,
        w: &World,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        portraits: [Option<Portrait>; 2],
        spread: f32,
    ) {
        let lc = self.localcoord;
        let spread = if lc[0] >= lc[1] * 1.7 { spread } else { 0.0 };
        let roots: Vec<usize> = w
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0)
            .map(|(i, _)| i)
            .take(2)
            .collect();
        for (side, &pi) in roots.iter().enumerate() {
            let dx = if side == 0 { -spread } else { spread };
            let side_hud = move |x: f32, y: f32| hud(x + dx, y);
            let hud: &dyn Fn(f32, f32) -> [f32; 2] = &side_hud;
            let p = &w.players[pi];
            let life = (p.life.max(0) as f32 / p.life_max.max(1) as f32).min(1.0);
            let bar = self.life[side].clone();
            // damage trail: hold on a fresh hit, `step` drains it
            if !self.trail_seen[side] {
                self.trail[side] = (life, 0, life);
                self.trail_seen[side] = true;
            }
            let (trail, hold, prev) = &mut self.trail[side];
            if life < *prev {
                *hold = bar.mid_delay;
            }
            *prev = life;
            let trail = trail.max(life);
            let lit = life > 0.0
                && life * 100.0 < bar.flash_low
                && (self.tick / (bar.flash_period / 2)) % 2 == 0;
            self.draw_bar(r, &bar, life, trail, lit, hud, u);
            let power = p.power.max(0) as f32 / p.power_max.max(1) as f32;
            let bar = self.power[side].clone();
            let level = (p.power / 1000).max(0);
            if bar.segments > 0 {
                let base = hud(bar.pos[0], bar.pos[1]);
                self.draw_elem(r, &bar.bg0, base, u, None);
                self.draw_elem(r, &bar.bg1, base, u, None);
                self.draw_segments(r, &bar, power * bar.segments as f32, hud, u);
            } else {
                self.draw_bar(r, &bar, power, power, false, hud, u);
            }
            let base = hud(bar.pos[0], bar.pos[1]);
            let stamp = if level >= 1 && bar.stamp_lit.present() {
                &bar.stamp_lit
            } else {
                &bar.stamp
            };
            self.draw_elem(r, stamp, base, u, None);
            let (font, lit_font, off) = self.power_counter[side];
            let font = if level >= 1 { lit_font.or(font) } else { font };
            let base = hud(bar.pos[0] + off[0], bar.pos[1] + off[1]);
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
                if let Some(fit) = face.fit {
                    // cover the box: crop the long side (faces sit high, keep the top)
                    let (tw, th) = r.texture_size(tex);
                    let (tw, th) = (tw.max(1) as f32, th.max(1) as f32);
                    let s = (fit[0] / tw).max(fit[1] / th);
                    let (sw, sh) = ((fit[0] / s).min(tw), (fit[1] / s).min(th));
                    let sx = (tw - sw) / 2.0;
                    let sy = (th - sh) * 0.15;
                    d.src = Some([sx as u32, sy as u32, sw as u32, sh as u32]);
                    d.axis = [sw / 2.0, sh / 2.0];
                    d.scale = [s * u, s * u];
                }
                d.hflip = face.face.facing < 0.0;
                d.angle = face.face.angle;
                r.draw(d);
            }
            // name, on its plate
            let tag = self.names[side].clone();
            let mut name = w.data[p.data].display_name.clone();
            if tag.upper {
                name = name.to_uppercase();
            }
            let tw = self.text_width(tag.font, &name);
            let align = tag.font.map_or(1, |f| f.f.align);
            let (tx0, tx1) = match align {
                a if a > 0 => (tag.pos[0], tag.pos[0] + tw),
                0 => (tag.pos[0] - tw / 2.0, tag.pos[0] + tw / 2.0),
                _ => (tag.pos[0] - tw, tag.pos[0]),
            };
            let (px0, px1) = (tx0 - tag.pad, tx1 + tag.pad);
            if tag.bg.present() {
                let y = tag.pos[1] + tag.bg.offset[1];
                self.draw_sliced(r, &tag.bg, tag.slice, px0, px1, y, hud, u);
            }
            self.text(r, tag.font, &name, hud(tag.pos[0], tag.pos[1]), u);
            let tag_end = if align < 0 { px0 } else { px1 };
            // win icons
            let wins = w.round.wins[p.team as usize] as usize;
            let kinds: Vec<String> = w
                .round
                .win_kinds
                .get(p.team as usize)
                .cloned()
                .unwrap_or_default();
            let mut icon_pos = self.winicon_pos[side];
            if self.winicon_follow[side] {
                icon_pos[0] += tag_end;
            }
            let off = self.winicon_offset[side];
            let at = |k: usize| {
                [
                    icon_pos[0] + off[0] * k as f32,
                    icon_pos[1] + off[1] * k as f32,
                ]
            };
            // empty marks for the rounds still needed
            let needed = w.round.config.match_wins as usize;
            if self.winicon_empty[side].present() {
                for k in wins..needed {
                    let e = self.winicon_empty[side].clone();
                    let pos = at(k);
                    self.draw_elem(r, &e, hud(pos[0], pos[1]), u, None);
                }
            }
            for k in 0..wins {
                let kind = kinds.get(k).cloned().unwrap_or_else(|| "n".into());
                let Some(e) = self.winicon[side].get(&kind).cloned() else {
                    continue;
                };
                let pos = at(k);
                self.draw_elem(r, &e, hud(pos[0], pos[1]), u, None);
            }
            // arena stocks: one mark per life left, a row under the win icons
            let stocks = p.stocks.max(0) as usize;
            for k in 0..stocks {
                let pos = [at(k)[0], icon_pos[1] + 14.0];
                let a = hud(pos[0] - 3.0, pos[1] - 3.0);
                let b = hud(pos[0] + 3.0, pos[1] + 3.0);
                r.rect(a, b, [1.0, 0.85, 0.2, 0.95]);
            }
        }
        // clock
        let clock = w.round.clock();
        let (off, font) = self.time_counter;
        if self.time_bg.present() {
            let e = self.time_bg.clone();
            let base = hud(self.time_pos[0], self.time_pos[1]);
            self.draw_elem(r, &e, base, u, None);
        }
        let base = hud(self.time_pos[0] + off[0], self.time_pos[1] + off[1]);
        let line = if clock < 0 {
            "o".to_string()
        } else {
            format!("{clock:02}")
        };
        self.text(r, font, &line, base, u);
        // round tag under the clock
        let rt = self.round_tag.clone();
        if rt.font.is_some() || rt.bg.present() {
            let base = hud(self.time_pos[0], self.time_pos[1]);
            self.draw_elem(r, &rt.bg, base, u, None);
            let line = rt.text.replace("%i", &w.round.round_no.max(1).to_string());
            let pos = hud(
                self.time_pos[0] + rt.offset[0],
                self.time_pos[1] + rt.offset[1],
            );
            self.text(r, rt.font, &line, pos, u);
        }
        // combo counters: hits the other team has taken in its current combo
        for side in 0..roots.len() {
            let Some(&other) = roots.get(1 - side) else {
                continue;
            };
            let dx = if side == 0 { -spread } else { spread };
            let side_hud = move |x: f32, y: f32| hud(x + dx, y);
            let hud: &dyn Fn(f32, f32) -> [f32; 2] = &side_hud;
            let hits = w.players[other].received_hits;
            let c = self.combo[side].clone();
            let st = &mut self.combo_state[side];
            if hits >= 2 {
                *st = (hits, c.displaytime);
            }
            if st.1 <= 0 || st.0 < 2 {
                continue;
            }
            let (shown_hits, _) = *st;
            let pivot = hud(c.pos[0], c.pos[1]);
            let label = c.text.replace("%i", &shown_hits.to_string());
            let tpos = hud(c.pos[0] + c.text_offset[0], c.pos[1] + c.text_offset[1]);
            for (f, line, pos) in [
                (c.counter_font, shown_hits.to_string(), pivot),
                (c.text_font, label, tpos),
            ] {
                let Some(Fnt { f, scale }) = f else { continue };
                if let Some(font) = self.fonts.get_mut(&f.index) {
                    font.draw_rotated(r, &line, pos, f.bank, f.align, u * scale, c.angle, pivot);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_fight_def_uses_the_extensions() {
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif/themes/ink");
        let lb = Lifebar::load(&motif, &motif).expect("ink fight.def loads");
        assert_eq!(lb.localcoord, [1280.0, 720.0]);
        assert_eq!(lb.power[0].segments, 3);
        assert!(lb.power[1].seg_part.present() && lb.power[1].stamp_lit.present());
        assert_eq!(lb.life[0].flash_low, 25.0);
        assert!(lb.life[1].fg0.present() && lb.life[1].flash.present());
        assert!(lb.names[0].bg.present() && lb.names[0].upper);
        assert_eq!(lb.faces[1].fit, Some([90.0, 90.0]));
        assert_eq!(lb.winicon_follow, [true, true]);
        assert!(lb.round_tag.font.is_some());
        assert_eq!(lb.combo[0].angle, 6.0);
        assert_eq!(lb.time_counter.1.map(|f| f.scale), Some(5.0));
    }
}
