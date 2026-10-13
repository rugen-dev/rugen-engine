//! `RUGEN_LOGIC_SCHEMA`: a hash of the sim, cns and formats sources.
//! Compiled character logic (stored in `.rgc` files and `cache/logic/`)
//! records it, so any change to the structures it serialises makes the
//! engine compile the text again.

use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let mut files = Vec::new();
    for d in ["src", "../cns/src", "../formats/src"] {
        let dir = here.join(d);
        println!("cargo:rerun-if-changed={}", dir.display());
        collect(&dir, &mut files);
    }
    files.sort();
    // FNV-1a over relative paths and contents
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        eat(f
            .strip_prefix(&here)
            .unwrap_or(f)
            .to_string_lossy()
            .as_bytes());
        eat(&std::fs::read(f).unwrap_or_default());
    }
    println!("cargo:rustc-env=RUGEN_LOGIC_SCHEMA={h:016x}");
}
