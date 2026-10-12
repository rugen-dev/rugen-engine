//! Dump one sprite from an SFF to a PNG: `cargo run -p rugen-formats --example sffdump -- file.sff group number out.png [--char]`
use rugen_formats::sff::{Pixels, Sff};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let is_char = a.iter().any(|s| s == "--char");
    let sff = Sff::open(&a[1], is_char)?;
    let (g, n): (u16, u16) = (a[2].parse()?, a[3].parse()?);
    let s = sff.get(g, n).ok_or("no such sprite")?;
    let rgba: Vec<u8> = match s.decoded().as_ref() {
        Pixels::Rgba(v) => v.clone(),
        Pixels::Packed(_) => unreachable!(),
        Pixels::Indexed(px) => {
            let pal = &sff.palettes[s.palette.unwrap()];
            px.iter()
                .flat_map(|&i| pal.get(i as usize).copied().unwrap_or([0; 4]))
                .collect()
        }
    };
    let mut enc = png::Encoder::new(
        std::fs::File::create(&a[4])?,
        s.width as u32,
        s.height as u32,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    eprintln!(
        "{}x{} offset {:?} version {:?} sprites {} palettes {}",
        s.width,
        s.height,
        s.offset,
        sff.version,
        sff.sprites.len(),
        sff.palettes.len()
    );
    Ok(())
}
