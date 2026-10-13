//! Tutorial: guided steps that work for any character. Basics first, then
//! every special move from the character's `.cmd`, detected when performed.

use rugen_cns::cmd::Key as CmdKey;
use rugen_sim::{CharData, World};

use crate::keylabel::Labels;

enum Check {
    WalkFwd,
    WalkBack,
    Jump,
    Crouch,
    Attack,
    Guard,
    Command(String),
}

struct Step {
    title: String,
    hint: String,
    check: Check,
}

pub struct Tutorial {
    steps: Vec<Step>,
    idx: usize,
    hold: i32,
    /// Ticks left showing "GOOD" before the next step.
    celebrate: i32,
    pub finished: bool,
    /// The START key's label for "PRESS P TO LEAVE".
    start: String,
}

/// A command key as the player presses it: directions as arrows (facing
/// right, as P1 starts), buttons as the bound key.
fn key_label(k: CmdKey, keys: &Labels) -> String {
    match k {
        CmdKey::U => "↑".into(),
        CmdKey::D => "↓".into(),
        CmdKey::F => "→".into(),
        CmdKey::B => "←".into(),
        CmdKey::UF => "↗".into(),
        CmdKey::UB => "↖".into(),
        CmdKey::DF => "↘".into(),
        CmdKey::DB => "↙".into(),
        CmdKey::A => keys.button("a"),
        CmdKey::Bt => keys.button("b"),
        CmdKey::C => keys.button("c"),
        CmdKey::X => keys.button("x"),
        CmdKey::Y => keys.button("y"),
        CmdKey::Z => keys.button("z"),
        CmdKey::S => keys.button("s"),
    }
}

impl Tutorial {
    pub fn new(data: &CharData, keys: &Labels) -> Self {
        let b = |n: &str| keys.button(n);
        let start = keys.start();
        let mut steps = vec![
            Step {
                title: "WALK FORWARD".into(),
                hint: "HOLD →".into(),
                check: Check::WalkFwd,
            },
            Step {
                title: "WALK BACK".into(),
                hint: "HOLD ←".into(),
                check: Check::WalkBack,
            },
            Step {
                title: "JUMP".into(),
                hint: "PRESS ↑".into(),
                check: Check::Jump,
            },
            Step {
                title: "CROUCH".into(),
                hint: "HOLD ↓".into(),
                check: Check::Crouch,
            },
            Step {
                title: "ATTACK".into(),
                hint: format!(
                    "PRESS {} {} {} OR {} {} {}",
                    b("a"),
                    b("b"),
                    b("c"),
                    b("x"),
                    b("y"),
                    b("z")
                ),
                check: Check::Attack,
            },
            Step {
                title: "GUARD".into(),
                hint: "HOLD ← AWAY FROM THE ENEMY".into(),
                check: Check::Guard,
            },
        ];
        let mut seen = std::collections::HashSet::new();
        for c in &data.cmd.commands {
            if !seen.insert(c.name.clone()) {
                continue;
            }
            let has_dir = c
                .elems
                .iter()
                .any(|e| e.keys.iter().any(|k| k.is_direction()));
            let has_btn = c
                .elems
                .iter()
                .any(|e| e.keys.iter().any(|k| !k.is_direction()));
            if c.elems.len() < 2 || !has_dir || !has_btn || c.elems.iter().any(|e| e.hold) {
                continue;
            }
            let hint = c
                .elems
                .iter()
                .map(|e| {
                    let keys: Vec<String> = e.keys.iter().map(|k| key_label(*k, keys)).collect();
                    let mut s = keys.join("+");
                    if e.release.is_some() {
                        s = format!("({s})");
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join(" , ");
            steps.push(Step {
                title: format!("SPECIAL: {}", c.name.to_uppercase()),
                hint,
                check: Check::Command(c.name.clone()),
            });
        }
        Self {
            steps,
            idx: 0,
            hold: 0,
            celebrate: 0,
            finished: false,
            start,
        }
    }

    /// Lines to show: title, hint, progress.
    pub fn text(&self) -> [String; 3] {
        if self.finished {
            return [
                "TUTORIAL COMPLETE".into(),
                format!("PRESS {} TO LEAVE", self.start),
                String::new(),
            ];
        }
        let s = &self.steps[self.idx];
        let title = if self.celebrate > 0 {
            "GOOD!".to_string()
        } else {
            s.title.clone()
        };
        [
            title,
            s.hint.clone(),
            format!(
                "STEP {} OF {}   {} SKIPS",
                self.idx + 1,
                self.steps.len(),
                self.start
            ),
        ]
    }

    pub fn skip(&mut self) {
        if !self.finished && self.celebrate == 0 {
            self.celebrate = 1;
        }
    }

    pub fn tick(&mut self, w: &World) {
        if self.finished {
            return;
        }
        if self.celebrate > 0 {
            self.celebrate -= 1;
            if self.celebrate == 0 {
                self.idx += 1;
                self.hold = 0;
                if self.idx >= self.steps.len() {
                    self.finished = true;
                }
            }
            return;
        }
        let Some(p) = w.players.first() else { return };
        let ok = match &self.steps[self.idx].check {
            Check::WalkFwd => {
                self.hold = if p.state_no == 20 && p.vel[0] > 0.0 {
                    self.hold + 1
                } else {
                    0
                };
                self.hold >= 20
            }
            Check::WalkBack => {
                self.hold = if p.state_no == 20 && p.vel[0] < 0.0 {
                    self.hold + 1
                } else {
                    0
                };
                self.hold >= 20
            }
            Check::Jump => (40..=52).contains(&p.state_no),
            Check::Crouch => {
                self.hold = if p.state_no == 11 { self.hold + 1 } else { 0 };
                self.hold >= 10
            }
            Check::Attack => p.move_type == rugen_cns::state::MoveType::A,
            Check::Guard => p.input.bb >= 30,
            Check::Command(name) => p.cmds.active(name),
        };
        if ok {
            self.celebrate = 60;
        }
    }
}
