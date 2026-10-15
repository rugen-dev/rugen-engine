//! SND sound archives: `ElecbyteSnd\0`, a table of WAV subfiles keyed by
//! `(group, number)`. Audio decoding is left to the audio crate; this
//! returns the raw WAV bytes.
//!
//! Nothing is held for a sound until it plays: a `.snd` on disk keeps only
//! its table and each sound is read from the file when asked for, a sound
//! section of an `.rgc` keeps its (small, Opus) section bytes and decodes a
//! sound each time it is asked for ([`crate::rgc::sounds`]), an archive
//! parsed from memory shares that one buffer. The audio crate holds the
//! decoded samples of what played, so a WAV copy here would be a second
//! copy of every sound (a big fighter's `.snd` is 70 MB).

use std::borrow::Cow;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::{Error, Result};

const F: &str = "snd";

/// One sound's WAV bytes, produced by [`Sound::bytes`].
#[derive(Clone)]
pub struct Sound(Repr);

#[derive(Clone)]
enum Repr {
    /// A range of the archive's shared buffer.
    Range {
        buf: Arc<Vec<u8>>,
        start: usize,
        end: usize,
    },
    /// A range of a `.snd` file, read when asked for.
    File {
        path: Arc<PathBuf>,
        start: u64,
        len: usize,
    },
    /// Produced when asked for (an Opus stream decoded to WAV).
    Lazy(Arc<Decode>),
}

type Decode = Box<dyn Fn() -> Vec<u8> + Send + Sync>;

impl Sound {
    /// A sound whose WAV bytes `decode` produces each time they are asked for.
    pub(crate) fn lazy(decode: Decode) -> Self {
        Sound(Repr::Lazy(Arc::new(decode)))
    }

    /// The WAV bytes: borrowed from an archive held in memory, else read
    /// or decoded now. Empty when a file read fails (the sound is skipped).
    pub fn bytes(&self) -> Cow<'_, [u8]> {
        match &self.0 {
            Repr::Range { buf, start, end } => Cow::Borrowed(&buf[*start..*end]),
            Repr::File { path, start, len } => {
                Cow::Owned(read_range(path, *start, *len).unwrap_or_else(|e| {
                    log::warn!("{}: sound at {start}: {e}", path.display());
                    Vec::new()
                }))
            }
            Repr::Lazy(decode) => Cow::Owned(decode()),
        }
    }

    /// Size of the stored bytes; for a decoded sound, of the stream.
    pub fn stored_len(&self) -> Option<usize> {
        match &self.0 {
            Repr::Range { start, end, .. } => Some(end - start),
            Repr::File { len, .. } => Some(*len),
            Repr::Lazy(_) => None,
        }
    }
}

fn read_range(path: &Path, start: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Repr::Range { start, end, .. } => write!(f, "Sound({} bytes)", end - start),
            Repr::File { len, .. } => write!(f, "Sound({len} bytes on disk)"),
            Repr::Lazy(_) => write!(f, "Sound(decoded on use)"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snd {
    pub version: (u16, u16),
    /// Raw WAV file bytes per key. First duplicate wins, like MUGEN.
    pub sounds: IndexMap<(i32, i32), Sound>,
}

/// One table entry: key, byte range of the WAV.
struct Row {
    key: (i32, i32),
    start: u64,
    len: usize,
}

/// Read the header and the linked table of an SND, `total` bytes long.
fn table<R: Read + Seek>(r: &mut R, total: u64) -> Result<((u16, u16), Vec<Row>)> {
    let mut sig = [0u8; 12];
    r.read_exact(&mut sig)?;
    if &sig != b"ElecbyteSnd\0" {
        return Err(Error::fmt(F, "bad signature"));
    }
    let version = (u16_(r)?, u16_(r)?);
    let count = u32_(r)?;
    let mut ofs = u32_(r)? as u64;
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for i in 0..count {
        if ofs + 16 > total || r.seek(SeekFrom::Start(ofs)).is_err() {
            log::debug!("snd: truncated at sound {i}/{count}");
            break;
        }
        let next = u32_(r)? as u64;
        let len = u32_(r)? as usize;
        let key = (u32_(r)? as i32, u32_(r)? as i32);
        let start = ofs + 16;
        let end = start.saturating_add(len as u64).min(total);
        if key.0 >= 0 && key.1 >= 0 {
            if !seen.insert(key) {
                log::debug!(
                    "snd: duplicate sound {},{} (index {i} ignored)",
                    key.0,
                    key.1
                );
            } else if len < 128 || end - start < 128 {
                log::debug!(
                    "snd: sound {},{} too small ({len} bytes), disabled",
                    key.0,
                    key.1
                );
            } else {
                rows.push(Row {
                    key,
                    start,
                    len: (end - start) as usize,
                });
            }
        }
        if next == 0 || next == ofs {
            break;
        }
        ofs = next;
    }
    Ok((version, rows))
}

impl Snd {
    /// The WAV bytes of a sound (see [`Sound::bytes`]).
    pub fn get(&self, group: i32, number: i32) -> Option<Cow<'_, [u8]>> {
        self.sounds.get(&(group, number)).map(Sound::bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::parse_shared(Arc::new(bytes.to_vec()))
    }

    /// Parse, keeping `buf` as the storage of every sound.
    pub fn parse_shared(buf: Arc<Vec<u8>>) -> Result<Self> {
        let (version, rows) = table(&mut Cursor::new(&buf[..]), buf.len() as u64)?;
        let sounds = rows
            .into_iter()
            .map(|r| {
                let start = r.start as usize;
                (
                    r.key,
                    Sound(Repr::Range {
                        buf: buf.clone(),
                        start,
                        end: start + r.len,
                    }),
                )
            })
            .collect();
        Ok(Snd { version, sounds })
    }

    /// Read the table of a `.snd` on disk; each sound is read from the file
    /// when asked for.
    pub fn open_file(path: &Path) -> Result<Self> {
        let mut f = std::io::BufReader::new(std::fs::File::open(path)?);
        let total = f.get_ref().metadata()?.len();
        let (version, rows) = table(&mut f, total)?;
        let path = Arc::new(path.to_path_buf());
        let sounds = rows
            .into_iter()
            .map(|r| {
                (
                    r.key,
                    Sound(Repr::File {
                        path: path.clone(),
                        start: r.start,
                        len: r.len,
                    }),
                )
            })
            .collect();
        Ok(Snd { version, sounds })
    }

    /// Open an SND file, or the sound section standing for it inside an
    /// `.rgc` / `.rgs` ([`crate::vfs`]).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        crate::vfs::open_snd(path.as_ref())
    }

    /// Serialize as an SND file (version kept, sounds in order, each as its
    /// WAV bytes).
    pub fn write(&self) -> Vec<u8> {
        let mut out = vec![0u8; 512];
        out[..12].copy_from_slice(b"ElecbyteSnd\0");
        out[12..14].copy_from_slice(&self.version.0.to_le_bytes());
        out[14..16].copy_from_slice(&self.version.1.to_le_bytes());
        out[16..20].copy_from_slice(&(self.sounds.len() as u32).to_le_bytes());
        out[20..24].copy_from_slice(&512u32.to_le_bytes());
        let n = self.sounds.len();
        for (i, (&(g, num), sound)) in self.sounds.iter().enumerate() {
            let wav = sound.bytes();
            let at = out.len();
            let next = if i + 1 == n {
                0
            } else {
                (at + 16 + wav.len()) as u32
            };
            out.extend_from_slice(&next.to_le_bytes());
            out.extend_from_slice(&(wav.len() as u32).to_le_bytes());
            out.extend_from_slice(&(g as u32).to_le_bytes());
            out.extend_from_slice(&(num as u32).to_le_bytes());
            out.extend_from_slice(&wav);
        }
        out
    }
}

fn u16_<R: Read>(r: &mut R) -> Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn u32_<R: Read>(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::reference;

    #[test]
    fn kfm_snd() {
        let Some(p) = reference("mugen-1.1b1/chars/kfm/kfm.snd") else {
            return;
        };
        let s = Snd::open(p).unwrap();
        assert_eq!(s.sounds.len(), 12);
        for (k, sound) in &s.sounds {
            let wav = sound.bytes();
            assert!(wav.starts_with(b"RIFF"), "sound {k:?} is not RIFF");
            assert_eq!(&wav[8..12], b"WAVE", "sound {k:?} is not WAVE");
        }
    }

    #[test]
    fn all_reference_snds_load() {
        let Some(root) = reference("") else { return };
        let mut n = 0;
        for dir in ["mugen-1.1b1", "ikemen-go-v1.0.0"] {
            for p in crate::fixtures::walk(&root.join(dir)) {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("snd")) {
                    let s = Snd::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    assert!(!s.sounds.is_empty(), "{} has no sounds", p.display());
                    n += 1;
                }
            }
        }
        if n == 0 {
            eprintln!("reference tree missing; skipping");
            return;
        }
        assert!(n >= 5, "found only {n} snd files");
    }

    #[test]
    fn garbage() {
        assert!(Snd::parse(b"nope").is_err());
        let mut v = b"ElecbyteSnd\0\x00\x00\x00\x00\xff\xff\xff\xff\x10\x00\x00\x00".to_vec();
        v.extend_from_slice(&[0xAB; 40]);
        let _ = Snd::parse(&v).unwrap();
    }
}
