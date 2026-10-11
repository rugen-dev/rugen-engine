//! `.cmd` files: `[Remap]`, `[Defaults]`, `[Command]` blocks, plus the
//! state controllers of `[Statedef -1]` (parsed by [`crate::state`]).
//!
//! Command syntax: elements separated by `,`; each element has optional
//! modifiers then keys joined by `+`. Modifiers: `~` release (with optional
//! tick count), `/` hold, `$` four-way direction, `>` no other key between.
//! Directions are uppercase (`B` back), buttons lowercase (`b`).

use crate::state::{Parsed, StateDef, parse_states};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    U,
    D,
    F,
    B,
    UF,
    UB,
    DF,
    DB,
    A,
    Bt,
    C,
    X,
    Y,
    Z,
    S,
}

impl Key {
    pub fn is_direction(self) -> bool {
        matches!(
            self,
            Key::U | Key::D | Key::F | Key::B | Key::UF | Key::UB | Key::DF | Key::DB
        )
    }
    fn parse(s: &str) -> Option<Key> {
        Some(match s {
            "U" => Key::U,
            "D" => Key::D,
            "F" => Key::F,
            "B" => Key::B,
            "UF" => Key::UF,
            "UB" => Key::UB,
            "DF" => Key::DF,
            "DB" => Key::DB,
            "a" => Key::A,
            "b" => Key::Bt,
            "c" => Key::C,
            "x" => Key::X,
            "y" => Key::Y,
            "z" => Key::Z,
            "s" => Key::S,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Elem {
    pub keys: Vec<Key>,
    /// `~`: key must be released; `Some(n)` = held at least n ticks before release.
    pub release: Option<i32>,
    /// `/`
    pub hold: bool,
    /// `$`
    pub four_way: bool,
    /// `>`
    pub strict: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub elems: Vec<Elem>,
    pub time: i32,
    pub buffer_time: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Cmd {
    pub commands: Vec<Command>,
    pub default_time: i32,
    pub default_buffer_time: i32,
    /// Button remaps `(from, to)` from `[Remap]`.
    pub remap: Vec<(Key, Key)>,
    pub states: Vec<StateDef>,
    pub errors: Vec<String>,
}

pub fn parse_elem(s: &str) -> Result<Elem> {
    let mut e = Elem::default();
    let mut rest = s.trim();
    loop {
        if let Some(r) = rest.strip_prefix('~') {
            let n = r.bytes().take_while(u8::is_ascii_digit).count();
            e.release = Some(if n == 0 {
                0
            } else {
                r[..n].parse().unwrap_or(0)
            });
            rest = &r[n..];
        } else if let Some(r) = rest.strip_prefix('/') {
            e.hold = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('$') {
            e.four_way = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('>') {
            e.strict = true;
            rest = r;
        } else {
            break;
        }
        rest = rest.trim_start();
    }
    for k in rest.split('+') {
        let k = k.trim();
        e.keys
            .push(Key::parse(k).ok_or_else(|| Error::new(format!("invalid key {k:?} in {s:?}")))?);
    }
    if e.keys.is_empty() {
        return Err(Error::new(format!("empty command element in {s:?}")));
    }
    Ok(e)
}

pub fn parse_command(s: &str) -> Result<Vec<Elem>> {
    s.split(',').map(parse_elem).collect()
}

pub fn parse(file: &str, text: &str) -> Cmd {
    let mut cmd = Cmd {
        default_time: 15,
        default_buffer_time: 1,
        ..Default::default()
    };
    let def = rugen_formats::def::Def::parse(text);
    if let Some(d) = def.section("defaults") {
        if let Some(t) = d.i32("command.time") {
            cmd.default_time = t;
        }
        if let Some(t) = d.i32("command.buffer.time") {
            cmd.default_buffer_time = t;
        }
    }
    if let Some(r) = def.section("remap") {
        for (k, v) in &r.entries {
            if let (Some(a), Some(b)) = (Key::parse(k), Key::parse(v.trim())) {
                cmd.remap.push((a, b));
            }
        }
    }
    for sec in def.sections_named("command") {
        let Some(name) = sec.str("name") else {
            cmd.errors
                .push(format!("{file}:{}: [Command] without name", sec.line));
            continue;
        };
        let Some(raw) = sec.raw("command") else {
            cmd.errors.push(format!(
                "{file}:{}: [Command] {name:?} without command",
                sec.line
            ));
            continue;
        };
        match parse_command(raw) {
            Ok(elems) => cmd.commands.push(Command {
                name: name.to_string(),
                elems,
                time: sec.i32("time").unwrap_or(cmd.default_time),
                buffer_time: sec.i32("buffer.time").unwrap_or(cmd.default_buffer_time),
            }),
            Err(e) => cmd
                .errors
                .push(format!("{file}:{}: {name:?}: {e}", sec.line)),
        }
    }
    let Parsed { states, errors } = parse_states(file, text);
    cmd.states = states;
    cmd.errors.extend(errors);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements() {
        let e = parse_command("~D, DF, F, x").unwrap();
        assert_eq!(e.len(), 4);
        assert_eq!(
            e[0],
            Elem {
                keys: vec![Key::D],
                release: Some(0),
                ..Default::default()
            }
        );
        assert_eq!(e[3].keys, vec![Key::X]);
        let e = parse_command("/$F, x+y").unwrap();
        assert!(e[0].hold && e[0].four_way);
        assert_eq!(e[1].keys, vec![Key::X, Key::Y]);
        let e = parse_command("~30$B").unwrap();
        assert_eq!(e[0].release, Some(30));
        assert!(e[0].four_way);
        assert!(parse_command(">a").unwrap()[0].strict);
        assert!(parse_command("q").is_err());
    }

    #[test]
    fn kfm_cmd() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../reference/mugen-1.1b1/chars/kfm/kfm.cmd");
        let Ok(bytes) = std::fs::read(p) else { return };
        let c = parse("kfm.cmd", &rugen_formats::text::decode(&bytes));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(
            c.commands
                .iter()
                .filter(|c| c.name == "TripleKFPalm")
                .count()
                == 2
        );
        assert_eq!(c.default_time, 15);
        let ff = c.commands.iter().find(|c| c.name == "FF").unwrap();
        assert_eq!(ff.time, 10);
        assert_eq!(ff.elems.len(), 2);
        assert_eq!(c.states.len(), 1);
        assert_eq!(c.states[0].no, -1);
        assert!(c.states[0].controllers.len() > 20);
    }
}
