//! Online fairness for "bring your own character" matches. No GPU, audio
//! or simulation dependencies, so both the game and `rugen-server` use it:
//!
//! * [`files`]: which files of a character / stage are gameplay-relevant
//!   (the def and everything its `[Files]` / `[BGdef]` reference, plus the
//!   common states it falls back to), path-normalised.
//! * [`fingerprint`]: sha256 over that canonical set's gameplay content:
//!   the text files and palettes byte for byte, the sprite file by its
//!   decoded pixels and palettes ([`rugen_formats::sff::Sff::canonical_hash`]),
//!   sounds not at all. Independent of the archive format (zip / rar /
//!   folder / `.rgc`), of junk files, of file order and of how sprites and
//!   sounds are compressed, so converting to `.rgc` and exporting back keep
//!   it; any change to what the simulation or the picture depends on
//!   changes it.
//! * [`package`]: verification of an `.rgc` / `.rgs` received from someone
//!   else (the online transfer format).
//! * [`profile`]: static analysis → [`FairnessProfile`] (stats, damage,
//!   tools, cheat flags, power score).
//! * [`verdict`]: FAIR / UNBALANCED / BLOCKED from two profiles.
//! * [`trust`]: Ed25519 signatures over canonical JSON (official server,
//!   truth feed, signed verdicts and results).

#![forbid(unsafe_code)]

pub mod files;
pub mod package;
pub mod profile;
pub mod trust;
pub mod verdict;

use std::path::Path;

pub use files::{ContentFiles, FileEntry, Kind, Role};
pub use rugen_proto::{CheatFlag, FAIR_VERSION, FairnessProfile, Severity, Verdict, VerdictKind};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Content(String),
    #[error("package: {0}")]
    Package(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Version tag hashed into every fingerprint.
pub const FP_TAG: &str = "rugen-fp-2";

/// The fingerprint row of one file: `raw:<sha256 of the bytes>` for text
/// and palettes, `spr:<canonical sprite hash>` for the sprite file, `None`
/// for the sound file (not part of the fingerprint).
pub fn row_of(kind: Kind, e: &FileEntry) -> Result<Option<String>> {
    let content = |err: rugen_formats::Error| Error::Content(format!("{}: {err}", e.path));
    Ok(match e.role {
        files::Role::Sound => None,
        files::Role::Sprite => {
            let sff = rugen_formats::vfs::open_sff_exact(&e.source, kind == Kind::Char)
                .map_err(content)?;
            Some(format!("spr:{}", hex(&sff.canonical_hash())))
        }
        _ => Some(format!(
            "raw:{}",
            sha256_hex(&rugen_formats::vfs::read(&e.source).map_err(content)?)
        )),
    })
}

/// Fingerprint of a canonical file set: sha256 over `rugen-fp-2 / kind /
/// def` and the sorted `(lowercase path, row)` list (see [`row_of`]).
pub fn fingerprint(files: &ContentFiles) -> Result<String> {
    let mut rows: Vec<(String, String)> = Vec::with_capacity(files.entries.len());
    for e in &files.entries {
        if let Some(r) = row_of(files.kind, e)? {
            rows.push((e.canon.clone(), r));
        }
    }
    Ok(fingerprint_of(files.kind, &files.def, rows))
}

/// [`fingerprint`] from already computed `(canonical path, row)` rows.
pub fn fingerprint_of(kind: Kind, def: &str, mut rows: Vec<(String, String)>) -> String {
    rows.sort();
    rows.dedup_by(|a, b| a.0 == b.0);
    let mut h = Sha256::new();
    h.update(FP_TAG.as_bytes());
    h.update(b"\n");
    h.update(kind.as_str().as_bytes());
    h.update(b"\n");
    h.update(files::canon(def).as_bytes());
    h.update(b"\n");
    for (path, sum) in rows {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(sum.as_bytes());
        h.update(b"\n");
    }
    hex(&h.finalize())
}

/// Fingerprint of the character whose main def is `def` (see [`files::char_files`]).
pub fn char_fingerprint(def: &Path, data_dir: Option<&Path>) -> Result<String> {
    fingerprint(&files::char_files(def, data_dir)?)
}

/// Fingerprint of the stage whose def is `def`.
pub fn stage_fingerprint(def: &Path) -> Result<String> {
    fingerprint(&files::stage_files(def)?)
}

/// First 12 hex digits, for logs and cards.
pub fn short(fp: &str) -> &str {
    fp.get(..12).unwrap_or(fp)
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};

    pub fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rugen-fair-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A one-sprite SFF v2 whose pixel is `colour`.
    pub fn tiny_sff(colour: u8) -> Vec<u8> {
        let mut sff = rugen_formats::sff::Sff {
            version: Some(rugen_formats::sff::Version::V2 { patch: 1 }),
            ..Default::default()
        };
        let mut pal = vec![[0u8; 4]; 256];
        pal[1] = [200, 10, 10, 255];
        let p = sff.push_palette((1, 1), pal);
        sff.push_indexed((0, 0), 2, 1, [0, 0], p, vec![colour, 0]);
        sff.write_exact()
    }

    /// A minimal character in `dir` (own common1.cns); `extra` is appended to its cns.
    pub fn tiny_char(dir: &Path, extra: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w(
            "tiny.def",
            "[Info]\nname = \"Tiny\"\n[Files]\ncmd = tiny.cmd\ncns = tiny.cns\nsprite = Tiny.SFF\nanim = tiny.air\nsound = tiny.snd\nstcommon = common1.cns\npal1 = tiny.act\n",
        );
        w("tiny.cmd", "[Statedef -1]\n");
        w(
            "tiny.cns",
            &format!(
                "[Data]\nlife = 1000\nattack = 100\n[Statedef 200]\nmovetype = A\n[State 200, h]\ntype = HitDef\ntrigger1 = 1\ndamage = 40\nguardflag = MA\n{extra}"
            ),
        );
        std::fs::write(dir.join("Tiny.SFF"), tiny_sff(1)).unwrap();
        w("tiny.air", "[Begin Action 0]\n0,0, 0,0, 10\n");
        w("tiny.snd", "sounds");
        w("tiny.act", "palette");
        w("common1.cns", "[Statedef 0]\n");
        dir.join("tiny.def")
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{tiny_char, tmp};
    use super::*;

    #[test]
    fn fingerprint_ignores_junk_and_location_but_not_edits() {
        let root = tmp("fp");
        let a = char_fingerprint(&tiny_char(&root.join("a"), ""), None).unwrap();
        // same content elsewhere, with junk and a renamed folder
        let def_b = tiny_char(&root.join("renamed_copy"), "");
        std::fs::write(root.join("renamed_copy/readme.txt"), "by me").unwrap();
        std::fs::write(root.join("renamed_copy/Thumbs.db"), "junk").unwrap();
        std::fs::create_dir_all(root.join("renamed_copy/extra")).unwrap();
        std::fs::write(root.join("renamed_copy/extra/unused.sff"), "zzz").unwrap();
        assert_eq!(char_fingerprint(&def_b, None).unwrap(), a);
        // one byte in the cns changes it
        let cns = root.join("renamed_copy/tiny.cns");
        let mut bytes = std::fs::read(&cns).unwrap();
        let i = bytes.iter().position(|&b| b == b'4').unwrap();
        bytes[i] = b'5';
        std::fs::write(&cns, bytes).unwrap();
        assert_ne!(char_fingerprint(&def_b, None).unwrap(), a);
        // a referenced palette change too
        let def_c = tiny_char(&root.join("c"), "");
        std::fs::write(root.join("c/tiny.act"), "palettf").unwrap();
        assert_ne!(char_fingerprint(&def_c, None).unwrap(), a);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn common_file_comes_from_data_when_missing() {
        let root = tmp("fp-common");
        let def = tiny_char(&root.join("chars/t"), "");
        std::fs::remove_file(root.join("chars/t/common1.cns")).unwrap();
        assert!(files::char_files(&def, Some(&root.join("nowhere"))).is_err());
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/common1.cns"), "[Statedef 0]\n").unwrap();
        let with_data = files::char_files(&def, Some(&root.join("data"))).unwrap();
        assert!(with_data.get("common1.cns").is_some());
        // same bytes as the char's own copy → same fingerprint
        let own = char_fingerprint(&tiny_char(&root.join("own"), ""), None).unwrap();
        assert_eq!(fingerprint(&with_data).unwrap(), own);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fingerprint_follows_pixels_not_bytes_and_ignores_sounds() {
        let root = tmp("fp-content");
        let a = char_fingerprint(&tiny_char(&root.join("a"), ""), None).unwrap();
        // other sounds: same fingerprint
        let def = tiny_char(&root.join("b"), "");
        std::fs::write(root.join("b/tiny.snd"), "other sounds").unwrap();
        assert_eq!(char_fingerprint(&def, None).unwrap(), a);
        // the same sprites stored as SFF v2 raw instead of RLE8: same fingerprint
        let sff = rugen_formats::sff::Sff::parse(&test_util::tiny_sff(1), true).unwrap();
        std::fs::write(root.join("b/Tiny.SFF"), sff.write_v2()).unwrap();
        assert_ne!(
            std::fs::read(root.join("b/Tiny.SFF")).unwrap(),
            test_util::tiny_sff(1)
        );
        assert_eq!(char_fingerprint(&def, None).unwrap(), a);
        // one pixel changes it
        std::fs::write(root.join("b/Tiny.SFF"), test_util::tiny_sff(2)).unwrap();
        assert_ne!(char_fingerprint(&def, None).unwrap(), a);
        // a broken sprite file cannot be fingerprinted
        std::fs::write(root.join("b/Tiny.SFF"), "junk").unwrap();
        assert!(char_fingerprint(&def, None).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn order_independent_rows() {
        let rows = vec![("b".to_string(), "2".to_string()), ("a".into(), "1".into())];
        let mut rev = rows.clone();
        rev.reverse();
        assert_eq!(
            fingerprint_of(Kind::Char, "x.def", rows),
            fingerprint_of(Kind::Char, "x.def", rev)
        );
    }
}
