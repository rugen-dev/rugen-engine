//! Optimised sprite pack (`.rspk`): an SFF decoded once at cache time.
//!
//! Loading a pack is one file read plus an index walk; pixels stay
//! LZ4-compressed in memory ([`Packed`]) until a sprite is first uploaded.
//! Compared to the source SFF the pack
//!
//! - stores identical pixel data once (linked sprites, duplicates),
//! - stores identical palettes once (SFFv1 gives every sprite its own copy),
//!   keeping the palettes `apply_v1_act` recolours apart from the rest,
//! - trims fully transparent borders of character sprites, moving the axis
//!   by the same amount so every texel lands where it did (never group 9000,
//!   whose size the HUD reads, never sprites whose index 0 is visible).
//!
//! It is a derived local cache: the fairness fingerprint, online packages and
//! verification always read the original files. Layout (little endian):
//!
//! ```text
//! header   magic "RUGENSPK", format u32, flags u32 (1 = is_char, 2 = trimmed),
//!          source len u64, source mtime ns u64, sff version u8 (0/1/2), patch u8, pad u16,
//!          palettes u32, palette keys u32, blobs u32, sprites u32, data offset u64, pad u32
//! palette  ncolors u16, ncolors * rgba
//! pal key  group u16, number u16, palette u32
//! blob     offset u64 (from data offset), packed len u32, raw len u32, rgba u8, pad[3]
//! sprite   group u16, number u16, w u16, h u16, axis x i16, axis y i16, palette u32 (MAX = none), blob u32
//! data     LZ4 blocks
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::sff::{Key, Palette, Pixels, Sff, Sprite, Version};
use crate::{Error, Result};

const F: &str = "rspk";
pub const MAGIC: &[u8; 8] = b"RUGENSPK";
/// Bump when the layout or the conversion rules change.
pub const FORMAT: u32 = 1;
const HEADER: usize = 64;
const NONE: u32 = u32::MAX;

/// LZ4-compressed pixels of one sprite, sharing the pack's buffer.
#[derive(Clone)]
pub struct Packed {
    buf: Arc<Vec<u8>>,
    start: usize,
    len: usize,
    raw_len: usize,
    rgba: bool,
}

impl Packed {
    /// Decompress into `Indexed` / `Rgba`. Corrupt data decodes to transparent pixels.
    pub fn decode(&self) -> Pixels {
        let src = self
            .buf
            .get(self.start..self.start + self.len)
            .unwrap_or(&[]);
        let px = lz4_flex::block::decompress(src, self.raw_len)
            .ok()
            .filter(|v| v.len() == self.raw_len)
            .unwrap_or_else(|| vec![0; self.raw_len]);
        if self.rgba {
            Pixels::Rgba(px)
        } else {
            Pixels::Indexed(px)
        }
    }

    /// Same value for sprites sharing these pixels.
    pub fn share_key(&self) -> (usize, usize) {
        (Arc::as_ptr(&self.buf) as usize, self.start)
    }

    pub fn is_rgba(&self) -> bool {
        self.rgba
    }
    /// Decompressed size in bytes.
    pub fn raw_len(&self) -> usize {
        self.raw_len
    }
    /// Compressed size in bytes.
    pub fn packed_len(&self) -> usize {
        self.len
    }
}

impl std::fmt::Debug for Packed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Packed({} {} -> {} bytes)",
            if self.rgba { "rgba" } else { "indexed" },
            self.len,
            self.raw_len
        )
    }
}

impl PartialEq for Packed {
    fn eq(&self, o: &Self) -> bool {
        self.rgba == o.rgba
            && self.raw_len == o.raw_len
            && self.buf.get(self.start..self.start + self.len)
                == o.buf.get(o.start..o.start + o.len)
    }
}
impl Eq for Packed {}

/// Size and modification time of the source file the pack was built from;
/// a pack whose stamp no longer matches is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Stamp {
    pub len: u64,
    pub mtime_ns: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let mtime_ns = m
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos() as u64;
        Some(Self {
            len: m.len(),
            mtime_ns,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The SFF was parsed as a character's (`Sff::parse(.., true)`).
    pub is_char: bool,
    /// Trim transparent borders (characters only: stages tile sprites by their size).
    pub trim: bool,
}

/// What a pack saved, for logs and benchmarks.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub sprites: usize,
    pub blobs: usize,
    pub palettes_in: usize,
    pub palettes_out: usize,
    /// Decoded pixel bytes of every sprite (what `Sff::parse` keeps in memory).
    pub raw_bytes: usize,
    /// Decoded bytes of the unique, trimmed blobs (texture memory when all are uploaded).
    pub unique_bytes: usize,
    /// LZ4 bytes on disk.
    pub packed_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: u32,
    pub is_char: bool,
    pub trimmed: bool,
    pub stamp: Stamp,
}

/// Read only the header (validity checks before loading).
pub fn read_header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < HEADER || &bytes[..8] != MAGIC {
        return Err(Error::fmt(F, "not a sprite pack"));
    }
    let flags = le32(bytes, 12);
    Ok(Header {
        format: le32(bytes, 8),
        is_char: flags & 1 != 0,
        trimmed: flags & 2 != 0,
        stamp: Stamp {
            len: le64(bytes, 16),
            mtime_ns: le64(bytes, 24),
        },
    })
}

/// Header of the pack file at `path`, reading just its first bytes.
pub fn read_header_file(path: &Path) -> Result<Header> {
    use std::io::Read;
    let mut b = [0u8; HEADER];
    std::fs::File::open(path)?.read_exact(&mut b)?;
    read_header(&b)
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

/// One unique block of pixels before compression.
struct Blob {
    rgba: bool,
    w: u16,
    px: Vec<u8>,
}

/// Visible bounding box `(x, y, w, h)`; `None` when nothing is visible.
fn visible_rect(px: &[u8], w: usize, h: usize, rgba: bool) -> Option<(usize, usize, usize, usize)> {
    let bpp = if rgba { 4 } else { 1 };
    let vis = |x: usize, y: usize| {
        let i = (y * w + x) * bpp;
        if rgba {
            px.get(i + 3).is_some_and(|&a| a != 0)
        } else {
            px.get(i).is_some_and(|&p| p != 0)
        }
    };
    let row_vis = |y: usize| (0..w).any(|x| vis(x, y));
    let top = (0..h).find(|&y| row_vis(y))?;
    let bottom = (0..h).rev().find(|&y| row_vis(y))?;
    let left = (0..w).find(|&x| (top..=bottom).any(|y| vis(x, y)))?;
    let right = (0..w).rev().find(|&x| (top..=bottom).any(|y| vis(x, y)))?;
    Some((left, top, right + 1 - left, bottom + 1 - top))
}

fn crop(px: &[u8], w: usize, bpp: usize, (x, y, cw, ch): (usize, usize, usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(cw * ch * bpp);
    for row in y..y + ch {
        let s = (row * w + x) * bpp;
        out.extend_from_slice(&px[s..s + cw * bpp]);
    }
    out
}

/// Palettes `Sff::apply_v1_act` recolours: those of `0,0` and `9000,0` in an SFFv1.
fn act_targets(sff: &Sff) -> Vec<usize> {
    if sff.version != Some(Version::V1) {
        return Vec::new();
    }
    [(0, 0), (9000, 0)]
        .iter()
        .filter_map(|k| sff.sprites.get(k).and_then(|s| s.palette))
        .collect()
}

/// Build a pack from a parsed (not yet act-recoloured) SFF.
pub fn write(sff: &Sff, opts: Options, stamp: Stamp) -> (Vec<u8>, Stats) {
    let mut stats = Stats {
        sprites: sff.sprites.len(),
        palettes_in: sff.palettes.len(),
        ..Default::default()
    };

    // palettes: dedupe by content, never merging act targets with the rest
    let targets = act_targets(sff);
    let mut pal_map: Vec<u32> = Vec::with_capacity(sff.palettes.len());
    let mut pal_out: Vec<&Palette> = Vec::new();
    let mut pal_seen: HashMap<(bool, &Palette), u32> = HashMap::new();
    for (i, p) in sff.palettes.iter().enumerate() {
        let key = (targets.contains(&i), p);
        let idx = *pal_seen.entry(key).or_insert_with(|| {
            pal_out.push(p);
            (pal_out.len() - 1) as u32
        });
        pal_map.push(idx);
    }
    stats.palettes_out = pal_out.len();

    // sprites: trim, then dedupe the pixel blocks
    let mut blobs: Vec<Blob> = Vec::new();
    let mut blob_seen: HashMap<u64, Vec<u32>> = HashMap::new();
    let mut rows: Vec<[u8; 20]> = Vec::with_capacity(sff.sprites.len());
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
        let index0_clear = rgba
            || s.palette
                .and_then(|p| sff.palettes.get(p))
                .and_then(|p| p.first())
                .is_some_and(|c| c[3] == 0);
        let mut rect = (0, 0, w, h);
        if opts.trim && *g != 9000 && w > 0 && h > 0 && px.len() == w * h * bpp && index0_clear {
            rect = visible_rect(px, w, h, rgba).unwrap_or((0, 0, 1, 1));
        }
        let ox = s.offset[0] as i32 - rect.0 as i32;
        let oy = s.offset[1] as i32 - rect.1 as i32;
        if i16::try_from(ox).is_err() || i16::try_from(oy).is_err() {
            rect = (0, 0, w, h);
        }
        let (ox, oy) = (
            (s.offset[0] as i32 - rect.0 as i32) as i16,
            (s.offset[1] as i32 - rect.1 as i32) as i16,
        );
        let data = if rect == (0, 0, w, h) {
            px.to_vec()
        } else {
            crop(px, w, bpp, rect)
        };
        let (tw, th) = (rect.2 as u16, rect.3 as u16);
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            (rgba, tw, &data).hash(&mut hs);
            hs.finish()
        };
        let cands = blob_seen.entry(hash).or_default();
        let blob = match cands.iter().find(|&&b| {
            let o = &blobs[b as usize];
            o.rgba == rgba && o.w == tw && o.px == data
        }) {
            Some(&b) => b,
            None => {
                stats.unique_bytes += data.len();
                blobs.push(Blob {
                    rgba,
                    w: tw,
                    px: data,
                });
                let b = (blobs.len() - 1) as u32;
                cands.push(b);
                b
            }
        };
        let pal = s
            .palette
            .and_then(|p| pal_map.get(p).copied())
            .unwrap_or(NONE);
        let mut row = [0u8; 20];
        row[0..2].copy_from_slice(&g.to_le_bytes());
        row[2..4].copy_from_slice(&n.to_le_bytes());
        row[4..6].copy_from_slice(&tw.to_le_bytes());
        row[6..8].copy_from_slice(&th.to_le_bytes());
        row[8..10].copy_from_slice(&ox.to_le_bytes());
        row[10..12].copy_from_slice(&oy.to_le_bytes());
        row[12..16].copy_from_slice(&pal.to_le_bytes());
        row[16..20].copy_from_slice(&blob.to_le_bytes());
        rows.push(row);
    }
    stats.blobs = blobs.len();

    let packed = compress_all(&blobs);
    stats.packed_bytes = packed.iter().map(Vec::len).sum();

    let mut out = vec![0u8; HEADER];
    out[..8].copy_from_slice(MAGIC);
    out[8..12].copy_from_slice(&FORMAT.to_le_bytes());
    let flags = u32::from(opts.is_char) | (u32::from(opts.trim) << 1);
    out[12..16].copy_from_slice(&flags.to_le_bytes());
    out[16..24].copy_from_slice(&stamp.len.to_le_bytes());
    out[24..32].copy_from_slice(&stamp.mtime_ns.to_le_bytes());
    let (vk, vp) = match sff.version {
        None => (0u8, 0u8),
        Some(Version::V1) => (1, 0),
        Some(Version::V2 { patch }) => (2, patch),
    };
    out[32] = vk;
    out[33] = vp;
    out[36..40].copy_from_slice(&(pal_out.len() as u32).to_le_bytes());
    out[40..44].copy_from_slice(&(sff.palette_keys.len() as u32).to_le_bytes());
    out[44..48].copy_from_slice(&(blobs.len() as u32).to_le_bytes());
    out[48..52].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    for p in &pal_out {
        let n = p.len().min(u16::MAX as usize);
        out.extend_from_slice(&(n as u16).to_le_bytes());
        for c in p.iter().take(n) {
            out.extend_from_slice(c);
        }
    }
    for (&(g, n), &i) in &sff.palette_keys {
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        let i = pal_map.get(i).copied().unwrap_or(NONE);
        out.extend_from_slice(&i.to_le_bytes());
    }
    let mut ofs = 0u64;
    for (b, p) in blobs.iter().zip(&packed) {
        out.extend_from_slice(&ofs.to_le_bytes());
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(&(b.px.len() as u32).to_le_bytes());
        out.extend_from_slice(&[u8::from(b.rgba), 0, 0, 0]);
        ofs += p.len() as u64;
    }
    for r in &rows {
        out.extend_from_slice(r);
    }
    let data_ofs = out.len() as u64;
    out[52..60].copy_from_slice(&data_ofs.to_le_bytes());
    for p in &packed {
        out.extend_from_slice(p);
    }
    (out, stats)
}

/// LZ4 every blob, spread over the available cores.
fn compress_all(blobs: &[Blob]) -> Vec<Vec<u8>> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    if threads == 1 || blobs.len() < 64 {
        return blobs
            .iter()
            .map(|b| lz4_flex::block::compress(&b.px))
            .collect();
    }
    let chunk = blobs.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = blobs
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|b| lz4_flex::block::compress(&b.px))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// Load a pack. Pixels stay compressed in `bytes` (shared, not copied).
pub fn read(bytes: Arc<Vec<u8>>) -> Result<(Sff, Header)> {
    let header = read_header(&bytes)?;
    if header.format != FORMAT {
        return Err(Error::fmt(
            F,
            format!("format {} (want {FORMAT})", header.format),
        ));
    }
    let b: &[u8] = &bytes;
    let bad = || Error::fmt(F, "truncated");
    let version = match (b[32], b[33]) {
        (1, _) => Some(Version::V1),
        (2, patch) => Some(Version::V2 { patch }),
        _ => None,
    };
    let n_pal = le32(b, 36) as usize;
    let n_keys = le32(b, 40) as usize;
    let n_blobs = le32(b, 44) as usize;
    let n_spr = le32(b, 48) as usize;
    let data_ofs = le64(b, 52) as usize;
    if data_ofs > b.len() {
        return Err(bad());
    }
    let mut at = HEADER;
    let mut palettes: Vec<Palette> = Vec::with_capacity(n_pal.min(1 << 16));
    for _ in 0..n_pal {
        let n = le16(b.get(at..at + 2).ok_or_else(bad)?, 0) as usize;
        at += 2;
        let raw = b.get(at..at + n * 4).ok_or_else(bad)?;
        palettes.push(
            raw.chunks_exact(4)
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect(),
        );
        at += n * 4;
    }
    let mut palette_keys = std::collections::BTreeMap::new();
    for _ in 0..n_keys {
        let r = b.get(at..at + 8).ok_or_else(bad)?;
        let i = le32(r, 4);
        if (i as usize) < palettes.len() {
            palette_keys.insert((le16(r, 0), le16(r, 2)), i as usize);
        }
        at += 8;
    }
    let mut blobs: Vec<Packed> = Vec::with_capacity(n_blobs.min(1 << 20));
    for _ in 0..n_blobs {
        let r = b.get(at..at + 20).ok_or_else(bad)?;
        let start = data_ofs.checked_add(le64(r, 0) as usize).ok_or_else(bad)?;
        let len = le32(r, 8) as usize;
        if start.checked_add(len).is_none_or(|e| e > b.len()) {
            return Err(bad());
        }
        blobs.push(Packed {
            buf: bytes.clone(),
            start,
            len,
            raw_len: le32(r, 12) as usize,
            rgba: r[16] != 0,
        });
        at += 20;
    }
    let mut sprites: IndexMap<Key, Sprite> = IndexMap::with_capacity(n_spr.min(1 << 20));
    for _ in 0..n_spr {
        let r = b.get(at..at + 20).ok_or_else(bad)?;
        let (group, number) = (le16(r, 0), le16(r, 2));
        let (width, height) = (le16(r, 4), le16(r, 6));
        let pal = le32(r, 12);
        let blob = blobs.get(le32(r, 16) as usize).ok_or_else(bad)?.clone();
        let bpp = if blob.rgba { 4 } else { 1 };
        if blob.raw_len != width as usize * height as usize * bpp {
            return Err(Error::fmt(
                F,
                format!("sprite {group},{number}: size mismatch"),
            ));
        }
        sprites.insert(
            (group, number),
            Sprite {
                group,
                number,
                width,
                height,
                offset: [le16(r, 8) as i16, le16(r, 10) as i16],
                palette: (pal != NONE && (pal as usize) < palettes.len()).then_some(pal as usize),
                pixels: Pixels::Packed(blob),
            },
        );
        at += 20;
    }
    if at > data_ofs {
        return Err(bad());
    }
    Ok((
        Sff {
            version,
            sprites,
            palettes,
            palette_keys,
        },
        header,
    ))
}

/// Read a pack file.
pub fn open(path: &Path) -> Result<(Sff, Header)> {
    read(std::fs::read(path)?.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every sprite of `a` draws the same texels as in `b` (same palette
    /// colours, same position relative to the axis).
    pub(crate) fn assert_same_image(a: &Sff, b: &Sff) {
        assert_eq!(a.sprites.len(), b.sprites.len());
        for (k, sa) in &a.sprites {
            let sb = &b.sprites[k];
            let (pa, pb) = (sa.decoded(), sb.decoded());
            let texel = |s: &Sprite, p: &Pixels, sff: &Sff, x: i32, y: i32| -> [u8; 4] {
                // coordinates relative to the axis
                let (tx, ty) = (x + s.offset[0] as i32, y + s.offset[1] as i32);
                if tx < 0 || ty < 0 || tx >= s.width as i32 || ty >= s.height as i32 {
                    return [0; 4];
                }
                let i = ty as usize * s.width as usize + tx as usize;
                match p {
                    Pixels::Indexed(px) => {
                        let c = px[i];
                        let pal = &sff.palettes[s.palette.unwrap()];
                        let v = pal.get(c as usize).copied().unwrap_or([0; 4]);
                        if v[3] == 0 { [0; 4] } else { v }
                    }
                    Pixels::Rgba(px) => {
                        let v = [px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3]];
                        if v[3] == 0 { [0; 4] } else { v }
                    }
                    Pixels::Packed(_) => unreachable!(),
                }
            };
            for y in -(sa.offset[1] as i32)..sa.height as i32 - sa.offset[1] as i32 {
                for x in -(sa.offset[0] as i32)..sa.width as i32 - sa.offset[0] as i32 {
                    assert_eq!(
                        texel(sa, &pa, a, x, y),
                        texel(sb, &pb, b, x, y),
                        "sprite {k:?} at {x},{y}"
                    );
                }
            }
            // and nothing visible outside the original rectangle
            assert!(sb.width <= sa.width.max(1) && sb.height <= sa.height.max(1));
        }
    }

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
        s
    }

    #[test]
    fn roundtrip_trims_and_dedupes() {
        let sff = sample();
        let (bytes, st) = write(
            &sff,
            Options {
                is_char: true,
                trim: true,
            },
            Stamp::default(),
        );
        assert_eq!(st.palettes_out, 1, "duplicate palettes merged");
        let (back, h) = read(bytes.into()).unwrap();
        assert!(h.is_char && h.trimmed);
        assert_same_image(&sff, &back);
        let a = &back.sprites[&(0, 0)];
        assert_eq!((a.width, a.height, a.offset), (2, 2, [1, 2]));
        assert_eq!(a.share_key(), back.sprites[&(0, 1)].share_key(), "one blob");
        let p = &back.sprites[&(9000, 0)];
        assert_eq!((p.width, p.height), (4, 3), "portraits keep their size");
        assert_eq!(back.palette_keys.len(), 2);
        let r = &back.sprites[&(2, 0)];
        assert_eq!((r.width, r.height, r.offset), (1, 1, [-1, -1]));
    }

    #[test]
    fn untrimmed_pack_is_bit_exact() {
        let sff = sample();
        let (bytes, _) = write(
            &sff,
            Options {
                is_char: false,
                trim: false,
            },
            Stamp::default(),
        );
        let (back, _) = read(bytes.into()).unwrap();
        for (k, s) in &sff.sprites {
            let b = &back.sprites[k];
            assert_eq!((s.width, s.height, s.offset), (b.width, b.height, b.offset));
            assert_eq!(s.pixels, *b.decoded());
            let pal = |f: &Sff, p: Option<usize>| p.map(|i| f.palettes[i].clone());
            assert_eq!(pal(&sff, s.palette), pal(&back, b.palette));
        }
    }

    #[test]
    fn act_targets_stay_apart() {
        let mut sff = sample();
        sff.version = Some(Version::V1);
        let (bytes, st) = write(
            &sff,
            Options {
                is_char: true,
                trim: true,
            },
            Stamp::default(),
        );
        assert_eq!(st.palettes_out, 2, "0,0's palette is not merged with 0,1's");
        let (mut back, _) = read(bytes.into()).unwrap();
        let mut orig = sff.clone();
        let act = vec![[7, 7, 7, 255]; 256];
        orig.apply_v1_act(&act);
        back.apply_v1_act(&act);
        assert_same_image(&orig, &back);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(read(Arc::new(b"RUGENSPK".to_vec())).is_err());
        let (mut bytes, _) = write(
            &sample(),
            Options {
                is_char: true,
                trim: true,
            },
            Stamp::default(),
        );
        bytes.truncate(bytes.len() - 3);
        assert!(read(bytes.into()).is_err());
    }

    #[test]
    fn reference_sffs_roundtrip() {
        let Some(root) = crate::fixtures::reference("mugen-1.1b1") else {
            eprintln!("reference tree missing: skipped");
            return;
        };
        for rel in [
            "chars/kfm/kfm.sff",
            "chars/kfm720/kfm720.sff",
            "stages/stage0.sff",
        ] {
            let Ok(sff) = Sff::open(root.join(rel), rel.starts_with("chars")) else {
                continue;
            };
            let opts = Options {
                is_char: true,
                trim: rel.starts_with("chars"),
            };
            let (bytes, _) = write(&sff, opts, Stamp::default());
            let (back, _) = read(bytes.into()).unwrap();
            assert_same_image(&sff, &back);
        }
    }
}
