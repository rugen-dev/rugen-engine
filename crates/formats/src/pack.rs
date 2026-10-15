//! Compressed sprite pixels ([`Packed`]) as loaded from an `.rgc` / `.rgs`
//! sprite section ([`crate::rgc::sprites`]), and source file stamps.
//!
//! Pixels stay zstd-compressed until a sprite is first drawn, as a range of
//! the section ([`Blob`]: held in memory, or read from the content file
//! when the sprite decodes).

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use crate::sff::Pixels;

/// zstd-compressed pixels of one sprite, sharing a section's buffer.
///
/// The stored block may be the sprite's visible rectangle only; `pad` then
/// says where it sits inside the zero-filled `w x h` image the sprite shows.
#[derive(Clone)]
pub struct Packed {
    buf: Arc<Blob>,
    start: usize,
    len: usize,
    /// Bytes of the stored block once decompressed.
    block_len: usize,
    rgba: bool,
    /// `[w, h, x, y, block w, block h]` when the block is padded on decode.
    pad: Option<[u16; 6]>,
}

/// Where a section's bytes live: in memory, or in the content file they
/// are stored in, read when a block is asked for. A fighter's sprite
/// section is 25-70 MB and only the sprites that draw are ever decoded,
/// so holding it resident was the biggest part of a loaded fighter.
pub enum Blob {
    Mem(Vec<u8>),
    /// `len` bytes of `file` from `offset` (a stored section). The handle
    /// stays open (shared for delete on Windows), so a converter that
    /// replaces the content file underneath does not change what it reads.
    File {
        file: std::fs::File,
        offset: u64,
        len: u64,
    },
}

impl Blob {
    pub fn mem(bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self::Mem(bytes))
    }

    /// `len` bytes of `path` from `offset`, read when asked.
    pub fn file(path: &Path, offset: u64, len: u64) -> std::io::Result<Arc<Self>> {
        let file = open_shared(path)?;
        Ok(Arc::new(Self::File { file, offset, len }))
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Mem(b) => b.len(),
            Self::File { len, .. } => *len as usize,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `len` bytes from `start`; `None` outside the blob or when the file
    /// cannot be read.
    pub fn slice(&self, start: usize, len: usize) -> Option<Cow<'_, [u8]>> {
        if start.checked_add(len)? > self.len() {
            return None;
        }
        match self {
            Self::Mem(b) => Some(Cow::Borrowed(&b[start..start + len])),
            Self::File { file, offset, .. } => {
                let mut out = vec![0u8; len];
                pread(file, offset + start as u64, &mut out)
                    .map_err(|e| log::warn!("content file: read at {start}: {e}"))
                    .ok()?;
                Some(Cow::Owned(out))
            }
        }
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mem(b) => write!(f, "Blob::Mem({} bytes)", b.len()),
            Self::File { offset, len, .. } => write!(f, "Blob::File({len} bytes at {offset})"),
        }
    }
}

#[cfg(windows)]
fn open_shared(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE: others may
    // still replace the file while this handle reads the old one
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2 | 0x4)
        .open(path)
}

#[cfg(not(windows))]
fn open_shared(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// Fill `buf` from `file` at `offset` without moving a shared cursor.
#[cfg(unix)]
fn pread(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(windows)]
fn pread(file: &std::fs::File, mut offset: u64, mut buf: &mut [u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
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
        buf: Arc<Blob>,
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
        let block = if self.block_len == 0 {
            Vec::new()
        } else {
            self.buf
                .slice(self.start, self.len)
                .and_then(|src| unzstd(&src, self.block_len))
                .unwrap_or_else(|| vec![0; self.block_len])
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
        (
            Arc::as_ptr(&self.buf) as *const u8 as usize,
            self.start,
            pad,
        )
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
