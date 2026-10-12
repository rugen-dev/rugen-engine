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

## Official server

The official rugen server is baked into the game at **compile time** (one
place: `crates/fair/src/trust.rs`, `OFFICIAL_SERVER` / `OFFICIAL_KEY`):

| variable | meaning | default |
|---|---|---|
| `RUGEN_OFFICIAL_SERVER` | address pinned first in ONLINE, default feed source of community servers | `https://play.rugen.dev` |
| `RUGEN_OFFICIAL_KEY` | the official server's Ed25519 **public** key (64 hex digits or base64) | the development key (see below) |

```sh
RUGEN_OFFICIAL_KEY=<public key hex> cargo build --release -p rugen
```

The default key is a **development key whose private half is public**
(`DEV_SIGNING_SEED` in `trust.rs`, used by tests and local servers): a build
without `RUGEN_OFFICIAL_KEY` trusts anybody who copies that seed, so never
ship one. Before the first release with the official server:

1. Generate the key pair once: `rugen-server --gen-key` prints
   `RUGEN_SIGNING_KEY=<seed>` (secret) and `RUGEN_OFFICIAL_KEY=<public>`.
2. Give the **seed** to the official server only (secret env
   `RUGEN_SIGNING_KEY` or a `signing.key` file in its data volume; see the
   rugen-server deploy guide). Never commit it.
3. Set the **public** key as the repository variable `RUGEN_OFFICIAL_KEY`
   (Settings > Secrets and variables > Actions > Variables). The Release
   workflow passes it to every build and refuses to release without it;
   `RUGEN_OFFICIAL_SERVER` can be set the same way to override the address.

Local testing against your own servers: run one `rugen-server` with
`RUGEN_SIGNING_KEY=<DEV_SIGNING_SEED>` (it becomes "official" for a default
build) and point the game at it with
`RUGEN_OFFICIAL_SERVER=http://127.0.0.1:8080 cargo build --release -p rugen`.

Signed statements are Ed25519 (ring) over canonical JSON: keys sorted by
bytes, no whitespace, integers only, serde_json string escaping; the signed
message is `rugen-signed-v1\n<purpose>\n<payload>` (purposes `truth`,
`verdict`, `result`, `identity`). Feeds expire (3 days, at most 7) and are
never accepted with a lower sequence than the one already held.
