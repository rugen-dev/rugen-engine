//! `rugen ui-gen --theme dojo`: the Dojo theme. A training hall at night:
//! dark lacquer walls, wooden pillars, beams and plaques, washi sheets in
//! wooden frames, sumi type, vermilion hanko seals and a brass focus. All
//! pixel art drawn here at half resolution and doubled (2-px pixels), plus
//! the theme's fonts, a `system.def` / `fight.def` screenpack and the screen
//! TOMLs (`crates/app/src/dojoscreens/`).
//!
//! Everything lands in `motif/themes/dojo/`; files missing there fall back
//! to the flat `motif/`.

use std::path::Path;

use crate::fontgen::{self, Generator, Style};
use crate::pixelize;
use crate::screenpack::{Canvas, to_sprite};
use rugen_formats::sff::{Sff, Version};

type Rgba = [u8; 4];

// ---------------------------------------------------------------- palette
// The one place the dojo colours are defined. The screen TOMLs use the same
// values as floats; `palette.gpl` is written from this table.

pub const LACQUER_0: Rgba = [20, 13, 11, 255];
pub const LACQUER_1: Rgba = [34, 20, 15, 255];
pub const LACQUER_2: Rgba = [58, 35, 24, 255];
pub const WOOD_0: Rgba = [87, 51, 31, 255];
pub const WOOD_1: Rgba = [122, 72, 41, 255];
pub const WOOD_2: Rgba = [162, 102, 58, 255];
pub const WOOD_3: Rgba = [199, 142, 87, 255];
pub const WASHI_0: Rgba = [214, 196, 160, 255];
pub const WASHI_1: Rgba = [238, 225, 196, 255];
pub const WASHI_2: Rgba = [249, 241, 222, 255];
pub const SUMI: Rgba = [29, 23, 20, 255];
pub const SUMI_SOFT: Rgba = [99, 80, 61, 255];
pub const VERMILION: Rgba = [180, 48, 26, 255];
pub const VERMILION_2: Rgba = [217, 83, 47, 255];
pub const VERMILION_0: Rgba = [120, 30, 18, 255];
pub const GOLD: Rgba = [227, 179, 76, 255];
pub const GOLD_2: Rgba = [246, 217, 143, 255];
pub const MOSS: Rgba = [93, 138, 58, 255];
pub const AMBER: Rgba = [212, 138, 34, 255];
pub const MUTED: Rgba = [184, 160, 126, 255];

const CLEAR: Rgba = [0, 0, 0, 0];

/// `(name, colour)` of every palette entry, for `palette.gpl`.
pub const PALETTE: [(&str, Rgba); 20] = [
    ("lacquer 0", LACQUER_0),
    ("lacquer 1", LACQUER_1),
    ("lacquer 2", LACQUER_2),
    ("wood 0", WOOD_0),
    ("wood 1", WOOD_1),
    ("wood 2", WOOD_2),
    ("wood 3", WOOD_3),
    ("washi 0", WASHI_0),
    ("washi 1", WASHI_1),
    ("washi 2", WASHI_2),
    ("sumi", SUMI),
    ("sumi soft", SUMI_SOFT),
    ("vermilion 0", VERMILION_0),
    ("vermilion", VERMILION),
    ("vermilion 2", VERMILION_2),
    ("gold", GOLD),
    ("gold 2", GOLD_2),
    ("moss", MOSS),
    ("amber", AMBER),
    ("muted", MUTED),
];

fn gpl() -> String {
    let mut s = String::from("GIMP Palette\nName: rugen dojo\nColumns: 4\n#\n");
    for (name, c) in PALETTE {
        s.push_str(&format!("{:3} {:3} {:3}\t{name}\n", c[0], c[1], c[2]));
    }
    s
}

// ---------------------------------------------------------------- helpers

/// Nearest-neighbour 2x upscale: half-resolution art stays crisp at 1280x720.
fn x2(c: &Canvas) -> Canvas {
    let mut out = Canvas::new(c.w * 2, c.h * 2);
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let p = c.get(x, y);
            if p[3] > 0 {
                out.rect(x * 2, y * 2, 2, 2, p);
            }
        }
    }
    out
}

fn alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], (c[3] as f32 * a.clamp(0.0, 1.0)) as u8]
}

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ seed.wrapping_mul(2_246_822_519);
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}

/// 4x4 ordered-dither threshold in 0..1.
fn bayer(x: i32, y: i32) -> f32 {
    const M: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (M[y.rem_euclid(4) as usize][x.rem_euclid(4) as usize] as f32 + 0.5) / 16.0
}

/// Filled circle (pixel centres inside `r`).
fn disc(c: &mut Canvas, cx: f32, cy: f32, r: f32, col: Rgba) {
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                c.put(x, y, col);
            }
        }
    }
}

/// Punch a transparent circle.
fn disc_clear(c: &mut Canvas, cx: f32, cy: f32, r: f32) {
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                c.put_raw(x, y, CLEAR);
            }
        }
    }
}

fn border(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, t: i32, col: Rgba) {
    c.rect(x, y, w, t, col);
    c.rect(x, y + h - t, w, t, col);
    c.rect(x, y, t, h, col);
    c.rect(x + w - t, y, t, h, col);
}

fn solid(w: i32, h: i32, col: Rgba) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, col);
    c
}

/// Every opaque pixel recoloured to `col` (keeps alpha).
fn tint(c: &Canvas, col: Rgba) -> Canvas {
    let mut out = Canvas::new(c.w, c.h);
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let p = c.get(x, y);
            if p[3] > 0 {
                out.put_raw(x, y, [col[0], col[1], col[2], p[3]]);
            }
        }
    }
    out
}

/// Text in one flat colour (no outline): carved or brushed lettering.
fn flat_text(g: &Generator, s: &str, col: Rgba) -> Canvas {
    let (w, h, px) = g.text_rgba(s, fontgen::BANK_SUMI, 1);
    tint(&Canvas::from_rgba(w, h, px), col)
}

/// Text in a font bank (outline and all).
fn text(g: &Generator, s: &str, bank: usize) -> Canvas {
    let (w, h, px) = g.text_rgba(s, bank, 1);
    Canvas::from_rgba(w, h, px)
}

/// Bounding box of the opaque pixels: `(x, y, w, h)`.
fn ink_box(c: &Canvas) -> (i32, i32, i32, i32) {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            if c.get(x, y)[3] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < 0 {
        (0, 0, 0, 0)
    } else {
        (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }
}

/// Blit `src` so its ink is centred on `(cx, cy)`.
fn blit_centred(dst: &mut Canvas, src: &Canvas, cx: i32, cy: i32) {
    let (x, y, w, h) = ink_box(src);
    dst.blit(src, cx - x - w / 2, cy - y - h / 2);
}

/// Distance from `p` to segment `a`-`b`.
fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    let (qx, qy) = (a[0] + dx * t - p[0], a[1] + dy * t - p[1]);
    libm::sqrtf(qx * qx + qy * qy)
}

/// A tapered brush stroke from `a` (width `w0`) to `b` (width `w1`), with
/// dry-brush gaps toward the tail.
#[allow(clippy::too_many_arguments)]
fn brush(
    c: &mut Canvas,
    a: [f32; 2],
    b: [f32; 2],
    w0: f32,
    w1: f32,
    col: Rgba,
    dry: f32,
    seed: u32,
) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let r = w0.max(w1) / 2.0 + 1.0;
    let (minx, maxx) = (a[0].min(b[0]) - r, a[0].max(b[0]) + r);
    let (miny, maxy) = (a[1].min(b[1]) - r, a[1].max(b[1]) + r);
    for y in miny as i32..=maxy as i32 {
        for x in minx as i32..=maxx as i32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
            let half = (w0 + (w1 - w0) * t) / 2.0;
            if seg_dist(p, a, b) > half {
                continue;
            }
            // dry streaks: thin gaps along the stroke, more of them near the tail
            let across = ((p[0] - a[0]) * -dy + (p[1] - a[1]) * dx) / libm::sqrtf(len2);
            let streak = hash(libm::floorf(across + 50.0) as i32, 0, seed);
            if t > 0.55 && streak < dry * (t - 0.55) * 2.2 {
                continue;
            }
            c.put(x, y, col);
        }
    }
}

/// An ensō: a brushed ring, thick where it starts, thinning and drying out
/// toward the open end.
#[allow(clippy::too_many_arguments)]
fn enso(c: &mut Canvas, cx: f32, cy: f32, r: f32, t: f32, col: Rgba, start: f32, seed: u32) {
    let span = std::f32::consts::TAU * 0.88;
    for y in (cy - r - t) as i32..=(cy + r + t) as i32 {
        for x in (cx - r - t) as i32..=(cx + r + t) as i32 {
            let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = libm::sqrtf(px * px + py * py);
            let mut ang = libm::atan2f(py, px) - start;
            while ang < 0.0 {
                ang += std::f32::consts::TAU;
            }
            if ang > span {
                continue;
            }
            let k = ang / span;
            let width = t * (1.0 - 0.55 * k) * (0.9 + 0.1 * libm::sinf(ang * 7.0));
            let mid = r + t * 0.15 * libm::sinf(ang * 2.0);
            if (d - mid).abs() > width / 2.0 {
                continue;
            }
            let across = libm::floorf((d - mid + width) * 1.5) as i32;
            if k > 0.6 && hash(across, 0, seed) < (k - 0.6) * 2.0 {
                continue;
            }
            c.put(x, y, col);
        }
    }
}

// ---------------------------------------------------------------- the hall

/// Wood grain: a plank of `base` with lighter and darker fibres running
/// along it (`vertical` = fibres top to bottom).
#[allow(clippy::too_many_arguments)]
fn plank(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, base: Rgba, vertical: bool, seed: u32) {
    c.rect(x, y, w, h, base);
    let (len, across) = if vertical { (h, w) } else { (w, h) };
    for a in 0..across {
        let r = hash(a, 0, seed);
        let fibre = if r < 0.18 {
            Some(WOOD_0)
        } else if r > 0.86 {
            Some(WOOD_2)
        } else {
            None
        };
        let Some(col) = fibre else { continue };
        let mut i = 0;
        while i < len {
            let run = 6 + (hash(a, i, seed + 7) * 30.0) as i32;
            if hash(a, i, seed + 3) < 0.7 {
                for k in i..(i + run).min(len) {
                    let (px, py) = if vertical {
                        (x + a, y + k)
                    } else {
                        (x + k, y + a)
                    };
                    c.put(px, py, alpha(col, 0.55));
                }
            }
            i += run + 2 + (hash(a, i, seed + 9) * 10.0) as i32;
        }
    }
}

/// A wooden pillar `w` wide: grain, lit inner edge, dark outline.
fn pillar(c: &mut Canvas, x: i32, w: i32, h: i32, lit_right: bool, seed: u32) {
    plank(c, x, 0, w, h, WOOD_1, true, seed);
    c.rect(x, 0, 1, h, LACQUER_0);
    c.rect(x + w - 1, 0, 1, h, LACQUER_0);
    let lx = if lit_right { x + w - 2 } else { x + 1 };
    c.rect(lx, 0, 1, h, WOOD_3);
    let sx = if lit_right { x + 1 } else { x + w - 2 };
    c.rect(sx, 0, 1, h, WOOD_0);
}

/// The hall backdrop (half resolution): lacquer wall dithered darker toward
/// the floor, a faint shoji lattice, wooden pillars at the edges, a beam
/// with a vermilion trim at the top and a plank floor band at the bottom.
fn hall(w: i32, h: i32, seed: u32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    c.vgrad(0, 0, w, h, LACQUER_1, LACQUER_0);
    // warm lamp glow from the upper middle, ordered-dithered (no soft blur)
    let (gx, gy) = (w as f32 * 0.5, h as f32 * 0.32);
    let (rx, ry) = (w as f32 * 0.55, h as f32 * 0.62);
    for y in 12..h - 30 {
        for x in 14..w - 14 {
            let (dx, dy) = ((x as f32 - gx) / rx, (y as f32 - gy) / ry);
            let d = libm::sqrtf(dx * dx + dy * dy);
            if d < 1.0 && bayer(x, y) < (1.0 - d) * 0.75 {
                c.put(x, y, LACQUER_2);
            }
        }
    }
    // shoji lattice, barely there
    let lattice = alpha(LACQUER_2, 0.45);
    for x in (40..w - 30).step_by(46) {
        c.rect(x, 14, 1, h - 48, lattice);
    }
    for y in (60..h - 40).step_by(58) {
        c.rect(22, y, w - 44, 1, lattice);
    }
    // floor
    let fy = h - 30;
    plank(&mut c, 0, fy, w, 30, WOOD_0, false, seed + 5);
    for y in [fy + 10, fy + 20] {
        c.rect(0, y, w, 1, alpha(LACQUER_0, 0.8));
    }
    c.rect(0, fy, w, 1, WOOD_2);
    c.rect(0, fy + 1, w, 1, LACQUER_0);
    // pillars
    pillar(&mut c, 0, 14, fy, true, seed + 1);
    pillar(&mut c, w - 14, 14, fy, false, seed + 2);
    // beam
    plank(&mut c, 0, 0, w, 10, WOOD_1, false, seed + 3);
    c.rect(0, 0, w, 1, WOOD_3);
    c.rect(0, 9, w, 1, LACQUER_0);
    c.rect(0, 10, w, 1, VERMILION);
    c.rect(0, 11, w, 1, LACQUER_0);
    c
}

// ---------------------------------------------------------------- panels and plaques

/// Washi sheet in a wooden frame (24x24 half res, 9-slice `[12; 4]` doubled).
fn sheet9(fill: Rgba, wood: Rgba, hi: Rgba, groove: Rgba) -> Canvas {
    let n = 24;
    let mut c = solid(n, n, LACQUER_0);
    c.rect(1, 1, n - 2, n - 2, wood);
    c.rect(1, 1, n - 2, 1, hi);
    c.rect(1, 1, 1, n - 2, hi);
    c.rect(3, 3, n - 6, n - 6, groove);
    c.rect(4, 4, n - 8, n - 8, fill);
    for (x, y) in [(2, 2), (n - 3, 2), (2, n - 3), (n - 3, n - 3)] {
        c.put(x, y, LACQUER_0);
    }
    x2(&c)
}

/// A wooden plaque (button): rounded corners, lit top edge, darker bottom
/// edge and a 1-px drop (16x16 half res, 9-slice `[8, 8, 8, 10]` doubled).
fn plaque9(fill: Rgba, hi: Rgba, lo: Rgba, edge: Rgba) -> Canvas {
    let n = 16;
    let mut c = Canvas::new(n as u32, n as u32 + 1);
    c.rect(1, 0, n - 2, n, edge);
    c.rect(0, 1, n, n - 2, edge);
    c.rect(1, 1, n - 2, n - 2, fill);
    c.rect(1, 1, n - 2, 1, hi);
    c.rect(1, n - 3, n - 2, 2, lo);
    c.rect(1, n, n - 2, 1, alpha(LACQUER_0, 0.85));
    x2(&c)
}

/// A key cap for hints: washi face, wooden depth at the bottom (8x10 half
/// res, 9-slice `[4, 4, 4, 8]` doubled).
fn key9(face: Rgba, depth: Rgba) -> Canvas {
    let mut c = Canvas::new(8, 10);
    c.rect(1, 0, 6, 10, LACQUER_0);
    c.rect(0, 1, 8, 8, LACQUER_0);
    c.rect(1, 1, 6, 6, face);
    c.rect(1, 7, 6, 2, depth);
    x2(&c)
}

/// A hanko seal: vermilion with nicked edges and a carved inner rule.
fn seal_blank(w: i32, h: i32, seed: u32) -> Canvas {
    let mut c = solid(w, h, VERMILION);
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        c.put_raw(x, y, CLEAR);
    }
    // nicks along the edge: an inked impression is never perfect
    for i in 0..w {
        if hash(i, 0, seed) < 0.12 {
            c.put_raw(i, 0, CLEAR);
        }
        if hash(i, 1, seed) < 0.12 {
            c.put_raw(i, h - 1, CLEAR);
        }
    }
    for i in 0..h {
        if hash(0, i, seed) < 0.12 {
            c.put_raw(0, i, CLEAR);
        }
        if hash(1, i, seed) < 0.12 {
            c.put_raw(w - 1, i, CLEAR);
        }
    }
    border(&mut c, 2, 2, w - 4, h - 4, 1, alpha(WASHI_1, 0.9));
    c
}

/// A seal with carved lettering.
fn seal(g: &Generator, s: &str, w: i32, h: i32, seed: u32) -> Canvas {
    let mut c = seal_blank(w, h, seed);
    blit_centred(&mut c, &flat_text(g, s, WASHI_1), w / 2, h / 2);
    c
}

// ---------------------------------------------------------------- set pieces

/// RUGEN as a brushed title: washi lettering with a lacquer outline, a seal
/// to its left and a vermilion brush stroke under it.
fn logo(word: &Generator, mark: &Generator) -> Canvas {
    let t = text(word, "RUGEN", fontgen::BANK_WASHI);
    let s = seal(mark, "R", 34, 34, 3);
    let (_, ty, tw, th) = ink_box(&t);
    let w = 34 + 8 + tw + 6;
    let h = (th + 16).max(40);
    let mut c = Canvas::new(w as u32, h as u32);
    c.blit(&s, 0, 2);
    c.blit(&t, 42, -ty + 1);
    let uy = th as f32 + 6.0;
    brush(
        &mut c,
        [44.0, uy],
        [42.0 + tw as f32, uy - 2.0],
        7.0,
        3.0,
        VERMILION,
        0.8,
        11,
    );
    c
}

/// The featured scroll (kakejiku), half res: a washi sheet hung from a
/// wooden roller with a weighted roller at the bottom, a faint vermilion
/// ensō and distant sumi mountains.
fn scroll(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    // brocade border then the paper
    c.rect(6, 6, w - 12, h - 12, LACQUER_0);
    c.rect(7, 7, w - 14, h - 14, WOOD_0);
    c.rect(10, 10, w - 20, h - 20, LACQUER_0);
    c.rect(11, 11, w - 22, h - 22, WASHI_1);
    // paper fibres
    for y in 11..h - 11 {
        for x in 11..w - 11 {
            if hash(x, y, 41) < 0.025 {
                c.put(x, y, WASHI_0);
            }
        }
    }
    enso(
        &mut c,
        w as f32 * 0.66,
        h as f32 * 0.46,
        h as f32 * 0.33,
        9.0,
        alpha(VERMILION, 0.22),
        -1.2,
        5,
    );
    // mountains in two washes
    let base = h - 12;
    for x in 11..w - 11 {
        let fx = x as f32;
        let far = 28.0 + 10.0 * libm::sinf(fx * 0.045) + 6.0 * libm::sinf(fx * 0.11 + 1.3);
        let near = 14.0 + 8.0 * libm::sinf(fx * 0.07 + 2.0) + 4.0 * libm::sinf(fx * 0.19);
        for y in (base - far as i32)..base {
            c.put(x, y, alpha(SUMI_SOFT, 0.16));
        }
        for y in (base - near as i32)..base {
            c.put(x, y, alpha(SUMI, 0.2));
        }
    }
    // rollers
    for (y, hh) in [(0, 8), (h - 8, 8)] {
        c.rect(2, y, w - 4, hh, LACQUER_0);
        plank(
            &mut c,
            3,
            y + 1,
            w - 6,
            hh - 2,
            WOOD_1,
            false,
            13 + y as u32,
        );
        c.rect(3, y + 1, w - 6, 1, WOOD_3);
        c.rect(0, y + 1, 4, hh - 2, GOLD);
        c.rect(w - 4, y + 1, 4, hh - 2, GOLD);
    }
    c
}

/// Noren curtain for the result screen: three vermilion panels hanging
/// from a rod, a washi ensō across them (half res).
fn noren(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let pw = (w - 8) / 3;
    for i in 0..3 {
        let x = 4 + i * pw;
        c.rect(x, 4, pw - 3, h - 4, VERMILION_0);
        c.rect(x + 1, 4, pw - 5, h - 6, VERMILION);
        for y in (8..h - 4).step_by(5) {
            c.rect(x + 1, y, pw - 5, 1, alpha(VERMILION_0, 0.35));
        }
    }
    enso(
        &mut c,
        w as f32 / 2.0,
        h as f32 * 0.42,
        h as f32 * 0.24,
        10.0,
        alpha(WASHI_1, 0.85),
        -0.9,
        9,
    );
    // slits between the panels stay open
    for i in 1..3 {
        let x = 4 + i * pw - 3;
        for y in 4..h {
            for k in 0..3 {
                c.put_raw(x + k, y, CLEAR);
            }
        }
    }
    c.rect(0, 0, w, 5, LACQUER_0);
    plank(&mut c, 0, 1, w, 3, WOOD_1, false, 21);
    c
}

/// A select-screen cell (32x32 half res).
fn cell(kind: &str) -> Canvas {
    let n = 32;
    let mut c = Canvas::new(n as u32, n as u32);
    match kind {
        "empty" => {
            c.rect(1, 1, n - 2, n - 2, alpha(LACQUER_2, 0.35));
            for i in (0..n).step_by(3) {
                for (x, y) in [(i, 0), (i, n - 1), (0, i), (n - 1, i)] {
                    c.put(x, y, alpha(WOOD_0, 0.9));
                }
            }
        }
        _ => {
            c.rect(0, 0, n, n, LACQUER_0);
            c.rect(1, 1, n - 2, n - 2, WOOD_1);
            c.rect(1, 1, n - 2, 1, WOOD_3);
            c.rect(2, 2, n - 4, n - 4, LACQUER_1);
        }
    }
    x2(&c)
}

/// Cursor frame for player `p` (1 vermilion, tab above; 2 brass, tab below),
/// 38x50 half res like the ink cursors so the TOML geometry matches.
fn cursor(g: &Generator, p: u8) -> Canvas {
    let (col, label, txt) = if p == 1 {
        (VERMILION, "1P", WASHI_2)
    } else {
        (GOLD, "2P", SUMI)
    };
    let tab = flat_text(g, label, txt);
    let (_, ty, tw, th) = ink_box(&tab);
    let mut c = Canvas::new(38, 50);
    let fy = if p == 1 { 12 } else { 0 };
    c.rect(0, fy, 38, 38, LACQUER_0);
    c.rect(1, fy + 1, 36, 36, col);
    for y in fy + 3..fy + 35 {
        for x in 3..35 {
            c.put_raw(x, y, CLEAR);
        }
    }
    c.rect(4, fy + 4, 30, 1, alpha(LACQUER_0, 0.6));
    let tab_y = if p == 1 { 0 } else { 37 };
    let tab_w = tw + 6;
    c.rect(0, tab_y, tab_w + 2, 13, LACQUER_0);
    c.rect(1, tab_y + 1, tab_w, 11, col);
    c.blit(&tab, 4, tab_y + 1 + (11 - th) / 2 - ty);
    c
}

/// The online avatar: a wooden-framed silhouette on washi (32x32 half res).
fn avatar() -> Canvas {
    let mut c = solid(32, 32, LACQUER_0);
    c.rect(1, 1, 30, 30, WOOD_1);
    c.rect(3, 3, 26, 26, WASHI_1);
    for y in 0..32 {
        for x in 0..32 {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 12.5);
            if dx * dx + dy * dy < 30.0 {
                c.put(x, y, SUMI_SOFT);
            }
            let (bx, by) = (x as f32 - 15.5, y as f32 - 30.0);
            if bx * bx / 120.0 + by * by / 80.0 < 1.0
                && (3..29).contains(&y)
                && (3..29).contains(&x)
            {
                c.put(x, y, SUMI_SOFT);
            }
        }
    }
    // headband in vermilion
    c.rect(10, 9, 12, 2, VERMILION);
    x2(&c)
}

fn badge(col: Rgba) -> Canvas {
    let mut c = solid(8, 8, LACQUER_0);
    c.rect(1, 1, 6, 6, col);
    c.rect(1, 1, 6, 1, alpha(WASHI_2, 0.45));
    x2(&c)
}

/// A news placeholder: a framed ink painting with a title plaque.
fn news(g: &Generator, w: i32, h: i32, title: &str, seed: u32) -> Canvas {
    let mut c = solid(w, h, LACQUER_0);
    c.rect(1, 1, w - 2, h - 2, WASHI_1);
    enso(
        &mut c,
        w as f32 * 0.7,
        h as f32 * 0.42,
        h as f32 * 0.3,
        (h as f32 * 0.07).max(3.0),
        alpha(VERMILION, 0.5),
        -1.0,
        seed,
    );
    for x in 1..w - 1 {
        let m = h as f32 * 0.25 + 6.0 * libm::sinf(x as f32 * 0.09 + seed as f32);
        for y in (h - m as i32)..h - 1 {
            c.put(x, y, alpha(SUMI, 0.22));
        }
    }
    let t = flat_text(g, title, WASHI_2);
    let (_, ty, tw, th) = ink_box(&t);
    c.rect(4, 4, tw + 8, th + 6, VERMILION);
    c.blit(&t, 8, 7 - ty);
    c
}

/// The theme picker card (186x103 half res): the hall, a plaque menu, a
/// washi sheet and the seal.
fn preview(word: &Generator, mark: &Generator) -> Canvas {
    let (w, h) = (186, 103);
    let mut c = hall(w, h, 77);
    let l = logo(word, mark);
    let small = downscale_half(&l);
    c.blit(&small, 18, 16);
    for (i, col) in [VERMILION, WOOD_1, WOOD_1].iter().enumerate() {
        let y = 46 + i as i32 * 11;
        c.rect(18, y, 62, 9, LACQUER_0);
        c.rect(19, y + 1, 60, 7, *col);
        c.rect(19, y + 1, 60, 1, if i == 0 { VERMILION_2 } else { WOOD_3 });
    }
    let (px, py, pw, ph) = (98, 18, 70, 56);
    c.rect(px, py, pw, ph, LACQUER_0);
    c.rect(px + 1, py + 1, pw - 2, ph - 2, WOOD_1);
    c.rect(px + 3, py + 3, pw - 6, ph - 6, WASHI_1);
    enso(
        &mut c,
        133.0,
        45.0,
        15.0,
        4.0,
        alpha(VERMILION, 0.6),
        -1.0,
        3,
    );
    x2(&c)
}

/// Halve a canvas (keep every other pixel): previews of full-size pieces.
fn downscale_half(c: &Canvas) -> Canvas {
    let mut out = Canvas::new(c.w / 2, c.h / 2);
    for y in 0..out.h as i32 {
        for x in 0..out.w as i32 {
            out.put_raw(x, y, c.get(x * 2, y * 2));
        }
    }
    out
}

// ---------------------------------------------------------------- HUD (1280x720, drawn doubled)

/// Life bar frame: a wooden beam with a dark well (440x44).
fn life_frame() -> Canvas {
    let mut c = Canvas::new(220, 22);
    c.rect(0, 0, 220, 22, LACQUER_0);
    plank(&mut c, 1, 1, 218, 20, WOOD_1, false, 31);
    c.rect(1, 1, 218, 1, WOOD_3);
    c.rect(1, 20, 218, 1, WOOD_0);
    c.rect(3, 3, 214, 16, LACQUER_0);
    // brass cap at the outer end
    c.rect(1, 1, 3, 20, GOLD);
    c.rect(1, 1, 3, 1, GOLD_2);
    x2(&c)
}

/// Life fill (428x32): `top` band, `mid` body, `bottom` band.
fn life_fill(top: Rgba, mid: Rgba, bottom: Rgba) -> Canvas {
    let mut c = solid(214, 16, mid);
    c.rect(0, 0, 214, 3, top);
    c.rect(0, 13, 214, 3, bottom);
    x2(&c)
}

/// 10% notches along the life well.
fn life_ticks() -> Canvas {
    let mut c = Canvas::new(428, 32);
    for i in 1..10 {
        let x = libm::roundf(i as f32 * 42.8) as i32 & !1;
        c.rect(x, 0, 2, 8, LACQUER_0);
        c.rect(x, 24, 2, 8, LACQUER_0);
    }
    c
}

/// Portrait frame: wood around a washi window (108x108, centred axis).
fn portrait_frame() -> Canvas {
    let mut c = solid(54, 54, LACQUER_0);
    c.rect(1, 1, 52, 52, WOOD_1);
    c.rect(1, 1, 52, 1, WOOD_3);
    c.rect(1, 1, 1, 52, WOOD_2);
    c.rect(3, 3, 48, 48, LACQUER_0);
    c.rect(4, 4, 46, 46, WASHI_0);
    for (x, y) in [(2, 2), (51, 2), (2, 51), (51, 51)] {
        c.put(x, y, GOLD);
    }
    x2(&c)
}

/// Name plate (3-slice source): lacquer with wooden end caps (48x36).
fn name_plate() -> Canvas {
    let mut c = solid(24, 18, LACQUER_0);
    c.rect(1, 1, 22, 16, LACQUER_2);
    c.rect(1, 1, 22, 1, WOOD_0);
    c.rect(1, 1, 3, 16, WOOD_1);
    c.rect(20, 1, 3, 16, WOOD_1);
    c.rect(1, 1, 3, 1, WOOD_3);
    c.rect(20, 1, 3, 1, WOOD_3);
    x2(&c)
}

/// A won round: a vermilion seal with a carved W (22x22).
fn win_mark(g: &Generator) -> Canvas {
    let mut c = seal_blank(11, 11, 17);
    blit_centred(&mut c, &flat_text(g, "W", WASHI_1), 5, 5);
    x2(&c)
}

/// A round still to win: a hollow wooden square (22x22).
fn empty_mark() -> Canvas {
    let mut c = Canvas::new(11, 11);
    border(&mut c, 0, 0, 11, 11, 1, LACQUER_0);
    border(&mut c, 1, 1, 9, 9, 1, WOOD_2);
    c.rect(2, 2, 7, 7, alpha(LACQUER_0, 0.6));
    x2(&c)
}

/// The clock: a hanging wooden plaque on two cords (168x84).
fn clock_plaque() -> Canvas {
    let mut c = Canvas::new(84, 42);
    c.rect(18, 0, 1, 6, LACQUER_0);
    c.rect(65, 0, 1, 6, LACQUER_0);
    c.rect(2, 5, 80, 36, LACQUER_0);
    c.rect(0, 6, 84, 34, LACQUER_0);
    plank(&mut c, 2, 6, 80, 34, WOOD_1, false, 51);
    c.rect(2, 6, 80, 1, WOOD_3);
    c.rect(2, 38, 80, 2, WOOD_0);
    c.rect(6, 9, 72, 26, LACQUER_1);
    c.rect(6, 9, 72, 1, LACQUER_0);
    for (x, y) in [(4, 8), (79, 8), (4, 36), (79, 36)] {
        c.put(x, y, GOLD);
    }
    x2(&c)
}

/// "ROUND n" tag under the clock: a vermilion strip (128x26).
fn round_tag() -> Canvas {
    let mut c = solid(64, 13, LACQUER_0);
    c.rect(1, 0, 62, 12, VERMILION);
    c.rect(1, 0, 62, 1, VERMILION_2);
    x2(&c)
}

/// The POWER label: washi type carved into a wooden plaque.
fn power_label(g: &Generator) -> Canvas {
    let t = flat_text(g, "POWER", WASHI_2);
    let (_, ty, tw, th) = ink_box(&t);
    let (w, h) = (tw + 10, th + 6);
    let mut c = solid(w, h, LACQUER_0);
    c.rect(1, 1, w - 2, h - 2, WOOD_1);
    c.rect(1, 1, w - 2, 1, WOOD_3);
    c.blit(&t, 5, 3 - ty);
    x2(&c)
}

/// Power level stamp: a square seal (56x56), vermilion when a bar is full.
fn level_stamp(lit: bool) -> Canvas {
    let c = if lit {
        seal_blank(28, 28, 23)
    } else {
        let mut c = solid(28, 28, LACQUER_0);
        c.rect(1, 1, 26, 26, WOOD_1);
        c.rect(1, 1, 26, 1, WOOD_3);
        c.rect(3, 3, 22, 22, LACQUER_1);
        c
    };
    x2(&c)
}

/// One power segment well (100x26): lacquer with a wooden rim.
fn power_well() -> Canvas {
    let mut c = solid(50, 13, LACQUER_0);
    c.rect(1, 1, 48, 11, WOOD_0);
    c.rect(1, 1, 48, 1, WOOD_2);
    c.rect(2, 2, 46, 9, LACQUER_1);
    x2(&c)
}

fn power_fill(col: Rgba, hi: Rgba) -> Canvas {
    let mut c = solid(46, 9, col);
    c.rect(0, 0, 46, 2, hi);
    x2(&c)
}

// ---------------------------------------------------------------- generate

const README: &str = r#"# motif/themes/dojo — Dojo

A training hall at night: dark lacquer walls, wooden pillars, beams and
plaques, washi sheets in wooden frames, sumi type, vermilion hanko seals and
a brass focus. Generated by `rugen ui-gen --theme dojo`
(`crates/app/src/dojogen.rs`, screens in `crates/app/src/dojoscreens/`);
edit the `.toml` screens or replace any WebP. The palette is in
`palette.gpl`.

Select it in OPTIONS → INTERFACE or with `[video] theme = "dojo"` in
rugen.toml. Files missing here fall back to the flat `motif/`.

Fonts (`font/`): 1 small (Press Start 8: tags, numbers), 2 menu (Jersey 20:
captions, hints), 3 title (Jersey 40), 4 num (Press Start 16), 5 code (room
codes), 6 body (Jersey 30: text, buttons, names). Banks: 12 sumi, 13
vermilion, 14 washi, 15 brass, 16 muted on lacquer, 17 soft sumi on washi.
Focus is always brass + vermilion; P1 is vermilion, P2 brass.

Steam Deck profile: `font1.deck` / `font2.deck` (`rugen-deck`, Pixelify Sans
at 2x, 14 px capitals) replace fonts 1 and 2 wherever a screen draws them at
scale 1; layouts keep the rows of the font they name. A node's `deck = { ... }`
table overrides its keys under that profile only (room for the bigger text).
"#;

/// Generate the dojo theme into `out` (usually `motif/themes/dojo`), taking
/// the TTFs from `flat` (`motif/font/src/`).
pub fn generate(flat: &Path, out: &Path, fallback_ttf: Option<&[u8]>) -> anyhow::Result<()> {
    let ui = out.join("ui");
    std::fs::create_dir_all(&ui)?;
    let roles = fontgen::dojo_roles();
    fontgen::write_roles(flat, &out.join("font"), &roles, fallback_ttf)?;
    let jersey = fontgen::role_ttf(flat, &roles[1], fallback_ttf)?;
    let press = fontgen::role_ttf(flat, &roles[0], fallback_ttf)?;
    let style = |size| Style {
        size,
        italic: 0,
        condense: 1.0,
        rim: false,
        highlight: false,
    };
    let j20 = Generator::new(&jersey, style(20))?;
    let j30 = Generator::new(&jersey, style(30))?;
    let j60 = Generator::new(&jersey, style(60))?;
    let p8 = Generator::new(&press, style(8))?;

    let w = |name: &str, c: &Canvas| pixelize::write_webp(&ui.join(name), c);
    let w2 = |name: &str, c: &Canvas| pixelize::write_webp(&ui.join(name), &x2(c));

    // backdrops
    w2("hall.webp", &hall(640, 360, 1))?;
    w2("hall_title.webp", &hall(640, 360, 2))?;
    let mut res = hall(640, 360, 3);
    res.blit(&noren(300, 250), 14, 12);
    w2("hall_result.webp", &res)?;
    // versus: the two halves of the hall in their players' light, a pillar between
    let mut vs = hall(640, 360, 4);
    vs.rect(14, 12, 300, 318, alpha(VERMILION_0, 0.55));
    vs.rect(326, 12, 300, 318, alpha(WOOD_0, 0.55));
    // dry brush sweeps behind each fighter
    for (i, (a, b, w0)) in [
        ([30.0, 300.0], [290.0, 60.0], 34.0),
        ([60.0, 330.0], [300.0, 130.0], 18.0),
        ([340.0, 60.0], [610.0, 290.0], 34.0),
        ([330.0, 140.0], [580.0, 330.0], 18.0),
    ]
    .into_iter()
    .enumerate()
    {
        let col = if i < 2 {
            alpha(VERMILION_2, 0.22)
        } else {
            alpha(GOLD, 0.16)
        };
        brush(&mut vs, a, b, w0, w0 * 0.4, col, 1.0, 30 + i as u32);
    }
    pillar(&mut vs, 312, 16, 330, true, 9);
    w2("hall_vs.webp", &vs)?;
    // panels and plaques
    w("sheet9.webp", &sheet9(WASHI_1, WOOD_1, WOOD_2, WOOD_0))?;
    w("sheet9_lit.webp", &sheet9(WASHI_2, GOLD, GOLD_2, WOOD_0))?;
    w(
        "sheet9_red.webp",
        &sheet9(WASHI_1, VERMILION, VERMILION_2, VERMILION_0),
    )?;
    w("lacq9.webp", &sheet9(LACQUER_1, WOOD_1, WOOD_2, LACQUER_0))?;
    w(
        "lacq9_lit.webp",
        &sheet9(LACQUER_2, GOLD, GOLD_2, LACQUER_0),
    )?;
    w("plaque9.webp", &plaque9(WOOD_1, WOOD_3, WOOD_0, LACQUER_0))?;
    w(
        "plaque9_lit.webp",
        &plaque9(VERMILION, VERMILION_2, VERMILION_0, GOLD),
    )?;
    w(
        "plaque9_red.webp",
        &plaque9(VERMILION, VERMILION_2, VERMILION_0, LACQUER_0),
    )?;
    w(
        "plaque9_dark.webp",
        &plaque9(LACQUER_2, WOOD_0, LACQUER_1, LACQUER_0),
    )?;
    w(
        "plaque9_gold.webp",
        &plaque9(GOLD, GOLD_2, AMBER, LACQUER_0),
    )?;
    w("key9.webp", &key9(WASHI_1, WOOD_1))?;
    w("key9_dark.webp", &key9(LACQUER_2, LACQUER_0))?;
    w("seal9.webp", &x2(&seal_blank(12, 12, 7)))?;
    // marks
    w2("logo.webp", &logo(&j60, &j30))?;
    let j80 = Generator::new(&jersey, style(80))?;
    w2("vs_seal.webp", &seal(&j80, "VS", 96, 84, 5))?;
    w2("win_seal.webp", &seal(&j60, "WIN", 100, 52, 6))?;
    let mut ens = Canvas::new(180, 180);
    enso(&mut ens, 90.0, 90.0, 76.0, 16.0, VERMILION, -1.0, 8);
    w2("enso.webp", &ens)?;
    w2("scroll.webp", &scroll(324, 204))?;
    // select
    for kind in ["char", "broken", "empty"] {
        w(&format!("cell_{kind}.webp"), &cell(kind))?;
    }
    let mut rnd = cell("char");
    blit_centred(&mut rnd, &x2(&flat_text(&j30, "?", WASHI_1)), 32, 32);
    w("cell_random.webp", &rnd)?;
    let mut warn = Canvas::new(13, 13);
    for y in 0..13 {
        for x in y..13 {
            warn.put(x, y, AMBER);
        }
    }
    warn.rect(9, 1, 1, 4, SUMI);
    warn.rect(9, 6, 1, 1, SUMI);
    w2("sel_warn.webp", &warn)?;
    let mut bad = Canvas::new(32, 32);
    brush(
        &mut bad,
        [5.0, 5.0],
        [27.0, 27.0],
        4.0,
        2.0,
        VERMILION,
        0.0,
        1,
    );
    brush(
        &mut bad,
        [27.0, 5.0],
        [5.0, 27.0],
        4.0,
        2.0,
        VERMILION,
        0.0,
        2,
    );
    w2("sel_x.webp", &bad)?;
    w2("sel_cur1.webp", &cursor(&p8, 1))?;
    w2("sel_cur2.webp", &cursor(&p8, 2))?;
    // online and misc
    w("avatar.webp", &avatar())?;
    w("badge_ok.webp", &badge(MOSS))?;
    w("badge_warn.webp", &badge(AMBER))?;
    w("badge_bad.webp", &badge(VERMILION))?;
    w2("news1.webp", &news(&j20, 332, 150, "RUGEN", 1))?;
    w2("news2.webp", &news(&j20, 147, 48, "ARENAS", 2))?;
    w2("news3.webp", &news(&j20, 147, 48, "ONLINE", 3))?;
    w("preview.webp", &preview(&j30, &j20))?;
    // controls test pad
    let mut base = Canvas::new(76, 76);
    disc(&mut base, 38.0, 38.0, 37.5, LACQUER_0);
    disc(&mut base, 38.0, 38.0, 35.5, WOOD_0);
    disc(&mut base, 38.0, 38.0, 31.5, LACQUER_1);
    w2("pad_base.webp", &base)?;
    let mut ball = Canvas::new(40, 40);
    disc(&mut ball, 20.0, 20.0, 20.0, LACQUER_0);
    disc(&mut ball, 20.0, 20.0, 18.0, VERMILION);
    disc(&mut ball, 17.0, 16.0, 6.0, VERMILION_2);
    w2("pad_ball.webp", &ball)?;
    for (name, fill, hi, drop) in [
        ("pad_btn.webp", WOOD_1, WOOD_3, 0.0),
        ("pad_btn_lit.webp", VERMILION, VERMILION_2, 2.0),
    ] {
        let mut b = Canvas::new(34, 37);
        disc(&mut b, 17.0, 20.0, 17.0, LACQUER_0);
        disc(&mut b, 17.0, 17.0 + drop, 17.0, LACQUER_0);
        disc(&mut b, 17.0, 17.0 + drop, 15.0, fill);
        disc(&mut b, 14.0, 13.0 + drop, 5.0, hi);
        w2(name, &b)?;
    }
    for (name, fill) in [("pill.webp", WOOD_1), ("pill_lit.webp", VERMILION)] {
        let mut p = Canvas::new(28, 11);
        p.rect(1, 0, 26, 11, LACQUER_0);
        p.rect(0, 1, 28, 9, LACQUER_0);
        p.rect(1, 1, 26, 9, fill);
        w2(name, &p)?;
    }
    for (name, col) in [("ring.webp", SUMI), ("ring_lit.webp", WASHI_2)] {
        let mut r = Canvas::new(24, 24);
        disc(&mut r, 12.0, 12.0, 12.0, col);
        disc_clear(&mut r, 12.0, 12.0, 9.5);
        w(name, &r)?;
    }

    screens(&ui)?;
    crate::settings_gen::dojo(&ui)?;
    std::fs::write(
        out.join("README.md"),
        format!("{README}{}", crate::settings_gen::README),
    )?;
    std::fs::write(out.join("palette.gpl"), gpl())?;
    let news_json = out.join("news.json");
    if !news_json.exists() {
        std::fs::write(
            news_json,
            r#"[
  { "title": "WELCOME TO RUGEN", "subtitle": "Drop .zip or .rar chars into chars/ and go", "image": "ui/news1.webp" },
  { "title": "ARENAS", "subtitle": "Platforms, pits and blast zones", "image": "ui/news2.webp" },
  { "title": "ONLINE", "subtitle": "Rooms, ranked and rollback", "image": "ui/news3.webp" }
]
"#,
        )?;
    }
    screenpack(out, &j60, &j30, &p8)?;
    Ok(())
}

/// The MUGEN-side screenpack: `system.sff` / `fight.sff` and their defs.
fn screenpack(out: &Path, j60: &Generator, j30: &Generator, p8: &Generator) -> anyhow::Result<()> {
    let mut sys = Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    };
    let l = logo(j60, j30);
    to_sprite(&mut sys, (0, 0), &l, [(l.w / 2) as i16, 0]);
    let vs = seal(j60, "VS", 70, 64, 5);
    to_sprite(
        &mut sys,
        (200, 0),
        &vs,
        [(vs.w / 2) as i16, (vs.h / 2) as i16],
    );
    to_sprite(&mut sys, (1, 0), &hall(426, 240, 1), [213, 0]);
    to_sprite(&mut sys, (100, 0), &hall(426, 240, 2), [213, 0]);
    let mut box_panel = solid(232, 100, LACQUER_0);
    box_panel.rect(2, 2, 228, 96, WASHI_1);
    to_sprite(&mut sys, (100, 1), &box_panel, [116, 0]);
    let mut face_plate = solid(108, 126, LACQUER_0);
    face_plate.rect(2, 2, 104, 122, WOOD_1);
    face_plate.rect(4, 4, 100, 118, WASHI_0);
    to_sprite(&mut sys, (120, 0), &face_plate, [0, 0]);
    to_sprite(&mut sys, (120, 1), &face_plate.flip_h(), [0, 0]);
    to_sprite(&mut sys, (121, 0), &solid(100, 14, VERMILION), [0, 0]);
    to_sprite(&mut sys, (121, 1), &solid(100, 14, GOLD), [0, 0]);
    let half = |c: Canvas| downscale_half(&c);
    to_sprite(&mut sys, (150, 0), &half(cell("char")), [1, 1]);
    let cur = |col: Rgba| {
        let mut c = Canvas::new(46, 46);
        border(&mut c, 0, 0, 46, 46, 3, col);
        c
    };
    to_sprite(&mut sys, (160, 0), &cur(VERMILION), [3, 3]);
    to_sprite(&mut sys, (161, 0), &cur(WASHI_2), [3, 3]);
    to_sprite(&mut sys, (170, 0), &cur(GOLD), [3, 3]);
    to_sprite(&mut sys, (171, 0), &cur(WASHI_2), [3, 3]);
    let mut opt = solid(380, 180, LACQUER_0);
    opt.rect(2, 2, 376, 176, WASHI_1);
    to_sprite(&mut sys, (300, 0), &opt, [190, 0]);
    to_sprite(
        &mut sys,
        (310, 0),
        &half(plaque_sized(300, 28, false)),
        [75, 0],
    );
    to_sprite(
        &mut sys,
        (311, 0),
        &half(plaque_sized(300, 28, true)),
        [75, 0],
    );
    let mut left = solid(253, 240, VERMILION_0);
    left.cut_right(213, 60);
    left.outline(LACQUER_0);
    to_sprite(&mut sys, (201, 0), &left, [0, 0]);
    let mut right = solid(253, 240, WOOD_0);
    right.cut_right(213, 60);
    right.outline(LACQUER_0);
    to_sprite(&mut sys, (201, 1), &right.flip_h(), [0, 0]);
    write(&out.join("system.sff"), &sys.write_v2())?;

    let mut fight = Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    };
    fight_hud(&mut fight, p8);
    write(&out.join("fight.sff"), &fight.write_v2())?;
    write(&out.join("system.def"), SYSTEM_DEF.as_bytes())?;
    write(&out.join("fight.def"), FIGHT_DEF.as_bytes())?;
    Ok(())
}

/// A plaque at a fixed size (for the MUGEN title menu sprites).
fn plaque_sized(w: i32, h: i32, lit: bool) -> Canvas {
    let (fill, hi, lo, edge) = if lit {
        (VERMILION, VERMILION_2, VERMILION_0, GOLD)
    } else {
        (WOOD_1, WOOD_3, WOOD_0, LACQUER_0)
    };
    let mut c = solid(w, h, edge);
    c.rect(2, 2, w - 4, h - 4, fill);
    c.rect(2, 2, w - 4, 2, hi);
    c.rect(2, h - 6, w - 4, 4, lo);
    c
}

/// The fight HUD sprites, drawn at 1280x720 (see `FIGHT_DEF`).
fn fight_hud(fight: &mut Sff, p8: &Generator) {
    to_sprite(fight, (10, 0), &life_frame(), [0, 0]);
    to_sprite(fight, (11, 0), &life_ticks(), [0, 0]);
    to_sprite(fight, (12, 0), &life_fill(GOLD_2, GOLD, AMBER), [0, 0]);
    to_sprite(
        fight,
        (13, 0),
        &life_fill(VERMILION_2, VERMILION, VERMILION_0),
        [0, 0],
    );
    to_sprite(
        fight,
        (14, 0),
        &life_fill(GOLD_2, VERMILION_2, VERMILION),
        [0, 0],
    );
    to_sprite(fight, (20, 0), &name_plate(), [0, 0]);
    let label = power_label(p8);
    to_sprite(fight, (30, 0), &label, [0, 0]);
    to_sprite(fight, (30, 1), &label, [label.w as i16, 0]);
    to_sprite(fight, (31, 0), &power_well(), [0, 0]);
    to_sprite(fight, (32, 0), &power_fill(GOLD, GOLD_2), [0, 0]);
    to_sprite(fight, (33, 0), &power_fill(WOOD_3, GOLD_2), [0, 0]);
    to_sprite(fight, (40, 0), &level_stamp(false), [28, 28]);
    to_sprite(fight, (41, 0), &level_stamp(true), [28, 28]);
    let pf = portrait_frame();
    to_sprite(fight, (50, 0), &pf, [(pf.w / 2) as i16, (pf.h / 2) as i16]);
    let clock = clock_plaque();
    to_sprite(fight, (60, 0), &clock, [(clock.w / 2) as i16, 0]);
    let tag = round_tag();
    to_sprite(fight, (61, 0), &tag, [(tag.w / 2) as i16, 0]);
    to_sprite(fight, (100, 0), &win_mark(p8), [11, 11]);
    to_sprite(fight, (110, 0), &empty_mark(), [11, 11]);
}

fn write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Write the screen TOMLs into `ui/`.
pub fn screens(ui: &Path) -> anyhow::Result<()> {
    for (name, text) in SCREENS {
        std::fs::write(ui.join(name), text)?;
    }
    Ok(())
}

/// The dojo screens (options / controls come from `settings_gen`).
pub const SCREENS: [(&str, &str); 11] = [
    ("title.toml", include_str!("dojoscreens/title.toml")),
    ("select.toml", include_str!("dojoscreens/select.toml")),
    ("vs.toml", include_str!("dojoscreens/vs.toml")),
    ("pause.toml", include_str!("dojoscreens/pause.toml")),
    ("result.toml", include_str!("dojoscreens/result.toml")),
    ("online.toml", include_str!("dojoscreens/online.toml")),
    ("stages.toml", include_str!("dojoscreens/stages.toml")),
    ("challenge.toml", include_str!("dojoscreens/challenge.toml")),
    ("practice.toml", include_str!("dojoscreens/practice.toml")),
    ("teams.toml", include_str!("dojoscreens/teams.toml")),
    ("debug.toml", include_str!("dojoscreens/debug.toml")),
];

const SYSTEM_DEF: &str = include_str!("dojoscreens/system.def");
const FIGHT_DEF: &str = include_str!("dojoscreens/fight.def");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dojo_screens_parse() {
        for (name, text) in SCREENS {
            rugen_ui::ScreenDef::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn font_banks_use_the_palette() {
        for (bank, col) in [
            (fontgen::BANK_SUMI, SUMI),
            (fontgen::BANK_VERMILION, VERMILION),
            (fontgen::BANK_GOLD, GOLD),
            (fontgen::BANK_MUTED, MUTED),
            (fontgen::BANK_SUMI_SOFT, SUMI_SOFT),
        ] {
            assert_eq!(fontgen::BANKS[bank][0], col, "bank {bank}");
        }
    }

    #[test]
    fn palette_is_readable() {
        // text colours on their grounds keep at least 4.5:1 (WCAG luminance)
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                libm::powf((v + 0.055) / 1.055, 2.4)
            }
        };
        let l = |c: Rgba| 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
        let ratio = |a: Rgba, b: Rgba| {
            let (x, y) = (l(a).max(l(b)), l(a).min(l(b)));
            (x + 0.05) / (y + 0.05)
        };
        for (fg, bg) in [
            (SUMI, WASHI_1),
            (SUMI_SOFT, WASHI_1),
            (VERMILION, WASHI_2),
            (WASHI_1, LACQUER_1),
            (MUTED, LACQUER_1),
            (GOLD, LACQUER_1),
            (WASHI_2, VERMILION),
            (WASHI_2, WOOD_1),
        ] {
            assert!(
                ratio(fg, bg) >= 4.5,
                "{fg:?} on {bg:?}: {:.2}",
                ratio(fg, bg)
            );
        }
    }
}
