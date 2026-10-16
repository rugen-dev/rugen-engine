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
//!   `pN.label.text = WINS` with `pN.label.font` / `.scale`: a caption after
//!   the last round slot, `pN.label.offset` from that slot.
//! - `[ArenaHUD]` colours (`r,g,b`) and font banks of the arena cards:
//!   `card.color`, `card.border`, `pN.accent`, `pN.shadow`, `face.bg`,
//!   `face.border`, `life.pN`, `life.empty`, `pip.color`, `power.off`,
//!   `power.on`, `clock.color`, `clock.shadow`, `tag.pN.bank`, `name.bank`,
//!   `num.pN.bank`, `label.bank`, `clock.bank`, `hint.title.bank` (the title
//!   of an in-world hint, the ledge timer). Unset keys keep the ink look.
//! - `[TeamHUD]` partner strips of a team match (face, small life bar and a
//!   caption plate per partner, stacked from `pN.pos`, P2 mirrored):
//!   `spacing`, `face.size`, `bar.size = w,h`, `simul.bar.width`, `border`,
//!   `pN.tag.pos` (the `TAG <key>` plate of a Tag team a player drives),
//!   colours `border.color`, `bar.bg`, `face.bg`, `life.color`,
//!   `red.color` (life a Tag partner wins back), `down.color`, and
//!   `label.font` (default: the name font at half scale). Without the
//!   section the strips sit under the `[Name]` plates.
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
use rugen_render::{Draw, PaletteId, Renderer, TextureId, Tint};
use rugen_sim::World;
use rugen_sim::round::Finish;
use rugen_sim::world::Event;

use crate::lang::{t, tf};
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

/// Colours (RGBA 0..1) and font banks of the arena HUD cards.
#[derive(Clone, Debug)]
struct ArenaStyle {
    card: [f32; 4],
    border: [f32; 4],
    accent: [[f32; 4]; 2],
    shadow: [[f32; 4]; 2],
    face_bg: [f32; 4],
    face_border: [f32; 4],
    life: [[f32; 4]; 2],
    life_empty: [f32; 4],
    pip: [f32; 4],
    power_off: [f32; 4],
    power_on: [f32; 4],
    clock: [f32; 4],
    clock_shadow: [f32; 4],
    tag_bank: [i32; 2],
    name_bank: i32,
    num_bank: [i32; 2],
    label_bank: i32,
    clock_bank: i32,
    /// The title of an in-world hint panel (`LEDGE · 1.4 S`).
    hint_title_bank: i32,
}

/// One piece of a hint row: plain text or a key cap.
#[derive(Clone, Debug, PartialEq)]
pub enum HintSeg {
    Text(String),
    Cap(String),
}

/// An in-world hint over a fighter (approved mockup "Arena features"):
/// `dark` = a panel in the clock's colours with an optional title (the
/// ledge), else a small card tag with a border (an item picked up).
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub dark: bool,
    pub title: Option<String>,
    pub rows: Vec<Vec<HintSeg>>,
    /// Hang under the anchor instead of over it (no room above).
    pub below: bool,
}

impl Hint {
    /// Height of the panel plus its gap to the anchor, in 1280x720 design
    /// pixels.
    pub fn design_height(&self) -> f32 {
        let rows = (self.rows.len() + usize::from(self.title.is_some())) as f32;
        14.0 + rows * 20.0 + (rows - 1.0).max(0.0) * 6.0 + 16.0
    }
}

impl Default for ArenaStyle {
    /// The Shonen Ink look (approved mockup "Arena HUD").
    fn default() -> Self {
        const INK: [f32; 4] = [0.078, 0.071, 0.063, 1.0];
        const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        const RED: [f32; 4] = [0.784, 0.063, 0.102, 1.0];
        Self {
            card: WHITE,
            border: INK,
            accent: [RED, INK],
            shadow: [RED, INK],
            face_bg: [0.914, 0.89, 0.839, 1.0],
            face_border: INK,
            life: [RED, [0.706, 0.325, 0.035, 1.0]],
            life_empty: [0.227, 0.216, 0.196, 1.0],
            pip: INK,
            power_off: [0.839, 0.816, 0.769, 1.0],
            power_on: [0.184, 0.435, 0.82, 1.0],
            clock: INK,
            clock_shadow: RED,
            tag_bank: [11, 11],
            name_bank: 9,
            num_bank: [10, 9],
            label_bank: 8,
            clock_bank: 11,
            hint_title_bank: 15,
        }
    }
}

impl ArenaStyle {
    fn parse(s: &Section) -> Self {
        let d = Self::default();
        let col = |k: &str, d: [f32; 4]| {
            s.f32s(k)
                .filter(|v| v.len() >= 3)
                .map_or(d, |v| [v[0] / 255.0, v[1] / 255.0, v[2] / 255.0, 1.0])
        };
        let bank = |k: &str, d: i32| s.i32(k).unwrap_or(d);
        Self {
            card: col("card.color", d.card),
            border: col("card.border", d.border),
            accent: [col("p1.accent", d.accent[0]), col("p2.accent", d.accent[1])],
            shadow: [col("p1.shadow", d.shadow[0]), col("p2.shadow", d.shadow[1])],
            face_bg: col("face.bg", d.face_bg),
            face_border: col("face.border", d.face_border),
            life: [col("life.p1", d.life[0]), col("life.p2", d.life[1])],
            life_empty: col("life.empty", d.life_empty),
            pip: col("pip.color", d.pip),
            power_off: col("power.off", d.power_off),
            power_on: col("power.on", d.power_on),
            clock: col("clock.color", d.clock),
            clock_shadow: col("clock.shadow", d.clock_shadow),
            tag_bank: [
                bank("tag.p1.bank", d.tag_bank[0]),
                bank("tag.p2.bank", d.tag_bank[1]),
            ],
            name_bank: bank("name.bank", d.name_bank),
            num_bank: [
                bank("num.p1.bank", d.num_bank[0]),
                bank("num.p2.bank", d.num_bank[1]),
            ],
            label_bank: bank("label.bank", d.label_bank),
            clock_bank: bank("clock.bank", d.clock_bank),
            hint_title_bank: bank("hint.title.bank", d.hint_title_bank),
        }
    }
}

/// rugen: `[TeamHUD]`, the partner strips of a team match: a small face,
/// a small life bar and a caption per partner (Turns: the fighters still to
/// come or down), under each side's name plate.
#[derive(Clone, Debug)]
struct TeamStyle {
    /// First strip per side; `None` = under the `[Name]` plate.
    pos: [Option<[f32; 2]>; 2],
    /// Vertical step between strips.
    spacing: f32,
    face: f32,
    bar: [f32; 2],
    /// Simul: partners fight at full strength, so their bars run as long
    /// as this.
    simul_bar: f32,
    border: f32,
    /// Tag: where the `TAG <key>` plate starts.
    tag_pos: [Option<[f32; 2]>; 2],
    ink: [f32; 4],
    paper: [f32; 4],
    face_bg: [f32; 4],
    life: [f32; 4],
    /// Life a Tag partner wins back while out (Ikemen red life).
    red: [f32; 4],
    down: [f32; 4],
    label: Option<Fnt>,
}

impl TeamStyle {
    fn parse(s: &Section) -> Self {
        let col = |k: &str, d: [f32; 4]| {
            s.f32s(k)
                .filter(|v| v.len() >= 3)
                .map_or(d, |v| [v[0] / 255.0, v[1] / 255.0, v[2] / 255.0, 1.0])
        };
        let pos = |k: &str| s.f32s(k).filter(|v| v.len() >= 2).map(|v| [v[0], v[1]]);
        let num = |k: &str, d: f32| s.f32(k).unwrap_or(d);
        let size = |k: &str, d: [f32; 2]| {
            s.f32s(k)
                .filter(|v| v.len() >= 2)
                .map_or(d, |v| [v[0], v[1]])
        };
        const INK: [f32; 4] = [0.078, 0.071, 0.063, 1.0];
        Self {
            pos: [pos("p1.pos"), pos("p2.pos")],
            spacing: num("spacing", 34.0),
            face: num("face.size", 30.0),
            bar: size("bar.size", [160.0, 10.0]),
            simul_bar: num("simul.bar.width", 300.0),
            border: num("border", 2.0),
            tag_pos: [pos("p1.tag.pos"), pos("p2.tag.pos")],
            ink: col("border.color", INK),
            paper: col("bar.bg", [1.0, 1.0, 1.0, 1.0]),
            face_bg: col("face.bg", [0.949, 0.933, 0.894, 1.0]),
            life: col("life.color", [0.91, 0.541, 0.541, 1.0]),
            red: col("red.color", [1.0, 0.824, 0.247, 1.0]),
            down: col("down.color", [0.725, 0.698, 0.643, 1.0]),
            label: Fnt::parse(s, "label"),
        }
    }
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
    /// rugen: `pN.label.*` caption after the round slots.
    winicon_label: [(Option<Fnt>, String, [f32; 2]); 2],
    /// rugen: `[ArenaHUD]` colours and banks.
    arena: ArenaStyle,
    /// rugen: `[TeamHUD]` partner strips.
    team: TeamStyle,
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
    /// Upload the HUD's sprites ahead of the first fight frame, as far as
    /// `budget` goes this call.
    pub fn warm(&mut self, r: &mut Renderer, budget: &mut crate::sprites::Budget) {
        self.sheet.warm(r, budget)
    }

    /// Give back the sheet's textures and the fonts' glyphs (the match is over).
    pub fn release_gpu(&mut self, r: &mut Renderer) {
        self.sheet.free_gpu(r);
        for f in self.fonts.values_mut() {
            f.free_gpu(r);
        }
    }

    /// `motif` holds the screenpack (fight.def, fight.sff, fonts); anything
    /// missing there is looked up in `data_dir`.
    pub fn load(motif: &Path, data_dir: &Path) -> anyhow::Result<Self> {
        let chain = crate::themes::screenpack_chain(motif, data_dir);
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
        let label = |p: &str| {
            (
                Fnt::parse(wi, &format!("{p}.label")),
                wi.raw(&format!("{p}.label.text"))
                    .map(|t| t.trim().to_string())
                    .unwrap_or_default(),
                pos2(wi, &format!("{p}.label.offset"), [0.0, 0.0]),
            )
        };
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
            winicon_label: [label("p1"), label("p2")],
            arena: ArenaStyle::parse(sec("arenahud")),
            team: TeamStyle::parse(sec("teamhud")),
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
                // a new round clears the last one's lines ("X WINS" ran on
                // into the next round's FIGHT)
                self.shown.clear();
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
                    .side_leader(*team as usize)
                    .map(|i| w.data[w.players[i].data].display_name.clone())
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

    /// Unscaled width of `line` in fight.def font `index` and where its ink
    /// (letters and digits) starts and ends, relative to the baseline that
    /// [`Lifebar::text_at`] draws on.
    pub fn text_ink(&self, index: i32, line: &str) -> Option<[f32; 3]> {
        let f = self.fonts.get(&index)?;
        let cell_top = 1.0 + f.font.offset[1] as f32 - f32::from(f.font.size[1]);
        let (t, b) = f
            .font
            .ink_rows()
            .unwrap_or((0, f.font.size[1].saturating_sub(1)));
        Some([
            f.width(line, 1.0),
            cell_top + f32::from(t),
            cell_top + f32::from(b) + 1.0,
        ])
    }

    /// Baseline offset (fight.def units) that centres capitals on a line.
    fn text_center(&self, f: Option<Fnt>) -> f32 {
        let Some(Fnt { f, scale }) = f else {
            return 0.0;
        };
        self.text_ink(f.index, "A")
            .map_or(0.0, |[_, top, bottom]| -(top + bottom) / 2.0 * scale)
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

    /// Arena HUD (approved mockup "Arena HUD"): one card per fighter centred
    /// along the bottom — portrait over the card's top edge, P1 / P2 tag,
    /// stock (or round) pips, name, life in percent, a 10-segment life bar
    /// and the power bars — and a small clock box at the top. Laid out in
    /// 1280x720 design pixels scaled to the fight.def's height.
    /// The partner strips of `side` (see [`TeamStyle`]).
    fn draw_team(
        &mut self,
        r: &mut Renderer,
        w: &World,
        side: usize,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        portrait: &dyn Fn(usize) -> Option<Portrait>,
    ) {
        use rugen_sim::team::TeamMode;
        let st = self.team.clone();
        let mode = w.team_mode(side);
        let leader = w.side_leader(side);
        let members = &w.teams.members[side];
        let active_member = leader.map_or(0, |i| w.players[i].member);
        // p2 strips run right to left from their anchor
        let dir = if side == 0 { 1.0 } else { -1.0 };
        let label = st.label.or_else(|| {
            self.names[side].font.map(|mut f| {
                f.scale *= 0.5;
                f
            })
        });
        let anchor = st.pos[side].unwrap_or_else(|| {
            let n = &self.names[side];
            [n.pos[0], n.pos[1] + 14.0]
        });
        let bar_w = if mode == TeamMode::Simul {
            st.simul_bar
        } else {
            st.bar[0]
        };
        let rect = |r: &mut Renderer, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]| {
            let (a, b) = (x0.min(x1), x0.max(x1));
            r.rect(hud(a, y0), hud(b, y1), c);
        };
        for (row, &i) in members.iter().filter(|&&i| Some(i) != leader).enumerate() {
            let p = &w.players[i];
            let y = anchor[1] + row as f32 * st.spacing;
            let x = anchor[0];
            let f = st.face;
            let b = st.border;
            // mini face
            rect(r, x, y, x + dir * f, y + f, st.ink);
            rect(
                r,
                x + dir * b,
                y + b,
                x + dir * (f - b),
                y + f - b,
                st.face_bg,
            );
            if let Some((tex, pal, _)) = portrait(i) {
                let (tw, th) = r.texture_size(tex);
                let (tw, th) = (tw.max(1) as f32, th.max(1) as f32);
                let inner = f - 2.0 * b;
                let s = (inner / tw).max(inner / th);
                let (sw, sh) = ((inner / s).min(tw), (inner / s).min(th));
                let cx = x + dir * f / 2.0;
                let mut d = Draw::new(tex, pal, hud(cx, y + f / 2.0), [sw / 2.0, sh / 2.0]);
                d.src = Some([
                    ((tw - sw) / 2.0) as u32,
                    ((th - sh) * 0.15) as u32,
                    sw as u32,
                    sh as u32,
                ]);
                d.scale = [s * u, s * u];
                d.hflip = side == 1;
                if !p.alive() || (mode == TeamMode::Turns && p.member < active_member) {
                    d.tint = Tint {
                        mul: [0.6, 0.6, 0.6],
                        color: 0.0,
                        ..Default::default()
                    };
                }
                r.draw(d);
            }
            // small life bar
            let bx = x + dir * (f + 6.0);
            let by = y + (f - st.bar[1]) / 2.0;
            let (bh, bw) = (st.bar[1], bar_w);
            rect(r, bx, by, bx + dir * bw, by + bh, st.ink);
            rect(
                r,
                bx + dir * b,
                by + b,
                bx + dir * (bw - b),
                by + bh - b,
                st.paper,
            );
            let inner = bw - 2.0 * b;
            let frac = |v: i32| (v.max(0) as f32 / p.life_max.max(1) as f32).clamp(0.0, 1.0);
            let fallen = !p.alive() || (mode == TeamMode::Turns && p.member < active_member);
            let life = if fallen { 0.0 } else { frac(p.life) };
            let red = if mode == TeamMode::Tag {
                frac(p.red_life).max(life)
            } else {
                life
            };
            let x0 = bx + dir * b;
            if red > life {
                rect(
                    r,
                    x0 + dir * inner * life,
                    by + b,
                    x0 + dir * inner * red,
                    by + bh - b,
                    st.red,
                );
            }
            if life > 0.0 {
                rect(r, x0, by + b, x0 + dir * inner * life, by + bh - b, st.life);
            } else {
                rect(r, x0, by + b, x0 + dir * inner, by + bh - b, st.down);
            }
            // caption under the bar: NAME · STATE
            let state = if fallen {
                t("fight.down")
            } else {
                match mode {
                    TeamMode::Turns if p.member == active_member + 1 => t("common.next"),
                    TeamMode::Turns => t("fight.waiting"),
                    TeamMode::Tag if red > life => t("fight.recovering"),
                    TeamMode::Tag => t("teams.ready"),
                    _ => "",
                }
            };
            let name = w.data[p.data].display_name.to_uppercase();
            let line = if state.is_empty() {
                name
            } else {
                format!("{name} · {state}")
            };
            // ... on a small plate right of the bar (p2: left of it)
            if let Some(mut font) = label {
                font.f.align = if side == 0 { 1 } else { -1 };
                let tw = self.text_width(Some(font), &line);
                let lx = bx + dir * (bw + 8.0);
                let (pad, ph) = (4.0, st.face * 0.55);
                let py = y + (f - ph) / 2.0;
                rect(r, lx, py, lx + dir * (tw + 2.0 * pad), py + ph, st.ink);
                rect(
                    r,
                    lx + dir * 1.0,
                    py + 1.0,
                    lx + dir * (tw + 2.0 * pad - 1.0),
                    py + ph - 1.0,
                    st.paper,
                );
                let base = py + ph / 2.0 + self.text_center(Some(font));
                self.text(r, Some(font), &line, hud(lx + dir * pad, base), u);
            }
        }
    }

    /// Tag: `TAG <key>` on a plate by the side's power bar
    /// (`[TeamHUD] pN.tag.pos`, else just inside the power bar's end).
    fn draw_tag_key(
        &mut self,
        r: &mut Renderer,
        side: usize,
        key: &str,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
    ) {
        let st = self.team.clone();
        let Some(mut font) = st.label.or_else(|| self.names[side].font) else {
            return;
        };
        let dir = if side == 0 { 1.0 } else { -1.0 };
        let pos = st.tag_pos[side].unwrap_or_else(|| {
            let b = &self.power[side];
            [b.pos[0] + b.range[1] + dir * 12.0, b.pos[1]]
        });
        font.f.align = if side == 0 { 1 } else { -1 };
        let line = tf("fight.tag_key", &[("key", &key)]);
        let tw = self.text_width(Some(font), &line);
        let (pad, h) = (6.0, st.face * 0.7);
        let rect = |r: &mut Renderer, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]| {
            r.rect(hud(x0.min(x1), y0), hud(x0.max(x1), y1), c);
        };
        rect(
            r,
            pos[0],
            pos[1] - h / 2.0,
            pos[0] + dir * (tw + 2.0 * pad),
            pos[1] + h / 2.0,
            st.ink,
        );
        rect(
            r,
            pos[0] + dir,
            pos[1] - h / 2.0 + 1.0,
            pos[0] + dir * (tw + 2.0 * pad - 1.0),
            pos[1] + h / 2.0 - 1.0,
            st.paper,
        );
        let base = pos[1] + self.text_center(Some(font));
        self.text(r, Some(font), &line, hud(pos[0] + dir * pad, base), u);
    }

    fn draw_arena(
        &mut self,
        r: &mut Renderer,
        w: &World,
        roots: &[usize],
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        portraits: [Option<Portrait>; 2],
    ) {
        let st = self.arena.clone();
        let lc = self.localcoord;
        let k = lc[1] / 720.0;
        let at = |x: f32, y: f32| hud(lc[0] / 2.0 + (x - 640.0) * k, y * k);
        let rect = |r: &mut Renderer, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]| {
            r.rect(at(x0, y0), at(x1, y1), c);
        };
        let us = u * k;
        // card geometry (outer box with its 4 px border)
        let (card_w, card_h, gap, bottom) = (372.0, 112.0, 48.0, 26.0);
        let top = 720.0 - bottom - card_h;
        let left0 = 640.0 - card_w - gap / 2.0;
        let stocks_max = w.stage.arena.stocks.max(0);
        for (side, &pi) in roots.iter().enumerate().take(2) {
            let p = &w.players[pi];
            let x0 = left0 + side as f32 * (card_w + gap);
            let (x1, y1) = (x0 + card_w, top + card_h);
            // hard shadow, border, card face
            rect(r, x0 + 6.0, top + 6.0, x1 + 6.0, y1 + 6.0, st.shadow[side]);
            rect(r, x0, top, x1, y1, st.border);
            rect(r, x0 + 4.0, top + 4.0, x1 - 4.0, y1 - 4.0, st.card);
            let ix = x0 + 18.0;
            // portrait square over the top edge
            let (fx, fy, fs) = (ix, top - 18.0, 72.0);
            rect(r, fx, fy, fx + fs, fy + fs, st.face_border);
            rect(
                r,
                fx + 3.0,
                fy + 3.0,
                fx + fs - 3.0,
                fy + fs - 3.0,
                st.face_bg,
            );
            if let Some((tex, pal, _)) = portraits[side] {
                let inner = fs - 6.0;
                let (tw, th) = r.texture_size(tex);
                let (tw, th) = (tw.max(1) as f32, th.max(1) as f32);
                let sc = (inner / tw).max(inner / th);
                let (sw, sh) = ((inner / sc).min(tw), (inner / sc).min(th));
                let centre = at(fx + fs / 2.0, fy + fs / 2.0);
                let mut d = Draw::new(tex, pal, centre, [sw / 2.0, sh / 2.0]);
                d.src = Some([
                    ((tw - sw) / 2.0) as u32,
                    ((th - sh) * 0.15) as u32,
                    sw as u32,
                    sh as u32,
                ]);
                d.scale = [sc * us, sc * us];
                d.hflip = side == 1;
                r.draw(d);
            }
            // a carried item: a tab on the card's top edge, at the right
            if let Some(it) = w.items.held_by(pi) {
                let tw = self
                    .fonts
                    .get(&1)
                    .map_or(30.0, |f| f.width(t("fight.item"), 1.0));
                let (bw, bh) = (tw + 44.0, 24.0);
                let (bx1, by0) = (x1 - 18.0, top - 14.0);
                let bx0 = bx1 - bw;
                rect(r, bx0, by0, bx1, by0 + bh, st.clock);
                rect(r, bx1 - 26.0, by0 + 3.0, bx1 - 4.0, by0 + bh - 3.0, st.card);
                let mid = self.text_ink(1, "A").map_or(3.5, |i| -(i[1] + i[2]) / 2.0);
                if let Some(f) = self.fonts.get_mut(&1) {
                    f.draw(
                        r,
                        t("fight.item"),
                        at(bx0 + 8.0, by0 + bh / 2.0 + mid),
                        st.clock_bank,
                        1,
                        us,
                    );
                }
                crate::play::draw_item(r, it.kind, at(bx1 - 15.0, by0 + bh / 2.0), 6.0 * us);
            }
            // tag, pips, name
            let tx = fx + fs + 12.0;
            rect(r, tx, top + 16.0, tx + 30.0, top + 32.0, st.accent[side]);
            if let Some(f) = self.fonts.get_mut(&1) {
                f.draw(
                    r,
                    &format!("P{}", side + 1),
                    at(tx + 15.0, top + 29.0),
                    st.tag_bank[side],
                    0,
                    us,
                );
            }
            let (filled, total) = if stocks_max > 0 {
                (p.stocks.clamp(0, stocks_max), stocks_max)
            } else {
                let need = i32::from(w.round.config.match_wins).max(1);
                (i32::from(w.round.wins[p.team as usize]).min(need), need)
            };
            // the life number keeps a fixed column (wide enough for 100)
            let num_w = self.fonts.get(&2).map_or(96.0, |f| f.width("100", 2.0));
            let num_x = x1 - 18.0;
            let pip_right = num_x - num_w - 12.0;
            for i in 0..total {
                let px = pip_right - (total - i) as f32 * 14.0;
                let (a0, a1) = (px, px + 10.0);
                if i < filled {
                    rect(r, a0, top + 19.0, a1, top + 29.0, st.pip);
                } else {
                    r.rect_outline(at(a0, top + 19.0), at(a1, top + 29.0), 2.0 * us, st.pip);
                }
            }
            // the name in the small font doubled; a long one in the narrower
            // caption font, cut to fit if it still does not
            let mut name = w.data[p.data].display_name.to_uppercase();
            let max_w = num_x - num_w - 10.0 - tx;
            let big = self
                .fonts
                .get(&1)
                .is_some_and(|f| f.width(&name, 2.0) <= max_w);
            let (font, scale) = if big || !self.fonts.contains_key(&3) {
                (1, 2.0)
            } else {
                (3, 1.0)
            };
            if let Some(f) = self.fonts.get(&font) {
                while name.chars().count() > 3 && f.width(&name, scale) > max_w {
                    name.pop();
                }
            }
            if let Some(f) = self.fonts.get_mut(&font) {
                f.draw(
                    r,
                    name.trim_end(),
                    at(tx, top + 58.0),
                    st.name_bank,
                    1,
                    scale * us,
                );
            }
            // life in percent, big, at the right
            let pct = if p.life <= 0 {
                0
            } else {
                ((p.life as i64 * 100 + p.life_max.max(1) as i64 - 1) / p.life_max.max(1) as i64)
                    .clamp(1, 100) as i32
            };
            let num_bank = st.num_bank[side];
            if let Some(f) = self.fonts.get_mut(&2) {
                f.draw(
                    r,
                    &pct.to_string(),
                    at(num_x, top + 58.0),
                    num_bank,
                    -1,
                    2.0 * us,
                );
            }
            // 10-segment life bar
            let (sx0, sx1, sy) = (x0 + 18.0, x1 - 18.0, top + 70.0);
            let seg_w = (sx1 - sx0 - 9.0 * 3.0) / 10.0;
            let lit = if p.life <= 0 {
                0
            } else {
                ((pct + 5) / 10).clamp(1, 10)
            };
            for i in 0..10 {
                let a = sx0 + i as f32 * (seg_w + 3.0);
                let c = if i < lit {
                    st.life[side]
                } else {
                    st.life_empty
                };
                rect(r, a, sy, a + seg_w, sy + 12.0, c);
            }
            // power: one pip per bar, filling as power builds
            if let Some(f) = self.fonts.get_mut(&1) {
                f.draw(
                    r,
                    t("online.power"),
                    at(sx0, top + 98.0),
                    st.label_bank,
                    1,
                    us,
                );
            }
            let bars = (p.power_max / 1000).max(1);
            let label_w = self
                .fonts
                .get(&1)
                .map_or(40.0, |f| f.width(t("online.power"), 1.0));
            let px0 = sx0 + label_w + 10.0;
            for i in 0..bars {
                let a = px0 + i as f32 * 46.0;
                rect(r, a, top + 90.0, a + 40.0, top + 98.0, st.power_off);
                let part = ((p.power - i * 1000) as f32 / 1000.0).clamp(0.0, 1.0);
                if part > 0.0 {
                    rect(r, a, top + 90.0, a + 40.0 * part, top + 98.0, st.power_on);
                }
            }
        }
        // the clock: a small black box with a red shadow, top centre
        if let Some(line) = arena_clock(w.round.time_left) {
            let tw = self.fonts.get(&2).map_or(40.0, |f| f.width(&line, 2.0));
            let (bw, bh) = (tw + 36.0, 46.0);
            let (bx, by) = (640.0 - bw / 2.0, 20.0);
            rect(
                r,
                bx + 4.0,
                by + 4.0,
                bx + bw + 4.0,
                by + bh + 4.0,
                st.clock_shadow,
            );
            rect(r, bx, by, bx + bw, by + bh, st.clock);
            if let Some(f) = self.fonts.get_mut(&2) {
                f.draw(r, &line, at(640.0, by + 38.0), st.clock_bank, 0, 2.0 * us);
            }
        }
    }

    /// An in-world hint (see [`Hint`]) centred over `anchor` (a world
    /// position, the fighter's head). `px` = world units per fight.def
    /// pixel; the panel is laid out in 1280x720 design pixels like the
    /// arena cards.
    pub fn draw_hint(&mut self, r: &mut Renderer, anchor: [f32; 2], px: f32, hint: &Hint) {
        let st = self.arena.clone();
        let s = px * self.localcoord[1] / 720.0;
        let Some(f) = self.fonts.get(&1) else {
            return;
        };
        let ink = self.text_ink(1, "A").unwrap_or([0.0, -7.0, 0.0]);
        let mid = -(ink[1] + ink[2]) / 2.0;
        let (pad, row_h, gap, cap_h) = (10.0, 20.0, 6.0, 20.0);
        let seg_w = |seg: &HintSeg| match seg {
            HintSeg::Text(t) => f.width(t, 1.0),
            HintSeg::Cap(t) => f.width(t, 1.0).max(12.0) + 8.0,
        };
        let row_w = |row: &[HintSeg]| {
            row.iter().map(seg_w).sum::<f32>() + 6.0 * row.len().saturating_sub(1) as f32
        };
        let mut w = hint.rows.iter().map(|r| row_w(r)).fold(0.0f32, f32::max);
        if let Some(t) = &hint.title {
            w = w.max(f.width(t, 1.0));
        }
        let rows = hint.rows.len() + usize::from(hint.title.is_some());
        let (bw, bh) = (
            w + 2.0 * pad,
            rows as f32 * row_h + rows.saturating_sub(1) as f32 * gap + 2.0 * pad - 4.0,
        );
        // bottom centre 14 px over the anchor (top centre under it)
        let x0 = -bw / 2.0;
        let y0 = if hint.below { 14.0 } else { -14.0 - bh };
        let at = |x: f32, y: f32| [anchor[0] + x * s, anchor[1] + y * s];
        let rect = |r: &mut Renderer, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]| {
            r.rect(at(x0, y0), at(x1, y1), c);
        };
        let (text_bank, cap_bg, cap_bank) = if hint.dark {
            rect(r, x0, y0, x0 + bw, y0 + bh, st.clock);
            (st.clock_bank, st.card, st.name_bank)
        } else {
            rect(
                r,
                x0 - 3.0,
                y0 - 3.0,
                x0 + bw + 3.0,
                y0 + bh + 3.0,
                st.border,
            );
            rect(r, x0, y0, x0 + bw, y0 + bh, st.card);
            (st.name_bank, st.clock, st.clock_bank)
        };
        let mut y = y0 + pad - 2.0;
        let Some(f) = self.fonts.get_mut(&1) else {
            return;
        };
        if let Some(t) = &hint.title {
            let base = y + row_h / 2.0 + mid;
            f.draw(r, t, at(x0 + pad, base), st.hint_title_bank, 1, s);
            y += row_h + gap;
        }
        for row in &hint.rows {
            let base = y + row_h / 2.0 + mid;
            let mut x = x0 + pad;
            for seg in row {
                match seg {
                    HintSeg::Text(t) => {
                        f.draw(r, t, at(x, base), text_bank, 1, s);
                        x += f.width(t, 1.0) + 6.0;
                    }
                    HintSeg::Cap(t) => {
                        let cw = f.width(t, 1.0).max(12.0) + 8.0;
                        let cy = y + (row_h - cap_h) / 2.0;
                        r.rect(at(x, cy), at(x + cw, cy + cap_h), cap_bg);
                        f.draw(r, t, at(x + cw / 2.0, base), cap_bank, 0, s);
                        x += cw + 6.0;
                    }
                }
            }
            y += row_h + gap;
        }
    }

    /// A CHALLENGE run's strip, bottom centre: `left` and `right` in a small
    /// box with a hard shadow, in the arena clock's colours (`[ArenaHUD]
    /// clock.color / clock.shadow / clock.bank`).
    pub fn draw_run_strip(
        &mut self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        left: &str,
        right: &str,
    ) {
        let st = self.arena.clone();
        let lc = self.localcoord;
        let k = lc[1] / 720.0;
        let at = |x: f32, y: f32| hud(lc[0] / 2.0 + (x - 640.0) * k, y * k);
        let us = u * k;
        let Some(f) = self.fonts.get_mut(&1) else {
            return;
        };
        let (pad, gap, bh) = (16.0, 40.0, 38.0);
        let bw = f.width(left, 2.0) + f.width(right, 2.0) + gap + 2.0 * pad;
        let (bx, by) = (640.0 - bw / 2.0, 720.0 - 22.0 - bh);
        r.rect(
            at(bx + 5.0, by + 5.0),
            at(bx + bw + 5.0, by + bh + 5.0),
            st.clock_shadow,
        );
        r.rect(at(bx, by), at(bx + bw, by + bh), st.clock);
        let base = by + bh / 2.0 + 9.0;
        f.draw(r, left, at(bx + pad, base), st.clock_bank, 1, 2.0 * us);
        f.draw(
            r,
            right,
            at(bx + bw - pad, base),
            st.clock_bank,
            -1,
            2.0 * us,
        );
    }

    /// A small tag (`CPU`) under a side's bars, near its screen edge
    /// (`spread`: fight.def units each side's elements move out).
    pub fn draw_side_tag(
        &mut self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        spread: f32,
        side: usize,
        text: &str,
    ) {
        let st = self.arena.clone();
        let lc = self.localcoord;
        let k = lc[1] / 720.0;
        let Some(f) = self.fonts.get_mut(&1) else {
            return;
        };
        let (pad, bh) = (10.0, 28.0);
        let bw = f.width(text, 1.5) + 2.0 * pad;
        let by = 150.0;
        let bx = if side == 0 {
            40.0
        } else {
            lc[0] / k - 40.0 - bw
        };
        let shift = if side == 0 { -spread } else { spread };
        let at = |x: f32, y: f32| hud(x * k + shift, y * k);
        r.rect(
            at(bx + 4.0, by + 4.0),
            at(bx + bw + 4.0, by + bh + 4.0),
            st.clock_shadow,
        );
        r.rect(at(bx, by), at(bx + bw, by + bh), st.clock);
        f.draw(
            r,
            text,
            at(bx + bw / 2.0, by + bh / 2.0 + 7.0),
            st.clock_bank,
            0,
            1.5 * u * k,
        );
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
        portraits: &[Option<Portrait>],
        spread: f32,
        arena: bool,
        tag_keys: &[Option<String>; 2],
    ) {
        let lc = self.localcoord;
        let spread = if lc[0] >= lc[1] * 1.7 { spread } else { 0.0 };
        // each side's main bar: its fighter (a team's fighter in the fight)
        let roots: Vec<usize> = (0..2).filter_map(|s| w.side_leader(s)).collect();
        let portrait = |i: usize| portraits.get(i).copied().flatten();
        let side_portraits = [0, 1].map(|s| roots.get(s).and_then(|&i| portrait(i)));
        if arena {
            self.draw_arena(r, w, &roots, hud, u, side_portraits);
        }
        for (side, &pi) in roots.iter().enumerate().filter(|_| !arena) {
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
            if let Some((tex, pal, axis)) = side_portraits[side] {
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
            let needed = w.wins_needed(p.team as usize) as usize;
            if self.winicon_empty[side].present() {
                for k in wins..needed {
                    let e = self.winicon_empty[side].clone();
                    let pos = at(k);
                    self.draw_elem(r, &e, hud(pos[0], pos[1]), u, None);
                }
            }
            let (label_font, label, label_off) = self.winicon_label[side].clone();
            if !label.is_empty() && needed > 0 {
                let pos = at(needed);
                let base = hud(pos[0] + label_off[0], pos[1] + label_off[1]);
                self.text(r, label_font, &label, base, u);
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
        if w.teams.active() && !arena {
            for (side, key) in tag_keys.iter().enumerate() {
                let dx = if side == 0 { -spread } else { spread };
                let side_hud = move |x: f32, y: f32| hud(x + dx, y);
                self.draw_team(r, w, side, &side_hud, u, &portrait);
                if let Some(key) = key {
                    self.draw_tag_key(r, side, key, &side_hud, u);
                }
            }
        }
        // clock
        let clock = w.round.clock();
        let (off, font) = self.time_counter;
        if arena {
        } else if self.time_bg.present() {
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
        if !arena {
            self.text(r, font, &line, base, u);
        }
        // round tag under the clock
        let rt = self.round_tag.clone();
        if !arena && (rt.font.is_some() || rt.bg.present()) {
            let base = hud(self.time_pos[0], self.time_pos[1]);
            self.draw_elem(r, &rt.bg, base, u, None);
            let mut line = rt.text.replace("%i", &w.round.round_no.max(1).to_string());
            // team matches name their modes: "TURNS · ROUND 2"
            if w.teams.active() {
                let mut modes: Vec<String> = Vec::new();
                for m in [w.team_mode(0), w.team_mode(1)] {
                    let m = m.as_str().to_uppercase();
                    if m != "SINGLE" && !modes.contains(&m) {
                        modes.push(m);
                    }
                }
                // two different modes: TEAMS (the plate is short)
                match modes.len() {
                    0 => {}
                    1 => line = format!("{} · {line}", modes[0]),
                    _ => line = tf("fight.teams_line", &[("line", &line)]),
                }
            }
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

/// The arena clock: real minutes and seconds left (`1:39`), counted up to
/// the next whole second like MUGEN's clock; `None` without a time limit.
fn arena_clock(time_left: i32) -> Option<String> {
    if time_left < 0 {
        return None;
    }
    let secs = (time_left + 59) / 60;
    Some(format!("{}:{:02}", secs / 60, secs % 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_clock_reads_minutes_and_seconds() {
        assert_eq!(arena_clock(99 * 60).as_deref(), Some("1:39"));
        assert_eq!(arena_clock(60 * 60).as_deref(), Some("1:00"));
        assert_eq!(arena_clock(59 * 60 + 1).as_deref(), Some("1:00"));
        assert_eq!(arena_clock(9 * 60).as_deref(), Some("0:09"));
        assert_eq!(arena_clock(1).as_deref(), Some("0:01"));
        assert_eq!(arena_clock(0).as_deref(), Some("0:00"));
        assert_eq!(arena_clock(-1), None);
    }

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
        // no [ArenaHUD]: the ink defaults
        assert_eq!(lb.arena.num_bank, [10, 9]);
        assert!(lb.winicon_label[0].1.is_empty());
    }

    #[test]
    fn dojo_fight_def_styles_the_arena_and_labels_wins() {
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif/themes/dojo");
        let lb = Lifebar::load(&motif, &motif).expect("dojo fight.def loads");
        assert_eq!(lb.localcoord, [1280.0, 720.0]);
        assert_eq!(lb.winicon_label[0].1, "WINS");
        assert!(lb.winicon_label[1].0.is_some());
        assert_eq!(lb.arena.card[0], 238.0 / 255.0);
        assert_eq!(lb.arena.tag_bank, [14, 12]);
    }
}
