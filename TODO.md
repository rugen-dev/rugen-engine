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

- [x] **Arena AI merged** (2da53a2): self-KOs on Ink Arena 230 → 10 over 80
  CPU matches at level 4.
- [x] **Server sim must match 0.6.2.** (rugen-server c1c106e, f84d97f)
  The server re-simulates only matches whose clients ran exactly the
  `rugen-sim` version in its Cargo.lock (build.rs); any other version is
  noted `unverified (version)` and stays an ordinary dispute (no
  settlement, flag or block). Players of different versions never meet
  (queue by `User-Agent`, rooms refuse them). 0.6.0 / 0.6.1 results stay
  as their reports leave them. The resim applies the room's TIME / ROUNDS
  like 0.6.2 clients. Still to do at release: bump the engine tag, rebuild,
  deploy the server first.
  Original note: 0.6.2 changes the simulation (one-way
  platforms, `airjump.neu` one-value fix), and the server re-simulates ranked
  matches with the engine tag it was built against (v0.6.0). Before
  releasing: check how the resim verifier treats a client whose sim differs
  (it must not flag honest 0.6.2 players as desyncs or liars), then tag
  v0.6.2, rebuild the server against it, deploy the server first, then
  publish the client. Decide what happens to 0.6.0 / 0.6.1 clients' ranked
  results (verify with the matching sim, or ask them to update).
- [x] **Fresh install, every package.** (b0a1eb1, 346e202, b9f25d6)
  Packages ship MUGEN 1.1b1's base data (common1.cns, fightfx, sounds)
  via scripts/stage-data.sh (no fighter loaded without it), the binary
  needs glibc 2.34 instead of 2.43, explicit .deb / .rpm dependencies; the
  camera / screen bounds now follow fighter units (CPU left the screen on
  HD stages). Install the rpm, the deb and the
  tarball in a clean user (no `~/.local/share/rugen`, no `rugen.toml`):
  first launch, drop a zip into `chars/` and a stage into `stages/`,
  conversion, title, every mode, options, save, quit, relaunch.
- [x] **Smoke test every screen in both themes** (8c5102e, ae100d3), keyboard, gamepad and
  mouse: title, all options tabs, controls, fighter select, stage select,
  versus, fight, pause and move list, result, every online screen, profile,
  leaderboard. No overlap, no clipped text, no dead button, no crash.
- [x] **Online end to end** (1239226, ae100d3; rugen-server ac67861,
  52a845e), two clients against a local server built from main: quick
  match, ranked, room by code, spectator, flee, rematch, an account created
  and logged in.
- [x] **No error or warning spam** (0d81b05) in a normal session log (`RUST_LOG=info`);
  character warnings stay at debug.
- [ ] **Release 0.6.2** once the boxes above are ticked: version bump, tag,
  release notes in the usual order, README roadmap boxes for 0.6.2 ticked,
  docs updated, download page checked.

## Bugs

- [x] **Mixed localcoords between fighters.** (70c1393) P2Dist, collision and push
  compare raw positions of two fighters whose localcoord (or FIGHTER SIZE)
  differ; convert through `local_scale` like the camera now does.
  Push and Clsn already converted; P2Dist / P2BodyDist, Parent/RootDist,
  the nearest enemy, auto-turn, guard distance and hit order now go
  through `World::rel_pos` (sim change).
- [x] **Offline replays do not record the round time** (2f4ee5d) (online matches are
  not recorded): a replay played after changing ROUND TIME may diverge.
- [x] **Windows / macOS packages lack data/.** (7d251e6) release.yml (manual run)
  does not ship the MUGEN base data that scripts/stage-data.sh adds to the
  Linux packages.

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
  Status (6cc4d37): not reproduced on 0.6.2. Goku's ki blast, Super Energy
  Blast, Kamehameha, explosions and auras blend correctly (Linux / Vulkan,
  training and Hidden Sand, quick play and through the menus with stage
  previews; 4000-frame CPU soaks logging every opaque draw found no lost
  AIR blend and no effect drawn opaque; Naruto / Sasuke too). Goku's SFF is
  v2.01 with index 0 at alpha 0 in every palette, PNG32 sprites keep their
  alpha, the `.rgc` pack is pixel-exact. The one blend mapping every path
  shares is now pinned by a test; fixed on the way: an explod alpha
  computed below 0 snapped back to full strength, `rugen view` drew every
  frame opaque. Not checked: blend interpolation (Goku has none). Also
  noticed: PalFX adds after multiplying (MUGEN adds first). Reopen with the
  move, stage, OS / GPU and a screenshot.
- [x] **Tutorial text overlap.** (b1428fb) The step title and the hint line overlap
  ("JUMP" over "PRESS ↑").
- [x] **Unsaved settings on window close.** (45bcd99) Closing the window with unsaved
  options or controls loses them silently; ask SAVE / DISCARD / CANCEL, as
  leaving the screen already does.
- [x] **Stage preview palettes leak.** (f9af2f0) Palettes of replaced stage previews
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

The *Planned* list in the README, with an approach for each. Screen
mockups (ink theme) for every item with a UI are on the design canvas
https://claude.ai/artifact/EzYKJY1QXwfNLLpncKHKrV, with local copies in
`~/.cache/rugen-work/roadmap-mockups/`; build both themes from them. Build in this
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

### Live watching (shared base for replays and brackets)
- [ ] **One match format: header + input stream.** Header: rugen version,
  fighter and stage fingerprints, fighter size, rules, round seed. Body: the
  confirmed inputs per frame. Saved replays stay `.json`; the live stream is
  compact binary (~2 bytes per player per frame, ~15 KB a minute).
- [ ] **Server keeps the input log of every live match.** The relay already
  forwards the inputs; keep a copy, fan out to spectators from the server (not
  from P1), drop the log when the match ends unless it is shared.
- [ ] **Watch any live match, even mid-match.** WATCH downloads missing
  fighters and stage through the package sync, fetches the backlog from frame
  0, fast-forwards the sim without drawing ("CATCHING UP…"), then follows
  live a few seconds behind the players (smooth start, no relaying to a
  player in tournaments).
- [ ] **WATCH from the lobby and the bracket.** Live rooms show WATCH in the
  lobby; the bracket's live match opens it on ENTER.

### Replays
- [ ] **Shareable replays.** Keep a finished match's input log on the server
  (see *Live watching*), get a short code; WATCH → BY CODE downloads and plays it,
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
