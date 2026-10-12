//! GPU-side helpers shared by screens: an SFF uploaded lazily as textures,
//! AIR animations drawn from it, and MUGEN bitmap fonts.

use std::collections::HashMap;

use rugen_formats::air::{Air, Blend as AirBlend, Frame};
use rugen_formats::fnt::Font;
use rugen_formats::sff::{Pixels, Sff};
use rugen_render::{Blend, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::anim::AnimState;

/// Texture plus the SFF palette slot it was authored with.
type Uploaded = (TextureId, Option<usize>);

/// Uploaded sprite: texture, palette, axis, size.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub tex: TextureId,
    pub pal: Option<PaletteId>,
    pub axis: [f32; 2],
    pub size: [f32; 2],
}

pub struct SpriteSheet {
    pub sff: Sff,
    textures: HashMap<(u16, u16), Option<Uploaded>>,
    palettes: Vec<PaletteId>,
}

impl SpriteSheet {
    pub fn new(sff: Sff) -> Self {
        Self {
            sff,
            textures: HashMap::new(),
            palettes: Vec::new(),
        }
    }

    pub fn init_gpu(&mut self, r: &mut Renderer) {
        if !self.palettes.is_empty() {
            return;
        }
        self.palettes = self.sff.palettes.iter().map(|p| r.add_palette(p)).collect();
        if self.palettes.is_empty() {
            self.palettes.push(r.add_palette(&[[0; 4]; 256]));
        }
    }

    pub fn get(&mut self, r: &mut Renderer, group: i32, number: i32) -> Option<Placed> {
        self.init_gpu(r);
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let spr = self.sff.get(key.0, key.1)?;
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
        Some(Placed {
            tex,
            pal,
            axis: [spr.offset[0] as f32, spr.offset[1] as f32],
            size: [spr.width as f32, spr.height as f32],
        })
    }

    /// Draw one AIR frame (sprite + frame offset/flip/blend/scale).
    pub fn draw_frame(
        &mut self,
        r: &mut Renderer,
        f: &Frame,
        pos: [f32; 2],
        scale: [f32; 2],
        facing: f32,
    ) {
        let Some(p) = self.get(r, f.group, f.number) else {
            return;
        };
        let mut d = Draw::new(
            p.tex,
            p.pal,
            [
                pos[0] + f.x as f32 * scale[0] * facing,
                pos[1] + f.y as f32 * scale[1],
            ],
            p.axis,
        );
        d.scale = [scale[0] * f.xscale, scale[1] * f.yscale];
        d.hflip = (facing < 0.0) != f.hflip;
        d.vflip = f.vflip;
        d.blend = match f.blend {
            AirBlend::None => Blend::Alpha,
            AirBlend::Add => Blend::Add {
                src: f.src_alpha,
                dst: f.dst_alpha,
            },
            AirBlend::Sub | AirBlend::SubAdd => Blend::Sub {
                src: f.src_alpha,
                dst: f.dst_alpha,
            },
        };
        r.draw(d);
    }
}

/// A running animation from an [`Air`] owned elsewhere.
#[derive(Clone, Debug)]
pub struct Anim {
    pub action: i32,
    pub state: AnimState,
}

impl Anim {
    pub fn new(action: i32) -> Self {
        Self {
            action,
            state: AnimState::start(action),
        }
    }
    pub fn step(&mut self, air: &Air) {
        if let Some(a) = air.get(self.action) {
            self.state.step(a);
        }
    }
    pub fn frame<'a>(&self, air: &'a Air) -> Option<&'a Frame> {
        air.get(self.action).and_then(|a| self.state.frame(a))
    }
}

/// `font = index, bank, align` as written in fight.def / system.def.
#[derive(Clone, Copy, Debug)]
pub struct FontRef {
    pub index: i32,
    pub bank: i32,
    /// 1 left, 0 centred, -1 right.
    pub align: i32,
}

impl FontRef {
    pub fn parse(s: &rugen_formats::def::Section, key: &str) -> Option<Self> {
        let v = s.i32s(key)?;
        let index = *v.first()?;
        if index < 0 {
            return None;
        }
        Some(Self {
            index,
            bank: v.get(1).copied().unwrap_or(0),
            align: v.get(2).copied().unwrap_or(0),
        })
    }
}

/// A bitmap font uploaded to the GPU.
pub struct FontGpu {
    pub font: Font,
    glyphs: HashMap<char, TextureId>,
    banks: Vec<PaletteId>,
}

impl FontGpu {
    pub fn new(font: Font) -> Self {
        Self {
            font,
            glyphs: HashMap::new(),
            banks: Vec::new(),
        }
    }

    fn init(&mut self, r: &mut Renderer) {
        if !self.banks.is_empty() {
            return;
        }
        self.banks = self.font.banks.iter().map(|p| r.add_palette(p)).collect();
        if self.banks.is_empty() {
            self.banks.push(r.add_palette(&[[255; 4]; 256]));
        }
    }

    pub fn width(&self, text: &str, scale: f32) -> f32 {
        self.font.text_width(text) as f32 * scale
    }

    /// Draw `text` with its bottom line at `pos.y` (MUGEN convention). `align`: 1 left, 0 centred, -1 right.
    pub fn draw(
        &mut self,
        r: &mut Renderer,
        text: &str,
        pos: [f32; 2],
        bank: i32,
        align: i32,
        scale: f32,
    ) {
        self.draw_clipped(r, text, pos, bank, align, scale, None, 1.0);
    }

    /// [`draw`](Self::draw) with a logical-space clip rectangle and alpha.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_clipped(
        &mut self,
        r: &mut Renderer,
        text: &str,
        pos: [f32; 2],
        bank: i32,
        align: i32,
        scale: f32,
        clip: Option<[f32; 4]>,
        alpha: f32,
    ) {
        self.draw_full(r, text, pos, bank, align, scale, clip, alpha, None);
    }

    /// [`draw`](Self::draw) rotated by `angle` degrees (counter-clockwise on
    /// screen) around `pivot`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_rotated(
        &mut self,
        r: &mut Renderer,
        text: &str,
        pos: [f32; 2],
        bank: i32,
        align: i32,
        scale: f32,
        angle: f32,
        pivot: [f32; 2],
    ) {
        let rot = (angle != 0.0).then_some((angle, pivot));
        self.draw_full(r, text, pos, bank, align, scale, None, 1.0, rot);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_full(
        &mut self,
        r: &mut Renderer,
        text: &str,
        pos: [f32; 2],
        bank: i32,
        align: i32,
        scale: f32,
        clip: Option<[f32; 4]>,
        alpha: f32,
        rot: Option<(f32, [f32; 2])>,
    ) {
        self.init(r);
        let bank = self.banks[(bank.max(0) as usize).min(self.banks.len() - 1)];
        let total = self.width(text, scale);
        let mut x = match align {
            a if a > 0 => pos[0],
            0 => pos[0] - total / 2.0,
            _ => pos[0] - total,
        } + self.font.offset[0] as f32 * scale;
        let y = pos[1] + (self.font.offset[1] - self.font.size[1] as i32 + 1) as f32 * scale;
        for c in text.chars() {
            let w = self.font.char_width(c) as f32;
            if let Some(g) = self.font.glyphs.get(&c)
                && g.width > 0
            {
                let tex = *self
                    .glyphs
                    .entry(c)
                    .or_insert_with(|| r.add_indexed(g.width as u32, g.height as u32, &g.pixels));
                let mut d = Draw::new(tex, Some(bank), [x, y], [0.0, 0.0]);
                if let Some((angle, pv)) = rot {
                    // same rotation as the renderer's: each glyph's corner turns
                    // around the pivot, then the glyph turns around its corner
                    let rad = angle * (std::f32::consts::PI / 180.0);
                    let (sn, cs) = (libm::sinf(rad), libm::cosf(rad));
                    let (dx, dy) = (x - pv[0], y - pv[1]);
                    d.pos = [pv[0] + dx * cs + dy * sn, pv[1] - dx * sn + dy * cs];
                    d.angle = angle;
                }
                d.scale = [scale, scale];
                d.clip = clip;
                d.color = [1.0, 1.0, 1.0, alpha];
                r.draw(d);
            }
            x += (w + self.font.spacing[0] as f32) * scale;
        }
    }
}
