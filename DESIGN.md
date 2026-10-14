# Rugen — MUGEN-compatible 2D fighting engine in Rust

Working name. Change later.

## Goal

Drop-in replacement for M.U.G.E.N / Ikemen GO content, written in Rust,
with two hard requirements:

1. **Runs everywhere.** Windows, Linux, macOS (x86_64 + aarch64). One codebase, one `cargo build`.
2. **Online play that is actually smooth.** Rollback netplay, designed in from the first line.

Everything else (interactive stages, tools, modes) is layered on after those two hold.

## Non-goals (for now)

- 3D.
- A new scripting language replacing CNS.
- "Fixing" MUGEN semantics. Compat first; quirks are features.

## Compatibility contract

Existing content must load and behave as in MUGEN 1.1 / Ikemen GO without edits:

| File | Role | Notes |
|------|------|-------|
| `.def` | Manifest (char / stage / screenpack) | INI-like. `mugenversion`, `localcoord` |
| `.cns` | States, constants, triggers | Expression lang + ~100 state controllers, ~200 triggers |
| `.cmd` | Command inputs + state entry rules | Shares CNS expression syntax |
| `.air` | Animations, clsn boxes | Text |
| `.sff` | Sprites | v1 (PCX) and v2 (PNG / RLE8 / RLE5 / LZ5), palettes |
| `.snd` | Sounds | WAV container |
| `.act` | Palettes | Raw 256×RGB |
| `.fnt` / `.def` fonts | Bitmap fonts | v1 + v2 |

Reference implementation for semantics: Ikemen GO source. When in doubt, match it, then diff frame-by-frame.

Content realities to handle, not fight:
- Case-insensitive file resolution (`KFM.sff` vs `kfm.sff`) on every OS.
- Backslashes in paths inside `.def`.
- CP1252 / Shift-JIS text. Decode leniently, never panic.
- Garbage files. Fuzz every parser.

New engine behavior is gated by `engineversion = 2` in `.def`. Absent = legacy mode.

## Architecture

Cargo workspace:

```
rugen/
  crates/
    formats/    # .sff .air .snd .act .fnt parsers, .rgc / .rgs content files (+ vfs). Fuzzable.
    cns/        # lexer, Pratt expression parser, bytecode compiler, VM
    sim/        # deterministic game simulation. No I/O, no floats-from-libm, no render.
    net/        # ggrs integration, transport, matchmaking client, desync detection
    render/     # wgpu renderer, camera, shaders, palette-on-GPU
    audio/      # kira
    opus/       # safe Opus encoder / decoder (unsafe-libopus: libopus in Rust)
    fair/       # content fingerprints, package verification, fairness analysis
    content/    # chars/ + stages/: import, .rgc conversion, export, cache
    app/        # winit, gilrs, config, UI glue; the binary
  assets/
  docs/
```

Rule: `sim` must compile without `render`, `audio`, `app`. If it can't, netplay is already broken.

### Determinism (netplay foundation)

- Numbers are `i32` and `f32`, because CNS semantics (int/float promotion, `7/2 = 3`, `7.0/2 = 3.5`, truncating casts) are defined on them and content depends on it. Fixed-point would break compatibility.
- `f32` add/sub/mul/div/sqrt are bit-exact across x86_64 and aarch64 under IEEE 754 as long as nothing fuses or extends precision. Rust never emits FMA without `mul_add`, and never uses x87 on x86_64. Rule: no `mul_add`, no `-ffast-math`-style flags.
- Transcendentals (`sin`, `pow`, `exp`, …) come from the `libm` crate (pure Rust, same bits everywhere), never from `std` (platform libm).
- Sim state = plain data, arena + indices. No `Rc`, no `Box<dyn>`, no pointers. Must be `Clone` and cheap.
- PRNG seeded per match, lives inside state. CNS `Random` reads from it.
- Ordered containers only (`Vec`, `BTreeMap`, `IndexMap`). Never `HashMap` iteration in sim.
- CNS VM state is part of the snapshot.
- Sim runs fixed 60 Hz. Render reads state, never mutates it.
- Target: full frame step < 2 ms so 7-frame rollback fits in budget. Snapshots
  must not copy immutable character data: compiled commands sit behind an
  `Arc` (only their runtime state is cloned), animations are read in place.
  `crates/sim/examples/rollbench.rs` measures save / load / step (Naruto vs
  Sasuke: 36 / 40 / 99 µs mean, a 12-frame rollback ≈ 1.7 ms).

### Netplay

- `ggrs` for rollback. Use its sync-test session mode in CI from day one; it catches nondeterminism locally with no network.
- Paths, chosen per match by round trip and jitter with hysteresis
  (`rugen_net::path::PathSelector`; direct > TURN > relay on a tie; a path
  that goes quiet for 0.8 s is left at once; a better one takes over after
  holding for 2 s):
  - **Direct** (`rugen_net::direct`, the default): a small ICE. One UDP socket
    per address family; host candidates (route lookup, global IPv6 only),
    server-reflexive (Cloudflare STUN, `stun.cloudflare.com:3478`; a second
    probe to port 443 tells endpoint-dependent "symmetric" NATs apart) and
    peer-reflexive (an authentic check from an unknown address). Signalling
    is opaque `T_DIRECT` payloads in the relay session, between the two
    players only: a hello with the privacy flag and a random nonce, then the
    candidates, only when both players allow direct connections. Key =
    HMAC-SHA256(nonce P1 ‖ nonce P2, "rugen direct v1" ‖ session); every
    direct datagram carries a counter (64-entry replay window) and an 8-byte
    truncated HMAC-SHA256 tag, anything else is dropped unparsed.
    Simultaneous checks on every pair every 50 ms; the first working pair is
    used at once, a faster one takes over (15 % / 10 ms margin); the selected
    pair is checked every 200 ms (consent, RTT), others every 1 s, then 10 s
    (NAT keep-alive). Both symmetric: only host pairs get 2.5 s, then TURN.
    The relay session stays open (hello every 2 s; the server expires idle
    sessions after 5 s) for signalling and fallback; BYE only at the end.
- Per-frame state hash exchanged; desync surfaces as an error, never silent drift.
- Content hashing: both sides verify char/stage hashes before match.
- Replays = input log + content hashes. Spectating = same stream.
- Transports (server tickets list them in order: `turn`, `udp`, `ws`):
  - **TURN** (`rugen_net::turn` + `stun`, a ~1k-line RFC 8656 client: long-term
    auth, Allocate / Refresh / CreatePermission / ChannelBind, UDP then TCP then
    TLS to the TURN server). Both players allocate a relayed address on the
    server-issued service (Cloudflare Realtime TURN on the official server), swap
    those addresses through the authenticated relay session (a TURN server's
    address, never a player's), bind a channel to each other and move ggrs
    traffic over ChannelData once a probe round trip works (≤ 1.5 s at match
    start, else it keeps trying in the background and switches mid-match). The
    server relay stays connected for signalling, keep-alives and as fallback
    (used again whenever TURN goes quiet for 0.8 s).
  - Cost discipline for metered TURN: ChannelData only (4-byte header), all
    ggrs messages of a tick coalesced into one datagram in a varint encoding,
    input acks / keep-alives dropped when an input carries them, refreshes just
    before expiry, allocation released on drop. ≈3.4 KB/s per player and
    direction (plain encoding: ≈7.7 KB/s).
  - **UDP relay** to the server (1.5 s probe), else the **WebSocket** `/relay`.
- Lag hiding (measured with `crates/net/examples/netsim.rs`, two peers through
  a delaying, lossy UDP proxy, Naruto vs Sasuke on recorded inputs):
  - Input delay from the chosen path's p95 round trip
    (`rugen_net::DelayPolicy`): `clamp(ceil((p95/2 + 16.7 ms) / 16.7) -
    (window - headroom), min_delay, 8)` with a 12-frame window, 3 frames of
    headroom (2 were not enough at 350 ms: ggrs still hit the window) and
    `min_delay = 1`: 1 frame up to 300 ms (180 ms rolls back ~6 frames), 2 at
    301-333, 3 at 334-366, 4 at 367-400, +1 per 33 ms after.
  - Smooth time sync (`rugen_net::Pacer`): while ahead of the peer the tick
    stretches by 1.5 % per frame of (smoothed) advantage, capped at 8 %
    (58.5 Hz at two frames ahead); ggrs' wait recommendation (whole frozen
    frames) is ignored. The renderer keeps the display rate.
  - Correction smoothing (display only): the re-simulated copy of the frame
    on screen gives each fighter / projectile an offset that decays over ~4
    frames; jumps over 48 px and getting hit snap.
  - Prediction stays ggrs' repeat-last: on 76k recorded frames it is within
    2 points of the best "taps" model and much better than releasing
    buttons (`rugen_net::predict`, `examples/predict.rs`).
  - Local latency: draw right after the tick, mailbox or FIFO with one
    queued frame, and on FIFO tick right after the vsync wait. Estimated
    key → display on this machine: 57 ms (AutoVsync, two queued) → 19 ms
    (mailbox) / 25 ms (FIFO).
- Result evidence: confirmed input log (RLE when the server offers TURN) plus
  a world checksum every 60 frames; the server compares both reports on the
  confirmed frames and disputes (and re-simulates, when built with `resim`) on
  a mismatch, so a match it never saw is still verified.
- Ending early: an online match cannot pause, so Esc / a pad's Select opens
  a RESUME / FLEE menu over the running fight (the local fighter stands
  still while it is open), and so does START. START has its own binding
  and never reaches the simulation (not sent, not recorded); MUGEN's s
  button belongs to the fighters (power charge, taunts, awakenings). FLEE sends a flee message on every path and a
  forfeit report (the other player wins, `ending = "flee"`), which the
  server settles at once: a normal loss, no extra penalty. The player left
  behind sees the flee message, or the relay's announcement of the other
  slot's BYE (a quit), or silence on every path for 8 s
  (`rugen_net::DISCONNECT_TIMEOUT`; path failover takes < 1 s, so only a
  dead connection gets there), and wins: "YOUR OPPONENT FLED" / "OPPONENT
  DISCONNECTED", then a report claiming the win. The server never takes that
  claim on trust: it waits 15 s for the other report (agreement or a
  dispute decide then), then settles only if its own relay session saw the
  other slot say BYE or go silent for 10 s while the claimant was still
  heard 2 s later (players HELLO the relay every 2 s on every path).
- Netplay seat: the local fighter reads every local control (both keyboard
  layouts, both gamepad slots), whichever side the server assigned.

### Cross-platform stack (no system libraries)

| Need | Crate | Why |
|------|-------|-----|
| GPU | `wgpu` | Vulkan / DX12 / Metal, one API |
| Window | `winit` | |
| Gamepads | `gilrs` | XInput / evdev / IOKit |
| Audio | `kira` (cpal) | |
| Math | `libm` | deterministic transcendentals |
| Rollback | `ggrs` | |
| Lexing | `logos` | |
| Config | `serde` + `toml` | |
| Compression | `zstd` | vendored C built by `cc` (no cmake / pkg-config) |
| Opus | `unsafe-libopus` | libopus 1.3.1 translated to Rust: no C toolchain at all |
| WebP | `image-webp` | pure Rust, lossless encode |

### Content files (`.rgc` / `.rgs`)

Characters and stages are played from rugen's own single-file format
(spec: [docs/rgc-format.md](docs/rgc-format.md)): a header + TOC + sections
read on demand. Sprites are stored pixel-exact (zero borders trimmed,
identical blocks / palettes once, one zstd-19 frame per block for random
access, decoded on first draw), sounds as Opus (next Opus rate, 32 kbit/s per
channel at 48 kHz, exact length), loose PNGs as lossless WebP when smaller,
everything else byte for byte, plus the logic compiled by the converting
engine (recompiled into `cache/logic/` by an engine with another
`LOGIC_SCHEMA`). Paths reach inside the file (`chars/x.rgc/x.def`) through
`rugen_formats::vfs`, so every loader works the same on folders and content
files. `Sff::write_exact` rebuilds the SFF for `rugen export`.

The content fingerprint (`rugen-fp-2`) hashes the def's canonical file set:
text and palettes by bytes, the sprite file by `Sff::canonical_hash` (what
the parser yields: version, palette slots, keys, every sprite's key, size,
axis, slot and pixels), sounds not at all. It is identical for a folder, an
archive, its `.rgc` and the export of that `.rgc`, on any machine.

CI: GitHub Actions matrix on all three OS, artifacts per commit. macOS universal binary, notarized.

## Phase plan

### Phase 0 — skeleton ✅ (2026-10-11)
- Workspace, crates above, CI green on all three OS with a window showing one sprite.
- `formats`: SFF v1/v2, AIR, DEF, SND, ACT loaders, tested against every file in the reference tree.
- `render`: wgpu, palette-on-GPU, logical camera with zoom, offscreen readback.
- `app`: `rugen view` sprite/anim viewer with Clsn boxes and `--shot`.

### Phase 1 — one character stands ✅ (2026-10-11)
- CNS lexer, parser, bytecode VM (`cns` crate).
- `sim`: char loading (def/cns/cmd/common1, constants), MUGEN frame order, Ikemen-exact animation timing, input buffer and command matching, state machine with persistent/trigger logic, ~25 state controllers, 100+ triggers, screen bounds, camera.
- KFM loads, idles, walks, attacks, jumps and lands on a flat stage; `rugen play` drives it with the keyboard.
- Headless tests cover all of the above and confirm clone-determinism.
- Still open from this phase: `ggrs` sync-test in debug builds (Phase 3 prerequisite).

### Phase 2 — a real match ✅ (2026-10-11)
- Player push, Clsn1/Clsn2 hit detection, HitDef with MUGEN defaults, hit pause, guard, get-hit variables and states, damage/KO, targets, HitBy/NotHitBy, HitOverride, ReversalDef, corner push, custom states.
- Sparks + fightfx, sounds (kira), projectiles, helpers, explods, Pause/SuperPause, PalFX/AfterImage/EnvShake, Trans/AngleDraw, binds, parent vars.
- Stage backgrounds (tiling, parallax, anims, sin, blends, windows), MUGEN camera with tension and zoomin/zoomout, vertical follow.
- Rounds, clock, win/lose, match flow; `fight.def` lifebars with MUGEN fonts; AI button jamming (`AILevel`).
- Screenpack: `system.def` title menu, select grid with cached PNG portraits, VS screen.
- Verified against community characters: `rugen check` reports zero unsupported controllers for the test set; 4000-frame AI soak clean.
- Content library: `chars/` and `stages/` hold archives, `cache/` holds extracted + converted content keyed by hash (manifest v3; since manifest v7 everything plays from `.rgc` / `.rgs` content files, see Phase 4).

### Phase 3 — online (core ✅)
- ✅ ggrs P2P over UDP (`--net ip:port [--port n] [--p2]`), input delay / prediction window from `rugen.toml`, desync detection, rollback-muted sound events, connection status HUD.
- ✅ Sync-test session (`--synctest`, and a 600-frame automated test) proves the sim is deterministic under rollback.
- ✅ Replays (setup + per-tick inputs, JSON) with byte-identical playback; NETPLAY title entry from `rugen.toml [net]`.
- ✅ `rugen-server` (separate repo [rugen-dev/rugen-server](https://github.com/rugen-dev/rugen-server)): self-hostable hub on axum + SQLite (WAL, versioned migrations): named accounts (username + Argon2id password, several devices; API 5) next to anonymous ones, lobby with public / private rooms (Argon2id passwords, rules, spectators, kick, rematch), quick match and ranked (Elo K=32, ping window from server-measured RTTs, 90-day seasons, leaderboard), **match relay** (UDP fast path + `/relay` WebSocket fallback; per-slot tokens; nobody learns another player's IP), result verification from both input streams + checksums, stored ranked replays. `rugen-proto` holds the JSON types and the relay framing; client in `rugen_net::online` / `rugen_net::relay` (ggrs `NonBlockingSocket` with virtual peer addresses), adaptive input delay, rollback-safe event filtering. Deployment: `deploy/` in that repo (Docker, Compose, Cloudflare Tunnel, Dokploy).
- ✅ ONLINE screens on the UI engine (`motif/ui/online.toml`): room create/join (typed code), ranked queue with opponent card, leaderboard; results reported after the match.
- ✅ Seasons, boards, brackets: `rugen_proto::season_reset` (pure) ends a 90-day season (final standings and a tier badge kept), per-fighter boards per season, tournament rooms of 8 / 16 (seeded by rating, matches called by the server).
- ✅ One match format (`rugen_proto::stream`): a header and the confirmed inputs. Players feed the server's input log on their relay session; viewers watch any match from it, even mid-match (`watch.rs`, `NetMode::Stream`: catch up, then 3 s behind); a finished match is shared by an 8-character code.
- Open: content hash handshake (desync detection catches mismatches today), synced online character select, server-side anti-cheat beyond agreement.

### Phase 4 — creator tools (started)
- ✅ `rugen check <char>`: load time, unsupported controllers, compile warnings with file:line. F2 hitbox overlay, F1 debug title.
- ✅ F3 overlay: state, state time, type/movetype, ctrl, anim/elem/animtime, pos/vel, life/power/hit count/juggle and the last inputs per player.
- ✅ Hot reload: a character's source files are re-read in place when they change on disk (a folder in `chars/` is reconverted into its `cache/rgc/` working copy, unchanged sections reused, then reloaded).
- ✅ Content files: `.rgc` / `.rgs` replace the extracted copies and the `.rugen-opt` optimiser cache; archives are converted in the background from launch (`crates/app/src/prepare.rs`: the title opens at once; one low-priority worker spreading its encoding over every core but two (kept for the game); stages first, a picked fighter jumps the queue; menus draw unready fighters dimmed with a progress ring and a PREPARING n / N badge; the versus screen waits only for an unready pick and preloads both fighters; content files appear only once complete, so online and replays never see a half-converted one; originals kept in `originals/` or deleted), folders are working copies, `rugen convert` / `rugen export` (back to a Fighter Factory folder, same fingerprint when converted again). Online packages are the content files (API 5).
- ✅ Verification: `rugen verify` (load + inspection + two headless soaks) writes a `verified` / `warn` / `broken` verdict into the cache manifest, runs automatically on first use per engine version, select-screen badge, broken characters refused.
- ✅ Content safety: `Limits` (helpers per root, explods, projectiles, sounds per tick), NaN/inf sanitizing, saturating life/power/counter math, `World::step_checked` crash isolation (offending player disabled, match continues), fuzz tests (random controllers, corrupted SFF/SND/AIR/DEF).
- ✅ User themes (`crates/app/src/themes.rs`): `themes/<name>/` next to `chars/`, any folder with a `system.def`, listed in OPTIONS → INTERFACE after the bundled themes (a bundled name wins). `theme.toml` (`name`, `base = "ink"`) lets a theme hold only what it changes (lookup: the theme, its base, the bundled `motif/`, `data/`); without a base it is a plain MUGEN screenpack. While one is active its files are polled every 0.5 s and a save reloads `system.def` and the screen on display in place (focus and state kept, `Ui::replace_def`); a TOML that fails to parse keeps the last good screen under a THEME ERROR strip. `rugen theme new <name> [--from ink|dojo]` starts one. Online matches simulate with the bundled `fight.def`, never a user theme's.
- ✅ Fighter debugger (`rugen debug <char> [char2]`, `crates/app/src/debugger.rs`): a local match drawn into the view of `ui/debug.toml` (ink, dojo; `Renderer::remap_since` moves the fight's queued vertices into the panel's rectangle, clipped) with PLAY / PAUSE, STEP, STEP BACK (a world copy after each of the last 600 frames), slow motion, boxes, and for the selected player (click or Tab, helpers too) state, file and state name, anim / elem / time / ctrl / physics / pos / vel / life / power, the variables that are set (changed this frame in red) and every controller the state machine looked at this frame: run (the trigger group that held) or not (the failed trigger and the value of its left side, evaluated on a world copy). The sim records it in `World::trace` (`CtrlTrace`, off by default, never checksummed or read by the simulation); trigger texts come from the fighter's files by each controller's line.
- Open: break on state, watch expressions, frame-advantage display.

### Phase 5 — new engine features (started)
- ✅ `[Platform]` stage sections: per-player ground level, landing on surfaces, walking off edges; `Pos Y`/`PosSet` stay ground-relative so legacy characters work unchanged (`stages/README.md`).
- ✅ Triggers `GroundY` and `OnPlatform` for new characters (legacy characters never use them).
- ✅ Arcade ladder (six random opponents), stage / screen music, fullscreen toggle.
- ✅ `[Arena]` stages: blast zones (ring-outs), stocks with respawn, floor spans (pits), no-wall mode; camera zooms out for height; engine air jumps / air dash / fast fall for legacy characters (`[game]` config, per-stage override). Sample: `examples/arena/arena.def`.
- ✅ Arena AI (`crates/sim/src/arena_ai.rs`): CPU fighters on `[Arena]` stages keep off the pit edge, veto moves learned this match to carry them off, cancel moves that do, recover (air jumps, air dash, hit-fall tech, their own air moves) and follow the opponent across platform tiers. Filters the CPU's buttons and vets its AI `ChangeState`s inside `World::step` (deterministic; replays record it). `[game] arena_ai` (default on), `rugen play --arena-ai 0|1`; never online.
- ✅ Ledge grab (`crates/sim/src/ledge.rs`): on `[Arena]` stages with a `floor` span (`ledge = 0` turns it off) a falling fighter whose hand reaches a deck edge while facing it (or drifting toward it) hangs up to 2 s, invulnerable for 40 ticks (16 on a regrab), two grabs per airtime; up climbs, forward rolls in, X jumps, down / back lets go. The engine owns the hanging fighter (its states do not run, like a Tag switch) and reads the action from the input buffer, so humans, CPUs and replays share one path; the hang is player state (cloned, checksummed only when set). A fighter left standing over the pit (a throw released past the edge) now falls. The arena AI aims its recovery at the ledge (air jumps that reach it count as safe) and climbs, or rolls / jumps past an opponent guarding the edge. The ink / dojo HUD shows human hangers a timer and their keys (`[ArenaHUD] hint.title.bank`).
- ✅ Arena items (`crates/sim/src/items.rs`): `[Arena] items = 1` drops a pickup onto the deck or a platform every 10–20 s (`item_every = min, max`), at most three lying around, gone after 20 s: a throwable (carried; the next B throws it, a knock-down hit delivered through `collision::hit_result` with an `Item` source), a heal (30 % life) and a power boost (1000). B picks up next to a standing fighter with control; that press is kept from the character until released, so it never also kicks. A hit knocks a carried item loose. Drops come from the items rng seeded by the match seed, the round number and the round's first frame (never `World::rng`); the items are world state, cloned and checksummed only on stages with items. The arena AI takes useful items in reach, walks to one on its surface when the opponent is not close (a heal first when hurt) and throws at an opponent in front. Drawn as ink shapes in front of the fighters; the arena card gets an ITEM tab, a human gets a "PICKED UP · <B> THROW" tag.
- ✅ Stage hazards (`crates/sim/src/hazards.rs`), all a pure function of `World::hazard_t` (fight ticks this round, frozen by pauses): `[Platform] path = dx, dy, ... ` + `speed` moves a platform along a closed loop of offsets (riders and lying items move with it); `[Platform] drop = every, gone` drops it for `gone` s every `every` s after 3 s of warning (`Platform::gone`: not there to land or stand on, riders fall); `[Arena] wind = every, length, strength` blows a gust every `every` s (alternating right / left, eased, half strength on the ground, never on a ledge hanger). Platform motions live in `Stage::motions` (the platforms themselves stay plain `Copy` data); `hazard_t` is checksummed only on stages with hazards. `[BG] platform = N` makes stage art follow platform N. The arena AI drops through a platform about to fall, never jumps onto one, leans into a gust near the downwind edge and counts the wind in its landing predictions. The HUD shows "DROPS IN N S" over a warned platform with red stripes and a WIND tag under the clock (ink and dojo).
- ✅ Team modes, Ikemen GO rules (`crates/sim/src/team.rs`): every team member is a root player added up front in Ikemen's order (P1, P2, then P3 / P4...); Turns members wait `removed`, the next comes in when one falls (winner keeps its life, the newcomer the fallen one's power), each side needs the other's Turns size in wins. Simul partners all fight (input slots: a human side's partners are CPU), `Simul.LoseOnKO` for human members, time over by life share; `Partner` / `NumPartner` / `EnemyNear(n)` / `TeamMode` follow the teams. Tag runs Ikemen's `tag.zss` natively: partners leave once the fight starts and wait `standby` (not hit, pushed, followed by the camera or anyone's P2), the TAG button (`Buttons::t`, bit 12, offline only: masked off on the wire) swaps, a fallen leader's partner jumps in, red life comes back outside; the engine's tag movements replace the player's states while they run. `World::teams` defaults to a classic 1v1, which never reaches the team paths (`golden_tests` pins 1v1 checksums). Offline only; online stays 1v1.
- ✅ Global LIFE (`MatchSpec::life` / `World::life`): every fighter's and partner's LifeMax, round-start life and `Const(data.life)` instead of their own `[Data] life`. Part of the match setup like the fighter size: `[game] life` (1000-10000, default 5000) offline, recorded in replays (absent = each fighter's own, so older replays and the golden checksums are unchanged), LAN protocol 4 carries the host's, online rooms take `Rules::life` (missing = 5000, quick match and ranked) and the server's re-simulation applies the same.
- ✅ Training edits (`crates/app/src/training.rs` `Edits`): the few training tools that change the world (the LESSONS' stopped clock, given back to Training's own clock when they end, LIFE REFILL after a combo, POWER ALWAYS FULL) are applied by the app between steps; a recording keeps each change with its frame (`Replay::edits`) and playback re-applies them, so training replays stay exact. Offline Training only. The sim's one hook is `Player::no_ko` (set by LIFE REFILL, false everywhere else): damage, LifeAdd and LifeSet leave at least 1 life, so the fighter is never KO'd.
- ✅ CHALLENGE runs (`crates/app/src/challenge.rs`): a title menu entry with one card per mode; the shell lines up a run's fights after a one-cursor fighter select (the arcade ladder runs on the same code). Survival: one round per fight (`RunRules` on the `FightSetup`, applied right after the world is built and recorded in replays), P1's life carried over plus 15 %, random opponents, never an `[Arena]` stage. Boss Rush: the `boss = 1` characters of `select.def` (game root or `data/`, matched to the roster by folder / archive / def name) in file order, else the five strongest by the balance verdict's `power_score`, weakest first. Time Attack: the arcade ladder's six fights against a run clock counting the frames the fights advance (intros and KOs included, the pause menu and the screens between fights not), stopped by a loss. Bests per fighter in `profile.json` (`challenge`); the sim is untouched.
- ✅ Storyboards (`crates/app/src/storyboard.rs`): MUGEN cutscene `.def`s with scenes, layers (`[Begin Action]` animations and texts with `textdelay`), fades, `clearcolor` / `layerall.pos` / `window` carried over, sounds, music and `bg.name` backgrounds (drawn by the stage background code); scene timing as Ikemen GO. The screenpack's `logo.storyboard` / `intro.storyboard` play once before the first title (only a system.def lookup when there are none), ARCADE plays the fighter's `[Arcade] intro.storyboard` before the first fight and `ending.storyboard` after a cleared ladder; START / Esc skip it, a fight button skips a scene. `rugen storyboard <file.def>` plays one. Files are read through the vfs, so storyboards inside `.rgc` content files work.
- Steam Deck profile (`crates/app/src/deck.rs`, `[video] profile = auto|deck|desktop`, OPTIONS > VIDEO > DEVICE): detected from `SteamDeck=1` or the DMI board (Jupiter / Galileo); a first launch writes a fullscreen 1280x800 rugen.toml, hints start on the pad with the Deck's button names (Steam Input's virtual pad included). For every profile: a local fight pauses when the window loses the focus, and a clock gap over 250 ms (a suspend) resumes with one tick instead of a burst (online sessions keep catching up). The 1280x720 layouts fill 16:10 (verified at `--window 1280x800`, both themes). Bigger text: `system.def` `fontN.deck` (both themes map fonts 1 and 2 to `rugen-deck`, Pixelify Sans at 2x, 14 px capitals) replaces fontN wherever a screen draws it at scale 1 (`Host::slot`); the layout keeps the height of the named font (`Painter::text_box_height`) and screen nodes take Deck-only overrides from a `deck = { ... }` table (`ScreenDef::parse_for`), so desktop layouts are unchanged. A profile change reloads the screen.
- Open: `[Zone]` / `[Interact]`, stage transitions, assists, Ikemen `TagIn` / `TagOut`, scripting, mod packs, wasm.

## Open questions

- Lua vs Rhai for screenpack/mode scripting. Ikemen uses Lua; Lua keeps its content portable.
- Matchmaking server: who hosts? Keep it tiny (one binary, SQLite) so anyone can.

## References

All local, see `reference/README.md`:

- `reference/mugen-1.1b1/` — official 1.1b1 binary + KFM + docs + tools. Black-box oracle (Wine).
- `reference/mugen-docs/` — official HTML spec: `cns.html`, `trigger.html`, `sctrls.html`, `air.html`, `spr.html`, `bgs.html`.
- `reference/Ikemen-GO/` — readable semantic spec (Go source).
- `reference/ikemen-go-v1.0.0/` — Ikemen runtime + content (KFM variants, `interactivestage` example) for frame-diff harness.
- ggrs: https://github.com/gschup/ggrs

Notable: 1.1b1 already ships `data/*.lua` (system scripting) and `data/indexed.frag` (GPU palette shader). Lua + GPU palettes are the lineage, not inventions.
