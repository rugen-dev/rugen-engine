# ui/ — the data-driven front end (theme "dojo")

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

Screens are designed in a virtual **1280x720** space. The window is always
720 units high (wider windows get more width, narrower ones keep 1280 and get
more height), and top-level nodes sit in the 1280x720 design frame, centred,
unless they say otherwise with `dock`: `left` / `center` / `right` and `top` /
`middle` / `bottom` pin a node to that edge of the window, `fill` (or a
`"100%"` size) stretches it edge to edge (backgrounds, bands: pair with
`fit = "cover"` for art), and `stretch-x` keeps a panel's left edge and grows
its width by the extra room. Children of a filled or stretched node can dock
inside it the same way. Pixel art is scaled nearest-neighbour, so draw assets
at half size and show them with `scale = 2` (or `w`/`h` doubled).

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
