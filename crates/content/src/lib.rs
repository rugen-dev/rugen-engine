//! Content library.
//!
//! Layout next to the executable (or `--root`):
//!
//! ```text
//! chars/              one .rgc per character (also .zip / .rar / folders to import)
//! chars/originals/    imported archives, kept when `[content] keep_originals`
//! stages/             one .rgs per stage (same rules)
//! data/               common1.cns, fightfx.*, common.snd, fight.*
//! cache/              derived data, safe to delete:
//!   chars/<id>-<key>/   manifest (rugen.json), portraits, card
//!   rgc/chars/          conversions of folders (creator working copies)
//!   logic/              logic compiled by this engine for older .rgc files
//!   net/                fighters / stages downloaded for online matches
//! ```
//!
//! The engine plays characters and stages from rugen's own content files
//! (`.rgc` / `.rgs`, see `docs/rgc-format.md` and [`convert`]): one file
//! each, sprites pixel-exact, sounds as Opus, loaded in place
//! (`chars/x.rgc/x.def` through `rugen_formats::vfs`). Anything new in
//! `chars/` / `stages/` is converted on launch:
//!
//! - an archive (`.zip`, `.rar`) becomes `chars/<id>.rgc`; once the new file
//!   is written and read back with the same fingerprint, the archive moves
//!   to `chars/originals/` (ignored by the scanner), or is deleted when
//!   `keep_originals` is off;
//! - a folder is a creator's working copy: it is never moved or deleted,
//!   only converted into `cache/rgc/`, again whenever its files change
//!   (unchanged sprites / sounds are reused, so a `.cns` edit converts in a
//!   moment). A folder wins over a content file with the same name;
//! - an `.rgc` / `.rgs` dropped in is used as it is (its fingerprint is
//!   recomputed from its content the first time).
//!
//! Older caches (extracted copies with `.rugen-opt/` artefacts, manifest
//! v6 and before) are removed once their content has a valid content file.

#![forbid(unsafe_code)]

pub mod convert;
pub mod export;
mod extract;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rugen_formats::rgc;
use rugen_formats::vfs;
use serde::{Deserialize, Serialize};

/// Bump when the cache layout changes. v7: content files (`.rgc` /
/// `.rgs`) replace the extracted copies and the `.rugen-opt/` artefacts;
/// manifests only describe them.
pub const CACHE_VERSION: u32 = 7;

/// Oldest engine cache version that can use an entry written now as is.
const MIN_READER: u32 = 7;

/// Mixed into the v4..v6 cache-directory hash (to find old entries).
const LEGACY_HASH_EPOCH: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("rar: {0}")]
    Rar(String),
    #[error("{0}")]
    Format(#[from] rugen_formats::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Rar,
    Dir,
    /// An `.rgc` / `.rgs` content file.
    Content,
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
    /// Main .def relative to the content file (see [`Manifest::def_path`]).
    pub def: String,
    pub display_name: String,
    #[serde(default)]
    pub author: String,
    /// Select-screen portraits (`9000,0` small, `9000,1` big) as RGBA PNGs in the cache dir.
    #[serde(default)]
    pub portrait_small: Option<PortraitInfo>,
    #[serde(default)]
    pub portrait_big: Option<PortraitInfo>,
    /// Identity of the content file this entry describes.
    pub source_hash: String,
    /// Result of `rugen verify` (headless soak); `None` until it has run.
    #[serde(default)]
    pub verify: Option<VerifyInfo>,
    /// Modder-supplied large render (`rugen_art.webp` / `.png` in the character), copied
    /// into the cache dir; the roster shows it instead of the 9000,1 portrait.
    #[serde(default)]
    pub art: Option<String>,
    /// Stage preview (`thumb.png` in the cache dir, from the first background sprite).
    #[serde(default)]
    pub thumb: Option<PortraitInfo>,
    /// Content fingerprint (`rugen-fair`): same for the same gameplay
    /// content whatever the container, folder name, junk files or sounds.
    #[serde(default)]
    pub fingerprint: Option<String>,
    /// Main .def relative to the package root (the def's folder).
    #[serde(default)]
    pub package_def: Option<String>,
    /// Size of the content file in bytes.
    #[serde(default)]
    pub content_size: u64,
    /// Static fairness analysis (characters), cached so online picks are instant.
    #[serde(default)]
    pub profile: Option<rugen_proto::FairnessProfile>,
    /// Small WebP card portrait (`card.webp`) sent with online picks.
    #[serde(default)]
    pub card: Option<String>,
    /// Problem computing the fingerprint (the entry cannot go online).
    #[serde(default)]
    pub fair_error: Option<String>,
    /// A stage (else a character).
    #[serde(default)]
    pub stage: bool,
    /// Oldest cache version able to read this entry unchanged (0: only the
    /// version that wrote it).
    #[serde(default)]
    pub min_reader: u32,
    /// The `.rgc` / `.rgs` this entry describes.
    #[serde(default)]
    pub content: Option<PathBuf>,
}

impl Manifest {
    /// Usable as is by this engine: written by it, or by a newer one that
    /// declares this version a compatible reader.
    fn readable(&self) -> bool {
        self.content.is_some()
            && (self.cache_version == CACHE_VERSION
                || (self.cache_version > CACHE_VERSION
                    && self.min_reader > 0
                    && self.min_reader <= CACHE_VERSION))
    }

    /// The main def as the engine loads it: inside the content file
    /// (`chars/x.rgc/x.def`), else relative to the cache directory `dir`.
    pub fn def_path(&self, dir: &Path) -> PathBuf {
        match &self.content {
            Some(c) => c.join(&self.def),
            None => dir.join(&self.def),
        }
    }

    /// The fairness profile with this entry's verify verdict filled in.
    pub fn fair_profile(&self) -> Option<rugen_proto::FairnessProfile> {
        let mut p = self.profile.clone()?;
        p.verify = self.verify.as_ref().map(|v| v.verdict.clone());
        Some(p)
    }
}

/// Verification verdict stored in the cache manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifyInfo {
    /// `verified`, `warn` or `broken`.
    pub verdict: String,
    pub issues: Vec<String>,
    /// Engine version that produced the verdict; a different engine re-verifies.
    pub engine_version: String,
    /// Unix seconds.
    pub when: u64,
}

impl VerifyInfo {
    pub fn is_broken(&self) -> bool {
        self.verdict == "broken"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortraitInfo {
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// Sprite axis (origin) relative to the top-left corner.
    pub axis: [i32; 2],
}

/// One import done by [`Library::import_new`].
#[derive(Debug)]
pub struct Imported {
    pub id: String,
    pub stage: bool,
    pub result: Result<PathBuf>,
}

/// Conversions run one at a time (the launch import and a menu may both ask).
static CONVERTING: Mutex<()> = Mutex::new(());
/// Manifests are written one at a time too.
static DESCRIBING: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
pub struct Library {
    /// Player files: `chars/`, `stages/`, `cache/`, `rugen.toml`, replays.
    pub root: PathBuf,
    /// The directory holding `motif/`: `root` for a portable folder,
    /// `/usr/share/rugen` (next to the binary's `../share`) for a system package.
    pub assets: PathBuf,
    /// Move imported archives to `chars/originals/` (else delete them once
    /// their content file is verified). `[content] keep_originals`.
    pub keep_originals: bool,
}

impl Library {
    /// Use `root` if it has a `chars/` directory, else the executable's directory, else cwd.
    pub fn discover(explicit: Option<&Path>) -> Self {
        let lib = Self::find(explicit);
        vfs::set_cache_dir(&lib.cache_dir());
        lib
    }

    fn find(explicit: Option<&Path>) -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(r) = explicit {
            // absolute: paths derived from the root (cache files the UI
            // loads) must not depend on where they are joined
            candidates.push(std::path::absolute(r).unwrap_or_else(|_| r.to_path_buf()));
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
        if let Some(root) = candidates.iter().find(|c| c.join("chars").is_dir()) {
            return Self::portable(root.clone());
        }
        // system package (rpm / deb): /usr/bin/rugen with the read-only
        // assets in /usr/share/rugen, player files in the user data dir
        let share = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.parent()?.join("share/rugen")))
            .filter(|s| s.join("motif").is_dir());
        if let (Some(assets), Some(root)) = (share, user_data_dir()) {
            for d in ["chars", "stages"] {
                let _ = fs::create_dir_all(root.join(d));
            }
            return Self {
                root,
                assets,
                keep_originals: true,
            };
        }
        Self::portable(candidates[0].clone())
    }

    fn portable(root: PathBuf) -> Self {
        Self {
            assets: root.clone(),
            root,
            keep_originals: true,
        }
    }

    pub fn chars_dir(&self) -> PathBuf {
        self.root.join("chars")
    }
    pub fn stages_dir(&self) -> PathBuf {
        self.root.join("stages")
    }
    /// `data/` next to the content (a player's own MUGEN data), else the
    /// one the release ships with the assets (`/usr/share/rugen/data` for a
    /// system package), falling back to the MUGEN 1.1b1 reference tree
    /// during development.
    pub fn data_dir(&self) -> PathBuf {
        let d = self.root.join("data");
        if d.join("common1.cns").exists() {
            return d;
        }
        let shipped = self.assets.join("data");
        if shipped.join("common1.cns").exists() {
            return shipped;
        }
        let fallback = self.root.join("reference/mugen-1.1b1/data");
        if fallback.join("common1.cns").exists() {
            return fallback;
        }
        d
    }
    /// Screenpack (motif) directory: `<root>/motif` when it holds a `system.def`,
    /// else `<root>/data`, else the data dir. Files missing from the motif fall
    /// back to [`Library::data_dir`].
    pub fn motif_dir(&self) -> PathBuf {
        for d in [self.assets.join("motif"), self.root.join("data")] {
            if d.join("system.def").exists() {
                return d;
            }
        }
        self.data_dir()
    }

    /// Motif directory for a UI theme: `<root>/motif/themes/<theme>` when it
    /// exists (files missing there fall back to the flat `motif/`, then
    /// `data/`), else [`Library::motif_dir`].
    pub fn motif_dir_for(&self, theme: &str) -> PathBuf {
        let theme = theme.trim();
        if !theme.is_empty() {
            let d = self.assets.join("motif").join("themes").join(theme);
            if d.join("system.def").exists() || d.join("ui").is_dir() {
                return d;
            }
        }
        self.motif_dir()
    }

    /// Installed UI themes: folders under `motif/themes/` (sorted) plus
    /// `classic` for the flat `motif/` when it has its own screens.
    pub fn themes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(self.assets.join("motif").join("themes")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && (p.join("system.def").exists() || p.join("ui").is_dir()) {
                    out.push(e.file_name().to_string_lossy().to_string());
                }
            }
        }
        out.sort();
        if self.assets.join("motif").join("ui").is_dir() {
            out.push("classic".into());
        }
        out
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Entries of `dir`: content files with extension `ext`, archives and
    /// folders. `originals/` and hidden names are skipped. When several
    /// share an id, a folder wins (a creator's working copy), then the
    /// content file, then archives.
    fn scan(dir: &Path, ext: &str) -> Vec<Entry> {
        let mut out: Vec<Entry> = Vec::new();
        let Ok(rd) = fs::read_dir(dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.eq_ignore_ascii_case("originals") {
                continue;
            }
            let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let x = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let kind = if p.is_dir() {
                Kind::Dir
            } else if x == ext {
                Kind::Content
            } else if x == "zip" {
                Kind::Zip
            } else if x == "rar" {
                Kind::Rar
            } else {
                continue;
            };
            let id = if kind == Kind::Dir { name } else { stem };
            let rank = |k: Kind| match k {
                Kind::Dir => 0,
                Kind::Content => 1,
                Kind::Zip => 2,
                Kind::Rar => 3,
            };
            match out.iter_mut().find(|o| o.id.eq_ignore_ascii_case(&id)) {
                Some(o) if rank(kind) < rank(o.kind) => {
                    *o = Entry {
                        id,
                        source: p,
                        kind,
                    }
                }
                Some(_) => {}
                None => out.push(Entry {
                    id,
                    source: p,
                    kind,
                }),
            }
        }
        out.sort_by_key(|a| a.id.to_ascii_lowercase());
        out
    }

    pub fn chars(&self) -> Vec<Entry> {
        Self::scan(&self.chars_dir(), "rgc")
    }
    /// Stages in `stages/` plus the built-ins in `motif/stages/` (a user
    /// stage with the same id wins).
    pub fn stages(&self) -> Vec<Entry> {
        let mut out = Self::scan(&self.stages_dir(), "rgs");
        for e in Self::scan(&self.assets.join("motif").join("stages"), "rgs") {
            if !out.iter().any(|o| o.id.eq_ignore_ascii_case(&e.id)) {
                out.push(e);
            }
        }
        out.sort_by_key(|a| a.id.to_ascii_lowercase());
        out
    }

    /// Stage used when nothing else is picked: the built-in `training`, else
    /// the first stage found, else MUGEN's stage0 from `reference/`.
    pub fn default_stage(&self) -> Option<PathBuf> {
        let stages = self.stages();
        stages
            .iter()
            .find(|e| e.id.eq_ignore_ascii_case("training"))
            .or_else(|| stages.first())
            .and_then(|e| self.prepare_stage(e).ok().map(|(d, m)| m.def_path(&d)))
            .or_else(|| {
                let p = self.root.join("reference/mugen-1.1b1/stages/stage0.def");
                p.exists().then_some(p)
            })
    }

    /// Find a char by id (case-insensitive), or treat `name` as a path to a
    /// .def / archive / folder / `.rgc`.
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
            } else if ext == "rgc" || ext == "rgs" {
                Kind::Content
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

    /// Convert the content file of a character, then describe it.
    pub fn prepare_char(&self, entry: &Entry) -> Result<(PathBuf, Manifest)> {
        self.prepare(entry, false)
    }

    pub fn prepare_stage(&self, entry: &Entry) -> Result<(PathBuf, Manifest)> {
        self.prepare(entry, true)
    }

    /// Rewrite a prepared entry's manifest (after verification).
    pub fn save_manifest(dir: &Path, m: &Manifest) -> Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join("rugen.json.tmp");
        fs::write(
            &tmp,
            serde_json::to_string_pretty(m).map_err(|e| Error::Other(e.to_string()))?,
        )?;
        fs::rename(tmp, dir.join("rugen.json"))?;
        Ok(())
    }

    /// Convert everything new in `chars/` and `stages/` (archives, changed
    /// folders) and tidy older caches. Called on launch; menus only find
    /// content files afterwards.
    pub fn import_new(&self) -> Vec<Imported> {
        let mut out = Vec::new();
        for (stage, entries) in [(false, self.chars()), (true, self.stages())] {
            // archives shadowed by a content file of the same id are imported too
            let dir = if stage {
                self.stages_dir()
            } else {
                self.chars_dir()
            };
            let mut todo: Vec<Entry> = entries
                .into_iter()
                .filter(|e| e.kind != Kind::Content)
                .collect();
            for e in Self::archives(&dir) {
                if !todo.iter().any(|t| t.source == e.source) {
                    todo.push(e);
                }
            }
            for e in todo {
                // prepared right away: the menus find the manifest ready
                let result = self
                    .prepare(&e, stage)
                    .map(|(d, m)| m.def_path(&d))
                    .inspect_err(|err| log::warn!("{}: not imported: {err}", e.id));
                out.push(Imported {
                    id: e.id,
                    stage,
                    result,
                });
            }
        }
        self.sweep_legacy();
        out
    }

    /// Every archive in `dir` (not only the ones winning their id).
    fn archives(dir: &Path) -> Vec<Entry> {
        let Ok(rd) = fs::read_dir(dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter_map(|e| {
                let p = e.path();
                let x = p.extension()?.to_string_lossy().to_ascii_lowercase();
                let kind = match x.as_str() {
                    "zip" => Kind::Zip,
                    "rar" => Kind::Rar,
                    _ => return None,
                };
                let id = p.file_stem()?.to_string_lossy().to_string();
                (p.is_file() && !e.file_name().to_string_lossy().starts_with('.')).then_some(
                    Entry {
                        id,
                        source: p,
                        kind,
                    },
                )
            })
            .collect()
    }

    /// The content file of `entry`, converting it when needed: `(path,
    /// fingerprint verified by a conversion just now, verdict carried over
    /// from an older cache)`.
    fn content_file(
        &self,
        entry: &Entry,
        stage: bool,
    ) -> Result<(PathBuf, Option<String>, Option<VerifyInfo>)> {
        match entry.kind {
            Kind::Content => Ok((entry.source.clone(), None, None)),
            Kind::Zip | Kind::Rar => self.convert_archive(entry, stage),
            Kind::Dir => self.working_copy(entry, stage).map(|(p, fp)| (p, fp, None)),
        }
    }

    fn kind_dir(stage: bool) -> &'static str {
        if stage { "stages" } else { "chars" }
    }

    fn rgc_kind(stage: bool) -> rgc::Kind {
        if stage {
            rgc::Kind::Stage
        } else {
            rgc::Kind::Char
        }
    }

    /// Import an archive into `<id>.rgc` next to it, then move it to
    /// `originals/` (or delete it).
    fn convert_archive(
        &self,
        entry: &Entry,
        stage: bool,
    ) -> Result<(PathBuf, Option<String>, Option<VerifyInfo>)> {
        let _guard = CONVERTING.lock().unwrap_or_else(|e| e.into_inner());
        let kind = Self::rgc_kind(stage);
        let dir = entry
            .source
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        let src_name = entry
            .source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // converted already (by another thread, or a run interrupted before the archive moved)
        if let Some(done) = converted_from(&dir, &entry.id, kind, &src_name) {
            if entry.source.exists() {
                self.dispose_original(&entry.source)?;
            }
            return Ok((done, None, None));
        }
        if !entry.source.exists() {
            return Err(Error::Other(format!(
                "{} vanished before it was imported",
                entry.source.display()
            )));
        }
        let out = free_name(&dir, &entry.id, kind.ext());
        let legacy = self.legacy_dir(entry, stage);
        let carried = legacy
            .as_ref()
            .and_then(|d| read_any_manifest(d))
            .and_then(|m| m.verify);
        let tmp = self.cache_dir().join("tmp").join(format!(
            "{}-{}",
            sanitize(&entry.id),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp)?;
        log::info!("importing {} ...", entry.source.display());
        let extracted = match entry.kind {
            Kind::Zip => extract::extract_zip(&entry.source, &tmp),
            _ => extract::extract_rar(&entry.source, &tmp),
        };
        let root = match (extracted, &legacy) {
            (Ok(()), _) => extract::content_root(&tmp),
            // no extractor (RAR without unrar): an older extracted cache still has the files
            (Err(e), Some(old)) => {
                log::warn!("{e}; converting the older cache copy instead");
                old.clone()
            }
            (Err(e), None) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(e);
            }
        };
        let result = (|| {
            let def = extract::find_main_def(&root, &entry.id, stage)
                .ok_or_else(|| Error::Other(format!("{}: no usable .def found", entry.id)))?;
            let src = convert::Source {
                root: root.clone(),
                def,
                kind,
                name: src_name.clone(),
            };
            convert::convert(&src, &out, &self.data_dir(), None)
        })();
        let _ = fs::remove_dir_all(&tmp);
        let report = result?;
        log::info!("{} → {}: {}", src_name, out.display(), report.summary());
        self.dispose_original(&entry.source)?;
        if let Some(old) = legacy {
            let _ = fs::remove_dir_all(old);
        }
        Ok((out, report.fingerprint, carried))
    }

    /// Move an imported archive to `originals/`, or delete it.
    fn dispose_original(&self, src: &Path) -> Result<()> {
        if self.keep_originals {
            let dir = src.parent().unwrap_or(Path::new(".")).join("originals");
            fs::create_dir_all(&dir)?;
            let name = src.file_name().unwrap_or_default().to_string_lossy();
            let mut target = dir.join(name.as_ref());
            let mut n = 2;
            while target.exists() {
                let stem = src.file_stem().unwrap_or_default().to_string_lossy();
                let ext = src.extension().unwrap_or_default().to_string_lossy();
                target = dir.join(format!("{stem}-{n}.{ext}"));
                n += 1;
            }
            fs::rename(src, &target)?;
            log::info!("kept the original as {}", target.display());
        } else {
            fs::remove_file(src)?;
            log::info!("removed {} (keep_originals = false)", src.display());
        }
        Ok(())
    }

    /// `cache/rgc/<chars|stages>/<id>-<hash of the folder path>.<ext>`: the
    /// conversion of a folder, redone (incrementally) when its files change.
    fn working_copy(&self, entry: &Entry, stage: bool) -> Result<(PathBuf, Option<String>)> {
        let kind = Self::rgc_kind(stage);
        let abs = fs::canonicalize(&entry.source).unwrap_or_else(|_| entry.source.clone());
        let key = blake3::hash(abs.to_string_lossy().as_bytes()).to_hex()[..10].to_string();
        let out = self
            .cache_dir()
            .join("rgc")
            .join(Self::kind_dir(stage))
            .join(format!("{}-{key}.{}", sanitize(&entry.id), kind.ext()));
        let root = extract::content_root(&entry.source);
        let side = WorkingCopy::path(&out);
        let fresh = |sig: &str| {
            out.is_file() && WorkingCopy::read(&out).is_some_and(|w| w.sig == sig && w.dir == root)
        };
        let sig = folder_signature(&root);
        if fresh(&sig) {
            return Ok((out, None));
        }
        let _guard = CONVERTING.lock().unwrap_or_else(|e| e.into_inner());
        let sig = folder_signature(&root);
        if fresh(&sig) {
            return Ok((out, None));
        }
        let def = extract::find_main_def(&root, &entry.id, stage)
            .ok_or_else(|| Error::Other(format!("{}: no usable .def found", entry.id)))?;
        if let Some(p) = out.parent() {
            fs::create_dir_all(p)?;
        }
        let src = convert::Source {
            root: root.clone(),
            def: def.clone(),
            kind,
            name: entry
                .source
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        };
        let prev = out.is_file().then(|| out.clone());
        let report = convert::convert(&src, &out, &self.data_dir(), prev.as_deref())?;
        log::info!(
            "{} (folder) → {}: {}",
            entry.id,
            out.display(),
            report.summary()
        );
        let w = WorkingCopy {
            dir: root,
            sig,
            stage,
        };
        fs::write(
            side,
            serde_json::to_string_pretty(&w).map_err(|e| Error::Other(e.to_string()))?,
        )?;
        Ok((out, report.fingerprint))
    }

    /// Where a v4..v6 engine extracted an archive: `cache/<kind>/<id>-<hash>`.
    fn legacy_dir_name(&self, entry: &Entry, stage: bool) -> PathBuf {
        let mut h = blake3::Hasher::new();
        h.update(&LEGACY_HASH_EPOCH.to_le_bytes());
        if let Ok(mut f) = fs::File::open(&entry.source) {
            let mut buf = vec![0u8; 1 << 20];
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
        }
        self.cache_dir().join(Self::kind_dir(stage)).join(format!(
            "{}-{}",
            sanitize(&entry.id),
            &h.finalize().to_hex()[..16]
        ))
    }

    /// The v4..v6 cache directory of an archive (extracted copy), if any.
    fn legacy_dir(&self, entry: &Entry, stage: bool) -> Option<PathBuf> {
        let dir = self.legacy_dir_name(entry, stage);
        let m = read_any_manifest(&dir)?;
        (m.cache_version < CACHE_VERSION && dir.join(&m.def).is_file()).then_some(dir)
    }

    /// Remove what older engines cached: extracted copies (manifest v6 and
    /// before, with their `.rugen-opt/`), RGPK upload packages and unpacked
    /// downloads.
    fn sweep_legacy(&self) {
        for kind in ["chars", "stages"] {
            let Ok(rd) = fs::read_dir(self.cache_dir().join(kind)) else {
                continue;
            };
            for e in rd.flatten() {
                if let Some(m) = read_any_manifest(&e.path())
                    && m.cache_version < CACHE_VERSION
                {
                    log::info!("removing old cache entry {}", e.path().display());
                    let _ = fs::remove_dir_all(e.path());
                }
            }
            if let Ok(rd) = fs::read_dir(self.net_dir(kind == "stages")) {
                for e in rd.flatten() {
                    if read_any_manifest(&e.path()).is_some_and(|m| m.cache_version < CACHE_VERSION)
                    {
                        let _ = fs::remove_dir_all(e.path());
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(self.cache_dir().join("packages"));
        let _ = fs::remove_dir_all(self.cache_dir().join("tmp"));
    }

    fn prepare(&self, entry: &Entry, stage: bool) -> Result<(PathBuf, Manifest)> {
        let (content, verified, carried) = self.content_file(entry, stage)?;
        let key = content_key(&content)?;
        let kind_dir = Self::kind_dir(stage);
        let dir = self
            .cache_dir()
            .join(kind_dir)
            .join(format!("{}-{key}", sanitize(&entry.id)));
        let _guard = DESCRIBING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut m) = read_any_manifest(&dir)
            && m.readable()
            && m.source_hash == key
        {
            if m.content.as_deref() != Some(content.as_path()) {
                m.content = Some(content.clone());
                Self::save_manifest(&dir, &m)?;
            }
            remember(&content, &dir);
            return Ok((dir, m));
        }
        let mut m = self.describe(&content, &dir, &entry.id, stage, verified)?;
        m.source_hash = key;
        m.verify = carried;
        Self::save_manifest(&dir, &m)?;
        // older manifests of the same id describing this file (or nothing valid)
        if let Ok(rd) = fs::read_dir(self.cache_dir().join(kind_dir)) {
            let prefix = format!("{}-", sanitize(&entry.id));
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path() == dir || !name.starts_with(&prefix) {
                    continue;
                }
                if read_any_manifest(&e.path()).is_none_or(|o| {
                    o.cache_version < CACHE_VERSION
                        || o.content.as_deref() == Some(content.as_path())
                }) {
                    let _ = fs::remove_dir_all(e.path());
                }
            }
        }
        remember(&content, &dir);
        Ok((dir, m))
    }

    /// Build the manifest of a content file (portraits, fingerprint, profile, card).
    fn describe(
        &self,
        content: &Path,
        dir: &Path,
        id: &str,
        stage: bool,
        verified: Option<String>,
    ) -> Result<Manifest> {
        let (archive, def_path) = convert::def_in(content)?;
        let meta = archive.meta().map_err(|e| Error::Other(e.to_string()))?;
        fs::create_dir_all(dir)?;
        let parsed = rugen_formats::def::Def::open(&def_path).ok();
        let info = |key: &str| {
            parsed
                .as_ref()
                .and_then(|d| d.section("info"))
                .and_then(|i| i.str(key).map(str::to_string))
        };
        let display_name = info("displayname")
            .or_else(|| info("name"))
            .unwrap_or_else(|| id.to_string());
        let author = info("author").unwrap_or_default();
        let fair_kind = if stage {
            rugen_fair::Kind::Stage
        } else {
            rugen_fair::Kind::Char
        };
        let mut m = Manifest {
            cache_version: CACHE_VERSION,
            id: id.to_string(),
            def: meta.def.clone(),
            display_name,
            author,
            portrait_small: None,
            portrait_big: None,
            source_hash: String::new(),
            verify: None,
            art: None,
            thumb: None,
            fingerprint: None,
            package_def: None,
            content_size: fs::metadata(content).map_or(0, |x| x.len()),
            profile: None,
            card: None,
            fair_error: None,
            stage,
            min_reader: MIN_READER,
            content: Some(content.to_path_buf()),
        };
        std::thread::scope(|s| {
            // fingerprint + profile beside the portraits
            let fair = s.spawn(|| -> std::result::Result<_, String> {
                let files = rugen_fair::files::content_files(fair_kind, &def_path, None)
                    .map_err(|e| e.to_string())?;
                let fp = match &verified {
                    Some(fp) => fp.clone(),
                    None => rugen_fair::fingerprint(&files).map_err(|e| e.to_string())?,
                };
                let profile = (!stage)
                    .then(|| rugen_fair::profile::analyze(&files).ok())
                    .flatten();
                Ok((fp, files.def.clone(), profile))
            });
            if let Some(d) = parsed.as_ref() {
                let sff = open_sff(&def_path, d, stage);
                match (&sff, stage) {
                    (Some(sff), false) => {
                        let (a, b) = portraits_of(dir, &def_path, d, sff);
                        m.portrait_small = a;
                        m.portrait_big = b;
                    }
                    (Some(sff), true) => m.thumb = extract_stage_thumb(dir, d, sff),
                    _ => {}
                }
            }
            match fair.join() {
                Ok(Ok((fp, def, profile))) => {
                    m.fingerprint = Some(fp);
                    m.package_def = Some(def);
                    m.profile = profile;
                }
                Ok(Err(e)) => {
                    log::warn!("{id}: not shareable online: {e}");
                    m.fair_error = Some(e);
                }
                Err(_) => m.fair_error = Some("fingerprint panicked".into()),
            }
        });
        if !stage {
            m.art = copy_custom_art(&archive, content, dir);
        }
        m.card = card_for(dir, &m);
        Ok(m)
    }
}

impl Library {
    /// The file uploaded for a prepared entry: its content file itself.
    pub fn package(&self, _dir: &Path, m: &Manifest) -> Result<PathBuf> {
        let content = m
            .content
            .clone()
            .ok_or_else(|| Error::Other(format!("{}: no content file", m.id)))?;
        if m.fingerprint.is_none() {
            return Err(Error::Other(format!("{}: no fingerprint", m.id)));
        }
        if content_key(&content)? != m.source_hash {
            return Err(Error::Other(format!(
                "{}: content file changed since it was prepared",
                m.id
            )));
        }
        Ok(content)
    }

    /// What a client declares online for a prepared entry: fingerprint,
    /// cached fairness profile, card portrait. `None` when it cannot go
    /// online (no fingerprint).
    pub fn declare(&self, dir: &Path, m: &Manifest) -> Option<rugen_proto::FighterDecl> {
        let fingerprint = m.fingerprint.clone()?;
        let portrait = m
            .card
            .as_ref()
            .and_then(|c| fs::read(dir.join(c)).ok())
            .map(|b| rugen_proto::b64_encode(&b))
            .filter(|b| b.len() <= rugen_proto::MAX_PORTRAIT_B64);
        let size = m
            .content
            .as_ref()
            .and_then(|c| fs::metadata(c).ok())
            .map_or(m.content_size, |x| x.len());
        Some(rugen_proto::FighterDecl {
            fingerprint,
            kind: if m.stage { "stage" } else { "char" }.into(),
            id: m.id.clone(),
            name: m.display_name.clone(),
            author: m.author.clone(),
            def: m.package_def.clone().unwrap_or_else(|| m.def.clone()),
            size,
            profile: if m.stage {
                None
            } else {
                Self::current_profile(dir, m)
            },
            portrait,
        })
    }

    /// The entry's fairness profile, analysed again (and saved) when the
    /// cached one predates this engine's [`rugen_fair::FAIR_VERSION`], so
    /// the server's verdict gets the fields it now weighs (height).
    fn current_profile(dir: &Path, m: &Manifest) -> Option<rugen_proto::FairnessProfile> {
        if m.profile
            .as_ref()
            .is_some_and(|p| p.version == rugen_fair::FAIR_VERSION)
        {
            return m.fair_profile();
        }
        let files =
            rugen_fair::files::content_files(rugen_fair::Kind::Char, &m.def_path(dir), None)
                .ok()?;
        match rugen_fair::profile::analyze(&files) {
            Ok(p) => {
                let mut fresh = m.clone();
                fresh.profile = Some(p);
                if let Err(e) = Self::save_manifest(dir, &fresh) {
                    log::debug!("{}: refreshed profile not saved: {e}", m.id);
                }
                fresh.fair_profile()
            }
            Err(e) => {
                log::debug!("{}: profile not refreshed: {e}", m.id);
                m.fair_profile()
            }
        }
    }

    /// Main .def of the prepared entry (local or downloaded) with this fingerprint.
    pub fn def_for_fingerprint(&self, fp: &str) -> Option<PathBuf> {
        self.find_fingerprint(fp).map(|(d, m)| m.def_path(&d))
    }

    /// Where downloaded content files go: `cache/net/<kind>/<fingerprint>.<ext>`
    /// (manifest in `cache/net/<kind>/<fingerprint>/`). Never inside
    /// `chars/` / `stages/`, never over local files.
    pub fn net_dir(&self, stage: bool) -> PathBuf {
        self.cache_dir()
            .join("net")
            .join(if stage { "stages" } else { "chars" })
    }

    /// Install a downloaded content file (whose fingerprint must be `fp`)
    /// into the net cache and return its prepared directory + manifest. The
    /// file at `pkg` is moved (or removed when the content is known already).
    pub fn install_package(
        &self,
        pkg: &Path,
        fp: &str,
        max_bytes: u64,
    ) -> Result<(PathBuf, Manifest)> {
        if !rugen_proto::is_fingerprint(fp) {
            return Err(Error::Other("bad fingerprint".into()));
        }
        let (kind, _) =
            rugen_fair::package::read_header(pkg).map_err(|e| Error::Other(e.to_string()))?;
        let stage = kind == rugen_fair::Kind::Stage;
        if let Some(found) = self.find_fingerprint(fp) {
            let _ = fs::remove_file(pkg);
            return Ok(found);
        }
        let ext = rugen_fair::package::ext(kind);
        let net = self.net_dir(stage);
        fs::create_dir_all(&net)?;
        // verified under a name the readers recognise, then moved into place
        let checking = net.join(format!(".{fp}.checking.{ext}"));
        move_file(pkg, &checking)?;
        let v = match rugen_fair::package::verify(&checking, max_bytes) {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_file(&checking);
                return Err(Error::Other(e.to_string()));
            }
        };
        if v.fingerprint != fp {
            let _ = fs::remove_file(&checking);
            return Err(Error::Other(format!(
                "downloaded content is {} instead of {}",
                rugen_fair::short(&v.fingerprint),
                rugen_fair::short(fp)
            )));
        }
        let dest = net.join(format!("{fp}.{ext}"));
        vfs::forget(&checking);
        fs::rename(&checking, &dest)?;
        vfs::forget(&dest);
        let dir = net.join(fp);
        let _ = fs::remove_dir_all(&dir);
        let id = Path::new(&v.def)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| fp[..12].to_string());
        let mut m = self.describe(&dest, &dir, &id, stage, Some(fp.to_string()))?;
        m.source_hash = content_key(&dest)?;
        Self::save_manifest(&dir, &m)?;
        remember(&dest, &dir);
        Ok((dir, m))
    }

    /// A prepared entry (local or downloaded) with this fingerprint.
    pub fn find_fingerprint(&self, fp: &str) -> Option<(PathBuf, Manifest)> {
        if !rugen_proto::is_fingerprint(fp) {
            return None;
        }
        let usable = |m: &Manifest| {
            m.readable()
                && m.fingerprint.as_deref() == Some(fp)
                && m.content.as_ref().is_some_and(|c| c.is_file())
        };
        for stage in [false, true] {
            let dir = self.net_dir(stage).join(fp);
            if let Some(m) = read_any_manifest(&dir)
                && usable(&m)
            {
                return Some((dir, m));
            }
        }
        for kind in ["chars", "stages"] {
            let Ok(rd) = fs::read_dir(self.cache_dir().join(kind)) else {
                continue;
            };
            for e in rd.flatten() {
                if let Some(m) = read_any_manifest(&e.path())
                    && usable(&m)
                {
                    return Some((e.path(), m));
                }
            }
        }
        None
    }
}

/// Sidecar of a folder's working copy (`<file>.source.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkingCopy {
    /// The folder it was converted from.
    dir: PathBuf,
    /// [`folder_signature`] at conversion time.
    sig: String,
    stage: bool,
}

impl WorkingCopy {
    fn path(content: &Path) -> PathBuf {
        let mut name = content.as_os_str().to_owned();
        name.push(".source.json");
        PathBuf::from(name)
    }

    fn read(content: &Path) -> Option<Self> {
        serde_json::from_slice(&fs::read(Self::path(content)).ok()?).ok()
    }
}

/// Count, total size, newest mtime and names of a folder's files: changes
/// whenever a file is added, removed, renamed or written.
fn folder_signature(dir: &Path) -> String {
    let mut h = blake3::Hasher::new();
    let (mut n, mut total, mut newest) = (0u64, 0u64, 0u64);
    for (rel, p) in extract::files_under(dir) {
        if let Some(s) = rugen_formats::pack::Stamp::of(&p) {
            n += 1;
            total += s.len;
            newest = newest.max(s.mtime_ns);
            h.update(rel.as_bytes());
            h.update(&s.len.to_le_bytes());
            h.update(&s.mtime_ns.to_le_bytes());
        }
    }
    format!("{n}:{total}:{newest}:{}", &h.finalize().to_hex()[..16])
}

/// For hot reload: when `def` lives in the working copy of a folder whose
/// files changed, convert it again (unchanged sections are reused) and
/// return `true`. `None` when `def` is not in a working copy.
pub fn refresh_working_copy(def: &Path, data_dir: &Path) -> Option<Result<bool>> {
    let (content, _) = vfs::split(def)?;
    let w = WorkingCopy::read(&content)?;
    let sig = folder_signature(&w.dir);
    if sig == w.sig {
        return Some(Ok(false));
    }
    let r = (|| {
        let _guard = CONVERTING.lock().unwrap_or_else(|e| e.into_inner());
        let kind = if w.stage {
            rgc::Kind::Stage
        } else {
            rgc::Kind::Char
        };
        let a = vfs::archive(&content).map_err(|e| Error::Other(e.to_string()))?;
        let meta = a.meta().map_err(|e| Error::Other(e.to_string()))?;
        let src = convert::Source {
            root: w.dir.clone(),
            def: meta.def.clone(),
            kind,
            name: meta.source.clone(),
        };
        let report = convert::convert(&src, &content, data_dir, Some(&content))?;
        log::info!("reconverted {}: {}", w.dir.display(), report.summary());
        let new = WorkingCopy { sig, ..w };
        fs::write(
            WorkingCopy::path(&content),
            serde_json::to_string_pretty(&new).map_err(|e| Error::Other(e.to_string()))?,
        )?;
        Ok(true)
    })();
    Some(r)
}

/// Content file → cache directory of its manifest, as prepared in this run.
static MANIFESTS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());

fn remember(content: &Path, dir: &Path) {
    if let Ok(mut m) = MANIFESTS.lock() {
        m.retain(|(c, _)| c != content);
        m.push((content.to_path_buf(), dir.to_path_buf()));
    }
}

/// The cache directory and manifest describing the character / stage whose
/// main def is `def` (`chars/x.rgc/x.def`, or a def next to its manifest).
pub fn manifest_for(def: &Path) -> Option<(PathBuf, Manifest)> {
    if let Some(dir) = def.parent()
        && let Some(m) = read_any_manifest(dir)
        && m.cache_version >= CACHE_VERSION
    {
        return Some((dir.to_path_buf(), m));
    }
    let (content, _) = vfs::split(def)?;
    let known = MANIFESTS.lock().ok().and_then(|m| {
        m.iter()
            .find(|(c, _)| *c == content)
            .map(|(_, d)| d.clone())
    });
    if let Some(dir) = known
        && let Some(m) = read_any_manifest(&dir)
        && m.content.as_deref() == Some(content.as_path())
    {
        return Some((dir, m));
    }
    let cache = vfs::cache_dir()?;
    for sub in ["chars", "stages", "net/chars", "net/stages"] {
        let Ok(rd) = fs::read_dir(cache.join(sub)) else {
            continue;
        };
        for e in rd.flatten() {
            if let Some(m) = read_any_manifest(&e.path())
                && m.content.as_deref() == Some(content.as_path())
            {
                remember(&content, &e.path());
                return Some((e.path(), m));
            }
        }
    }
    None
}

/// Identity of a content file: its claimed fingerprint, size and mtime.
fn content_key(content: &Path) -> Result<String> {
    let a = vfs::archive(content).map_err(|e| Error::Other(e.to_string()))?;
    let s = rugen_formats::pack::Stamp::of(content)
        .ok_or_else(|| Error::Other(format!("{} vanished", content.display())))?;
    let mut h = blake3::Hasher::new();
    h.update(&a.fingerprint);
    h.update(&s.len.to_le_bytes());
    h.update(&s.mtime_ns.to_le_bytes());
    Ok(h.finalize().to_hex()[..16].to_string())
}

/// `<id>.<ext>` in `dir` (or `<id>-2.<ext>`, …) converted from `src_name`.
fn converted_from(dir: &Path, id: &str, kind: rgc::Kind, src_name: &str) -> Option<PathBuf> {
    let candidates = std::iter::once(dir.join(format!("{id}.{}", kind.ext())))
        .chain((2..10).map(|n| dir.join(format!("{id}-{n}.{}", kind.ext()))));
    for c in candidates {
        if !c.is_file() {
            continue;
        }
        if let Ok(a) = vfs::archive(&c)
            && a.kind == kind
            && a.meta().is_ok_and(|m| m.source == src_name)
        {
            return Some(c);
        }
    }
    None
}

/// `<id>.<ext>` in `dir`, or `<id>-2.<ext>`, … when taken.
fn free_name(dir: &Path, id: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{id}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{id}-{n}.{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Rename, or copy + remove across file systems.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_err() {
        fs::copy(from, to)?;
        fs::remove_file(from)?;
    }
    Ok(())
}

/// Card portrait (`card.webp`) from the big portrait or the stage thumbnail.
fn card_for(dir: &Path, m: &Manifest) -> Option<String> {
    let src = if m.stage {
        m.thumb.as_ref()
    } else {
        m.portrait_big.as_ref().or(m.portrait_small.as_ref())
    };
    src.and_then(|p| write_card(&dir.join(&p.file), &dir.join("card.webp")).ok())
}

/// The sprite file of a char (`[Files] sprite`) or stage (`[BGdef] spr`), parsed.
fn open_sff(
    def_path: &Path,
    parsed: &rugen_formats::def::Def,
    stage: bool,
) -> Option<rugen_formats::sff::Sff> {
    let def_dir = def_path.parent()?;
    let name = if stage {
        parsed.section("bgdef")?.raw("spr")?
    } else {
        parsed.section("files")?.raw("sprite")?
    };
    let path = vfs::resolve(def_dir, name.trim())?;
    match rugen_formats::sff::Sff::open(&path, !stage) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            None
        }
    }
}

fn read_any_manifest(dir: &Path) -> Option<Manifest> {
    #[derive(Deserialize)]
    struct Head {
        cache_version: u32,
        #[serde(default)]
        def: String,
    }
    let text = fs::read_to_string(dir.join("rugen.json")).ok()?;
    match serde_json::from_str::<Manifest>(&text) {
        Ok(m) => Some(m),
        // an old manifest: enough to recognise (and remove) it
        Err(_) => serde_json::from_str::<Head>(&text).ok().map(|h| Manifest {
            cache_version: h.cache_version,
            id: String::new(),
            def: h.def,
            display_name: String::new(),
            author: String::new(),
            portrait_small: None,
            portrait_big: None,
            source_hash: String::new(),
            verify: None,
            art: None,
            thumb: None,
            fingerprint: None,
            package_def: None,
            content_size: 0,
            profile: None,
            card: None,
            fair_error: None,
            stage: false,
            min_reader: 0,
            content: None,
        }),
    }
}

/// Card portrait: the PNG scaled to fit 96x96, lossless WebP (a few KB).
fn write_card(png_path: &Path, out: &Path) -> Result<String> {
    let (w, h, rgba) = read_png(png_path)?;
    if w == 0 || h == 0 {
        return Err(Error::Other("empty portrait".into()));
    }
    let s = (96.0 / w.max(h) as f32).min(1.0);
    let (tw, th) = (
        ((w as f32 * s) as u32).max(1),
        ((h as f32 * s) as u32).max(1),
    );
    let mut px = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            let sx = (x * w / tw).min(w - 1);
            let sy = (y * h / th).min(h - 1);
            let i = ((sy * w + sx) * 4) as usize;
            px.extend_from_slice(rgba.get(i..i + 4).unwrap_or(&[0, 0, 0, 0]));
        }
    }
    let mut bytes = Vec::new();
    image_webp::WebPEncoder::new(&mut bytes)
        .encode(&px, tw, th, image_webp::ColorType::Rgba8)
        .map_err(|e| Error::Other(e.to_string()))?;
    fs::write(out, bytes)?;
    Ok(out
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default())
}

/// Convert the select-screen portraits (9000,0 / 9000,1) to PNG in `dir`.
fn portraits_of(
    dir: &Path,
    def_path: &Path,
    parsed: &rugen_formats::def::Def,
    sff: &rugen_formats::sff::Sff,
) -> (Option<PortraitInfo>, Option<PortraitInfo>) {
    let def_dir = def_path.parent().unwrap_or(Path::new("."));
    let Some(files) = parsed.section("files") else {
        return (None, None);
    };
    // SFF v1 chars take their colours from pal1 (.act)
    let act = files
        .raw("pal1")
        .and_then(|p| vfs::resolve(def_dir, p.trim()))
        .and_then(|p| rugen_formats::act::open(p).ok());
    let v1 = matches!(sff.version, Some(rugen_formats::sff::Version::V1));
    let mut out = [None, None];
    for (k, number) in [0u16, 1].into_iter().enumerate() {
        let Some(s) = sff.get(9000, number) else {
            continue;
        };
        let rgba: Vec<u8> = match s.decoded().as_ref() {
            rugen_formats::sff::Pixels::Packed(_) => continue,
            rugen_formats::sff::Pixels::Rgba(px) => px.clone(),
            rugen_formats::sff::Pixels::Indexed(px) => {
                // pal1 replaces the shared (first) palette; sprites with their own palette keep it
                let pal = match (&act, v1, s.palette) {
                    (Some(a), true, Some(0)) => a.clone(),
                    (_, _, Some(i)) => sff.palettes.get(i).cloned().unwrap_or_default(),
                    _ => Vec::new(),
                };
                px.iter()
                    .flat_map(|&i| {
                        let c = pal.get(i as usize).copied().unwrap_or([0, 0, 0, 0]);
                        if i == 0 { [0, 0, 0, 0] } else { c }
                    })
                    .collect()
            }
        };
        let file = format!("portrait_{}.png", if number == 0 { "small" } else { "big" });
        let path = dir.join(&file);
        if let Err(e) = write_png(&path, s.width as u32, s.height as u32, &rgba) {
            log::warn!("{}: {e}", path.display());
            continue;
        }
        out[k] = Some(PortraitInfo {
            file,
            width: s.width as u32,
            height: s.height as u32,
            axis: [s.offset[0] as i32, s.offset[1] as i32],
        });
    }
    let [a, b] = out;
    (a, b)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let mut enc = png::Encoder::new(fs::File::create(path)?, w.max(1), h.max(1));
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc
        .write_header()
        .map_err(|e| Error::Other(e.to_string()))?;
    let mut data = rgba.to_vec();
    data.resize((w.max(1) * h.max(1) * 4) as usize, 0);
    wr.write_image_data(&data)
        .map_err(|e| Error::Other(e.to_string()))?;
    Ok(())
}

/// Read a cached portrait back as straight RGBA.
pub fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let dec = png::Decoder::new(std::io::BufReader::new(fs::File::open(path)?));
    let mut reader = dec.read_info().map_err(|e| Error::Other(e.to_string()))?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| Error::Other(e.to_string()))?;
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// `rugen_art.webp` / `rugen_art.png` up to one folder deep in the content
/// file → `art.webp` / `art.png` in the cache dir.
fn copy_custom_art(a: &rgc::Archive, content: &Path, dir: &Path) -> Option<String> {
    let e = a.files().find(|e| {
        let c = rgc::canon(&e.path);
        let name = c.rsplit('/').next().unwrap_or(&c);
        c.matches('/').count() <= 1 && (name == "rugen_art.webp" || name == "rugen_art.png")
    })?;
    let (name, bytes) = if e.kind == rgc::SectionKind::Image {
        // a PNG stored as lossless WebP: use the WebP itself
        ("art.webp", a.read(e).ok()?)
    } else if rgc::canon(&e.path).ends_with(".webp") {
        ("art.webp", vfs::read(&content.join(&e.path)).ok()?)
    } else {
        ("art.png", vfs::read(&content.join(&e.path)).ok()?)
    };
    fs::write(dir.join(name), bytes).ok()?;
    Some(name.to_string())
}

/// Stage preview: the first `[BG]` sprite (or the largest sprite) scaled to 160 px wide.
fn extract_stage_thumb(
    dir: &Path,
    parsed: &rugen_formats::def::Def,
    sff: &rugen_formats::sff::Sff,
) -> Option<PortraitInfo> {
    let first = parsed
        .sections_named("bg")
        .filter_map(|s| s.i32s("spriteno"))
        .find(|v| v.len() >= 2)
        .map(|v| (v[0] as u16, v[1] as u16));
    let sprite = first.and_then(|(g, n)| sff.get(g, n)).or_else(|| {
        sff.sprites
            .values()
            .max_by_key(|s| s.width as u32 * s.height as u32)
    })?;
    if sprite.width == 0 || sprite.height == 0 {
        return None;
    }
    let pal = sprite
        .palette
        .and_then(|i| sff.palettes.get(i))
        .cloned()
        .unwrap_or_default();
    let pixels = sprite.decoded();
    let rgba_at = |x: usize, y: usize| -> [u8; 4] {
        let i = y * sprite.width as usize + x;
        match pixels.as_ref() {
            rugen_formats::sff::Pixels::Packed(_) => [0; 4],
            rugen_formats::sff::Pixels::Rgba(px) => px
                .get(i * 4..i * 4 + 4)
                .map_or([0; 4], |p| [p[0], p[1], p[2], p[3]]),
            rugen_formats::sff::Pixels::Indexed(px) => {
                let k = px.get(i).copied().unwrap_or(0);
                if k == 0 {
                    [0, 0, 0, 0]
                } else {
                    pal.get(k as usize).copied().unwrap_or([0, 0, 0, 255])
                }
            }
        }
    };
    let tw = 160u32.min(sprite.width as u32);
    let th = ((sprite.height as u32 * tw) / sprite.width as u32).clamp(1, 160);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            let sx = (x as usize * sprite.width as usize) / tw as usize;
            let sy = (y as usize * sprite.height as usize) / th as usize;
            out.extend_from_slice(&rgba_at(sx, sy));
        }
    }
    write_png(&dir.join("thumb.png"), tw, th, &out).ok()?;
    Some(PortraitInfo {
        file: "thumb.png".into(),
        width: tw,
        height: th,
        axis: [0, 0],
    })
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

/// `$XDG_DATA_HOME/rugen`, else `~/.local/share/rugen`.
fn user_data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("rugen"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/rugen"))
}

#[cfg(test)]
mod tests;
