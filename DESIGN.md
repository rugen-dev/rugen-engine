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
    formats/    # .sff .air .snd .act .fnt parsers. Zero engine deps. Fuzzable.
    cns/        # lexer, Pratt expression parser, bytecode compiler, VM
    sim/        # deterministic game simulation. No I/O, no floats-from-libm, no render.
    net/        # ggrs integration, transport, matchmaking client, desync detection
    render/     # wgpu renderer, camera, shaders, palette-on-GPU
    audio/      # kira
    app/        # winit, gilrs, config, UI glue; the binary
  assets/
  docs/
```

Rule: `sim` must compile without `render`, `audio`, `app`. If it can't, netplay is already broken.

### Determinism (netplay foundation)

- Fixed-point math (`fixed` crate, e.g. `I32F32`) for all sim values. No `f32` in sim state.
- No libm calls in sim. Own trig/sqrt tables.
- Sim state = plain data, arena + indices. No `Rc`, no `Box<dyn>`, no pointers. Must be `Clone` and cheap.
- PRNG seeded per match, lives inside state. CNS `Random` reads from it.
- Ordered containers only (`Vec`, `BTreeMap`, `IndexMap`). Never `HashMap` iteration in sim.
- CNS VM state is part of the snapshot.
- Sim runs fixed 60 Hz. Render reads state, never mutates it.
- Target: full frame step < 2 ms so 7-frame rollback fits in budget.

### Netplay

- `ggrs` for rollback. Use its sync-test session mode in CI from day one; it catches nondeterminism locally with no network.
- UDP, NAT hole-punch via tiny matchmaking/STUN server. Relay fallback for symmetric NAT.
- Per-frame state hash exchanged; desync surfaces as an error, never silent drift.
- Content hashing: both sides verify char/stage hashes before match.
- Replays = input log + content hashes. Spectating = same stream.

### Cross-platform stack (pure Rust, no C deps)

| Need | Crate | Why |
|------|-------|-----|
| GPU | `wgpu` | Vulkan / DX12 / Metal, one API |
| Window | `winit` | |
| Gamepads | `gilrs` | XInput / evdev / IOKit |
| Audio | `kira` (cpal) | |
| Math | `fixed` | determinism |
| Rollback | `ggrs` | |
| Lexing | `logos` | |
| Config | `serde` + `toml` | |

CI: GitHub Actions matrix on all three OS, artifacts per commit. macOS universal binary, notarized.

## Phase plan

### Phase 0 — skeleton
- Workspace, crates above, CI green on all three OS with a window showing one sprite.
- `formats`: SFF v1/v2 + AIR loader. Sprite/anim viewer in `app`.

### Phase 1 — one character stands
- CNS lexer, parser, bytecode VM.
- Trigger + state controller coverage tracked in `docs/cns-coverage.md`.
- KFM loads, idles, walks, attacks a dummy on a flat stage.
- `ggrs` sync-test running every frame in debug builds.

### Phase 2 — a real match
- Hit detection, juggle, hitpause, guard, projectiles, helpers, explods.
- Stage loader with camera bounds, parallax, native smooth zoom.
- Rounds, timer, win/lose. Local versus.
- Frame-by-frame diff harness against Ikemen GO recordings.

### Phase 3 — online
- Real UDP sessions, matchmaking server, relay fallback.
- Content hash check, desync detection, replays, spectator.
- Input delay/rollback window tuning UI.

### Phase 4 — creator tools
- Hitbox/hurtbox overlay, frame data overlay, input display.
- Hot reload of `.cns` / `.air` / `.cmd` on file change.
- Real error messages with file:line.
- CNS debugger: break on state, watch triggers, step frames.

### Phase 5 — new engine features (`engineversion = 2`)
- Interactive stages: `[Platform]`, `[Zone]`, `[Interact]` sections. Legacy stages = one implicit floor.
- New triggers (`GroundY`, `OnPlatform`, `StageZone`) that return flat values for legacy chars.
- Ground query replaces `y = 0` assumption in landing / fall logic.
- Stage transitions, tag/assist, scriptable round rules (Lua or Rhai).
- Mod packs: single archive + manifest + hashes.
- wasm target.

## Open questions

- Fixed-point vs emulating MUGEN f32 exactly: fixed wins for netplay; measure drift vs Ikemen on real chars.
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
