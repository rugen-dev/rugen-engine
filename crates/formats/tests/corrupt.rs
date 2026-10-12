//! Truncated or bit-flipped content files must produce an error or a partial
//! result, never a panic. Skipped when the reference tree is absent.

use std::path::PathBuf;

use rugen_formats::air::Air;
use rugen_formats::def::Def;
use rugen_formats::sff::Sff;
use rugen_formats::snd::Snd;

fn kfm(name: &str) -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../reference/mugen-1.1b1/chars/kfm")
        .join(name);
    std::fs::read(p).ok()
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn variants(bytes: &[u8], seed: u64) -> Vec<Vec<u8>> {
    let mut rng = Lcg(seed);
    let mut out = Vec::new();
    for k in 0..24 {
        let mut v = bytes.to_vec();
        match k % 3 {
            0 => {
                let cut = (rng.next() as usize) % bytes.len().max(1);
                v.truncate(cut);
            }
            1 => {
                for _ in 0..16 {
                    let i = (rng.next() as usize) % v.len().max(1);
                    if let Some(b) = v.get_mut(i) {
                        *b ^= 1 << (rng.next() % 8);
                    }
                }
            }
            _ => {
                for _ in 0..4 {
                    let i = (rng.next() as usize) % v.len().saturating_sub(4).max(1);
                    if i + 4 <= v.len() {
                        v[i..i + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
                    }
                }
            }
        }
        out.push(v);
    }
    out
}

#[test]
fn corrupt_sff_never_panics() {
    let Some(bytes) = kfm("kfm.sff") else { return };
    for v in variants(&bytes, 1) {
        let _ = Sff::parse(&v, true);
        let _ = Sff::parse(&v, false);
    }
}

#[test]
fn corrupt_snd_never_panics() {
    let Some(bytes) = kfm("kfm.snd") else { return };
    for v in variants(&bytes, 2) {
        let _ = Snd::parse(&v);
    }
}

#[test]
fn corrupt_text_files_never_panic() {
    for (name, seed) in [
        ("kfm.air", 3u64),
        ("kfm.def", 4),
        ("kfm.cns", 5),
        ("kfm.cmd", 6),
    ] {
        let Some(bytes) = kfm(name) else { continue };
        for v in variants(&bytes, seed) {
            let text = rugen_formats::text::decode(&v);
            let _ = Air::parse(&text);
            let _ = Def::parse(&text);
        }
    }
}
