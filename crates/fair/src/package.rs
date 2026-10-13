//! Content received from someone else (online transfers): an `.rgc`
//! (character) or `.rgs` (stage) file, checked before anything trusts it.
//!
//! The file itself is the package: nothing is unpacked. [`verify`] opens
//! it (the reader already refuses unsafe paths, duplicate entries,
//! sections outside the file and absurd sizes), checks the size cap and the
//! main def, rebuilds the canonical file set of that def *inside* the file
//! (its common states must be there too: a package never borrows the
//! receiver's `data/`) and recomputes the fingerprint from the content,
//! never from the claim in the header.

use std::path::{Path, PathBuf};

use rugen_formats::rgc;

use crate::files::{ContentFiles, Kind, content_files, safe_rel};
use crate::{Error, Result, fingerprint, hex};

/// File extension of a package of `kind`.
pub fn ext(kind: Kind) -> &'static str {
    match kind {
        Kind::Char => "rgc",
        Kind::Stage => "rgs",
    }
}

/// A verified content file.
#[derive(Clone, Debug)]
pub struct Verified {
    pub kind: Kind,
    /// The content file.
    pub path: PathBuf,
    /// Main def inside it.
    pub def: String,
    /// Fingerprint the file's header claims.
    pub claimed: String,
    /// Fingerprint recomputed from the content.
    pub fingerprint: String,
    pub files: ContentFiles,
    /// Decompressed bytes of all sections.
    pub bytes: u64,
}

impl Verified {
    /// The main def as a path the engine loads (`x.rgc/x.def`).
    pub fn def_path(&self) -> PathBuf {
        self.path.join(&self.def)
    }

    /// The recomputed fingerprint is the claimed one.
    pub fn honest(&self) -> bool {
        self.claimed == self.fingerprint
    }
}

fn kind_of(k: rgc::Kind) -> Kind {
    match k {
        rgc::Kind::Char => Kind::Char,
        rgc::Kind::Stage => Kind::Stage,
    }
}

/// Verify the content file at `path`, refusing more than `max_bytes` of
/// decompressed content.
pub fn verify(path: &Path, max_bytes: u64) -> Result<Verified> {
    let bad = |m: String| Error::Package(m);
    // a fresh read: a file replaced under the same name must not hit the cache
    rugen_formats::vfs::forget(path);
    let a = rugen_formats::vfs::archive(path).map_err(|e| bad(e.to_string()))?;
    let bytes: u64 = a.entries.iter().map(|e| e.raw_len).sum();
    if bytes > max_bytes {
        return Err(bad(format!(
            "content larger than {} MB",
            max_bytes / (1024 * 1024)
        )));
    }
    let meta = a.meta().map_err(|e| bad(e.to_string()))?;
    let def = meta.def.clone();
    if !safe_rel(&def) || a.find(&def).is_none() {
        return Err(bad(format!("bad main def {def:?}")));
    }
    let kind = kind_of(a.kind);
    let files = content_files(kind, &path.join(&def), None)?;
    let fingerprint = fingerprint(&files)?;
    Ok(Verified {
        kind,
        path: path.to_path_buf(),
        def,
        claimed: hex(&a.fingerprint),
        fingerprint,
        files,
        bytes,
    })
}

/// The kind and claimed fingerprint of a content file, from its header only.
pub fn read_header(path: &Path) -> Result<(Kind, String)> {
    let a = rgc::Archive::open(path).map_err(|e| Error::Package(e.to_string()))?;
    Ok((kind_of(a.kind), hex(&a.fingerprint)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::char_files;
    use crate::test_util::{tiny_char, tmp};
    use rugen_formats::rgc::{Entry, Meta, SectionKind, Writer};

    /// A content file holding every file of `dir` byte for byte.
    fn pack(dir: &Path, out: &Path, def: &str, claim: [u8; 32], extra: &[(&str, &[u8])]) {
        let mut w = Writer::create(out, rgc::Kind::Char).unwrap();
        w.add_meta(&Meta {
            def: def.into(),
            ..Default::default()
        })
        .unwrap();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let b = std::fs::read(e.path()).unwrap();
            let mut en = Entry::new(SectionKind::File, &e.file_name().to_string_lossy());
            en.hash = rgc::sha256(&b);
            w.add(en, &b, 3).unwrap();
        }
        for (p, b) in extra {
            w.add(Entry::new(SectionKind::File, p), b, 3).unwrap();
        }
        w.finish(claim).unwrap();
    }

    fn raw(fp: &str) -> [u8; 32] {
        let b = rugen_proto::from_hex(fp).unwrap();
        b.try_into().unwrap()
    }

    #[test]
    fn verify_recomputes_the_fingerprint() {
        let root = tmp("pkg-verify");
        let def = tiny_char(&root.join("src"), "");
        let fp = fingerprint(&char_files(&def, None).unwrap()).unwrap();
        let p = root.join("t.rgc");
        pack(
            &root.join("src"),
            &p,
            "tiny.def",
            raw(&fp),
            &[("readme.txt", b"hi")],
        );
        let v = verify(&p, 1 << 20).unwrap();
        assert!(v.honest());
        assert_eq!(v.fingerprint, fp, "same fingerprint as the folder");
        assert_eq!(v.kind, Kind::Char);
        assert!(v.def_path().ends_with("t.rgc/tiny.def"));
        assert_eq!(read_header(&p).unwrap(), (Kind::Char, fp.clone()));
        // a lying header is caught
        let liar = root.join("liar.rgc");
        pack(&root.join("src"), &liar, "tiny.def", [0; 32], &[]);
        let v = verify(&liar, 1 << 20).unwrap();
        assert!(!v.honest());
        assert_eq!(v.fingerprint, fp);
        // size cap, bad def, garbage
        assert!(verify(&p, 10).is_err());
        let nodef = root.join("nodef.rgc");
        pack(&root.join("src"), &nodef, "missing.def", [0; 32], &[]);
        assert!(verify(&nodef, 1 << 20).is_err());
        std::fs::write(root.join("junk.rgc"), b"RUGENRGC garbage").unwrap();
        assert!(verify(&root.join("junk.rgc"), 1 << 20).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
