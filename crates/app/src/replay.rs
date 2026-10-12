//! Replays: the match setup plus every frame's raw inputs. The simulation is
//! deterministic, so replaying the inputs reproduces the match exactly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::play::FightSetup;

pub const REPLAY_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Replay {
    pub version: u32,
    pub p1: PathBuf,
    pub p2: PathBuf,
    pub stage: Option<PathBuf>,
    pub ai: [i32; 2],
    /// `[p1 bits, p2 bits]` per tick (see `Buttons::bits`).
    pub inputs: Vec<[u16; 2]>,
}

impl Replay {
    pub fn new(setup: &FightSetup) -> Self {
        Self {
            version: REPLAY_VERSION,
            p1: setup.p1.clone(),
            p2: setup.p2.clone(),
            stage: setup.stage.clone(),
            ai: setup.ai,
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
