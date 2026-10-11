//! Deterministic fighting game simulation.
//!
//! Rules (see DESIGN.md "Determinism"):
//! - `i32` / `f32` only; transcendentals via `libm`, never `std` (`f32::sin` etc.)
//! - no `mul_add`
//! - no `HashMap` iteration; use `Vec`, `BTreeMap`, `IndexMap`
//! - no I/O, no time, no randomness outside [`Rng`]
//! - [`World`] is plain data: `Clone`, no `Rc`, no `Box<dyn>`. Immutable
//!   character assets live in [`CharData`] behind `Arc`, referenced by index.

#![forbid(unsafe_code)]
// Transcendentals and mul_add are banned through clippy.toml `disallowed-methods`.

pub mod anim;
pub mod chardata;
pub mod command;
pub mod context;
pub mod input;
pub mod player;
pub mod sctrl;
pub mod world;

pub use chardata::CharData;
pub use input::Buttons;
pub use player::Player;
pub use world::{Stage, World};

/// Deterministic PRNG living inside simulation state (xorshift64*).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % u64::from(n.max(1))) as u32
    }
    /// MUGEN `Random`: 0..=999.
    pub fn random(&mut self) -> i32 {
        self.below(1000) as i32
    }
    pub fn state(&self) -> u64 {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Load(String),
    #[error(transparent)]
    Format(#[from] rugen_formats::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_seed_same_sequence() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
