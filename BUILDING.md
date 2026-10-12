# Building

Stable Rust 1.85+ (`rustup`). Then `cargo build --release`.

## System dependencies

Only Linux needs anything beyond the Rust toolchain (gamepad + audio backends).

| OS | Install |
|----|---------|
| Fedora | `sudo dnf install systemd-devel alsa-lib-devel` |
| Debian / Ubuntu | `sudo apt install libudev-dev libasound2-dev` |
| Arch | `sudo pacman -S systemd alsa-lib` |
| Windows | nothing |
| macOS | nothing |

## Reference content

`reference/fetch.sh` downloads MUGEN 1.1b1, the official docs, Ikemen GO source and its v1.0.0 content pack. Needed for the test fixtures and the frame-diff harness, not for building.

## Checks CI runs

```
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

## Releases

Push a tag `vX.Y.Z` (or run the Release workflow manually). One job per OS builds, in this order in the release notes:

- **Windows**: x64, x86 (32-bit) and ARM64 zips (cross-compiled on one runner)
- **Linux** x86_64: `.rpm` (`cargo-generate-rpm`), `.deb` (`cargo-deb`) and a portable `.tar.gz`
- **macOS**: Apple Silicon and Intel tarballs

The packages install the binary to `/usr/bin` and `motif/` to `/usr/share/rugen`; the game then keeps player files (chars, stages, cache, `rugen.toml`) in `~/.local/share/rugen`. Zips and tarballs are self-contained folders. CI itself never does release builds, to keep free-tier minutes low.

Local packages: `cargo install cargo-generate-rpm cargo-deb && cargo build --release -p rugen && cargo generate-rpm -p crates/app && cargo deb -p rugen --no-build`.
