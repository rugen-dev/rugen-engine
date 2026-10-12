//! `rugen fontgen`: build MUGEN FNT v2 fonts (`.def` + SFF v2) from a TTF as
//! crisp pixel type in an arena-fighter style: bold condensed, thresholded
//! (no anti-aliasing), sheared one pixel every few rows for italics, with a
//! 1-px dark outline, a 1-px drop shadow and an optional 1-px coloured rim.
//! Colour banks: 0 white/red rim, 1 white/orange rim, 2 red, 3 orange,
//! 4 yellow, 5 blue, 6 grey, 7 plain white, 8 dark.

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

/// `(face, highlight, outline, shadow, rim)` RGBA per bank.
pub const BANKS: [[[u8; 4]; 5]; 12] = [
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
        })
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

    /// Total sprite height.
    pub fn height(&self) -> u32 {
        self.cell_h + 2 * self.pad() + 1
    }

    /// Width used for spaces.
    pub fn space_width(&self) -> u32 {
        let scaled = self.font.as_scaled(PxScale::from(self.style.size as f32));
        (scaled.h_advance(self.font.glyph_id(' ')) * self.style.condense).max(2.0) as u32
    }

    /// Thresholded, sheared, condensed mask of `c` at face resolution.
    fn mask(&self, c: char) -> Option<(u32, u32, Vec<bool>)> {
        let id = self.font.glyph_id(c);
        let scale = PxScale {
            x: self.style.size as f32 * self.style.condense,
            y: self.style.size as f32,
        };
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(0.0, self.ascent));
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
            sff.push_palette((0, b as u16), bank_palette(b));
        }
        for code in 33u16..=126 {
            let c = char::from_u32(code as u32).unwrap_or(' ');
            if let Some(g) = self.glyph(c) {
                sff.push_indexed((0, code), g.w as u16, g.h as u16, [0, 0], 0, g.px);
            }
        }
        // sprites carry padding, the shadow column and the shear: pull the
        // advance back so faces sit one pixel apart
        let spacing = 1 - self.extra();
        let def = format!(
            "; generated by `rugen fontgen`: pixel italic sans, outline + shadow; banks 0 white/red 1 white/orange 2 red 3 orange 4 yellow 5 blue 6 grey 7 white 8 dark\n\
             [FNT v2]\nfntversion = 2,00\nname = \"{name}\"\n\n\
             [Def]\nType = bitmap\nBankType = palette\nSize = {},{}\nSpacing = {spacing},0\nOffset = {},{}\nFile = {sff_name}\n",
            self.space_width().max(self.style.size / 2) as i32 - spacing,
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
}

/// The four motif fonts: chunky display, menu, small body, HUD numbers.
/// All OFL pixel fonts rendered at their native pixel size (crisp, no AA).
pub fn roles() -> [Role; 4] {
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
        },
        Role {
            name: "rugen-menu",
            file: "Silkscreen-Bold.ttf",
            style: st(16, 0, false, true),
        },
        Role {
            name: "rugen-small",
            file: "PressStart2P-Regular.ttf",
            style: st(8, 0, false, false),
        },
        Role {
            name: "rugen-num",
            file: "PressStart2P-Regular.ttf",
            style: st(16, 0, true, true),
        },
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
    for role in roles() {
        let bytes = role_ttf(motif, &role, fallback)?;
        let g = Generator::new(&bytes, role.style)?;
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
        "0123456789 !?.,:",
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
