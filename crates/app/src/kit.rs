//! Art for the front end: CC0 kit pieces (Kenney "UI Pack - Pixel Adventure",
//! `motif/kit/`) recoloured onto the rugen palette, plus pixel backgrounds,
//! the logo and feature art drawn with real pixel technique (dithered
//! gradients, 1-px rims, palette only). Everything is drawn at 1x on a
//! 640x360 grid and shown at 2x by the screens.

use std::path::Path;

use crate::pixelize::{self, BLACK, BLUE, GOLD, GREEN, MAROON, NAVY, RED, Rgba, STEEL};
use crate::screenpack::Canvas;

const KIT: &str = "kit/kenney-pixel-adventure";

/// Load a kit tile (`large/tile_0003.png` → index 3 of the 32x32 set).
pub fn tile(motif: &Path, set: &str, index: u32) -> anyhow::Result<Canvas> {
    let p = motif
        .join(KIT)
        .join(set)
        .join(format!("tile_{index:04}.png"));
    let (w, h, rgba) = read_png_rgba(&p)
        .map_err(|e| anyhow::anyhow!("{}: {e} (is the CC0 kit in motif/kit?)", p.display()))?;
    Ok(Canvas::from_rgba(w, h, rgba))
}

/// Decode any PNG (palette, grey, 16-bit...) to straight 8-bit RGBA.
pub fn read_png_rgba(path: &Path) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => anyhow::bail!("palette not expanded"),
    };
    anyhow::ensure!(
        rgba.len() == (w * h * 4) as usize,
        "unexpected pixel buffer size"
    );
    Ok((w, h, rgba))
}

fn clone(c: &Canvas) -> Canvas {
    Canvas::from_rgba(c.w, c.h, c.px.clone())
}

/// Expand a tile into `w x h` keeping its corners: edges and the middle are
/// repeated (not stretched) so the pixel look stays crisp.
#[allow(dead_code)]
pub fn nine_slice(tile: &Canvas, inset: u32, w: u32, h: u32) -> Canvas {
    let (tw, th) = (tile.w as i32, tile.h as i32);
    let i = inset as i32;
    let mut out = Canvas::new(w, h);
    let (w, h) = (w as i32, h as i32);
    let map = |v: i32, size: i32, tsize: i32| -> i32 {
        if v < i {
            v
        } else if v >= size - i {
            tsize - (size - v)
        } else {
            i + (v - i) % (tsize - 2 * i).max(1)
        }
    };
    for y in 0..h {
        let sy = map(y, h, th);
        for x in 0..w {
            let sx = map(x, w, tw);
            out.put(x, y, tile.get(sx, sy));
        }
    }
    out
}

/// Shear into a parallelogram: row `y` shifts right by `skew * (h-1-y) / (h-1)`.
#[allow(dead_code)]
pub fn shear(c: &Canvas, skew: u32) -> Canvas {
    let (w, h) = (c.w as i32, c.h as i32);
    let mut out = Canvas::new(c.w + skew, c.h);
    for y in 0..h {
        let dx = skew as i32 * (h - 1 - y) / (h - 1).max(1);
        for x in 0..w {
            out.put(x + dx, y, c.get(x, y));
        }
    }
    out
}

fn remapped(
    motif: &Path,
    set: &str,
    index: u32,
    ramp: &[Rgba],
    bias: f32,
) -> anyhow::Result<Canvas> {
    let mut t = tile(motif, set, index)?;
    pixelize::remap(&mut t, ramp, bias);
    Ok(t)
}

/// Verification badge 7x7: diamond with a black rim.
pub fn badge(color: Rgba) -> Canvas {
    let mut c = Canvas::new(7, 7);
    for y in 0..7 {
        let half = 3 - (y - 3i32).abs();
        c.rect(3 - half, y, half * 2 + 1, 1, color);
    }
    c.put(3, 1, [255, 255, 255, 200]);
    c.outline(BLACK);
    c
}

// ---------------------------------------------------------------- kit export

/// Recolour every kit tile onto the palette and write `<theme>/ui/kit/*.webp`
/// for modders (plus the role assets built by `ui-gen`).
pub fn import(motif: &Path, theme: &Path) -> anyhow::Result<usize> {
    let out = theme.join("ui").join("kit");
    std::fs::create_dir_all(&out)?;
    std::fs::write(motif.join("palette.gpl"), pixelize::gpl())?;
    let mut n = 0;
    let large = [
        (1, "block", "maroon", &MAROON[..]),
        (2, "flat", "navy", &NAVY[..]),
        (3, "bevel", "navy", &NAVY[..]),
        (8, "frame", "steel", &STEEL[..]),
        (9, "frame2", "steel", &STEEL[..]),
        (32, "hollow", "gold", &GOLD[..]),
        (41, "circle", "steel", &STEEL[..]),
        (42, "circle_dark", "navy", &NAVY[..]),
        (43, "banner_l", "maroon", &MAROON[..]),
        (44, "banner_m", "maroon", &MAROON[..]),
        (45, "banner_r", "maroon", &MAROON[..]),
        (64, "hex", "gold", &GOLD[..]),
        (66, "hex_dark", "navy", &NAVY[..]),
        (67, "hex_steel", "steel", &STEEL[..]),
        (79, "hex_framed", "gold", &GOLD[..]),
        (82, "bar_l", "gold", &GOLD[..]),
        (83, "bar_m", "gold", &GOLD[..]),
        (84, "bar_r", "gold", &GOLD[..]),
    ];
    for (idx, name, ramp_name, ramp) in large {
        let c = remapped(motif, "large", idx, ramp, 0.0)?;
        pixelize::write_webp(&out.join(format!("{name}_{ramp_name}.webp")), &c)?;
        n += 1;
    }
    for idx in [
        3u32, 4, 8, 9, 24, 25, 26, 35, 36, 37, 38, 44, 45, 56, 57, 58,
    ] {
        let Ok(c) = remapped(motif, "small", idx, &STEEL, 0.0) else {
            continue;
        };
        pixelize::write_webp(&out.join(format!("icon_{idx:02}.webp")), &c)?;
        n += 1;
    }
    // extra recolours modders asked for
    for (idx, name, ramp) in [
        (66u32, "hex_green", &GREEN[..]),
        (66, "hex_red", &RED[..]),
        (66, "hex_blue", &BLUE[..]),
    ] {
        let c = remapped(motif, "large", idx, ramp, 0.0)?;
        pixelize::write_webp(&out.join(format!("{name}.webp")), &c)?;
        n += 1;
    }
    let _ = clone;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_slice_keeps_corners_and_repeats_middle() {
        let mut t = Canvas::new(8, 8);
        t.rect(0, 0, 8, 8, [1, 1, 1, 255]);
        t.rect(0, 0, 1, 1, [9, 9, 9, 255]);
        t.rect(3, 3, 2, 2, [5, 5, 5, 255]);
        let n = nine_slice(&t, 3, 20, 10);
        assert_eq!(n.get(0, 0)[0], 9);
        assert_eq!(n.get(19, 9)[0], 1);
        assert_eq!(n.get(10, 4)[0], 5);
    }

    #[test]
    fn shear_widens_by_skew() {
        let mut t = Canvas::new(4, 4);
        t.rect(0, 0, 4, 4, [1, 1, 1, 255]);
        let s = shear(&t, 3);
        assert_eq!(s.w, 7);
        assert_eq!(s.get(3, 0)[3], 255);
        assert_eq!(s.get(0, 0)[3], 0);
        assert_eq!(s.get(0, 3)[3], 255);
    }
}
