//! MUGEN bitmap fonts.
//!
//! * v1 `.fnt`: `ElecbyteFnt` header, one PCX strip and a text block with
//!   `[Def]` (Size, Spacing, Colors, Offset) and `[Map]` (char, x, width).
//!   Colour banks are slices of the PCX palette.
//! * v2 `.def`: `[Def]` with `Type = bitmap`, `File = x.sff`; glyphs are
//!   sprites `(0, char)` and banks are the SFF palettes.

use std::collections::HashMap;
use std::path::Path;

use crate::sff::{Palette, Pixels, Sff};
use crate::text;
use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct Glyph {
    pub width: u16,
    pub height: u16,
    /// Indexed pixels, `width * height`.
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Font {
    pub name: String,
    pub size: [u16; 2],
    pub spacing: [i32; 2],
    pub offset: [i32; 2],
    pub glyphs: HashMap<char, Glyph>,
    /// One palette per colour bank.
    pub banks: Vec<Palette>,
}

impl Font {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = crate::vfs::read(path)?;
        if data.starts_with(b"ElecbyteFnt\0") {
            Self::parse_v1(&data)
        } else {
            Self::parse_v2(path, &text::decode(&data))
        }
    }

    /// Width of `s` in pixels at scale 1.
    pub fn text_width(&self, s: &str) -> i32 {
        let mut w = 0;
        for (i, c) in s.chars().enumerate() {
            if i > 0 {
                w += self.spacing[0];
            }
            w += self.char_width(c);
        }
        w
    }

    pub fn char_width(&self, c: char) -> i32 {
        match self.glyphs.get(&c) {
            Some(g) => g.width as i32,
            None => self.size[0] as i32,
        }
    }

    fn parse_v1(data: &[u8]) -> Result<Self> {
        let bad = |m: &str| Error::fmt("fnt v1", m);
        let u32_at =
            |o: usize| -> u32 { u32::from_le_bytes(data[o..o + 4].try_into().unwrap_or([0; 4])) };
        if data.len() < 32 {
            return Err(bad("truncated header"));
        }
        let (pcx_ofs, pcx_len, txt_ofs, txt_len) = (
            u32_at(16) as usize,
            u32_at(20) as usize,
            u32_at(24) as usize,
            u32_at(28) as usize,
        );
        let pcx = data
            .get(pcx_ofs..pcx_ofs + pcx_len)
            .ok_or_else(|| bad("pcx out of range"))?;
        if pcx.len() < 128 + 769 {
            return Err(bad("pcx too small"));
        }
        let u16_at = |o: usize| u16::from_le_bytes([pcx[o], pcx[o + 1]]);
        let encoding = pcx[2];
        let (xmin, ymin, xmax, ymax) = (u16_at(4), u16_at(6), u16_at(8), u16_at(10));
        let w = xmax.wrapping_sub(xmin).wrapping_add(1);
        let h = ymax.wrapping_sub(ymin).wrapping_add(1);
        let bpl = u16_at(66) as usize;
        let rle = &pcx[128..pcx.len() - 769];
        let px = if encoding == 1 {
            crate::sff::pcx_decode(rle, w, h, bpl)
        } else {
            rle.to_vec()
        };
        let raw_pal = &pcx[pcx.len() - 768..];
        let mut pal: Palette = vec![[0u8; 4]; 256];
        for (i, c) in pal.iter_mut().enumerate() {
            *c = [
                raw_pal[i * 3],
                raw_pal[i * 3 + 1],
                raw_pal[i * 3 + 2],
                if i == 0 { 0 } else { 255 },
            ];
        }

        let txt = data
            .get(txt_ofs..txt_ofs + txt_len)
            .ok_or_else(|| bad("text out of range"))?;
        let txt = text::decode(txt);
        let mut f = Font {
            size: [w, h],
            ..Default::default()
        };
        let mut colors = 16;
        let mut section = String::new();
        // (char, start x, width)
        let mut map: Vec<(char, Option<i32>, Option<i32>)> = Vec::new();
        for line in txt.lines() {
            let line = text::strip_comment(line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                section = rest.trim_end_matches(']').trim().to_ascii_lowercase();
                continue;
            }
            match section.as_str() {
                "def" => {
                    if let Some((k, v)) = line.split_once('=') {
                        let v = v.trim();
                        let nums: Vec<i32> = v.split(',').map(|s| text::atoi(s.trim())).collect();
                        match k.trim().to_ascii_lowercase().as_str() {
                            "size" => {
                                f.size = [
                                    nums.first().copied().unwrap_or(w as i32).max(0) as u16,
                                    nums.get(1).copied().unwrap_or(h as i32).max(0) as u16,
                                ];
                            }
                            "spacing" => {
                                f.spacing = [
                                    nums.first().copied().unwrap_or(0),
                                    nums.get(1).copied().unwrap_or(0),
                                ];
                            }
                            "offset" => {
                                f.offset = [
                                    nums.first().copied().unwrap_or(0),
                                    nums.get(1).copied().unwrap_or(0),
                                ];
                            }
                            "colors" => colors = nums.first().copied().unwrap_or(16).clamp(1, 255),
                            _ => {}
                        }
                    }
                }
                "map" => {
                    let mut it = line.split_whitespace();
                    let Some(cs) = it.next() else { continue };
                    let c = if let Some(hex) =
                        cs.strip_prefix("0x").or_else(|| cs.strip_prefix("0X"))
                    {
                        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                            Some(c) => c,
                            None => continue,
                        }
                    } else {
                        match cs.chars().next() {
                            Some(c) => c,
                            None => continue,
                        }
                    };
                    let x = it.next().map(text::atoi);
                    let cw = it.next().map(text::atoi);
                    map.push((c, x, cw));
                }
                _ => {}
            }
        }
        // glyphs are consecutive `size.x` columns unless the map says otherwise
        let mut ofs: i32 = 0;
        for (c, x, cw) in map {
            if let Some(x) = x {
                ofs = x;
            }
            let mut gw = f.size[0] as i32;
            if let Some(cw) = cw {
                if cw < 0 {
                    ofs -= -cw;
                    gw = -cw;
                } else {
                    gw = cw;
                }
            }
            let gw = gw.max(0) as u16;
            let mut pixels = vec![0u8; gw as usize * h as usize];
            for row in 0..h as usize {
                for col in 0..gw as usize {
                    let sx = ofs + col as i32;
                    if sx >= 0 && (sx as usize) < w as usize {
                        pixels[row * gw as usize + col] = px[row * w as usize + sx as usize];
                    }
                }
            }
            f.glyphs.insert(
                c,
                Glyph {
                    width: gw,
                    height: h,
                    pixels,
                },
            );
            ofs += gw as i32;
        }
        // colour banks: slices of the palette, `colors` entries each, from the top
        let c = ((colors + 15) / 16 * 16).min(255) as usize;
        let nbanks = (255 / c).max(1);
        for i in 0..nbanks {
            let mut p = pal.clone();
            let lo = 256 - c * (i + 1);
            let hi = 256 - c * i;
            p[256 - c..256].copy_from_slice(&pal[lo..hi]);
            f.banks.push(p);
        }
        Ok(f)
    }

    fn parse_v2(path: &Path, text: &str) -> Result<Self> {
        let def = crate::def::Def::parse(text);
        let d = def
            .section("def")
            .ok_or_else(|| Error::fmt("font def", "missing [Def]"))?;
        let ty = d.str("type").unwrap_or("bitmap").to_ascii_lowercase();
        if ty != "bitmap" {
            return Err(Error::fmt(
                "font def",
                format!("font type {ty:?} not supported"),
            ));
        }
        let pair = |k: &str, a: i32, b: i32| -> [i32; 2] {
            let v = d.i32s(k).unwrap_or_default();
            [
                v.first().copied().unwrap_or(a),
                v.get(1).copied().unwrap_or(b),
            ]
        };
        let size = pair("size", 0, 0);
        let mut f = Font {
            name: def
                .section("fnt v2")
                .and_then(|s| s.str("name"))
                .unwrap_or("")
                .trim_matches('"')
                .to_string(),
            size: [size[0].max(0) as u16, size[1].max(0) as u16],
            spacing: pair("spacing", 0, 0),
            offset: pair("offset", 0, 0),
            ..Default::default()
        };
        let file = d
            .str("file")
            .ok_or_else(|| Error::fmt("font def", "missing file"))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let sff_path = resolve(dir, file).unwrap_or_else(|| dir.join(file));
        let sff = Sff::open(&sff_path, false)?;
        for ((group, number), spr) in &sff.sprites {
            if *group != 0 {
                continue;
            }
            let Some(c) = char::from_u32(*number as u32) else {
                continue;
            };
            let decoded = spr.decoded();
            let Pixels::Indexed(px) = decoded.as_ref() else {
                continue;
            };
            f.glyphs.insert(
                c,
                Glyph {
                    width: spr.width,
                    height: spr.height,
                    pixels: px.clone(),
                },
            );
        }
        // banks: palettes (0, i) in order; fall back to the first palette
        let mut keys: Vec<(&(u16, u16), &usize)> = sff
            .palette_keys
            .iter()
            .filter(|((g, _), _)| *g == 0)
            .collect();
        keys.sort_by_key(|((_, n), _)| *n);
        for (_, idx) in keys {
            if let Some(p) = sff.palettes.get(*idx) {
                f.banks.push(p.clone());
            }
        }
        if f.banks.is_empty() {
            f.banks = sff.palettes.clone();
        }
        if f.banks.is_empty() {
            f.banks.push(vec![[255, 255, 255, 255]; 256]);
        }
        Ok(f)
    }
}

/// Case-insensitive file lookup in `dir`.
fn resolve(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    let want = Path::new(name)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == want)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn font_dir() -> Option<PathBuf> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1/font");
        p.exists().then_some(p)
    }

    #[test]
    fn v1_jg() {
        let Some(dir) = font_dir() else { return };
        let f = Font::open(dir.join("jg.fnt")).expect("jg.fnt");
        assert_eq!(f.size, [8, 8]);
        assert!(f.glyphs.contains_key(&'A'), "has 'A'");
        assert!(f.banks.len() > 1, "16-colour font has several banks");
        assert!(f.text_width("ABC") > 0);
    }

    #[test]
    fn v2_num1() {
        let Some(dir) = font_dir() else { return };
        let f = Font::open(dir.join("num1.def")).expect("num1.def");
        assert_eq!(f.size, [9, 13]);
        assert!(f.glyphs.contains_key(&'0'));
        assert!(f.banks.len() >= 8, "8 banks, got {}", f.banks.len());
    }
}
