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
    /// `[p1 bits, p2 bits]` per tick (see `Buttons::bits`).
    pub inputs: Vec<[u16; 2]>,
}

impl Replay {
    /// A recording of `setup`, simulated with `fighter_size`.
    pub fn new(setup: &FightSetup, fighter_size: FighterSize) -> Self {
        Self {
            version: REPLAY_VERSION,
            p1: setup.p1.clone(),
            p2: setup.p2.clone(),
            stage: setup.stage.clone(),
            ai: setup.ai,
            fighter_size,
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
        assert_eq!(old.setup().fighter_size, Some(FighterSize::None));
        let r = Replay {
            fighter_size: FighterSize::Chibi,
            ..old
        };
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.fighter_size, FighterSize::Chibi);
        assert_eq!(back.setup().fighter_size, Some(FighterSize::Chibi));
    }
}
