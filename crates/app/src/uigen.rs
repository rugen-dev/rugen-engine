//! `rugen ui-gen`: writes the "Neon Dojo" front-end into a theme dir
//! (`motif/themes/dojo/ui/` by default) — WebP assets drawn with the
//! palette (see `kit.rs`), the `title.toml` / `options.toml` /
//! `select.toml` screens and a README for modders. Fonts, the CC0 kit,
//! `palette.gpl` and `news.json` stay shared in `motif/`; hero art dropped
//! into `motif/art/` is pixelized into `<theme>/ui/art/`.

use std::path::Path;

use crate::fontgen::{self, Generator, Style};

use crate::kit;
use crate::pixelize;

const SELECT_TOML: &str = r##"# rugen character select — Neon Dojo roster. Coordinates are 1280x720.
# Data: title, prompt, room, cells[] (name thumb badge broken p1 p2 hover index),
# shown{name author art verdict badge stats present}, p1/p2 (same shape), versus,
# ready, searching, query, sort, count, stage{name thumb has_thumb}, profile, rating.
background = [0.05, 0.06, 0.1, 1.0]

[[node]]
kind = "image"
image = "ui/bg.webp"
w = 1280
h = 720

# header
[[node]]
kind = "text"
text = "{title}"
x = 40
y = 16
font = 3
bank = 0
scale = 2
enter = "down"

[[node]]
kind = "text"
text = "{prompt}"
x = 470
y = 28
font = 1
bank = 4
scale = 2

# big art plate (left column)
[[node]]
kind = "image"
id = "art"
image = "ui/art_frame.webp"
x = 40
y = 68
w = 400
h = 440
enter = "left"
enter_time = 20

[[node]]
kind = "image"
image = "{shown.art}"
x = 60
y = 84
h = 404
visible = "shown.present"
enter = "left"
enter_time = 20

# info strip under the plate
[[node]]
kind = "panel"
image = "ui/plate_lit.webp"
x = 40
y = 516
w = 400
h = 44
enter = "left"
enter_time = 22

  [[node.children]]
  kind = "text"
  text = "{shown.name}"
  anchor = "left"
  x = 20
  font = 2
  bank = 7
  scale = 2

[[node]]
kind = "panel"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 40
y = 566
w = 400
h = 108
enter = "left"
enter_time = 24

  [[node.children]]
  kind = "text"
  text = "BY {shown.author}"
  x = 14
  y = 10
  font = 1
  bank = 6
  scale = 2

  [[node.children]]
  kind = "image"
  image = "{shown.badge}"
  x = 14
  y = 36
  w = 16
  h = 16
  visible = "shown.badge"

  [[node.children]]
  kind = "text"
  text = "{shown.verdict}"
  x = 38
  y = 34
  font = 1
  bank = 4
  scale = 2

  [[node.children]]
  kind = "text"
  text = "{shown.stats}"
  x = 14
  y = 58
  font = 1
  bank = 7
  scale = 2

  [[node.children]]
  kind = "text"
  text = "P1 {p1.name}   P2 {p2.name}"
  x = 14
  y = 82
  font = 1
  bank = 5
  scale = 2
  visible = "versus"

# roster grid
[[node]]
kind = "panel"
color = [0.05, 0.06, 0.12, 0.8]
x = 466
y = 62
w = 780
h = 330

[[node]]
kind = "grid"
id = "cells"
x = 470
y = 66
w = 772
h = 322
columns = 10
cell_w = 77
cell_h = 77
items = "cells"
enter = "right"
enter_time = 20

  [[node.children]]
  kind = "button"
  w = 76
  h = 76
  image = "ui/cell.webp"
  image_focus = "ui/cell_lit.webp"
  on_press = "pick:{item.index}"

    [[node.children.children]]
    kind = "image"
    image = "{item.thumb}"
    x = 6
    y = 6
    w = 64
    h = 64
    visible = "item.thumb"

    [[node.children.children]]
    kind = "panel"
    color = [0.0, 0.0, 0.0, 0.65]
    x = 6
    y = 6
    w = 64
    h = 64
    visible = "item.broken"

    [[node.children.children]]
    kind = "image"
    image = "ui/cell_p1.webp"
    w = 76
    h = 76
    visible = "item.p1"

    [[node.children.children]]
    kind = "image"
    image = "ui/cell_p2.webp"
    w = 76
    h = 76
    visible = "item.p2"

    [[node.children.children]]
    kind = "image"
    image = "{item.badge}"
    anchor = "tr"
    x = 5
    y = 5
    w = 14
    h = 14
    visible = "item.badge"

# grid toolbar
[[node]]
kind = "text"
text = "{count}"
x = 474
y = 406
font = 1
bank = 6
scale = 2

[[node]]
kind = "button"
id = "search"
x = 748
y = 400
w = 130
h = 32
image = "ui/plate.webp"
image_focus = "ui/plate_lit.webp"
text = "SEARCH"
font = 1
bank = 7
scale = 2
on_press = "search"

[[node]]
kind = "button"
id = "sort"
x = 886
y = 400
w = 214
h = 32
image = "ui/plate.webp"
image_focus = "ui/plate_lit.webp"
text = "SORT: {sort}"
font = 1
bank = 7
scale = 2
on_press = "sort"

[[node]]
kind = "button"
id = "random"
x = 1108
y = 400
w = 134
h = 32
image = "ui/plate.webp"
image_focus = "ui/plate_lit.webp"
text = "RANDOM"
font = 1
bank = 7
scale = 2
on_press = "random"

# player card (skewed, big gold number)
[[node]]
kind = "image"
image = "ui/card.webp"
x = 466
y = 448
w = 444
h = 120
enter = "up"
enter_time = 20
enter_delay = 4

[[node]]
kind = "text"
text = "{profile.name}"
x = 492
y = 458
font = 1
bank = 2
scale = 2

[[node]]
kind = "text"
text = "{profile.wins}"
x = 492
y = 480
font = 3
bank = 3
scale = 3

[[node]]
kind = "text"
text = "WINS"
x = 492
y = 538
font = 1
bank = 6
scale = 2

[[node]]
kind = "image"
image = "{profile.avatar}"
x = 818
y = 462
w = 56
h = 56

[[node]]
kind = "text"
text = "LEVEL {profile.level}"
x = 636
y = 470
font = 1
bank = 4
scale = 2

[[node]]
kind = "text"
text = "{profile.losses} LOSSES"
x = 636
y = 496
font = 1
bank = 6
scale = 2

[[node]]
kind = "text"
text = "RATING {rating}"
x = 636
y = 522
font = 1
bank = 7
scale = 2

# stage strip
[[node]]
kind = "panel"
image = "ui/panel.webp"
nine = [8, 8, 8, 8]
x = 920
y = 448
w = 322
h = 120
enter = "up"
enter_time = 20

  [[node.children]]
  kind = "text"
  text = "STAGE"
  x = 12
  y = 8
  font = 1
  bank = 4
  scale = 2

  [[node.children]]
  kind = "image"
  image = "{stage.thumb}"
  x = 12
  y = 32
  w = 120
  h = 68
  visible = "stage.has_thumb"

  [[node.children]]
  kind = "panel"
  color = [0.08, 0.1, 0.18, 1.0]
  x = 12
  y = 32
  w = 120
  h = 68
  visible = "!stage.has_thumb"

  [[node.children]]
  kind = "text"
  text = "{stage.name}"
  x = 144
  y = 34
  w = 170
  font = 1
  bank = 7
  scale = 2

  [[node.children]]
  kind = "button"
  id = "stage_prev"
  x = 144
  y = 66
  w = 44
  h = 32
  image = "ui/plate.webp"
  image_focus = "ui/plate_lit.webp"
  text = "<"
  font = 2
  bank = 7
  scale = 2
  on_press = "stage:prev"

  [[node.children]]
  kind = "button"
  id = "stage_next"
  x = 194
  y = 66
  w = 44
  h = 32
  image = "ui/plate.webp"
  image_focus = "ui/plate_lit.webp"
  text = ">"
  font = 2
  bank = 7
  scale = 2
  on_press = "stage:next"

  [[node.children]]
  kind = "button"
  id = "stage_random"
  x = 244
  y = 66
  w = 68
  h = 32
  image = "ui/plate.webp"
  image_focus = "ui/plate_lit.webp"
  text = "RND"
  font = 1
  bank = 7
  scale = 2
  on_press = "stage:random"

# confirm / back
[[node]]
kind = "button"
id = "confirm"
x = 920
y = 598
w = 322
h = 54
image = "ui/button_lit.webp"
image_focus = "ui/button_lit.webp"
text = "CONFIRM"
font = 2
bank = 7
bank_focus = 4
scale = 2
on_press = "confirm"
visible = "ready"
hover_scale = 1.03

[[node]]
kind = "button"
id = "back"
x = 466
y = 598
w = 200
h = 54
image = "ui/button.webp"
image_focus = "ui/button_lit.webp"
text = "BACK"
font = 2
bank = 7
bank_focus = 4
scale = 2
on_press = "back"

# footer bar
[[node]]
kind = "panel"
color = [0.04, 0.05, 0.08, 1.0]
anchor = "bl"
w = 1280
h = 30

[[node]]
kind = "panel"
color = [0.63, 0.07, 0.17, 1.0]
anchor = "bl"
y = 30
w = 1280
h = 2

[[node]]
kind = "text"
text = "ROOM {room}     {count}"
anchor = "bl"
x = 24
y = 8
font = 1
bank = 6
scale = 2

[[node]]
kind = "text"
text = "ENTER  PICK     ESC  BACK"
anchor = "br"
x = 24
y = 8
font = 1
bank = 6
scale = 2
"##;

const TITLE_TOML: &str = r##"# rugen title screen — Neon Dojo. Edit freely, see README.md. Coordinates are 1280x720.
background = [0.05, 0.06, 0.1, 1.0]

# background: base gloss, slash layer, floor — each slides in at its own pace
[[node]]
kind = "image"
image = "ui/bg_sky.webp"
w = 1280
h = 720
enter = "fade"
enter_time = 24

[[node]]
kind = "image"
image = "ui/bg_far.webp"
w = 1280
h = 720
enter = "right"
enter_time = 30

[[node]]
kind = "image"
image = "ui/bg_near.webp"
w = 1280
h = 720
enter = "up"
enter_time = 22

[[node]]
kind = "image"
id = "logo"
image = "ui/logo.webp"
x = 56
y = 36
scale = 2
enter = "down"
enter_time = 22

# profile strip (skewed slab, red avatar square)
[[node]]
kind = "image"
image = "ui/strip.webp"
anchor = "tr"
x = 24
y = 28
w = 560
h = 76
enter = "right"
enter_time = 20

[[node]]
kind = "image"
image = "{profile.avatar}"
anchor = "tr"
x = 514
y = 38
w = 56
h = 56
enter = "right"
enter_time = 20

[[node]]
kind = "text"
text = "{profile.name}"
anchor = "tr"
x = 24
y = 38
w = 470
font = 2
bank = 7
scale = 2
align = "left"
pad = 0
enter = "right"
enter_time = 20

[[node]]
kind = "text"
text = "LEVEL {profile.level}   {profile.shards} SHARDS   {profile.wins}W {profile.losses}L"
anchor = "tr"
x = 24
y = 72
w = 470
font = 1
bank = 4
scale = 2
align = "left"
pad = 0
enter = "right"
enter_time = 20

# main menu: one slanted button per entry of `menu`
[[node]]
kind = "vbox"
id = "menu"
x = 60
y = 196
w = 400
gap = 8
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
  bank_focus = 7
  scale = 2
  align = "left"
  pad = 28
  hover_scale = 1.03
  on_press = "{item.action}"

# featured panel (cut corner, eyebrow label)
[[node]]
kind = "image"
image = "{news.0.image}"
x = 520
y = 196
w = 488
h = 258
enter = "up"
enter_time = 22
enter_delay = 4

[[node]]
kind = "panel"
color = [0.04, 0.05, 0.09, 0.92]
x = 520
y = 400
w = 488
h = 54
enter = "up"
enter_time = 22
enter_delay = 4

[[node]]
kind = "text"
text = "FEATURED"
x = 534
y = 406
font = 1
bank = 2
scale = 2
enter = "up"
enter_time = 22
enter_delay = 4

[[node]]
kind = "text"
text = "{news.0.title}"
x = 534
y = 426
font = 2
bank = 7
scale = 2
enter = "up"
enter_time = 22
enter_delay = 4

[[node]]
kind = "text"
text = "{news.0.subtitle}"
x = 520
y = 464
w = 488
font = 1
bank = 6
scale = 2
enter = "up"
enter_time = 22
enter_delay = 6

# two smaller tiles
[[node]]
kind = "image"
image = "{news.1.image}"
x = 1024
y = 196
w = 228
h = 120
enter = "up"
enter_time = 22
enter_delay = 8

[[node]]
kind = "text"
text = "{news.1.title}"
x = 1024
y = 322
font = 1
bank = 7
scale = 2
enter = "up"
enter_time = 22
enter_delay = 8

[[node]]
kind = "image"
image = "{news.2.image}"
x = 1024
y = 352
w = 228
h = 120
enter = "up"
enter_time = 22
enter_delay = 12

[[node]]
kind = "text"
text = "{news.2.title}"
x = 1024
y = 478
font = 1
bank = 7
scale = 2
enter = "up"
enter_time = 22
enter_delay = 12

# footer bar with a red rule
[[node]]
kind = "panel"
color = [0.04, 0.05, 0.08, 1.0]
anchor = "bl"
w = 1280
h = 30

[[node]]
kind = "panel"
color = [0.63, 0.07, 0.17, 1.0]
anchor = "bl"
y = 30
w = 1280
h = 2

[[node]]
kind = "text"
text = "RUGEN {version}"
anchor = "bl"
x = 24
y = 8
font = 1
bank = 6
scale = 2

[[node]]
kind = "text"
text = "ENTER / CLICK  SELECT     ESC  BACK     F11  FULLSCREEN"
anchor = "br"
x = 24
y = 8
font = 1
bank = 6
scale = 2
"##;

const NEWS_JSON: &str = r#"[
  { "title": "WELCOME TO RUGEN", "subtitle": "Drop .zip / .rar characters into chars/ and play.", "image": "ui/news1.webp" },
  { "title": "ARENAS", "subtitle": "Platforms, blast zones, stocks.", "image": "ui/news2.webp" },
  { "title": "NETPLAY", "subtitle": "Rollback online, room codes.", "image": "ui/news3.webp" }
]
"#;

const README: &str = r#"# ui/ — the data-driven front end (theme "dojo")

Menus are TOML files rendered by the `rugen-ui` engine; no code needed to
re-skin or re-arrange them. A theme lives in `motif/themes/<name>/` (this
folder plus `system.def` / `fight.def` / `.sff`); fonts, the CC0 kit, the
palette, `news.json` and `art/` are shared in `motif/`. `rugen ui-gen
[--out motif/themes/<name>]` regenerates these defaults for the Neon Dojo look.

## Files

| file | purpose |
|---|---|
| `title.toml` | main menu (data: `menu[]`, `news[]`, `profile`, `version`) |
| `options.toml` | settings in tabs (see "Options and controls" below) |
| `controls.toml` | key bindings page, test pad, presets |
| `select.toml` | roster / character select (data: `cells[]`, `shown`, `p1`, `p2`, `stage`, `profile`, `rating`, `prompt`, `count`, `sort`, `ready`); actions `pick:<i>`, `confirm`, `random`, `search`, `sort`, `stage:prev|next|random`, `back` |
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
`adjust:<row>:<±1>`, `open:keys1` opens the controls page). Buttons are reachable by keyboard /
gamepad (spatial navigation) and mouse (hover focuses, click presses).

Binds use dotted paths: `{profile.name}`, `{news.0.title}`, `{item.label}`.
`visible = "capture.active"` hides a node when the value is false/empty;
`"!path"` negates.
"#;

/// Generate a theme's `ui/*` (assets, screens, README) into `theme`
/// (`motif/themes/<name>`); fonts, the CC0 kit, the palette, `news.json` and
/// `art/` are shared and live in `motif`. `fallback_ttf` is only used when a
/// role font is missing from `motif/font/src/`.
pub fn generate(fallback_ttf: Option<&[u8]>, motif: &Path, theme: &Path) -> anyhow::Result<()> {
    let ui = theme.join("ui");
    std::fs::create_dir_all(&ui)?;
    std::fs::write(motif.join("palette.gpl"), pixelize::gpl())?;
    fontgen::generate_roles(motif, fallback_ttf)?;
    let title_role = &fontgen::roles()[0];
    let title_ttf = fontgen::role_ttf(motif, title_role, fallback_ttf)?;
    let logo_gen = Generator::new(
        &title_ttf,
        Style {
            size: 32,
            italic: 0,
            condense: 1.0,
            rim: false,
            highlight: false,
        },
    )?;
    let caption_gen = Generator::new(&title_ttf, title_role.style)?;
    kit::import(motif, theme)?;
    let w = |name: &str, c: &crate::screenpack::Canvas| pixelize::write_webp(&ui.join(name), c);
    w("bg.webp", &kit::bg_composite())?;
    w("bg_sky.webp", &kit::bg_sky())?;
    w("bg_far.webp", &kit::bg_far())?;
    w("bg_near.webp", &kit::bg_near())?;
    w("logo.webp", &kit::logo(&logo_gen))?;
    w("button.webp", &kit::button(motif, false)?)?;
    w("button_lit.webp", &kit::button(motif, true)?)?;
    w("panel.webp", &kit::panel(motif)?)?;
    w("row.webp", &kit::row(motif, false)?)?;
    w("row_lit.webp", &kit::row(motif, true)?)?;
    w("avatar.webp", &kit::avatar(motif)?)?;
    w("strip.webp", &kit::strip(motif)?)?;
    w("cell.webp", &kit::cell(motif, "normal")?)?;
    w("cell_lit.webp", &kit::cell(motif, "lit")?)?;
    w("cell_p1.webp", &kit::cell(motif, "p1")?)?;
    w("cell_p2.webp", &kit::cell(motif, "p2")?)?;
    w("badge_ok.webp", &kit::badge(pixelize::GREEN[2]))?;
    w("badge_warn.webp", &kit::badge(pixelize::GOLD[3]))?;
    w("badge_bad.webp", &kit::badge(pixelize::RED[2]))?;
    w("art_frame.webp", &kit::art_frame(motif)?)?;
    w("card.webp", &kit::card(motif)?)?;
    w("plate.webp", &kit::plate(motif, false)?)?;
    w("plate_lit.webp", &kit::plate(motif, true)?)?;
    w(
        "news1.webp",
        &kit::news_arena(&caption_gen, 244, 129, "RUGEN"),
    )?;
    w(
        "news2.webp",
        &kit::news_platforms(&caption_gen, 114, 60, "ARENAS"),
    )?;
    w(
        "news3.webp",
        &kit::news_online(&caption_gen, 114, 60, "ONLINE"),
    )?;
    pixelize_art(motif, theme)?;
    std::fs::write(ui.join("title.toml"), TITLE_TOML)?;
    crate::settings_gen::dojo(&ui, &title_ttf)?;
    std::fs::write(ui.join("select.toml"), SELECT_TOML)?;
    std::fs::write(
        ui.join("README.md"),
        format!("{README}{}", crate::settings_gen::README),
    )?;
    let news = motif.join("news.json");
    if !news.exists() {
        std::fs::write(news, NEWS_JSON)?;
    }
    Ok(())
}

/// Hero images dropped into `motif/art/` (png / webp) → `<theme>/ui/art/<name>.webp`,
/// downscaled to the 640-px grid and quantised to the palette.
pub fn pixelize_art(motif: &Path, theme: &Path) -> anyhow::Result<usize> {
    let src = motif.join("art");
    let Ok(rd) = std::fs::read_dir(&src) else {
        return Ok(0);
    };
    let out = theme.join("ui").join("art");
    let pal = pixelize::full_palette();
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .map(|x| x.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "png" && ext != "webp" {
            continue;
        }
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let dst = out.join(format!("{stem}.webp"));
        match pixelize::pixelize_file(&p, &dst, Some(640), &pal, true, None) {
            Ok((w, h)) => {
                log::info!("pixelized {} -> {} ({w}x{h})", p.display(), dst.display());
                n += 1;
            }
            Err(err) => log::warn!("{}: {err}", p.display()),
        }
    }
    Ok(n)
}
