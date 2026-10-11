//! Locate reference content (gitignored, see `reference/fetch.sh`).

use std::path::{Path, PathBuf};

/// Path under `reference/`, or `None` if the reference tree is not fetched.
/// Tests that depend on it `return` early so a fresh clone still passes.
pub fn reference(rel: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference")
        .join(rel);
    p.exists().then_some(p)
}

/// All files under `dir`, recursively.
pub fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}
