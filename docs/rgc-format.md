# rugen content files: `.rgc` and `.rgs`

A character (`.rgc`) or a stage (`.rgs`) in one file: every file of the
original MUGEN folder, with sprites stored pixel-exact, sounds as Opus,
loose PNG images as lossless WebP and everything else byte for byte, plus
the logic compiled by the engine that converted it. The engine plays the
file in place (nothing is unpacked); `rugen export` gives the MUGEN folder
back.

Reader / writer: `rugen_formats::rgc` (container, `sprites`, `sounds`),
`rugen_formats::vfs` (paths into a content file). Converter:
`rugen_content::convert`. Export: `rugen_content::export`. Verification of
a file received from someone else: `rugen_fair::package::verify`.

All integers are little endian. Offsets are from the start of the file
unless stated otherwise.

## Container

```text
offset  size  field
0       8     magic: "RUGENRGC" (character) or "RUGENRGS" (stage)
8       2     major version = 1   (a reader refuses any other major)
10      2     minor version = 0   (additions older readers can skip)
12      4     flags (0)
16      8     TOC offset
24      4     TOC length (stored, zstd)
28      4     TOC length decompressed
32      32    fingerprint (sha256, raw) recorded at conversion
64      ...   sections, each at its own offset
...           TOC
```

The header is written last (the writer streams the sections, then the
TOC, then seeks back), so a file is complete only once its header names a
TOC. Readers only need the header and the TOC to know everything; each
section is read (and decompressed) on its own, so the file can be memory
mapped or read with positional reads.

The recorded fingerprint is a claim: anything that trusts a file it did not
convert itself recomputes it from the content (see Fingerprint).

### Table of contents

zstd-compressed:

```text
count u32
entry * count:
  kind u8        1 Meta, 2 File, 3 Sprites, 4 Sounds, 5 Image, 6 Logic
                 (unknown kinds from a newer minor are skipped)
  codec u8       0 stored, 1 zstd (the whole section)
  flags u16      per kind, below
  offset u64
  len u64        bytes stored
  raw_len u64    bytes after decompression (= len when stored)
  src_len u64    size of the source file it was converted from (0 if unknown)
  src_mtime u64  its modification time in ns (0 if unknown)
  hash [32]      File: sha256 of the bytes; Sprites: canonical sprite hash;
                 Sounds / Image: sha256 of the original file;
                 Meta / Logic: sha256 of the payload
  path_len u16
  path           original path relative to the content root, `/` separated,
                 original case; empty for Meta and Logic
```

Readers reject: more than 16384 entries, a section outside the file, a
decompressed size above 1 GiB, a stored section whose two lengths differ,
an unsafe path (empty, absolute, `\`, `:`, NUL, `.` / `..` / empty parts,
over 1024 bytes) and two file sections whose paths are equal ignoring case.
Lookups are case-insensitive and accept `\` (MUGEN content mixes both).

`src_len` / `src_mtime` let a re-conversion of a folder (a creator's
working copy) copy unchanged sections verbatim.

### Section kinds

| kind | content |
|------|---------|
| Meta (1) | JSON: `def` (main def, relative path), `name`, `author`, `converter` (`rugen x.y.z / converter n`), `created` (unix seconds), `source` (archive or folder name), `engine_files` (paths the converter copied in from the engine's `data/`, e.g. `common1.cns`, so the file is self-contained) |
| File (2) | one original file, byte for byte. zstd level 19 when that saves at least 2 %; already-compressed formats (mp3, ogg, jpg, png, ...) are stored |
| Sprites (3) | one `.sff`, see below. Flag 1: parsed as the character's sprite file (SFFv1 palette rule) |
| Sounds (4) | one `.snd`, see below |
| Image (5) | a loose `.png` as lossless WebP (stored). Flag 1: the original was a PNG (exported back as PNG). Only 8-bit PNGs, and only when the WebP is smaller |
| Logic (6) | `schema_len u16, schema, payload`: the AIR / CMD / CNS compiled by the converting engine (bincode of `rugen_sim` structures). Used only when `schema` equals the running engine's `LOGIC_SCHEMA`; otherwise the text is compiled again and kept in `cache/logic/` |

A sprite or sound file that does not decode is stored as a File section,
byte for byte, so conversion never loses anything.

## Sprites section

Each sprite keeps its key, size, axis, palette slot and pixels exactly as
the SFF parser returns them, so the original SFF is rebuilt pixel-exact and
the canonical hash is the same before and after.

```text
header   48 bytes: magic "RSPR", version u16 = 1, flags u16 (1 = character),
         sff version u8 (0 / 1 / 2), patch u8, pad u16,
         slots u32, palettes u32, keys u32, blocks u32, sprites u32,
         data offset u64 (from the section start), pad[8]
palette  * palettes: ncolors u16, ncolors * RGBA
slot     * slots: palette u32           (palette slot → stored palette)
key      * keys: group u16, number u16, slot u32   (SFFv2 palette keys)
block    * blocks: offset u64 (from data), zstd len u32, raw len u32,
         w u16, h u16, rgba u8, free u8, pad[2]
sprite   * sprites (file order): group u16, number u16, w u16, h u16,
         axis x i16, axis y i16, block x u16, block y u16,
         slot u32 (MAX = none), block u32 (MAX = no pixels)
data     zstd frames, one per block
```

- **Trim**: borders of zero bytes are cut; a block is the visible
  rectangle, placed at (block x, block y) inside the `w x h` sprite and
  padded back with zeros on decode. A sprite with nothing visible keeps a
  1x1 block. (`free = 1`: pixel bytes that do not match the sprite size,
  e.g. a PNG sprite of another size, kept as they are.)
- **Dedup**: identical blocks (linked sprites, duplicates, frames that only
  differ by their axis) are stored once; identical palettes once (SFFv1
  gives every sprite its own copy).
- **Random access**: every block is its own zstd frame (level 19); a sprite
  decodes alone. In memory the section stays compressed and a sprite is
  decoded when first drawn.
- **Drawing view**: character sprites are handed to the renderer as their
  visible block with the axis moved by the same amount (every texel lands
  where it did), except group 9000 (the HUD reads its size), sprites whose
  palette index 0 is visible and stage sprites (stages tile by size).

Measured on G6 Naruto (1243 sprites, 96 MB of trimmed unique pixels):
LZ4 30.2 MB, zstd 3 21.5 MB, zstd 19 17.9 MB, zstd 19 with a trained
dictionary 18.0 MB, zstd 19 XOR-delta against the previous frame 17.9 MB,
zstd 19 with the previous frame as dictionary 17.5 MB, zstd 19 solid 1 MB
blocks 17.3 MB, LZMA (lc4 pb0) 16.5 MB but decoding 8x slower than zstd
(0.8 s vs 0.1 s for every sprite on one core). Per-sprite zstd 19 is within
4 % of the solid / delta variants and keeps random access and fast first
draws.

### Rebuilding the SFF (export)

`Sff::write_exact` writes the same version: SFFv1 as PCX sprites, each
followed by its palette unless it shares the previous sprite's (`samepal`),
linked duplicates, and palette slots that only dropped duplicate sprites
introduced written as dropped duplicates again, so every slot keeps its
number (SFFv1 `.act` palettes recolour by slot); SFFv2 with RLE8 indexed
sprites, PNG (format 12) true-colour sprites, links for duplicates, the
original patch level and palette keys. Parsing the exported file gives the
same sprites, palettes and keys (in both the character and the stage parse
mode).

## Sounds section

```text
header  32 bytes: magic "RSND", version u16 = 1, flags u16,
        snd version u16 u16, sounds u32, data offset u64, pad[8]
sound   40 bytes * sounds: group i32, number i32,
        codec u8 (0 = WAV bytes, 1 = Opus), channels u8, zstd u8, pad u8,
        rate u32, frames u32, preskip u32,
        offset u64 (from data), len u32, raw len u32
data    WAV bytes (zstd when the flag says so), or Opus packets as
        (len u16, packet) pairs
```

A PCM sound (8/16/24/32-bit integer or 32-bit float, mono or stereo) is
resampled with a windowed sinc (Blackman, 16 zero crossings) to the lowest
Opus rate at or above its own (8 / 12 / 16 / 24 / 48 kHz: 11.025 kHz →
12 kHz, 22.05 kHz → 24 kHz, 44.1 kHz → 48 kHz) and encoded in 20 ms
packets, VBR, complexity 10, application `audio`, at 32 kbit/s per channel
at 48 kHz (24 at 24 kHz, 20 at 16 kHz, 16 at 12 kHz, 12 at 8 kHz; stereo
twice that). `preskip` is the encoder look-ahead; decoding drops it and
keeps exactly `frames` samples, so the sound lasts as long as before (to
the sample, at its new rate) and the mixer plays it at the rate its WAV
header names. Sounds that are not plain PCM, or where Opus would not be
smaller (tiny sounds), keep their WAV bytes. Sounds decode to 16-bit WAV
on first use and are then kept; a fight decodes its fighters' sounds on a
background thread right after loading.

Codec: [unsafe-libopus](https://crates.io/crates/unsafe-libopus), the
reference libopus 1.3.1 translated to Rust with c2rust (encoder and
decoder). It needs no C compiler, cmake, pkg-config or system library, so
it builds the same on every release target; every Opus binding that builds
libopus from C (`opus`, `audiopus_sys`, `opusic-sys`, `opus-static-sys`,
`opus-head-sys`, `opus-codec`) needs cmake or pkg-config, and the pure-Rust
re-implementations were either decoder-only or young. The unsafe calls are
wrapped in `crates/opus`.

## Fingerprint

`rugen-fair` fingerprint, tag `rugen-fp-2`: sha256 over the kind, the main
def and the sorted rows of the def's canonical file set (the def and what
`[Files]` / `[BGdef]` reference, plus the common states file):

- text files and palettes: `raw:<sha256 of the bytes>`;
- the sprite file: `spr:<canonical sprite hash>`: sha256 over the SFF
  version (v1 / v2), every palette slot, the palette keys and every
  sprite's key, size, axis, slot and decoded pixels in file order
  (`Sff::canonical_hash`);
- the sound file: no row (audio is not gameplay).

It does not depend on the container (folder, zip, rar, `.rgc`), junk
files, file order, how the sprites are compressed or how the sounds are
encoded, so converting, exporting and converting again keep it, and two
machines converting the same character get the same value. It is the key
of online matchups, the official blocklist and character ratings.

### Older games online (API 4)

rugen 0.5 and earlier (lobby API 4) sent RGPK packages (the canonical files
byte for byte, zstd) fingerprinted with `rugen-fp-1` (sha256 of every
file's bytes, sounds included). Neither side can read the other's
packages, and the same character has different fingerprints in the two
generations, so players of different APIs can never share a match:

- Clients send `X-Rugen-Api: 5` on every request; a request without it is
  API 4. A server serving both says `api: 5, api_min: 4` in `/health`
  (0.5 clients never read `api`), never pairs different APIs in its
  queue, refuses joining a room whose host runs the other API (with a
  message saying who must update) and marks those rooms in its lobby
  (`api` on each room).
- This game refuses a server that serves API 4 only, and says why when a
  downloaded package is an RGPK / zip (an opponent on an older game, on
  such a server) instead of a content-file parse error.
- Blocklist entries and ratings stay keyed by fingerprint: a `rugen-fp-1`
  entry does not cover the `.rgc` of the same character. Block the
  character again by its `rugen-fp-2` fingerprint.

## Content library rules

- `chars/` / `stages/`: an archive (`.zip`, `.rar`) is converted on launch
  into `<id>.rgc` / `<id>.rgs` next to it. The file is written under a
  hidden name, read back and its fingerprint recomputed from its content;
  only then is it renamed into place and the archive moved to `originals/`
  (`[content] keep_originals = true`, the default) or deleted (`false`).
  `originals/` is never scanned.
- A folder is a creator's working copy: never moved or deleted. It is
  converted into `cache/rgc/`, again whenever a file in it changes (size,
  mtime, name); unchanged sprite / sound sections are copied from the
  previous conversion, so a `.cns` edit converts in a moment, and a
  running fight reloads the character (hot reload). A folder wins over a
  content file with the same name.
- A content file dropped in is used as it is.
- `rugen convert <folder|archive> [out]` writes a shareable file without
  touching the source; `rugen export <file> [dir]` writes the folder back
  (sprites identical, sounds as decoded WAV, images as PNG; the files the
  converter copied from `data/` are left out).

## Measurements

Same machine (12 threads), engine v0.5.0 before (archive + extracted copy +
`.rugen-opt/` optimiser cache) and after (content file). Sizes in MB.

| | KFM (folder) | G6 Naruto (.rar) | G6 Sasuke Rinnegan (.rar) | stage0-720 (.zip) |
|---|---|---|---|---|
| original | 0.59 | 41.2 (71 extracted) | 45.2 (86 extracted) | 0.08 |
| before, on disk | 1.95 | 147.8 | 179.9 | 0.80 |
| after: content file | 0.29 | **20.9** | **26.5** | 0.07 |
| sprites | 0.21 | 18.49 (SFF 41.4) | 24.85 (SFF 60.5) | 0.07 |
| sounds | 0.01 | 1.68 (SND 31.7) | 1.48 | – |
| logic (compiled) | 0.03 | 0.10 | 0.10 | – |
| other files | 0.03 | 0.66 | 0.06 | 0.01 |
| conversion | 0.3–0.8 s | 7–12 s | 11–12 s | 0.7 s |
| cached prepare (menus) | 0.3 → 0.1 ms | 15–30 → 0.1 ms | 17–31 → 0.1 ms | 0.1 → 0.0 ms |
| fight load | 4.7 → 4.6 ms | 14–24 → 8–15 ms | 14–55 → 10–17 ms | 0.1 → 0.1 ms |

Conversion includes extracting the archive, encoding, writing and reading
the file back to recompute its fingerprint. Before, the first import took
1.2 s (Naruto) / 1.3 s (Sasuke) but left 106 / 135 MB of cache. Fight load
ranges are medians of repeated runs on a busy machine (KFM: mean of 100
loads, same harness). With the original archives kept in `originals/`
(the default), add their size. Naruto's 0.59 MB sprite-sheet PNG stays a
PNG: `image-webp`'s lossless encoder made it 0.84 MB.

Rollback is unchanged (Naruto vs Sasuke, `crates/sim/examples/rollbench.rs`):
save / load / step 9.7 / 9.3 / 61.3 µs from `.rgc` vs 9.9 / 9.5 / 60.1 µs
before, same final checksum.
