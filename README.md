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

Keys live in `rugen.toml` (written on first run). Defaults: P1 arrows +
`a s d` / `z x c` / Enter, P2 `i j k l` + `b n m` / `t y u` / Shift. Gamepads
work out of the box. In a match: Space pause, `.` step, F2 hitboxes and
platforms, F3 state / frame data / input overlay, F1 debug title, +/- zoom,
Esc back.

Online from the menu: set `[net] remote = "ip:port"`, `side = 1` or `2`, and
the `p1` / `p2` / `stage` ids in `rugen.toml`; a NETPLAY entry then appears on
the title screen (both machines need the same content). Menu matches are
recorded into `replays/`; `rugen play ... --record file.json` records quick
play and `rugen replay file.json` plays a recording back.

For character authors: editing a character's `.cns` / `.cmd` / `.air` while a
match runs reloads it in place (checked once a second), and `rugen check`
lists what the engine cannot run yet.

Linux needs `libudev` and `alsa` headers, see [BUILDING.md](BUILDING.md). The `reference/` tree comes from `reference/fetch.sh`.

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
