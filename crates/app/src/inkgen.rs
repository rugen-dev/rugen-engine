//! `rugen ui-gen --theme ink`: the Shonen Ink theme. Paper, screentone,
//! black ink panels with hard offset shadows, red stamps and "!!" marks,
//! skewed slabs, speed-line fans — all pixel art drawn here, plus a matching
//! `system.def` / `fight.def` screenpack and the fonts.
//!
//! Everything lands in `motif/themes/ink/`; files missing there fall back to
//! the flat `motif/`.

use std::path::Path;

use crate::fontgen::{self, Generator, Style};
use crate::kit;
use crate::pixelize;
use crate::screenpack::{Canvas, to_sprite};
use rugen_formats::sff::{Sff, Version};

type Rgba = [u8; 4];

pub const PAPER: Rgba = [243, 239, 228, 255];
pub const PAPER_DARK: Rgba = [214, 208, 194, 255];
pub const INK: Rgba = [17, 17, 17, 255];
pub const INK_SOFT: Rgba = [60, 58, 54, 255];
pub const RED: Rgba = [224, 32, 32, 255];
pub const WHITE: Rgba = [255, 255, 255, 255];

// ---------------------------------------------------------------- helpers

/// Nearest-neighbour 2x upscale so a half-resolution drawing stays crisp at 1280x720.
fn x2(c: &Canvas) -> Canvas {
    let mut out = Canvas::new(c.w * 2, c.h * 2);
    for y in 0..c.h as i32 {
        for x in 0..c.w as i32 {
            let p = c.get(x, y);
            if p[3] == 0 {
                continue;
            }
            out.rect(x * 2, y * 2, 2, 2, p);
        }
    }
    out
}

/// Rotate by `deg` (counter-clockwise, nearest neighbour), growing the canvas to fit.
fn rotate(c: &Canvas, deg: f32) -> Canvas {
    let rad = deg * std::f32::consts::PI / 180.0;
    let (s, co) = (libm::sinf(rad), libm::cosf(rad));
    let (w, h) = (c.w as f32, c.h as f32);
    let nw = (w * co.abs() + h * s.abs()) as u32 + 2;
    let nh = (w * s.abs() + h * co.abs()) as u32 + 2;
    let mut out = Canvas::new(nw, nh);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ncx, ncy) = (nw as f32 / 2.0, nh as f32 / 2.0);
    for y in 0..nh as i32 {
        for x in 0..nw as i32 {
            let dx = x as f32 + 0.5 - ncx;
            let dy = y as f32 + 0.5 - ncy;
            // inverse rotation
            let sx = dx * co - dy * s + cx;
            let sy = dx * s + dy * co + cy;
            let p = c.get(libm::floorf(sx) as i32, libm::floorf(sy) as i32);
            if p[3] > 0 {
                out.put(x, y, p);
            }
        }
    }
    out
}

fn text(g: &Generator, s: &str, bank: usize) -> Canvas {
    let (w, h, px) = g.text_rgba(s, bank, 1);
    Canvas::from_rgba(w, h, px)
}

/// Ink text with a hard red offset shadow (the logo treatment).
fn shadowed(g: &Generator, s: &str, off: i32) -> Canvas {
    let ink = text(g, s, fontgen::BANK_INK);
    let red = text(g, s, fontgen::BANK_RED);
    let mut out = Canvas::new(ink.w + off as u32, ink.h + off as u32);
    out.blit(&red, off, off);
    out.blit(&ink, 0, 0);
    out
}

fn border(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, t: i32, col: Rgba) {
    c.rect(x, y, w, t, col);
    c.rect(x, y + h - t, w, t, col);
    c.rect(x, y, t, h, col);
    c.rect(x + w - t, y, t, h, col);
}

/// Halftone screentone: a dot every `pitch` pixels, denser as `density` rises.
#[allow(clippy::too_many_arguments)]
fn screentone(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, pitch: i32, col: Rgba, density: f32) {
    for yy in (0..h).step_by(pitch as usize) {
        let row = yy / pitch;
        for xx in (0..w).step_by(pitch as usize) {
            let colm = xx / pitch;
            let on = if density >= 0.99 {
                true
            } else if density >= 0.5 {
                (row + colm) % 2 == 0 || density > 0.74
            } else {
                (row % 2 == 0) && (colm % 2 == 0)
            };
            if on {
                c.put(x + xx, y + yy, col);
            }
        }
    }
}

// ---------------------------------------------------------------- pieces (half resolution)

/// The paper backdrop (640x360): cream, screentone, a soft vignette of darker tone at the edges.
pub fn paper(w: u32, h: u32) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.rect(0, 0, w as i32, h as i32, PAPER);
    screentone(
        &mut c,
        0,
        0,
        w as i32,
        h as i32,
        6,
        [228, 223, 210, 255],
        0.4,
    );
    // heavier tone band at the bottom like a manga gutter
    screentone(&mut c, 0, h as i32 - 40, w as i32, 40, 4, PAPER_DARK, 0.5);
    c.noise(0.004, [225, 220, 205, 255], 7);
    c
}

/// Speed-line fan converging on the right edge (transparent elsewhere).
pub fn speedlines(w: u32, h: u32, lines: i32, alpha: u8) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (ox, oy) = (w as f32 + 30.0, h as f32 * 0.5);
    for i in 0..lines {
        let a = -1.25 + 2.5 * (i as f32 + 0.5) / lines as f32;
        let (dx, dy) = (libm::cosf(a), libm::sinf(a));
        let len = w as f32 * 0.62;
        let thick = if i % 3 == 0 { 2 } else { 1 };
        let mut t = 70.0;
        while t < len {
            let x = ox - dx * t;
            let y = oy - dy * t;
            let fade = ((t - 70.0) / (len - 70.0)).clamp(0.0, 1.0);
            let a8 = (alpha as f32 * (1.0 - fade * 0.8)) as u8;
            c.rect(
                x as i32,
                y as i32,
                thick,
                thick,
                [INK[0], INK[1], INK[2], a8],
            );
            t += 1.0;
        }
    }
    c
}

/// Hatch tone block (manga panel background).
fn hatch(w: u32, h: u32) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.rect(0, 0, w as i32, h as i32, WHITE);
    for d in (0..(w + h) as i32).step_by(5) {
        for y in 0..h as i32 {
            let x = d - y;
            if x >= 0 && x < w as i32 {
                c.put(x, y, [200, 196, 186, 255]);
            }
        }
    }
    c
}

/// 9-slice tile (40x40 half res): `fill` box with a 2-px `edge` border and a
/// 6-px hard shadow at the right/bottom. Use `nine = [12, 12, 24, 24]` on the 2x file.
pub fn panel_tile(fill: Rgba, edge: Rgba, shadow: Rgba) -> Canvas {
    let mut c = Canvas::new(40, 40);
    c.rect(6, 6, 34, 34, shadow);
    c.rect(0, 0, 34, 34, fill);
    border(&mut c, 0, 0, 34, 34, 2, edge);
    c
}

/// Skewed slab (menu button): `w x h` half res, fill, optional "!!" mark at the right.
pub fn slab(w: u32, h: u32, fill: Rgba, bang: Option<&Generator>) -> Canvas {
    let skew = (h / 3) as i32;
    let mut c = Canvas::new(w + skew as u32, h);
    c.para_grad(0, 0, w as i32, h as i32, skew, fill, fill);
    // thin lighter line along the top to read as a cut edge
    let hi = if fill == INK {
        INK_SOFT
    } else {
        [255, 120, 120, 255]
    };
    c.rect(skew, 0, w as i32, 1, hi);
    if let Some(g) = bang {
        let t = text(g, "!!", fontgen::BANK_PAPER);
        c.blit(&t, w as i32 - t.w as i32 - 4, (h as i32 - t.h as i32) / 2);
    }
    c
}

/// Flat slab without skew for rows and plates.
pub fn plate(w: u32, h: u32, fill: Rgba, edge: Rgba) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.rect(0, 0, w as i32, h as i32, fill);
    border(&mut c, 0, 0, w as i32, h as i32, 1, edge);
    c
}

/// Roster cell (38x38): white box with an ink border; `p1`/`p2` are frames only.
pub fn cell(kind: &str) -> Canvas {
    let mut c = Canvas::new(38, 38);
    match kind {
        "p1" => border(&mut c, 0, 0, 38, 38, 3, RED),
        "p2" => {
            border(&mut c, 0, 0, 38, 38, 3, INK);
            border(&mut c, 3, 3, 32, 32, 1, WHITE);
        }
        "lit" => {
            c.rect(0, 0, 38, 38, WHITE);
            border(&mut c, 0, 0, 38, 38, 2, RED);
            border(&mut c, 2, 2, 34, 34, 1, INK);
        }
        _ => {
            c.rect(0, 0, 38, 38, WHITE);
            border(&mut c, 0, 0, 38, 38, 2, INK);
        }
    }
    c
}

/// Red stamp: text inside a 2-px red frame, tilted.
pub fn stamp(g: &Generator, s: &str) -> Canvas {
    let t = text(g, s, fontgen::BANK_RED);
    let (cw, ch) = (t.w + 10, t.h + 8);
    let mut c = Canvas::new(cw, ch);
    border(&mut c, 0, 0, cw as i32, ch as i32, 2, RED);
    c.blit(&t, 5, 4);
    rotate(&c, 4.0)
}

/// The RUGEN wordmark: ink letters, red offset shadow, tilted like a manga title.
pub fn logo(g: &Generator) -> Canvas {
    let word = shadowed(g, "RUGEN", 3);
    let mut c = Canvas::new(word.w + 6, word.h + 10);
    c.blit(&word, 3, 2);
    // ink underline stroke that thins to the right
    for x in 0..word.w as i32 {
        let t = 3 - (x * 3) / word.w.max(1) as i32;
        c.rect(3 + x, word.h as i32 + 4, 1, t.max(1), INK);
    }
    rotate(&c, 4.0)
}

/// Manga panel for the news slots: hatched tone, a big caption and a red stamp.
pub fn news(
    g: &Generator,
    small: &Generator,
    w: u32,
    h: u32,
    caption: &str,
    stamp_text: &str,
) -> Canvas {
    let mut c = hatch(w, h);
    // ink burst rays from the bottom-left
    for i in 0..14 {
        let a = -0.1 - 1.4 * i as f32 / 14.0;
        let (dx, dy) = (libm::cosf(a), libm::sinf(a));
        let mut t = 10.0;
        while t < w as f32 * 1.3 {
            let x = dx * t;
            let y = h as f32 - 1.0 + dy * t;
            if x < 0.0 || y < 0.0 {
                break;
            }
            c.put(x as i32, y as i32, INK_SOFT);
            t += 1.0;
        }
    }
    let cap = shadowed(g, caption, 2);
    let small_panel = h < 80;
    let (cx, cy) = if small_panel {
        (w as i32 - cap.w as i32 - 8, h as i32 - cap.h as i32 - 6)
    } else {
        ((w as i32 - cap.w as i32) / 2, (h as i32 - cap.h as i32) / 2)
    };
    // paper plate behind the caption
    c.rect(cx - 4, cy - 2, cap.w as i32 + 8, cap.h as i32 + 4, PAPER);
    border(
        &mut c,
        cx - 4,
        cy - 2,
        cap.w as i32 + 8,
        cap.h as i32 + 4,
        1,
        INK,
    );
    c.blit(&cap, cx, cy);
    let st = stamp(small, stamp_text);
    c.blit(&st, 3, 3);
    border(&mut c, 0, 0, w as i32, h as i32, 2, INK);
    c
}

/// Avatar (28x28): paper square, ink border, a simple ink face.
pub fn avatar() -> Canvas {
    let mut c = Canvas::new(28, 28);
    c.rect(0, 0, 28, 28, WHITE);
    border(&mut c, 0, 0, 28, 28, 2, INK);
    // hair spikes
    for (x, h) in [(5, 7), (9, 10), (13, 8), (17, 11), (21, 7)] {
        c.rect(x, 12 - h, 3, h, INK);
    }
    c.rect(6, 10, 16, 12, [250, 220, 190, 255]);
    c.rect(9, 14, 2, 2, INK);
    c.rect(17, 14, 2, 2, INK);
    c.rect(11, 19, 6, 1, INK);
    c.rect(4, 22, 20, 4, RED);
    c
}

/// Diamond badge (verification mark) 8x8.
pub fn badge(col: Rgba) -> Canvas {
    let mut c = kit::badge(col);
    c.outline(INK);
    c
}

// ---------------------------------------------------------------- fight HUD pieces

fn bar_plate(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, WHITE);
    border(&mut c, 0, 0, w, h, 2, INK);
    c
}

fn bar_fill(w: i32, h: i32, col: Rgba) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    c.rect(0, 0, w, h, col);
    // segment ticks every 16 px
    for x in (16..w).step_by(16) {
        c.rect(x, 0, 1, h, [col[0] / 2, col[1] / 2, col[2] / 2, 255]);
    }
    c
}

fn face_frame(size: i32, accent: Rgba) -> Canvas {
    let mut c = Canvas::new(size as u32, size as u32);
    c.rect(0, 0, size, size, WHITE);
    border(&mut c, 0, 0, size, size, 2, INK);
    c.rect(2, size - 5, size - 4, 3, accent);
    c
}

fn timer_slab() -> Canvas {
    let mut c = Canvas::new(60, 34);
    c.rect(4, 4, 52, 28, RED);
    c.rect(0, 0, 52, 28, INK);
    border(&mut c, 0, 0, 52, 28, 2, PAPER);
    c
}

fn win_mark(kind: u8) -> Canvas {
    let mut c = Canvas::new(11, 11);
    let fill = match kind {
        0 => RED,
        1 => [255, 150, 30, 255],
        2 => [255, 225, 70, 255],
        3 => [90, 160, 255, 255],
        _ => INK,
    };
    c.rect(1, 1, 9, 9, fill);
    border(&mut c, 0, 0, 11, 11, 1, INK);
    c
}

fn empty_mark() -> Canvas {
    let mut c = Canvas::new(11, 11);
    c.rect(0, 0, 11, 11, WHITE);
    border(&mut c, 0, 0, 11, 11, 1, INK);
    c
}

// ---------------------------------------------------------------- screens (TOML)

// HOME / SELECT / VS / PAUSE / RESULT live in `inkscreens.rs`.

const README: &str = r#"# motif/themes/ink — Shonen Ink

Paper, screentone dots, black ink panels with hard offset shadows, red stamps
and "!!" marks, speed lines. Generated by `rugen ui-gen --theme ink`
(`crates/app/src/inkgen.rs`); edit the `.toml` screens or replace any WebP.

Select it in OPTIONS → UI THEME or with `[video] theme = "ink"` in rugen.toml.
Files missing here fall back to the flat `motif/`.

Font banks used on paper: 9 = ink, 10 = red, 11 = paper (for text on slabs),
8 = soft grey.
"#;

/// Re-skin a flat-motif screen for the ink theme (assets, banks, colours).
fn reskin(src: &str) -> String {
    let mut s = src.to_string();
    for (a, b) in [
        (
            "background = [0.03, 0.04, 0.08, 1.0]",
            "background = [0.95, 0.94, 0.89, 1.0]",
        ),
        (
            "background = [0.02, 0.02, 0.05, 1.0]",
            "background = [0.95, 0.94, 0.89, 1.0]",
        ),
        (
            "background = [0.02, 0.03, 0.07, 1.0]",
            "background = [0.95, 0.94, 0.89, 1.0]",
        ),
        ("image = \"ui/bg.webp\"", "image = \"ui/paper.webp\""),
        (
            "image = \"ui/panel.webp\"\nnine = [8, 8, 8, 8]",
            "image = \"ui/panel9.webp\"\nnine = [12, 12, 24, 24]",
        ),
        (
            "image = \"ui/panel.webp\"\n  nine = [8, 8, 8, 8]",
            "image = \"ui/panel9.webp\"\n  nine = [12, 12, 24, 24]",
        ),
        (
            "image = \"ui/art_frame.webp\"\nnine = [10, 10, 10, 10]",
            "image = \"ui/art9.webp\"\nnine = [12, 12, 28, 28]",
        ),
        (
            "image = \"ui/art_frame.webp\"\n  nine = [10, 10, 10, 10]",
            "image = \"ui/art9.webp\"\n  nine = [12, 12, 28, 28]",
        ),
        (
            "image = \"ui/card.webp\"\nnine = [10, 10, 10, 10]",
            "image = \"ui/panel9_black.webp\"\nnine = [12, 12, 24, 24]",
        ),
        (
            "image = \"ui/row.webp\"\n    nine = [6, 6, 6, 6]",
            "image = \"ui/row.webp\"\n    nine = [4, 4, 4, 4]",
        ),
        (
            "image = \"ui/button_lit.webp\"",
            "image = \"ui/slab_lit.webp\"",
        ),
        (
            "image_focus = \"ui/button_lit.webp\"",
            "image_focus = \"ui/slab_lit.webp\"",
        ),
        ("image = \"ui/button.webp\"", "image = \"ui/slab.webp\""),
        (
            "image = \"ui/plate_lit.webp\"",
            "image = \"ui/slab_lit.webp\"",
        ),
        (
            "image_focus = \"ui/plate_lit.webp\"",
            "image_focus = \"ui/plate_lit.webp\"",
        ),
        // VS halves: red ink and black ink
        (
            "color = [0.55, 0.12, 0.16, 0.85]",
            "color = [0.88, 0.13, 0.13, 0.92]",
        ),
        (
            "color = [0.12, 0.2, 0.55, 0.85]",
            "color = [0.07, 0.07, 0.07, 0.92]",
        ),
        (
            "color = [0.0, 0.0, 0.0, 0.75]",
            "color = [0.95, 0.94, 0.89, 0.85]",
        ),
        (
            "color = [0.0, 0.0, 0.0, 0.6]",
            "color = [0.95, 0.94, 0.89, 0.85]",
        ),
        (
            "color = [0.08, 0.1, 0.18, 1.0]",
            "color = [0.84, 0.82, 0.76, 1.0]",
        ),
        // particles: ink flecks and red sparks
        (
            "color = [1.0, 0.75, 0.3, 0.7]",
            "color = [0.07, 0.07, 0.07, 0.5]",
        ),
        (
            "color2 = [0.5, 0.7, 1.0, 0.5]",
            "color2 = [0.88, 0.13, 0.13, 0.6]",
        ),
        (
            "color = [1.0, 0.8, 0.3, 0.8]",
            "color = [0.88, 0.13, 0.13, 0.7]",
        ),
        (
            "color2 = [1.0, 0.3, 0.3, 0.5]",
            "color2 = [0.07, 0.07, 0.07, 0.5]",
        ),
    ] {
        s = s.replace(a, b);
    }
    // text on buttons/slabs is paper-coloured; everything else ink/red/grey
    s = s.replace("bank = 7\n  bank_focus = 4", "bank = 11\n  bank_focus = 11");
    s = s.replace(
        "bank = 7\n    bank_focus = 4",
        "bank = 11\n    bank_focus = 11",
    );
    s = s.replace(
        "bank = 7\n      bank_focus = 4",
        "bank = 11\n      bank_focus = 11",
    );
    s = s.replace("bank = 7\nbank_focus = 4", "bank = 11\nbank_focus = 11");
    s = s.replace("bank = 7", "bank = 9");
    s = s.replace("bank = 4", "bank = 10");
    s = s.replace("bank = 6", "bank = 8");
    s = s.replace("bank = 0", "bank = 10");
    s = s.replace("bank = 5", "bank = 9");
    s = s.replace("bank = 1\n", "bank = 10\n");
    s
}

/// Generate the ink theme into `out` (usually `motif/themes/ink`), taking
/// fonts and the flat screens to re-skin from `flat` (`motif/`).
pub fn generate(flat: &Path, out: &Path, fallback_ttf: Option<&[u8]>) -> anyhow::Result<()> {
    let ui = out.join("ui");
    std::fs::create_dir_all(&ui)?;
    fontgen::generate_roles_into(flat, &out.join("font"), fallback_ttf)?;
    let title_role = &fontgen::roles()[0];
    let ttf = fontgen::role_ttf(flat, title_role, fallback_ttf)?;
    let logo_gen = Generator::new(
        &ttf,
        Style {
            size: 40,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
    )?;
    let big = Generator::new(
        &ttf,
        Style {
            size: 24,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
    )?;
    let small = Generator::new(
        &ttf,
        Style {
            size: 8,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
    )?;
    let menu_role = &fontgen::roles()[1];
    let menu_ttf = fontgen::role_ttf(flat, menu_role, fallback_ttf)?;
    let menu_gen = Generator::new(&menu_ttf, menu_role.style)?;

    let w = |name: &str, c: &Canvas| pixelize::write_webp(&ui.join(name), c);
    let w2 = |name: &str, c: &Canvas| pixelize::write_webp(&ui.join(name), &x2(c));

    // backgrounds
    w2("paper.webp", &paper(640, 360))?;
    w2("lines.webp", &speedlines(640, 360, 46, 110))?;
    w2("lines_left.webp", &speedlines(640, 360, 30, 70).flip_h())?;
    w2("hatch.webp", &hatch(100, 56))?;
    // panels
    w2("panel9.webp", &panel_tile(WHITE, INK, INK))?;
    w2("panel9_red.webp", &panel_tile(WHITE, INK, RED))?;
    w2("panel9_black.webp", &panel_tile(INK, INK, RED))?;
    w2("art9.webp", &panel_tile(WHITE, INK, RED))?;
    // slabs and plates
    w2("slab.webp", &slab(200, 24, INK, None))?;
    w2("slab_lit.webp", &slab(200, 24, RED, Some(&menu_gen)))?;
    w2("slab_red.webp", &slab(130, 28, RED, None))?;
    w2("plate.webp", &plate(70, 16, WHITE, INK))?;
    w2("plate_lit.webp", &plate(70, 16, RED, INK))?;
    w2("row.webp", &plate(40, 16, WHITE, INK))?;
    w2("row_lit.webp", &plate(40, 16, RED, INK))?;
    // cells
    w2("cell.webp", &cell("normal"))?;
    w2("cell_lit.webp", &cell("lit"))?;
    w2("cell_p1.webp", &cell("p1"))?;
    w2("cell_p2.webp", &cell("p2"))?;
    w2("badge_ok.webp", &badge([60, 170, 80, 255]))?;
    w2("badge_warn.webp", &badge([235, 170, 30, 255]))?;
    w2("badge_bad.webp", &badge(RED))?;
    w2("avatar.webp", &avatar())?;
    // marks
    w("logo.webp", &logo(&logo_gen))?;
    w("stamp_chapter.webp", &stamp(&small, "CHAPTER 03"))?;
    w("bang.webp", &text(&big, "!!", fontgen::BANK_RED))?;
    w("vs.webp", &rotate(&shadowed(&logo_gen, "VS", 4), -6.0))?;
    // news placeholders
    w2(
        "news1.webp",
        &news(&big, &small, 332, 150, "RUGEN", "NEW CHAPTER"),
    )?;
    w2(
        "news2.webp",
        &news(&small, &small, 147, 48, "ARENAS", "STAGES"),
    )?;
    w2(
        "news3.webp",
        &news(&small, &small, 147, 48, "ONLINE", "RANKED"),
    )?;

    // screens: the book's HOME / SELECT / VS / PAUSE / RESULT, the rest re-skinned
    crate::inkscreens::assets(&ui, &ttf)?;
    crate::inkscreens::screens(&ui)?;
    for name in ["options.toml", "online.toml"] {
        let src = flat.join("ui").join(name);
        match std::fs::read_to_string(&src) {
            Ok(text) => std::fs::write(ui.join(name), reskin(&text))?,
            Err(e) => log::warn!("{}: {e} (screen not re-skinned)", src.display()),
        }
    }
    std::fs::write(out.join("README.md"), README)?;
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
    screenpack(out, &ttf, &logo_gen, &menu_gen)?;
    Ok(())
}

/// The MUGEN-side screenpack of the theme: `system.sff` / `fight.sff` and their defs.
fn screenpack(
    out: &Path,
    _ttf: &[u8],
    logo_gen: &Generator,
    menu_gen: &Generator,
) -> anyhow::Result<()> {
    let mut sys = Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    };
    let l = logo(logo_gen);
    to_sprite(&mut sys, (0, 0), &l, [(l.w / 2) as i16, 0]);
    let vs = rotate(&shadowed(logo_gen, "VS", 3), -6.0);
    to_sprite(
        &mut sys,
        (200, 0),
        &vs,
        [(vs.w / 2) as i16, (vs.h / 2) as i16],
    );
    let mut back = paper(426, 240);
    let lines = speedlines(426, 240, 36, 90);
    back.blit(&lines, 0, 0);
    to_sprite(&mut sys, (1, 0), &back, [213, 0]);
    to_sprite(&mut sys, (100, 0), &paper(426, 240), [213, 0]);
    let mut box_panel = plate(232, 100, WHITE, INK);
    border(&mut box_panel, 0, 0, 232, 100, 2, INK);
    to_sprite(&mut sys, (100, 1), &box_panel, [116, 0]);
    let mut face_plate = plate(108, 126, WHITE, INK);
    border(&mut face_plate, 0, 0, 108, 126, 2, INK);
    to_sprite(&mut sys, (120, 0), &face_plate, [0, 0]);
    to_sprite(&mut sys, (120, 1), &face_plate.flip_h(), [0, 0]);
    to_sprite(&mut sys, (121, 0), &slab(100, 14, RED, None), [0, 0]);
    to_sprite(&mut sys, (121, 1), &slab(100, 14, INK, None), [0, 0]);
    to_sprite(&mut sys, (150, 0), &cell("normal"), [1, 1]);
    to_sprite(&mut sys, (160, 0), &cell("p1"), [3, 3]);
    to_sprite(&mut sys, (161, 0), &cell("lit"), [3, 3]);
    to_sprite(&mut sys, (170, 0), &cell("p2"), [3, 3]);
    to_sprite(&mut sys, (171, 0), &cell("lit"), [3, 3]);
    let mut opt = plate(380, 180, WHITE, INK);
    border(&mut opt, 0, 0, 380, 180, 2, INK);
    to_sprite(&mut sys, (300, 0), &opt, [190, 0]);
    to_sprite(&mut sys, (310, 0), &slab(150, 14, INK, None), [75, 0]);
    to_sprite(
        &mut sys,
        (311, 0),
        &slab(150, 14, RED, Some(menu_gen)),
        [75, 0],
    );
    // VS halves: red ink and black ink with a cut edge
    let mut left = Canvas::new(253, 240);
    left.rect(0, 0, 253, 240, RED);
    left.cut_right(213, 60);
    left.outline(INK);
    to_sprite(&mut sys, (201, 0), &left, [0, 0]);
    let mut right = Canvas::new(253, 240);
    right.rect(0, 0, 253, 240, INK);
    right.cut_right(213, 60);
    right.outline(PAPER);
    to_sprite(&mut sys, (201, 1), &right.flip_h(), [0, 0]);
    write(&out.join("system.sff"), &sys.write_v2())?;

    let mut fight = Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    };
    to_sprite(&mut fight, (10, 0), &bar_plate(140, 14), [0, 0]);
    to_sprite(&mut fight, (12, 0), &bar_fill(128, 8, INK), [0, 0]);
    to_sprite(&mut fight, (13, 0), &bar_fill(128, 8, RED), [0, 0]);
    to_sprite(&mut fight, (40, 0), &bar_plate(104, 8), [0, 0]);
    to_sprite(&mut fight, (43, 0), &bar_fill(96, 4, INK), [0, 0]);
    to_sprite(&mut fight, (50, 0), &face_frame(34, RED), [0, 0]);
    to_sprite(&mut fight, (51, 0), &face_frame(34, INK), [0, 0]);
    to_sprite(&mut fight, (60, 0), &timer_slab(), [26, 17]);
    for k in 0..7u16 {
        to_sprite(&mut fight, (100 + k, 0), &win_mark(k as u8), [5, 5]);
    }
    to_sprite(&mut fight, (110, 0), &empty_mark(), [5, 5]);
    write(&out.join("fight.sff"), &fight.write_v2())?;
    write(&out.join("system.def"), SYSTEM_DEF.as_bytes())?;
    write(&out.join("fight.def"), FIGHT_DEF.as_bytes())?;
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

const SYSTEM_DEF: &str = r#"; rugen Shonen Ink screenpack, generated by `rugen ui-gen --theme ink`
[Info]
name = "rugen ink"
author = "rugen"
versiondate = 2026
mugenversion = 1.1
localcoord = 426,240

[Files]
spr = system.sff
snd = system.snd
select = select.def
fight = fight.def
font1 = font/rugen-small.def
font2 = font/rugen-menu.def
font3 = font/rugen-title.def
font4 = font/rugen-num.def

[Music]
title.bgm =
select.bgm =
vs.bgm =

[Title Info]
fadein.time = 12
fadeout.time = 10
menu.pos = 213,118
menu.item.font = 2,11,0
menu.item.active.font = 2,11,0
menu.item.spacing = 0,17
menu.item.bg.spr = 310,0
menu.item.active.bg.spr = 311,0
menu.item.bg.offset = 0,-13
menu.itemname.arcade = "ARCADE"
menu.itemname.versus = "VS MODE"
menu.itemname.training = "TRAINING"
menu.itemname.watch = "WATCH"
menu.itemname.options = "OPTIONS"
menu.itemname.exit = "EXIT"
menu.window.margins.y = 12, 8
menu.window.visibleitems = 7
menu.boxcursor.visible = 0
cursor.move.snd = 100,0
cursor.done.snd = 100,1
cancel.snd = 100,2

[TitleBGdef]
bgclearcolor = 243,239,228

[TitleBG 0]
type = normal
spriteno = 1,0
start = 0,0

[TitleBG 1]
type = normal
spriteno = 0,0
start = 0,26
sin.y = 1,120,0

[Select Info]
fadein.time = 10
fadeout.time = 10
rows = 2
columns = 5
wrapping = 0
pos = 106,150
showemptyboxes = 1
moveoveremptyboxes = 0
cell.size = 40,40
cell.spacing = 3
cell.bg.spr = 150,0
cell.random.spr = 150,0
cell.random.switchtime = 4
p1.cursor.startcell = 0,0
p1.cursor.active.spr = 160,0
p1.cursor.done.spr = 161,0
p1.cursor.move.snd = 100,0
p1.cursor.done.snd = 100,1
p1.random.move.snd = 100,0
p2.cursor.startcell = 0,4
p2.cursor.active.spr = 170,0
p2.cursor.done.spr = 171,0
p2.cursor.blink = 1
p2.cursor.move.snd = 100,0
p2.cursor.done.snd = 100,1
p2.random.move.snd = 100,0
stage.move.snd = 100,0
stage.done.snd = 100,1
cancel.snd = 100,2
portrait.spr = 9000,0
portrait.offset = 7,7
portrait.scale = 1,1
title.offset = 213,34
title.font = 3,9,0
p1.face.spr = 9000,1
p1.face.offset = 8,36
p1.face.scale = 0.75,0.75
p1.face.facing = 1
p2.face.spr = 9000,1
p2.face.offset = 418,36
p2.face.scale = 0.75,0.75
p2.face.facing = -1
p1.name.offset = 8,166
p1.name.font = 1,11,1
p2.name.offset = 418,166
p2.name.font = 1,11,-1
stage.pos = 213,236
stage.active.font = 1,10,0
stage.active2.font = 1,9,0
stage.done.font = 1,9,0

[SelectBGdef]
bgclearcolor = 243,239,228

[SelectBG 0]
type = normal
spriteno = 100,0
start = 0,0

[SelectBG 1]
type = normal
spriteno = 120,0
start = -211,32

[SelectBG 2]
type = normal
spriteno = 120,1
start = 103,32

[SelectBG 3]
type = normal
spriteno = 121,1
start = -209,154

[SelectBG 4]
type = normal
spriteno = 121,0
start = 103,154

[SelectBG 5]
type = normal
spriteno = 100,1
start = 0,142

[VS Screen]
time = 150
fadein.time = 12
fadeout.time = 12
match.text = "MATCH %i"
match.offset = 213,24
match.font = 2,9,0
p1.spr = 9000,1
p1.offset = 30,42
p1.facing = 1
p1.scale = 1,1
p2.spr = 9000,1
p2.offset = 396,42
p2.facing = -1
p2.scale = 1,1
p1.name.offset = 90,210
p1.name.font = 2,11,0
p2.name.offset = 336,210
p2.name.font = 2,11,0

[VersusBGdef]
bgclearcolor = 243,239,228

[VersusBG 0]
type = normal
spriteno = 1,0
start = 0,0

[VersusBG 1]
type = normal
spriteno = 201,0
start = -213,0

[VersusBG 2]
type = normal
spriteno = 201,1
start = -40,0

[VersusBG 3]
type = normal
spriteno = 200,0
start = 0,120
layerno = 1
sin.y = 2,60,0

[Option Info]
fadein.time = 10
fadeout.time = 10

[OptionBGdef]
bgclearcolor = 243,239,228

[OptionBG 0]
type = normal
spriteno = 1,0
start = 0,0

[OptionBG 1]
type = normal
spriteno = 300,0
start = 0,36
"#;

const FIGHT_DEF: &str = r#"; rugen Shonen Ink fight screen, generated by `rugen ui-gen --theme ink`
[Info]
localcoord = 426,240

[Files]
sff = fight.sff
snd = fight.snd
font1 = font/rugen-small.def
font2 = font/rugen-num.def
font3 = font/rugen-menu.def
font4 = font/rugen-title.def
fightfx.sff = fightfx.sff
fightfx.air = fightfx.air
common.snd = common.snd

[FightFx]
scale = 1

[Lifebar]
p1.pos = 46,16
p1.bg0.spr = 10,0
p1.mid.spr = 12,0
p1.mid.offset = 6,3
p1.front.spr = 13,0
p1.front.offset = 6,3
p1.range.x = 0,128
p2.pos = 380,16
p2.bg0.spr = 10,0
p2.bg0.facing = -1
p2.mid.spr = 12,0
p2.mid.offset = -6,3
p2.mid.facing = -1
p2.front.spr = 13,0
p2.front.offset = -6,3
p2.front.facing = -1
p2.range.x = 0,-128

[Powerbar]
p1.pos = 46,33
p1.bg0.spr = 40,0
p1.front.spr = 43,0
p1.front.offset = 4,2
p1.range.x = 0,96
p1.counter.offset = 112,8
p1.counter.font = 1,10,1
p2.pos = 380,33
p2.bg0.spr = 40,0
p2.bg0.facing = -1
p2.front.spr = 43,0
p2.front.offset = -4,2
p2.front.facing = -1
p2.range.x = 0,-96
p2.counter.offset = -112,8
p2.counter.font = 1,10,-1
level1.snd = 21,0
level2.snd = 21,0
level3.snd = 21,0

[Face]
p1.pos = 4,8
p1.bg.spr = 50,0
p1.face.spr = 9000,0
p1.face.facing = 1
p1.face.offset = 9,4
p2.pos = 382,8
p2.bg.spr = 51,0
p2.bg.facing = 1
p2.face.spr = 9000,0
p2.face.facing = -1
p2.face.offset = 31,4

[Name]
p1.pos = 52,13
p1.name.font = 1,9,1
p2.pos = 374,13
p2.name.font = 1,9,-1

[Time]
pos = 213,30
bg.spr = 60,0
counter.offset = 0,10
counter.font = 2,11,0
framespercount = 60

[Combo]
team1.pos = 22,206
team1.start.x = -40
team1.counter.font = 4,10,1
team1.counter.shake = 1
team1.text.text = HITS!!
team1.text.font = 3,9,1
team1.text.offset = 2,12
team1.displaytime = 90
team2.pos = 404,206
team2.start.x = 460
team2.counter.font = 4,10,-1
team2.counter.shake = 1
team2.text.text = HITS!!
team2.text.font = 3,9,-1
team2.text.offset = -2,12
team2.displaytime = 90

[Round]
match.wins = 2
match.maxdrawgames = 1
start.waittime = 30
pos = 0,0
round.time = 0
round.default.offset = 213,104
round.default.font = 4,9,0
round.default.text = ROUND %i
round.default.displaytime = 60
round1.snd = 0,1
round2.snd = 0,2
round3.snd = 0,3
round.sndtime = 0
fight.time = 0
fight.offset = 213,112
fight.font = 4,10,0
fight.text = FIGHT!!
fight.displaytime = 45
fight.snd = 1,0
fight.sndtime = 0
ctrl.time = 30
KO.time = 0
KO.offset = 213,104
KO.font = 4,10,0
KO.text = K.O.!!
KO.displaytime = 90
KO.snd = 2,0
DKO.offset = 213,104
DKO.font = 4,10,0
DKO.text = DOUBLE K.O.!!
DKO.displaytime = 60
DKO.snd = 2,1
TO.offset = 213,104
TO.font = 4,9,0
TO.text = TIME OVER
TO.displaytime = 60
TO.snd = 2,2
KO.sndtime = 0
slow.time = 60
over.waittime = 45
over.hittime = 10
over.wintime = 45
over.time = 210
win.time = 60
win.offset = 213,104
win.font = 4,9,0
win.text = %s WINS!!
win.displaytime = 540
win2.offset = 213,104
win2.font = 4,9,0
win2.text = %s AND %s WIN!!
win2.displaytime = 540
draw.offset = 213,104
draw.font = 4,9,0
draw.text = DRAW GAME
draw.displaytime = 540

[WinIcon]
p1.pos = 52,44
p2.pos = 374,44
p1.iconoffset = 13,0
p2.iconoffset = -13,0
p1.counter.offset = 0,0
p1.counter.font = 1,9
p2.counter.offset = 0,0
p2.counter.font = 1,9
p1.empty.spr = 110,0
p2.empty.spr = 110,0
p1.n.spr = 100,0
p2.n.spr = 100,0
p1.s.spr = 101,0
p2.s.spr = 101,0
p1.h.spr = 102,0
p2.h.spr = 102,0
p1.throw.spr = 103,0
p2.throw.spr = 103,0
p1.c.spr = 104,0
p2.c.spr = 104,0
p1.t.spr = 105,0
p2.t.spr = 105,0
p1.suicide.spr = 106,0
p2.suicide.spr = 106,0
p1.perfect.spr = 105,0
p2.perfect.spr = 105,0
"#;
