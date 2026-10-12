//! Generated OPTIONS / CONTROLS screens for the bundled themes: the TOML
//! layouts (`ui/options.toml`, `ui/controls.toml`), their pixel-art pieces
//! (tab slabs, hatch tiles, the arcade test pad) and each theme's
//! `ui/preview.webp` card for the theme picker. Called from `inkgen` and
//! `uigen`.

use std::path::Path;

use crate::fontgen::{self, Generator, Style};
use crate::pixelize;
use crate::screenpack::Canvas;

type Rgba = [u8; 4];

const PAPER: Rgba = [243, 239, 228, 255];
const INK: Rgba = [17, 17, 17, 255];
const RED: Rgba = [224, 32, 32, 255];
const WHITE: Rgba = [255, 255, 255, 255];

fn x2(c: &Canvas) -> Canvas {
    let mut out = Canvas::new(c.w * 2, c.h * 2);
    for y in 0..out.h as i32 {
        for x in 0..out.w as i32 {
            out.put_raw(x, y, c.get(x / 2, y / 2));
        }
    }
    out
}

fn disc(c: &mut Canvas, cx: f32, cy: f32, r: f32, col: Rgba) {
    let (x0, x1) = (libm::floorf(cx - r) as i32, libm::ceilf(cx + r) as i32);
    let (y0, y1) = (libm::floorf(cy - r) as i32, libm::ceilf(cy + r) as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                c.put(x, y, col);
            }
        }
    }
}

/// Filled disc with a `t`-px edge (a transparent `fill` leaves a ring).
fn ringed(c: &mut Canvas, cx: f32, cy: f32, r: f32, t: f32, fill: Rgba, edge: Rgba) {
    disc(c, cx, cy, r, edge);
    if fill[3] == 0 {
        let mut hole = Canvas::new(c.w, c.h);
        disc(&mut hole, cx, cy, r - t, WHITE);
        for y in 0..c.h as i32 {
            for x in 0..c.w as i32 {
                if hole.get(x, y)[3] > 0 {
                    c.put_raw(x, y, [0, 0, 0, 0]);
                }
            }
        }
    } else {
        disc(c, cx, cy, r - t, fill);
    }
}

/// Rounded pill `w x h` with an edge.
fn pill(w: i32, h: i32, t: i32, fill: Rgba, edge: Rgba) -> Canvas {
    let mut c = Canvas::new(w as u32, h as u32);
    let r = h as f32 / 2.0;
    for (col, inset) in [(edge, 0), (fill, t)] {
        let rr = r - inset as f32;
        disc(&mut c, r, r, rr, col);
        disc(&mut c, w as f32 - r, r, rr, col);
        c.rect(r as i32, inset, w - 2 * r as i32, h - 2 * inset, col);
    }
    c
}

/// Diagonal hatch tile: `on` px lines every `period` px (seamless at `size`).
fn hatch_tile(size: u32, period: i32, on: i32, col: Rgba) -> Canvas {
    let mut c = Canvas::new(size, size);
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            if (x + y) % period < on {
                c.put(x, y, col);
            }
        }
    }
    c
}

/// Left column tab: slanted ink slab, or the wider red one attached to the panel.
fn tab(active: bool) -> Canvas {
    let (w, h, skew) = if active { (165, 30, 9) } else { (145, 25, 9) };
    let mut c = Canvas::new(w, h);
    for y in 0..h as i32 {
        let dx = skew * (h as i32 - 1 - y) / (h as i32 - 1);
        let right = if active {
            w as i32
        } else {
            w as i32 - skew + dx
        };
        c.rect(dx, y, right - dx, 1, if active { RED } else { INK });
    }
    x2(&c)
}

fn text(g: &Generator, s: &str, bank: usize) -> Canvas {
    let (w, h, px) = g.text_rgba(s, bank, 1);
    Canvas::from_rgba(w, h, px)
}

fn para(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, skew: i32, col: Rgba) {
    for yy in 0..h {
        let dx = skew * (h - 1 - yy) / (h - 1).max(1);
        c.rect(x + dx, y + yy, w, 1, col);
    }
}

/// Shonen Ink card for the theme picker (186x103 half res → 372x206).
fn ink_preview(g: &Generator) -> Canvas {
    let (w, h) = (186, 103);
    let mut c = Canvas::new(w, h);
    c.rect(0, 0, w as i32, h as i32, PAPER);
    for y in (0..h as i32).step_by(4) {
        for x in (0..w as i32).step_by(4) {
            c.put(x + (y / 4 % 2) * 2, y, [17, 17, 17, 40]);
        }
    }
    let red = text(g, "RUGEN", fontgen::BANK_RED);
    let ink = text(g, "RUGEN", fontgen::BANK_INK);
    c.blit(&red, 9, 8);
    c.blit(&ink, 7, 6);
    para(&mut c, 8, 32, 74, 11, 5, RED);
    para(&mut c, 8, 46, 64, 10, 5, INK);
    para(&mut c, 8, 59, 64, 10, 5, INK);
    // the hero panel with a hatch
    let (px, py, pw, ph) = (104, 8, 76, 58);
    c.rect(px + 3, py + 3, pw, ph, INK);
    c.rect(px, py, pw, ph, WHITE);
    for d in (0..pw + ph).step_by(5) {
        for y in 0..ph {
            let x = d - y;
            if x >= 0 && x < pw {
                c.put(px + x, py + y, [200, 196, 186, 255]);
            }
        }
    }
    c.rect(px, py, pw, 1, INK);
    c.rect(px, py + ph - 1, pw, 1, INK);
    c.rect(px, py, 1, ph, INK);
    c.rect(px + pw - 1, py, 1, ph, INK);
    x2(&c)
}

/// Neon Dojo card for the theme picker.
fn dojo_preview(g: &Generator) -> Canvas {
    let (w, h) = (186i32, 103i32);
    let mut c = Canvas::new(w as u32, h as u32);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        c.rect(0, y, w, 1, [mix(20, 5), mix(26, 7), mix(43, 13), 255]);
    }
    for y in 0..h {
        let x0 = -20 + (h - y) * 2 / 5;
        c.rect(x0, y, 100, 1, [199, 31, 59, 34]);
    }
    let red = text(g, "RUGEN", fontgen::BANK_RED);
    let white = text(g, "RUGEN", 7);
    c.blit(&red, 10, 8);
    c.blit(&white, 8, 6);
    para(&mut c, 8, 32, 74, 11, 3, [199, 31, 59, 255]);
    for y in [46, 59] {
        para(&mut c, 8, y, 64, 10, 3, [59, 74, 107, 255]);
        para(&mut c, 9, y + 1, 62, 8, 3, [28, 34, 54, 255]);
    }
    for y in 0..58 {
        let dx = (58 - y) / 7;
        c.put(106 + dx, 8 + y, [59, 74, 107, 255]);
        c.put(106 + dx + 72, 8 + y, [59, 74, 107, 255]);
        if y == 0 || y == 57 {
            c.rect(106 + dx, 8 + y, 73, 1, [59, 74, 107, 255]);
        } else {
            c.rect(107 + dx, 8 + y, 71, 1, [11, 15, 26, 255]);
        }
    }
    x2(&c)
}

fn preview_font(ttf: &[u8]) -> anyhow::Result<Generator<'_>> {
    Generator::new(
        ttf,
        Style {
            size: 16,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
    )
}

/// Shonen Ink: options/controls TOML, tab slabs, hatches, test-pad pieces, preview.
pub fn ink(ui: &Path, ttf: &[u8]) -> anyhow::Result<()> {
    let w = |name: &str, c: &Canvas| pixelize::write_webp(&ui.join(name), c);
    w("tab.webp", &tab(false))?;
    w("tab_on.webp", &tab(true))?;
    w("hatch_red.webp", &hatch_tile(18, 9, 3, [224, 32, 32, 30]))?;
    w("hatch_fine.webp", &hatch_tile(14, 7, 1, [17, 17, 17, 22]))?;
    let mut base = Canvas::new(76, 76);
    disc(&mut base, 38.0, 38.0, 37.5, INK);
    w("pad_base.webp", &x2(&base))?;
    let mut ball = Canvas::new(40, 40);
    ringed(&mut ball, 20.0, 20.0, 20.0, 2.5, RED, INK);
    w("pad_ball.webp", &x2(&ball))?;
    for (name, fill, drop) in [("pad_btn.webp", WHITE, 0.0), ("pad_btn_lit.webp", RED, 2.0)] {
        let mut b = Canvas::new(34, 37);
        disc(&mut b, 17.0, 20.0, 17.0, INK);
        ringed(&mut b, 17.0, 17.0 + drop, 17.0, 2.5, fill, INK);
        w(name, &x2(&b))?;
    }
    w("pill.webp", &x2(&pill(28, 11, 2, WHITE, INK)))?;
    w("pill_lit.webp", &x2(&pill(28, 11, 2, RED, INK)))?;
    let mut ring = Canvas::new(24, 24);
    ringed(&mut ring, 12.0, 12.0, 12.0, 3.0, [0, 0, 0, 0], INK);
    w("ring.webp", &ring)?;
    let mut ring_lit = Canvas::new(24, 24);
    ringed(&mut ring_lit, 12.0, 12.0, 12.0, 3.0, [0, 0, 0, 0], WHITE);
    w("ring_lit.webp", &ring_lit)?;
    w("preview.webp", &ink_preview(&preview_font(ttf)?))?;
    let [(_, options, controls), _] = screens();
    std::fs::write(ui.join("options.toml"), options)?;
    std::fs::write(ui.join("controls.toml"), controls)?;
    Ok(())
}

/// Neon Dojo: options/controls TOML and the theme preview.
pub fn dojo(ui: &Path, ttf: &[u8]) -> anyhow::Result<()> {
    pixelize::write_webp(&ui.join("preview.webp"), &dojo_preview(&preview_font(ttf)?))?;
    let [_, (_, options, controls)] = screens();
    std::fs::write(ui.join("options.toml"), options)?;
    std::fs::write(ui.join("controls.toml"), controls)?;
    Ok(())
}

/// Shared README section describing the two screens' data.
pub const README: &str = r#"
## Options and controls

`options.toml` binds to `tabs[]` (`id label active ext press`), `tab`
(`id title subtitle index`), `rows[]`, `show_rows`, `themes[]` (`id label
preview has_preview active press`), `show_themes`, `help` (sentence for the
focused row) and `keys` (`left right left_glyph right_glyph`: P1's change
keys). A row has `id label help widget` plus, per widget: `is_pips max on`,
`is_choice options sel`, `is_slider frac`, `is_text`, `is_link`, and `text`
/ `has_text` (value shown next to it), `adjustable`, `press left right`
actions (`cycle:<id>`, `adjust:<id>:<±1>`, `open:keys1|keys2`). Keep the
ids `tabs[i].tab`, `rows[i].row` and `themes[i].card`: the engine follows the
focus through them (active tab, help line, Esc back to the tab column).

`controls.toml` binds to `players[]` (`label active press`), `device`
(`label glyph is_pad press`), `binds[]` (`id label group icon icon_glyph
round square cap glyph listening lit hot conflict clean conflict_text
show_group show_conflict press`), `conflicts` (`count any none text`),
`pad.<action>` (`cap glyph lit label`) with `pad.dx` / `pad.dy` (stick
offset), `presets[]` (`label active press`), `listening`, `fixed`. Keep
`binds[i].row` as the row id (Backspace clears the focused row). Actions:
`listen:<i>`, `preset:<i>`, `player:<n>`, `device:next`, `defaults`, `back`.

Widgets used here: `pips` (`max`/`max_from`, `value`, `color` / `color2`,
`border`), `choice` (`options`, `value`, `pad`, `bank` / `bank_focus`,
`color` / `color2`), `slider` with `knob = [w, h]`, `knob_color`,
`track_color`, `keycap` (`text` or `glyph`, `w` = min width, `depth`,
`border_color`, `text_color`), `icon` (`glyph`, `color`). Glyphs: `up down
left right tri_left tri_right tri_up tri_down check warn back ext inf kbd
pad`; a choice option or keycap text of `@name` draws that glyph. `when =
"focus"|"idle"` shows a node only while its row is (not) focused, `group`
keeps up/down inside a column, `tile = true` repeats an image, `wrap = true`
wraps text, `blink = <ticks>` blinks, and `x`/`y` may bind (`"{pad.dx}"`).
"#;

const INK_OPTIONS: &str = r##"# rugen options — Shonen Ink. Coordinates are 1280x720; data in ../README.md.
background = [0.95, 0.94, 0.89, 1.0]
exit_time = 8

[[node]]
kind = "image"
fit = "cover"
image = "ui/paper.webp"
w = "100%"
h = "100%"

# ---------------------------------------------------------------- header
[[node]]
kind = "panel"
dock = "top"
w = "100%"
h = 84
color = [0.067, 0.067, 0.067, 1.0]

[[node]]
kind = "text"
dock = "left top"
text = "OPTIONS"
x = 44
y = 25
font = 3
bank = 10
scale = 2

[[node]]
kind = "text"
dock = "left top"
text = "OPTIONS"
x = 40
y = 21
font = 3
bank = 11
scale = 2

[[node]]
kind = "text"
dock = "right top"
text = "SAVED TO RUGEN.TOML"
anchor = "tr"
x = 40
y = 34
font = 2
bank = 6

# ---------------------------------------------------------------- tabs
[[node]]
kind = "vbox"
dock = "left"
id = "tabs"
x = 30
y = 120
w = 330
h = 470
gap = 10
items = "tabs"
group = "tabs"
enter = "left"
enter_time = 14
stagger = 2

  [[node.children]]
  kind = "button"
  id = "tab"
  w = 290
  h = 50
  on_press = "{item.press}"

    [[node.children.children]]
    kind = "image"
    image = "ui/tab.webp"
    w = 290
    h = 50
    visible = "!item.active"

    [[node.children.children]]
    kind = "image"
    image = "ui/tab_on.webp"
    w = 330
    h = 60
    visible = "item.active"

    [[node.children.children]]
    kind = "text"
    text = "{item.label}"
    anchor = "left"
    x = 24
    font = 1
    bank = 11
    scale = 2
    visible = "!item.active"

    [[node.children.children]]
    kind = "text"
    text = "{item.label}"
    anchor = "left"
    x = 26
    y = 5
    font = 1
    bank = 11
    scale = 2
    visible = "item.active"

    [[node.children.children]]
    kind = "icon"
    glyph = "ext"
    anchor = "right"
    x = 34
    w = 14
    h = 14
    color = [0.95, 0.94, 0.89, 1.0]
    visible = "item.ext"

# ---------------------------------------------------------------- content panel
[[node]]
kind = "panel"
dock = "stretch-x"
image = "ui/panel9.webp"
nine = [12, 12, 24, 24]
x = 360
y = 110
w = 892
h = 482
enter = "up"
enter_time = 14

  [[node.children]]
  kind = "text"
  text = "{tab.title}"
  x = 28
  y = 22
  font = 1
  bank = 9
  scale = 2

  [[node.children]]
  kind = "text"
  text = "{tab.subtitle}"
  x = 240
  y = 26
  font = 2
  bank = 9
  alpha = 0.6
  visible = "!show_themes"

  [[node.children]]
  kind = "list"
  dock = "stretch-x"
  id = "rows"
  x = 20
  y = 62
  w = 830
  h = 392
  items = "rows"
  group = "rows"
  visible = "show_rows"

    [[node.children.children]]
    kind = "button"
    id = "row"
    h = 46
    on_press = "{item.press}"
    on_left = "{item.left}"
    on_right = "{item.right}"

      [[node.children.children.children]]
      kind = "image"
      image = "ui/hatch_red.webp"
      tile = true
      w = "100%"
      h = "100%"
      when = "focus"

      [[node.children.children.children]]
      kind = "panel"
      w = 8
      color = [0.878, 0.125, 0.125, 1.0]
      when = "focus"

      [[node.children.children.children]]
      kind = "panel"
      anchor = "bl"
      h = 2
      color = [0.067, 0.067, 0.067, 0.14]

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      anchor = "left"
      x = 22
      font = 2
      scale = 1.25
      bank = 9
      when = "idle"

      [[node.children.children.children]]
      kind = "icon"
      glyph = "tri_right"
      anchor = "left"
      x = 22
      w = 14
      h = 14
      when = "focus"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      anchor = "left"
      x = 46
      font = 2
      scale = 1.25
      bank = 9
      when = "focus"

      [[node.children.children.children]]
      kind = "hbox"
      anchor = "right"
      x = 16
      w = 620
      gap = 12
      justify = "end"
      align = "center"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "tri_left"
        w = 14
        h = 14
        color = [0.878, 0.125, 0.125, 1.0]
        when = "focus"
        visible = "item.adjustable"

        [[node.children.children.children.children]]
        kind = "pips"
        h = 22
        gap = 5
        max_from = "item.max"
        value = "item.on"
        visible = "item.is_pips"

        [[node.children.children.children.children]]
        kind = "choice"
        h = 30
        pad = 14
        font = 2
        bank = 9
        bank_focus = 11
        options = "item.options"
        value = "item.sel"
        visible = "item.is_choice"

        [[node.children.children.children.children]]
        kind = "slider"
        w = 240
        h = 16
        min = 0
        max = 1
        value = "item.frac"
        knob = [20, 32]
        focus = false
        visible = "item.is_slider"

        [[node.children.children.children.children]]
        kind = "text"
        text = "{item.text}"
        font = 1
        bank = 9
        scale = 2
        visible = "item.has_text"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "ext"
        w = 14
        h = 14
        visible = "item.is_link"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "tri_right"
        w = 14
        h = 14
        color = [0.878, 0.125, 0.125, 1.0]
        when = "focus"
        visible = "item.adjustable"

  # INTERFACE: theme picker
  [[node.children]]
  kind = "text"
  text = "UI THEME"
  x = 28
  y = 62
  font = 2
  bank = 10
  visible = "show_themes"

  [[node.children]]
  kind = "hbox"
  id = "themes"
  x = 28
  y = 90
  w = 840
  h = 300
  gap = 40
  items = "themes"
  group = "themes"
  visible = "show_themes"

    [[node.children.children]]
    kind = "button"
    id = "card"
    w = 392
    h = 272
    on_press = "{item.press}"

      [[node.children.children.children]]
      kind = "panel"
      image = "ui/panel9_red.webp"
      nine = [12, 12, 24, 24]
      w = 392
      h = 226
      visible = "item.active"

      [[node.children.children.children]]
      kind = "panel"
      image = "ui/panel9.webp"
      nine = [12, 12, 24, 24]
      w = 392
      h = 226
      visible = "!item.active"

      [[node.children.children.children]]
      kind = "image"
      image = "{item.preview}"
      x = 4
      y = 4
      w = 372
      h = 206
      visible = "item.has_preview"

      [[node.children.children.children]]
      kind = "image"
      image = "ui/hatch_red.webp"
      tile = true
      y = 228
      w = 380
      h = 40
      when = "focus"

      [[node.children.children.children]]
      kind = "icon"
      glyph = "tri_right"
      x = 4
      y = 241
      w = 14
      h = 14
      when = "focus"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      x = 26
      y = 240
      font = 1
      bank = 9
      scale = 2

      [[node.children.children.children]]
      kind = "panel"
      x = 256
      y = 232
      w = 124
      h = 32
      color = [0.878, 0.125, 0.125, 1.0]
      border = 3
      visible = "item.active"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "check"
        anchor = "left"
        x = 14
        w = 14
        h = 14
        color = [1.0, 1.0, 1.0, 1.0]

        [[node.children.children.children.children]]
        kind = "text"
        text = "ACTIVE"
        anchor = "left"
        x = 38
        font = 2
        bank = 11

      [[node.children.children.children]]
      kind = "text"
      text = "ENTER TO USE"
      anchor = "tr"
      x = 12
      y = 242
      font = 2
      bank = 9
      alpha = 0.6
      visible = "!item.active"

  [[node.children]]
  kind = "text"
  text = "APPLIED AT ONCE AND SAVED"
  x = 28
  y = 400
  font = 2
  bank = 9
  alpha = 0.6
  visible = "show_themes"

# ---------------------------------------------------------------- help bar
[[node]]
kind = "panel"
dock = "stretch-x"
image = "ui/panel9_black.webp"
nine = [12, 12, 24, 24]
x = 360
y = 604
w = 892
h = 78
enter = "up"
enter_time = 18

  [[node.children]]
  kind = "panel"
  anchor = "left"
  x = 22
  y = -6
  w = 72
  h = 32
  border = 3
  border_color = [1.0, 0.35, 0.27, 1.0]

    [[node.children.children]]
    kind = "text"
    text = "HELP"
    anchor = "center"
    font = 2
    bank = 10

  [[node.children]]
  kind = "text"
  dock = "stretch-x"
  text = "{help}"
  x = 114
  y = 4
  w = 740
  h = 58
  wrap = true
  line_gap = 8
  font = 2
  scale = 1.25
  bank = 11

# ---------------------------------------------------------------- key hints
[[node]]
kind = "hbox"
dock = "left"
x = 30
y = 624
w = 320
h = 34
gap = 10
align = "center"

  [[node.children]]
  kind = "keycap"
  w = 34
  h = 30
  text = "{keys.left}"
  glyph = "{keys.left_glyph}"
  font = 2
  bank = 9

  [[node.children]]
  kind = "keycap"
  w = 34
  h = 30
  text = "{keys.right}"
  glyph = "{keys.right_glyph}"
  font = 2
  bank = 9

  [[node.children]]
  kind = "text"
  text = "CHANGE"
  font = 2
  bank = 9
  alpha = 0.6

[[node]]
kind = "hbox"
dock = "left"
x = 30
y = 664
w = 320
h = 34
gap = 10
align = "center"

  [[node.children]]
  kind = "keycap"
  h = 30
  text = "ESC"
  font = 2
  bank = 9

  [[node.children]]
  kind = "text"
  text = "SAVE & BACK"
  font = 2
  bank = 9
  alpha = 0.6
"##;

const INK_CONTROLS: &str = r##"# rugen controls — Shonen Ink. Coordinates are 1280x720; data in ../README.md.
background = [0.95, 0.94, 0.89, 1.0]
exit_time = 8

[[node]]
kind = "image"
fit = "cover"
image = "ui/paper.webp"
w = "100%"
h = "100%"

# ---------------------------------------------------------------- header
[[node]]
kind = "panel"
dock = "top"
w = "100%"
h = 84
color = [0.067, 0.067, 0.067, 1.0]

[[node]]
kind = "text"
dock = "left top"
text = "CONTROLS"
x = 44
y = 25
font = 3
bank = 10
scale = 2

[[node]]
kind = "text"
dock = "left top"
text = "CONTROLS"
x = 40
y = 21
font = 3
bank = 11
scale = 2

[[node]]
kind = "hbox"
dock = "top"
x = 470
y = 22
w = 300
h = 40
gap = 8
items = "players"
group = "header"

  [[node.children]]
  kind = "button"
  w = 72
  h = 40
  color = [0.2, 0.2, 0.2, 1.0]
  color_focus = [0.878, 0.125, 0.125, 1.0]
  selected = "item.active"
  text = "{item.label}"
  font = 1
  scale = 2
  bank = 11
  bank_focus = 11
  on_press = "{item.press}"

[[node]]
kind = "hbox"
dock = "right top"
anchor = "tr"
x = 40
y = 22
w = 520
h = 40
gap = 10
justify = "end"
align = "center"
group = "header"

  [[node.children]]
  kind = "text"
  text = "DEVICE"
  font = 2
  bank = 11
  alpha = 0.7

  [[node.children]]
  kind = "keycap"
  h = 38
  pad = 12
  text = "{device.label}"
  font = 2
  bank = 11
  bank_focus = 11
  color = [0.067, 0.067, 0.067, 1.0]
  color_focus = [0.878, 0.125, 0.125, 1.0]
  border_color = [1.0, 1.0, 1.0, 1.0]
  focus = true
  on_press = "{device.press}"

  [[node.children]]
  kind = "icon"
  glyph = "tri_down"
  w = 12
  h = 12
  color = [1.0, 1.0, 1.0, 1.0]

# ---------------------------------------------------------------- test pad
[[node]]
kind = "panel"
dock = "left"
image = "ui/panel9.webp"
nine = [12, 12, 24, 24]
x = 40
y = 112
w = 612
h = 452
enter = "up"
enter_time = 14

  [[node.children]]
  kind = "image"
  image = "ui/hatch_fine.webp"
  tile = true
  x = 4
  y = 4
  w = 592
  h = 432

  [[node.children]]
  kind = "text"
  text = "TEST PAD"
  x = 22
  y = 18
  font = 2
  bank = 10

  [[node.children]]
  kind = "text"
  text = "PRESS YOUR KEYS, THEY LIGHT UP"
  anchor = "tr"
  x = 34
  y = 18
  font = 2
  bank = 9
  alpha = 0.6

  [[node.children]]
  kind = "image"
  image = "ui/pad_base.webp"
  x = 76
  y = 110
  w = 152
  h = 152

  # the stick leans with the held directions
  [[node.children]]
  kind = "panel"
  x = "{pad.dx}"
  y = "{pad.dy}"
  w = 600
  h = 440

    [[node.children.children]]
    kind = "panel"
    x = 136
    y = 124
    w = 32
    h = 76
    color = [0.33, 0.33, 0.33, 1.0]

    [[node.children.children]]
    kind = "image"
    image = "ui/pad_ball.webp"
    x = 112
    y = 88
    w = 80
    h = 80

  [[node.children]]
  kind = "keycap"
  x = 130
  y = 52
  w = 44
  h = 32
  text = "{pad.up.cap}"
  glyph = "{pad.up.glyph}"
  selected = "pad.up.lit"
  color_focus = [0.878, 0.125, 0.125, 1.0]
  text_color_focus = [1.0, 1.0, 1.0, 1.0]
  font = 2
  bank = 9
  bank_focus = 11

  [[node.children]]
  kind = "keycap"
  x = 130
  y = 278
  w = 44
  h = 32
  text = "{pad.down.cap}"
  glyph = "{pad.down.glyph}"
  selected = "pad.down.lit"
  color_focus = [0.878, 0.125, 0.125, 1.0]
  text_color_focus = [1.0, 1.0, 1.0, 1.0]
  font = 2
  bank = 9
  bank_focus = 11

  [[node.children]]
  kind = "keycap"
  x = 20
  y = 168
  w = 44
  h = 32
  text = "{pad.left.cap}"
  glyph = "{pad.left.glyph}"
  selected = "pad.left.lit"
  color_focus = [0.878, 0.125, 0.125, 1.0]
  text_color_focus = [1.0, 1.0, 1.0, 1.0]
  font = 2
  bank = 9
  bank_focus = 11

  [[node.children]]
  kind = "keycap"
  x = 240
  y = 168
  w = 44
  h = 32
  text = "{pad.right.cap}"
  glyph = "{pad.right.glyph}"
  selected = "pad.right.lit"
  color_focus = [0.878, 0.125, 0.125, 1.0]
  text_color_focus = [1.0, 1.0, 1.0, 1.0]
  font = 2
  bank = 9
  bank_focus = 11
@BUTTONS@
  [[node.children]]
  kind = "text"
  text = "STICK"
  x = 112
  y = 352
  font = 2
  bank = 9
  alpha = 0.6

  [[node.children]]
  kind = "text"
  text = "TOP ROW X Y Z - LOW ROW A B C"
  x = 296
  y = 352
  font = 2
  bank = 9
  alpha = 0.6

  [[node.children]]
  kind = "image"
  image = "ui/pill.webp"
  image_focus = "ui/pill_lit.webp"
  selected = "pad.start.lit"
  x = 402
  y = 392
  w = 56
  h = 22

  [[node.children]]
  kind = "keycap"
  x = 468
  y = 386
  w = 44
  h = 32
  text = "{pad.start.cap}"
  glyph = "{pad.start.glyph}"
  selected = "pad.start.lit"
  color_focus = [0.878, 0.125, 0.125, 1.0]
  text_color_focus = [1.0, 1.0, 1.0, 1.0]
  font = 2
  bank = 9
  bank_focus = 11

# ---------------------------------------------------------------- presets
[[node]]
kind = "text"
dock = "left"
text = "PRESETS"
x = 40
y = 580
font = 2
bank = 9

[[node]]
kind = "hbox"
dock = "left"
x = 40
y = 604
w = 620
h = 44
gap = 10
items = "presets"
group = "presets"

  [[node.children]]
  kind = "keycap"
  h = 40
  pad = 12
  depth = 0
  text = "{item.label}"
  font = 2
  bank = 9
  bank_focus = 11
  color = [1.0, 1.0, 1.0, 1.0]
  color_focus = [0.067, 0.067, 0.067, 1.0]
  selected = "item.active"
  focus = true
  on_press = "{item.press}"

    [[node.children.children]]
    kind = "panel"
    anchor = "bottom"
    y = -8
    h = 4
    color = [0.878, 0.125, 0.125, 1.0]
    when = "focus"

[[node]]
kind = "hbox"
dock = "left"
x = 40
y = 664
w = 600
h = 34
gap = 10
align = "center"

  [[node.children]]
  kind = "keycap"
  h = 30
  text = "ESC"
  font = 2
  bank = 9

  [[node.children]]
  kind = "text"
  text = "BACK TO OPTIONS"
  font = 2
  bank = 9
  alpha = 0.6

# ---------------------------------------------------------------- bindings
[[node]]
kind = "panel"
dock = "stretch-x"
image = "ui/panel9.webp"
nine = [12, 12, 24, 24]
x = 680
y = 112
w = 572
h = 532
enter = "up"
enter_time = 18

  [[node.children]]
  kind = "text"
  text = "BINDINGS"
  x = 22
  y = 18
  font = 2
  bank = 9

  [[node.children]]
  kind = "hbox"
  anchor = "tr"
  x = 32
  y = 12
  w = 300
  h = 26
  gap = 8
  justify = "end"
  align = "center"

    [[node.children.children]]
    kind = "icon"
    glyph = "warn"
    w = 14
    h = 14
    color = [0.878, 0.125, 0.125, 1.0]
    visible = "conflicts.any"

    [[node.children.children]]
    kind = "icon"
    glyph = "check"
    w = 14
    h = 14
    visible = "conflicts.none"

    [[node.children.children]]
    kind = "text"
    text = "{conflicts.text}"
    font = 2
    bank = 10
    visible = "conflicts.any"

    [[node.children.children]]
    kind = "text"
    text = "{conflicts.text}"
    font = 2
    bank = 9
    visible = "conflicts.none"

  [[node.children]]
  kind = "list"
  dock = "stretch-x"
  id = "binds"
  x = 16
  y = 46
  w = 528
  h = 464
  gap = 2
  items = "binds"
  group = "binds"

    [[node.children.children]]
    kind = "button"
    id = "row"
    h = 38
    on_press = "{item.press}"

      [[node.children.children.children]]
      kind = "panel"
      color = [0.878, 0.125, 0.125, 1.0]
      visible = "item.listening"

      [[node.children.children.children]]
      kind = "panel"
      color = [0.067, 0.067, 0.067, 1.0]
      visible = "item.lit"

      [[node.children.children.children]]
      kind = "image"
      image = "ui/hatch_red.webp"
      tile = true
      w = "100%"
      h = "100%"
      when = "focus"

      [[node.children.children.children]]
      kind = "panel"
      w = 6
      color = [0.878, 0.125, 0.125, 1.0]
      when = "focus"

      [[node.children.children.children]]
      kind = "panel"
      anchor = "bl"
      h = 2
      color = [0.067, 0.067, 0.067, 0.14]

      [[node.children.children.children]]
      kind = "keycap"
      anchor = "left"
      x = 14
      w = 24
      h = 24
      pad = 0
      depth = 0
      color = [0.0, 0.0, 0.0, 0.0]
      border_color_focus = [1.0, 1.0, 1.0, 1.0]
      text_color_focus = [1.0, 1.0, 1.0, 1.0]
      text = "{item.icon}"
      glyph = "{item.icon_glyph}"
      font = 1
      bank = 9
      bank_focus = 11
      selected = "item.hot"
      visible = "item.square"

      [[node.children.children.children]]
      kind = "image"
      image = "ui/ring.webp"
      image_focus = "ui/ring_lit.webp"
      selected = "item.hot"
      anchor = "left"
      x = 14
      w = 24
      h = 24
      visible = "item.round"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.icon}"
      anchor = "left"
      x = 14
      w = 24
      align = "center"
      font = 1
      bank = 9
      bank_focus = 11
      selected = "item.hot"
      visible = "item.round"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      anchor = "left"
      x = 52
      font = 2
      bank = 9
      bank_focus = 11
      selected = "item.hot"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.group}"
      anchor = "left"
      x = 160
      font = 2
      bank = 9
      alpha = 0.6
      bank_focus = 11
      selected = "item.hot"
      visible = "item.show_group"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.conflict_text}"
      anchor = "left"
      x = 160
      font = 2
      bank = 10
      bank_focus = 11
      selected = "item.lit"
      visible = "item.show_conflict"

      [[node.children.children.children]]
      kind = "text"
      text = "PRESS A KEY - ESC CANCEL"
      anchor = "left"
      x = 160
      font = 2
      bank = 11
      blink = 40
      visible = "item.listening"

      [[node.children.children.children]]
      kind = "keycap"
      anchor = "right"
      x = 12
      w = 70
      h = 30
      text = "{item.cap}"
      glyph = "{item.glyph}"
      font = 2
      bank = 9
      visible = "item.clean"

      [[node.children.children.children]]
      kind = "keycap"
      anchor = "right"
      x = 12
      w = 70
      h = 30
      text = "{item.cap}"
      glyph = "{item.glyph}"
      font = 2
      bank = 10
      border_color = [0.878, 0.125, 0.125, 1.0]
      text_color = [0.878, 0.125, 0.125, 1.0]
      visible = "item.conflict"

      [[node.children.children.children]]
      kind = "keycap"
      anchor = "right"
      x = 12
      w = 70
      h = 30
      text = "..."
      font = 2
      bank = 9
      visible = "item.listening"

# ---------------------------------------------------------------- footer
[[node]]
kind = "hbox"
dock = "left"
x = 680
y = 656
w = 580
h = 34
gap = 8
align = "center"

  [[node.children]]
  kind = "keycap"
  h = 30
  text = "ENTER"
  font = 2
  bank = 9

  [[node.children]]
  kind = "text"
  text = "REBIND"
  font = 2
  bank = 9
  alpha = 0.6

  [[node.children]]
  kind = "keycap"
  w = 40
  h = 30
  glyph = "back"

  [[node.children]]
  kind = "text"
  text = "CLEAR"
  font = 2
  bank = 9
  alpha = 0.6

  [[node.children]]
  kind = "keycap"
  h = 30
  text = "F5"
  font = 2
  bank = 9

  [[node.children]]
  kind = "text"
  text = "DEFAULTS"
  font = 2
  bank = 9
  alpha = 0.6

  [[node.children]]
  kind = "keycap"
  h = 30
  text = "TAB"
  font = 2
  bank = 9

  [[node.children]]
  kind = "text"
  text = "1P/2P"
  font = 2
  bank = 9
  alpha = 0.6
"##;

const DOJO_OPTIONS: &str = r##"# rugen options — Neon Dojo. Coordinates are 1280x720; data in README.md.
background = [0.03, 0.04, 0.08, 1.0]

[[node]]
kind = "image"
fit = "cover"
image = "ui/bg.webp"
w = "100%"
h = "100%"

[[node]]
kind = "text"
dock = "left top"
text = "OPTIONS"
x = 60
y = 34
font = 3
bank = 0
scale = 2
enter = "down"

[[node]]
kind = "text"
dock = "right top"
text = "SAVED TO RUGEN.TOML"
anchor = "tr"
x = 60
y = 46
font = 2
bank = 6

[[node]]
kind = "vbox"
dock = "left"
id = "tabs"
x = 60
y = 120
w = 260
h = 470
gap = 10
items = "tabs"
group = "tabs"
enter = "left"

  [[node.children]]
  kind = "button"
  id = "tab"
  h = 54
  image = "ui/button.webp"
  image_focus = "ui/button_lit.webp"
  selected = "item.active"
  on_press = "{item.press}"

    [[node.children.children]]
    kind = "text"
    text = "{item.label}"
    anchor = "left"
    x = 26
    font = 2
    bank = 7
    bank_focus = 4
    selected = "item.active"

    [[node.children.children]]
    kind = "icon"
    glyph = "ext"
    anchor = "right"
    x = 24
    w = 14
    h = 14
    color = [0.85, 0.87, 0.95, 1.0]
    visible = "item.ext"

[[node]]
kind = "panel"
dock = "stretch-x"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 350
y = 110
w = 870
h = 480
pad = 24
enter = "up"

  [[node.children]]
  kind = "text"
  text = "{tab.title}"
  x = 4
  font = 2
  bank = 4
  scale = 2

  [[node.children]]
  kind = "text"
  text = "{tab.subtitle}"
  x = 260
  y = 10
  font = 2
  bank = 6
  visible = "!show_themes"

  [[node.children]]
  kind = "list"
  dock = "stretch-x"
  id = "rows"
  y = 54
  w = 822
  h = 370
  gap = 6
  items = "rows"
  group = "rows"
  visible = "show_rows"

    [[node.children.children]]
    kind = "button"
    id = "row"
    h = 44
    image = "ui/row.webp"
    image_focus = "ui/row_lit.webp"
    on_press = "{item.press}"
    on_left = "{item.left}"
    on_right = "{item.right}"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      anchor = "left"
      x = 22
      font = 2
      bank = 7

      [[node.children.children.children]]
      kind = "hbox"
      anchor = "right"
      x = 18
      w = 600
      gap = 10
      justify = "end"
      align = "center"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "tri_left"
        w = 10
        h = 10
        color = [1.0, 0.8, 0.3, 1.0]
        when = "focus"
        visible = "item.adjustable"

        [[node.children.children.children.children]]
        kind = "pips"
        h = 18
        gap = 4
        max_from = "item.max"
        value = "item.on"
        border = 2
        color = [0.1, 0.12, 0.2, 1.0]
        color2 = [1.0, 0.75, 0.25, 1.0]
        border_color = [0.45, 0.55, 0.8, 1.0]
        visible = "item.is_pips"

        [[node.children.children.children.children]]
        kind = "choice"
        h = 28
        pad = 12
        border = 2
        font = 2
        bank = 7
        bank_focus = 4
        color = [0.1, 0.12, 0.2, 1.0]
        color2 = [0.6, 0.12, 0.2, 1.0]
        border_color = [0.45, 0.55, 0.8, 1.0]
        text_color = [0.85, 0.87, 0.95, 1.0]
        text_color_focus = [1.0, 0.8, 0.3, 1.0]
        options = "item.options"
        value = "item.sel"
        visible = "item.is_choice"

        [[node.children.children.children.children]]
        kind = "slider"
        w = 220
        h = 12
        min = 0
        max = 1
        value = "item.frac"
        knob = [14, 24]
        border = 2
        track_color = [0.1, 0.12, 0.2, 1.0]
        color2 = [0.6, 0.12, 0.2, 1.0]
        knob_color = [1.0, 0.75, 0.25, 1.0]
        border_color = [0.45, 0.55, 0.8, 1.0]
        focus = false
        visible = "item.is_slider"

        [[node.children.children.children.children]]
        kind = "text"
        text = "{item.text}"
        font = 2
        bank = 4
        visible = "item.has_text"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "ext"
        w = 14
        h = 14
        color = [1.0, 0.8, 0.3, 1.0]
        visible = "item.is_link"

        [[node.children.children.children.children]]
        kind = "icon"
        glyph = "tri_right"
        w = 10
        h = 10
        color = [1.0, 0.8, 0.3, 1.0]
        when = "focus"
        visible = "item.adjustable"

  [[node.children]]
  kind = "hbox"
  id = "themes"
  y = 60
  w = 822
  h = 320
  gap = 30
  items = "themes"
  group = "themes"
  visible = "show_themes"

    [[node.children.children]]
    kind = "button"
    id = "card"
    w = 390
    h = 280
    image = "ui/row.webp"
    image_focus = "ui/row_lit.webp"
    on_press = "{item.press}"

      [[node.children.children.children]]
      kind = "image"
      image = "{item.preview}"
      x = 9
      y = 9
      w = 372
      h = 206
      visible = "item.has_preview"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      x = 16
      y = 232
      font = 2
      bank = 7

      [[node.children.children.children]]
      kind = "text"
      text = "ACTIVE"
      anchor = "tr"
      x = 16
      y = 232
      font = 2
      bank = 4
      visible = "item.active"

      [[node.children.children.children]]
      kind = "text"
      text = "ENTER TO USE"
      anchor = "tr"
      x = 16
      y = 232
      font = 2
      bank = 6
      visible = "!item.active"

[[node]]
kind = "panel"
dock = "stretch-x"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 350
y = 604
w = 870
h = 76

  [[node.children]]
  kind = "text"
  dock = "stretch-x"
  text = "{help}"
  x = 24
  w = 820
  h = 76
  wrap = true
  font = 2
  bank = 7

[[node]]
kind = "hbox"
dock = "left"
x = 60
y = 640
w = 280
h = 34
gap = 10
align = "center"

  [[node.children]]
  kind = "keycap"
  h = 30
  text = "ESC"
  font = 2
  bank = 7
  color = [0.1, 0.12, 0.2, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]

  [[node.children]]
  kind = "text"
  text = "BACK / SAVE"
  font = 2
  bank = 6
"##;

const DOJO_CONTROLS: &str = r##"# rugen controls — Neon Dojo. Coordinates are 1280x720; data in README.md.
background = [0.03, 0.04, 0.08, 1.0]

[[node]]
kind = "image"
fit = "cover"
image = "ui/bg.webp"
w = "100%"
h = "100%"

[[node]]
kind = "text"
dock = "left top"
text = "CONTROLS"
x = 60
y = 34
font = 3
bank = 0
scale = 2

[[node]]
kind = "hbox"
dock = "top"
x = 470
y = 30
w = 200
h = 40
gap = 8
items = "players"
group = "header"

  [[node.children]]
  kind = "button"
  w = 72
  h = 40
  image = "ui/plate.webp"
  image_focus = "ui/plate_lit.webp"
  selected = "item.active"
  text = "{item.label}"
  font = 2
  bank = 7
  bank_focus = 4
  on_press = "{item.press}"

[[node]]
kind = "keycap"
dock = "right top"
anchor = "tr"
x = 60
y = 30
h = 38
pad = 12
text = "{device.label}"
font = 2
bank = 7
bank_focus = 4
color = [0.1, 0.12, 0.2, 1.0]
color_focus = [0.6, 0.12, 0.2, 1.0]
border = 2
border_color = [0.45, 0.55, 0.8, 1.0]
focus = true
on_press = "{device.press}"
group = "header"

[[node]]
kind = "panel"
dock = "left"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 60
y = 110
w = 560
h = 440
pad = 24

  [[node.children]]
  kind = "text"
  text = "TEST PAD"
  font = 2
  bank = 4

  [[node.children]]
  kind = "keycap"
  x = 110
  y = 70
  w = 50
  h = 34
  text = "{pad.up.cap}"
  glyph = "{pad.up.glyph}"
  selected = "pad.up.lit"
  font = 2
  bank = 7
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  text_color = [0.85, 0.87, 0.95, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]

  [[node.children]]
  kind = "keycap"
  x = 110
  y = 190
  w = 50
  h = 34
  text = "{pad.down.cap}"
  glyph = "{pad.down.glyph}"
  selected = "pad.down.lit"
  font = 2
  bank = 7
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  text_color = [0.85, 0.87, 0.95, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]

  [[node.children]]
  kind = "keycap"
  x = 40
  y = 130
  w = 50
  h = 34
  text = "{pad.left.cap}"
  glyph = "{pad.left.glyph}"
  selected = "pad.left.lit"
  font = 2
  bank = 7
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  text_color = [0.85, 0.87, 0.95, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]

  [[node.children]]
  kind = "keycap"
  x = 180
  y = 130
  w = 50
  h = 34
  text = "{pad.right.cap}"
  glyph = "{pad.right.glyph}"
  selected = "pad.right.lit"
  font = 2
  bank = 7
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  text_color = [0.85, 0.87, 0.95, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]
@DOJO_BUTTONS@
  [[node.children]]
  kind = "keycap"
  x = 290
  y = 320
  w = 120
  h = 34
  text = "START {pad.start.cap}"
  selected = "pad.start.lit"
  font = 2
  bank = 7
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]

[[node]]
kind = "hbox"
dock = "left"
x = 60
y = 580
w = 600
h = 44
gap = 10
items = "presets"
group = "presets"

  [[node.children]]
  kind = "keycap"
  h = 40
  pad = 12
  depth = 0
  text = "{item.label}"
  font = 2
  bank = 7
  bank_focus = 4
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]
  border_color_focus = [1.0, 0.8, 0.3, 1.0]
  selected = "item.active"
  focus = true
  on_press = "{item.press}"

[[node]]
kind = "panel"
dock = "stretch-x"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 660
y = 110
w = 560
h = 520
pad = 20

  [[node.children]]
  kind = "text"
  text = "BINDINGS"
  font = 2
  bank = 4

  [[node.children]]
  kind = "text"
  text = "{conflicts.text}"
  anchor = "tr"
  font = 2
  bank = 0
  visible = "conflicts.any"

  [[node.children]]
  kind = "text"
  text = "{conflicts.text}"
  anchor = "tr"
  font = 2
  bank = 6
  visible = "conflicts.none"

  [[node.children]]
  kind = "list"
  dock = "stretch-x"
  id = "binds"
  y = 30
  w = 520
  h = 450
  gap = 2
  items = "binds"
  group = "binds"

    [[node.children.children]]
    kind = "button"
    id = "row"
    h = 38
    image = "ui/row.webp"
    image_focus = "ui/row_lit.webp"
    on_press = "{item.press}"

      [[node.children.children.children]]
      kind = "panel"
      color = [0.6, 0.12, 0.2, 1.0]
      visible = "item.hot"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      anchor = "left"
      x = 18
      font = 2
      bank = 7

      [[node.children.children.children]]
      kind = "text"
      text = "{item.group}"
      anchor = "left"
      x = 130
      font = 2
      bank = 6
      visible = "item.show_group"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.conflict_text}"
      anchor = "left"
      x = 130
      font = 2
      bank = 0
      visible = "item.show_conflict"

      [[node.children.children.children]]
      kind = "text"
      text = "PRESS A KEY - ESC CANCEL"
      anchor = "left"
      x = 130
      font = 2
      bank = 4
      blink = 40
      visible = "item.listening"

      [[node.children.children.children]]
      kind = "keycap"
      anchor = "right"
      x = 10
      w = 70
      h = 30
      text = "{item.cap}"
      glyph = "{item.glyph}"
      font = 2
      bank = 7
      color = [0.1, 0.12, 0.2, 1.0]
      text_color = [0.85, 0.87, 0.95, 1.0]
      border = 2
      border_color = [0.45, 0.55, 0.8, 1.0]
      visible = "item.clean"

      [[node.children.children.children]]
      kind = "keycap"
      anchor = "right"
      x = 10
      w = 70
      h = 30
      text = "{item.cap}"
      glyph = "{item.glyph}"
      font = 2
      bank = 0
      color = [0.1, 0.12, 0.2, 1.0]
      text_color = [1.0, 0.3, 0.3, 1.0]
      border = 2
      border_color = [1.0, 0.3, 0.3, 1.0]
      visible = "item.conflict"

[[node]]
kind = "text"
dock = "left"
text = "ENTER REBIND   BACKSPACE CLEAR   F5 DEFAULTS   TAB 1P/2P   ESC BACK"
x = 60
y = 660
font = 2
bank = 6
"##;

/// One arcade button (image, label, key cap) of the ink test pad at `(x, y)`.
fn ink_button(id: &str, label: &str, x: i32, y: i32) -> String {
    format!(
        r#"
  [[node.children]]
  kind = "image"
  image = "ui/pad_btn.webp"
  image_focus = "ui/pad_btn_lit.webp"
  selected = "pad.{id}.lit"
  x = {x}
  y = {y}
  w = 68
  h = 74

  [[node.children]]
  kind = "text"
  text = "{label}"
  x = {x}
  y = {y}
  w = 68
  h = 68
  align = "center"
  font = 1
  scale = 2
  bank = 9
  bank_focus = 11
  selected = "pad.{id}.lit"

  [[node.children]]
  kind = "keycap"
  x = {kx}
  y = {ky}
  w = 44
  h = 32
  text = "{{pad.{id}.cap}}"
  glyph = "{{pad.{id}.glyph}}"
  selected = "pad.{id}.lit"
  color_focus = [0.878, 0.125, 0.125, 1.0]
  text_color_focus = [1.0, 1.0, 1.0, 1.0]
  font = 2
  bank = 9
  bank_focus = 11
"#,
        kx = x + 12,
        ky = y + 84,
    )
}

fn dojo_button(id: &str, label: &str, x: i32, y: i32) -> String {
    format!(
        r#"
  [[node.children]]
  kind = "keycap"
  x = {x}
  y = {y}
  w = 64
  h = 64
  text = "{label}"
  selected = "pad.{id}.lit"
  font = 2
  bank = 7
  bank_focus = 4
  color = [0.1, 0.12, 0.2, 1.0]
  color_focus = [0.6, 0.12, 0.2, 1.0]
  border = 2
  border_color = [0.45, 0.55, 0.8, 1.0]

  [[node.children]]
  kind = "text"
  text = "{{pad.{id}.cap}}"
  x = {x}
  y = {ty}
  w = 64
  align = "center"
  font = 2
  bank = 6
"#,
        ty = y + 72,
    )
}

/// The final TOML texts (buttons expanded).
pub fn screens() -> [(&'static str, String, String); 2] {
    let ink_buttons: String = [
        ("x", "X", 304, 90),
        ("y", "Y", 394, 70),
        ("z", "Z", 484, 70),
        ("a", "A", 304, 214),
        ("b", "B", 394, 194),
        ("c", "C", 484, 194),
    ]
    .iter()
    .map(|(id, l, x, y)| ink_button(id, l, *x, *y))
    .collect();
    let dojo_buttons: String = [
        ("x", "X", 280, 60),
        ("y", "Y", 360, 50),
        ("z", "Z", 440, 50),
        ("a", "A", 280, 170),
        ("b", "B", 360, 160),
        ("c", "C", 440, 160),
    ]
    .iter()
    .map(|(id, l, x, y)| dojo_button(id, l, *x, *y))
    .collect();
    [
        (
            "ink",
            INK_OPTIONS.to_string(),
            INK_CONTROLS.replace("@BUTTONS@", &ink_buttons),
        ),
        (
            "dojo",
            DOJO_OPTIONS.to_string(),
            DOJO_CONTROLS.replace("@DOJO_BUTTONS@", &dojo_buttons),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_screens_parse() {
        for (theme, options, controls) in screens() {
            rugen_ui::ScreenDef::parse(&options).unwrap_or_else(|e| panic!("{theme} options: {e}"));
            rugen_ui::ScreenDef::parse(&controls)
                .unwrap_or_else(|e| panic!("{theme} controls: {e}"));
            assert!(!controls.contains('@'), "{theme}: placeholder left");
        }
    }
}
