//! FIGHTER SIZE: fighters zoomed by a round step into a common height band.
//!
//! Fighters are drawn in their own art's size, so a 46-unit Goku stands next
//! to a 105-unit Kung Fu Man. The option measures each fighter's standing
//! height and zooms the whole fighter (sprites, Clsn, velocities, explods,
//! projectiles, offsets) by a round step (x1.1 .. x3.0 or /1.1 .. /3.0) into
//! the option's height band (fighters inside it keep their size), the way Ikemen's character
//! `localcoord` already zooms one: the fighter acts as if authored at
//! `localcoord / factor` ([`crate::World::local_scale`]). Constants stay in
//! the authored units, so the character's own logic is untouched.
//!
//! It changes the simulation: replays record it and online peers share it.

pub use rugen_proto::size::{FighterSize, describe, height_from_axis, steps, zoom_for};

use crate::CharData;

/// A fighter's standing height in 320x240 units: the axis y of sprite 0,0
/// (else of the first frame of action 0), times its `[Size] yscale`, in its
/// authored localcoord. `None` when neither sprite exists or it is flat.
pub fn standing_height(d: &CharData) -> Option<f32> {
    let axis_y = |g: u16, n: u16| d.sff.get(g, n).map(|s| f32::from(s.offset[1]));
    let y = axis_y(0, 0).filter(|y| *y > 0.0).or_else(|| {
        let f = d.air.get(0)?.frames.first()?;
        axis_y(u16::try_from(f.group).ok()?, u16::try_from(f.number).ok()?)
    })?;
    let yscale = d.konst("size.yscale").unwrap_or(1.0);
    height_from_axis(y, yscale, d.localcoord[0])
}

/// How much `size` zooms fighter `d` (1 = unchanged); see [`zoom_for`].
pub fn factor(d: &CharData, size: FighterSize) -> f32 {
    standing_height(d).map_or(1.0, |h| zoom_for(h, size))
}
