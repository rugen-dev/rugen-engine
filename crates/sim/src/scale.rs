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

use serde::{Deserialize, Serialize};

use crate::CharData;

/// The `[game] fighter_size` choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FighterSize {
    /// Every fighter keeps its own size.
    None,
    /// Band 44-56 units tall (320x240 space).
    Chibi,
    /// Band 62-78.
    Small,
    /// Band 58-82.
    #[default]
    Normal,
}

/// The round zoom steps 1.1, 1.2 .. 3.0: a fighter is multiplied or divided
/// by one of them.
pub fn steps() -> impl Iterator<Item = f32> {
    (11..=30u8).map(|k| f32::from(k) / 10.0)
}

impl FighterSize {
    pub const ALL: [Self; 4] = [Self::None, Self::Chibi, Self::Small, Self::Normal];

    /// Standing heights (320x240 units) a fighter keeps as they are;
    /// `None` leaves every fighter alone.
    pub fn band(self) -> Option<(f32, f32)> {
        match self {
            Self::None => None,
            Self::Chibi => Some((44.0, 56.0)),
            Self::Small => Some((62.0, 78.0)),
            Self::Normal => Some((58.0, 82.0)),
        }
    }

    /// The config / wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Chibi => "chibi",
            Self::Small => "small",
            Self::Normal => "normal",
        }
    }

    /// From the config / wire name (any case); `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Chibi => "CHIBI",
            Self::Small => "SMALL",
            Self::Normal => "NORMAL",
        }
    }

    /// One byte for handshakes: the index in [`Self::ALL`].
    pub fn to_byte(self) -> u8 {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(3) as u8
    }

    pub fn from_byte(b: u8) -> Option<Self> {
        Self::ALL.get(usize::from(b)).copied()
    }
}

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
    let h = y * yscale * 320.0 / d.localcoord[0].max(1.0);
    (h.is_finite() && h > 1.0).then_some(h)
}

/// How much `size` zooms fighter `d` (1 = unchanged); see [`zoom_for`].
pub fn factor(d: &CharData, size: FighterSize) -> f32 {
    standing_height(d).map_or(1.0, |h| zoom_for(h, size))
}

/// The zoom for standing height `h`: 1 inside the band, else the smallest
/// [`steps`] multiplier (below the band) or divider (above it) that lands in
/// the band; when none does, the one that gets closest.
pub fn zoom_for(h: f32, size: FighterSize) -> f32 {
    let Some((lo, hi)) = size.band() else {
        return 1.0;
    };
    if (lo..=hi).contains(&h) {
        return 1.0;
    }
    let zooms: Vec<f32> = steps().map(|s| if h < lo { s } else { 1.0 / s }).collect();
    let miss = |z: f32| {
        let v = h * z;
        if v < lo {
            lo - v
        } else if v > hi {
            v - hi
        } else {
            0.0
        }
    };
    // first strictly better wins, so ties go to the step closest to 1
    let mut best = zooms[0];
    for &z in &zooms {
        if miss(z) < miss(best) {
            best = z;
        }
        if miss(best) == 0.0 {
            break;
        }
    }
    best
}

/// `x1.3`, `/1.3` or `x1` for logs.
pub fn describe(zoom: f32) -> String {
    if zoom == 1.0 {
        "x1".into()
    } else if zoom > 1.0 {
        format!("x{zoom:.1}")
    } else {
        format!("/{:.1}", 1.0 / zoom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in FighterSize::ALL {
            assert_eq!(FighterSize::parse(s.as_str()), Some(s));
            assert_eq!(FighterSize::from_byte(s.to_byte()), Some(s));
        }
        assert_eq!(FighterSize::parse(" Chibi "), Some(FighterSize::Chibi));
        assert_eq!(FighterSize::parse("huge"), None);
        assert_eq!(FighterSize::from_byte(9), None);
        assert_eq!(FighterSize::default(), FighterSize::Normal);
    }

    #[test]
    fn round_steps_into_the_band() {
        use FighterSize::*;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let div = |s: f32| 1.0 / s;
        // normal 58-82: Naruto 66 / Sasuke 75 stay, Goku 46 x1.3 = 59.8, KFM 105 /1.3 = 80.8
        assert_eq!(zoom_for(66.0, Normal), 1.0);
        assert_eq!(zoom_for(75.0, Normal), 1.0);
        assert!(
            close(zoom_for(46.0, Normal), 1.3),
            "x1.2 = 55.2 is still short"
        );
        assert!(
            close(zoom_for(105.0, Normal), div(1.3)),
            "/1.2 = 87.5 is still tall"
        );
        // small 62-78
        assert!(close(zoom_for(46.0, Small), 1.4));
        assert_eq!(zoom_for(66.0, Small), 1.0);
        assert!(close(zoom_for(105.0, Small), div(1.4)));
        // chibi 44-56
        assert!(close(zoom_for(66.0, Chibi), div(1.2)));
        assert!(close(zoom_for(75.0, Chibi), div(1.4)));
        assert_eq!(zoom_for(46.0, Chibi), 1.0);
        assert!(close(zoom_for(105.0, Chibi), div(1.9)));
        // nothing lands: the closest step (a speck grows by 3, a giant shrinks by 3)
        assert!(close(zoom_for(10.0, Normal), 3.0));
        assert!(close(zoom_for(400.0, Normal), div(3.0)));
        assert_eq!(zoom_for(46.0, None), 1.0);
        assert_eq!(steps().count(), 20);
        assert_eq!(describe(zoom_for(46.0, Normal)), "x1.3");
        assert_eq!(describe(zoom_for(105.0, Normal)), "/1.3");
        assert_eq!(describe(1.0), "x1");
    }
}
