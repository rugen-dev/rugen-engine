# rugen art direction

rugen ships two UI themes, picked in OPTIONS → INTERFACE or with
`[video] theme = "ink" | "dojo"` in `rugen.toml`:

- **Shonen Ink** (`themes/ink/`, the default): a manga page.
- **Dojo** (`themes/dojo/`): a training hall at night.

Both are pixel art drawn in code and share the same screens, data and
behaviour; only the skin differs. The fight HUD (`fight.def`) and the
screenpack (`system.def`) come with each theme.

## Shared rules

- **Canvas.** Screens are laid out in a 1280x720 virtual space. Art is drawn
  at half resolution (640x360) and doubled with nearest neighbour, so a
  pixel is 2x2 on screen. No anti-aliasing, no soft glows, no smooth
  gradients: dithering or screentone, hard highlights, dark outlines.
- **Generated, then editable.** `rugen ui-gen --theme ink` and
  `rugen ui-gen --theme dojo` draw every WebP, write the fonts, the
  screenpack and copy the screen TOMLs into `motif/themes/<theme>/`. The
  sources are `crates/app/src/inkgen.rs` / `dojogen.rs` (art) and
  `crates/app/src/inkscreens/` / `dojoscreens/` (screens); the options and
  controls pages come from `crates/app/src/settings_gen.rs`. Hand-edit the
  output if you like, but a regeneration overwrites it: change the source.
  The output is reproducible: `generated_themes_match_the_committed_files`
  (`cargo test -p rugen`) fails when a generator change lands without its
  regenerated files (both themes and the flat `motif/font/`).
- **Fonts.** `font/src/` holds the two OFL pixel fonts (Press Start 2P and
  Jersey 10); `rugen fontgen` cuts them into role fonts (title, menu, small,
  numbers, code) with one colour bank per ink of each theme. Banks are listed
  in each theme's `README.md`.
- **One accent per screen.** A red stamp, a vermilion seal, the focused
  slab: one thing pulls the eye, everything else stays quiet.
- **Focus is always visible.** The focused element changes colour *and*
  shape (a slab grows, a frame thickens, a row gets a bar); the mouse
  hovers the same focus the keys move.
- **Hints name real keys.** Key caps show the player's own bindings, or the
  pad's button names once a pad was used (`hint.ok`, `hint.back`,
  `keys.*`).
- **Files missing in a theme** fall back to the flat `motif/` (fonts,
  `news.json`, the kit tiles, the brand icon).

## Shonen Ink

Paper, screentone dots, black ink panels with hard offset shadows, red
stamps and "!!" marks, speed-line fans, skewed slabs.

| colour | RGB | use |
|---|---|---|
| paper | 243 239 228 | page background, light panels |
| paper dark | 214 208 194 | hatching, idle tiles |
| ink | 17 17 17 | panels, slabs, outlines, text on paper |
| ink soft | 60 58 54 | secondary text |
| red | 224 32 32 | the accent: focus, P1, stamps, alerts |
| white | 255 255 255 | text on ink slabs, cards |

- Headings in the title font, slanted; menus are black slabs that turn red
  and grow when focused, with a `▶` and a one-line description under them.
- Panels: white or paper with a 3 px ink border and a hard ink (or red)
  shadow offset down-right. Screentone (`dots`, `hatch`) fills empty paper.
- Stamps (`stamp9`, `stamp_chapter`) for verdicts, READY!, ranks.
- Font banks on paper: 9 ink, 10 red, 11 paper (text on slabs), 8 soft grey.

## Dojo

Dark lacquer walls, wooden pillars, beams and plaques, washi sheets in
wooden frames, sumi type, vermilion hanko seals and a brass focus. The full
palette is `themes/dojo/palette.gpl` (written from `dojogen.rs`):

| ramp | use |
|---|---|
| lacquer 0-2 | the hall: page background, dark panels |
| wood 0-3 | frames, pillars, plaques (buttons) |
| washi 0-2 | paper sheets: content panels, cards |
| sumi, sumi soft | text on washi |
| vermilion 0-2 | the accent: seals, P1, the focused plaque |
| gold 0-2 | brass: the focus edge, P2, values |
| moss, amber, muted | status (verified / warning) and hints on lacquer |

- Text sits on washi in sumi, or on lacquer in washi / muted; buttons are
  wooden plaques that turn vermilion with a brass edge when focused.
- Frames are 9-sliced wood; sheets never touch the screen edge.
- Font banks: 12 sumi, 13 vermilion, 14 washi, 15 brass, 16 muted on
  lacquer, 17 soft sumi on washi. Body text uses Jersey (fonts 2, 3, 6),
  tags and numbers Press Start (1, 4).

## Hero art

News panels and featured art are plain images referenced from the screen
TOMLs or `news.json` (`"image": "ui/news1.webp"`). To bring outside art into
a theme's palette, run `rugen pixelize in.png out.webp --width 320
--palette motif/themes/dojo/palette.gpl --dither` and point the TOML at the
result. Keep compositions simple: one subject, strong silhouette, no text.
Character portraits come from the fighters themselves (sprite 9000,0 and the
select portraits), stage thumbnails are rendered from the stages.

## Stages

`motif/stages/` holds the bundled stages (`training`, `ink_dojo`,
`ink_arena`), drawn by `rugen stage-gen`. See
`stages/README.md` for the `[Arena]` section the arena uses.
