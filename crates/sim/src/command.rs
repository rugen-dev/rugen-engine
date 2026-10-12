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

/// A compiled command: the immutable part, shared by every snapshot of the
/// world (rollback clones only the small runtime state, see [`CommandList`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandDef {
    pub name: String,
    pub steps: Vec<Step>,
    pub max_time: i32,
    pub max_buf_time: i32,
    pub loop_order: Vec<usize>,
}

/// Runtime matching state of one command (per-step flags and timers live in
/// [`CommandList`]'s flat arrays).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CmdState {
    pub cur_time: i32,
    pub buffered: i32,
    pub complete_frame: bool,
}

/// One command's definition and its runtime state, borrowed for a step.
struct Run<'a> {
    def: &'a CommandDef,
    st: &'a mut CmdState,
    completed: &'a mut [bool],
    step_timers: &'a mut [i32],
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

impl CommandDef {
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
            loop_order,
            steps,
        }
    }
}

impl Run<'_> {
    fn clear(&mut self, buf_reset: bool) {
        self.st.cur_time = 0;
        if buf_reset {
            self.st.buffered = 0;
        }
        self.completed.iter_mut().for_each(|c| *c = false);
        self.step_timers.iter_mut().for_each(|t| *t = 0);
    }

    fn greater_fail(&self, i: usize, ib: &InputBuffer) -> bool {
        let check = |k: Key| -> bool {
            if ib.state(k, false, false) == 1 {
                return !self.def.steps[i]
                    .keys
                    .iter()
                    .any(|sk| sk.key == k && !sk.tilde);
            }
            if ib.state(k, true, false) == 1 {
                return !self.def.steps[i]
                    .keys
                    .iter()
                    .any(|sk| sk.key == k && sk.tilde);
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

    fn step(&mut self, ib: &InputBuffer) {
        if self.st.buffered > 0 {
            self.st.buffered -= 1;
        }
        if self.def.steps.is_empty() {
            return;
        }
        let mut anydone = false;
        for i in 0..self.def.steps.len() {
            if self.completed[i] {
                self.step_timers[i] += 1;
                anydone = true;
            }
        }
        if anydone {
            self.st.cur_time += 1;
        } else if self.st.cur_time > 0 {
            self.clear(false);
        }
        for idx in 0..self.def.loop_order.len() {
            let i = self.def.loop_order[idx];
            if i > 0 && !self.completed[i - 1] {
                continue;
            }
            let mut matched = true;
            for k in &self.def.steps[i].keys {
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
            if self.def.steps[i].greater
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
                    self.st.cur_time = 0;
                }
            }
        }
        self.st.complete_frame = *self.completed.last().unwrap_or(&false);
        if !self.st.complete_frame && self.st.cur_time < self.def.max_time {
            return;
        }
        self.clear(false);
        if self.st.complete_frame {
            self.st.buffered = self.st.buffered.max(self.def.max_buf_time);
        }
    }
}

/// Every command of a character, compiled once and shared.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CommandTable {
    pub defs: Vec<CommandDef>,
    /// Start of each command's steps in the flat per-step arrays.
    offsets: Vec<usize>,
    /// Command indices per name (a CMD may define one name several times).
    /// Only looked up, never iterated (iteration order is not deterministic).
    by_name: std::collections::HashMap<String, Vec<u16>>,
    steps: usize,
}

impl CommandTable {
    pub fn new(defs: Vec<CommandDef>) -> Self {
        let mut offsets = Vec::with_capacity(defs.len());
        let mut by_name: std::collections::HashMap<String, Vec<u16>> = Default::default();
        let mut steps = 0;
        for (i, d) in defs.iter().enumerate() {
            offsets.push(steps);
            steps += d.steps.len();
            by_name.entry(d.name.clone()).or_default().push(i as u16);
        }
        Self {
            defs,
            offsets,
            by_name,
            steps,
        }
    }
}

/// All of a player's commands. `active(name)` answers the `Command` trigger.
///
/// The compiled commands sit behind an `Arc`; only the runtime state (three
/// flat arrays of plain values) is copied when the world is snapshotted for
/// rollback or a helper copies its root's commands.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CommandList {
    pub table: std::sync::Arc<CommandTable>,
    pub state: Vec<CmdState>,
    completed: Vec<bool>,
    step_timers: Vec<i32>,
}

impl Clone for CommandList {
    fn clone(&self) -> Self {
        Self {
            table: self.table.clone(),
            state: self.state.clone(),
            completed: self.completed.clone(),
            step_timers: self.step_timers.clone(),
        }
    }

    // reuses the buffers (keyctrl helpers copy their root's commands every frame)
    fn clone_from(&mut self, src: &Self) {
        if !std::sync::Arc::ptr_eq(&self.table, &src.table) {
            self.table = src.table.clone();
        }
        self.state.clone_from(&src.state);
        self.completed.clone_from(&src.completed);
        self.step_timers.clone_from(&src.step_timers);
    }
}

impl CommandList {
    pub fn compile(cmd: &cmdfmt::Cmd) -> Self {
        Self::from_defs(cmd.commands.iter().map(CommandDef::compile).collect())
    }

    pub fn from_defs(defs: Vec<CommandDef>) -> Self {
        let table = CommandTable::new(defs);
        Self {
            state: vec![CmdState::default(); table.defs.len()],
            completed: vec![false; table.steps],
            step_timers: vec![0; table.steps],
            table: std::sync::Arc::new(table),
        }
    }

    pub fn len(&self) -> usize {
        self.state.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    fn run(&mut self, i: usize) -> Run<'_> {
        let def = &self.table.defs[i];
        let o = self.table.offsets[i];
        let n = def.steps.len();
        Run {
            def,
            st: &mut self.state[i],
            completed: &mut self.completed[o..o + n],
            step_timers: &mut self.step_timers[o..o + n],
        }
    }

    pub fn step(&mut self, ib: &InputBuffer) {
        let mut done: Vec<usize> = Vec::new();
        for i in 0..self.state.len() {
            let mut c = self.run(i);
            c.step(ib);
            if c.st.complete_frame {
                c.clear(false);
                done.push(i);
                c.st.complete_frame = false;
            }
        }
        if done.is_empty() {
            return;
        }
        let table = self.table.clone();
        for d in done {
            for &j in &table.by_name[&table.defs[d].name] {
                self.run(j as usize).clear(false);
            }
        }
    }

    pub fn reset(&mut self) {
        for i in 0..self.state.len() {
            self.run(i).clear(true);
        }
    }

    pub fn active(&self, name: &str) -> bool {
        self.table
            .by_name
            .get(name)
            .is_some_and(|ix| ix.iter().any(|&i| self.state[i as usize].buffered > 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, s: &str, time: i32, buf: i32) -> CommandList {
        CommandList::from_defs(vec![CommandDef::compile(&cmdfmt::Command {
            name: name.into(),
            elems: cmdfmt::parse_command(s).unwrap(),
            time,
            buffer_time: buf,
        })])
    }

    fn tick(c: &mut CommandList, ib: &mut InputBuffer, d: &str, b: &str) -> bool {
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
        c.state[0].buffered > 0
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
        c.reset();
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
