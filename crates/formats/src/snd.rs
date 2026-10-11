//! SND sound archives: `ElecbyteSnd\0`, a table of WAV subfiles keyed by
//! `(group, number)`. Audio decoding is left to the audio crate; this
//! returns the raw WAV bytes.

use std::io::{Cursor, Read, Seek, SeekFrom};

use indexmap::IndexMap;

use crate::{Error, Result};

const F: &str = "snd";

#[derive(Clone, Debug, Default)]
pub struct Snd {
    pub version: (u16, u16),
    /// Raw WAV file bytes per key. First duplicate wins, like MUGEN.
    pub sounds: IndexMap<(i32, i32), Vec<u8>>,
}

impl Snd {
    pub fn get(&self, group: i32, number: i32) -> Option<&[u8]> {
        self.sounds.get(&(group, number)).map(Vec::as_slice)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
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
                    snd.sounds.insert(key, bytes[start..end].to_vec());
                }
            }
            if next == 0 || next == ofs {
                break;
            }
            ofs = next;
        }
        Ok(snd)
    }

    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::parse(&std::fs::read(path)?)
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
