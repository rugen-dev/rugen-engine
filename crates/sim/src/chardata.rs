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
use rugen_formats::pack::{self, Stamp};
use rugen_formats::sff::Sff;
use rugen_formats::text::decode;

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

/// How [`CharData::load_opts`] reads a character.
#[derive(Clone, Copy, Debug)]
pub struct LoadOptions {
    /// Use the optimised artefacts in [`opt_dir`] when they are present and
    /// match the source files (they fall back to the originals otherwise).
    pub optimised: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { optimised: true }
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
    def_path: PathBuf,
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
            def_path: def_path.to_path_buf(),
            dir,
        })
    }

    /// Every file the compiled logic depends on, with its stamp; `None` if one is missing.
    fn logic_stamps(&self) -> Option<Vec<(String, u64, u64)>> {
        let mut paths = vec![self.def_path.clone(), self.air.clone(), self.cmd.1.clone()];
        for n in &self.state_files {
            paths.push(resolve(&self.dir, n).ok()?);
        }
        paths.push(self.common.1.clone());
        paths
            .into_iter()
            .map(|p| {
                let s = Stamp::of(&p)?;
                Some((p.to_string_lossy().to_string(), s.len, s.mtime_ns))
            })
            .collect()
    }
}

/// Everything parsed from the AIR / CMD / CNS files.
#[derive(serde::Serialize, serde::Deserialize)]
struct Logic {
    consts: IndexMap<String, f32>,
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
        let cns_text = decode(&std::fs::read(cns_path)?);
        let consts = load_consts(&Def::parse(&cns_text), src.localcoord[0]);

        let (cmd_name, cmd_path) = &src.cmd;
        let cmd_text = decode(&std::fs::read(cmd_path)?);
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
        for n in &src.state_files {
            let text = if n == cns_name {
                cns_text.clone()
            } else {
                decode(&std::fs::read(resolve(&src.dir, n)?)?)
            };
            add(n, rugen_cns::state::parse_states(n, &text), &mut warnings);
        }
        let (common, common_path) = &src.common;
        let common_text = decode(&std::fs::read(common_path)?);
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

    /// The cached logic in `path` if it was built by this engine from these sources.
    fn read_cached(path: &Path, src: &Sources) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let mut r: &[u8] = &bytes;
        let header: LogicHeader = bincode::deserialize_from(&mut r).ok()?;
        if !header.matches(src) {
            return None;
        }
        bincode::deserialize_from(&mut r).ok()
    }

    fn write_cached(&self, path: &Path, src: &Sources) -> Result<()> {
        let header = LogicHeader {
            magic: *LOGIC_MAGIC,
            schema: LOGIC_SCHEMA.to_string(),
            sources: src
                .logic_stamps()
                .ok_or_else(|| Error::Load("a logic source file vanished".into()))?,
        };
        let mut out = bincode::serialize(&header).map_err(|e| Error::Load(e.to_string()))?;
        out.extend(bincode::serialize(self).map_err(|e| Error::Load(e.to_string()))?);
        write_atomic(path, &out)
    }
}

const LOGIC_MAGIC: &[u8; 8] = b"RUGENLGC";
/// Engine version + hash of the sim / cns / formats sources (see `build.rs`):
/// any engine change invalidates every cached logic file.
pub const LOGIC_SCHEMA: &str = concat!(env!("CARGO_PKG_VERSION"), "-", env!("RUGEN_LOGIC_SCHEMA"));

#[derive(serde::Serialize, serde::Deserialize)]
struct LogicHeader {
    magic: [u8; 8],
    schema: String,
    /// `(path, len, mtime ns)` of every source file.
    sources: Vec<(String, u64, u64)>,
}

impl LogicHeader {
    fn matches(&self, src: &Sources) -> bool {
        &self.magic == LOGIC_MAGIC
            && self.schema == LOGIC_SCHEMA
            && src.logic_stamps().is_some_and(|s| s == self.sources)
    }
}

/// Optimised sprite pack file name inside [`opt_dir`].
pub const SPRITES_FILE: &str = "sprites.rspk";
/// Precompiled AIR / CMD / CNS file name inside [`opt_dir`].
pub const LOGIC_FILE: &str = "logic.bin";

/// Where the optimised artefacts of the character (or stage) `def_path`
/// live: `<def dir>/.rugen-opt/<def stem>/`. A local derived cache: never
/// part of the fingerprint, packages or verification.
pub fn opt_dir(def_path: &Path) -> PathBuf {
    let stem = def_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    def_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(".rugen-opt")
        .join(stem)
}

/// The sprite pack at `pack` if it was built from `source` (same size and
/// mtime, current format, parsed the same way).
pub fn load_sprite_pack(pack: &Path, source: &Path, is_char: bool) -> Option<Sff> {
    let h = pack::read_header_file(pack).ok()?;
    if h.format != pack::FORMAT || h.is_char != is_char || Some(h.stamp) != Stamp::of(source) {
        return None;
    }
    match pack::open(pack) {
        Ok((sff, _)) => Some(sff),
        Err(e) => {
            log::warn!("{}: {e}", pack.display());
            None
        }
    }
}

fn sprite_pack_fresh(pack: &Path, source: &Path, is_char: bool) -> bool {
    pack::read_header_file(pack).is_ok_and(|h| {
        h.format == pack::FORMAT && h.is_char == is_char && Some(h.stamp) == Stamp::of(source)
    })
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

/// What [`optimise`] did.
#[derive(Clone, Debug, Default)]
pub struct OptReport {
    /// Sprite pack built now (`None`: it was already fresh).
    pub sprites: Option<pack::Stats>,
    /// Logic cache built now.
    pub logic: bool,
}

/// Whether the optimised artefacts of `def_path` exist and match the sources:
/// `(sprites, logic)`.
pub fn opt_fresh(def_path: &Path, data_dir: &Path) -> (bool, bool) {
    let Ok(src) = Sources::resolve(def_path, data_dir) else {
        return (false, false);
    };
    let dir = opt_dir(def_path);
    let sprites = sprite_pack_fresh(&dir.join(SPRITES_FILE), &src.sff, true);
    let logic = std::fs::read(dir.join(LOGIC_FILE))
        .ok()
        .and_then(|b| bincode::deserialize::<LogicHeader>(&b).ok())
        .is_some_and(|h| h.matches(&src));
    (sprites, logic)
}

/// Build the stale or missing optimised artefacts of a character into
/// [`opt_dir`]. `parsed` is the character's SFF if the caller already
/// parsed it (`Sff::parse(.., true)`, before any palette is applied).
pub fn optimise(def_path: &Path, data_dir: &Path, parsed: Option<Sff>) -> Result<OptReport> {
    let src = Sources::resolve(def_path, data_dir)?;
    let dir = opt_dir(def_path);
    let mut report = OptReport::default();
    std::thread::scope(|s| -> Result<()> {
        let logic = s.spawn(|| -> Result<bool> {
            let path = dir.join(LOGIC_FILE);
            if Logic::read_cached(&path, &src).is_some() {
                return Ok(false);
            }
            Logic::parse(&src)?.write_cached(&path, &src)?;
            Ok(true)
        });
        let pack_path = dir.join(SPRITES_FILE);
        if !sprite_pack_fresh(&pack_path, &src.sff, true) {
            let stamp =
                Stamp::of(&src.sff).ok_or_else(|| Error::Load("sprite file vanished".into()))?;
            let sff = match parsed {
                Some(s) => s,
                None => Sff::open(&src.sff, true)?,
            };
            let (bytes, stats) = pack::write(
                &sff,
                pack::Options {
                    is_char: true,
                    trim: true,
                },
                stamp,
            );
            write_atomic(&pack_path, &bytes)?;
            report.sprites = Some(stats);
        }
        report.logic = logic
            .join()
            .map_err(|_| Error::Load("logic optimiser panicked".into()))??;
        Ok(())
    })?;
    Ok(report)
}

/// Build a stage's sprite pack (untrimmed: stages tile sprites by their size)
/// into [`opt_dir`] unless it is fresh. Returns whether one was written.
pub fn optimise_stage(def_path: &Path, sff_path: &Path, parsed: Option<Sff>) -> Result<bool> {
    let pack_path = opt_dir(def_path).join(SPRITES_FILE);
    if sprite_pack_fresh(&pack_path, sff_path, false) {
        return Ok(false);
    }
    let stamp = Stamp::of(sff_path).ok_or_else(|| Error::Load("sprite file vanished".into()))?;
    let sff = match parsed {
        Some(s) => s,
        None => Sff::open(sff_path, false)?,
    };
    let opts = pack::Options {
        is_char: false,
        trim: false,
    };
    write_atomic(&pack_path, &pack::write(&sff, opts, stamp).0)?;
    Ok(true)
}

impl CharData {
    pub fn load(def_path: &Path) -> Result<Arc<Self>> {
        Self::load_with_data(def_path, Path::new(""))
    }

    /// `data_dir` holds the engine's `common1.cns`; the char's own directory is searched first.
    /// Uses the optimised artefacts when they are valid.
    pub fn load_with_data(def_path: &Path, data_dir: &Path) -> Result<Arc<Self>> {
        Self::load_opts(def_path, data_dir, LoadOptions::default())
    }

    /// Load from the original files only, ignoring any optimised artefacts
    /// (verification, equivalence tests).
    pub fn load_original(def_path: &Path, data_dir: &Path) -> Result<Arc<Self>> {
        Self::load_opts(def_path, data_dir, LoadOptions { optimised: false })
    }

    pub fn load_opts(def_path: &Path, data_dir: &Path, opts: LoadOptions) -> Result<Arc<Self>> {
        let src = Sources::resolve(def_path, data_dir)?;
        let opt = opts.optimised.then(|| opt_dir(def_path));
        let mut warnings = Vec::new();

        let rest = || {
            let logic = match opt
                .as_ref()
                .and_then(|d| Logic::read_cached(&d.join(LOGIC_FILE), &src))
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
        };
        let pack = opt.as_ref().map(|d| d.join(SPRITES_FILE));
        let (sff, logic, snd) = match pack.filter(|p| sprite_pack_fresh(p, &src.sff, true)) {
            // optimised: logic and sounds on a helper thread while this one reads the pack
            Some(pack) => std::thread::scope(|s| {
                let rest = s.spawn(rest);
                let sff = match pack::open(&pack) {
                    Ok((sff, _)) => Ok(sff),
                    Err(e) => {
                        log::warn!("{}: {e}", pack.display());
                        Sff::open(&src.sff, true).map_err(Error::from)
                    }
                };
                let (logic, snd) = rest
                    .join()
                    .unwrap_or_else(|_| (Err(Error::Load("logic loading panicked".into())), None));
                (sff, logic, snd)
            }),
            // originals: one after the other, as MUGEN files have always loaded
            None => {
                let sff = Sff::open(&src.sff, true).map_err(Error::from);
                let (logic, snd) = rest();
                (sff, logic, snd)
            }
        };
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
            let ctrls = st
                .controllers
                .iter()
                .map(|c| compile_ctrl(c, &mut warnings, file))
                .collect();
            d.warnings.extend(warnings);
            d.states.insert(
                st.no,
                State {
                    ctrls,
                    def: st,
                    file: file.to_string(),
                },
            );
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
