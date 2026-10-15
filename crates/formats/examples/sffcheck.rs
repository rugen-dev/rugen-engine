//! Check that `Sff::write_exact` and the `.rgc` sprite section give back
//! every sprite of SFF files: `cargo run -p rugen-formats --release --example sffcheck -- a.sff [b.sff ..] [--char]`

use rugen_formats::rgc::sprites;
use rugen_formats::sff::Sff;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_char = args.iter().any(|a| a == "--char");
    for p in args.iter().filter(|a| !a.starts_with("--")) {
        let t = std::time::Instant::now();
        let sff = Sff::open(p, is_char).expect("parse");
        let parse = t.elapsed();
        let t = std::time::Instant::now();
        let bytes = sff.write_exact();
        let back = Sff::parse(&bytes, is_char).expect("reparse");
        let mut bad = 0;
        for (k, s) in &sff.sprites {
            match back.sprites.get(k) {
                Some(b) if b == s => {}
                Some(b) => {
                    bad += 1;
                    if bad < 4 {
                        let (pa, pb) = (s.decoded(), b.decoded());
                        let len = |p: &rugen_formats::sff::Pixels| match p {
                            rugen_formats::sff::Pixels::Indexed(v) => v.len(),
                            rugen_formats::sff::Pixels::Rgba(v) => v.len(),
                            _ => 0,
                        };
                        println!(
                            "  {k:?}: {}x{} {:?} pal {:?} len {} vs {}x{} {:?} pal {:?} len {}",
                            s.width,
                            s.height,
                            s.offset,
                            s.palette,
                            len(&pa),
                            b.width,
                            b.height,
                            b.offset,
                            b.palette,
                            len(&pb)
                        );
                    }
                }
                None => bad += 1,
            }
        }
        let pal_ok = back.palettes == sff.palettes && back.palette_keys == sff.palette_keys;
        let write = t.elapsed();
        let t = std::time::Instant::now();
        let (sec, st) = sprites::encode(&sff, is_char);
        let enc = t.elapsed();
        let t = std::time::Instant::now();
        let exact = sprites::unpacked(
            &sprites::decode(
                rugen_formats::pack::Blob::mem(sec.clone()),
                sprites::View::Exact,
            )
            .unwrap(),
        );
        let dec = t.elapsed();
        let same = exact.canonical_hash() == sff.canonical_hash();
        println!(
            "{p}: {:?} {} sprites, {} palettes; write_exact: {} bad, palettes ok {pal_ok}, {} -> {} bytes; \
             section {} bytes ({} blocks) exact {same}; parse {parse:?} write {write:?} encode {enc:?} decode {dec:?}",
            sff.version,
            sff.sprites.len(),
            sff.palettes.len(),
            bad,
            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            bytes.len(),
            sec.len(),
            st.blocks
        );
    }
}
