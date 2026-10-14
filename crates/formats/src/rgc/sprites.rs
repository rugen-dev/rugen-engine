//! `Sprites` sections: one SFF, decoded once and stored pixel-exact.
//!
//! Every sprite keeps its key, size, axis, palette slot and pixels exactly
//! as [`Sff::parse`] returns them, so the original SFF can be rebuilt and
//! [`Sff::canonical_hash`] is the same before and after. On disk:
//!
//! - fully transparent borders (zero bytes) are cut off and only the
//!   visible block is stored, with its position inside the sprite;
//! - identical blocks are stored once (linked sprites, duplicates);
//! - identical palettes are stored once, palette slots map onto them;
//! - each block is its own zstd frame (level 19), so any sprite decodes
//!   alone, without its neighbours (random access, nothing up front).
//!
//! Layout (little endian, offsets from the section start):
//!
//! ```text
//! header   48 bytes: magic "RSPR", version u16, flags u16 (1 = character),
//!          sff version u8 (0 / 1 / 2), patch u8, pad u16, slots u32,
//!          palettes u32, keys u32, blocks u32, sprites u32, data offset u64, pad[8]
//! palette  ncolors u16, ncolors * rgba
//! slot     palette u32
//! key      group u16, number u16, slot u32
//! block    offset u64 (from data), zstd len u32, raw len u32, w u16, h u16, rgba u8,
//!          free u8 (1: bytes that do not match the sprite size, no geometry), pad[2]
//! sprite   group u16, number u16, w u16, h u16, axis x i16, axis y i16,
//!          block x u16, block y u16, slot u32 (MAX = none), block u32 (MAX = no pixels)
//! data     zstd frames
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::pack::Packed;
use crate::sff::{Key, Palette, Pixels, Sff, Sprite, Version};
use crate::{Error, Result};

const F: &str = "rgc sprites";
pub const MAGIC: &[u8; 4] = b"RSPR";
pub const VERSION: u16 = 1;
const HEADER: usize = 48;
const NONE: u32 = u32::MAX;
/// zstd level of the pixel blocks.
pub const LEVEL: i32 = 19;

/// How decoded sprites are presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Exactly as the SFF had them (export, fingerprints).
    Exact,
    /// For drawing: character sprites show only their visible block (the
    /// axis moves with it, so every texel lands where it did), except
    /// group 9000 (the HUD reads its size) and sprites whose index 0 is
    /// visible. Stage sprites stay whole (stages tile them by their size).
    Runtime,
}

/// What a section saved, for logs and benchmarks.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub sprites: usize,
    pub blocks: usize,
    pub palettes_in: usize,
    pub palettes_out: usize,
    /// Decoded bytes of every sprite.
    pub raw_bytes: usize,
    /// Decoded bytes of the unique visible blocks.
    pub unique_bytes: usize,
    /// zstd bytes.
    pub packed_bytes: usize,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]))
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]))
}

/// Visible rectangle `(x, y, w, h)`: everything outside it is zero bytes.
/// A sprite with nothing visible keeps a 1x1 block (0x0 if it is empty).
fn visible_rect(px: &[u8], w: usize, h: usize, bpp: usize) -> (usize, usize, usize, usize) {
    if w == 0 || h == 0 || px.len() != w * h * bpp {
        return (0, 0, w, h);
    }
    let vis = |x: usize, y: usize| {
        let i = (y * w + x) * bpp;
        px[i..i + bpp].iter().any(|&b| b != 0)
    };
    let row_vis = |y: usize| (0..w).any(|x| vis(x, y));
    let Some(top) = (0..h).find(|&y| row_vis(y)) else {
        return (0, 0, 1, 1);
    };
    let bottom = (0..h).rev().find(|&y| row_vis(y)).unwrap_or(top);
    let left = (0..w)
        .find(|&x| (top..=bottom).any(|y| vis(x, y)))
        .unwrap_or(0);
    let right = (0..w)
        .rev()
        .find(|&x| (top..=bottom).any(|y| vis(x, y)))
        .unwrap_or(w - 1);
    (left, top, right + 1 - left, bottom + 1 - top)
}

fn crop(px: &[u8], w: usize, bpp: usize, (x, y, cw, ch): (usize, usize, usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(cw * ch * bpp);
    for row in y..y + ch {
        let s = (row * w + x) * bpp;
        out.extend_from_slice(&px[s..s + cw * bpp]);
    }
    out
}

struct Block {
    rgba: bool,
    /// Pixel bytes that do not match the sprite's size (kept as parsed).
    free: bool,
    w: u16,
    h: u16,
    px: Vec<u8>,
}

/// Encode a parsed SFF (`is_char`: parsed with `Sff::parse(.., true)`).
pub fn encode(sff: &Sff, is_char: bool) -> (Vec<u8>, Stats) {
    let mut stats = Stats {
        sprites: sff.sprites.len(),
        palettes_in: sff.palettes.len(),
        ..Default::default()
    };
    // palettes: unique contents, slots point at them
    let mut uniq: Vec<&Palette> = Vec::new();
    let mut seen: HashMap<&Palette, u32> = HashMap::new();
    let slots: Vec<u32> = sff
        .palettes
        .iter()
        .map(|p| {
            *seen.entry(p).or_insert_with(|| {
                uniq.push(p);
                (uniq.len() - 1) as u32
            })
        })
        .collect();
    stats.palettes_out = uniq.len();

    let mut blocks: Vec<Block> = Vec::new();
    let mut by_hash: HashMap<u64, Vec<u32>> = HashMap::new();
    let mut rows: Vec<[u8; 24]> = Vec::with_capacity(sff.sprites.len());
    for ((g, n), s) in &sff.sprites {
        let decoded = s.decoded();
        let (rgba, px): (bool, &[u8]) = match decoded.as_ref() {
            Pixels::Indexed(p) => (false, p),
            Pixels::Rgba(p) => (true, p),
            Pixels::Packed(_) => (false, &[]),
        };
        stats.raw_bytes += px.len();
        let (w, h) = (s.width as usize, s.height as usize);
        let bpp = if rgba { 4 } else { 1 };
        let (block, x, y) = if px.is_empty() {
            (NONE, 0, 0)
        } else {
            // pixels that do not fill the sprite (a PNG of another size) are kept as they are
            let free = px.len() != w * h * bpp;
            let rect = if free {
                (0, 0, 0, 0)
            } else {
                visible_rect(px, w, h, bpp)
            };
            let data = if free || rect == (0, 0, w, h) {
                px.to_vec()
            } else {
                crop(px, w, bpp, rect)
            };
            let (bw, bh) = (rect.2 as u16, rect.3 as u16);
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut hs = std::collections::hash_map::DefaultHasher::new();
                (rgba, free, bw, bh, &data).hash(&mut hs);
                hs.finish()
            };
            let cands = by_hash.entry(hash).or_default();
            let b = match cands.iter().find(|&&b| {
                let o = &blocks[b as usize];
                o.rgba == rgba && o.free == free && o.w == bw && o.h == bh && o.px == data
            }) {
                Some(&b) => b,
                None => {
                    stats.unique_bytes += data.len();
                    blocks.push(Block {
                        rgba,
                        free,
                        w: bw,
                        h: bh,
                        px: data,
                    });
                    let b = (blocks.len() - 1) as u32;
                    cands.push(b);
                    b
                }
            };
            (b, rect.0 as u16, rect.1 as u16)
        };
        let slot = s
            .palette
            .map_or(NONE, |p| u32::try_from(p).unwrap_or(NONE - 1));
        let mut row = [0u8; 24];
        row[0..2].copy_from_slice(&g.to_le_bytes());
        row[2..4].copy_from_slice(&n.to_le_bytes());
        row[4..6].copy_from_slice(&s.width.to_le_bytes());
        row[6..8].copy_from_slice(&s.height.to_le_bytes());
        row[8..10].copy_from_slice(&s.offset[0].to_le_bytes());
        row[10..12].copy_from_slice(&s.offset[1].to_le_bytes());
        row[12..14].copy_from_slice(&x.to_le_bytes());
        row[14..16].copy_from_slice(&y.to_le_bytes());
        row[16..20].copy_from_slice(&slot.to_le_bytes());
        row[20..24].copy_from_slice(&block.to_le_bytes());
        rows.push(row);
    }
    stats.blocks = blocks.len();
    let packed = compress_all(&blocks);
    stats.packed_bytes = packed.iter().map(Vec::len).sum();

    let mut out = vec![0u8; HEADER];
    out[..4].copy_from_slice(MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&u16::from(is_char).to_le_bytes());
    let (vk, vp) = match sff.version {
        None => (0u8, 0u8),
        Some(Version::V1) => (1, 0),
        Some(Version::V2 { patch }) => (2, patch),
    };
    out[8] = vk;
    out[9] = vp;
    out[12..16].copy_from_slice(&(slots.len() as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(uniq.len() as u32).to_le_bytes());
    out[20..24].copy_from_slice(&(sff.palette_keys.len() as u32).to_le_bytes());
    out[24..28].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
    out[28..32].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    for p in &uniq {
        let n = p.len().min(u16::MAX as usize);
        out.extend_from_slice(&(n as u16).to_le_bytes());
        for c in p.iter().take(n) {
            out.extend_from_slice(c);
        }
    }
    for s in &slots {
        out.extend_from_slice(&s.to_le_bytes());
    }
    for (&(g, n), &i) in &sff.palette_keys {
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(i as u32).to_le_bytes());
    }
    let mut ofs = 0u64;
    for (b, p) in blocks.iter().zip(&packed) {
        out.extend_from_slice(&ofs.to_le_bytes());
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(&(b.px.len() as u32).to_le_bytes());
        out.extend_from_slice(&b.w.to_le_bytes());
        out.extend_from_slice(&b.h.to_le_bytes());
        out.extend_from_slice(&[u8::from(b.rgba), u8::from(b.free), 0, 0]);
        ofs += p.len() as u64;
    }
    for r in &rows {
        out.extend_from_slice(r);
    }
    let data_ofs = out.len() as u64;
    out[32..40].copy_from_slice(&data_ofs.to_le_bytes());
    for p in &packed {
        out.extend_from_slice(p);
    }
    (out, stats)
}

/// zstd every block, spread over the available cores.
fn compress_all(blocks: &[Block]) -> Vec<Vec<u8>> {
    let z = |b: &Block| {
        if b.px.is_empty() {
            Vec::new()
        } else {
            zstd::bulk::compress(&b.px, LEVEL).unwrap_or_default()
        }
    };
    let threads = crate::encode_threads();
    if threads == 1 || blocks.len() < 16 {
        return blocks.iter().map(z).collect();
    }
    // big blocks first spread evenly: hand out indices from a shared counter
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Vec<u8>> = vec![Vec::new(); blocks.len()];
    let results = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(b) = blocks.get(i) else { break };
                        done.push((i, z(b)));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect::<Vec<_>>()
    });
    for (i, v) in results {
        out[i] = v;
    }
    out
}

/// The header fields of a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub is_char: bool,
    pub version: Option<Version>,
    pub sprites: usize,
}

pub fn read_header(b: &[u8]) -> Result<Header> {
    if b.len() < HEADER || &b[..4] != MAGIC {
        return Err(Error::fmt(F, "not a sprite section"));
    }
    if le16(b, 4) != VERSION {
        return Err(Error::fmt(F, format!("version {}", le16(b, 4))));
    }
    Ok(Header {
        is_char: le16(b, 6) & 1 != 0,
        version: match (b[8], b[9]) {
            (1, _) => Some(Version::V1),
            (2, patch) => Some(Version::V2 { patch }),
            _ => None,
        },
        sprites: le32(b, 28) as usize,
    })
}

/// Load a section. Pixels stay compressed in `bytes` (shared, not copied).
pub fn decode(bytes: Arc<Vec<u8>>, view: View) -> Result<Sff> {
    let header = read_header(&bytes)?;
    let b: &[u8] = &bytes;
    let bad = || Error::fmt(F, "truncated");
    let n_slots = le32(b, 12) as usize;
    let n_pal = le32(b, 16) as usize;
    let n_keys = le32(b, 20) as usize;
    let n_blocks = le32(b, 24) as usize;
    let n_spr = le32(b, 28) as usize;
    let data_ofs = le64(b, 32) as usize;
    if data_ofs > b.len() || n_slots > 1 << 20 || n_spr > 1 << 20 || n_blocks > n_spr.max(1) {
        return Err(bad());
    }
    let mut at = HEADER;
    let mut uniq: Vec<Palette> = Vec::with_capacity(n_pal.min(1 << 16));
    for _ in 0..n_pal {
        let n = le16(b.get(at..at + 2).ok_or_else(bad)?, 0) as usize;
        at += 2;
        let raw = b.get(at..at + n * 4).ok_or_else(bad)?;
        uniq.push(
            raw.chunks_exact(4)
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect(),
        );
        at += n * 4;
    }
    let mut palettes: Vec<Palette> = Vec::with_capacity(n_slots);
    for _ in 0..n_slots {
        let i = le32(b.get(at..at + 4).ok_or_else(bad)?, 0) as usize;
        palettes.push(uniq.get(i).ok_or_else(bad)?.clone());
        at += 4;
    }
    let mut palette_keys = std::collections::BTreeMap::new();
    for _ in 0..n_keys {
        let r = b.get(at..at + 8).ok_or_else(bad)?;
        palette_keys.insert((le16(r, 0), le16(r, 2)), le32(r, 4) as usize);
        at += 8;
    }
    struct B {
        start: usize,
        len: usize,
        raw: usize,
        w: u16,
        h: u16,
        rgba: bool,
        free: bool,
    }
    let mut blocks: Vec<B> = Vec::with_capacity(n_blocks);
    for _ in 0..n_blocks {
        let r = b.get(at..at + 24).ok_or_else(bad)?;
        let start = data_ofs.checked_add(le64(r, 0) as usize).ok_or_else(bad)?;
        let len = le32(r, 8) as usize;
        if start.checked_add(len).is_none_or(|e| e > b.len()) {
            return Err(bad());
        }
        let (w, h, rgba, free) = (le16(r, 16), le16(r, 18), r[20] != 0, r[21] != 0);
        let raw = le32(r, 12) as usize;
        if !free && raw != w as usize * h as usize * if rgba { 4 } else { 1 } {
            return Err(Error::fmt(F, "block size mismatch"));
        }
        if raw as u64 > super::MAX_SECTION {
            return Err(bad());
        }
        blocks.push(B {
            start,
            len,
            raw,
            w,
            h,
            rgba,
            free,
        });
        at += 24;
    }
    let mut sprites: IndexMap<Key, Sprite> = IndexMap::with_capacity(n_spr);
    for _ in 0..n_spr {
        let r = b.get(at..at + 24).ok_or_else(bad)?;
        at += 24;
        let (group, number) = (le16(r, 0), le16(r, 2));
        let (w, h) = (le16(r, 4), le16(r, 6));
        let axis = [le16(r, 8) as i16, le16(r, 10) as i16];
        let (x, y) = (le16(r, 12), le16(r, 14));
        let slot = le32(r, 16);
        let palette = (slot != NONE).then_some(slot as usize);
        let blk = le32(r, 20);
        let mut spr = Sprite {
            group,
            number,
            width: w,
            height: h,
            offset: axis,
            palette,
            pixels: Pixels::Indexed(Vec::new()),
        };
        if blk != NONE {
            let bl = blocks.get(blk as usize).ok_or_else(bad)?;
            if !bl.free
                && (x as usize + bl.w as usize > w as usize
                    || y as usize + bl.h as usize > h as usize)
            {
                return Err(Error::fmt(F, format!("sprite {group},{number}: bad block")));
            }
            let whole = bl.free || (x, y, bl.w, bl.h) == (0, 0, w, h);
            let index0_clear = bl.rgba
                || palette
                    .and_then(|p| palettes.get(p))
                    .and_then(|p| p.first())
                    .is_some_and(|c| c[3] == 0);
            let ax = axis[0] as i32 - x as i32;
            let ay = axis[1] as i32 - y as i32;
            let trim = view == View::Runtime
                && header.is_char
                && !whole
                && group != 9000
                && index0_clear
                && i16::try_from(ax).is_ok()
                && i16::try_from(ay).is_ok();
            let pad = (!whole && !trim).then_some([w, h, x, y, bl.w, bl.h]);
            if trim {
                spr.width = bl.w;
                spr.height = bl.h;
                spr.offset = [ax as i16, ay as i16];
            }
            spr.pixels = Pixels::Packed(Packed::new(
                bytes.clone(),
                bl.start,
                bl.len,
                bl.raw,
                bl.rgba,
                pad,
            ));
        }
        sprites.insert((group, number), spr);
    }
    if at > data_ofs {
        return Err(bad());
    }
    Ok(Sff {
        version: header.version,
        sprites,
        palettes,
        palette_keys,
    })
}

/// A copy of `sff` whose pixels are all decoded (`Indexed` / `Rgba`).
pub fn unpacked(sff: &Sff) -> Sff {
    let mut out = sff.clone();
    for s in out.sprites.values_mut() {
        if let Pixels::Packed(p) = &s.pixels {
            s.pixels = p.decode();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Sff {
        let mut s = Sff {
            version: Some(Version::V2 { patch: 1 }),
            ..Default::default()
        };
        let mut pal = vec![[0u8; 4]; 256];
        pal[1] = [255, 0, 0, 255];
        pal[2] = [0, 255, 0, 255];
        let p0 = s.push_palette((1, 1), pal.clone());
        let p1 = s.push_palette((1, 2), pal.clone()); // duplicate content
        #[rustfmt::skip]
        let px = vec![
            0, 0, 0, 0,
            0, 1, 2, 0,
            0, 0, 1, 0,
        ];
        s.push_indexed((0, 0), 4, 3, [2, 3], p0, px.clone());
        s.push_indexed((0, 1), 4, 3, [-5, 7], p1, px.clone()); // same pixels, other axis
        s.push_indexed((9000, 0), 4, 3, [0, 0], p0, px);
        s.push_indexed((1, 0), 2, 2, [1, 1], p0, vec![0; 4]); // nothing visible
        #[rustfmt::skip]
        let rgba = vec![
            0, 0, 0, 0,  9, 9, 9, 0,
            0, 0, 0, 0,  1, 2, 3, 200,
        ];
        s.push_rgba((2, 0), 2, 2, [0, 0], rgba);
        // an empty sprite (forward link) and one whose index 0 is visible
        s.sprites.insert(
            (3, 0),
            Sprite {
                group: 3,
                number: 0,
                width: 5,
                height: 5,
                offset: [0, 0],
                palette: Some(0),
                pixels: Pixels::Indexed(Vec::new()),
            },
        );
        let mut opaque = pal.clone();
        opaque[0] = [1, 1, 1, 255];
        let p2 = s.push_palette((1, 3), opaque);
        s.push_indexed((4, 0), 3, 1, [1, 0], p2, vec![0, 2, 0]);
        s
    }

    #[test]
    fn exact_view_is_bit_exact() {
        let sff = sample();
        let (bytes, st) = encode(&sff, true);
        assert_eq!(st.palettes_out, 2, "duplicate palettes stored once");
        let back = unpacked(&decode(Arc::new(bytes), View::Exact).unwrap());
        assert_eq!(back.version, sff.version);
        assert_eq!(back.palettes, sff.palettes);
        assert_eq!(back.palette_keys, sff.palette_keys);
        assert_eq!(back.sprites, sff.sprites);
        assert_eq!(back.canonical_hash(), sff.canonical_hash());
    }

    #[test]
    fn runtime_view_trims_like_the_pack() {
        let sff = sample();
        let (bytes, _) = encode(&sff, true);
        let rt = decode(Arc::new(bytes.clone()), View::Runtime).unwrap();
        let a = &rt.sprites[&(0, 0)];
        assert_eq!((a.width, a.height, a.offset), (2, 2, [1, 2]));
        assert_eq!(a.share_key(), rt.sprites[&(0, 1)].share_key(), "one block");
        let p = &rt.sprites[&(9000, 0)];
        assert_eq!((p.width, p.height), (4, 3), "portraits keep their size");
        assert_eq!(*p.decoded(), sff.sprites[&(9000, 0)].pixels);
        let r = &rt.sprites[&(2, 0)];
        assert_eq!(
            (r.width, r.height),
            (1, 2),
            "only zero bytes are cut: transparent rgb stays"
        );
        let o = &rt.sprites[&(4, 0)];
        assert_eq!((o.width, o.height), (3, 1), "visible index 0: whole");
        // stages never trim
        let (sb, _) = encode(&sff, false);
        let st = decode(Arc::new(sb), View::Runtime).unwrap();
        assert_eq!(st.sprites[&(0, 0)].width, 4);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode(Arc::new(b"RSPR".to_vec()), View::Exact).is_err());
        let (mut bytes, _) = encode(&sample(), true);
        bytes.truncate(bytes.len() - 3);
        assert!(decode(Arc::new(bytes), View::Exact).is_err());
    }

    #[test]
    fn reference_sffs_roundtrip() {
        let Some(root) = crate::fixtures::reference("mugen-1.1b1") else {
            eprintln!("reference tree missing: skipped");
            return;
        };
        for rel in [
            "chars/kfm/kfm.sff",
            "chars/kfm/intro.sff",
            "stages/stage0.sff",
            "data/fightfx.sff",
        ] {
            let is_char = rel.starts_with("chars");
            let Ok(sff) = Sff::open(root.join(rel), is_char) else {
                continue;
            };
            let (bytes, _) = encode(&sff, is_char);
            let back = unpacked(&decode(Arc::new(bytes), View::Exact).unwrap());
            assert_eq!(back.sprites, sff.sprites, "{rel}");
            assert_eq!(back.canonical_hash(), sff.canonical_hash(), "{rel}");
        }
    }
}
