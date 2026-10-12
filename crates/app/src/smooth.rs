//! Visual correction smoothing for rollback netplay (display only, never
//! touches the simulation).
//!
//! When a rollback changes where a fighter or projectile is on the frame
//! that was already on screen, the picture would jump. Instead the
//! renderer keeps drawing it where it was and eases it onto the corrected
//! position over a few frames: the error (what was shown minus what the
//! corrected timeline says for that same frame) becomes a display offset
//! that decays to zero in ~4 frames. Large jumps (teleports, a hit the
//! prediction missed) snap at once: easing those would show a fighter
//! somewhere it never was for longer.

use std::collections::HashMap;

use rugen_sim::World;

/// Identity of a drawn entity across rollbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A player or helper (`Player::id`).
    Player(i32),
    /// A projectile: owner id, projectile id, n-th of that id.
    Proj(i32, i32, u16),
}

/// Errors above this many pixels snap instead of easing.
pub const SNAP: f32 = 48.0;
/// Offset kept per frame (0.55^4 ~ 0.09: gone in about 4 frames).
pub const DECAY: f32 = 0.55;

/// One entity's position and whether it must snap (hit / teleport states).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pos {
    pub key: Key,
    pub pos: [f32; 2],
    pub snap: bool,
}

/// Positions of every smoothed entity in `w` (stage space, unscaled).
pub fn positions(w: &World) -> Vec<Pos> {
    let mut out = Vec::with_capacity(w.players.len() + w.projs.len());
    for p in w.players.iter().filter(|p| !p.removed) {
        out.push(Pos {
            key: Key::Player(p.id),
            pos: p.pos,
            // getting hit: the hit itself was the misprediction; show it now
            snap: p.move_type == rugen_cns::state::MoveType::H,
        });
    }
    for (pr, key) in w.projs.iter().zip(proj_keys(w)) {
        out.push(Pos {
            key,
            pos: pr.pos,
            snap: false,
        });
    }
    out
}

/// The key of every projectile of `w`, by index.
pub fn proj_keys(w: &World) -> Vec<Key> {
    let mut seen: HashMap<(i32, i32), u16> = HashMap::new();
    w.projs
        .iter()
        .map(|pr| {
            let n = seen.entry((pr.owner_id, pr.id)).or_default();
            *n += 1;
            Key::Proj(pr.owner_id, pr.id, *n - 1)
        })
        .collect()
}

#[derive(Default)]
pub struct Smoother {
    offsets: HashMap<Key, [f32; 2]>,
    /// Corrections applied (for the overlay / tests).
    pub corrections: u64,
}

impl Smoother {
    /// Display offset of an entity (add to its simulated position).
    pub fn offset(&self, key: Key) -> [f32; 2] {
        self.offsets.get(&key).copied().unwrap_or([0.0, 0.0])
    }

    /// One displayed tick. `shown` = positions drawn last (simulated, before
    /// this tick's advance, without offsets); `corrected` = the same frame
    /// after the rollback re-simulated it, when this tick rolled back.
    pub fn tick(&mut self, shown: &[Pos], corrected: Option<&[Pos]>) {
        // older offsets fade
        self.offsets.retain(|_, o| {
            o[0] *= DECAY;
            o[1] *= DECAY;
            o[0].abs() + o[1].abs() > 0.25
        });
        let Some(corrected) = corrected else {
            return;
        };
        for c in corrected {
            let Some(s) = shown.iter().find(|s| s.key == c.key) else {
                continue;
            };
            let prev = self.offset(c.key);
            // what was on screen minus what should have been there
            let err = [
                s.pos[0] + prev[0] / DECAY - c.pos[0],
                s.pos[1] + prev[1] / DECAY - c.pos[1],
            ];
            let size = err[0].abs().max(err[1].abs());
            if c.snap || s.snap || size > SNAP {
                self.offsets.remove(&c.key);
            } else if size > 0.5 {
                self.offsets.insert(c.key, err);
                self.corrections += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: i32, x: f32) -> Pos {
        Pos {
            key: Key::Player(id),
            pos: [x, 0.0],
            snap: false,
        }
    }

    #[test]
    fn a_small_correction_eases_in_over_a_few_frames() {
        let mut s = Smoother::default();
        // shown at x = 100, the rollback says it was at 90 on that frame
        s.tick(&[p(1, 100.0)], Some(&[p(1, 90.0)]));
        let o = s.offset(Key::Player(1))[0];
        assert!((o - 10.0).abs() < 1e-4, "{o}");
        let mut seen = vec![o];
        for _ in 0..8 {
            s.tick(&[p(1, 90.0)], None);
            seen.push(s.offset(Key::Player(1))[0]);
        }
        assert!(seen.windows(2).all(|w| w[1] < w[0] || w[1] == 0.0));
        assert!(seen[4] < 1.0, "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 0.0);
    }

    #[test]
    fn big_jumps_and_hits_snap() {
        let mut s = Smoother::default();
        s.tick(&[p(1, 0.0)], Some(&[p(1, 200.0)]));
        assert_eq!(s.offset(Key::Player(1)), [0.0, 0.0]);
        let mut hit = p(2, 30.0);
        hit.snap = true;
        s.tick(&[p(2, 40.0)], Some(&[hit]));
        assert_eq!(s.offset(Key::Player(2)), [0.0, 0.0]);
        // no correction, nothing changes; unknown entities are left alone
        s.tick(&[p(3, 5.0)], Some(&[p(4, 9.0)]));
        assert_eq!(s.corrections, 0);
    }

    #[test]
    fn a_second_correction_accounts_for_the_offset_on_screen() {
        let mut s = Smoother::default();
        s.tick(&[p(1, 100.0)], Some(&[p(1, 90.0)]));
        // next tick: drawn at 95 (sim) + offset; the rollback moves it again
        s.tick(&[p(1, 95.0)], Some(&[p(1, 95.0)]));
        // the screen showed 95 + 10: the corrected 95 eases in from there
        let o = s.offset(Key::Player(1))[0];
        assert!((o - 10.0).abs() < 1e-3, "{o}");
    }

    #[test]
    fn smoothing_never_touches_the_simulation() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1");
        let Ok(data) = rugen_sim::CharData::load_with_data(
            &root.join("chars/kfm/kfm.def"),
            &root.join("data"),
        ) else {
            return;
        };
        let mut w = World::new(rugen_sim::Stage::default(), 3);
        w.add_player(data.clone(), 0, 1);
        w.add_player(data, 1, 2);
        w.start_round();
        let mut reference = w.clone();
        let mut s = Smoother::default();
        let input = [rugen_sim::Buttons::from_bits(0b1000); 2];
        for _ in 0..120 {
            let before = positions(&w);
            w.step(&input);
            let after = positions(&w);
            s.tick(&before, Some(&after));
            reference.step(&input);
        }
        assert_eq!(w.checksum(), reference.checksum());
    }
}
