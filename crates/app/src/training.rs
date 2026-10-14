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
    /// A column per fighter with its inputs and how long each was held.
    pub inputs: bool,
    /// What the dummy (P2) holds when nothing else drives it.
    pub stance: Stance,
    /// When the dummy guards.
    pub block: Block,
    /// AFTER BLOCK / HIT: play the recording slot as a reversal.
    pub reversal: bool,
    /// Recording slot RECORD / PLAY / LOOP and the reversal use (0..SLOTS).
    pub slot: usize,
}

/// The dummy's stance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stance {
    #[default]
    Stand,
    Crouch,
    Jump,
}

/// When the dummy guards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Block {
    #[default]
    None,
    All,
    /// Takes the first hit, then guards until it has been left alone a moment.
    AfterFirstHit,
    /// Guards each attack or not, at random.
    Random,
}

/// Recording slots.
pub const SLOTS: usize = 3;
/// Longest recording: 10 s.
pub const RECORD_FRAMES: usize = 600;

/// One adjustable row of a training panel: `(id, label, options, selected)`.
type Row = (&'static str, &'static str, Vec<&'static str>, usize);

const ON_OFF: [&str; 2] = ["OFF", "ON"];

impl TrainingCfg {
    fn dummy_rows(&self) -> Vec<Row> {
        vec![
            (
                "stance",
                "STANCE",
                vec!["STAND", "CROUCH", "JUMP"],
                self.stance as usize,
            ),
            (
                "block",
                "BLOCK",
                vec!["NONE", "ALL", "AFTER FIRST HIT", "RANDOM"],
                self.block as usize,
            ),
            (
                "reversal",
                "AFTER BLOCK / HIT",
                vec!["NOTHING", "REVERSAL SLOT"],
                usize::from(self.reversal),
            ),
            ("slot", "RECORDING SLOT", vec!["1", "2", "3"], self.slot),
        ]
    }

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
            (
                "inputs",
                "INPUT DISPLAY",
                ON_OFF.to_vec(),
                usize::from(self.inputs),
            ),
        ]
    }

    /// Set row `id` to option `index` (clamped, cycling). False for an unknown row.
    fn set(&mut self, id: &str, index: usize) -> bool {
        match id {
            "hitboxes" => self.hitboxes = index % 2 == 1,
            "frame_data" => self.frame_data = index % 2 == 1,
            "inputs" => self.inputs = index % 2 == 1,
            "stance" => self.stance = [Stance::Stand, Stance::Crouch, Stance::Jump][index.min(2)],
            "block" => {
                self.block =
                    [Block::None, Block::All, Block::AfterFirstHit, Block::Random][index.min(3)]
            }
            "reversal" => self.reversal = index % 2 == 1,
            "slot" => self.slot = index.min(SLOTS - 1),
            _ => return false,
        }
        true
    }

    fn row(&self, id: &str) -> Option<Row> {
        self.display_rows()
            .into_iter()
            .chain(self.dummy_rows())
            .find(|r| r.0 == id)
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

    /// The DUMMY panel's rows for the pause screen.
    pub fn dummy_json(&self) -> Value {
        Value::Array(self.dummy_rows().into_iter().map(row_json).collect())
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

/// One line of the input display: a direction and buttons held together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// Numpad notation relative to facing: 6 forward, 4 back, 2 down, 5 neutral.
    pub dir: u8,
    /// A B C X Y Z S held.
    pub buttons: [bool; 7],
    /// Ticks this combination has been held.
    pub frames: u32,
}

impl Held {
    /// What a fighter's input buffer (the one its commands read) holds now.
    pub fn of(b: &rugen_sim::input::InputBuffer) -> Self {
        let (u, d) = (b.ub > 0, b.db > 0);
        let (back, fwd) = (b.bb > 0, b.fb > 0);
        let row = if u {
            6
        } else if d {
            0
        } else {
            3
        };
        let col = if back {
            1
        } else if fwd {
            3
        } else {
            2
        };
        Self {
            dir: row + col,
            buttons: [b.ab, b.btb, b.cb, b.xb, b.yb, b.zb, b.sb].map(|v| v > 0),
            frames: 1,
        }
    }

    fn same(&self, o: &Held) -> bool {
        self.dir == o.dir && self.buttons == o.buttons
    }
}

/// Lines the input display keeps per fighter.
pub const INPUT_LINES: usize = 10;

/// A fighter's recent inputs, newest first; a combination held on extends its line.
#[derive(Clone, Debug, Default)]
pub struct InputLog {
    pub lines: std::collections::VecDeque<Held>,
}

impl InputLog {
    pub fn push(&mut self, h: Held) {
        match self.lines.front_mut() {
            Some(top) if top.same(&h) => top.frames = top.frames.saturating_add(1),
            _ => {
                self.lines.push_front(h);
                self.lines.truncate(INPUT_LINES);
            }
        }
    }
}

/// Live training state of one match.
#[derive(Clone, Debug, Default)]
pub struct Training {
    pub cfg: TrainingCfg,
    /// Frame data of P1's and P2's moves.
    pub meters: [FrameMeter; 2],
    /// Inputs of P1 and P2, as their input buffers saw them.
    pub inputs: [InputLog; 2],
    /// What each player presses for A B C X Y Z S (their own keys or pad).
    pub labels: [[String; 7]; 2],
    /// What P1 presses for START (it ends a recording).
    pub start: String,
    /// The dummy's recordings: P2 inputs with forward as right (see [`mirror`]).
    pub slots: [Vec<u16>; SLOTS],
    pub rec: Rec,
    dummy: DummyMind,
}

/// What the recorder is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rec {
    #[default]
    Idle,
    /// P1's controls drive the dummy; its inputs go into the slot.
    Recording,
    /// The slot drives the dummy, frame by frame.
    Playing { at: usize, looping: bool },
}

/// The pause menu's recorder buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Record,
    Play,
    Loop,
}

/// The dummy's memory between ticks (all of it outside the sim).
#[derive(Clone, Debug, Default)]
struct DummyMind {
    /// The other side was attacking last tick.
    threat: bool,
    /// Guard the current attack (BLOCK = RANDOM decides it per attack).
    guard: bool,
    /// AFTER FIRST HIT: hit once, guarding until left alone.
    armed: bool,
    /// Ticks with nothing happening (disarms AFTER FIRST HIT).
    calm: u32,
    /// Hit or guarding last tick (a reversal fires when that ends).
    hurt: bool,
    /// xorshift state for BLOCK = RANDOM (never the sim's random numbers).
    rng: u32,
}

/// Ticks of calm after which AFTER FIRST HIT lets the next first hit through.
const CALM_TICKS: u32 = 40;

/// Swap left and right: recordings keep forward as right whichever way the
/// dummy faced, so they play back the same from either side.
pub fn mirror(b: rugen_sim::Buttons) -> rugen_sim::Buttons {
    rugen_sim::Buttons {
        l: b.r,
        r: b.l,
        ..b
    }
}

/// What the dummy's side of the world looks like this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct DummyView {
    /// The dummy faces right.
    pub facing_right: bool,
    /// P1 (or a helper or projectile of P1) is attacking.
    pub threat: bool,
    /// The attack must be guarded low (`guardflag` L without H) / high (H without L).
    pub low: bool,
    pub high: bool,
    /// The dummy is in a hit or guard state; `hit`: a get-hit state (not guarding).
    pub hurt: bool,
    pub hit: bool,
    pub ctrl: bool,
}

impl DummyView {
    pub fn of(world: &rugen_sim::World) -> Option<Self> {
        use rugen_cns::state::MoveType;
        use rugen_sim::hit::hf;
        let d = world.players.get(1)?;
        let mut v = DummyView {
            facing_right: d.facing >= 0.0,
            hurt: d.move_type == MoveType::H,
            hit: d.move_type == MoveType::H && d.state_no >= 5000,
            ctrl: d.ctrl,
            ..Default::default()
        };
        let mut flags = 0;
        for p in world.players.iter().filter(|p| p.root == 0 && !p.removed) {
            if p.move_type == MoveType::A {
                v.threat = true;
                if let Some(h) = &p.hitdef {
                    flags |= h.guardflag;
                }
            }
        }
        for pr in world.projs.iter().filter(|p| !p.removed) {
            if world.players.get(pr.owner).is_some_and(|o| o.root == 0) {
                v.threat = true;
                flags |= pr.hitdef.guardflag;
            }
        }
        v.low = flags & hf::L != 0 && flags & hf::H == 0;
        v.high = flags & hf::H != 0 && flags & hf::L == 0;
        Some(v)
    }
}

impl Training {
    pub fn new(cfg: TrainingCfg) -> Self {
        Self {
            cfg,
            dummy: DummyMind {
                rng: 0x2545_f491,
                ..Default::default()
            },
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
        for (log, p) in self.inputs.iter_mut().zip(&world.players) {
            log.push(Held::of(&p.input));
        }
    }

    /// The inputs the world steps with this tick: P1's own, and the dummy's
    /// (recording, playback, stance and guard, else P2's own). Input only:
    /// the replay records what this returns.
    pub fn inputs_for(
        &mut self,
        live: [rugen_sim::Buttons; 2],
        view: DummyView,
    ) -> [rugen_sim::Buttons; 2] {
        use rugen_sim::Buttons;
        let to_world = |b: Buttons| if view.facing_right { b } else { mirror(b) };
        let slot = self.cfg.slot.min(SLOTS - 1);
        let mind = &mut self.dummy;
        let was_hurt = std::mem::replace(&mut mind.hurt, view.hurt);
        match self.rec {
            Rec::Recording => {
                // P1's controls play the dummy; P1 stands still
                let buf = &mut self.slots[slot];
                buf.push(to_world(live[0]).bits());
                if buf.len() >= RECORD_FRAMES {
                    self.rec = Rec::Idle;
                }
                return [Buttons::default(), live[0]];
            }
            Rec::Playing { at, looping } => {
                let buf = &self.slots[slot];
                if let Some(&f) = buf.get(at) {
                    self.rec = if at + 1 < buf.len() {
                        Rec::Playing {
                            at: at + 1,
                            looping,
                        }
                    } else if looping {
                        Rec::Playing { at: 0, looping }
                    } else {
                        Rec::Idle
                    };
                    return [live[0], to_world(Buttons::from_bits(f))];
                }
                self.rec = Rec::Idle;
            }
            Rec::Idle => {}
        }
        // AFTER BLOCK / HIT: the slot as a reversal, once the dummy can act again
        if self.cfg.reversal && was_hurt && !view.hurt && view.ctrl && !self.slots[slot].is_empty()
        {
            self.rec = Rec::Playing {
                at: 1,
                looping: false,
            };
            let f = Buttons::from_bits(self.slots[slot][0]);
            if self.slots[slot].len() == 1 {
                self.rec = Rec::Idle;
            }
            return [live[0], to_world(f)];
        }
        // guard: hold back (and down for a low), per BLOCK
        if view.threat && !mind.threat {
            mind.rng ^= mind.rng << 13;
            mind.rng ^= mind.rng >> 17;
            mind.rng ^= mind.rng << 5;
            mind.guard = mind.rng & 1 == 1;
        }
        mind.threat = view.threat;
        if view.hit {
            mind.armed = true;
        }
        mind.calm = if view.threat || view.hurt {
            0
        } else {
            mind.calm.saturating_add(1)
        };
        if mind.calm > CALM_TICKS {
            mind.armed = false;
        }
        let guard = view.threat
            && match self.cfg.block {
                Block::None => false,
                Block::All => true,
                Block::AfterFirstHit => mind.armed,
                Block::Random => mind.guard,
            };
        let mut b = Buttons::default();
        match self.cfg.stance {
            Stance::Stand => {}
            Stance::Crouch => b.d = true,
            Stance::Jump => b.u = true,
        }
        if guard {
            b.u = false;
            // back, as seen from the dummy (to_world turns it around when it faces left)
            b.l = true;
            if view.low {
                b.d = true;
            } else if view.high {
                b.d = false;
            }
        }
        let own = Buttons::from_bits(to_world(b).bits() | live[1].bits());
        [live[0], own]
    }

    /// A recorder button from the pause menu. False when there is nothing to play.
    pub fn command(&mut self, c: Command) -> bool {
        let slot = self.cfg.slot.min(SLOTS - 1);
        match c {
            Command::Record => {
                self.slots[slot].clear();
                self.rec = Rec::Recording;
            }
            Command::Play | Command::Loop if self.slots[slot].is_empty() => return false,
            Command::Play | Command::Loop => {
                self.rec = Rec::Playing {
                    at: 0,
                    looping: c == Command::Loop,
                };
            }
        }
        true
    }

    /// START during a recording ends it (true: START was used up).
    pub fn stop_recording(&mut self) -> bool {
        if self.rec == Rec::Recording {
            self.rec = Rec::Idle;
            return true;
        }
        false
    }

    /// Recorded frames per slot (the pause menu shows them).
    pub fn slot_frames(&self) -> [usize; SLOTS] {
        std::array::from_fn(|i| self.slots[i].len())
    }

    /// Button names as each player presses them (`J`, a pad's `A`...).
    pub fn set_labels(&mut self, labels: [&crate::keylabel::Labels; 2]) {
        for (out, l) in self.labels.iter_mut().zip(labels) {
            *out = std::array::from_fn(|i| l.text(4 + i));
        }
        self.start = labels[0].start().to_uppercase();
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
const KEYS: [f32; 4] = [1.0, 0.82, 0.3, 1.0];

/// 7x7 arrows: right and down-right; the others are mirrors.
const ARROW_RIGHT: [u8; 7] = [
    0b0001000, 0b0001100, 0b1111110, 0b1111111, 0b1111110, 0b0001100, 0b0001000,
];
const ARROW_DOWN_RIGHT: [u8; 7] = [
    0b0000000, 0b1000000, 0b0100010, 0b0010010, 0b0001010, 0b0000110, 0b0111110,
];

/// Numpad direction `dir` (relative to facing, 5 = neutral: a dot) as a
/// 7x7 pixel arrow with its top left at `(x, y)` (screen units).
fn draw_arrow(r: &mut Renderer, hud: &Hud, dir: u8, x: f32, y: f32, px: f32, c: [f32; 4]) {
    // (bitmap, mirror x, mirror y, transpose)
    let (bits, fx, fy, tr) = match dir {
        6 => (ARROW_RIGHT, false, false, false),
        4 => (ARROW_RIGHT, true, false, false),
        2 => (ARROW_RIGHT, false, false, true),
        8 => (ARROW_RIGHT, false, true, true),
        3 => (ARROW_DOWN_RIGHT, false, false, false),
        1 => (ARROW_DOWN_RIGHT, true, false, false),
        9 => (ARROW_DOWN_RIGHT, false, true, false),
        7 => (ARROW_DOWN_RIGHT, true, true, false),
        _ => {
            let a = (hud.at)(x + 2.0 * px, y + 2.0 * px);
            let b = (hud.at)(x + 5.0 * px, y + 5.0 * px);
            r.rect(a, b, [c[0], c[1], c[2], c[3] * 0.6]);
            return;
        }
    };
    for row in 0..7 {
        for col in 0..7 {
            let (br, bc) = if tr { (col, row) } else { (row, col) };
            if bits[br] & (0b1000000 >> bc) == 0 {
                continue;
            }
            let cx = if fx { 6 - col } else { col } as f32;
            let cy = if fy { 6 - row } else { row } as f32;
            let a = (hud.at)(x + cx * px, y + cy * px);
            let b = (hud.at)(x + (cx + 1.0) * px, y + (cy + 1.0) * px);
            r.rect(a, b, c);
        }
    }
}

impl Training {
    /// The overlays the settings ask for (drawing only).
    pub fn draw(&self, r: &mut Renderer, hud: &Hud) {
        if self.cfg.frame_data {
            self.draw_frame_data(r, hud);
        }
        if self.cfg.inputs {
            self.draw_inputs(r, hud);
        }
        self.draw_recorder(r, hud);
    }

    /// While recording or playing: a tag under the clock.
    fn draw_recorder(&self, r: &mut Renderer, hud: &Hud) {
        let k = (hud.screen_h / 240.0).max(0.5);
        let slot = self.cfg.slot.min(SLOTS - 1);
        let (text, color) = match self.rec {
            Rec::Idle => return,
            Rec::Recording => (
                format!(
                    "REC SLOT {}  {:.1} S / 10 S  {} STOPS",
                    slot + 1,
                    self.slots[slot].len() as f32 / 60.0,
                    if self.start.is_empty() {
                        "START"
                    } else {
                        &self.start
                    }
                ),
                [1.0, 0.25, 0.25, 1.0],
            ),
            Rec::Playing { looping, .. } => (
                format!(
                    "{} SLOT {}",
                    if looping { "LOOP" } else { "PLAY" },
                    slot + 1
                ),
                [0.5, 1.0, 0.6, 1.0],
            ),
        };
        let px = k;
        let w = text.chars().count() as f32 * 4.0 * px + 14.0 * k;
        let (top, h) = (50.0 * k, 11.0 * k);
        let x0 = hud.screen_w / 2.0 - w / 2.0;
        r.rect(
            (hud.at)(x0, top),
            (hud.at)(x0 + w, top + h),
            [0.0, 0.0, 0.0, 0.7],
        );
        // the dot: solid while recording, a play mark otherwise
        r.rect(
            (hud.at)(x0 + 4.0 * k, top + 3.0 * k),
            (hud.at)(x0 + 9.0 * k, top + 8.0 * k),
            color,
        );
        let p = (hud.at)(x0 + 12.0 * k, top + 3.0 * k);
        text_at(r, &text, p[0], p[1], px * hud.u, false, color);
    }

    /// INPUT DISPLAY: a column per fighter at the screen edge, newest on top.
    fn draw_inputs(&self, r: &mut Renderer, hud: &Hud) {
        let k = (hud.screen_h / 240.0).max(0.5);
        let px = k;
        let line = 8.5 * k;
        let pad = 3.0 * k;
        // under the combo counter and the frame data panel, above the power bars
        let top = 108.0 * k;
        for side in 0..2 {
            let log = &self.inputs[side];
            let labels = &self.labels[side];
            let rows: Vec<(String, u8, String)> = log
                .lines
                .iter()
                .map(|h| {
                    let keys: Vec<&str> = h
                        .buttons
                        .iter()
                        .zip(labels)
                        .filter(|(on, _)| **on)
                        .map(|(_, l)| l.as_str())
                        .collect();
                    (format!("{:>3}", h.frames.min(999)), h.dir, keys.join(" "))
                })
                .collect();
            let widest = rows
                .iter()
                .map(|(_, _, keys)| keys.chars().count())
                .max()
                .unwrap_or(0)
                .max(4) as f32;
            // frames, a gap, the arrow cell, a gap, the keys
            let w = (3.0 * 4.0 + 4.0 + 7.0 + 4.0 + widest * 4.0) * px + 2.0 * pad;
            let h = INPUT_LINES as f32 * line + 2.0 * pad;
            let (x0, x1) = if side == 0 {
                (6.0 * k, 6.0 * k + w)
            } else {
                (hud.screen_w - 6.0 * k - w, hud.screen_w - 6.0 * k)
            };
            r.rect((hud.at)(x0, top), (hud.at)(x1, top + h), PANEL_BG);
            for (n, (frames, dir, keys)) in rows.iter().enumerate() {
                let y = top + pad + n as f32 * line;
                // older lines fade
                let fade = 1.0 - n as f32 / (INPUT_LINES as f32 * 1.6);
                let col = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * fade];
                let x = x0 + pad;
                let p = (hud.at)(x, y + px);
                text_at(r, frames, p[0], p[1], px * hud.u, false, col(DIM));
                let ax = x + 16.0 * px;
                draw_arrow(r, hud, *dir, ax, y, px, col(INK));
                let p = (hud.at)(ax + 11.0 * px, y + px);
                text_at(r, keys, p[0], p[1], px * hud.u, false, col(KEYS));
            }
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
            teams: None,
            run: None,
            seed: None,
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

    /// The input display reads the sim's input buffer: directions relative
    /// to facing in numpad notation, a held combination extends its line.
    #[test]
    fn inputs_come_from_the_buffer_and_merge_while_held() {
        use rugen_sim::input::InputBuffer;
        let mut buf = InputBuffer::default();
        let mut log = InputLog::default();
        let tick = |buf: &mut InputBuffer, log: &mut InputLog, d: bool, f: bool, a: bool| {
            buf.update(
                false, d, false, f, a, false, false, false, false, false, false,
            );
            log.push(Held::of(buf));
        };
        for _ in 0..3 {
            tick(&mut buf, &mut log, false, false, false);
        }
        tick(&mut buf, &mut log, true, false, false);
        tick(&mut buf, &mut log, true, true, false);
        tick(&mut buf, &mut log, false, true, true);
        tick(&mut buf, &mut log, false, true, true);
        let got: Vec<(u8, bool, u32)> = log
            .lines
            .iter()
            .map(|h| (h.dir, h.buttons[0], h.frames))
            .collect();
        assert_eq!(
            got,
            vec![(6, true, 2), (3, false, 1), (2, false, 1), (5, false, 3)],
            "newest first: 6+A held 2, 3, 2, neutral 3"
        );
        for _ in 0..40 {
            tick(&mut buf, &mut log, true, false, false);
            tick(&mut buf, &mut log, false, false, false);
        }
        assert_eq!(log.lines.len(), INPUT_LINES, "a fixed number of lines");
    }

    fn press(f: impl Fn(&mut rugen_sim::Buttons)) -> rugen_sim::Buttons {
        let mut b = rugen_sim::Buttons::default();
        f(&mut b);
        b
    }

    fn view(facing_right: bool) -> DummyView {
        DummyView {
            facing_right,
            ctrl: true,
            ..Default::default()
        }
    }

    /// RECORD takes P1's controls for the dummy (P1 stands still), stores
    /// them with forward as right, and PLAY gives them back frame for frame,
    /// turned around when the dummy faces the other way.
    #[test]
    fn record_then_play_and_loop() {
        let mut t = Training::new(TrainingCfg::default());
        let none = rugen_sim::Buttons::default();
        assert!(!t.command(Command::Play), "nothing recorded yet");
        assert!(t.command(Command::Record));
        let left = press(|b| b.l = true);
        let punch = press(|b| b.a = true);
        // the dummy faces left: P1 walking left walks it forward
        let out = t.inputs_for([left, none], view(false));
        assert_eq!(out, [none, left], "P1 still, the dummy moves");
        t.inputs_for([punch, none], view(false));
        assert!(t.stop_recording());
        assert!(!t.stop_recording(), "only once");
        assert_eq!(t.slots[0].len(), 2);
        assert_eq!(
            rugen_sim::Buttons::from_bits(t.slots[0][0]),
            press(|b| b.r = true),
            "stored as forward"
        );
        // played facing right: forward is right
        assert!(t.command(Command::Play));
        let out = t.inputs_for([none, none], view(true));
        assert_eq!(out[1], press(|b| b.r = true));
        assert_eq!(t.inputs_for([none, none], view(true))[1], punch);
        assert_eq!(t.rec, Rec::Idle, "PLAY stops at the end");
        assert_eq!(t.inputs_for([none, none], view(true))[1], none);
        // LOOP starts over
        assert!(t.command(Command::Loop));
        for _ in 0..5 {
            t.inputs_for([none, none], view(true));
        }
        assert!(matches!(t.rec, Rec::Playing { looping: true, .. }));
        // a recording stops by itself after 10 s
        t.cfg.slot = 2;
        assert!(t.command(Command::Record));
        for _ in 0..RECORD_FRAMES + 30 {
            t.inputs_for([left, none], view(true));
        }
        assert_eq!(t.slots[2].len(), RECORD_FRAMES);
        assert_eq!(t.rec, Rec::Idle);
        assert_eq!(t.slot_frames(), [2, 0, RECORD_FRAMES]);
    }

    /// BLOCK: none, all (back, down for a low), after the first hit.
    #[test]
    fn the_dummy_guards_as_set() {
        let none = rugen_sim::Buttons::default();
        let threat = |facing_right: bool| DummyView {
            threat: true,
            ..view(facing_right)
        };
        let mut t = Training::new(TrainingCfg::default());
        assert_eq!(t.inputs_for([none, none], threat(true))[1], none, "NONE");
        t.cfg.block = Block::All;
        assert_eq!(
            t.inputs_for([none, none], threat(true))[1],
            press(|b| b.l = true),
            "back is left when facing right"
        );
        assert_eq!(
            t.inputs_for([none, none], threat(false))[1],
            press(|b| b.r = true)
        );
        let low = DummyView {
            low: true,
            ..threat(true)
        };
        assert_eq!(
            t.inputs_for([none, none], low)[1],
            press(|b| {
                b.l = true;
                b.d = true
            })
        );
        assert_eq!(t.inputs_for([none, none], view(true))[1], none, "no attack");
        // crouching: down held, guard crouching unless the attack is an overhead
        t.cfg.stance = Stance::Crouch;
        assert_eq!(
            t.inputs_for([none, none], view(true))[1],
            press(|b| b.d = true)
        );
        let high = DummyView {
            high: true,
            ..threat(true)
        };
        assert_eq!(t.inputs_for([none, none], high)[1], press(|b| b.l = true));
        // AFTER FIRST HIT: the first attack lands, then it guards
        let mut t = Training::new(TrainingCfg {
            block: Block::AfterFirstHit,
            ..Default::default()
        });
        assert_eq!(t.inputs_for([none, none], threat(true))[1], none);
        let hit = DummyView {
            hurt: true,
            hit: true,
            ctrl: false,
            ..threat(true)
        };
        t.inputs_for([none, none], hit);
        assert_eq!(
            t.inputs_for([none, none], threat(true))[1],
            press(|b| b.l = true)
        );
        // left alone a while: the next first hit lands again
        for _ in 0..CALM_TICKS + 2 {
            t.inputs_for([none, none], view(true));
        }
        assert_eq!(t.inputs_for([none, none], threat(true))[1], none);
    }

    /// AFTER BLOCK / HIT = REVERSAL SLOT: the slot plays the tick the dummy
    /// can act again.
    #[test]
    fn a_reversal_plays_when_the_dummy_recovers() {
        let none = rugen_sim::Buttons::default();
        let mut t = Training::new(TrainingCfg {
            reversal: true,
            ..Default::default()
        });
        let up = press(|b| b.u = true);
        t.slots[0] = vec![up.bits(), up.bits()];
        let hurt = DummyView {
            hurt: true,
            ctrl: false,
            ..view(true)
        };
        assert_eq!(t.inputs_for([none, none], hurt)[1], none);
        assert_eq!(t.inputs_for([none, none], view(true))[1], up, "recovered");
        assert_eq!(t.inputs_for([none, none], view(true))[1], up);
        assert_eq!(t.rec, Rec::Idle);
        assert_eq!(t.inputs_for([none, none], view(true))[1], none);
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
