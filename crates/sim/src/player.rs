//! Runtime state of one character instance. Plain data, cloned for rollback.

use rugen_cns::state::{MoveType, Physics, StateType};

use crate::anim::AnimState;
use crate::command::CommandList;
use crate::explod::Trans;
use crate::fx::{AfterImage, PalFx};
use crate::hit::{GetHitVar, HitDef, MoveContact, MoveHitVar};
use crate::input::InputBuffer;
use crate::projectile::ProjContact;

/// `HitBy` / `NotHitBy` slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitBySlot {
    pub attr: u32,
    pub time: i32,
    pub not: bool,
}

/// `TargetBind` state: bound to `to` at `pos` for `time` more ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bind {
    pub to: i32,
    pub time: i32,
    pub pos: [f32; 2],
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
    // ---- projectiles
    pub pctype: ProjContact,
    pub pctime: i32,
    pub pcid: i32,
    // ---- drawing
    /// `Trans` controller for this tick (reset every tick).
    pub trans: Trans,
    pub alpha: [i32; 2],
    /// Rotation set by AngleSet/Add/Mul; only drawn while `angle_drawn`.
    pub angle: f32,
    pub angle_drawn: bool,
    pub angle_scale: [f32; 2],
    pub palfx: PalFx,
    pub afterimage: Option<Box<AfterImage>>,
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
            pctype: ProjContact::None,
            pctime: -1,
            pcid: 0,
            trans: Trans::Default,
            alpha: [255, 0],
            angle: 0.0,
            angle_drawn: false,
            angle_scale: [1.0, 1.0],
            palfx: PalFx::default(),
            afterimage: None,
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
