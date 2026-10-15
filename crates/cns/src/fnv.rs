//! FNV-1a for the name tables the simulation looks up every frame (commands,
//! constants): a few bytes hashed in a handful of cycles where SipHash
//! pays its setup, and the same order on every machine (the tables are
//! never iterated by the simulation anyway).

use std::hash::{BuildHasher, Hasher};

#[derive(Clone, Copy, Debug, Default)]
pub struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

/// `BuildHasher` for [`Fnv`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FnvBuild;

impl BuildHasher for FnvBuild {
    type Hasher = Fnv;
    fn build_hasher(&self) -> Fnv {
        Fnv::default()
    }
}

pub type FnvHashMap<K, V> = std::collections::HashMap<K, V, FnvBuild>;
pub type FnvIndexMap<K, V> = indexmap::IndexMap<K, V, FnvBuild>;
