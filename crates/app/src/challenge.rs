//! CHALLENGE modes and the arcade ladder: a run of CPU fights lined up one
//! after another by the shell (each mode is described on its
//! [`Mode`] variant). Best results are kept per fighter in the profile.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::game::Mode;
use crate::play::RunRules;

/// Fights in the arcade ladder.
pub const LADDER: u32 = 6;
/// Survival: share of the life bar given back after each win, in percent.
pub const SURVIVAL_HEAL: i32 = 15;
/// Boss Rush without flagged bosses: this many of the strongest fighters.
pub const BOSS_FALLBACK: usize = 5;

/// One fighter's best CHALLENGE results (`profile.json` → `challenge`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChallengeBest {
    /// Display name when the record was set (the screen shows it even if
    /// the fighter is gone).
    pub name: String,
    /// Survival: most wins in one run.
    pub survival: u32,
    /// Boss Rush: most bosses beaten in one run, out of `boss_total`.
    pub boss_rush: u32,
    pub boss_total: u32,
}

impl ChallengeBest {
    fn boss_cleared(&self) -> bool {
        self.boss_total > 0 && self.boss_rush >= self.boss_total
    }

    fn boss_text(&self) -> String {
        if self.boss_cleared() {
            "CLEARED".into()
        } else {
            format!("{} / {}", self.boss_rush, self.boss_total)
        }
    }
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
    /// Boss Rush: the bosses' manifest ids in fight order.
    pub bosses: Vec<String>,
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
            bosses: Vec::new(),
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
            Mode::BossRush => Some(self.bosses.len() as u32),
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
    /// Boss Rush reads [`Run::next_boss`] instead.
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

    /// Boss Rush: the next boss's manifest id.
    pub fn next_boss(&self) -> Option<&str> {
        self.bosses.get(self.wins as usize).map(String::as_str)
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
            Mode::BossRush => {
                let total = self.bosses.len() as u32;
                // a clear of a longer line beats one of a shorter line
                let now = (self.wins >= total && total > 0, self.wins, total);
                let was = (best.boss_cleared(), best.boss_rush, best.boss_total);
                total > 0 && now > was
            }
            _ => false,
        };
        if better {
            best.name = self.p1_name.clone();
            if self.mode == Mode::Survival {
                best.survival = self.wins;
            }
            if self.mode == Mode::BossRush {
                best.boss_rush = self.wins;
                best.boss_total = self.bosses.len() as u32;
            }
        }
        self.new_best = better;
        better
    }

    /// The strip over the fight: left and right halves (`None` in Arcade).
    pub fn hud(&self) -> Option<(String, String)> {
        let n = self.length().unwrap_or(0);
        Some(match self.mode {
            Mode::Survival => (
                format!("WINS {}", self.wins),
                format!("LIFE +{SURVIVAL_HEAL}% NEXT"),
            ),
            Mode::BossRush => (
                format!("BOSS {} / {n}", (self.wins + 1).min(n)),
                "NO CONTINUES".into(),
            ),
            _ => return None,
        })
    }

    /// The result screen's two run rows (after [`Run::finish`] and, for an
    /// ended run, [`Run::record`] against `best`). Empty in Arcade.
    pub fn rows(&self, best: &ChallengeBest) -> Vec<(String, String)> {
        let n = self.length().unwrap_or(0);
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
            (Mode::BossRush, None) => vec![
                ("BOSSES".into(), format!("{} / {n}", self.wins)),
                ("NEXT".into(), format!("BOSS {}", self.wins + 1)),
            ],
            (Mode::BossRush, Some(cleared)) => vec![
                (
                    "BOSSES".into(),
                    if cleared {
                        "CLEARED".into()
                    } else {
                        format!("{} / {n}", self.wins)
                    },
                ),
                ("BEST".into(), tag(best.boss_text())),
            ],
            _ => Vec::new(),
        }
    }
}

/// A fighter the Boss Rush line can take.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Cache manifest id.
    pub id: String,
    /// Names select.def may use for it (folder / archive / def stem).
    pub names: Vec<String>,
    /// Balance verdict strength (`FairnessProfile::power_score`).
    pub power: f32,
}

/// The `boss = 1` characters of a select.def's `[Characters]`, as written
/// (`kfm`, `suave.zip/newsuave.def`, `kfm/alt-kfm.def`), in file order.
pub fn select_def_bosses(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_chars = false;
    for line in text.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_chars = line.eq_ignore_ascii_case("[characters]");
            continue;
        }
        if !in_chars || line.is_empty() {
            continue;
        }
        let mut parts = line.split(',').map(str::trim);
        let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
            continue;
        };
        let boss = parts.any(|p| {
            p.split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("boss")
                    && v.trim().parse::<i32>().is_ok_and(|v| v != 0)
            })
        });
        if boss {
            out.push(name.to_string());
        }
    }
    out
}

/// The names a select.def entry can match: the folder or archive stem and
/// the def stem (`suave.zip/newsuave.def` → `suave`, `newsuave`).
fn entry_names(entry: &str) -> Vec<String> {
    let entry = entry.replace('\\', "/");
    let stem = |s: &str| {
        let s = s.rsplit('/').next().unwrap_or(s);
        match s.rsplit_once('.') {
            Some((a, _)) if !a.is_empty() => a.to_string(),
            _ => s.to_string(),
        }
    };
    let mut out = Vec::new();
    if let Some(first) = entry.split('/').next() {
        out.push(stem(first));
    }
    out.push(stem(&entry));
    out.dedup();
    out
}

/// The Boss Rush line: the flagged characters found in the roster, in
/// select.def order; with none, the [`BOSS_FALLBACK`] strongest fighters,
/// weakest first (the strongest is the last fight). The flag says whether
/// the line came from select.def.
pub fn boss_line(candidates: &[Candidate], flagged: &[String]) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    for f in flagged {
        let names = entry_names(f);
        let hit = candidates.iter().find(|c| {
            c.names
                .iter()
                .any(|n| names.iter().any(|m| m.eq_ignore_ascii_case(n)))
        });
        if let Some(c) = hit
            && !out.contains(&c.id)
        {
            out.push(c.id.clone());
        }
    }
    if !out.is_empty() {
        return (out, true);
    }
    let mut by_power: Vec<&Candidate> = candidates.iter().collect();
    by_power.sort_by(|a, b| {
        b.power
            .total_cmp(&a.power)
            .then_with(|| a.id.to_lowercase().cmp(&b.id.to_lowercase()))
    });
    by_power.truncate(BOSS_FALLBACK);
    by_power.reverse();
    (by_power.into_iter().map(|c| c.id.clone()).collect(), false)
}

/// The select.def a player can flag bosses in: `select.def` in the game
/// root, else `data/select.def`.
pub fn select_def(root: &Path) -> Option<PathBuf> {
    [
        root.join("select.def"),
        root.join("data").join("select.def"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Boss Rush line for the installed roster (manifest ids), and whether the
/// bosses came from select.def.
pub fn bosses(lib: &rugen_content::Library) -> (Vec<String>, bool) {
    let flagged = select_def(&lib.root)
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| select_def_bosses(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let mut cands = Vec::new();
    for e in lib.chars() {
        let Ok((_, m)) = lib.prepare_char(&e) else {
            continue;
        };
        if m.verify.as_ref().is_some_and(|v| v.is_broken()) {
            continue;
        }
        let mut names = vec![e.id.clone()];
        if let Some(s) = e.source.file_stem() {
            names.push(s.to_string_lossy().to_string());
        }
        if let Some(s) = Path::new(&m.def.replace('\\', "/")).file_stem() {
            names.push(s.to_string_lossy().to_string());
        }
        cands.push(Candidate {
            id: m.id.clone(),
            names,
            power: m.profile.as_ref().map_or(0.0, |p| p.power_score),
        });
    }
    boss_line(&cands, &flagged)
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
pub fn cards(
    records: &std::collections::BTreeMap<String, ChallengeBest>,
    boss_count: usize,
    bosses_flagged: bool,
) -> Value {
    let pick = |f: &dyn Fn(&ChallengeBest) -> Option<(u64, String)>| {
        records
            .values()
            .filter_map(|b| f(b).map(|(k, t)| (k, format!("{t} · {}", b.name.to_uppercase()))))
            .max_by_key(|(k, _)| *k)
            .map(|(_, t)| t)
    };
    let survival =
        pick(&|b| (b.survival > 0).then(|| (u64::from(b.survival), wins_text(b.survival))));
    let boss = pick(&|b| {
        (b.boss_total > 0).then(|| {
            let key = (u64::from(b.boss_cleared()) << 40)
                | (u64::from(b.boss_rush) << 20)
                | u64::from(b.boss_total);
            (key, b.boss_text())
        })
    });
    let none = || "NO RECORD YET".to_string();
    let boss_rule = if bosses_flagged {
        "BOSSES FROM SELECT.DEF"
    } else {
        "THE STRONGEST FIGHTERS"
    };
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
        {
            "id": "bossrush",
            "title": "BOSS RUSH",
            "desc": "THE TOUGHEST FIGHTERS BACK TO BACK: BOSSES MARKED IN SELECT.DEF, OR THE FIVE STRONGEST IF NONE ARE.",
            "hud_l": crate::uiscreen::plural(boss_count, "BOSS", "BOSSES"),
            "hud_r": "NO CONTINUES",
            "note": boss_rule,
            "best": boss.unwrap_or_else(none),
            "action": "mode:bossrush",
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

    #[test]
    fn boss_rush_records_the_furthest_run() {
        let mut r = run(Mode::BossRush);
        r.bosses = vec!["a".into(), "b".into(), "c".into()];
        assert!(r.finish(true, 1, 1000));
        assert_eq!(r.next_boss(), Some("b"));
        assert_eq!(r.hud().unwrap().0, "BOSS 2 / 3");
        assert!(!r.finish(false, 0, 1000));
        let mut best = ChallengeBest::default();
        assert!(r.record(&mut best));
        assert_eq!((best.boss_rush, best.boss_total), (1, 3));
        let mut clear = run(Mode::BossRush);
        clear.bosses = vec!["a".into(), "b".into()];
        clear.finish(true, 1, 1000);
        assert!(!clear.finish(true, 1, 1000));
        assert_eq!(clear.over, Some(true));
        assert!(clear.record(&mut best), "a clear beats a deeper loss");
        assert_eq!(best.boss_text(), "CLEARED");
        assert_eq!(clear.rows(&best)[1].1, "CLEARED · NEW");
    }

    #[test]
    fn select_def_marks_bosses() {
        let text = "[Options]\nboss = 1\n[Characters]\nkfm, stages/kfm.def\n\
                    suave.zip/newsuave.def, stages/x.def, boss=1 ; the boss\n\
                    ; goku, boss=1\nRyu, order=3, Boss = 1\nrandomselect\n[ExtraStages]\nboss=1\n";
        assert_eq!(
            select_def_bosses(text),
            vec!["suave.zip/newsuave.def", "Ryu"]
        );
        assert_eq!(
            entry_names("suave.zip/newsuave.def"),
            vec!["suave", "newsuave"]
        );
        assert_eq!(entry_names("kfm"), vec!["kfm"]);
    }

    #[test]
    fn boss_line_uses_flags_else_the_strongest() {
        let c = |id: &str, power: f32| Candidate {
            id: id.into(),
            names: vec![id.into()],
            power,
        };
        let roster = vec![
            c("kfm", 100.0),
            c("goku", 180.0),
            c("naruto", 150.0),
            c("sasuke", 160.0),
            c("ryu", 120.0),
            c("weak", 50.0),
        ];
        let flagged = ["Goku".into(), "missing".into(), "kfm/kfm.def".into()];
        assert_eq!(
            boss_line(&roster, &flagged),
            (vec!["goku".to_string(), "kfm".to_string()], true)
        );
        let (line, from_def) = boss_line(&roster, &["missing".into()]);
        assert!(!from_def, "no flagged fighter installed: the strongest");
        assert_eq!(
            line,
            vec!["kfm", "ryu", "naruto", "sasuke", "goku"],
            "the five strongest, weakest first"
        );
        assert_eq!(boss_line(&roster[..2], &[]).0, vec!["kfm", "goku"]);
    }

    /// Old profiles have no `challenge` records; the screen says so.
    #[test]
    fn cards_show_the_best_over_every_fighter() {
        let old: crate::uiscreen::Profile =
            serde_json::from_str(r#"{"name":"P","wins":3}"#).unwrap();
        assert!(old.challenge.is_empty());
        let empty = cards(&old.challenge, 5, false);
        assert_eq!(empty[0]["best"], "NO RECORD YET");
        assert_eq!(empty[1]["hud_l"], "5 BOSSES");
        assert_eq!(empty[1]["note"], "THE STRONGEST FIGHTERS");
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
        let v = cards(&recs, 5, true);
        assert_eq!(v[0]["best"], "23 WINS · NARUTO");
        assert_eq!(v[1]["best"], "CLEARED · SASUKE");
        assert_eq!(v[1]["note"], "BOSSES FROM SELECT.DEF");
    }
}
