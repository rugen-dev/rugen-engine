# motif/ui — the data-driven front end

Menus are TOML files rendered by the `rugen-ui` engine; no code needed to
re-skin or re-arrange them. `rugen ui-gen` regenerates these defaults.

## Files

| file | purpose |
|---|---|
| `title.toml` | main menu (data: `menu[]`, `news[]`, `profile`, `version`) |
| `options.toml` | settings (data: `rows[]` with `label`/`value`/`press`/`left`/`right`, `capture`) |
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
`adjust:<row>:<±1>`, `open:keys1`). Buttons are reachable by keyboard /
gamepad (spatial navigation) and mouse (hover focuses, click presses).

Binds use dotted paths: `{profile.name}`, `{news.0.title}`, `{item.label}`.
`visible = "capture.active"` hides a node when the value is false/empty;
`"!path"` negates.
