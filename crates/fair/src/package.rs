//! Optimised content packages: exactly the canonical gameplay files of one
//! character or stage (see [`crate::files`]), zstd-compressed.
//!
//! ```text
//! "RGPK" version:u8 kind:u8 fingerprint:[64 ascii]      (clear header)
//! zstd( def_len:u16 def  count:u32  { path_len:u16 path size:u64 bytes }* )
//! ```
//!
//! Only whole files that the engine never loads are left out (readmes,
//! unreferenced sprites / sounds / palettes, music, other .def variants);
//! referenced files are copied byte for byte, so the fingerprint of the
//! unpacked package equals the fingerprint of the source. Unpacking is
//! defensive: safe relative paths only, no duplicates, entry / size caps,
//! and the unpacked set must be exactly the canonical set of its def.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::files::{ContentFiles, Kind, canon, content_files, safe_rel};
use crate::{Error, Result, fingerprint};

pub const MAGIC: &[u8; 4] = b"RGPK";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 4 + 1 + 1 + 64;
/// Entries a package may hold.
pub const MAX_ENTRIES: usize = 512;
/// zstd level: good ratio on SFF / SND without slowing the pick screen.
const LEVEL: i32 = 6;

/// Write the package of `files` (whose fingerprint is `fp`) to `out`.
pub fn write(files: &ContentFiles, fp: &str, out: impl Write) -> Result<()> {
    let mut out = out;
    if fp.len() != 64 {
        return Err(Error::Package("bad fingerprint".into()));
    }
    out.write_all(MAGIC)?;
    out.write_all(&[VERSION, kind_byte(files.kind)])?;
    out.write_all(fp.as_bytes())?;
    let mut z = zstd::stream::write::Encoder::new(out, LEVEL)?;
    put_str(&mut z, &files.def)?;
    z.write_all(&(files.entries.len() as u32).to_le_bytes())?;
    for e in &files.entries {
        let data = std::fs::read(&e.source)?;
        put_str(&mut z, &e.path)?;
        z.write_all(&(data.len() as u64).to_le_bytes())?;
        z.write_all(&data)?;
    }
    z.finish()?.flush()?;
    Ok(())
}

/// Build the package into `path` (written to a temp file, then renamed).
pub fn write_file(files: &ContentFiles, fp: &str, path: &Path) -> Result<u64> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        write(files, fp, f)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(std::fs::metadata(path)?.len())
}

fn kind_byte(k: Kind) -> u8 {
    match k {
        Kind::Char => 0,
        Kind::Stage => 1,
    }
}

fn put_str(w: &mut impl Write, s: &str) -> Result<()> {
    let b = s.as_bytes();
    if b.len() > u16::MAX as usize {
        return Err(Error::Package("path too long".into()));
    }
    w.write_all(&(b.len() as u16).to_le_bytes())?;
    w.write_all(b)?;
    Ok(())
}

fn get_str(r: &mut impl Read) -> Result<String> {
    let mut n = [0u8; 2];
    r.read_exact(&mut n)?;
    let mut b = vec![0u8; u16::from_le_bytes(n) as usize];
    r.read_exact(&mut b)?;
    String::from_utf8(b).map_err(|_| Error::Package("path is not utf-8".into()))
}

/// The clear header of a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    /// Fingerprint claimed by whoever built the package.
    pub fingerprint: String,
}

pub fn read_header(r: &mut impl Read) -> Result<Header> {
    let mut h = [0u8; HEADER_LEN];
    r.read_exact(&mut h)
        .map_err(|_| Error::Package("truncated header".into()))?;
    if &h[..4] != MAGIC {
        return Err(Error::Package("not a rugen package".into()));
    }
    if h[4] != VERSION {
        return Err(Error::Package(format!("package version {}", h[4])));
    }
    let kind = match h[5] {
        0 => Kind::Char,
        1 => Kind::Stage,
        k => return Err(Error::Package(format!("unknown kind {k}"))),
    };
    let fingerprint = String::from_utf8(h[6..].to_vec())
        .ok()
        .filter(|f| rugen_proto::is_fingerprint(f))
        .ok_or_else(|| Error::Package("bad fingerprint in header".into()))?;
    Ok(Header { kind, fingerprint })
}

/// An unpacked, re-verified package.
#[derive(Clone, Debug)]
pub struct Unpacked {
    pub kind: Kind,
    pub dir: PathBuf,
    /// Main def inside `dir`.
    pub def: String,
    /// Fingerprint the package claimed.
    pub claimed: String,
    /// Fingerprint recomputed from the unpacked files.
    pub fingerprint: String,
    pub files: ContentFiles,
    /// Uncompressed bytes.
    pub bytes: u64,
}

impl Unpacked {
    pub fn def_path(&self) -> PathBuf {
        self.dir.join(&self.def)
    }

    /// The recomputed fingerprint is the claimed one.
    pub fn honest(&self) -> bool {
        self.claimed == self.fingerprint
    }
}

/// Unpack a package into the new directory `dest` (must not exist yet),
/// refusing more than `max_bytes` of content, then recompute its canonical
/// file set and fingerprint. `dest` is removed on error.
pub fn unpack(r: impl Read, dest: &Path, max_bytes: u64) -> Result<Unpacked> {
    if dest.exists() {
        return Err(Error::Package(format!("{} already exists", dest.display())));
    }
    std::fs::create_dir_all(dest)?;
    let out = unpack_inner(r, dest, max_bytes);
    if out.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    out
}

fn unpack_inner(mut r: impl Read, dest: &Path, max_bytes: u64) -> Result<Unpacked> {
    let header = read_header(&mut r)?;
    let mut z = zstd::stream::read::Decoder::new(r)?;
    let def = get_str(&mut z)?;
    let mut n = [0u8; 4];
    z.read_exact(&mut n)?;
    let count = u32::from_le_bytes(n) as usize;
    if count == 0 || count > MAX_ENTRIES {
        return Err(Error::Package(format!("{count} entries")));
    }
    let mut seen: Vec<String> = Vec::with_capacity(count);
    let mut total = 0u64;
    for _ in 0..count {
        let path = get_str(&mut z)?;
        if !safe_rel(&path) {
            return Err(Error::Package(format!("unsafe path {path:?}")));
        }
        let c = canon(&path);
        if seen.contains(&c) {
            return Err(Error::Package(format!("duplicate entry {path:?}")));
        }
        seen.push(c);
        let mut sz = [0u8; 8];
        z.read_exact(&mut sz)?;
        let size = u64::from_le_bytes(sz);
        total = total.saturating_add(size);
        if total > max_bytes {
            return Err(Error::Package(format!(
                "content larger than {} MB",
                max_bytes / (1024 * 1024)
            )));
        }
        let target = dest.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&target)?;
        let copied = std::io::copy(&mut (&mut z).take(size), &mut f)?;
        if copied != size {
            return Err(Error::Package(format!("{path}: truncated")));
        }
    }
    // nothing may follow the last entry
    let mut extra = [0u8; 1];
    if z.read(&mut extra)? != 0 {
        return Err(Error::Package("trailing data".into()));
    }
    if !safe_rel(&def) || !seen.contains(&canon(&def)) || def.contains('/') {
        return Err(Error::Package(format!("bad main def {def:?}")));
    }
    let files = content_files(header.kind, &dest.join(&def), None)?;
    let mut want: Vec<&str> = files.entries.iter().map(|e| e.canon.as_str()).collect();
    want.sort_unstable();
    seen.sort_unstable();
    if want != seen.iter().map(String::as_str).collect::<Vec<_>>() {
        let extra: Vec<&String> = seen
            .iter()
            .filter(|s| !want.contains(&s.as_str()))
            .collect();
        return Err(Error::Package(format!(
            "file set does not match its def (unreferenced: {extra:?})"
        )));
    }
    let fingerprint = fingerprint(&files)?;
    Ok(Unpacked {
        kind: header.kind,
        dir: dest.to_path_buf(),
        def,
        claimed: header.fingerprint,
        fingerprint,
        files,
        bytes: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::char_files;
    use crate::test_util::{tiny_char, tmp};

    #[test]
    fn roundtrip_keeps_the_fingerprint_and_drops_junk() {
        let root = tmp("pkg-roundtrip");
        let def = tiny_char(&root.join("src"), "");
        std::fs::write(root.join("src/readme.txt"), "hello").unwrap();
        std::fs::write(root.join("src/unused.sff"), "x").unwrap();
        let files = char_files(&def, None).unwrap();
        let fp = fingerprint(&files).unwrap();
        let pkg = root.join("t.rgpk");
        write_file(&files, &fp, &pkg).unwrap();
        let u = unpack(
            std::fs::File::open(&pkg).unwrap(),
            &root.join("out"),
            1 << 20,
        )
        .unwrap();
        assert!(u.honest());
        assert_eq!(u.fingerprint, fp);
        assert!(!root.join("out/readme.txt").exists());
        assert!(!root.join("out/unused.sff").exists());
        assert!(root.join("out/tiny.cns").exists());
        // a second unpack into the same place is refused
        assert!(
            unpack(
                std::fs::File::open(&pkg).unwrap(),
                &root.join("out"),
                1 << 20
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lying_header_and_bad_packages_are_caught() {
        let root = tmp("pkg-lies");
        let def = tiny_char(&root.join("src"), "");
        let files = char_files(&def, None).unwrap();
        let real = fingerprint(&files).unwrap();
        let fake = "0".repeat(64);
        let mut bytes = Vec::new();
        write(&files, &fake, &mut bytes).unwrap();
        let u = unpack(&bytes[..], &root.join("a"), 1 << 20).unwrap();
        assert!(!u.honest());
        assert_eq!(u.fingerprint, real);
        // size cap
        assert!(unpack(&bytes[..], &root.join("b"), 10).is_err());
        assert!(!root.join("b").exists(), "cleaned up");
        // truncated / garbage
        assert!(unpack(&bytes[..bytes.len() / 2], &root.join("c"), 1 << 20).is_err());
        assert!(unpack(&b"RGPK\x01\x00"[..], &root.join("d"), 1 << 20).is_err());
        // an extra unreferenced file smuggled in
        let mut evil = Vec::new();
        {
            evil.extend_from_slice(MAGIC);
            evil.extend_from_slice(&[VERSION, 0]);
            evil.extend_from_slice(real.as_bytes());
            let mut z = zstd::stream::write::Encoder::new(&mut evil, 1).unwrap();
            put_str(&mut z, &files.def).unwrap();
            z.write_all(&((files.entries.len() + 1) as u32).to_le_bytes())
                .unwrap();
            for e in &files.entries {
                let d = std::fs::read(&e.source).unwrap();
                put_str(&mut z, &e.path).unwrap();
                z.write_all(&(d.len() as u64).to_le_bytes()).unwrap();
                z.write_all(&d).unwrap();
            }
            put_str(&mut z, "../escape.txt").unwrap();
            z.write_all(&1u64.to_le_bytes()).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        assert!(unpack(&evil[..], &root.join("e"), 1 << 20).is_err());
        assert!(!root.join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
