//! `[Statedef n]` and `[State n, label]` blocks.
//!
//! Trigger logic per controller:
//! `all(triggerall) && (all(trigger1) || all(trigger2) || ...)`, trigger
//! numbers contiguous from 1 (a gap drops the rest, like MUGEN). Controller
//! parameters are kept as raw text; the simulation compiles the ones it
//! treats as expressions with [`Controller::exprs`].

use indexmap::IndexMap;

use crate::code::Code;
use crate::parser::{compile, compile_list};
use crate::{Error, Result};

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StateType {
    #[default]
    S,
    C,
    A,
    L,
    /// Unchanged
    U,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MoveType {
    #[default]
    I,
    A,
    H,
    U,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Physics {
    S,
    C,
    A,
    #[default]
    N,
    U,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StateDef {
    pub no: i32,
    pub line: usize,
    pub state_type: StateType,
    pub move_type: MoveType,
    pub physics: Physics,
    pub anim: Option<Code>,
    pub velset: Option<Vec<Code>>,
    pub ctrl: Option<Code>,
    pub poweradd: Option<Code>,
    pub juggle: Option<Code>,
    pub facep2: Option<Code>,
    pub hitdefpersist: Option<Code>,
    pub movehitpersist: Option<Code>,
    pub hitcountpersist: Option<Code>,
    pub sprpriority: Option<Code>,
    pub controllers: Vec<Controller>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Controller {
    /// Lowercased `type`.
    pub kind: String,
    /// Text after `State` in the header, e.g. `200, 1`.
    pub label: String,
    pub line: usize,
    pub triggerall: Vec<Code>,
    /// `triggers[i]` = all `trigger{i+1}` lines.
    pub triggers: Vec<Vec<Code>>,
    pub persistent: i32,
    pub ignorehitpause: bool,
    /// Remaining parameters, lowercased keys, raw values, first wins.
    pub params: IndexMap<String, String>,
}

impl Controller {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }
    /// Compile a parameter as a comma-separated expression list.
    pub fn exprs(&self, name: &str) -> Result<Option<Vec<Code>>> {
        match self.param(name) {
            None => Ok(None),
            Some(v) => compile_list(v)
                .map(Some)
                .map_err(|e| Error::new(format!("{name}: {e}"))),
        }
    }
    /// Like [`exprs`](Self::exprs) but splits a leading `S`/`F` FX prefix (sounds, anims, sparks).
    pub fn exprs_fx(&self, name: &str) -> Result<Option<(Option<char>, Vec<Code>)>> {
        match self.param(name) {
            None => Ok(None),
            Some(v) => {
                let (prefix, rest) = crate::parser::strip_fx_prefix(v);
                compile_list(rest)
                    .map(|c| Some((prefix, c)))
                    .map_err(|e| Error::new(format!("{name}: {e}")))
            }
        }
    }
    /// Compile a parameter as one expression.
    pub fn expr(&self, name: &str) -> Result<Option<Code>> {
        match self.param(name) {
            None => Ok(None),
            Some(v) => compile(v)
                .map(Some)
                .map_err(|e| Error::new(format!("{name}: {e}"))),
        }
    }
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub states: Vec<StateDef>,
    /// Problems that did not stop parsing, `file:line: message`.
    pub errors: Vec<String>,
}

struct Section<'a> {
    name: String,
    rest: String,
    line: usize,
    entries: Vec<(String, &'a str, usize)>,
}

fn sections(text: &str) -> Vec<Section<'_>> {
    let mut out: Vec<Section> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = rugen_formats::text::strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        if let Some((name, rest)) = rugen_formats::text::section(raw) {
            out.push(Section {
                name: name.trim_end().to_string(),
                rest: rest.to_string(),
                line: i + 1,
                entries: Vec::new(),
            });
            continue;
        }
        let Some(sec) = out.last_mut() else { continue };
        let Some(eq) = line.find('=') else { continue };
        let key = line[..eq].trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        sec.entries.push((key, line[eq + 1..].trim(), i + 1));
    }
    out
}

/// Parse all states in a `.cns` / `.cmd` text. `file` labels errors.
pub fn parse_states(file: &str, text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut cur: Option<StateDef> = None;
    for sec in sections(text) {
        match sec.name.as_str() {
            "statedef" => {
                if let Some(s) = cur.take() {
                    out.states.push(s);
                }
                match statedef(&sec) {
                    Ok(s) => cur = Some(s),
                    Err(e) => out.errors.push(format!("{file}:{}: {e}", sec.line)),
                }
            }
            "state" => {
                let Some(st) = cur.as_mut() else {
                    out.errors.push(format!(
                        "{file}:{}: [State] before any [Statedef]",
                        sec.line
                    ));
                    continue;
                };
                match controller(&sec) {
                    Ok(c) => st.controllers.push(c),
                    Err(e) => out.errors.push(format!("{file}:{}: {e}", sec.line)),
                }
            }
            _ => {}
        }
    }
    if let Some(s) = cur {
        out.states.push(s);
    }
    out
}

fn first<'a>(sec: &Section<'a>, key: &str) -> Option<&'a str> {
    sec.entries
        .iter()
        .find(|(k, _, _)| k == key)
        .map(|(_, v, _)| *v)
}

fn statedef(sec: &Section) -> Result<StateDef> {
    let no = rugen_formats::text::atoi(&sec.rest);
    let mut s = StateDef {
        no,
        line: sec.line,
        ..Default::default()
    };
    let letter = |key: &str| {
        first(sec, key)
            .and_then(|v| v.chars().next())
            .map(|c| c.to_ascii_lowercase())
    };
    s.state_type = match letter("type") {
        None | Some('s') => StateType::S,
        Some('c') => StateType::C,
        Some('a') => StateType::A,
        Some('l') => StateType::L,
        Some('u') => StateType::U,
        Some(c) => return Err(Error::new(format!("invalid type {c:?}"))),
    };
    s.move_type = match letter("movetype") {
        None | Some('i') => MoveType::I,
        Some('a') => MoveType::A,
        Some('h') => MoveType::H,
        Some('u') => MoveType::U,
        Some(c) => return Err(Error::new(format!("invalid movetype {c:?}"))),
    };
    s.physics = match letter("physics") {
        None | Some('n') => Physics::N,
        Some('s') => Physics::S,
        Some('c') => Physics::C,
        Some('a') => Physics::A,
        Some('u') => Physics::U,
        Some(c) => return Err(Error::new(format!("invalid physics {c:?}"))),
    };
    let one = |key: &str| -> Result<Option<Code>> {
        match first(sec, key) {
            None => Ok(None),
            Some("") => Ok(None),
            Some(v) => compile(v)
                .map(Some)
                .map_err(|e| Error::new(format!("{key}: {e}"))),
        }
    };
    s.anim = one("anim")?;
    s.ctrl = one("ctrl")?;
    s.poweradd = one("poweradd")?;
    s.juggle = one("juggle")?;
    s.facep2 = one("facep2")?;
    s.hitdefpersist = one("hitdefpersist")?;
    s.movehitpersist = one("movehitpersist")?;
    s.hitcountpersist = one("hitcountpersist")?;
    s.sprpriority = one("sprpriority")?;
    s.velset = match first(sec, "velset") {
        None => None,
        Some(v) => Some(compile_list(v).map_err(|e| Error::new(format!("velset: {e}")))?),
    };
    Ok(s)
}

fn controller(sec: &Section) -> Result<Controller> {
    let mut c = Controller {
        label: sec.rest.clone(),
        line: sec.line,
        persistent: 1,
        ..Default::default()
    };
    let mut numbered: Vec<(u32, Code)> = Vec::new();
    for (key, value, line) in &sec.entries {
        let at = |e: Error| Error::new(format!("line {line}: {key}: {e}"));
        match key.as_str() {
            "type" => {
                if c.kind.is_empty() {
                    c.kind = value.to_ascii_lowercase();
                }
            }
            "triggerall" => c.triggerall.push(compile(value).map_err(at)?),
            "persistent" => c.persistent = rugen_formats::text::atoi(value),
            "ignorehitpause" => c.ignorehitpause = rugen_formats::text::atoi(value) != 0,
            k if k.starts_with("trigger") => {
                let n: u32 = k[7..]
                    .parse()
                    .map_err(|_| Error::new(format!("line {line}: bad trigger name {k:?}")))?;
                if n == 0 {
                    return Err(Error::new(format!("line {line}: trigger0 is not valid")));
                }
                numbered.push((n, compile(value).map_err(at)?));
            }
            _ => {
                c.params
                    .entry(key.clone())
                    .or_insert_with(|| value.to_string());
            }
        }
    }
    if c.kind.is_empty() {
        return Err(Error::new("missing type"));
    }
    let max = numbered.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut groups: Vec<Vec<Code>> = vec![Vec::new(); max as usize];
    for (n, code) in numbered {
        groups[n as usize - 1].push(code);
    }
    let contiguous = groups
        .iter()
        .position(Vec::is_empty)
        .unwrap_or(groups.len());
    if contiguous < groups.len() {
        log::warn!(
            "line {}: trigger{} missing, later triggers ignored",
            sec.line,
            contiguous + 1
        );
        groups.truncate(contiguous);
    }
    if groups.is_empty() && c.kind != "null" {
        return Err(Error::new("no trigger1"));
    }
    c.triggers = groups;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugen_formats::text::decode;

    #[test]
    fn basic() {
        let p = parse_states(
            "t",
            "[Statedef 200]\ntype = S\nmovetype=A\nphysics = S\nanim = 200\nctrl = 0\nvelset = 0, 1\n\n[State 200, 1]\ntype = HitDef\ntrigger1 = AnimElem = 2\nattr = S, NA\ndamage = 10\n\n[State 200, 2] ;x\ntype = ChangeState\ntriggerall = 1\ntrigger1 = AnimTime = 0\ntrigger1 = time > 3\ntrigger2 = 0\nvalue = 0\nctrl = 1\n",
        );
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let s = &p.states[0];
        assert_eq!(
            (s.no, s.state_type, s.move_type, s.physics),
            (200, StateType::S, MoveType::A, Physics::S)
        );
        assert_eq!(s.velset.as_ref().unwrap().len(), 2);
        assert_eq!(s.controllers.len(), 2);
        let c = &s.controllers[1];
        assert_eq!(c.kind, "changestate");
        assert_eq!(c.triggerall.len(), 1);
        assert_eq!(c.triggers.len(), 2);
        assert_eq!(c.triggers[0].len(), 2);
        assert_eq!(c.param("value"), Some("0"));
        assert_eq!(s.controllers[0].param("attr"), Some("S, NA"));
    }

    #[test]
    fn trigger_gap_and_errors() {
        let p = parse_states(
            "t",
            "[Statedef 1]\n[State 1]\ntype = Null\ntrigger1 = 1\ntrigger3 = 1\n[State 1]\ntype = Null\ntrigger1 = 1 +\n",
        );
        assert_eq!(p.states[0].controllers.len(), 1);
        assert_eq!(p.states[0].controllers[0].triggers.len(), 1);
        assert_eq!(p.errors.len(), 1);
    }

    fn reference(rel: &str) -> Option<String> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference")
            .join(rel);
        std::fs::read(p).ok().map(|b| decode(&b))
    }

    #[test]
    fn corpus_parses_clean() {
        let files = [
            "mugen-1.1b1/chars/kfm/kfm.cns",
            "mugen-1.1b1/chars/kfm/kfm.cmd",
            "mugen-1.1b1/data/common1.cns",
            "ikemen-go-v1.0.0/chars/kfm720/kfm720.cns",
            "ikemen-go-v1.0.0/chars/kfm720/kfm720.cmd",
        ];
        let mut total = 0;
        for f in files {
            let Some(text) = reference(f) else { continue };
            let p = parse_states(f, &text);
            assert!(p.errors.is_empty(), "{f}:\n{}", p.errors.join("\n"));
            assert!(!p.states.is_empty(), "{f}: no states");
            total += p.states.iter().map(|s| s.controllers.len()).sum::<usize>();
            // every parameter that is commonly an expression must compile
            for s in &p.states {
                for c in &s.controllers {
                    let string_valued = matches!(
                        c.kind.as_str(),
                        "nothitby" | "hitby" | "assertspecial" | "hitoverride" | "statetypeset"
                    );
                    for k in [
                        "value",
                        "x",
                        "y",
                        "anim",
                        "time",
                        "elem",
                        "v",
                        "fv",
                        "var",
                        "id",
                        "pos",
                        "ctrl",
                        "damage",
                        "pausetime",
                        "sparkxy",
                        "sparkno",
                        "hitsound",
                        "guardsound",
                    ] {
                        if string_valued && k == "value" {
                            continue;
                        }
                        if let Err(e) = c.exprs_fx(k) {
                            panic!("{f}:{} [{}] {e}", c.line, c.kind);
                        }
                    }
                }
            }
        }
        if total > 0 {
            assert!(total > 500, "only {total} controllers");
        }
    }
}
