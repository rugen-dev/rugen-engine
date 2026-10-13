//! SFF sprite archives, versions 1.01, 2.0.0 and 2.0.1.
//!
//! Semantics follow Ikemen GO's `image.go` loader, including its tolerance of
//! truncated run-length data and its SFFv1 palette discovery. Layouts:
//!
//! ```text
//! header         "ElecbyteSpr\0", version[4] (lo3 lo2 lo1 hi), ...
//! v1 subheader   32 bytes: next u32, len u32, x i16, y i16, group u16, no u16, link u16, samepal u8, pad[13]; PCX follows
//! v2 sprite node 28 bytes: group u16, no u16, w u16, h u16, x i16, y i16, link u16, fmt u8, depth u8, ofs u32, len u32, pal u16, flags u16
//! v2 pal node    16 bytes: group u16, no u16, ncols u16, link u16, ofs u32, len u32
//! ```

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use indexmap::IndexMap;

use crate::{Error, Result};

const F: &str = "sff";

/// `(group, number)` key used by sprites and palettes.
pub type Key = (u16, u16);

/// 256-entry RGBA palette. Index 0 is transparent for 8-bit sprites.
pub type Palette = Vec<[u8; 4]>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V1,
    /// `patch` is the third version byte: 0 for 2.0.0.0, 1 for 2.0.1.0.
    V2 {
        patch: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pixels {
    /// 8-bit palette indices, `w * h` bytes, row-major.
    Indexed(Vec<u8>),
    /// Straight (non-premultiplied) RGBA8, `w * h * 4` bytes.
    Rgba(Vec<u8>),
    /// Still compressed, from an `.rgc` / `.rgs` sprite section ([`crate::pack`]);
    /// [`Sprite::decoded`] turns it into one of the variants above.
    Packed(crate::pack::Packed),
}

impl Pixels {
    /// Decompressed form (borrowed when not packed).
    pub fn decoded(&self) -> std::borrow::Cow<'_, Pixels> {
        match self {
            Pixels::Packed(p) => std::borrow::Cow::Owned(p.decode()),
            other => std::borrow::Cow::Borrowed(other),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    pub group: u16,
    pub number: u16,
    pub width: u16,
    pub height: u16,
    /// Axis offset: sprite origin relative to its top-left corner.
    pub offset: [i16; 2],
    /// Index into [`Sff::palettes`]. `None` for RGBA sprites.
    pub palette: Option<usize>,
    pub pixels: Pixels,
}

#[derive(Clone, Debug, Default)]
pub struct Sff {
    pub version: Option<Version>,
    /// In file order. First occurrence of a duplicate key wins the map.
    pub sprites: IndexMap<Key, Sprite>,
    pub palettes: Vec<Palette>,
    /// SFFv2 palette keys. SFFv1 palettes are anonymous.
    pub palette_keys: BTreeMap<Key, usize>,
}

impl Sprite {
    /// Pixels ready for upload: `Indexed` or `Rgba`, `width * height` texels.
    pub fn decoded(&self) -> std::borrow::Cow<'_, Pixels> {
        self.pixels.decoded()
    }

    /// Identity of the pixel data for texture sharing: sprites with the same
    /// key hold the same pixels (linked / duplicate sprites of a content file).
    /// `None` when the data is not shared.
    pub fn share_key(&self) -> Option<(usize, usize, u64)> {
        match &self.pixels {
            Pixels::Packed(p) => Some(p.share_key()),
            _ => None,
        }
    }
}

impl Sff {
    pub fn get(&self, group: u16, number: u16) -> Option<&Sprite> {
        self.sprites.get(&(group, number))
    }

    /// Parse from bytes. `is_char` enables the legacy palette rule for a
    /// character's `0,0` sprite (MUGEN loads stages and screenpacks without it).
    pub fn parse(bytes: &[u8], is_char: bool) -> Result<Self> {
        let mut r = Cursor::new(bytes);
        let mut sig = [0u8; 12];
        r.read_exact(&mut sig)?;
        if &sig != b"ElecbyteSpr\0" {
            return Err(Error::fmt(F, "bad signature"));
        }
        let v = [u8_(&mut r)?, u8_(&mut r)?, u8_(&mut r)?, u8_(&mut r)?];
        let _ = u32_(&mut r)?;
        match v[3] {
            1 => parse_v1(&mut r, is_char),
            2 => parse_v2(&mut r, v[1]),
            n => Err(Error::fmt(F, format!("unsupported version {n}"))),
        }
    }

    pub fn open(path: impl AsRef<std::path::Path>, is_char: bool) -> Result<Self> {
        crate::vfs::open_sff(path.as_ref(), is_char)
    }

    /// Give an SFFv1 character the colours of its selected `.act`.
    ///
    /// MUGEN (and Ikemen's `remapPal` of 1,1) replaces the palette used by sprite
    /// `0,0` and the `9000,0` portrait; sprites flagged as sharing that palette
    /// follow, while sprites carrying their own palette keep it. The palette read
    /// from `0,0` itself is never shown, and in some files it is junk.
    pub fn apply_v1_act(&mut self, act: &Palette) {
        if self.version != Some(Version::V1) {
            return;
        }
        let mut targets: Vec<usize> = Vec::new();
        for key in [(0, 0), (9000, 0)] {
            if let Some(p) = self.sprites.get(&key).and_then(|s| s.palette)
                && !targets.contains(&p)
            {
                targets.push(p);
            }
        }
        for p in targets {
            if let Some(dst) = self.palettes.get_mut(p) {
                *dst = act.clone();
                dst.resize(256, [0; 4]);
                dst[0][3] = 0;
            }
        }
    }
}

// ---------------------------------------------------------------- readers

type R<'a> = Cursor<&'a [u8]>;

fn u8_(r: &mut R) -> Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
fn u16_(r: &mut R) -> Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn i16_(r: &mut R) -> Result<i16> {
    Ok(u16_(r)? as i16)
}
fn u32_(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn slice<'a>(r: &R<'a>, start: u64, len: u64) -> &'a [u8] {
    let data = *r.get_ref();
    let s = (start as usize).min(data.len());
    let e = (start.saturating_add(len) as usize).min(data.len());
    &data[s..e]
}

// ---------------------------------------------------------------- v1

fn parse_v1(r: &mut R, is_char: bool) -> Result<Sff> {
    let count = u32_(r)?;
    let mut shofs = u32_(r)? as u64;
    let mut sff = Sff {
        version: Some(Version::V1),
        ..Default::default()
    };
    let mut list: Vec<Sprite> = Vec::with_capacity(count.min(1 << 16) as usize);
    let mut prev_pal: Option<usize> = None;
    let mut have_prev = false;

    for i in 0..count as usize {
        r.seek(SeekFrom::Start(shofs))?;
        let next = u32_(r)? as u64;
        let len = u32_(r)? as u64;
        let offset = [i16_(r)?, i16_(r)?];
        let group = u16_(r)?;
        let number = u16_(r)?;
        let link = u16_(r)? as usize;
        let same_pal = u8_(r)? != 0;

        let mut spr = Sprite {
            group,
            number,
            width: 0,
            height: 0,
            offset,
            palette: None,
            pixels: Pixels::Indexed(Vec::new()),
        };

        if len == 0 {
            if link < i {
                let src = &list[link];
                spr.width = src.width;
                spr.height = src.height;
                spr.pixels = src.pixels.clone();
                spr.palette = src.palette;
            }
        } else {
            let pcx = shofs + 32;
            let first = is_char && (!have_prev || (group == 0 && number == 0));
            read_v1_sprite(
                r,
                &mut sff,
                &mut spr,
                pcx,
                len,
                next,
                same_pal && have_prev,
                prev_pal,
                first,
            )?;
            have_prev = true;
            prev_pal = spr.palette;
        }
        if !sff.sprites.contains_key(&(group, number)) {
            sff.sprites.insert((group, number), spr.clone());
        } else {
            log::warn!("sff: duplicate sprite {group},{number} (index {i} ignored)");
        }
        list.push(spr);
        shofs = next;
        if shofs == 0 {
            break;
        }
    }
    Ok(sff)
}

#[allow(clippy::too_many_arguments)]
fn read_v1_sprite(
    r: &mut R,
    sff: &mut Sff,
    spr: &mut Sprite,
    offset: u64,
    mut len: u64,
    next: u64,
    same_pal: bool,
    prev_pal: Option<usize>,
    char_first: bool,
) -> Result<()> {
    // PCX header
    r.seek(SeekFrom::Start(offset + 2))?;
    let encoding = u8_(r)?;
    let bpp = u8_(r)?;
    if bpp != 8 {
        return Err(Error::fmt(F, format!("pcx depth {bpp}, expected 8")));
    }
    let rect = [u16_(r)?, u16_(r)?, u16_(r)?, u16_(r)?];
    r.seek(SeekFrom::Start(offset + 66))?;
    let bpl = u16_(r)? as usize;
    spr.width = rect[2].wrapping_sub(rect[0]).wrapping_add(1);
    spr.height = rect[3].wrapping_sub(rect[1]).wrapping_add(1);
    let data_start = offset + 128;

    let (rle, pal_ofs): (&[u8], Option<u64>) = if char_first {
        if next > offset {
            len = next - offset;
        }
        let rle = slice(r, data_start, len.saturating_sub(128));
        (rle, Some((offset + len).saturating_sub(768)))
    } else {
        let block_end = if next > offset { next } else { offset + len };
        let pal_ofs = if same_pal {
            block_end
        } else {
            // scan backwards for the 0x0C palette marker
            let data = *r.get_ref();
            let mut found = None;
            let mut pos = block_end as i64 - 769;
            while pos >= data_start as i64 {
                if data.get(pos as usize) == Some(&0x0C) {
                    found = Some(pos as u64);
                    break;
                }
                pos -= 1;
            }
            found.unwrap_or((block_end as i64 - 769).max(0) as u64)
        };
        let rle = slice(r, data_start, pal_ofs.saturating_sub(data_start));
        (rle, if same_pal { None } else { Some(pal_ofs + 1) })
    };

    spr.palette = match (same_pal, pal_ofs) {
        (true, _) => Some(prev_pal.unwrap_or_else(|| push_pal(sff, vec![[0; 4]; 256]))),
        (false, Some(p)) => {
            let raw = slice(r, p, 768);
            let mut pal = vec![[0u8; 4]; 256];
            for (i, c) in pal.iter_mut().enumerate() {
                let rgb = raw.get(i * 3..i * 3 + 3).unwrap_or(&[0, 0, 0]);
                *c = [rgb[0], rgb[1], rgb[2], if i == 0 { 0 } else { 255 }];
            }
            Some(push_pal(sff, pal))
        }
        (false, None) => None,
    };

    let px = if encoding == 1 {
        pcx_decode(rle, spr.width, spr.height, bpl)
    } else {
        rle.to_vec()
    };
    spr.pixels = Pixels::Indexed(fit(px, spr.width, spr.height));
    Ok(())
}

fn push_pal(sff: &mut Sff, pal: Palette) -> usize {
    sff.palettes.push(pal);
    sff.palettes.len() - 1
}

/// PCX RLE with scanline padding (`bpl` bytes per encoded row).
pub(crate) fn pcx_decode(rle: &[u8], w: u16, h: u16, bpl: usize) -> Vec<u8> {
    let total = w as usize * h as usize;
    if rle.is_empty() || bpl == 0 {
        return rle.to_vec();
    }
    let mut p = vec![0u8; total];
    let (mut i, mut j, mut k, w) = (0usize, 0usize, 0usize, w as usize);
    while j < total {
        let mut n = 1usize;
        let mut d = rle[i];
        if i < rle.len() - 1 {
            i += 1;
        }
        if d >= 0xC0 {
            n = (d & 0x3F) as usize;
            d = rle[i];
            if i < rle.len() - 1 {
                i += 1;
            }
        }
        while n > 0 {
            if k < w && j < total {
                p[j] = d;
                j += 1;
            }
            k += 1;
            if k == bpl {
                k = 0;
                n = 1;
            }
            n -= 1;
        }
    }
    p
}

// ---------------------------------------------------------------- v2

fn parse_v2(r: &mut R, patch: u8) -> Result<Sff> {
    for _ in 0..4 {
        u32_(r)?;
    }
    let spr_ofs = u32_(r)? as u64;
    let spr_count = u32_(r)?;
    let pal_ofs = u32_(r)? as u64;
    let pal_count = u32_(r)?;
    let lofs = u32_(r)? as u64;
    let _ldata_len = u32_(r)?;
    let tofs = u32_(r)? as u64;

    let mut sff = Sff {
        version: Some(Version::V2 { patch }),
        ..Default::default()
    };

    // palettes
    let mut unique: BTreeMap<Key, usize> = BTreeMap::new();
    for i in 0..pal_count as usize {
        r.seek(SeekFrom::Start(pal_ofs + i as u64 * 16))?;
        let key = (u16_(r)?, u16_(r)?);
        let _ncols = u16_(r)?;
        let link = u16_(r)? as usize;
        let ofs = u32_(r)? as u64;
        let len = u32_(r)?;
        let pal = if let Some(&old) = unique.get(&key) {
            log::warn!("sff: duplicate palette {},{} (index {i})", key.0, key.1);
            sff.palettes[old].clone()
        } else if len == 0 {
            sff.palettes
                .get(link)
                .cloned()
                .unwrap_or_else(|| vec![[0; 4]; 256])
        } else {
            read_v2_palette(r, lofs + ofs, len, patch)
        };
        unique.entry(key).or_insert(i);
        sff.palette_keys.entry(key).or_insert(i);
        sff.palettes.push(pal);
    }

    // sprites
    let mut list: Vec<Sprite> = Vec::with_capacity(spr_count.min(1 << 16) as usize);
    for i in 0..spr_count as usize {
        r.seek(SeekFrom::Start(spr_ofs + i as u64 * 28))?;
        let group = u16_(r)?;
        let number = u16_(r)?;
        let width = u16_(r)?;
        let height = u16_(r)?;
        let offset = [i16_(r)?, i16_(r)?];
        let link = u16_(r)? as usize;
        let format = u8_(r)?;
        let depth = u8_(r)?;
        let ofs = u32_(r)? as u64;
        let len = u32_(r)?;
        let palidx = u16_(r)? as usize;
        let flags = u16_(r)?;
        let data = if flags & 1 == 0 {
            lofs + ofs
        } else {
            tofs + ofs
        };

        let mut spr = Sprite {
            group,
            number,
            width,
            height,
            offset,
            palette: Some(palidx),
            pixels: Pixels::Indexed(Vec::new()),
        };
        if len == 0 {
            if link < i {
                let src = &list[link];
                spr.width = src.width;
                spr.height = src.height;
                spr.pixels = src.pixels.clone();
                if spr.palette.is_none() {
                    spr.palette = src.palette;
                }
            } else {
                spr.palette = Some(0);
            }
        } else {
            match read_v2_pixels(r, data, len, format, depth, width, height) {
                Ok(px) => spr.pixels = px,
                Err(e) => {
                    log::warn!("sff: sprite {group},{number}: {e}");
                    spr.pixels = Pixels::Indexed(vec![0; width as usize * height as usize]);
                }
            }
            if matches!(spr.pixels, Pixels::Rgba(_)) {
                spr.palette = None;
            }
        }
        if !sff.sprites.contains_key(&(group, number)) {
            sff.sprites.insert((group, number), spr.clone());
        } else {
            log::warn!("sff: duplicate sprite {group},{number} (index {i} ignored)");
        }
        list.push(spr);
    }
    Ok(sff)
}

fn read_v2_palette(r: &R, ofs: u64, len: u32, patch: u8) -> Palette {
    let raw = slice(r, ofs, len as u64);
    let n = (len / 4) as usize;
    let mut depth = 1;
    while depth < n {
        depth *= 2;
    }
    let depth = depth.clamp(16, 256);
    let mut pal = vec![[0u8; 4]; depth];
    for (i, c) in pal.iter_mut().enumerate() {
        if i < n {
            if let Some(b) = raw.get(i * 4..i * 4 + 4) {
                *c = [b[0], b[1], b[2], b[3]];
            }
        }
        if patch == 0 {
            c[3] = if i == 0 { 0 } else { 255 };
        }
    }
    pal
}

fn read_v2_pixels(
    r: &R,
    ofs: u64,
    len: u32,
    format: u8,
    depth: u8,
    w: u16,
    h: u16,
) -> Result<Pixels> {
    let data = slice(r, ofs, len as u64);
    match format {
        0 => match depth {
            8 => Ok(Pixels::Indexed(fit(data.to_vec(), w, h))),
            24 => Ok(Pixels::Rgba(rgb_to_rgba(data, w, h))),
            32 => Ok(Pixels::Rgba(fit4(data.to_vec(), w, h))),
            d => Err(Error::fmt(F, format!("raw depth {d}"))),
        },
        2 => Ok(Pixels::Indexed(rle8_decode(
            data.get(4..).unwrap_or(&[]),
            w,
            h,
        ))),
        3 => Ok(Pixels::Indexed(rle5_decode(
            data.get(4..).unwrap_or(&[]),
            w,
            h,
        ))),
        4 => Ok(Pixels::Indexed(lz5_decode(
            data.get(4..).unwrap_or(&[]),
            w,
            h,
        ))),
        10..=12 => png_decode(data.get(4..).unwrap_or(&[]), format, w, h),
        f => Err(Error::fmt(F, format!("unknown format {f}"))),
    }
}

fn fit(mut v: Vec<u8>, w: u16, h: u16) -> Vec<u8> {
    v.resize(w as usize * h as usize, 0);
    v
}
fn fit4(mut v: Vec<u8>, w: u16, h: u16) -> Vec<u8> {
    v.resize(w as usize * h as usize * 4, 0);
    v
}
fn rgb_to_rgba(src: &[u8], w: u16, h: u16) -> Vec<u8> {
    let n = w as usize * h as usize;
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        if let Some(p) = src.get(i * 3..i * 3 + 3) {
            out[i * 4..i * 4 + 3].copy_from_slice(p);
            out[i * 4 + 3] = 255;
        }
    }
    out
}

fn rle8_decode(rle: &[u8], w: u16, h: u16) -> Vec<u8> {
    let total = w as usize * h as usize;
    if rle.is_empty() {
        return vec![0; total];
    }
    let mut p = vec![0u8; total];
    let (mut i, mut j) = (0usize, 0usize);
    while j < total {
        let mut n = 1usize;
        let mut d = rle[i];
        if i < rle.len() - 1 {
            i += 1;
        }
        if d & 0xC0 == 0x40 {
            n = (d & 0x3F) as usize;
            d = rle[i];
            if i < rle.len() - 1 {
                i += 1;
            }
        }
        for _ in 0..n {
            if j < total {
                p[j] = d;
                j += 1;
            }
        }
        if n == 0 && i >= rle.len() - 1 {
            break; // exhausted input with zero-length run: avoid spinning
        }
    }
    p
}

/// SFFv2 RLE8 (format 2): `0x40 | n, value` runs, other bytes literal.
fn rle8_encode(px: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < px.len() {
        let v = px[i];
        let mut n = 1;
        while n < 63 && i + n < px.len() && px[i + n] == v {
            n += 1;
        }
        if n > 1 || v & 0xC0 == 0x40 {
            out.push(0x40 | n as u8);
            out.push(v);
        } else {
            out.push(v);
        }
        i += n;
    }
}

/// An 8-bit PCX image (128-byte header + RLE rows), as SFFv1 stores sprites.
fn pcx_encode(px: &[u8], w: u16, h: u16, out: &mut Vec<u8>) {
    let bpl = (w as usize + 1) & !1;
    let mut hdr = [0u8; 128];
    hdr[0] = 10;
    hdr[1] = 5;
    hdr[2] = 1;
    hdr[3] = 8;
    hdr[8..10].copy_from_slice(&w.wrapping_sub(1).to_le_bytes());
    hdr[10..12].copy_from_slice(&h.wrapping_sub(1).to_le_bytes());
    hdr[12..14].copy_from_slice(&72u16.to_le_bytes());
    hdr[14..16].copy_from_slice(&72u16.to_le_bytes());
    hdr[65] = 1;
    hdr[66..68].copy_from_slice(&(bpl as u16).to_le_bytes());
    hdr[68..70].copy_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&hdr);
    let (w, h) = (w as usize, h as usize);
    let mut row = vec![0u8; bpl];
    for y in 0..h {
        row.fill(0);
        let src = px.get(y * w..(y + 1) * w).unwrap_or(&[]);
        row[..src.len()].copy_from_slice(src);
        let mut i = 0;
        while i < bpl {
            let v = row[i];
            let mut n = 1;
            while n < 63 && i + n < bpl && row[i + n] == v {
                n += 1;
            }
            if n > 1 || v >= 0xC0 {
                out.push(0xC0 | n as u8);
            }
            out.push(v);
            i += n;
        }
    }
}

fn rle5_decode(rle: &[u8], w: u16, h: u16) -> Vec<u8> {
    let total = w as usize * h as usize;
    if rle.is_empty() {
        return vec![0; total];
    }
    let mut p = vec![0u8; total];
    let (mut i, mut j) = (0usize, 0usize);
    let last = rle.len() - 1;
    while j < total {
        let mut rl = rle[i] as i32;
        if i < last {
            i += 1;
        }
        let mut dl = (rle[i] & 0x7F) as i32;
        let mut c = 0u8;
        if rle[i] >> 7 != 0 {
            if i < last {
                i += 1;
            }
            c = rle[i];
        }
        if i < last {
            i += 1;
        }
        loop {
            if j < total {
                p[j] = c;
                j += 1;
            }
            rl -= 1;
            if rl < 0 {
                dl -= 1;
                if dl < 0 {
                    break;
                }
                c = rle[i] & 0x1F;
                rl = (rle[i] >> 5) as i32;
                if i < last {
                    i += 1;
                }
            }
        }
        if i >= last && j < total && rl < 0 {
            // input exhausted; remaining pixels stay 0
            if rle[last] == 0 {
                break;
            }
        }
    }
    p
}

fn lz5_decode(rle: &[u8], w: u16, h: u16) -> Vec<u8> {
    let total = w as usize * h as usize;
    if rle.is_empty() {
        return vec![0; total];
    }
    let mut p = vec![0u8; total];
    let last = rle.len() - 1;
    let (mut i, mut j) = (0usize, 0usize);
    let mut ct = rle[i];
    let mut cts = 0u32;
    let mut rb = 0u8;
    let mut rbc = 0u32;
    if i < last {
        i += 1;
    }
    let mut guard = 0usize;
    while j < total {
        guard += 1;
        if guard > total * 2 + 16 {
            break; // malformed stream that never advances
        }
        let mut d = rle[i] as usize;
        if i < last {
            i += 1;
        }
        if ct & (1u8.wrapping_shl(cts)) != 0 {
            let n: i64;
            if d & 0x3F == 0 {
                d = (d << 2 | rle[i] as usize) + 1;
                if i < last {
                    i += 1;
                }
                n = rle[i] as i64 + 2;
                if i < last {
                    i += 1;
                }
            } else {
                rb |= ((d & 0xC0) as u8).wrapping_shr(rbc);
                rbc += 2;
                n = (d & 0x3F) as i64;
                if rbc < 8 {
                    d = rle[i] as usize + 1;
                    if i < last {
                        i += 1;
                    }
                } else {
                    d = rb as usize + 1;
                    rb = 0;
                    rbc = 0;
                }
            }
            let mut n = n;
            loop {
                if j < total {
                    p[j] = if d <= j { p[j - d] } else { 0 };
                    j += 1;
                }
                n -= 1;
                if n < 0 {
                    break;
                }
            }
        } else {
            let n: usize;
            if d & 0xE0 == 0 {
                n = rle[i] as usize + 8;
                if i < last {
                    i += 1;
                }
            } else {
                n = d >> 5;
                d &= 0x1F;
            }
            for _ in 0..n {
                if j < total {
                    p[j] = d as u8;
                    j += 1;
                }
            }
        }
        cts += 1;
        if cts >= 8 {
            ct = rle[i];
            cts = 0;
            if i < last {
                i += 1;
            }
        }
    }
    p
}

/// RGBA8 pixels as a PNG stream (best compression; used by the v2 writer).
fn png_encode_rgba(px: &[u8], w: u16, h: u16) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut wr = enc.write_header().expect("png header into a Vec");
        let need = w as usize * h as usize * 4;
        let mut data = px.to_vec();
        data.resize(need, 0);
        wr.write_image_data(&data).expect("png data into a Vec");
    }
    out
}

fn png_decode(data: &[u8], format: u8, w: u16, h: u16) -> Result<Pixels> {
    let mut dec = png::Decoder::new(Cursor::new(data));
    if format != 10 {
        dec.set_transformations(
            png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
        );
    }
    let mut reader = dec.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    match (format, info.color_type, info.bit_depth) {
        (10, png::ColorType::Indexed, png::BitDepth::Eight) => Ok(Pixels::Indexed(fit(buf, w, h))),
        (10, png::ColorType::Indexed, _) => Err(Error::fmt(
            F,
            format!("indexed png bit depth {:?}", info.bit_depth),
        )),
        (10, ct, _) => Err(Error::fmt(
            F,
            format!("png format 10 with color type {ct:?}"),
        )),
        (_, png::ColorType::Rgba, png::BitDepth::Eight) => Ok(Pixels::Rgba(fit4(
            buf,
            info.width as u16,
            info.height as u16,
        ))),
        (_, ct, bd) => Err(Error::fmt(F, format!("png color {ct:?} depth {bd:?}"))),
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::reference;

    fn load(rel: &str, is_char: bool) -> Option<Sff> {
        let p = reference(rel)?;
        Some(Sff::open(p, is_char).unwrap_or_else(|e| panic!("{rel}: {e}")))
    }

    fn check_sane(sff: &Sff) {
        for (k, s) in &sff.sprites {
            let n = s.width as usize * s.height as usize;
            match &s.pixels {
                Pixels::Indexed(px) => {
                    assert_eq!(px.len(), n, "sprite {k:?} indexed size");
                    let pal = s.palette.expect("indexed sprite has palette");
                    assert!(
                        pal < sff.palettes.len(),
                        "sprite {k:?} palette {pal} out of range"
                    );
                }
                Pixels::Rgba(px) => assert_eq!(px.len(), n * 4, "sprite {k:?} rgba size"),
                Pixels::Packed(_) => panic!("parse never packs"),
            }
        }
    }

    #[test]
    fn v1_act_replaces_the_shared_palette_only() {
        // G6 Sasuke: junk palette behind 0,0 made every other idle frame dark
        let spr = |group, number, palette| Sprite {
            group,
            number,
            width: 1,
            height: 1,
            offset: [0, 0],
            palette: Some(palette),
            pixels: Pixels::Indexed(vec![1]),
        };
        let mut sff = Sff {
            version: Some(Version::V1),
            palettes: vec![vec![[9, 9, 9, 255]; 256], vec![[7, 7, 7, 255]; 256]],
            ..Default::default()
        };
        sff.sprites.insert((0, 0), spr(0, 0, 0));
        sff.sprites.insert((0, 1), spr(0, 1, 0));
        sff.sprites.insert((5, 0), spr(5, 0, 1));
        let act = vec![[1, 2, 3, 255]; 256];
        let mut v2 = sff.clone();
        v2.version = Some(Version::V2 { patch: 1 });
        sff.apply_v1_act(&act);
        assert_eq!(sff.palettes[0][1], [1, 2, 3, 255], "0,0 and its sharers");
        assert_eq!(sff.palettes[0][0][3], 0, "index 0 stays transparent");
        assert_eq!(sff.palettes[1][1], [7, 7, 7, 255], "own palettes kept");
        v2.apply_v1_act(&act);
        assert_eq!(v2.palettes[0][1], [9, 9, 9, 255], "SFFv2 untouched");
    }

    #[test]
    fn v1_system_sff() {
        let Some(sff) = load("mugen-1.1b1/data/system.sff", false) else {
            return;
        };
        assert_eq!(sff.version, Some(Version::V1));
        assert!(!sff.sprites.is_empty());
        check_sane(&sff);
    }

    #[test]
    fn v1_kfm_intro_is_char() {
        let Some(sff) = load("mugen-1.1b1/chars/kfm/intro.sff", true) else {
            return;
        };
        assert_eq!(sff.version, Some(Version::V1));
        check_sane(&sff);
    }

    #[test]
    fn v2_0_0_stage0() {
        let Some(sff) = load("mugen-1.1b1/stages/stage0.sff", false) else {
            return;
        };
        assert_eq!(sff.version, Some(Version::V2 { patch: 0 }));
        check_sane(&sff);
        let p = &sff.palettes[0];
        assert_eq!(p[0][3], 0);
        assert_eq!(p[1][3], 255);
    }

    #[test]
    fn v2_0_1_kfm() {
        let Some(sff) = load("mugen-1.1b1/chars/kfm/kfm.sff", true) else {
            return;
        };
        assert_eq!(sff.version, Some(Version::V2 { patch: 1 }));
        check_sane(&sff);
        let s = sff.get(0, 0).expect("kfm has sprite 0,0");
        assert!(s.width > 0 && s.height > 0);
        // KFM's stand sprite is non-blank: some pixel uses a non-zero palette index
        let Pixels::Indexed(px) = &s.pixels else {
            panic!("kfm 0,0 is indexed")
        };
        assert!(px.iter().any(|&b| b != 0));
        assert!(
            sff.palette_keys.contains_key(&(1, 1)),
            "kfm has palette 1,1"
        );
    }

    #[test]
    fn v2_kfm720() {
        let Some(sff) = load("ikemen-go-v1.0.0/chars/kfm720/kfm720.sff", true) else {
            return;
        };
        check_sane(&sff);
        assert!(sff.get(0, 0).is_some());
    }

    #[test]
    fn all_reference_sffs_load() {
        let Some(root) = reference("") else { return };
        let mut n = 0;
        for dir in ["mugen-1.1b1", "ikemen-go-v1.0.0"] {
            for entry in crate::fixtures::walk(&root.join(dir)) {
                if entry
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("sff"))
                {
                    let is_char = entry.to_string_lossy().contains("/chars/");
                    let sff = Sff::open(&entry, is_char)
                        .unwrap_or_else(|e| panic!("{}: {e}", entry.display()));
                    check_sane(&sff);
                    n += 1;
                }
            }
        }
        if n == 0 {
            eprintln!("reference tree missing; skipping");
            return;
        }
        assert!(n > 20, "found only {n} sff files");
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(Sff::parse(b"", false).is_err());
        assert!(Sff::parse(b"ElecbyteSpr\0", false).is_err());
        let mut v = b"ElecbyteSpr\0\x00\x01\x00\x02".to_vec();
        v.extend_from_slice(&[0xFF; 64]);
        let _ = Sff::parse(&v, true);
    }

    #[test]
    fn rle8_roundtrip_simple() {
        // run of 5 x 0x07 then literal 0x09
        let px = rle8_decode(&[0x45, 0x07, 0x09], 3, 2);
        assert_eq!(px, [7, 7, 7, 7, 7, 9]);
    }
}

// ---------------------------------------------------------------- writer

/// Size, palette slot and pixels of a sprite already written (links).
type PixelsKey<'a> = (u16, u16, Option<usize>, &'a [u8]);

impl Sff {
    /// Serialize as SFF v2.0.1: raw 8-bit indexed sprites (format 0) or
    /// PNG-compressed RGBA (format 12, depth 32), palettes with their alpha kept. Palettes are
    /// written in index order with the keys from [`Sff::palette_keys`] or `(0, i)`.
    pub fn write_v2(&self) -> Vec<u8> {
        let mut out = vec![0u8; 512];
        out[..12].copy_from_slice(b"ElecbyteSpr\0");
        out[12..16].copy_from_slice(&[0, 1, 0, 2]);
        let mut ldata: Vec<u8> = Vec::new();
        let mut pal_nodes: Vec<u8> = Vec::new();
        let mut key_of: Vec<Key> = (0..self.palettes.len()).map(|i| (0, i as u16)).collect();
        for (k, &i) in &self.palette_keys {
            if let Some(slot) = key_of.get_mut(i) {
                *slot = *k;
            }
        }
        for (i, pal) in self.palettes.iter().enumerate() {
            let ofs = ldata.len() as u32;
            for c in pal.iter().take(256) {
                ldata.extend_from_slice(c);
            }
            let len = (pal.len().min(256) * 4) as u32;
            let (g, n) = key_of[i];
            pal_nodes.extend_from_slice(&g.to_le_bytes());
            pal_nodes.extend_from_slice(&n.to_le_bytes());
            pal_nodes.extend_from_slice(&(pal.len().min(256) as u16).to_le_bytes());
            pal_nodes.extend_from_slice(&0u16.to_le_bytes());
            pal_nodes.extend_from_slice(&ofs.to_le_bytes());
            pal_nodes.extend_from_slice(&len.to_le_bytes());
        }
        let mut spr_nodes: Vec<u8> = Vec::new();
        for ((g, n), s) in &self.sprites {
            let ofs = ldata.len() as u32;
            let (fmt, depth, len) = match s.decoded().as_ref() {
                Pixels::Packed(_) => unreachable!("decoded"),
                Pixels::Indexed(px) => {
                    ldata.extend_from_slice(px);
                    (0u8, 8u8, px.len() as u32)
                }
                Pixels::Rgba(px) => {
                    let png = png_encode_rgba(px, s.width, s.height);
                    ldata.extend_from_slice(&(px.len() as u32).to_le_bytes());
                    ldata.extend_from_slice(&png);
                    (12u8, 32u8, png.len() as u32 + 4)
                }
            };
            spr_nodes.extend_from_slice(&g.to_le_bytes());
            spr_nodes.extend_from_slice(&n.to_le_bytes());
            spr_nodes.extend_from_slice(&s.width.to_le_bytes());
            spr_nodes.extend_from_slice(&s.height.to_le_bytes());
            spr_nodes.extend_from_slice(&s.offset[0].to_le_bytes());
            spr_nodes.extend_from_slice(&s.offset[1].to_le_bytes());
            spr_nodes.extend_from_slice(&0u16.to_le_bytes());
            spr_nodes.push(fmt);
            spr_nodes.push(depth);
            spr_nodes.extend_from_slice(&ofs.to_le_bytes());
            spr_nodes.extend_from_slice(&len.to_le_bytes());
            spr_nodes.extend_from_slice(&(s.palette.unwrap_or(0) as u16).to_le_bytes());
            spr_nodes.extend_from_slice(&0u16.to_le_bytes());
        }
        let spr_ofs = out.len() as u32;
        out.extend_from_slice(&spr_nodes);
        let pal_ofs = out.len() as u32;
        out.extend_from_slice(&pal_nodes);
        let ldata_ofs = out.len() as u32;
        out.extend_from_slice(&ldata);
        let put = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        put(&mut out, 36, spr_ofs);
        put(&mut out, 40, self.sprites.len() as u32);
        put(&mut out, 44, pal_ofs);
        put(&mut out, 48, self.palettes.len() as u32);
        put(&mut out, 52, ldata_ofs);
        put(&mut out, 56, ldata.len() as u32);
        put(&mut out, 60, ldata_ofs + ldata.len() as u32);
        put(&mut out, 64, 0);
        out
    }

    /// Serialize so that [`Sff::parse`] (with the same `is_char`) gives this
    /// SFF back: same version (v1 as PCX, v2 with RLE8 / PNG), sprite order,
    /// sizes, axes, palette slots (SFFv1 palette sharing included) and
    /// pixels. Identical sprites become links. Used to export `.rgc` files.
    pub fn write_exact(&self) -> Vec<u8> {
        match self.version {
            Some(Version::V1) => self.write_exact_v1(),
            Some(Version::V2 { patch }) => self.write_exact_v2(patch),
            None => self.write_exact_v2(1),
        }
    }

    fn write_exact_v2(&self, patch: u8) -> Vec<u8> {
        let mut out = vec![0u8; 512];
        out[..12].copy_from_slice(b"ElecbyteSpr\0");
        out[12..16].copy_from_slice(&[0, patch, 0, 2]);
        // palette keys: slots without one (duplicate keys in the source) get unused keys
        let mut key_of: Vec<Option<Key>> = vec![None; self.palettes.len()];
        for (k, &i) in &self.palette_keys {
            if let Some(slot) = key_of.get_mut(i) {
                *slot = Some(*k);
            }
        }
        let used: std::collections::BTreeSet<Key> = self.palette_keys.keys().copied().collect();
        let mut spare = (0..=u16::MAX)
            .rev()
            .flat_map(|g| (0..=u16::MAX).rev().map(move |n| (g, n)))
            .filter(|k| !used.contains(k));
        let mut ldata: Vec<u8> = Vec::new();
        let mut pal_nodes: Vec<u8> = Vec::new();
        for (i, pal) in self.palettes.iter().enumerate() {
            let (g, n) = key_of[i].unwrap_or_else(|| spare.next().unwrap_or((0, 0)));
            let ofs = ldata.len() as u32;
            let count = pal.len().min(256);
            for c in pal.iter().take(count) {
                ldata.extend_from_slice(c);
            }
            pal_nodes.extend_from_slice(&g.to_le_bytes());
            pal_nodes.extend_from_slice(&n.to_le_bytes());
            pal_nodes.extend_from_slice(&(count as u16).to_le_bytes());
            pal_nodes.extend_from_slice(&0u16.to_le_bytes());
            pal_nodes.extend_from_slice(&ofs.to_le_bytes());
            pal_nodes.extend_from_slice(&((count * 4) as u32).to_le_bytes());
        }
        let mut spr_nodes: Vec<u8> = Vec::new();
        // pixels already written: (w, h, pixels) -> node index
        let mut written: std::collections::HashMap<(u16, u16, bool, &[u8]), u16> =
            std::collections::HashMap::new();
        let decoded: Vec<std::borrow::Cow<'_, Pixels>> =
            self.sprites.values().map(|s| s.decoded()).collect();
        for (i, (s, px)) in self.sprites.values().zip(&decoded).enumerate() {
            let node = |out: &mut Vec<u8>, link: u16, fmt: u8, depth: u8, ofs: u32, len: u32| {
                out.extend_from_slice(&s.group.to_le_bytes());
                out.extend_from_slice(&s.number.to_le_bytes());
                out.extend_from_slice(&s.width.to_le_bytes());
                out.extend_from_slice(&s.height.to_le_bytes());
                out.extend_from_slice(&s.offset[0].to_le_bytes());
                out.extend_from_slice(&s.offset[1].to_le_bytes());
                out.extend_from_slice(&link.to_le_bytes());
                out.push(fmt);
                out.push(depth);
                out.extend_from_slice(&ofs.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&(s.palette.unwrap_or(0) as u16).to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
            };
            match px.as_ref() {
                Pixels::Indexed(p) if p.is_empty() => {
                    // no pixels: a link to itself reads back as an empty sprite
                    node(&mut spr_nodes, i as u16, 0, 8, 0, 0);
                }
                Pixels::Indexed(p) => {
                    if s.palette.is_some()
                        && let Some(&j) = written.get(&(s.width, s.height, false, p.as_slice()))
                    {
                        node(&mut spr_nodes, j, 0, 8, 0, 0);
                        continue;
                    }
                    let ofs = ldata.len() as u32;
                    ldata.extend_from_slice(&(p.len() as u32).to_le_bytes());
                    rle8_encode(p, &mut ldata);
                    node(&mut spr_nodes, 0, 2, 8, ofs, ldata.len() as u32 - ofs);
                    if i <= u16::MAX as usize {
                        written
                            .entry((s.width, s.height, false, p.as_slice()))
                            .or_insert(i as u16);
                    }
                }
                Pixels::Rgba(p) => {
                    // a linked true-colour sprite reads back with its node's palette
                    if s.palette.is_some()
                        && let Some(&j) = written.get(&(s.width, s.height, true, p.as_slice()))
                    {
                        node(&mut spr_nodes, j, 0, 8, 0, 0);
                        continue;
                    }
                    let ofs = ldata.len() as u32;
                    let png = png_encode_rgba(p, s.width, s.height);
                    ldata.extend_from_slice(&(p.len() as u32).to_le_bytes());
                    ldata.extend_from_slice(&png);
                    node(&mut spr_nodes, 0, 12, 32, ofs, png.len() as u32 + 4);
                    if i <= u16::MAX as usize {
                        written
                            .entry((s.width, s.height, true, p.as_slice()))
                            .or_insert(i as u16);
                    }
                }
                Pixels::Packed(_) => unreachable!("decoded"),
            }
        }
        let spr_ofs = out.len() as u32;
        out.extend_from_slice(&spr_nodes);
        let pal_ofs = out.len() as u32;
        out.extend_from_slice(&pal_nodes);
        let ldata_ofs = out.len() as u32;
        out.extend_from_slice(&ldata);
        let put = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        put(&mut out, 36, spr_ofs);
        put(&mut out, 40, self.sprites.len() as u32);
        put(&mut out, 44, pal_ofs);
        put(&mut out, 48, self.palettes.len() as u32);
        put(&mut out, 52, ldata_ofs);
        put(&mut out, 56, ldata.len() as u32);
        put(&mut out, 60, ldata_ofs + ldata.len() as u32);
        put(&mut out, 64, 0);
        out
    }

    /// SFF v1: PCX sprites, each with its palette appended unless it shares
    /// the previous sprite's (the `samepal` flag), links for duplicates.
    ///
    /// The reader numbers palettes in the order sprites bring their own,
    /// including duplicate-key sprites it then drops; palette slots that no
    /// kept sprite introduces are written as such dropped duplicates, so
    /// every slot keeps its number.
    fn write_exact_v1(&self) -> Vec<u8> {
        struct Out {
            key: Key,
            offset: [i16; 2],
            link: u16,
            same_pal: bool,
            data: Vec<u8>,
        }
        let pcx_with = |px: &[u8], w: u16, h: u16, pal: Option<&Palette>| {
            let mut data = Vec::new();
            pcx_encode(px, w, h, &mut data);
            if let Some(pal) = pal {
                data.push(0x0C);
                for k in 0..256 {
                    let c = pal.get(k).copied().unwrap_or([0; 4]);
                    data.extend_from_slice(&c[..3]);
                }
            }
            data
        };
        let mut entries: Vec<Out> = Vec::with_capacity(self.sprites.len());
        let mut prev_pal: Option<usize> = None;
        let mut have_prev = false;
        // palette slots the reader has created so far
        let mut slots = 0usize;
        let mut written: std::collections::HashMap<PixelsKey<'_>, u16> =
            std::collections::HashMap::new();
        let decoded: Vec<std::borrow::Cow<'_, Pixels>> =
            self.sprites.values().map(|s| s.decoded()).collect();
        for (s, px) in self.sprites.values().zip(&decoded) {
            let px: &[u8] = match px.as_ref() {
                Pixels::Indexed(p) => p,
                // SFFv1 has no true colour (never parsed that way)
                Pixels::Rgba(p) => p,
                Pixels::Packed(_) => unreachable!("decoded"),
            };
            let index = entries.len() as u16;
            if px.is_empty() {
                // len 0 + a link to itself: an empty sprite
                entries.push(Out {
                    key: (s.group, s.number),
                    offset: s.offset,
                    link: index,
                    same_pal: false,
                    data: Vec::new(),
                });
                continue;
            }
            if let Some(&j) = written.get(&(s.width, s.height, s.palette, px)) {
                entries.push(Out {
                    key: (s.group, s.number),
                    offset: s.offset,
                    link: j,
                    same_pal: false,
                    data: Vec::new(),
                });
                continue;
            }
            let same_pal = have_prev && s.palette.is_some() && s.palette == prev_pal;
            if !same_pal && let Some(p) = s.palette {
                // slots introduced by dropped duplicates before this sprite
                while slots < p {
                    let Some(last) = entries.last().map(|e| e.key) else {
                        break;
                    };
                    entries.push(Out {
                        key: last,
                        offset: [0, 0],
                        link: entries.len() as u16,
                        same_pal: false,
                        data: pcx_with(&[0], 1, 1, self.palettes.get(slots)),
                    });
                    prev_pal = Some(slots);
                    have_prev = true;
                    slots += 1;
                }
            }
            let same_pal = have_prev && s.palette.is_some() && s.palette == prev_pal;
            let pal = (!same_pal).then(|| {
                s.palette
                    .and_then(|p| self.palettes.get(p))
                    .cloned()
                    .unwrap_or_default()
            });
            if !same_pal {
                slots += 1;
            }
            let index = entries.len() as u16;
            entries.push(Out {
                key: (s.group, s.number),
                offset: s.offset,
                link: index,
                same_pal,
                data: pcx_with(px, s.width, s.height, pal.as_ref()),
            });
            have_prev = true;
            prev_pal = s.palette;
            written
                .entry((s.width, s.height, s.palette, px))
                .or_insert(index);
        }
        // trailing slots no kept sprite uses
        while slots < self.palettes.len() {
            let Some(last) = entries.last().map(|e| e.key) else {
                break;
            };
            entries.push(Out {
                key: last,
                offset: [0, 0],
                link: entries.len() as u16,
                same_pal: false,
                data: pcx_with(&[0], 1, 1, self.palettes.get(slots)),
            });
            slots += 1;
        }
        let mut out = vec![0u8; 512];
        out[..12].copy_from_slice(b"ElecbyteSpr\0");
        out[12..16].copy_from_slice(&[0, 1, 0, 1]);
        let groups: std::collections::BTreeSet<u16> = self.sprites.keys().map(|k| k.0).collect();
        out[16..20].copy_from_slice(&(groups.len() as u32).to_le_bytes());
        out[20..24].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        out[24..28].copy_from_slice(&512u32.to_le_bytes());
        out[28..32].copy_from_slice(&32u32.to_le_bytes());
        let n = entries.len();
        for (i, e) in entries.iter().enumerate() {
            let at = out.len();
            let next = if i + 1 == n {
                0
            } else {
                (at + 32 + e.data.len()) as u32
            };
            let mut h = [0u8; 32];
            h[0..4].copy_from_slice(&next.to_le_bytes());
            h[4..8].copy_from_slice(&(e.data.len() as u32).to_le_bytes());
            h[8..10].copy_from_slice(&e.offset[0].to_le_bytes());
            h[10..12].copy_from_slice(&e.offset[1].to_le_bytes());
            h[12..14].copy_from_slice(&e.key.0.to_le_bytes());
            h[14..16].copy_from_slice(&e.key.1.to_le_bytes());
            h[16..18].copy_from_slice(&e.link.to_le_bytes());
            h[18] = u8::from(e.same_pal);
            out.extend_from_slice(&h);
            out.extend_from_slice(&e.data);
        }
        out
    }

    /// sha256 over everything [`Sff::parse`] yields that drawing depends on:
    /// version (v1 / v2), palette slots, palette keys, and every sprite's
    /// key, size, axis, palette slot and decoded pixels, in file order.
    /// Independent of how the file was compressed (PCX, RLE, LZ5, PNG, an
    /// `.rgc` section); the content fingerprint uses it for sprites.
    pub fn canonical_hash(&self) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(b"rugen-spr-1\n");
        h.update([match self.version {
            None => 0u8,
            Some(Version::V1) => 1,
            Some(Version::V2 { .. }) => 2,
        }]);
        h.update((self.palettes.len() as u32).to_le_bytes());
        for p in &self.palettes {
            h.update((p.len() as u32).to_le_bytes());
            for c in p {
                h.update(c);
            }
        }
        h.update((self.palette_keys.len() as u32).to_le_bytes());
        for (&(g, n), &i) in &self.palette_keys {
            h.update(g.to_le_bytes());
            h.update(n.to_le_bytes());
            h.update((i as u32).to_le_bytes());
        }
        h.update((self.sprites.len() as u32).to_le_bytes());
        for s in self.sprites.values() {
            h.update(s.group.to_le_bytes());
            h.update(s.number.to_le_bytes());
            h.update(s.width.to_le_bytes());
            h.update(s.height.to_le_bytes());
            h.update(s.offset[0].to_le_bytes());
            h.update(s.offset[1].to_le_bytes());
            h.update(s.palette.map_or(u32::MAX, |p| p as u32).to_le_bytes());
            let px = s.decoded();
            let (kind, bytes): (u8, &[u8]) = match px.as_ref() {
                Pixels::Indexed(p) => (0, p),
                Pixels::Rgba(p) => (1, p),
                Pixels::Packed(_) => (2, &[]),
            };
            h.update([kind]);
            h.update((bytes.len() as u32).to_le_bytes());
            h.update(bytes);
        }
        h.finalize().into()
    }

    /// Add an indexed sprite using palette slot `pal`.
    pub fn push_indexed(
        &mut self,
        key: Key,
        w: u16,
        h: u16,
        axis: [i16; 2],
        pal: usize,
        px: Vec<u8>,
    ) {
        self.sprites.insert(
            key,
            Sprite {
                group: key.0,
                number: key.1,
                width: w,
                height: h,
                offset: axis,
                palette: Some(pal),
                pixels: Pixels::Indexed(px),
            },
        );
    }

    /// Add an RGBA sprite.
    pub fn push_rgba(&mut self, key: Key, w: u16, h: u16, axis: [i16; 2], px: Vec<u8>) {
        self.sprites.insert(
            key,
            Sprite {
                group: key.0,
                number: key.1,
                width: w,
                height: h,
                offset: axis,
                palette: None,
                pixels: Pixels::Rgba(px),
            },
        );
    }

    /// Add a palette under `key`, returning its slot.
    pub fn push_palette(&mut self, key: Key, pal: Palette) -> usize {
        self.palettes.push(pal);
        let i = self.palettes.len() - 1;
        self.palette_keys.insert(key, i);
        i
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    #[test]
    fn v2_roundtrip() {
        let mut sff = Sff {
            version: Some(Version::V2 { patch: 1 }),
            ..Default::default()
        };
        let mut pal = vec![[0u8; 4]; 256];
        pal[1] = [255, 0, 0, 255];
        pal[2] = [0, 255, 0, 128];
        let p = sff.push_palette((0, 0), pal.clone());
        sff.push_indexed((0, 65), 3, 2, [1, -2], p, vec![0, 1, 2, 2, 1, 0]);
        sff.push_rgba((9, 1), 2, 1, [0, 0], vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let bytes = sff.write_v2();
        let back = Sff::parse(&bytes, false).expect("parse");
        assert_eq!(back.version, Some(Version::V2 { patch: 1 }));
        assert_eq!(back.palettes.len(), 1);
        assert_eq!(back.palettes[0][2], [0, 255, 0, 128]);
        let a = back.get(0, 65).unwrap();
        assert_eq!((a.width, a.height, a.offset), (3, 2, [1, -2]));
        assert_eq!(a.pixels, Pixels::Indexed(vec![0, 1, 2, 2, 1, 0]));
        assert_eq!(a.palette, Some(0));
        let b = back.get(9, 1).unwrap();
        assert_eq!(b.pixels, Pixels::Rgba(vec![1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(back.palette_keys.get(&(0, 0)), Some(&0));
    }

    /// `write_exact` gives back what was parsed, for both versions.
    pub(crate) fn assert_exact_roundtrip(sff: &Sff, is_char: bool, what: &str) {
        let bytes = sff.write_exact();
        let back = Sff::parse(&bytes, is_char).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(back.version, sff.version, "{what}");
        assert_eq!(back.palettes, sff.palettes, "{what}");
        assert_eq!(back.palette_keys, sff.palette_keys, "{what}");
        assert_eq!(back.sprites, sff.sprites, "{what}");
        assert_eq!(back.canonical_hash(), sff.canonical_hash(), "{what}");
        // and the other parse mode reads the written file the same way
        let other = Sff::parse(&bytes, !is_char).unwrap();
        assert_eq!(other.canonical_hash(), sff.canonical_hash(), "{what}");
    }

    #[test]
    fn exact_writer_roundtrips_reference_files() {
        let Some(root) = crate::fixtures::reference("") else {
            return;
        };
        let mut n = 0;
        for dir in ["mugen-1.1b1", "ikemen-go-v1.0.0"] {
            for entry in crate::fixtures::walk(&root.join(dir)) {
                if !entry
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("sff"))
                {
                    continue;
                }
                let is_char = entry.to_string_lossy().contains("/chars/");
                let sff = Sff::open(&entry, is_char).unwrap();
                assert_exact_roundtrip(&sff, is_char, &entry.display().to_string());
                n += 1;
            }
        }
        if n > 0 {
            assert!(n > 20);
        }
    }

    #[test]
    fn exact_writer_keeps_v1_palette_sharing_and_links() {
        let mut sff = Sff {
            version: Some(Version::V1),
            ..Default::default()
        };
        let pal = |c: u8| -> Palette {
            (0..256)
                .map(|i| [c, i as u8, 0, if i == 0 { 0 } else { 255 }])
                .collect()
        };
        sff.palettes = vec![pal(1), pal(2)];
        sff.push_indexed((0, 0), 3, 2, [1, 1], 0, vec![0, 1, 2, 0xC5, 0xC5, 0xC5]);
        sff.push_indexed((0, 1), 2, 1, [0, 0], 0, vec![7, 7]); // shares the palette
        sff.push_indexed((5, 0), 3, 2, [9, 9], 1, vec![1; 6]); // own palette
        sff.push_indexed((5, 1), 3, 2, [2, 2], 0, vec![0, 1, 2, 0xC5, 0xC5, 0xC5]); // link
        sff.sprites.insert(
            (6, 0),
            Sprite {
                group: 6,
                number: 0,
                width: 0,
                height: 0,
                offset: [0, 0],
                palette: None,
                pixels: Pixels::Indexed(Vec::new()),
            },
        );
        assert_exact_roundtrip(&sff, true, "v1 sample");
        let mut act = vec![[3, 3, 3, 255]; 256];
        act[0][3] = 0;
        let mut a = Sff::parse(&sff.write_exact(), true).unwrap();
        a.apply_v1_act(&act);
        assert_eq!(a.palettes[0], act, "0,0 and its sharers follow pal1");
        assert_eq!(a.palettes[1], pal(2));
    }
}
