//! The arena panels: a replay of a `rugen-gym` match shows, next to each
//! fighter, the player behind it at work:
//!
//! - its badge and name, and `THINKING n MS` with a bar: the match does
//!   not wait, this is how long its next plan has been in the making;
//! - its brain as a ladder of rules, each with its conditions in words:
//!   the rule that decides now is a tile in its card's color, lit when it
//!   has just fired;
//! - how the last move ended (`RULE 2 COMBO 1 HIT`);
//! - what the player said when it wrote the brain.
//!
//! A color means one thing only, the type of a card, and a legend at the
//! bottom of the screen names each. The clock is always amber.
//!
//! The panel is compact by default (three steps of the plan); the whole
//! plan shows with `RUGEN_ARENA_HUD=full`.
//!
//! Everything is read from the replay's `gym` section by the frame on
//! screen, so pausing, stepping and rewinding show the right moment.

use rugen_render::Renderer;

use crate::replay::{Gym, GymPlan, GymStep};
use crate::training::Hud;

const PANEL_BG: [f32; 4] = [0.03, 0.03, 0.05, 0.74];
const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.96];
const DIM: [f32; 4] = [0.66, 0.70, 0.78, 0.9];
const NAME: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The clock, and nothing else.
const CLOCK: [f32; 4] = [1.0, 0.62, 0.15, 1.0];
/// An answer that named no card: no type, so no color.
const NO_CARD: [f32; 4] = [0.22, 0.23, 0.27, 0.95];

/// Characters per line of a panel (the built-in font: 4 units a glyph).
const COLS: usize = 26;
/// Ticks a new card takes to slide in.
const SLIDE: f32 = 10.0;
/// The thinking bar is full after this many ticks (2000 ms).
const BAR_TICKS: f32 = 120.0;

/// The card types, in the legend's order, with the one color each has.
const KINDS: [(&str, [f32; 4]); 8] = [
    ("stance", [0.16, 0.58, 0.38, 0.95]),
    ("attack", [0.80, 0.22, 0.18, 0.95]),
    ("projectile", [0.20, 0.50, 0.92, 0.95]),
    ("super", [0.80, 0.58, 0.05, 0.95]),
    ("special", [0.60, 0.32, 0.85, 0.95]),
    ("boost", [0.10, 0.62, 0.66, 0.95]),
    ("charge", [0.55, 0.78, 1.0, 0.95]),
    ("move", [0.38, 0.42, 0.50, 0.95]),
];

/// A player's badge: the family of models its name belongs to, as two
/// letters on that family's own color. Original marks, not the vendors'
/// logos. A name of no known family gets its first two letters on grey;
/// a bot gets a robot's face.
pub fn badge(name: &str) -> (String, [f32; 4]) {
    const FAMILIES: [(&[&str], &str, [f32; 3]); 12] = [
        (
            &["claude", "opus", "sonnet", "haiku", "fable"],
            "CL",
            [0.80, 0.42, 0.26],
        ),
        (
            &["gpt", "openai", "o1", "o3", "o4"],
            "GP",
            [0.10, 0.60, 0.48],
        ),
        (&["gemini", "gemma"], "GE", [0.26, 0.48, 0.95]),
        (&["qwen"], "QW", [0.42, 0.30, 0.90]),
        (&["deepseek"], "DS", [0.22, 0.36, 0.86]),
        (&["llama", "meta"], "LL", [0.05, 0.50, 0.88]),
        (
            &["mistral", "mixtral", "codestral"],
            "MI",
            [0.95, 0.48, 0.08],
        ),
        (&["grok", "xai"], "GK", [0.30, 0.30, 0.34]),
        (&["kimi", "moonshot"], "KI", [0.12, 0.14, 0.30]),
        (&["glm", "zhipu"], "GL", [0.16, 0.42, 0.78]),
        (&["phi", "copilot"], "PH", [0.00, 0.47, 0.83]),
        (&["nova", "titan"], "NV", [0.90, 0.55, 0.05]),
    ];
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("bot") {
        return ("BOT".into(), [0.42, 0.46, 0.52, 1.0]);
    }
    // whole words of the name, so "o3-mini" is OpenAI's and "phi" is not
    // found inside "delphi"
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    for (keys, letters, c) in FAMILIES {
        if words.iter().any(|w| keys.iter().any(|k| w.starts_with(k))) {
            return (letters.into(), [c[0], c[1], c[2], 1.0]);
        }
    }
    let letters: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(2)
        .collect::<String>()
        .to_uppercase();
    (letters, [0.34, 0.36, 0.42, 1.0])
}

/// Draw a badge with its corner at `(x, y)` (screen units), `k` a pixel;
/// its width. A family with a pixel mark shows it (9x9 on an 11x11 ground, see
/// [`crate::arenalogos`]); the others their letters.
fn draw_badge(r: &mut Renderer, hud: &Hud, name: &str, x: f32, y: f32, k: f32) -> f32 {
    let (letters, color) = badge(name);
    if let Some(m) = crate::arenalogos::mark(&letters) {
        let n = crate::arenalogos::SIZE;
        let rgb = |c: u32| {
            [
                (c >> 16 & 0xff) as f32 / 255.0,
                (c >> 8 & 0xff) as f32 / 255.0,
                (c & 0xff) as f32 / 255.0,
                1.0,
            ]
        };
        // a pixel of ground all round the mark
        let side = (n + 2) as f32 * k;
        r.rect(
            (hud.at)(x, y),
            (hud.at)(x + side, y + side),
            rgb(crate::arenalogos::GROUND),
        );
        for (i, &c) in m.iter().enumerate().filter(|(_, c)| **c != 0) {
            let (px, py) = (x + (1 + i % n) as f32 * k, y + (1 + i / n) as f32 * k);
            r.rect((hud.at)(px, py), (hud.at)(px + k, py + k), rgb(c));
        }
        return side + 2.0 * k;
    }
    // no mark: the letters, centred in the same square
    let y = y + 2.0 * k;
    let (w, h) = (11.0 * k, 7.0 * k);
    r.rect((hud.at)(x, y), (hud.at)(x + w, y + h), color);
    if letters == "BOT" {
        // a robot's face: an antenna, two eyes, a mouth
        let dot = |r: &mut Renderer, cx: f32, cy: f32, dw: f32, dh: f32| {
            r.rect(
                (hud.at)(x + cx * k, y + cy * k),
                (hud.at)(x + (cx + dw) * k, y + (cy + dh) * k),
                INK,
            );
        };
        dot(r, 5.0, 0.0, 1.0, 1.0);
        dot(r, 2.0, 2.0, 2.0, 2.0);
        dot(r, 7.0, 2.0, 2.0, 2.0);
        dot(r, 3.0, 5.0, 5.0, 1.0);
    } else {
        let p = (hud.at)(x + 1.5 * k, y + k);
        let half = letters.chars().count() as f32 * 4.0 * k * hud.u / 2.0;
        crate::play::draw_text(r, &letters, [p[0] + half, p[1]], k * hud.u, INK);
    }
    w + 2.0 * k
}

fn kind_color(kind: &str) -> [f32; 4] {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(NO_CARD, |(_, c)| *c)
}

/// Ticks (60 a second) as whole milliseconds.
fn ms(ticks: u32) -> u32 {
    ticks * 1000 / 60
}

/// Text for the panel's font, which has letters, digits and little else:
/// comparisons are spelled out and apostrophes dropped.
fn plain(text: &str) -> String {
    text.replace(">=", " AT LEAST ")
        .replace("<=", " AT MOST ")
        .replace('<', " UNDER ")
        .replace('>', " OVER ")
        .replace(['\'', '\u{2019}'], "")
        .to_uppercase()
}

/// A card's name as a panel writes it, one character short of a line (a
/// card and a chip start inside the panel's edge).
fn shown(name: &str) -> String {
    name.replace('_', " ")
        .to_uppercase()
        .chars()
        .take(COLS - 1)
        .collect()
}

/// `text` in lines of at most `cols` characters, broken at spaces, at most
/// `rows` lines (the last one ends with `...` when text is left over).
pub fn wrap(text: &str, cols: usize, rows: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word: String = word.chars().take(cols).collect();
        let fits = lines
            .last()
            .is_some_and(|l| l.chars().count() + 1 + word.chars().count() <= cols);
        if fits {
            let l = lines.last_mut().expect("a line fits");
            l.push(' ');
            l.push_str(&word);
        } else if lines.len() < rows {
            lines.push(word);
        } else {
            // text is left over: the last line says so
            if let Some(l) = lines.last_mut() {
                let keep: String = l.chars().take(cols.saturating_sub(3)).collect();
                *l = format!("{keep}...");
            }
            break;
        }
    }
    lines
}

/// What a side's panel shows on a frame.
#[derive(Debug, PartialEq)]
pub struct Moment<'a> {
    /// Ticks since the side was shown the state, while its answer is out.
    pub thinking: Option<u32>,
    /// The plan that runs, and the step it is at (from 0); `None` once it
    /// is over.
    pub plan: Option<(&'a GymPlan, Option<usize>)>,
    /// The last step that ended.
    pub last: Option<&'a GymStep>,
    /// The side's latest answer, when it was refused.
    pub refused: Option<&'a GymPlan>,
}

/// The live file a match in progress is followed from (`rugen replay
/// --live`), set once at launch.
pub static LIVE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// One line of a live file, after its header.
#[derive(Debug, PartialEq)]
pub enum Live {
    /// A tick's inputs.
    Input([u16; 2]),
    /// A side was shown the state on this frame.
    Ask(usize, u32),
    /// An answer came in.
    Plan(GymPlan),
    /// A step ended.
    Step(GymStep),
    End,
}

impl Live {
    fn parse(line: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if let Some(i) = v.get("i") {
            let b = |n: usize| i.get(n)?.as_u64().map(|x| x as u16);
            return Some(Live::Input([b(0)?, b(1)?]));
        }
        if let Some(side) = v.get("ask") {
            let frame = v.get("frame")?.as_u64()? as u32;
            return Some(Live::Ask(side.as_u64()? as usize, frame));
        }
        if let Some(p) = v.get("plan") {
            return serde_json::from_value(p.clone()).ok().map(Live::Plan);
        }
        if let Some(s) = v.get("step") {
            return serde_json::from_value(s.clone()).ok().map(Live::Step);
        }
        v.get("end").map(|_| Live::End)
    }
}

/// A match in progress, read from the file `rugen-gym --live` writes: the
/// lines that came since the last look.
pub struct LiveFeed {
    file: std::io::BufReader<std::fs::File>,
    /// A line whose end has not been written yet.
    partial: String,
    ended: bool,
    /// Frames shown since the match ended.
    lingered: u32,
}

impl LiveFeed {
    /// Follow `path` from after its first line (the replay's header).
    pub fn open(path: &std::path::Path) -> std::io::Result<Self> {
        use std::io::BufRead as _;
        let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut header = String::new();
        file.read_line(&mut header)?;
        Ok(Self {
            file,
            partial: String::new(),
            ended: false,
            lingered: 0,
        })
    }

    /// The complete lines written since the last call.
    pub fn poll(&mut self) -> Vec<Live> {
        use std::io::BufRead as _;
        let mut out = Vec::new();
        loop {
            match self.file.read_line(&mut self.partial) {
                Ok(n) if n > 0 && self.partial.ends_with('\n') => {
                    if let Some(l) = Live::parse(self.partial.trim()) {
                        self.ended |= l == Live::End;
                        out.push(l);
                    }
                    self.partial.clear();
                }
                // nothing more yet (or half a line: its rest comes later)
                _ => return out,
            }
        }
    }

    /// The match is over and its last picture was held for a moment (asked
    /// once per frame with nothing left to play).
    pub fn finished(&mut self) -> bool {
        if self.ended {
            self.lingered += 1;
        }
        self.lingered > 180
    }
}

pub struct ArenaHud {
    gym: Gym,
    /// A match followed live: the frame each side was asked on, until its
    /// answer comes in.
    asked: [Option<u32>; 2],
    /// The whole plan (`RUGEN_ARENA_HUD=full`), for analysis; else three
    /// steps of it, so the panel ends above the fighters' heads.
    full: bool,
}

impl ArenaHud {
    pub fn new(gym: Gym) -> Self {
        Self {
            gym,
            asked: [None; 2],
            full: std::env::var("RUGEN_ARENA_HUD").is_ok_and(|v| v.eq_ignore_ascii_case("full")),
        }
    }

    /// A card's name on the panel and its kind: a move's label in the
    /// fighter's `.cmd`; a stance's id (its label is a sentence, written
    /// for the player) and a card that is not in the deck keep their id.
    fn card<'a>(&'a self, side: usize, id: &'a str) -> (&'a str, &'a str) {
        match self.gym.decks[side].iter().find(|c| c.id == id) {
            Some(c) if c.kind == "stance" => (id, "stance"),
            Some(c) => (c.label.as_str(), c.kind.as_str()),
            None => (id, ""),
        }
    }

    /// A line of a match followed live.
    /// The name of the player on `side` (a model, or the built-in AI).
    pub fn player(&self, side: usize) -> &str {
        self.gym.players.get(side).map_or("", String::as_str)
    }

    pub fn live(&mut self, line: Live) {
        match line {
            Live::Ask(side, frame) if side < 2 => self.asked[side] = Some(frame),
            Live::Plan(p) if p.side < 2 => {
                self.asked[p.side] = None;
                self.gym.plans.push(p);
            }
            Live::Step(s) => self.gym.steps.push(s),
            _ => {}
        }
    }

    pub fn moment(&self, side: usize, frame: u32) -> Moment<'_> {
        let mine = |p: &&GymPlan| p.side == side;
        // the next answer says when it was asked for; live, it is not in
        // yet and the question itself does
        let thinking = self
            .gym
            .plans
            .iter()
            .filter(mine)
            .find(|p| p.frame > frame)
            .map(|p| p.asked)
            .or(self.asked[side])
            .filter(|asked| *asked <= frame)
            .map(|asked| frame - asked);
        let steps = || {
            self.gym
                .steps
                .iter()
                .filter(move |s| s.side == side && s.frame <= frame)
        };
        // the plan that runs is the one of the last step that ended, at the
        // step that one led to (or stayed on, while its fighter is hit)
        let plan = steps().next_back().and_then(|s| {
            let plan = self.gym.plans.get(s.plan)?;
            let at = match s.to {
                None => Some(s.step.saturating_sub(1)),
                Some(0) => None,
                Some(k) => Some(k - 1),
            };
            Some((plan, at.filter(|k| *k < plan.steps.len())))
        });
        let latest = self
            .gym
            .plans
            .iter()
            .filter(mine)
            .rfind(|p| p.frame <= frame);
        Moment {
            thinking,
            plan,
            last: steps().rfind(|s| s.step > 0),
            refused: latest.filter(|p| !p.ok),
        }
    }

    /// `combo`: the hits each side has landed in a row (the game's own
    /// counters are not drawn over the panels).
    pub fn draw(&self, r: &mut Renderer, hud: &Hud, frame: u32, combo: [i32; 2]) {
        // pixel size: the same on screen whatever the stage's resolution
        let k = (hud.screen_h / 240.0).max(0.5);
        let px = k;
        let pad = 2.0 * k;
        let cols = 38usize;
        let w = cols as f32 * 4.0 * px + 2.0 * pad;
        let inner = w - 2.0 * pad;
        let row = 7.0 * k;
        let top = 50.0 * k;
        let text = |r: &mut Renderer, t: &str, x: f32, y: f32, max: usize, color: [f32; 4]| {
            let t: String = t.chars().take(max).collect();
            let p = (hud.at)(x, y);
            let half = t.chars().count() as f32 * 4.0 * px * hud.u / 2.0;
            crate::play::draw_text(r, &t, [p[0] + half, p[1]], px * hud.u, color);
        };
        for (side, &hits) in combo.iter().enumerate() {
            let m = self.moment(side, frame);
            let x0 = if side == 0 {
                6.0 * k
            } else {
                hud.screen_w - 6.0 * k - w
            };
            let x = x0 + pad;
            // the rows of the ladder: the rules of the brain (or the steps
            // of the plan) around the one that decides now
            let rows_n = if self.full { 10 } else { 3 };
            let (list, at): (&[crate::replay::GymPlanStep], Option<usize>) = match m.plan {
                Some((plan, at)) => (&plan.steps, at),
                None => (&[], None),
            };
            let first = at
                .unwrap_or(0)
                .saturating_sub(1)
                .min(list.len().saturating_sub(rows_n));
            let rows: Vec<(usize, &crate::replay::GymPlanStep)> =
                list.iter().enumerate().skip(first).take(rows_n).collect();
            let lines = rows_n;
            let h = pad
                + 8.0 * k + 4.5 * k // badge, name, clock
                + lines as f32 * 9.0 * k // the rules
                + row // how the last one ended
                + row // why
                + pad;
            r.rect((hud.at)(x0, top), (hud.at)(x0 + w, top + h), PANEL_BG);
            let mut y = top + pad;

            // the badge and the name, and the clock at the panel's other end
            let name = self.gym.players[side].to_uppercase();
            // the game's own computer player writes no brain and is not asked
            let cpu = self.gym.players[side].to_lowercase().starts_with("cpu")
                && !self.gym.plans.iter().any(|p| p.side == side);
            let clock = if cpu {
                String::new()
            } else {
                format!("THINKING {} MS", ms(m.thinking.unwrap_or(0)))
            };
            let bw = draw_badge(r, hud, &name, x, y, k);
            text(r, &name, x + bw, y + k, cols - clock.len() - 5, NAME);
            let cx = x + inner - clock.len() as f32 * 4.0 * px;
            text(r, &clock, cx, y + k, cols, CLOCK);
            y += 8.0 * k;
            let full = (m.thinking.unwrap_or(0) as f32 / BAR_TICKS).min(1.0);
            // the bar starts after the badge, which is as tall as both rows
            let (bx, bar) = (x + bw, inner - bw);
            r.rect(
                (hud.at)(bx, y),
                (hud.at)(x + inner, y + k),
                [1.0, 1.0, 1.0, 0.12],
            );
            r.rect((hud.at)(bx, y), (hud.at)(bx + full * bar, y + k), CLOCK);
            y += 4.5 * k;

            // the ladder: the rule that decides now as a tile in its card's
            // color, the others as lines
            let brain = m.plan.is_some_and(|(p, _)| p.brain);
            if rows.is_empty() {
                r.rect_outline(
                    (hud.at)(x, y),
                    (hud.at)(x + inner, y + 8.0 * k),
                    k * hud.u,
                    [1.0, 1.0, 1.0, 0.18],
                );
                let what = if cpu {
                    "THE FIGHTERS OWN BUILT-IN AI"
                } else if m.refused.is_some() {
                    "ANSWER REFUSED"
                } else {
                    "NO BRAIN YET"
                };
                text(r, what, x + 1.5 * k, y + 1.5 * k, cols - 1, DIM);
            }
            for (n, step) in &rows {
                let (label, kind) = self.card(side, &step.card);
                // a card with several names: the first, so its conditions fit
                let name = shown(label.split(" / ").next().unwrap_or(label));
                let line = format!("{} {}  {}", n + 1, name, plain(&step.note));
                if at == Some(*n) {
                    let since = m.last.map_or(0, |s| frame.saturating_sub(s.frame)) as f32;
                    let glow = (1.0 - since / (SLIDE * 2.0)).max(0.0);
                    r.rect(
                        (hud.at)(x, y),
                        (hud.at)(x + inner, y + 8.0 * k),
                        kind_color(kind),
                    );
                    // a bright edge when it has just fired
                    if glow > 0.0 {
                        r.rect_outline(
                            (hud.at)(x, y),
                            (hud.at)(x + inner, y + 8.0 * k),
                            k * hud.u,
                            [1.0, 1.0, 1.0, glow],
                        );
                    }
                    text(r, &line, x + 1.5 * k, y + 1.5 * k, cols - 1, INK);
                } else {
                    let mut chip = kind_color(kind);
                    chip[3] = 0.7;
                    r.rect(
                        (hud.at)(x, y + 1.5 * k),
                        (hud.at)(x + 2.0 * k, y + 6.5 * k),
                        chip,
                    );
                    text(r, &line, x + 4.0 * k, y + 1.5 * k, cols - 2, DIM);
                }
                y += 9.0 * k;
            }
            y += (lines - rows.len().min(lines)) as f32 * 9.0 * k;

            // how the last step ended and where the plan went
            if let Some(s) = m.last {
                let age = frame.saturating_sub(s.frame) as f32;
                let fresh = (1.0 - age / 90.0).max(0.0);
                let went = match s.to {
                    Some(0) => " - PLAN OVER".to_string(),
                    Some(n) => format!(" TO {n}"),
                    None => String::new(),
                };
                // a brain's rule names the move that ended; a plan's step
                // says where the plan went
                let line = if brain {
                    let (label, _) = self.card(side, &s.card);
                    format!(
                        "RULE {} {} {}",
                        s.step,
                        shown(label),
                        s.result.replace('_', " ").to_uppercase()
                    )
                } else {
                    format!(
                        "STEP {} {}{went}",
                        s.step,
                        s.result.replace('_', " ").to_uppercase()
                    )
                };
                let c = 0.6 + 0.4 * fresh;
                text(r, &line, x, y + k, cols, [c, c, c, 0.95]);
            }
            y += row;

            // its combo, while it lasts, at the row's other end
            if hits >= 2 {
                let c = format!("{hits} HITS");
                let cx = x + inner - c.len() as f32 * 4.0 * px;
                text(r, &c, cx, y - row + k, cols, NAME);
            }

            // why: the player's words for the plan that runs, or why its
            // last answer was refused
            let why = match (m.refused, m.plan) {
                (Some(p), _) => Some(format!("REFUSED: {}", p.error)),
                (None, Some((p, _))) => Some(p.say.clone()),
                (None, None) => None,
            };
            if let Some(l) = why
                .as_deref()
                .map(|t| wrap(t, cols, 1))
                .and_then(|l| l.into_iter().next())
            {
                text(r, &plain(&l), x, y + k, cols, DIM);
            }
        }
        self.legend(r, hud, k);
    }

    /// What each color is, in one line at the bottom of the screen
    /// (between the power bars): only the types the two decks have.
    fn legend(&self, r: &mut Renderer, hud: &Hud, k: f32) {
        let used: Vec<&(&str, [f32; 4])> = KINDS
            .iter()
            .filter(|(kind, _)| self.gym.decks.iter().flatten().any(|c| c.kind == *kind))
            .collect();
        // a square, a gap, the name, a wider gap
        let width = |name: &str| (5.0 + name.len() as f32 * 4.0 + 8.0) * k;
        let total: f32 = used.iter().map(|(name, _)| width(name)).sum::<f32>() - 8.0 * k;
        let y = hud.screen_h - 9.0 * k;
        let mut x = (hud.screen_w - total) / 2.0;
        r.rect(
            (hud.at)(x - 3.0 * k, y - 2.0 * k),
            (hud.at)(x + total + 3.0 * k, y + 7.0 * k),
            PANEL_BG,
        );
        for (name, color) in used {
            r.rect((hud.at)(x, y), (hud.at)(x + 4.0 * k, y + 5.0 * k), *color);
            let p = (hud.at)(x + 5.0 * k, y);
            let label = name.to_uppercase();
            let half = label.len() as f32 * 4.0 * k * hud.u / 2.0;
            crate::play::draw_text(r, &label, [p[0] + half, p[1]], k * hud.u, INK);
            x += width(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{GymCard, GymPlanStep};

    fn gym() -> Gym {
        let card = |id: &str, kind: &str| GymCard {
            id: id.into(),
            label: id.replace('_', " "),
            kind: kind.into(),
            power: 0,
        };
        let plan = |side, asked, frame, ok: bool, cards: &[&str]| GymPlan {
            side,
            asked,
            frame,
            ok,
            say: "close in".into(),
            steps: cards
                .iter()
                .map(|c| GymPlanStep {
                    card: (*c).into(),
                    note: String::new(),
                })
                .collect(),
            brain: false,
            error: if ok {
                String::new()
            } else {
                "no card `x`".into()
            },
        };
        let step = |plan, step, frame, result: &str, to| GymStep {
            plan,
            side: 0,
            step,
            card: String::new(),
            frame,
            result: result.into(),
            to,
        };
        let deck = vec![
            card("approach", "stance"),
            card("block", "stance"),
            card("punch", "attack"),
        ];
        Gym {
            players: ["a".into(), "b".into()],
            decks: [deck.clone(), deck],
            plans: vec![
                plan(0, 100, 130, true, &["approach", "punch", "block"]),
                plan(1, 100, 400, true, &["block"]),
                plan(0, 131, 300, false, &[]),
            ],
            steps: vec![
                step(0, 0, 130, "start", Some(1)),
                step(0, 1, 180, "done", Some(2)),
                step(0, 2, 200, "interrupted", None),
                step(0, 2, 230, "i_am_hit", Some(3)),
                step(0, 3, 260, "done", Some(0)),
            ],
        }
    }

    #[test]
    fn a_frame_shows_the_plan_and_where_it_is() {
        let hud = ArenaHud::new(gym());
        let at = |f: u32| hud.moment(0, f).plan.map(|(p, at)| (p.steps.len(), at));
        assert_eq!(at(120), None, "no plan yet");
        assert_eq!(hud.moment(0, 120).thinking, Some(20), "asked at 100");
        assert_eq!(
            at(130),
            Some((3, Some(0))),
            "the plan took over at its step 1"
        );
        assert_eq!(at(190), Some((3, Some(1))), "step 1 done: at step 2");
        assert_eq!(
            at(210),
            Some((3, Some(1))),
            "hit during step 2: stays on it"
        );
        assert_eq!(at(240), Some((3, Some(2))), "the rule sent it to step 3");
        assert_eq!(at(270), Some((3, None)), "the plan is over");
        assert_eq!(
            hud.moment(0, 190).last.map(|s| s.result.as_str()),
            Some("done")
        );
        assert_eq!(
            hud.moment(0, 240).last.map(|s| s.result.as_str()),
            Some("i_am_hit")
        );
        // thinking about the next answer, which comes in refused
        assert_eq!(hud.moment(0, 200).thinking, Some(69));
        assert!(hud.moment(0, 299).refused.is_none());
        assert_eq!(
            hud.moment(0, 300).refused.map(|p| p.error.as_str()),
            Some("no card `x`")
        );
        // the slow side has no plan for a long time
        let m = hud.moment(1, 390);
        assert_eq!((m.plan, m.thinking), (None, Some(290)));
    }

    /// A match followed live: the lines of the feed, and a side thinking
    /// before its answer is known.
    #[test]
    fn a_live_match_is_followed_line_by_line() {
        assert_eq!(Live::parse(r#"{"i":[4,8]}"#), Some(Live::Input([4, 8])));
        assert_eq!(
            Live::parse(r#"{"ask":1,"frame":90}"#),
            Some(Live::Ask(1, 90))
        );
        assert_eq!(Live::parse(r#"{"end":true}"#), Some(Live::End));
        assert_eq!(Live::parse("{\"i\":[4"), None, "half a line");
        let mut hud = ArenaHud::new(Gym {
            plans: Vec::new(),
            steps: Vec::new(),
            ..gym()
        });
        hud.live(Live::Ask(0, 100));
        assert_eq!(
            hud.moment(0, 130).thinking,
            Some(30),
            "asked, no answer yet"
        );
        let plan = Live::parse(
            r#"{"plan":{"side":0,"asked":100,"frame":140,"ok":true,"say":"now","steps":[{"card":"punch","note":"hit 1"}]}}"#,
        )
        .unwrap();
        hud.live(plan);
        hud.live(Live::parse(r#"{"step":{"plan":0,"side":0,"step":0,"card":"","frame":140,"result":"start","to":1}}"#).unwrap());
        hud.live(Live::Ask(0, 141));
        let m = hud.moment(0, 150);
        assert_eq!(m.thinking, Some(9));
        assert_eq!(
            m.plan.map(|(p, at)| (p.say.as_str(), at)),
            Some(("now", Some(0)))
        );

        // the file is read as it grows, half lines kept for later
        let path = std::env::temp_dir().join(format!("rugen-live-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"header\":1}\n{\"i\":[1,2]}\n{\"i\":[3").unwrap();
        let mut feed = LiveFeed::open(&path).unwrap();
        assert_eq!(feed.poll(), [Live::Input([1, 2])]);
        assert_eq!(feed.poll(), []);
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(f, ",4]}}\n{{\"end\":true}}\n").unwrap();
        assert_eq!(feed.poll(), [Live::Input([3, 4]), Live::End]);
        assert!(!feed.finished(), "the last picture is held a moment");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_color_is_a_card_type_only() {
        assert_eq!(kind_color("stance"), KINDS[0].1);
        assert_eq!(kind_color(""), NO_CARD, "not a card: no type's color");
        let mut seen: Vec<[u32; 3]> = KINDS
            .iter()
            .map(|(_, c)| [c[0], c[1], c[2]].map(|v| (v * 100.0) as u32))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), KINDS.len(), "no two types share a color");
        assert_eq!(ms(60), 1000);
        assert_eq!(ms(27), 450);
    }

    #[test]
    fn players_get_their_familys_badge() {
        assert_eq!(badge("claude-opus-5.5").0, "CL");
        assert_eq!(badge("gpt-5").0, "GP");
        assert_eq!(badge("o3-mini").0, "GP");
        assert_eq!(badge("qwen3-max").0, "QW");
        assert_eq!(badge("deepseek-r2").0, "DS");
        assert_eq!(badge("bot-rush").0, "BOT");
        assert_eq!(badge("delphi-1").0, "DE");
        for letters in ["CL", "GP", "GE", "QW", "DS", "LL", "GK", "MI", "KI", "GL"] {
            let m = crate::arenalogos::mark(letters).expect(letters);
            let ink = m.iter().filter(|p| **p != 0).count();
            assert!((12..70).contains(&ink), "{letters}: {ink} pixels");
        }
        assert!(crate::arenalogos::mark("BOT").is_none());
    }

    #[test]
    fn notes_are_spelled_for_the_font() {
        assert_eq!(plain("until distance<60"), "UNTIL DISTANCE UNDER 60");
        assert_eq!(plain("if power>=1000"), "IF POWER AT LEAST 1000");
        assert_eq!(
            plain("I'm behind, so I'll close in"),
            "IM BEHIND, SO ILL CLOSE IN"
        );
    }

    #[test]
    fn words_wrap_and_long_text_is_cut() {
        assert_eq!(
            wrap("he is far so close in", 10, 3),
            ["he is far", "so close", "in"]
        );
        assert_eq!(
            wrap("one two three four five six", 8, 2),
            ["one two", "three..."]
        );
        assert_eq!(wrap("", 10, 3), Vec::<String>::new());
    }
}
