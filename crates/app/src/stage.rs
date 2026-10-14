//! MUGEN background definitions: `[BGdef]` + `[BG ...]` from a stage .def,
//! or `[TitleBGdef]` / `[SelectBGdef]` / `[VersusBGdef]` from system.def.
//! Visual only; the simulation keeps just the bounds in `rugen_sim::Stage`.
//!
//! The element maths is Ikemen GO's (`readBackGround`, `bgAction`,
//! `backGround.drawTransformed`, `Animation.Draw`, `renderSpriteQuad`,
//! `emitTiles`): scroll by `delta`, zoom per layer (`zoomdelta`, else the
//! delta), `scalestart` / `scaledelta`, parallax rasters (`xscale` shears,
//! `width` trapezoids converging on the screen's centre), tiling with
//! `tilespacing`, `window` / `maskwindow` / `windowdelta`, `velocity`,
//! `sin.x` / `sin.y`, `positionlink`, `mask`, `trans`, and the `[BGCtrl]`
//! controllers (Anim, Visible, Enable, PalFX, PosSet, PosAdd, SinX, SinY,
//! VelSet, VelAdd). Everything is laid out on the screen first (stage units
//! at zoom 1, top-left origin), then mapped into the caller's draw space.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use rugen_formats::air::Air;
use rugen_formats::def::{Def, Section};
use rugen_formats::sff::Sff;
use rugen_render::{Blend, PaletteId, Quad, Renderer, TextureId, Tint};
use rugen_sim::anim::AnimState;

/// Uploaded stage texture plus optional palette slot index.
type Tex = (TextureId, Option<usize>);

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Normal,
    Anim,
    Parallax,
}

/// Ikemen `bgAction`: velocity and sine motion of one element.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BgAction {
    offset: [f32; 2],
    sinoffset: [f32; 2],
    pos: [f32; 2],
    vel: [f32; 2],
    radius: [f32; 2],
    sintime: [i32; 2],
    sinlooptime: [i32; 2],
}

impl BgAction {
    fn action(&mut self, update_time: bool) {
        for i in 0..2 {
            self.pos[i] += self.vel[i];
            if self.sinlooptime[i] > 0 {
                self.sinoffset[i] = self.radius[i]
                    * libm::sinf(
                        2.0 * std::f32::consts::PI * self.sintime[i] as f32
                            / self.sinlooptime[i] as f32,
                    );
                if update_time {
                    self.sintime[i] += 1;
                    if self.sintime[i] >= self.sinlooptime[i] {
                        self.sintime[i] = 0;
                    }
                }
            } else {
                self.sinoffset[i] = 0.0;
            }
            self.offset[i] = self.pos[i] + self.sinoffset[i];
        }
    }
}

/// A background element's own PalFX (`[BGCtrl] type = PalFX`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BgPalFx {
    on: bool,
    add: [i32; 3],
    mul: [i32; 3],
    /// r, g, b amplitude and period.
    sinadd: [i32; 4],
    invert: bool,
    color: f32,
}

#[derive(Clone, Debug)]
struct Elem {
    kind: Kind,
    /// `spriteno` (normal, parallax) or the animation's first frame.
    sprite: [i32; 2],
    action: i32,
    anim: AnimState,
    id: i32,
    layer: i32,
    start: [f32; 2],
    delta: [f32; 2],
    positionlink: bool,
    /// Element whose sine offsets a position-linked element adds.
    link: usize,
    /// Colour 0 transparent (`mask = 1`, any `trans`, animations).
    mask: bool,
    /// `None` = the animation frame's own blend.
    blend: Option<Blend>,
    /// x / y: 0 none, 1 infinite, n copies.
    tile: [i32; 2],
    /// Ikemen's `tile.xspacing` / `yspacing` (normal sprites: gap + size).
    spacing: [i32; 2],
    width: Option<[f32; 2]>,
    rasterx: [f32; 2],
    yscalestart: f32,
    yscaledelta: f32,
    scalestart: [f32; 2],
    scaledelta: [f32; 2],
    xshear: f32,
    zoomdelta: Option<[f32; 2]>,
    zoomscaledelta: [Option<f32>; 2],
    window: Option<[i32; 4]>,
    maskwindow: bool,
    windowdelta: [f32; 2],
    startv: [f32; 2],
    startrad: [f32; 2],
    startsint: [i32; 2],
    startsinlt: [i32; 2],
    bga: BgAction,
    visible: bool,
    enabled: bool,
    roundpos: bool,
    palfx: BgPalFx,
    /// rugen: `platform = N` (1-based `[Platform]` section) makes the
    /// element follow that platform when it moves or drops.
    platform: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CtrlType {
    Null,
    Anim,
    Visible,
    Enable,
    PalFx,
    PosSet,
    PosAdd,
    SinX,
    SinY,
    VelSet,
    VelAdd,
}

#[derive(Clone, Debug)]
struct BgCtrl {
    ty: CtrlType,
    elems: Vec<usize>,
    start: i32,
    end: i32,
    looptime: i32,
    x: Option<f32>,
    y: Option<f32>,
    value: [i32; 3],
    palfx: BgPalFx,
}

/// How a stage frames the fight screen vertically (`[Camera] cuthigh /
/// cutlow / overdrawlow`, `[StageInfo] zoffset`, in stage units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub zoffset: f32,
    pub height: f32,
    pub cut_high: f32,
    /// `None` = MUGEN's guess: `height - zoffset - height * 5 %`.
    pub cut_low: Option<f32>,
    pub overdraw_low: f32,
}

impl Default for Framing {
    fn default() -> Self {
        Self {
            zoffset: 210.0,
            height: 240.0,
            cut_high: 0.0,
            cut_low: None,
            overdraw_low: 0.0,
        }
    }
}

impl Framing {
    /// Distance (stage units, at zoom 1) from the top of a screen
    /// `screen_h` tall to the ground line. The stage's width always fills
    /// the screen's (MUGEN, Ikemen GO): a screen shorter than the stage
    /// cuts `cuthigh` / `cutlow` from its top and bottom in proportion, a
    /// taller one shows up to `overdrawlow` below the stage and the rest
    /// above (Ikemen GO `Camera.aspectcorrection`).
    pub fn ground_level(&self, screen_h: f32) -> f32 {
        let diff = self.height - screen_h;
        let correction = if diff < 0.0 {
            (diff + (-diff / 2.0).min(self.overdraw_low.max(0.0))).min(0.0)
        } else if diff > 0.0 {
            let low = self
                .cut_low
                .unwrap_or(self.height - self.zoffset - self.height * 0.05);
            let high = self.cut_high;
            if high + low <= 0.0 {
                diff
            } else {
                let top = high * diff / (high + low);
                if diff - top <= low { top } else { diff - low }
            }
        } else {
            0.0
        };
        self.zoffset - correction
    }
}

/// `[Shadow]` / `[Reflection]`: how fighters are mirrored on the floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloorFx {
    /// 0..=255 (0 = off).
    pub intensity: f32,
    pub xscale: f32,
    /// Positive: the image falls below the feet, flipped.
    pub yscale: f32,
    /// Positive shears the far end to the left.
    pub xshear: f32,
    /// `fade.range = top, bottom` (player y): gone above `top`, full below.
    pub fade: Option<[f32; 2]>,
    pub offset: [f32; 2],
}

impl FloorFx {
    fn read(def: &Def, sec: &str, intensity: f32, yscale: f32) -> Self {
        let s = def.section(sec);
        let f = |k: &str| s.and_then(|s| s.f32(k));
        Self {
            intensity: f("intensity").unwrap_or(intensity).clamp(0.0, 255.0),
            xscale: f("xscale").unwrap_or(1.0),
            yscale: f("yscale").unwrap_or(yscale),
            xshear: f("xshear").unwrap_or(0.0),
            fade: s
                .and_then(|s| s.f32s("fade.range"))
                .filter(|v| v.len() >= 2 && v[0] < v[1])
                .map(|v| [v[0], v[1]]),
            offset: s.map_or([0.0, 0.0], |s| pair(s, "offset", [0.0, 0.0])),
        }
    }
}

/// Where and how a background is looked at for one frame.
#[derive(Clone, Copy, Debug)]
pub struct BgView {
    /// Camera position in stage units (x with the shake; y multiplied by
    /// the zoom, as Ikemen GO keeps `Camera.Pos[1]`).
    pub pos: [f32; 2],
    /// Camera zoom (1 = the stage's own scale).
    pub zoom: f32,
    /// Screen size in stage units at zoom 1.
    pub screen: [f32; 2],
    /// A stage (ground line, zoom around it) rather than a menu backdrop.
    pub stage: bool,
    /// Screen distance from the top to the ground line at zoom 1
    /// ([`Framing::ground_level`]).
    pub ground_level: f32,
    pub shake_y: f32,
    /// Draw-space position of the screen's top-left corner.
    pub origin: [f32; 2],
    /// Draw units per screen unit.
    pub k: f32,
    /// Draw-space clip rectangle (a preview box).
    pub clip: Option<[f32; 4]>,
}

/// Where a stage preview lands: the screen (centred on the camera) maps
/// to `dst` = `[x, y, w, h]`, `k` target units per stage unit.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub k: f32,
    pub dst: [f32; 4],
}

pub struct StageVisual {
    pub framing: Framing,
    /// `[Shadow]` (MUGEN draws one by default: intensity 128, yscale 0.4).
    pub shadow: FloorFx,
    /// `[Reflection]` (off unless it has an intensity).
    pub reflection: FloorFx,
    sff: Sff,
    air: Air,
    elems: Vec<Elem>,
    ctrls: Vec<BgCtrl>,
    pub localcoord: [f32; 2],
    pub clear_color: [f64; 4],
    /// `[StageInfo] xscale / yscale`.
    scale: [f32; 2],
    /// `[StageInfo] hires`: elements are drawn at half size.
    hires: bool,
    textures: HashMap<(u16, u16), Option<Tex>>,
    shared: crate::sprites::Shared,
    palettes: Vec<PaletteId>,
    /// The same palettes with colour 0 opaque (`mask = 0`).
    opaque: Vec<Option<PaletteId>>,
    /// Ticks since the round started (`[BGCtrl] time`).
    time: i32,
    /// `[StageInfo] resetbg` (default on): back to the start each round.
    pub resetbg: bool,
    /// The round the elements were last reset for.
    round: i32,
    /// Offsets (stage units) of the hazard platforms, by platform index.
    platform_offsets: Vec<(usize, [f32; 2])>,
}

fn pair(s: &Section, k: &str, d: [f32; 2]) -> [f32; 2] {
    s.f32s(k).map_or(d, |v| {
        [
            v.first().copied().unwrap_or(d[0]),
            v.get(1).copied().unwrap_or(d[1]),
        ]
    })
}

fn ipair(s: &Section, k: &str, d: [i32; 2]) -> [i32; 2] {
    s.i32s(k).map_or(d, |v| {
        [
            v.first().copied().unwrap_or(d[0]),
            v.get(1).copied().unwrap_or(d[1]),
        ]
    })
}

fn flag(s: &Section, k: &str) -> Option<bool> {
    s.i32(k).map(|v| v != 0)
}

impl StageVisual {
    pub fn load(def_path: &Path) -> anyhow::Result<Self> {
        Self::load_prefixed(def_path, "bg", None)
    }

    /// Load `[<prefix>def]` + `[<prefix> ...]` sections. `default_sff` is used
    /// when the def section has no `spr=` (system.def backgrounds share system.sff).
    pub fn load_prefixed(
        def_path: &Path,
        prefix: &str,
        default_sff: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let def = Def::open(def_path).with_context(|| def_path.display().to_string())?;
        let dir = def_path.parent().unwrap_or(Path::new("."));
        let def_name = format!("{prefix}def");
        let bgdef = def
            .section(&def_name)
            .with_context(|| format!("no [{def_name}] in {}", def_path.display()))?;
        let sff_path = match bgdef.raw("spr").filter(|s| !s.trim().is_empty()) {
            Some(spr) => rugen_sim::chardata::resolve(dir, spr.trim())?,
            None => default_sff
                .map(Path::to_path_buf)
                .with_context(|| format!("[{def_name}] needs spr="))?,
        };
        // from the stage's .rgs when it is one (sprites decode on first draw)
        let sff = Sff::open(&sff_path, false).with_context(|| sff_path.display().to_string())?;
        let is_stage = prefix.eq_ignore_ascii_case("bg");
        // MUGEN (and Ikemen) clear to magenta under `debugbg = 1`, so holes
        // in the art show
        let debugbg = is_stage && bgdef.i32("debugbg").is_some_and(|v| v != 0);
        let clear_color = if debugbg {
            [1.0, 0.0, 1.0, 1.0]
        } else {
            bgdef
                .f32s("bgclearcolor")
                .filter(|v| v.len() >= 3)
                .map_or([0.0, 0.0, 0.0, 1.0], |v| {
                    [
                        (v[0] / 255.0) as f64,
                        (v[1] / 255.0) as f64,
                        (v[2] / 255.0) as f64,
                        1.0,
                    ]
                })
        };
        // animations live inside the def file itself
        let text = rugen_formats::text::decode(&rugen_formats::vfs::read(def_path)?);
        let air = Air::parse(&text);
        let info = def.section("stageinfo");
        let localcoord = info
            .or_else(|| def.section("info"))
            .and_then(|s| s.f32s("localcoord"))
            .filter(|v| v.len() >= 2 && v[0] > 0.0)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let hires = is_stage && info.and_then(|s| flag(s, "hires")).unwrap_or(false);
        let k = if hires { 2.0 } else { 1.0 };
        let scale = if is_stage {
            [
                info.and_then(|s| s.f32("xscale")).map_or(1.0, |v| v * k),
                info.and_then(|s| s.f32("yscale")).map_or(1.0, |v| v * k),
            ]
        } else {
            [1.0, 1.0]
        };
        let cam = def.section("camera");
        let cf = |k: &str| cam.and_then(|c| c.f32(k));
        let framing = Framing {
            zoffset: info
                .and_then(|s| s.f32("zoffset"))
                .unwrap_or(localcoord[1] * 0.875),
            height: localcoord[1],
            cut_high: cf("cuthigh").unwrap_or(0.0),
            cut_low: cf("cutlow"),
            overdraw_low: cf("overdrawlow").unwrap_or(0.0),
        };
        // stages older than MUGEN 1.0 move the camera in whole units
        let version = def
            .section("info")
            .and_then(|s| s.raw("mugenversion"))
            .unwrap_or("")
            .trim()
            .to_string();
        let roundpos = bgdef.i32("roundpos").map_or(
            is_stage && !(version.starts_with("1.") || version == "1"),
            |v| v != 0,
        );
        let mut elems: Vec<Elem> = Vec::new();
        for s in def.sections_named(prefix) {
            let Some(e) = Self::read_elem(s, &sff, &air, &elems, roundpos, is_stage) else {
                continue;
            };
            elems.push(e);
        }
        // a position-linked element's start is an offset from its link
        let mut link = 0;
        for i in 0..elems.len() {
            if elems[i].positionlink && i > 0 {
                let base = elems[link].start;
                elems[i].start[0] += base[0];
                elems[i].start[1] += base[1];
                elems[i].link = link;
            } else {
                link = i;
                elems[i].link = i;
            }
        }
        let ctrls = Self::read_ctrls(&def, &elems, prefix);
        let mut sv = Self {
            framing,
            shadow: FloorFx::read(&def, "shadow", 128.0, 0.4),
            reflection: FloorFx::read(&def, "reflection", 0.0, 1.0),
            sff,
            air,
            elems,
            ctrls,
            localcoord,
            clear_color,
            scale,
            hires,
            textures: HashMap::new(),
            shared: Default::default(),
            palettes: Vec::new(),
            opaque: Vec::new(),
            time: 0,
            resetbg: info.and_then(|s| flag(s, "resetbg")).unwrap_or(true),
            round: 0,
            platform_offsets: Vec::new(),
        };
        sv.reset();
        Ok(sv)
    }

    fn read_elem(
        s: &Section,
        sff: &Sff,
        air: &Air,
        prev: &[Elem],
        roundpos: bool,
        is_stage: bool,
    ) -> Option<Elem> {
        let ty = s.raw("type").map(|t| t.trim().to_ascii_lowercase())?;
        let mut kind = match ty.chars().next() {
            Some('n') => Kind::Normal,
            Some('a') => Kind::Anim,
            Some('p') => Kind::Parallax,
            // dummies only anchor position links and controllers; they
            // draw nothing, kept as an invisible element
            Some('d') => Kind::Normal,
            _ => return None,
        };
        let dummy = ty.starts_with('d');
        let mut action = -1;
        let mut sprite = [-1, -1];
        // menus mask colour 0 unless told otherwise (rugen's own
        // screenpacks rely on it); stages follow MUGEN: opaque unless
        // `mask = 1` or a `trans`
        let mut mask = !is_stage;
        if !dummy {
            let has_anim =
                (kind != Kind::Normal || s.raw("spriteno").is_none()) && s.i32("actionno").is_some();
            if has_anim {
                action = s.i32("actionno").unwrap_or(-1);
                if air.get(action).is_none() {
                    log::warn!("stage: [{} {}] action {action} not found", s.name, s.rest);
                    return None;
                }
                if kind == Kind::Normal {
                    kind = Kind::Anim;
                }
                mask = true;
            } else {
                sprite = ipair(s, "spriteno", [-1, -1]);
                if let Some(m) = s.i32("mask") {
                    mask = m != 0;
                }
            }
        }
        let positionlink = flag(s, "positionlink").unwrap_or(false);
        let link = prev.iter().rposition(|e| !e.positionlink);
        let (mut startv, mut delta) = ([0.0, 0.0], [1.0, 1.0]);
        if positionlink && let Some(l) = link {
            startv = prev[l].startv;
            delta = prev[l].delta;
        }
        if !positionlink {
            delta = pair(s, "delta", delta);
        }
        let trans = s
            .raw("trans")
            .map(|t| t.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let mut blend = match trans.as_str() {
            "add" => Some(Blend::Add { src: 255, dst: 255 }),
            "add1" => Some(Blend::Add { src: 255, dst: 128 }),
            "addalpha" => Some(Blend::Add { src: 255, dst: 0 }),
            "sub" | "subadd" => Some(Blend::Sub { src: 255, dst: 255 }),
            "none" | "default" => Some(Blend::Alpha),
            _ => None,
        };
        if matches!(blend, Some(Blend::Add { .. } | Blend::Sub { .. })) {
            mask = true;
            if let Some(a) = s.i32s("alpha").filter(|a| !a.is_empty()) {
                let c = |v: i32| v.clamp(0, 255) as u8;
                blend = match blend {
                    Some(Blend::Add { src, dst }) => Some(Blend::Add {
                        src: a.first().map_or(src, |v| c(*v)),
                        dst: a.get(1).map_or(dst, |v| c(*v)),
                    }),
                    Some(Blend::Sub { src, dst }) => Some(Blend::Sub {
                        src: a.first().map_or(src, |v| c(*v)),
                        dst: a.get(1).map_or(dst, |v| c(*v)),
                    }),
                    b => b,
                };
            }
        }
        let mut tile = [0, 0];
        if let Some(t) = s.i32s("tile").filter(|t| !t.is_empty()) {
            tile = [t[0], t.get(1).copied().unwrap_or(0)];
            if kind == Kind::Parallax {
                tile[1] = 0;
            }
            if tile[0] < 0 {
                tile[0] = i32::MAX;
            }
        }
        let mut spacing = [0, 0];
        let (mut width, mut rasterx, mut yscalestart, mut yscaledelta) =
            (None, [1.0, 1.0], 100.0, 0.0);
        if kind == Kind::Parallax {
            match s.f32s("width").filter(|w| w.len() >= 2) {
                Some(w) => width = Some([w[0].trunc(), w[1].trunc()]),
                None => rasterx = pair(s, "xscale", rasterx),
            }
            yscalestart = s.f32("yscalestart").unwrap_or(100.0);
            yscaledelta = s.f32("yscaledelta").unwrap_or(0.0);
        } else {
            spacing = ipair(s, "tilespacing", [0, 0]);
            if spacing[1] == 0
                && let Some(v) = s.i32s("tilespacing")
                && v.len() == 1
            {
                // MUGEN: one value spaces both ways
                spacing[1] = v[0];
            }
            if action < 0 {
                if let Some(sp) = u16::try_from(sprite[0])
                    .ok()
                    .zip(u16::try_from(sprite[1]).ok())
                    .and_then(|(g, n)| sff.get(g, n))
                {
                    spacing[0] += sp.width as i32;
                    spacing[1] += sp.height as i32;
                }
            } else {
                if spacing[0] == 0 {
                    tile[0] = 0;
                }
                if spacing[1] == 0 {
                    tile[1] = 0;
                }
            }
        }
        let window = s
            .i32s("window")
            .filter(|v| v.len() >= 4)
            .map(|v| [v[0], v[1], v[2], v[3]]);
        let maskwindow = s
            .i32s("maskwindow")
            .filter(|v| v.len() >= 4)
            .map(|v| [v[0], v[1], v[2], v[3]]);
        let zoomdelta = s.f32s("zoomdelta").filter(|v| !v.is_empty()).map(|v| {
            let x = v[0];
            [x, v.get(1).copied().unwrap_or(x)]
        });
        let zsd = s.f32s("zoomscaledelta").unwrap_or_default();
        let mut startrad = [0.0; 2];
        let mut startsinlt = [0; 2];
        let mut startsint = [0; 2];
        for (i, key) in ["sin.x", "sin.y"].iter().enumerate() {
            if let Some(v) = s.f32s(key).filter(|v| !v.is_empty()) {
                startrad[i] = v[0];
                if let Some(lt) = v.get(1) {
                    startsinlt[i] = *lt as i32;
                }
                if startsinlt[i] > 0
                    && let Some(ph) = v.get(2)
                {
                    let lt = startsinlt[i];
                    let mut st = ((ph * lt as f32 / 360.0) as i32) % lt;
                    if st < 0 {
                        st += lt;
                    }
                    startsint[i] = st;
                }
            }
        }
        if !positionlink {
            startv = pair(s, "velocity", startv);
        } else if s.raw("velocity").is_some() {
            startv = pair(s, "velocity", startv);
        }
        Some(Elem {
            kind,
            sprite,
            action,
            anim: AnimState::start(action),
            id: s.i32("id").unwrap_or(0),
            layer: s.i32("layerno").unwrap_or(0),
            start: pair(s, "start", [0.0, 0.0]),
            delta,
            positionlink,
            link: 0,
            mask,
            blend,
            tile,
            spacing,
            width,
            rasterx,
            yscalestart,
            yscaledelta,
            scalestart: pair(s, "scalestart", [1.0, 1.0]),
            scaledelta: pair(s, "scaledelta", [0.0, 0.0]),
            xshear: s.f32("xshear").unwrap_or(0.0),
            zoomdelta,
            zoomscaledelta: [zsd.first().copied(), zsd.get(1).copied()],
            window: maskwindow.or(window),
            maskwindow: maskwindow.is_some(),
            windowdelta: pair(s, "windowdelta", [0.0, 0.0]),
            startv,
            startrad,
            startsint,
            startsinlt,
            bga: BgAction::default(),
            visible: !dummy,
            enabled: true,
            roundpos: flag(s, "roundpos").unwrap_or(roundpos),
            palfx: BgPalFx::default(),
            platform: s
                .i32("platform")
                .filter(|n| *n >= 1)
                .map(|n| n as usize - 1),
        })
    }

    /// `[BGCtrlDef]` + `[BGCtrl]` groups (Ikemen `loadStage`).
    fn read_ctrls(def: &Def, elems: &[Elem], prefix: &str) -> Vec<BgCtrl> {
        let by_id = |ids: &[i32]| -> Vec<usize> {
            let mut out = Vec::new();
            for &id in ids {
                if id < 0 {
                    continue;
                }
                for (i, e) in elems.iter().enumerate() {
                    if e.id == id && !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
            out
        };
        let all: Vec<usize> = (0..elems.len()).collect();
        let ctrldef = format!("{prefix}ctrldef");
        let ctrl = format!("{prefix}ctrl");
        let mut out = Vec::new();
        let (mut group, mut looptime) = (all.clone(), -1);
        for s in &def.sections {
            let name = s.name.to_ascii_lowercase();
            if name == ctrldef {
                let ids = s.i32s("ctrlid").unwrap_or_default();
                group = if ids.is_empty() || (ids.len() == 1 && ids[0] == -1) {
                    all.clone()
                } else {
                    by_id(&ids)
                };
                looptime = s.i32("looptime").unwrap_or(-1);
            } else if name == ctrl {
                let ty = match s
                    .raw("type")
                    .map(|t| t.trim().to_ascii_lowercase())
                    .unwrap_or_default()
                    .as_str()
                {
                    "anim" => CtrlType::Anim,
                    "visible" => CtrlType::Visible,
                    "enable" => CtrlType::Enable,
                    "null" => CtrlType::Null,
                    "palfx" => CtrlType::PalFx,
                    "posset" => CtrlType::PosSet,
                    "posadd" => CtrlType::PosAdd,
                    "sinx" => CtrlType::SinX,
                    "siny" => CtrlType::SinY,
                    "velset" => CtrlType::VelSet,
                    "veladd" => CtrlType::VelAdd,
                    other => {
                        log::debug!("stage: [BGCtrl {}] type {other:?} not supported", s.rest);
                        continue;
                    }
                };
                let elems_of = match s.i32s("ctrlid").filter(|v| !v.is_empty()) {
                    Some(ids) if ids.len() > 1 || ids[0] != -1 => by_id(&ids),
                    Some(_) => all.clone(),
                    None => group.clone(),
                };
                let t = s.i32s("time").unwrap_or_default();
                let start = t.first().copied().unwrap_or(0);
                let end = t.get(1).copied().unwrap_or(start);
                let lt = t.get(2).copied().unwrap_or(looptime);
                let v = s.f32s("value").unwrap_or_default();
                let vi = s.i32s("value").unwrap_or_default();
                let xy = matches!(
                    ty,
                    CtrlType::PosSet | CtrlType::PosAdd | CtrlType::VelSet | CtrlType::VelAdd
                );
                let mut palfx = BgPalFx {
                    on: true,
                    mul: [256; 3],
                    color: 1.0,
                    ..Default::default()
                };
                if ty == CtrlType::PalFx {
                    let tri = |k: &str, d: [i32; 3]| {
                        s.i32s(k).map_or(d, |v| {
                            [
                                v.first().copied().unwrap_or(d[0]),
                                v.get(1).copied().unwrap_or(d[1]),
                                v.get(2).copied().unwrap_or(d[2]),
                            ]
                        })
                    };
                    palfx.add = tri("add", [0; 3]);
                    palfx.mul = tri("mul", [256; 3]);
                    if let Some(v) = s.i32s("sinadd").filter(|v| v.len() >= 4) {
                        let sg = if v[3] < 0 { -1 } else { 1 };
                        palfx.sinadd = [v[0] * sg, v[1] * sg, v[2] * sg, v[3] * sg];
                    }
                    palfx.invert = s.i32("invertall").is_some_and(|v| v != 0);
                    palfx.color = s.f32("color").map_or(1.0, |c| c / 256.0);
                }
                out.push(BgCtrl {
                    ty,
                    elems: elems_of,
                    start,
                    end,
                    looptime: lt,
                    x: if xy { s.f32("x") } else { v.first().copied() },
                    y: if xy { s.f32("y") } else { None },
                    value: [
                        vi.first().copied().unwrap_or(0),
                        vi.get(1).copied().unwrap_or(0),
                        vi.get(2).copied().unwrap_or(0),
                    ],
                    palfx,
                });
            }
        }
        out
    }

    /// A new round started: reset when the stage asks for it.
    pub fn round_started(&mut self, round: i32) {
        if round != self.round {
            self.round = round;
            if self.resetbg {
                self.reset();
            }
        }
    }

    /// Back to the round's start (`[StageInfo] resetbg`, and on load).
    pub fn reset(&mut self) {
        self.time = 0;
        for e in &mut self.elems {
            e.bga = BgAction {
                vel: e.startv,
                radius: e.startrad,
                sintime: e.startsint,
                sinlooptime: e.startsinlt,
                ..Default::default()
            };
            e.visible = e.visible || e.sprite[0] >= 0 || e.action >= 0;
            e.enabled = true;
            e.palfx = BgPalFx::default();
            e.anim = AnimState::start(e.action);
        }
    }

    pub fn init_gpu(&mut self, r: &mut Renderer) {
        self.palettes = self.sff.palettes.iter().map(|p| r.add_palette(p)).collect();
        if self.palettes.is_empty() {
            self.palettes.push(r.add_palette(&[[0; 4]; 256]));
        }
        self.opaque = vec![None; self.palettes.len()];
    }

    /// Where the moving / dropping platforms are now (stage units from
    /// their place in the def): elements with `platform = N` follow.
    pub fn set_platform_offsets(&mut self, offsets: Vec<(usize, [f32; 2])>) {
        self.platform_offsets = offsets;
    }

    /// Advance one tick (Ikemen `Stage.action` then `Stage.tick`):
    /// controllers, velocities, sine motion, animations.
    pub fn step(&mut self) {
        let ctrls = std::mem::take(&mut self.ctrls);
        for c in &ctrls {
            if c.start < 0 || (c.looptime >= 0 && c.start >= c.looptime) {
                continue;
            }
            let end = if c.looptime > 0 {
                c.end.min(c.looptime)
            } else {
                c.end
            };
            let active = self.time >= c.start
                && if c.looptime > 0 {
                    (self.time - c.start) % c.looptime <= end - c.start
                } else {
                    self.time <= end
                };
            if active {
                self.run_ctrl(c);
            }
        }
        self.ctrls = ctrls;
        let mut link = 0;
        for i in 0..self.elems.len() {
            let enabled = self.elems[i].enabled;
            self.elems[i].bga.action(enabled);
            if i > 0 && self.elems[i].positionlink {
                let so = self.elems[link].bga.sinoffset;
                self.elems[i].bga.offset[0] += so[0];
                self.elems[i].bga.offset[1] += so[1];
            } else {
                link = i;
            }
            let e = &mut self.elems[i];
            if e.enabled
                && e.action >= 0
                && let Some(a) = self.air.get(e.action)
            {
                e.anim.step(a);
            }
        }
        self.time += 1;
    }

    fn run_ctrl(&mut self, c: &BgCtrl) {
        for &i in &c.elems {
            let Some(e) = self.elems.get_mut(i) else {
                continue;
            };
            match c.ty {
                CtrlType::Null => {}
                CtrlType::Anim => {
                    if self.air.get(c.value[0]).is_some() {
                        e.action = c.value[0];
                        e.anim = AnimState::start(c.value[0]);
                        if e.kind == Kind::Normal {
                            e.kind = Kind::Anim;
                        }
                    }
                }
                CtrlType::Visible => e.visible = c.value[0] != 0,
                CtrlType::Enable => e.enabled = c.value[0] != 0,
                CtrlType::PalFx => e.palfx = c.palfx,
                CtrlType::PosSet => {
                    if let Some(x) = c.x {
                        e.bga.pos[0] = x;
                    }
                    if let Some(y) = c.y {
                        e.bga.pos[1] = y;
                    }
                }
                CtrlType::PosAdd => {
                    if let Some(x) = c.x {
                        e.bga.pos[0] += x;
                    }
                    if let Some(y) = c.y {
                        e.bga.pos[1] += y;
                    }
                }
                CtrlType::VelSet => {
                    if let Some(x) = c.x {
                        e.bga.vel[0] = x;
                    }
                    if let Some(y) = c.y {
                        e.bga.vel[1] = y;
                    }
                }
                CtrlType::VelAdd => {
                    if let Some(x) = c.x {
                        e.bga.vel[0] += x;
                    }
                    if let Some(y) = c.y {
                        e.bga.vel[1] += y;
                    }
                }
                CtrlType::SinX | CtrlType::SinY => {
                    let k = usize::from(c.ty == CtrlType::SinY);
                    let mut period = c.value[1];
                    if c.value[0] == 0 {
                        period = 0;
                    }
                    // the last value is a time offset, not a phase
                    let st = if period != 0 {
                        let ph = c.value[2] as f32 / period as f32;
                        let mut st = ((ph - ph.trunc()) * period as f32) as i32;
                        if st < 0 {
                            st += period.abs();
                        }
                        st
                    } else {
                        0
                    };
                    e.bga.radius[k] = c.x.unwrap_or(0.0);
                    e.bga.sinlooptime[k] = period;
                    e.bga.sintime[k] = st;
                }
            }
        }
    }

    fn palette(&mut self, r: &mut Renderer, slot: usize, opaque: bool) -> PaletteId {
        if self.palettes.is_empty() {
            self.init_gpu(r);
        }
        let slot = slot.min(self.palettes.len() - 1);
        if !opaque {
            return self.palettes[slot];
        }
        if let Some(p) = self.opaque[slot] {
            return p;
        }
        let mut colors = self
            .sff
            .palettes
            .get(slot)
            .cloned()
            .unwrap_or_else(|| vec![[0; 4]; 256]);
        if let Some(c) = colors.first_mut() {
            c[3] = 255;
        }
        let p = r.add_palette(&colors);
        self.opaque[slot] = Some(p);
        p
    }

    /// Texture, palette, axis and size of a sprite.
    fn sprite(
        &mut self,
        r: &mut Renderer,
        group: i32,
        number: i32,
        opaque: bool,
    ) -> Option<(TextureId, Option<PaletteId>, [f32; 2], [f32; 2])> {
        if self.palettes.is_empty() {
            self.init_gpu(r);
        }
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let spr = self.sff.get(key.0, key.1)?;
        let (w, h) = (spr.width as f32, spr.height as f32);
        let off = [spr.offset[0] as f32, spr.offset[1] as f32];
        let shared = &mut self.shared;
        let entry = self
            .textures
            .entry(key)
            .or_insert_with(|| Some(crate::sprites::upload_sprite(r, spr, shared)));
        let (tex, slot) = (*entry)?;
        let pal = slot.map(|p| self.palette(r, p, opaque));
        Some((tex, pal, off, [w, h]))
    }

    /// Draw elements on `layer` (0 = behind players, 1 = in front) of a
    /// menu backdrop: `cam` is the camera position, `screen_w` the screen's
    /// width in the backdrop's units (x = 0 at its centre, y = 0 at its top).
    pub fn draw(
        &mut self,
        r: &mut Renderer,
        layer: i32,
        cam: [f32; 2],
        scale: f32,
        screen_w: f32,
        tint: Tint,
    ) {
        let _ = scale;
        let view = BgView {
            pos: cam,
            zoom: 1.0,
            screen: [screen_w, self.localcoord[1]],
            stage: false,
            ground_level: self.framing.zoffset,
            shake_y: 0.0,
            origin: [-screen_w / 2.0, 0.0],
            k: 1.0,
            clip: None,
        };
        self.draw_bg(r, layer, &view, tint);
    }

    /// Draw `layer` into a box of another screen (a stage preview): the
    /// view's stage rectangle is mapped onto `view.dst` and clipped to it.
    pub fn draw_view(
        &mut self,
        r: &mut Renderer,
        layer: i32,
        cam: [f32; 2],
        scale: f32,
        view: View,
    ) {
        let _ = scale;
        let screen = [view.dst[2] / view.k, view.dst[3] / view.k];
        let bg = BgView {
            pos: cam,
            zoom: 1.0,
            screen,
            stage: true,
            ground_level: self.framing.ground_level(screen[1]),
            shake_y: 0.0,
            origin: [view.dst[0], view.dst[1]],
            k: view.k,
            clip: Some([
                view.dst[0],
                view.dst[1],
                view.dst[0] + view.dst[2],
                view.dst[1] + view.dst[3],
            ]),
        };
        self.draw_bg(r, layer, &bg, Tint::default());
    }

    /// Release the uploaded sprites and palettes (a preview that is replaced).
    pub fn free_gpu(&mut self, r: &mut Renderer) {
        let mut ids: std::collections::HashSet<TextureId> =
            self.textures.values().flatten().map(|t| t.0).collect();
        ids.extend(self.shared.values().copied());
        for id in ids {
            r.free_texture(id);
        }
        for p in self.palettes.drain(..) {
            r.free_palette(p);
        }
        for p in self.opaque.drain(..).flatten() {
            r.free_palette(p);
        }
        self.textures.clear();
        self.shared.clear();
    }

    /// Draw the elements of `layer` as seen through `view`.
    pub fn draw_bg(&mut self, r: &mut Renderer, layer: i32, view: &BgView, tint: Tint) {
        for idx in 0..self.elems.len() {
            let e = &self.elems[idx];
            if e.layer != layer || !e.visible || !e.enabled {
                continue;
            }
            // the element's current picture: its sprite or animation frame
            let frame = if e.action >= 0 {
                let Some(f) = self
                    .air
                    .get(e.action)
                    .and_then(|a| e.anim.frame(a))
                    .cloned()
                else {
                    continue;
                };
                Some(f)
            } else {
                None
            };
            let (group, number) = match &frame {
                Some(f) => (f.group, f.number),
                None => (e.sprite[0], e.sprite[1]),
            };
            let opaque = !e.mask;
            let Some((tex, pal, off, size)) = self.sprite(r, group, number, opaque) else {
                continue;
            };
            let e = self.elems[idx].clone();
            let blend = match (e.blend, &frame) {
                (Some(b), _) => b,
                (None, Some(f)) => crate::sprites::air_blend(f),
                (None, None) => Blend::Alpha,
            };
            let follow = e
                .platform
                .and_then(|k| self.platform_offsets.iter().find(|o| o.0 == k))
                .map_or([0.0, 0.0], |o| o.1);
            let quads = layout(&e, frame.as_ref(), off, size, view, self, follow);
            let tint = combine_tint(tint, &e.palfx, self.time);
            for q in quads {
                let corners = q.corners.map(|c| {
                    [
                        view.origin[0] + c[0] * view.k,
                        view.origin[1] + c[1] * view.k,
                    ]
                });
                let clip = match (q.window, view.clip) {
                    (Some(w), Some(c)) => {
                        let w = [
                            view.origin[0] + w[0] * view.k,
                            view.origin[1] + w[1] * view.k,
                            view.origin[0] + w[2] * view.k,
                            view.origin[1] + w[3] * view.k,
                        ];
                        Some([w[0].max(c[0]), w[1].max(c[1]), w[2].min(c[2]), w[3].min(c[3])])
                    }
                    (Some(w), None) => Some([
                        view.origin[0] + w[0] * view.k,
                        view.origin[1] + w[1] * view.k,
                        view.origin[0] + w[2] * view.k,
                        view.origin[1] + w[3] * view.k,
                    ]),
                    (None, c) => c,
                };
                if clip.is_some_and(|c| c[0] >= c[2] || c[1] >= c[3]) {
                    continue;
                }
                // a trapezoid maps its texture row by row
                let (top, bottom) = (q.corners[1][0] - q.corners[0][0], q.corners[2][0] - q.corners[3][0]);
                let strips = if (top - bottom).abs() > 0.5 {
                    let h = ((q.corners[3][1] - q.corners[0][1]) * view.k).abs();
                    (h / 3.0).ceil().clamp(1.0, 128.0) as u32
                } else {
                    1
                };
                r.draw_quad(&Quad {
                    tex,
                    pal,
                    corners,
                    color: [1.0; 4],
                    tint,
                    blend,
                    clip,
                    strips,
                });
            }
        }
    }
}

/// A background quad on the screen: corners top-left, top-right,
/// bottom-right, bottom-left, and the element's window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenQuad {
    corners: [[f32; 2]; 4],
    window: Option<[f32; 4]>,
}

/// The element's PalFX on top of the stage's (BGPalFX).
fn combine_tint(base: Tint, fx: &BgPalFx, time: i32) -> Tint {
    if !fx.on {
        return base;
    }
    let mut add = fx.add.map(|v| v as f32);
    if fx.sinadd[3] != 0 {
        let s = libm::sinf(2.0 * std::f32::consts::PI * time as f32 / fx.sinadd[3] as f32);
        for (a, amp) in add.iter_mut().zip(fx.sinadd) {
            *a += amp as f32 * s;
        }
    }
    let mut t = base;
    for k in 0..3 {
        t.add[k] += add[k] / 255.0;
        t.mul[k] *= fx.mul[k] as f32 / 256.0;
    }
    t.color *= fx.color.clamp(0.0, 1.0);
    t.invert ^= fx.invert;
    t
}

/// Ikemen GO's `drawTransformed` + `Animation.Draw` + `renderSpriteQuad`
/// for one element, in screen units (stage units at zoom 1, top-left
/// origin): every tile's corners.
fn layout(
    e: &Elem,
    frame: Option<&rugen_formats::air::Frame>,
    off: [f32; 2],
    size: [f32; 2],
    view: &BgView,
    sv: &StageVisual,
    follow: [f32; 2],
) -> Vec<ScreenQuad> {
    let bgscl = if sv.hires { 0.5 } else { 1.0 };
    let stgscl = sv.scale;
    let lscl = stgscl;
    let ds = view.zoom.max(0.01);
    let mut pos = view.pos;
    let (w, h) = (size[0].max(1.0), size[1].max(1.0));

    // parallax `width`: a trapezoid whose x axis is the sprite's centre
    let mut xscale = [1.0f32, 1.0];
    let mut xofs = 0.0;
    let mut scalestart_x = e.scalestart[0];
    let mut is_parallax = false;
    if e.kind == Kind::Parallax
        && let Some(wd) = e.width
    {
        xscale = [wd[0] / w, wd[1] / w];
        scalestart_x = scalestart_x.abs();
        xofs = scalestart_x * (-wd[0] / 2.0 + off[0] * xscale[0]);
        is_parallax = true;
    }
    let xras = (e.rasterx[1] - e.rasterx[0]) / e.rasterx[0];
    let xbs0 = xscale[1];
    let delta_x = (e.delta[0] * bgscl).max(0.0);
    let delta_y = (e.delta[1] * bgscl).max(0.0);
    let (zoom_x, zoom_y, y_zoom_delta, x_recip) = match e.zoomdelta {
        Some(zd) => {
            let zx = ds + (1.0 - ds) * (1.0 - zd[0]);
            let zy = ds + (1.0 - ds) * (1.0 - zd[1]);
            (zx, zy, zd[1], 1.0 + zd[0] * (1.0 / zx - 1.0))
        }
        None => (
            (ds + (1.0 - ds) * (1.0 - delta_x)).max(0.0),
            (ds + (1.0 - ds) * (1.0 - delta_y)).max(0.0),
            delta_y,
            1.0,
        ),
    };
    let _ = y_zoom_delta;
    let xs3 = e.zoomscaledelta[0].map_or(1.0, |z| (ds + (1.0 - ds) * (1.0 - z)) / zoom_x);
    let ys3 = e.zoomscaledelta[1].map_or(1.0, |z| (ds + (1.0 - ds) * (1.0 - z)) / zoom_y);

    // scroll against the camera
    let (mut x, y_scroll) = if e.roundpos {
        let px = libm::floorf(pos[0] / stgscl[0]);
        let py = libm::floorf(pos[1] / ds / stgscl[1]);
        pos = [libm::floorf(pos[0]), libm::floorf(pos[1])];
        (
            e.start[0] + xofs - px * e.delta[0] + e.bga.offset[0],
            py * e.delta[1],
        )
    } else {
        (
            e.start[0] + xofs - pos[0] / stgscl[0] * e.delta[0] + e.bga.offset[0],
            pos[1] / ds / stgscl[1] * e.delta[1],
        )
    };
    let mut y = e.start[1] - y_scroll + e.bga.offset[1];
    x *= bgscl;
    x += follow[0] / stgscl[0];
    y += follow[1] / stgscl[1];

    let mut ysdelta = e.yscalestart + e.yscaledelta * pos[1];
    if e.yscalestart * ysdelta < 0.0 {
        ysdelta = -ysdelta;
    }
    if ysdelta == 0.0 {
        ysdelta = 100.0;
    }
    let ys2 = e.scaledelta[1] * pos[1] * e.delta[1] * bgscl / ds / stgscl[1];
    let ys = (100.0 / ysdelta) * e.scalestart[1] * bgscl + ys2;
    let xs = e.scaledelta[0] * pos[0] * e.delta[0] * bgscl / stgscl[0];

    // the ground line stays put while the camera zooms around it
    let correction = view.ground_level - sv.framing.zoffset;
    if view.stage {
        let zoff = sv.framing.zoffset;
        y = y * bgscl + ((zoff - view.shake_y) / zoom_y - zoff) / stgscl[1];
        y += correction / (zoom_y * stgscl[1]);
    } else {
        y = y * bgscl + ((view.screen[1] - view.shake_y) / zoom_y - view.screen[1]) / stgscl[1];
    }
    let sclx = zoom_x * lscl[0];
    let scly = zoom_y * lscl[1];

    // window, on the screen
    let window = e.window.map(|r0| {
        let rect = [
            r0[0].min(r0[2]) as f32,
            r0[1].min(r0[3]) as f32,
            r0[0].max(r0[2]) as f32,
            r0[1].max(r0[3]) as f32,
        ];
        let (win_w, win_h) = if e.maskwindow {
            (rect[2] - rect[0], rect[3] - rect[1])
        } else {
            (rect[2] - rect[0] + 1.0, rect[3] - rect[1] + 1.0)
        };
        let wscl = [0, 1].map(|i| {
            let d = match e.zoomdelta {
                Some(zd) => zd[i].max(0.0),
                None => (e.windowdelta[i] * bgscl).max(0.0),
            };
            (ds + (1.0 - ds) * (1.0 - d)).max(0.0) * bgscl * lscl[i]
        });
        let mut wx = rect[0] - pos[0] / stgscl[0] * e.windowdelta[0] + view.screen[0] / 2.0 / sclx;
        if !e.maskwindow {
            wx -= view.screen[0] / 2.0 / lscl[0];
        }
        wx *= wscl[0];
        if !view.stage && wscl[0] == 1.0 {
            wx += (view.screen[0] - sv.localcoord[0]) / 2.0;
        }
        let mut wy = rect[1] - pos[1] / ds / stgscl[1] * e.windowdelta[1];
        if view.stage {
            let zoff = sv.framing.zoffset;
            wy += (zoff - view.shake_y) / scly - zoff / stgscl[1];
            wy += correction / scly;
        }
        wy *= wscl[1];
        wy -= view.shake_y;
        [wx, wy, wx + win_w * wscl[0], wy + win_h * wscl[1]]
    });

    // Animation.Draw
    let (hf, vf) = match frame {
        Some(f) => (
            if f.hflip { -1.0 } else { 1.0 } * f.xscale,
            if f.vflip { -1.0 } else { 1.0 } * f.yscale,
        ),
        None => (1.0, 1.0),
    };
    let x_shear = xras * x / ((ys * ys3).abs() * lscl[1] * h * e.scalestart[1]) * x_recip
        * e.scalestart[1]
        - e.xshear;
    let x_shear = if x_shear.is_finite() { x_shear } else { 0.0 };
    let xsoffset = -e.xshear * e.scalestart[1].signum() * (off[1] * scly);
    let draw_x = x - xsoffset;
    let draw_y = y;
    let xs_p = xscale[0] * bgscl * (scalestart_x + xs) * xs3;
    let xbs_p = xbs0 * bgscl * (scalestart_x + xs) * xs3;
    let ys_p = ys * ys3;
    let xts = xs_p * sclx * hf;
    let ysf = ys_p * scly * vf;
    let (fx, fy) = frame.map_or((0.0, 0.0), |f| (f.x as f32, f.y as f32));
    let mut rx = sclx * draw_x + xts * fx;
    let mut ry = scly * draw_y + ysf * fy;
    if xts < 0.0 {
        rx = -rx;
    }
    if ysf < 0.0 {
        ry = -ry;
    }
    rx = -rx + xts.abs() * off[0];
    ry = -ry + ysf.abs() * off[1];
    let xbs = sclx * xbs_p * hf;
    let rxadd = sclx * x_shear;
    let rcx = view.screen[0] / 2.0;
    // initRenderSpriteQuad
    let mut spacing = e.spacing.map(|v| v as f32);
    if e.tile[0] == 0 {
        spacing[0] = 0.0;
    } else if spacing[0] > 0.0 {
        spacing[0] -= w;
    }
    if e.tile[1] == 0 {
        spacing[1] = 0.0;
    } else if spacing[1] > 0.0 {
        spacing[1] -= h;
    }
    let _ = is_parallax;
    if xts >= 0.0 {
        rx = -rx;
    }
    rx += rcx;
    if ysf < 0.0 {
        ry = -ry;
    }
    // renderSpriteQuad, in Ikemen's y-up space (0 = top of the screen)
    let x1 = rx;
    let y1 = ry - ysf * h;
    let x2 = x1 + xbs * w;
    let x3 = rx + xts * w;
    let y3 = ry;
    let x4 = rx;
    let mut out = Vec::new();
    let screen_h = view.screen[1];
    let emit_row = |x1: f32, y1: f32, x2: f32, x3: f32, y3: f32, x4: f32, out: &mut Vec<ScreenQuad>| {
        emit_h_tiles(
            x1, y1, x2, x3, y3, x4, w, spacing[0], hf, rcx, e.tile[0], view.screen[0], window, out,
        );
    };
    let tiled = e.tile[0] != 0 || e.tile[1] != 0;
    if !tiled {
        let (mut x1, mut x2) = (x1, x2);
        if rxadd != 0.0 {
            x1 += rxadd * ysf * h;
            x2 = x1 + xbs * w;
        }
        emit_row(x1, y1, x2, x3, y3, x4, &mut out);
        return out;
    }
    let row_step = ysf * ((spacing[1] + h) / vf - h);
    // rows above (infinite y tiling): each starts at the previous row's top
    if e.tile[1] == 1 && xbs != 0.0 {
        let tx1 = x1 + rxadd * ysf * h;
        let (mut tx3, mut tx4, mut ty3) = (x3, x4, y3);
        let _ = tx1;
        for _ in 0..64 {
            let ny1 = ty3 + row_step;
            let (nx1, nx2) = (tx4, tx3);
            let nx3 = tx4 - rxadd * ysf * h + (xts / xbs) * (tx3 - tx4);
            let ny3 = ny1 + ysf * h;
            let nx4 = tx4 - rxadd * ysf * h;
            if (ny3 - ty3).abs() < 0.01 {
                break;
            }
            (tx3, tx4, ty3) = (nx3, nx4, ny3);
            // entirely above the screen (y-up: at or over 0)
            if ny1 >= 0.0 && ny3 >= 0.0 {
                break;
            }
            emit_row(nx1, ny1, nx2, nx3, ny3, nx4, &mut out);
        }
    }
    if e.tile[1] == 0 || xts != 0.0 {
        let (mut x1, mut y1, mut x3, mut y3, mut x4) = (x1 + rxadd * ysf * h, y1, x3, y3, x4);
        let mut x2 = x1 + xbs * w;
        let mut n = e.tile[1];
        for _ in 0..256 {
            if y1 <= -screen_h && y3 <= -screen_h && ysf * ((spacing[1] + h) / vf) > 0.0 {
                break;
            }
            emit_row(x1, y1, x2, x3, y3, x4, &mut out);
            if e.tile[1] != 1 && n != 0 {
                n -= 1;
            }
            if n == 0 {
                break;
            }
            let y4n = y1 - row_step;
            let (nx4, ny3) = (x1, y4n);
            let nx3 = x2;
            let nx2 = x1 + rxadd * ysf * h + (xbs / xts) * (x2 - x1);
            let ny1 = ny3 - ysf * h;
            let nx1 = x1 + rxadd * ysf * h;
            if (y1 - ny1).abs() < 0.01 {
                break;
            }
            (x1, y1, x2, x3, y3, x4) = (nx1, ny1, nx2, nx3, ny3, nx4);
        }
    }
    out
}

/// Ikemen `emitHTiles`: one row of tiles (corners in y-up screen space,
/// bottom x1..x2 at y1, top x4..x3 at y3), converging on the screen's
/// centre `rcx` when the top and bottom periods differ (parallax).
#[allow(clippy::too_many_arguments)]
fn emit_h_tiles(
    mut x1: f32,
    y1: f32,
    mut x2: f32,
    x3: f32,
    y3: f32,
    x4: f32,
    width: f32,
    xspacing: f32,
    xas: f32,
    rcx: f32,
    xflag: i32,
    xmax: f32,
    window: Option<[f32; 4]>,
    out: &mut Vec<ScreenQuad>,
) {
    let topdist = (x3 - x4) * (((xspacing + width) / xas) / width);
    let botdist = (x2 - x1) * (((xspacing + width) / xas) / width);
    if topdist.abs() >= 0.01 {
        let db = (x4 - rcx) * (botdist - topdist) / topdist.abs();
        x1 += db;
        x2 += db;
    }
    let (mut left, mut right) = (0i64, 1i64);
    let ceil = |v: f32| -> i64 {
        if v.is_finite() {
            libm::ceilf(v.clamp(-1.0e6, 1.0e6)) as i64
        } else {
            0
        }
    };
    let safe = |a: f32, b: f32| if b.abs() < 1.0e-4 { a / 1.0e-4 } else { a / b };
    if xflag != 0 {
        if topdist >= 0.01 {
            if x1 > x2 {
                left = 1 - ceil(safe(x4, topdist).max(safe(x1, botdist)));
                right = ceil(safe(xmax - x3, topdist).max(safe(xmax - x2, botdist)));
            } else {
                left = 1 - ceil(safe(x3, topdist).max(safe(x2, botdist)));
                right = ceil(safe(xmax - x4, topdist).max(safe(xmax - x1, botdist)));
            }
        } else if topdist <= -0.01 {
            if x1 > x2 {
                left = 1 - ceil(safe(xmax - x3, -topdist).max(safe(xmax - x2, -botdist)));
                right = ceil(safe(x4, -topdist).max(safe(x1, -botdist)));
            } else {
                left = 1 - ceil(safe(xmax - x4, -topdist).max(safe(xmax - x1, -botdist)));
                right = ceil(safe(x3, -topdist).max(safe(x2, -botdist)));
            }
        }
        if xflag != 1 {
            left = 0;
            right = right.min(i64::from(xflag.max(1)));
        }
    }
    // never more than a few hundred tiles, whatever the numbers
    let (left, right) = (left.max(-512), right.min(left.max(-512) + 1024));
    for n in left..right {
        let n = n as f32;
        let (b1, b2) = (x1 + n * botdist, x2 + n * botdist);
        let (t3, t4) = (x3 + n * topdist, x4 + n * topdist);
        // y-up to y-down
        out.push(ScreenQuad {
            corners: [[t4, -y3], [t3, -y3], [b2, -y1], [b1, -y1]],
            window,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(pos: [f32; 2], zoom: f32) -> BgView {
        BgView {
            pos,
            zoom,
            screen: [320.0, 240.0],
            stage: true,
            ground_level: 210.0,
            shake_y: 0.0,
            origin: [0.0, 0.0],
            k: 1.0,
            clip: None,
        }
    }

    fn visual(text: &str) -> StageVisual {
        let dir = std::env::temp_dir().join(format!("rugen-stagevis-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let def = dir.join(format!("t{}.def", text.len()));
        std::fs::write(&def, text).unwrap();
        // the SFF is never read for the layout maths: an empty one
        let mut sv = StageVisual {
            framing: Framing::default(),
            shadow: FloorFx::read(&Def::parse(""), "shadow", 128.0, 0.4),
            reflection: FloorFx::read(&Def::parse(""), "reflection", 0.0, 1.0),
            sff: Sff::default(),
            air: Air::parse(text),
            elems: Vec::new(),
            ctrls: Vec::new(),
            localcoord: [320.0, 240.0],
            clear_color: [0.0; 4],
            scale: [1.0, 1.0],
            hires: false,
            textures: HashMap::new(),
            shared: Default::default(),
            palettes: Vec::new(),
            opaque: Vec::new(),
            time: 0,
            resetbg: true,
            round: 0,
            platform_offsets: Vec::new(),
        };
        let d = Def::parse(text);
        for s in d.sections_named("bg") {
            let mut e = StageVisual::read_elem(s, &sv.sff, &sv.air, &sv.elems, false, true).unwrap();
            e.link = sv.elems.len();
            sv.elems.push(e);
        }
        sv.ctrls = StageVisual::read_ctrls(&d, &sv.elems, "bg");
        sv.reset();
        sv
    }

    fn quads(sv: &StageVisual, i: usize, v: &BgView, size: [f32; 2], off: [f32; 2]) -> Vec<ScreenQuad> {
        layout(&sv.elems[i], None, off, size, v, sv, [0.0, 0.0])
    }

    /// A plain element: `start` is from the top-centre of the screen, it
    /// scrolls by `delta` and keeps its axis.
    #[test]
    fn normal_elements_scroll_by_delta() {
        let sv = visual("[BG a]\ntype = normal\nspriteno = 0,0\nstart = 10,20\ndelta = 0.5,1\n");
        let q = quads(&sv, 0, &view([0.0, 0.0], 1.0), [100.0, 50.0], [5.0, 2.0]);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].corners[0], [160.0 + 10.0 - 5.0, 20.0 - 2.0]);
        assert_eq!(q[0].corners[2], [160.0 + 10.0 - 5.0 + 100.0, 20.0 - 2.0 + 50.0]);
        // the camera 100 to the right: the element moves 50 left
        let q2 = quads(&sv, 0, &view([100.0, 0.0], 1.0), [100.0, 50.0], [5.0, 2.0]);
        assert_eq!(q2[0].corners[0][0], q[0].corners[0][0] - 50.0);
    }

    /// Zooming out shrinks a layer by its delta: a far layer (delta 0)
    /// keeps its size, a delta-1 layer follows the camera's zoom, both
    /// around the ground line.
    #[test]
    fn layers_zoom_by_their_delta() {
        let sv = visual(
            "[BG far]\ntype = normal\nspriteno = 0,0\ndelta = 0,0\n\
             [BG near]\ntype = normal\nspriteno = 0,0\ndelta = 1,1\n",
        );
        let v = view([0.0, 0.0], 0.5);
        let far = quads(&sv, 0, &v, [100.0, 50.0], [0.0, 0.0]);
        let near = quads(&sv, 1, &v, [100.0, 50.0], [0.0, 0.0]);
        let width = |q: &ScreenQuad| q.corners[1][0] - q.corners[0][0];
        assert!((width(&far[0]) - 100.0).abs() < 1e-3);
        assert!((width(&near[0]) - 50.0).abs() < 1e-3);
        // y = 0 (the top at zoom 1) lands half way to the ground (210)
        assert!((near[0].corners[0][1] - 105.0).abs() < 1e-3, "{:?}", near[0]);
    }

    /// Tiling covers the screen with `tilespacing` gaps between copies.
    #[test]
    fn tiles_cover_the_screen_with_spacing() {
        let mut sv = visual("[BG t]\ntype = normal\nspriteno = 0,0\ntile = 1,0\ntilespacing = 10\n");
        // no SFF here: add the sprite's width as loading does
        sv.elems[0].spacing[0] += 40;
        let q = quads(&sv, 0, &view([37.0, 0.0], 1.0), [40.0, 20.0], [0.0, 0.0]);
        let mut xs: Vec<f32> = q.iter().map(|q| q.corners[0][0]).collect();
        xs.sort_by(f32::total_cmp);
        assert!(xs[0] <= 0.0 && *xs.last().unwrap() + 50.0 >= 320.0, "{xs:?}");
        for p in xs.windows(2) {
            assert!((p[1] - p[0] - 50.0).abs() < 1e-3);
        }
        // a finite count draws that many, to the right
        let sv = visual("[BG t]\ntype = normal\nspriteno = 0,0\ntile = 3,0\n");
        assert_eq!(quads(&sv, 0, &view([0.0, 0.0], 1.0), [40.0, 20.0], [0.0, 0.0]).len(), 3);
    }

    /// A parallax floor (`xscale = top, bottom`): the bottom edge scrolls
    /// `bottom / top` times as fast as the top, so the tiles shear with the
    /// camera and neighbours always meet.
    #[test]
    fn parallax_floors_shear_and_stay_seamless() {
        let sv = visual(
            "[BG floor]\ntype = parallax\nspriteno = 0,0\ndelta = 0.5,1\nxscale = 1,2\ntile = 1\nstart = 0,200\n",
        );
        for cam in [0.0, 120.0, -333.0] {
            let q = quads(&sv, 0, &view([cam, 0.0], 1.0), [64.0, 10.0], [0.0, 0.0]);
            let mut q = q.clone();
            q.sort_by(|a, b| a.corners[0][0].total_cmp(&b.corners[0][0]));
            for p in q.windows(2) {
                assert!((p[0].corners[1][0] - p[1].corners[0][0]).abs() < 1e-2, "top seam");
                assert!((p[0].corners[2][0] - p[1].corners[3][0]).abs() < 1e-2, "bottom seam");
            }
            // the tile under the screen's centre: bottom shift = xras * scroll
            let shear = |q: &ScreenQuad| q.corners[3][0] - q.corners[0][0];
            let s0 = shear(&q[0]);
            for t in &q {
                assert!((shear(t) - s0).abs() > -1.0, "sheared consistently");
            }
            if cam == 0.0 {
                assert!(s0.abs() < 1e-3, "no shear with the camera at 0");
            } else {
                assert!(s0.abs() > 1.0, "sheared at camera {cam}");
            }
        }
    }

    /// Velocity, sine motion and the controllers move elements tick by tick.
    #[test]
    fn velocity_sine_and_controllers() {
        let mut sv = visual(
            "[BG a]\ntype = normal\nspriteno = 0,0\nid = 3\nvelocity = 1,0\n\
             [BG b]\ntype = normal\nspriteno = 0,0\nid = 4\nsin.y = 10,40,90\n\
             [BGCtrlDef x]\nctrlid = 3\n\
             [BGCtrl stop]\ntype = VelSet\ntime = 5\nx = 0\n\
             [BGCtrl hide]\ntype = Visible\nctrlid = 4\ntime = 8\nvalue = 0\n",
        );
        for _ in 0..10 {
            sv.step();
        }
        assert_eq!(sv.elems[0].bga.pos[0], 5.0, "moved 5 ticks, then stopped");
        // sin.y phase 90 starts at the top of its swing
        assert!(!sv.elems[1].visible);
        let mut sv2 = visual("[BG b]\ntype = normal\nspriteno = 0,0\nsin.y = 10,40,90\n");
        sv2.step();
        assert!((sv2.elems[0].bga.offset[1] - 10.0).abs() < 1e-3);
    }

    /// The ground line on screens shorter and taller than the stage.
    #[test]
    fn framing_cuts_and_overdraws() {
        let f = Framing {
            zoffset: 200.0,
            height: 240.0,
            cut_high: 0.0,
            cut_low: None,
            overdraw_low: 10.0,
        };
        assert_eq!(f.ground_level(240.0), 200.0);
        // 4:3 stage on a 16:9 screen: 60 units to cut, cutlow = 240-200-12 = 28
        assert_eq!(f.ground_level(180.0), 200.0 - (60.0 - 28.0));
        // taller screen: 10 below the stage, the rest above
        assert_eq!(f.ground_level(300.0), 200.0 + 50.0);
    }
}
