//! Locate reference content (gitignored, see `reference/fetch.sh`).

use std::path::PathBuf;

/// Path under `reference/`, or `None` if the reference tree is not fetched.
/// Tests that depend on it `return` early so a fresh clone still passes.
pub fn reference(rel: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference")
        .join(rel);
    p.exists().then_some(p)
}
