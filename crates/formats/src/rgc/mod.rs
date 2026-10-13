//! rugen content files: `.rgc` (character) and `.rgs` (stage).
//!
//! One file per character / stage holding every file of the original
//! folder: sprites re-encoded pixel-exact ([`sprites`]), sounds as Opus
//! ([`sounds`]), loose PNGs as lossless WebP, everything else (def, cns,
//! cmd, air, act, readmes, music) byte for byte, plus the compiled logic.
//! The full specification is `docs/rgc-format.md`; in short (little endian):
//!
//! ```text
//! header  64 bytes: magic "RUGENRGC" / "RUGENRGS", major u16, minor u16,
//!         flags u32, toc offset u64, toc len u32, toc raw len u32,
//!         fingerprint [32]
//! sections, each at its own offset (random access, nothing to unpack)
//! toc     zstd( count u32, entry* )
//! entry   kind u8, codec u8, flags u16, offset u64, len u64, raw len u64,
//!         source len u64, source mtime ns u64, hash [32], path len u16, path
//! ```
//!
//! Readers check the magic and refuse another major version; minor
//! versions only add section kinds or flags that older readers skip.

pub mod sounds;
pub mod sprites;

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const F: &str = "rgc";

pub const MAGIC_CHAR: &[u8; 8] = b"RUGENRGC";
pub const MAGIC_STAGE: &[u8; 8] = b"RUGENRGS";
/// Incompatible layout changes bump this; readers refuse other majors.
pub const MAJOR: u16 = 1;
/// Additive changes (new section kinds / flags) bump this.
pub const MINOR: u16 = 0;
pub const HEADER_LEN: usize = 64;
/// Entries a TOC may list.
pub const MAX_ENTRIES: usize = 16_384;
/// Largest decompressed section a reader accepts.
pub const MAX_SECTION: u64 = 1 << 30;
/// zstd level for files, logic and the TOC.
pub const LEVEL: i32 = 19;

/// What a content file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Char,
    Stage,
}

impl Kind {
    pub fn magic(self) -> &'static [u8; 8] {
        match self {
            Kind::Char => MAGIC_CHAR,
            Kind::Stage => MAGIC_STAGE,
        }
    }

    /// File extension (`rgc` / `rgs`).
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Char => "rgc",
            Kind::Stage => "rgs",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "rgc" => Some(Kind::Char),
            "rgs" => Some(Kind::Stage),
            _ => None,
        }
    }
}

/// Section kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// JSON [`Meta`].
    Meta,
    /// A file kept byte for byte.
    File,
    /// An SFF, as a [`sprites`] section.
    Sprites,
    /// An SND, as a [`sounds`] section.
    Sounds,
    /// A loose image as lossless WebP (`flags`: [`IMAGE_PNG`]).
    Image,
    /// Compiled logic of the main def: schema len u16, schema, payload.
    Logic,
    /// Unknown kind from a newer minor version (skipped).
    Other(u8),
}

impl SectionKind {
    fn byte(self) -> u8 {
        match self {
            SectionKind::Meta => 1,
            SectionKind::File => 2,
            SectionKind::Sprites => 3,
            SectionKind::Sounds => 4,
            SectionKind::Image => 5,
            SectionKind::Logic => 6,
            SectionKind::Other(b) => b,
        }
    }

    fn from_byte(b: u8) -> Self {
        match b {
            1 => SectionKind::Meta,
            2 => SectionKind::File,
            3 => SectionKind::Sprites,
            4 => SectionKind::Sounds,
            5 => SectionKind::Image,
            6 => SectionKind::Logic,
            b => SectionKind::Other(b),
        }
    }

    /// Sections that stand for a file of the original folder.
    pub fn is_file(self) -> bool {
        matches!(
            self,
            SectionKind::File | SectionKind::Sprites | SectionKind::Sounds | SectionKind::Image
        )
    }
}

/// `Image` flag: the original was a PNG (exported back as PNG).
pub const IMAGE_PNG: u16 = 1;
/// `Sprites` flag: parsed as a character's sprite file (SFFv1 palette rule).
pub const SPRITES_CHAR: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Stored,
    Zstd,
}

/// One TOC row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: SectionKind,
    pub codec: Codec,
    pub flags: u16,
    /// From the start of the file.
    pub offset: u64,
    /// Bytes stored.
    pub len: u64,
    /// Bytes after decompression.
    pub raw_len: u64,
    /// Size and mtime (ns) of the source file it was converted from (0 if
    /// unknown): lets a re-conversion reuse unchanged sections.
    pub src_len: u64,
    pub src_mtime: u64,
    /// `File`: sha256 of the bytes. `Sprites`: [`crate::sff::Sff::canonical_hash`].
    /// `Sounds` / `Image`: sha256 of the original file. Others: of the payload.
    pub hash: [u8; 32],
    /// Path of the original file relative to the content root, `/`
    /// separated, original case. Empty for `Meta` / `Logic`.
    pub path: String,
}

impl Entry {
    pub fn new(kind: SectionKind, path: &str) -> Self {
        Self {
            kind,
            codec: Codec::Stored,
            flags: 0,
            offset: 0,
            len: 0,
            raw_len: 0,
            src_len: 0,
            src_mtime: 0,
            hash: [0; 32],
            path: path.to_string(),
        }
    }
}

/// The `Meta` section.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Meta {
    /// Main def, relative to the content root.
    pub def: String,
    pub name: String,
    pub author: String,
    /// `rugen x.y.z` that converted it.
    pub converter: String,
    /// Unix seconds.
    pub created: u64,
    /// File or folder name it was converted from.
    pub source: String,
    /// Files the converter added from the engine's `data/` because the
    /// content references them without shipping them (`common1.cns`).
    pub engine_files: Vec<String>,
}

/// Canonical form of a relative path: `/` separators, no `.` parts, lowercase.
pub fn canon(path: &str) -> String {
    clean(path).to_ascii_lowercase()
}

/// `/` separators, no empty or `.` parts, surrounding quotes and spaces removed.
pub fn clean(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .replace('\\', "/")
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// A relative path that stays inside its root.
pub fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains(['\\', ':', '\0'])
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
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

/// An opened content file: header and TOC in memory, sections read on demand.
#[derive(Clone, Debug)]
pub struct Archive {
    pub path: PathBuf,
    pub kind: Kind,
    pub minor: u16,
    /// Fingerprint recorded at conversion (a claim: verify with `rugen-fair`).
    pub fingerprint: [u8; 32],
    pub entries: Vec<Entry>,
    pub file_len: u64,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut f = std::fs::File::open(path)?;
        let file_len = f.metadata()?.len();
        let mut h = [0u8; HEADER_LEN];
        f.read_exact(&mut h)
            .map_err(|_| Error::fmt(F, "truncated header"))?;
        let kind = match &h[..8] {
            m if m == MAGIC_CHAR => Kind::Char,
            m if m == MAGIC_STAGE => Kind::Stage,
            _ => return Err(Error::fmt(F, "not a rugen content file")),
        };
        let major = le16(&h, 8);
        if major != MAJOR {
            return Err(Error::fmt(
                F,
                format!("format {major} (this engine reads {MAJOR})"),
            ));
        }
        let minor = le16(&h, 10);
        let toc_ofs = le64(&h, 16);
        let toc_len = le32(&h, 24) as u64;
        let toc_raw = le32(&h, 28) as u64;
        if toc_ofs < HEADER_LEN as u64
            || toc_ofs.checked_add(toc_len).is_none_or(|e| e > file_len)
            || toc_raw > 64 << 20
        {
            return Err(Error::fmt(F, "bad table of contents"));
        }
        let mut fingerprint = [0u8; 32];
        fingerprint.copy_from_slice(&h[32..64]);
        f.seek(SeekFrom::Start(toc_ofs))?;
        let mut z = vec![0u8; toc_len as usize];
        f.read_exact(&mut z)?;
        let toc = zstd::bulk::decompress(&z, toc_raw as usize)
            .map_err(|e| Error::fmt(F, format!("toc: {e}")))?;
        let entries = parse_toc(&toc, file_len)?;
        Ok(Self {
            path: path.to_path_buf(),
            kind,
            minor,
            fingerprint,
            entries,
            file_len,
        })
    }

    /// The section standing for the original file `path` (case-insensitive).
    pub fn find(&self, path: &str) -> Option<&Entry> {
        let want = canon(path);
        self.entries
            .iter()
            .find(|e| e.kind.is_file() && canon(&e.path) == want)
    }

    /// The first section of `kind`.
    pub fn first(&self, kind: SectionKind) -> Option<&Entry> {
        self.entries.iter().find(|e| e.kind == kind)
    }

    /// Bytes of a section as stored.
    pub fn read_raw(&self, e: &Entry) -> Result<Vec<u8>> {
        let mut f = std::fs::File::open(&self.path)?;
        f.seek(SeekFrom::Start(e.offset))?;
        let mut buf = vec![0u8; e.len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Bytes of a section, decompressed.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>> {
        let raw = self.read_raw(e)?;
        match e.codec {
            Codec::Stored => Ok(raw),
            Codec::Zstd => {
                let out = zstd::bulk::decompress(&raw, e.raw_len as usize)
                    .map_err(|err| Error::fmt(F, format!("{}: {err}", e.path)))?;
                if out.len() as u64 != e.raw_len {
                    return Err(Error::fmt(F, format!("{}: short section", e.path)));
                }
                Ok(out)
            }
        }
    }

    pub fn meta(&self) -> Result<Meta> {
        let e = self
            .first(SectionKind::Meta)
            .ok_or_else(|| Error::fmt(F, "no meta section"))?;
        serde_json::from_slice(&self.read(e)?).map_err(|e| Error::fmt(F, format!("meta: {e}")))
    }

    /// `(schema, payload)` of the compiled logic section, if any.
    pub fn logic(&self) -> Option<(String, Vec<u8>)> {
        let e = self.first(SectionKind::Logic)?;
        let b = self.read(e).ok()?;
        let n = le16(b.get(..2)?, 0) as usize;
        let schema = String::from_utf8(b.get(2..2 + n)?.to_vec()).ok()?;
        Some((schema, b[2 + n..].to_vec()))
    }

    /// Paths of the original files, in TOC order.
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.kind.is_file())
    }
}

/// Fixed part of a TOC row (the path follows).
const TOC_ROW: usize = 78;

fn parse_toc(b: &[u8], file_len: u64) -> Result<Vec<Entry>> {
    let bad = || Error::fmt(F, "truncated table of contents");
    let count = le32(b.get(..4).ok_or_else(bad)?, 0) as usize;
    if count > MAX_ENTRIES {
        return Err(Error::fmt(F, format!("{count} sections")));
    }
    let mut at = 4;
    let mut out = Vec::with_capacity(count);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..count {
        let r = b.get(at..at + TOC_ROW).ok_or_else(bad)?;
        let kind = SectionKind::from_byte(r[0]);
        let codec = match r[1] {
            0 => Codec::Stored,
            1 => Codec::Zstd,
            c => return Err(Error::fmt(F, format!("codec {c}"))),
        };
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&r[44..76]);
        let plen = le16(r, 76) as usize;
        at += TOC_ROW;
        let path = String::from_utf8(b.get(at..at + plen).ok_or_else(bad)?.to_vec())
            .map_err(|_| Error::fmt(F, "path is not utf-8"))?;
        at += plen;
        let e = Entry {
            kind,
            codec,
            flags: le16(r, 2),
            offset: le64(r, 4),
            len: le64(r, 12),
            raw_len: le64(r, 20),
            src_len: le64(r, 28),
            src_mtime: le64(r, 36),
            hash,
            path,
        };
        if e.offset < HEADER_LEN as u64 || e.offset.checked_add(e.len).is_none_or(|x| x > file_len)
        {
            return Err(Error::fmt(F, format!("{}: outside the file", e.path)));
        }
        if e.raw_len > MAX_SECTION || (e.codec == Codec::Stored && e.raw_len != e.len) {
            return Err(Error::fmt(F, format!("{}: bad size", e.path)));
        }
        if e.kind.is_file() {
            if !safe_rel(&e.path) {
                return Err(Error::fmt(F, format!("unsafe path {:?}", e.path)));
            }
            if !seen.insert(canon(&e.path)) {
                return Err(Error::fmt(F, format!("duplicate path {:?}", e.path)));
            }
        }
        out.push(e);
    }
    Ok(out)
}

fn toc_bytes(entries: &[Entry]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        b.push(e.kind.byte());
        b.push(match e.codec {
            Codec::Stored => 0,
            Codec::Zstd => 1,
        });
        b.extend_from_slice(&e.flags.to_le_bytes());
        for v in [e.offset, e.len, e.raw_len, e.src_len, e.src_mtime] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&e.hash);
        let p = e.path.as_bytes();
        b.extend_from_slice(&(p.len().min(u16::MAX as usize) as u16).to_le_bytes());
        b.extend_from_slice(&p[..p.len().min(u16::MAX as usize)]);
    }
    b
}

/// zstd at `level` when it saves at least 2 %, else stored.
pub fn compress(raw: &[u8], level: i32) -> (Codec, Vec<u8>) {
    if raw.len() >= 64
        && let Ok(z) = zstd::bulk::compress(raw, level)
        && z.len() < raw.len() - raw.len() / 50
    {
        return (Codec::Zstd, z);
    }
    (Codec::Stored, raw.to_vec())
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

/// Writes a content file section by section, then the TOC and header.
pub struct Writer {
    out: std::io::BufWriter<std::fs::File>,
    kind: Kind,
    pos: u64,
    entries: Vec<Entry>,
}

impl Writer {
    pub fn create(path: &Path, kind: Kind) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        out.write_all(&[0u8; HEADER_LEN])?;
        Ok(Self {
            out,
            kind,
            pos: HEADER_LEN as u64,
            entries: Vec::new(),
        })
    }

    /// Append a section whose bytes are already in their stored form
    /// (`e.codec`, `e.raw_len` set by the caller).
    pub fn add_stored(&mut self, mut e: Entry, data: &[u8]) -> Result<()> {
        if e.kind.is_file() {
            let c = canon(&e.path);
            if self
                .entries
                .iter()
                .any(|o| o.kind.is_file() && canon(&o.path) == c)
            {
                return Err(Error::fmt(F, format!("duplicate path {:?}", e.path)));
            }
        }
        e.offset = self.pos;
        e.len = data.len() as u64;
        if e.codec == Codec::Stored {
            e.raw_len = e.len;
        }
        self.out.write_all(data)?;
        self.pos += e.len;
        self.entries.push(e);
        Ok(())
    }

    /// Append `raw`, zstd-compressed when that pays.
    pub fn add(&mut self, mut e: Entry, raw: &[u8], level: i32) -> Result<()> {
        let (codec, data) = compress(raw, level);
        e.codec = codec;
        e.raw_len = raw.len() as u64;
        self.add_stored(e, &data)
    }

    pub fn add_meta(&mut self, meta: &Meta) -> Result<()> {
        let json = serde_json::to_vec(meta).map_err(|e| Error::fmt(F, e.to_string()))?;
        let mut e = Entry::new(SectionKind::Meta, "");
        e.hash = sha256(&json);
        self.add(e, &json, LEVEL)
    }

    pub fn add_logic(&mut self, schema: &str, payload: &[u8]) -> Result<()> {
        let mut raw = Vec::with_capacity(payload.len() + schema.len() + 2);
        raw.extend_from_slice(&(schema.len() as u16).to_le_bytes());
        raw.extend_from_slice(schema.as_bytes());
        raw.extend_from_slice(payload);
        let mut e = Entry::new(SectionKind::Logic, "");
        e.hash = sha256(&raw);
        self.add(e, &raw, 9)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Write the TOC and header. `fingerprint`: see [`Archive::fingerprint`].
    pub fn finish(mut self, fingerprint: [u8; 32]) -> Result<()> {
        let toc = toc_bytes(&self.entries);
        let z = zstd::bulk::compress(&toc, LEVEL).map_err(Error::Io)?;
        let toc_ofs = self.pos;
        self.out.write_all(&z)?;
        let mut h = [0u8; HEADER_LEN];
        h[..8].copy_from_slice(self.kind.magic());
        h[8..10].copy_from_slice(&MAJOR.to_le_bytes());
        h[10..12].copy_from_slice(&MINOR.to_le_bytes());
        h[16..24].copy_from_slice(&toc_ofs.to_le_bytes());
        h[24..28].copy_from_slice(&(z.len() as u32).to_le_bytes());
        h[28..32].copy_from_slice(&(toc.len() as u32).to_le_bytes());
        h[32..64].copy_from_slice(&fingerprint);
        self.out.flush()?;
        let mut f = self
            .out
            .into_inner()
            .map_err(|e| Error::Io(e.into_error()))?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&h)?;
        f.sync_all()?;
        Ok(())
    }
}

/// The kind of a content file from its first bytes, without parsing the rest.
pub fn sniff(path: &Path) -> Option<Kind> {
    let mut b = [0u8; 8];
    std::fs::File::open(path).ok()?.read_exact(&mut b).ok()?;
    match &b {
        m if m == MAGIC_CHAR => Some(Kind::Char),
        m if m == MAGIC_STAGE => Some(Kind::Stage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_roundtrip_and_garbage() {
        let dir = std::env::temp_dir().join(format!("rugen-rgc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("t.rgc");
        let mut w = Writer::create(&p, Kind::Char).unwrap();
        w.add_meta(&Meta {
            def: "t.def".into(),
            name: "T".into(),
            ..Default::default()
        })
        .unwrap();
        let text = b"[Info]\nname = \"T\"\n".repeat(20);
        let mut e = Entry::new(SectionKind::File, "Sub/T.DEF");
        e.hash = sha256(&text);
        w.add(e.clone(), &text, LEVEL).unwrap();
        assert!(
            w.add(Entry::new(SectionKind::File, "sub/t.def"), b"x", 1)
                .is_err()
        );
        w.add_logic("schema-1", b"payload").unwrap();
        w.finish([7; 32]).unwrap();
        let a = Archive::open(&p).unwrap();
        assert_eq!(a.kind, Kind::Char);
        assert_eq!(a.fingerprint, [7; 32]);
        assert_eq!(a.meta().unwrap().name, "T");
        let f = a.find("sub\\t.def").unwrap();
        assert_eq!(f.codec, Codec::Zstd);
        assert_eq!(a.read(f).unwrap(), text);
        assert_eq!(a.logic().unwrap(), ("schema-1".into(), b"payload".to_vec()));
        assert_eq!(sniff(&p), Some(Kind::Char));
        // truncations and garbage are errors, never panics
        let bytes = std::fs::read(&p).unwrap();
        for cut in [0, 10, 63, 64, bytes.len() / 2, bytes.len() - 1] {
            std::fs::write(&p, &bytes[..cut]).unwrap();
            assert!(Archive::open(&p).is_err(), "cut at {cut}");
        }
        let mut bad = bytes.clone();
        bad[8] = 9;
        std::fs::write(&p, &bad).unwrap();
        assert!(Archive::open(&p).is_err(), "other major");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths() {
        assert!(safe_rel("a/b.def"));
        for bad in ["", "../x", "/x", "a/../b", "c:x", "a//b", "a\\b"] {
            assert!(!safe_rel(bad), "{bad}");
        }
        assert_eq!(canon(".\\Sub\\KFM.SFF"), "sub/kfm.sff");
    }
}
