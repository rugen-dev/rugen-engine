//! `rugen ui-gen`: writes the default front-end into `motif/ui/` — WebP
//! assets drawn procedurally in the pixel style, the `title.toml` /
//! `options.toml` screens, `news.json` and a README for modders.

use std::path::Path;

use crate::fontgen::{Generator, Style};
use crate::screenpack::Canvas;

type Rgba = [u8; 4];

const NAVY: Rgba = [10, 14, 30, 255];
const NAVY_LIGHT: Rgba = [22, 34, 66, 255];
const TEAL: Rgba = [30, 90, 110, 255];
const MAROON: Rgba = [118, 30, 48, 255];
const MAROON_DARK: Rgba = [70, 16, 30, 255];
const MAROON_LIT: Rgba = [170, 46, 62, 255];
const ORANGE: Rgba = [255, 150, 60, 255];
const PANEL: Rgba = [20, 22, 34, 232];
const PANEL_DARK: Rgba = [10, 11, 18, 240];
const RIM: Rgba = [70, 80, 110, 255];
const BLACK: Rgba = [4, 4, 8, 255];

fn write_webp(path: &Path, c: &Canvas) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    image_webp::WebPEncoder::new(file)
        .encode(&c.px, c.w, c.h, image_webp::ColorType::Rgba8)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(())
}

/// Night-sky backdrop: dithered navy gradient, teal horizon glow, diagonal
/// light shafts and sparse stars. 640x360, drawn at 2x.
fn backdrop() -> Canvas {
    let (w, h) = (640u32, 360u32);
    let mut c = Canvas::new(w, h);
    c.vgrad(0, 0, w as i32, h as i32, NAVY, NAVY_LIGHT);
    c.vgrad(0, 250, w as i32, 110, [22, 34, 66, 0], TEAL);
    // light shafts
    for (x0, width, a) in [(-80, 70, 18u8), (120, 40, 14), (330, 90, 12), (520, 50, 16)] {
        for y in 0..h as i32 {
            let x = x0 + y;
            c.rect(x, y, width, 1, [120, 160, 220, a]);
        }
    }
    c.noise(0.004, [220, 230, 255, 200], 7);
    c.noise(0.002, [255, 240, 200, 255], 19);
    // ground band
    c.vgrad(0, 330, w as i32, 30, [12, 30, 40, 255], [6, 12, 20, 255]);
    c
}

/// Slanted Brawlhalla-style menu button (200x27, shown at 2x).
fn button(lit: bool) -> Canvas {
    let (w, h, skew) = (190, 27, 7);
    let mut c = Canvas::new((w + skew + 2) as u32, h as u32);
    if lit {
        c.para_grad(1, 0, w, h, skew, MAROON_LIT, MAROON);
        c.para_edge(1, 0, w, h, skew, BLACK);
        c.para_edge(2, 1, w - 2, h - 2, skew, [255, 190, 150, 255]);
        // slash highlight near the right end
        for yy in 1..h - 1 {
            let dx = skew * (h - 1 - yy) / (h - 1);
            c.put(w - 30 + yy / 2 + dx, yy, [255, 220, 200, 255]);
        }
    } else {
        c.para_grad(1, 0, w, h, skew, MAROON, MAROON_DARK);
        c.para_edge(1, 0, w, h, skew, BLACK);
        c.para_edge(2, 1, w - 2, h - 2, skew, [160, 70, 90, 255]);
    }
    // bottom shadow line
    c.rect(0, h - 1, w + skew + 2, 1, [0, 0, 0, 120]);
    c
}

/// 9-slice dark glossy panel (24x24, borders 8).
fn panel() -> Canvas {
    let mut c = Canvas::new(24, 24);
    c.vgrad(0, 0, 24, 24, PANEL, PANEL_DARK);
    c.outline(RIM);
    c.rect(1, 1, 22, 1, [120, 130, 170, 255]);
    c
}

/// Settings row (380x20, shown at 2x).
fn row(lit: bool) -> Canvas {
    let (w, h, skew) = (374, 20, 5);
    let mut c = Canvas::new((w + skew + 1) as u32, h as u32);
    if lit {
        c.para_grad(0, 0, w, h, skew, MAROON_LIT, MAROON_DARK);
        c.para_edge(0, 0, w, h, skew, [255, 190, 150, 255]);
    } else {
        c.para_grad(0, 0, w, h, skew, [30, 32, 48, 220], [16, 16, 26, 220]);
        c.para_edge(0, 0, w, h, skew, [60, 66, 92, 255]);
    }
    c
}

/// Placeholder feature art: gradient, big slash, bold caption.
fn news_art(
    generator: &Generator,
    w: u32,
    h: u32,
    top: Rgba,
    bottom: Rgba,
    caption: &str,
) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.vgrad(0, 0, w as i32, h as i32, top, bottom);
    c.slash(w as i32 / 3, 16, [255, 255, 255, 40]);
    c.slash(w as i32 / 3 + 22, 6, [255, 255, 255, 70]);
    c.noise(0.01, [255, 255, 255, 90], 3);
    let (tw, th, px) = generator.text_rgba(caption, 1, 2);
    let t = Canvas::from_rgba(tw, th, px);
    c.blit(&t, (w as i32 - tw as i32) / 2, (h as i32 - th as i32) / 2);
    c.outline(BLACK);
    c
}

/// Logo: big italic "RUGEN" with a slashed highlight and a red under-bar.
fn logo(generator: &Generator) -> Canvas {
    let (w, h, px) = generator.text_rgba("RUGEN", 0, 3);
    let mut c = Canvas::new(w + 12, h + 10);
    let text = Canvas::from_rgba(w, h, px);
    c.blit(&text, 6, 0);
    for y in 0..h as i32 {
        for x in 0..(w + 12) as i32 {
            let p = c.get(x, y);
            if p[3] == 0 || p[0] < 200 {
                continue;
            }
            let d = x + y - (w as i32 / 2);
            if (14..20).contains(&d) {
                c.put(x, y, [255, 230, 120, 255]);
            } else if y > (h as i32) * 2 / 3 {
                c.put(x, y, [214, 218, 232, 255]);
            }
        }
    }
    // under-bar with a cut end
    let bar_y = h as i32 + 3;
    c.para_grad(10, bar_y, w as i32 - 20, 5, 4, ORANGE, [200, 60, 50, 255]);
    c
}

fn avatar() -> Canvas {
    let mut c = Canvas::new(28, 28);
    c.vgrad(0, 0, 28, 28, [70, 90, 150, 255], [30, 40, 80, 255]);
    // simple pixel helmet silhouette
    c.rect(8, 6, 12, 10, [230, 210, 180, 255]);
    c.rect(6, 4, 16, 4, [60, 40, 40, 255]);
    c.rect(7, 16, 14, 8, [200, 60, 60, 255]);
    c.outline([240, 240, 255, 255]);
    c
}

const TITLE_TOML: &str = r##"# rugen title screen — edit freely, see README.md. Coordinates are 1280x720.
background = [0.03, 0.04, 0.08, 1.0]

[[node]]
kind = "image"
image = "ui/bg.webp"
w = 1280
h = 720

[[node]]
kind = "image"
id = "logo"
image = "ui/logo.webp"
x = 56
y = 30
scale = 2
enter = "down"
enter_time = 22

# profile strip
[[node]]
kind = "panel"
id = "profile"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
anchor = "tr"
x = 24
y = 22
w = 440
h = 76
enter = "right"
enter_time = 20

  [[node.children]]
  kind = "image"
  image = "{profile.avatar}"
  x = 10
  y = 10
  w = 56
  h = 56

  [[node.children]]
  kind = "text"
  text = "{profile.name}"
  x = 80
  y = 10
  font = 2
  bank = 7
  scale = 2

  [[node.children]]
  kind = "text"
  text = "LEVEL {profile.level}   {profile.shards} SHARDS   {profile.wins} W / {profile.losses} L"
  x = 80
  y = 46
  font = 1
  bank = 4
  scale = 2

# main menu: one slanted button per entry of `menu`
[[node]]
kind = "vbox"
id = "menu"
x = 60
y = 236
w = 400
gap = 10
items = "menu"
enter = "left"
enter_time = 20

  [[node.children]]
  kind = "button"
  id = "{item.id}"
  h = 54
  image = "ui/button.webp"
  image_focus = "ui/button_lit.webp"
  text = "{item.label}"
  font = 2
  bank = 7
  bank_focus = 4
  scale = 2
  align = "left"
  pad = 26
  hover_scale = 1.03
  on_press = "{item.action}"

# featured panel
[[node]]
kind = "panel"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 500
y = 236
w = 500
h = 330
enter = "up"
enter_time = 22
enter_delay = 4

  [[node.children]]
  kind = "image"
  image = "{news.0.image}"
  x = 6
  y = 6
  w = 488
  h = 258

  [[node.children]]
  kind = "panel"
  color = [0.0, 0.0, 0.0, 0.6]
  anchor = "bl"
  x = 6
  y = 6
  w = 488
  h = 60

  [[node.children]]
  kind = "text"
  text = "{news.0.title}"
  anchor = "bl"
  x = 18
  y = 34
  font = 2
  bank = 7
  scale = 2

  [[node.children]]
  kind = "text"
  text = "{news.0.subtitle}"
  anchor = "bl"
  x = 18
  y = 12
  w = 460
  font = 1
  bank = 6
  scale = 2

# two smaller panels
[[node]]
kind = "panel"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 1016
y = 236
w = 240
h = 160
enter = "up"
enter_time = 22
enter_delay = 8

  [[node.children]]
  kind = "image"
  image = "{news.1.image}"
  x = 6
  y = 6
  w = 228
  h = 120

  [[node.children]]
  kind = "text"
  text = "{news.1.title}"
  anchor = "bl"
  x = 12
  y = 8
  font = 1
  bank = 7
  scale = 2

[[node]]
kind = "panel"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 1016
y = 406
w = 240
h = 160
enter = "up"
enter_time = 22
enter_delay = 12

  [[node.children]]
  kind = "image"
  image = "{news.2.image}"
  x = 6
  y = 6
  w = 228
  h = 120

  [[node.children]]
  kind = "text"
  text = "{news.2.title}"
  anchor = "bl"
  x = 12
  y = 8
  font = 1
  bank = 7
  scale = 2

# footer
[[node]]
kind = "text"
text = "RUGEN {version}"
anchor = "bl"
x = 24
y = 16
font = 1
bank = 6
scale = 2

[[node]]
kind = "text"
text = "ENTER / CLICK  SELECT     ESC  BACK     F11  FULLSCREEN"
anchor = "br"
x = 24
y = 16
font = 1
bank = 6
scale = 2
"##;

const OPTIONS_TOML: &str = r##"# rugen options screen — rows come from the engine (`rows`), layout is yours.
background = [0.03, 0.04, 0.08, 1.0]

[[node]]
kind = "image"
image = "ui/bg.webp"
w = 1280
h = 720

[[node]]
kind = "text"
text = "OPTIONS"
anchor = "top"
y = 44
font = 3
bank = 0
scale = 2
enter = "down"

[[node]]
kind = "panel"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 220
y = 130
w = 840
h = 540
pad = 24
enter = "up"
enter_time = 18

  [[node.children]]
  kind = "list"
  id = "rows"
  w = 792
  h = 420
  gap = 6
  items = "rows"

    [[node.children.children]]
    kind = "button"
    id = "{item.id}"
    h = 40
    image = "ui/row.webp"
    image_focus = "ui/row_lit.webp"
    on_press = "{item.press}"
    on_left = "{item.left}"
    on_right = "{item.right}"

      [[node.children.children.children]]
      kind = "text"
      text = "{item.label}"
      anchor = "left"
      x = 24
      font = 2
      bank = 7
      scale = 2

      [[node.children.children.children]]
      kind = "text"
      text = "{item.value}"
      anchor = "right"
      x = 28
      font = 2
      bank = 4
      scale = 2

  [[node.children]]
  kind = "button"
  id = "back"
  anchor = "bl"
  w = 220
  h = 54
  image = "ui/button.webp"
  image_focus = "ui/button_lit.webp"
  text = "BACK"
  font = 2
  bank = 7
  bank_focus = 4
  scale = 2
  on_press = "back"

  [[node.children]]
  kind = "text"
  text = "LEFT / RIGHT  CHANGE      ENTER  SELECT      SAVED ON EXIT"
  anchor = "br"
  x = 8
  y = 60
  font = 1
  bank = 6
  scale = 2

# key capture overlay
[[node]]
kind = "panel"
color = [0.0, 0.0, 0.0, 0.75]
visible = "capture.active"

  [[node.children]]
  kind = "text"
  text = "{capture.prompt}"
  anchor = "center"
  y = -20
  font = 2
  bank = 4
  scale = 3

  [[node.children]]
  kind = "text"
  text = "{capture.progress}     ESC  CANCEL"
  anchor = "center"
  y = 36
  font = 1
  bank = 7
  scale = 2
"##;

const NEWS_JSON: &str = r#"[
  { "title": "WELCOME TO RUGEN", "subtitle": "Drop .zip / .rar characters into chars/ and play.", "image": "ui/news1.webp" },
  { "title": "ARENAS", "subtitle": "Platforms, blast zones, stocks.", "image": "ui/news2.webp" },
  { "title": "NETPLAY", "subtitle": "Rollback online, room codes.", "image": "ui/news3.webp" }
]
"#;

const README: &str = r#"# motif/ui — the data-driven front end

Menus are TOML files rendered by the `rugen-ui` engine; no code needed to
re-skin or re-arrange them. `rugen ui-gen` regenerates these defaults.

## Files

| file | purpose |
|---|---|
| `title.toml` | main menu (data: `menu[]`, `news[]`, `profile`, `version`) |
| `options.toml` | settings (data: `rows[]` with `label`/`value`/`press`/`left`/`right`, `capture`) |
| `news.json` | the three feature panels: `title`, `subtitle`, `image` |
| `*.webp` | art. **Prefer WebP** (lossless or lossy) — a quarter the size of PNG. PNG also works. |

Assets are also addressable as SFF sprites: `image = "sff:100,0"` (system.sff).

## Layout

Everything is laid out in a virtual **1280x720** space, scaled to the window
with letterboxing; pixel art is scaled nearest-neighbour, so draw assets at
half size and show them with `scale = 2` (or `w`/`h` doubled).

A node has `kind` = `panel` | `image` | `text` | `button` | `vbox` | `hbox` |
`list` | `grid`, a position (`x`, `y`, `w`, `h`, numbers or `"50%"`), an
`anchor` (`tl` `tr` `bl` `br` `center` `top` `bottom` `left` `right`), and
`children`. Containers stack children with `gap`; `items = "news"`
instances the children once per array element, exposing `{item.*}` and
`{index}`. `list`/`grid` scroll (mouse wheel, focus follows); `grid` takes
`columns`, `cell_w`, `cell_h`.

Looks: `image` / `image_focus`, `nine = [l, t, r, b]` for 9-slice panels,
`color` / `color_focus` solid fills, `skew` for slanted shapes, `alpha`,
`enter = "left" | "right" | "up" | "down" | "fade"` with `enter_time` /
`enter_delay` (ticks), `hover_scale`.

Text: `text` with `{binds}`, `font` (system.def font index), `bank`
(palette), `scale`, `align`, `pad` (inset for left/right aligned text).

Buttons: `on_press`, `on_left`, `on_right` are action strings the engine
understands (`mode:arcade`, `mode:options`, `netplay`, `quit`, `back`,
`adjust:<row>:<±1>`, `open:keys1`). Buttons are reachable by keyboard /
gamepad (spatial navigation) and mouse (hover focuses, click presses).

Binds use dotted paths: `{profile.name}`, `{news.0.title}`, `{item.label}`.
`visible = "capture.active"` hides a node when the value is false/empty;
`"!path"` negates.
"#;

/// Generate `motif/ui/*` (assets, screens, news, README).
pub fn generate(ttf: &[u8], motif: &Path) -> anyhow::Result<()> {
    let ui = motif.join("ui");
    std::fs::create_dir_all(&ui)?;
    let style = |size, italic| Style {
        size,
        italic,
        condense: 0.9,
        rim: true,
        highlight: true,
    };
    let logo_gen = Generator::new(ttf, style(44, 3))?;
    let caption_gen = Generator::new(ttf, style(20, 3))?;
    write_webp(&ui.join("bg.webp"), &backdrop())?;
    write_webp(&ui.join("logo.webp"), &logo(&logo_gen))?;
    write_webp(&ui.join("button.webp"), &button(false))?;
    write_webp(&ui.join("button_lit.webp"), &button(true))?;
    write_webp(&ui.join("panel.webp"), &panel())?;
    write_webp(&ui.join("row.webp"), &row(false))?;
    write_webp(&ui.join("row_lit.webp"), &row(true))?;
    write_webp(&ui.join("avatar.webp"), &avatar())?;
    write_webp(
        &ui.join("news1.webp"),
        &news_art(
            &caption_gen,
            244,
            129,
            [40, 70, 140, 255],
            [120, 40, 90, 255],
            "RUGEN",
        ),
    )?;
    write_webp(
        &ui.join("news2.webp"),
        &news_art(
            &caption_gen,
            114,
            60,
            [30, 100, 90, 255],
            [20, 40, 70, 255],
            "ARENA",
        ),
    )?;
    write_webp(
        &ui.join("news3.webp"),
        &news_art(
            &caption_gen,
            114,
            60,
            [140, 60, 40, 255],
            [60, 20, 50, 255],
            "ONLINE",
        ),
    )?;
    std::fs::write(ui.join("title.toml"), TITLE_TOML)?;
    std::fs::write(ui.join("options.toml"), OPTIONS_TOML)?;
    std::fs::write(ui.join("README.md"), README)?;
    let news = motif.join("news.json");
    if !news.exists() {
        std::fs::write(news, NEWS_JSON)?;
    }
    Ok(())
}
