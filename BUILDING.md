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
