//! Training lab: tools for the TRAINING mode only. They draw over the fight
//! or (the dummy) choose the inputs P2 is fed; they never touch the
//! simulation's state, so a training replay plays back exactly and nothing
//! here can reach an online match.
//!
//! The pause menu edits a [`TrainingCfg`]; the shell keeps it between
//! matches of a session.

use rugen_render::Renderer;
use serde_json::{Value, json};

/// What the training tools show and do. Edited from the pause menu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainingCfg {
    /// Clsn1 (attack) boxes red, Clsn2 (hurt) boxes blue, over each sprite.
    pub hitboxes: bool,
    /// Startup / active / recovery and advantage of each fighter's last move.
    pub frame_data: bool,
}

/// One adjustable row of a training panel: `(id, label, options, selected)`.
type Row = (&'static str, &'static str, Vec<&'static str>, usize);

const ON_OFF: [&str; 2] = ["OFF", "ON"];

impl TrainingCfg {
    fn display_rows(&self) -> Vec<Row> {
        vec![
            (
                "hitboxes",
                "HITBOXES",
                ON_OFF.to_vec(),
                usize::from(self.hitboxes),
            ),
            (
                "frame_data",
                "FRAME DATA",
                ON_OFF.to_vec(),
                usize::from(self.frame_data),
            ),
        ]
    }

    /// Set row `id` to option `index` (clamped, cycling). False for an unknown row.
    fn set(&mut self, id: &str, index: usize) -> bool {
        match id {
            "hitboxes" => self.hitboxes = index % 2 == 1,
            "frame_data" => self.frame_data = index % 2 == 1,
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

/// How a move met the other fighter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contact {
    Hit,
    Block,
}

/// One fighter as the frame meter sees it on one tick (read from the sim
/// after the step; see [`sample`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    pub state_no: i32,
    /// `MoveType = A`.
    pub attacking: bool,
    /// `MoveType = H` (hit or guarding).
    pub hurt: bool,
    pub ctrl: bool,
    /// The current animation frame has a Clsn1 (attack) box.
    pub active: bool,
    /// Frozen in hit pause / a Pause: the tick does not count as a move frame.
    pub frozen: bool,
    /// The last contact of the fighter's move (`MoveContact`).
    pub contact: Option<Contact>,
}

/// Root player `i` of `world` for the frame meter.
pub fn sample(world: &rugen_sim::World, i: usize) -> Option<Sample> {
    use rugen_cns::state::MoveType;
    use rugen_sim::hit::MoveContact;
    let p = world.players.get(i)?;
    let active = world
        .data
        .get(p.anim_owner)
        .and_then(|d| d.air.get(p.anim.action))
        .and_then(|a| p.anim.frame(a))
        .is_some_and(|f| !f.clsn1.is_empty());
    Some(Sample {
        state_no: p.state_no,
        attacking: p.move_type == MoveType::A,
        hurt: p.move_type == MoveType::H,
        ctrl: p.ctrl,
        active,
        frozen: p.hit_pause_time > 0 || p.pause_bool,
        contact: match p.mctype {
            MoveContact::Hit => Some(Contact::Hit),
            MoveContact::Guarded => Some(Contact::Block),
            MoveContact::None | MoveContact::Reversed => None,
        },
    })
}

/// Frame data of one move, as measured.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameData {
    /// The move's state number.
    pub state: i32,
    /// Frames up to and including the first one with a Clsn1 (`None`: no attack box).
    pub startup: Option<u32>,
    /// Frames from the first to the last one with a Clsn1.
    pub active: Option<u32>,
    /// Frames after the last active one until control returns (`None`: cancelled).
    pub recovery: Option<u32>,
    /// Frames the attacker can act before the other fighter, after a hit / a block.
    pub on_hit: Option<i32>,
    pub on_block: Option<i32>,
}

/// The move being measured.
#[derive(Clone, Debug, Default)]
struct Track {
    state: i32,
    /// Move frames so far (frozen ticks do not count); 1 on the first frame.
    t: u32,
    first: Option<u32>,
    last: Option<u32>,
    /// Move frame on which control came back.
    end: Option<u32>,
    /// World frame on which the attacker / the defender could act again.
    free: Option<u32>,
    opp_free: Option<u32>,
    contact: Option<Contact>,
    /// The move hit or was blocked.
    touched: bool,
    /// World frames since the move started (gives up on endless moves).
    age: u32,
}

/// Longest a move (and the other fighter's recovery) is followed, in frames.
const TRACK_LIMIT: u32 = 600;

/// Measures one fighter's moves: when it attacks (`MoveType = A`), the
/// frames up to its first attack box, how long it has one, and how long
/// until it has control again; when the move hits or is blocked, who can
/// act first afterwards. Reads the sim, never writes it.
#[derive(Clone, Debug, Default)]
pub struct FrameMeter {
    track: Option<Track>,
    prev: Sample,
    /// The last move measured (what the panel shows).
    pub last: Option<FrameData>,
}

impl FrameMeter {
    /// One simulated tick: `me` attacks, `opp` defends; `frame` is the world frame.
    pub fn observe(&mut self, frame: u32, me: Sample, opp: Sample) {
        let prev = std::mem::replace(&mut self.prev, me);
        // a new move: the first attack frame, or a cancel into another attack state
        let starts = me.attacking
            && (!prev.attacking
                || (me.state_no != prev.state_no
                    && self.track.as_ref().is_some_and(|t| t.end.is_none())));
        if starts {
            self.finish();
            self.track = Some(Track {
                state: me.state_no,
                ..Default::default()
            });
        }
        let Some(t) = self.track.as_mut() else {
            return;
        };
        t.age += 1;
        if t.end.is_none() {
            if me.hurt {
                // hit out of it: no data
                self.track = None;
                return;
            }
            if !me.frozen || t.t == 0 {
                t.t += 1;
            }
            if me.active && !me.frozen {
                t.first.get_or_insert(t.t);
                t.last = Some(t.t);
            }
            // control back (not on the move's own first frame): it is over;
            // landing and other recovery states without control still count
            if me.ctrl && t.t > 1 {
                t.end = Some(t.t);
                t.free = Some(frame);
            }
        }
        // this move connected (its contact resets with every new state)
        if me.contact.is_some() {
            t.contact = me.contact;
            t.touched = true;
        }
        if t.touched {
            if opp.hurt {
                t.opp_free = None;
            } else if t.opp_free.is_none() && opp.ctrl {
                t.opp_free = Some(frame);
            }
        }
        let opp_done = !t.touched || t.opp_free.is_some();
        if t.end.is_some() && opp_done || t.age > TRACK_LIMIT {
            self.finish();
        }
    }

    /// Publish the move being measured (complete or not).
    fn finish(&mut self) {
        let Some(t) = self.track.take() else {
            return;
        };
        let advantage = match (t.free, t.opp_free) {
            (Some(a), Some(b)) => Some(b as i32 - a as i32),
            _ => None,
        };
        let mut fd = FrameData {
            state: t.state,
            startup: t.first,
            active: t.first.zip(t.last).map(|(f, l)| l - f + 1),
            recovery: t.end.zip(t.last).map(|(e, l)| e.saturating_sub(l + 1)),
            on_hit: None,
            on_block: None,
        };
        // the same move again keeps what the other outcome measured
        if let Some(old) = self.last.as_ref().filter(|o| o.state == t.state) {
            fd.on_hit = old.on_hit;
            fd.on_block = old.on_block;
        }
        match (t.contact, advantage) {
            (Some(Contact::Hit), Some(a)) => fd.on_hit = Some(a),
            (Some(Contact::Block), Some(a)) => fd.on_block = Some(a),
            _ => {}
        }
        self.last = Some(fd);
    }
}

/// Live training state of one match.
#[derive(Clone, Debug, Default)]
pub struct Training {
    pub cfg: TrainingCfg,
    /// Frame data of P1's and P2's moves.
    pub meters: [FrameMeter; 2],
}

impl Training {
    pub fn new(cfg: TrainingCfg) -> Self {
        Self {
            cfg,
            ..Default::default()
        }
    }

    /// After every simulated tick: measure (reads the world only).
    pub fn observe(&mut self, world: &rugen_sim::World) {
        let (Some(a), Some(b)) = (sample(world, 0), sample(world, 1)) else {
            return;
        };
        self.meters[0].observe(world.frame, a, b);
        self.meters[1].observe(world.frame, b, a);
    }
}

/// Pixel text with its left edge (or right edge, `right`) at `x`.
fn text_at(r: &mut Renderer, t: &str, x: f32, y: f32, px: f32, right: bool, color: [f32; 4]) {
    let w = t.chars().count() as f32 * 4.0 * px;
    let cx = if right { x - w / 2.0 } else { x + w / 2.0 };
    crate::play::draw_text(r, t, [cx, y], px, color);
}

/// Where the training overlays sit, in the fight's HUD units.
pub struct Hud<'a> {
    /// Screen units (`0..screen_w`, `0..screen_h`) to drawing coordinates.
    pub at: &'a dyn Fn(f32, f32) -> [f32; 2],
    /// Drawing units per screen unit.
    pub u: f32,
    pub screen_w: f32,
    pub screen_h: f32,
}

const PANEL_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.62];
const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.96];
const DIM: [f32; 4] = [0.75, 0.78, 0.85, 0.9];

impl Training {
    /// The overlays the settings ask for (drawing only).
    pub fn draw(&self, r: &mut Renderer, hud: &Hud) {
        if self.cfg.frame_data {
            self.draw_frame_data(r, hud);
        }
    }

    /// FRAME DATA: a small panel per fighter under the life bars.
    fn draw_frame_data(&self, r: &mut Renderer, hud: &Hud) {
        // pixel size: the same on screen whatever the stage's resolution
        let k = (hud.screen_h / 240.0).max(0.5);
        let px = k;
        let line = 7.0 * k;
        let pad = 3.0 * k;
        for side in 0..2 {
            let m = &self.meters[side];
            let (title, lines) = match &m.last {
                Some(fd) => (format!("P{} STATE {}", side + 1, fd.state), fd.lines()),
                None => (
                    format!("P{} FRAME DATA", side + 1),
                    ["ATTACK TO MEASURE A MOVE".to_string(), String::new()],
                ),
            };
            let chars = lines
                .iter()
                .chain(std::iter::once(&title))
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0) as f32;
            let w = chars * 4.0 * px + 2.0 * pad;
            let h = 3.0 * line + 2.0 * pad - 2.0 * k;
            let top = 50.0 * k;
            let (x0, x1) = if side == 0 {
                (6.0 * k, 6.0 * k + w)
            } else {
                (hud.screen_w - 6.0 * k - w, hud.screen_w - 6.0 * k)
            };
            r.rect((hud.at)(x0, top), (hud.at)(x1, top + h), PANEL_BG);
            let tx = |r: &mut Renderer, t: &str, row: f32, c: [f32; 4]| {
                let y = top + pad + row * line;
                let p = (hud.at)(if side == 0 { x0 + pad } else { x1 - pad }, y);
                text_at(r, t, p[0], p[1], px * hud.u, side == 1, c);
            };
            tx(r, &title, 0.0, DIM);
            tx(r, &lines[0], 1.0, INK);
            tx(r, &lines[1], 2.0, INK);
        }
    }
}

/// Signed advantage: `+3`, `-2`, `0`.
fn signed(v: i32) -> String {
    if v > 0 {
        format!("+{v}")
    } else {
        v.to_string()
    }
}

impl FrameData {
    /// The panel's lines: `STARTUP 5  ACTIVE 3  RECOVERY 12`, `HIT +2  BLOCK -4`.
    pub fn lines(&self) -> [String; 2] {
        let n = |v: Option<u32>| v.map_or("-".to_string(), |v| v.to_string());
        let a = |v: Option<i32>| v.map_or("-".to_string(), signed);
        [
            format!(
                "STARTUP {}  ACTIVE {}  RECOVERY {}",
                n(self.startup),
                n(self.active),
                n(self.recovery)
            ),
            format!("ON HIT {}  ON BLOCK {}", a(self.on_hit), a(self.on_block)),
        ]
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

    fn idle() -> Sample {
        Sample {
            ctrl: true,
            ..Default::default()
        }
    }

    fn attack(active: bool) -> Sample {
        Sample {
            state_no: 200,
            attacking: true,
            active,
            ..Default::default()
        }
    }

    /// Feed `me` / `opp` pairs from world frame `from`; returns the next frame.
    fn feed(m: &mut FrameMeter, from: u32, seq: &[(Sample, Sample)]) -> u32 {
        for (k, (a, b)) in seq.iter().enumerate() {
            m.observe(from + k as u32, *a, *b);
        }
        from + seq.len() as u32
    }

    /// A whiffed jab: two frames of startup, two active, five of recovery.
    #[test]
    fn a_whiff_gives_startup_active_recovery() {
        let mut m = FrameMeter::default();
        let mut seq = vec![(idle(), idle())];
        seq.push((attack(false), idle()));
        seq.push((attack(false), idle()));
        seq.push((attack(true), idle()));
        seq.push((attack(true), idle()));
        seq.extend(std::iter::repeat_n((attack(false), idle()), 5));
        seq.push((idle(), idle()));
        feed(&mut m, 100, &seq);
        let fd = m.last.clone().expect("measured");
        assert_eq!(fd.state, 200);
        assert_eq!(
            (fd.startup, fd.active, fd.recovery),
            (Some(3), Some(2), Some(5))
        );
        assert_eq!((fd.on_hit, fd.on_block), (None, None), "it touched nothing");
        assert_eq!(fd.lines()[0], "STARTUP 3  ACTIVE 2  RECOVERY 5");
        assert_eq!(fd.lines()[1], "ON HIT -  ON BLOCK -");
    }

    /// A hit: hit pause frames are not move frames, and the advantage is
    /// how much sooner the attacker acts than the defender.
    #[test]
    fn a_hit_and_a_block_give_the_advantage() {
        let mut m = FrameMeter::default();
        let hit = |active, frozen| Sample {
            contact: Some(Contact::Hit),
            frozen,
            ..attack(active)
        };
        let hurt = Sample {
            hurt: true,
            ..Default::default()
        };
        let mut seq = vec![(idle(), idle()), (attack(false), idle())];
        seq.push((attack(false), idle()));
        seq.push((hit(true, false), hurt));
        seq.extend(std::iter::repeat_n((hit(true, true), hurt), 4));
        seq.push((hit(true, false), hurt));
        seq.extend(std::iter::repeat_n((hit(false, false), hurt), 6));
        // the attacker acts again; the defender 3 frames later
        seq.push((idle(), hurt));
        seq.push((idle(), hurt));
        seq.push((idle(), hurt));
        seq.push((idle(), idle()));
        let next = feed(&mut m, 0, &seq);
        let fd = m.last.clone().expect("measured");
        assert_eq!(
            (fd.startup, fd.active, fd.recovery),
            (Some(3), Some(2), Some(6))
        );
        assert_eq!(fd.on_hit, Some(3));
        assert_eq!(fd.lines()[1], "ON HIT +3  ON BLOCK -");
        // the same move blocked: the defender acts 2 frames first; the hit stays
        let block = |active| Sample {
            contact: Some(Contact::Block),
            ..attack(active)
        };
        let guard = Sample {
            hurt: true,
            ..Default::default()
        };
        let mut seq = vec![(attack(false), idle()), (attack(false), idle())];
        seq.push((block(true), guard));
        seq.push((block(false), guard));
        seq.push((block(false), idle()));
        seq.push((block(false), idle()));
        seq.push((idle(), idle()));
        feed(&mut m, next, &seq);
        let fd = m.last.clone().expect("measured");
        assert_eq!(fd.on_block, Some(-2));
        assert_eq!(fd.on_hit, Some(3), "kept from the hit");
        assert_eq!(fd.lines()[1], "ON HIT +3  ON BLOCK -2");
    }

    /// Hit out of a move: nothing to show; a cancel shows the first move cut short.
    #[test]
    fn interrupted_moves() {
        let mut m = FrameMeter::default();
        let hurt = Sample {
            hurt: true,
            ..Default::default()
        };
        feed(
            &mut m,
            0,
            &[
                (idle(), idle()),
                (attack(false), idle()),
                (hurt, idle()),
                (hurt, idle()),
            ],
        );
        assert_eq!(m.last, None);
        let special = Sample {
            state_no: 1000,
            ..attack(false)
        };
        feed(
            &mut m,
            10,
            &[
                (idle(), idle()),
                (attack(true), idle()),
                (attack(false), idle()),
                (special, idle()),
            ],
        );
        let fd = m.last.clone().expect("the cancelled jab");
        assert_eq!(fd.state, 200);
        assert_eq!(
            (fd.startup, fd.active, fd.recovery),
            (Some(1), Some(1), None)
        );
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
