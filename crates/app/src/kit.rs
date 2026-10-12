//! Art for the front end: CC0 kit pieces (Kenney "UI Pack - Pixel Adventure",
//! `motif/kit/`) recoloured onto the rugen palette, plus pixel backgrounds,
//! the logo and feature art drawn with real pixel technique (dithered
//! gradients, 1-px rims, palette only). Everything is drawn at 1x on a
//! 640x360 grid and shown at 2x by the screens.

use std::path::Path;

use crate::fontgen::Generator;
use crate::pixelize::{self, BLACK, BLUE, GOLD, GREEN, MAROON, NAVY, RED, Rgba, STEEL};
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

// ---------------------------------------------------------------- Neon Dojo palette

/// Backdrop: near-black navy.
pub const DJ_BG0: Rgba = [13, 15, 26, 255];
pub const DJ_BG1: Rgba = [19, 26, 44, 255];
/// Dark 45° slashes.
pub const DJ_SLASH0: Rgba = [26, 34, 54, 255];
pub const DJ_SLASH1: Rgba = [35, 44, 69, 255];
/// Glossy panels: top, mid, bottom, rims.
pub const DJ_PANEL_HI: Rgba = [47, 59, 92, 255];
pub const DJ_PANEL: Rgba = [28, 34, 54, 255];
pub const DJ_PANEL_LO: Rgba = [17, 22, 39, 255];
pub const DJ_RIM: Rgba = [59, 74, 107, 255];
pub const DJ_RIM_HI: Rgba = [90, 109, 147, 255];
pub const DJ_EDGE: Rgba = [6, 6, 12, 255];
/// Accent red (active, P1), cyan (P2), gold (numbers).
pub const DJ_RED: Rgba = [199, 31, 59, 255];
pub const DJ_RED_HI: Rgba = [255, 95, 122, 255];
pub const DJ_RED_LO: Rgba = [122, 16, 36, 255];
pub const DJ_RED_LINE: Rgba = [255, 46, 74, 255];
pub const DJ_CYAN: Rgba = [57, 192, 255, 255];
pub const DJ_CYAN_LO: Rgba = [26, 106, 153, 255];
pub const DJ_GOLD: Rgba = [255, 179, 71, 255];
pub const DJ_TEXT: Rgba = [207, 214, 230, 255];

/// Glossy slanted slab: dithered gradient, lighter top band, 1-px dark edge
/// and a 1-px rim inside it, plus a highlight line under the top rim.
fn slab(w: i32, h: i32, skew: i32, top: Rgba, mid: Rgba, bottom: Rgba, rim: Rgba) -> Canvas {
    let mut c = Canvas::new((w + skew) as u32, h as u32);
    c.para_grad(0, 0, w, h, skew, mid, bottom);
    c.para_grad(0, 0, w, (h / 2).max(1), skew, top, mid);
    c.para_edge(0, 0, w, h, skew, DJ_EDGE);
    c.para_edge(1, 1, w - 2, h - 2, skew, rim);
    // gloss line under the rim
    let dx = skew * (h - 1 - 2) / (h - 1).max(1);
    c.rect(2 + dx, 2, w - 4, 1, lerp(top, [255, 255, 255, 255], 0.25));
    c
}

fn lerp(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3])]
}

/// Thin 45° highlight across the slab (the mock's "slash" on the active button).
fn slab_slash(c: &mut Canvas, x0: i32, width: i32, color: Rgba) {
    for y in 0..c.h as i32 {
        for x in x0 + y / 2..x0 + y / 2 + width {
            let p = c.get(x, y);
            if p[3] > 0 {
                c.put(x, y, color);
            }
        }
    }
}

// ---------------------------------------------------------------- panels

/// 9-slice panel 24x24 (`nine = [8,8,8,8]`): dark glossy with a steel-blue rim.
pub fn panel(_motif: &Path) -> anyhow::Result<Canvas> {
    let mut c = Canvas::new(24, 24);
    c.vgrad(0, 0, 24, 24, DJ_PANEL, DJ_PANEL_LO);
    c.vgrad(0, 0, 24, 8, DJ_PANEL_HI, DJ_PANEL);
    c.outline(DJ_EDGE);
    c.rect(1, 1, 22, 1, DJ_RIM_HI);
    c.rect(1, 22, 22, 1, DJ_RIM);
    c.rect(1, 1, 1, 22, DJ_RIM);
    c.rect(22, 1, 1, 22, DJ_RIM);
    Ok(c)
}

/// Big art plate 200x220 (shown at 2x = 400x440): dark cyan-tinted gloss,
/// red 2-px frame, bottom-right corner cut diagonally.
pub fn art_frame(_motif: &Path) -> anyhow::Result<Canvas> {
    let (w, h) = (200, 220);
    let mut c = Canvas::new(w as u32, h as u32);
    c.vgrad(0, 0, w, h, [24, 44, 66, 255], DJ_PANEL_LO);
    c.vgrad(0, 0, w, 40, [34, 70, 98, 255], [24, 44, 66, 255]);
    // faint diagonal light
    for y in 0..h {
        for x in (w - 60 + y / 2)..(w - 50 + y / 2) {
            let p = c.get(x, y);
            if p[3] > 0 {
                c.put(x, y, lerp(p, [255, 255, 255, 255], 0.06));
            }
        }
    }
    let cut = 28;
    // frame: deep outer line, 2 px red, light top line
    c.outline(DJ_RED_LO);
    c.rect(1, 1, w - 2, 2, DJ_RED);
    c.rect(1, h - 3, w - 2, 2, DJ_RED);
    c.rect(1, 1, 2, h - 2, DJ_RED);
    c.rect(w - 3, 1, 2, h - 2, DJ_RED);
    c.rect(1, 1, w - 2, 1, DJ_RED_HI);
    // diagonal cut (bottom-right): clear the triangle, then redraw the edge
    for k in 0..cut {
        let y = h - cut + k;
        for x in (w - k - 1)..w {
            c.put_raw(x, y, [0, 0, 0, 0]);
        }
    }
    for k in 0..cut {
        let y = h - cut + k;
        c.put(w - k - 1, y, DJ_RED_LO);
        c.put(w - k - 2, y, DJ_RED);
        c.put(w - k - 3, y, DJ_RED);
    }
    Ok(c)
}

/// Stats card 215x60 (shown at 2x = 430x120): skewed dark glossy card with
/// a gold base rule.
pub fn card(_motif: &Path) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (216, 60, 6);
    let mut c = slab(w, h, skew, DJ_PANEL_HI, DJ_PANEL, DJ_PANEL_LO, DJ_RIM);
    c.rect(2, h - 3, w - 4, 1, DJ_GOLD);
    c.rect(2, h - 2, w - 4, 1, [196, 128, 40, 255]);
    Ok(c)
}

/// Profile strip 280x38 (shown at 2x = 560x76): skewed dark panel with a red
/// avatar square on the left.
pub fn strip(_motif: &Path) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (273, 38, 7);
    let mut c = slab(w, h, skew, DJ_PANEL_HI, DJ_PANEL, DJ_PANEL_LO, DJ_RIM);
    let dx = |y: i32| skew * (h - 1 - y) / (h - 1);
    for y in 5..33 {
        c.rect(5 + dx(y), y, 28, 1, DJ_RED);
    }
    c.rect(5 + dx(5), 5, 28, 1, DJ_RED_HI);
    for y in 5..33 {
        c.put(5 + dx(y), y, DJ_RED_LO);
        c.put(32 + dx(y), y, DJ_RED_LO);
    }
    Ok(c)
}

/// Slanted menu button 197x27 (shown at 2x = 394x54): navy slab, red when lit.
pub fn button(_motif: &Path, lit: bool) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (190, 27, 7);
    let mut c = if lit {
        slab(w, h, skew, [230, 60, 90, 255], DJ_RED, DJ_RED_LO, DJ_RED_HI)
    } else {
        slab(w, h, skew, DJ_PANEL_HI, DJ_PANEL, DJ_PANEL_LO, DJ_RIM)
    };
    if lit {
        slab_slash(&mut c, w - 44, 2, [255, 200, 210, 255]);
        slab_slash(&mut c, w - 36, 1, [255, 150, 170, 255]);
    }
    c.rect(0, h - 1, w + skew, 1, [0, 0, 0, 120]);
    Ok(c)
}

/// Settings row 379x20 (shown at 2x).
pub fn row(_motif: &Path, lit: bool) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (374, 20, 5);
    Ok(if lit {
        slab(w, h, skew, [230, 60, 90, 255], DJ_RED, DJ_RED_LO, DJ_RED_HI)
    } else {
        slab(w, h, skew, DJ_PANEL, DJ_PANEL_LO, DJ_PANEL_LO, DJ_RIM)
    })
}

/// Slanted name plate 220x22 (shown at 2x).
pub fn plate(_motif: &Path, lit: bool) -> anyhow::Result<Canvas> {
    let (w, h, skew) = (214, 22, 6);
    Ok(if lit {
        slab(w, h, skew, [230, 60, 90, 255], DJ_RED, DJ_RED_LO, DJ_RED_HI)
    } else {
        slab(w, h, skew, DJ_PANEL_HI, DJ_PANEL, DJ_PANEL_LO, DJ_RIM)
    })
}

/// Roster cell 38x38: `normal`, `lit` (gold rim), `p1` (red frame) / `p2` (cyan frame).
pub fn cell(_motif: &Path, kind: &str) -> anyhow::Result<Canvas> {
    let n = 38;
    let base = |rim: Rgba| {
        let mut c = Canvas::new(n as u32, n as u32);
        c.vgrad(0, 0, n, n, DJ_PANEL, DJ_PANEL_LO);
        c.vgrad(0, 0, n, 10, DJ_PANEL_HI, DJ_PANEL);
        c.outline(DJ_EDGE);
        c.rect(1, 1, n - 2, 1, rim);
        c.rect(1, n - 2, n - 2, 1, rim);
        c.rect(1, 1, 1, n - 2, rim);
        c.rect(n - 2, 1, 1, n - 2, rim);
        c
    };
    let frame = |col: Rgba, hi: Rgba, corner_left: bool| {
        let mut c = Canvas::new(n as u32, n as u32);
        for t in 0..2 {
            c.rect(t, t, n - 2 * t, 1, col);
            c.rect(t, n - 1 - t, n - 2 * t, 1, col);
            c.rect(t, t, 1, n - 2 * t, col);
            c.rect(n - 1 - t, t, 1, n - 2 * t, col);
        }
        c.rect(0, 0, n, 1, hi);
        let x = if corner_left { 0 } else { n - 10 };
        c.rect(x, 0, 10, 10, col);
        c.rect(x + 2, 2, 6, 6, hi);
        c
    };
    Ok(match kind {
        "lit" => base(DJ_GOLD),
        "p1" => frame(DJ_RED_LINE, DJ_RED_HI, true),
        "p2" => frame(DJ_CYAN, [170, 230, 255, 255], false),
        _ => base(DJ_RIM),
    })
}

/// Verification badge 7x7: diamond with a black rim.
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

/// Default avatar 28x28: red square, white pixel bust.
pub fn avatar(_motif: &Path) -> anyhow::Result<Canvas> {
    let mut c = Canvas::new(28, 28);
    c.vgrad(0, 0, 28, 28, DJ_RED, DJ_RED_LO);
    c.outline(DJ_RED_LO);
    c.rect(1, 1, 26, 1, DJ_RED_HI);
    c.rect(11, 6, 6, 7, DJ_TEXT);
    c.rect(10, 7, 1, 4, [160, 170, 190, 255]);
    c.rect(17, 7, 1, 4, [160, 170, 190, 255]);
    c.rect(11, 8, 6, 1, DJ_BG0);
    c.rect(8, 15, 12, 7, DJ_TEXT);
    c.rect(8, 15, 12, 1, [255, 255, 255, 255]);
    c.rect(13, 15, 2, 7, DJ_RED_LO);
    Ok(c)
}

// ---------------------------------------------------------------- backgrounds

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed;
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}

/// Diagonal band (top-left → bottom-right, 2:1 like the mock's 28°) across `c`.
fn band(c: &mut Canvas, x0: i32, width: i32, color: Rgba) {
    for y in 0..c.h as i32 {
        c.rect(x0 + y / 2, y, width, 1, color);
    }
}

/// Base layer 640x360: navy-to-black diagonal gloss with faint dust.
pub fn bg_sky() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let d = (x + y * 2) as f32 / (w as i32 + h as i32 * 2) as f32;
            let t = 1.0 - (d - 0.55).abs() * 2.2;
            let col = if hash(x, y, 1) < t.clamp(0.0, 1.0) {
                DJ_BG1
            } else {
                DJ_BG0
            };
            c.put(x, y, col);
        }
    }
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if hash(x, y, 5) < 0.004 {
                c.put(x, y, DJ_SLASH1);
            }
        }
    }
    c
}

/// Slash layer 640x360 (transparent): dark bands and one red cut.
pub fn bg_far() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    band(&mut c, -40, 70, DJ_SLASH0);
    band(&mut c, 70, 26, DJ_SLASH1);
    band(&mut c, 330, 90, DJ_SLASH0);
    band(&mut c, 450, 36, DJ_SLASH1);
    band(&mut c, 520, 14, DJ_RED_LO);
    band(&mut c, 534, 2, DJ_RED_LINE);
    band(&mut c, -220, 10, [70, 14, 24, 255]);
    band(&mut c, -210, 1, [160, 30, 48, 255]);
    c
}

/// Near layer 640x360 (transparent): dark floor band with a red rule.
pub fn bg_near() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    c.vgrad(0, 318, w as i32, (h - 318) as i32, DJ_PANEL_LO, DJ_BG0);
    c.rect(0, 317, w as i32, 1, DJ_RED_LO);
    c.rect(0, 316, w as i32, 1, DJ_RED);
    for x in (0..w as i32).step_by(40) {
        c.rect(x, 318, 1, (h - 318) as i32, DJ_BG1);
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

/// "RUGEN" wordmark: white italic face, 1-px dark outline, red offset
/// shadow, a red slash cut through the letters and a red under-bar.
pub fn logo(g: &Generator) -> Canvas {
    let (w, h, px) = g.text_rgba("RUGEN", 7, 2);
    let text = Canvas::from_rgba(w, h, px);
    let mut c = Canvas::new(w + 8, h + 12);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if text.get(x, y)[3] > 0 {
                c.put(x + 3, y + 3, DJ_RED_LO);
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
            c.put(x, y, if face { [255, 255, 255, 255] } else { DJ_EDGE });
        }
    }
    let x0 = (w as i32) * 2 / 5;
    for y in 0..h as i32 {
        for x in x0 + (h as i32 - y) / 2..x0 + (h as i32 - y) / 2 + 2 {
            let p = c.get(x, y);
            if p[3] > 0 && p[0] > 200 {
                c.put(x, y, DJ_RED_LINE);
            }
        }
    }
    let bar_y = h as i32 + 5;
    c.para_grad(6, bar_y, w as i32 - 12, 3, 3, DJ_RED_HI, DJ_RED);
    c
}

/// Feature art: glossy violet-to-maroon slab with a light slash, cut
/// bottom-left corner, two fighter silhouettes on a platform.
pub fn news_arena(_g: &Generator, w: u32, h: u32, _caption: &str) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.vgrad(0, 0, wi, hi, [43, 47, 90, 255], [90, 30, 58, 255]);
    c.vgrad(0, 0, wi, hi / 4, [60, 66, 120, 255], [43, 47, 90, 255]);
    for y in 0..hi {
        for x in (wi / 2 - 30 + y / 2)..(wi / 2 - 16 + y / 2) {
            let p = c.get(x, y);
            if p[3] > 0 {
                c.put(x, y, lerp(p, [255, 255, 255, 255], 0.14));
            }
        }
    }
    let py = hi * 3 / 4;
    c.rect(wi / 2 - 44, py, 88, 4, DJ_PANEL_HI);
    c.rect(wi / 2 - 44, py, 88, 1, DJ_GOLD);
    for (fx, col) in [(wi / 2 - 26, DJ_RED_LINE), (wi / 2 + 16, DJ_CYAN)] {
        c.rect(fx + 2, py - 12, 4, 4, col);
        c.rect(fx + 1, py - 8, 6, 5, col);
        c.rect(fx + 1, py - 3, 2, 3, col);
        c.rect(fx + 5, py - 3, 2, 3, col);
    }
    c.outline([74, 90, 133, 255]);
    cut_corner_bl(&mut c, 12);
    c
}

/// Feature tile: dark navy with floating platforms.
pub fn news_platforms(_g: &Generator, w: u32, h: u32, _caption: &str) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.vgrad(0, 0, wi, hi, DJ_PANEL, DJ_PANEL_LO);
    for y in 0..hi {
        for x in 0..wi {
            if hash(x, y, 41) < 0.012 {
                c.put(x, y, DJ_RIM_HI);
            }
        }
    }
    for (px, py, pw) in [
        (8, hi - 12, wi / 3),
        (wi / 2 - 10, hi / 2, wi / 3),
        (wi * 2 / 3, hi - 20, wi / 4),
    ] {
        c.rect(px, py, pw, 5, DJ_PANEL_HI);
        c.rect(px, py, pw, 1, DJ_GOLD);
        c.rect(px, py + 4, pw, 1, DJ_PANEL_LO);
    }
    c.outline(DJ_RIM);
    c
}

/// Feature tile: violet with a network of cyan nodes.
pub fn news_online(_g: &Generator, w: u32, h: u32, _caption: &str) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    c.vgrad(0, 0, wi, hi, [42, 27, 61, 255], DJ_PANEL_LO);
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
                c.put(
                    ax + (bx - ax) * s / steps,
                    ay + (by - ay) * s / steps,
                    DJ_CYAN_LO,
                );
            }
        }
    }
    for &(x, y) in &nodes {
        c.rect(x - 2, y - 2, 5, 5, DJ_CYAN);
        c.rect(x - 1, y - 1, 3, 3, [190, 235, 255, 255]);
    }
    c.outline(DJ_RIM);
    c
}

/// Remove a diagonal triangle at the bottom-left corner (`run` px).
fn cut_corner_bl(c: &mut Canvas, run: i32) {
    let h = c.h as i32;
    for k in 0..run {
        let y = h - run + k;
        for x in 0..k + 1 {
            c.put_raw(x, y, [0, 0, 0, 0]);
        }
        c.put(k + 1, y, [74, 90, 133, 255]);
    }
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
    fn backgrounds_are_opaque_where_drawn() {
        let sky = bg_sky();
        assert!((0..sky.h as i32).all(|y| sky.get(0, y)[3] == 255));
        let far = bg_far();
        assert_eq!(far.get(530, 10)[3], 255, "red cut");
        assert_eq!(far.get(300, 0)[3], 0, "gap between bands");
    }
}
