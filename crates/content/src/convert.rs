//! Converting a character / stage folder into one `.rgc` / `.rgs` file.
//!
//! Every file of the folder goes in (no minifier: nothing is dropped):
//!
//! - `.sff` → sprite section, pixel-exact ([`rugen_formats::rgc::sprites`]);
//! - `.snd` → sound section, Opus ([`rugen_formats::rgc::sounds`]);
//! - `.png` → lossless WebP when it is an 8-bit image and WebP is smaller;
//! - anything else (def, cns, cmd, air, act, txt, mp3 / ogg music, …) byte
//!   for byte, zstd-compressed when that pays;
//! - characters also get their logic compiled by this engine.
//!
//! Files that fail to decode (a broken SFF / SND, an odd PNG) are kept byte
//! for byte too, so conversion never loses anything. A common states file
//! the character uses from the engine's `data/` is copied in, so the
//! content file is self-contained (the opponent online simulates the same
//! states). The fingerprint is computed from the folder before conversion
//! and again from the written file; the file is only kept when both agree.
//!
//! A re-conversion of a folder (creator workflow) reuses every section of
//! the previous file whose source has the same size and modification time.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rugen_formats::pack::Stamp;
use rugen_formats::rgc::{self, Archive, Entry, Meta, SectionKind, Writer};

use crate::{Error, Result};

/// Bump when conversion output changes in a way that should rebuild every
/// section of a working copy.
pub const CONVERTER: u32 = 1;

/// What to convert.
#[derive(Clone, Debug)]
pub struct Source {
    /// Folder holding the content (the def is inside).
    pub root: PathBuf,
    /// Main def relative to `root`.
    pub def: String,
    pub kind: rgc::Kind,
    /// File or folder name it came from (`G6_Naruto.rar`), for the record.
    pub name: String,
}

/// What a conversion produced.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// `None` when the content cannot be fingerprinted (a referenced file
    /// missing, an unreadable sprite file): it still plays locally.
    pub fingerprint: Option<String>,
    pub bytes: u64,
    pub sprites: u64,
    pub sounds: u64,
    pub images: u64,
    pub logic: u64,
    pub files: u64,
    /// Sections copied from the previous file.
    pub reused: usize,
    pub millis: u128,
    /// Files kept byte for byte because they did not decode.
    pub kept_raw: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mb = |b: u64| b as f64 / 1e6;
        format!(
            "{:.1} MB (sprites {:.1}, sounds {:.1}, images {:.1}, logic {:.1}, files {:.1}) in {} ms{}",
            mb(self.bytes),
            mb(self.sprites),
            mb(self.sounds),
            mb(self.images),
            mb(self.logic),
            mb(self.files),
            self.millis,
            if self.reused > 0 {
                format!(", {} sections reused", self.reused)
            } else {
                String::new()
            }
        )
    }
}

fn fair_kind(k: rgc::Kind) -> rugen_fair::Kind {
    match k {
        rgc::Kind::Char => rugen_fair::Kind::Char,
        rgc::Kind::Stage => rugen_fair::Kind::Stage,
    }
}

/// Extensions whose data is already compressed: stored as they are.
fn incompressible(path: &str) -> bool {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "mp3"
            | "ogg"
            | "oga"
            | "opus"
            | "flac"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "zip"
            | "rar"
            | "7z"
            | "gz"
            | "xz"
            | "bz2"
            | "mp4"
            | "avi"
            | "webm"
            | "mkv"
            | "wmv"
            | "png"
            | "rgc"
            | "rgs"
    )
}

fn ext_is(path: &str, ext: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
}

/// One finished section.
struct Section {
    entry: Entry,
    data: Vec<u8>,
    /// Sprite sections encoded here: the hash of every block's pixels, so
    /// the written file is checked block by block ([`verify_written`]).
    /// `None` (reused or raw sections) means a full decode check.
    block_hashes: Option<Vec<[u8; 32]>>,
}

impl Section {
    fn new(entry: Entry, data: Vec<u8>) -> Self {
        Self {
            entry,
            data,
            block_hashes: None,
        }
    }
}

/// A PNG as lossless WebP, when every pixel survives (8-bit RGB / RGBA /
/// grey / indexed) and the WebP is smaller.
fn png_to_webp(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().ok()?;
    if r.info().bit_depth == png::BitDepth::Sixteen || r.info().is_animated() {
        return None;
    }
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let rgba: Vec<u8> = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf,
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buf
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => {
            buf.iter().flat_map(|&g| [g, g, g, 255]).collect()
        }
        _ => return None,
    };
    if rgba.len() != n * 4 {
        return None;
    }
    let opaque = rgba.chunks_exact(4).all(|c| c[3] == 255);
    let mut out = Vec::new();
    let enc = image_webp::WebPEncoder::new(&mut out);
    let ok = if opaque {
        let rgb: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|c| [c[0], c[1], c[2]])
            .collect();
        enc.encode(&rgb, w, h, image_webp::ColorType::Rgb8)
    } else {
        enc.encode(&rgba, w, h, image_webp::ColorType::Rgba8)
    };
    ok.ok()?;
    (out.len() < bytes.len()).then_some(out)
}

/// A file kept byte for byte.
fn raw_section(path: &str, bytes: &[u8], stamp: Option<Stamp>) -> Section {
    let mut e = Entry::new(SectionKind::File, path);
    e.hash = rgc::sha256(bytes);
    stamp_entry(&mut e, stamp);
    let (codec, data) = if incompressible(path) {
        (rgc::Codec::Stored, bytes.to_vec())
    } else {
        rgc::compress(bytes, rgc::level())
    };
    e.codec = codec;
    e.raw_len = bytes.len() as u64;
    Section::new(e, data)
}

fn stamp_entry(e: &mut Entry, stamp: Option<Stamp>) {
    if let Some(s) = stamp {
        e.src_len = s.len;
        e.src_mtime = s.mtime_ns;
    }
}

/// Convert `src` into the content file `out`. `data_dir` holds the
/// engine's `common1.cns`; `prev` is an earlier conversion of the same
/// folder whose unchanged sections may be reused.
pub fn convert(src: &Source, out: &Path, data_dir: &Path, prev: Option<&Path>) -> Result<Report> {
    let t = Instant::now();
    let kind = src.kind;
    let def_path = src.root.join(&src.def);
    let parsed = rugen_formats::def::Def::open(&def_path)
        .map_err(|e| Error::Other(format!("{}: {e}", src.def)))?;
    let info = |key: &str| {
        parsed
            .section("info")
            .and_then(|i| i.str(key).map(str::to_string))
    };
    // the canonical file set: which file is the sprite file, which common
    // states come from the engine
    // a set that cannot be built (a referenced file missing) still converts:
    // the content file just has no fingerprint, like the folder
    let files = rugen_fair::files::content_files(fair_kind(kind), &def_path, Some(data_dir))
        .inspect_err(|e| log::warn!("{}: no fingerprint: {e}", src.name))
        .ok();
    let def_dir = def_path.parent().unwrap_or(&src.root).to_path_buf();
    let rel_of = |p: &Path| -> Option<String> {
        p.strip_prefix(&src.root)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    };
    let named_sprite = || {
        let name = match kind {
            rgc::Kind::Char => parsed.section("files")?.raw("sprite")?,
            rgc::Kind::Stage => parsed.section("bgdef")?.raw("spr")?,
        };
        rugen_formats::vfs::resolve(&def_dir, name.trim())
    };
    let sprite_file = files
        .as_ref()
        .and_then(|f| {
            f.entries
                .iter()
                .find(|e| e.role == rugen_fair::Role::Sprite)
        })
        .map(|e| e.source.clone())
        .or_else(named_sprite)
        .and_then(|p| rel_of(&p))
        .map(|r| rgc::canon(&r));
    // files the content uses from data/ (common1.cns): copied in, at the
    // path the def names, relative to the def's folder
    let def_rel_dir = rel_of(&def_dir).unwrap_or_default();
    let engine_files: Vec<(String, PathBuf)> = files
        .iter()
        .flat_map(|f| f.entries.iter())
        .filter(|e| !e.source.starts_with(&src.root))
        .map(|e| {
            let rel = if def_rel_dir.is_empty() {
                e.path.clone()
            } else {
                format!("{def_rel_dir}/{}", e.path)
            };
            (rel, e.source.clone())
        })
        .collect();

    let prev = prev.and_then(|p| Archive::open(p).ok()).filter(|a| {
        a.meta()
            .is_ok_and(|m| m.converter == converter_tag() && a.kind == kind)
    });
    let reusable = |path: &str, stamp: Option<Stamp>, want: &[SectionKind]| -> Option<Section> {
        let a = prev.as_ref()?;
        let s = stamp?;
        let e = a.find(path)?;
        if e.path != path
            || e.src_len != s.len
            || e.src_mtime != s.mtime_ns
            || !want.contains(&e.kind)
        {
            return None;
        }
        Some(Section::new(e.clone(), a.read_raw(e).ok()?))
    };

    let all = crate::extract::files_under(&src.root);
    let mut sffs = Vec::new();
    let mut snds = Vec::new();
    let mut others = Vec::new();
    for (rel, p) in all {
        if ext_is(&rel, "sff") {
            sffs.push((rel, p));
        } else if ext_is(&rel, "snd") {
            snds.push((rel, p));
        } else {
            others.push((rel, p));
        }
    }
    for (rel, p) in &engine_files {
        if !others.iter().any(|(r, _)| rgc::canon(r) == rgc::canon(rel)) {
            others.push((rel.clone(), p.clone()));
        }
    }
    let mut kept_raw: Vec<String> = Vec::new();
    let mut reused = 0usize;
    let next = std::sync::atomic::AtomicUsize::new(0);
    // sprites, sounds and the rest on their own threads (each spreads over
    // the cores); with one encoder thread allowed (a game converting in
    // the background on a small machine) the phases run one after another
    let threads = rugen_formats::encode_threads().clamp(1, 8);
    let do_spr = || {
        let mut out = Vec::new();
        for (rel, p) in &sffs {
            let stamp = Stamp::of(p);
            if let Some(sec) = reusable(rel, stamp, &[SectionKind::Sprites, SectionKind::File]) {
                out.push((sec, true, None));
                continue;
            }
            let Ok(bytes) = std::fs::read(p) else {
                continue;
            };
            let is_char =
                kind == rgc::Kind::Char && sprite_file.as_deref() == Some(&rgc::canon(rel));
            // streamed: a batch of sprites at a time, never the decoded file
            match rgc::sprites::encode_file(&bytes, is_char) {
                Ok(enc) => {
                    let mut e = Entry::new(SectionKind::Sprites, rel);
                    e.flags = if is_char { rgc::SPRITES_CHAR } else { 0 };
                    e.hash = enc.hash;
                    e.raw_len = enc.bytes.len() as u64;
                    stamp_entry(&mut e, stamp);
                    out.push((
                        Section {
                            entry: e,
                            data: enc.bytes,
                            block_hashes: Some(enc.block_hashes),
                        },
                        false,
                        None,
                    ));
                }
                Err(err) => {
                    log::warn!("{rel}: {err}; kept as it is");
                    out.push((raw_section(rel, &bytes, stamp), false, Some(rel.clone())));
                }
            }
        }
        out
    };
    let do_snd = || {
        let mut out = Vec::new();
        for (rel, p) in &snds {
            let stamp = Stamp::of(p);
            if let Some(sec) = reusable(rel, stamp, &[SectionKind::Sounds, SectionKind::File]) {
                out.push((sec, true, None));
                continue;
            }
            let Ok(bytes) = std::fs::read(p) else {
                continue;
            };
            match rugen_formats::snd::Snd::parse(&bytes) {
                Ok(snd) => {
                    let (data, _) = rgc::sounds::encode(&snd);
                    let mut e = Entry::new(SectionKind::Sounds, rel);
                    e.hash = rgc::sha256(&bytes);
                    e.raw_len = data.len() as u64;
                    stamp_entry(&mut e, stamp);
                    out.push((Section::new(e, data), false, None));
                }
                Err(err) => {
                    log::warn!("{rel}: {err}; kept as it is");
                    out.push((raw_section(rel, &bytes, stamp), false, Some(rel.clone())));
                }
            }
        }
        out
    };
    let do_logic = || {
        (kind == rgc::Kind::Char)
            .then(|| rugen_sim::chardata::compile_logic(&def_path, data_dir))
            .and_then(|r| {
                r.map_err(|e| log::warn!("{}: logic not compiled: {e}", src.def))
                    .ok()
            })
    };
    // the rest, a few files per thread
    let do_rest = || {
        let mut out = Vec::new();
        loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some((rel, p)) = others.get(i) else { break };
            let stamp = Stamp::of(p);
            if let Some(sec) = reusable(rel, stamp, &[SectionKind::Image, SectionKind::File]) {
                out.push((sec, true, None));
                continue;
            }
            let Ok(bytes) = std::fs::read(p) else {
                continue;
            };
            if ext_is(rel, "png")
                && let Some(webp) = png_to_webp(&bytes)
            {
                let mut e = Entry::new(SectionKind::Image, rel);
                e.flags = rgc::IMAGE_PNG;
                e.hash = rgc::sha256(&bytes);
                e.raw_len = webp.len() as u64;
                stamp_entry(&mut e, stamp);
                out.push((Section::new(e, webp), false, None));
                continue;
            }
            out.push((raw_section(rel, &bytes, stamp), false, None));
        }
        out
    };
    let (spr, snd, rest, logic) = if threads == 1 {
        (do_spr(), do_snd(), do_rest(), do_logic())
    } else {
        std::thread::scope(|s| {
            let spr = s.spawn(do_spr);
            let snd = s.spawn(do_snd);
            let logic = s.spawn(do_logic);
            let workers: Vec<_> = (0..threads).map(|_| s.spawn(do_rest)).collect();
            let rest: Vec<_> = workers
                .into_iter()
                .flat_map(|w| w.join().unwrap_or_default())
                .collect();
            (
                spr.join().unwrap_or_default(),
                snd.join().unwrap_or_default(),
                rest,
                logic.join().ok().flatten(),
            )
        })
    };

    // fingerprint of the folder: sprite rows from the hashes just computed
    let mut sections: Vec<Section> = Vec::new();
    for (sec, was_reused, raw) in spr.into_iter().chain(snd).chain(rest) {
        if was_reused {
            reused += 1;
        }
        if let Some(r) = raw {
            kept_raw.push(r);
        }
        sections.push(sec);
    }
    sections.sort_by(|a, b| {
        a.entry
            .path
            .to_ascii_lowercase()
            .cmp(&b.entry.path.to_ascii_lowercase())
    });
    let fingerprint = match &files {
        Some(f) => fingerprint_with(f, &src.root, &sections)
            .inspect_err(|e| log::warn!("{}: no fingerprint: {e}", src.name))
            .ok(),
        None => None,
    };

    // write next to the target, then verify, then move into place
    // a hidden name the library never lists, with the extension the readers know
    let tmp = out.with_file_name(format!(
        ".{}.converting.{}",
        out.file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        kind.ext()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut report = Report {
        fingerprint: fingerprint.clone(),
        reused,
        kept_raw,
        ..Default::default()
    };
    {
        let mut w = Writer::create(&tmp, kind)?;
        let meta = Meta {
            def: src.def.clone(),
            name: info("displayname")
                .or_else(|| info("name"))
                .unwrap_or_default(),
            author: info("author").unwrap_or_default(),
            converter: converter_tag(),
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            source: src.name.clone(),
            engine_files: engine_files.iter().map(|(r, _)| r.clone()).collect(),
        };
        w.add_meta(&meta)?;
        if let Some(l) = &logic {
            w.add_logic(rugen_sim::chardata::LOGIC_SCHEMA, l)?;
        }
        for s in &sections {
            let n = s.data.len() as u64;
            match s.entry.kind {
                SectionKind::Sprites => report.sprites += n,
                SectionKind::Sounds => report.sounds += n,
                SectionKind::Image => report.images += n,
                _ => report.files += n,
            }
            w.add_stored(s.entry.clone(), &s.data)?;
        }
        if let Some(e) = w.entries().iter().find(|e| e.kind == SectionKind::Logic) {
            report.logic = e.len;
        }
        let raw = fingerprint
            .as_deref()
            .and_then(rugen_proto::from_hex)
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .unwrap_or([0; 32]);
        w.finish(raw)?;
    }
    // re-read the written file: every section as built, every sprite block
    // decoding to the pixels that were hashed, the fingerprint from its TOC
    let check = verify_written(&tmp, kind, &sections, fingerprint.as_deref());
    if let Err(e) = check {
        let _ = std::fs::remove_file(&tmp);
        rugen_formats::vfs::forget(&tmp);
        return Err(Error::Other(format!("{}: converted file {e}", src.name)));
    }
    rugen_formats::vfs::forget(&tmp);
    std::fs::rename(&tmp, out)?;
    rugen_formats::vfs::forget(out);
    report.bytes = std::fs::metadata(out).map_or(0, |m| m.len());
    report.millis = t.elapsed().as_millis();
    Ok(report)
}

/// Check the file just written at `tmp` against what was built, without
/// decoding the SFFs a second time:
///
/// - the archive opens (header, TOC, meta, main def) and every section's
///   stored bytes are the ones built in memory;
/// - every sprite block decompresses to the pixel bytes the encoder cropped
///   and hashed ([`rgc::sprites::verify_blocks`]); sections without block
///   hashes (reused, raw) are decoded whole and their canonical hash compared;
/// - the fingerprint recomputed from the canonical file set *inside* the
///   file (its common states included) is the one claimed.
fn verify_written(
    tmp: &Path,
    kind: rgc::Kind,
    sections: &[Section],
    fingerprint: Option<&str>,
) -> std::result::Result<(), String> {
    let read_back = |e: &str| format!("does not read back: {e}");
    let a = Archive::open(tmp).map_err(|e| read_back(&e.to_string()))?;
    if a.kind != kind {
        return Err(read_back("other kind"));
    }
    let meta = a.meta().map_err(|e| read_back(&e.to_string()))?;
    if !rugen_fair::files::safe_rel(&meta.def) || a.find(&meta.def).is_none() {
        return Err(read_back(&format!("bad main def {:?}", meta.def)));
    }
    for s in sections {
        let e = a
            .entries
            .iter()
            .find(|e| e.kind == s.entry.kind && rgc::canon(&e.path) == rgc::canon(&s.entry.path))
            .ok_or_else(|| read_back(&format!("{} missing", s.entry.path)))?;
        let stored = a.read_raw(e).map_err(|e| read_back(&e.to_string()))?;
        if stored != s.data || e.hash != s.entry.hash || e.raw_len != s.entry.raw_len {
            return Err(read_back(&format!("{} differs", s.entry.path)));
        }
        if e.kind == SectionKind::Sprites {
            match &s.block_hashes {
                Some(h) => rgc::sprites::verify_blocks(&stored, h)
                    .map_err(|err| read_back(&format!("{}: {err}", e.path)))?,
                None => {
                    let sff = rgc::sprites::decode(
                        rugen_formats::pack::Blob::mem(stored),
                        rgc::sprites::View::Exact,
                    )
                    .map_err(|err| read_back(&format!("{}: {err}", e.path)))?;
                    if sff.canonical_hash() != e.hash {
                        return Err(read_back(&format!("{}: other sprites", e.path)));
                    }
                }
            }
        }
    }
    let Some(fp) = fingerprint else {
        // nothing to compare: at least the def must read back
        return rugen_formats::vfs::read(&tmp.join(&meta.def))
            .map(|_| ())
            .map_err(|e| read_back(&e.to_string()));
    };
    let files = rugen_fair::files::content_files(fair_kind(kind), &tmp.join(&meta.def), None)
        .map_err(|e| read_back(&e.to_string()))?;
    let mut rows = Vec::new();
    for e in &files.entries {
        let known = (e.role == rugen_fair::Role::Sprite)
            .then(|| {
                let rel = e
                    .source
                    .strip_prefix(tmp)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                a.entries.iter().find(|s| {
                    s.kind == SectionKind::Sprites && rgc::canon(&s.path) == rgc::canon(&rel)
                })
            })
            .flatten();
        let row = match known {
            Some(s) => Some(format!("spr:{}", rugen_fair::hex(&s.hash))),
            None => rugen_fair::row_of(files.kind, e).map_err(|e| read_back(&e.to_string()))?,
        };
        if let Some(r) = row {
            rows.push((e.canon.clone(), r));
        }
    }
    let got = rugen_fair::fingerprint_of(files.kind, &files.def, rows);
    if got != fp {
        return Err(format!(
            "reads back as {} instead of {}",
            rugen_fair::short(&got),
            rugen_fair::short(fp)
        ));
    }
    Ok(())
}

/// Convert a folder, `.zip` or `.rar` (`rugen convert`) into `out`.
pub fn convert_path(src: &Path, out: &Path, kind: rgc::Kind, data_dir: &Path) -> Result<Report> {
    let id = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let stage = kind == rgc::Kind::Stage;
    let (root, tmp) = if src.is_dir() {
        (crate::extract::content_root(src), None)
    } else {
        let tmp = std::env::temp_dir().join(format!(
            "rugen-convert-{}-{}",
            id.replace(|c: char| !c.is_ascii_alphanumeric(), "_"),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let ext = src
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let r = match ext.as_str() {
            "zip" => crate::extract::extract_zip(src, &tmp),
            "rar" => crate::extract::extract_rar(src, &tmp),
            _ => Err(Error::Other(format!(
                "{}: not a folder, .zip or .rar",
                src.display()
            ))),
        };
        if let Err(e) = r {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        (crate::extract::content_root(&tmp), Some(tmp))
    };
    let r = crate::extract::find_main_def(&root, &id, stage)
        .ok_or_else(|| Error::Other(format!("{}: no usable .def found", src.display())))
        .and_then(|def| {
            convert(
                &Source {
                    root: root.clone(),
                    def,
                    kind,
                    name,
                },
                out,
                data_dir,
                None,
            )
        });
    if let Some(t) = tmp {
        let _ = std::fs::remove_dir_all(t);
    }
    r
}

/// `rugen <version> / converter <n>`: sections of a file converted by
/// another converter are never reused.
pub fn converter_tag() -> String {
    format!(
        "rugen {} / converter {CONVERTER}",
        env!("CARGO_PKG_VERSION")
    )
}

/// The fingerprint of the folder's canonical file set, using the sprite
/// hashes of the converted sections instead of decoding the SFF again.
fn fingerprint_with(
    files: &rugen_fair::ContentFiles,
    root: &Path,
    sections: &[Section],
) -> Result<String> {
    let mut rows = Vec::new();
    for e in &files.entries {
        let known = (e.role == rugen_fair::Role::Sprite)
            .then(|| {
                let rel = e
                    .source
                    .strip_prefix(root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                sections.iter().find(|s| {
                    s.entry.kind == SectionKind::Sprites
                        && rgc::canon(&s.entry.path) == rgc::canon(&rel)
                })
            })
            .flatten();
        let row = match known {
            Some(s) => Some(format!("spr:{}", rugen_fair::hex(&s.entry.hash))),
            None => {
                rugen_fair::row_of(files.kind, e).map_err(|err| Error::Other(err.to_string()))?
            }
        };
        if let Some(r) = row {
            rows.push((e.canon.clone(), r));
        }
    }
    Ok(rugen_fair::fingerprint_of(files.kind, &files.def, rows))
}

/// The opened content file and its main def path inside it.
pub fn def_in(content: &Path) -> Result<(Arc<Archive>, PathBuf)> {
    let a = rugen_formats::vfs::archive(content).map_err(|e| Error::Other(e.to_string()))?;
    let m = a.meta().map_err(|e| Error::Other(e.to_string()))?;
    Ok((a, content.join(m.def)))
}
