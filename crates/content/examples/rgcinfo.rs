//! List the sections of `.rgc` / `.rgs` files with their sizes.
//!
//! `cargo run --release -p rugen-content --example rgcinfo -- file.rgc [more.rgc] [--all]`

use rugen_formats::rgc::{Archive, SectionKind};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.iter().any(|a| a == "--all");
    for p in args.iter().filter(|a| !a.starts_with("--")) {
        let a = match Archive::open(p) {
            Ok(a) => a,
            Err(e) => {
                println!("{p}: {e}");
                continue;
            }
        };
        let meta = a.meta().unwrap_or_default();
        println!(
            "{p}: {:?}, {} sections, {:.2} MB, def {}, converted by {}",
            a.kind,
            a.entries.len(),
            a.file_len as f64 / 1e6,
            meta.def,
            meta.converter
        );
        let mut by: std::collections::BTreeMap<String, (usize, u64, u64)> = Default::default();
        for e in &a.entries {
            let k = match e.kind {
                SectionKind::Meta => "meta".to_string(),
                SectionKind::File => "file".into(),
                SectionKind::Sprites => "sprites".into(),
                SectionKind::Sounds => "sounds".into(),
                SectionKind::Image => "image".into(),
                SectionKind::Logic => "logic".into(),
                SectionKind::Other(b) => format!("other {b}"),
            };
            let slot = by.entry(k.clone()).or_default();
            slot.0 += 1;
            slot.1 += e.len;
            slot.2 += e.raw_len;
            if all {
                println!(
                    "  {k:<8} {:>10} stored {:>10} raw  {}",
                    e.len, e.raw_len, e.path
                );
            }
        }
        for (k, (n, len, raw)) in by {
            println!(
                "  {k:<8} x{n:<4} {:>8.2} MB stored ({:.2} MB decompressed)",
                len as f64 / 1e6,
                raw as f64 / 1e6
            );
        }
    }
}
