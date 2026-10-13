<img src="motif/brand/rugen.png" alt="rugen logo" width="96" align="right">

# rugen

A M.U.G.E.N-compatible 2D fighting game engine in Rust: your existing
characters and stages, unchanged, with rollback netplay.

## Download

Builds for Windows, Linux (`.rpm`, `.deb`, tarball) and macOS are on the
[releases page](https://github.com/rugen-dev/rugen-engine/releases). The
[install guide](https://rugen.dev/getting-started/install/) explains each one.

## Quick start

1. Drop fighters (`.zip`, `.rar`, folders or `.rgc`) into `chars/`.
2. Drop stages (`.zip`, `.rar`, folders or `.rgs`) into `stages/`.
3. Run `rugen`. New content is converted on launch, then the title screen opens.

```sh
rugen                 # the full game
rugen play kfm        # a quick match against the computer
```

## Features

- Loads M.U.G.E.N 1.1 / Ikemen GO content as it is (`.def` `.cns` `.cmd`
  `.air` `.sff` `.snd`), and packs it into compact `.rgc` / `.rgs` files.
- Arcade, VS, Arena (stocks, blast zones, platforms), Training, a Tutorial
  built from each fighter's own moves, and Watch.
- Online play with rollback: rooms, quick match, ranked, spectators, peer to
  peer first with TURN and relay fallbacks.
- Bring your own character: players don't need the same fighters; the server
  judges each matchup and the files sync in the background.
- Every character is verified before its first match, so a broken one can't
  ruin a fight.
- Data-driven menus with two themes (Shonen Ink and Dojo), keyboard, gamepad
  and mouse.
- One codebase for Windows, Linux and macOS.

## Roadmap

**Done**

- [x] M.U.G.E.N 1.1 / Ikemen GO characters and stages, unchanged
- [x] Compact `.rgc` / `.rgs` content files (lossless sprites, Opus sound)
- [x] Arcade, VS, Training, Tutorial, Watch
- [x] Arena mode: stocks, blast zones, platforms, Smash-style HUD
- [x] Rollback netplay: rooms, quick match, ranked, spectators, flee
- [x] Bring your own character, with server-judged matchups
- [x] Accounts, leaderboards, room codes without lookalike characters
- [x] Pause menu with a move list read from each fighter's `.cmd`
- [x] Fighter size option (none / chibi / small / normal)
- [x] Stage select built for hundreds of stages
- [x] Two themes (Ink and Dojo), keyboard, gamepad and mouse, explicit SAVE
- [x] Daily S3 backups for servers

**Next (0.6.2)**

- [ ] Fix: sounds of earlier fighters playing in later fights
- [ ] One-way arena platforms: jump through from below, double-tap down to drop
- [ ] Arena AI: CPU fighters stay on stage, recover and use platforms

**Planned**

- [ ] Training lab: hitbox display, frame data, input display, record / playback dummy
- [ ] Team modes: Simul, Turns, Tag
- [ ] Survival, Boss Rush, Time Attack
- [ ] Storyboards: fighter intros and arcade endings
- [ ] Arena: ledge grab, items to pick up and throw, stage hazards
- [ ] Online: ranked seasons, per-fighter leaderboards, tournament brackets in rooms
- [ ] Shareable replays by code
- [ ] Creator tools: user themes folder with live reload, fighter debugger
- [ ] Translations (French first), Steam Deck layout

**Not planned for now**

- In-game browsing and downloading of fighters from a public catalogue
- Discord Rich Presence

## Build from source

Stable Rust 1.85+. Linux also needs the udev and ALSA headers (see
[BUILDING.md](BUILDING.md)).

```sh
cargo build --release
./target/release/rugen
```

## Documentation

Everything else lives on **[rugen.dev](https://rugen.dev)**:

- Playing: [install](https://rugen.dev/getting-started/install/),
  [first launch](https://rugen.dev/getting-started/first-launch/),
  [characters & stages](https://rugen.dev/getting-started/content/),
  [controls](https://rugen.dev/getting-started/controls/),
  [settings](https://rugen.dev/getting-started/settings/),
  [game modes](https://rugen.dev/playing/modes/),
  [online](https://rugen.dev/playing/online/),
  [CLI reference](https://rugen.dev/playing/cli/)
- Creating: [characters](https://rugen.dev/creating/characters/),
  [stages](https://rugen.dev/creating/stages/),
  [screenpacks & themes](https://rugen.dev/creating/screenpacks/)
- Hosting: [running a server](https://rugen.dev/server/overview/)
- Engine: [architecture](https://rugen.dev/engine/architecture/),
  [compatibility status](https://rugen.dev/engine/compatibility/),
  [building](https://rugen.dev/engine/building/),
  [contributing](https://rugen.dev/engine/contributing/)

## Related repositories

- [rugen-server](https://github.com/rugen-dev/rugen-server): the online hub
  (lobby, rooms, ranked, relay, matchups).
- [rugen-docs](https://github.com/rugen-dev/rugen-docs): the source of
  rugen.dev.

## Contributing

Read the [contributing guide](https://rugen.dev/engine/contributing/), then
[DESIGN.md](DESIGN.md) for the architecture and the rules the code follows.
[BUILDING.md](BUILDING.md) has the system dependencies and the checks CI runs.

## License

MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)). Content under `reference/` keeps its own
licenses and is not committed.
