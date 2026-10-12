//! `rugen screenpack-gen`: builds the rugen motif (`motif/`): pixel fonts,
//! procedurally drawn `system.sff` / `fight.sff`, and the `system.def` /
//! `fight.def` that lay them out for a 426x240 (16:9) pixel grid.
//!
//! Look: arena-fighter composition (dark glossy slanted panels with diagonal
//! cuts, red/orange/white accents, slanted segmented bars, angled frames,
//! hexagon timer) executed as crisp pixel art: dithered gradients, 1-px
//! outlines and 1-px rims, no smooth blur. Everything is an ordinary MUGEN
//! screenpack, so creators can retheme by editing files.

use std::path::Path;

use rugen_formats::sff::{Sff, Version};

use crate::fontgen::{self, Generator, Style};

/// Straight-RGBA drawing surface at the 426x240 pixel grid.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

pub type Rgba = [u8; 4];

fn lerp(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let mut o = [0u8; 4];
    for i in 0..4 {
        o[i] = (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8;
    }
    o
}

fn hash2(x: u32, y: u32) -> f32 {
    let mut n = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}

/// 4x4 ordered-dither threshold in 0..1.
fn bayer(x: i32, y: i32) -> f32 {
    const M: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (M[(y.rem_euclid(4)) as usize][(x.rem_euclid(4)) as usize] as f32 + 0.5) / 16.0
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
        }
    }

    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return [0; 4];
        }
        let o = ((y as u32 * self.w + x as u32) * 4) as usize;
        [self.px[o], self.px[o + 1], self.px[o + 2], self.px[o + 3]]
    }

    pub fn put(&mut self, x: i32, y: i32, c: Rgba) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let o = ((y as u32 * self.w + x as u32) * 4) as usize;
        let a = c[3] as f32 / 255.0;
        if a >= 1.0 {
            self.px[o..o + 4].copy_from_slice(&c);
            return;
        }
        if a <= 0.0 {
            return;
        }
        let d = &mut self.px[o..o + 4];
        let da = d[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for i in 0..3 {
            let v = (c[i] as f32 * a + d[i] as f32 * da * (1.0 - a)) / oa.max(0.0001);
            d[i] = v as u8;
        }
        d[3] = (oa * 255.0) as u8;
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }

    /// Dithered vertical gradient (two-colour ordered dither per row band).
    pub fn vgrad(&mut self, x: i32, y: i32, w: i32, h: i32, top: Rgba, bottom: Rgba) {
        for yy in 0..h {
            let t = yy as f32 / (h - 1).max(1) as f32;
            for xx in 0..w {
                let c = if bayer(x + xx, y + yy) < t {
                    bottom
                } else {
                    top
                };
                self.put(x + xx, y + yy, c);
            }
        }
    }

    /// Dithered vertical gradient inside a parallelogram (rows shift right by
    /// `skew * (h - 1 - row) / (h - 1)`).
    #[allow(clippy::too_many_arguments)]
    pub fn para_grad(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        skew: i32,
        top: Rgba,
        bottom: Rgba,
    ) {
        for yy in 0..h {
            let dx = skew * (h - 1 - yy) / (h - 1).max(1);
            let t = yy as f32 / (h - 1).max(1) as f32;
            for xx in 0..w {
                let c = if bayer(x + xx, y + yy) < t {
                    bottom
                } else {
                    top
                };
                self.put(x + dx + xx, y + yy, c);
            }
        }
    }

    /// 1-px edge of a parallelogram (top light, bottom/sides dark or a rim colour).
    pub fn para_edge(&mut self, x: i32, y: i32, w: i32, h: i32, skew: i32, c: Rgba) {
        for yy in 0..h {
            let dx = skew * (h - 1 - yy) / (h - 1).max(1);
            if yy == 0 || yy == h - 1 {
                self.rect(x + dx, y + yy, w, 1, c);
            } else {
                self.put(x + dx, y + yy, c);
                self.put(x + dx + w - 1, y + yy, c);
            }
        }
    }

    /// Filled hexagon (flat top/bottom) with centre `(cx, cy)`.
    pub fn hexagon(&mut self, cx: i32, cy: i32, rw: i32, rh: i32, c: Rgba) {
        for yy in -rh..=rh {
            let t = 1.0 - (yy.abs() as f32 / rh as f32) * 0.5;
            let half = (rw as f32 * t) as i32;
            self.rect(cx - half, cy + yy, half * 2 + 1, 1, c);
        }
    }

    /// Replace every pixel that borders transparency with `c` (1-px rim).
    pub fn outline(&mut self, c: Rgba) {
        let mut edge = Vec::new();
        for y in 0..self.h as i32 {
            for x in 0..self.w as i32 {
                if self.get(x, y)[3] == 0 {
                    continue;
                }
                let open = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|(dx, dy)| self.get(x + dx, y + dy)[3] == 0);
                if open {
                    edge.push((x, y));
                }
            }
        }
        for (x, y) in edge {
            self.put(x, y, c);
        }
    }

    /// Diagonal slash band from top-left to bottom-right (pixel hatch).
    pub fn slash(&mut self, x0: i32, width: i32, c: Rgba) {
        for y in 0..self.h as i32 {
            let x = x0 + y / 2;
            self.rect(x, y, width, 1, c);
        }
    }

    pub fn noise(&mut self, density: f32, c: Rgba, seed: u32) {
        for y in 0..self.h {
            for x in 0..self.w {
                if hash2(x + seed, y) < density {
                    self.put(x as i32, y as i32, c);
                }
            }
        }
    }

    pub fn blit(&mut self, src: &Canvas, x: i32, y: i32) {
        for yy in 0..src.h {
            for xx in 0..src.w {
                let c = src.get(xx as i32, yy as i32);
                if c[3] > 0 {
                    self.put(x + xx as i32, y + yy as i32, c);
                }
            }
        }
    }

    pub fn from_rgba(w: u32, h: u32, px: Vec<u8>) -> Self {
        Self { w, h, px }
    }

    /// Cut the triangle above the diagonal from `(x0, 0)` to `(x0 + run, h)` (right side removed).
    pub fn cut_right(&mut self, x0: i32, run: i32) {
        for y in 0..self.h as i32 {
            let cut = x0 + run * y / (self.h as i32 - 1).max(1);
            for x in cut..self.w as i32 {
                self.put_raw(x, y, [0; 4]);
            }
        }
    }

    /// Write a pixel even when it is transparent (cuts, clears).
    pub fn put_raw(&mut self, x: i32, y: i32, c: Rgba) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let o = ((y as u32 * self.w + x as u32) * 4) as usize;
        self.px[o..o + 4].copy_from_slice(&c);
    }

    pub fn flip_h(&self) -> Canvas {
        let mut out = Canvas::new(self.w, self.h);
        for y in 0..self.h as i32 {
            for x in 0..self.w as i32 {
                out.put_raw(self.w as i32 - 1 - x, y, self.get(x, y));
            }
        }
        out
    }
}

pub fn to_sprite(sff: &mut Sff, key: (u16, u16), c: &Canvas, axis: [i16; 2]) {
    sff.push_rgba(key, c.w as u16, c.h as u16, axis, c.px.clone());
}

const W: i32 = 426;
const H: i32 = 240;

const BLACK: Rgba = [8, 8, 12, 255];
const PANEL: Rgba = [30, 32, 44, 255];
const PANEL_DARK: Rgba = [14, 14, 22, 255];
const PANEL_LIGHT: Rgba = [52, 56, 74, 255];
const RED: Rgba = [230, 40, 40, 255];
const RED_DARK: Rgba = [120, 16, 24, 255];
const ORANGE: Rgba = [255, 140, 30, 255];
const WHITE: Rgba = [245, 245, 250, 255];
const GREY: Rgba = [120, 124, 140, 255];

/// Near-black backdrop with diagonal dark slashes and a red slash accent.
fn backdrop(seed: u32, accent: bool) -> Canvas {
    let mut c = Canvas::new(W as u32, H as u32);
    c.vgrad(0, 0, W, H, [14, 14, 22, 255], BLACK);
    for k in 0..12 {
        c.slash(-140 + k * 48, 18, [18, 18, 28, 255]);
    }
    if accent {
        c.slash(-60, 30, [70, 14, 20, 255]);
        c.slash(-28, 4, [160, 30, 36, 255]);
        c.slash(300, 14, [40, 18, 10, 255]);
        c.slash(316, 2, [200, 110, 30, 255]);
    }
    c.noise(0.004, [60, 62, 80, 255], seed);
    c
}

/// Glossy slanted panel with a 1-px rim: dithered gradient, lighter top line.
fn panel(w: i32, h: i32, skew: i32, rim: Rgba) -> Canvas {
    let mut c = Canvas::new((w + skew.abs()) as u32, h as u32);
    let x = if skew < 0 { -skew } else { 0 };
    c.para_grad(x, 0, w, h, skew, PANEL, PANEL_DARK);
    // gloss: upper half a touch lighter
    c.para_grad(x, 0, w, h / 2, skew, PANEL_LIGHT, PANEL);
    c.para_edge(x, 0, w, h, skew, [6, 6, 10, 255]);
    c.para_edge(x + 1, 1, w - 2, h - 2, skew, rim);
    c
}

/// Panel with its right side cut diagonally (portrait plates).
fn cut_panel(w: i32, h: i32, rim: Rgba) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    c.vgrad(0, 0, w, h, PANEL, PANEL_DARK);
    c.vgrad(0, 0, w, h / 3, PANEL_LIGHT, PANEL);
    c.cut_right(w - 30, 30);
    c.outline([6, 6, 10, 255]);
    let mut rimmed = Canvas::new(w as u32, h as u32);
    rimmed.blit(&c, 0, 0);
    // inner rim one pixel inside the outline
    let mut inner = Canvas::new(w as u32, h as u32);
    inner.vgrad(1, 1, w - 2, h - 2, PANEL, PANEL_DARK);
    inner.vgrad(1, 1, w - 2, h / 3, PANEL_LIGHT, PANEL);
    inner.cut_right(w - 31, 30);
    inner.outline(rim);
    rimmed.blit(&inner, 0, 0);
    rimmed
}

fn cell_box(size: i32) -> Canvas {
    let mut c = Canvas::new(size as u32, size as u32);
    c.vgrad(0, 0, size, size, PANEL, PANEL_DARK);
    c.rect(0, 0, size, 1, PANEL_LIGHT);
    c.rect(0, 0, 1, size, PANEL_LIGHT);
    c.rect(0, size - 1, size, 1, [6, 6, 10, 255]);
    c.rect(size - 1, 0, 1, size, [6, 6, 10, 255]);
    // clipped corner
    for k in 0..6 {
        c.rect(size - 6 + k, k, 6 - k, 1, [0, 0, 0, 0]);
    }
    c
}

fn cursor_frame(size: i32, color: Rgba) -> Canvas {
    let mut c = Canvas::new(size as u32, size as u32);
    let light = lerp(color, WHITE, 0.6);
    let t = 2;
    c.rect(0, 0, size, t, color);
    c.rect(0, size - t, size, t, color);
    c.rect(0, 0, t, size, color);
    c.rect(size - t, 0, t, size, color);
    c.rect(0, 0, size, 1, light);
    c.rect(0, 0, 1, size, light);
    // corner ticks
    for (x, y) in [(0, 0), (size - 7, 0), (0, size - 7), (size - 7, size - 7)] {
        c.rect(x, y, 7, 7, color);
        c.rect(x + 2, y + 2, 3, 3, light);
    }
    c
}

/// Segmented slanted HP bar fill: gaps every 16 px show the plate underneath.
fn bar_fill(w: i32, h: i32, skew: i32, top: Rgba, bottom: Rgba) -> Canvas {
    let mut c = Canvas::new((w + skew) as u32, h as u32);
    c.para_grad(0, 0, w, h, skew, top, bottom);
    c.para_edge(0, 0, w, h, skew, lerp(top, WHITE, 0.4));
    for yy in 0..h {
        let dx = skew * (h - 1 - yy) / (h - 1).max(1);
        let mut x = 15;
        while x < w {
            c.put_raw(x + dx, yy, [0; 4]);
            x += 16;
        }
    }
    c
}

fn bar_plate(w: i32, h: i32, skew: i32) -> Canvas {
    let mut c = Canvas::new((w + skew) as u32, h as u32);
    c.para_grad(0, 0, w, h, skew, [20, 20, 30, 255], [10, 10, 16, 255]);
    c.para_edge(0, 0, w, h, skew, [4, 4, 8, 255]);
    c.para_edge(1, 1, w - 2, h - 2, skew, [48, 50, 66, 255]);
    c
}

fn face_frame(size: i32, rim: Rgba) -> Canvas {
    let skew = 6;
    let mut c = Canvas::new((size + skew) as u32, size as u32);
    c.para_grad(0, 0, size, size, skew, PANEL_DARK, BLACK);
    c.para_edge(0, 0, size, size, skew, [4, 4, 8, 255]);
    c.para_edge(1, 1, size - 2, size - 2, skew, rim);
    c
}

fn timer_hex() -> Canvas {
    let mut c = Canvas::new(52, 34);
    c.hexagon(25, 16, 25, 16, [6, 6, 10, 255]);
    c.hexagon(25, 16, 23, 14, PANEL_DARK);
    c.hexagon(25, 16, 23, 14, PANEL);
    c.vgrad(4, 3, 44, 12, PANEL_LIGHT, PANEL);
    let mut inner = Canvas::new(52, 34);
    inner.hexagon(25, 16, 24, 15, RED);
    inner.hexagon(25, 16, 22, 13, [0, 0, 0, 0]);
    c.blit(&inner, 0, 0);
    c
}

fn win_mark(kind: u8) -> Canvas {
    let mut c = Canvas::new(11, 11);
    let (col, light) = match kind {
        0 => (RED, [255, 170, 170, 255]),
        1 => (ORANGE, [255, 220, 160, 255]),
        2 => ([90, 160, 255, 255], [190, 220, 255, 255]),
        3 => ([70, 205, 85, 255], [200, 255, 200, 255]),
        4 => (WHITE, WHITE),
        5 => ([255, 225, 70, 255], [255, 250, 200, 255]),
        _ => ([170, 80, 220, 255], [230, 190, 255, 255]),
    };
    for y in 0..11i32 {
        let half = 5 - (y - 5).abs();
        c.rect(5 - half, y, half * 2 + 1, 1, col);
    }
    c.outline([4, 4, 8, 255]);
    c.rect(5, 2, 1, 3, light);
    c.rect(4, 3, 1, 1, light);
    c
}

fn empty_mark() -> Canvas {
    let mut c = Canvas::new(11, 11);
    for y in 0..11i32 {
        let half = 5 - (y - 5).abs();
        c.rect(5 - half, y, half * 2 + 1, 1, PANEL_DARK);
    }
    c.outline(GREY);
    c
}

fn menu_item(w: i32, h: i32, lit: bool) -> Canvas {
    let skew = 8;
    let mut c = Canvas::new((w + skew) as u32, h as u32);
    if lit {
        c.para_grad(0, 0, w, h, skew, RED, RED_DARK);
        c.para_edge(0, 0, w, h, skew, [40, 4, 8, 255]);
        c.para_edge(1, 1, w - 2, h - 2, skew, [255, 150, 150, 255]);
        // slash highlight
        for yy in 0..h {
            let dx = skew * (h - 1 - yy) / (h - 1).max(1);
            c.put(w - 40 + yy / 2 + dx, yy, [255, 200, 200, 255]);
            c.put(w - 39 + yy / 2 + dx, yy, [255, 120, 120, 255]);
        }
    } else {
        c.para_grad(0, 0, w, h, skew, [26, 28, 38, 200], [12, 12, 18, 200]);
        c.para_edge(0, 0, w, h, skew, [6, 6, 10, 255]);
        c.para_edge(1, 1, w - 2, h - 2, skew, [60, 62, 80, 255]);
    }
    c
}

/// Logo text with a diagonal slashed highlight through the letters.
fn logo(generator: &Generator) -> Canvas {
    let (w, h, px) = generator.text_rgba("RUGEN", 0, 3);
    let mut c = Canvas::from_rgba(w, h, px);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let p = c.get(x, y);
            if p[3] == 0 || p[0] < 200 {
                continue;
            }
            let d = x + y - (w as i32 / 2);
            let band = (12..18).contains(&d);
            let lower = y > (h as i32) * 2 / 3;
            if band {
                c.put_raw(x, y, [255, 230, 120, 255]);
            } else if lower {
                c.put_raw(x, y, [220, 222, 232, 255]);
            }
        }
    }
    c
}

fn write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Generate the screenpack (system/fight .def + .sff) into `out`
/// (a theme dir such as `motif/themes/dojo`); fonts go to the shared `motif/font`.
pub fn generate(ttf: &[u8], motif: &Path, out: &Path) -> anyhow::Result<()> {
    let style = |size, italic, rim| Style {
        size,
        italic,
        condense: 0.9,
        rim,
        highlight: true,
    };
    // motif fonts: OFL pixel fonts from motif/font/src (the TTF is only a fallback)
    fontgen::generate_roles(motif, Some(ttf))?;

    // ---- system.sff
    let mut sys = Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    };
    let logo_gen = Generator::new(ttf, style(58, 3, true))?;
    let l = logo(&logo_gen);
    to_sprite(&mut sys, (0, 0), &l, [(l.w / 2) as i16, 0]);
    let vs_gen = Generator::new(ttf, style(40, 3, true))?;
    let (vw, vh, vpx) = vs_gen.text_rgba("VS", 2, 2);
    sys.push_rgba(
        (200, 0),
        vw as u16,
        vh as u16,
        [(vw / 2) as i16, (vh / 2) as i16],
        vpx,
    );
    to_sprite(&mut sys, (1, 0), &backdrop(1, true), [W as i16 / 2, 0]);
    to_sprite(&mut sys, (100, 0), &backdrop(9, false), [W as i16 / 2, 0]);
    to_sprite(&mut sys, (100, 1), &panel(232, 100, 10, RED_DARK), [116, 0]);
    let plate = cut_panel(108, 126, RED);
    to_sprite(&mut sys, (120, 0), &plate, [0, 0]);
    to_sprite(&mut sys, (120, 1), &plate.flip_h(), [0, 0]);
    to_sprite(&mut sys, (121, 0), &panel(100, 14, 6, RED), [0, 0]);
    to_sprite(
        &mut sys,
        (121, 1),
        &panel(100, 14, 6, [90, 160, 255, 255]),
        [0, 0],
    );
    to_sprite(&mut sys, (150, 0), &cell_box(40), [1, 1]);
    to_sprite(
        &mut sys,
        (160, 0),
        &cursor_frame(44, [90, 160, 255, 255]),
        [3, 3],
    );
    to_sprite(&mut sys, (161, 0), &cursor_frame(44, WHITE), [3, 3]);
    to_sprite(&mut sys, (170, 0), &cursor_frame(44, RED), [3, 3]);
    to_sprite(&mut sys, (171, 0), &cursor_frame(44, WHITE), [3, 3]);
    to_sprite(&mut sys, (300, 0), &panel(380, 180, 12, RED_DARK), [190, 0]);
    to_sprite(&mut sys, (310, 0), &menu_item(150, 14, false), [75, 0]);
    to_sprite(&mut sys, (311, 0), &menu_item(150, 14, true), [75, 0]);
    // VS halves
    let mut left = Canvas::new(W as u32 / 2 + 40, H as u32);
    left.vgrad(0, 0, W / 2 + 40, H, [70, 12, 20, 255], [24, 6, 10, 255]);
    for k in 0..6 {
        left.slash(-80 + k * 50, 12, [90, 18, 26, 255]);
    }
    left.cut_right(W / 2 - 20, 60);
    left.outline(RED);
    to_sprite(&mut sys, (201, 0), &left, [0, 0]);
    let mut right = Canvas::new(W as u32 / 2 + 40, H as u32);
    right.vgrad(0, 0, W / 2 + 40, H, [10, 20, 60, 255], [6, 8, 24, 255]);
    for k in 0..6 {
        right.slash(-80 + k * 50, 12, [14, 28, 80, 255]);
    }
    right.cut_right(W / 2 - 20, 60);
    right.outline([90, 160, 255, 255]);
    to_sprite(&mut sys, (201, 1), &right.flip_h(), [0, 0]);
    write(&out.join("system.sff"), &sys.write_v2())?;

    // ---- fight.sff
    let mut fight = Sff {
        version: Some(Version::V2 { patch: 1 }),
        ..Default::default()
    };
    to_sprite(&mut fight, (10, 0), &bar_plate(140, 14, 7), [0, 0]);
    to_sprite(
        &mut fight,
        (12, 0),
        &bar_fill(128, 8, 4, [255, 120, 70, 255], [150, 20, 30, 255]),
        [0, 0],
    );
    to_sprite(
        &mut fight,
        (13, 0),
        &bar_fill(128, 8, 4, [255, 235, 130, 255], [255, 110, 20, 255]),
        [0, 0],
    );
    to_sprite(&mut fight, (40, 0), &bar_plate(104, 8, 4), [0, 0]);
    to_sprite(
        &mut fight,
        (43, 0),
        &bar_fill(96, 4, 2, [170, 230, 255, 255], [40, 110, 230, 255]),
        [0, 0],
    );
    to_sprite(&mut fight, (50, 0), &face_frame(34, RED), [0, 0]);
    to_sprite(
        &mut fight,
        (51, 0),
        &face_frame(34, [90, 160, 255, 255]),
        [0, 0],
    );
    to_sprite(&mut fight, (60, 0), &timer_hex(), [26, 17]);
    for k in 0..7u16 {
        to_sprite(&mut fight, (100 + k, 0), &win_mark(k as u8), [5, 5]);
    }
    to_sprite(&mut fight, (110, 0), &empty_mark(), [5, 5]);
    write(&out.join("fight.sff"), &fight.write_v2())?;

    write(&out.join("system.def"), SYSTEM_DEF.as_bytes())?;
    write(&out.join("fight.def"), FIGHT_DEF.as_bytes())?;
    write(&out.join("README.md"), MOTIF_README.as_bytes())?;
    Ok(())
}

const MOTIF_README: &str = "# motif/

The rugen screenpack: an ordinary MUGEN motif (system.def, fight.def, SFF v2
art, FNT v2 fonts) for a 426x240 (16:9) pixel grid. Everything here is
generated by `rugen screenpack-gen` from procedural pixel art and a system
TTF; edit the .def files or replace the art to retheme. Files missing here
fall back to `data/`.

rugen extensions used by this motif: `menu.item.bg.spr`,
`menu.item.active.bg.spr`, `menu.item.bg.offset` in [Title Info]; `bg.spr` in
[Time]; `p1.empty.spr` in [WinIcon].
";

const SYSTEM_DEF: &str = r#"; rugen screenpack, generated by `rugen screenpack-gen`
[Info]
name = "rugen"
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
menu.item.font = 2,7,0
menu.item.active.font = 2,1,0
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
bgclearcolor = 8,8,12

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
title.font = 3,0,0
p1.face.spr = 9000,1
p1.face.offset = 8,36
p1.face.scale = 0.75,0.75
p1.face.facing = 1
p2.face.spr = 9000,1
p2.face.offset = 418,36
p2.face.scale = 0.75,0.75
p2.face.facing = -1
p1.name.offset = 8,166
p1.name.font = 1,0,1
p2.name.offset = 418,166
p2.name.font = 1,0,-1
stage.pos = 213,236
stage.active.font = 1,1,0
stage.active2.font = 1,7,0
stage.done.font = 1,7,0

[SelectBGdef]
bgclearcolor = 8,8,12

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
match.font = 2,1,0
p1.spr = 9000,1
p1.offset = 30,42
p1.facing = 1
p1.scale = 1,1
p2.spr = 9000,1
p2.offset = 396,42
p2.facing = -1
p2.scale = 1,1
p1.name.offset = 90,210
p1.name.font = 2,0,0
p2.name.offset = 336,210
p2.name.font = 2,5,0

[VersusBGdef]
bgclearcolor = 8,8,12

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
bgclearcolor = 8,8,12

[OptionBG 0]
type = normal
spriteno = 1,0
start = 0,0

[OptionBG 1]
type = normal
spriteno = 300,0
start = 0,36
"#;

const FIGHT_DEF: &str = r#"; rugen fight screen, generated by `rugen screenpack-gen`
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
p1.counter.font = 1,3,1
p2.pos = 380,33
p2.bg0.spr = 40,0
p2.bg0.facing = -1
p2.front.spr = 43,0
p2.front.offset = -4,2
p2.front.facing = -1
p2.range.x = 0,-96
p2.counter.offset = -112,8
p2.counter.font = 1,3,-1
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
p1.name.font = 1,0,1
p2.pos = 374,13
p2.name.font = 1,5,-1

[Time]
pos = 213,30
bg.spr = 60,0
counter.offset = 0,10
counter.font = 2,7,0
framespercount = 60

[Combo]
team1.pos = 22,206
team1.start.x = -40
team1.counter.font = 4,3,1
team1.counter.shake = 1
team1.text.text = HITS
team1.text.font = 3,7,1
team1.text.offset = 2,12
team1.displaytime = 90
team2.pos = 404,206
team2.start.x = 460
team2.counter.font = 4,3,-1
team2.counter.shake = 1
team2.text.text = HITS
team2.text.font = 3,7,-1
team2.text.offset = -2,12
team2.displaytime = 90

[Round]
match.wins = 2
match.maxdrawgames = 1
start.waittime = 30
pos = 0,0
round.time = 0
round.default.offset = 213,104
round.default.font = 4,0,0
round.default.text = ROUND %i
round.default.displaytime = 60
round1.snd = 0,1
round2.snd = 0,2
round3.snd = 0,3
round.sndtime = 0
fight.time = 0
fight.offset = 213,112
fight.font = 4,3,0
fight.text = FIGHT!
fight.displaytime = 45
fight.snd = 1,0
fight.sndtime = 0
ctrl.time = 30
KO.time = 0
KO.offset = 213,104
KO.font = 4,2,0
KO.text = K.O.
KO.displaytime = 90
KO.snd = 2,0
DKO.offset = 213,104
DKO.font = 4,2,0
DKO.text = DOUBLE K.O.
DKO.displaytime = 60
DKO.snd = 2,1
TO.offset = 213,104
TO.font = 4,1,0
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
win.font = 4,0,0
win.text = %s WINS
win.displaytime = 540
win2.offset = 213,104
win2.font = 4,0,0
win2.text = %s AND %s WIN
win2.displaytime = 540
draw.offset = 213,104
draw.font = 4,1,0
draw.text = DRAW GAME
draw.displaytime = 540

[WinIcon]
p1.pos = 52,44
p2.pos = 374,44
p1.iconoffset = 13,0
p2.iconoffset = -13,0
p1.counter.offset = 0,0
p1.counter.font = 1,0
p2.counter.offset = 0,0
p2.counter.font = 1,0
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
