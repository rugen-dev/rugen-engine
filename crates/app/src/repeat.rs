//! Held directions repeat in menus: the press itself moves once, then after
//! [`MENU`]'s delay the direction fires again every few ticks while it is
//! held (keyboard and pads alike: it works on the merged buttons). The
//! shell applies it to every menu; the stage strip runs its own copy with a
//! ramp to a faster rate for long lists.

use rugen_sim::Buttons;

/// When a held direction fires again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    /// Ticks held before the first repeat.
    pub delay: u32,
    /// Ticks between repeats.
    pub every: u32,
    /// Held this many ticks, repeat every `fast` ticks instead (0 = never).
    pub ramp: u32,
    pub fast: u32,
}

/// Menus: first repeat after 0.4 s, then every 0.08 s.
pub const MENU: Rate = Rate {
    delay: 24,
    every: 5,
    ramp: 0,
    fast: 0,
};

/// One player's held directions (up, down, left, right).
#[derive(Clone, Debug)]
pub struct Repeat {
    rate: Rate,
    held: [u32; 4],
}

impl Default for Repeat {
    fn default() -> Self {
        Self::new(MENU)
    }
}

impl Repeat {
    pub fn new(rate: Rate) -> Self {
        Self { rate, held: [0; 4] }
    }

    /// One tick with `held` buttons down: the directions that repeat this
    /// tick (never the first press, which the caller already has as an
    /// edge). Opposite directions held together cancel out.
    pub fn tick(&mut self, held: Buttons) -> Buttons {
        let dirs = [
            held.u && !held.d,
            held.d && !held.u,
            held.l && !held.r,
            held.r && !held.l,
        ];
        let mut fire = [false; 4];
        for (k, on) in dirs.into_iter().enumerate() {
            if !on {
                self.held[k] = 0;
                continue;
            }
            self.held[k] = self.held[k].saturating_add(1);
            let t = self.held[k];
            let r = self.rate;
            let every = if r.fast > 0 && t >= r.ramp {
                r.fast
            } else {
                r.every.max(1)
            };
            // t = 1 is the press itself
            fire[k] = t > r.delay && (t - 1 - r.delay) % every == 0;
        }
        Buttons {
            u: fire[0],
            d: fire[1],
            l: fire[2],
            r: fire[3],
            ..Default::default()
        }
    }

    /// Forget what was held (a new screen).
    pub fn clear(&mut self) {
        self.held = [0; 4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down() -> Buttons {
        Buttons {
            d: true,
            ..Default::default()
        }
    }

    #[test]
    fn menus_repeat_after_the_delay() {
        let mut r = Repeat::default();
        let fired: Vec<u32> = (1..=60).filter(|_| r.tick(down()).d).collect();
        // held from tick 1: first repeat 24 ticks (0.4 s) after the press,
        // then every 5 (0.08 s)
        assert_eq!(fired, vec![25, 30, 35, 40, 45, 50, 55, 60]);
        // let go: it starts over
        assert!(!r.tick(Buttons::default()).d);
        assert!((1..=24).all(|_| !r.tick(down()).d));
        assert!(r.tick(down()).d);
    }

    #[test]
    fn opposite_directions_cancel_and_ramps_speed_up() {
        let mut r = Repeat::default();
        let both = Buttons {
            l: true,
            r: true,
            ..Default::default()
        };
        assert!((0..100).all(|_| r.tick(both) == Buttons::default()));
        let mut strip = Repeat::new(Rate {
            delay: 16,
            every: 5,
            ramp: 31,
            fast: 2,
        });
        let right = Buttons {
            r: true,
            ..Default::default()
        };
        let fired: Vec<u32> = (1..=40).filter(|_| strip.tick(right).r).collect();
        assert_eq!(fired, vec![17, 22, 27, 31, 33, 35, 37, 39]);
    }
}
