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
//! manifest (main .def, display name, author, portraits converted to PNG so
//! menus never open the full SFF) and returns the prepared directory.
//! A `cache/VERSION` file invalidates every entry when the cache format changes.

#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// Bump when the cache layout or conversion output changes.
pub const CACHE_VERSION: u32 = 4;

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
    #[serde(default)]
    pub author: String,
    /// Select-screen portraits (`9000,0` small, `9000,1` big) as RGBA PNGs in the cache dir.
    #[serde(default)]
    pub portrait_small: Option<PortraitInfo>,
    #[serde(default)]
    pub portrait_big: Option<PortraitInfo>,
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
    /// Screenpack (motif) directory: `<root>/motif` when it holds a `system.def`,
    /// else `<root>/data`, else the data dir. Files missing from the motif fall
    /// back to [`Library::data_dir`].
    pub fn motif_dir(&self) -> PathBuf {
        for d in [self.root.join("motif"), self.root.join("data")] {
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
            let d = self.root.join("motif").join("themes").join(theme);
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
        if let Ok(rd) = fs::read_dir(self.root.join("motif").join("themes")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && (p.join("system.def").exists() || p.join("ui").is_dir()) {
                    out.push(e.file_name().to_string_lossy().to_string());
                }
            }
        }
        out.sort();
        if self.root.join("motif").join("ui").is_dir() {
            out.push("classic".into());
        }
        out
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
        out.sort_by_key(|a| a.id.to_ascii_lowercase());
        out
    }

    pub fn chars(&self) -> Vec<Entry> {
        Self::scan(&self.chars_dir())
    }
    /// Stages in `stages/` plus the built-ins in `motif/stages/` (a user
    /// stage with the same id wins).
    pub fn stages(&self) -> Vec<Entry> {
        let mut out = Self::scan(&self.stages_dir());
        for e in Self::scan(&self.root.join("motif").join("stages")) {
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
            .and_then(|e| self.prepare_stage(e).ok().map(|(d, m)| d.join(m.def)))
            .or_else(|| {
                let p = self.root.join("reference/mugen-1.1b1/stages/stage0.def");
                p.exists().then_some(p)
            })
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

    /// Rewrite a prepared entry's manifest (after verification).
    pub fn save_manifest(dir: &Path, m: &Manifest) -> Result<()> {
        fs::write(
            dir.join("rugen.json"),
            serde_json::to_string_pretty(m).map_err(|e| Error::Other(e.to_string()))?,
        )?;
        Ok(())
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
        let parsed = rugen_formats::def::Def::open(dir.join(&def)).ok();
        let info = |key: &str| {
            parsed
                .as_ref()
                .and_then(|d| d.section("info"))
                .and_then(|i| i.str(key).map(str::to_string))
        };
        let display_name = info("displayname")
            .or_else(|| info("name"))
            .unwrap_or_else(|| entry.id.clone());
        let author = info("author").unwrap_or_default();
        let (portrait_small, portrait_big) = if kind_dir == "chars" {
            match parsed.as_ref() {
                Some(d) => extract_portraits(&dir, &def, d),
                None => (None, None),
            }
        } else {
            (None, None)
        };
        let art = if kind_dir == "chars" {
            copy_custom_art(&dir)
        } else {
            None
        };
        let thumb = if kind_dir == "stages" {
            parsed
                .as_ref()
                .and_then(|d| extract_stage_thumb(&dir, &def, d))
        } else {
            None
        };
        let m = Manifest {
            cache_version: CACHE_VERSION,
            id: entry.id.clone(),
            def,
            display_name,
            author,
            portrait_small,
            portrait_big,
            art,
            thumb,
            source_hash: hash,
            verify: None,
        };
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&m).map_err(|e| Error::Other(e.to_string()))?,
        )?;
        Ok((dir, m))
    }
}

/// Convert the select-screen portraits to PNG next to the manifest.
fn extract_portraits(
    dir: &Path,
    def: &str,
    parsed: &rugen_formats::def::Def,
) -> (Option<PortraitInfo>, Option<PortraitInfo>) {
    let def_dir = dir.join(def);
    let def_dir = def_dir.parent().unwrap_or(dir);
    let Some(files) = parsed.section("files") else {
        return (None, None);
    };
    let Some(spr) = files.raw("sprite") else {
        return (None, None);
    };
    let Some(sff_path) = find_ci(def_dir, spr.trim()) else {
        return (None, None);
    };
    let sff = match rugen_formats::sff::Sff::open(&sff_path, true) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("{}: {e}", sff_path.display());
            return (None, None);
        }
    };
    // SFF v1 chars take their colours from pal1 (.act)
    let act = files
        .raw("pal1")
        .and_then(|p| find_ci(def_dir, p.trim()))
        .and_then(|p| rugen_formats::act::open(p).ok());
    let v1 = matches!(sff.version, Some(rugen_formats::sff::Version::V1));
    let mut out = [None, None];
    for (k, number) in [0u16, 1].into_iter().enumerate() {
        let Some(s) = sff.get(9000, number) else {
            continue;
        };
        let rgba: Vec<u8> = match &s.pixels {
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

/// Case-insensitive lookup of `name` (which may contain subdirectories) under `dir`.
/// `rugen_art.webp` / `rugen_art.png` anywhere up to two levels deep → `art.<ext>` in the cache dir.
fn copy_custom_art(dir: &Path) -> Option<String> {
    fn scan(d: &Path) -> Option<PathBuf> {
        fs::read_dir(d).ok()?.flatten().map(|e| e.path()).find(|p| {
            let name = p
                .file_name()
                .map(|f| f.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            name == "rugen_art.webp" || name == "rugen_art.png"
        })
    }
    let src = scan(dir).or_else(|| {
        fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .find_map(|p| scan(&p))
    })?;
    let ext = src.extension()?.to_string_lossy().to_ascii_lowercase();
    let name = format!("art.{ext}");
    fs::copy(&src, dir.join(&name)).ok()?;
    Some(name)
}

/// Stage preview: the first `[BG]` sprite (or the largest sprite) scaled to 160 px wide.
fn extract_stage_thumb(
    dir: &Path,
    def: &str,
    parsed: &rugen_formats::def::Def,
) -> Option<PortraitInfo> {
    let def_dir = dir.join(def);
    let def_dir = def_dir.parent().unwrap_or(dir);
    let spr = parsed.section("bgdef")?.raw("spr")?.trim().to_string();
    let path = find_ci(def_dir, &spr)?;
    let sff = rugen_formats::sff::Sff::open(&path, false).ok()?;
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
    let rgba_at = |x: usize, y: usize| -> [u8; 4] {
        let i = y * sprite.width as usize + x;
        match &sprite.pixels {
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

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    let mut cur = dir.to_path_buf();
    for part in Path::new(name).components() {
        let want = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        let next = fs::read_dir(&cur)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == want)
            })?;
        cur = next;
    }
    Some(cur)
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
