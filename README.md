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
```

Linux needs `libudev` and `alsa` headers, see [BUILDING.md](BUILDING.md). The `reference/` tree comes from `reference/fetch.sh`.

## Status

Phase 0 done: all MUGEN file formats load, a wgpu viewer plays character animations with collision boxes. Next: the CNS state machine. See DESIGN.md.

## License

Engine: MIT OR Apache-2.0. Reference content under `reference/` keeps its own licenses and is not committed.
