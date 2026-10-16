//! Tutorial: guided steps that work for any character. Basics first, then
//! every special move from the character's `.cmd`, detected when performed.

use rugen_cns::cmd::Key as CmdKey;
use rugen_sim::{CharData, World};

use crate::keylabel::Labels;
use crate::lang::{t, tf};

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
    /// Run as the LESSONS of a Training match (Esc leaves, free training
    /// follows) instead of a tutorial match of its own.
    pub lessons: bool,
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
                title: t("lessons.walk_forward").into(),
                hint: t("lessons.hold_forward").into(),
                check: Check::WalkFwd,
            },
            Step {
                title: t("lessons.walk_back").into(),
                hint: t("lessons.hold_back").into(),
                check: Check::WalkBack,
            },
            Step {
                title: t("common.jump").into(),
                hint: t("lessons.press_up").into(),
                check: Check::Jump,
            },
            Step {
                title: t("lessons.crouch").into(),
                hint: t("lessons.hold_down").into(),
                check: Check::Crouch,
            },
            Step {
                title: t("online.attack").into(),
                hint: tf(
                    "lessons.press_abc_or_xyz",
                    &[
                        ("a", &b("a")),
                        ("b", &b("b")),
                        ("c", &b("c")),
                        ("x", &b("x")),
                        ("y", &b("y")),
                        ("z", &b("z")),
                    ],
                ),
                check: Check::Attack,
            },
            Step {
                title: t("lessons.guard").into(),
                hint: t("lessons.hold_back_away_from_the_enemy").into(),
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
                title: tf("lessons.special_name", &[("name", &c.name.to_uppercase())]),
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
            lessons: false,
        }
    }

    /// Lines to show: title, hint, progress.
    pub fn text(&self) -> [String; 3] {
        if self.finished && self.lessons {
            return [
                t("lessons.lessons_complete").into(),
                tf(
                    "lessons.free_training_next_key_now",
                    &[("key", &self.start)],
                ),
                String::new(),
            ];
        }
        if self.finished {
            return [
                t("lessons.tutorial_complete").into(),
                tf("lessons.press_key_to_leave", &[("key", &self.start)]),
                String::new(),
            ];
        }
        let s = &self.steps[self.idx];
        let title = if self.celebrate > 0 {
            t("lessons.good").to_string()
        } else {
            s.title.clone()
        };
        let progress = if self.lessons {
            tf(
                "lessons.lesson_i_of_n_key_skips_esc_leaves",
                &[
                    ("i", &(self.idx + 1)),
                    ("n", &self.steps.len()),
                    ("key", &self.start),
                ],
            )
        } else {
            tf(
                "lessons.step_i_of_n_key_skips",
                &[
                    ("i", &(self.idx + 1)),
                    ("n", &self.steps.len()),
                    ("key", &self.start),
                ],
            )
        };
        [title, s.hint.clone(), progress]
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
