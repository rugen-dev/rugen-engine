//! ACT palettes: 256 RGB triples stored last-index-first (Photoshop order).
//! Index 0 is transparent. Short files fill what they have.

use crate::sff::Palette;

pub fn parse(bytes: &[u8]) -> Palette {
    let mut pal = vec![[0u8; 4]; 256];
    for (i, rgb) in bytes.chunks_exact(3).take(256).enumerate() {
        let dst = 255 - i;
        pal[dst] = [rgb[0], rgb[1], rgb[2], if dst == 0 { 0 } else { 255 }];
    }
    pal
}

pub fn open(path: impl AsRef<std::path::Path>) -> crate::Result<Palette> {
    Ok(parse(&crate::vfs::read(path.as_ref())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversed_order() {
        let mut bytes = Vec::new();
        for i in 0..256u32 {
            bytes.extend_from_slice(&[i as u8, 0, 0]);
        }
        let pal = parse(&bytes);
        assert_eq!(pal[255], [0, 0, 0, 255], "first triple lands at index 255");
        assert_eq!(
            pal[0],
            [255, 0, 0, 0],
            "last triple lands at index 0, transparent"
        );
        assert_eq!(pal.len(), 256);
        assert_eq!(parse(&bytes[..6])[254], [1, 0, 0, 255]);
    }

    #[test]
    fn kfm_act() {
        let Some(p) = crate::fixtures::reference("ikemen-go-v1.0.0/fightfx/1-1.act") else {
            return;
        };
        let pal = open(p).unwrap();
        assert_eq!(pal[0][3], 0);
        assert!(
            pal.iter()
                .skip(1)
                .any(|c| c[3] == 255 && (c[0] | c[1] | c[2]) != 0)
        );
    }
}
