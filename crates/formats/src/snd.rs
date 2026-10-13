//! SND sound archives: `ElecbyteSnd\0`, a table of WAV subfiles keyed by
//! `(group, number)`. Audio decoding is left to the audio crate; this
//! returns the raw WAV bytes.
//!
//! Sounds are views into one shared copy of the file ([`Sound`]): loading is a
//! single read, with no per-sound copy. Sounds from an `.rgc` sound section
//! are Opus streams turned into WAV bytes the first time they are asked for
//! ([`crate::rgc::sounds`]).

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;

use crate::{Error, Result};

const F: &str = "snd";

/// One sound's WAV bytes.
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
    /// Decoded on first use, then kept.
    Lazy(Arc<Lazy>),
}

type Decode = Box<dyn Fn() -> Vec<u8> + Send + Sync>;

struct Lazy {
    decode: Decode,
    wav: OnceLock<Vec<u8>>,
}

impl Sound {
    /// A sound whose WAV bytes `decode` produces on first use.
    pub(crate) fn lazy(decode: Decode) -> Self {
        Sound(Repr::Lazy(Arc::new(Lazy {
            decode,
            wav: OnceLock::new(),
        })))
    }

    /// Already decoded (or never needed decoding).
    pub fn is_ready(&self) -> bool {
        match &self.0 {
            Repr::Range { .. } => true,
            Repr::Lazy(l) => l.wav.get().is_some(),
        }
    }
}

impl std::ops::Deref for Sound {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match &self.0 {
            Repr::Range { buf, start, end } => &buf[*start..*end],
            Repr::Lazy(l) => l.wav.get_or_init(|| (l.decode)()),
        }
    }
}

impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Repr::Range { start, end, .. } => write!(f, "Sound({} bytes)", end - start),
            Repr::Lazy(l) => match l.wav.get() {
                Some(w) => write!(f, "Sound({} bytes, decoded)", w.len()),
                None => write!(f, "Sound(not decoded yet)"),
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snd {
    pub version: (u16, u16),
    /// Raw WAV file bytes per key. First duplicate wins, like MUGEN.
    pub sounds: IndexMap<(i32, i32), Sound>,
}

impl Snd {
    pub fn get(&self, group: i32, number: i32) -> Option<&[u8]> {
        self.sounds.get(&(group, number)).map(|s| &**s)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::parse_shared(Arc::new(bytes.to_vec()))
    }

    /// Parse, keeping `buf` as the storage of every sound.
    pub fn parse_shared(buf: Arc<Vec<u8>>) -> Result<Self> {
        let bytes: &[u8] = &buf;
        let mut r = Cursor::new(bytes);
        let mut sig = [0u8; 12];
        r.read_exact(&mut sig)?;
        if &sig != b"ElecbyteSnd\0" {
            return Err(Error::fmt(F, "bad signature"));
        }
        let version = (u16_(&mut r)?, u16_(&mut r)?);
        let count = u32_(&mut r)?;
        let mut ofs = u32_(&mut r)? as u64;
        let mut snd = Snd {
            version,
            sounds: IndexMap::new(),
        };
        for i in 0..count {
            if r.seek(SeekFrom::Start(ofs)).is_err() || ofs as usize + 16 > bytes.len() {
                log::warn!("snd: truncated at sound {i}/{count}");
                break;
            }
            let next = u32_(&mut r)? as u64;
            let len = u32_(&mut r)? as usize;
            let key = (u32_(&mut r)? as i32, u32_(&mut r)? as i32);
            let start = ofs as usize + 16;
            let end = start.saturating_add(len).min(bytes.len());
            if key.0 >= 0 && key.1 >= 0 {
                if snd.sounds.contains_key(&key) {
                    log::warn!(
                        "snd: duplicate sound {},{} (index {i} ignored)",
                        key.0,
                        key.1
                    );
                } else if len < 128 || end - start < 128 {
                    log::warn!(
                        "snd: sound {},{} too small ({len} bytes), disabled",
                        key.0,
                        key.1
                    );
                } else {
                    snd.sounds.insert(
                        key,
                        Sound(Repr::Range {
                            buf: buf.clone(),
                            start,
                            end,
                        }),
                    );
                }
            }
            if next == 0 || next == ofs {
                break;
            }
            ofs = next;
        }
        Ok(snd)
    }

    /// Open an SND file, or the sound section standing for it inside an
    /// `.rgc` / `.rgs` ([`crate::vfs`]).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        crate::vfs::open_snd(path.as_ref())
    }

    /// Decode every sound not decoded yet on a background thread, so the
    /// first play of each needs no work (sounds from `.rgc` sections).
    pub fn prefetch(&self) {
        let pending: Vec<Sound> = self
            .sounds
            .values()
            .filter(|s| !s.is_ready())
            .cloned()
            .collect();
        if pending.is_empty() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("snd-prefetch".into())
            .spawn(move || {
                for s in pending {
                    let _ = s.len();
                }
            });
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
        for (i, (&(g, num), wav)) in self.sounds.iter().enumerate() {
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
            out.extend_from_slice(wav);
        }
        out
    }
}

fn u16_(r: &mut Cursor<&[u8]>) -> Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn u32_(r: &mut Cursor<&[u8]>) -> Result<u32> {
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
        for (k, wav) in &s.sounds {
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
