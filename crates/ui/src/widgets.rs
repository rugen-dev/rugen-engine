//! Settings widgets and built-in glyphs: `pips`, `choice`, `keycap`, `icon`,
//! the styled `slider`, borders and tiled images. Kept apart from the layout
//! engine so a theme only restyles them through TOML keys.
//!
//! Glyphs are 7x7 pixel bitmaps drawn with rectangles, so they work with any
//! font and take a colour: `up down left right tri_left tri_right tri_up
//! tri_down check warn back ext inf kbd pad`. Text that is exactly `@name`
//! (a choice option, a keycap label) draws that glyph instead.

use serde_json::Value;

use crate::{Ctx, NodeDef, Painter, Rect, Scope, interpolate, value_num, value_text};

pub(crate) const INK: [f32; 4] = [0.067, 0.067, 0.067, 1.0];
pub(crate) const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub(crate) const RED: [f32; 4] = [0.878, 0.125, 0.125, 1.0];

/// Rows of 7 bits, bit 6 = leftmost pixel.
pub fn glyph_bits(name: &str) -> Option<[u8; 7]> {
    Some(match name {
        "up" => [0x08, 0x1C, 0x3E, 0x7F, 0x1C, 0x1C, 0x1C],
        "down" => [0x1C, 0x1C, 0x1C, 0x7F, 0x3E, 0x1C, 0x08],
        "left" => [0x08, 0x18, 0x3F, 0x7F, 0x3F, 0x18, 0x08],
        "right" => [0x08, 0x0C, 0x7E, 0x7F, 0x7E, 0x0C, 0x08],
        "tri_right" => [0x20, 0x30, 0x38, 0x3C, 0x38, 0x30, 0x20],
        "tri_left" => [0x02, 0x06, 0x0E, 0x1E, 0x0E, 0x06, 0x02],
        "tri_down" => [0x00, 0x7F, 0x3E, 0x1C, 0x08, 0x00, 0x00],
        "tri_up" => [0x00, 0x08, 0x1C, 0x3E, 0x7F, 0x00, 0x00],
        "check" => [0x01, 0x03, 0x06, 0x4C, 0x78, 0x30, 0x00],
        "warn" => [0x08, 0x14, 0x2A, 0x2A, 0x41, 0x49, 0x7F],
        "back" => [0x1F, 0x35, 0x6B, 0x77, 0x6B, 0x35, 0x1F],
        "ext" => [0x0F, 0x03, 0x05, 0x09, 0x10, 0x20, 0x40],
        "inf" => [0x00, 0x36, 0x49, 0x49, 0x36, 0x00, 0x00],
        "kbd" => [0x00, 0x7F, 0x55, 0x7F, 0x41, 0x7F, 0x00],
        "pad" => [0x00, 0x3E, 0x7F, 0x6B, 0x7F, 0x63, 0x00],
        _ => return None,
    })
}

/// `@name` → the glyph name, when the text is a known glyph.
pub fn glyph_of(text: &str) -> Option<&str> {
    let name = text.strip_prefix('@')?;
    glyph_bits(name).map(|_| name)
}

/// Draw a glyph centred in `r`, with whole-number pixels when it fits.
pub fn draw_glyph(
    p: &mut dyn Painter,
    name: &str,
    r: Rect,
    color: [f32; 4],
    alpha: f32,
    clip: Option<Rect>,
) {
    let Some(bits) = glyph_bits(name) else {
        return;
    };
    let side = r.w.min(r.h);
    let px = if side >= 7.0 {
        libm::floorf(side / 7.0)
    } else {
        side / 7.0
    };
    let x0 = libm::floorf(r.x + (r.w - px * 7.0) / 2.0);
    let y0 = libm::floorf(r.y + (r.h - px * 7.0) / 2.0);
    let col = [color[0], color[1], color[2], color[3] * alpha];
    for (row, b) in bits.iter().enumerate() {
        let mut col_i = 0;
        while col_i < 7 {
            if b & (0x40 >> col_i) == 0 {
                col_i += 1;
                continue;
            }
            let start = col_i;
            while col_i < 7 && b & (0x40 >> col_i) != 0 {
                col_i += 1;
            }
            p.draw_rect(
                Rect {
                    x: x0 + start as f32 * px,
                    y: y0 + row as f32 * px,
                    w: (col_i - start) as f32 * px,
                    h: px,
                },
                col,
                0.0,
                clip,
            );
        }
    }
}

/// Four edges of thickness `t` inside `r` (bottom edge `t + depth`).
pub fn draw_border(
    p: &mut dyn Painter,
    r: Rect,
    t: f32,
    depth: f32,
    color: [f32; 4],
    alpha: f32,
    clip: Option<Rect>,
) {
    if t <= 0.0 && depth <= 0.0 {
        return;
    }
    let c = [color[0], color[1], color[2], color[3] * alpha];
    let bottom = t + depth;
    p.draw_rect(Rect { h: t, ..r }, c, 0.0, clip);
    p.draw_rect(
        Rect {
            y: r.y + r.h - bottom,
            h: bottom,
            ..r
        },
        c,
        0.0,
        clip,
    );
    p.draw_rect(Rect { w: t, ..r }, c, 0.0, clip);
    p.draw_rect(
        Rect {
            x: r.x + r.w - t,
            w: t,
            ..r
        },
        c,
        0.0,
        clip,
    );
}

fn fade(c: [f32; 4], alpha: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * alpha]
}

/// Bound number of pips (`max_from`, else `max`, else 5).
fn pip_count(n: &NodeDef, ctx: &Ctx) -> usize {
    let v = n
        .max_from
        .as_deref()
        .map(|b| value_num(ctx.bound(crate::bind_path(b))))
        .or(n.max)
        .unwrap_or(5.0);
    (v.max(0.0) as usize).min(64)
}

/// Choice options (strings) from the `options` bind.
fn choice_options(n: &NodeDef, ctx: &Ctx) -> Vec<String> {
    match n
        .options
        .as_deref()
        .and_then(|b| ctx.bound(crate::bind_path(b)))
    {
        Some(Value::Array(a)) => a.iter().map(value_text).collect(),
        _ => Vec::new(),
    }
}

/// Selected choice index: a number, or the option equal to a string value.
fn choice_index(n: &NodeDef, ctx: &Ctx, opts: &[String]) -> Option<usize> {
    let v = ctx.bound(crate::bind_path(n.value.as_deref()?))?;
    match v {
        Value::Number(x) => x.as_f64().map(|f| f as usize),
        Value::String(s) => opts.iter().position(|o| o == s),
        Value::Bool(b) => Some(usize::from(*b)),
        _ => None,
    }
}

fn label_width(p: &mut dyn Painter, n: &NodeDef, text: &str, h: f32) -> f32 {
    if glyph_of(text).is_some() {
        return (h * 0.5).max(7.0);
    }
    p.text_width(n.font.unwrap_or(1), text, n.scale.unwrap_or(1.0))
}

/// Widths of each choice segment.
fn segments(p: &mut dyn Painter, n: &NodeDef, opts: &[String], h: f32) -> Vec<f32> {
    let pad = n.pad.unwrap_or(12.0);
    let min = n.cell_w.unwrap_or(0.0);
    opts.iter()
        .map(|o| (label_width(p, n, o, h) + 2.0 * pad).max(min))
        .collect()
}

/// Natural size for widgets that size themselves when `w`/`h` are not given.
pub(crate) fn natural_w(n: &NodeDef, ctx: &Ctx, h: f32, p: &mut dyn Painter) -> Option<f32> {
    use crate::Kind;
    match n.kind {
        Kind::Pips => {
            let c = pip_count(n, ctx) as f32;
            let gap = n.gap.unwrap_or(5.0);
            Some(c * h + (c - 1.0).max(0.0) * gap)
        }
        Kind::Choice => {
            let opts = choice_options(n, ctx);
            let b = n.border.unwrap_or(3.0);
            let segs = segments(p, n, &opts, h);
            let total: f32 = segs.iter().sum();
            Some((total - b * (segs.len() as f32 - 1.0).max(0.0)).max(0.0))
        }
        Kind::Keycap => {
            let pad = n.pad.unwrap_or(8.0);
            let glyph = n
                .glyph
                .as_deref()
                .map(|g| interpolate(g, ctx))
                .filter(|g| glyph_bits(g).is_some());
            let inner = match glyph {
                Some(_) => h * 0.45,
                None => {
                    let t = n.text.as_deref().map(|t| interpolate(t, ctx));
                    let t = t.unwrap_or_default();
                    label_width(p, n, &t, h)
                }
            };
            Some(inner + 2.0 * pad + 2.0 * n.border.unwrap_or(3.0))
        }
        _ => None,
    }
}

/// Text centred in `r` (glyph when it is `@name`).
#[allow(clippy::too_many_arguments)]
fn label(
    p: &mut dyn Painter,
    n: &NodeDef,
    text: &str,
    r: Rect,
    bank: i32,
    glyph_color: [f32; 4],
    alpha: f32,
    clip: Option<Rect>,
) {
    if let Some(g) = glyph_of(text) {
        let s = (r.h * 0.5).max(7.0);
        draw_glyph(
            p,
            g,
            Rect {
                x: r.x + (r.w - s) / 2.0,
                y: r.y + (r.h - s) / 2.0,
                w: s,
                h: s,
            },
            glyph_color,
            alpha,
            clip,
        );
        return;
    }
    let font = n.font.unwrap_or(1);
    let scale = n.scale.unwrap_or(1.0);
    let th = p.text_height(font, scale);
    // centred on the glyphs' ink, like button labels
    let ink = p.text_center_dy(font, scale);
    p.draw_text(
        font,
        text,
        r.x + r.w / 2.0,
        r.y + (r.h + th) / 2.0 - ink,
        bank,
        0,
        scale,
        alpha,
        clip,
    );
}

/// `pips`: `max` (or `max_from`) squares, the first `value` filled.
pub(crate) fn draw_pips(
    p: &mut dyn Painter,
    n: &NodeDef,
    ctx: &Ctx,
    r: Rect,
    alpha: f32,
    clip: Option<Rect>,
) {
    let count = pip_count(n, ctx);
    let on = n
        .value
        .as_deref()
        .map(|v| value_num(ctx.bound(crate::bind_path(v))))
        .unwrap_or(0.0);
    let gap = n.gap.unwrap_or(5.0);
    let s = r.h;
    let b = n.border.unwrap_or(3.0);
    let edge = n.border_color.unwrap_or(INK);
    for i in 0..count {
        let cell = Rect {
            x: r.x + i as f32 * (s + gap),
            y: r.y,
            w: s,
            h: s,
        };
        let fill = if (i as f32) < on {
            n.color2.unwrap_or(RED)
        } else {
            n.color.unwrap_or(WHITE)
        };
        p.draw_rect(cell, fade(fill, alpha), 0.0, clip);
        draw_border(p, cell, b, 0.0, edge, alpha, clip);
    }
}

/// `choice`: segmented boxes, the selected one filled with `color2`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_choice(
    p: &mut dyn Painter,
    n: &NodeDef,
    ctx: &Ctx,
    r: Rect,
    lit: bool,
    alpha: f32,
    clip: Option<Rect>,
) {
    let opts = choice_options(n, ctx);
    let sel = choice_index(n, ctx, &opts);
    let segs = segments(p, n, &opts, r.h);
    let b = n.border.unwrap_or(3.0);
    // a thicker focus edge is drawn inside the cells; they keep their places
    let edge_b = if lit { n.border_focus.unwrap_or(b) } else { b };
    if lit && let Some([dx, dy]) = n.shadow_focus {
        let total: f32 = segs.iter().sum::<f32>() - b * (segs.len() as f32 - 1.0).max(0.0);
        let c = n.shadow_color.unwrap_or(INK);
        let sh = Rect {
            x: r.x + dx,
            y: r.y + dy,
            w: total.max(0.0),
            h: r.h,
        };
        p.draw_rect(sh, fade(c, alpha), 0.0, clip);
    }
    let edge = if lit {
        n.border_color_focus.or(n.border_color)
    } else {
        n.border_color
    }
    .unwrap_or(INK);
    let mut x = r.x;
    for (i, (o, w)) in opts.iter().zip(segs).enumerate() {
        let on = sel == Some(i);
        let cell = Rect {
            x,
            y: r.y,
            w,
            h: r.h,
        };
        let fill = if on && lit {
            n.color_focus.or(n.color2).unwrap_or(INK)
        } else if on {
            n.color2.unwrap_or(INK)
        } else {
            n.color.unwrap_or(WHITE)
        };
        p.draw_rect(cell, fade(fill, alpha), 0.0, clip);
        draw_border(p, cell, edge_b, 0.0, edge, alpha, clip);
        let (bank, gc) = if on {
            (
                n.bank_focus.or(n.bank).unwrap_or(0),
                n.text_color_focus.unwrap_or(WHITE),
            )
        } else {
            (n.bank.unwrap_or(0), n.text_color.unwrap_or(INK))
        };
        label(p, n, o, cell, bank, gc, alpha, clip);
        x += w - b;
    }
}

/// `keycap`: a key with a deeper bottom edge; `glyph` or `text` on it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_keycap(
    p: &mut dyn Painter,
    n: &NodeDef,
    ctx: &Ctx,
    text: Option<&str>,
    r: Rect,
    lit: bool,
    alpha: f32,
    clip: Option<Rect>,
) {
    let face = if lit {
        n.color_focus.or(n.color)
    } else {
        n.color
    }
    .unwrap_or(WHITE);
    let edge = if lit {
        n.border_color_focus.or(n.border_color)
    } else {
        n.border_color
    }
    .unwrap_or(INK);
    let b = n.border.unwrap_or(3.0);
    let depth = n.depth.unwrap_or(3.0);
    p.draw_rect(r, fade(face, alpha), 0.0, clip);
    draw_border(p, r, b, depth, edge, alpha, clip);
    let inner = Rect {
        x: r.x + b,
        y: r.y + b,
        w: r.w - 2.0 * b,
        h: r.h - 2.0 * b - depth,
    };
    let gc = if lit {
        n.text_color_focus.or(n.text_color)
    } else {
        n.text_color
    }
    .unwrap_or(INK);
    let glyph = n
        .glyph
        .as_deref()
        .map(|g| interpolate(g, ctx))
        .filter(|g| glyph_bits(g).is_some());
    if let Some(g) = glyph {
        let s = (inner.h * 0.8).max(7.0);
        draw_glyph(
            p,
            &g,
            Rect {
                x: inner.x + (inner.w - s) / 2.0,
                y: inner.y + (inner.h - s) / 2.0,
                w: s,
                h: s,
            },
            gc,
            alpha,
            clip,
        );
        return;
    }
    let bank = if lit { n.bank_focus.or(n.bank) } else { n.bank }.unwrap_or(0);
    if let Some(t) = text.filter(|t| !t.is_empty()) {
        label(p, n, t, inner, bank, gc, alpha, clip);
    }
}

/// `icon`: one glyph filling the rect in `color` (`color_focus` when lit).
pub(crate) fn draw_icon(
    p: &mut dyn Painter,
    n: &NodeDef,
    ctx: &Ctx,
    r: Rect,
    lit: bool,
    alpha: f32,
    clip: Option<Rect>,
) {
    let Some(g) = n.glyph.as_deref().map(|g| interpolate(g, ctx)) else {
        return;
    };
    let c = if lit {
        n.color_focus.or(n.color)
    } else {
        n.color
    }
    .unwrap_or(INK);
    draw_glyph(p, g.trim_start_matches('@'), r, c, alpha, clip);
}

/// Styled slider (any of `knob`, `knob_color`, `track_color` set): a bordered
/// track filled with `color2` up to the value and a bordered knob.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_slider(
    p: &mut dyn Painter,
    n: &NodeDef,
    frac: f32,
    r: Rect,
    lit: bool,
    alpha: f32,
    clip: Option<Rect>,
) {
    let b = n.border.unwrap_or(3.0);
    let edge = n.border_color.unwrap_or(INK);
    p.draw_rect(r, fade(n.track_color.unwrap_or(WHITE), alpha), 0.0, clip);
    p.draw_rect(
        Rect { w: r.w * frac, ..r },
        fade(n.color2.unwrap_or(INK), alpha),
        0.0,
        clip,
    );
    draw_border(p, r, b, 0.0, edge, alpha, clip);
    let [kw, kh] = n.knob.unwrap_or([20.0, r.h * 2.0]);
    let knob = Rect {
        x: r.x + r.w * frac - kw / 2.0,
        y: r.y + (r.h - kh) / 2.0,
        w: kw,
        h: kh,
    };
    let kc = if lit {
        n.color_focus.or(n.knob_color)
    } else {
        n.knob_color
    }
    .unwrap_or(RED);
    p.draw_rect(knob, fade(kc, alpha), 0.0, clip);
    draw_border(p, knob, b, 0.0, edge, alpha, clip);
}

/// Greedy word wrap of `text` into lines at most `width` wide.
pub(crate) fn wrap_lines(
    p: &mut dyn Painter,
    font: i32,
    scale: f32,
    text: &str,
    width: f32,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if !cur.is_empty() && p.text_width(font, &candidate, scale) > width {
            lines.push(std::mem::replace(&mut cur, word.to_string()));
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Repeat an image at its own size (times `scale`) across `r`.
pub(crate) fn draw_tiled(
    p: &mut dyn Painter,
    key: &str,
    r: Rect,
    scale: f32,
    alpha: f32,
    clip: Option<Rect>,
) {
    let Some((iw, ih)) = p.image_size(key) else {
        return;
    };
    let (tw, th) = (iw as f32 * scale, ih as f32 * scale);
    if tw < 1.0 || th < 1.0 {
        return;
    }
    let clip = Some(clip.map_or(r, |c| c.intersect(&r)));
    let mut y = r.y;
    while y < r.y + r.h {
        let mut x = r.x;
        while x < r.x + r.w {
            p.draw_image(key, None, Rect { x, y, w: tw, h: th }, 0.0, alpha, clip);
            x += tw;
        }
        y += th;
    }
}
