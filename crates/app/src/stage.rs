//! MUGEN background definitions: `[BGdef]` + `[BG ...]` from a stage .def,
//! or `[TitleBGdef]` / `[SelectBGdef]` / `[VersusBGdef]` from system.def.
//! Visual only; the simulation keeps just the bounds in `rugen_sim::Stage`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use rugen_formats::air::Air;
use rugen_formats::def::Def;
use rugen_formats::sff::{Pixels, Sff};
use rugen_render::{Blend, Draw, PaletteId, Renderer, TextureId, Tint};
use rugen_sim::anim::AnimState;

/// Uploaded stage texture plus optional palette slot index.
type Tex = (TextureId, Option<usize>);
/// Resolved sprite: texture, palette, axis, scale.
type Placed = (TextureId, Option<PaletteId>, [f32; 2], [f32; 2]);

#[derive(Clone, Debug)]
enum Kind {
    Normal {
        group: i32,
        number: i32,
    },
    Anim {
        action: i32,
        state: AnimState,
    },
    Parallax {
        group: i32,
        number: i32,
        xscale: [f32; 2],
    },
}

#[derive(Clone, Debug)]
struct Elem {
    kind: Kind,
    layer: i32,
    start: [f32; 2],
    delta: [f32; 2],
    tile: [i32; 2],
    tilespacing: [i32; 2],
    velocity: [f32; 2],
    /// Accumulated velocity offset.
    scroll: [f32; 2],
    scale: [f32; 2],
    /// sin.x / sin.y: amplitude, period, phase.
    sin_x: Option<[f32; 3]>,
    sin_y: Option<[f32; 3]>,
    blend: Blend,
    /// `window = x1,y1, x2,y2` in screen coordinates (top-left origin).
    window: Option<[f32; 4]>,
}

pub struct StageVisual {
    sff: Sff,
    air: Air,
    elems: Vec<Elem>,
    pub localcoord: [f32; 2],
    pub clear_color: [f64; 4],
    textures: HashMap<(u16, u16), Option<Tex>>,
    palettes: Vec<PaletteId>,
    tick: i32,
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
        let sff = Sff::open(&sff_path, false).with_context(|| sff_path.display().to_string())?;
        let clear_color =
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
                });
        // animations live inside the def file itself
        let text = rugen_formats::text::decode(&std::fs::read(def_path)?);
        let air = Air::parse(&text);
        let localcoord = def
            .section("stageinfo")
            .or_else(|| def.section("info"))
            .and_then(|s| s.f32s("localcoord"))
            .filter(|v| v.len() >= 2 && v[0] > 0.0)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let mut elems = Vec::new();
        for s in def.sections_named(prefix) {
            let ty = s
                .raw("type")
                .map(|t| t.trim().to_ascii_lowercase())
                .unwrap_or_else(|| "normal".into());
            let pair = |k: &str, d: [f32; 2]| {
                s.f32s(k).map_or(d, |v| {
                    [
                        v.first().copied().unwrap_or(d[0]),
                        v.get(1).copied().unwrap_or(d[1]),
                    ]
                })
            };
            let ipair = |k: &str, d: [i32; 2]| {
                s.i32s(k).map_or(d, |v| {
                    [
                        v.first().copied().unwrap_or(d[0]),
                        v.get(1).copied().unwrap_or(d[1]),
                    ]
                })
            };
            let sprite = ipair("spriteno", [0, 0]);
            let kind = match ty.as_str() {
                "anim" => {
                    let action = s.i32("actionno").unwrap_or(0);
                    if air.get(action).is_none() {
                        log::warn!("stage: [BG {}] action {action} not found", s.rest);
                        continue;
                    }
                    Kind::Anim {
                        action,
                        state: AnimState::start(action),
                    }
                }
                "parallax" => {
                    let xs = pair("xscale", [1.0, 1.0]);
                    let xs = match s.f32s("width") {
                        Some(w) if w.len() >= 2 => {
                            let sw = sff
                                .get(sprite[0] as u16, sprite[1] as u16)
                                .map_or(1.0, |sp| sp.width as f32)
                                .max(1.0);
                            [w[0] / sw, w[1] / sw]
                        }
                        _ => xs,
                    };
                    Kind::Parallax {
                        group: sprite[0],
                        number: sprite[1],
                        xscale: xs,
                    }
                }
                "dummy" => continue,
                _ => Kind::Normal {
                    group: sprite[0],
                    number: sprite[1],
                },
            };
            let trans = s
                .raw("trans")
                .map(|t| t.trim().to_ascii_lowercase())
                .unwrap_or_default();
            let alpha = s.i32s("alpha").unwrap_or_default();
            let a = |i: usize, d: i32| alpha.get(i).copied().unwrap_or(d).clamp(0, 255) as u8;
            let blend = match trans.as_str() {
                "add" => Blend::Add { src: 255, dst: 255 },
                "add1" => Blend::Add { src: 255, dst: 128 },
                "addalpha" => Blend::Add {
                    src: a(0, 255),
                    dst: a(1, 0),
                },
                "sub" => Blend::Sub { src: 255, dst: 255 },
                _ => Blend::Alpha,
            };
            let sin = |k: &str| {
                s.f32s(k)
                    .filter(|v| v.len() >= 2)
                    .map(|v| [v[0], v[1].max(1.0), v.get(2).copied().unwrap_or(0.0)])
            };
            elems.push(Elem {
                kind,
                layer: s.i32("layerno").unwrap_or(0),
                start: pair("start", [0.0, 0.0]),
                delta: pair("delta", [1.0, 1.0]),
                tile: ipair("tile", [0, 0]),
                tilespacing: ipair("tilespacing", [0, 0]),
                velocity: pair("velocity", [0.0, 0.0]),
                scroll: [0.0, 0.0],
                scale: pair("scalestart", [1.0, 1.0]),
                sin_x: sin("sin.x"),
                sin_y: sin("sin.y"),
                blend,
                window: s
                    .f32s("window")
                    .filter(|v| v.len() >= 4)
                    .map(|v| [v[0], v[1], v[2], v[3]]),
            });
        }
        Ok(Self {
            sff,
            air,
            elems,
            localcoord,
            clear_color,
            textures: HashMap::new(),
            palettes: Vec::new(),
            tick: 0,
        })
    }

    pub fn init_gpu(&mut self, r: &mut Renderer) {
        self.palettes = self.sff.palettes.iter().map(|p| r.add_palette(p)).collect();
        if self.palettes.is_empty() {
            self.palettes.push(r.add_palette(&[[0; 4]; 256]));
        }
    }

    /// Advance animations and velocities one tick.
    pub fn step(&mut self) {
        self.tick += 1;
        for e in &mut self.elems {
            e.scroll[0] += e.velocity[0];
            e.scroll[1] += e.velocity[1];
            if let Kind::Anim { action, state } = &mut e.kind
                && let Some(a) = self.air.get(*action)
            {
                state.step(a);
            }
        }
    }

    fn sprite(&mut self, r: &mut Renderer, group: i32, number: i32) -> Option<Placed> {
        if self.palettes.is_empty() {
            self.init_gpu(r);
        }
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let spr = self.sff.get(key.0, key.1)?;
        let (w, h) = (spr.width as f32, spr.height as f32);
        let entry = self
            .textures
            .entry(key)
            .or_insert_with(|| match &spr.pixels {
                Pixels::Indexed(px) => Some((
                    r.add_indexed(spr.width as u32, spr.height as u32, px),
                    spr.palette,
                )),
                Pixels::Rgba(px) => {
                    Some((r.add_rgba(spr.width as u32, spr.height as u32, px), None))
                }
            });
        let (tex, pal) = (*entry)?;
        let pal = pal.map(|p| self.palettes[p.min(self.palettes.len() - 1)]);
        Some((
            tex,
            pal,
            [spr.offset[0] as f32, spr.offset[1] as f32],
            [w, h],
        ))
    }

    /// Draw elements on `layer` (0 = behind players, 1 = in front).
    /// `cam` is the camera position in stage units; `scale` = stage units per element unit.
    pub fn draw(
        &mut self,
        r: &mut Renderer,
        layer: i32,
        cam: [f32; 2],
        scale: f32,
        screen_w: f32,
        tint: Tint,
    ) {
        let elems = self.elems.clone();
        for (idx, e) in elems.iter().enumerate() {
            if e.layer != layer {
                continue;
            }
            let (group, number, frame_off, xscale, hflip) = match &e.kind {
                Kind::Normal { group, number } => (*group, *number, [0.0, 0.0], [1.0, 1.0], false),
                Kind::Parallax {
                    group,
                    number,
                    xscale,
                } => (*group, *number, [0.0, 0.0], [xscale[0], 1.0], false),
                Kind::Anim { action, state } => {
                    let Some(f) = self.air.get(*action).and_then(|a| state.frame(a)) else {
                        continue;
                    };
                    (
                        f.group,
                        f.number,
                        [f.x as f32, f.y as f32],
                        [f.xscale, f.yscale],
                        f.hflip,
                    )
                }
            };
            let Some((tex, pal, axis, size)) = self.sprite(r, group, number) else {
                continue;
            };
            let t = self.tick as f32;
            let sx = e.sin_x.map_or(0.0, |s| {
                s[0] * libm::sinf(2.0 * std::f32::consts::PI * (t + s[2]) / s[1])
            });
            let sy = e.sin_y.map_or(0.0, |s| {
                s[0] * libm::sinf(2.0 * std::f32::consts::PI * (t + s[2]) / s[1])
            });
            // element position in stage units: start - camera * delta, measured from the camera center
            let base_x = cam[0] + (e.start[0] + e.scroll[0] + sx + frame_off[0]) * scale
                - cam[0] * e.delta[0];
            let base_y =
                (e.start[1] + e.scroll[1] + sy + frame_off[1]) * scale - cam[1] * e.delta[1];
            let (w, h) = (
                size[0] * scale * xscale[0] * e.scale[0],
                size[1] * scale * xscale[1] * e.scale[1],
            );
            let step_x = w + e.tilespacing[0] as f32 * scale;
            let step_y = h + e.tilespacing[1] as f32 * scale;
            let color = [1.0, 1.0, 1.0, 1.0];
            // windows are relative to the screen, which is centred on the camera
            let clip = e.window.map(|w| {
                let left = cam[0] - self.localcoord[0] * scale / 2.0;
                [
                    left + w[0] * scale,
                    w[1] * scale,
                    left + (w[2] + 1.0) * scale,
                    (w[3] + 1.0) * scale,
                ]
            });
            // tiling: 1 = infinite, n = n copies
            let xs: Vec<f32> = if e.tile[0] == 1 && step_x > 0.0 {
                let left = cam[0] - screen_w / 2.0 - w;
                let right = cam[0] + screen_w / 2.0 + w;
                let first = base_x - axis[0] * scale * e.scale[0] * xscale[0];
                let k0 = libm::floorf((left - first) / step_x) as i32;
                let k1 = libm::ceilf((right - first) / step_x) as i32;
                (k0..=k1).map(|k| base_x + k as f32 * step_x).collect()
            } else if e.tile[0] > 1 {
                (0..e.tile[0]).map(|k| base_x + k as f32 * step_x).collect()
            } else {
                vec![base_x]
            };
            let ys: Vec<f32> = if e.tile[1] == 1 && step_y > 0.0 {
                (-4..=8).map(|k| base_y + k as f32 * step_y).collect()
            } else if e.tile[1] > 1 {
                (0..e.tile[1]).map(|k| base_y + k as f32 * step_y).collect()
            } else {
                vec![base_y]
            };
            for &x in &xs {
                for &y in &ys {
                    let mut d = Draw::new(tex, pal, [x, y], axis);
                    d.scale = [
                        scale * xscale[0] * e.scale[0],
                        scale * xscale[1] * e.scale[1],
                    ];
                    d.hflip = hflip;
                    d.color = color;
                    d.tint = tint;
                    d.blend = e.blend;
                    d.clip = clip;
                    r.draw(d);
                }
            }
            let _ = idx;
        }
    }
}
