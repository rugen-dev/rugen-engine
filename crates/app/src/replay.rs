//! Replays: the match setup plus every frame's raw inputs. The simulation is
//! deterministic, so replaying the inputs reproduces the match exactly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use rugen_sim::scale::FighterSize;

use crate::play::FightSetup;

fn size_none() -> FighterSize {
    FighterSize::None
}

pub const REPLAY_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Replay {
    pub version: u32,
    pub p1: PathBuf,
    pub p2: PathBuf,
    pub stage: Option<PathBuf>,
    pub ai: [i32; 2],
    /// FIGHTER SIZE the match was simulated with; replays from before the
    /// option played every fighter at its own size.
    #[serde(default = "size_none")]
    pub fighter_size: FighterSize,
    /// Arena AI plugin for the CPU sides; replays from before the option
    /// played without it.
    #[serde(default)]
    pub arena_ai: bool,
    /// Round clock the match was simulated with (seconds, <= 0 = none);
    /// replays from before it was recorded play with the options' clock.
    #[serde(default)]
    pub round_time: Option<i32>,
    /// Team modes and partners (offline team matches); absent for 1v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<crate::play::TeamSetup>,
    /// A CHALLENGE run's rules for this fight (rounds, P1's carried life);
    /// absent for every other match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<crate::play::RunRules>,
    /// The match seed; replays from before matches had their own were
    /// simulated with [`rugen_sim::headless::LEGACY_SEED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// `[p1 bits, p2 bits]` per tick (see `Buttons::bits`).
    pub inputs: Vec<[u16; 2]>,
}

impl Replay {
    /// A recording of `setup`, simulated with `fighter_size`, `arena_ai`,
    /// a `round_time` clock and the match `seed`.
    pub fn new(
        setup: &FightSetup,
        fighter_size: FighterSize,
        arena_ai: bool,
        round_time: i32,
        seed: u64,
    ) -> Self {
        Self {
            version: REPLAY_VERSION,
            p1: setup.p1.clone(),
            p2: setup.p2.clone(),
            stage: setup.stage.clone(),
            ai: setup.ai,
            fighter_size,
            arena_ai,
            round_time: Some(round_time),
            teams: setup.teams.clone(),
            run: setup.run,
            seed: Some(seed),
            inputs: Vec::new(),
        }
    }

    pub fn setup(&self) -> FightSetup {
        FightSetup {
            p1: self.p1.clone(),
            p2: self.p2.clone(),
            stage: self.stage.clone(),
            ai: self.ai,
            net: None,
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: None,
            fighter_size: Some(self.fighter_size),
            arena_ai: Some(self.arena_ai),
            round_time: self.round_time,
            teams: self.teams.clone(),
            run: self.run,
            seed: Some(self.seed.unwrap_or(rugen_sim::headless::LEGACY_SEED)),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec(self)?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let r: Replay = serde_json::from_slice(&std::fs::read(path)?)?;
        anyhow::ensure!(
            r.version == REPLAY_VERSION,
            "replay version {} (expected {REPLAY_VERSION})",
            r.version
        );
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_keep_their_fighter_size() {
        // recorded before the option: every fighter at its own size
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[[1,2]]}"#,
        )
        .unwrap();
        assert_eq!(old.fighter_size, FighterSize::None);
        assert!(!old.arena_ai, "old replays played without the arena AI");
        assert_eq!(old.setup().arena_ai, Some(false));
        assert_eq!(old.setup().fighter_size, Some(FighterSize::None));
        assert_eq!(
            old.setup().round_time,
            None,
            "old replays: the options' clock"
        );
        let r = Replay {
            fighter_size: FighterSize::Chibi,
            ..old
        };
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.fighter_size, FighterSize::Chibi);
        assert_eq!(back.setup().fighter_size, Some(FighterSize::Chibi));
    }

    /// A match played with ROUND TIME 30 replays with 30, whatever the
    /// options say when it is watched.
    #[test]
    fn replays_keep_their_round_time() {
        let setup = Replay {
            version: REPLAY_VERSION,
            p1: "a".into(),
            p2: "b".into(),
            stage: None,
            ai: [0, 4],
            fighter_size: FighterSize::Normal,
            arena_ai: false,
            round_time: None,
            teams: None,
            run: None,
            seed: None,
            inputs: Vec::new(),
        }
        .setup();
        let r = Replay::new(&setup, FighterSize::Normal, false, 30, 1);
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.round_time, Some(30));
        assert_eq!(back.setup().round_time, Some(30));
        let none = Replay::new(&setup, FighterSize::Normal, false, -1, 1);
        assert_eq!(
            none.setup().round_time,
            Some(-1),
            "no clock is recorded too"
        );
    }

    /// A team match replays as one: modes and partners ride along; 1v1
    /// replays write no `teams` and old ones read as 1v1.
    #[test]
    fn replays_keep_their_teams() {
        use rugen_sim::team::TeamMode;
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(old.setup().teams, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("teams"));
        let mut setup = old.setup();
        setup.teams = Some(crate::play::TeamSetup {
            mode: [TeamMode::Turns, TeamMode::Single],
            partners: [vec!["c".into(), "d".into()], vec![]],
            ids: Default::default(),
        });
        let r = Replay::new(&setup, FighterSize::Normal, false, 60, 1);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""mode":["turns","single"]"#), "{json}");
        let back: Replay = serde_json::from_str(&json).unwrap();
        assert_eq!(back.setup().teams, setup.teams);
    }

    /// A Survival fight replays with its one round and P1's carried life;
    /// other replays write no `run` and old ones read without one.
    #[test]
    fn replays_keep_their_run_rules() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(old.setup().run, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("run"));
        let mut setup = old.setup();
        setup.run = Some(crate::play::RunRules {
            match_wins: 1,
            p1_life: Some(640),
        });
        let r = Replay::new(&setup, FighterSize::Normal, false, 60, 1);
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.setup().run, setup.run);
    }

    /// Per-match seed: a replay plays with the seed it recorded; one from
    /// before matches had their own plays with seed 1, as it was simulated.
    #[test]
    fn replays_keep_their_seed() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(old.seed, None);
        assert_eq!(old.setup().seed, Some(rugen_sim::headless::LEGACY_SEED));
        let r = Replay::new(&old.setup(), FighterSize::Normal, false, 60, 0xDEAD_BEEF);
        let json = serde_json::to_string(&r).unwrap();
        let back: Replay = serde_json::from_str(&json).unwrap();
        assert_eq!(back.seed, Some(0xDEAD_BEEF));
        assert_eq!(back.setup().seed, Some(0xDEAD_BEEF));
    }
}
