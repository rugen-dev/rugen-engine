//! Deterministic fighting game simulation.
//!
//! Rules (see DESIGN.md "Determinism"):
//! - `i32` / `f32` only; transcendentals via `libm`, never `std` (`f32::sin` etc.)
//! - no `mul_add`
//! - no `HashMap` iteration; use `Vec`, `BTreeMap`, `IndexMap`
//! - no I/O, no time, no randomness outside [`Rng`]
//! - state must be plain data: `Clone`, no `Rc`, no `Box<dyn>`

#![forbid(unsafe_code)]
#![deny(clippy::suboptimal_flops, clippy::imprecise_flops)]

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
    /// Uniform in `0..n` (n > 0), like MUGEN's `Random` trigger range helper.
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % u64::from(n)) as u32
    }
}

/// Whole-match state. Everything rollback snapshots is in here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub frame: u32,
    pub rng: Rng,
}

impl State {
    pub fn new(seed: u64) -> Self {
        Self {
            frame: 0,
            rng: Rng::new(seed),
        }
    }
    /// Advance one 60 Hz tick.
    pub fn step(&mut self) {
        self.frame += 1;
    }
    /// Cheap hash for desync detection across peers.
    pub fn checksum(&self) -> u64 {
        self.frame as u64 ^ self.rng.0.rotate_left(17)
    }
}

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
    #[test]
    fn step_is_deterministic() {
        let (mut a, mut b) = (State::new(7), State::new(7));
        for _ in 0..100 {
            a.step();
            b.step();
        }
        assert_eq!(a, b);
        assert_eq!(a.checksum(), b.checksum());
    }
}
