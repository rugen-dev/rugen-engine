# rugen

MUGEN-compatible 2D fighting game engine in Rust.

- Loads existing M.U.G.E.N / Ikemen GO content unchanged (`.def` `.cns` `.cmd` `.air` `.sff` `.snd`).
- Runs on Windows, Linux, macOS from one codebase.
- Rollback netplay designed in from the first commit.

See [DESIGN.md](DESIGN.md) for architecture and phase plan, [reference/README.md](reference/README.md) for reference material.

## Build

```
cargo build --release
cargo run -p rugen -- view reference/mugen-1.1b1/chars/kfm/kfm.def 200
cargo run -p rugen -- play reference/mugen-1.1b1/chars/kfm/kfm.def
```

In `play`: arrows move, Z X C = a b c, A S D = x y z, Enter = start, Space pause, `.` step, B boxes, Esc quit.

Linux needs `libudev` and `alsa` headers, see [BUILDING.md](BUILDING.md). The `reference/` tree comes from `reference/fetch.sh`.

## Status

Phases 0 and 1 done: all MUGEN file formats load, the CNS compiler and VM run KFM's real state files, and `rugen play` lets you walk, jump and attack with KFM on a flat stage. Next: hit detection, player push, HitDef and the get-hit states (Phase 2). See DESIGN.md.

## License

Engine: MIT OR Apache-2.0. Reference content under `reference/` keeps its own licenses and is not committed.
