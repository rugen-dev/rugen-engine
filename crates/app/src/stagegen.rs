//! `rugen stage-gen`: the built-in stages shipped in `motif/stages/`.
//!
//! - `training`: a high-detail take on MUGEN's blue "Training Room" grid.
//! - `ink_dojo`: a Shonen Ink manga dojo (paper, screentone, brush ink).
//! - `ink_arena`: the dojo sky over a long floating ink island (1280 units)
//!   with nine platforms in three tiers and a pit; its `[Arena]` section
//!   turns on stocks, blast zones and ring-outs (the ARENA mode's stage),
//!   ledges, items and wind; the top-left platform slides, the top-right
//!   one drops now and then (their art follows with `platform = N`).
//!
//! Both are built for the far camera: every layer tiles sideways, layers run
//! far above and below the play area, and the clear colour matches the top
//! of the sky, so nothing runs out down to `zoomout = 0.25`. Art is drawn at
//! 2x (`scalestart = .5`) in a 320x240 stage, so pixels stay crisp and chunky.

use std::path::Path;

use rugen_formats::sff::{Sff, Version};

use crate::screenpack::{Canvas, Rgba, to_sprite};

/// Art density: sprite pixels per stage unit.
const D: i32 = 2;

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8;
    [m(0), m(1), m(2), m(3)]
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], (c[3] as f32 * a.clamp(0.0, 1.0)) as u8]
}

/// Deterministic noise in 0..1.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
}

fn hline(c: &mut Canvas, y: i32, x0: i32, x1: i32, t: i32, col: Rgba) {
    c.rect(x0, y, x1 - x0, t, col);
}

fn vline(c: &mut Canvas, x: i32, y0: i32, y1: i32, t: i32, col: Rgba) {
    c.rect(x, y0, t, y1 - y0, col);
}

fn disc(c: &mut Canvas, cx: f32, cy: f32, r: f32, col: Rgba) {
    for y in (cy - r) as i32..=(cy + r) as i32 {
        for x in (cx - r) as i32..=(cx + r) as i32 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                c.put(x, y, col);
            }
        }
    }
}

/// Screentone: dots on a `pitch` grid whose radius follows `density(x, y)` in 0..1.
fn tone(c: &mut Canvas, pitch: i32, col: Rgba, density: impl Fn(i32, i32) -> f32) {
    let (w, h) = (c.w as i32, c.h as i32);
    let mut y = 0;
    while y < h + pitch {
        let row = y / pitch;
        let off = if row % 2 == 1 { pitch / 2 } else { 0 };
        let mut x = -pitch;
        while x < w + pitch {
            let (cx, cy) = (x + off, y);
            let d = density(cx.clamp(0, w - 1), cy.clamp(0, h - 1));
            if d > 0.02 {
                disc(c, cx as f32, cy as f32, d * pitch as f32 * 0.5, col);
            }
            x += pitch;
        }
        y += pitch;
    }
}

/// Wrap a canvas horizontally so tiles join (copy columns by `x mod w`).
fn wrap_x(c: &Canvas, wide: &Canvas, pad: i32) -> Canvas {
    let mut out = Canvas::new(c.w, c.h);
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let mut px = wide.get(x + pad, y);
            // the padded margins fold back onto the tile
            let l = wide.get(x + pad + c.w as i32, y);
            let r = wide.get(x + pad - c.w as i32, y);
            for extra in [l, r] {
                if extra[3] > 0 && px[3] < 255 {
                    px = if px[3] == 0 {
                        extra
                    } else {
                        mix(extra, px, px[3] as f32 / 255.0)
                    };
                }
            }
            out.put(x, y, px);
        }
    }
    out
}

// ---------------------------------------------------------------- shared

/// 3x5 pixel digits and signs for stage rulers.
fn glyph(ch: char) -> [u8; 5] {
    match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    }
}

/// Draw `text` with the 3x5 font, `px` pixels per cell, left-aligned at (x, y).
fn text(c: &mut Canvas, x: i32, y: i32, px: i32, col: Rgba, text: &str) -> i32 {
    let mut cx = x;
    for ch in text.chars() {
        let g = glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for col_i in 0..3 {
                if bits & (0b100 >> col_i) != 0 {
                    c.rect(cx + col_i * px, y + row as i32 * px, px, px, col);
                }
            }
        }
        cx += 4 * px;
    }
    cx - x - px
}

/// Width of `text` in the 3x5 font at `px`.
fn text_w(t: &str, px: i32) -> i32 {
    (t.chars().count() as i32 * 4 - 1) * px
}

/// A soft horizontal band: alpha peaks in the middle row and fades to the edges.
fn band(c: &mut Canvas, y0: i32, y1: i32, col: Rgba, peak: f32) {
    let h = (y1 - y0).max(1) as f32;
    for y in y0..y1 {
        let t = (y - y0) as f32 / h;
        let a = (1.0 - (t * 2.0 - 1.0).abs()).clamp(0.0, 1.0) * peak;
        c.rect(0, y, c.w as i32, 1, with_alpha(col, a));
    }
}

// ---------------------------------------------------------------- training

const T_SKY: Rgba = [4, 9, 40, 255];
const T_WALL: Rgba = [10, 40, 150, 255];
const T_LINE: Rgba = [46, 104, 255, 255];
const T_GLOW: Rgba = [120, 210, 255, 255];
const T_FLOOR: Rgba = [8, 36, 140, 255];
const T_DEEP: Rgba = [3, 12, 60, 255];

/// Half the room in stage units: the camera runs to ±T_HALF.
const T_HALF: i32 = 1200;

/// Far wall (delta .3): night-blue gradient, a very large faint grid and tall
/// light columns every 400 units that give the room its size.
fn training_far() -> Canvas {
    let (w, h) = (400 * D, 900 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.vgrad(0, 0, w, h, T_SKY, mix(T_SKY, T_WALL, 0.7));
    let cell = 100 * D;
    for y in (0..h).step_by(cell as usize) {
        let a = 0.06 + 0.2 * (y as f32 / h as f32);
        hline(&mut c, y, 0, w, D, with_alpha(T_LINE, a));
    }
    for x in (0..w).step_by(cell as usize) {
        for y in 0..h {
            let a = 0.06 + 0.2 * (y as f32 / h as f32);
            c.rect(x, y, D, 1, with_alpha(T_LINE, a));
        }
    }
    // a light column: a bright core with a soft halo, from the floor up to a fade
    let cx = w / 2;
    for y in 0..h {
        let t = y as f32 / h as f32;
        let a = (t * t * 1.1).min(1.0);
        for dx in -18 * D..=18 * D {
            let u = (dx.abs() as f32) / (18.0 * D as f32);
            let halo = (1.0 - u) * (1.0 - u) * 0.5 * a;
            c.put(cx + dx, y, with_alpha(T_LINE, halo));
        }
        c.rect(cx - D, y, 2 * D, 1, with_alpha(T_GLOW, a));
    }
    // a wide glow where the column meets the floor
    band(&mut c, h - 40 * D, h, T_GLOW, 0.35);
    c
}

/// Scan band (delta .3, drifts up, additive): a thin horizontal light.
fn training_scan() -> Canvas {
    let (w, h) = (400 * D, 26 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    band(&mut c, 0, h, T_GLOW, 0.22);
    hline(&mut c, h / 2, 0, w, D, with_alpha(T_GLOW, 0.5));
    c
}

/// Mid wall (delta .7): MUGEN's training grid, lines only, fading out toward
/// the top; the far wall shows through it.
fn training_wall() -> Canvas {
    let (w, h) = (200 * D, 420 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let fade = 140 * D;
    // a faint tint so the near wall reads as a surface, not just lines
    for y in 0..h {
        let a = (y as f32 / fade as f32).min(1.0) * 0.22;
        c.rect(0, y, w, 1, with_alpha(T_WALL, a));
    }
    let cell = 40 * D;
    for y in (h % cell..h).step_by(cell as usize) {
        let a = (y as f32 / fade as f32).min(1.0) * 0.9;
        hline(&mut c, y, 0, w, D, with_alpha(T_LINE, a));
    }
    for x in (0..w).step_by(cell as usize) {
        for y in 0..h {
            let a = (y as f32 / fade as f32).min(1.0) * 0.9;
            c.rect(x, y, D, 1, with_alpha(T_LINE, a));
        }
    }
    c
}

/// Horizon line (delta 1, one sprite across the room: anim layers do not
/// tile) in three brightness steps for the pulse anim.
fn training_horizon(level: f32) -> Canvas {
    let (w, h) = (2 * (T_HALF + 400) * D, 10 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    band(&mut c, 0, h, T_GLOW, 0.25 * level);
    hline(
        &mut c,
        h / 2 - D,
        0,
        w,
        2 * D,
        with_alpha(T_GLOW, 0.5 + 0.5 * level),
    );
    c
}

/// Floor (delta 1, tiled 200 units): perspective rows and converging columns.
fn training_floor() -> Canvas {
    let (w, h) = (200 * D, 440 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.vgrad(0, 0, w, 120 * D, T_FLOOR, mix(T_FLOOR, T_DEEP, 0.5));
    c.vgrad(
        0,
        120 * D,
        w,
        h - 120 * D,
        mix(T_FLOOR, T_DEEP, 0.5),
        T_DEEP,
    );
    let mut y = 3.0f32 * D as f32;
    let mut step = 4.0f32 * D as f32;
    while (y as i32) < h {
        let a = 1.0 - (y / h as f32) * 0.7;
        hline(&mut c, y as i32, 0, w, D, with_alpha(T_LINE, a));
        y += step;
        step *= 1.32;
    }
    let cell = 40 * D;
    for x in (0..w).step_by(cell as usize) {
        for y in 0..h {
            let a = 1.0 - (y as f32 / h as f32) * 0.75;
            c.rect(x, y, D, 1, with_alpha(T_LINE, a));
        }
    }
    hline(&mut c, 0, 0, w, D, T_GLOW);
    c
}

/// Floor ruler (delta 1, not tiled): ticks every 50 units, numbers every 100,
/// 0 at the centre, across the whole room.
fn training_ruler() -> Canvas {
    let span = T_HALF + 400;
    let (w, h) = (2 * span * D, 16 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let mid = w / 2;
    let mut u = -span;
    while u <= span {
        let x = mid + u * D;
        let major = u % 100 == 0;
        let th = if major { 2 * D } else { D };
        let tall = if major { 6 * D } else { 3 * D };
        c.rect(
            x - th / 2,
            0,
            th,
            tall,
            with_alpha(T_GLOW, if major { 0.9 } else { 0.5 }),
        );
        if major {
            let label = if u == 0 {
                "0".to_string()
            } else if u > 0 {
                format!("+{u}")
            } else {
                format!("{u}")
            };
            let tw = text_w(&label, D);
            let col = if u == 0 {
                T_GLOW
            } else {
                with_alpha(T_GLOW, 0.75)
            };
            text(&mut c, x - tw / 2, 8 * D, D, col, &label);
        }
        u += 50;
    }
    c
}

const TRAINING_DEF: &str = r#"; rugen built-in stage, generated by `rugen stage-gen`
; A very large room: the camera runs 1200 units each way, so the whole
; screen can be a fighter's dash. Floor ruler in stage units, 0 at centre.
[Info]
name = "Training Room EX"
displayname = "Training Room EX"
author = "rugen"
mugenversion = 1.1

[Camera]
startx = 0
starty = 0
boundleft = -1200
boundright = 1200
boundhigh = -700
boundlow = 0
verticalfollow = .25
floortension = 20
tension = 50
zoomout = 0.22
zoomin = 1.15
overdrawhigh = 0
overdrawlow = 0
cuthigh = 35
cutlow = 25

[PlayerInfo]
p1startx = -70
p1starty = 0
p1facing = 1
p2startx = 70
p2starty = 0
p2facing = -1
leftbound = -2600
rightbound = 2600

[Bound]
screenleft = 15
screenright = 15

[StageInfo]
zoffset = 190
autoturn = 1
resetBG = 1
localcoord = 320, 240
xscale = 1
yscale = 1

[Shadow]
intensity = 110
color = 0,0,0
yscale = .3

[Reflection]
intensity = 40

[Music]
bgmusic =
bgmvolume = 100

[BGdef]
spr = training.sff
debugbg = 0
bgclearcolor = 4,9,40

; horizon pulse: 4 s breathing
[Begin Action 10]
3,0, 0,0, 60
3,1, 0,0, 60
3,2, 0,0, 60
3,1, 0,0, 60

[BG far]
type = normal
spriteno = 0, 0
start = 0, -700
delta = .3, .3
tile = 1, 0
scalestart = .5, .5

[BG scan]
type = normal
spriteno = 1, 0
start = 0, -40
delta = .3, .3
velocity = 0, -0.25
tile = 1, 1
tilespacing = 0, 300
trans = add
scalestart = .5, .5

[BG wall]
type = normal
spriteno = 2, 0
start = 0, -232
delta = .7, .7
tile = 1, 0
scalestart = .5, .5

[BG horizon]
type = anim
actionno = 10
start = 0, 185
delta = 1, 1
trans = add
scalestart = .5, .5

[BG floor]
type = normal
spriteno = 4, 0
start = 0, 188
delta = 1, 1
tile = 1, 0
scalestart = .5, .5

[BG ruler]
type = normal
spriteno = 5, 0
start = 0, 191
delta = 1, 1
scalestart = .5, .5
"#;

// ---------------------------------------------------------------- ink dojo

const PAPER: Rgba = [243, 239, 228, 255];
const PAPER2: Rgba = [226, 219, 202, 255];
const INK: Rgba = [17, 17, 17, 255];
const RED: Rgba = [224, 32, 32, 255];
const WOOD: Rgba = [204, 192, 168, 255];

/// A brush stroke along `pts` (canvas px): discs whose radius runs from
/// `w0` to `w1`, with a dry edge (random gaps) past `dry` of the length.
fn stroke(c: &mut Canvas, pts: &[(f32, f32)], w0: f32, w1: f32, col: Rgba, dry: f32, seed: u32) {
    let mut len = 0.0;
    for p in pts.windows(2) {
        len += libm::hypotf(p[1].0 - p[0].0, p[1].1 - p[0].1);
    }
    let mut done = 0.0;
    let mut i = 0u32;
    for p in pts.windows(2) {
        let seg = libm::hypotf(p[1].0 - p[0].0, p[1].1 - p[0].1).max(0.001);
        let steps = (seg / 1.5).ceil() as i32;
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            let u = (done + t * seg) / len.max(0.001);
            let x = p[0].0 + (p[1].0 - p[0].0) * t;
            let y = p[0].1 + (p[1].1 - p[0].1) * t;
            let r = w0 + (w1 - w0) * u;
            let wob = 1.0 + (hash(i as i32, 0, seed) - 0.5) * 0.5;
            i += 1;
            if u > dry && hash(i as i32, 1, seed) > 0.86 {
                continue;
            }
            disc(c, x, y, (r * wob).max(0.6), col);
        }
        done += seg;
    }
}

/// Mountain ridge painted as a wash: ink whose alpha runs from `a_top` at the
/// ridge to `a_bot` at the bottom, dry-brush edge, paper grain. Tiles sideways.
fn wash_ridge(
    width: i32,
    height: i32,
    seed: u32,
    a_top: f32,
    a_bot: f32,
    peaks: i32,
    dry: bool,
) -> Canvas {
    let (w, h) = (width * D, height * D);
    let pad = 40 * D;
    let mut wide = Canvas::new((w + 2 * pad) as u32, h as u32);
    let mut ridge = vec![0f32; (w + 2 * pad) as usize];
    for (x, r) in ridge.iter_mut().enumerate() {
        let u = (x as f32 - pad as f32) / w as f32;
        let mut v = 0.0;
        for k in 1..=3 {
            let ph = hash(k, 0, seed) * std::f32::consts::TAU;
            v += libm::sinf(u * std::f32::consts::TAU * (peaks * k) as f32 / 2.0 + ph) / k as f32;
        }
        let jag = (hash(x as i32 / D, 1, seed) - 0.5) * 2.0 * D as f32;
        *r = h as f32 * (0.42 - 0.24 * v) + jag;
    }
    for x in 0..wide.w as i32 {
        let top = ridge[x as usize].max(2.0) as i32;
        for y in top..h {
            let t = ((y - top) as f32 / (h - top).max(1) as f32).clamp(0.0, 1.0);
            let mut a = a_top + (a_bot - a_top) * t;
            let edge = (y - top) as f32 / (5.0 * D as f32);
            if dry {
                // dry edge: gaps in the first few rows under the ridge
                if edge < 1.0 && hash(x, y, seed ^ 0x55) > 0.45 + edge * 0.5 {
                    continue;
                }
            } else if edge < 1.0 {
                // wet edge: the wash just ramps in
                a *= 0.4 + 0.6 * edge;
            }
            // paper grain
            a *= 0.9 + 0.2 * hash(x / D, y / D, seed ^ 0x99);
            // a darker brush line right at the ridge
            if y < top + D {
                a = (a + 0.18).min(1.0);
            }
            wide.put(x, y, with_alpha(INK, a));
        }
    }
    wrap_x(&Canvas::new(w as u32, h as u32), &wide, pad)
}

/// A mist band: paper, strongest in the middle rows, ragged soft edges. Tiles.
fn mist(width: i32, height: i32, seed: u32) -> Canvas {
    let (w, h) = (width * D, height * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for y in 0..h {
        let t = y as f32 / h as f32;
        let core = (1.0 - (t * 2.0 - 1.0).abs()).clamp(0.0, 1.0);
        for x in 0..w {
            // long horizontal streaks of varying strength
            let u = (x as f32 / w as f32) * std::f32::consts::TAU;
            let streak = 0.75 + 0.25 * libm::sinf(u * 3.0 + hash(y / (6 * D), 0, seed) * 6.0);
            let a = core * core * streak * (0.85 + 0.15 * hash(x / D, y / D, seed));
            if a > 0.03 {
                c.put(x, y, with_alpha(PAPER, a.min(1.0)));
            }
        }
    }
    c
}

/// Waterfall frame: a dark cleft in the rock with paper streaks sliding down
/// (offset by `frame`), spray at the foot.
fn waterfall(frame: i32) -> Canvas {
    let (w, h) = (40 * D, 80 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let (x0, x1) = (13 * D, 27 * D);
    // the rock on both sides: ink wash, darker toward the water
    for y in 0..h {
        for x in 0..w {
            let dist = if x < x0 {
                (x0 - x) as f32
            } else if x >= x1 {
                (x - x1 + 1) as f32
            } else {
                0.0
            };
            if dist == 0.0 {
                continue;
            }
            let a = (0.85 - dist / (13.0 * D as f32) * 0.85).max(0.0)
                * (0.85 + 0.3 * hash(x / D, y / D, 60));
            c.put(x, y, with_alpha(INK, a));
        }
    }
    // the water: paper streaks
    for x in x0..x1 {
        let col_seed = hash(x / D, 0, 61);
        for y in 0..h {
            let phase = ((y + frame * 5 * D) / (3 * D)) as i32;
            let on = hash(x / D, phase, 62) > 0.4 || col_seed > 0.8;
            let a = if on { 0.95 } else { 0.55 };
            c.put(x, y, with_alpha(PAPER, a));
        }
    }
    // spray
    for i in 0..16 {
        let x = x0 as f32 - 4.0 * D as f32 + hash(i, frame, 63) * (x1 - x0 + 8 * D) as f32;
        let y = h as f32 - 6.0 * D as f32 - hash(i, frame + 7, 64) * 10.0 * D as f32;
        disc(
            &mut c,
            x,
            y,
            D as f32 * (0.7 + hash(i, 3, 65)),
            with_alpha(PAPER, 0.85),
        );
    }
    c
}

/// A pine leaning in from the left: brush trunk, branches, needle clusters.
fn pine() -> Canvas {
    let (w, h) = (300 * D, 230 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let d = D as f32;
    // trunk from the bottom-left corner, bending right
    stroke(
        &mut c,
        &[
            (10.0 * d, h as f32),
            (40.0 * d, 150.0 * d),
            (90.0 * d, 95.0 * d),
            (150.0 * d, 70.0 * d),
        ],
        7.0 * d,
        2.5 * d,
        INK,
        2.0,
        71,
    );
    // branches
    stroke(
        &mut c,
        &[
            (70.0 * d, 112.0 * d),
            (120.0 * d, 100.0 * d),
            (190.0 * d, 104.0 * d),
        ],
        3.5 * d,
        1.2 * d,
        INK,
        2.0,
        72,
    );
    stroke(
        &mut c,
        &[
            (50.0 * d, 135.0 * d),
            (30.0 * d, 120.0 * d),
            (4.0 * d, 118.0 * d),
        ],
        3.0 * d,
        1.0 * d,
        INK,
        2.0,
        73,
    );
    stroke(
        &mut c,
        &[
            (150.0 * d, 70.0 * d),
            (200.0 * d, 58.0 * d),
            (250.0 * d, 62.0 * d),
        ],
        2.5 * d,
        1.0 * d,
        INK,
        2.0,
        74,
    );
    // needle pads: three stacked flat ellipses per cluster, ragged edges,
    // a few needles hanging from the underside
    let clusters = [
        (190.0, 94.0, 46.0),
        (118.0, 90.0, 36.0),
        (250.0, 54.0, 40.0),
        (160.0, 58.0, 30.0),
        (16.0, 110.0, 28.0),
        (62.0, 102.0, 24.0),
    ];
    for (i, (cx, cy, rx)) in clusters.iter().enumerate() {
        let (cx, cy, rx) = (cx * d, cy * d, rx * d);
        for layer in 0..3 {
            let ly = cy - 4.0 * d + layer as f32 * 4.0 * d;
            let lrx = rx * (0.7 + 0.15 * layer as f32);
            let ry = 3.2 * d;
            for y in (ly - ry * 1.6) as i32..(ly + ry * 1.6) as i32 {
                for x in (cx - lrx * 1.1) as i32..(cx + lrx * 1.1) as i32 {
                    let (dx, dy) = ((x as f32 - cx) / lrx, (y as f32 - ly) / ry);
                    let r = dx * dx + dy * dy;
                    let n = hash(x / D, y / D, 80 + i as u32 * 3 + layer as u32);
                    if r < 1.0 && (r < 0.6 || n > r - 0.4) {
                        c.put(x, y, with_alpha(INK, 0.9));
                    } else if r < 1.5 && n > 0.88 {
                        c.put(x, y, with_alpha(INK, 0.6));
                    }
                }
            }
        }
        for k in 0..7 {
            let u = (k as f32 + 0.5) / 7.0 * 2.0 - 1.0;
            let (sx, sy) = (cx + u * rx * 0.85, cy + 7.0 * d);
            stroke(
                &mut c,
                &[
                    (sx, sy),
                    (
                        sx + u * 2.0 * d,
                        sy + (5.0 + 3.0 * hash(k, i as i32, 95)) * d,
                    ),
                ],
                0.9 * d,
                0.3 * d,
                INK,
                0.0,
                90 + k as u32,
            );
        }
    }
    c
}

/// Bamboo stand (three stalks, leaves swaying by `frame`).
fn bamboo(frame: i32) -> Canvas {
    let (w, h) = (90 * D, 200 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let d = D as f32;
    let sway = [0.0f32, 1.6, -1.2][frame.rem_euclid(3) as usize] * d;
    let stalks = [(22.0, 3.2, 0.0), (48.0, 2.6, 24.0), (72.0, 2.1, 50.0)];
    for (i, (x, wdt, top)) in stalks.iter().enumerate() {
        let x = x * d;
        stroke(
            &mut c,
            &[(x, h as f32), (x + sway * 0.4, top * d)],
            wdt * d,
            wdt * d * 0.8,
            INK,
            2.0,
            100 + i as u32,
        );
        // nodes: paper lines
        let mut y = h as f32 - 26.0 * d;
        while y > top * d + 10.0 * d {
            c.rect(
                (x - wdt * d) as i32,
                y as i32,
                (wdt * 2.0 * d) as i32,
                D,
                with_alpha(PAPER, 0.85),
            );
            y -= 26.0 * d;
        }
        // leaves: pairs of thin tapering strokes from the nodes
        let mut y = h as f32 - 52.0 * d;
        let mut k: i32 = 0;
        while y > top * d + 4.0 * d {
            let dir = if (k as usize + i) % 2 == 0 { -1.0 } else { 1.0 };
            let lx = x + sway;
            for leaf in 0..3i32 {
                let ang = -0.25 + leaf as f32 * 0.35 + hash(k, leaf, 110 + i as u32) * 0.3;
                let len = (12.0 + hash(k, leaf + 3, 111) * 8.0) * d;
                let ex = lx + dir * libm::cosf(ang) * len;
                let ey = y - libm::sinf(ang) * len * 0.6 + 2.0 * d;
                stroke(
                    &mut c,
                    &[(lx, y), (ex, ey)],
                    1.8 * d,
                    0.3 * d,
                    INK,
                    0.0,
                    120 + k as u32 * 3 + leaf as u32,
                );
            }
            y -= 40.0 * d;
            k += 1;
        }
    }
    c
}

/// A crane in flight: a long neck, trailing legs, two curved wing strokes.
fn crane() -> Canvas {
    let (w, h) = (22 * D, 14 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let d = D as f32;
    // body + neck + head
    stroke(
        &mut c,
        &[
            (4.0 * d, 8.0 * d),
            (11.0 * d, 7.5 * d),
            (17.0 * d, 6.0 * d),
            (20.5 * d, 5.0 * d),
        ],
        1.3 * d,
        0.45 * d,
        INK,
        0.0,
        130,
    );
    // legs trailing
    stroke(
        &mut c,
        &[(5.0 * d, 8.0 * d), (0.5 * d, 9.5 * d)],
        0.5 * d,
        0.3 * d,
        INK,
        0.0,
        133,
    );
    // wings: up and down curves from the shoulder
    stroke(
        &mut c,
        &[(10.0 * d, 7.5 * d), (8.0 * d, 4.0 * d), (5.0 * d, 1.0 * d)],
        1.5 * d,
        0.4 * d,
        INK,
        0.0,
        131,
    );
    stroke(
        &mut c,
        &[
            (10.5 * d, 8.0 * d),
            (12.0 * d, 11.0 * d),
            (14.5 * d, 13.5 * d),
        ],
        1.4 * d,
        0.4 * d,
        INK,
        0.0,
        132,
    );
    c.rect(20 * D, 4 * D, 2 * D, D, RED);
    c
}

/// The artist's red seal, tilted, with a paper glyph.
fn seal() -> Canvas {
    let size = 30 * D;
    let mut c = Canvas::new(size as u32, size as u32);
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    let ang = -0.1f32;
    let (ca, sa) = (libm::cosf(ang), libm::sinf(ang));
    let half = 11.0 * D as f32;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let (rx, ry) = (dx * ca + dy * sa, -dx * sa + dy * ca);
            if rx.abs() <= half && ry.abs() <= half {
                // worn edge
                let edge = (half - rx.abs()).min(half - ry.abs());
                if edge < 1.5 * D as f32 && hash(x, y, 140) > 0.6 {
                    continue;
                }
                c.put(x, y, RED);
                // the glyph: a paper grid of strokes (a made-up kanji)
                let (gx, gy) = (rx / half, ry / half);
                let bar = (gy.abs() < 0.08)
                    || ((gy + 0.55).abs() < 0.08 && gx.abs() < 0.7)
                    || ((gy - 0.55).abs() < 0.08 && gx.abs() < 0.7);
                let post = gx.abs() < 0.08 && gy.abs() < 0.8;
                if bar && gx.abs() < 0.8 || post {
                    c.put(x, y, PAPER);
                }
            }
        }
    }
    c
}

/// Ink drops drifting down (tiled both ways, velocity on the layer).
fn drops() -> Canvas {
    let (w, h) = (400 * D, 300 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for i in 0..14 {
        let x = hash(i, 0, 150) * w as f32;
        let y = hash(i, 1, 150) * h as f32;
        let r = (0.5 + hash(i, 2, 150) * 0.6) * D as f32;
        disc(
            &mut c,
            x,
            y,
            r,
            with_alpha(INK, 0.5 + 0.3 * hash(i, 3, 150)),
        );
    }
    c
}

/// Sky: paper with a light wash at the very top (no dots).
fn ink_sky() -> Canvas {
    let (w, h) = (320 * D, 560 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, PAPER);
    for y in 0..(220 * D) {
        let t = 1.0 - y as f32 / (220.0 * D as f32);
        for x in 0..w {
            let a = t * t * 0.16 * (0.85 + 0.3 * hash(x / (3 * D), y / (3 * D), 160));
            c.put(x, y, with_alpha(INK, a));
        }
    }
    c
}

/// Dojo veranda: two thick ink posts, beam with calligraphy, hanging rope and a red paper charm.
fn ink_dojo_frame() -> Canvas {
    let (w, h) = (200 * D, 330 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let beam_y = 12 * D;
    // beam with wood grain
    c.rect(0, beam_y, w, 18 * D, INK);
    for i in 0..6 {
        let y = beam_y + 3 * D + i * 2 * D;
        for x in 0..w {
            if hash(x / (3 * D), i, 7) > 0.55 {
                c.put(x, y, with_alpha(WOOD, 0.35));
            }
        }
    }
    hline(&mut c, beam_y + 18 * D, 0, w, 2 * D, with_alpha(INK, 0.6));
    // post, one per tile
    let post_x = 20 * D;
    let pw = 16 * D;
    c.rect(post_x, beam_y, pw, h - beam_y, INK);
    for y in beam_y..h {
        if (y / D) % 9 < 1 {
            continue;
        }
        if hash(y / (4 * D), 3, 11) > 0.6 {
            c.rect(post_x + 4 * D, y, D, 1, with_alpha(WOOD, 0.4));
        }
    }
    // dry edge on the post
    for y in beam_y..h {
        if hash(y, 5, 12) > 0.7 {
            c.put_raw(post_x + pw - 1, y, [0; 4]);
        }
        if hash(y, 6, 12) > 0.8 {
            c.put_raw(post_x, y, [0; 4]);
        }
    }
    c.rect(post_x - 4 * D, h - 12 * D, pw + 8 * D, 12 * D, INK);
    // shimenawa rope sagging from the beam with paper zigzags
    let (x0, x1) = (post_x + pw, w + post_x);
    for x in x0..x1 {
        let u = (x - x0) as f32 / (x1 - x0) as f32;
        let sag = libm::sinf(u * std::f32::consts::PI) * 16.0 * D as f32;
        let y = beam_y + 22 * D + sag as i32;
        c.rect(x.min(w - 1), y, 1, 3 * D, INK);
        if (x - x0) % (36 * D) == 18 * D {
            // shide: zigzag paper strips, inked outline so they read on paper
            for k in 0..4 {
                let zx = x + if k % 2 == 0 { 0 } else { 4 * D };
                let zy = y + 3 * D + k * 5 * D;
                c.rect(zx - 1, zy, 4 * D + 2, 5 * D + 1, INK);
                c.rect(zx, zy + D, 4 * D, 4 * D, PAPER);
            }
        }
    }
    // red charm on the post
    c.rect(post_x - 2 * D, 90 * D, pw + 4 * D, 34 * D, RED);
    c.rect(post_x, 94 * D, pw, 2 * D, PAPER);
    c.rect(post_x, 118 * D, pw, 2 * D, PAPER);
    c
}

/// Wooden deck: plank seams in perspective, ink hatching toward the camera, red trim.
fn ink_floor() -> Canvas {
    let (w, h) = (200 * D, 440 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, PAPER);
    let mut y = 0.0f32;
    let mut step = 5.0 * D as f32;
    let mut row = 0;
    while (y as i32) < h {
        let y0 = y as i32;
        let y1 = (y + step) as i32;
        hline(&mut c, y0, 0, w, D.max((step / 10.0) as i32), INK);
        let seam = 50 * D;
        let off = if row % 2 == 0 { 0 } else { seam / 2 };
        for sx in (off..w).step_by(seam as usize) {
            vline(&mut c, sx, y0, y1, D, INK);
        }
        let dens = (y / h as f32).min(1.0);
        for gy in y0 + D..y1 {
            for gx in 0..w {
                if (gx + gy * 2) % (6 * D) == 0 && hash(gx, gy, 5) < 0.25 + dens * 0.5 {
                    c.put(gx, gy, with_alpha(INK, 0.55));
                }
            }
        }
        y += step;
        step *= 1.28;
        row += 1;
    }
    // heavy ground stroke and the red trim under it
    c.rect(0, 0, w, 3 * D, INK);
    c.rect(0, 3 * D, w, D, RED);
    c
}

const INK_DEF: &str = r#"; rugen built-in stage, generated by `rugen stage-gen`
; Sumi-e: washes and brush strokes on paper, mist between the ridges.
[Info]
name = "Ink Dojo"
displayname = "Ink Dojo"
author = "rugen"
mugenversion = 1.1

[Camera]
startx = 0
starty = 0
boundleft = -600
boundright = 600
boundhigh = -300
boundlow = 0
verticalfollow = .25
floortension = 20
tension = 50
zoomout = 0.3
zoomin = 1.15
cuthigh = 35
cutlow = 25

[PlayerInfo]
p1startx = -70
p1starty = 0
p1facing = 1
p2startx = 70
p2starty = 0
p2facing = -1
leftbound = -1400
rightbound = 1400

[Bound]
screenleft = 15
screenright = 15

[StageInfo]
zoffset = 190
autoturn = 1
resetBG = 1
localcoord = 320, 240
xscale = 1
yscale = 1

[Shadow]
intensity = 140
color = 17,17,17
yscale = .25

[Reflection]
intensity = 0

[Music]
bgmusic =
bgmvolume = 100

[BGdef]
spr = ink_dojo.sff
debugbg = 0
bgclearcolor = 243,239,228

; waterfall streaks
[Begin Action 30]
4,0, 0,0, 5
4,1, 0,0, 5
4,2, 0,0, 5
4,3, 0,0, 5

; bamboo sway
[Begin Action 40]
10,0, 0,0, 50
10,1, 0,0, 50
10,0, 0,0, 50
10,2, 0,0, 50

[BG sky]
type = normal
spriteno = 0, 0
start = 0, -370
delta = .1, .1
tile = 1, 0
scalestart = .5, .5

[BG seal]
type = normal
spriteno = 1, 0
start = 118, 70
delta = .15, .15
scalestart = .5, .5

[BG crane]
type = normal
spriteno = 2, 0
start = -300, 56
delta = .2, .2
velocity = 0.45, 0
tile = 1, 0
tilespacing = 2600, 0
scalestart = .5, .5

[BG far ridge]
type = normal
spriteno = 3, 0
start = 0, 66
delta = .3, .3
tile = 1, 0
scalestart = .5, .5

[BG mist far]
type = normal
spriteno = 5, 0
start = 0, 112
delta = .38, .38
velocity = 0.1, 0
tile = 1, 0
scalestart = .5, .5

[BG mid ridge]
type = normal
spriteno = 6, 0
start = 0, 108
delta = .5, .5
tile = 1, 0
scalestart = .5, .5

[BG waterfall]
type = anim
actionno = 30
start = 170, 114
delta = .5, .5
scalestart = .5, .5

[BG mist near]
type = normal
spriteno = 7, 0
start = 0, 150
delta = .62, .62
velocity = 0.18, 0
tile = 1, 0
scalestart = .5, .5

[BG near ridge]
type = normal
spriteno = 8, 0
start = 0, 144
delta = .75, .75
tile = 1, 0
scalestart = .5, .5

[BG pine]
type = normal
spriteno = 9, 0
start = -470, 190
delta = .9, .9
scalestart = .5, .5

[BG bamboo]
type = anim
actionno = 40
start = 500, 190
delta = .9, .9
scalestart = .5, .5

[BG drops]
type = normal
spriteno = 11, 0
start = 0, -200
delta = .85, .85
velocity = 0.06, 0.3
tile = 1, 1
scalestart = .5, .5

[BG dojo]
type = normal
spriteno = 12, 0
start = 0, 4
delta = .85, .85
tile = 1, 0
scalestart = .5, .5

[BG deck]
type = normal
spriteno = 13, 0
start = 0, 188
delta = 1, 1
tile = 1, 0
scalestart = .5, .5
"#;

// ---------------------------------------------------------------- ink arena

/// Abyss under the island: ink wash deepening toward the bottom (tiled).
fn ink_abyss() -> Canvas {
    let (w, h) = (320 * D, 360 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for y in 0..h {
        let t = y as f32 / h as f32;
        let a = (libm::powf(t, 0.7) * 0.95).min(0.95);
        for x in 0..w {
            let g = 0.9 + 0.2 * hash(x / (2 * D), y / (2 * D), 201);
            c.put(x, y, with_alpha(INK, (a * g).min(1.0)));
        }
    }
    c
}

/// The ink moon: a paper disc with a thick brush ring and a grey wash inside.
fn moon() -> Canvas {
    let r = 38.0 * D as f32;
    let s = (2.0 * r + 10.0) as u32;
    let mut c = Canvas::new(s, s);
    let m = s as f32 / 2.0;
    // ring: a circular brush stroke with a dry outer edge
    for y in 0..s as i32 {
        for x in 0..s as i32 {
            let (dx, dy) = (x as f32 + 0.5 - m, y as f32 + 0.5 - m);
            let d = libm::sqrtf(dx * dx + dy * dy);
            if d > r {
                continue;
            }
            let ang = libm::atan2f(dy, dx);
            let thick = (4.0 + 2.6 * libm::sinf(ang * 2.0 + 0.8)) * D as f32;
            let n = hash(x, y, 210);
            if d > r - thick {
                if d > r - 1.2 * D as f32 && n > 0.55 {
                    continue;
                }
                c.put(x, y, with_alpha(INK, 0.92));
            } else {
                // inside: a faint wash, heavier toward the lower right
                let a = 0.05 + 0.12 * ((dx + dy) / r * 0.5 + 0.5).clamp(0.0, 1.0);
                c.put(x, y, mix(PAPER, INK, a * (0.8 + 0.4 * n)));
            }
        }
    }
    c
}

/// A drifting rock shard: a ragged ink silhouette with a paper top.
fn shard(seed: u32, width: i32, height: i32) -> Canvas {
    let (w, h) = (width * D, height * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for y in 0..h {
        let t = y as f32 / h as f32;
        // narrows to a point at the bottom, ragged sides
        let half = (w as f32 / 2.0) * (1.0 - t * t) * (0.9 + 0.2 * hash(y / D, 0, seed));
        let cx = w as f32 / 2.0 + (hash(y / (3 * D), 1, seed) - 0.5) * 3.0 * D as f32;
        let (l, r) = ((cx - half) as i32, (cx + half) as i32);
        for x in l.max(0)..r.min(w) {
            let edge = ((x - l).min(r - x)) as f32 / (3.0 * D as f32);
            let col = if y < 3 * D {
                PAPER2
            } else if edge < 1.0 {
                INK
            } else {
                mix(PAPER2, INK, 0.55 + 0.45 * t)
            };
            c.put(x, y, col);
        }
    }
    c
}

/// Ash motes rising from the abyss (tiled both ways, velocity on the layer).
fn ash() -> Canvas {
    let (w, h) = (300 * D, 240 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for i in 0..7 {
        let x = hash(i, 0, 220) * w as f32;
        let y = hash(i, 1, 220) * h as f32;
        let r = (0.5 + hash(i, 2, 220) * 0.5) * D as f32;
        disc(
            &mut c,
            x,
            y,
            r,
            with_alpha(INK, 0.3 + 0.2 * hash(i, 3, 220)),
        );
    }
    c
}

/// Width of one island section in stage units. The 1280-unit deck is four
/// sections (shallow, deep, deep, shallow): two sprites, each placed twice.
const LOBE: i32 = 320;

/// One section of the floating island: a `LOBE`-wide wooden deck strip on a
/// rock that tapers to a point, inked outline, screentone shading
/// underneath. Sections tile edge to edge: the deck and hatching repeat
/// seamlessly and neighbouring rocks merge at the top, so a row of them
/// reads as one long island hanging several points over the pit.
fn ink_island_lobe(depth: i32, seed: u32) -> Canvas {
    let (w, h) = (LOBE * D, depth * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let deck = 10 * D;
    for y in deck..h {
        // the rock narrows with a ragged edge (straight where sections meet)
        let t = (y - deck) as f32 / (h - deck) as f32;
        let rag = (t * 4.0).min(1.0);
        let inset = (t * t * w as f32 * 0.48) as i32
            + (hash(y / (3 * D), 9, seed) * 4.0 * D as f32 * rag) as i32;
        let (l, r) = (inset, w - inset);
        if l >= r {
            continue;
        }
        c.rect(l, y, r - l, 1, INK);
        if r - l > 6 * D {
            // paper inside, hatched tone getting darker toward the tip
            let (pl, pr) = if inset == 0 {
                (0, w)
            } else {
                (l + 2 * D, r - 2 * D)
            };
            c.rect(pl, y, pr - pl, 1, PAPER2);
            for x in pl..pr {
                if (x + y) % (5 * D) < D && hash(x, y, 31) < 0.3 + t * 0.7 {
                    c.put(x, y, INK);
                }
            }
        }
    }
    // the deck: planks with seams and a heavy top stroke
    c.rect(0, 0, w, deck, INK);
    c.rect(0, 2 * D, w, deck - 5 * D, WOOD);
    for sx in (0..w).step_by((32 * D) as usize) {
        vline(&mut c, sx, 2 * D, deck - 3 * D, D, INK);
    }
    // red lip along the deck edge: the thing you fall off
    hline(&mut c, deck - 3 * D, 0, w, 2 * D, RED);
    c
}

/// Mooring post closing each end of the island deck (centred on the edge).
fn ink_island_post() -> Canvas {
    let (w, h) = (8 * D, 16 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(D, 0, w - 2 * D, h, INK);
    c.rect(2 * D, D, w - 4 * D, 3 * D, WOOD);
    // the rope knot
    c.rect(0, 12 * D, w, 2 * D, RED);
    c
}

/// A floating plank platform `len` units wide.
fn ink_platform(len: i32) -> Canvas {
    let (w, h) = ((len + 8) * D, 18 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let x0 = 4 * D;
    let x1 = w - 4 * D;
    c.rect(x0 - 2 * D, 0, x1 - x0 + 4 * D, 8 * D, INK);
    c.rect(x0, 2 * D, x1 - x0, 4 * D, WOOD);
    for sx in (x0 + 12 * D..x1).step_by((28 * D) as usize) {
        vline(&mut c, sx, 2 * D, 6 * D, D, INK);
    }
    // two hanging paper charms
    for cx in [x0 + 10 * D, x1 - 14 * D] {
        vline(&mut c, cx + 2 * D, 8 * D, 11 * D, D, INK);
        c.rect(cx, 11 * D, 5 * D, 7 * D, RED);
        c.rect(cx + D, 13 * D, 3 * D, D, PAPER);
    }
    c
}

const ARENA_DEF: &str = r#"; rugen built-in arena stage, generated by `rugen stage-gen`
; Long floating ink island with nine platforms in three tiers over a pit: no
; walls, 3 stocks, knock the opponent past a blast line (ARENA mode). Ledges,
; items, a sliding and a dropping platform on the top tier, and wind.
[Info]
name = "Ink Arena"
displayname = "Ink Arena"
author = "rugen"
mugenversion = 1.1

[Camera]
startx = 0
starty = 0
boundleft = -760
boundright = 760
boundhigh = -360
boundlow = 0
verticalfollow = 0.6
floortension = 20
tension = 70
; far enough to show the whole island and the bottom blast line
zoomout = 0.25
zoomin = 1.15

[PlayerInfo]
p1startx = -160
p1starty = 0
p1facing = 1
p2startx = 160
p2starty = 0
p2facing = -1
leftbound = -2000
rightbound = 2000

[Bound]
screenleft = 15
screenright = 15

[StageInfo]
zoffset = 190
autoturn = 1
resetBG = 1
localcoord = 320, 240
xscale = 1
yscale = 1

[Shadow]
intensity = 120
color = 17,17,17
yscale = .25

[Reflection]
intensity = 0

[Arena]
; players crossing this box are KO'd (left, top, right, bottom)
blastzone = -920, -540, 920, 160
; no screen-edge walls: knockback carries players off the island
walls = 0
; lives per round
stocks = 3
; the island deck; beyond it is the pit down to the bottom blast line
floor = -640, 640
spawn_y = -140
air_jumps = 1
air_dash = 1
fast_fall = 1
; catch the deck's edges and climb back on (on by default with a floor)
ledge = 1
; pickups (a throwable, a heal, a power boost) every 10-20 s
items = 1
; a gust every 40 s for 4 s, 0.4 units a tick, right then left
wind = 40, 4, 0.4

[Platform]
x = -540, -400
y = -80

[Platform]
x = -240, -100
y = -80

[Platform]
x = 100, 240
y = -80

[Platform]
x = 400, 540
y = -80

[Platform]
x = -390, -250
y = -160

[Platform]
x = -70, 70
y = -160

[Platform]
x = 250, 390
y = -160

[Platform]
x = -230, -90
y = -240
; slides 120 units left and back
path = -120, 0
speed = 0.8

[Platform]
x = 90, 230
y = -240
; drops for 5 s every 20 s (3 s of warning)
drop = 20, 5

[Music]
bgmusic =
bgmvolume = 100

[BGdef]
spr = ink_arena.sff
debugbg = 0
bgclearcolor = 243,239,228

[BG sky]
type = normal
spriteno = 0, 0
start = 0, -370
delta = .1, .1
tile = 1, 0
scalestart = .5, .5

[BG moon]
type = normal
spriteno = 1, 0
start = -170, 24
delta = .15, .15
scalestart = .5, .5

[BG cloud sea]
type = normal
spriteno = 2, 0
start = 0, 150
delta = .3, .3
velocity = 0.05, 0
tile = 1, 0
scalestart = .5, .5

[BG shard a]
type = normal
spriteno = 3, 0
start = -330, 60
delta = .5, .5
sin.y = 6, 180, 0
scalestart = .5, .5

[BG shard b]
type = normal
spriteno = 3, 1
start = 300, 20
delta = .5, .5
sin.y = 8, 220, 120
scalestart = .5, .5

[BG shard c]
type = normal
spriteno = 3, 2
start = 90, 110
delta = .55, .55
sin.y = 5, 160, 240
scalestart = .5, .5

[BG abyss]
type = normal
spriteno = 4, 0
start = 0, 186
delta = .6, .6
tile = 1, 0
scalestart = .5, .5

[BG mist far]
type = normal
spriteno = 9, 0
start = 0, 212
delta = .7, .7
velocity = 0.2, 0
tile = 1, 0
scalestart = .5, .5

[BG ash]
type = normal
spriteno = 10, 0
start = 0, 100
delta = .8, .8
velocity = 0.02, -0.12
tile = 1, 1
scalestart = .5, .5

[BG island 1]
type = normal
spriteno = 5, 0
start = -640, 188
delta = 1, 1
scalestart = .5, .5

[BG island 2]
type = normal
spriteno = 7, 0
start = -320, 188
delta = 1, 1
scalestart = .5, .5

[BG island 3]
type = normal
spriteno = 7, 0
start = 0, 188
delta = 1, 1
scalestart = .5, .5

[BG island 4]
type = normal
spriteno = 5, 0
start = 320, 188
delta = 1, 1
scalestart = .5, .5

[BG post left]
type = normal
spriteno = 8, 0
start = -640, 186
delta = 1, 1
scalestart = .5, .5

[BG post right]
type = normal
spriteno = 8, 0
start = 640, 186
delta = 1, 1
scalestart = .5, .5

[BG platform low -470]
type = normal
spriteno = 6, 0
start = -470, 108
delta = 1, 1
scalestart = .5, .5

[BG platform low -170]
type = normal
spriteno = 6, 0
start = -170, 108
delta = 1, 1
scalestart = .5, .5

[BG platform low 170]
type = normal
spriteno = 6, 0
start = 170, 108
delta = 1, 1
scalestart = .5, .5

[BG platform low 470]
type = normal
spriteno = 6, 0
start = 470, 108
delta = 1, 1
scalestart = .5, .5

[BG platform mid -320]
type = normal
spriteno = 6, 0
start = -320, 28
delta = 1, 1
scalestart = .5, .5

[BG platform mid 0]
type = normal
spriteno = 6, 0
start = 0, 28
delta = 1, 1
scalestart = .5, .5

[BG platform mid 320]
type = normal
spriteno = 6, 0
start = 320, 28
delta = 1, 1
scalestart = .5, .5

[BG platform top -160]
type = normal
spriteno = 6, 0
start = -160, -52
delta = 1, 1
; follows the moving platform (8th [Platform])
platform = 8
scalestart = .5, .5

[BG platform top 160]
type = normal
spriteno = 6, 0
start = 160, -52
delta = 1, 1
; falls with the dropping platform (9th [Platform])
platform = 9
scalestart = .5, .5

[BG mist near]
type = normal
spriteno = 11, 0
start = 0, 236
delta = 1, 1
velocity = 0.4, 0
tile = 1, 0
scalestart = .5, .5
"#;

// ---------------------------------------------------------------- ink stadium

/// Dark roof: ink wash, heaviest at the top (tiled).
fn stadium_roof() -> Canvas {
    let (w, h) = (320 * D, 420 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, PAPER);
    // the dark roof takes the top 45%, the hall below stays paper
    let roof = (h as f32 * 0.45) as i32;
    for y in 0..roof {
        let t = 1.0 - y as f32 / roof as f32;
        let a = (t * t * 0.92).min(0.92);
        for x in 0..w {
            let g = 0.9 + 0.2 * hash(x / (2 * D), y / (2 * D), 301);
            c.put(x, y, with_alpha(INK, (a * g).min(1.0)));
        }
    }
    // rafters: a few ink beams across the roof
    for i in 0..3 {
        let y = roof - 30 * D - i * 46 * D;
        hline(&mut c, y, 0, w, 3 * D, with_alpha(INK, 0.7));
    }
    c
}

/// Barrier rail between the crowd and the ring: an ink wall with a paper top rail.
fn barrier() -> Canvas {
    let (w, h) = (200 * D, 16 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 2 * D, w, h - 2 * D, INK);
    c.rect(0, 0, w, 2 * D, PAPER2);
    hline(&mut c, 2 * D, 0, w, D, with_alpha(PAPER, 0.3));
    for x in (0..w).step_by((25 * D) as usize) {
        c.rect(x, 2 * D, D, h - 2 * D, with_alpha(PAPER, 0.25));
    }
    c
}

/// A crowd tier: rows of heads and shoulders in ink, denser and darker toward
/// the back; the front row is drawn solid. Tiles sideways.
fn crowd(width: i32, rows: i32, seed: u32) -> Canvas {
    let row_h = 9 * D;
    let (w, h) = (width * D, rows * row_h + 6 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for row in 0..rows {
        let back = 1.0 - row as f32 / rows as f32;
        let y = row * row_h + 6 * D;
        let a = 0.55 + 0.4 * (1.0 - back);
        let mut x = -(hash(row, 0, seed) * 8.0 * D as f32) as i32;
        while x < w + 8 * D {
            let hw = (3.0 + hash(x, row, seed) * 1.5) * D as f32;
            let hh = (3.0 + hash(x, row + 7, seed) * 1.5) * D as f32;
            let cx = x as f32 + hw;
            let cy = y as f32 + hh;
            // shoulders then head, so heads overlap the row behind
            let sw = hw * 2.2;
            c.rect(
                (cx - sw) as i32,
                (cy + hh * 0.6) as i32,
                (sw * 2.0) as i32,
                row_h,
                with_alpha(INK, a),
            );
            disc(&mut c, cx, cy, hw.min(hh), with_alpha(INK, a));
            // a few raised arms
            if hash(x, row + 3, seed) > 0.9 {
                let ax = cx
                    + if hash(x, row + 4, seed) > 0.5 {
                        hw
                    } else {
                        -hw
                    };
                c.rect(
                    ax as i32,
                    (cy - hh * 2.2) as i32,
                    D,
                    (hh * 2.0) as i32,
                    with_alpha(INK, a),
                );
                disc(
                    &mut c,
                    ax + 0.5,
                    cy - hh * 2.3,
                    1.2 * D as f32,
                    with_alpha(INK, a),
                );
            }
            x += (hw * 2.0 + (1.0 + hash(x, row + 1, seed) * 3.0) * D as f32) as i32;
        }
    }
    c
}

/// A banner hanging from the roof: paper cloth, a mark, ink edges.
fn banner(red: bool) -> Canvas {
    let (w, h) = (34 * D, 50 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    // rod and strings
    c.rect(0, 0, w, 2 * D, INK);
    c.rect(4 * D, 2 * D, D, 3 * D, INK);
    c.rect(w - 5 * D, 2 * D, D, 3 * D, INK);
    // cloth with a ragged bottom
    for y in 5 * D..h {
        let frayed = y > h - 4 * D && hash(y, 0, 310) > 0.5;
        let (l, r) = if frayed {
            (3 * D, w - 3 * D)
        } else {
            (2 * D, w - 2 * D)
        };
        c.rect(l, y, r - l, 1, PAPER);
        c.put(l, y, INK);
        c.put(r - 1, y, INK);
    }
    hline(&mut c, 5 * D, 2 * D, w - 2 * D, D, INK);
    // the mark: a skewed block with two paper bars (the rugen mark, reduced)
    let col = if red { RED } else { INK };
    for y in 12 * D..34 * D {
        let skew = (34 * D - y) / 6;
        c.rect(7 * D + skew, y, 20 * D, 1, col);
    }
    c.rect(11 * D, 17 * D, 9 * D, 2 * D, PAPER);
    c.rect(11 * D, 23 * D, 12 * D, 2 * D, PAPER);
    // a thin ink line under the mark
    hline(&mut c, 38 * D, 8 * D, w - 8 * D, D, with_alpha(INK, 0.7));
    c
}

/// Ring ropes: three lines across the tile (ink, red, ink) with the post shadow line.
fn ropes() -> Canvas {
    let (w, h) = (200 * D, 60 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    for (i, col) in [INK, RED, INK].into_iter().enumerate() {
        let y = 6 * D + i as i32 * 20 * D;
        hline(&mut c, y, 0, w, 3 * D, col);
        // twist marks
        for x in (0..w).step_by((7 * D) as usize) {
            c.rect(x, y, D, 3 * D, with_alpha(PAPER, 0.35));
        }
    }
    c
}

/// Ring post: an ink post with a red cap and the rope knots.
fn ring_post() -> Canvas {
    let (w, h) = (12 * D, 76 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(3 * D, 0, 6 * D, h, INK);
    c.rect(D, 0, 10 * D, 5 * D, RED);
    for i in 0..3 {
        let y = 10 * D + i * 20 * D;
        c.rect(0, y, w, 5 * D, INK);
        c.rect(D, y + D, w - 2 * D, 3 * D, PAPER2);
    }
    c
}

/// Ring canvas and apron: paper mat with a red centre line and perspective
/// seams, a black apron under the edge (delta 1, tiled).
fn ring_floor() -> Canvas {
    let (w, h) = (200 * D, 440 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let mat = 70 * D;
    c.rect(0, 0, w, mat, PAPER);
    // mat grain
    for y in 0..mat {
        for x in 0..w {
            if hash(x / D, y / D, 320) > 0.93 {
                c.put(x, y, with_alpha(INK, 0.18));
            }
        }
    }
    c.rect(0, 0, w, 3 * D, INK);
    hline(&mut c, 16 * D, 0, w, 2 * D, RED);
    let mut y = 3.0 * D as f32;
    let mut step = 6.0 * D as f32;
    while (y as i32) < mat {
        hline(&mut c, y as i32, 0, w, D, with_alpha(INK, 0.25));
        y += step;
        step *= 1.25;
    }
    // apron: ink with a paper skirt line, then the hall floor
    c.rect(0, mat, w, 10 * D, INK);
    hline(&mut c, mat + 3 * D, 0, w, D, with_alpha(PAPER, 0.5));
    for y in mat + 10 * D..h {
        let t = (y - mat) as f32 / (h - mat) as f32;
        c.rect(0, y, w, 1, mix(PAPER2, INK, 0.75 + 0.25 * t));
    }
    c
}

/// A camera flash in the crowd: a bright dot with a halo (frame 0) and a blank (frame 1).
fn flash(on: bool) -> Canvas {
    let s = 14 * D;
    let mut c = Canvas::new(s as u32, s as u32);
    if on {
        let m = s as f32 / 2.0;
        disc(&mut c, m, m, 6.0 * D as f32, with_alpha(PAPER, 0.35));
        disc(&mut c, m, m, 3.0 * D as f32, with_alpha(PAPER, 0.8));
        disc(&mut c, m, m, 1.5 * D as f32, PAPER);
    }
    c
}

const STADIUM_DEF: &str = r#"; rugen built-in stage, generated by `rugen stage-gen`
; A raised ring under a dark roof, a screentone crowd on two tiers, three
; ropes with the middle one red: the ranked and tournament stage.
[Info]
name = "Ink Stadium"
displayname = "Ink Stadium"
author = "rugen"
mugenversion = 1.1

[Camera]
startx = 0
starty = 0
boundleft = -480
boundright = 480
boundhigh = -300
boundlow = 0
verticalfollow = .25
floortension = 20
tension = 50
zoomout = 0.35
zoomin = 1.1
cuthigh = 35
cutlow = 25

[PlayerInfo]
p1startx = -70
p1starty = 0
p1facing = 1
p2startx = 70
p2starty = 0
p2facing = -1
leftbound = -1200
rightbound = 1200

[Bound]
screenleft = 15
screenright = 15

[StageInfo]
zoffset = 190
autoturn = 1
resetBG = 1
localcoord = 320, 240
xscale = 1
yscale = 1

[Shadow]
intensity = 160
color = 17,17,17
yscale = .3

[Reflection]
intensity = 0

[Music]
bgmusic =
bgmvolume = 100

[BGdef]
spr = ink_stadium.sff
debugbg = 0
bgclearcolor = 243,239,228

; camera flashes, three rhythms
[Begin Action 60]
12,1, 0,0, 170
12,0, 0,0, 3
12,1, 0,0, 240
12,0, 0,0, 3
[Begin Action 61]
12,1, 0,0, 70
12,0, 0,0, 3
12,1, 0,0, 310
12,0, 0,0, 3
[Begin Action 62]
12,1, 0,0, 400
12,0, 0,0, 3
12,1, 0,0, 130
12,0, 0,0, 3

[BG roof]
type = normal
spriteno = 0, 0
start = 0, -440
delta = .1, .1
tile = 1, 0
scalestart = .5, .5

[BG crowd back]
type = normal
spriteno = 1, 0
start = 0, 104
delta = .3, .3
tile = 1, 0
sin.y = 1.2, 150, 0
scalestart = .5, .5

[BG flash a]
type = anim
actionno = 60
start = -120, 84
delta = .3, .3
trans = add
scalestart = .5, .5

[BG flash b]
type = anim
actionno = 61
start = 170, 76
delta = .3, .3
trans = add
scalestart = .5, .5

[BG flash c]
type = anim
actionno = 62
start = 40, 92
delta = .3, .3
trans = add
scalestart = .5, .5

[BG crowd front]
type = normal
spriteno = 2, 0
start = 0, 128
delta = .5, .5
tile = 1, 0
sin.y = 1.4, 120, 60
scalestart = .5, .5

[BG barrier]
type = normal
spriteno = 3, 0
start = 0, 124
delta = .6, .6
tile = 1, 0
scalestart = .5, .5

[BG banner left]
type = normal
spriteno = 4, 0
start = -330, 8
delta = 1, 1
sin.x = 1.5, 90, 0
scalestart = .5, .5

[BG banner right]
type = normal
spriteno = 5, 0
start = 330, 8
delta = 1, 1
sin.x = 1.5, 90, 180
scalestart = .5, .5

[BG floor]
type = normal
spriteno = 6, 0
start = 0, 188
delta = 1, 1
tile = 1, 0
scalestart = .5, .5

[BG ropes]
type = normal
spriteno = 7, 0
start = 0, 118
delta = 1, 1
tile = 1, 0
scalestart = .5, .5

[BG post left]
type = normal
spriteno = 8, 0
start = -440, 116
delta = 1, 1
scalestart = .5, .5

[BG post right]
type = normal
spriteno = 8, 0
start = 440, 116
delta = 1, 1
scalestart = .5, .5
"#;

fn sff() -> Sff {
    Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    }
}

/// Write the stages under `out` (`out/training/`, `out/ink_dojo/`, `out/ink_arena/`).
pub fn generate(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("training");
    std::fs::create_dir_all(&dir)?;
    let mut s = sff();
    let top = |c: &Canvas| [(c.w / 2) as i16, 0];
    let far = training_far();
    to_sprite(&mut s, (0, 0), &far, top(&far));
    let scan = training_scan();
    to_sprite(
        &mut s,
        (1, 0),
        &scan,
        [(scan.w / 2) as i16, (scan.h / 2) as i16],
    );
    let wall = training_wall();
    to_sprite(&mut s, (2, 0), &wall, top(&wall));
    for (i, level) in [0.3f32, 0.65, 1.0].into_iter().enumerate() {
        let hz = training_horizon(level);
        to_sprite(
            &mut s,
            (3, i as u16),
            &hz,
            [(hz.w / 2) as i16, (hz.h / 2) as i16],
        );
    }
    let floor = training_floor();
    to_sprite(&mut s, (4, 0), &floor, top(&floor));
    let ruler = training_ruler();
    to_sprite(&mut s, (5, 0), &ruler, top(&ruler));
    std::fs::write(dir.join("training.sff"), s.write_v2())?;
    std::fs::write(dir.join("training.def"), TRAINING_DEF)?;

    let dir = out.join("ink_dojo");
    std::fs::create_dir_all(&dir)?;
    let mut s = sff();
    let top = |c: &Canvas| [(c.w / 2) as i16, 0];
    let bottom = |c: &Canvas| [(c.w / 2) as i16, c.h as i16];
    let mid = |c: &Canvas| [(c.w / 2) as i16, (c.h / 2) as i16];
    let sky = ink_sky();
    to_sprite(&mut s, (0, 0), &sky, top(&sky));
    let sl = seal();
    to_sprite(&mut s, (1, 0), &sl, mid(&sl));
    let cr = crane();
    to_sprite(&mut s, (2, 0), &cr, mid(&cr));
    let far = wash_ridge(360, 90, 21, 0.4, 0.04, 5, false);
    to_sprite(&mut s, (3, 0), &far, top(&far));
    for f in 0..4 {
        let wf = waterfall(f);
        to_sprite(&mut s, (4, f as u16), &wf, top(&wf));
    }
    let m1 = mist(320, 40, 3);
    to_sprite(&mut s, (5, 0), &m1, mid(&m1));
    let midr = wash_ridge(300, 90, 4, 0.62, 0.1, 5, false);
    to_sprite(&mut s, (6, 0), &midr, top(&midr));
    let m2 = mist(300, 34, 9);
    to_sprite(&mut s, (7, 0), &m2, mid(&m2));
    let near = wash_ridge(260, 60, 8, 0.96, 0.6, 4, true);
    to_sprite(&mut s, (8, 0), &near, top(&near));
    let pn = pine();
    to_sprite(&mut s, (9, 0), &pn, bottom(&pn));
    for f in 0..3 {
        let bb = bamboo(f);
        to_sprite(&mut s, (10, f as u16), &bb, bottom(&bb));
    }
    let dr = drops();
    to_sprite(&mut s, (11, 0), &dr, top(&dr));
    let frame = ink_dojo_frame();
    to_sprite(&mut s, (12, 0), &frame, top(&frame));
    let deck = ink_floor();
    to_sprite(&mut s, (13, 0), &deck, top(&deck));
    std::fs::write(dir.join("ink_dojo.sff"), s.write_v2())?;
    std::fs::write(dir.join("ink_dojo.def"), INK_DEF)?;

    let dir = out.join("ink_arena");
    std::fs::create_dir_all(&dir)?;
    let mut s = sff();
    let sky = ink_sky();
    to_sprite(&mut s, (0, 0), &sky, top(&sky));
    let mn = moon();
    to_sprite(&mut s, (1, 0), &mn, mid(&mn));
    let sea = wash_ridge(400, 70, 33, 0.22, 0.0, 3, false);
    to_sprite(&mut s, (2, 0), &sea, top(&sea));
    for (i, (seed, w, h)) in [(41u32, 40, 34), (42, 56, 44), (43, 30, 26)]
        .into_iter()
        .enumerate()
    {
        let sh = shard(seed, w, h);
        to_sprite(&mut s, (3, i as u16), &sh, mid(&sh));
    }
    let ab = ink_abyss();
    to_sprite(&mut s, (4, 0), &ab, top(&ab));
    let lobe_a = ink_island_lobe(90, 13);
    to_sprite(&mut s, (5, 0), &lobe_a, [0, 0]);
    let pf = ink_platform(140);
    to_sprite(&mut s, (6, 0), &pf, top(&pf));
    let lobe_b = ink_island_lobe(120, 7);
    to_sprite(&mut s, (7, 0), &lobe_b, [0, 0]);
    let post = ink_island_post();
    to_sprite(&mut s, (8, 0), &post, top(&post));
    let m1 = mist(320, 60, 17);
    to_sprite(&mut s, (9, 0), &m1, mid(&m1));
    let motes = ash();
    to_sprite(&mut s, (10, 0), &motes, top(&motes));
    let m2 = mist(300, 90, 23);
    to_sprite(&mut s, (11, 0), &m2, mid(&m2));
    std::fs::write(dir.join("ink_arena.sff"), s.write_v2())?;
    std::fs::write(dir.join("ink_arena.def"), ARENA_DEF)?;

    let dir = out.join("ink_stadium");
    std::fs::create_dir_all(&dir)?;
    let mut s = sff();
    let roof = stadium_roof();
    to_sprite(&mut s, (0, 0), &roof, top(&roof));
    let back = crowd(320, 4, 51);
    to_sprite(&mut s, (1, 0), &back, bottom(&back));
    let front = crowd(300, 3, 52);
    to_sprite(&mut s, (2, 0), &front, bottom(&front));
    let bar = barrier();
    to_sprite(&mut s, (3, 0), &bar, top(&bar));
    let bl = banner(true);
    to_sprite(&mut s, (4, 0), &bl, [0, 0]);
    let br = banner(false);
    to_sprite(&mut s, (5, 0), &br, [0, 0]);
    let fl = ring_floor();
    to_sprite(&mut s, (6, 0), &fl, top(&fl));
    let rp = ropes();
    to_sprite(&mut s, (7, 0), &rp, top(&rp));
    let post = ring_post();
    to_sprite(&mut s, (8, 0), &post, [0, 0]);
    for (i, on) in [true, false].into_iter().enumerate() {
        let f = flash(on);
        to_sprite(&mut s, (12, i as u16), &f, mid(&f));
    }
    std::fs::write(dir.join("ink_stadium.sff"), s.write_v2())?;
    std::fs::write(dir.join("ink_stadium.def"), STADIUM_DEF)?;
    Ok(())
}
