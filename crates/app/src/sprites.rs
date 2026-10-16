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
    if let Some(u) = shared_upload(spr, shared) {
        return u;
    }
    upload_pixels(r, spr, &spr.decoded(), shared)
}

/// The sprite's texture when its pixels were uploaded already (a linked or
/// duplicate sprite of a content file).
pub fn shared_upload(spr: &Sprite, shared: &Shared) -> Option<Uploaded> {
    let k = spr.share_key()?;
    shared.get(&k).map(|&tex| (tex, palette_slot(spr)))
}

/// Upload `pixels`, the decoded form of `spr` (decoded elsewhere: on the
/// decoder thread), and remember it for the sprites sharing its data.
pub fn upload_pixels(
    r: &mut Renderer,
    spr: &Sprite,
    pixels: &Pixels,
    shared: &mut Shared,
) -> Uploaded {
    let (w, h) = (spr.width as u32, spr.height as u32);
    let tex = match pixels {
        Pixels::Indexed(px) => r.add_indexed(w, h, px),
        Pixels::Rgba(px) => r.add_rgba(w, h, px),
        Pixels::Packed(_) => r.white(),
    };
    if let Some(k) = spr.share_key() {
        shared.insert(k, tex);
    }
    (tex, palette_slot(spr))
}

/// The SFF palette slot an indexed sprite is drawn with (`None`: RGBA).
fn palette_slot(spr: &Sprite) -> Option<usize> {
    let indexed = match &spr.pixels {
        Pixels::Indexed(_) => true,
        Pixels::Rgba(_) => false,
        Pixels::Packed(p) => !p.is_rgba(),
    };
    if indexed { spr.palette } else { None }
}

/// Bytes a sprite's texture takes (what an upload costs).
pub fn texture_bytes(spr: &Sprite) -> usize {
    let rgba = match &spr.pixels {
        Pixels::Indexed(_) => false,
        Pixels::Rgba(_) => true,
        Pixels::Packed(p) => p.is_rgba(),
    };
    spr.width as usize * spr.height as usize * if rgba { 4 } else { 1 }
}

/// A sprite of one sheet: `(sheet slot, group, number)`.
pub type SpriteKey = (usize, u16, u16);

/// What one frame may spend on sprite uploads ahead of their draw: bytes,
/// textures (a texture costs ~0.7 ms of driver time on GL whatever its
/// size, ~0.05 ms on Vulkan) and main-thread time.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub bytes: usize,
    pub textures: usize,
    pub deadline: std::time::Instant,
}

impl Budget {
    pub fn new(bytes: usize, textures: usize, time: std::time::Duration) -> Self {
        Self {
            bytes,
            textures,
            deadline: std::time::Instant::now() + time,
        }
    }

    /// Room for one more upload.
    pub fn left(&self) -> bool {
        self.bytes > 0 && self.textures > 0 && std::time::Instant::now() < self.deadline
    }

    /// One texture of `bytes` went up.
    pub fn spent(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_sub(bytes);
        self.textures = self.textures.saturating_sub(1);
    }
}

/// Sprites decoded off the main thread: packed sprites (content files) go
/// to a worker, their pixels come back for the main thread to upload under
/// a per-frame budget ([`Decoder::take`]). A sprite needed before its
/// pixels are back is decoded on the spot as before; the late result is
/// then dropped.
pub struct Decoder {
    jobs: std::sync::mpsc::Sender<(SpriteKey, rugen_formats::pack::Packed)>,
    done: std::sync::mpsc::Receiver<(SpriteKey, Pixels)>,
    /// Sent to the worker, not back yet.
    pending: std::collections::HashSet<SpriteKey>,
    /// Back from the worker, not uploaded yet (the budget ran out).
    ready: Vec<(SpriteKey, Pixels)>,
    /// Decoded bytes in flight (bounded so a long queue does not pile up
    /// memory ahead of the uploads).
    in_flight: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// Decoded bytes the worker may hold ahead of the uploads.
    pub const MAX_IN_FLIGHT: usize = 16 << 20;

    pub fn new() -> Self {
        let (jobs, job_rx) = std::sync::mpsc::channel::<(SpriteKey, rugen_formats::pack::Packed)>();
        let (done_tx, done) = std::sync::mpsc::channel();
        // the worker ends with the match (its sender goes); a receiver gone
        // first (the match dropped mid-decode) just ends the loop
        let _ = std::thread::Builder::new()
            .name("sprite decode".into())
            .spawn(move || {
                for (key, packed) in job_rx {
                    if done_tx.send((key, packed.decode())).is_err() {
                        break;
                    }
                }
            });
        Self {
            jobs,
            done,
            pending: Default::default(),
            ready: Vec::new(),
            in_flight: 0,
        }
    }

    /// True when `key` is being decoded or waits for its upload.
    pub fn knows(&self, key: SpriteKey) -> bool {
        self.pending.contains(&key) || self.ready.iter().any(|(k, _)| *k == key)
    }

    /// Decoded (or decoding) bytes not uploaded yet.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Hand a packed sprite to the worker. False when it is known already,
    /// the worker is gone or the sprite is not packed (nothing to decode:
    /// the caller uploads it as it is).
    pub fn request(&mut self, key: SpriteKey, spr: &Sprite) -> bool {
        let Pixels::Packed(p) = &spr.pixels else {
            return false;
        };
        if self.knows(key) {
            return true;
        }
        if self.in_flight >= Self::MAX_IN_FLIGHT || self.jobs.send((key, p.clone())).is_err() {
            return false;
        }
        self.in_flight += texture_bytes(spr);
        self.pending.insert(key);
        true
    }

    /// Pull what the worker finished into the ready list.
    fn poll(&mut self) {
        while let Ok((key, px)) = self.done.try_recv() {
            self.pending.remove(&key);
            self.ready.push((key, px));
        }
    }

    /// The decoded pixels of `key` when they are back (a sprite needed
    /// right now takes them out of turn).
    pub fn take_ready(&mut self, key: SpriteKey) -> Option<Pixels> {
        self.poll();
        let i = self.ready.iter().position(|(k, _)| *k == key)?;
        let (_, px) = self.ready.swap_remove(i);
        self.in_flight = self.in_flight.saturating_sub(px.len());
        Some(px)
    }

    /// The next decoded sprite waiting for its upload, oldest first.
    pub fn take(&mut self) -> Option<(SpriteKey, Pixels)> {
        self.poll();
        if self.ready.is_empty() {
            return None;
        }
        let (key, px) = self.ready.remove(0);
        self.in_flight = self.in_flight.saturating_sub(px.len());
        Some((key, px))
    }

    /// Forget what was decoded for sheet `slot` (its textures were freed:
    /// a hot reload replaced the data).
    pub fn forget(&mut self, slot: usize) {
        self.ready.retain(|(k, _)| k.0 != slot);
        self.pending.retain(|k| k.0 != slot);
    }
}

/// Byte length of decoded pixels.
trait PixelLen {
    fn len(&self) -> usize;
}

impl PixelLen for Pixels {
    fn len(&self) -> usize {
        match self {
            Pixels::Indexed(v) | Pixels::Rgba(v) => v.len(),
            Pixels::Packed(_) => 0,
        }
    }
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

    /// Upload sprites not on the GPU yet, in file order, as far as `budget`
    /// goes this call.
    pub fn warm(&mut self, r: &mut Renderer, budget: &mut Budget) {
        self.init_gpu(r);
        for (key, spr) in &self.sff.sprites {
            if !budget.left() {
                break;
            }
            if self.textures.contains_key(key) {
                continue;
            }
            let shared = &mut self.shared;
            let up = upload_sprite(r, spr, shared);
            self.textures.insert(*key, Some(up));
            budget.spent(texture_bytes(spr));
        }
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

/// Pack a font's glyphs onto one indexed texture page (rows of glyphs, a
/// transparent texel between them), returning the page and each glyph's
/// `[x, y, w, h]`; `None` when the font has no glyph or outgrows the GPU's
/// texture limit (the caller then uploads glyphs one by one).
fn glyph_atlas(r: &mut Renderer, font: &Font) -> Option<(TextureId, HashMap<char, [u32; 4]>)> {
    let mut glyphs: Vec<(char, &rugen_formats::fnt::Glyph)> = font
        .glyphs
        .iter()
        .filter(|(_, g)| g.width > 0 && g.height > 0)
        .map(|(c, g)| (*c, g))
        .collect();
    if glyphs.is_empty() {
        return None;
    }
    glyphs.sort_by_key(|(c, _)| *c);
    let max = r.max_texture_size();
    let area: u64 = glyphs
        .iter()
        .map(|(_, g)| (g.width as u64 + 1) * (g.height as u64 + 1))
        .sum();
    let width = ((area as f64).sqrt() as u32 + 1)
        .next_power_of_two()
        .clamp(64, max);
    let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
    let mut places = HashMap::with_capacity(glyphs.len());
    for (c, g) in &glyphs {
        let (w, h) = (g.width as u32, g.height as u32);
        if w + 1 > width {
            return None;
        }
        if x + w + 1 > width {
            x = 0;
            y += row;
            row = 0;
        }
        places.insert(*c, [x, y, w, h]);
        x += w + 1;
        row = row.max(h + 1);
    }
    let height = y + row;
    if height > max {
        return None;
    }
    let mut pixels = vec![0u8; (width * height) as usize];
    for (c, g) in &glyphs {
        let [px, py, w, h] = places[c];
        for line in 0..h {
            let Some(src) = g.pixels.get((line * w) as usize..((line + 1) * w) as usize) else {
                break;
            };
            let at = ((py + line) * width + px) as usize;
            pixels[at..at + w as usize].copy_from_slice(src);
        }
    }
    Some((r.add_indexed(width, height, &pixels), places))
}

/// A bitmap font uploaded to the GPU.
pub struct FontGpu {
    pub font: Font,
    /// Every glyph on one texture page, and where each sits (`[x, y, w, h]`):
    /// a line of text is one draw call instead of one per character.
    atlas: Option<(TextureId, HashMap<char, [u32; 4]>)>,
    /// One texture per glyph when the font does not fit a page.
    glyphs: HashMap<char, TextureId>,
    banks: Vec<PaletteId>,
    /// Window pixels per drawing unit: upright glyphs start on whole window
    /// pixels so a fractionally scaled pixel font keeps even pixel columns.
    pub snap: Option<[f32; 2]>,
    /// Font pixels the ink band sits below the cell centre when a label is
    /// centred on `size[1]` (see [`FontGpu::center_dy`]).
    ink_dy: f32,
    /// Height of the capitals' ink band in font pixels (the line height
    /// when the font has no capitals).
    cap_px: f32,
}

impl FontGpu {
    pub fn new(font: Font) -> Self {
        // draw_xf puts the cell top at baseline - h + 1 + offset; a centred
        // label puts the baseline h / 2 under the centre
        let h = font.size[1] as f32;
        let band = font.ink_rows();
        let ink_dy = band.map_or(0.0, |(t, b)| {
            1.0 + font.offset[1] as f32 - h / 2.0 + (t as f32 + b as f32 + 1.0) / 2.0
        });
        let cap_px = font
            .cap_height()
            .map_or_else(|| band.map_or(h, |(t, b)| (b - t + 1) as f32), f32::from)
            .max(1.0);
        Self {
            font,
            atlas: None,
            glyphs: HashMap::new(),
            banks: Vec::new(),
            snap: None,
            ink_dy,
            cap_px,
        }
    }

    /// Where the middle of the capitals lands below the line `pos.y` a
    /// draw call names, at `scale` (negative: above it).
    fn ink_centre(&self, scale: f32) -> f32 {
        (self.ink_dy - self.font.size[1] as f32 / 2.0) * scale
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
        self.atlas = glyph_atlas(r, &self.font);
    }

    pub fn width(&self, text: &str, scale: f32) -> f32 {
        self.font.text_width(text) as f32 * scale
    }

    /// Give back the glyph textures and bank palettes (the font goes away).
    pub fn free_gpu(&mut self, r: &mut Renderer) {
        let page = self.atlas.take().map(|(t, _)| t);
        r.free_textures(self.glyphs.drain().map(|(_, t)| t).chain(page));
        r.free_palettes(self.banks.drain(..));
    }

    /// The scale the Japanese face `ja` draws at next to this font at
    /// `scale`: its capitals at least as tall as this font's, in half
    /// steps (never below 1: the 16 px kana stand in for an 8 px face).
    pub fn companion_scale(&self, ja: &FontGpu, scale: f32) -> f32 {
        (libm::ceilf(self.cap_px * scale / ja.cap_px * 2.0 - 0.1) / 2.0).max(1.0)
    }

    /// The `pos.y` the Japanese face draws at so its capitals are centred
    /// on this font's capitals drawn at `y`.
    pub fn companion_y(&self, ja: &FontGpu, y: f32, scale: f32, ja_scale: f32) -> f32 {
        y + self.ink_centre(scale) - ja.ink_centre(ja_scale)
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
                let (tex, rect) = match &self.atlas {
                    Some((page, places)) => (*page, places.get(&c).copied()),
                    None => (
                        *self.glyphs.entry(c).or_insert_with(|| {
                            r.add_indexed(g.width as u32, g.height as u32, &g.pixels)
                        }),
                        None,
                    ),
                };
                let at = match self.snap {
                    Some([kx, ky]) if xf.is_none() && kx > 0.0 && ky > 0.0 => {
                        [libm::roundf(x * kx) / kx, libm::roundf(y * ky) / ky]
                    }
                    _ => [x, y],
                };
                let mut d = Draw::new(tex, Some(bank), at, [0.0, 0.0]);
                d.rect = rect;
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

/// Kana, kanji and CJK punctuation: drawn with the Japanese face.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x30FF | 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// `text` cut into runs that keep the theme's font (`false`) and runs of
/// Japanese characters (`true`).
pub fn script_runs(text: &str) -> Vec<(bool, &str)> {
    let mut out: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut cur: Option<bool> = None;
    for (i, c) in text.char_indices() {
        let ja = is_cjk(c);
        match cur {
            Some(k) if k == ja => {}
            Some(k) => {
                out.push((k, &text[start..i]));
                start = i;
                cur = Some(ja);
            }
            None => cur = Some(ja),
        }
    }
    if let Some(k) = cur {
        out.push((k, &text[start..]));
    }
    out
}

/// Width of `text` drawn with `font`, Japanese runs with `ja` at their
/// companion scale (see [`FontGpu::companion_scale`]).
pub fn mixed_width(
    fonts: &HashMap<i32, FontGpu>,
    font: i32,
    ja: i32,
    text: &str,
    scale: f32,
) -> f32 {
    let (Some(f), Some(j)) = (fonts.get(&font), fonts.get(&ja)) else {
        return fonts
            .get(&font)
            .map_or(text.len() as f32 * 8.0 * scale, |f| f.width(text, scale));
    };
    let js = f.companion_scale(j, scale);
    script_runs(text)
        .iter()
        .map(|(cjk, run)| {
            if *cjk {
                j.width(run, js)
            } else {
                f.width(run, scale)
            }
        })
        .sum()
}

/// [`FontGpu::draw_xf`] of `text` in `font`, its Japanese runs in `ja`,
/// sized and centred on the font's capitals. `snap`: window pixels per
/// unit (upright text starts on whole pixels).
#[allow(clippy::too_many_arguments)]
pub fn mixed_draw(
    fonts: &mut HashMap<i32, FontGpu>,
    r: &mut Renderer,
    font: i32,
    ja: i32,
    text: &str,
    pos: [f32; 2],
    bank: i32,
    align: i32,
    scale: f32,
    clip: Option<[f32; 4]>,
    alpha: f32,
    xf: Option<(f32, [f32; 2])>,
    snap: Option<[f32; 2]>,
) {
    if !fonts.contains_key(&ja) || !text.chars().any(is_cjk) {
        if let Some(f) = fonts.get_mut(&font) {
            f.snap = snap;
            f.draw_xf(r, text, pos, bank, align, scale, clip, alpha, xf);
            f.snap = None;
        }
        return;
    }
    let total = mixed_width(fonts, font, ja, text, scale);
    let (js, jy) = {
        let (Some(f), Some(j)) = (fonts.get(&font), fonts.get(&ja)) else {
            return;
        };
        let js = f.companion_scale(j, scale);
        (js, f.companion_y(j, pos[1], scale, js))
    };
    let mut x = match align {
        a if a > 0 => pos[0],
        0 => pos[0] - total / 2.0,
        _ => pos[0] - total,
    };
    for (cjk, run) in script_runs(text) {
        let (slot, s, y) = if cjk {
            (ja, js, jy)
        } else {
            (font, scale, pos[1])
        };
        let Some(f) = fonts.get_mut(&slot) else {
            continue;
        };
        f.snap = snap;
        f.draw_xf(r, run, [x, y], bank, 1, s, clip, alpha, xf);
        f.snap = None;
        x += f.width(run, s);
    }
}
