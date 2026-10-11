//! Function-type triggers. The simulation answers each one through
//! [`crate::Context::trigger`]; arguments that are expressions arrive on the
//! VM stack (`args`), static ones (names, flags) live in the variant.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

/// `S C A L` bits for StateType / HitDefAttr, `A I H` for MoveType.
pub mod flags {
    pub const ST_S: u32 = 1;
    pub const ST_C: u32 = 2;
    pub const ST_A: u32 = 4;
    pub const ST_L: u32 = 8;
    pub const ST_U: u32 = 16;
    pub const MT_I: u32 = 1;
    pub const MT_A: u32 = 2;
    pub const MT_H: u32 = 4;
    pub const MT_U: u32 = 8;
    // HitDefAttr second part: attack class bits
    pub const AT_NA: u32 = 1 << 0;
    pub const AT_NT: u32 = 1 << 1;
    pub const AT_NP: u32 = 1 << 2;
    pub const AT_SA: u32 = 1 << 3;
    pub const AT_ST: u32 = 1 << 4;
    pub const AT_SP: u32 = 1 << 5;
    pub const AT_HA: u32 = 1 << 6;
    pub const AT_HT: u32 = 1 << 7;
    pub const AT_HP: u32 = 1 << 8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TeamMode {
    Single,
    Simul,
    Turns,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameWhich {
    Name,
    P1Name,
    P2Name,
    P3Name,
    P4Name,
    AuthorName,
}

/// Which old-style projectile clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjEvent {
    Hit,
    Guarded,
    Contact,
}

/// `args` = number of values popped from the stack, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    // --- nullary, player state
    AILevel,
    Alive,
    Anim,
    AnimTime,
    BackEdge,
    BackEdgeBodyDist,
    BackEdgeDist,
    BottomEdge,
    CameraPos(Axis),
    CameraZoom,
    CanRecover,
    Ctrl,
    DrawGame,
    Facing,
    FrontEdge,
    FrontEdgeBodyDist,
    FrontEdgeDist,
    GameHeight,
    GameTime,
    GameWidth,
    HitCount,
    HitFall,
    HitOver,
    HitPauseTime,
    HitShakeOver,
    HitVel(Axis),
    Id,
    InGuardDist,
    IsHomeTeam,
    LeftEdge,
    Life,
    LifeMax,
    Lose,
    MatchNo,
    MatchOver,
    MoveContact,
    MoveGuarded,
    MoveHit,
    MoveReversed,
    NumEnemy,
    NumPartner,
    NumProj,
    P2BodyDist(Axis),
    P2Dist(Axis),
    P2Life,
    P2StateNo,
    PalNo,
    ParentDist(Axis),
    Pos(Axis),
    Power,
    PowerMax,
    PrevStateNo,
    Random,
    RightEdge,
    RootDist(Axis),
    RoundNo,
    RoundsExisted,
    RoundState,
    ScreenPos(Axis),
    ScreenHeight,
    ScreenWidth,
    StateNo,
    TeamSide,
    TicksPerSecond,
    Time,
    TopEdge,
    UniqHitCount,
    Vel(Axis),
    Win,
    WinKO,
    WinTime,
    WinPerfect,
    LoseKO,
    LoseTime,
    // --- enum-valued; bare use returns the raw flag bits
    StateType,
    P2StateType,
    MoveType,
    P2MoveType,
    TeamModeRaw,
    /// `StateType = S` style comparison, already folded: 1 if current & mask != 0
    StateTypeIs {
        p2: bool,
        mask: u32,
    },
    MoveTypeIs {
        p2: bool,
        mask: u32,
    },
    TeamModeIs(TeamMode),
    /// `HitDefAttr = SC, NA, ST` : (state mask, attack-class mask)
    HitDefAttrIs {
        st: u32,
        at: u32,
    },
    // --- string comparisons; the context compares case-insensitively
    NameIs {
        which: NameWhich,
        text: String,
    },
    StageVarIs {
        name: String,
        text: String,
    },
    Command(String),
    // --- one static name
    Const(String),
    GetHitVar(String),
    // --- stack args
    /// args: 1 (element number)
    AnimElemTime,
    /// args: 1 (time)
    AnimElemNo,
    /// args: 1
    AnimExist,
    /// args: 1
    SelfAnimExist,
    /// args: 1 (id, -1 = any)
    NumHelper,
    /// args: 1 (id, -1 = any)
    NumExplod,
    /// args: 1 (id, -1 = any)
    NumTarget,
    /// args: 1
    NumProjId,
    /// args: 1
    PlayerIdExist,
    /// args: 1 (id, 0 = any)
    ProjHitTime,
    /// args: 1
    ProjGuardedTime,
    /// args: 1
    ProjContactTime,
    /// args: 1
    ProjCancelTime,
    /// args: 1 (id, i32::MIN = any)
    IsHelper,
    /// args: 1 ; Const240p / 480p / 720p scaling helpers (value is the localcoord height)
    ConstCoord(u32),
}

impl Trigger {
    /// Number of stack arguments the trigger consumes.
    pub fn arity(&self) -> usize {
        use Trigger::*;
        match self {
            AnimElemTime | AnimElemNo | AnimExist | SelfAnimExist | NumHelper | NumExplod
            | NumTarget | NumProjId | PlayerIdExist | ProjHitTime | ProjGuardedTime
            | ProjContactTime | ProjCancelTime | IsHelper | ConstCoord(_) => 1,
            _ => 0,
        }
    }
}
