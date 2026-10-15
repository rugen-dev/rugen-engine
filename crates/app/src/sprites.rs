//! GPU-side helpers shared by screens: an SFF uploaded lazily as textures,
//! AIR animations drawn from it, and MUGEN bitmap fonts.

use std::collections::HashMap;

use rugen_formats::air::{Air, Blend as AirBlend, Frame};
use rugen_formats::fnt::Font;
use rugen_formats::sff::{Pixels, Sff, Sprite};
use rugen_render::{Blend, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::anim::AnimState;

/// Texture plus the SFF palette slot it was authored with.
type Uploaded = (TextureId, Option<usize>);

/// Textures already uploaded for shared pixel data ([`Sprite::share_key`]):
/// linked / duplicate sprites of a content file upload once.
pub type Shared = HashMap<(usize, usize, u64), TextureId>;

/// Upload a sprite (decompressing it if it comes from a content file).
/// Returns the texture and, for indexed sprites, the SFF palette slot.
pub fn upload_sprite(r: &mut Renderer, spr: &Sprite, shared: &mut Shared) -> Uploaded {
    let (w, h) = (spr.width as u32, spr.height as u32);
    let indexed = match &spr.pixels {
        Pixels::Indexed(_) => true,
        Pixels::Rgba(_) => false,
        Pixels::Packed(p) => !p.is_rgba(),
    };
    let pal = if indexed { spr.palette } else { None };
    if let Some(k) = spr.share_key()
        && let Some(&tex) = shared.get(&k)
    {
        return (tex, pal);
    }
    let tex = match spr.decoded().as_ref() {
        Pixels::Indexed(px) => r.add_indexed(w, h, px),
        Pixels::Rgba(px) => r.add_rgba(w, h, px),
        Pixels::Packed(_) => r.white(),
    };
    if let Some(k) = spr.share_key() {
        shared.insert(k, tex);
    }
    (tex, pal)
}

/// Free every texture a sheet uploaded (`textures` plus the linked sprites
/// in `shared`, each once) and forget them; the renderer's white texture
/// (a packed sprite's stand-in) stays.
pub fn free_uploads(
    r: &mut Renderer,
    textures: &mut HashMap<(u16, u16), Option<Uploaded>>,
    shared: &mut Shared,
) {
    let white = r.white();
    let mut ids: std::collections::HashSet<TextureId> =
        textures.drain().flat_map(|(_, t)| t).map(|t| t.0).collect();
    ids.extend(shared.drain().map(|(_, t)| t));
    ids.remove(&white);
    r.free_textures(ids);
}

/// The blend an AIR frame asks for (`A`, `A1`, `S`, `AS<n>D<n>`).
pub fn air_blend(f: &Frame) -> Blend {
    match f.blend {
        AirBlend::None => Blend::Alpha,
        AirBlend::Add => Blend::Add {
            src: f.src_alpha,
            dst: f.dst_alpha,
        },
        AirBlend::Sub | AirBlend::SubAdd => Blend::Sub {
            src: f.src_alpha,
            dst: f.dst_alpha,
        },
    }
}

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
    shared: Shared,
    palettes: Vec<PaletteId>,
}

impl SpriteSheet {
    pub fn new(sff: Sff) -> Self {
        Self {
            sff,
            textures: HashMap::new(),
            shared: Shared::new(),
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
        let shared = &mut self.shared;
        let entry = self
            .textures
            .entry(key)
            .or_insert_with(|| Some(upload_sprite(r, spr, shared)));
        let (tex, pal) = (*entry)?;
        let pal = pal.map(|p| self.palettes[p.min(self.palettes.len() - 1)]);
        Some(Placed {
            tex,
            pal,
            axis: [spr.offset[0] as f32, spr.offset[1] as f32],
            size: [spr.width as f32, spr.height as f32],
        })
    }

    /// Give back the uploaded textures and palettes (the sheet goes away).
    pub fn free_gpu(&mut self, r: &mut Renderer) {
        free_uploads(r, &mut self.textures, &mut self.shared);
        r.free_palettes(self.palettes.drain(..));
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
        d.blend = air_blend(f);
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
    /// Window pixels per drawing unit: upright glyphs start on whole window
    /// pixels so a fractionally scaled pixel font keeps even pixel columns.
    pub snap: Option<[f32; 2]>,
    /// Font pixels the ink band sits below the cell centre when a label is
    /// centred on `size[1]` (see [`FontGpu::center_dy`]).
    ink_dy: f32,
}

impl FontGpu {
    pub fn new(font: Font) -> Self {
        // draw_xf puts the cell top at baseline - h + 1 + offset; a centred
        // label puts the baseline h / 2 under the centre
        let h = font.size[1] as f32;
        let ink_dy = font.ink_rows().map_or(0.0, |(t, b)| {
            1.0 + font.offset[1] as f32 - h / 2.0 + (t as f32 + b as f32 + 1.0) / 2.0
        });
        Self {
            font,
            glyphs: HashMap::new(),
            banks: Vec::new(),
            snap: None,
            ink_dy,
        }
    }

    /// How far below a box's centre the visible text lands when centred on
    /// the line height, at `scale`: subtract it to centre the ink.
    pub fn center_dy(&self, scale: f32) -> f32 {
        self.ink_dy * scale
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

    /// Give back the glyph textures and bank palettes (the font goes away).
    pub fn free_gpu(&mut self, r: &mut Renderer) {
        r.free_textures(self.glyphs.drain().map(|(_, t)| t));
        r.free_palettes(self.banks.drain(..));
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
        self.draw_xf(r, text, pos, bank, align, scale, clip, alpha, None);
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
        let xf = (angle != 0.0).then_some((angle, pivot));
        self.draw_xf(r, text, pos, bank, align, scale, None, 1.0, xf);
    }

    /// [`draw_clipped`](Self::draw_clipped) rotated by `xf = (degrees ccw, pivot)`;
    /// the clip applies in the unrotated frame.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_xf(
        &mut self,
        r: &mut Renderer,
        text: &str,
        pos: [f32; 2],
        bank: i32,
        align: i32,
        scale: f32,
        clip: Option<[f32; 4]>,
        alpha: f32,
        xf: Option<(f32, [f32; 2])>,
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
                let at = match self.snap {
                    Some([kx, ky]) if xf.is_none() && kx > 0.0 && ky > 0.0 => {
                        [libm::roundf(x * kx) / kx, libm::roundf(y * ky) / ky]
                    }
                    _ => [x, y],
                };
                let mut d = Draw::new(tex, Some(bank), at, [0.0, 0.0]);
                d.scale = [scale, scale];
                d.clip = clip;
                if let Some((angle, pivot)) = xf {
                    d.pos = pivot;
                    d.axis = [(pivot[0] - x) / scale, (pivot[1] - y) / scale];
                    d.angle = angle;
                    d.clip_local = true;
                }
                d.color = [1.0, 1.0, 1.0, alpha];
                r.draw(d);
            }
            x += (w + self.font.spacing[0] as f32) * scale;
        }
    }
}
