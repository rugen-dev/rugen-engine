//! Runtime state of one character instance. Plain data, cloned for rollback.

use rugen_cns::state::{MoveType, Physics, StateType};

use crate::anim::AnimState;
use crate::command::CommandList;
use crate::explod::Trans;
use crate::fx::{AfterImage, PalFx};
use crate::hit::{GetHitVar, HitDef, MoveContact, MoveHitVar};
use crate::input::{AiInput, InputBuffer};
use crate::projectile::ProjContact;

/// `HitBy` / `NotHitBy` slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitBySlot {
    pub attr: u32,
    pub time: i32,
    pub not: bool,
}

/// `TargetBind` / `BindToParent` state: bound to `to` at `pos` for `time` more ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bind {
    pub to: i32,
    pub time: i32,
    pub pos: [f32; 2],
    /// Facing to force while bound (0 = keep).
    pub facing: f32,
}

/// One `HitOverride` slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitOverride {
    pub attr: u32,
    pub stateno: i32,
    /// Ticks left; 0 = inactive, -1 = forever.
    pub time: i32,
    pub forceair: bool,
    pub forceguard: bool,
}

/// AssertSpecial flags, reset every tick.
pub mod assert {
    pub const INTRO: u32 = 1 << 0;
    pub const INVISIBLE: u32 = 1 << 1;
    pub const ROUNDNOTOVER: u32 = 1 << 2;
    pub const NOBARDISPLAY: u32 = 1 << 3;
    pub const NOBG: u32 = 1 << 4;
    pub const NOFG: u32 = 1 << 5;
    pub const NOSTANDGUARD: u32 = 1 << 6;
    pub const NOCROUCHGUARD: u32 = 1 << 7;
    pub const NOAIRGUARD: u32 = 1 << 8;
    pub const NOAUTOTURN: u32 = 1 << 9;
    pub const NOJUGGLECHECK: u32 = 1 << 10;
    pub const NOKOSND: u32 = 1 << 11;
    pub const NOKOSLOW: u32 = 1 << 12;
    pub const NOSHADOW: u32 = 1 << 13;
    pub const GLOBALNOSHADOW: u32 = 1 << 14;
    pub const NOMUSIC: u32 = 1 << 15;
    pub const NOWALK: u32 = 1 << 16;
    pub const TIMERFREEZE: u32 = 1 << 17;
    pub const UNGUARDABLE: u32 = 1 << 18;
    pub const NOKO: u32 = 1 << 19;
    pub const NOINTRORESET: u32 = 1 << 20;

    pub fn parse(name: &str) -> Option<u32> {
        Some(match name {
            "intro" => INTRO,
            "invisible" => INVISIBLE,
            "roundnotover" => ROUNDNOTOVER,
            "nobardisplay" => NOBARDISPLAY,
            "nobg" => NOBG,
            "nofg" => NOFG,
            "nostandguard" => NOSTANDGUARD,
            "nocrouchguard" => NOCROUCHGUARD,
            "noairguard" => NOAIRGUARD,
            "noautoturn" => NOAUTOTURN,
            "nojugglecheck" => NOJUGGLECHECK,
            "nokosnd" => NOKOSND,
            "nokoslow" => NOKOSLOW,
            "noshadow" => NOSHADOW,
            "globalnoshadow" => GLOBALNOSHADOW,
            "nomusic" => NOMUSIC,
            "nowalk" => NOWALK,
            "timerfreeze" => TIMERFREEZE,
            "unguardable" => UNGUARDABLE,
            "noko" => NOKO,
            "nointroreset" => NOINTRORESET,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    /// Index into `World::data`.
    pub data: usize,
    pub team: u8,
    pub id: i32,
    /// Stage coordinates, logical pixels of the char's localcoord. y = 0 is the ground, up is negative.
    pub pos: [f32; 2],
    pub old_pos: [f32; 2],
    /// x relative to facing.
    pub vel: [f32; 2],
    /// +1 right, -1 left.
    pub facing: f32,
    pub state_no: i32,
    pub prev_state_no: i32,
    pub state_time: i32,
    pub state_type: StateType,
    pub move_type: MoveType,
    pub physics: Physics,
    pub anim: AnimState,
    /// Data index whose AIR the current animation comes from.
    pub anim_owner: usize,
    pub prev_anim_no: i32,
    pub ctrl: bool,
    pub life: i32,
    pub life_max: i32,
    pub power: i32,
    pub power_max: i32,
    pub vars: Vec<i32>,
    pub fvars: Vec<f32>,
    pub sysvars: Vec<i32>,
    pub sysfvars: Vec<f32>,
    pub input: InputBuffer,
    pub cmds: CommandList,
    /// Buffered ChangeState applied before the current state runs (`stchtmp`).
    pub pending_state: Option<i32>,
    /// Persistent counters for the current state's controllers.
    pub persist: Vec<i32>,
    pub spr_priority: i32,
    pub assert_flags: u32,
    pub pos_freeze: bool,
    pub width_edge: [f32; 2],
    pub width_player: [f32; 2],
    pub screen_bound: bool,
    pub move_camera: [bool; 2],
    pub juggle: i32,
    pub air_jump_count: i32,
    /// Engine air dash (see `World::movement`): used this airtime, ticks left.
    pub air_dash_used: bool,
    pub air_dash_time: i32,
    /// Frame (+1) of the last forward / back press, and whether it completed a double tap.
    pub tap_tick: [u32; 2],
    pub tap_pending: [bool; 2],
    /// Frame (+1) of the last down press: a second one soon after drops
    /// through the platform stood on (see `World::platform_drop`).
    pub down_tap: u32,
    /// Platform being dropped through (index into `Stage::platforms`) and the
    /// ticks it stays ignored; it is also ignored until the feet are below it.
    pub drop_through: Option<(usize, i32)>,
    /// Lives left this round (`[Arena] stocks`); 0 = stocks disabled.
    pub stocks: i32,
    /// On a ledge (see [`crate::ledge`]).
    pub ledge: Option<crate::ledge::Hang>,
    /// Ledge grabs this airtime, and ticks before the next one is allowed.
    pub ledge_grabs: u8,
    pub ledge_cool: i32,
    /// Arena items: B held last tick, and B kept from the character until
    /// released (it picked up or threw an item, see [`crate::items`]).
    pub item_b_last: bool,
    pub item_b_mask: bool,
    pub pal_no: i32,
    /// Set while a negative-state block or current state is being run; -1,-2,-3 or 0.
    pub minus: i8,
    // ---- hits
    /// Data index whose states (and ChangeAnim2 anims) this player runs; `data` unless in a custom state.
    pub state_owner: usize,
    pub hitdef: Option<HitDef>,
    pub hitdef_targets: Vec<i32>,
    pub targets: Vec<i32>,
    pub ghv: GetHitVar,
    pub mhv: MoveHitVar,
    pub mctype: MoveContact,
    pub mctime: i32,
    pub hit_pause_time: i32,
    /// Hit this tick (`CSF_gethit`).
    pub gethit: bool,
    /// 0 idle, -1 just hit, 1 in hit state, 2 falling.
    pub hittmp: i8,
    pub prev_fallflag: bool,
    pub hit_count: i32,
    pub guard_count: i32,
    pub uniq_hit_count: i32,
    pub unhittable_time: i32,
    pub hitby: [HitBySlot; 2],
    pub hover: [HitOverride; 8],
    /// Slot chosen by the last hit this tick, -1 = none.
    pub hover_idx: i32,
    pub inguarddist: bool,
    /// `SCF_guard`: holding back in a state that may guard.
    pub guarding: bool,
    pub attack_mul: f32,
    pub defence_mul: f32,
    pub fall_defence_mul: f32,
    pub player_push: bool,
    pub pushed: bool,
    pub received_hits: i32,
    pub down_hit_offset: bool,
    /// Facing to apply after hit processing (`p1facing`).
    pub p1facing: f32,
    pub counter_hit: bool,
    pub ko: bool,
    pub bind: Option<Bind>,
    pub pause_move_time: i32,
    pub super_move_time: i32,
    /// True while a Pause/SuperPause freezes this player this tick.
    pub pause_bool: bool,
    pub super_defence_mul: f32,
    pub super_defence_mul_buffer: f32,
    // ---- helpers
    /// 0 = root player, >0 = helper.
    pub helper_index: i32,
    pub helper_id: i32,
    pub parent_id: i32,
    /// Index of the root player this entity belongs to.
    pub root: usize,
    /// `helpertype = player`: pushes, is screen-bound and counted as a player.
    pub player_type: bool,
    pub keyctrl: bool,
    pub children: Vec<i32>,
    pub removed: bool,
    // ---- teams (see `crate::team`)
    /// Input slot driving this root (`World::step`'s `inputs[slot]`): its
    /// own index in 1v1; Turns / Tag members share their side's.
    pub input_slot: usize,
    /// Position in its team (0 = the first pick).
    pub member: u8,
    /// Tag: out of the fight (leaving, waiting outside, or down): not hit,
    /// not pushed, not followed by the camera, never anyone's P2.
    pub standby: bool,
    /// Tag: the engine's switch movement this player is in, if any.
    pub tag: crate::team::TagPhase,
    /// Tag: distance still to run onto the screen (stage units).
    pub tag_dist: f32,
    /// Tag: life it wins back while waiting outside (Ikemen red life).
    pub red_life: i32,
    /// Life last tick (red life follows the damage taken).
    pub last_life: i32,
    /// The TAG button: held last tick, pressed this tick.
    pub tag_held: bool,
    pub tag_press: bool,
    /// Set by `World::step_checked` when this player's content panicked: it no
    /// longer runs states and loses the round.
    pub broken: bool,
    /// The "state machine stuck" warning was already issued for this player.
    pub loop_warned: bool,
    // ---- projectiles
    pub pctype: ProjContact,
    pub pctime: i32,
    pub pcid: i32,
    /// Height of the surface under the player (0 = floor, negative = platform).
    pub ground: f32,
    // ---- AI
    /// 0 = human, 1..=8 = computer controlled (`AILevel`).
    pub ai_level: i32,
    pub ai: AiInput,
    /// Arena AI plugin memory (see [`crate::arena_ai`]).
    pub arena: crate::arena_ai::ArenaAiState,
    // ---- drawing
    /// `Trans` controller for this tick (reset every tick).
    pub trans: Trans,
    pub alpha: [i32; 2],
    /// Rotation set by AngleSet/Add/Mul; only drawn while `angle_drawn`.
    pub angle: f32,
    pub angle_drawn: bool,
    pub angle_scale: [f32; 2],
    pub palfx: PalFx,
    /// Own palette effects (MUGEN `ownpal`): roots always; a helper only
    /// with `ownpal = 1`, else it shows its root's PalFX and its own PalFX
    /// controllers do nothing (Ikemen `getPalfx`, `palFX.Run`).
    pub ownpal: bool,
    /// Its states ran outside a hitpause this tick (Ikemen `acttmp > 0`):
    /// the animation still steps at the end of the tick in which a hit
    /// started a hitpause.
    pub acted: bool,
    /// The last ChangeAnim asked for an action its AIR lacks: MUGEN leaves
    /// the player without a valid animation, drawing nothing and without
    /// boxes, until a valid change. The previous anim state stays for
    /// triggers.
    pub anim_none: bool,
    pub afterimage: Option<Box<AfterImage>>,
    /// `[Size]` in effect: the cns constants for a root, the Helper's
    /// `size.*` parameters (else the root's constants) for a helper.
    pub size: Size,
}

/// A player's `[Size]` constants (char-local units), see [`Player::size`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub xscale: f32,
    pub yscale: f32,
    pub ground_back: f32,
    pub ground_front: f32,
    pub air_back: f32,
    pub air_front: f32,
    pub height: f32,
    pub proj_doscale: i32,
    pub head_pos: [f32; 2],
    pub mid_pos: [f32; 2],
    pub shadowoffset: f32,
}

impl Default for Size {
    fn default() -> Self {
        Self {
            xscale: 1.0,
            yscale: 1.0,
            ground_back: 15.0,
            ground_front: 16.0,
            air_back: 12.0,
            air_front: 12.0,
            height: 60.0,
            proj_doscale: 0,
            head_pos: [0.0; 2],
            mid_pos: [0.0; 2],
            shadowoffset: 0.0,
        }
    }
}

impl Size {
    /// The `[Size]` constants of a character's cns.
    pub fn from_data(d: &crate::CharData) -> Self {
        let z = Self::default();
        let k = |name: &str, def: f32| d.konst(name).unwrap_or(def);
        Self {
            xscale: k("size.xscale", z.xscale),
            yscale: k("size.yscale", z.yscale),
            ground_back: k("size.ground.back", z.ground_back),
            ground_front: k("size.ground.front", z.ground_front),
            air_back: k("size.air.back", z.air_back),
            air_front: k("size.air.front", z.air_front),
            height: k("size.height", z.height),
            proj_doscale: k("size.proj.doscale", 0.0) as i32,
            head_pos: [k("size.head.pos.x", 0.0), k("size.head.pos.y", 0.0)],
            mid_pos: [k("size.mid.pos.x", 0.0), k("size.mid.pos.y", 0.0)],
            shadowoffset: k("size.shadowoffset", 0.0),
        }
    }

    /// `Const(name)` for the per-player size keys; `None` for any other name.
    pub fn konst(&self, name: &str) -> Option<f32> {
        Some(match name {
            "size.xscale" => self.xscale,
            "size.yscale" => self.yscale,
            "size.ground.back" => self.ground_back,
            "size.ground.front" => self.ground_front,
            "size.air.back" => self.air_back,
            "size.air.front" => self.air_front,
            "size.height" => self.height,
            "size.proj.doscale" => self.proj_doscale as f32,
            "size.head.pos.x" => self.head_pos[0],
            "size.head.pos.y" => self.head_pos[1],
            "size.mid.pos.x" => self.mid_pos[0],
            "size.mid.pos.y" => self.mid_pos[1],
            "size.shadowoffset" => self.shadowoffset,
            _ => return None,
        })
    }
}

impl Player {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        data: usize,
        team: u8,
        id: i32,
        life: i32,
        power_max: i32,
        pos_x: f32,
        facing: f32,
        pal_no: i32,
    ) -> Self {
        Self {
            data,
            team,
            id,
            pos: [pos_x, 0.0],
            old_pos: [pos_x, 0.0],
            vel: [0.0, 0.0],
            facing,
            state_no: 0,
            prev_state_no: 0,
            state_time: 0,
            state_type: StateType::S,
            move_type: MoveType::I,
            physics: Physics::S,
            anim: AnimState::start(0),
            anim_owner: data,
            prev_anim_no: 0,
            ctrl: false,
            life,
            life_max: life,
            power: 0,
            power_max,
            vars: vec![0; 60],
            fvars: vec![0.0; 40],
            sysvars: vec![0; 5],
            sysfvars: vec![0.0; 5],
            input: InputBuffer::default(),
            cmds: CommandList::default(),
            pending_state: None,
            persist: Vec::new(),
            spr_priority: 0,
            assert_flags: 0,
            pos_freeze: false,
            width_edge: [0.0, 0.0],
            width_player: [0.0, 0.0],
            screen_bound: true,
            move_camera: [true, true],
            juggle: 0,
            air_jump_count: 0,
            air_dash_used: false,
            air_dash_time: 0,
            tap_tick: [0, 0],
            tap_pending: [false, false],
            down_tap: 0,
            drop_through: None,
            stocks: 0,
            ledge: None,
            ledge_grabs: 0,
            ledge_cool: 0,
            item_b_last: false,
            item_b_mask: false,
            pal_no,
            minus: 0,
            state_owner: data,
            hitdef: None,
            hitdef_targets: Vec::new(),
            targets: Vec::new(),
            ghv: GetHitVar::default(),
            mhv: MoveHitVar::default(),
            mctype: MoveContact::None,
            mctime: 0,
            hit_pause_time: 0,
            gethit: false,
            hittmp: 0,
            prev_fallflag: false,
            hit_count: 0,
            guard_count: 0,
            uniq_hit_count: 0,
            unhittable_time: 0,
            hitby: [HitBySlot::default(); 2],
            hover: [HitOverride::default(); 8],
            hover_idx: -1,
            inguarddist: false,
            guarding: false,
            attack_mul: 1.0,
            defence_mul: 1.0,
            fall_defence_mul: 1.0,
            player_push: true,
            pushed: false,
            received_hits: 0,
            down_hit_offset: false,
            p1facing: 0.0,
            counter_hit: false,
            ko: false,
            bind: None,
            pause_move_time: 0,
            super_move_time: 0,
            pause_bool: false,
            super_defence_mul: 1.0,
            super_defence_mul_buffer: 1.0,
            helper_index: 0,
            helper_id: 0,
            parent_id: -1,
            root: 0,
            player_type: true,
            keyctrl: true,
            children: Vec::new(),
            removed: false,
            input_slot: 0,
            member: 0,
            standby: false,
            tag: crate::team::TagPhase::None,
            tag_dist: 0.0,
            red_life: life,
            last_life: life,
            tag_held: false,
            tag_press: false,
            broken: false,
            loop_warned: false,
            pctype: ProjContact::None,
            pctime: -1,
            pcid: 0,
            ground: 0.0,
            ai_level: 0,
            ai: AiInput::default(),
            arena: Default::default(),
            trans: Trans::Default,
            alpha: [255, 0],
            angle: 0.0,
            angle_drawn: false,
            angle_scale: [1.0, 1.0],
            palfx: PalFx::default(),
            ownpal: true,
            acted: false,
            anim_none: false,
            afterimage: None,
            size: Size::default(),
        }
    }

    pub fn alive(&self) -> bool {
        !self.ko
    }
    pub fn is_helper(&self) -> bool {
        self.helper_index > 0
    }
    pub fn hit_pause(&self) -> bool {
        self.hit_pause_time > 0
    }
    pub fn in_guard_state(&self) -> bool {
        crate::hit::is_guard_state(self.state_no)
    }
    pub fn move_hit(&self) -> i32 {
        if self.mctype == MoveContact::Hit {
            self.mctime.abs()
        } else {
            0
        }
    }
    pub fn move_guarded(&self) -> i32 {
        if self.mctype == MoveContact::Guarded {
            self.mctime.abs()
        } else {
            0
        }
    }
    pub fn move_contact(&self) -> i32 {
        if self.mctype != MoveContact::None {
            self.mctime.abs()
        } else {
            0
        }
    }

    pub fn asserted(&self, flag: u32) -> bool {
        self.assert_flags & flag != 0
    }

    pub fn set_facing(&mut self, f: f32) {
        if f != 0.0 && (self.facing < 0.0) != (f < 0.0) {
            self.facing = -self.facing;
            self.vel[0] = -self.vel[0];
        }
    }
}
