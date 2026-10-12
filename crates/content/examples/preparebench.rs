//! Time `Library::prepare_char` for characters under a content root.
//!
//! `cargo run --release -p rugen-content --example preparebench -- <root> <id>... [--fresh]`
//!
//! `--fresh` deletes each entry's cache directory first (first-import time);
//! without it the second, cached call is timed too.

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fresh = args.iter().any(|a| a == "--fresh");
    let mut it = args.iter().filter(|a| !a.starts_with("--"));
    let root = std::path::PathBuf::from(it.next().expect("usage: preparebench <root> <id>..."));
    let lib = rugen_content::Library::discover(Some(&root));
    for id in it {
        let e = lib
            .find_char(id)
            .unwrap_or_else(|| panic!("{id}: not found"));
        if fresh && let Ok(rd) = std::fs::read_dir(lib.cache_dir().join("chars")) {
            for d in rd.flatten() {
                let n = d.file_name().to_string_lossy().to_string();
                if n.starts_with(&format!("{id}-")) {
                    let _ = std::fs::remove_dir_all(d.path());
                }
            }
        }
        let t = Instant::now();
        let (dir, _m) = lib.prepare_char(&e).unwrap();
        let first = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        lib.prepare_char(&e).unwrap();
        let again = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{id}: prepare {first:.0} ms, cached prepare {again:.1} ms, {}",
            dir.display()
        );
    }
}
