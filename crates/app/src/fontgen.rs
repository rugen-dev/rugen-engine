//! `rugen fontgen`: build MUGEN FNT v2 fonts (`.def` + SFF v2) from a TTF
//! with a pixelised 3D look: hard-edged glyphs, extruded sides, a bevel
//! highlight and a drop shadow. Colour banks: 0 gold, 1 white, 2 red,
//! 3 blue, 4 green, 5 grey, 6 dark.

use std::path::Path;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use rugen_formats::sff::{Sff, Version};

const IDX_HI: u8 = 1;
const IDX_FACE: u8 = 2;
const IDX_SHADE: u8 = 3;
const IDX_SIDE: u8 = 4;
const IDX_SIDE_DARK: u8 = 5;
const IDX_SHADOW: u8 = 6;

#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Cell height in pixels (font px size).
    pub size: u32,
    /// Extrusion depth in pixels.
    pub depth: u32,
    /// Pixelation: glyphs are rasterised at `size / pixel` and scaled up.
    pub pixel: u32,
    pub shadow: bool,
}

/// `[hi, face, shade, side, side_dark, shadow]` RGBA per bank.
pub const BANKS: [[[u8; 4]; 6]; 7] = [
    [
        [255, 244, 190, 255],
        [250, 200, 60, 255],
        [205, 140, 20, 255],
        [125, 75, 12, 255],
        [80, 45, 6, 255],
        [0, 0, 0, 170],
    ],
    [
        [255, 255, 255, 255],
        [232, 232, 240, 255],
        [175, 175, 195, 255],
        [95, 95, 115, 255],
        [55, 55, 70, 255],
        [0, 0, 0, 170],
    ],
    [
        [255, 190, 190, 255],
        [235, 55, 55, 255],
        [175, 25, 25, 255],
        [95, 12, 12, 255],
        [60, 6, 6, 255],
        [0, 0, 0, 170],
    ],
    [
        [190, 215, 255, 255],
        [70, 130, 245, 255],
        [35, 85, 185, 255],
        [18, 45, 105, 255],
        [10, 25, 60, 255],
        [0, 0, 0, 170],
    ],
    [
        [200, 255, 200, 255],
        [70, 205, 85, 255],
        [35, 145, 55, 255],
        [18, 85, 32, 255],
        [10, 45, 18, 255],
        [0, 0, 0, 170],
    ],
    [
        [225, 225, 235, 255],
        [150, 150, 170, 255],
        [100, 100, 120, 255],
        [55, 55, 68, 255],
        [32, 32, 42, 255],
        [0, 0, 0, 170],
    ],
    [
        [120, 120, 140, 255],
        [60, 60, 75, 255],
        [40, 40, 52, 255],
        [22, 22, 30, 255],
        [12, 12, 18, 255],
        [0, 0, 0, 120],
    ],
];

pub fn bank_palette(bank: usize) -> Vec<[u8; 4]> {
    let b = BANKS[bank.min(BANKS.len() - 1)];
    let mut pal = vec![[0u8; 4]; 256];
    pal[IDX_HI as usize] = b[0];
    pal[IDX_FACE as usize] = b[1];
    pal[IDX_SHADE as usize] = b[2];
    pal[IDX_SIDE as usize] = b[3];
    pal[IDX_SIDE_DARK as usize] = b[4];
    pal[IDX_SHADOW as usize] = b[5];
    pal
}

/// One rendered glyph: indexed pixels, `w x h`, where `h` is the cell height.
pub struct Glyph {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

pub struct Generator<'a> {
    font: FontRef<'a>,
    style: Style,
    /// Face cell height before extrusion.
    cell_h: u32,
    ascent: f32,
    raster_scale: f32,
}

impl<'a> Generator<'a> {
    pub fn new(ttf: &'a [u8], style: Style) -> anyhow::Result<Self> {
        let font = FontRef::try_from_slice(ttf).map_err(|e| anyhow::anyhow!("font: {e}"))?;
        let pixel = style.pixel.max(1);
        let raster = (style.size / pixel).max(4) as f32;
        let scaled = font.as_scaled(PxScale::from(raster));
        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let cell_h = libm::ceilf(ascent - descent) as u32 * pixel;
        Ok(Self {
            font,
            style,
            cell_h,
            ascent,
            raster_scale: raster,
        })
    }

    /// Total sprite height: face cell plus extrusion and shadow.
    pub fn height(&self) -> u32 {
        self.cell_h + self.style.depth + u32::from(self.style.shadow)
    }

    /// Width used for spaces.
    pub fn space_width(&self) -> u32 {
        let scaled = self.font.as_scaled(PxScale::from(self.raster_scale));
        (scaled.h_advance(self.font.glyph_id(' ')) * self.style.pixel as f32) as u32
    }

    /// Binary mask of `c` at face resolution: `(w, h, mask)`.
    fn mask(&self, c: char) -> Option<(u32, u32, Vec<bool>)> {
        let pixel = self.style.pixel.max(1);
        let id = self.font.glyph_id(c);
        let glyph = id.with_scale_and_position(
            PxScale::from(self.raster_scale),
            ab_glyph::point(0.0, self.ascent),
        );
        let outline = self.font.outline_glyph(glyph)?;
        let b = outline.px_bounds();
        let bw = libm::ceilf(b.width()) as u32 + 1;
        let cell_small = (self.cell_h / pixel).max(1);
        let y0 = b.min.y.max(0.0) as u32;
        let mut small = vec![false; (bw * cell_small) as usize];
        outline.draw(|x, y, cov| {
            if cov >= 0.5 {
                let yy = y + y0;
                if yy < cell_small && x < bw {
                    small[(yy * bw + x) as usize] = true;
                }
            }
        });
        let w = bw * pixel;
        let h = self.cell_h;
        let mut mask = vec![false; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x / pixel, y / pixel);
                if sy < cell_small && sx < bw && small[(sy * bw + sx) as usize] {
                    mask[(y * w + x) as usize] = true;
                }
            }
        }
        Some((w, h, mask))
    }

    /// Render one glyph with the 3D effect.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        let (mw, mh, mask) = self.mask(c)?;
        if !mask.iter().any(|&m| m) {
            return None;
        }
        let depth = self.style.depth;
        let sh = u32::from(self.style.shadow);
        let (w, h) = (mw + depth + sh, mh + depth + sh);
        let mut px = vec![0u8; (w * h) as usize];
        let at = |x: u32, y: u32| mask[(y * mw + x) as usize];
        let mut put = |x: u32, y: u32, v: u8| {
            if x < w && y < h {
                px[(y * w + x) as usize] = v;
            }
        };
        if self.style.shadow {
            for y in 0..mh {
                for x in 0..mw {
                    if at(x, y) {
                        put(x + depth + 1, y + depth + 1, IDX_SHADOW);
                    }
                }
            }
        }
        for k in (1..=depth).rev() {
            let v = if k == depth { IDX_SIDE_DARK } else { IDX_SIDE };
            for y in 0..mh {
                for x in 0..mw {
                    if at(x, y) {
                        put(x + k, y + k, v);
                    }
                }
            }
        }
        for y in 0..mh {
            for x in 0..mw {
                if !at(x, y) {
                    continue;
                }
                let top = y == 0 || !at(x, y - 1);
                let left = x == 0 || !at(x - 1, y);
                let bottom = y + 1 >= mh || !at(x, y + 1);
                let v = if top || left {
                    IDX_HI
                } else if bottom {
                    IDX_SHADE
                } else {
                    IDX_FACE
                };
                put(x, y, v);
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
            sff.push_palette((0, b as u16), bank_palette(b));
        }
        for code in 33u16..=126 {
            let c = char::from_u32(code as u32).unwrap_or(' ');
            if let Some(g) = self.glyph(c) {
                sff.push_indexed((0, code), g.w as u16, g.h as u16, [0, 0], 0, g.px);
            }
        }
        let def = format!(
            "; generated by `rugen fontgen`: pixel 3D, banks 0 gold 1 white 2 red 3 blue 4 green 5 grey 6 dark\n\
             [FNT v2]\nfntversion = 2,00\nname = \"{name}\"\n\n\
             [Def]\nType = bitmap\nBankType = palette\nSize = {},{}\nSpacing = 1,0\nOffset = 0,0\nFile = {sff_name}\n",
            self.space_width().max(2),
            self.height()
        );
        (def, sff)
    }

    /// Render a line of text as straight RGBA with the given bank (for logos).
    pub fn text_rgba(&self, text: &str, bank: usize, spacing: i32) -> (u32, u32, Vec<u8>) {
        let pal = bank_palette(bank);
        let glyphs: Vec<Option<Glyph>> = text.chars().map(|c| self.glyph(c)).collect();
        let space = self.space_width();
        let mut w: i32 = 0;
        for (i, g) in glyphs.iter().enumerate() {
            if i > 0 {
                w += spacing;
            }
            w += g.as_ref().map_or(space as i32, |g| g.w as i32);
        }
        let (w, h) = (w.max(1) as u32, self.height());
        let mut out = vec![0u8; (w * h * 4) as usize];
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
                    if v == 0 {
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
        (w, h, out)
    }
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

/// A bold TTF from the system when none is given.
pub fn default_ttf() -> Option<std::path::PathBuf> {
    [
        "/usr/share/fonts/google-droid-sans-fonts/DroidSans-Bold.ttf",
        "/usr/share/fonts/liberation-sans-fonts/LiberationSans-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
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
        "0123456789 !?.,:",
    ];
    let rendered: Vec<(u32, u32, Vec<u8>)> = lines
        .iter()
        .map(|l| generator.text_rgba(l, bank, 2))
        .collect();
    let w = rendered.iter().map(|r| r.0).max().unwrap_or(1) + 8;
    let h: u32 = rendered.iter().map(|r| r.1 + 4).sum::<u32>() + 8;
    let mut out = vec![40u8; (w * h * 4) as usize];
    for p in out.chunks_exact_mut(4) {
        p[3] = 255;
    }
    let mut y0 = 4;
    for (rw, rh, px) in &rendered {
        for y in 0..*rh {
            for x in 0..*rw {
                let s = ((y * rw + x) * 4) as usize;
                if px[s + 3] == 0 {
                    continue;
                }
                let d = (((y + y0) * w + x + 4) * 4) as usize;
                out[d..d + 4].copy_from_slice(&px[s..s + 4]);
            }
        }
        y0 += rh + 4;
    }
    (w, h, out)
}
