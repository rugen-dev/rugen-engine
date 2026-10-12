//! Pixel-art pipeline shared by `rugen pixelize`, `rugen kit-import` and
//! `rugen ui-gen`: a fixed palette (`motif/palette.gpl`), box downscaling,
//! palette quantisation with ordered dithering, 1-px outlines and
//! luminance remapping (recolouring CC0 kit pieces onto one of our ramps).

use std::path::Path;

use crate::screenpack::Canvas;

pub type Rgba = [u8; 4];

/// A named colour ramp, dark to light.
#[derive(Clone, Debug)]
pub struct Ramp {
    pub name: &'static str,
    pub colors: Vec<Rgba>,
}

/// The rugen palette: navy nights, maroon panels, gold accents.
pub const NAVY: [Rgba; 6] = [
    [6, 8, 18, 255],
    [12, 16, 34, 255],
    [22, 30, 58, 255],
    [38, 50, 92, 255],
    [66, 84, 140, 255],
    [110, 130, 190, 255],
];
pub const MAROON: [Rgba; 6] = [
    [40, 8, 20, 255],
    [78, 18, 36, 255],
    [118, 30, 48, 255],
    [170, 46, 62, 255],
    [220, 80, 88, 255],
    [250, 140, 130, 255],
];
pub const GOLD: [Rgba; 6] = [
    [70, 36, 10, 255],
    [150, 80, 20, 255],
    [220, 140, 40, 255],
    [255, 190, 70, 255],
    [255, 230, 140, 255],
    [255, 250, 220, 255],
];
pub const STEEL: [Rgba; 6] = [
    [16, 18, 26, 255],
    [40, 44, 60, 255],
    [84, 90, 118, 255],
    [140, 148, 180, 255],
    [200, 206, 228, 255],
    [245, 247, 255, 255],
];
pub const TEAL: [Rgba; 5] = [
    [8, 30, 36, 255],
    [16, 70, 80, 255],
    [30, 120, 130, 255],
    [70, 190, 190, 255],
    [160, 240, 230, 255],
];
pub const RED: [Rgba; 4] = [
    [80, 10, 14, 255],
    [190, 40, 40, 255],
    [240, 70, 60, 255],
    [255, 140, 120, 255],
];
pub const GREEN: [Rgba; 4] = [
    [10, 50, 30, 255],
    [30, 130, 70, 255],
    [90, 220, 110, 255],
    [190, 255, 190, 255],
];
pub const BLUE: [Rgba; 4] = [
    [10, 30, 80, 255],
    [40, 90, 200, 255],
    [90, 160, 255, 255],
    [190, 220, 255, 255],
];
pub const BLACK: Rgba = [4, 4, 8, 255];
pub const WHITE: Rgba = [255, 255, 255, 255];

pub fn ramps() -> Vec<Ramp> {
    vec![
        Ramp {
            name: "navy",
            colors: NAVY.to_vec(),
        },
        Ramp {
            name: "maroon",
            colors: MAROON.to_vec(),
        },
        Ramp {
            name: "gold",
            colors: GOLD.to_vec(),
        },
        Ramp {
            name: "steel",
            colors: STEEL.to_vec(),
        },
        Ramp {
            name: "teal",
            colors: TEAL.to_vec(),
        },
        Ramp {
            name: "red",
            colors: RED.to_vec(),
        },
        Ramp {
            name: "green",
            colors: GREEN.to_vec(),
        },
        Ramp {
            name: "blue",
            colors: BLUE.to_vec(),
        },
    ]
}

/// Every palette colour (ramps + black + white).
pub fn full_palette() -> Vec<Rgba> {
    let mut out = vec![BLACK, WHITE];
    for r in ramps() {
        out.extend(r.colors);
    }
    out
}

/// GIMP palette text for `motif/palette.gpl`.
pub fn gpl() -> String {
    let mut s = String::from("GIMP Palette\nName: rugen\nColumns: 6\n#\n");
    s.push_str(&format!(
        "{:3} {:3} {:3}\tblack\n",
        BLACK[0], BLACK[1], BLACK[2]
    ));
    s.push_str(&format!(
        "{:3} {:3} {:3}\twhite\n",
        WHITE[0], WHITE[1], WHITE[2]
    ));
    for r in ramps() {
        for (i, c) in r.colors.iter().enumerate() {
            s.push_str(&format!(
                "{:3} {:3} {:3}\t{} {}\n",
                c[0], c[1], c[2], r.name, i
            ));
        }
    }
    s
}

/// Parse a GIMP `.gpl` palette (lines `r g b [name]`).
pub fn load_gpl(path: &Path) -> anyhow::Result<Vec<Rgba>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.contains(':') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(r), Some(g), Some(b)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>()) {
            out.push([r, g, b, 255]);
        }
    }
    anyhow::ensure!(!out.is_empty(), "{}: no colours", path.display());
    Ok(out)
}

/// Overwrite a pixel (no alpha blending), unlike `Canvas::put`.
fn set(c: &mut Canvas, x: i32, y: i32, p: Rgba) {
    if x < 0 || y < 0 || x >= c.w as i32 || y >= c.h as i32 {
        return;
    }
    let o = ((y as u32 * c.w + x as u32) * 4) as usize;
    c.px[o..o + 4].copy_from_slice(&p);
}

pub fn luminance(c: Rgba) -> f32 {
    (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0
}

/// Perceptually weighted squared distance.
fn dist(a: Rgba, b: Rgba) -> f32 {
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    0.3 * dr * dr + 0.59 * dg * dg + 0.11 * db * db
}

pub fn nearest(c: Rgba, palette: &[Rgba]) -> Rgba {
    let mut best = palette.first().copied().unwrap_or(BLACK);
    let mut bd = f32::MAX;
    for &p in palette {
        let d = dist(c, p);
        if d < bd {
            bd = d;
            best = p;
        }
    }
    best
}

const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Quantise to `palette`; with `dither`, 4x4 ordered dithering of strength `spread` (0..1).
pub fn quantize(c: &mut Canvas, palette: &[Rgba], dither: bool, spread: f32) {
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let p = c.get(x, y);
            if p[3] == 0 {
                continue;
            }
            let mut q = p;
            if dither {
                let t = (BAYER4[(y & 3) as usize][(x & 3) as usize] as f32 / 16.0 - 0.5)
                    * spread
                    * 255.0;
                for ch in &mut q[..3] {
                    *ch = (*ch as f32 + t).clamp(0.0, 255.0) as u8;
                }
            }
            let mut n = nearest(q, palette);
            n[3] = if p[3] < 128 { 0 } else { 255 };
            set(c, x, y, if n[3] == 0 { [0, 0, 0, 0] } else { n });
        }
    }
}

/// Box-filter downscale to `target_w` pixels wide (keeps aspect).
pub fn downscale(c: &Canvas, target_w: u32) -> Canvas {
    if target_w == 0 || target_w >= c.w {
        return Canvas::from_rgba(c.w, c.h, c.px.clone());
    }
    let target_h = ((c.h as u64 * target_w as u64) / c.w as u64).max(1) as u32;
    let mut out = Canvas::new(target_w, target_h);
    for y in 0..target_h {
        let y0 = (y as u64 * c.h as u64 / target_h as u64) as i32;
        let y1 = (((y + 1) as u64 * c.h as u64) / target_h as u64).max(y0 as u64 + 1) as i32;
        for x in 0..target_w {
            let x0 = (x as u64 * c.w as u64 / target_w as u64) as i32;
            let x1 = (((x + 1) as u64 * c.w as u64) / target_w as u64).max(x0 as u64 + 1) as i32;
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = c.get(xx, yy);
                    let w = p[3] as u32;
                    r += p[0] as u32 * w;
                    g += p[1] as u32 * w;
                    b += p[2] as u32 * w;
                    a += w;
                    n += 1;
                }
            }
            let ch = |v: u32| v.checked_div(a).map_or(0, |q| q as u8);
            let px = [ch(r), ch(g), ch(b), (a / n.max(1)) as u8];
            set(&mut out, x as i32, y as i32, px);
        }
    }
    out
}

/// Recolour onto a ramp by luminance: dark pixels get the dark end, light
/// pixels the light end; `bias` (−1..1) shifts the mapping.
pub fn remap(c: &mut Canvas, ramp: &[Rgba], bias: f32) {
    let n = ramp.len().max(1) as f32;
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let p = c.get(x, y);
            if p[3] == 0 {
                continue;
            }
            let l = (luminance(p) + bias).clamp(0.0, 0.999);
            let mut q = ramp[(l * n) as usize];
            q[3] = p[3];
            set(c, x, y, q);
        }
    }
}

/// 1-px outline around every opaque pixel.
pub fn outline(c: &mut Canvas, color: Rgba) {
    c.outline(color);
}

/// `rugen pixelize` entry: decode, downscale, quantise, outline, encode.
pub fn pixelize_file(
    input: &Path,
    output: &Path,
    width: Option<u32>,
    palette: &[Rgba],
    dither: bool,
    outline_color: Option<Rgba>,
) -> anyhow::Result<(u32, u32)> {
    let (w, h, rgba) = if input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        crate::kit::read_png_rgba(input)?
    } else {
        crate::uiscreen::load_image(input)?
    };
    let src = Canvas::from_rgba(w, h, rgba);
    let mut c = match width {
        Some(tw) => downscale(&src, tw),
        None => src,
    };
    quantize(&mut c, palette, dither, 0.18);
    if let Some(col) = outline_color {
        outline(&mut c, col);
    }
    write_webp(output, &c)?;
    Ok((c.w, c.h))
}

pub fn write_webp(path: &Path, c: &Canvas) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    image_webp::WebPEncoder::new(file)
        .encode(&c.px, c.w, c.h, image_webp::ColorType::Rgba8)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_hits_palette_only() {
        let mut c = Canvas::new(8, 8);
        c.vgrad(0, 0, 8, 8, [0, 0, 0, 255], [255, 255, 255, 255]);
        let pal = full_palette();
        quantize(&mut c, &pal, true, 0.2);
        for y in 0..8 {
            for x in 0..8 {
                assert!(pal.contains(&c.get(x, y)));
            }
        }
    }

    #[test]
    fn downscale_halves() {
        let mut c = Canvas::new(16, 8);
        c.rect(0, 0, 8, 8, [255, 0, 0, 255]);
        c.rect(8, 0, 8, 8, [0, 0, 255, 255]);
        let d = downscale(&c, 2);
        assert_eq!((d.w, d.h), (2, 1));
        assert_eq!(d.get(0, 0)[0], 255);
        assert_eq!(d.get(1, 0)[2], 255);
    }

    #[test]
    fn remap_keeps_alpha_and_uses_ramp_ends() {
        let mut c = Canvas::new(2, 1);
        c.put(0, 0, [10, 10, 10, 255]);
        c.put(1, 0, [250, 250, 250, 200]);
        remap(&mut c, &GOLD, 0.0);
        assert_eq!(c.get(0, 0), GOLD[0]);
        let light = c.get(1, 0);
        assert_eq!(light[..3], GOLD[5][..3]);
        assert_eq!(light[3], 200);
    }

    #[test]
    fn gpl_round_trip() {
        let dir = std::env::temp_dir().join("rugen-gpl-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("p.gpl");
        std::fs::write(&p, gpl()).unwrap();
        let pal = load_gpl(&p).unwrap();
        assert_eq!(pal.len(), full_palette().len());
    }
}
