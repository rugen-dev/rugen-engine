//! Matchup verdict from two fairness profiles (instant, on the server).
//!
//! * **BLOCKED** when either side has a block-level cheat flag or is broken.
//! * **UNBALANCED** when the power-score ratio exceeds [`Thresholds::ratio`]
//!   or a single stat gap is extreme (life / attack / defence ×2).
//! * Community ratings (per fingerprint, from verified results) refine it
//!   once both sides have [`Thresholds::min_games`]: a rating gap above
//!   [`Thresholds::community_gap`] is UNBALANCED whatever the scores say,
//!   a small gap overrides a power-score-only UNBALANCED (the community
//!   has shown the fight is even).
//! * FIGHTER SIZE: the power scores are content only, so the verdict
//!   weighs the standing heights both fighters end up at under the match's
//!   size option (reach, speed and target all grow with the zoom): the
//!   score ratio is multiplied by `(taller / shorter) ^` [`HEIGHT_WEIGHT`]
//!   when both profiles know their height.
//! * Otherwise **FAIR**.

use rugen_proto::size::{FighterSize, zoom_for};
use rugen_proto::{CharRating, FairnessProfile, Severity, Verdict, VerdictKind};

/// How strongly the height ratio of the two fighters (as FIGHTER SIZE
/// leaves them) moves the power-score ratio: x1.3 against /1.3 (1.69x as
/// tall) makes the taller one 1.3x stronger. A bigger fighter reaches and
/// moves further but also offers a bigger target, hence below 1.
pub const HEIGHT_WEIGHT: f32 = 0.5;

/// The heights both fighters stand at under `size`, when both are known.
pub fn sized_heights(profiles: [&FairnessProfile; 2], size: FighterSize) -> Option<[f32; 2]> {
    let h = |p: &FairnessProfile| p.height.map(|h| h * zoom_for(h, size));
    Some([h(profiles[0])?, h(profiles[1])?])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Stronger / weaker power score above which the fight is unbalanced.
    pub ratio: f32,
    /// Community rating difference that makes a fight unbalanced.
    pub community_gap: i32,
    /// Games each fingerprint needs before its community rating counts.
    pub min_games: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            ratio: 1.6,
            community_gap: 200,
            min_games: 10,
        }
    }
}

/// `names` label the reasons (`["NARUTO", "SASUKE"]`); `size` is the
/// match's FIGHTER SIZE (a room's `rules.fighter_size`).
pub fn verdict(
    profiles: [&FairnessProfile; 2],
    ratings: [Option<&CharRating>; 2],
    names: [&str; 2],
    size: FighterSize,
    t: &Thresholds,
) -> Verdict {
    let mut blocked = Vec::new();
    for (p, name) in profiles.iter().zip(names) {
        for f in p.flags.iter().filter(|f| f.severity == Severity::Block) {
            blocked.push(format!("{name}: {}", f.detail));
        }
        if p.verify.as_deref() == Some("broken") {
            blocked.push(format!("{name}: fails rugen verify (broken)"));
        }
    }
    let (mut a, mut b) = (
        profiles[0].power_score.max(1.0),
        profiles[1].power_score.max(1.0),
    );
    let mut size_note = None;
    if let Some([ha, hb]) = sized_heights(profiles, size) {
        let m = libm::powf(ha / hb, HEIGHT_WEIGHT).clamp(0.5, 2.0);
        let k = libm::sqrtf(m);
        a *= k;
        b /= k;
        if (m - 1.0).abs() > 0.05 {
            let taller = if ha >= hb { names[0] } else { names[1] };
            size_note = Some(format!(
                "{taller} stands taller ({:.0} vs {:.0}, fighter size {})",
                ha.max(hb),
                ha.min(hb),
                size.as_str()
            ));
        }
    }
    let ratio = a.max(b) / a.min(b);
    if !blocked.is_empty() {
        return Verdict {
            kind: VerdictKind::Blocked,
            reasons: blocked,
            ratio,
        };
    }
    let stronger = if a >= b { names[0] } else { names[1] };
    let mut unbalanced = Vec::new();
    let mut notes: Vec<String> = size_note.into_iter().collect();
    if ratio > t.ratio {
        unbalanced.push(format!(
            "{stronger} is {ratio:.1}x stronger (power {a:.0} vs {b:.0})"
        ));
    }
    let gap = |x: i32, y: i32| x.max(1).max(y.max(1)) as f32 / x.max(1).min(y.max(1)) as f32;
    for (what, x, y) in [
        ("life", profiles[0].life, profiles[1].life),
        ("attack", profiles[0].attack, profiles[1].attack),
        ("defence", profiles[0].defence, profiles[1].defence),
    ] {
        if gap(x, y) >= 2.0 {
            unbalanced.push(format!("{what} {x} vs {y}"));
        }
    }
    if let [Some(ra), Some(rb)] = ratings
        && ra.games >= t.min_games
        && rb.games >= t.min_games
    {
        let diff = ra.rating - rb.rating;
        let fav = if diff >= 0 { names[0] } else { names[1] };
        let expect = 1.0 / (1.0 + libm::powf(10.0, -(diff.abs() as f32) / 400.0));
        if diff.abs() > t.community_gap {
            unbalanced.push(format!(
                "community: {fav} expected to win {:.0}% ({} vs {})",
                expect * 100.0,
                ra.rating,
                rb.rating
            ));
        } else if diff.abs() <= t.community_gap / 2 {
            // proven even by real matches: forgive a score-only gap
            let only_score = unbalanced.len() == 1 && ratio > t.ratio;
            if only_score {
                unbalanced.clear();
            }
            notes.push(format!(
                "community says even ({} vs {}, {} / {} games)",
                ra.rating, rb.rating, ra.games, rb.games
            ));
        }
    }
    if unbalanced.is_empty() {
        notes.insert(0, format!("power {a:.0} vs {b:.0}"));
        Verdict {
            kind: VerdictKind::Fair,
            reasons: notes,
            ratio,
        }
    } else {
        unbalanced.extend(notes);
        Verdict {
            kind: VerdictKind::Unbalanced,
            reasons: unbalanced,
            ratio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugen_proto::CheatFlag;

    fn p(score: f32) -> FairnessProfile {
        FairnessProfile {
            life: 1000,
            attack: 100,
            defence: 100,
            power_score: score,
            ..Default::default()
        }
    }

    fn v(a: &FairnessProfile, b: &FairnessProfile) -> Verdict {
        verdict(
            [a, b],
            [None, None],
            ["A", "B"],
            FighterSize::None,
            &Thresholds::default(),
        )
    }

    #[test]
    fn thresholds() {
        assert_eq!(v(&p(100.0), &p(120.0)).kind, VerdictKind::Fair);
        assert_eq!(v(&p(100.0), &p(159.0)).kind, VerdictKind::Fair);
        let u = v(&p(100.0), &p(170.0));
        assert_eq!(u.kind, VerdictKind::Unbalanced);
        assert!(u.reasons[0].starts_with("B is 1.7x"), "{:?}", u.reasons);
        let mut tank = p(110.0);
        tank.life = 2200;
        assert_eq!(v(&p(100.0), &tank).kind, VerdictKind::Unbalanced);
    }

    /// Content scores are even; the heights FIGHTER SIZE leaves them at
    /// decide (x1.3 against /1.3 is a 1.3x reach edge).
    #[test]
    fn fighter_size_weighs_the_heights() {
        let t = Thresholds::default();
        let sized = |score: f32, h: f32| FairnessProfile {
            height: Some(h),
            ..p(score)
        };
        // Goku 46 x1.3 = 59.8 against KFM 105 /1.3 = 80.8: 1.35x as tall
        let (goku, kfm) = (sized(130.0, 46.0), sized(100.0, 105.0));
        let normal = verdict(
            [&goku, &kfm],
            [None, None],
            ["G", "K"],
            FighterSize::Normal,
            &t,
        );
        let expect = 1.3 / libm::sqrtf(80.8 / 59.8);
        assert!((normal.ratio - expect).abs() < 0.01, "{normal:?}");
        assert_eq!(normal.kind, VerdictKind::Fair);
        assert!(
            normal
                .reasons
                .iter()
                .any(|r| r.contains("K stands taller (81 vs 60")),
            "{normal:?}"
        );
        // left at their own sizes KFM is 2.3x as tall: his reach makes up
        // for the content gap and then some
        let none = verdict(
            [&goku, &kfm],
            [None, None],
            ["G", "K"],
            FighterSize::None,
            &t,
        );
        let expect = libm::sqrtf(105.0 / 46.0) / 1.3;
        assert!((none.ratio - expect).abs() < 0.01, "{none:?}");
        assert!(
            none.reasons
                .iter()
                .any(|r| r.contains("K stands taller (105 vs 46")),
            "{none:?}"
        );
        // the same fighter against itself is even at any size
        let even = verdict(
            [&kfm, &kfm],
            [None, None],
            ["A", "B"],
            FighterSize::Chibi,
            &t,
        );
        assert_eq!(even.ratio, 1.0);
        // an extreme gap is unbalanced on size alone
        let (big, small) = (sized(100.0, 150.0), sized(100.0, 30.0));
        let none = verdict(
            [&big, &small],
            [None, None],
            ["A", "B"],
            FighterSize::None,
            &t,
        );
        assert_eq!(none.kind, VerdictKind::Unbalanced, "{none:?}");
        assert!(none.reasons[0].starts_with("A is 2.0x"), "{none:?}");
        let normal = verdict(
            [&big, &small],
            [None, None],
            ["A", "B"],
            FighterSize::Normal,
            &t,
        );
        assert_eq!(
            normal.kind,
            VerdictKind::Fair,
            "both land in the band: {normal:?}"
        );
        // an unknown height (older profile) keeps the content-only verdict
        let old = verdict(
            [&p(100.0), &small],
            [None, None],
            ["A", "B"],
            FighterSize::None,
            &t,
        );
        assert_eq!(old.ratio, 1.0);
        assert_eq!(sized_heights([&p(1.0), &small], FighterSize::Normal), None);
    }

    #[test]
    fn cheats_and_broken_block() {
        let mut c = p(100.0);
        c.flags.push(CheatFlag {
            code: "regen".into(),
            severity: Severity::Block,
            detail: "state -2: LifeAdd".into(),
        });
        let b = v(&p(100.0), &c);
        assert_eq!(b.kind, VerdictKind::Blocked);
        assert_eq!(b.reasons, vec!["B: state -2: LifeAdd"]);
        let mut broken = p(100.0);
        broken.verify = Some("broken".into());
        assert_eq!(v(&broken, &p(100.0)).kind, VerdictKind::Blocked);
    }

    #[test]
    fn community_refines() {
        let t = Thresholds::default();
        let (a, b) = (p(100.0), p(175.0));
        let even = CharRating {
            rating: 1500,
            games: 40,
            wins: 20,
        };
        let even2 = CharRating {
            rating: 1530,
            games: 30,
            wins: 16,
        };
        let r = verdict(
            [&a, &b],
            [Some(&even), Some(&even2)],
            ["A", "B"],
            FighterSize::None,
            &t,
        );
        assert_eq!(r.kind, VerdictKind::Fair, "{r:?}");
        let strong = CharRating {
            rating: 1800,
            games: 30,
            wins: 25,
        };
        let r = verdict(
            [&a, &a],
            [Some(&even), Some(&strong)],
            ["A", "B"],
            FighterSize::None,
            &t,
        );
        assert_eq!(r.kind, VerdictKind::Unbalanced);
        assert!(r.reasons[0].contains("community: B"), "{r:?}");
        // too few games: ignored
        let new = CharRating {
            rating: 2000,
            games: 3,
            wins: 3,
        };
        let r = verdict(
            [&a, &a],
            [Some(&even), Some(&new)],
            ["A", "B"],
            FighterSize::None,
            &t,
        );
        assert_eq!(r.kind, VerdictKind::Fair);
    }
}
