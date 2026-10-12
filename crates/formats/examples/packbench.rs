//! Build a sprite pack from an SFF and time both sides:
//! `cargo run --release -p rugen-formats --example packbench -- file.sff [--stage] [--no-trim]`
use std::time::Instant;

use rugen_formats::pack::{self, Options, Stamp};
use rugen_formats::sff::Sff;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::Path::new(&a[1]);
    let is_char = !a.iter().any(|s| s == "--stage");
    let trim = is_char && !a.iter().any(|s| s == "--no-trim");
    let t = Instant::now();
    let src = std::fs::read(path).unwrap();
    let sff = Sff::parse(&src, is_char).unwrap();
    let parse = t.elapsed();
    let t = Instant::now();
    let (bytes, st) = pack::write(&sff, Options { is_char, trim }, Stamp::of(path).unwrap());
    let write = t.elapsed();
    let out = std::env::temp_dir().join("packbench.rspk");
    std::fs::write(&out, &bytes).unwrap();
    let t = Instant::now();
    let (back, _) = pack::open(&out).unwrap();
    let read = t.elapsed();
    let t = Instant::now();
    let mut n = 0usize;
    for s in back.sprites.values() {
        n += match s.decoded().as_ref() {
            rugen_formats::sff::Pixels::Indexed(p) | rugen_formats::sff::Pixels::Rgba(p) => p.len(),
            _ => 0,
        };
    }
    let decode_all = t.elapsed();
    println!(
        "{}: sff {:.1} MB parse {:.0} ms | pack {:.1} MB write {:.0} ms read {:.1} ms decode-all {:.0} ms",
        path.display(),
        src.len() as f64 / 1e6,
        parse.as_secs_f64() * 1e3,
        bytes.len() as f64 / 1e6,
        write.as_secs_f64() * 1e3,
        read.as_secs_f64() * 1e3,
        decode_all.as_secs_f64() * 1e3,
    );
    println!(
        "  sprites {} blobs {} palettes {} -> {} | pixels {:.1} MB raw -> {:.1} MB unique/trimmed ({:.1} MB decoded from pack)",
        st.sprites,
        st.blobs,
        st.palettes_in,
        st.palettes_out,
        st.raw_bytes as f64 / 1e6,
        st.unique_bytes as f64 / 1e6,
        n as f64 / 1e6
    );
    let _ = std::fs::remove_file(out);
}
