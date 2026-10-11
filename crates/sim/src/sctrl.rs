//! State controllers, compiled from raw parameters at load time.

use rugen_cns::state::Controller;
use rugen_cns::{Code, Error, Result, VarKind, compile, compile_list};

use crate::explod::{PosType, Space, Trans};
use crate::hit::{HitDefSpec, attr};
use crate::projectile::ProjectileSpec;

/// Compiled Explod / ModifyExplod parameters.
#[derive(Clone, Debug)]
pub struct ExplodSpec {
    pub anim: Option<(bool, Code)>,
    pub id: Option<Code>,
    pub pos: Option<Vec<Code>>,
    pub postype: Option<PosType>,
    pub space: Option<Space>,
    pub facing: Option<Code>,
    pub vfacing: Option<Code>,
    pub bindtime: Option<Code>,
    pub bindid: Option<Code>,
    pub vel: Option<Vec<Code>>,
    pub accel: Option<Vec<Code>>,
    pub friction: Option<Vec<Code>>,
    pub random: Option<Vec<Code>>,
    pub removetime: Option<Code>,
    pub supermovetime: Option<Code>,
    pub pausemovetime: Option<Code>,
    pub supermove: Option<Code>,
    pub scale: Option<Vec<Code>>,
    pub sprpriority: Option<Code>,
    pub ontop: Option<Code>,
    pub under: Option<Code>,
    pub removeongethit: Option<Code>,
    pub removeonchangestate: Option<Code>,
    pub ignorehitpause: Option<Code>,
    pub trans: Option<Trans>,
    pub alpha: Option<Vec<Code>>,
    pub angle: Option<Code>,
    pub animelem: Option<Code>,
    pub animfreeze: Option<Code>,
}

/// `trans = none/add/add1/addalpha/sub` (case-insensitive).
pub fn parse_trans(c: &Controller) -> Option<Trans> {
    c.param("trans")
        .map(|t| match t.trim().to_ascii_lowercase().as_str() {
            "none" => Trans::None,
            "add" => Trans::Add,
            "add1" => Trans::Add1,
            "addalpha" => Trans::AddAlpha,
            "sub" => Trans::Sub,
            _ => Trans::Default,
        })
}

/// PalFX / AllPalFX / BGPalFX parameters.
#[derive(Clone, Debug)]
pub struct PalFxSpec {
    pub time: Option<Code>,
    pub add: Option<Vec<Code>>,
    pub mul: Option<Vec<Code>>,
    pub sinadd: Option<Vec<Code>>,
    pub sinmul: Option<Vec<Code>>,
    pub sincolor: Option<Vec<Code>>,
    pub invertall: Option<Code>,
    pub color: Option<Code>,
}

impl PalFxSpec {
    pub fn compile(c: &Controller, prefix: &str) -> Result<Self> {
        let k = |n: &str| format!("{prefix}{n}");
        Ok(Self {
            time: one(c, &k("time"))?,
            add: c.exprs(&k("add"))?,
            mul: c.exprs(&k("mul"))?,
            sinadd: c.exprs(&k("sinadd"))?,
            sinmul: c.exprs(&k("sinmul"))?,
            sincolor: c.exprs(&k("sincolor"))?,
            invertall: one(c, &k("invertall"))?,
            color: one(c, &k("color"))?,
        })
    }
}

/// AfterImage parameters.
#[derive(Clone, Debug)]
pub struct AfterImageSpec {
    pub time: Option<Code>,
    pub length: Option<Code>,
    pub palcolor: Option<Code>,
    pub palinvertall: Option<Code>,
    pub palbright: Option<Vec<Code>>,
    pub palcontrast: Option<Vec<Code>>,
    pub palpostbright: Option<Vec<Code>>,
    pub paladd: Option<Vec<Code>>,
    pub palmul: Option<Vec<Code>>,
    pub timegap: Option<Code>,
    pub framegap: Option<Code>,
    pub trans: Option<Trans>,
    pub alpha: Option<Vec<Code>>,
    pub ignorehitpause: Option<Code>,
}

impl AfterImageSpec {
    pub fn compile(c: &Controller) -> Result<Self> {
        Ok(Self {
            time: one(c, "time")?,
            length: one(c, "length")?,
            palcolor: one(c, "palcolor")?,
            palinvertall: one(c, "palinvertall")?,
            palbright: c.exprs("palbright")?,
            palcontrast: c.exprs("palcontrast")?,
            palpostbright: c.exprs("palpostbright")?,
            paladd: c.exprs("paladd")?,
            palmul: c.exprs("palmul")?,
            timegap: one(c, "timegap")?,
            framegap: one(c, "framegap")?,
            trans: parse_trans(c),
            alpha: c.exprs("alpha")?,
            ignorehitpause: one(c, "ignorehitpause")?,
        })
    }
}

impl ExplodSpec {
    pub fn compile(c: &Controller, need_anim: bool) -> Result<Self> {
        let anim = match c.exprs_fx("anim")? {
            Some((prefix, mut codes)) if !codes.is_empty() => {
                Some((prefix == Some('f'), codes.remove(0)))
            }
            _ if need_anim => return Err(Error::new("missing anim")),
            _ => None,
        };
        let trans = parse_trans(c);
        Ok(Self {
            anim,
            id: one(c, "id")?,
            pos: c.exprs("pos")?,
            postype: c.param("postype").and_then(PosType::parse),
            space: c.param("space").map(|s| {
                if s.trim().eq_ignore_ascii_case("screen") {
                    Space::Screen
                } else {
                    Space::Stage
                }
            }),
            facing: one(c, "facing")?,
            vfacing: one(c, "vfacing")?,
            bindtime: one(c, "bindtime")?,
            bindid: one(c, "bindid")?,
            vel: c.exprs("vel")?,
            accel: c.exprs("accel")?,
            friction: c.exprs("friction")?,
            random: c.exprs("random")?,
            removetime: one(c, "removetime")?,
            supermovetime: one(c, "supermovetime")?,
            pausemovetime: one(c, "pausemovetime")?,
            supermove: one(c, "supermove")?,
            scale: c.exprs("scale")?,
            sprpriority: one(c, "sprpriority")?,
            ontop: one(c, "ontop")?,
            under: one(c, "under")?,
            removeongethit: one(c, "removeongethit")?,
            removeonchangestate: one(c, "removeonchangestate")?,
            ignorehitpause: one(c, "ignorehitpause")?,
            trans,
            alpha: c.exprs("alpha")?,
            angle: one(c, "angle")?,
            animelem: one(c, "animelem")?,
            animfreeze: one(c, "animfreeze")?,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Sctrl {
    Null,
    ChangeState {
        value: Code,
        ctrl: Option<Code>,
        anim: Option<Code>,
    },
    SelfState {
        value: Code,
        ctrl: Option<Code>,
        anim: Option<Code>,
    },
    ChangeAnim {
        value: Code,
        elem: Option<Code>,
    },
    ChangeAnim2 {
        value: Code,
        elem: Option<Code>,
    },
    VelSet {
        x: Option<Code>,
        y: Option<Code>,
    },
    VelAdd {
        x: Option<Code>,
        y: Option<Code>,
    },
    VelMul {
        x: Option<Code>,
        y: Option<Code>,
    },
    PosAdd {
        x: Option<Code>,
        y: Option<Code>,
    },
    PosSet {
        x: Option<Code>,
        y: Option<Code>,
    },
    PosFreeze {
        value: Option<Code>,
    },
    CtrlSet {
        value: Code,
    },
    StateTypeSet {
        statetype: Option<char>,
        movetype: Option<char>,
        physics: Option<char>,
    },
    VarSet {
        kind: VarKind,
        index: Code,
        value: Code,
    },
    VarAdd {
        kind: VarKind,
        index: Code,
        value: Code,
    },
    VarRangeSet {
        float: bool,
        value: Code,
        first: Option<Code>,
        last: Option<Code>,
    },
    Turn,
    PlaySnd {
        own: bool,
        group: Code,
        number: Code,
        channel: Option<Code>,
        volume: Option<Code>,
    },
    SprPriority {
        value: Code,
    },
    AssertSpecial {
        flags: Vec<String>,
    },
    Width {
        edge: Option<Vec<Code>>,
        player: Option<Vec<Code>>,
        value: Option<Vec<Code>>,
    },
    ScreenBound {
        value: Option<Code>,
        movecamera: Option<Vec<Code>>,
    },
    PowerAdd {
        value: Code,
    },
    PowerSet {
        value: Code,
    },
    LifeAdd {
        value: Code,
        kill: Option<Code>,
        absolute: Option<Code>,
    },
    LifeSet {
        value: Code,
    },
    Gravity,
    HitDef(Box<HitDefSpec>),
    HitVelSet {
        x: Option<Code>,
        y: Option<Code>,
    },
    HitFallSet {
        value: Option<Code>,
        xvel: Option<Code>,
        yvel: Option<Code>,
    },
    HitFallVel,
    HitFallDamage,
    HitAdd {
        value: Code,
    },
    MoveHitReset,
    /// `NotHitBy` (`not = true`) / `HitBy`: slot 0 or 1, attr bits, time.
    HitBy {
        not: bool,
        slot: usize,
        attr: u32,
        time: Option<Code>,
    },
    TargetState {
        value: Code,
        id: Option<Code>,
    },
    TargetLifeAdd {
        value: Code,
        kill: Option<Code>,
        absolute: Option<Code>,
        id: Option<Code>,
    },
    TargetPowerAdd {
        value: Code,
        id: Option<Code>,
    },
    TargetVelSet {
        x: Option<Code>,
        y: Option<Code>,
        id: Option<Code>,
    },
    TargetVelAdd {
        x: Option<Code>,
        y: Option<Code>,
        id: Option<Code>,
    },
    TargetFacing {
        value: Code,
        id: Option<Code>,
    },
    TargetBind {
        time: Option<Code>,
        pos: Option<Vec<Code>>,
        id: Option<Code>,
    },
    TargetDrop {
        excludeid: Option<Code>,
        keepone: Option<Code>,
    },
    PlayerPush {
        value: Option<Code>,
    },
    AttackMulSet {
        value: Code,
    },
    DefenceMulSet {
        value: Code,
    },
    FallEnvShake,
    EnvShake {
        time: Code,
        freq: Option<Code>,
        ampl: Option<Code>,
        phase: Option<Code>,
    },
    Trans {
        trans: Trans,
        alpha: Option<Vec<Code>>,
    },
    AngleDraw {
        value: Option<Code>,
        scale: Option<Vec<Code>>,
    },
    AngleSet {
        value: Option<Code>,
    },
    AngleAdd {
        value: Option<Code>,
    },
    AngleMul {
        value: Option<Code>,
    },
    PalFx(Box<PalFxSpec>),
    AllPalFx(Box<PalFxSpec>),
    BgPalFx(Box<PalFxSpec>),
    AfterImage(Box<AfterImageSpec>),
    AfterImageTime {
        time: Option<Code>,
    },
    Explod(Box<ExplodSpec>),
    ModifyExplod {
        id: Option<Code>,
        spec: Box<ExplodSpec>,
    },
    RemoveExplod {
        id: Option<Code>,
    },
    ExplodBindTime {
        id: Option<Code>,
        time: Option<Code>,
    },
    Pause {
        time: Code,
        movetime: Option<Code>,
    },
    SuperPause {
        time: Option<Code>,
        movetime: Option<Code>,
        anim: Option<(bool, Code)>,
        pos: Option<Vec<Code>>,
        sound: Option<(bool, Vec<Code>)>,
        darken: Option<Code>,
        p2defmul: Option<Code>,
        poweradd: Option<Code>,
        unhittable: Option<Code>,
    },
    Projectile(Box<ProjectileSpec>),
    Helper {
        stateno: Option<Code>,
        id: Option<Code>,
        pos: Option<Vec<Code>>,
        postype: Option<PosType>,
        facing: Option<Code>,
        keyctrl: Option<Code>,
        player_type: bool,
        supermovetime: Option<Code>,
        pausemovetime: Option<Code>,
    },
    DestroySelf {
        recursive: Option<Code>,
        removeexplods: Option<Code>,
    },
    /// Recognised but not implemented yet; runs as a no-op.
    Unsupported(String),
}

fn one(c: &Controller, name: &str) -> Result<Option<Code>> {
    c.expr(name)
}
fn req(c: &Controller, name: &str) -> Result<Code> {
    c.expr(name)?
        .ok_or_else(|| Error::new(format!("missing {name}")))
}
fn letter(c: &Controller, name: &str) -> Option<char> {
    c.param(name)
        .and_then(|v| v.trim().chars().next())
        .map(|ch| ch.to_ascii_lowercase())
}

impl Sctrl {
    pub fn compile(c: &Controller) -> Result<Self> {
        Ok(match c.kind.as_str() {
            "null" => Sctrl::Null,
            "changestate" | "selfstate" => {
                let value = req(c, "value")?;
                let ctrl = one(c, "ctrl")?;
                let anim = one(c, "anim")?;
                if c.kind == "changestate" {
                    Sctrl::ChangeState { value, ctrl, anim }
                } else {
                    Sctrl::SelfState { value, ctrl, anim }
                }
            }
            "changeanim" | "changeanim2" => {
                let (prefix, mut codes) = c
                    .exprs_fx("value")?
                    .ok_or_else(|| Error::new("missing value"))?;
                if prefix.is_some() {
                    return Ok(Sctrl::Unsupported(format!(
                        "{} with {:?} prefix",
                        c.kind, prefix
                    )));
                }
                let value = codes
                    .drain(..1)
                    .next()
                    .ok_or_else(|| Error::new("empty value"))?;
                let elem = one(c, "elem")?;
                if c.kind == "changeanim" {
                    Sctrl::ChangeAnim { value, elem }
                } else {
                    Sctrl::ChangeAnim2 { value, elem }
                }
            }
            "velset" => Sctrl::VelSet {
                x: one(c, "x")?,
                y: one(c, "y")?,
            },
            "veladd" => Sctrl::VelAdd {
                x: one(c, "x")?,
                y: one(c, "y")?,
            },
            "velmul" => Sctrl::VelMul {
                x: one(c, "x")?,
                y: one(c, "y")?,
            },
            "posadd" => Sctrl::PosAdd {
                x: one(c, "x")?,
                y: one(c, "y")?,
            },
            "posset" => Sctrl::PosSet {
                x: one(c, "x")?,
                y: one(c, "y")?,
            },
            "posfreeze" => Sctrl::PosFreeze {
                value: one(c, "value")?,
            },
            "ctrlset" => Sctrl::CtrlSet {
                value: req(c, "value")?,
            },
            "statetypeset" => Sctrl::StateTypeSet {
                statetype: letter(c, "statetype"),
                movetype: letter(c, "movetype"),
                physics: letter(c, "physics"),
            },
            "varset" | "varadd" => {
                let (kind, index) = if let Some(v) = c.param("v") {
                    (VarKind::Var, compile(v)?)
                } else if let Some(v) = c.param("fv") {
                    (VarKind::FVar, compile(v)?)
                } else {
                    // var(n) = value style: find a key like var(3), fvar(3), sysvar(3)
                    let mut found = None;
                    for (k, v) in &c.params {
                        for (prefix, kind) in [
                            ("var(", VarKind::Var),
                            ("fvar(", VarKind::FVar),
                            ("sysvar(", VarKind::SysVar),
                            ("sysfvar(", VarKind::SysFVar),
                        ] {
                            if let Some(rest) = k.strip_prefix(prefix) {
                                let idx = rest.trim_end_matches(')');
                                found = Some((kind, compile(idx)?, compile(v)?));
                            }
                        }
                    }
                    match found {
                        Some((kind, index, value)) => {
                            return Ok(if c.kind == "varset" {
                                Sctrl::VarSet { kind, index, value }
                            } else {
                                Sctrl::VarAdd { kind, index, value }
                            });
                        }
                        None => return Err(Error::new("missing v= / fv= / var(n)=")),
                    }
                };
                let value = req(c, "value")?;
                if c.kind == "varset" {
                    Sctrl::VarSet { kind, index, value }
                } else {
                    Sctrl::VarAdd { kind, index, value }
                }
            }
            "varrangeset" => {
                let (float, value) = if let Some(v) = c.param("fvalue") {
                    (true, compile(v)?)
                } else {
                    (false, req(c, "value")?)
                };
                Sctrl::VarRangeSet {
                    float,
                    value,
                    first: one(c, "first")?,
                    last: one(c, "last")?,
                }
            }
            "turn" => Sctrl::Turn,
            "playsnd" => {
                let (prefix, codes) = c
                    .exprs_fx("value")?
                    .ok_or_else(|| Error::new("missing value"))?;
                if codes.len() < 2 {
                    return Err(Error::new("value needs group, number"));
                }
                Sctrl::PlaySnd {
                    own: prefix != Some('f'),
                    group: codes[0].clone(),
                    number: codes[1].clone(),
                    channel: one(c, "channel")?,
                    volume: one(c, "volumescale")?.or(one(c, "volume")?),
                }
            }
            "sprpriority" => Sctrl::SprPriority {
                value: req(c, "value")?,
            },
            "assertspecial" => Sctrl::AssertSpecial {
                flags: ["flag", "flag2", "flag3"]
                    .iter()
                    .filter_map(|k| c.param(k))
                    .map(|s| s.trim().to_ascii_lowercase())
                    .collect(),
            },
            "width" => Sctrl::Width {
                edge: c.exprs("edge")?,
                player: c.exprs("player")?,
                value: c.exprs("value")?,
            },
            "screenbound" => Sctrl::ScreenBound {
                value: one(c, "value")?,
                movecamera: c.exprs("movecamera")?,
            },
            "poweradd" => Sctrl::PowerAdd {
                value: req(c, "value")?,
            },
            "powerset" => Sctrl::PowerSet {
                value: req(c, "value")?,
            },
            "lifeadd" => Sctrl::LifeAdd {
                value: req(c, "value")?,
                kill: one(c, "kill")?,
                absolute: one(c, "absolute")?,
            },
            "lifeset" => Sctrl::LifeSet {
                value: req(c, "value")?,
            },
            "gravity" => Sctrl::Gravity,
            "hitdef" => Sctrl::HitDef(Box::new(HitDefSpec::compile(c)?)),
            "hitvelset" => Sctrl::HitVelSet {
                x: one(c, "x")?,
                y: one(c, "y")?,
            },
            "hitfallset" => Sctrl::HitFallSet {
                value: one(c, "value")?,
                xvel: one(c, "xvel")?,
                yvel: one(c, "yvel")?,
            },
            "hitfallvel" => Sctrl::HitFallVel,
            "hitfalldamage" => Sctrl::HitFallDamage,
            "hitadd" => Sctrl::HitAdd {
                value: req(c, "value")?,
            },
            "movehitreset" => Sctrl::MoveHitReset,
            "nothitby" | "hitby" => {
                let (slot, v) = if let Some(v) = c.param("value") {
                    (0, v)
                } else if let Some(v) = c.param("value2") {
                    (1, v)
                } else {
                    return Err(Error::new("missing value / value2"));
                };
                let bits =
                    attr::parse(v, true).ok_or_else(|| Error::new(format!("bad attr {v:?}")))?;
                Sctrl::HitBy {
                    not: c.kind == "nothitby",
                    slot,
                    attr: bits,
                    time: one(c, "time")?,
                }
            }
            "targetstate" => Sctrl::TargetState {
                value: req(c, "value")?,
                id: one(c, "id")?,
            },
            "targetlifeadd" => Sctrl::TargetLifeAdd {
                value: req(c, "value")?,
                kill: one(c, "kill")?,
                absolute: one(c, "absolute")?,
                id: one(c, "id")?,
            },
            "targetpoweradd" => Sctrl::TargetPowerAdd {
                value: req(c, "value")?,
                id: one(c, "id")?,
            },
            "targetvelset" => Sctrl::TargetVelSet {
                x: one(c, "x")?,
                y: one(c, "y")?,
                id: one(c, "id")?,
            },
            "targetveladd" => Sctrl::TargetVelAdd {
                x: one(c, "x")?,
                y: one(c, "y")?,
                id: one(c, "id")?,
            },
            "targetfacing" => Sctrl::TargetFacing {
                value: req(c, "value")?,
                id: one(c, "id")?,
            },
            "targetbind" => Sctrl::TargetBind {
                time: one(c, "time")?,
                pos: c.exprs("pos")?,
                id: one(c, "id")?,
            },
            "targetdrop" => Sctrl::TargetDrop {
                excludeid: one(c, "excludeid")?,
                keepone: one(c, "keepone")?,
            },
            "playerpush" => Sctrl::PlayerPush {
                value: one(c, "value")?,
            },
            "attackmulset" => Sctrl::AttackMulSet {
                value: req(c, "value")?,
            },
            "defencemulset" => Sctrl::DefenceMulSet {
                value: req(c, "value")?,
            },
            "fallenvshake" => Sctrl::FallEnvShake,
            "envshake" => Sctrl::EnvShake {
                time: req(c, "time")?,
                freq: one(c, "freq")?,
                ampl: one(c, "ampl")?,
                phase: one(c, "phase")?,
            },
            "trans" => Sctrl::Trans {
                trans: parse_trans(c).unwrap_or(Trans::Default),
                alpha: c.exprs("alpha")?,
            },
            "angledraw" => Sctrl::AngleDraw {
                value: one(c, "value")?,
                scale: c.exprs("scale")?,
            },
            "angleset" => Sctrl::AngleSet {
                value: one(c, "value")?,
            },
            "angleadd" => Sctrl::AngleAdd {
                value: one(c, "value")?,
            },
            "anglemul" => Sctrl::AngleMul {
                value: one(c, "value")?,
            },
            "palfx" => Sctrl::PalFx(Box::new(PalFxSpec::compile(c, "")?)),
            "allpalfx" => Sctrl::AllPalFx(Box::new(PalFxSpec::compile(c, "")?)),
            "bgpalfx" => Sctrl::BgPalFx(Box::new(PalFxSpec::compile(c, "")?)),
            "afterimage" => Sctrl::AfterImage(Box::new(AfterImageSpec::compile(c)?)),
            "afterimagetime" => Sctrl::AfterImageTime {
                time: one(c, "time")?.or(one(c, "value")?),
            },
            "explod" => Sctrl::Explod(Box::new(ExplodSpec::compile(c, true)?)),
            "modifyexplod" => Sctrl::ModifyExplod {
                id: one(c, "id")?,
                spec: Box::new(ExplodSpec::compile(c, false)?),
            },
            "removeexplod" => Sctrl::RemoveExplod { id: one(c, "id")? },
            "explodbindtime" => Sctrl::ExplodBindTime {
                id: one(c, "id")?,
                time: one(c, "time")?.or(one(c, "value")?),
            },
            "pause" => Sctrl::Pause {
                time: req(c, "time")?,
                movetime: one(c, "movetime")?,
            },
            "projectile" => Sctrl::Projectile(Box::new(ProjectileSpec::compile(c)?)),
            "helper" => Sctrl::Helper {
                stateno: one(c, "stateno")?,
                id: one(c, "id")?,
                pos: c.exprs("pos")?,
                postype: c.param("postype").and_then(PosType::parse),
                facing: one(c, "facing")?,
                keyctrl: one(c, "keyctrl")?,
                player_type: c
                    .param("helpertype")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("player")),
                supermovetime: one(c, "supermovetime")?,
                pausemovetime: one(c, "pausemovetime")?,
            },
            "destroyself" => Sctrl::DestroySelf {
                recursive: one(c, "recursive")?,
                removeexplods: one(c, "removeexplods")?,
            },
            "superpause" => {
                let anim = match c.exprs_fx("anim")? {
                    Some((prefix, mut codes)) if !codes.is_empty() => {
                        Some((prefix != Some('s'), codes.remove(0)))
                    }
                    _ => None,
                };
                let sound = c
                    .exprs_fx("sound")?
                    .map(|(prefix, codes)| (prefix != Some('s'), codes));
                Sctrl::SuperPause {
                    time: one(c, "time")?,
                    movetime: one(c, "movetime")?,
                    anim,
                    pos: c.exprs("pos")?,
                    sound,
                    darken: one(c, "darken")?,
                    p2defmul: one(c, "p2defmul")?,
                    poweradd: one(c, "poweradd")?,
                    unhittable: one(c, "unhittable")?,
                }
            }
            other => {
                // make sure the parameters at least compile where they are expressions
                let _ = compile_list;
                Sctrl::Unsupported(other.to_string())
            }
        })
    }
}
