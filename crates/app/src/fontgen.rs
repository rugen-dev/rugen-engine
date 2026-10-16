//! `rugen fontgen`: build MUGEN FNT v2 fonts (`.def` + SFF v2) from a TTF as
//! crisp pixel type in an arena-fighter style: bold condensed, thresholded
//! (no anti-aliasing), sheared one pixel every few rows for italics, with a
//! 1-px dark outline, a 1-px drop shadow and an optional 1-px coloured rim.
//! Colour banks: 0 white/red rim, 1 white/orange rim, 2 red, 3 orange,
//! 4 yellow, 5 blue, 6 grey, 7 plain white, 8 dark; 9-11 ink (paper
//! pages), 12-17 dojo (washi and lacquer).

use std::path::Path;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use rugen_formats::sff::{Sff, Version};

const IDX_FACE: u8 = 1;
const IDX_HI: u8 = 2;
const IDX_OUTLINE: u8 = 3;
const IDX_SHADOW: u8 = 4;
const IDX_RIM: u8 = 5;

#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Font pixel size (cell height ≈ size).
    pub size: u32,
    /// Italic: shear one pixel every `italic` rows (0 = upright).
    pub italic: u32,
    /// Horizontal squeeze (0.85 = condensed).
    pub condense: f32,
    /// 1-px coloured rim outside the outline.
    pub rim: bool,
    /// Lighter top row on each glyph (pixel highlight).
    pub highlight: bool,
}

/// Bank indices for the ink theme (paper backgrounds).
pub const BANK_INK: usize = 9;
pub const BANK_RED: usize = 10;
pub const BANK_PAPER: usize = 11;
/// Bank indices for the dojo theme (washi sheets on dark lacquer); screens
/// name them in TOML, the faces match `dojogen`'s palette.
pub const BANK_SUMI: usize = 12;
#[cfg_attr(not(test), allow(dead_code))]
pub const BANK_VERMILION: usize = 13;
pub const BANK_WASHI: usize = 14;
#[cfg_attr(not(test), allow(dead_code))]
pub const BANK_GOLD: usize = 15;
#[cfg_attr(not(test), allow(dead_code))]
pub const BANK_MUTED: usize = 16;
#[cfg_attr(not(test), allow(dead_code))]
pub const BANK_SUMI_SOFT: usize = 17;

/// `(face, highlight, outline, shadow, rim)` RGBA per bank.
pub const BANKS: [[[u8; 4]; 5]; 18] = [
    [
        [255, 255, 255, 255],
        [255, 255, 255, 255],
        [22, 10, 14, 255],
        [0, 0, 0, 150],
        [230, 40, 40, 200],
    ],
    [
        [255, 255, 255, 255],
        [255, 255, 255, 255],
        [24, 14, 8, 255],
        [0, 0, 0, 150],
        [255, 140, 30, 200],
    ],
    [
        [240, 50, 45, 255],
        [255, 150, 140, 255],
        [40, 6, 6, 255],
        [0, 0, 0, 150],
        [255, 120, 100, 160],
    ],
    [
        [255, 150, 30, 255],
        [255, 220, 150, 255],
        [50, 20, 4, 255],
        [0, 0, 0, 150],
        [255, 200, 120, 160],
    ],
    [
        [255, 225, 70, 255],
        [255, 250, 200, 255],
        [50, 40, 4, 255],
        [0, 0, 0, 150],
        [255, 240, 160, 160],
    ],
    [
        [90, 160, 255, 255],
        [190, 220, 255, 255],
        [6, 18, 50, 255],
        [0, 0, 0, 150],
        [140, 190, 255, 160],
    ],
    [
        [168, 172, 188, 255],
        [220, 222, 232, 255],
        [14, 14, 22, 255],
        [0, 0, 0, 150],
        [120, 130, 160, 120],
    ],
    [
        [255, 255, 255, 255],
        [255, 255, 255, 255],
        [18, 18, 26, 255],
        [0, 0, 0, 150],
        [0, 0, 0, 0],
    ],
    [
        [40, 42, 54, 255],
        [70, 72, 88, 255],
        [8, 8, 12, 255],
        [0, 0, 0, 120],
        [0, 0, 0, 0],
    ],
    // 9: ink on paper
    [
        [17, 17, 17, 255],
        [17, 17, 17, 255],
        [243, 239, 228, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 10: red stamp ink
    [
        [224, 32, 32, 255],
        [224, 32, 32, 255],
        [17, 17, 17, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 11: paper on ink slabs
    [
        [243, 239, 228, 255],
        [255, 255, 255, 255],
        [17, 17, 17, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 12: dojo sumi on washi
    [
        [29, 23, 20, 255],
        [29, 23, 20, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 13: dojo vermilion seal ink (on washi or lacquer)
    [
        [180, 48, 26, 255],
        [180, 48, 26, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 14: dojo washi on lacquer, wood and vermilion
    [
        [244, 234, 210, 255],
        [255, 250, 238, 255],
        [20, 13, 11, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 15: dojo brass on lacquer
    [
        [227, 179, 76, 255],
        [246, 217, 143, 255],
        [20, 13, 11, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 16: dojo muted text on lacquer
    [
        [184, 160, 126, 255],
        [184, 160, 126, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
    // 17: dojo soft sumi on washi
    [
        [99, 80, 61, 255],
        [99, 80, 61, 255],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
    ],
];

pub fn bank_palette(bank: usize) -> Vec<[u8; 4]> {
    let b = BANKS[bank.min(BANKS.len() - 1)];
    let mut pal = vec![[0u8; 4]; 256];
    pal[IDX_FACE as usize] = b[0];
    pal[IDX_HI as usize] = b[1];
    pal[IDX_OUTLINE as usize] = b[2];
    pal[IDX_SHADOW as usize] = b[3];
    pal[IDX_RIM as usize] = b[4];
    pal
}

/// One rendered glyph: indexed pixels, `w x h` where `h` is the cell height.
pub struct Glyph {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

pub struct Generator<'a> {
    font: FontRef<'a>,
    style: Style,
    cell_h: u32,
    ascent: f32,
    /// Red stamp ink (bank 10) without its dark outline: thin caption type
    /// stays sharp on paper instead of smudging into its own outline.
    plain_red: bool,
    /// Pixel scale handed to the rasteriser (`style.size` unless snapped).
    px: f32,
    /// Whole multiple of the face's own pixel grid (snapped fonts), else 0.
    mult: u32,
    /// Carry [`LATIN`] beyond ASCII (the room-code font does not).
    latin: bool,
    /// Characters beyond ASCII, the symbols and [`LATIN`] (the Japanese
    /// font's kana and kanji).
    extra: Vec<char>,
}

impl<'a> Generator<'a> {
    pub fn new(ttf: &'a [u8], style: Style) -> anyhow::Result<Self> {
        let font = FontRef::try_from_slice(ttf).map_err(|e| anyhow::anyhow!("font: {e}"))?;
        let scaled = font.as_scaled(PxScale::from(style.size as f32));
        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let cell_h = libm::ceilf(ascent - descent) as u32;
        Ok(Self {
            font,
            style,
            cell_h,
            ascent,
            plain_red: false,
            px: style.size as f32,
            mult: 0,
            latin: true,
            extra: Vec::new(),
        })
    }

    /// Also cut these characters (sorted, no repeats; ASCII is always in).
    pub fn with_chars(mut self, chars: impl IntoIterator<Item = char>) -> Self {
        let mut v: Vec<char> = chars.into_iter().filter(|c| *c as u32 > 126).collect();
        v.sort_unstable();
        v.dedup();
        self.extra = v;
        self
    }

    /// Lay the face on its own pixel grid of `unit` font units (a pixel
    /// font whose outlines are not whole multiples of the em, like Pixelify
    /// Sans): the scale becomes the whole multiple of the grid nearest to
    /// `style.size`, the baseline and each glyph's left edge sit on screen
    /// pixel edges, so every design pixel covers exactly `mult` x `mult`
    /// pixels and weight overshoots under half a pixel drop out.
    pub fn snapped(mut self, unit: f32) -> Self {
        let height = self.font.height_unscaled();
        let mult = libm::roundf(self.style.size as f32 * unit / height).max(1.0);
        let ppu = mult / unit;
        self.px = ppu * height;
        self.mult = mult as u32;
        let scaled = self.font.as_scaled(PxScale::from(self.px));
        // the grid's origin: the bottom of `H` (on the baseline or a
        // design overshoot below it) lands on a pixel edge
        let bottom = self
            .font
            .outline(self.font.glyph_id('H'))
            .map_or(0.0, |o| o.bounds.min.y);
        let edge = scaled.ascent() - bottom * ppu;
        self.ascent = libm::roundf(edge) + bottom * ppu;
        self.cell_h = libm::ceilf(self.ascent - scaled.descent()) as u32;
        self
    }

    /// ASCII and the symbols only (the room-code font: codes never hold
    /// accented letters).
    pub fn ascii_only(mut self) -> Self {
        self.latin = false;
        self
    }

    /// Drop the dark outline of the red ink bank (caption fonts).
    pub fn plain_red(mut self) -> Self {
        self.plain_red = true;
        self
    }

    /// Pixels added around the face: outline (1) + rim (1) + shadow (1, right/bottom).
    fn pad(&self) -> u32 {
        1 + u32::from(self.style.rim)
    }

    /// Extra columns every glyph carries for the italic shear.
    fn shear(&self) -> u32 {
        (self.cell_h - 1)
            .checked_div(self.style.italic)
            .unwrap_or(0)
    }

    /// Sprite width beyond the glyph's own advance (padding, shadow, shear).
    fn extra(&self) -> i32 {
        2 * self.pad() as i32 + 1 + self.shear() as i32
    }

    /// Window-free gap between two faces: one pixel of the font's own grid.
    fn gap(&self) -> i32 {
        if self.mult > 0 {
            return self.mult as i32;
        }
        (self.style.size / 10).max(1) as i32
    }

    /// Total sprite height.
    pub fn height(&self) -> u32 {
        self.cell_h + 2 * self.pad() + 1
    }

    /// Width used for spaces.
    pub fn space_width(&self) -> u32 {
        let scaled = self.font.as_scaled(PxScale::from(self.px));
        (scaled.h_advance(self.font.glyph_id(' ')) * self.style.condense).max(2.0) as u32
    }

    /// Thresholded, sheared, condensed mask of `c` at face resolution.
    fn mask(&self, c: char) -> Option<(u32, u32, Vec<bool>)> {
        if let Some(m) = self.symbol_mask(c) {
            return Some(m);
        }
        let id = self.glyph_id(c)?;
        let scale = PxScale {
            x: self.px * self.style.condense,
            y: self.px,
        };
        // snapped faces: the glyph's left edge on a pixel edge
        let x = match (self.mult, self.font.outline(id)) {
            (0, _) | (_, None) => 0.0,
            (_, Some(o)) => {
                let left = o.bounds.min.x * self.font.as_scaled(scale).h_scale_factor();
                libm::floorf(left) - left
            }
        };
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(x, self.ascent));
        let outline = self.font.outline_glyph(glyph)?;
        let b = outline.px_bounds();
        let bw = libm::ceilf(b.width()) as u32 + 1;
        let h = self.cell_h;
        let y0 = b.min.y.max(0.0) as u32;
        let mut cov = vec![false; (bw * h) as usize];
        outline.draw(|x, y, v| {
            let yy = y + y0;
            if yy < h && x < bw && v >= 0.45 {
                cov[(yy * bw + x) as usize] = true;
            }
        });
        let (cov, bw) = if c == '0' && self.style.italic == 0 {
            slash_zero(cov, bw, h)
        } else {
            (cov, bw)
        };
        let shear = (h - 1).checked_div(self.style.italic).unwrap_or(0);
        let w = bw + shear;
        let mut out = vec![false; (w * h) as usize];
        for y in 0..h {
            let dx = (h - 1 - y).checked_div(self.style.italic).unwrap_or(0);
            for x in 0..bw {
                if cov[(y * bw + x) as usize] {
                    out[(y * w + x + dx) as usize] = true;
                }
            }
        }
        Some((w, h, out))
    }

    /// The face's glyph for `c`, or `None` (never `.notdef`'s box). Romanian
    /// comma-below letters fall back to the cedilla forms older faces
    /// draw for them.
    fn glyph_id(&self, c: char) -> Option<ab_glyph::GlyphId> {
        let id = self.font.glyph_id(c);
        if id.0 != 0 {
            return Some(id);
        }
        let alt = match c {
            'Ș' => 'Ş',
            'ș' => 'ş',
            'Ț' => 'Ţ',
            'ț' => 'ţ',
            _ => return None,
        };
        let id = self.font.glyph_id(alt);
        (id.0 != 0).then_some(id)
    }

    /// Hand-drawn 8x8 symbols (arrows, triangles, check, warning, dot) the
    /// pixel TTFs lack, scaled to the font size and sat on the baseline.
    fn symbol_mask(&self, c: char) -> Option<(u32, u32, Vec<bool>)> {
        let rows = symbol_rows(c)?;
        // the 7 inked rows span the capital height, bottom on the baseline
        let (cap_top, cap_h) = self.cap_rows();
        let h = self.cell_h;
        let w = libm::roundf(8.0 * cap_h as f32 / 7.0).max(1.0) as u32;
        let mut out = vec![false; (w * h) as usize];
        for y in 0..cap_h {
            let ry = (y * 7 / cap_h.max(1)) as usize;
            let row: Vec<char> = rows[ry].chars().collect();
            for x in 0..w {
                let rx = ((x * 8) / w).min(7) as usize;
                let yy = cap_top + y;
                if row[rx] == 'X' && yy < h {
                    out[(yy * w + x) as usize] = true;
                }
            }
        }
        Some((w, h, out))
    }

    /// First row and height of the capital `H` in the face mask.
    fn cap_rows(&self) -> (u32, u32) {
        let fallback = {
            let k = (self.style.size / 8).max(1);
            let top = (libm::roundf(self.ascent) as i32 - 7 * k as i32).max(0) as u32;
            (top, 7 * k)
        };
        if self.mult > 0 {
            // snapped: the inked rows of the thresholded `H` (the outline's
            // bounds carry the weight overshoot)
            if let Some((w, h, m)) = self.mask('H') {
                let rows: Vec<u32> = (0..h)
                    .filter(|&y| (0..w).any(|x| m[(y * w + x) as usize]))
                    .collect();
                if let (Some(&t), Some(&b)) = (rows.first(), rows.last()) {
                    return (t, b - t + 1);
                }
            }
            return fallback;
        }
        let id = self.font.glyph_id('H');
        let scale = PxScale::from(self.px);
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(0.0, self.ascent));
        let Some(outline) = self.font.outline_glyph(glyph) else {
            return fallback;
        };
        let b = outline.px_bounds();
        let top = libm::roundf(b.min.y.max(0.0)) as u32;
        let h = libm::roundf(b.height()) as u32;
        if h == 0 { fallback } else { (top, h) }
    }

    /// Render one glyph: shadow, rim, outline, face, highlight.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        let (mw, mh, mask) = self.mask(c)?;
        if !mask.iter().any(|&m| m) {
            return None;
        }
        let pad = self.pad();
        let (w, h) = (mw + 2 * pad + 1, mh + 2 * pad + 1);
        let at = |x: i32, y: i32| -> bool {
            x >= 0
                && y >= 0
                && x < mw as i32
                && y < mh as i32
                && mask[(y as u32 * mw + x as u32) as usize]
        };
        let near = |x: i32, y: i32, r: i32| -> bool {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() + dy.abs() <= r && at(x + dx, y + dy) {
                        return true;
                    }
                }
            }
            false
        };
        let mut px = vec![0u8; (w * h) as usize];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (fx, fy) = (x - pad as i32, y - pad as i32);
                let idx = if at(fx, fy) {
                    if self.style.highlight && !at(fx, fy - 1) {
                        IDX_HI
                    } else {
                        IDX_FACE
                    }
                } else if near(fx, fy, 1) {
                    IDX_OUTLINE
                } else if near(fx - 1, fy - 1, 1) {
                    IDX_SHADOW
                } else if self.style.rim && near(fx, fy, 2) {
                    IDX_RIM
                } else {
                    0
                };
                px[(y as u32 * w + x as u32) as usize] = idx;
            }
        }
        Some(Glyph { w, h, px })
    }

    /// Build the SFF (sprites `(0, code)`, palettes `(0, bank)`) and the `.def` text.
    pub fn build(&self, name: &str, sff_name: &str) -> (String, Sff) {
        let mut sff = Sff {
            version: Some(Version::V2 { patch: 1 }),
            ..Default::default()
        };
        for b in 0..BANKS.len() {
            let mut pal = bank_palette(b);
            if self.plain_red && b == BANK_RED {
                pal[IDX_OUTLINE as usize] = [0, 0, 0, 0];
            }
            sff.push_palette((0, b as u16), pal);
        }
        let latin = LATIN
            .iter()
            .flat_map(|r| r.clone())
            .filter(|_| self.latin)
            .filter(|c| !SYMBOLS.contains(*c));
        let extra = self
            .extra
            .iter()
            .filter(|c| !SYMBOLS.contains(**c))
            .filter(|c| !self.latin || !LATIN.iter().any(|r| r.contains(c)))
            .filter_map(|c| u16::try_from(*c as u32).ok());
        let codes = (33u16..=126)
            .chain(SYMBOLS.chars().map(|c| c as u16))
            .chain(latin.map(|c| c as u16))
            .chain(extra);
        for code in codes {
            let c = char::from_u32(code as u32).unwrap_or(' ');
            if let Some(g) = self.glyph(c) {
                sff.push_indexed((0, code), g.w as u16, g.h as u16, [0, 0], 0, g.px);
            }
        }
        // sprites carry padding, the shadow column and the shear: pull the
        // advance back so faces sit one font pixel apart (2 px for fonts drawn
        // at twice a 10 px grid, so letters never touch)
        let spacing = self.gap() - self.extra();
        let def = format!(
            "; generated by `rugen fontgen`: pixel italic sans, outline + shadow; banks 0 white/red 1 white/orange 2 red 3 orange 4 yellow 5 blue 6 grey 7 white 8 dark, 9-11 ink, 12-17 dojo\n\
             [FNT v2]\nfntversion = 2,00\nname = \"{name}\"\n\n\
             [Def]\nType = bitmap\nBankType = palette\nSize = {},{}\nSpacing = {spacing},0\nOffset = {},{}\nFile = {sff_name}\n",
            self.space_width().max(self.style.size * 3 / 10) as i32 - spacing,
            self.height(),
            -(self.pad() as i32),
            self.pad() as i32 + 1
        );
        (def, sff)
    }

    /// Render a line of text as straight RGBA with the given bank (for logos).
    pub fn text_rgba(&self, text: &str, bank: usize, spacing: i32) -> (u32, u32, Vec<u8>) {
        let pal = bank_palette(bank);
        let glyphs: Vec<Option<Glyph>> = text.chars().map(|c| self.glyph(c)).collect();
        let space = self.space_width();
        let extra = self.extra();
        let spacing = spacing - extra;
        let mut w: i32 = 0;
        for (i, g) in glyphs.iter().enumerate() {
            if i > 0 {
                w += spacing;
            }
            w += g.as_ref().map_or(space as i32, |g| g.w as i32);
        }
        let (w, h) = ((w + extra).max(1) as u32, self.height());
        let mut out = vec![0u8; (w * h * 4) as usize];
        // two passes so outlines never cover a neighbour's face
        for pass in 0..2 {
            let mut x0: i32 = 0;
            for (i, g) in glyphs.iter().enumerate() {
                if i > 0 {
                    x0 += spacing;
                }
                let Some(g) = g else {
                    x0 += space as i32;
                    continue;
                };
                for y in 0..g.h {
                    for x in 0..g.w {
                        let v = g.px[(y * g.w + x) as usize];
                        let is_face = matches!(v, IDX_FACE | IDX_HI);
                        if v == 0 || (pass == 0) == is_face {
                            continue;
                        }
                        let (dx, dy) = (x0 + x as i32, y as i32);
                        if dx < 0 || dx >= w as i32 || dy >= h as i32 {
                            continue;
                        }
                        let o = ((dy as u32 * w + dx as u32) * 4) as usize;
                        out[o..o + 4].copy_from_slice(&pal[v as usize]);
                    }
                }
                x0 += g.w as i32;
            }
        }
        (w, h, out)
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The font's own pixel size inside a rendered mask (2 for a 10 px pixel
/// face drawn at 20 px): the GCD of every run of ink and of gaps between.
fn grid_unit(mask: &[bool], w: u32, h: u32) -> u32 {
    let mut g = 0;
    let mut runs = |cells: &mut dyn Iterator<Item = bool>| {
        let mut prev = None;
        let mut len = 0;
        let mut seen_ink = false;
        for c in cells {
            if Some(c) == prev {
                len += 1;
                continue;
            }
            // count interior runs only (leading blanks depend on the bounds)
            if prev.is_some() && seen_ink {
                g = gcd(g, len);
            }
            seen_ink |= c || prev == Some(true);
            prev = Some(c);
            len = 1;
        }
        if prev == Some(true) {
            g = gcd(g, len);
        }
    };
    for y in 0..h {
        runs(&mut (0..w).map(|x| mask[(y * w + x) as usize]));
    }
    for x in 0..w {
        runs(&mut (0..h).map(|y| mask[(y * w + x) as usize]));
    }
    g.max(1)
}

/// Draw a slash inside the counter of a zero so it never reads as `O`
/// (works on the font's own pixel grid). A counter too narrow for a slash
/// gets a new zero, four font pixels wide, slashed. Returns the mask and
/// its width.
fn slash_zero(mut mask: Vec<bool>, w: u32, h: u32) -> (Vec<bool>, u32) {
    let u = grid_unit(&mask, w, h);
    let ink: Vec<(u32, u32)> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| mask[(y * w + x) as usize])
        .collect();
    let (Some(x0), Some(y0)) = (ink.iter().map(|p| p.0).min(), ink.iter().map(|p| p.1).min())
    else {
        return (mask, w);
    };
    let x1 = ink.iter().map(|p| p.0).max().unwrap_or(x0);
    let y1 = ink.iter().map(|p| p.1).max().unwrap_or(y0);
    let (gw, gh) = ((x1 - x0) / u + 1, (y1 - y0) / u + 1);
    let cell = |gx: u32, gy: u32| mask[((y0 + gy * u) * w + x0 + gx * u) as usize];
    // outside = blank cells reachable from the border of the bounding box
    let mut outside = vec![false; (gw * gh) as usize];
    let mut stack: Vec<(u32, u32)> = Vec::new();
    for gy in 0..gh {
        for gx in 0..gw {
            if (gx == 0 || gy == 0 || gx == gw - 1 || gy == gh - 1) && !cell(gx, gy) {
                stack.push((gx, gy));
            }
        }
    }
    while let Some((gx, gy)) = stack.pop() {
        let i = (gy * gw + gx) as usize;
        if outside[i] || cell(gx, gy) {
            continue;
        }
        outside[i] = true;
        if gx > 0 {
            stack.push((gx - 1, gy));
        }
        if gy > 0 {
            stack.push((gx, gy - 1));
        }
        if gx + 1 < gw {
            stack.push((gx + 1, gy));
        }
        if gy + 1 < gh {
            stack.push((gx, gy + 1));
        }
    }
    let inner: Vec<(u32, u32)> = (0..gh)
        .flat_map(|gy| (0..gw).map(move |gx| (gx, gy)))
        .filter(|&(gx, gy)| !cell(gx, gy) && !outside[(gy * gw + gx) as usize])
        .collect();
    let (Some(ix0), Some(iy0)) = (
        inner.iter().map(|p| p.0).min(),
        inner.iter().map(|p| p.1).min(),
    ) else {
        return (mask, w);
    };
    let ix1 = inner.iter().map(|p| p.0).max().unwrap_or(ix0);
    let iy1 = inner.iter().map(|p| p.1).max().unwrap_or(iy0);
    if ix1 == ix0 {
        // one font pixel wide: redraw as a 4-wide slashed zero, same height
        //  .XX.
        //  X.XX
        //  XX.X
        //  X..X
        //  .XX.
        let nw = (x0 + 4 * u + 1).max(w);
        let mut out = vec![false; (nw * h) as usize];
        let last = gh - 1;
        for gy in 0..gh {
            for gx in 0..4u32 {
                let edge_row = gy == 0 || gy == last;
                let on = if edge_row {
                    gx == 1 || gx == 2
                } else if gx == 0 || gx == 3 {
                    true
                } else {
                    // the slash: top-right inner cell down to bottom-left
                    let t = gy - 1;
                    (t == 0 && gx == 2) || (t == 1 && gx == 1)
                };
                if !on {
                    continue;
                }
                for dy in 0..u {
                    for dx in 0..u {
                        let (x, y) = (x0 + gx * u + dx, y0 + gy * u + dy);
                        if y < h {
                            out[(y * nw + x) as usize] = true;
                        }
                    }
                }
            }
        }
        return (out, nw);
    }
    let mut fill: Vec<(u32, u32)> = Vec::new();
    // bottom-left to top-right through the counter
    let (iw, ih) = (ix1 - ix0, iy1 - iy0);
    for gy in iy0..=iy1 {
        let t = iy1 - gy;
        let gx = ix0 + (t * iw + ih / 2).checked_div(ih).unwrap_or(0);
        fill.push((gx.min(ix1), gy));
    }
    for (gx, gy) in fill {
        if !inner.contains(&(gx, gy)) {
            continue;
        }
        for dy in 0..u {
            for dx in 0..u {
                let (x, y) = (x0 + gx * u + dx, y0 + gy * u + dy);
                if x < w && y < h {
                    mask[(y * w + x) as usize] = true;
                }
            }
        }
    }
    (mask, w)
}

/// Letters and punctuation every text font carries beyond ASCII, where the
/// face has them: Latin-1 (`¡`-`ÿ`), Latin Extended-A (`Ā`-`ſ`: Œ œ Š š Ž
/// ž Ł ł Ğ ğ İ ı Ş ş ...), Romanian `Ș ș Ț ț` and typographic quotes,
/// dashes, the ellipsis and the euro sign. Translations draw from these.
pub const LATIN: [std::ops::RangeInclusive<char>; 7] = [
    '¡'..='¬',
    '®'..='ÿ',
    'Ā'..='ſ',
    'Ș'..='ț',
    '–'..='—',
    '‘'..='…',
    '€'..='€',
];

/// Symbols every generated font carries beyond ASCII (see [`symbol_rows`]).
pub const SYMBOLS: &str = "▶◀·⚠✓●←→↑↓↖↗↘↙";

/// 8x8 bitmap of a symbol, top row first (`X` = ink).
fn symbol_rows(c: char) -> Option<[String; 8]> {
    const RIGHT: [&str; 8] = [
        "...X....", "...XX...", "XXXXXX..", "XXXXXXX.", "XXXXXX..", "...XX...", "...X....",
        "........",
    ];
    const DOWN_RIGHT: [&str; 8] = [
        "XX......", "XXX.....", ".XXX..XX", "..XXX.XX", "...XXXXX", "..XXXXXX", ".XXXXXXX",
        "........",
    ];
    let own = |r: [&str; 8]| r.map(str::to_string);
    let mirror_h = |r: [&str; 8]| r.map(|row| row.chars().rev().collect::<String>());
    // the 8th row is spacing: mirror the seven inked rows only
    let mirror_v = |r: [String; 8]| {
        let mut out = r.clone();
        out[..7].reverse();
        out
    };
    // rotate RIGHT 90 degrees clockwise: column x of the result is row 7 - x
    let down = || {
        let src: Vec<Vec<char>> = RIGHT.iter().map(|r| r.chars().collect()).collect();
        let mut out: [String; 8] = Default::default();
        for (y, row) in out.iter_mut().enumerate() {
            *row = (0..8).map(|x| src[7 - x][y]).collect();
        }
        out
    };
    Some(match c {
        '▶' => own([
            "X.......", "XXX.....", "XXXXX...", "XXXXXXX.", "XXXXX...", "XXX.....", "X.......",
            "........",
        ]),
        '◀' => mirror_h([
            "X.......", "XXX.....", "XXXXX...", "XXXXXXX.", "XXXXX...", "XXX.....", "X.......",
            "........",
        ]),
        '·' => own([
            "........", "........", "........", "...XX...", "...XX...", "........", "........",
            "........",
        ]),
        '⚠' => own([
            "...XX...", "..X..X..", "..X..X..", ".XX..XX.", ".XXXXXX.", "XXX..XXX", "XXXXXXXX",
            "........",
        ]),
        '✓' => own([
            "........", ".......X", "......XX", "X....XX.", "XX..XX..", ".XXXX...", "..XX....",
            "........",
        ]),
        '●' => own([
            "........", "..XXXX..", ".XXXXXX.", ".XXXXXX.", ".XXXXXX.", ".XXXXXX.", "..XXXX..",
            "........",
        ]),
        // a crisp hand-drawn `%` (some pixel faces close it up)
        '%' => own([
            "XX....X.", "XX...X..", "....X...", "...X....", "..X.....", ".X...XX.", "X....XX.",
            "........",
        ]),
        '→' => own(RIGHT),
        '←' => mirror_h(RIGHT),
        '↓' => down(),
        '↑' => mirror_v(down()),
        '↘' => own(DOWN_RIGHT),
        '↙' => mirror_h(DOWN_RIGHT),
        '↗' => mirror_v(own(DOWN_RIGHT)),
        '↖' => mirror_v(mirror_h(DOWN_RIGHT)),
        _ => return None,
    })
}

/// Write `<out>.def` and `<out>.sff` next to each other.
pub fn write_font(generator: &Generator, out_def: &Path, name: &str) -> anyhow::Result<()> {
    let sff_name = out_def
        .with_extension("sff")
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{name}.sff"));
    let (def, sff) = generator.build(name, &sff_name);
    if let Some(dir) = out_def.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out_def, def)?;
    std::fs::write(out_def.with_extension("sff"), sff.write_v2())?;
    Ok(())
}

/// A font role of the motif: which TTF under `motif/font/src/` renders it and how.
#[derive(Clone, Debug)]
pub struct Role {
    /// Output name (`motif/font/<name>.def`).
    pub name: &'static str,
    /// Source TTF file name under `motif/font/src/`.
    pub file: &'static str,
    pub style: Style,
    /// The face's own pixel in font units when its outlines are not whole
    /// multiples of the em (see [`Generator::snapped`]).
    pub grid: Option<f32>,
}

/// The Steam Deck's small text (`fontN.deck` in both themes): Pixelify
/// Sans at twice its own pixel grid (11 px to the em, 1000 / 11 font
/// units), 14 px capitals and a 10 px x-height, Latin, Latin Extended and
/// Cyrillic.
pub const DECK_FONT: &str = "rugen-deck";

fn deck_role() -> Role {
    Role {
        name: DECK_FONT,
        file: "PixelifySans-Regular.ttf",
        style: Style {
            size: 26,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
        grid: Some(1000.0 / 11.0),
    }
}

/// System font slot (`font5` in every theme's `system.def`) of
/// [`CODE_FONT`]: big heavy type for room codes (`font = 5` in a screen).
// screens name it in TOML (`font = 5`); Rust only checks the slot exists
#[cfg_attr(not(test), allow(dead_code))]
pub const CODE_FONT_SLOT: i32 = 5;
/// The room-code font: Press Start 2P at four times its 8 px grid, no rim
/// or highlight, slashed zero, `1` / `I`, `5` / `S`, `2` / `Z`, `8` / `B`
/// drawn apart by the face itself.
pub const CODE_FONT: &str = "rugen-code";

/// The Japanese text font (`fontN.ja` in both themes): DotGothic16 on its
/// own 16 px grid (1000 / 16 font units), 13 px capitals, ASCII plus the
/// kana and every kanji `motif/lang/ja.toml` uses (see [`ja_chars`]).
/// `rugen-ja2` is the same face at 2x for the dojo theme's body font.
pub const JA_FONT: &str = "rugen-ja";

fn ja_roles() -> [Role; 2] {
    let role = |name, size| Role {
        name,
        file: "DotGothic16-Regular.ttf",
        style: Style {
            size,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
        grid: Some(1000.0 / 16.0),
    };
    [role(JA_FONT, 16), role("rugen-ja2", 32)]
}

/// The characters the Japanese font carries beyond ASCII: hiragana,
/// katakana, the common CJK punctuation and every character at or above
/// U+3000 in `motif/lang/ja.toml` (the kanji the translation uses), so the
/// font follows the text and stays small.
pub fn ja_chars(motif: &Path) -> Vec<char> {
    let mut v: Vec<char> = ('\u{3041}'..='\u{3096}')
        .chain('\u{30A1}'..='\u{30FA}')
        .chain(['\u{3001}', '\u{3002}', '\u{300C}', '\u{300D}', '\u{3005}'])
        .chain(['\u{30FB}', '\u{30FC}', '\u{309B}', '\u{309C}', '\u{3000}'])
        .collect();
    if let Ok(text) = std::fs::read_to_string(motif.join("lang").join("ja.toml")) {
        v.extend(text.chars().filter(|c| *c as u32 >= 0x3000));
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The motif fonts: chunky display, menu, small body, HUD numbers and the
/// room-code font, the Steam Deck's small text and the Japanese fonts. All
/// OFL pixel fonts rendered at a whole multiple of their native pixel size
/// (crisp, no AA); every zero is slashed.
pub fn roles() -> [Role; 8] {
    let st = |size, italic, rim, highlight| Style {
        size,
        italic,
        condense: 1.0,
        rim,
        highlight,
    };
    [
        Role {
            name: "rugen-title",
            file: "PressStart2P-Regular.ttf",
            style: st(16, 0, true, true),
            grid: None,
        },
        // body / caption text: Jersey 10 at twice its 10 px grid, a bold,
        // condensed mixed-case pixel face that stays readable at 1x
        Role {
            name: "rugen-menu",
            file: "Jersey10-Regular.ttf",
            style: st(20, 0, false, false),
            grid: None,
        },
        Role {
            name: "rugen-small",
            file: "PressStart2P-Regular.ttf",
            style: st(8, 0, false, false),
            grid: None,
        },
        Role {
            name: "rugen-num",
            file: "PressStart2P-Regular.ttf",
            style: st(16, 0, true, true),
            grid: None,
        },
        Role {
            name: CODE_FONT,
            file: "PressStart2P-Regular.ttf",
            style: st(32, 0, false, false),
            grid: None,
        },
        deck_role(),
        ja_roles()[0].clone(),
        ja_roles()[1].clone(),
    ]
}

/// The dojo theme's fonts (`motif/themes/dojo/font/`): Jersey 10 at 2x,
/// 3x and 4x its 10 px grid for captions, body and titles; Press Start for
/// tags, HUD numbers and room codes. Slots: 1 small, 2 menu (captions),
/// 3 title, 4 num, 5 code, 6 body; Pixelify Sans for the Steam Deck,
/// DotGothic16 for Japanese.
pub fn dojo_roles() -> [Role; 9] {
    let st = |size, rim, highlight| Style {
        size,
        italic: 0,
        condense: 1.0,
        rim,
        highlight,
    };
    [
        Role {
            name: "rugen-small",
            file: "PressStart2P-Regular.ttf",
            style: st(8, false, false),
            grid: None,
        },
        Role {
            name: "rugen-menu",
            file: "Jersey10-Regular.ttf",
            style: st(20, false, false),
            grid: None,
        },
        Role {
            name: "rugen-title",
            file: "Jersey10-Regular.ttf",
            style: st(40, false, false),
            grid: None,
        },
        Role {
            name: "rugen-num",
            file: "PressStart2P-Regular.ttf",
            style: st(16, false, false),
            grid: None,
        },
        Role {
            name: CODE_FONT,
            file: "PressStart2P-Regular.ttf",
            style: st(32, false, false),
            grid: None,
        },
        Role {
            name: "dojo-body",
            file: "Jersey10-Regular.ttf",
            style: st(30, false, false),
            grid: None,
        },
        deck_role(),
        ja_roles()[0].clone(),
        ja_roles()[1].clone(),
    ]
}

/// Bytes of a role's TTF: `motif/font/src/<file>`, else `fallback`.
pub fn role_ttf(motif: &Path, role: &Role, fallback: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    let p = motif.join("font").join("src").join(role.file);
    if p.exists() {
        return Ok(std::fs::read(&p)?);
    }
    match fallback {
        Some(b) => {
            log::warn!(
                "{} missing, using the fallback TTF for {}",
                p.display(),
                role.name
            );
            Ok(b.to_vec())
        }
        None => anyhow::bail!("{} missing and no fallback TTF", p.display()),
    }
}

/// Write every role font into `motif/font/`.
pub fn generate_roles(motif: &Path, fallback: Option<&[u8]>) -> anyhow::Result<()> {
    generate_roles_into(motif, &motif.join("font"), fallback)
}

/// Write every role font into `out_dir`, reading the TTFs from `motif/font/src/`
/// (themes keep their own copy of the fonts next to their screens).
pub fn generate_roles_into(
    motif: &Path,
    out_dir: &Path,
    fallback: Option<&[u8]>,
) -> anyhow::Result<()> {
    write_roles(motif, out_dir, &roles(), fallback)
}

/// Write `roles` into `out_dir`, reading the TTFs from `motif/font/src/`.
pub fn write_roles(
    motif: &Path,
    out_dir: &Path,
    roles: &[Role],
    fallback: Option<&[u8]>,
) -> anyhow::Result<()> {
    for role in roles.iter().cloned() {
        let bytes = role_ttf(motif, &role, fallback)?;
        let mut g = Generator::new(&bytes, role.style)?;
        if let Some(unit) = role.grid {
            g = g.snapped(unit);
        }
        // caption fonts: thin red type without the dark outline
        if role.name == "rugen-menu" || role.name == DECK_FONT {
            g = g.plain_red();
        }
        if role.name == CODE_FONT {
            g = g.ascii_only();
        }
        if role.name.starts_with(JA_FONT) {
            // thin strokes: no dark outline on the red bank either
            g = g.ascii_only().plain_red().with_chars(ja_chars(motif));
        }
        write_font(&g, &out_dir.join(format!("{}.def", role.name)), role.name)?;
    }
    Ok(())
}

/// A bold (preferably condensed) sans TTF from the system when none is given.
pub fn default_ttf() -> Option<std::path::PathBuf> {
    [
        "/usr/share/fonts/liberation-narrow/LiberationSansNarrow-Bold.ttf",
        "/usr/share/fonts/liberation-sans-narrow-fonts/LiberationSansNarrow-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed-Bold.ttf",
        "/usr/share/fonts/google-droid-sans-fonts/DroidSans-Bold.ttf",
        "/usr/share/fonts/liberation-sans-fonts/LiberationSans-Bold.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.exists())
}

/// Preview sheet of the printable glyphs as RGBA (for `--preview`).
pub fn preview(generator: &Generator, bank: usize) -> (u32, u32, Vec<u8>) {
    let lines = [
        "ABCDEFGHIJKLM",
        "NOPQRSTUVWXYZ",
        "abcdefghijklm",
        "nopqrstuvwxyz",
        "0123456789 !?.,:%",
        SYMBOLS,
    ];
    let rendered: Vec<(u32, u32, Vec<u8>)> = lines
        .iter()
        .map(|l| generator.text_rgba(l, bank, 2))
        .collect();
    let w = rendered.iter().map(|r| r.0).max().unwrap_or(1) + 8;
    let h: u32 = rendered.iter().map(|r| r.1 + 4).sum::<u32>() + 8;
    let mut out = vec![18u8; (w * h * 4) as usize];
    for p in out.chunks_exact_mut(4) {
        p[3] = 255;
    }
    let mut y0 = 4;
    for (rw, rh, px) in &rendered {
        for y in 0..*rh {
            for x in 0..*rw {
                let s = ((y * rw + x) * 4) as usize;
                let a = px[s + 3] as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let d = (((y + y0) * w + x + 4) * 4) as usize;
                for k in 0..3 {
                    out[d + k] = (px[s + k] as f32 * a + out[d + k] as f32 * (1.0 - a)) as u8;
                }
            }
        }
        y0 += rh + 4;
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(g: &Generator, c: char) -> Vec<String> {
        let (w, h, m) = g.mask(c).unwrap();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| if m[(y * w + x) as usize] { 'X' } else { '.' })
                    .collect::<String>()
            })
            .filter(|r| r.contains('X'))
            .collect()
    }

    #[test]
    fn every_theme_has_the_code_font_slot() {
        let themes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif/themes");
        let want = format!("font{CODE_FONT_SLOT} = font/{CODE_FONT}.def");
        for t in ["ink", "dojo"] {
            let def = std::fs::read_to_string(themes.join(t).join("system.def")).unwrap();
            assert!(def.lines().any(|l| l.trim() == want), "{t}: no {want}");
        }
        let flat = themes.join("../font").join(format!("{CODE_FONT}.def"));
        assert!(flat.exists(), "{} missing", flat.display());
    }

    #[test]
    fn zero_never_matches_the_letter_o() {
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif");
        for role in roles().into_iter().chain(dojo_roles()) {
            let ttf = role_ttf(&motif, &role, None).expect("role TTF in motif/font/src");
            let g = Generator::new(&ttf, role.style).unwrap();
            let zero = rows(&g, '0');
            assert_ne!(zero, rows(&g, 'O'), "{}: 0 looks like O", role.name);
            // the counter holds ink: a dot (three runs on a row) or a slash
            // (a row that is not its own mirror image)
            let marked = zero[1..zero.len() - 1].iter().any(|r| {
                let r = r.trim_matches('.');
                let runs = r.split('.').filter(|p| !p.is_empty()).count();
                runs >= 3 || r.chars().rev().collect::<String>() != r
            });
            assert!(marked, "{}: 0 has an empty counter: {zero:?}", role.name);
        }
    }

    /// Pixelify Sans is not drawn on whole multiples of its em: snapped, its
    /// pixels land on 2x2 screen pixels with nothing half-covered left over.
    #[test]
    fn deck_font_sits_on_its_own_pixel_grid() {
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif");
        let role = deck_role();
        let ttf = role_ttf(&motif, &role, None).expect("Pixelify Sans in motif/font/src");
        let g = Generator::new(&ttf, role.style)
            .unwrap()
            .snapped(role.grid.unwrap());
        assert_eq!(g.mult, 2);
        assert_eq!(g.gap(), 2);
        assert_eq!(g.cap_rows().1, 14, "capitals: 7 design pixels");
        for c in "HBEMNSZaegkmsxyz258%".chars() {
            let (w, h, m) = g.mask(c).unwrap();
            assert_eq!(grid_unit(&m, w, h), 2, "{c:?} is off the 2 px grid");
        }
        let xh = rows(&g, 'x').len();
        assert_eq!(xh, 10, "x-height: 5 design pixels");
        // unsnapped, the same face loses whole strokes at that size
        let raw = Generator::new(&ttf, role.style).unwrap();
        let (w, h, m) = raw.mask('B').unwrap();
        assert_ne!(grid_unit(&m, w, h), 2);
    }

    /// Every text font of both themes (and the flat motif) draws the
    /// accented Latin translations will need: no character of the sample
    /// falls back to a blank cell.
    #[test]
    fn text_fonts_carry_accented_latin() {
        const SAMPLE: &str = "ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖØÙÚÛÜÝÞß àáâãäåæçèéêëìíîïðñòóôõöøùúûüýþÿ \
            Œœ Šš Žž Ÿ Łł Ğğ İı Şş Șș Țț Ññ ĄąĆćĘęŃńŚśŹźŻż ČčĎďĚěŇňŘřŤťŮů Őő Űű Ăă \
            ¡¿ «» € „“” ‘’ … – — Straße, Œuvre, Łódź, Ağrı, Şişli, Ñandú";
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif");
        let dirs = [
            motif.join("font"),
            motif.join("themes/ink/font"),
            motif.join("themes/dojo/font"),
        ];
        let mut checked = 0;
        for dir in dirs {
            for e in std::fs::read_dir(&dir).unwrap() {
                let p = e.unwrap().path();
                let name = p.file_stem().unwrap().to_string_lossy().to_string();
                if p.extension().is_none_or(|x| x != "def") || name == CODE_FONT {
                    continue;
                }
                let f = rugen_formats::fnt::Font::open(&p).unwrap();
                let missing: String = SAMPLE
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .filter(|c| {
                        f.glyphs
                            .get(c)
                            .is_none_or(|g| g.pixels.iter().all(|&px| px == 0))
                    })
                    .collect();
                assert!(
                    missing.is_empty(),
                    "{}: no glyph for {missing:?}",
                    p.display()
                );
                assert!(f.text_width(SAMPLE) > 0);
                checked += 1;
            }
        }
        assert!(checked >= 15, "only {checked} fonts checked");
    }

    /// Snapping a face already drawn on its grid changes nothing.
    #[test]
    fn snapping_a_grid_aligned_face_is_a_no_op() {
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif");
        let role = roles()[2].clone();
        assert_eq!(role.file, "PressStart2P-Regular.ttf");
        let ttf = role_ttf(&motif, &role, None).unwrap();
        let plain = Generator::new(&ttf, role.style).unwrap();
        let snapped = Generator::new(&ttf, role.style).unwrap().snapped(125.0);
        for c in "ABC0xyz%".chars() {
            assert_eq!(rows(&plain, c), rows(&snapped, c), "{c:?}");
        }
    }

    #[test]
    fn caption_font_keeps_letters_apart_and_red_plain() {
        let motif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif");
        let role = roles()[1].clone();
        assert_eq!(role.name, "rugen-menu");
        let ttf = role_ttf(&motif, &role, None).expect("caption TTF in motif/font/src");
        let g = Generator::new(&ttf, role.style).unwrap().plain_red();
        // one pixel of the 2x grid between faces
        assert_eq!(g.gap(), 2);
        let (def, sff) = g.build("t", "t.sff");
        assert!(def.contains(&format!("Spacing = {},0", 2 - g.extra())));
        // capitals span 10 px: twice Jersey 10's 5-pixel cap height
        assert_eq!(g.cap_rows().1, 10);
        let red = &sff.palettes[BANK_RED];
        assert_eq!(
            red[IDX_OUTLINE as usize][3], 0,
            "red captions have no dark outline"
        );
    }
}
