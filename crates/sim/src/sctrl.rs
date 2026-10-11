//! State controllers, compiled from raw parameters at load time.

use rugen_cns::state::Controller;
use rugen_cns::{Code, Error, Result, VarKind, compile, compile_list};

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
                    own: prefix == Some('s'),
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
            other => {
                // make sure the parameters at least compile where they are expressions
                let _ = compile_list;
                Sctrl::Unsupported(other.to_string())
            }
        })
    }
}
