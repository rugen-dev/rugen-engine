//! TEAM MODE step (`ui/teams.toml`, mockup "v2 · Team mode step"): before
//! the fighter grid of VS MODE and WATCH each side picks SINGLE, TURNS,
//! SIMUL or TAG (←→) and a team size of 2–4 (↑↓), then READY. The fighter
//! grid then has that many slots per side. ARENA and the CHALLENGE runs
//! never get here (their fights are 1v1 for balance).
//!
//! The players drive the step directly (two at once in VS, one cursor for
//! both sides in WATCH); the UI engine only draws it and turns clicks into
//! `tm:` / `size:` actions.

use rugen_sim::Buttons;
use rugen_sim::team::{MAX_MEMBERS, TeamMode};
use serde_json::{Value, json};

use crate::game::Mode;
use crate::roster::Team;

/// Ticks between both sides being ready and the fighter grid.
const GO_TICKS: u32 = 20;

/// One side of the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Side {
    /// Index into [`TeamMode::ALL`].
    mode: usize,
    /// Team size for the team modes (2..=MAX_MEMBERS).
    size: usize,
    ready: bool,
}

impl Side {
    fn team(&self) -> Team {
        let m = TeamMode::ALL[self.mode];
        (m, if m == TeamMode::Single { 1 } else { self.size })
    }
}

/// What a tick of the step asks the screen to do.
#[derive(Debug, PartialEq, Eq)]
pub enum StepOut {
    Nothing,
    /// Something changed (sound: `true` = a confirm, `false` = a move).
    Changed(bool),
    /// Both sides are ready: on to the fighter grid with these teams and
    /// these computer-controlled sides.
    Done([Team; 2], [bool; 2]),
    /// Backed out of the step.
    Leave,
}

pub struct TeamStep {
    pub mode: Mode,
    sides: [Side; 2],
    /// Sides the computer plays (VS: HUMAN / CPU per side; WATCH: both).
    cpu: [bool; 2],
    /// Both ready: ticks left before the grid.
    go_in: Option<u32>,
    /// What each side presses for TAG (the rules line).
    tag_keys: [String; 2],
}

/// Card texts per mode: name, description lines.
const CARDS: [(&str, &str); 4] = [
    ("SINGLE", "ONE FIGHTER, CLASSIC 1-ON-1"),
    ("TURNS", "ONE AFTER ANOTHER, WINNER STAYS IN"),
    ("SIMUL", "ALL ON SCREEN AT ONCE"),
    ("TAG", "SWAP PARTNERS MID-FIGHT"),
];

/// Pictogram rectangles on a 56 x 56 grid: (x, y, w, h, partner). A
/// partner is drawn in the muted ink; SINGLE has none.
fn pictogram(mode: TeamMode) -> &'static [(u8, u8, u8, u8, bool)] {
    match mode {
        TeamMode::Single => &[
            (22, 6, 12, 12, false),
            (20, 20, 16, 22, false),
            (20, 42, 6, 12, false),
            (30, 42, 6, 12, false),
        ],
        TeamMode::Turns => &[
            (9, 9, 10, 10, false),
            (8, 21, 12, 18, false),
            (25, 9, 10, 10, true),
            (24, 21, 12, 18, true),
            (40, 9, 10, 10, true),
            (39, 21, 12, 18, true),
            (8, 46, 38, 3, false),
            (44, 43, 4, 9, false),
        ],
        TeamMode::Simul => &[
            (12, 6, 10, 10, false),
            (9, 18, 16, 22, false),
            (9, 40, 6, 12, false),
            (19, 40, 6, 12, false),
            (34, 6, 10, 10, false),
            (31, 18, 16, 22, false),
            (31, 40, 6, 12, false),
            (41, 40, 6, 12, false),
        ],
        TeamMode::Tag => &[
            (11, 9, 10, 10, false),
            (10, 21, 12, 20, false),
            (35, 9, 10, 10, true),
            (34, 21, 12, 20, true),
            (16, 48, 26, 3, false),
            (16, 44, 3, 6, false),
            (39, 44, 3, 6, false),
            (36, 42, 6, 3, false),
        ],
    }
}

impl TeamStep {
    /// The step for `mode`, starting from `last` (the teams and CPU sides
    /// picked the last time) when there is one.
    pub fn new(mode: Mode, last: Option<([Team; 2], [bool; 2])>, tag_keys: [String; 2]) -> Self {
        let cpu = match (mode, last) {
            (Mode::Versus, Some((_, c))) => c,
            (Mode::Versus, None) => [false, false],
            _ => [true, true],
        };
        let last = last.map(|l| l.0);
        let side = |t: Option<Team>| {
            let (m, n) = t.unwrap_or((TeamMode::Single, 1));
            Side {
                mode: TeamMode::ALL.iter().position(|x| *x == m).unwrap_or(0),
                size: n.clamp(2, MAX_MEMBERS),
                ready: false,
            }
        };
        Self {
            mode,
            sides: [side(last.map(|l| l[0])), side(last.map(|l| l[1]))],
            cpu,
            go_in: None,
            tag_keys,
        }
    }

    /// Two players choose at once, each their own side (VS, both HUMAN);
    /// otherwise one cursor does 1P, then 2P.
    fn dual(&self) -> bool {
        self.mode == Mode::Versus && !self.cpu[0] && !self.cpu[1]
    }

    /// The side the single cursor works on: 1P until it is ready.
    fn cursor_side(&self) -> usize {
        usize::from(self.sides[0].ready)
    }

    fn human(&self, side: usize) -> bool {
        self.mode == Mode::Versus && !self.cpu[side]
    }

    /// VS: HUMAN / CPU sides step HUMAN VS HUMAN → HUMAN VS CPU → CPU VS
    /// CPU → CPU VS HUMAN (a side of its own by a click). Choices start over.
    fn toggle(&mut self, side: Option<usize>) -> bool {
        if self.mode != Mode::Versus {
            return false;
        }
        self.cpu = match side {
            Some(s) => {
                let mut c = self.cpu;
                c[s] = !c[s];
                c
            }
            None => match self.cpu {
                [false, false] => [false, true],
                [false, true] => [true, true],
                [true, true] => [true, false],
                [true, false] => [false, false],
            },
        };
        for s in &mut self.sides {
            s.ready = false;
        }
        self.go_in = None;
        true
    }

    /// The teams chosen so far.
    pub fn teams(&self) -> [Team; 2] {
        [self.sides[0].team(), self.sides[1].team()]
    }

    fn arm(&mut self) {
        self.go_in = (self.sides[0].ready && self.sides[1].ready).then_some(GO_TICKS);
    }

    /// One side's buttons pressed this tick.
    fn press(&mut self, side: usize, b: Buttons) -> StepOut {
        // X: HUMAN / CPU (VS)
        if b.x && self.toggle(None) {
            return StepOut::Changed(false);
        }
        let dual = self.dual();
        let s = &mut self.sides[side];
        let accept = b.a || b.c || b.y || b.z || b.s;
        if b.b {
            if s.ready {
                s.ready = false;
                self.arm();
                return StepOut::Changed(false);
            }
            // one cursor on 2P: back to 1P's choice
            if !dual && side == 1 && self.sides[0].ready {
                self.sides[0].ready = false;
                self.arm();
                return StepOut::Changed(false);
            }
            // P1 backs out of the screen (P2's back only un-readies)
            return if side == 0 || !dual {
                StepOut::Leave
            } else {
                StepOut::Nothing
            };
        }
        if s.ready {
            return StepOut::Nothing;
        }
        if accept {
            s.ready = true;
            self.arm();
            return StepOut::Changed(true);
        }
        let n = TeamMode::ALL.len();
        let mut moved = false;
        if b.l {
            s.mode = (s.mode + n - 1) % n;
            moved = true;
        }
        if b.r {
            s.mode = (s.mode + 1) % n;
            moved = true;
        }
        if TeamMode::ALL[s.mode] != TeamMode::Single {
            if b.u && s.size < MAX_MEMBERS {
                s.size += 1;
                moved = true;
            }
            if b.d && s.size > 2 {
                s.size -= 1;
                moved = true;
            }
        }
        if moved {
            StepOut::Changed(false)
        } else {
            StepOut::Nothing
        }
    }

    /// One tick: the players' presses (VS: each their own side; WATCH: either
    /// drives the cursor's side), then the countdown once both are ready.
    pub fn tick(&mut self, pressed: [Buttons; 2]) -> StepOut {
        let mut out = StepOut::Nothing;
        let note = |o: StepOut, out: &mut StepOut| match o {
            StepOut::Nothing => {}
            StepOut::Changed(c) => {
                if !matches!(out, StepOut::Leave | StepOut::Changed(true)) {
                    *out = StepOut::Changed(c);
                }
            }
            other => *out = other,
        };
        if self.dual() {
            for (side, b) in pressed.into_iter().enumerate() {
                let o = self.press(side, b);
                note(o, &mut out);
            }
        } else {
            let b = Buttons::from_bits(pressed[0].bits() | pressed[1].bits());
            let o = self.press(self.cursor_side(), b);
            note(o, &mut out);
        }
        if matches!(out, StepOut::Leave) {
            return out;
        }
        if let Some(left) = self.go_in.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.go_in = None;
                return StepOut::Done(self.teams(), self.cpu);
            }
        }
        out
    }

    /// A click: `tm:<side>:<mode>` picks a card (again: READY),
    /// `size:<side>:<n>` a team size, `cpu:<side>:0` flips HUMAN / CPU.
    /// True when it changed something.
    pub fn action(&mut self, a: &str) -> bool {
        let mut parts = a.split(':');
        let (Some(head), Some(side), Some(v)) = (parts.next(), parts.next(), parts.next()) else {
            return false;
        };
        let (Ok(side), Ok(v)) = (side.parse::<usize>(), v.parse::<usize>()) else {
            return false;
        };
        if head == "cpu" && side < 2 {
            return self.toggle(Some(side));
        }
        if side > 1 || (!self.dual() && side != self.cursor_side()) {
            return false;
        }
        let s = &mut self.sides[side];
        if s.ready {
            return false;
        }
        match head {
            "tm" if v < TeamMode::ALL.len() => {
                if s.mode == v {
                    s.ready = true;
                } else {
                    s.mode = v;
                }
            }
            "size" if (2..=MAX_MEMBERS).contains(&v) => s.size = v,
            _ => return false,
        }
        self.arm();
        true
    }

    /// The rules line under a side's cards.
    fn rules(&self, side: usize) -> String {
        let human = self.human(side);
        match TeamMode::ALL[self.sides[side].mode] {
            TeamMode::Single => "ONE FIGHTER EACH: THE CLASSIC 1 ON 1.".into(),
            TeamMode::Turns => {
                "ONE FIGHTER AT A TIME. THE WINNER STAYS AND KEEPS ITS LIFE; THE NEXT ONE COMES IN WITH THE FALLEN ONE'S POWER.".into()
            }
            TeamMode::Simul if human => {
                "YOUR TEAM FIGHTS TOGETHER. YOUR PARTNERS ARE CPU. THE ROUND IS LOST IF YOUR OWN FIGHTER FALLS.".into()
            }
            TeamMode::Simul => "THE WHOLE TEAM FIGHTS AT ONCE, SIDE BY SIDE.".into(),
            TeamMode::Tag if human => format!(
                "PRESS TAG ({}) TO SWAP. THE WAITING PARTNER GETS RED LIFE BACK.",
                self.tag_keys[side].to_uppercase()
            ),
            TeamMode::Tag => "THE CPU SWAPS PARTNERS. THE WAITING ONE GETS RED LIFE BACK.".into(),
        }
    }

    fn side_json(&self, side: usize) -> Value {
        let s = self.sides[side];
        let (mode, size) = s.team();
        let active = !s.ready && (self.dual() || side == self.cursor_side());
        let cards: Vec<Value> = TeamMode::ALL
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let on = i == s.mode;
                // focused while choosing, filled once ready, dimmed beside a ready pick
                let lit = on && !s.ready;
                let chosen = on && s.ready;
                let dim = !on && s.ready;
                let pix: Vec<Value> = pictogram(*m)
                    .iter()
                    .map(|&(x, y, w, h, partner)| {
                        json!({
                            "x": x, "y": y, "w": w, "h": h,
                            "main": !partner && !lit && !chosen && !dim,
                            "red": !partner && lit,
                            "white": !partner && chosen,
                            "mute": partner || dim,
                        })
                    })
                    .collect();
                let sizes: Vec<Value> = (2..=MAX_MEMBERS)
                    .map(|n| {
                        json!({
                            "label": n.to_string(),
                            "on": n == s.size,
                            "off": n != s.size,
                            "press": format!("size:{side}:{n}"),
                        })
                    })
                    .collect();
                json!({
                    "name": CARDS[i].0,
                    "desc": CARDS[i].1,
                    "press": format!("tm:{side}:{i}"),
                    "plain": !lit && !chosen && !dim,
                    "lit": lit,
                    "chosen": chosen,
                    "dim": dim,
                    "pix": pix,
                    "sizes": sizes,
                    "show_sizes": on && *m != TeamMode::Single,
                })
            })
            .collect();
        let other = 1 - side;
        let slots: Vec<Value> = (0..size).map(|_| json!({})).collect();
        let next = if s.ready && !self.sides[other].ready {
            format!("WAITING FOR {}P", other + 1)
        } else if s.ready {
            "READY: PICK THE FIGHTERS NEXT".to_string()
        } else {
            crate::uiscreen::plural(
                size,
                "SLOT TO FILL ON THE GRID",
                "SLOTS TO FILL ON THE GRID",
            )
        };
        let tag = format!("{}P", side + 1);
        let cpu = self.cpu[side];
        json!({
            "tag": tag,
            "toggle": self.mode == Mode::Versus,
            "fixed": self.mode != Mode::Versus,
            "control": if cpu { "CPU" } else { "HUMAN" },
            "human": !cpu,
            "cpu": cpu,
            "flip": format!("cpu:{side}:0"),
            "status": if s.ready { "READY ✓" } else if active { "CHOOSING..." } else { "" },
            "ready": s.ready,
            "choosing": !s.ready,
            "active": active,
            "idle": !active,
            "cards": cards,
            "title": if size > 1 {
                format!("{} · {} FIGHTERS", mode.as_str().to_uppercase(), size)
            } else {
                "SINGLE · 1 FIGHTER".to_string()
            },
            "rules": self.rules(side),
            "slots": slots,
            "next": next,
        })
    }

    /// The data context of `ui/teams.toml`.
    pub fn data(&self) -> Value {
        json!({
            "title": self.mode.label(),
            "p1": self.side_json(0),
            "p2": self.side_json(1),
            "note": if self.dual() { "EACH SIDE PICKS ITS OWN MODE" } else { "1P FIRST, THEN 2P" },
            "toggle": self.mode == Mode::Versus,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: impl Fn(&mut Buttons)) -> Buttons {
        let mut b = Buttons::default();
        f(&mut b);
        b
    }

    /// VS: each player picks a mode (←→) and a size (↑↓) for their own
    /// side, READY locks it; both ready leads to the fighter grid.
    #[test]
    fn each_side_picks_its_own_team() {
        let mut t = TeamStep::new(Mode::Versus, None, ["N".into(), "8".into()]);
        assert_eq!(t.teams(), [(TeamMode::Single, 1); 2]);
        let right = press(|b| b.r = true);
        let up = press(|b| b.u = true);
        let ok = press(|b| b.a = true);
        // P1: TURNS, then SIMUL of 3; P2: TAG (left wraps around) of 2
        t.tick([right, press(|b| b.l = true)]);
        t.tick([right, Buttons::default()]);
        t.tick([up, Buttons::default()]);
        assert_eq!(t.teams(), [(TeamMode::Simul, 3), (TeamMode::Tag, 2)]);
        assert_eq!(t.tick([ok, Buttons::default()]), StepOut::Changed(true));
        assert_eq!(
            t.tick([right, Buttons::default()]),
            StepOut::Nothing,
            "1P is locked"
        );
        assert_eq!(t.data()["p1"]["next"], "WAITING FOR 2P");
        t.tick([Buttons::default(), ok]);
        let mut out = StepOut::Nothing;
        for _ in 0..GO_TICKS {
            out = t.tick([Buttons::default(); 2]);
            if out != StepOut::Nothing {
                break;
            }
        }
        assert_eq!(
            out,
            StepOut::Done([(TeamMode::Simul, 3), (TeamMode::Tag, 2)], [false, false])
        );
    }

    /// WATCH: one cursor does 1P, then 2P; back walks it back.
    #[test]
    fn one_cursor_does_both_sides_in_watch() {
        let mut t = TeamStep::new(Mode::Watch, None, Default::default());
        let ok = press(|b| b.a = true);
        let back = press(|b| b.b = true);
        let right = press(|b| b.r = true);
        t.tick([right, Buttons::default()]);
        t.tick([Buttons::default(), ok]);
        assert_eq!(t.teams()[0], (TeamMode::Turns, 2));
        t.tick([right, Buttons::default()]);
        t.tick([right, Buttons::default()]);
        assert_eq!(t.teams()[1], (TeamMode::Simul, 2), "the cursor moved to 2P");
        assert_eq!(
            t.data()["p2"]["rules"],
            "THE WHOLE TEAM FIGHTS AT ONCE, SIDE BY SIDE."
        );
        assert_eq!(t.tick([back, Buttons::default()]), StepOut::Changed(false));
        assert!(!t.sides[0].ready, "back un-readies 1P");
        assert_eq!(t.tick([back, Buttons::default()]), StepOut::Leave);
    }

    /// VS against the computer: X steps HUMAN / CPU; with a CPU side one
    /// cursor does both sides (either player's keys), like WATCH.
    #[test]
    fn versus_sides_can_be_cpu() {
        let mut t = TeamStep::new(Mode::Versus, None, Default::default());
        let x = press(|b| b.x = true);
        let ok = press(|b| b.a = true);
        assert_eq!(t.cpu, [false, false], "two humans by default");
        assert_eq!(t.tick([x, Buttons::default()]), StepOut::Changed(false));
        assert_eq!(t.cpu, [false, true]);
        assert_eq!(t.data()["p2"]["control"], "CPU");
        // P1 readies its side, then picks the CPU's
        t.tick([ok, Buttons::default()]);
        t.tick([press(|b| b.r = true), Buttons::default()]);
        assert_eq!(t.teams()[1], (TeamMode::Turns, 2));
        t.tick([ok, Buttons::default()]);
        let mut out = StepOut::Nothing;
        for _ in 0..GO_TICKS {
            out = t.tick([Buttons::default(); 2]);
            if out != StepOut::Nothing {
                break;
            }
        }
        assert_eq!(
            out,
            StepOut::Done([(TeamMode::Single, 1), (TeamMode::Turns, 2)], [false, true])
        );
        // clicks flip one side; WATCH has no toggle
        assert!(t.action("cpu:0:0"));
        assert_eq!(t.cpu, [true, true]);
        let mut w = TeamStep::new(Mode::Watch, None, Default::default());
        assert!(!w.action("cpu:0:0"));
        assert_eq!(w.tick([x, Buttons::default()]), StepOut::Nothing);
        // the last choice comes back
        let last = Some(([(TeamMode::Single, 1); 2], [false, true]));
        assert_eq!(
            TeamStep::new(Mode::Versus, last, Default::default()).cpu,
            [false, true]
        );
    }

    /// Clicks: a card, the same card again for READY, a size square.
    #[test]
    fn clicks_pick_cards_and_sizes() {
        let mut t = TeamStep::new(
            Mode::Versus,
            Some(([(TeamMode::Tag, 4), (TeamMode::Single, 1)], [false, false])),
            Default::default(),
        );
        assert_eq!(t.teams()[0], (TeamMode::Tag, 4), "the last pick comes back");
        assert!(t.action("tm:1:1"));
        assert!(t.action("size:1:3"));
        assert_eq!(t.teams()[1], (TeamMode::Turns, 3));
        assert!(t.action("tm:1:1"));
        assert!(t.sides[1].ready);
        assert!(!t.action("size:1:2"), "locked once ready");
        assert!(!t.action("size:0:9"));
    }
}
