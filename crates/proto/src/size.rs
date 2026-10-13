//! FIGHTER SIZE: the option, its height bands and round zoom steps.
//!
//! Shared by the simulation (`rugen_sim::scale`, which measures fighters and
//! zooms them), the fairness verdict (which weighs the sizes fighters end
//! up at) and the rooms' rules.

use serde::{Deserialize, Serialize};

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

/// A standing height in 320x240 units from the axis y of a fighter's
/// standing sprite, its `[Size] yscale` and its localcoord width; `None`
/// when flat or not finite.
pub fn height_from_axis(axis_y: f32, yscale: f32, localcoord_w: f32) -> Option<f32> {
    let h = axis_y * yscale * 320.0 / localcoord_w.max(1.0);
    (h.is_finite() && h > 1.0).then_some(h)
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
        let names: Vec<&str> = FighterSize::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, crate::FIGHTER_SIZES, "the rooms' rule values");
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
        assert_eq!(height_from_axis(105.0, 1.0, 320.0), Some(105.0));
        assert_eq!(height_from_axis(210.0, 1.0, 640.0), Some(105.0));
        assert_eq!(height_from_axis(0.0, 1.0, 320.0), Option::None);
    }
}
