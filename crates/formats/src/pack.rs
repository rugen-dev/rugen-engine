//! Compressed sprite pixels ([`Packed`]) as loaded from an `.rgc` / `.rgs`
//! sprite section ([`crate::rgc::sprites`]), and source file stamps.
//!
//! Pixels stay zstd-compressed in memory, sharing the section's buffer,
//! until a sprite is first drawn.

use std::path::Path;
use std::sync::Arc;

use crate::sff::Pixels;

/// zstd-compressed pixels of one sprite, sharing a section's buffer.
///
/// The stored block may be the sprite's visible rectangle only; `pad` then
/// says where it sits inside the zero-filled `w x h` image the sprite shows.
#[derive(Clone)]
pub struct Packed {
    buf: Arc<Vec<u8>>,
    start: usize,
    len: usize,
    /// Bytes of the stored block once decompressed.
    block_len: usize,
    rgba: bool,
    /// `[w, h, x, y, block w, block h]` when the block is padded on decode.
    pad: Option<[u16; 6]>,
}

thread_local! {
    static DCTX: std::cell::RefCell<Option<zstd::bulk::Decompressor<'static>>> =
        const { std::cell::RefCell::new(None) };
    static CCTX: std::cell::RefCell<Option<(i32, zstd::bulk::Compressor<'static>)>> =
        const { std::cell::RefCell::new(None) };
}

/// zstd `src` at `level` into one frame, reusing this thread's context (a
/// fresh context per block costs more than compressing a small block).
pub(crate) fn zstd(src: &[u8], level: i32) -> std::io::Result<Vec<u8>> {
    CCTX.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref().is_none_or(|(l, _)| *l != level) {
            *c = Some((level, zstd::bulk::Compressor::new(level)?));
        }
        match c.as_mut() {
            Some((_, z)) => z.compress(src),
            None => zstd::bulk::compress(src, level),
        }
    })
}

/// Decompress a zstd frame of exactly `n` bytes, reusing this thread's context.
pub(crate) fn unzstd(src: &[u8], n: usize) -> Option<Vec<u8>> {
    DCTX.with(|d| {
        let mut d = d.borrow_mut();
        if d.is_none() {
            *d = zstd::bulk::Decompressor::new().ok();
        }
        d.as_mut()?.decompress(src, n).ok()
    })
    .filter(|v| v.len() == n)
}

impl Packed {
    pub(crate) fn new(
        buf: Arc<Vec<u8>>,
        start: usize,
        len: usize,
        block_len: usize,
        rgba: bool,
        pad: Option<[u16; 6]>,
    ) -> Self {
        Self {
            buf,
            start,
            len,
            block_len,
            rgba,
            pad,
        }
    }

    /// Decompress into `Indexed` / `Rgba`. Corrupt data decodes to transparent pixels.
    pub fn decode(&self) -> Pixels {
        let src = self
            .buf
            .get(self.start..self.start + self.len)
            .unwrap_or(&[]);
        let block = if self.block_len == 0 {
            Vec::new()
        } else {
            unzstd(src, self.block_len).unwrap_or_else(|| vec![0; self.block_len])
        };
        let px = match self.pad {
            None => block,
            Some([w, h, x, y, bw, bh]) => {
                let bpp = if self.rgba { 4 } else { 1 };
                let (w, h, x, y, bw, bh) = (
                    w as usize,
                    h as usize,
                    x as usize,
                    y as usize,
                    bw as usize,
                    bh as usize,
                );
                let mut out = vec![0u8; w * h * bpp];
                for row in 0..bh.min(h.saturating_sub(y)) {
                    let src = row * bw * bpp;
                    let dst = ((y + row) * w + x) * bpp;
                    let n = (bw.min(w.saturating_sub(x))) * bpp;
                    if let (Some(s), Some(d)) = (block.get(src..src + n), out.get_mut(dst..dst + n))
                    {
                        d.copy_from_slice(s);
                    }
                }
                out
            }
        };
        if self.rgba {
            Pixels::Rgba(px)
        } else {
            Pixels::Indexed(px)
        }
    }

    /// Same value for sprites showing the same pixels (one texture).
    pub fn share_key(&self) -> (usize, usize, u64) {
        let pad = self.pad.map_or(0, |p| {
            1 | (u64::from(p[0]) << 1)
                ^ (u64::from(p[1]) << 17)
                ^ (u64::from(p[2]) << 33)
                ^ (u64::from(p[3]) << 47)
        });
        (Arc::as_ptr(&self.buf) as usize, self.start, pad)
    }

    pub fn is_rgba(&self) -> bool {
        self.rgba
    }
    /// Decompressed size in bytes (after padding).
    pub fn raw_len(&self) -> usize {
        match self.pad {
            None => self.block_len,
            Some([w, h, ..]) => w as usize * h as usize * if self.rgba { 4 } else { 1 },
        }
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
            self.raw_len()
        )
    }
}

impl PartialEq for Packed {
    fn eq(&self, o: &Self) -> bool {
        self.decode() == o.decode()
    }
}
impl Eq for Packed {}

/// Size and modification time of a file; a derived artefact whose stamp
/// no longer matches its source is stale.
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
