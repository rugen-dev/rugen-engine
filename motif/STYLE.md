# rugen art direction

Pixel arena fighter. Night sky, slanted maroon panels, gold accents, 1-px
rims, 45° light shafts. Brawlhalla composition, retro execution: everything is
drawn on a **640x360 pixel canvas and shown at 2x** (the screens' virtual
space is 1280x720, so a 1x asset of `w x h` pixels is placed with `w*2 x h*2`
or `scale = 2`). No anti-aliasing, no soft glows, no smooth gradients:
dithered gradients (4x4 Bayer), hard highlights, dark outlines.

## Palette — `palette.gpl`

Five ramps plus black and white, dark → light:

| ramp | use |
|---|---|
| `navy` | backgrounds, panels, cells, rows |
| `maroon` | buttons, name plates, P1 accents, selected rows |
| `gold` | highlights, frames of the focused element, the logo, bars |
| `steel` | neutral text, icons, rims |
| `teal` | online / network accents |
| `red` `green` `blue` | status (broken / verified / P2) |

Load it in Aseprite or GIMP (`File → Open` the `.gpl`). Keep art in these
colours; `rugen pixelize` snaps anything else to them.

## Where things come from

| asset | source |
|---|---|
| `font/rugen-*.def` | `font/src/*.ttf` (OFL pixel fonts) through `rugen fontgen` roles: title = Press Start 2P 16 px, menu = Silkscreen Bold 16 px, small = Press Start 2P 8 px, numbers = Press Start 2P 16 px |
| `ui/kit/*.webp` | `kit/kenney-pixel-adventure/` (CC0) tiles recoloured by luminance onto a ramp (`rugen kit-import`) |
| `ui/button*.webp`, `panel`, `row*`, `plate*`, `cell*`, `art_frame`, `card` | kit tiles 9-sliced (edges repeated, never stretched) and sheared into parallelograms |
| `ui/bg_sky.webp` `bg_far.webp` `bg_near.webp` | drawn: dithered sky + moon + stars, two ridge silhouettes with fog, arena floor + torii gates + light shafts; `bg.webp` is the merge |
| `ui/logo.webp` | the title font at 32 px, gold two-tone bevel, slash highlight, navy drop shadow |
| `ui/news*.webp` | drawn placeholders (dusk arena, platforms, network) — replace with hero art |
| `ui/art/*.webp` | anything you put in `art/`, pixelized |

Everything regenerates with `rugen ui-gen`; hand-edit the WebPs afterwards if
you want, they are just files.

## Hero art

AI-generated or painted images go to `motif/art/<name>.png|webp` and come out
as `motif/ui/art/<name>.webp` at 640 px wide, palette-quantised and dithered.
Reference them from the TOML screens (`image = "ui/art/<name>.webp"`) or
from `news.json`.

Sizes that fit the default screens (final pixel size, so source images at
2–4x these are ideal):

| slot | pixels |
|---|---|
| featured news panel | 244 x 129 |
| small news panels | 114 x 60 |
| big character plate (select) | 200 x 200 |
| full-screen background | 640 x 360 |

Prompt scaffolding that lands in the palette after pixelizing:

> `<subject>`, night sky, deep navy and maroon, warm gold rim light from the
> right, strong silhouette, flat shading, high contrast, wide shot, no text,
> no watermark

For character splash art ask for "bust shot, three-quarter view, looking
left, dark background"; for stages "wide establishing shot, horizon in the
lower third".

## Composition rules

- Left column: logo top-left, menu as a stack of slanted buttons.
- Right side: panels for content; the selected item is maroon-lit, others
  dark navy.
- One accent per screen (the moon, a plate, a focused frame) — not three.
- Text: title font for headings only, menu font for buttons and names, small
  font for everything else; white (bank 7) on dark, gold (bank 4) for values
  and the focused label, grey (bank 6) for hints.
- Keep 1-px dark outlines on everything that sits on the background.
