# TODO

Open work for the next pass. Each item needs: a fix in both themes where it
touches UI (ink and dojo), tests where the sim or a parser changes,
screenshots checked by eye, `cargo fmt --all --check`, `cargo clippy
--workspace --all-targets -- -D warnings` and `cargo test --workspace` before
each commit. Tick the box and add the commit hash when done.

## Bugs

- [ ] **Black boxes around effects (top priority).** Goku's ki blasts (and
  likely other effects) show their black background instead of blending, as
  MUGEN does. The renderer has Add / Sub blends and the AIR parser reads
  `A`, so the loss is somewhere on the way. Check each path with Goku:
  - Explod `trans` / `alpha`, and the `Trans` sctrl on helpers;
  - Projectile animations (`projanim`, `projhitanim`), which may not carry
    the AIR frame blend;
  - AIR frame flags `A`, `A1`, `S`, `AS<n>D<n>`, and blend on interpolated
    frames;
  - SFF v2 sprites (Goku's `.sff` is v2): palette index 0 must be
    transparent, 32-bit or PNG sprites keep their alpha;
  - the `.rgc` conversion (lossless WebP / zstd): make sure no step
    flattens alpha or palette index 0.
  Compare a frame against the original files with `rugen view` before and
  after conversion. Add a regression test for the path that was broken.
- [ ] **Tutorial text overlap.** The step title and the hint line overlap
  ("JUMP" over "PRESS ↑").
- [ ] **Unsaved settings on window close.** Closing the window with unsaved
  options or controls loses them silently; ask SAVE / DISCARD / CANCEL, as
  leaving the screen already does.
- [ ] **Stage preview palettes leak.** Palettes of replaced stage previews
  on the stage select are never freed.

## Gameplay

- [ ] **Hit sparks and fighter size.** fightfx hit sparks are not zoomed with
  the fighter size, so they look large on chibi-sized fighters.
- [ ] **Ranked balance and fighter size.** The balance verdict is computed
  from content only and ignores the size factor (×1.3 against ÷1.3 changes
  reach). Add the size to the verdict inputs.
- [ ] **Arena clock.** Show m:ss in the arena HUD instead of MUGEN seconds
  ("97").
- [ ] **Arena zoom.** Fully zoomed out, fighters are about 25 px tall on Ink
  Arena; respect a minimum on-screen size or the user's zoom-out setting.

## Online

- [ ] **Host picks the stage.** Online rooms still keep the old stage strip
  on the fighter pick; move it to the new stage select screen, host picks,
  the other side watches the choice.

## UI

- [ ] **Mouse cursor in menus, alongside the keyboard.** The mouse was
  turned off (`[input] mouse = false`) because it fought the keyboard. Make
  it work and turn it back on by default:
  - hover moves the focus and click activates, on every screen (title,
    options, controls, select, stage select, online, pause);
  - the keyboard / pad always wins: a key press takes the focus back, and
    the cursor only takes over once the mouse actually moves (not on a
    stray event while it rests);
  - hide the cursor after a few seconds without mouse movement and during
    fights;
  - the wheel scrolls lists and the stage strip; sliders and toggles work by
    click and drag.
- [ ] **Held direction repeats in menus.** Holding ↑ / ↓ (or ← / →) moves
  only once; it should repeat: first repeat after about 0.4 s, then every
  ~0.08 s, on keyboard and pad, on every list, grid and slider (the stage
  strip already does this; share one repeat helper).
- [ ] **Pad labels outside fights.** Hints on menu screens (select
  `pick_key`, title) always show keyboard keys; switch to the pad's button
  names when the last input came from a gamepad, as fight overlays do.
- [ ] **Stage thumbnails.** Bundled ink stages look plain on the strip
  (first background sprite only); render a small real preview once and cache
  it.
- [ ] **Gamepad paging on the stage strip.** L1 isn't mapped; page with
  L1 / R1.
- [ ] **Controls dropdown arrow** sits just outside its box.
- [ ] **Help text** repeats the panel subtitle on some options tabs.
- [ ] **Online opponent card screenshot.** The opponent name on the online
  fighter pick is covered by a unit test only; check it with two clients.

## Docs and repo

- [ ] **Fresh screenshots** of the Dojo and Ink themes for the docs themes
  page and the README.
- [ ] **`motif/STYLE.md`** still describes the old pixel-art Dojo; update it
  to the current themes.
- [ ] **`stages/README.md`** still shows the old `[Arena]` numbers.
- [ ] **CONTRIBUTING.md** with the dual-licence clause ("contributions are
  licensed MIT OR Apache-2.0"), commit format, checks to run and "no
  copyrighted content in pull requests"; plus a pull-request template.
