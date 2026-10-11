//! HitDef evaluation, hit variables, collision (push + hit detection).
//! Ported from Ikemen GO's `hitResultCheck`, `hitDetectionPlayer` and
//! `pushDetection`, reduced to what characters (no projectiles, helpers,
//! reversals, binds or hit overrides yet) need.

use indexmap::IndexMap;
use rugen_cns::state::Controller;
use rugen_cns::state::{MoveType, StateType};
use rugen_cns::{Code, Error, Result};

/// Attribute bits: state part (low 6 bits) and attack class (bit 6+), like Ikemen.
pub mod attr {
    pub const ST_S: u32 = 1;
    pub const ST_C: u32 = 2;
    pub const ST_A: u32 = 4;
    pub const ST_L: u32 = 8;
    pub const ST_MASK: u32 = 63;
    pub const AT_NA: u32 = 1 << 6;
    pub const AT_NT: u32 = 1 << 7;
    pub const AT_NP: u32 = 1 << 8;
    pub const AT_SA: u32 = 1 << 9;
    pub const AT_ST: u32 = 1 << 10;
    pub const AT_SP: u32 = 1 << 11;
    pub const AT_HA: u32 = 1 << 12;
    pub const AT_HT: u32 = 1 << 13;
    pub const AT_HP: u32 = 1 << 14;
    pub const AT_AT: u32 = AT_NT | AT_ST | AT_HT;
    pub const AT_AH: u32 = AT_HA | AT_HT | AT_HP;
    pub const AT_ALL: u32 = 0x7fc0;

    /// `S, NA, ST` → bits. Empty attack list means "all" (HitBy/NotHitBy style when `all_if_empty`).
    pub fn parse(s: &str, all_if_empty: bool) -> Option<u32> {
        let mut parts = s.split(',').map(str::trim);
        let st = parts.next()?;
        let mut bits = 0;
        for c in st.chars() {
            bits |= match c.to_ascii_lowercase() {
                's' => ST_S,
                'c' => ST_C,
                'a' => ST_A,
                'l' => ST_L,
                _ => return None,
            };
        }
        let mut any = false;
        for p in parts {
            any = true;
            bits |= match p.to_ascii_lowercase().as_str() {
                "na" => AT_NA,
                "nt" => AT_NT,
                "np" => AT_NP,
                "sa" => AT_SA,
                "st" => AT_ST,
                "sp" => AT_SP,
                "ha" => AT_HA,
                "ht" => AT_HT,
                "hp" => AT_HP,
                "aa" => AT_NA | AT_SA | AT_HA,
                "at" => AT_AT,
                "ap" => AT_NP | AT_SP | AT_HP,
                "n" => AT_NA | AT_NT | AT_NP,
                "s" => AT_SA | AT_ST | AT_SP,
                "h" | "a" => AT_AH,
                "" => 0,
                _ => return None,
            };
        }
        if !any && all_if_empty {
            bits |= AT_ALL;
        }
        Some(bits)
    }

    pub fn state_bits(t: super::StateType) -> u32 {
        match t {
            super::StateType::S => ST_S,
            super::StateType::C => ST_C,
            super::StateType::A => ST_A,
            super::StateType::L => ST_L,
            super::StateType::U => 0,
        }
    }
}

/// hitflag / guardflag bits.
pub mod hf {
    pub const H: u32 = 1;
    pub const L: u32 = 2;
    pub const A: u32 = 4;
    pub const D: u32 = 8;
    pub const F: u32 = 16;
    pub const P: u32 = 32;
    pub const MINUS: u32 = 64;
    pub const PLUS: u32 = 128;

    pub fn parse(s: &str) -> u32 {
        let mut f = 0;
        for c in s.chars() {
            f |= match c.to_ascii_lowercase() {
                'h' => H,
                'l' => L,
                'm' => H | L,
                'a' => A,
                'd' => D,
                'f' => F,
                'p' => P,
                '-' => MINUS,
                '+' => PLUS,
                _ => 0,
            };
        }
        f
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Reaction {
    #[default]
    Light = 0,
    Medium = 1,
    Hard = 2,
    Back = 3,
    Up = 4,
    DiagUp = 5,
}

impl Reaction {
    fn parse(s: &str) -> Option<Reaction> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "light" => Reaction::Light,
            "medium" | "med" => Reaction::Medium,
            "hard" => Reaction::Hard,
            "back" => Reaction::Back,
            "up" => Reaction::Up,
            "diagup" => Reaction::DiagUp,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HitType {
    None = 0,
    #[default]
    High = 1,
    Low = 2,
    Trip = 3,
}

impl HitType {
    fn parse(s: &str) -> Option<HitType> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "none" => HitType::None,
            "high" => HitType::High,
            "low" => HitType::Low,
            "trip" => HitType::Trip,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Trade {
    #[default]
    Hit,
    Miss,
    Dodge,
}

/// Compiled HitDef controller: fixed strings parsed, numeric params kept as code.
#[derive(Clone, Debug)]
pub struct HitDefSpec {
    pub attr: u32,
    pub hitflag: u32,
    pub guardflag: u32,
    pub affectteam: i32,
    pub animtype: Reaction,
    pub air_animtype: Option<Reaction>,
    pub fall_animtype: Option<Reaction>,
    pub ground_type: HitType,
    pub air_type: Option<HitType>,
    pub trade: Trade,
    /// Everything numeric: key → (fx prefix, expressions)
    pub exprs: IndexMap<String, (Option<char>, Vec<Code>)>,
}

impl HitDefSpec {
    pub fn compile(c: &Controller) -> Result<Self> {
        let attr_s = c
            .param("attr")
            .ok_or_else(|| Error::new("HitDef needs attr"))?;
        let attr =
            attr::parse(attr_s, false).ok_or_else(|| Error::new(format!("bad attr {attr_s:?}")))?;
        let mut spec = HitDefSpec {
            attr,
            hitflag: c
                .param("hitflag")
                .map_or(hf::H | hf::L | hf::A | hf::F, hf::parse),
            guardflag: c.param("guardflag").map_or(0, hf::parse),
            affectteam: match c
                .param("affectteam")
                .map(|s| s.trim().to_ascii_lowercase())
                .as_deref()
            {
                Some("b") => 0,
                Some("f") => -1,
                _ => 1,
            },
            animtype: c
                .param("animtype")
                .and_then(Reaction::parse)
                .unwrap_or_default(),
            air_animtype: c.param("air.animtype").and_then(Reaction::parse),
            fall_animtype: c.param("fall.animtype").and_then(Reaction::parse),
            ground_type: c
                .param("ground.type")
                .and_then(HitType::parse)
                .unwrap_or_default(),
            air_type: c.param("air.type").and_then(HitType::parse),
            trade: Trade::Hit,
            exprs: IndexMap::new(),
        };
        for (k, v) in &c.params {
            match k.as_str() {
                "attr" | "hitflag" | "guardflag" | "affectteam" | "animtype" | "air.animtype"
                | "fall.animtype" | "ground.type" | "air.type" => {}
                "priority" => {
                    let mut it = v.split(',');
                    let n = it.next().unwrap_or("4").trim();
                    spec.exprs.insert(
                        "priority".into(),
                        (
                            None,
                            vec![
                                rugen_cns::compile(n)
                                    .map_err(|e| Error::new(format!("priority: {e}")))?,
                            ],
                        ),
                    );
                    spec.trade = match it.next().map(|s| s.trim().to_ascii_lowercase()).as_deref() {
                        Some("miss") => Trade::Miss,
                        Some("dodge") => Trade::Dodge,
                        _ => Trade::Hit,
                    };
                }
                _ => {
                    let (prefix, rest) = rugen_cns::parser::strip_fx_prefix(v);
                    let codes = rugen_cns::compile_list(rest)
                        .map_err(|e| Error::new(format!("{k}: {e}")))?;
                    spec.exprs.insert(k.clone(), (prefix, codes));
                }
            }
        }
        Ok(spec)
    }
}

/// Fully evaluated HitDef, with MUGEN's defaults applied.
#[derive(Clone, Debug, PartialEq)]
pub struct HitDef {
    pub attr: u32,
    pub hitflag: u32,
    pub guardflag: u32,
    pub affectteam: i32,
    pub animtype: Reaction,
    pub air_animtype: Reaction,
    pub fall_animtype: Reaction,
    pub ground_type: HitType,
    pub air_type: HitType,
    pub priority: i32,
    pub trade: Trade,
    pub damage: i32,
    pub guard_damage: i32,
    pub pausetime: [i32; 2],
    pub guard_pausetime: [i32; 2],
    pub sparkno: i32,
    pub sparkno_own: bool,
    pub guard_sparkno: i32,
    pub guard_sparkno_own: bool,
    pub sparkxy: [f32; 2],
    pub hitsound: [i32; 2],
    pub hitsound_own: bool,
    pub guardsound: [i32; 2],
    pub guardsound_own: bool,
    pub ground_slidetime: i32,
    pub guard_slidetime: i32,
    pub ground_hittime: i32,
    pub guard_hittime: i32,
    pub air_hittime: i32,
    pub down_hittime: i32,
    pub guard_ctrltime: i32,
    pub airguard_ctrltime: i32,
    pub guard_dist: f32,
    pub yaccel: f32,
    pub ground_velocity: [f32; 2],
    pub guard_velocity: f32,
    pub air_velocity: [f32; 2],
    pub airguard_velocity: [f32; 2],
    pub ground_cornerpush_veloff: f32,
    pub air_cornerpush_veloff: f32,
    pub down_cornerpush_veloff: f32,
    pub guard_cornerpush_veloff: f32,
    pub airguard_cornerpush_veloff: f32,
    pub air_juggle: i32,
    pub mindist: [Option<f32>; 2],
    pub maxdist: [Option<f32>; 2],
    pub p1sprpriority: i32,
    pub p2sprpriority: i32,
    pub p1facing: i32,
    pub p1getp2facing: i32,
    pub p2facing: i32,
    pub p1stateno: i32,
    pub p2stateno: i32,
    pub p2getp1state: bool,
    pub forcestand: bool,
    pub ground_fall: bool,
    pub air_fall: bool,
    pub forcenofall: bool,
    pub fall_xvelocity: Option<f32>,
    pub fall_yvelocity: f32,
    pub fall_recover: bool,
    pub fall_recovertime: i32,
    pub fall_damage: i32,
    pub down_velocity: [f32; 2],
    pub down_bounce: bool,
    pub id: i32,
    pub chainid: i32,
    pub nochainid: [i32; 2],
    pub hitonce: i32,
    pub kill: bool,
    pub guard_kill: bool,
    pub fall_kill: bool,
    pub numhits: i32,
    pub hit_getpower: i32,
    pub guard_getpower: i32,
    pub hit_givepower: i32,
    pub guard_givepower: i32,
    pub envshake_time: i32,
    pub fall_envshake_time: i32,
    /// Set after the first contact when `hitonce`, or when the HitDef is spent.
    pub active: bool,
    /// Attacker's state type when the HitDef was declared (for the attr check).
    pub owner_id: i32,
}

/// What the getter remembers about the last hit (`GetHitVar`).
#[derive(Clone, Debug, PartialEq)]
pub struct GetHitVar {
    pub attr: u32,
    pub guardflag: u32,
    pub hit_type: HitType,
    pub animtype: Reaction,
    pub air_animtype: Reaction,
    pub ground_animtype: Reaction,
    pub fall_animtype: Reaction,
    pub airtype: HitType,
    pub groundtype: HitType,
    pub damage: i32,
    pub hitcount: i32,
    pub guardcount: i32,
    pub fallcount: i32,
    pub hitshaketime: i32,
    pub hittime: i32,
    pub slidetime: i32,
    pub ctrltime: i32,
    pub xvel: f32,
    pub yvel: f32,
    pub yaccel: f32,
    pub xveladd: f32,
    pub yveladd: f32,
    pub hitid: i32,
    pub xoff: f32,
    pub yoff: f32,
    pub fall_xvelocity: Option<f32>,
    pub fall_yvelocity: f32,
    pub fall_time: i32,
    pub fall_recover: bool,
    pub fall_recovertime: i32,
    pub fall_damage: i32,
    pub fall_kill: bool,
    pub fall_envshake_time: i32,
    pub playerid: i32,
    pub fallflag: bool,
    pub guarded: bool,
    pub p2getp1state: bool,
    pub forcestand: bool,
    pub power: i32,
    pub kill: bool,
    pub priority: i32,
    pub facing: i32,
    pub frame: bool,
    pub guardko: bool,
    pub down_recovertime: i32,
    /// `(attacker id, juggle points left)`
    pub targeted_by: Vec<(i32, i32)>,
}

impl Default for GetHitVar {
    fn default() -> Self {
        Self {
            attr: 0,
            guardflag: 0,
            hit_type: HitType::None,
            animtype: Reaction::Light,
            air_animtype: Reaction::Light,
            ground_animtype: Reaction::Light,
            fall_animtype: Reaction::Back,
            airtype: HitType::None,
            groundtype: HitType::None,
            damage: 0,
            hitcount: 0,
            guardcount: 0,
            fallcount: 0,
            hitshaketime: 0,
            hittime: -1,
            slidetime: 0,
            ctrltime: 0,
            xvel: 0.0,
            yvel: 0.0,
            yaccel: 0.35,
            xveladd: 0.0,
            yveladd: 0.0,
            hitid: -1,
            xoff: 0.0,
            yoff: 0.0,
            fall_xvelocity: None,
            fall_yvelocity: -4.5,
            fall_time: 0,
            fall_recover: false,
            fall_recovertime: 0,
            fall_damage: 0,
            fall_kill: true,
            fall_envshake_time: 0,
            playerid: -1,
            fallflag: false,
            guarded: false,
            p2getp1state: false,
            forcestand: false,
            power: 0,
            kill: true,
            priority: 0,
            facing: 0,
            frame: false,
            guardko: false,
            down_recovertime: 0,
            targeted_by: Vec::new(),
        }
    }
}

impl GetHitVar {
    /// Reset between hits, keeping counters and pending damage (Ikemen `selectiveReset`).
    pub fn selective_reset(&mut self) {
        let keep = (
            self.guardko,
            self.damage,
            self.down_recovertime,
            self.fallcount,
            self.fallflag,
            self.frame,
            self.guardcount,
            std::mem::take(&mut self.targeted_by),
            self.hitcount,
            self.kill,
            self.power,
        );
        *self = GetHitVar {
            xoff: self.xoff,
            yoff: self.yoff,
            ..Default::default()
        };
        self.guardko = keep.0;
        self.damage = keep.1;
        self.down_recovertime = keep.2;
        self.fallcount = keep.3;
        self.fallflag = keep.4;
        self.frame = keep.5;
        self.guardcount = keep.6;
        self.targeted_by = keep.7;
        self.hitcount = keep.8;
        self.kill = keep.9;
        self.power = keep.10;
    }

    pub fn juggle_for(&self, id: i32, default: i32) -> i32 {
        self.targeted_by
            .iter()
            .find(|t| t.0 == id)
            .map_or(default, |t| t.1)
    }
    pub fn add_id(&mut self, id: i32, juggle: i32) {
        let j = self.juggle_for(id, juggle);
        self.targeted_by.retain(|t| t.0 != id);
        self.targeted_by.push((id, j));
    }
    pub fn chain_id(&self) -> i32 {
        self.hitid.max(0)
    }
}

/// What the attacker remembers about its last contact.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoveHitVar {
    pub power: i32,
    pub cornerpush_veloff: f32,
    pub frame: bool,
    pub playerid: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MoveContact {
    #[default]
    None,
    Hit,
    Guarded,
    Reversed,
}

/// Rectangle overlap in stage space; `boxes` are `[x1,y1,x2,y2]` relative to `pos`, flipped by `facing`.
pub fn boxes_overlap(
    a: &[[f32; 4]],
    pos_a: [f32; 2],
    facing_a: f32,
    b: &[[f32; 4]],
    pos_b: [f32; 2],
    facing_b: f32,
) -> Option<f32> {
    for ra in a {
        let (mut l1, mut r1) = (ra[0], ra[2]);
        if facing_a < 0.0 {
            (l1, r1) = (-r1, -l1);
        }
        let (l1, r1) = (l1.min(r1) + pos_a[0], l1.max(r1) + pos_a[0]);
        let (t1, b1) = (ra[1].min(ra[3]) + pos_a[1], ra[1].max(ra[3]) + pos_a[1]);
        for rb in b {
            let (mut l2, mut r2) = (rb[0], rb[2]);
            if facing_b < 0.0 {
                (l2, r2) = (-r2, -l2);
            }
            let (l2, r2) = (l2.min(r2) + pos_b[0], l2.max(r2) + pos_b[0]);
            let (t2, b2) = (rb[1].min(rb[3]) + pos_b[1], rb[1].max(rb[3]) + pos_b[1]);
            if l1 < r2 && l2 < r1 && t1 < b2 && t2 < b1 {
                return Some(r1.min(r2) - l1.max(l2));
            }
        }
    }
    None
}

/// Is `move_type`'s state-type part of `attr` hittable by `hitflag`? (attrCheck's hitflag part)
pub fn hitflag_allows(hitflag: u32, st: StateType, hittmp: i8, in_guard_state: bool) -> bool {
    let ok = match st {
        StateType::S => hitflag & hf::H != 0,
        StateType::C => hitflag & hf::L != 0,
        StateType::A => hitflag & hf::A != 0,
        StateType::L => hitflag & hf::D != 0,
        StateType::U => true,
    };
    if !ok {
        return false;
    }
    if hitflag & hf::F == 0 && hittmp >= 2 {
        return false;
    }
    if hitflag & hf::MINUS != 0 && hittmp > 0 {
        return false;
    }
    if hitflag & hf::PLUS != 0 && (hittmp <= 0 || in_guard_state) {
        return false;
    }
    true
}

pub fn is_guard_state(no: i32) -> bool {
    no == 120 || (130..=132).contains(&no) || no == 140 || (150..=155).contains(&no)
}

pub fn gethit_animtype(ghv: &GetHitVar, st: StateType) -> Reaction {
    if ghv.fallflag {
        ghv.fall_animtype
    } else if st == StateType::A {
        ghv.air_animtype
    } else if ghv.ground_animtype as i32 >= Reaction::Back as i32 && ghv.yvel == 0.0 {
        Reaction::Hard
    } else {
        ghv.ground_animtype
    }
}

pub fn movetype_is_hit(m: MoveType) -> bool {
    m == MoveType::H
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_parse() {
        assert_eq!(attr::parse("S, NA", false), Some(attr::ST_S | attr::AT_NA));
        assert_eq!(
            attr::parse("SCA, NA, SA, HA", false),
            Some(attr::ST_S | attr::ST_C | attr::ST_A | attr::AT_NA | attr::AT_SA | attr::AT_HA)
        );
        assert_eq!(
            attr::parse("SCA", true),
            Some(attr::ST_MASK & 7 | attr::AT_ALL)
        );
        assert_eq!(attr::parse("A, AT", false), Some(attr::ST_A | attr::AT_AT));
        assert_eq!(hf::parse("MAF"), hf::H | hf::L | hf::A | hf::F);
        assert_eq!(hf::parse("HL-"), hf::H | hf::L | hf::MINUS);
    }

    #[test]
    fn overlap() {
        let a = [[10.0, -70.0, 40.0, -60.0]];
        let b = [[-10.0, -80.0, 10.0, 0.0]];
        assert!(boxes_overlap(&a, [0.0, 0.0], 1.0, &b, [30.0, 0.0], -1.0).is_some());
        assert!(
            boxes_overlap(&a, [0.0, 0.0], -1.0, &b, [30.0, 0.0], -1.0).is_none(),
            "facing left, box is behind"
        );
        assert!(boxes_overlap(&a, [0.0, 0.0], 1.0, &b, [60.0, 0.0], -1.0).is_none());
    }
}
