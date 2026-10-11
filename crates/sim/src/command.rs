//! Command matching (Ikemen GO `Command.Step`). Each command keeps per-step
//! completion flags and timers; `buffered > 0` means the command fired within
//! its buffer window.

use rugen_cns::cmd as cmdfmt;

use crate::input::{InputBuffer, Key};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepKey {
    pub key: Key,
    pub slash: bool,
    pub tilde: bool,
    pub dollar: bool,
    pub charge: i32,
}

impl StepKey {
    fn is_dir_release(&self) -> bool {
        self.tilde && self.key.is_direction()
    }
    fn is_button_press(&self) -> bool {
        !self.tilde && !self.key.is_direction()
    }
    fn is_button_release(&self) -> bool {
        self.tilde && !self.key.is_direction()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub keys: Vec<StepKey>,
    pub greater: bool,
}

impl Step {
    fn single_direction(&self) -> bool {
        self.keys.len() == 1 && self.keys[0].key.is_direction()
    }
    /// Can this step and the next complete in the same frame (direction → button)?
    fn dir_to_button(&self, next: &Step) -> bool {
        if next.keys.iter().any(|k| k.slash) {
            return false;
        }
        if self
            .keys
            .iter()
            .any(|k| k.is_button_press() || k.is_button_release())
        {
            return false;
        }
        for k in &self.keys {
            if next.keys.iter().any(|n| n.key == k.key) {
                return false;
            }
        }
        if next.keys.iter().any(|n| n.is_button_press()) {
            return true;
        }
        self.keys.iter().any(|k| k.is_dir_release())
            && next.keys.iter().any(|n| !n.is_dir_release())
    }
}

/// Compiled command plus its runtime matching state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub steps: Vec<Step>,
    pub max_time: i32,
    pub max_buf_time: i32,
    pub cur_time: i32,
    pub buffered: i32,
    pub completed: Vec<bool>,
    pub step_timers: Vec<i32>,
    pub loop_order: Vec<usize>,
    pub complete_frame: bool,
}

fn key_of(k: cmdfmt::Key) -> Key {
    match k {
        cmdfmt::Key::U => Key::U,
        cmdfmt::Key::D => Key::D,
        cmdfmt::Key::F => Key::F,
        cmdfmt::Key::B => Key::B,
        cmdfmt::Key::UF => Key::UF,
        cmdfmt::Key::UB => Key::UB,
        cmdfmt::Key::DF => Key::DF,
        cmdfmt::Key::DB => Key::DB,
        cmdfmt::Key::A => Key::A,
        cmdfmt::Key::Bt => Key::Bt,
        cmdfmt::Key::C => Key::C,
        cmdfmt::Key::X => Key::X,
        cmdfmt::Key::Y => Key::Y,
        cmdfmt::Key::Z => Key::Z,
        cmdfmt::Key::S => Key::S,
    }
}

impl Command {
    pub fn compile(c: &cmdfmt::Command) -> Self {
        let mut steps: Vec<Step> = c
            .elems
            .iter()
            .map(|e| Step {
                greater: e.strict,
                keys: e
                    .keys
                    .iter()
                    .map(|&k| StepKey {
                        key: key_of(k),
                        slash: e.hold,
                        tilde: e.release.is_some(),
                        dollar: e.four_way && key_of(k).is_direction(),
                        charge: e.release.unwrap_or(0),
                    })
                    .collect(),
            })
            .collect();
        // "F, F" → "F, >~F, >F" so one press cannot satisfy both steps
        if steps.len() >= 2
            && steps
                .windows(2)
                .any(|w| w[0].single_direction() && w[1].single_direction() && w[0] == w[1])
        {
            let mut out = vec![steps[0].clone()];
            for i in 1..steps.len() {
                let (prev, cur) = (&steps[i - 1], &steps[i]);
                if prev.single_direction() && cur.single_direction() && prev == cur {
                    let k = cur.keys[0];
                    out.push(Step {
                        greater: true,
                        keys: vec![StepKey {
                            tilde: !k.tilde,
                            charge: 0,
                            ..k
                        }],
                    });
                    out.push(Step {
                        greater: true,
                        keys: cur.keys.clone(),
                    });
                } else {
                    out.push(cur.clone());
                }
            }
            steps = out;
        }
        let n = steps.len();
        // reverse evaluation order, except forward runs of direction→button steps
        let mut loop_order = Vec::with_capacity(n);
        let mut i = n as isize - 1;
        while i >= 0 {
            let iu = i as usize;
            if iu > 0 && steps[iu - 1].dir_to_button(&steps[iu]) {
                let mut start = iu - 1;
                while start > 0 && steps[start - 1].dir_to_button(&steps[start]) {
                    start -= 1;
                }
                loop_order.extend(start..=iu);
                i = start as isize - 1;
            } else {
                loop_order.push(iu);
                i -= 1;
            }
        }
        Self {
            name: c.name.clone(),
            max_time: c.time,
            max_buf_time: c.buffer_time,
            cur_time: 0,
            buffered: 0,
            completed: vec![false; n],
            step_timers: vec![0; n],
            loop_order,
            steps,
            complete_frame: false,
        }
    }

    pub fn clear(&mut self, buf_reset: bool) {
        self.cur_time = 0;
        if buf_reset {
            self.buffered = 0;
        }
        self.completed.iter_mut().for_each(|c| *c = false);
        self.step_timers.iter_mut().for_each(|t| *t = 0);
    }

    fn greater_fail(&self, i: usize, ib: &InputBuffer) -> bool {
        let check = |k: Key| -> bool {
            if ib.state(k, false, false) == 1 {
                return !self.steps[i].keys.iter().any(|sk| sk.key == k && !sk.tilde);
            }
            if ib.state(k, true, false) == 1 {
                return !self.steps[i].keys.iter().any(|sk| sk.key == k && sk.tilde);
            }
            false
        };
        [
            Key::U,
            Key::D,
            Key::B,
            Key::F,
            Key::UF,
            Key::UB,
            Key::DF,
            Key::DB,
            Key::A,
            Key::Bt,
            Key::C,
            Key::X,
            Key::Y,
            Key::Z,
            Key::S,
        ]
        .into_iter()
        .any(check)
    }

    pub fn step(&mut self, ib: &InputBuffer) {
        if self.buffered > 0 {
            self.buffered -= 1;
        }
        if self.steps.is_empty() {
            return;
        }
        let mut anydone = false;
        for i in 0..self.steps.len() {
            if self.completed[i] {
                self.step_timers[i] += 1;
                anydone = true;
            }
        }
        if anydone {
            self.cur_time += 1;
        } else if self.cur_time > 0 {
            self.clear(false);
        }
        for idx in 0..self.loop_order.len() {
            let i = self.loop_order[idx];
            if i > 0 && !self.completed[i - 1] {
                continue;
            }
            let mut matched = true;
            for k in &self.steps[i].keys {
                let t = ib.state(k.key, k.tilde, k.dollar);
                let mut ok = if k.slash { t > 0 } else { t == 1 };
                if ok && k.charge > 1 && ib.charge(k.key, k.tilde, k.dollar) < k.charge {
                    ok = false;
                }
                if !ok {
                    matched = false;
                    break;
                }
            }
            if self.steps[i].greater
                && i > 0
                && self.completed[i - 1]
                && !self.completed[i]
                && self.greater_fail(i, ib)
            {
                matched = false;
                self.completed[i - 1] = false;
                self.step_timers[i - 1] = 0;
            }
            if matched {
                self.completed[i] = true;
                self.step_timers[i] = 0;
                if i > 0 {
                    self.completed[i - 1] = false;
                    self.step_timers[i - 1] = 0;
                }
                if i == 0 {
                    self.cur_time = 0;
                }
            }
        }
        self.complete_frame = *self.completed.last().unwrap_or(&false);
        if !self.complete_frame && self.cur_time < self.max_time {
            return;
        }
        self.clear(false);
        if self.complete_frame {
            self.buffered = self.buffered.max(self.max_buf_time);
        }
    }
}

/// All of a player's commands. `active(name)` answers the `Command` trigger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandList {
    pub commands: Vec<Command>,
}

impl CommandList {
    pub fn compile(cmd: &cmdfmt::Cmd) -> Self {
        Self {
            commands: cmd.commands.iter().map(Command::compile).collect(),
        }
    }

    pub fn step(&mut self, ib: &InputBuffer) {
        let mut done: Vec<String> = Vec::new();
        for c in &mut self.commands {
            c.step(ib);
            if c.complete_frame {
                c.clear(false);
                done.push(c.name.clone());
                c.complete_frame = false;
            }
        }
        for name in done {
            for c in self.commands.iter_mut().filter(|c| c.name == name) {
                c.clear(false);
            }
        }
    }

    pub fn reset(&mut self) {
        self.commands.iter_mut().for_each(|c| c.clear(true));
    }

    pub fn active(&self, name: &str) -> bool {
        self.commands
            .iter()
            .any(|c| c.name == name && c.buffered > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, s: &str, time: i32, buf: i32) -> Command {
        Command::compile(&cmdfmt::Command {
            name: name.into(),
            elems: cmdfmt::parse_command(s).unwrap(),
            time,
            buffer_time: buf,
        })
    }

    fn tick(c: &mut Command, ib: &mut InputBuffer, d: &str, b: &str) -> bool {
        ib.update(
            d.contains('U'),
            d.contains('D'),
            d.contains('B'),
            d.contains('F'),
            b.contains('a'),
            b.contains('b'),
            b.contains('c'),
            b.contains('x'),
            b.contains('y'),
            b.contains('z'),
            b.contains('s'),
        );
        c.step(ib);
        c.buffered > 0
    }

    #[test]
    fn single_button() {
        let mut c = cmd("x", "x", 1, 1);
        let mut ib = InputBuffer::default();
        assert!(tick(&mut c, &mut ib, "", "x"));
        assert!(!tick(&mut c, &mut ib, "", "x"), "held, not re-pressed");
        assert!(!tick(&mut c, &mut ib, "", ""));
        assert!(tick(&mut c, &mut ib, "", "x"));
    }

    #[test]
    fn qcf() {
        let mut c = cmd("QCF_x", "~D, DF, F, x", 15, 1);
        let mut ib = InputBuffer::default();
        assert!(!tick(&mut c, &mut ib, "D", ""));
        assert!(!tick(&mut c, &mut ib, "DF", ""));
        assert!(!tick(&mut c, &mut ib, "F", ""));
        assert!(tick(&mut c, &mut ib, "F", "x"), "QCF+x fires");
        // too slow: 20 idle frames between D and the rest
        c.clear(true);
        assert!(!tick(&mut c, &mut ib, "", ""));
        assert!(!tick(&mut c, &mut ib, "D", ""));
        for _ in 0..20 {
            tick(&mut c, &mut ib, "", "");
        }
        tick(&mut c, &mut ib, "DF", "");
        tick(&mut c, &mut ib, "F", "");
        assert!(!tick(&mut c, &mut ib, "F", "x"), "expired");
    }

    #[test]
    fn hold_and_double_tap() {
        let mut c = cmd("holdfwd", "/$F", 1, 1);
        let mut ib = InputBuffer::default();
        assert!(tick(&mut c, &mut ib, "F", ""));
        assert!(tick(&mut c, &mut ib, "F", ""), "hold stays true");
        assert!(tick(&mut c, &mut ib, "UF", ""), "$F ignores the diagonal");
        let mut c = cmd("FF", "F, F", 10, 1);
        let mut ib = InputBuffer::default();
        assert!(!tick(&mut c, &mut ib, "F", ""));
        assert!(
            !tick(&mut c, &mut ib, "F", ""),
            "holding is not a second tap"
        );
        assert!(!tick(&mut c, &mut ib, "", ""));
        assert!(tick(&mut c, &mut ib, "F", ""), "second tap");
    }
}
