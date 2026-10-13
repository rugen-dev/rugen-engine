# TODO

Open work for the next pass. Each item needs: a fix in both themes where it
touches UI (ink and dojo), tests where the sim or a parser changes,
screenshots checked by eye, `cargo fmt --all --check`, `cargo clippy
--workspace --all-targets -- -D warnings` and `cargo test --workspace` before
each commit. Tick the box and add the commit hash when done.

## Order for tonight

rugen gets shared publicly tomorrow. Work in this order and do not start a
later section while an earlier one has open boxes:

1. **Launch blockers**: every box under *Launch readiness* and *Bugs*.
2. **Polish**: *UI*, *Gameplay*, *Online*, *Docs and repo*.
3. **Roadmap features**: *Roadmap plan* below, in its order. Each one ships
   only when it is complete, tested and documented; a half-done feature stays
   on its branch, never on `main`.

## Launch readiness

- [ ] **Arena AI merged.** The `arena-ai` branch (worktree
  `~/.cache/rugen-work/wt-arenaai`) is reviewed, merged and its numbers
  checked: CPU fighters on Ink Arena almost never self-destruct.
- [ ] **Fresh install, every package.** Install the rpm, the deb and the
  tarball in a clean user (no `~/.local/share/rugen`, no `rugen.toml`):
  first launch, drop a zip into `chars/` and a stage into `stages/`,
  conversion, title, every mode, options, save, quit, relaunch.
- [ ] **Smoke test every screen in both themes**, keyboard, gamepad and
  mouse: title, all options tabs, controls, fighter select, stage select,
  versus, fight, pause and move list, result, every online screen, profile,
  leaderboard. No overlap, no clipped text, no dead button, no crash.
- [ ] **Online end to end** with two clients against play.rugen.dev: quick
  match, ranked, room by code, spectator, flee, rematch, an account created
  and logged in.
- [ ] **No error or warning spam** in a normal session log (`RUST_LOG=info`);
  character warnings stay at debug.
- [ ] **Release 0.6.2** once the boxes above are ticked: version bump, tag,
  release notes in the usual order, README roadmap boxes for 0.6.2 ticked,
  docs updated, download page checked.

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

## Roadmap plan

The *Planned* list in the README, with an approach for each. Build in this
order; each item is one branch, with tests, docs (rugen.dev page plus README
box) and screenshots.

### Training lab
- [ ] **Hitbox display.** Toggle in the Training pause menu and on a key
  (F1). Draw Clsn1 red and Clsn2 blue from the sim's current frame, after the
  sprite, with the fighter-size zoom applied. Off in every other mode.
- [ ] **Frame data.** For the last move, show startup / active / recovery and
  advantage on hit and block, measured live from the sim (first frame with a
  Clsn1, last one, return to ctrl). Small panel in the corner, both fighters.
- [ ] **Input display.** A scrolling column per player with the direction
  arrow and the bound button keys, the hold length in frames; reads the same
  input buffer the sim uses.
- [ ] **Record and playback dummy.** Training menu: RECORD (take control of
  the dummy for up to 10 s), PLAY, LOOP; dummy options BLOCK: none / all /
  after first hit, and a reversal slot. Recordings are input lists, so they
  are deterministic.

### Team modes
- [ ] **Turns.** Pick 1–4 fighters per side on fighter select; the winner of
  a round stays, keeps its life, the loser's next fighter comes in. Needs the
  select screen to pick several and the round code to swap a player.
- [ ] **Simul.** Two fighters per side on screen at once (P1 + partner CPU or
  P3). Needs 4-player support in the sim (targets, pushing, camera) and the
  `teammode` / `numpartner` triggers characters already check.
- [ ] **Tag.** Like Simul but one active fighter per side; a tag button
  swaps, the partner jumps in from the edge. Start with the Ikemen GO tag
  rules so tag-ready characters (Naruto and Sasuke carry tag code) behave.

### More modes
- [ ] **Survival.** One life bar against an endless line of random opponents;
  a little life back between fights; count of wins on the result screen.
- [ ] **Boss Rush.** The roster's `boss = 1` fighters (select.def) back to
  back; if none are flagged, the five strongest by the balance verdict.
- [ ] **Time Attack.** The arcade ladder with a run timer; best times saved
  per fighter in the profile.

### Storyboards
- [ ] **Intros and endings.** Play MUGEN `.def` storyboards: the screenpack
  intro and logo, each fighter's `intro.storyboard` before its first arcade
  fight and `ending.storyboard` after the last. Skip with START. Scenes,
  layers, BG, text and sound per the MUGEN storyboard format.

### Arena
- [ ] **Ledge grab.** When a falling fighter's hand reaches the edge of the
  deck while facing it, hang for up to 2 s with brief invulnerability; then
  up = climb, forward = roll in, jump = jump. Limited regrabs per airtime.
  The arena AI uses it for recovery.
- [ ] **Items.** `[Arena] items = 1` spawns pickups every 10–20 s on the deck
  and platforms: a throwable (projectile on throw), a heal and a power
  boost. Grab with a button near an item; deterministic spawn from the round
  seed for netplay.
- [ ] **Stage hazards.** Moving platforms (path and speed in `[Arena]`) and
  timed events (a platform that drops, a wind push), all deterministic.

### Online
- [ ] **Ranked seasons.** A season id and dates on the server; ratings soft
  reset at season start (pull toward 1200), last season's tier kept as a
  badge on the profile.
- [ ] **Per-fighter leaderboards.** Server keeps wins and rating per
  fingerprint; LEADERBOARD gets a FIGHTERS tab.
- [ ] **Tournament brackets.** In a room: 8 or 16 players, single
  elimination, the server seeds by rating and calls each match; spectators
  watch the bracket.

### Replays
- [ ] **Shareable replays.** Upload a finished replay to the server (small:
  inputs only), get a short code; WATCH → BY CODE downloads and plays it,
  fetching missing fighters through the existing package sync. Kept 30 days.

### Creators
- [ ] **User themes folder.** `themes/` next to `chars/`; any folder with a
  `system.def` shows up in INTERFACE; the game watches its files and reloads
  the screen on save, no rebuild.
- [ ] **Fighter debugger.** `rugen debug <fighter>`: pause, step one frame,
  show state number, anim, elem, vars, fvars, which controllers fired this
  frame and why; click a player to inspect.

### Reach
- [ ] **Translations (i18n).** Move every UI string into `motif/lang/en.toml`
  keyed by id; add `pt-BR`, `es`, `fr`, `it`, `de`, `pl`, `tr`, `id`, `fil`
  and `ja`. Add accented Latin glyphs to the generated fonts; Japanese needs
  its own pixel font (kana plus common kanji). Language picker in INTERFACE,
  default from the OS locale. Buttons must fit the longest language.
- [ ] **Steam Deck layout.** A 1280x800 profile: controller-first hints,
  bigger text, no hover-only actions, suspend and resume safe.
