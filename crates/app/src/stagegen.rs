//! `rugen stage-gen`: the built-in stages shipped in `motif/stages/`.
//!
//! - `training`: a high-detail take on MUGEN's blue "Training Room" grid.
//! - `ink_dojo`: a Shonen Ink manga dojo (paper, screentone, brush ink).
//!
//! Both are built for the far camera: every layer tiles sideways, layers run
//! far above and below the play area, and the clear colour matches the top
//! of the sky, so nothing runs out down to `zoomout = 0.3`. Art is drawn at
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

// ---------------------------------------------------------------- training

const T_SKY: Rgba = [4, 9, 40, 255];
const T_WALL: Rgba = [10, 40, 150, 255];
const T_LINE: Rgba = [46, 104, 255, 255];
const T_GLOW: Rgba = [120, 210, 255, 255];
const T_FLOOR: Rgba = [8, 36, 140, 255];
const T_DEEP: Rgba = [3, 12, 60, 255];

/// Far wall: night-blue gradient with a big faint grid (delta .5).
fn training_far() -> Canvas {
    let (w, h) = (256 * D, 520 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.vgrad(0, 0, w, h, T_SKY, mix(T_SKY, T_WALL, 0.75));
    let cell = 64 * D;
    for y in (0..h).step_by(cell as usize) {
        let a = 0.08 + 0.22 * (y as f32 / h as f32);
        hline(&mut c, y, 0, w, D, with_alpha(T_LINE, a));
    }
    for x in (0..w).step_by(cell as usize) {
        for y in 0..h {
            let a = 0.08 + 0.22 * (y as f32 / h as f32);
            c.rect(x, y, D, 1, with_alpha(T_LINE, a));
        }
    }
    // distant light strips near the horizon
    for i in 0..4 {
        let x = i * cell + cell / 2 - 6 * D;
        c.rect(x, h - 70 * D, 12 * D, 3 * D, with_alpha(T_GLOW, 0.35));
    }
    c
}

/// Near wall: MUGEN's training grid, fading out toward the top.
fn training_wall() -> Canvas {
    let (w, h) = (160 * D, 300 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    let fade = 90 * D;
    for y in 0..h {
        let a = (y as f32 / fade as f32).min(1.0);
        let base = mix(mix(T_WALL, T_SKY, 0.35), T_WALL, y as f32 / h as f32);
        c.rect(0, y, w, 1, with_alpha(base, a));
    }
    let cell = 40 * D;
    for y in (h % cell..h).step_by(cell as usize) {
        let a = (y as f32 / fade as f32).min(1.0);
        hline(&mut c, y, 0, w, D, with_alpha(T_LINE, a));
    }
    for x in (0..w).step_by(cell as usize) {
        for y in 0..h {
            let a = (y as f32 / fade as f32).min(1.0);
            c.rect(x, y, D, 1, with_alpha(T_LINE, a));
        }
    }
    // horizon glow along the floor line
    hline(&mut c, h - 3 * D, 0, w, D, with_alpha(T_GLOW, 0.5));
    hline(&mut c, h - 2 * D, 0, w, 2 * D, T_GLOW);
    c
}

/// Floor: perspective rows getting further apart toward the camera.
fn training_floor() -> Canvas {
    let (w, h) = (160 * D, 440 * D);
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

const TRAINING_DEF: &str = r#"; rugen built-in stage, generated by `rugen stage-gen`
[Info]
name = "Training Room EX"
displayname = "Training Room EX"
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
intensity = 110
color = 0,0,0
yscale = .3

[Reflection]
intensity = 32

[Music]
bgmusic =
bgmvolume = 100

[BGdef]
spr = training.sff
debugbg = 0
bgclearcolor = 4,9,40

[BG far]
type = normal
spriteno = 0, 0
start = 0, -330
delta = .5, .5
tile = 1, 0
scalestart = .5, .5

[BG wall]
type = normal
spriteno = 1, 0
start = 0, -112
delta = 1, 1
tile = 1, 0
scalestart = .5, .5

[BG floor]
type = normal
spriteno = 2, 0
start = 0, 188
delta = 1, 1
tile = 1, 0
scalestart = .5, .5
"#;

// ---------------------------------------------------------------- ink dojo

const PAPER: Rgba = [243, 239, 228, 255];
const PAPER2: Rgba = [226, 219, 202, 255];
const INK: Rgba = [17, 17, 17, 255];
const RED: Rgba = [224, 32, 32, 255];
const WOOD: Rgba = [204, 192, 168, 255];

/// Sky: paper, screentone getting denser toward the top.
fn ink_sky() -> Canvas {
    let (w, h) = (320 * D, 560 * D);
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, PAPER);
    tone(&mut c, 6 * D, [17, 17, 17, 70], |_, y| {
        (1.0 - y as f32 / (h as f32 * 0.8)).clamp(0.0, 1.0) * 0.55
    });
    c
}

/// Red sun with an ink ring (single, not tiled).
fn ink_sun() -> Canvas {
    let r = 40.0 * D as f32;
    let s = (2.0 * r + 8.0) as u32;
    let mut c = Canvas::new(s, s);
    let m = s as f32 / 2.0;
    disc(&mut c, m, m, r, INK);
    disc(&mut c, m, m, r - 3.0 * D as f32, RED);
    // brush-dry streaks across the sun
    for i in 0..5 {
        let y = (m - r * 0.5 + i as f32 * r * 0.25) as i32;
        let len = (r * (1.2 - 0.15 * i as f32)) as i32;
        c.rect(m as i32 - len / 2, y, len, D, with_alpha(PAPER, 0.35));
    }
    c
}

/// Mountain range silhouette; `far` ranges are screentoned grey.
fn ink_mountains(width: i32, height: i32, seed: u32, far: bool) -> Canvas {
    let (w, h) = (width * D, height * D);
    let pad = 40 * D;
    let mut wide = Canvas::new((w + 2 * pad) as u32, h as u32);
    let peaks = 5;
    let mut ridge = vec![0f32; (w + 2 * pad) as usize];
    for (x, r) in ridge.iter_mut().enumerate() {
        let u = (x as f32 - pad as f32) / w as f32;
        let mut v = 0.0;
        for k in 1..=3 {
            let ph = hash(k, 0, seed) * std::f32::consts::TAU;
            v += libm::sinf(u * std::f32::consts::TAU * (peaks * k) as f32 / 2.0 + ph) / k as f32;
        }
        let jag = (hash(x as i32 / D, 1, seed) - 0.5) * 3.0 * D as f32;
        *r = h as f32 * (0.45 - 0.22 * v) + jag;
    }
    for x in 0..wide.w as i32 {
        let top = ridge[x as usize].max(2.0) as i32;
        for y in top..h {
            let col = if far {
                if ((x / D) % 3 == 0 && (y / D) % 3 == 0) || y < top + D {
                    INK
                } else {
                    [0; 4]
                }
            } else if y < top + 2 * D {
                INK
            } else {
                // inside: solid ink with paper hatching near the ridge (snow light)
                let near = (y - top) as f32 / (30.0 * D as f32);
                if near < 1.0 && (x + y) % (4 * D) < D && hash(x, y, seed) > near {
                    PAPER2
                } else {
                    INK
                }
            };
            if col[3] > 0 {
                wide.put(x, y, col);
            }
        }
    }
    wrap_x(&Canvas::new(w as u32, h as u32), &wide, pad)
}

/// Dojo veranda: two thick ink posts, beam, hanging rope and a red paper charm.
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
    // posts every 200 units, one per tile
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
    // post foot block
    c.rect(post_x - 4 * D, h - 12 * D, pw + 8 * D, 12 * D, INK);
    // shimenawa rope sagging from the beam with paper zigzags
    let (x0, x1) = (post_x + pw, w + post_x);
    for x in x0..x1 {
        let u = (x - x0) as f32 / (x1 - x0) as f32;
        let sag = libm::sinf(u * std::f32::consts::PI) * 16.0 * D as f32;
        let y = beam_y + 22 * D + sag as i32;
        c.rect(x.min(w - 1), y, 1, 3 * D, INK);
        if (x - x0) % (36 * D) == 18 * D {
            for k in 0..4 {
                let zx = x + if k % 2 == 0 { 0 } else { 4 * D };
                c.rect(zx, y + 3 * D + k * 5 * D, 4 * D, 5 * D, PAPER);
                c.rect(zx, y + 3 * D + k * 5 * D, 4 * D, D, INK);
            }
        }
    }
    // red charm on the post
    c.rect(post_x - 2 * D, 90 * D, pw + 4 * D, 34 * D, RED);
    c.rect(post_x, 94 * D, pw, 2 * D, PAPER);
    c.rect(post_x, 118 * D, pw, 2 * D, PAPER);
    c
}

/// Wooden deck: plank seams in perspective, ink hatching toward the camera.
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
        // staggered vertical seams
        let seam = 50 * D;
        let off = if row % 2 == 0 { 0 } else { seam / 2 };
        for sx in (off..w).step_by(seam as usize) {
            vline(&mut c, sx, y0, y1, D, INK);
        }
        // grain hatching, denser in front
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
    // heavy ground stroke
    c.rect(0, 0, w, 3 * D, INK);
    c
}

const INK_DEF: &str = r#"; rugen built-in stage, generated by `rugen stage-gen`
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

[BG sky]
type = normal
spriteno = 0, 0
start = 0, -370
delta = .1, .1
tile = 1, 0
scalestart = .5, .5

[BG sun]
type = normal
spriteno = 1, 0
start = 110, 92
delta = .12, .12
scalestart = .5, .5

[BG far range]
type = normal
spriteno = 2, 0
start = 0, 118
delta = .25, .25
tile = 1, 0
scalestart = .5, .5

[BG near range]
type = normal
spriteno = 3, 0
start = 0, 136
delta = .45, .45
tile = 1, 0
scalestart = .5, .5

[BG dojo]
type = normal
spriteno = 4, 0
start = 0, 4
delta = .85, .85
tile = 1, 0
scalestart = .5, .5

[BG deck]
type = normal
spriteno = 5, 0
start = 0, 188
delta = 1, 1
tile = 1, 0
scalestart = .5, .5
"#;

fn sff() -> Sff {
    Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    }
}

/// Write both stages under `out` (`out/training/`, `out/ink_dojo/`).
pub fn generate(out: &Path) -> anyhow::Result<()> {
    let dir = out.join("training");
    std::fs::create_dir_all(&dir)?;
    let mut s = sff();
    let far = training_far();
    to_sprite(&mut s, (0, 0), &far, [(far.w / 2) as i16, 0]);
    let wall = training_wall();
    to_sprite(&mut s, (1, 0), &wall, [(wall.w / 2) as i16, 0]);
    let floor = training_floor();
    to_sprite(&mut s, (2, 0), &floor, [(floor.w / 2) as i16, 0]);
    std::fs::write(dir.join("training.sff"), s.write_v2())?;
    std::fs::write(dir.join("training.def"), TRAINING_DEF)?;

    let dir = out.join("ink_dojo");
    std::fs::create_dir_all(&dir)?;
    let mut s = sff();
    let layers = [
        ink_sky(),
        ink_sun(),
        ink_mountains(360, 90, 21, true),
        ink_mountains(300, 70, 4, false),
        ink_dojo_frame(),
        ink_floor(),
    ];
    for (i, c) in layers.iter().enumerate() {
        let axis = if i == 1 {
            [(c.w / 2) as i16, (c.h / 2) as i16]
        } else {
            [(c.w / 2) as i16, 0]
        };
        to_sprite(&mut s, (i as u16, 0), c, axis);
    }
    std::fs::write(dir.join("ink_dojo.sff"), s.write_v2())?;
    std::fs::write(dir.join("ink_dojo.def"), INK_DEF)?;
    Ok(())
}
