//! CHALLENGE modes and the arcade ladder: a run of CPU fights lined up one
//! after another by the shell (each mode is described on its
//! [`Mode`] variant). Best results are kept per fighter in the profile.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::game::Mode;
use crate::play::RunRules;

/// Fights in the arcade ladder.
pub const LADDER: u32 = 6;
/// Survival: share of the life bar given back after each win, in percent.
pub const SURVIVAL_HEAL: i32 = 15;

/// One fighter's best CHALLENGE results (`profile.json` → `challenge`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChallengeBest {
    /// Display name when the record was set (the screen shows it even if
    /// the fighter is gone).
    pub name: String,
    /// Survival: most wins in one run.
    pub survival: u32,
}

fn wins_text(n: u32) -> String {
    crate::uiscreen::plural(n as usize, "WIN", "WINS")
}

/// A run in progress: who plays, how far they got.
#[derive(Clone, Debug)]
pub struct Run {
    pub mode: Mode,
    pub p1: PathBuf,
    /// P1's cache manifest id (the profile key) and display name.
    pub p1_id: String,
    pub p1_name: String,
    /// Fights won so far.
    pub wins: u32,
    /// Survival: P1's life for the next fight (`None` = full).
    pub life: Option<i32>,
    /// The run ended: `Some(true)` cleared, `Some(false)` lost.
    pub over: Option<bool>,
    /// The ended run beat the fighter's best.
    pub new_best: bool,
    /// Survival's opponent dice.
    seed: u32,
    /// Roster index of the last opponent (Survival avoids a repeat).
    last_foe: Option<usize>,
}

impl Run {
    pub fn new(mode: Mode, p1: PathBuf, p1_id: String, p1_name: String, seed: u32) -> Self {
        Self {
            mode,
            p1,
            p1_id,
            p1_name,
            wins: 0,
            life: None,
            over: None,
            new_best: false,
            seed: seed | 1,
            last_foe: None,
        }
    }

    /// Fights in a full run (`None`: Survival never ends on its own).
    pub fn length(&self) -> Option<u32> {
        match self.mode {
            Mode::Survival => None,
            _ => Some(LADDER),
        }
    }

    /// The rules of the next fight.
    pub fn rules(&self) -> Option<RunRules> {
        (self.mode == Mode::Survival).then_some(RunRules {
            match_wins: 1,
            p1_life: self.life,
        })
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        if n == 0 { 0 } else { self.seed as usize % n }
    }

    /// Roster index (of `n` fighters) of the next opponent after a win:
    /// the arcade ladder's fixed walk, or a random one in Survival (never
    /// the same twice in a row when there is a choice).
    pub fn next_foe(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        let k = if self.mode == Mode::Survival {
            let mut k = self.rand(n);
            if n > 1 && Some(k) == self.last_foe {
                k = (k + 1 + self.rand(n - 1)) % n;
            }
            k
        } else {
            (self.wins.saturating_sub(1) as usize * 7 + 3) % n
        };
        self.last_foe = Some(k);
        k
    }

    /// Survival: remember the first opponent (the fighter select's pick).
    pub fn first_foe(&mut self, index: Option<usize>) {
        self.last_foe = index;
    }

    /// Stage index (of `n`) for the next fight.
    pub fn next_stage(&self, n: usize) -> usize {
        self.wins.saturating_sub(1) as usize % n.max(1)
    }

    /// A fight ended: P1 `won` with `life` of `life_max` left. Survival
    /// heals a little; the run ends on a loss or after its last fight.
    /// True while the run goes on.
    pub fn finish(&mut self, won: bool, life: i32, life_max: i32) -> bool {
        if self.over.is_some() {
            return false;
        }
        if !won {
            self.over = Some(false);
            return false;
        }
        self.wins += 1;
        if self.mode == Mode::Survival {
            let max = life_max.max(1);
            self.life = Some((life.max(1) + max * SURVIVAL_HEAL / 100).min(max));
        }
        if self.length().is_some_and(|n| self.wins >= n) {
            self.over = Some(true);
            return false;
        }
        true
    }

    /// Write an ended CHALLENGE run into the fighter's record; true when it
    /// beat the best so far (also kept in [`Run::new_best`]).
    pub fn record(&mut self, best: &mut ChallengeBest) -> bool {
        if self.over.is_none() {
            return false;
        }
        let better = match self.mode {
            Mode::Survival => self.wins > best.survival,
            _ => false,
        };
        if better {
            best.name = self.p1_name.clone();
            if self.mode == Mode::Survival {
                best.survival = self.wins;
            }
        }
        self.new_best = better;
        better
    }

    /// The strip over the fight: left and right halves (`None` in Arcade).
    pub fn hud(&self) -> Option<(String, String)> {
        Some(match self.mode {
            Mode::Survival => (
                format!("WINS {}", self.wins),
                format!("LIFE +{SURVIVAL_HEAL}% NEXT"),
            ),
            _ => return None,
        })
    }

    /// The result screen's two run rows (after [`Run::finish`] and, for an
    /// ended run, [`Run::record`] against `best`). Empty in Arcade.
    pub fn rows(&self, best: &ChallengeBest) -> Vec<(String, String)> {
        let tag = |s: String| {
            if self.new_best {
                format!("{s} · NEW")
            } else {
                s
            }
        };
        match (self.mode, self.over) {
            (Mode::Survival, None) => vec![
                ("WINS".into(), self.wins.to_string()),
                ("NEXT".into(), format!("LIFE +{SURVIVAL_HEAL}%")),
            ],
            (Mode::Survival, Some(_)) => vec![
                ("WINS".into(), self.wins.to_string()),
                ("BEST".into(), tag(wins_text(best.survival))),
            ],
            _ => Vec::new(),
        }
    }
}

/// Stages a CHALLENGE run fights on: every installed stage except the
/// `[Arena]` ones (stocks and blast zones belong to ARENA), else whatever
/// there is.
pub fn fight_stages(lib: &rugen_content::Library) -> Vec<PathBuf> {
    let all: Vec<PathBuf> = lib
        .stages()
        .iter()
        .filter_map(|e| lib.prepare_stage(e).ok().map(|(d, m)| m.def_path(&d)))
        .collect();
    let plain: Vec<PathBuf> = all
        .iter()
        .filter(|p| {
            rugen_formats::def::Def::open(p).is_ok_and(|d| {
                let a = rugen_sim::Stage::from_def(&d).arena;
                a.stocks <= 0 && a.blastzone.is_none()
            })
        })
        .cloned()
        .collect();
    if plain.is_empty() { all } else { plain }
}

/// The CHALLENGE screen's cards (`ui/challenge.toml`): each mode's rules
/// and the best result over every fighter.
pub fn cards(records: &std::collections::BTreeMap<String, ChallengeBest>) -> Value {
    let pick = |f: &dyn Fn(&ChallengeBest) -> Option<(u64, String)>| {
        records
            .values()
            .filter_map(|b| f(b).map(|(k, t)| (k, format!("{t} · {}", b.name.to_uppercase()))))
            .max_by_key(|(k, _)| *k)
            .map(|(_, t)| t)
    };
    let survival =
        pick(&|b| (b.survival > 0).then(|| (u64::from(b.survival), wins_text(b.survival))));
    let none = || "NO RECORD YET".to_string();
    json!([
        {
            "id": "survival",
            "title": "SURVIVAL",
            "desc": "ONE LIFE BAR AGAINST AN ENDLESS LINE OF RANDOM OPPONENTS. A LITTLE LIFE COMES BACK BETWEEN FIGHTS.",
            "hud_l": "ONE ROUND EACH",
            "hud_r": format!("LIFE +{SURVIVAL_HEAL}% NEXT"),
            "note": "THE RUN ENDS AT THE FIRST LOSS",
            "best": survival.unwrap_or_else(none),
            "action": "mode:survival",
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mode: Mode) -> Run {
        Run::new(mode, "p1.def".into(), "naruto".into(), "Naruto".into(), 7)
    }

    /// Survival: one round a fight, life carried over plus 15 %, endless
    /// until a loss; the first loss ends it with the wins counted.
    #[test]
    fn survival_carries_life_and_ends_on_a_loss() {
        let mut r = run(Mode::Survival);
        assert_eq!(
            r.rules(),
            Some(RunRules {
                match_wins: 1,
                p1_life: None
            })
        );
        assert!(r.finish(true, 400, 1000));
        assert_eq!(r.life, Some(550));
        assert!(r.finish(true, 950, 1000));
        assert_eq!(r.life, Some(1000), "never above full");
        assert_eq!(r.rules().unwrap().p1_life, Some(1000));
        for _ in 0..30 {
            assert!(r.finish(true, 500, 1000), "no end while winning");
        }
        assert!(!r.finish(false, 0, 1000));
        assert_eq!(r.over, Some(false));
        assert_eq!(r.wins, 32);
        assert_eq!(r.hud().unwrap().0, "WINS 32");
    }

    #[test]
    fn survival_never_meets_the_same_opponent_twice_in_a_row() {
        let mut r = run(Mode::Survival);
        r.first_foe(Some(2));
        let mut last = 2;
        for _ in 0..200 {
            let k = r.next_foe(3);
            assert!(k < 3);
            assert_ne!(k, last);
            last = k;
        }
        assert_eq!(r.next_foe(1), 0, "a single fighter fights itself");
    }

    /// The arcade ladder walks the roster as it always did.
    #[test]
    fn ladder_picks_follow_the_arcade_walk() {
        let mut r = run(Mode::Arcade);
        r.finish(true, 1, 1000);
        assert_eq!(r.next_foe(10), 3);
        assert_eq!(r.next_stage(4), 0);
        r.finish(true, 1, 1000);
        assert_eq!(r.next_foe(10), 0);
        assert_eq!(r.next_stage(4), 1);
        assert_eq!(r.hud(), None, "no strip in arcade");
    }

    /// Old profiles have no `challenge` records; the screen says so.
    #[test]
    fn cards_show_the_best_over_every_fighter() {
        let old: crate::uiscreen::Profile =
            serde_json::from_str(r#"{"name":"P","wins":3}"#).unwrap();
        assert!(old.challenge.is_empty());
        let empty = cards(&old.challenge);
        assert_eq!(empty[0]["best"], "NO RECORD YET");
        // records as profile.json holds them
        let rec = |v: Value| serde_json::from_value::<ChallengeBest>(v).unwrap();
        let mut recs = std::collections::BTreeMap::new();
        recs.insert(
            "a".to_string(),
            rec(json!({ "name": "Naruto", "survival": 23, "time_attack": 60 * 400 })),
        );
        recs.insert(
            "b".to_string(),
            rec(json!({
                "name": "Sasuke",
                "survival": 4,
                "boss_rush": 5,
                "boss_total": 5,
                "time_attack": 60 * 408 + 6,
            })),
        );
        let v = cards(&recs);
        assert_eq!(v[0]["best"], "23 WINS · NARUTO");
    }
}
