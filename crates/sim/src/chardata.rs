//! Immutable per-character assets: constants, states, commands, animations.
//! Loaded once, shared by every instance via `Arc`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use rugen_cns::cmd::Cmd;
use rugen_cns::state::{Controller, StateDef};
use rugen_formats::air::Air;
use rugen_formats::def::Def;
use rugen_formats::sff::Sff;
use rugen_formats::text::decode;

use crate::sctrl::Sctrl;
use crate::{Error, Result};

/// One compiled state controller with its trigger logic.
#[derive(Clone, Debug)]
pub struct Ctrl {
    pub kind: String,
    pub label: String,
    pub line: usize,
    pub triggerall: Vec<rugen_cns::Code>,
    pub triggers: Vec<Vec<rugen_cns::Code>>,
    /// 1 = every time, n = every nth time, `i32::MAX` = once per state entry.
    pub persistent: i32,
    pub ignorehitpause: bool,
    pub op: Sctrl,
}

#[derive(Clone, Debug)]
pub struct State {
    pub def: StateDef,
    pub ctrls: Vec<Ctrl>,
    /// Which file defined it, for messages.
    pub file: String,
}

#[derive(Debug, Clone)]
pub struct CharData {
    pub name: String,
    pub display_name: String,
    pub author: String,
    pub dir: PathBuf,
    pub localcoord: [f32; 2],
    pub mugen_version: f32,
    /// Constants keyed by `Const()` names: `data.life`, `size.xscale`, `velocity.walk.fwd.x` …
    pub consts: IndexMap<String, f32>,
    pub states: BTreeMap<i32, State>,
    pub cmd: Cmd,
    pub sff: Sff,
    pub air: Air,
    pub snd: Option<rugen_formats::snd::Snd>,
    pub palette_files: Vec<PathBuf>,
    /// Non-fatal load problems.
    pub warnings: Vec<String>,
}

impl CharData {
    pub fn load(def_path: &Path) -> Result<Arc<Self>> {
        Self::load_with_data(def_path, Path::new(""))
    }

    /// `data_dir` holds the engine's `common1.cns`; the char's own directory is searched first.
    pub fn load_with_data(def_path: &Path, data_dir: &Path) -> Result<Arc<Self>> {
        let def = Def::open(def_path)?;
        let dir = def_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let info = def
            .section("info")
            .ok_or_else(|| Error::Load("no [Info] section".into()))?;
        let files = def
            .section("files")
            .ok_or_else(|| Error::Load("no [Files] section".into()))?;
        let mut warnings = Vec::new();

        let localcoord = info
            .f32s("localcoord")
            .filter(|v| v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let mugen_version = info.f32("mugenversion").unwrap_or(0.0);

        let find = |name: &str| resolve(&dir, name);
        let sff_name = files
            .raw("sprite")
            .ok_or_else(|| Error::Load("no sprite= in [Files]".into()))?;
        let sff = Sff::open(find(sff_name)?, true)?;
        let air_name = files
            .raw("anim")
            .ok_or_else(|| Error::Load("no anim= in [Files]".into()))?;
        let air = Air::open(find(air_name)?)?;
        let snd = match files.raw("sound").filter(|s| !s.is_empty()) {
            Some(n) => {
                match find(n).and_then(|p| rugen_formats::snd::Snd::open(p).map_err(Into::into)) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        warnings.push(format!("sound {n}: {e}"));
                        None
                    }
                }
            }
            None => None,
        };
        let mut palette_files = Vec::new();
        for i in 1..=12 {
            if let Some(p) = files.raw(&format!("pal{i}")).filter(|s| !s.is_empty()) {
                match find(p) {
                    Ok(path) => palette_files.push(path),
                    Err(e) => warnings.push(format!("pal{i}: {e}")),
                }
            }
        }

        // constants come from the cns file (and st if cns is absent)
        let cns_name = files
            .raw("cns")
            .or_else(|| files.raw("st"))
            .ok_or_else(|| Error::Load("no cns= in [Files]".into()))?;
        let cns_text = decode(&std::fs::read(find(cns_name)?)?);
        let consts = load_consts(&Def::parse(&cns_text), localcoord[0]);

        // commands
        let cmd_name = files
            .raw("cmd")
            .ok_or_else(|| Error::Load("no cmd= in [Files]".into()))?;
        let cmd_text = decode(&std::fs::read(find(cmd_name)?)?);
        let cmd = rugen_cns::cmd::parse(cmd_name, &cmd_text);
        warnings.extend(cmd.errors.iter().cloned());

        // states: cmd, cns, st, st1..st9, then stcommon; first definition wins
        let mut states: BTreeMap<i32, State> = BTreeMap::new();
        let mut add = |file: &str, parsed: rugen_cns::state::Parsed, warnings: &mut Vec<String>| {
            warnings.extend(parsed.errors);
            for s in parsed.states {
                if states.contains_key(&s.no) {
                    continue;
                }
                let ctrls = s
                    .controllers
                    .iter()
                    .map(|c| compile_ctrl(c, warnings, file))
                    .collect();
                states.insert(
                    s.no,
                    State {
                        ctrls,
                        def: s,
                        file: file.to_string(),
                    },
                );
            }
        };
        add(
            cmd_name,
            rugen_cns::state::parse_states(cmd_name, &cmd_text),
            &mut warnings,
        );
        let mut state_files: Vec<String> = Vec::new();
        for key in ["cns", "st"] {
            if let Some(n) = files.raw(key).filter(|s| !s.is_empty()) {
                state_files.push(n.to_string());
            }
        }
        for i in 0..=9 {
            if let Some(n) = files.raw(&format!("st{i}")).filter(|s| !s.is_empty()) {
                state_files.push(n.to_string());
            }
        }
        state_files.dedup();
        for n in &state_files {
            let text = if n == cns_name {
                cns_text.clone()
            } else {
                decode(&std::fs::read(find(n)?)?)
            };
            add(n, rugen_cns::state::parse_states(n, &text), &mut warnings);
        }
        let common = files
            .raw("stcommon")
            .filter(|s| !s.is_empty())
            .unwrap_or("common1.cns");
        let common_path = find(common)
            .or_else(|_| resolve(data_dir, common))
            .or_else(|_| find_common(&dir, common))?;
        let common_text = decode(&std::fs::read(&common_path)?);
        add(
            common,
            rugen_cns::state::parse_states(common, &common_text),
            &mut warnings,
        );

        Ok(Arc::new(Self {
            name: info.str("name").unwrap_or("").to_string(),
            display_name: info
                .str("displayname")
                .or(info.str("name"))
                .unwrap_or("")
                .to_string(),
            author: info.str("author").unwrap_or("").to_string(),
            dir,
            localcoord,
            mugen_version,
            consts,
            states,
            cmd,
            sff,
            air,
            snd,
            palette_files,
            warnings,
        }))
    }

    pub fn konst(&self, name: &str) -> Option<f32> {
        self.consts.get(name).copied()
    }

    /// Copy with extra states parsed from `text` (overriding existing numbers). For tests and tools.
    pub fn with_states(&self, file: &str, text: &str) -> Result<Arc<Self>> {
        let mut d = self.clone();
        let parsed = rugen_cns::state::parse_states(file, text);
        if !parsed.errors.is_empty() {
            return Err(Error::Load(parsed.errors.join("\n")));
        }
        for st in parsed.states {
            let mut warnings = Vec::new();
            let ctrls = st
                .controllers
                .iter()
                .map(|c| compile_ctrl(c, &mut warnings, file))
                .collect();
            if !warnings.is_empty() {
                return Err(Error::Load(warnings.join("\n")));
            }
            d.states.insert(
                st.no,
                State {
                    ctrls,
                    def: st,
                    file: file.to_string(),
                },
            );
        }
        Ok(Arc::new(d))
    }
}

fn compile_ctrl(c: &Controller, warnings: &mut Vec<String>, file: &str) -> Ctrl {
    let op = match Sctrl::compile(c) {
        Ok(op) => op,
        Err(e) => {
            warnings.push(format!("{file}:{}: [{}] {e}", c.line, c.kind));
            Sctrl::Null
        }
    };
    Ctrl {
        kind: c.kind.clone(),
        label: c.label.clone(),
        line: c.line,
        triggerall: c.triggerall.clone(),
        triggers: c.triggers.clone(),
        persistent: if c.persistent <= 0 {
            i32::MAX
        } else if c.persistent > 128 {
            1
        } else {
            c.persistent
        },
        ignorehitpause: c.ignorehitpause,
        op,
    }
}

/// Shared fight effects: `data/fightfx.sff` + `.air` and `data/common.snd`.
#[derive(Debug)]
pub struct FxData {
    pub sff: Sff,
    pub air: Air,
    pub snd: Option<rugen_formats::snd::Snd>,
    pub localcoord: [f32; 2],
}

impl FxData {
    pub fn load(data_dir: &Path) -> Result<Arc<Self>> {
        let sff = Sff::open(resolve(data_dir, "fightfx.sff")?, false)?;
        let air = Air::open(resolve(data_dir, "fightfx.air")?)?;
        let snd = resolve(data_dir, "common.snd")
            .ok()
            .and_then(|p| rugen_formats::snd::Snd::open(p).ok());
        // fightfx.air carries no localcoord; MUGEN 1.1's fight.def is 320x240
        Ok(Arc::new(Self {
            sff,
            air,
            snd,
            localcoord: [320.0, 240.0],
        }))
    }
}

/// Case-insensitive path resolution with backslash normalisation.
pub fn resolve(dir: &Path, name: &str) -> Result<PathBuf> {
    let name = name.replace('\\', "/");
    let direct = dir.join(&name);
    if direct.exists() {
        return Ok(direct);
    }
    let mut cur = dir.to_path_buf();
    for part in name.split('/').filter(|p| !p.is_empty()) {
        let found = std::fs::read_dir(&cur)
            .ok()
            .and_then(|rd| {
                rd.flatten()
                    .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))
            })
            .map(|e| e.path())
            .ok_or_else(|| Error::Load(format!("{part} not found in {}", cur.display())))?;
        cur = found;
    }
    Ok(cur)
}

/// `data/common1.cns` relative to the char dir's ancestors (`chars/kfm/../../data`).
fn find_common(dir: &Path, name: &str) -> Result<PathBuf> {
    for up in dir.ancestors().skip(1).take(4) {
        if let Ok(p) = resolve(&up.join("data"), name) {
            return Ok(p);
        }
    }
    Err(Error::Load(format!(
        "{name} not found in any data/ directory above {}",
        dir.display()
    )))
}

/// Defaults are in 320x240 space; sizes, velocities and movement scale with localcoord.
const DEFAULTS: &[(&str, f32, bool)] = &[
    ("data.life", 1000.0, false),
    ("data.power", 3000.0, false),
    ("data.attack", 100.0, false),
    ("data.defence", 100.0, false),
    ("data.fall.defence_up", 50.0, false),
    ("data.liedown.time", 60.0, false),
    ("data.airjuggle", 15.0, false),
    ("data.sparkno", 2.0, false),
    ("data.guard.sparkno", 40.0, false),
    ("data.ko.echo", 0.0, false),
    ("data.volume", 0.0, false),
    ("data.intpersistindex", 60.0, false),
    ("data.floatpersistindex", 40.0, false),
    ("size.xscale", 1.0, false),
    ("size.yscale", 1.0, false),
    ("size.ground.back", 15.0, true),
    ("size.ground.front", 16.0, true),
    ("size.air.back", 12.0, true),
    ("size.air.front", 12.0, true),
    ("size.height", 60.0, true),
    ("size.attack.dist", 160.0, true),
    ("size.proj.attack.dist", 90.0, true),
    ("size.proj.doscale", 0.0, false),
    ("size.head.pos.x", 0.0, true),
    ("size.head.pos.y", 0.0, true),
    ("size.mid.pos.x", 0.0, true),
    ("size.mid.pos.y", 0.0, true),
    ("size.shadowoffset", 0.0, true),
    ("size.draw.offset.x", 0.0, true),
    ("size.draw.offset.y", 0.0, true),
    ("velocity.walk.fwd.x", 2.4, true),
    ("velocity.walk.back.x", -2.2, true),
    ("velocity.run.fwd.x", 4.6, true),
    ("velocity.run.fwd.y", 0.0, true),
    ("velocity.run.back.x", -4.5, true),
    ("velocity.run.back.y", -3.8, true),
    ("velocity.jump.neu.x", 0.0, true),
    ("velocity.jump.y", -8.4, true),
    ("velocity.jump.back.x", -2.55, true),
    ("velocity.jump.fwd.x", 2.5, true),
    ("velocity.runjump.back.x", -2.55, true),
    ("velocity.runjump.back.y", -8.1, true),
    ("velocity.runjump.fwd.x", 4.0, true),
    ("velocity.runjump.fwd.y", -8.1, true),
    ("velocity.airjump.neu.x", 0.0, true),
    ("velocity.airjump.y", -8.1, true),
    ("velocity.airjump.back.x", -2.55, true),
    ("velocity.airjump.fwd.x", 2.5, true),
    ("velocity.air.gethit.groundrecover.x", -0.15, true),
    ("velocity.air.gethit.groundrecover.y", -3.5, true),
    ("velocity.air.gethit.airrecover.mul.x", 0.5, false),
    ("velocity.air.gethit.airrecover.mul.y", 0.2, false),
    ("velocity.air.gethit.airrecover.add.x", 0.0, true),
    ("velocity.air.gethit.airrecover.add.y", -4.5, true),
    ("velocity.air.gethit.airrecover.back", -1.0, true),
    ("velocity.air.gethit.airrecover.fwd", 0.0, true),
    ("velocity.air.gethit.airrecover.up", -2.0, true),
    ("velocity.air.gethit.airrecover.down", 1.5, true),
    ("movement.airjump.num", 1.0, false),
    ("movement.airjump.height", 35.0, true),
    ("movement.yaccel", 0.44, true),
    ("movement.stand.friction", 0.85, false),
    ("movement.crouch.friction", 0.82, false),
    ("movement.stand.friction.threshold", 2.0, true),
    ("movement.crouch.friction.threshold", 0.05, true),
    ("movement.air.gethit.groundlevel", 25.0, true),
    (
        "movement.air.gethit.groundrecover.ground.threshold",
        -20.0,
        true,
    ),
    ("movement.air.gethit.groundrecover.groundlevel", 10.0, true),
    ("movement.air.gethit.airrecover.threshold", -1.0, true),
    ("movement.air.gethit.airrecover.yaccel", 0.35, true),
    ("movement.air.gethit.trip.groundlevel", 15.0, true),
    ("movement.down.bounce.offset.x", 0.0, true),
    ("movement.down.bounce.offset.y", 20.0, true),
    ("movement.down.bounce.yaccel", 0.4, true),
    ("movement.down.bounce.groundlevel", 12.0, true),
    ("movement.down.friction.threshold", 0.05, true),
];

/// Read `[Data] [Size] [Velocity] [Movement]` into `Const()`-style keys.
pub fn load_consts(def: &Def, localcoord_w: f32) -> IndexMap<String, f32> {
    let ratio = localcoord_w / 320.0;
    let mut m: IndexMap<String, f32> = DEFAULTS
        .iter()
        .map(|&(k, v, scaled)| (k.to_string(), if scaled { v * ratio } else { v }))
        .collect();
    for sec in ["data", "size", "velocity", "movement"] {
        let Some(s) = def.section(sec) else { continue };
        for (key, raw) in &s.entries {
            let vals: Vec<f32> = raw
                .split(',')
                .map(|v| rugen_formats::text::atof(v).unwrap_or(0.0))
                .collect();
            if vals.is_empty() {
                continue;
            }
            let base = format!("{sec}.{key}");
            // keys that are documented as x/y pairs get .x/.y; scalars keep the bare name,
            // but velocity scalars are also exposed as .x (velocity.walk.fwd.x)
            let pair = matches!(
                key.as_str(),
                "head.pos"
                    | "mid.pos"
                    | "draw.offset"
                    | "run.fwd"
                    | "run.back"
                    | "jump.neu"
                    | "runjump.back"
                    | "runjump.fwd"
                    | "airjump.neu"
                    | "air.gethit.groundrecover"
                    | "air.gethit.airrecover.mul"
                    | "air.gethit.airrecover.add"
                    | "down.bounce.offset"
            );
            if pair {
                m.insert(format!("{base}.x"), vals[0]);
                m.insert(format!("{base}.y"), vals.get(1).copied().unwrap_or(0.0));
                if key == "jump.neu" {
                    m.insert(
                        "velocity.jump.y".into(),
                        vals.get(1).copied().unwrap_or(0.0),
                    );
                }
                if key == "airjump.neu" {
                    m.insert(
                        "velocity.airjump.y".into(),
                        vals.get(1).copied().unwrap_or(0.0),
                    );
                }
            } else if sec == "velocity" && !key.contains("airrecover.") {
                m.insert(format!("{base}.x"), vals[0]);
            } else {
                m.insert(base, vals[0]);
            }
        }
    }
    let f = m.get("data.fall.defence_up").copied().unwrap_or(50.0);
    m.insert("data.fall.defence_mul".into(), 100.0 / (f + 100.0));
    m
}
