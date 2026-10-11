//! `cargo run -p rugen-formats --example sfflist -- file.sff` lists sprites.
fn main() {
    let path = std::env::args().nth(1).expect("sff path");
    let sff = rugen_formats::sff::Sff::open(&path, false).expect("open");
    println!("version {:?}, {} palettes", sff.version, sff.palettes.len());
    for ((g, n), s) in &sff.sprites {
        println!(
            "{g},{n}: {}x{} axis {:?} pal {:?}",
            s.width, s.height, s.offset, s.palette
        );
    }
}
