//! Runtime state of one character instance. Plain data, cloned for rollback.

use rugen_cns::state::{MoveType, Physics, StateType};

use crate::anim::AnimState;
use crate::command::CommandList;
use crate::input::InputBuffer;

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
    pub hit_pause: i32,
    pub pal_no: i32,
    /// Set while a negative-state block or current state is being run; -1,-2,-3 or 0.
    pub minus: i8,
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
            hit_pause: 0,
            pal_no,
            minus: 0,
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
