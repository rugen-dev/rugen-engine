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
//! * Otherwise **FAIR**.

use rugen_proto::{CharRating, FairnessProfile, Severity, Verdict, VerdictKind};

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

/// `names` label the reasons (`["NARUTO", "SASUKE"]`).
pub fn verdict(
    profiles: [&FairnessProfile; 2],
    ratings: [Option<&CharRating>; 2],
    names: [&str; 2],
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
    let (a, b) = (
        profiles[0].power_score.max(1.0),
        profiles[1].power_score.max(1.0),
    );
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
    let mut notes = Vec::new();
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
        verdict([a, b], [None, None], ["A", "B"], &Thresholds::default())
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
        let r = verdict([&a, &b], [Some(&even), Some(&even2)], ["A", "B"], &t);
        assert_eq!(r.kind, VerdictKind::Fair, "{r:?}");
        let strong = CharRating {
            rating: 1800,
            games: 30,
            wins: 25,
        };
        let r = verdict([&a, &a], [Some(&even), Some(&strong)], ["A", "B"], &t);
        assert_eq!(r.kind, VerdictKind::Unbalanced);
        assert!(r.reasons[0].contains("community: B"), "{r:?}");
        // too few games: ignored
        let new = CharRating {
            rating: 2000,
            games: 3,
            wins: 3,
        };
        let r = verdict([&a, &a], [Some(&even), Some(&new)], ["A", "B"], &t);
        assert_eq!(r.kind, VerdictKind::Fair);
    }
}
