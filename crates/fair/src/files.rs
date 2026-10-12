//! The gameplay-relevant file set of a character or stage.
//!
//! A character is its main `.def` plus what `[Files]` references, resolved
//! the way the engine loads it (relative to the def, case-insensitive,
//! backslashes allowed): `cmd`, `cns`, `st`, `st0`..`st9`, `stcommon`
//! (default `common1.cns`; when absent from the character it comes from the
//! engine's `data/` and is carried in the package so the opponent simulates
//! exactly the same states), `anim`, `sprite`, `sound`, `pal1`..`pal12`.
//! A stage is its `.def` plus `[BGdef] spr`. Music, readmes, unreferenced
//! sprites / sounds / palettes and anything else are not part of the set.
//!
//! Each entry has a package path (relative to the def's folder, original
//! case) and a canonical path (lowercase, `/`) used by the fingerprint.

use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Char,
    Stage,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Char => "char",
            Kind::Stage => "stage",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "char" => Some(Kind::Char),
            "stage" => Some(Kind::Stage),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Path inside the package, `/`-separated, original case.
    pub path: String,
    /// Lowercase [`FileEntry::path`]: what the fingerprint hashes.
    pub canon: String,
    pub source: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ContentFiles {
    pub kind: Kind,
    /// Package path of the main def.
    pub def: String,
    /// Sorted by canonical path, no duplicates; includes the def.
    pub entries: Vec<FileEntry>,
}

impl ContentFiles {
    pub fn total_size(&self) -> u64 {
        self.entries
            .iter()
            .filter_map(|e| std::fs::metadata(&e.source).ok())
            .map(|m| m.len())
            .sum()
    }

    /// The entry for a canonical path.
    pub fn get(&self, canon: &str) -> Option<&FileEntry> {
        self.entries.iter().find(|e| e.canon == canon)
    }
}

/// Lowercase, `/`-separated, no leading `./`.
pub fn canon(path: &str) -> String {
    clean(path).to_ascii_lowercase()
}

fn clean(name: &str) -> String {
    let s = name.trim().trim_matches('"').replace('\\', "/");
    s.split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// A relative path that stays inside its root (no `..`, no absolute parts).
pub fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && !path.contains(['\\', ':', '\0'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}

/// Case-insensitive lookup like the engine's loader.
pub fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = clean(name);
    if name.is_empty() {
        return None;
    }
    let direct = dir.join(&name);
    if direct.is_file() {
        return Some(direct);
    }
    let mut cur = dir.to_path_buf();
    for part in name.split('/') {
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
    cur.is_file().then_some(cur)
}

struct Builder {
    root: PathBuf,
    entries: Vec<FileEntry>,
}

impl Builder {
    fn new(def: &Path) -> Result<(Self, String)> {
        let root = def
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = def
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| Error::Content(format!("{}: not a file", def.display())))?;
        let mut b = Self {
            root,
            entries: Vec::new(),
        };
        b.push(name.clone(), def.to_path_buf())?;
        Ok((b, name))
    }

    fn push(&mut self, path: String, source: PathBuf) -> Result<()> {
        if !safe_rel(&path) {
            return Err(Error::Content(format!(
                "{path}: references a file outside the character folder"
            )));
        }
        let canon = path.to_ascii_lowercase();
        if !self.entries.iter().any(|e| e.canon == canon) {
            self.entries.push(FileEntry {
                path,
                canon,
                source,
            });
        }
        Ok(())
    }

    /// Add a file referenced as `name`; returns false when it does not exist.
    fn add(&mut self, name: &str) -> Result<bool> {
        let Some(found) = resolve(&self.root, name) else {
            return Ok(false);
        };
        let rel = found
            .strip_prefix(&self.root)
            .map_err(|_| Error::Content(format!("{name}: references a file outside its folder")))?
            .to_string_lossy()
            .replace('\\', "/");
        self.push(rel, found)?;
        Ok(true)
    }

    fn finish(mut self, kind: Kind, def: String) -> ContentFiles {
        self.entries.sort_by(|a, b| a.canon.cmp(&b.canon));
        ContentFiles {
            kind,
            def,
            entries: self.entries,
        }
    }
}

/// Gameplay files of a character. `data_dir` holds the engine's
/// `common1.cns` (used when the character has no own copy); `None` means
/// the common file must be inside the character (packages).
pub fn char_files(def_path: &Path, data_dir: Option<&Path>) -> Result<ContentFiles> {
    let def = rugen_formats::def::Def::open(def_path)
        .map_err(|e| Error::Content(format!("{}: {e}", def_path.display())))?;
    let files = def
        .section("files")
        .ok_or_else(|| Error::Content(format!("{}: no [Files]", def_path.display())))?;
    let (mut b, def_name) = Builder::new(def_path)?;
    let mut required = vec!["sprite", "anim", "cmd"];
    if files.raw("cns").is_some_and(|v| !v.trim().is_empty()) {
        required.push("cns");
    } else {
        required.push("st");
    }
    for key in required {
        let name = files.raw(key).unwrap_or("").trim();
        if name.is_empty() || !b.add(name)? {
            return Err(Error::Content(format!(
                "{}: [Files] {key} = {name:?} not found",
                def_path.display()
            )));
        }
    }
    let mut optional = vec!["st".to_string(), "cns".to_string(), "sound".to_string()];
    optional.extend((0..=9).map(|i| format!("st{i}")));
    optional.extend((1..=12).map(|i| format!("pal{i}")));
    for key in optional {
        if let Some(name) = files.raw(&key).map(str::trim).filter(|n| !n.is_empty()) {
            let found = b.add(name)?;
            if !found && key.starts_with("st") {
                return Err(Error::Content(format!(
                    "{}: [Files] {key} = {name:?} not found",
                    def_path.display()
                )));
            }
        }
    }
    let common = files
        .raw("stcommon")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("common1.cns");
    if !b.add(common)? {
        // engine data: carried at the referenced path so the package resolves it first
        let src = data_dir
            .and_then(|d| resolve(d, common))
            .or_else(|| find_common_above(&b.root, common))
            .ok_or_else(|| {
                Error::Content(format!(
                    "{}: stcommon {common:?} not found in the character or data/",
                    def_path.display()
                ))
            })?;
        b.push(clean(common), src)?;
    }
    Ok(b.finish(Kind::Char, def_name))
}

/// `data/<name>` in the ancestors of a character folder (`chars/kfm/../../data`).
fn find_common_above(dir: &Path, name: &str) -> Option<PathBuf> {
    dir.ancestors()
        .skip(1)
        .take(4)
        .find_map(|up| resolve(&up.join("data"), name))
}

/// Gameplay files of a stage: the def and its `[BGdef] spr`.
pub fn stage_files(def_path: &Path) -> Result<ContentFiles> {
    let def = rugen_formats::def::Def::open(def_path)
        .map_err(|e| Error::Content(format!("{}: {e}", def_path.display())))?;
    let (mut b, def_name) = Builder::new(def_path)?;
    if let Some(spr) = def
        .section("bgdef")
        .and_then(|s| s.raw("spr"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && !b.add(spr)?
    {
        return Err(Error::Content(format!(
            "{}: [BGdef] spr = {spr:?} not found",
            def_path.display()
        )));
    }
    Ok(b.finish(Kind::Stage, def_name))
}

/// Files of `kind` rooted at `def`.
pub fn content_files(kind: Kind, def: &Path, data_dir: Option<&Path>) -> Result<ContentFiles> {
    match kind {
        Kind::Char => char_files(def, data_dir),
        Kind::Stage => stage_files(def),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths() {
        assert!(safe_rel("kfm.def"));
        assert!(safe_rel("sub/kfm.sff"));
        for bad in ["", "../x", "/etc/passwd", "a/../b", "c:\\x", "a//b", "./a"] {
            assert!(!safe_rel(bad), "{bad}");
        }
        assert_eq!(canon(".\\Sub\\KFM.SFF"), "sub/kfm.sff");
    }
}
