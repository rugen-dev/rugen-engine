//! Shonen Ink menu screens from the approved UI book: HOME, CHARACTER SELECT,
//! VERSUS, PAUSE and RESULT. Draws their pixel-art pieces (slanted manga
//! panels, speed-line fans, bursts, katakana SFX, keycaps, slabs) and writes
//! the screen TOMLs. Called by [`crate::inkgen::generate`].
//!
//! Pieces are drawn at half resolution and doubled (2-px pixels), like the
//! rest of the theme. TOML coordinates are the book's 1280x720 coordinates.

use std::path::Path;

use crate::fontgen::{self, Generator};
use crate::inkgen::{INK, PAPER, RED, WHITE};
use crate::pixelize;
use crate::screenpack::Canvas;

type Rgba = [u8; 4];

const PAPER2: Rgba = [230, 223, 205, 255];
const RED_DARK: Rgba = [150, 16, 16, 255];

// ---------------------------------------------------------------- drawing helpers

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

/// Pixel centres inside the polygon (even-odd rule).
fn poly_mask(w: u32, h: u32, pts: &[(f32, f32)]) -> Vec<bool> {
    let mut m = vec![false; (w * h) as usize];
    for y in 0..h as i32 {
        let fy = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if (a.1 <= fy && b.1 > fy) || (b.1 <= fy && a.1 > fy) {
                xs.push(a.0 + (fy - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            let x0 = libm::ceilf(pair[0] - 0.5).max(0.0) as i32;
            let x1 = libm::floorf(pair[1] - 0.5).min(w as f32 - 1.0) as i32;
            for x in x0..=x1 {
                m[(y as u32 * w + x as u32) as usize] = true;
            }
        }
    }
    m
}

fn fill_poly(c: &mut Canvas, pts: &[(f32, f32)], col: Rgba) {
    let m = poly_mask(c.w, c.h, pts);
    for y in 0..c.h {
        for x in 0..c.w {
            if m[(y * c.w + x) as usize] {
                c.put(x as i32, y as i32, col);
            }
        }
    }
}

/// Keep only pixels where `mask` is set.
fn apply_mask(c: &mut Canvas, mask: &[bool]) {
    for (i, m) in mask.iter().enumerate() {
        if !m {
            c.px[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
        }
    }
}

/// Pixels of `mask` within `t` pixels of its outside (a border band).
fn border_of(mask: &[bool], w: u32, h: u32, t: i32) -> Vec<bool> {
    let at = |x: i32, y: i32| {
        x >= 0 && y >= 0 && x < w as i32 && y < h as i32 && mask[(y as u32 * w + x as u32) as usize]
    };
    let mut out = vec![false; mask.len()];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if !at(x, y) {
                continue;
            }
            'near: for dy in -t + 1..t {
                for dx in -t + 1..t {
                    if !at(x + dx, y + dy) {
                        out[(y as u32 * w + x as u32) as usize] = true;
                        break 'near;
                    }
                }
            }
        }
    }
    out
}

fn paint_mask(c: &mut Canvas, mask: &[bool], col: Rgba) {
    for y in 0..c.h {
        for x in 0..c.w {
            if mask[(y * c.w + x) as usize] {
                c.put(x as i32, y as i32, col);
            }
        }
    }
}

/// Manga speed-line fan: thin wedges from radius `r0` outwards around `(cx, cy)`.
fn speed_fan(c: &mut Canvas, cx: f32, cy: f32, r0: f32, opacity: f32, col: Rgba, seed: u32) {
    let mut s = seed.max(1) as u64;
    let mut rnd = || {
        s = (s * 16807) % 2_147_483_647;
        s as f32 / 2_147_483_647.0
    };
    let ro = 1600.0;
    let tau = std::f32::consts::TAU;
    for _ in 0..150 {
        let a = rnd() * tau;
        let da = 0.004 + rnd() * 0.012;
        let ri = r0 + rnd() * 60.0;
        let pts = [
            (cx + libm::cosf(a) * ri, cy + libm::sinf(a) * ri),
            (cx + libm::cosf(a - da) * ro, cy + libm::sinf(a - da) * ro),
            (cx + libm::cosf(a + da) * ro, cy + libm::sinf(a + da) * ro),
        ];
        fill_poly(c, &pts, alpha(col, opacity));
    }
}

/// Screentone dots every `pitch` pixels.
fn tone(c: &mut Canvas, pitch: i32, col: Rgba) {
    for y in (0..c.h as i32).step_by(pitch as usize) {
        let off = if (y / pitch) % 2 == 0 { 0 } else { pitch / 2 };
        for x in (off..c.w as i32).step_by(pitch as usize) {
            c.put(x, y, col);
        }
    }
}

/// 45-degree hatching inside `(x, y, w, h)`.
fn hatch(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, pitch: i32, col: Rgba) {
    for yy in y..y + h {
        for xx in x..x + w {
            if (xx + yy).rem_euclid(pitch) == 0 {
                c.put(xx, yy, col);
            }
        }
    }
}

fn text(g: &Generator, s: &str, bank: usize) -> Canvas {
    let (w, h, px) = g.text_rgba(s, bank, 1);
    Canvas::from_rgba(w, h, px)
}

/// Grow every opaque pixel by `r` in `col` behind the image (thick outline).
fn thick_outline(c: &Canvas, r: i32, col: Rgba) -> Canvas {
    let mut out = Canvas::new(c.w + 2 * r as u32, c.h + 2 * r as u32);
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            if c.get(x, y)[3] > 0 {
                out.rect(x, y, 2 * r + 1, 2 * r + 1, col);
            }
        }
    }
    out.blit(c, r, r);
    out
}

// ---------------------------------------------------------------- katakana

/// 9x9 katakana for the SFX and the logo accent (the pixel TTFs have none).
fn kana_rows(c: char) -> Option<[&'static str; 9]> {
    Some(match c {
        'ル' => [
            "..X...X..",
            "..X...X..",
            "..X...X..",
            "..X...X..",
            "..X...X..",
            ".X....X.X",
            ".X....XX.",
            "X.....X..",
            ".........",
        ],
        'ゲ' => [
            "..X...X.X",
            ".X....X.X",
            ".XXXXXXX.",
            "X....X...",
            ".....X...",
            "....X....",
            "...X.....",
            "..X......",
            ".........",
        ],
        'ン' => [
            ".........",
            ".X.......",
            "..X.....X",
            "........X",
            ".......X.",
            "......X..",
            "....XX...",
            "XXXX.....",
            ".........",
        ],
        'ド' => [
            "..X...X.X",
            "..X...X.X",
            "..X......",
            "..XX.....",
            "..X.XX...",
            "..X......",
            "..X......",
            "..X......",
            ".........",
        ],
        'ゴ' => [
            "......X.X",
            "......X.X",
            "XXXXXX...",
            ".....X...",
            ".....X...",
            ".....X...",
            "XXXXXX...",
            ".........",
            ".........",
        ],
        _ => return None,
    })
}

/// A katakana word, `k` pixels per bitmap pixel, one blank column apart.
fn kana(word: &str, k: i32, col: Rgba) -> Canvas {
    let n = word.chars().count() as i32;
    let mut c = Canvas::new((n * 10 * k) as u32, (9 * k) as u32);
    for (i, ch) in word.chars().enumerate() {
        let Some(rows) = kana_rows(ch) else { continue };
        for (y, row) in rows.iter().enumerate() {
            for (x, p) in row.chars().enumerate() {
                if p == 'X' {
                    c.rect((i as i32 * 10 + x as i32) * k, y as i32 * k, k, k, col);
                }
            }
        }
    }
    c
}

// ---------------------------------------------------------------- pieces (half resolution)

/// Skewed slab: parallelogram leaning right (top edge shifted by `skew`).
fn slab(w: i32, h: i32, skew: i32, fill: Rgba, mark: Option<&Canvas>) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let pts = [
        (skew as f32, 0.0),
        (w as f32, 0.0),
        ((w - skew) as f32, h as f32),
        (0.0, h as f32),
    ];
    fill_poly(&mut c, &pts, fill);
    if let Some(m) = mark {
        c.blit(m, w - skew - m.w as i32 - 14, (h - m.h as i32) / 2);
    }
    c
}

/// A white manga panel `w x h` with polygon `pts` (panel space), `t`-px ink
/// border and a hard shadow offset by `sh` in `shadow`. Returns the back
/// (shadow, fill, decoration from `deco`) and the border-only frame, both
/// `(w + sh) x (h + sh)`.
#[allow(clippy::too_many_arguments)]
fn panel(
    w: u32,
    h: u32,
    sh: i32,
    pts: &[(f32, f32)],
    fill: Rgba,
    shadow: Rgba,
    t: i32,
    deco: &dyn Fn(&mut Canvas),
) -> (Canvas, Canvas) {
    let (cw, ch) = (w + sh as u32, h + sh as u32);
    let mask = poly_mask(cw, ch, pts);
    let shifted: Vec<(f32, f32)> = pts
        .iter()
        .map(|(x, y)| (x + sh as f32, y + sh as f32))
        .collect();
    let mut back = Canvas::new(cw, ch);
    fill_poly(&mut back, &shifted, shadow);
    let mut inner = Canvas::new(cw, ch);
    fill_poly(&mut inner, pts, fill);
    deco(&mut inner);
    apply_mask(&mut inner, &mask);
    back.blit(&inner, 0, 0);
    let mut frame = Canvas::new(cw, ch);
    paint_mask(&mut frame, &border_of(&mask, cw, ch, t), INK);
    back.blit(&frame, 0, 0);
    (back, frame)
}

fn rect_pts(w: f32, h: f32) -> [(f32, f32); 4] {
    [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
}

/// Nine-slice tile: `fill` with a `t`-px border and a thicker bottom `tb`.
fn tile(n: i32, fill: Rgba, edge: Rgba, t: i32, tb: i32) -> Canvas {
    let mut c = Canvas::new(n as u32, n as u32);
    c.rect(0, 0, n, n, edge);
    c.rect(t, t, n - 2 * t, n - t - tb, fill);
    c
}

/// Spiky burst `w x h` with `n` points, filled and stroked.
fn burst(w: u32, h: u32, n: usize, inner: f32, fill: Rgba, stroke: Rgba, t: i32) -> Canvas {
    let mut pts = Vec::new();
    let mut s: u64 = 3;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    for i in 0..n * 2 {
        s = (s * 48271) % 2_147_483_647;
        let a = i as f32 / (n * 2) as f32 * std::f32::consts::TAU;
        let k = if i % 2 == 1 {
            inner + (s % 100) as f32 / 1000.0
        } else {
            1.0 - (s % 60) as f32 / 1000.0
        };
        pts.push((
            cx + libm::cosf(a) * (cx - 2.0) * k,
            cy + libm::sinf(a) * (cy - 2.0) * k,
        ));
    }
    let mask = poly_mask(w, h, &pts);
    let mut c = Canvas::new(w, h);
    paint_mask(&mut c, &mask, fill);
    paint_mask(&mut c, &border_of(&mask, w, h, t), stroke);
    c
}

/// Speech bubble: ellipse with an ink rim and a tail at `(tx, tip)`.
fn bubble(w: u32, h: u32, tail: (f32, f32, f32)) -> Canvas {
    let body_h = h as f32 - 16.0;
    let (cx, cy, rx, ry) = (
        w as f32 / 2.0,
        body_h / 2.0,
        w as f32 / 2.0 - 1.0,
        body_h / 2.0 - 1.0,
    );
    let mut pts = Vec::new();
    for i in 0..64 {
        let a = i as f32 / 64.0 * std::f32::consts::TAU;
        pts.push((cx + libm::cosf(a) * rx, cy + libm::sinf(a) * ry));
    }
    let mut mask = poly_mask(w, h, &pts);
    let (tx, tw, tip) = tail;
    let tail_mask = poly_mask(
        w,
        h,
        &[
            (tx, cy + ry - 6.0),
            (tx + tw, cy + ry - 6.0),
            (tx - 2.0, tip),
        ],
    );
    for (m, t) in mask.iter_mut().zip(tail_mask) {
        *m |= t;
    }
    let mut c = Canvas::new(w, h);
    paint_mask(&mut c, &mask, WHITE);
    paint_mask(&mut c, &border_of(&mask, w, h, 2), INK);
    c
}

fn paper_bg(w: u32, h: u32) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.rect(0, 0, w as i32, h as i32, PAPER);
    tone(&mut c, 4, [17, 17, 17, 22]);
    c
}

// ---------------------------------------------------------------- assets

/// Draw every piece into `ui/`.
pub fn assets(ui: &Path, ttf: &[u8]) -> anyhow::Result<()> {
    let style = |size| fontgen::Style {
        size,
        italic: 0,
        condense: 1.0,
        rim: false,
        highlight: false,
    };
    let small = &Generator::new(ttf, style(8))?;
    let big = &Generator::new(ttf, style(24))?;
    let logo_gen = &Generator::new(ttf, style(48))?;
    let win_gen = &Generator::new(ttf, style(40))?;
    let w2 = |name: &str, c: &Canvas| pixelize::write_webp(&ui.join(name), &x2(c));

    // ---- HOME
    let mut bg = paper_bg(640, 360);
    speed_fan(&mut bg, 505.0, 125.0, 100.0, 0.16, INK, 7);
    w2("home_bg.webp", &bg)?;

    let word = text(logo_gen, "RUGEN", fontgen::BANK_INK);
    let red = text(logo_gen, "RUGEN", fontgen::BANK_RED);
    let white = text(logo_gen, "RUGEN", fontgen::BANK_PAPER);
    let (lw, lh) = (word.w as i32 + 8, word.h as i32 + 26);
    let mut logo = Canvas::new(lw as u32 + 40, lh as u32 + 14);
    let (ox, oy) = (2, 16);
    logo.blit(&white, ox - 1, oy - 1);
    logo.blit(&red, ox + 3, oy + 3);
    logo.blit(&word, ox, oy);
    // brush underline under the word
    let (uy, uw) = (oy + word.h as i32 + 4, word.w as f32);
    fill_poly(
        &mut logo,
        &[
            (ox as f32 + 3.0, uy as f32 + 2.0),
            (ox as f32 + uw, uy as f32),
            (ox as f32 + uw * 0.96, uy as f32 + 6.0),
            (ox as f32 + 3.0 + uw * 0.02, uy as f32 + 5.0),
        ],
        INK,
    );
    let accent = kana("ルゲン", 2, RED);
    logo.blit(&accent, (word.w as i32 * 80 / 100).min(lw), 2);
    w2("logo_home.webp", &logo)?;

    w2("stamp9.webp", &tile(8, PAPER, RED, 2, 2))?;
    w2("key9.webp", &tile(8, WHITE, INK, 1, 3))?;
    w2("key9_dark.webp", &tile(8, INK, WHITE, 1, 3))?;

    let bang = text(small, "!!", fontgen::BANK_PAPER);
    w2("slab_home_lit.webp", &slab(250, 29, 9, RED, Some(&bang)))?;
    w2("slab_home_shadow.webp", &slab(250, 29, 9, RED_DARK, None))?;
    w2("slab_home.webp", &slab(220, 22, 9, INK, None))?;
    w2("slab_exit.webp", &slab(100, 18, 9, PAPER2, None))?;
    w2("slab_exit_lit.webp", &slab(120, 21, 9, RED, None))?;
    w2("slab_red_wide.webp", &slab(220, 18, 9, RED, None))?;

    // featured panel: white, slanted left edge, speed lines, hatch, ink shadow
    let fpts = [(0.0, 0.0), (320.0, 0.0), (320.0, 200.0), (20.0, 200.0)];
    let (fback, fframe) = panel(320, 200, 4, &fpts, WHITE, INK, 3, &|c: &mut Canvas| {
        speed_fan(c, 235.0, 125.0, 70.0, 0.5, INK, 11);
        hatch(c, 200, 0, 120, 200, 4, alpha(INK, 0.12));
    });
    w2("feat_back.webp", &fback)?;
    w2("feat_frame.webp", &fframe)?;
    // "NEW FIGHTER!" bubble with its tail
    let mut nb = bubble(100, 66, (32.0, 9.0, 66.0));
    let l1 = text(small, "NEW", fontgen::BANK_INK);
    let l2 = text(small, "FIGHTER!", fontgen::BANK_INK);
    nb.blit(&l1, (100 - l1.w as i32) / 2, 14);
    nb.blit(&l2, (100 - l2.w as i32) / 2, 27);
    w2("bubble_new.webp", &nb)?;

    // small home panels
    let (arenas, _) = panel(
        145,
        75,
        4,
        &rect_pts(145.0, 75.0),
        WHITE,
        INK,
        3,
        &|c: &mut Canvas| {
            hatch(c, 0, 0, 145, 75, 4, alpha(INK, 0.15));
        },
    );
    let mut arenas = arenas;
    let st = text(small, "ARENAS", fontgen::BANK_RED);
    let mut stamp = Canvas::new(st.w + 8, st.h + 6);
    stamp.rect(0, 0, stamp.w as i32, stamp.h as i32, RED);
    stamp.rect(2, 2, stamp.w as i32 - 4, stamp.h as i32 - 4, PAPER);
    stamp.blit(&st, 4, 3);
    arenas.blit(&stamp, 7, 7);
    w2("card_arenas.webp", &arenas)?;
    let (ranked, _) = panel(
        145,
        75,
        4,
        &rect_pts(145.0, 75.0),
        INK,
        RED,
        1,
        &|c: &mut Canvas| {
            speed_fan(c, 145.0, 0.0, 45.0, 0.25, WHITE, 5);
        },
    );
    w2("card_ranked.webp", &ranked)?;

    // ---- SELECT
    w2("sel_bg.webp", &paper_bg(640, 360))?;
    let mut cell = Canvas::new(32, 32);
    cell.rect(0, 0, 32, 32, INK);
    cell.rect(2, 2, 28, 28, WHITE);
    w2("cell_char.webp", &cell)?;
    w2("cell_broken.webp", &cell)?;
    let mut rnd = Canvas::new(32, 32);
    rnd.rect(0, 0, 32, 32, INK);
    let q = text(big, "?", fontgen::BANK_PAPER);
    rnd.blit(&q, (32 - q.w as i32) / 2, (32 - q.h as i32) / 2 + 1);
    w2("cell_random.webp", &rnd)?;
    let mut empty = Canvas::new(32, 32);
    hatch(&mut empty, 0, 0, 32, 32, 4, alpha(INK, 0.13));
    for i in (0..32).step_by(4) {
        for (x, y) in [(i, 0), (i, 31), (0, i), (31, i)] {
            empty.rect(x, y, 2, 1, alpha(INK, 0.3));
        }
    }
    w2("cell_empty.webp", &empty)?;
    let mut warn = Canvas::new(13, 13);
    fill_poly(&mut warn, &[(0.0, 0.0), (13.0, 0.0), (13.0, 13.0)], RED);
    warn.rect(9, 1, 1, 4, WHITE);
    warn.rect(9, 6, 1, 1, WHITE);
    w2("sel_warn.webp", &warn)?;
    let mut bad = Canvas::new(32, 32);
    for i in 4..28 {
        bad.rect(i - 1, i - 1, 3, 3, RED);
        bad.rect(31 - i - 1, i - 1, 3, 3, RED);
    }
    w2("sel_x.webp", &bad)?;
    // cursors: 1P red frame with a tab above, 2P ink frame with a tab below
    let tab1 = text(small, "1P", fontgen::BANK_PAPER);
    let mut c1 = Canvas::new(38, 50);
    c1.rect(0, 12, 38, 38, RED);
    c1.rect(3, 15, 32, 32, [0, 0, 0, 0]);
    for y in 15..47 {
        for x in 3..35 {
            c1.put_raw(x, y, [0, 0, 0, 0]);
        }
    }
    c1.rect(0, 0, tab1.w as i32 + 6, 13, RED);
    c1.blit(&tab1, 3, 1);
    w2("sel_cur1.webp", &c1)?;
    let tab2 = text(small, "2P", fontgen::BANK_PAPER);
    let mut c2 = Canvas::new(38, 50);
    c2.rect(0, 0, 38, 38, INK);
    for y in 3..35 {
        for x in 3..35 {
            c2.put_raw(x, y, [0, 0, 0, 0]);
        }
    }
    c2.rect(0, 37, tab2.w as i32 + 6, 13, INK);
    c2.blit(&tab2, 3, 38);
    w2("sel_cur2.webp", &c2)?;
    // player cards: slanted outer edge, speed lines, hard shadows
    let p1pts = [(0.0, 0.0), (200.0, 0.0), (180.0, 110.0), (0.0, 110.0)];
    let (b1, f1) = panel(200, 110, 4, &p1pts, WHITE, RED, 3, &|c: &mut Canvas| {
        speed_fan(c, 50.0, 80.0, 40.0, 0.3, INK, 13);
    });
    w2("card1_back.webp", &b1)?;
    w2("card1_frame.webp", &f1)?;
    let p2pts = [(20.0, 0.0), (200.0, 0.0), (200.0, 110.0), (0.0, 110.0)];
    let (b2, f2) = panel(200, 110, 4, &p2pts, WHITE, INK, 3, &|c: &mut Canvas| {
        speed_fan(c, 150.0, 80.0, 40.0, 0.3, INK, 17);
    });
    w2("card2_back.webp", &b2)?;
    w2("card2_frame.webp", &f2)?;
    let (sp, _) = panel(180, 75, 3, &rect_pts(180.0, 75.0), WHITE, INK, 3, &|_| {});
    w2("stage_panel.webp", &sp)?;
    w2("slab_card1.webp", &slab(115, 20, 9, RED, None))?;
    w2("slab_card2.webp", &slab(115, 20, 9, INK, None).flip_h())?;

    // ---- VERSUS
    let mut vs = Canvas::new(640, 360);
    vs.rect(0, 0, 640, 360, INK);
    let red_half = [(0.0, 0.0), (350.0, 0.0), (290.0, 360.0), (0.0, 360.0)];
    let mut left = Canvas::new(640, 360);
    fill_poly(&mut left, &red_half, RED);
    speed_fan(&mut left, 150.0, 180.0, 0.0, 0.35, INK, 19);
    tone(&mut left, 3, [0, 0, 0, 58]);
    apply_mask(&mut left, &poly_mask(640, 360, &red_half));
    let mut right = Canvas::new(640, 360);
    speed_fan(&mut right, 490.0, 180.0, 0.0, 0.25, WHITE, 23);
    let not_red: Vec<bool> = poly_mask(640, 360, &red_half).iter().map(|m| !m).collect();
    apply_mask(&mut right, &not_red);
    vs.blit(&right, 0, 0);
    vs.blit(&left, 0, 0);
    w2("vs_bg.webp", &vs)?;
    let mut line = Canvas::new(640, 360);
    let seg = |c: &mut Canvas, t: f32, col: Rgba| {
        fill_poly(
            c,
            &[
                (356.0 - t, 0.0),
                (356.0 + t, 0.0),
                (296.0 + t, 360.0),
                (296.0 - t, 360.0),
            ],
            col,
        );
    };
    seg(&mut line, 4.0, WHITE);
    seg(&mut line, 1.5, INK);
    w2("vs_line.webp", &line)?;
    let mut vb = burst(140, 110, 14, 0.68, RED, INK, 3);
    let vtxt = text(logo_gen, "VS", fontgen::BANK_PAPER);
    let vred = text(logo_gen, "VS", fontgen::BANK_RED);
    let mut v = Canvas::new(vtxt.w + 3, vtxt.h + 3);
    v.blit(&vred, 3, 3);
    v.blit(&vtxt, 0, 0);
    let v = thick_outline(&v, 2, INK);
    vb.blit(&v, (140 - v.w as i32) / 2, (110 - v.h as i32) / 2);
    w2("vs_burst.webp", &vb)?;
    w2(
        "sfx_dododo.webp",
        &thick_outline(&kana("ドドド", 3, WHITE), 1, INK),
    )?;
    w2("sfx_gogogo.webp", &kana("ゴゴゴ", 3, RED))?;

    // ---- PAUSE
    let mut dark = Canvas::new(640, 360);
    tone(&mut dark, 3, [0, 0, 0, 58]);
    w2("tone_dark.webp", &dark)?;
    let (pc, _) = panel(220, 280, 4, &rect_pts(220.0, 280.0), WHITE, RED, 3, &|_| {});
    w2("pause_card.webp", &pc)?;
    let (mp, _) = panel(
        330,
        270,
        4,
        &rect_pts(330.0, 270.0),
        PAPER,
        INK,
        3,
        &|c: &mut Canvas| {
            tone(c, 4, [17, 17, 17, 22]);
        },
    );
    w2("moves_panel.webp", &mp)?;
    w2("slab_pause_lit.webp", &slab(190, 26, 9, RED, None))?;
    w2("slab_pause.webp", &slab(170, 22, 9, INK, None))?;
    w2("slab_pause_paper.webp", &slab(170, 22, 9, PAPER2, None))?;

    // ---- RESULT
    let mut rb = paper_bg(640, 360);
    speed_fan(&mut rb, 160.0, 190.0, 30.0, 0.45, INK, 29);
    let rpts = [(0.0, 0.0), (360.0, 0.0), (295.0, 360.0), (0.0, 360.0)];
    let mut rh = Canvas::new(640, 360);
    fill_poly(&mut rh, &rpts, alpha(RED, 0.95));
    speed_fan(&mut rh, 160.0, 190.0, 30.0, 0.3, WHITE, 31);
    apply_mask(&mut rh, &poly_mask(640, 360, &rpts));
    rb.blit(&rh, 0, 0);
    w2("result_bg.webp", &rb)?;
    let win = text(win_gen, "WIN!!", fontgen::BANK_PAPER);
    let wink = text(win_gen, "WIN!!", fontgen::BANK_INK);
    let mut wc = Canvas::new(win.w + 3, win.h + 3);
    wc.blit(&wink, 3, 3);
    wc.blit(&win, 0, 0);
    w2("win.webp", &wc)?;
    w2("bubble_quote.webp", &bubble(150, 80, (40.0, 14.0, 80.0)))?;
    let (rp, _) = panel(215, 200, 4, &rect_pts(215.0, 200.0), WHITE, INK, 3, &|_| {});
    w2("report_panel.webp", &rp)?;
    let mut dots = Canvas::new(191, 2);
    for x in (0..191).step_by(3) {
        dots.rect(x, 0, 2, 2, INK);
    }
    w2("dots.webp", &dots)?;
    w2("slab_result_lit.webp", &slab(215, 26, 9, RED, None))?;
    w2("slab_result.webp", &slab(195, 22, 9, INK, None))?;
    Ok(())
}

/// Write the screen TOMLs into `ui/`.
pub fn screens(ui: &Path) -> anyhow::Result<()> {
    for (name, text) in [
        ("title.toml", TITLE_TOML),
        ("select.toml", SELECT_TOML),
        ("vs.toml", VS_TOML),
        ("pause.toml", PAUSE_TOML),
        ("result.toml", RESULT_TOML),
        ("online.toml", ONLINE_TOML),
        ("stages.toml", STAGES_TOML),
        ("challenge.toml", CHALLENGE_TOML),
        ("practice.toml", PRACTICE_TOML),
        ("debug.toml", DEBUG_TOML),
    ] {
        std::fs::write(ui.join(name), text)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- screens

const TITLE_TOML: &str = include_str!("inkscreens/title.toml");
const SELECT_TOML: &str = include_str!("inkscreens/select.toml");
const VS_TOML: &str = include_str!("inkscreens/vs.toml");
const PAUSE_TOML: &str = include_str!("inkscreens/pause.toml");
const RESULT_TOML: &str = include_str!("inkscreens/result.toml");
const ONLINE_TOML: &str = include_str!("inkscreens/online.toml");
const STAGES_TOML: &str = include_str!("inkscreens/stages.toml");
const CHALLENGE_TOML: &str = include_str!("inkscreens/challenge.toml");
const PRACTICE_TOML: &str = include_str!("inkscreens/practice.toml");
const DEBUG_TOML: &str = include_str!("inkscreens/debug.toml");
