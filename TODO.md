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
- [x] **Online camera determinism.** (d25a9e4) The camera is part of the sim, but
  online sessions used each player's own `[camera] zoom_out / zoom_in` while
  `rugen-server`'s re-simulation uses 0.6 / 1.25: two players with different
  settings could desync. Every online session (LAN netplay, rooms, quick
  match, ranked, spectating) now uses `rugen_sim::headless::ONLINE_ZOOM`
  (0.6 / 1.25); offline and the sync test keep the player's settings.
- [x] **LAN netplay settings desync.** (c2134fb) LAN P2P (`--net`, the `[net]
  remote` NETPLAY entry) took air jumps, air dash and fast fall from each
  player's own `[game]` options: two players with different settings could
  desync. The host's values now ride the handshake next to the fighter
  size and the guest simulates with them; every punch carries a LAN
  protocol number (2), and a peer without it (0.6.1 and earlier, which
  would use its own settings) is refused with "the other player runs an
  older rugen".
- [x] **Per-match seed.** (ae8eee5) Quick play and menu matches built the
  world with seed 1, so arena item drops (and CNS `Random`) repeated every
  match. Offline matches draw a fresh seed, replays record it (older ones
  play seed 1); online matches derive `online_seed(match_id)` on both
  peers, spectators and the server's re-simulation (server patch:
  `~/.cache/rugen-work/creators/server-seed.patch`, to apply with the
  engine tag bump); LAN hosts send it in the handshake (protocol 3), a
  protocol 2 peer plays seed 1 with both sides. Goldens pin seed 1.

## Gameplay

- [x] **Hit sparks and fighter size.** (88f615c) fightfx hit sparks are not zoomed with
  the fighter size, so they look large on chibi-sized fighters. Drawing only:
  fightfx explods follow their owner's size (`World::fx_scale`).
- [x] **Ranked balance and fighter size.** (f028b53, 8cabb2c) The balance verdict is computed
  from content only and ignores the size factor (×1.3 against ÷1.3 changes
  reach). Add the size to the verdict inputs. Profiles (FAIR_VERSION 2)
  carry the standing height; `verdict()` takes the match's size and weighs
  (taller / shorter)^0.5. The server must pass `rules.fighter_size` when it
  moves past the 0.6.2 engine tag (one-line change in `recompute_verdict`).
- [x] **Arena clock.** (5ec81fc) Show m:ss in the arena HUD instead of MUGEN seconds
  ("97").
- [x] **Arena zoom.** (c970f86) Fully zoomed out, fighters are about 25 px tall on Ink
  Arena; respect a minimum on-screen size or the user's zoom-out setting.
  Arena stages stop pulling back once the shortest fighter would drop below
  a tenth of the screen height (sim change on arena stages).

## Online

- [x] **Host picks the stage.** (289661a) Online rooms still keep the old stage strip
  on the fighter pick; move it to the new stage select screen, host picks,
  the other side watches the choice. No protocol change (the server already
  takes P1's stage); checked with two clients on a local server, P2
  downloading the host's stage.

## UI

- [x] **Mouse cursor in menus, alongside the keyboard.** (6eea26e) The mouse was
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
  On by default for new configs; an existing `rugen.toml` keeps the
  `mouse = false` the old default wrote (OPTIONS > VIDEO turns it on).
- [x] **Held direction repeats in menus.** (255c78a) Holding ↑ / ↓ (or ← / →) moves
  only once; it should repeat: first repeat after about 0.4 s, then every
  ~0.08 s, on keyboard and pad, on every list, grid and slider (the stage
  strip already does this; share one repeat helper).
- [x] **Pad labels outside fights.** (00371e8) Hints on menu screens (select
  `pick_key`, title) always show keyboard keys; switch to the pad's button
  names when the last input came from a gamepad, as fight overlays do.
- [x] **Stage thumbnails.** (37dde52) Bundled ink stages look plain on the strip
  (first background sprite only); render a small real preview once and cache
  it.
- [x] **Gamepad paging on the stage strip.** (2bc300e) L1 isn't mapped; page with
  L1 / R1.
- [x] **Controls dropdown arrow** (dd28904) sits just outside its box. Also
  fixed: a wide START key (2P `ENTER`) covered its label on the test pad
  (836884e).
- [x] **Help text** (e7d1643) repeats the panel subtitle on some options tabs.
- [x] **Online opponent card screenshot.** (08e588d) The opponent name on the online
  fighter pick is covered by a unit test only; check it with two clients.
  The name showed, but the card kept "THEY ARE PICKING..." after the
  opponent picked (the online screen is not ticked while the pick is open);
  fixed, checked in both themes.

## Docs and repo

- [x] **Fresh screenshots** of the Dojo and Ink themes for the docs themes
  page and the README. (no commit) Title, fighter select, stage select,
  fight, arena, online hub, host stage pick and matchup in both themes, in
  `~/.cache/rugen-work/polish/shots/` (1600x900); the README embeds no
  screenshots and the themes page lives in rugen-docs.
- [x] **`motif/STYLE.md`** (6c579a0) still describes the old pixel-art Dojo; update it
  to the current themes.
- [x] **`stages/README.md`** (4aa13ed) still shows the old `[Arena]` numbers.
- [x] **CONTRIBUTING.md** (15b6528) with the dual-licence clause ("contributions are
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
- [x] **Hitbox display.** (3af85f0) Toggle in the Training pause menu and on a key
  (F1). Draw Clsn1 red and Clsn2 blue from the sim's current frame, after the
  sprite, with the fighter-size zoom applied. Off in every other mode.
- [x] **Frame data.** (47e77cc) For the last move, show startup / active / recovery and
  advantage on hit and block, measured live from the sim (first frame with a
  Clsn1, last one, return to ctrl). Small panel in the corner, both fighters.
- [x] **Input display.** (75f60c7) A scrolling column per player with the direction
  arrow and the bound button keys, the hold length in frames; reads the same
  input buffer the sim uses.
- [x] **Record and playback dummy.** (aeba62c) Training menu: RECORD (take control of
  the dummy for up to 10 s), PLAY, LOOP; dummy options BLOCK: none / all /
  after first hit, and a reversal slot. Recordings are input lists, so they
  are deterministic.
  Also STANCE stand / crouch / jump, BLOCK random, three slots. Not built
  from the mockup: STANCE CPU and LIFE / POWER REFILL, which would change
  the simulation (AI level, life) rather than the dummy's inputs.

### Team modes
- [x] **Turns.** (4d43a08) Pick 1–4 fighters per side on fighter select; the winner of
  a round stays, keeps its life, the loser's next fighter comes in. Needs the
  select screen to pick several and the round code to swap a player.
  VS MODE and WATCH: X / Y step a side's team (SINGLE, then 2-4 per mode),
  Ikemen GO rules (the newcomer brings the fallen fighter's power; each
  side needs the other's Turns size in wins). Sim change for team matches
  only: `golden_tests` pins 1v1 checksums (KFM, plus Naruto / Sasuke /
  Goku with `--ignored`).
- [x] **Simul.** (4736078) Two fighters per side on screen at once (P1 + partner CPU or
  P3). Needs 4-player support in the sim (targets, pushing, camera) and the
  `teammode` / `numpartner` triggers characters already check.
  2-4 per side; partners of a human side are CPU (no P3 / P4 humans: two
  local players). Ikemen rules: `Simul.LoseOnKO` for the human fighter,
  time over by life share, `Partner` / `EnemyNear(n)` / `NumEnemy` see the
  teams; Naruto / Sasuke's own simul tag system runs (partner waits off
  screen).
- [x] **Tag.** (1fcc4f9) Like Simul but one active fighter per side; a tag button
  swaps, the partner jumps in from the edge. Start with the Ikemen GO tag
  rules so tag-ready characters (Naruto and Sasuke carry tag code) behave.
  `tag.zss` run natively (leave / wait / run in / jump in, red life,
  computer swaps); new TAG binding (N / 8 / LT). Naruto / Sasuke's own
  Uno tag system stays idle in Tag (it runs in Simul). Not done: Ikemen's
  `TagIn` / `TagOut` controllers and `TeamLeader`-style triggers for
  characters written for Ikemen's tag; Team.LifeShare (off by default in
  Ikemen too).

### More modes
- [x] **Survival.** (88b44ae) One life bar against an endless line of random opponents;
  a little life back between fights; count of wins on the result screen.
  CHALLENGE title entry with a card per mode (mockup "Challenge modes");
  one round per fight, +15 % life after a win, no `[Arena]` stages, best
  wins per fighter in profile.json. No sim change (`RunRules` set the
  rounds and P1's life on the built world; replays record them).
- [x] **Boss Rush.** (dcf987a) The roster's `boss = 1` fighters (select.def) back to
  back; if none are flagged, the five strongest by the balance verdict.
  rugen has no select.def of its own: it reads `select.def` (or
  `data/select.def`) in the game root only for the flags, matched to
  `chars/` by folder, archive or def name. The fallback line fights the
  weakest of the five first; no continues.
- [x] **Time Attack.** (3b1a538) The arcade ladder with a run timer; best times saved
  per fighter in the profile. The clock counts fight frames (intros and
  KOs too, not the pause menu or the screens between fights); a loss ends
  the run without a time. Screens: ink / dojo challenge, survival, boss
  rush and time attack shots in `~/.cache/rugen-work/modes/`.

### Storyboards
- [x] **Intros and endings.** (1dc4b3e) Play MUGEN `.def` storyboards: the screenpack
  intro and logo, each fighter's `intro.storyboard` before its first arcade
  fight and `ending.storyboard` after the last. Skip with START. Scenes,
  layers, BG, text and sound per the MUGEN storyboard format.
  Checked with MUGEN 1.1b1's KFM intro / ending and the KFM motif's logo
  and intro (scrolling `bg.name` background), also from inside the `.rgc`.
  A fight button skips one scene, START / Esc the whole storyboard;
  `rugen storyboard <file.def>`. Not done: the game over, credits and
  default ending storyboards (rugen has no continue screen or credits),
  TrueType font colours, Ikemen's extra keys (`layerX.scale`, `jump`...).
  None of the community fighters in `chars/` ships one. The bundled
  themes have no intro.

### Arena
- [x] **Ledge grab.** (b0e4551) When a falling fighter's hand reaches the edge of the
  deck while facing it, hang for up to 2 s with brief invulnerability; then
  up = climb, forward = roll in, jump = jump. Limited regrabs per airtime.
  The arena AI uses it for recovery.
  Facing the edge or drifting toward it (legacy fighters cannot turn in
  the air); jump = the X button, down / back lets go; 40 ticks of
  invulnerability (16 on a regrab), two grabs per airtime. Also fixed:
  a throw released past the edge left the fighter standing on air.
  Self-KOs at level 4, 80 matches on Ink Arena: 19 before, 9 after.
  Shots: `~/.cache/rugen-work/arena/shot-ledge-{ink,dojo}.png`.
- [x] **Items.** (36dc7a8) `[Arena] items = 1` spawns pickups every 10–20 s on the deck
  and platforms: a throwable (projectile on throw), a heal and a power
  boost. Grab with a button near an item; deterministic spawn from the round
  seed for netplay.
  B picks up and throws (the press never reaches the character); heal
  30 %, power 1000; at most three lying, 20 s each; `item_every = min, max`
  seconds. The round seed is the match seed, the round number and the
  round's first frame (every match its own seed since ae8eee5, see
  *Bugs*). The
  examples/arena golden checksum changed on purpose. Self-KOs at level 4
  with items: 10 per 80 matches. Shots:
  `~/.cache/rugen-work/arena/shot-items-{ink,dojo}.png`.
- [x] **Stage hazards.** (289c0e7) Moving platforms (path and speed in `[Arena]`) and
  timed events (a platform that drops, a wind push), all deterministic.
  Path, speed and drop are keys of the platform's own `[Platform]`
  section (several platforms can move), wind is `[Arena] wind = every,
  length, strength`; everything runs off the round's fight clock.
  `[BG] platform = N` moves the art. Ink Arena: the top-left platform
  slides, the top-right one drops 5 s every 20 s, a 4 s gust every 40 s.
  Self-KOs at level 4 on the final Ink Arena (ledges, items, hazards),
  80 matches: 15 and 12 for seeds 1 and 101 (19 and 12 before the Arena
  work); the gap to ledge-only (9, 8) is no hazard ring-out in the
  traces (none within 2 s of a gust), mostly long dash moves.
  Shots: `~/.cache/rugen-work/arena/shot-hazards-{ink,dojo}.png`,
  `shot-wind-ink.png`.

### Online
- [x] **Ranked seasons.** (ead3a26; rugen-server ea293df, 4b94e1b) A season id and dates on the server; ratings soft
  reset at season start (pull toward 1200), last season's tier kept as a
  badge on the profile.
  `rugen_proto::season_reset` is the pure reset (tested); the server keeps
  every finished season's final standings (schema v7) and serves them with
  `?season=N`; the badge is the tier of the final rating. LEADERBOARD steps
  through past seasons with left / right. `POST /v1/admin/season_end` ends
  a season early. Shots: `~/.cache/rugen-work/ro/shots/board-{ink,dojo}.png`,
  `board-s2.png`.
- [x] **Per-fighter leaderboards.** (fad0209; rugen-server c662609) Server keeps wins and rating per
  fingerprint; LEADERBOARD gets a FIGHTERS tab.
  Per player and fighter, per season (schema v8), ranked only; a new
  season starts a player's fighter rating from a soft reset of the last
  one. Tab switches PLAYERS / FIGHTERS, a FIGHTER stepper cycles the
  fighters with a board. Shots: `~/.cache/rugen-work/ro/shots/fboard-{ink,dojo}.png`.
- [x] **Tournament brackets.** (1a2c5c7; rugen-server 5277884) In a room: 8 or 16 players, single
  elimination, the server seeds by rating and calls each match; spectators
  watch the bracket.
  FORMAT on CREATE ROOM; the host starts the bracket (2+ entrants, byes
  for the top seeds); a disputed / failed match is called once more, then
  a walkover (the player still in the room, else the one further on, else
  the higher seed; 3 min without a start). Scores come from the reports'
  additive `rounds`. Shots: `~/.cache/rugen-work/ro/shots/bracket-{ink,dojo}.png`,
  `create-format-{ink,dojo}.png`.

### Live watching (shared base for replays and brackets)
- [x] **One match format: header + input stream.** (5ee5279) Header: rugen version,
  fighter and stage fingerprints, fighter size, rules, round seed. Body: the
  confirmed inputs per frame. Saved replays stay `.json`; the live stream is
  compact binary (~2 bytes per player per frame, ~15 KB a minute).
  `rugen_proto::stream`: `MatchHeader` (fingerprints, names and package
  sizes, rules, seed, players, game version) and RGM1 chunks; uploads
  ride the relay session (`STREAM` / `STREAM_ACK`) when the ticket says
  `live`.
- [x] **Server keeps the input log of every live match.** (078857f; rugen-server 21eaede, 5c19f78) The relay already
  forwards the inputs; keep a copy, fan out to spectators from the server (not
  from P1), drop the log when the match ends unless it is shared.
  Each player feeds only its own half of every frame; bounded (15 min a
  match, `RUGEN_LIVE_MB` = 64 for all); a finished match's log stays 15
  minutes (late viewers, sharing from the result screen), then goes.
  `GET /v1/live`, `GET /v1/live/{id}?from=N` (binary chunks).
- [x] **Watch any live match, even mid-match.** (bbe2cf4) WATCH downloads missing
  fighters and stage through the package sync, fetches the backlog from frame
  0, fast-forwards the sim without drawing ("CATCHING UP…"), then follows
  live a few seconds behind the players (smooth start, no relaying to a
  player in tournaments).
  `watch::Feed` polls the log, `watch::Prep` downloads what is missing,
  `NetMode::Stream` plays it with the header's seed and the online rules;
  start buffer 1.5 s, catch-up at 120 frames a drawn frame (silent, not
  drawn), then 3 s behind. A match of another game version is refused.
  Tested with two clients and a spectator on a local server, a room match
  and a bracket match. Shots: `~/.cache/rugen-work/ro/shots/watch-{ink,dojo}.png`,
  `watch-catchup-{ink,dojo}.png`.
- [x] **WATCH from the lobby and the bracket.** (b820af7) Live rooms show WATCH in the
  lobby; the bracket's live match opens it on ENTER.
  ROOMS also lists live quick / ranked matches. Shots:
  `~/.cache/rugen-work/ro/shots/lobby-live-ink.png`,
  `bracket-live-{ink,dojo}.png`, `bracket-watch-ink.png`.

### Replays
- [ ] **Shareable replays.** Keep a finished match's input log on the server
  (see *Live watching*), get a short code; WATCH → BY CODE downloads and plays it,
  fetching missing fighters through the existing package sync. Kept 30 days.

### Creators
- [x] **User themes folder.** (7149d7c) `themes/` next to `chars/`; any folder with a
  `system.def` shows up in INTERFACE; the game watches its files and reloads
  the screen on save, no rebuild.
  `theme.toml` (`name`, `base = "ink"`) lets a theme hold only what it
  changes; `rugen theme new <name> [--from ink|dojo]` starts one. A save
  reloads system.def and the screen on display in place (focus kept), a
  broken TOML keeps the last good screen under a THEME ERROR strip; the
  fight HUD follows at the next fight. Online matches simulate with the
  bundled fight.def. Not built from the mockup: the LANGUAGE half of the
  INTERFACE page (i18n comes last). Shots:
  `~/.cache/rugen-work/creators/shots/themes-{ink,dojo,live-reload,error}.png`.
- [x] **Fighter debugger.** (72228bc) `rugen debug <fighter>`: pause, step one frame,
  show state number, anim, elem, vars, fvars, which controllers fired this
  frame and why; click a player to inspect.
  Also STEP BACK (600 frames), slow motion, boxes, Tab through players
  and helpers; failed triggers show the value of their left side. The
  sim records outcomes in `World::trace` (off by default, never
  checksummed). Not done: a break on state, watch expressions, the
  special states' failed controllers (only fired -1 / -2 / -3 ones are
  listed: -1 alone fails dozens every frame). Shots:
  `~/.cache/rugen-work/creators/shots/debugger-{ink,dojo}.png`.

### Reach
- [ ] **Translations (i18n).** Move every UI string into `motif/lang/en.toml`
  keyed by id; add `pt-BR`, `es`, `fr`, `it`, `de`, `pl`, `tr`, `id`, `fil`
  and `ja`. Add accented Latin glyphs to the generated fonts; Japanese needs
  its own pixel font (kana plus common kanji). Language picker in INTERFACE,
  default from the OS locale. Buttons must fit the longest language.
- [ ] **Steam Deck layout.** A 1280x800 profile: controller-first hints,
  bigger text, no hover-only actions, suspend and resume safe.
  Partly done (6aec258): `[video] profile = auto|deck|desktop` (detected
  on a Deck), first launch fullscreen 1280x800, pad-first hints with the
  Deck's button names, pause on focus loss, no catch-up burst after a
  suspend; every action already works by pad (the only hover effect is
  `hover_scale`, visual), layouts checked at 1280x800 in both themes
  (`~/.cache/rugen-work/creators/shots/deck-*.png`). Left unticked:
  **bigger text**. The menus draw 1:1 at 1280x800 with pixel fonts (small
  font 11 px); scaling them by a non-integer factor blurs them and the
  next size up (the 23 px menu font) overflows fixed boxes everywhere. It
  needs a 13-14 px "small" font per theme (fontgen output is not
  reproducible on this machine: ui-gen rewrites the ink fonts) and a pass
  over every screen of both themes.
