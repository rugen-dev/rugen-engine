# rugen

MUGEN-compatible 2D fighting game engine in Rust.

- Loads existing M.U.G.E.N / Ikemen GO content unchanged (`.def` `.cns` `.cmd` `.air` `.sff` `.snd`).
- Runs on Windows, Linux, macOS from one codebase.
- Rollback netplay designed in from the first commit.

See [DESIGN.md](DESIGN.md) for architecture and phase plan, [reference/README.md](reference/README.md) for reference material.

## Build

```
cargo build --release
./target/release/rugen                      # title → select → fight
./target/release/rugen play kfm             # quick match against the computer
./target/release/rugen play kfm kfm --net 1.2.3.4:7000 --port 7000       # host side
./target/release/rugen play kfm kfm --net 5.6.7.8:7000 --port 7000 --p2  # joining side
./target/release/rugen check G6_Naruto      # which controllers a character uses that we lack
./target/release/rugen list
```

Put characters (`.zip`, `.rar` or folders) in `chars/` and stages in `stages/`;
they are extracted and converted into `cache/` on first use. `data/` holds the
screenpack (`system.def`, `fight.def`, `common1.cns`, ...); without one the
bundled MUGEN 1.1 data in `reference/` is used.

Keys live in `rugen.toml` (written on first run). Defaults: P1 `z q s d` to
move, `j k l` = a b c, `u i o` = x y z, `m` = start (`p` is free); P2 arrows +
numpad `1 2 3` / `4 5 6`, Enter = start. Gamepads
work out of the box. In a match: Space pause, `.` step, F2 hitboxes and
platforms, F3 state / frame data / input overlay, F1 debug title, +/- zoom,
Esc back.

Menus answer the base keys whatever the bindings: **Esc** backs out (on the
title it first jumps to EXIT; on the select screen it takes back the last pick
before leaving), **Enter** confirms, the **arrows** move the focus. A key a
player mapping already uses stays that player's: with the defaults the arrows
and Enter are P2's, which on single-cursor screens (title, options, arcade /
training / arena select, pause, result) drives the same focus, while in the VS
MODE select they move P2's own cursor and P1 uses its bound keys. Unbound
arrows / Enter, and gamepads without a player slot, act as P1. In menus A (and
the other attack buttons, Start) confirm and **B cancels** (pad South / East).
The mouse is off by default (cursor hidden, no hover): `[input] mouse = true`
or OPTIONS > VIDEO > MOUSE IN MENUS turns it on.

ONLINE is always on the title screen. Without a server it offers a **direct
match** (LAN / internet): one player picks HOST (plays 1P), the other JOIN
(2P), each types the other's address (the screen shows yours; port `[net]
port`, fighters / stage from `[net] p1` / `p2` / `stage`). Set `[net] server =
"host:7777"` in `rugen.toml` (a `rugen-server`, see below) for **Custom Room**
(create a 5-letter code, the other player types it under **Join a Room**),
**Ranked** (queue, Elo matchmaking with a widening rating window, seasons),
**Leaderboard**. Accounts are anonymous keys stored in `profile.json`; the
character you bring online is `profile.json` `main_char` (else `[net] p1`).
Results are reported by both sides and only count when they agree. Both
machines need the same characters installed (a match is declined otherwise).

Without a server you can still connect directly: `[net] remote = "ip:port"`,
`side = 1` or `2`, `p1` / `p2` / `stage` ids → NETPLAY entry in ONLINE; or by room code
through the server's rendezvous only: `[net] relay = "host:7777"`, `room =
"code"` (or `rugen play kfm --room code --relay host:7777 [--p2]`). The
server only swaps addresses; every match stays peer to peer. Music: stage `bgmusic` and
system.def `[Music]` entries play when they are mp3/ogg/flac/wav (MIDI is
skipped). F11 toggles fullscreen. Menu matches are
recorded into `replays/`; `rugen play ... --record file.json` records quick
play and `rugen replay file.json` plays a recording back.

Title menu: ARCADE, VS MODE, ARENA (platform-fighter rules: pick your
fighter and the CPU's, fight on a stage with an `[Arena]` section — stocks,
blast zones, ring-out, air jumps; the built-in `motif/stages/ink_arena`, or
any stage you add with one), TRAINING (you also pick the sparring partner),
TUTORIAL (guided basics then every special move of the chosen character,
detected as you perform it), WATCH (you pick both CPU fighters), ONLINE,
OPTIONS (volumes, camera zoom limits, fullscreen, computer level, net delay,
key rebinding; saves `rugen.toml`), EXIT. `[camera] zoom_out` /
`zoom_in` in `rugen.toml` decide how far players can separate before the
screen edge stops them, on any stage.

Menus are data: `motif/themes/<theme>/ui/*.toml` + WebP art (see
`motif/themes/dojo/ui/README.md`); themes live side by side under `motif/themes/`;
`rugen ui-gen` regenerates the defaults. Mouse, keyboard and gamepad all work
in them.

The art direction is pixel art on a 640x360 grid shown at 2x: a night-sky
palette (`motif/palette.gpl`: navy, maroon, gold, steel, teal), CC0 UI pieces
from Kenney's "UI Pack - Pixel Adventure" (`motif/kit/`) recoloured onto that
palette, OFL pixel fonts (Press Start 2P, Jersey 10) rendered at whole multiples of their pixel grid,
and three parallax background layers. `motif/STYLE.md` has the rules, sizes
and prompts for making more. Drop any image into `motif/art/` and
`rugen ui-gen` pixelizes it (`rugen pixelize in.png out.webp --width 640
--dither` does one by hand); `rugen kit-import` re-exports the kit pieces.

For character authors: editing a character's `.cns` / `.cmd` / `.air` while a
match runs reloads it in place (checked once a second), and `rugen check`
lists what the engine cannot run yet.

Broken characters cannot ruin a match: every character is verified the first
time it is used (`rugen verify <char>` / `--all` does it by hand): it is
loaded, inspected (unsupported controllers, missing sprites) and run through
two 3000-frame headless matches. The verdict is stored in its cache manifest
and shown as a green / yellow / red mark on the select screen; `broken`
characters (crash, do not load, get stuck) cannot be picked and `rugen play`
refuses them unless `--force`. The engine also caps helpers, explods,
projectiles and sounds per tick, sanitizes non-finite positions, and if a
character still panics mid-match it is disabled and loses while the other
player keeps playing.

Linux needs `libudev` and `alsa` headers, see [BUILDING.md](BUILDING.md). The `reference/` tree comes from `reference/fetch.sh`.

## Screenpack

The UI is an ordinary MUGEN motif in `motif/` (system.def, fight.def, SFF v2
art, FNT v2 fonts) for a 426x240 pixel grid: dark slanted panels, slashed
accents, pixel italic type. It is generated, not hand-drawn:

```
rugen screenpack-gen [--font bold.ttf] [--out motif/themes/dojo]   # regenerate the fight/system screenpack of a theme
rugen fontgen bold.ttf --size 13 --italic 3 --rim --out motif/font/my.def --preview my.png
```

Edit the `.def` files to re-layout, swap sprites in the SFFs to retheme, or
drop a classic MUGEN screenpack into `data/`; anything missing in `motif/`
falls back to `data/`.

## Hosting a server

```
cargo build --release -p rugen-relay --bin rugen-server
./target/release/rugen-server --port 7777 --data /var/lib/rugen --name "my server"
```

One binary, one data directory (`state.json`, written atomically), one port
number open for **both UDP and TCP**. UDP is the rendezvous (NAT hole
punching), TCP is the JSON API (`GET /` shows it is up). Everything is
rate-limited per IP and validated; there are no passwords, accounts are random
keys the clients keep. Seasons roll every 90 days with a soft rating reset.
Players point `[net] server` at `host:7777`.

## Status

Playable. Every MUGEN file format loads (SFF v1/v2, AIR, SND, ACT, DEF, FNT,
CNS/CMD), the CNS compiler and VM run real community characters with every
controller they use, matches go through rounds with the `fight.def` lifebars,
the screenpack title/select/VS screens work, the camera pans and zooms like
MUGEN 1.1, and online play uses rollback (ggrs) with a sync test that proves
the simulation is deterministic. Stages can declare platforms. See DESIGN.md
for what is left (replays, spectators, matchmaking, creator tools).

## License

Engine: MIT OR Apache-2.0. Reference content under `reference/` keeps its own licenses and is not committed.
