//! Training lab: tools for the TRAINING mode only. They draw over the fight
//! or (the dummy) choose the inputs P2 is fed; they never touch the
//! simulation's state, so a training replay plays back exactly and nothing
//! here can reach an online match.
//!
//! The pause menu edits a [`TrainingCfg`]; the shell keeps it between
//! matches of a session.

use serde_json::{Value, json};

/// What the training tools show and do. Edited from the pause menu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainingCfg {
    /// Clsn1 (attack) boxes red, Clsn2 (hurt) boxes blue, over each sprite.
    pub hitboxes: bool,
}

/// One adjustable row of a training panel: `(id, label, options, selected)`.
type Row = (&'static str, &'static str, Vec<&'static str>, usize);

const ON_OFF: [&str; 2] = ["OFF", "ON"];

impl TrainingCfg {
    fn display_rows(&self) -> Vec<Row> {
        vec![(
            "hitboxes",
            "HITBOXES",
            ON_OFF.to_vec(),
            usize::from(self.hitboxes),
        )]
    }

    /// Set row `id` to option `index` (clamped, cycling). False for an unknown row.
    fn set(&mut self, id: &str, index: usize) -> bool {
        match id {
            "hitboxes" => self.hitboxes = index % 2 == 1,
            _ => return false,
        }
        true
    }

    fn row(&self, id: &str) -> Option<Row> {
        self.display_rows().into_iter().find(|r| r.0 == id)
    }

    /// Pause menu actions: `tr:<row>:<±1>` steps a row, `pick:<row>:<option
    /// label>` picks one (a click). True when the settings changed.
    pub fn action(&mut self, a: &str) -> bool {
        if let Some(rest) = a.strip_prefix("tr:") {
            let Some((id, d)) = rest.split_once(':') else {
                return false;
            };
            let (Some(r), Ok(d)) = (self.row(id), d.parse::<i64>()) else {
                return false;
            };
            let n = r.2.len() as i64;
            let to = (r.3 as i64 + d).rem_euclid(n.max(1)) as usize;
            return self.set(id, to);
        }
        if let Some(rest) = a.strip_prefix("pick:") {
            let Some((id, label)) = rest.split_once(':') else {
                return false;
            };
            let Some(r) = self.row(id) else {
                return false;
            };
            let Some(i) = r.2.iter().position(|o| o.eq_ignore_ascii_case(label)) else {
                return false;
            };
            return self.set(id, i);
        }
        false
    }

    /// The DISPLAY panel's rows for the pause screen.
    pub fn display_json(&self) -> Value {
        Value::Array(self.display_rows().into_iter().map(row_json).collect())
    }
}

fn row_json((id, label, options, sel): Row) -> Value {
    json!({
        "id": id,
        "label": label,
        "options": options,
        "sel": sel,
        "left": format!("tr:{id}:-1"),
        "right": format!("tr:{id}:1"),
        "press": format!("tr:{id}:1"),
    })
}

/// The training lab runs in local TRAINING matches only: never in another
/// mode, a tutorial or an online match.
pub fn applies(setup: &crate::play::FightSetup) -> bool {
    setup.mode == Some(crate::game::Mode::Training) && !setup.tutorial && setup.net.is_none()
}

/// Live training state of one match.
#[derive(Clone, Debug, Default)]
pub struct Training {
    pub cfg: TrainingCfg,
}

impl Training {
    pub fn new(cfg: TrainingCfg) -> Self {
        Self { cfg }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(mode: Option<crate::game::Mode>) -> crate::play::FightSetup {
        crate::play::FightSetup {
            p1: "a.def".into(),
            p2: "b.def".into(),
            stage: None,
            ai: [0, 0],
            net: None,
            tutorial: false,
            ids: Default::default(),
            mode,
            fighter_size: None,
            arena_ai: None,
            round_time: None,
        }
    }

    /// Hitboxes and the other tools exist in TRAINING only.
    #[test]
    fn only_training_matches_get_the_lab() {
        use crate::game::Mode;
        assert!(applies(&setup(Some(Mode::Training))));
        for m in [
            None,
            Some(Mode::Versus),
            Some(Mode::Arcade),
            Some(Mode::Arena),
        ] {
            assert!(!applies(&setup(m)), "{m:?}");
        }
        let mut s = setup(Some(Mode::Training));
        s.tutorial = true;
        assert!(!applies(&s), "a tutorial");
        let mut s = setup(Some(Mode::Training));
        s.net = Some(crate::play::NetMode::SyncTest);
        assert!(!applies(&s), "any net session");
    }

    #[test]
    fn rows_step_cycle_and_pick() {
        let mut c = TrainingCfg::default();
        assert!(!c.hitboxes, "off until asked for");
        assert!(c.action("tr:hitboxes:1"));
        assert!(c.hitboxes);
        assert!(c.action("tr:hitboxes:1"));
        assert!(!c.hitboxes, "cycles");
        assert!(c.action("tr:hitboxes:-1"));
        assert!(c.hitboxes, "and backwards");
        assert!(c.action("pick:hitboxes:OFF"));
        assert!(!c.hitboxes);
        assert!(!c.action("tr:nothing:1"));
        assert!(!c.action("pick:hitboxes:MAYBE"));
        let rows = c.display_json();
        assert_eq!(rows[0]["id"], "hitboxes");
        assert_eq!(rows[0]["sel"], 0);
        assert_eq!(rows[0]["right"], "tr:hitboxes:1");
    }
}
