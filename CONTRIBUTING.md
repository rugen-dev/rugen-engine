# Contributing to rugen

Thanks for helping. Bug reports, fixes, compatibility work on real MUGEN
content, themes and docs are all welcome.

## Licence

rugen is dual-licensed under the MIT licence and the Apache License 2.0
(`LICENSE-MIT`, `LICENSE-APACHE`). Unless you explicitly state otherwise,
any contribution you intentionally submit for inclusion in rugen is
licensed MIT OR Apache-2.0, as defined in the Apache-2.0 licence, without
any additional terms or conditions.

## No copyrighted content in pull requests

Do not add characters, stages, screenpacks, sprites, sounds, music, fonts or
any other files you do not have the right to share under the licences
above. That includes MUGEN's own files and anything ripped from a
commercial game. Tests that need real content read it from `reference/`
(fetched by `reference/fetch.sh`, never committed) and skip when it is
missing; keep it that way. Fonts and art the project ships are either drawn
in code (`rugen ui-gen`, `rugen stage-gen`) or carry an open licence next to
them (`motif/font/src/OFL-*.txt`, `motif/kit/`).

## Before you start

- Read `DESIGN.md`: it explains the crates, the determinism rules of the
  simulation and how netplay works. `BUILDING.md` lists the system packages.
- For anything bigger than a fix, open an issue first so we can agree on
  the approach.
- The simulation (`crates/sim`) must stay deterministic: no `HashMap`
  iteration, no platform `f32` transcendentals (use `libm`), no `mul_add`,
  all state in the snapshot. A change to what the simulation computes needs
  a regression test and a note in the pull request: it changes replays and
  online matches.
- UI changes go into both themes (Shonen Ink and Dojo; see
  `motif/STYLE.md`), with a screenshot of each.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/):
`type(scope): subject`, lower case, imperative or descriptive, no full stop.

```
fix(sim): fighters of different localcoord measure each other in their own units
feat(online): the host picks the stage on the stage select screen
docs(stages): the [Arena] sample matches the Ink Arena numbers
```

Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`,
`chore`. The scope is the crate or the area (`sim`, `net`, `ui`, `online`,
`stagesel`, `render`...). Use the body to say why, when it is not obvious.
One logical change per commit.

## Checks

Run these before you push (CI runs them too):

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Screens can be checked without a person at the keyboard: `rugen --frames N
--shot out.png [--open online|options|controls] [--keys 90:Enter]
[--mouse 60:640,360,90:click]` and `rugen play c1 c2 --stage s --frames N
--shot out.png` write a screenshot of frame N.

## Pull requests

Fill in the template: what changed and why, how you tested it, screenshots
for UI work (both themes), and whether the simulation or the online
protocol changed. Keep pull requests focused; a refactor and a feature are
easier to review apart.
