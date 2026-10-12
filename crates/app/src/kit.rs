//! Art for the front end: CC0 kit pieces (Kenney "UI Pack - Pixel Adventure",
//! `motif/kit/`) recoloured onto the rugen palette, plus pixel backgrounds,
//! the logo and feature art drawn with real pixel technique (dithered
//! gradients, 1-px rims, palette only). Everything is drawn at 1x on a
//! 640x360 grid and shown at 2x by the screens.

use std::path::Path;

use crate::fontgen::Generator;
use crate::pixelize::{self, BLACK, BLUE, GOLD, GREEN, MAROON, NAVY, RED, Rgba, STEEL, TEAL};
use crate::screenpack::Canvas;

const KIT: &str = "kit/kenney-pixel-adventure";

/// Load a kit tile (`large/tile_0003.png` → index 3 of the 32x32 set).
pub fn tile(motif: &Path, set: &str, index: u32) -> anyhow::Result<Canvas> {
    let p = motif
        .join(KIT)
        .join(set)
        .join(format!("tile_{index:04}.png"));
    let (w, h, rgba) = read_png_rgba(&p)
        .map_err(|e| anyhow::anyhow!("{}: {e} (is the CC0 kit in motif/kit?)", p.display()))?;
    Ok(Canvas::from_rgba(w, h, rgba))
}

/// Decode any PNG (palette, grey, 16-bit...) to straight 8-bit RGBA.
pub fn read_png_rgba(path: &Path) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => anyhow::bail!("palette not expanded"),
    };
    anyhow::ensure!(
        rgba.len() == (w * h * 4) as usize,
        "unexpected pixel buffer size"
    );
    Ok((w, h, rgba))
}

fn clone(c: &Canvas) -> Canvas {
    Canvas::from_rgba(c.w, c.h, c.px.clone())
}

/// Expand a tile into `w x h` keeping its corners: edges and the middle are
/// repeated (not stretched) so the pixel look stays crisp.
pub fn nine_slice(tile: &Canvas, inset: u32, w: u32, h: u32) -> Canvas {
    let (tw, th) = (tile.w as i32, tile.h as i32);
    let i = inset as i32;
    let mut out = Canvas::new(w, h);
    let (w, h) = (w as i32, h as i32);
    let map = |v: i32, size: i32, tsize: i32| -> i32 {
        if v < i {
            v
        } else if v >= size - i {
            tsize - (size - v)
        } else {
            i + (v - i) % (tsize - 2 * i).max(1)
        }
    };
    for y in 0..h {
        let sy = map(y, h, th);
        for x in 0..w {
            let sx = map(x, w, tw);
            out.put(x, y, tile.get(sx, sy));
        }
    }
    out
}

/// Shear into a parallelogram: row `y` shifts right by `skew * (h-1-y) / (h-1)`.
pub fn shear(c: &Canvas, skew: u32) -> Canvas {
    let (w, h) = (c.w as i32, c.h as i32);
    let mut out = Canvas::new(c.w + skew, c.h);
    for y in 0..h {
        let dx = skew as i32 * (h - 1 - y) / (h - 1).max(1);
        for x in 0..w {
            out.put(x + dx, y, c.get(x, y));
        }
    }
    out
}

fn remapped(
    motif: &Path,
    set: &str,
    index: u32,
    ramp: &[Rgba],
    bias: f32,
) -> anyhow::Result<Canvas> {
    let mut t = tile(motif, set, index)?;
    pixelize::remap(&mut t, ramp, bias);
    Ok(t)
}

/// Draw `top` over `base` (alpha-aware, 1-bit).
fn over(base: &mut Canvas, top: &Canvas, x: i32, y: i32) {
    base.blit(top, x, y);
}

/// A 1-px diagonal highlight band across opaque pixels (`d = x + y`).
fn slash_highlight(c: &mut Canvas, d0: i32, width: i32, color: Rgba) {
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let p = c.get(x, y);
            if p[3] == 0 {
                continue;
            }
            let d = x + y;
            if d >= d0 && d < d0 + width {
                c.put(x, y, [color[0], color[1], color[2], p[3]]);
            }
        }
    }
}

// ---------------------------------------------------------------- panels

/// 9-slice panel 24x24 (`nine = [8,8,8,8]`): navy bevel.
pub fn panel(motif: &Path) -> anyhow::Result<Canvas> {
    let t = remapped(motif, "large", 3, &NAVY, -0.08)?;
    Ok(nine_slice(&t, 6, 24, 24))
}

/// Big art plate 32x32 (`nine = [10,10,10,10]`): dark fill + gold frame.
pub fn art_frame(motif: &Path) -> anyhow::Result<Canvas> {
    let mut base = remapped(motif, "large", 3, &NAVY, -0.18)?;
    let frame = remapped(motif, "large", 32, &GOLD, -0.05)?;
    over(&mut base, &frame, 0, 0);
    Ok(base)
}

/// Player card 32x32 (`nine = [10,10,10,10]`): steel bevel, gold base rule.
pub fn card(motif: &Path) -> anyhow::Result<Canvas> {
    let mut c = remapped(motif, "large", 8, &NAVY, 0.02)?;
    c.rect(2, 28, 28, 2, GOLD[2]);
    c.rect(2, 27, 28, 1, GOLD[4]);
    Ok(c)
}

/// Slanted menu button 197x27 (shown at 2x = 394x54).
pub fn button(motif: &Path, lit: bool) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (190, 27, 7);
    let t = if lit {
        remapped(motif, "large", 1, &MAROON, 0.3)?
    } else {
        remapped(motif, "large", 1, &MAROON, -0.1)?
    };
    let mut c = nine_slice(&t, 8, w, h);
    if lit {
        slash_highlight(&mut c, w as i32 - 34, 3, MAROON[5]);
        slash_highlight(&mut c, w as i32 - 26, 1, MAROON[5]);
    }
    let mut s = shear(&c, skew);
    // ground shadow
    s.rect(0, h as i32 - 1, (w + skew) as i32, 1, [0, 0, 0, 110]);
    Ok(s)
}

/// Settings row 379x20 (shown at 2x).
pub fn row(motif: &Path, lit: bool) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (374, 20, 5);
    let t = if lit {
        remapped(motif, "large", 2, &MAROON, 0.12)?
    } else {
        remapped(motif, "large", 2, &NAVY, -0.02)?
    };
    let c = nine_slice(&t, 6, w, h);
    Ok(shear(&c, skew))
}

/// Slanted name plate 220x22 (shown at 2x).
pub fn plate(motif: &Path, lit: bool) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (214, 22, 6);
    let t = if lit {
        remapped(motif, "large", 1, &MAROON, 0.22)?
    } else {
        remapped(motif, "large", 2, &NAVY, 0.06)?
    };
    let c = nine_slice(&t, 6, w, h);
    Ok(shear(&c, skew))
}

/// Roster cell 38x38: `normal`, `lit` (gold frame), `p1` / `p2` (hollow frames).
pub fn cell(motif: &Path, kind: &str) -> anyhow::Result<Canvas> {
    let n = 38;
    Ok(match kind {
        "lit" => {
            let mut base = remapped(motif, "large", 3, &NAVY, 0.08)?;
            base = nine_slice(&base, 6, n, n);
            let f = remapped(motif, "large", 32, &GOLD, 0.1)?;
            over(&mut base, &nine_slice(&f, 6, n, n), 0, 0);
            base
        }
        "p1" => {
            let f = remapped(motif, "large", 32, &MAROON, 0.3)?;
            let mut c = nine_slice(&f, 6, n, n);
            c.rect(0, 0, 9, 9, MAROON[4]);
            c.rect(0, 0, 9, 1, MAROON[5]);
            c
        }
        "p2" => {
            let f = remapped(motif, "large", 32, &BLUE, 0.15)?;
            let mut c = nine_slice(&f, 6, n, n);
            c.rect(n as i32 - 9, 0, 9, 9, BLUE[2]);
            c.rect(n as i32 - 9, 0, 9, 1, BLUE[3]);
            c
        }
        _ => {
            let base = remapped(motif, "large", 3, &NAVY, -0.12)?;
            nine_slice(&base, 6, n, n)
        }
    })
}

/// Verification badge 7x7: palette diamond with a black rim.
pub fn badge(color: Rgba) -> Canvas {
    let mut c = Canvas::new(7, 7);
    for y in 0..7 {
        let half = 3 - (y - 3i32).abs();
        c.rect(3 - half, y, half * 2 + 1, 1, color);
    }
    c.put(3, 1, [255, 255, 255, 200]);
    c.outline(BLACK);
    c
}

/// Default avatar 28x28: steel circle with a pixel bust.
pub fn avatar(motif: &Path) -> anyhow::Result<Canvas> {
    let mut c = remapped(motif, "large", 42, &NAVY, 0.0)?;
    let c2 = pixelize::downscale(&c, 28);
    c = c2;
    // bust: head, shoulders, headband
    c.rect(11, 7, 6, 7, STEEL[4]);
    c.rect(10, 8, 1, 4, STEEL[3]);
    c.rect(17, 8, 1, 4, STEEL[3]);
    c.rect(11, 9, 6, 1, MAROON[3]);
    c.rect(8, 15, 12, 6, MAROON[2]);
    c.rect(8, 15, 12, 1, MAROON[4]);
    c.rect(13, 15, 2, 6, GOLD[3]);
    Ok(c)
}

// ---------------------------------------------------------------- backgrounds

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed;
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}

/// Sky layer 640x360: dithered night gradient, moon, stars.
pub fn bg_sky() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    c.vgrad(0, 0, w as i32, 240, NAVY[0], NAVY[2]);
    c.vgrad(0, 240, w as i32, 120, NAVY[2], NAVY[1]);
    // stars: small, a few crosses
    for y in 0..300 {
        for x in 0..w as i32 {
            let r = hash(x, y, 7);
            if r < 0.0035 {
                c.put(x, y, if r < 0.0012 { STEEL[5] } else { STEEL[3] });
            } else if r > 0.99985 {
                c.put(x, y, STEEL[5]);
                c.put(x - 1, y, STEEL[4]);
                c.put(x + 1, y, STEEL[4]);
                c.put(x, y - 1, STEEL[4]);
                c.put(x, y + 1, STEEL[4]);
            }
        }
    }
    // moon with a dithered terminator
    let (mx, my, r) = (418, 66, 26);
    for y in -r..=r {
        for x in -r..=r {
            let d2 = x * x + y * y;
            if d2 > r * r {
                continue;
            }
            let shade = (x + r) as f32 / (2 * r) as f32; // light on the right
            let col = if d2 > (r - 1) * (r - 1) {
                GOLD[1]
            } else if shade + (hash(x, y, 3) - 0.5) * 0.3 > 0.55 {
                GOLD[4]
            } else {
                GOLD[3]
            };
            c.put(mx + x, my + y, col);
        }
    }
    // craters
    for (cx, cy, cr) in [(-10, -6, 5), (8, 10, 4), (12, -12, 3)] {
        for y in -cr..=cr {
            for x in -cr..=cr {
                if x * x + y * y <= cr * cr {
                    c.put(mx + cx + x, my + cy + y, GOLD[2]);
                }
            }
        }
    }
    c
}

fn ridge(x: i32, seed: u32, base: f32, amp: f32) -> i32 {
    let fx = x as f32;
    let v = base
        + amp
            * (0.5 * libm::sinf(fx * 0.011 + seed as f32)
                + 0.3 * libm::sinf(fx * 0.027 + seed as f32 * 1.7)
                + 0.2 * libm::sinf(fx * 0.061 + seed as f32 * 0.4));
    v as i32
}

/// Far layer 640x360 (transparent): two mountain silhouettes with fog.
pub fn bg_far() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    for x in 0..w as i32 {
        let top = ridge(x, 11, 212.0, 26.0);
        for y in top..h as i32 {
            let fog = (y - top) < 14 && hash(x, y, 5) < 0.5 * (1.0 - (y - top) as f32 / 14.0);
            c.put(x, y, if fog { NAVY[4] } else { NAVY[3] });
        }
        c.put(x, top, NAVY[4]);
    }
    for x in 0..w as i32 {
        let top = ridge(x, 29, 252.0, 20.0);
        for y in top..h as i32 {
            let fog = (y - top) < 10 && hash(x, y, 9) < 0.5 * (1.0 - (y - top) as f32 / 10.0);
            c.put(x, y, if fog { NAVY[3] } else { NAVY[2] });
        }
        c.put(x, top, NAVY[3]);
    }
    c
}

/// Near layer 640x360 (transparent): arena floor, pillars, light shafts.
pub fn bg_near() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    // light shafts at 45 degrees
    for (x0, width, a) in [(-120, 36, 22u8), (60, 18, 16), (330, 54, 14), (500, 26, 18)] {
        for y in 0..h as i32 {
            let x = x0 + y;
            for xx in x..x + width {
                if hash(xx, y, 21) < 0.55 {
                    c.put(xx, y, [STEEL[5][0], STEEL[5][1], STEEL[5][2], a]);
                }
            }
        }
    }
    // floor
    c.vgrad(0, 300, w as i32, 60, NAVY[1], NAVY[0]);
    c.rect(0, 300, w as i32, 1, MAROON[2]);
    c.rect(0, 301, w as i32, 1, MAROON[1]);
    for x in (0..w as i32).step_by(32) {
        c.rect(x, 302, 1, 58, NAVY[2]);
    }
    // torii-like gate silhouettes left and right
    for gx in [38, 548] {
        c.rect(gx, 150, 8, 152, NAVY[0]);
        c.rect(gx + 44, 150, 8, 152, NAVY[0]);
        c.rect(gx - 10, 142, 72, 7, NAVY[0]);
        c.rect(gx - 14, 136, 80, 6, MAROON[1]);
        c.rect(gx + 2, 162, 48, 4, NAVY[0]);
        c.rect(gx - 14, 135, 80, 1, MAROON[3]);
    }
    c
}

/// All three layers merged (for screens that only take one background).
pub fn bg_composite() -> Canvas {
    let mut c = bg_sky();
    over(&mut c, &bg_far(), 0, 0);
    over(&mut c, &bg_near(), 0, 0);
    c
}

// ---------------------------------------------------------------- logo and feature art

/// "RUGEN" wordmark: gold two-tone bevel, slash highlight, maroon outline,
/// navy drop shadow, red under-bar.
pub fn logo(g: &Generator) -> Canvas {
    let (w, h, px) = g.text_rgba("RUGEN", 7, 2);
    let text = Canvas::from_rgba(w, h, px);
    let mut c = Canvas::new(w + 8, h + 12);
    // drop shadow (offset copy, navy)
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let p = text.get(x, y);
            if p[3] > 0 {
                c.put(x + 3, y + 3, NAVY[0]);
            }
        }
    }
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let p = text.get(x, y);
            if p[3] == 0 {
                continue;
            }
            let face = p[0] > 200;
            let col = if face {
                if y < (h as i32) * 9 / 20 {
                    GOLD[4]
                } else {
                    GOLD[3]
                }
            } else {
                MAROON[0]
            };
            c.put(x, y, col);
        }
    }
    slash_highlight(&mut c, (w as i32) / 2 + 2, 4, GOLD[5]);
    // under-bar
    let bar_y = h as i32 + 4;
    c.para_grad(6, bar_y, w as i32 - 16, 5, 4, MAROON[4], MAROON[2]);
    c.rect(6 + 4, bar_y, w as i32 - 16, 1, GOLD[4]);
    c
}

/// Feature art: dusk arena (sun, ridges, platform, two silhouettes).
pub fn news_arena(g: &Generator, w: u32, h: u32, caption: &str) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.vgrad(0, 0, wi, hi * 2 / 3, NAVY[1], MAROON[2]);
    c.vgrad(0, hi * 2 / 3, wi, hi / 3, MAROON[2], GOLD[1]);
    // sun
    let (sx, sy, r) = (wi * 2 / 3, hi / 2, hi / 4);
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                let col = if y > r / 3 && (y / 2) % 2 == 0 {
                    GOLD[2]
                } else {
                    GOLD[3]
                };
                c.put(sx + x, sy + y, col);
            }
        }
    }
    for x in 0..wi {
        let top = ridge(x, 3, hi as f32 * 0.62, hi as f32 * 0.12);
        for y in top..hi {
            c.put(x, y, NAVY[1]);
        }
        c.put(x, top, NAVY[3]);
        let top2 = ridge(x, 17, hi as f32 * 0.8, hi as f32 * 0.07);
        for y in top2..hi {
            c.put(x, y, NAVY[0]);
        }
    }
    // platform with two fighters
    let py = hi - 14;
    c.rect(wi / 2 - 40, py, 80, 4, STEEL[2]);
    c.rect(wi / 2 - 40, py, 80, 1, GOLD[3]);
    for (fx, col) in [(wi / 2 - 22, MAROON[4]), (wi / 2 + 14, BLUE[2])] {
        c.rect(fx + 2, py - 12, 4, 4, col);
        c.rect(fx + 1, py - 8, 6, 5, col);
        c.rect(fx + 1, py - 3, 2, 3, col);
        c.rect(fx + 5, py - 3, 2, 3, col);
    }
    c.slash(wi / 3, 10, [255, 255, 255, 28]);
    caption_on(&mut c, g, caption);
    c.outline(BLACK);
    c
}

/// Feature art: floating platforms under stars.
pub fn news_platforms(g: &Generator, w: u32, h: u32, caption: &str) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.vgrad(0, 0, wi, hi, NAVY[0], NAVY[2]);
    for y in 0..hi {
        for x in 0..wi {
            if hash(x, y, 41) < 0.01 {
                c.put(x, y, STEEL[4]);
            }
        }
    }
    for (px, py, pw) in [
        (8, hi - 14, wi / 3),
        (wi / 2 - 10, hi / 2, wi / 3),
        (wi * 2 / 3, hi - 22, wi / 4),
    ] {
        c.rect(px, py, pw, 6, NAVY[3]);
        c.rect(px, py, pw, 1, GOLD[3]);
        c.rect(px, py + 1, pw, 1, GOLD[2]);
        c.rect(px, py + 5, pw, 1, NAVY[1]);
    }
    caption_on(&mut c, g, caption);
    c.outline(BLACK);
    c
}

/// Feature art: network nodes.
pub fn news_online(g: &Generator, w: u32, h: u32, caption: &str) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.vgrad(0, 0, wi, hi, TEAL[0], NAVY[1]);
    let nodes = [
        (12, 12),
        (wi - 16, 10),
        (wi / 2, hi / 2 + 4),
        (16, hi - 12),
        (wi - 14, hi - 10),
    ];
    for (i, &(ax, ay)) in nodes.iter().enumerate() {
        for &(bx, by) in nodes.iter().skip(i + 1) {
            let steps = (ax - bx).abs().max((ay - by).abs()).max(1);
            for s in 0..=steps {
                if s % 3 == 0 {
                    continue;
                }
                let x = ax + (bx - ax) * s / steps;
                let y = ay + (by - ay) * s / steps;
                c.put(x, y, TEAL[2]);
            }
        }
    }
    for &(x, y) in &nodes {
        c.rect(x - 2, y - 2, 5, 5, TEAL[4]);
        c.rect(x - 1, y - 1, 3, 3, TEAL[3]);
    }
    caption_on(&mut c, g, caption);
    c.outline(BLACK);
    c
}

fn caption_on(c: &mut Canvas, g: &Generator, caption: &str) {
    let (tw, th, px) = g.text_rgba(caption, 1, 2);
    let t = Canvas::from_rgba(tw, th, px);
    let x = (c.w as i32 - tw as i32) / 2;
    let y = (c.h as i32 - th as i32) / 2;
    c.rect(x - 6, y - 3, tw as i32 + 12, th as i32 + 6, [0, 0, 0, 120]);
    c.blit(&t, x, y);
}

// ---------------------------------------------------------------- kit export

/// Recolour every kit tile onto the palette and write `<theme>/ui/kit/*.webp`
/// for modders (plus the role assets built by `ui-gen`).
pub fn import(motif: &Path, theme: &Path) -> anyhow::Result<usize> {
    let out = theme.join("ui").join("kit");
    std::fs::create_dir_all(&out)?;
    let mut n = 0;
    let large = [
        (1, "block", "maroon", &MAROON[..]),
        (2, "flat", "navy", &NAVY[..]),
        (3, "bevel", "navy", &NAVY[..]),
        (8, "frame", "steel", &STEEL[..]),
        (9, "frame2", "steel", &STEEL[..]),
        (32, "hollow", "gold", &GOLD[..]),
        (41, "circle", "steel", &STEEL[..]),
        (42, "circle_dark", "navy", &NAVY[..]),
        (43, "banner_l", "maroon", &MAROON[..]),
        (44, "banner_m", "maroon", &MAROON[..]),
        (45, "banner_r", "maroon", &MAROON[..]),
        (64, "hex", "gold", &GOLD[..]),
        (66, "hex_dark", "navy", &NAVY[..]),
        (67, "hex_steel", "steel", &STEEL[..]),
        (79, "hex_framed", "gold", &GOLD[..]),
        (82, "bar_l", "gold", &GOLD[..]),
        (83, "bar_m", "gold", &GOLD[..]),
        (84, "bar_r", "gold", &GOLD[..]),
    ];
    for (idx, name, ramp_name, ramp) in large {
        let c = remapped(motif, "large", idx, ramp, 0.0)?;
        pixelize::write_webp(&out.join(format!("{name}_{ramp_name}.webp")), &c)?;
        n += 1;
    }
    for idx in [
        3u32, 4, 8, 9, 24, 25, 26, 35, 36, 37, 38, 44, 45, 56, 57, 58,
    ] {
        let Ok(c) = remapped(motif, "small", idx, &STEEL, 0.0) else {
            continue;
        };
        pixelize::write_webp(&out.join(format!("icon_{idx:02}.webp")), &c)?;
        n += 1;
    }
    // extra recolours modders asked for
    for (idx, name, ramp) in [
        (66u32, "hex_green", &GREEN[..]),
        (66, "hex_red", &RED[..]),
        (66, "hex_blue", &BLUE[..]),
    ] {
        let c = remapped(motif, "large", idx, ramp, 0.0)?;
        pixelize::write_webp(&out.join(format!("{name}.webp")), &c)?;
        n += 1;
    }
    let _ = clone;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_slice_keeps_corners_and_repeats_middle() {
        let mut t = Canvas::new(8, 8);
        t.rect(0, 0, 8, 8, [1, 1, 1, 255]);
        t.rect(0, 0, 1, 1, [9, 9, 9, 255]);
        t.rect(3, 3, 2, 2, [5, 5, 5, 255]);
        let n = nine_slice(&t, 3, 20, 10);
        assert_eq!(n.get(0, 0)[0], 9);
        assert_eq!(n.get(19, 9)[0], 1);
        assert_eq!(n.get(10, 4)[0], 5);
    }

    #[test]
    fn shear_widens_by_skew() {
        let mut t = Canvas::new(4, 4);
        t.rect(0, 0, 4, 4, [1, 1, 1, 255]);
        let s = shear(&t, 3);
        assert_eq!(s.w, 7);
        assert_eq!(s.get(3, 0)[3], 255);
        assert_eq!(s.get(0, 0)[3], 0);
        assert_eq!(s.get(0, 3)[3], 255);
    }

    #[test]
    fn backgrounds_use_the_palette() {
        let pal = pixelize::full_palette();
        for c in [bg_sky(), bg_far()] {
            for y in (0..c.h as i32).step_by(7) {
                for x in (0..c.w as i32).step_by(11) {
                    let p = c.get(x, y);
                    if p[3] == 255 {
                        assert!(pal.contains(&p), "{p:?} at {x},{y}");
                    }
                }
            }
        }
    }
}
