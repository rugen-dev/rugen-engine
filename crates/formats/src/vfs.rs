//! Paths that reach inside `.rgc` / `.rgs` content files.
//!
//! A content file stands for the folder it was converted from, so
//! `chars/kfm.rgc/kfm.def` names the def inside `chars/kfm.rgc` and
//! `chars/kfm.rgc/kfm.sff` its sprites. Every loader in the engine goes
//! through these functions (`Def::open`, `Sff::open`, `Snd::open`, ...), so
//! content loads the same from a folder or from a content file. Lookups
//! inside a content file are case-insensitive and accept `\` like MUGEN's.
//!
//! Opened content files are remembered (header + table of contents only)
//! and reopened when their size or modification time changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::pack::Stamp;
use crate::rgc::{self, Archive, SectionKind};
use crate::sff::Sff;
use crate::snd::Snd;
use crate::{Error, Result};

type Registry = Mutex<HashMap<PathBuf, (Stamp, Arc<Archive>)>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn is_content_ext(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| rgc::Kind::from_ext(e).is_some())
}

/// `(content file, path inside it)` when `path` reaches into a content
/// file (the inner path is empty for the content file itself).
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    for a in path.ancestors() {
        if is_content_ext(a) && a.is_file() {
            let inner = path
                .strip_prefix(a)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            return Some((a.to_path_buf(), rgc::clean(&inner)));
        }
    }
    None
}

/// The opened content file at `path` (cached).
pub fn archive(path: &Path) -> Result<Arc<Archive>> {
    let stamp = Stamp::of(path).ok_or_else(|| {
        Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} not found", path.display()),
        ))
    })?;
    if let Ok(r) = registry().lock()
        && let Some((s, a)) = r.get(path)
        && *s == stamp
    {
        return Ok(a.clone());
    }
    let a = Arc::new(Archive::open(path)?);
    if let Ok(mut r) = registry().lock() {
        r.insert(path.to_path_buf(), (stamp, a.clone()));
    }
    Ok(a)
}

/// Forget a cached content file (after it was rewritten or deleted).
pub fn forget(path: &Path) {
    if let Ok(mut r) = registry().lock() {
        r.remove(path);
    }
}

fn not_found(path: &Path) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{} not found", path.display()),
    ))
}

/// The section for `path` inside its content file, if it is one.
fn entry(path: &Path) -> Option<Result<(Arc<Archive>, rgc::Entry)>> {
    let (file, inner) = split(path)?;
    Some(archive(&file).and_then(|a| {
        let e = a.find(&inner).cloned().ok_or_else(|| not_found(path))?;
        Ok((a, e))
    }))
}

/// Bytes of a file: from disk, or rebuilt from its content-file section in
/// its original format (sprites as SFF, sounds as SND with WAV sounds,
/// images as PNG).
pub fn read(path: &Path) -> Result<Vec<u8>> {
    match entry(path) {
        None => Ok(std::fs::read(path)?),
        Some(r) => {
            let (a, e) = r?;
            match e.kind {
                SectionKind::File => a.read(&e),
                SectionKind::Sprites => Ok(open_sff_view(path, true, rgc::sprites::View::Exact)
                    .map(|s| rgc::sprites::unpacked(&s).write_exact())?),
                SectionKind::Sounds => Ok(open_snd(path)?.write()),
                SectionKind::Image => webp_to_png(&a.read(&e)?),
                _ => Err(not_found(path)),
            }
        }
    }
}

/// Read a file's bytes into the page cache and drop them: a plain file as
/// it is, a content-file section in its stored form (no rebuild of an SFF
/// or SND like [`read`] does). `false` when there is nothing to read.
pub fn warm(path: &Path) -> bool {
    match entry(path) {
        None => std::fs::read(path).is_ok(),
        Some(r) => r.and_then(|(a, e)| a.read_raw(&e)).is_ok(),
    }
}

/// A WebP image as PNG bytes (RGBA8).
pub fn webp_to_png(webp: &[u8]) -> Result<Vec<u8>> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(webp))
        .map_err(|e| Error::fmt("webp", e.to_string()))?;
    let (w, h) = d.dimensions();
    let has_alpha = d.has_alpha();
    let mut buf = vec![0u8; d.output_buffer_size().unwrap_or(0)];
    d.read_image(&mut buf)
        .map_err(|e| Error::fmt("webp", e.to_string()))?;
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(if has_alpha {
            png::ColorType::Rgba
        } else {
            png::ColorType::Rgb
        });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut wr = enc
            .write_header()
            .map_err(|e| Error::fmt("png", e.to_string()))?;
        wr.write_image_data(&buf)
            .map_err(|e| Error::fmt("png", e.to_string()))?;
    }
    Ok(out)
}

/// The file or directory exists (on disk or inside a content file).
pub fn exists(path: &Path) -> bool {
    match split(path) {
        None => path.exists(),
        Some((file, inner)) => {
            inner.is_empty()
                || archive(&file).is_ok_and(|a| {
                    let want = rgc::canon(&inner);
                    a.files().any(|e| {
                        let c = rgc::canon(&e.path);
                        c == want || c.starts_with(&format!("{want}/"))
                    })
                })
        }
    }
}

/// A file (not a directory) exists.
pub fn is_file(path: &Path) -> bool {
    match split(path) {
        None => path.is_file(),
        Some((file, inner)) => archive(&file).is_ok_and(|a| a.find(&inner).is_some()),
    }
}

/// Names of the entries directly in `dir`, with whether each is a directory.
pub fn read_dir(dir: &Path) -> Vec<(String, bool)> {
    match split(dir) {
        None => std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| {
                        let p = e.path();
                        let is_dir = p.is_dir() && !is_content_ext(&p);
                        (e.file_name().to_string_lossy().to_string(), is_dir)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some((file, inner)) => {
            let Ok(a) = archive(&file) else {
                return Vec::new();
            };
            let prefix = if inner.is_empty() {
                String::new()
            } else {
                format!("{}/", rgc::canon(&inner))
            };
            let mut out: Vec<(String, bool)> = Vec::new();
            for e in a.files() {
                let path = rgc::clean(&e.path);
                if !path.to_ascii_lowercase().starts_with(&prefix) {
                    continue;
                }
                let rest = &path[prefix.len()..];
                let (name, is_dir) = match rest.split_once('/') {
                    Some((d, _)) => (d.to_string(), true),
                    None => (rest.to_string(), false),
                };
                if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                    out.push((name, is_dir));
                }
            }
            out
        }
    }
}

/// Case-insensitive lookup of `name` (may contain `/` or `\`, `..` only on
/// disk) under `dir`, the way MUGEN resolves files.
pub fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim().trim_matches('"').replace('\\', "/");
    if name.is_empty() {
        return None;
    }
    if let Some((file, inner)) = split(dir) {
        let a = archive(&file).ok()?;
        let joined = if inner.is_empty() {
            rgc::clean(&name)
        } else {
            format!("{inner}/{}", rgc::clean(&name))
        };
        // `..` stays inside the content file root at most
        let mut parts: Vec<&str> = Vec::new();
        for p in joined.split('/') {
            if p == ".." {
                parts.pop()?;
            } else {
                parts.push(p);
            }
        }
        let e = a.find(&parts.join("/"))?;
        return Some(file.join(&e.path));
    }
    let direct = dir.join(&name);
    if direct.is_file() {
        return Some(direct);
    }
    let mut cur = dir.to_path_buf();
    for part in name.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            cur = cur.parent()?.to_path_buf();
            continue;
        }
        cur = std::fs::read_dir(&cur)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?
            .path();
    }
    exists(&cur).then_some(cur)
}

/// Size and mtime of a file, or of the content file holding it.
pub fn stamp(path: &Path) -> Option<Stamp> {
    match split(path) {
        None => Stamp::of(path),
        Some((file, _)) => Stamp::of(&file),
    }
}

/// Size of a file (inside a content file: its stored, decoded size).
pub fn len(path: &Path) -> Option<u64> {
    match entry(path) {
        None => std::fs::metadata(path).ok().map(|m| m.len()),
        Some(r) => r.ok().map(|(_, e)| e.raw_len),
    }
}

/// An SFF, from disk or from its sprite section (drawing view: see
/// [`rgc::sprites::View::Runtime`]).
pub fn open_sff(path: &Path, is_char: bool) -> Result<Sff> {
    open_sff_view(path, is_char, rgc::sprites::View::Runtime)
}

/// [`open_sff`] with the sprites exactly as the SFF had them.
pub fn open_sff_exact(path: &Path, is_char: bool) -> Result<Sff> {
    open_sff_view(path, is_char, rgc::sprites::View::Exact)
}

fn open_sff_view(path: &Path, is_char: bool, view: rgc::sprites::View) -> Result<Sff> {
    match entry(path) {
        None => Sff::parse(&std::fs::read(path)?, is_char),
        Some(r) => {
            let (a, e) = r?;
            match e.kind {
                SectionKind::Sprites => rgc::sprites::decode(Arc::new(a.read(&e)?), view),
                SectionKind::File => Sff::parse(&a.read(&e)?, is_char),
                _ => Err(Error::fmt(
                    "sff",
                    format!("{} is not a sprite file", path.display()),
                )),
            }
        }
    }
}

/// An SND, from disk or from its sound section.
pub fn open_snd(path: &Path) -> Result<Snd> {
    match entry(path) {
        None => Snd::open_file(path),
        Some(r) => {
            let (a, e) = r?;
            match e.kind {
                SectionKind::Sounds => rgc::sounds::decode(Arc::new(a.read(&e)?)),
                SectionKind::File => Snd::parse_shared(Arc::new(a.read(&e)?)),
                _ => Err(Error::fmt(
                    "snd",
                    format!("{} is not a sound file", path.display()),
                )),
            }
        }
    }
}

/// `(schema, payload)` of the compiled logic stored with the content file
/// holding `path`, if any.
pub fn logic(path: &Path) -> Option<(String, Vec<u8>)> {
    let (file, _) = split(path)?;
    archive(&file).ok()?.logic()
}

fn cache_slot() -> &'static RwLock<Option<PathBuf>> {
    static C: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(None))
}

/// Where derived data of content files may be written (`cache/`), set by
/// the content library at start-up.
pub fn set_cache_dir(dir: &Path) {
    if let Ok(mut c) = cache_slot().write() {
        *c = Some(dir.to_path_buf());
    }
}

pub fn cache_dir() -> Option<PathBuf> {
    cache_slot().read().ok()?.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgc::{Entry, Kind, Writer};

    #[test]
    fn paths_reach_into_content_files() {
        let dir = std::env::temp_dir().join(format!("rugen-vfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("chars/Tiny.rgc");
        let mut w = Writer::create(&p, Kind::Char).unwrap();
        for (path, body) in [("Tiny.def", "[Info]\nname=Tiny\n"), ("sub/A.cns", "x")] {
            let mut e = Entry::new(SectionKind::File, path);
            e.hash = rgc::sha256(body.as_bytes());
            w.add(e, body.as_bytes(), 3).unwrap();
        }
        w.finish([0; 32]).unwrap();
        assert_eq!(split(&p.join("tiny.def")).unwrap().1, "tiny.def");
        assert!(split(&dir.join("chars")).is_none());
        let def = resolve(&p, "TINY.DEF").unwrap();
        assert_eq!(def, p.join("Tiny.def"));
        assert_eq!(read(&def).unwrap(), b"[Info]\nname=Tiny\n");
        assert_eq!(
            resolve(&p.join("sub"), "..\\Tiny.def").unwrap(),
            p.join("Tiny.def")
        );
        assert!(resolve(&p, "../escape").is_none());
        assert!(exists(&p.join("SUB")) && !is_file(&p.join("sub")));
        assert!(is_file(&p.join("sub/a.cns")));
        let mut names = read_dir(&p);
        names.sort();
        assert_eq!(
            names,
            vec![("Tiny.def".to_string(), false), ("sub".to_string(), true)]
        );
        assert!(read(&p.join("missing.txt")).is_err());
        // a plain file on disk still works
        std::fs::write(dir.join("x.txt"), "y").unwrap();
        assert_eq!(read(&dir.join("x.txt")).unwrap(), b"y");
        assert!(resolve(&dir, "X.TXT").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
