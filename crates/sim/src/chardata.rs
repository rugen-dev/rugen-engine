//! Immutable per-character assets: constants, states, commands, animations.
//! Loaded once, shared by every instance via `Arc`.
//!
//! A character loads the same from its folder or from inside an `.rgc`
//! content file (`chars/x.rgc/x.def`, see `rugen_formats::vfs`). An `.rgc`
//! carries its AIR / CMD / CNS compiled by the engine that converted it
//! ([`compile_logic`]); when that engine was another build, the text is
//! compiled again and the result kept in `cache/logic/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rugen_cns::cmd::Cmd;
use rugen_cns::state::{Controller, StateDef};
use rugen_formats::air::Air;
use rugen_formats::def::Def;
use rugen_formats::sff::Sff;
use rugen_formats::text::decode;
use rugen_formats::vfs;

use crate::sctrl::Sctrl;
use crate::{Error, Result};

/// One compiled state controller with its trigger logic.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct State {
    /// The `[Statedef]` header. Its `controllers` are compiled into
    /// [`ctrls`](Self::ctrls) and left empty: the raw parameter text and a
    /// second copy of every trigger would otherwise be held for nothing
    /// (the debugger reads controller text from the files, by line).
    pub def: StateDef,
    pub ctrls: Vec<Ctrl>,
    /// Which file defined it, for messages.
    pub file: String,
}

/// Compile a parsed state: its controllers become [`Ctrl`]s and the raw
/// ones are dropped.
fn compile_state(mut s: StateDef, file: &str, warnings: &mut Vec<String>) -> State {
    let ctrls = s
        .controllers
        .iter()
        .map(|c| compile_ctrl(c, warnings, file))
        .collect();
    s.controllers = Vec::new();
    State {
        ctrls,
        def: s,
        file: file.to_string(),
    }
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
    pub consts: rugen_cns::fnv::FnvIndexMap<String, f32>,
    pub states: BTreeMap<i32, State>,
    pub cmd: Cmd,
    pub sff: Sff,
    pub air: Air,
    pub snd: Option<rugen_formats::snd::Snd>,
    pub palette_files: Vec<PathBuf>,
    /// Non-fatal load problems.
    pub warnings: Vec<String>,
}

/// How [`CharData::load_opts`] reads a character.
#[derive(Clone, Copy, Debug)]
pub struct LoadOptions {
    /// Use compiled logic (from the `.rgc`, else `cache/logic/`) when it was
    /// built by this engine; `false` always compiles the text.
    pub precompiled: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { precompiled: true }
    }
}

/// Files a character's `[Files]` section resolves to.
struct Sources {
    dir: PathBuf,
    name: String,
    display_name: String,
    author: String,
    localcoord: [f32; 2],
    mugen_version: f32,
    sff: PathBuf,
    air: PathBuf,
    /// `(name, resolved)`, `None` when no sound is set.
    snd: Option<(String, Result<PathBuf>)>,
    palette_files: Vec<PathBuf>,
    pal_warnings: Vec<String>,
    cns: (String, PathBuf),
    cmd: (String, PathBuf),
    /// cns, st, st0..st9 in load order (deduplicated like the loader always did).
    state_files: Vec<String>,
    common: (String, PathBuf),
}

impl Sources {
    fn resolve(def_path: &Path, data_dir: &Path) -> Result<Self> {
        let def = Def::open(def_path)?;
        let dir = def_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let info = def
            .section("info")
            .ok_or_else(|| Error::Load("no [Info] section".into()))?;
        let files = def
            .section("files")
            .ok_or_else(|| Error::Load("no [Files] section".into()))?;
        let localcoord = info
            .f32s("localcoord")
            .filter(|v| v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let find = |name: &str| resolve(&dir, name);
        let sff_name = files
            .raw("sprite")
            .ok_or_else(|| Error::Load("no sprite= in [Files]".into()))?;
        let sff = find(sff_name)?;
        let air_name = files
            .raw("anim")
            .ok_or_else(|| Error::Load("no anim= in [Files]".into()))?;
        let air = find(air_name)?;
        let snd = files
            .raw("sound")
            .filter(|s| !s.is_empty())
            .map(|n| (n.to_string(), find(n)));
        let mut palette_files = Vec::new();
        let mut pal_warnings = Vec::new();
        for i in 1..=12 {
            if let Some(p) = files.raw(&format!("pal{i}")).filter(|s| !s.is_empty()) {
                match find(p) {
                    Ok(path) => palette_files.push(path),
                    Err(e) => pal_warnings.push(format!("pal{i}: {e}")),
                }
            }
        }
        // constants come from the cns file (and st if cns is absent)
        let cns_name = files
            .raw("cns")
            .or_else(|| files.raw("st"))
            .ok_or_else(|| Error::Load("no cns= in [Files]".into()))?;
        let cns = (cns_name.to_string(), find(cns_name)?);
        let cmd_name = files
            .raw("cmd")
            .ok_or_else(|| Error::Load("no cmd= in [Files]".into()))?;
        let cmd = (cmd_name.to_string(), find(cmd_name)?);
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
        let common_name = files
            .raw("stcommon")
            .filter(|s| !s.is_empty())
            .unwrap_or("common1.cns");
        let common_path = find(common_name)
            .or_else(|_| resolve(data_dir, common_name))
            .or_else(|_| find_common(&dir, common_name))?;
        Ok(Self {
            name: info.str("name").unwrap_or("").to_string(),
            display_name: info
                .str("displayname")
                .or(info.str("name"))
                .unwrap_or("")
                .to_string(),
            author: info.str("author").unwrap_or("").to_string(),
            localcoord,
            mugen_version: info.f32("mugenversion").unwrap_or(0.0),
            sff,
            air,
            snd,
            palette_files,
            pal_warnings,
            cns,
            cmd,
            state_files,
            common: (common_name.to_string(), common_path),
            dir,
        })
    }

    /// Every file the compiled logic is made from, in load order.
    fn logic_files(&self) -> Result<Vec<PathBuf>> {
        let mut out = vec![self.air.clone(), self.cmd.1.clone()];
        for n in &self.state_files {
            out.push(resolve(&self.dir, n)?);
        }
        out.push(self.common.1.clone());
        Ok(out)
    }
}

/// Everything parsed from the AIR / CMD / CNS files.
#[derive(serde::Serialize, serde::Deserialize)]
struct Logic {
    consts: rugen_cns::fnv::FnvIndexMap<String, f32>,
    states: BTreeMap<i32, State>,
    cmd: Cmd,
    air: Air,
    warnings: Vec<String>,
}

impl Logic {
    fn parse(src: &Sources) -> Result<Self> {
        let air = Air::open(&src.air)?;
        let mut warnings = Vec::new();
        let (cns_name, cns_path) = &src.cns;
        let cns_text = decode(&vfs::read(cns_path)?);
        let consts = load_consts(&Def::parse(&cns_text), src.localcoord[0]);

        let (cmd_name, cmd_path) = &src.cmd;
        let cmd_text = decode(&vfs::read(cmd_path)?);
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
                states.insert(s.no, compile_state(s, file, warnings));
            }
        };
        add(
            cmd_name,
            rugen_cns::state::parse_states(cmd_name, &cmd_text),
            &mut warnings,
        );
        for n in &src.state_files {
            let text = if n == cns_name {
                cns_text.clone()
            } else {
                decode(&vfs::read(&resolve(&src.dir, n)?)?)
            };
            add(n, rugen_cns::state::parse_states(n, &text), &mut warnings);
        }
        let (common, common_path) = &src.common;
        let common_text = decode(&vfs::read(common_path)?);
        add(
            common,
            rugen_cns::state::parse_states(common, &common_text),
            &mut warnings,
        );
        Ok(Self {
            consts,
            states,
            cmd,
            air,
            warnings,
        })
    }

    fn to_bytes(&self) -> Result<Vec<u8>> {
        bincode::serialize(self).map_err(|e| Error::Load(e.to_string()))
    }

    fn from_bytes(b: &[u8]) -> Option<Self> {
        bincode::deserialize(b).ok()
    }

    /// The compiled logic of a character inside a content file: its own
    /// when this engine built it, else the `cache/logic/` copy, else
    /// compiled now (and cached). `None` outside content files.
    fn precompiled(def_path: &Path, src: &Sources) -> Option<Self> {
        if let Some((schema, payload)) = vfs::logic(def_path)
            && schema == LOGIC_SCHEMA
            && let Some(l) = Self::from_bytes(&payload)
        {
            return Some(l);
        }
        let cache = logic_cache_path(def_path, src)?;
        if let Ok(b) = std::fs::read(&cache)
            && let Some(l) = Self::from_bytes(&b)
        {
            return Some(l);
        }
        let l = Self::parse(src).ok()?;
        if let Ok(b) = l.to_bytes() {
            let _ = write_atomic(&cache, &b);
        }
        Some(l)
    }
}

/// `cache/logic/<key>.bin` for a character inside a content file: the key
/// hashes the engine schema and the bytes of every logic source, so a
/// stale or foreign entry is never picked up.
fn logic_cache_path(def_path: &Path, src: &Sources) -> Option<PathBuf> {
    vfs::split(def_path)?;
    let dir = vfs::cache_dir()?;
    let mut key = Vec::new();
    key.extend_from_slice(LOGIC_SCHEMA.as_bytes());
    for p in std::iter::once(def_path.to_path_buf()).chain(src.logic_files().ok()?) {
        key.extend_from_slice(&rugen_formats::rgc::sha256(&vfs::read(&p).ok()?));
    }
    let h = rugen_formats::rgc::sha256(&key);
    let hex: String = h[..16].iter().map(|b| format!("{b:02x}")).collect();
    Some(dir.join("logic").join(format!("{hex}.bin")))
}

/// Engine version + hash of the sim / cns / formats sources (see `build.rs`):
/// compiled logic made by any other build is compiled again.
pub const LOGIC_SCHEMA: &str = concat!(env!("CARGO_PKG_VERSION"), "-", env!("RUGEN_LOGIC_SCHEMA"));

/// Compile the AIR / CMD / CNS of the character `def_path` into the bytes
/// an `.rgc` stores next to its text (valid for [`LOGIC_SCHEMA`]).
pub fn compile_logic(def_path: &Path, data_dir: &Path) -> Result<Vec<u8>> {
    let src = Sources::resolve(def_path, data_dir)?;
    Logic::parse(&src)?.to_bytes()
}

/// Write via a temporary file so a crash never leaves half a file behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl CharData {
    pub fn load(def_path: &Path) -> Result<Arc<Self>> {
        Self::load_with_data(def_path, Path::new(""))
    }

    /// `data_dir` holds the engine's `common1.cns`; the char's own directory is searched first.
    /// Uses precompiled logic when it is valid.
    pub fn load_with_data(def_path: &Path, data_dir: &Path) -> Result<Arc<Self>> {
        if let Some(d) = take_preloaded(def_path, data_dir) {
            return Ok(d);
        }
        Self::load_opts(def_path, data_dir, LoadOptions::default())
    }

    /// Load a character ahead of time (a versus screen before the fight):
    /// the next [`CharData::load_with_data`] of the same files takes it
    /// instead of reading them again. Kept for a minute, a few at a time.
    pub fn preload(def_path: &Path, data_dir: &Path) {
        if let Ok(d) = Self::load_opts(def_path, data_dir, LoadOptions::default()) {
            let mut p = PRELOADED.lock().unwrap_or_else(|e| e.into_inner());
            p.retain(|e| e.3.elapsed() < PRELOAD_KEEP && e.0 != def_path);
            p.push((
                def_path.to_path_buf(),
                data_dir.to_path_buf(),
                d,
                std::time::Instant::now(),
            ));
            while p.len() > 4 {
                p.remove(0);
            }
        }
    }

    /// Load compiling the text files, ignoring any precompiled logic
    /// (verification, equivalence tests).
    pub fn load_original(def_path: &Path, data_dir: &Path) -> Result<Arc<Self>> {
        Self::load_opts(def_path, data_dir, LoadOptions { precompiled: false })
    }

    pub fn load_opts(def_path: &Path, data_dir: &Path, opts: LoadOptions) -> Result<Arc<Self>> {
        let src = Sources::resolve(def_path, data_dir)?;
        let mut warnings = Vec::new();
        // logic and sounds on a helper thread while this one reads the sprites
        let (sff, logic, snd) = std::thread::scope(|s| {
            let rest = s.spawn(|| {
                let logic = match opts
                    .precompiled
                    .then(|| Logic::precompiled(def_path, &src))
                    .flatten()
                {
                    Some(l) => Ok(l),
                    None => Logic::parse(&src),
                };
                let snd = src.snd.as_ref().map(|(n, found)| {
                    found
                        .as_ref()
                        .map_err(|e| Error::Load(e.to_string()))
                        .and_then(|p| rugen_formats::snd::Snd::open(p).map_err(Into::into))
                        .map_err(|e| format!("sound {n}: {e}"))
                });
                (logic, snd)
            });
            let sff = Sff::open(&src.sff, true).map_err(Error::from);
            let (logic, snd) = rest
                .join()
                .unwrap_or_else(|_| (Err(Error::Load("logic loading panicked".into())), None));
            (sff, logic, snd)
        });
        let mut sff = sff?;
        let logic = logic?;
        let snd = match snd {
            Some(Ok(s)) => Some(s),
            Some(Err(e)) => {
                warnings.push(e);
                None
            }
            None => None,
        };
        warnings.extend(src.pal_warnings.iter().cloned());
        // SFFv1 sprites 0,0 / 9000,0 (and those sharing their palette) take pal1's colours
        if let Some(first) = src.palette_files.first() {
            match rugen_formats::act::open(first) {
                Ok(act) => sff.apply_v1_act(&act),
                Err(e) => warnings.push(format!("{}: {e}", first.display())),
            }
        }
        warnings.extend(logic.warnings);

        Ok(Arc::new(Self {
            name: src.name,
            display_name: src.display_name,
            author: src.author,
            dir: src.dir,
            localcoord: src.localcoord,
            mugen_version: src.mugen_version,
            consts: logic.consts,
            states: logic.states,
            cmd: logic.cmd,
            sff,
            air: logic.air,
            snd,
            palette_files: src.palette_files,
            warnings,
        }))
    }
    pub fn konst(&self, name: &str) -> Option<f32> {
        self.consts.get(name).copied()
    }

    /// Copy with extra states parsed from `text` (overriding existing numbers). For tests and tools.
    /// Like [`with_states`](Self::with_states) but keeps going on compile
    /// problems (bad controllers become `Null`), as the real loader does.
    pub fn with_states_lenient(&self, file: &str, text: &str) -> Arc<Self> {
        let mut d = self.clone();
        let parsed = rugen_cns::state::parse_states(file, text);
        d.warnings.extend(parsed.errors);
        for st in parsed.states {
            let mut warnings = Vec::new();
            let state = compile_state(st, file, &mut warnings);
            d.warnings.extend(warnings);
            d.states.insert(state.def.no, state);
        }
        Arc::new(d)
    }

    pub fn with_states(&self, file: &str, text: &str) -> Result<Arc<Self>> {
        let mut d = self.clone();
        let parsed = rugen_cns::state::parse_states(file, text);
        if !parsed.errors.is_empty() {
            return Err(Error::Load(parsed.errors.join("\n")));
        }
        for st in parsed.states {
            let mut warnings = Vec::new();
            let state = compile_state(st, file, &mut warnings);
            if !warnings.is_empty() {
                return Err(Error::Load(warnings.join("\n")));
            }
            d.states.insert(state.def.no, state);
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

/// Shared fight effects: `data/fightfx.sff` + `.air`, `common.snd`, `fight.snd`, round timings.
#[derive(Debug)]
pub struct FxData {
    pub sff: Sff,
    pub air: Air,
    /// `common.snd` (PlaySnd with the `F` prefix, hit sounds).
    pub snd: Option<rugen_formats::snd::Snd>,
    /// `fight.snd` (announcer: round, fight, KO).
    pub fight_snd: Option<rugen_formats::snd::Snd>,
    pub localcoord: [f32; 2],
    pub round: crate::round::RoundConfig,
}

/// Resolve `name` in `dirs` in order (screenpack first, then data).
pub fn resolve_in(dirs: &[&Path], name: &str) -> Result<PathBuf> {
    let mut last = Err(Error::Load(format!("{name}: no directories")));
    for d in dirs {
        match resolve(d, name) {
            Ok(p) => return Ok(p),
            Err(e) => last = Err(e),
        }
    }
    last
}

/// Lookup order for screenpack files: the motif (a theme folder), the flat
/// `motif/` it lives under (`motif/themes/<x>` → `motif`), then `data_dir`.
pub fn motif_chain(motif: &Path, data_dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![motif.to_path_buf()];
    if let Some(parent) = motif.parent()
        && parent.file_name().is_some_and(|n| n == "themes")
        && let Some(flat) = parent.parent()
        && flat != data_dir
    {
        out.push(flat.to_path_buf());
    }
    if data_dir != motif {
        out.push(data_dir.to_path_buf());
    }
    out
}

impl FxData {
    pub fn load(data_dir: &Path) -> Result<Arc<Self>> {
        Self::load_with_motif(data_dir, data_dir)
    }

    /// `fight.def` (round timings, file names) comes from the motif when it
    /// has one; referenced files resolve in the motif first, then `data_dir`.
    pub fn load_with_motif(motif: &Path, data_dir: &Path) -> Result<Arc<Self>> {
        let chain = motif_chain(motif, data_dir);
        let dirs: Vec<&Path> = chain.iter().map(PathBuf::as_path).collect();
        let resolve = |name: &str| resolve_in(&dirs, name);
        let fight = resolve("fight.def").ok().and_then(|p| Def::open(p).ok());
        let files = fight.as_ref().and_then(|d| d.section("files").cloned());
        let name = |key: &str, default: &str| {
            files
                .as_ref()
                .and_then(|f| f.raw(key).map(str::to_string))
                .unwrap_or_else(|| default.to_string())
        };
        let sff = Sff::open(resolve(&name("fightfx.sff", "fightfx.sff"))?, false)?;
        let air = Air::open(resolve(&name("fightfx.air", "fightfx.air"))?)?;
        let snd = resolve(&name("common.snd", "common.snd"))
            .ok()
            .and_then(|p| rugen_formats::snd::Snd::open(p).ok());
        let fight_snd = resolve(&name("snd", "fight.snd"))
            .ok()
            .and_then(|p| rugen_formats::snd::Snd::open(p).ok());
        let mut round = fight
            .as_ref()
            .map(crate::round::RoundConfig::from_fight_def)
            .unwrap_or_default();
        if let Ok(cfg) = resolve("mugen.cfg").and_then(|p| Def::open(p).map_err(Into::into))
            && let Some(t) = cfg.section("options").and_then(|o| o.i32("time"))
        {
            round.round_time = t;
        }
        // fightfx.air carries no localcoord; MUGEN 1.1's fight.def is 320x240
        Ok(Arc::new(Self {
            sff,
            air,
            snd,
            fight_snd,
            localcoord: [320.0, 240.0],
            round,
        }))
    }
}

/// Case-insensitive path resolution with backslash normalisation.
pub fn resolve(dir: &Path, name: &str) -> Result<PathBuf> {
    vfs::resolve(dir, name)
        .ok_or_else(|| Error::Load(format!("{} not found in {}", name.trim(), dir.display())))
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
pub fn load_consts(def: &Def, localcoord_w: f32) -> rugen_cns::fnv::FnvIndexMap<String, f32> {
    let ratio = localcoord_w / 320.0;
    let mut m: rugen_cns::fnv::FnvIndexMap<String, f32> = DEFAULTS
        .iter()
        .map(|&(k, v, scaled)| (k.to_string(), if scaled { v * ratio } else { v }))
        .collect();
    for sec in ["data", "size", "velocity", "movement"] {
        let Some(s) = def.section(sec) else { continue };
        // Ikemen: run jumps and air jumps default to the regular jump's
        // velocities, read before their own keys
        let late = |k: &str| k.starts_with("runjump.") || k.starts_with("airjump.");
        for pass in 0..2 {
            if sec == "velocity" && pass == 1 {
                let g = |m: &rugen_cns::fnv::FnvIndexMap<String, f32>, k: &str| {
                    m.get(k).copied().unwrap_or(0.0)
                };
                let (nx, ny) = (g(&m, "velocity.jump.neu.x"), g(&m, "velocity.jump.y"));
                let (bx, fx) = (g(&m, "velocity.jump.back.x"), g(&m, "velocity.jump.fwd.x"));
                for (k, v) in [
                    ("velocity.runjump.back.x", bx),
                    ("velocity.runjump.back.y", ny),
                    ("velocity.runjump.fwd.x", fx),
                    ("velocity.runjump.fwd.y", ny),
                    ("velocity.airjump.neu.x", nx),
                    ("velocity.airjump.y", ny),
                    ("velocity.airjump.back.x", bx),
                    ("velocity.airjump.fwd.x", fx),
                ] {
                    m.insert(k.into(), v);
                }
            }
            for (key, raw) in &s.entries {
                if (sec == "velocity" && late(key)) != (pass == 1) {
                    continue;
                }
                // a missing or empty component keeps its default (Ikemen `ReadF32`)
                let vals: Vec<Option<f32>> =
                    raw.split(',').map(rugen_formats::text::atof).collect();
                if vals.is_empty() {
                    continue;
                }
                let mut put = |k: String, v: Option<f32>| {
                    if let Some(v) = v {
                        m.insert(k, v);
                    } else {
                        m.entry(k).or_insert(0.0);
                    }
                };
                let x = vals[0];
                let y = vals.get(1).copied().flatten();
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
                    put(format!("{base}.x"), x);
                    put(format!("{base}.y"), y);
                    if key == "jump.neu" {
                        put("velocity.jump.y".into(), y);
                    }
                    if key == "airjump.neu" {
                        put("velocity.airjump.y".into(), y);
                    }
                } else if sec == "velocity" && !key.contains("airrecover.") {
                    put(format!("{base}.x"), x);
                } else {
                    put(base, x);
                }
            }
        }
    }
    let f = m.get("data.fall.defence_up").copied().unwrap_or(50.0);
    m.insert("data.fall.defence_mul".into(), 100.0 / (f + 100.0));
    m
}

/// Characters loaded ahead of a fight ([`CharData::preload`]): def, data
/// dir, data, when.
type Preloaded = (
    std::path::PathBuf,
    std::path::PathBuf,
    Arc<CharData>,
    std::time::Instant,
);
static PRELOADED: std::sync::Mutex<Vec<Preloaded>> = std::sync::Mutex::new(Vec::new());
const PRELOAD_KEEP: std::time::Duration = std::time::Duration::from_secs(60);

/// Take a fresh preloaded copy of `def_path` (identical to loading it now).
fn take_preloaded(def_path: &Path, data_dir: &Path) -> Option<Arc<CharData>> {
    let mut p = PRELOADED.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|e| e.3.elapsed() < PRELOAD_KEEP);
    let i = p.iter().position(|e| e.0 == def_path && e.1 == data_dir)?;
    Some(p.remove(i).2)
}

#[cfg(test)]
mod const_tests {
    use super::*;

    /// Ikemen `ReadF32`: `airjump.neu = 0` sets x only; y keeps its default,
    /// which is the regular jump's (Goku's air jumps went nowhere before).
    #[test]
    fn one_value_pairs_keep_their_default_y() {
        let def = Def::parse(
            "[Velocity]\njump.neu = 0,-9\njump.fwd = 4\nairjump.neu = 0\nairjump.fwd = 0\n",
        );
        let m = load_consts(&def, 320.0);
        assert_eq!(m["velocity.jump.y"], -9.0);
        assert_eq!(m["velocity.airjump.y"], -9.0, "y defaults to jump.neu y");
        assert_eq!(m["velocity.airjump.fwd.x"], 0.0);
        assert_eq!(m["velocity.runjump.fwd.x"], 4.0, "runjump defaults to jump");
        assert_eq!(m["velocity.runjump.fwd.y"], -9.0);
        // both values given: used as is
        let def = Def::parse("[Velocity]\nairjump.neu = 1,-6\n");
        let m = load_consts(&def, 320.0);
        assert_eq!(m["velocity.airjump.neu.x"], 1.0);
        assert_eq!(m["velocity.airjump.y"], -6.0);
    }
}
