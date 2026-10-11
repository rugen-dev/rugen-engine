//! Content library.
//!
//! Layout next to the executable (or `--root`):
//!
//! ```text
//! chars/   one .zip / .rar / folder per character
//! stages/  one .zip / .rar / folder per stage
//! data/    common1.cns, fightfx.*, common.snd, fight.*
//! cache/   extracted + converted content, keyed by archive hash; safe to delete
//! ```
//!
//! Archives are never read directly by the engine. [`Library::prepare_char`]
//! extracts them once into `cache/chars/<hash>/`, writes a `rugen.json`
//! manifest (main .def, display name) and returns the prepared directory.
//! A `cache/VERSION` file invalidates every entry when the cache format changes.

#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// Bump when the cache layout or conversion output changes.
pub const CACHE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("rar: {0}")]
    Rar(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Rar,
    Dir,
}

/// A character or stage as found in `chars/` / `stages/`.
#[derive(Clone, Debug)]
pub struct Entry {
    /// File stem, used as the id on the command line and in menus.
    pub id: String,
    pub source: PathBuf,
    pub kind: Kind,
}

/// Written into each prepared cache directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub cache_version: u32,
    pub id: String,
    /// Main .def relative to the cache dir.
    pub def: String,
    pub display_name: String,
    pub source_hash: String,
}

#[derive(Clone, Debug)]
pub struct Library {
    pub root: PathBuf,
}

impl Library {
    /// Use `root` if it has a `chars/` directory, else the executable's directory, else cwd.
    pub fn discover(explicit: Option<&Path>) -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(r) = explicit {
            candidates.push(r.to_path_buf());
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.to_path_buf());
            // cargo run: target/debug/rugen → repo root
            for up in dir.ancestors().take(4) {
                candidates.push(up.to_path_buf());
            }
        }
        let root = candidates
            .iter()
            .find(|c| c.join("chars").is_dir())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone());
        Self { root }
    }

    pub fn chars_dir(&self) -> PathBuf {
        self.root.join("chars")
    }
    pub fn stages_dir(&self) -> PathBuf {
        self.root.join("stages")
    }
    /// `data/` next to the content, falling back to the MUGEN 1.1b1 reference tree during development.
    pub fn data_dir(&self) -> PathBuf {
        let d = self.root.join("data");
        if d.join("common1.cns").exists() {
            return d;
        }
        let fallback = self.root.join("reference/mugen-1.1b1/data");
        if fallback.join("common1.cns").exists() {
            return fallback;
        }
        d
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    fn scan(dir: &Path) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let kind = if p.is_dir() {
                Kind::Dir
            } else if ext == "zip" {
                Kind::Zip
            } else if ext == "rar" {
                Kind::Rar
            } else {
                continue;
            };
            out.push(Entry {
                id: stem,
                source: p,
                kind,
            });
        }
        out.sort_by(|a, b| a.id.to_ascii_lowercase().cmp(&b.id.to_ascii_lowercase()));
        out
    }

    pub fn chars(&self) -> Vec<Entry> {
        Self::scan(&self.chars_dir())
    }
    pub fn stages(&self) -> Vec<Entry> {
        Self::scan(&self.stages_dir())
    }

    /// Find a char by id (case-insensitive), or treat `name` as a path to a .def / archive / folder.
    pub fn find_char(&self, name: &str) -> Option<Entry> {
        let p = Path::new(name);
        if p.exists() {
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let kind = if p.is_dir() {
                Kind::Dir
            } else if ext == "zip" {
                Kind::Zip
            } else if ext == "rar" {
                Kind::Rar
            } else if ext == "def" {
                // a .def inside a folder: the folder is the entry
                return Some(Entry {
                    id: p.file_stem()?.to_string_lossy().to_string(),
                    source: p.parent()?.to_path_buf(),
                    kind: Kind::Dir,
                });
            } else {
                return None;
            };
            return Some(Entry {
                id: p.file_stem()?.to_string_lossy().to_string(),
                source: p.to_path_buf(),
                kind,
            });
        }
        self.chars()
            .into_iter()
            .find(|e| e.id.eq_ignore_ascii_case(name))
    }

    /// Cache key: archive bytes hashed (folders: path + newest mtime), plus the cache version.
    fn source_hash(entry: &Entry) -> Result<String> {
        let mut h = blake3::Hasher::new();
        h.update(&CACHE_VERSION.to_le_bytes());
        match entry.kind {
            Kind::Dir => {
                h.update(entry.source.to_string_lossy().as_bytes());
                let mut newest = 0u64;
                for e in walk(&entry.source) {
                    if let Ok(m) = e.metadata()
                        && let Ok(t) = m.modified()
                    {
                        newest = newest.max(
                            t.duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0),
                        );
                    }
                }
                h.update(&newest.to_le_bytes());
            }
            Kind::Zip | Kind::Rar => {
                let mut f = fs::File::open(&entry.source)?;
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    h.update(&buf[..n]);
                }
            }
        }
        Ok(h.finalize().to_hex()[..16].to_string())
    }

    /// Extract (if needed) and return the prepared directory + manifest.
    pub fn prepare_char(&self, entry: &Entry) -> Result<(PathBuf, Manifest)> {
        self.prepare(entry, "chars")
    }
    pub fn prepare_stage(&self, entry: &Entry) -> Result<(PathBuf, Manifest)> {
        self.prepare(entry, "stages")
    }

    fn prepare(&self, entry: &Entry, kind_dir: &str) -> Result<(PathBuf, Manifest)> {
        let hash = Self::source_hash(entry)?;
        let dir = self
            .cache_dir()
            .join(kind_dir)
            .join(format!("{}-{}", sanitize(&entry.id), hash));
        let manifest_path = dir.join("rugen.json");
        if let Ok(text) = fs::read_to_string(&manifest_path)
            && let Ok(m) = serde_json::from_str::<Manifest>(&text)
            && m.cache_version == CACHE_VERSION
            && m.source_hash == hash
            && dir.join(&m.def).exists()
        {
            return Ok((dir, m));
        }
        log::info!("preparing {} from {}", entry.id, entry.source.display());
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        match entry.kind {
            Kind::Zip => extract_zip(&entry.source, &dir)?,
            Kind::Rar => extract_rar(&entry.source, &dir)?,
            Kind::Dir => copy_dir(&entry.source, &dir)?,
        }
        flatten_single_subdir(&dir)?;
        remove_junk(&dir);
        let def = find_main_def(&dir, &entry.id, kind_dir == "stages")
            .ok_or_else(|| Error::Other(format!("{}: no usable .def found", entry.id)))?;
        let display_name = rugen_formats::def::Def::open(dir.join(&def))
            .ok()
            .and_then(|d| {
                d.section("info")
                    .and_then(|i| i.str("displayname").or(i.str("name")).map(str::to_string))
            })
            .unwrap_or_else(|| entry.id.clone());
        let m = Manifest {
            cache_version: CACHE_VERSION,
            id: entry.id.clone(),
            def,
            display_name,
            source_hash: hash,
        };
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&m).map_err(|e| Error::Other(e.to_string()))?,
        )?;
        Ok((dir, m))
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn walk(dir: &Path) -> Vec<fs::DirEntry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                }
                out.push(e);
            }
        }
    }
    out
}

fn extract_zip(src: &Path, dst: &Path) -> Result<()> {
    let f = fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(f)?;
    archive.extract(dst)?;
    Ok(())
}

/// RAR has no pure-Rust decoder; use `unrar`, `7z` or `bsdtar` from PATH.
fn extract_rar(src: &Path, dst: &Path) -> Result<()> {
    use std::process::Command;
    let attempts: [(&str, Vec<String>); 3] = [
        (
            "unrar",
            vec![
                "x".into(),
                "-o+".into(),
                "-inul".into(),
                src.display().to_string(),
                format!("{}/", dst.display()),
            ],
        ),
        (
            "7z",
            vec![
                "x".into(),
                "-y".into(),
                format!("-o{}", dst.display()),
                src.display().to_string(),
            ],
        ),
        (
            "bsdtar",
            vec![
                "-xf".into(),
                src.display().to_string(),
                "-C".into(),
                dst.display().to_string(),
            ],
        ),
    ];
    let mut tried = Vec::new();
    for (tool, args) in attempts {
        match Command::new(tool)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(st) if st.success() => return Ok(()),
            Ok(st) => tried.push(format!("{tool} exited with {st}")),
            Err(_) => tried.push(format!("{tool} not found")),
        }
    }
    Err(Error::Rar(format!(
        "{}: no RAR extractor worked ({}). Install unrar/7z or repack as .zip",
        src.display(),
        tried.join(", ")
    )))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    for e in walk(src) {
        let p = e.path();
        let rel = p.strip_prefix(src).unwrap_or(&p);
        let out = dst.join(rel);
        if p.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&p, &out)?;
        }
    }
    Ok(())
}

/// `archive.zip` containing a single top-level folder: hoist its contents.
fn flatten_single_subdir(dir: &Path) -> Result<()> {
    loop {
        let entries: Vec<PathBuf> = fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
        if entries.len() == 1 && entries[0].is_dir() {
            let sub = entries[0].clone();
            let tmp = dir.join(".hoist");
            fs::rename(&sub, &tmp)?;
            for e in fs::read_dir(&tmp)?.flatten() {
                fs::rename(e.path(), dir.join(e.file_name()))?;
            }
            fs::remove_dir(&tmp)?;
        } else {
            return Ok(());
        }
    }
}

fn remove_junk(dir: &Path) {
    for e in walk(dir) {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if name == "thumbs.db" || name == ".ds_store" || name == "desktop.ini" {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Main .def: `<id>.def`, else the first .def with a `[Files] sprite=` (char) or `[StageInfo]`/`[BGdef]` (stage).
fn find_main_def(dir: &Path, id: &str, stage: bool) -> Option<String> {
    let mut defs: Vec<PathBuf> = walk(dir)
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("def")))
        .collect();
    defs.sort();
    let rel = |p: &Path| {
        p.strip_prefix(dir)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    if let Some(p) = defs.iter().find(|p| {
        p.file_stem()
            .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(id))
    }) {
        return Some(rel(p));
    }
    for p in &defs {
        let Ok(d) = rugen_formats::def::Def::open(p) else {
            continue;
        };
        let ok = if stage {
            d.section("stageinfo").is_some() || d.section("bgdef").is_some()
        } else {
            d.section("files").and_then(|f| f.raw("sprite")).is_some()
        };
        if ok {
            return Some(rel(p));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("rugen-content-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("chars")).unwrap();
        // build a tiny zip with a nested folder
        let zpath = tmp.join("chars/Tiny.zip");
        {
            let f = fs::File::create(&zpath).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("Tiny/tiny.def", opts).unwrap();
            use std::io::Write;
            z.write_all(b"[Info]\nname = \"Tiny\"\ndisplayname = \"Tiny Guy\"\n[Files]\nsprite = tiny.sff\n").unwrap();
            z.start_file("Tiny/Thumbs.db", opts).unwrap();
            z.write_all(b"junk").unwrap();
            z.finish().unwrap();
        }
        let lib = Library::discover(Some(&tmp));
        assert_eq!(lib.root, tmp);
        let chars = lib.chars();
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].id, "Tiny");
        let (dir, m) = lib.prepare_char(&chars[0]).unwrap();
        assert_eq!(m.def, "tiny.def", "hoisted out of the single folder");
        assert_eq!(m.display_name, "Tiny Guy");
        assert!(!dir.join("Thumbs.db").exists());
        // second call hits the cache
        let (dir2, _) = lib.prepare_char(&chars[0]).unwrap();
        assert_eq!(dir, dir2);
        assert!(lib.find_char("tiny").is_some());
        let _ = fs::remove_dir_all(&tmp);
    }
}
