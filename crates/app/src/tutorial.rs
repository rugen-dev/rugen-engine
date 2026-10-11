//! Tutorial: guided steps that work for any character. Basics first, then
//! every special move from the character's `.cmd`, detected when performed.

use rugen_cns::cmd::Key as CmdKey;
use rugen_sim::{CharData, World};

use crate::config::PlayerKeys;

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
}

fn key_label(k: CmdKey, keys: &PlayerKeys) -> String {
    let n = keys.names();
    let up = |s: &str| s.to_uppercase();
    match k {
        CmdKey::U => "UP".into(),
        CmdKey::D => "DOWN".into(),
        CmdKey::F => "FWD".into(),
        CmdKey::B => "BACK".into(),
        CmdKey::UF => "UP-FWD".into(),
        CmdKey::UB => "UP-BACK".into(),
        CmdKey::DF => "DOWN-FWD".into(),
        CmdKey::DB => "DOWN-BACK".into(),
        CmdKey::A => format!("A({})", up(n[4])),
        CmdKey::Bt => format!("B({})", up(n[5])),
        CmdKey::C => format!("C({})", up(n[6])),
        CmdKey::X => format!("X({})", up(n[7])),
        CmdKey::Y => format!("Y({})", up(n[8])),
        CmdKey::Z => format!("Z({})", up(n[9])),
        CmdKey::S => format!("START({})", up(n[10])),
    }
}

impl Tutorial {
    pub fn new(data: &CharData, keys: &PlayerKeys) -> Self {
        let n = keys.names();
        let up = |s: &str| s.to_uppercase();
        let mut steps = vec![
            Step {
                title: "WALK FORWARD".into(),
                hint: format!("HOLD {}", up(n[3])),
                check: Check::WalkFwd,
            },
            Step {
                title: "WALK BACK".into(),
                hint: format!("HOLD {}", up(n[2])),
                check: Check::WalkBack,
            },
            Step {
                title: "JUMP".into(),
                hint: format!("PRESS {}", up(n[0])),
                check: Check::Jump,
            },
            Step {
                title: "CROUCH".into(),
                hint: format!("HOLD {}", up(n[1])),
                check: Check::Crouch,
            },
            Step {
                title: "ATTACK".into(),
                hint: format!(
                    "PRESS {} {} {} OR {} {} {}",
                    up(n[4]),
                    up(n[5]),
                    up(n[6]),
                    up(n[7]),
                    up(n[8]),
                    up(n[9])
                ),
                check: Check::Attack,
            },
            Step {
                title: "GUARD".into(),
                hint: format!("HOLD {} AWAY FROM THE ENEMY", up(n[2])),
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
        }
    }

    /// Lines to show: title, hint, progress.
    pub fn text(&self) -> [String; 3] {
        if self.finished {
            return [
                "TUTORIAL COMPLETE".into(),
                "PRESS START TO LEAVE".into(),
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
                "STEP {} OF {}   START SKIPS",
                self.idx + 1,
                self.steps.len()
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
