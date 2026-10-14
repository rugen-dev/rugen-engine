//! Replays: the match setup plus every frame's raw inputs. The simulation is
//! deterministic, so replaying the inputs reproduces the match exactly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use rugen_sim::scale::FighterSize;

use crate::play::FightSetup;

fn size_none() -> FighterSize {
    FighterSize::None
}

pub const REPLAY_VERSION: u32 = 1;

/// An online or LAN match: where it was played and against what.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OnlineMeta {
    /// The server's name (its host, never an address of a player), or `lan`.
    pub server: String,
    /// The server's match id (0 on LAN).
    #[serde(skip_serializing_if = "is_zero")]
    pub match_id: u64,
    /// Our side: 0 = P1, 1 = P2.
    pub side: u8,
    /// The other player's engine and OS / arch when its build sent them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<rugen_proto::MachineSummary>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// A fighter or stage of a replay: the id it goes by (the file stem of its
/// content file or folder, as in `chars/`) and its content fingerprint.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

impl ContentRef {
    /// The ref of the fighter / stage whose main def is `def`.
    pub fn of(def: &Path) -> Self {
        let m = rugen_content::manifest_for(def).map(|(_, m)| m);
        // the manifest's id: a downloaded fighter's file is named by its
        // fingerprint, its id is the def's stem
        let id = m
            .as_ref()
            .map(|m| m.id.clone())
            .filter(|id| !id.is_empty())
            .or_else(|| ids_of(def).into_iter().next())
            .unwrap_or_default();
        Self {
            id,
            fingerprint: m
                .and_then(|m| m.fingerprint)
                .filter(|f| rugen_proto::is_fingerprint(f)),
        }
    }
}

/// The fighters and stage of a replay by id and fingerprint (replays from
/// before them stored absolute paths only).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentRefs {
    pub p1: ContentRef,
    pub p2: ContentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<ContentRef>,
    /// Team partners, in the order of `teams.partners`.
    #[serde(default, skip_serializing_if = "no_partners")]
    pub partners: [Vec<ContentRef>; 2],
}

fn no_partners(p: &[Vec<ContentRef>; 2]) -> bool {
    p.iter().all(Vec::is_empty)
}

/// The ids a def path may go by, best first: the content file's stem
/// (`chars/x.rgc/x.def`), the folder's name (`chars/kfm/kfm.def`, a cache
/// folder's `x-<hash>` without the hash), the def's stem. Read from the
/// path alone, so a path that no longer exists still names its fighter.
pub fn ids_of(def: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(s)) {
            out.push(s.to_string());
        }
    };
    // a replay written on Windows names its paths with backslashes
    let text = def.to_string_lossy();
    let comps: Vec<&str> = text.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
    let ext =
        |c: &str, e: &str| c.len() > e.len() && c[c.len() - e.len()..].eq_ignore_ascii_case(e);
    if let Some(c) = comps
        .iter()
        .rev()
        .find(|c| ext(c, ".rgc") || ext(c, ".rgs"))
    {
        push(&c[..c.len() - 4]);
    }
    let Some(last) = comps.last() else {
        return out;
    };
    if ext(last, ".def") {
        if comps.len() >= 2 {
            let dir = comps[comps.len() - 2];
            if let Some((a, hash)) = dir.rsplit_once('-')
                && hash.len() >= 8
                && hash.chars().all(|c| c.is_ascii_alphanumeric())
            {
                push(a);
            }
            if !ext(dir, ".rgc") && !ext(dir, ".rgs") {
                push(dir);
            }
        }
        push(&last[..last.len() - 4]);
    } else {
        push(last);
    }
    out
}

/// How a replay stores `def`: relative to the game root `root` when it is
/// inside it, else only the id. Never an absolute path.
pub fn private_path(def: &Path, root: &Path, id: &str) -> PathBuf {
    if !def.is_absolute() && !def.as_os_str().is_empty() && !def.starts_with("..") {
        // already relative (a test's `a`, a path under the root)
        return def.to_path_buf();
    }
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    match def.strip_prefix(&root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
        _ => PathBuf::from(id),
    }
}

/// The def a replay's `path` and `r` (absent from old replays) stand for
/// in `lib`: the path as it is (an old replay's absolute path that still
/// exists), the path under the game root, the fingerprint (wherever the
/// content is now), then the id (a fighter moved to another folder, an old
/// replay's path gone). `None`: not on this machine.
pub fn resolve_def(
    lib: &rugen_content::Library,
    path: &Path,
    r: Option<&ContentRef>,
    stage: bool,
) -> Option<PathBuf> {
    use rugen_formats::vfs;
    if !path.as_os_str().is_empty() && vfs::exists(path) && path_is_def(path) {
        return Some(path.to_path_buf());
    }
    if path.is_relative() {
        let p = lib.root.join(path);
        if vfs::exists(&p) && path_is_def(&p) {
            return Some(p);
        }
    }
    if let Some(fp) = r.and_then(|r| r.fingerprint.as_deref())
        && let Some(d) = lib.def_for_fingerprint(fp)
    {
        return Some(d);
    }
    let mut ids: Vec<String> = r.map(|r| r.id.clone()).into_iter().collect();
    ids.extend(ids_of(path));
    for id in ids.iter().filter(|i| !i.is_empty()) {
        let found = if stage {
            lib.stages()
                .into_iter()
                .find(|e| e.id.eq_ignore_ascii_case(id))
                .and_then(|e| lib.prepare_stage(&e).ok())
        } else {
            lib.chars()
                .into_iter()
                .find(|e| e.id.eq_ignore_ascii_case(id))
                .and_then(|e| lib.prepare_char(&e).ok())
        };
        if let Some((d, m)) = found {
            return Some(m.def_path(&d));
        }
    }
    None
}

/// A main def (not a folder that happens to share the name).
fn path_is_def(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("def"))
}

/// Longest part of a replay file name (a fighter, a team, the stage).
const NAME_PART_MAX: usize = 32;

/// A fighter or stage id as part of a file name: `[A-Za-z0-9_]` only
/// (spaces become `_`, anything else is dropped), at most
/// [`NAME_PART_MAX`] characters; `unknown` when nothing is left.
pub fn name_part(id: &str) -> String {
    let s: String = id
        .chars()
        .filter_map(|c| match c {
            c if c.is_ascii_alphanumeric() || c == '_' => Some(c),
            ' ' => Some('_'),
            _ => None,
        })
        .take(NAME_PART_MAX)
        .collect();
    if s.is_empty() { "unknown".into() } else { s }
}

/// A side of a replay file name: the fighter, or a team's members joined
/// by `_and_` while that stays short, else the leader and `_team`.
fn side_part(lead: &str, partners: &[String]) -> String {
    let lead = name_part(lead);
    if partners.is_empty() {
        return lead;
    }
    let all: Vec<String> = std::iter::once(lead.clone())
        .chain(partners.iter().map(|p| name_part(p)))
        .collect();
    let joined = all.join("_and_");
    if joined.len() <= NAME_PART_MAX {
        joined
    } else {
        let short: String = lead.chars().take(NAME_PART_MAX - 5).collect();
        format!("{short}_team")
    }
}

/// `dir/<stem>.json`, else `dir/<stem>-2.json`, `-3`, ... (the first free).
pub fn free_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.json"));
    if !first.exists() {
        return first;
    }
    (2..10_000)
        .map(|n| dir.join(format!("{stem}-{n}.json")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// A replay file found in `replays/`.
#[derive(Clone, Debug)]
pub struct Saved {
    pub path: PathBuf,
    /// When it was recorded (Unix seconds): its `recorded_at`, else the
    /// file's modification time (replays from before the header).
    pub at: u64,
    pub replay: Replay,
}

/// The replays in `dir`, newest first: readable names and the old
/// `<unix time>.json` ones alike (files that are not replays are skipped).
pub fn list_saved(dir: &Path) -> Vec<Saved> {
    let mut out: Vec<Saved> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .filter_map(|path| {
            let replay = Replay::load(&path).ok()?;
            let mtime = || {
                std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs())
            };
            let at = replay
                .recorded_at
                .as_deref()
                .and_then(crate::machine::parse_rfc3339)
                .unwrap_or_else(mtime);
            Some(Saved { path, at, replay })
        })
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.path.cmp(&b.path)));
    out
}

/// The `mode` a replay records for a match started from `mode`.
pub fn mode_name(mode: Option<crate::game::Mode>) -> &'static str {
    use crate::game::Mode;
    match mode {
        Some(Mode::Arcade) => "arcade",
        Some(Mode::Versus) => "vs",
        Some(Mode::Training) => "training",
        Some(Mode::Tutorial) => "tutorial",
        Some(Mode::Watch) => "watch",
        Some(Mode::Arena) => "arena",
        Some(Mode::Survival) => "survival",
        Some(Mode::BossRush) => "boss_rush",
        Some(Mode::TimeAttack) => "time_attack",
        Some(Mode::Debugger) => "debugger",
        Some(Mode::Online | Mode::Netplay) => "online",
        Some(Mode::Options | Mode::Exit) | None => "quick",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Replay {
    pub version: u32,
    /// The build that recorded it; absent from older replays. Header
    /// fields (this one to `online`) never change how the match plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<crate::machine::Engine>,
    /// The machine that recorded it (no personal data, see
    /// [`crate::machine`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<crate::machine::Machine>,
    /// `arcade`, `vs`, `training`, `watch`, `arena`, `survival`,
    /// `boss_rush`, `time_attack`, `online`, `lan`, `quick` (`rugen play`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// When the match started, UTC RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    /// Online and LAN matches: where and against what.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<OnlineMeta>,
    pub p1: PathBuf,
    pub p2: PathBuf,
    pub stage: Option<PathBuf>,
    pub ai: [i32; 2],
    /// FIGHTER SIZE the match was simulated with; replays from before the
    /// option played every fighter at its own size.
    #[serde(default = "size_none")]
    pub fighter_size: FighterSize,
    /// Arena AI plugin for the CPU sides; replays from before the option
    /// played without it.
    #[serde(default)]
    pub arena_ai: bool,
    /// Round clock the match was simulated with (seconds, <= 0 = none);
    /// replays from before it was recorded play with the options' clock.
    #[serde(default)]
    pub round_time: Option<i32>,
    /// Team modes and partners (offline team matches); absent for 1v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<crate::play::TeamSetup>,
    /// A CHALLENGE run's rules for this fight (rounds, P1's carried life);
    /// absent for every other match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<crate::play::RunRules>,
    /// The match seed; replays from before matches had their own were
    /// simulated with [`rugen_sim::headless::LEGACY_SEED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Training: the lab's world edits and the frame (input index) each
    /// took effect on; absent for every other match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<(u32, crate::training::Edits)>,
    /// The LIFE every fighter had; absent: each fighter's own (replays
    /// from before the setting, or a match played that way).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub life: Option<i32>,
    /// The camera, movement and online rules the match was played with
    /// (absent from older replays: they play with the player's settings).
    #[serde(default, skip_serializing_if = "crate::play::SimRules::is_empty")]
    pub sim: crate::play::SimRules,
    /// The fighters and stage by id and content fingerprint (`p1`, `p2`,
    /// `stage` then hold at most a path relative to the game root); absent
    /// from replays that stored absolute paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentRefs>,
    /// `[p1 bits, p2 bits]` per tick (see `Buttons::bits`).
    pub inputs: Vec<[u16; 2]>,
}

impl Replay {
    /// A recording of `setup`, simulated with `fighter_size`, `arena_ai`,
    /// a `round_time` clock, the match `seed` and `life` (`None`: each
    /// fighter's own).
    pub fn new(
        setup: &FightSetup,
        fighter_size: FighterSize,
        arena_ai: bool,
        round_time: i32,
        seed: u64,
        life: Option<i32>,
    ) -> Self {
        Self {
            version: REPLAY_VERSION,
            engine: Some(crate::machine::Engine::current()),
            machine: Some(crate::machine::Machine::current()),
            mode: Some(mode_name(setup.mode).into()),
            recorded_at: Some(crate::machine::now_rfc3339()),
            online: None,
            p1: setup.p1.clone(),
            p2: setup.p2.clone(),
            stage: setup.stage.clone(),
            ai: setup.ai,
            fighter_size,
            arena_ai,
            round_time: Some(round_time),
            teams: setup.teams.clone(),
            run: setup.run,
            seed: Some(seed),
            edits: Vec::new(),
            life,
            sim: Default::default(),
            content: Some(ContentRefs {
                p1: ContentRef::of(&setup.p1),
                p2: ContentRef::of(&setup.p2),
                stage: setup.stage.as_deref().map(ContentRef::of),
                partners: setup.teams.as_ref().map_or_else(Default::default, |t| {
                    [0, 1].map(|s| t.partners[s].iter().map(|p| ContentRef::of(p)).collect())
                }),
            }),
            inputs: Vec::new(),
        }
    }

    /// The match setup with every fighter and the stage found in `lib`
    /// ([`resolve_def`]); what cannot be found keeps its recorded path
    /// (loading it then says what is missing).
    pub fn resolve(&self, lib: &rugen_content::Library) -> FightSetup {
        let refs = self.content.as_ref();
        let find = |p: &Path, r: Option<&ContentRef>, stage: bool| {
            resolve_def(lib, p, r, stage).unwrap_or_else(|| {
                log::warn!("replay: {} is not on this machine", p.display());
                p.to_path_buf()
            })
        };
        let mut setup = self.setup();
        setup.p1 = find(&self.p1, refs.map(|r| &r.p1), false);
        setup.p2 = find(&self.p2, refs.map(|r| &r.p2), false);
        setup.stage = self
            .stage
            .as_ref()
            .map(|s| find(s, refs.and_then(|r| r.stage.as_ref()), true));
        if let Some(t) = setup.teams.as_mut() {
            for side in 0..2 {
                for (i, p) in t.partners[side].iter_mut().enumerate() {
                    *p = find(p, refs.and_then(|r| r.partners[side].get(i)), false);
                }
            }
        }
        setup
    }

    /// 6 hex characters of the match itself (seed and inputs): stable for
    /// the same match, different for another one.
    pub fn content_hash(&self) -> String {
        // FNV-1a 64
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |b: &[u8]| {
            for x in b {
                h ^= u64::from(*x);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(&self.seed.unwrap_or(0).to_le_bytes());
        for [a, b] in &self.inputs {
            eat(&a.to_le_bytes());
            eat(&b.to_le_bytes());
        }
        format!("{:06x}", (h ^ (h >> 32)) & 0xff_ffff)
    }

    /// The file name a replay is saved under, without `.json`:
    /// `<p1>-vs-<p2>-<stage>-<hash>` (ids, team members joined by `_and_`),
    /// e.g. `G6_Obito_Rikudou-vs-G6_Sakura-Forest_of_Dead_Trees-3f9a2c`.
    pub fn file_stem(&self) -> String {
        let refs = self.content.as_ref();
        let id = |p: &Path, r: Option<&ContentRef>| {
            r.map(|r| r.id.clone())
                .filter(|i| !i.is_empty())
                .or_else(|| ids_of(p).into_iter().next())
                .unwrap_or_default()
        };
        let partners = |side: usize| -> Vec<String> {
            self.teams.as_ref().map_or_else(Vec::new, |t| {
                t.partners[side]
                    .iter()
                    .enumerate()
                    .map(|(i, p)| id(p, refs.and_then(|r| r.partners[side].get(i))))
                    .collect()
            })
        };
        let mut parts = vec![
            side_part(&id(&self.p1, refs.map(|r| &r.p1)), &partners(0)),
            "vs".to_string(),
            side_part(&id(&self.p2, refs.map(|r| &r.p2)), &partners(1)),
        ];
        if let Some(s) = &self.stage {
            parts.push(name_part(&id(s, refs.and_then(|r| r.stage.as_ref()))));
        }
        parts.push(self.content_hash());
        parts.join("-")
    }

    /// The header in words (`rugen replay <file> --info`): what recorded
    /// it, where, and what it plays; never the inputs.
    pub fn info(&self) -> String {
        let mut rows: Vec<(&str, String)> = vec![("version", self.version.to_string())];
        match &self.engine {
            Some(e) => {
                let mut line = format!("rugen {}", e.version);
                if !e.commit.is_empty() {
                    line.push_str(&format!(" ({})", e.commit));
                }
                rows.push(("engine", format!("{line} · {} · {}", e.target, e.build)));
            }
            None => rows.push(("engine", "not recorded (an older rugen)".into())),
        }
        if let Some(at) = &self.recorded_at {
            rows.push(("recorded", at.clone()));
        }
        if let Some(m) = &self.mode {
            rows.push(("mode", m.clone()));
        }
        if let Some(m) = &self.machine {
            rows.push(("machine", m.line()));
            let mut more = format!("{} {}", m.os, m.os_version).trim().to_string();
            if !m.driver.is_empty() {
                more.push_str(&format!(" · driver {}", m.driver));
            }
            rows.push(("", more));
        }
        if let Some(o) = &self.online {
            let mut line = o.server.clone();
            if o.match_id != 0 {
                line.push_str(&format!(" · match {}", o.match_id));
            }
            line.push_str(&format!(" · we played P{}", o.side + 1));
            rows.push(("online", line));
            rows.push((
                "peer",
                o.peer
                    .as_ref()
                    .map_or_else(|| "no details sent (an older rugen)".into(), |m| m.line()),
            ));
        }
        let refs = self.content.as_ref();
        let content = |p: &Path, r: Option<&ContentRef>| {
            let id = r
                .map(|r| r.id.clone())
                .unwrap_or_else(|| ids_of(p).into_iter().next().unwrap_or_default());
            let fp = r
                .and_then(|r| r.fingerprint.as_deref())
                .map(|f| format!(" (fingerprint {})", &f[..f.len().min(12)]))
                .unwrap_or_default();
            format!("{id}{fp} · {}", p.display())
        };
        rows.push(("p1", content(&self.p1, refs.map(|r| &r.p1))));
        rows.push(("p2", content(&self.p2, refs.map(|r| &r.p2))));
        if let Some(s) = &self.stage {
            rows.push(("stage", content(s, refs.and_then(|r| r.stage.as_ref()))));
        }
        if let Some(t) = &self.teams {
            for side in 0..2 {
                if !t.partners[side].is_empty() {
                    let names: Vec<String> = t.partners[side]
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            refs.and_then(|r| r.partners[side].get(i))
                                .map(|r| r.id.clone())
                                .unwrap_or_else(|| ids_of(p).into_iter().next().unwrap_or_default())
                        })
                        .collect();
                    rows.push((
                        if side == 0 { "p1 team" } else { "p2 team" },
                        format!("{:?} with {}", t.mode[side], names.join(", ")).to_lowercase(),
                    ));
                }
            }
        }
        let secs = self.inputs.len() / 60;
        rows.push((
            "frames",
            format!("{} ({}:{:02})", self.inputs.len(), secs / 60, secs % 60),
        ));
        let mut rules = vec![
            format!(
                "seed {}",
                self.seed.unwrap_or(rugen_sim::headless::LEGACY_SEED)
            ),
            format!("fighter size {}", self.fighter_size.as_str()),
        ];
        if let Some(t) = self.round_time {
            rules.push(if t > 0 {
                format!("round time {t}")
            } else {
                "no round clock".into()
            });
        }
        if let Some(l) = self.life {
            rules.push(format!("life {l}"));
        }
        if self.arena_ai {
            rules.push("arena ai".into());
        }
        if !self.edits.is_empty() {
            rules.push(format!("{} training edits", self.edits.len()));
        }
        if let Some([o, i]) = self.sim.zoom {
            rules.push(format!("zoom {o}-{i}"));
        }
        if let Some(m) = self.sim.movement {
            rules.push(format!(
                "air jumps {}{}{}",
                m.air_jumps,
                if m.air_dash { " · air dash" } else { "" },
                if m.fast_fall { " · fast fall" } else { "" }
            ));
        }
        if self.sim.online {
            rules.push("online rules".into());
        }
        if let Some(w) = self.sim.match_wins {
            rules.push(format!("{w} round(s) to win"));
        }
        rows.push(("rules", rules.join(" · ")));
        rows.iter()
            .map(|(k, v)| format!("{k:<9} {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The replay as it is written: fighters and the stage at most relative
    /// to the game root `root` (else by id), never an absolute path.
    pub fn private(&self, root: &Path) -> Self {
        let mut r = self.clone();
        let refs = r.content.get_or_insert_with(|| ContentRefs {
            p1: ContentRef::of(&self.p1),
            p2: ContentRef::of(&self.p2),
            stage: self.stage.as_deref().map(ContentRef::of),
            partners: Default::default(),
        });
        r.p1 = private_path(&self.p1, root, &refs.p1.id);
        r.p2 = private_path(&self.p2, root, &refs.p2.id);
        r.stage = self.stage.as_ref().map(|s| {
            let id = refs
                .stage
                .as_ref()
                .map(|r| r.id.clone())
                .unwrap_or_default();
            private_path(s, root, &id)
        });
        if let Some(t) = r.teams.as_mut() {
            for side in 0..2 {
                for (i, p) in t.partners[side].iter_mut().enumerate() {
                    let id = refs.partners[side]
                        .get(i)
                        .map(|r| r.id.clone())
                        .unwrap_or_else(|| ids_of(p).into_iter().next().unwrap_or_default());
                    *p = private_path(p, root, &id);
                }
            }
        }
        r
    }

    pub fn setup(&self) -> FightSetup {
        FightSetup {
            life: Some(self.life.unwrap_or(0)),
            p1: self.p1.clone(),
            p2: self.p2.clone(),
            stage: self.stage.clone(),
            ai: self.ai,
            net: None,
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: None,
            fighter_size: Some(self.fighter_size),
            arena_ai: Some(self.arena_ai),
            round_time: self.round_time,
            teams: self.teams.clone(),
            run: self.run,
            seed: Some(self.seed.unwrap_or(rugen_sim::headless::LEGACY_SEED)),
            sim: self.sim,
        }
    }

    /// Write the replay to `path` in its private form ([`Replay::private`],
    /// `root` = the game root).
    pub fn save(&self, path: &Path, root: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec(&self.private(root))?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let r: Replay = serde_json::from_slice(&std::fs::read(path)?)?;
        anyhow::ensure!(
            r.version == REPLAY_VERSION,
            "replay version {} (expected {REPLAY_VERSION})",
            r.version
        );
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_keep_their_fighter_size() {
        // recorded before the option: every fighter at its own size
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[[1,2]]}"#,
        )
        .unwrap();
        assert_eq!(old.fighter_size, FighterSize::None);
        assert!(!old.arena_ai, "old replays played without the arena AI");
        assert_eq!(old.setup().arena_ai, Some(false));
        assert_eq!(old.setup().fighter_size, Some(FighterSize::None));
        assert_eq!(
            old.setup().round_time,
            None,
            "old replays: the options' clock"
        );
        let r = Replay {
            fighter_size: FighterSize::Chibi,
            ..old
        };
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.fighter_size, FighterSize::Chibi);
        assert_eq!(back.setup().fighter_size, Some(FighterSize::Chibi));
    }

    /// LIFE: a recording keeps it; one from before the setting plays each
    /// fighter's own life (`Some(0)`), whatever the options say now.
    #[test]
    fn replays_keep_their_life() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[[1,2]]}"#,
        )
        .unwrap();
        assert_eq!(old.life, None);
        assert_eq!(old.setup().life, Some(0), "old replays: each fighter's own");
        let text = serde_json::to_string(&old).unwrap();
        assert!(!text.contains("life"), "absent stays absent: {text}");
        let r = Replay::new(&old.setup(), FighterSize::Normal, false, 99, 7, Some(5000));
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.life, Some(5000));
        assert_eq!(back.setup().life, Some(5000));
    }

    /// A match played with ROUND TIME 30 replays with 30, whatever the
    /// options say when it is watched.
    #[test]
    fn replays_keep_their_round_time() {
        let setup = Replay {
            version: REPLAY_VERSION,
            engine: None,
            machine: None,
            mode: None,
            recorded_at: None,
            online: None,
            p1: "a".into(),
            p2: "b".into(),
            stage: None,
            ai: [0, 4],
            fighter_size: FighterSize::Normal,
            arena_ai: false,
            round_time: None,
            teams: None,
            run: None,
            seed: None,
            edits: Vec::new(),
            life: None,
            sim: Default::default(),
            content: None,
            inputs: Vec::new(),
        }
        .setup();
        let r = Replay::new(&setup, FighterSize::Normal, false, 30, 1, None);
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.round_time, Some(30));
        assert_eq!(back.setup().round_time, Some(30));
        let none = Replay::new(&setup, FighterSize::Normal, false, -1, 1, None);
        assert_eq!(
            none.setup().round_time,
            Some(-1),
            "no clock is recorded too"
        );
    }

    /// A team match replays as one: modes and partners ride along; 1v1
    /// replays write no `teams` and old ones read as 1v1.
    #[test]
    fn replays_keep_their_teams() {
        use rugen_sim::team::TeamMode;
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(old.setup().teams, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("teams"));
        let mut setup = old.setup();
        setup.teams = Some(crate::play::TeamSetup {
            mode: [TeamMode::Turns, TeamMode::Single],
            partners: [vec!["c".into(), "d".into()], vec![]],
            ids: Default::default(),
        });
        let r = Replay::new(&setup, FighterSize::Normal, false, 60, 1, None);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""mode":["turns","single"]"#), "{json}");
        let back: Replay = serde_json::from_str(&json).unwrap();
        assert_eq!(back.setup().teams, setup.teams);
    }

    /// A Survival fight replays with its one round and P1's carried life;
    /// other replays write no `run` and old ones read without one.
    #[test]
    fn replays_keep_their_run_rules() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(old.setup().run, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("run"));
        let mut setup = old.setup();
        setup.run = Some(crate::play::RunRules {
            match_wins: 1,
            p1_life: Some(640),
        });
        let r = Replay::new(&setup, FighterSize::Normal, false, 60, 1, None);
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.setup().run, setup.run);
    }

    /// Per-match seed: a replay plays with the seed it recorded; one from
    /// before matches had their own plays with seed 1, as it was simulated.
    #[test]
    fn replays_keep_their_seed() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(old.seed, None);
        assert_eq!(old.setup().seed, Some(rugen_sim::headless::LEGACY_SEED));
        let r = Replay::new(
            &old.setup(),
            FighterSize::Normal,
            false,
            60,
            0xDEAD_BEEF,
            None,
        );
        let json = serde_json::to_string(&r).unwrap();
        let back: Replay = serde_json::from_str(&json).unwrap();
        assert_eq!(back.seed, Some(0xDEAD_BEEF));
        assert_eq!(back.setup().seed, Some(0xDEAD_BEEF));
    }

    /// Engine, machine, mode and time ride along and come back; a replay
    /// from before them reads without them and plays as it did.
    #[test]
    fn replays_say_what_recorded_them() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"inputs":[[1,2]]}"#,
        )
        .unwrap();
        assert!(old.engine.is_none() && old.machine.is_none() && old.online.is_none());
        assert!(!serde_json::to_string(&old).unwrap().contains("engine"));
        let mut setup = old.setup();
        setup.mode = Some(crate::game::Mode::Arcade);
        let mut r = Replay::new(&setup, FighterSize::None, false, 60, 3, None);
        r.online = Some(OnlineMeta {
            server: "play.example.dev".into(),
            match_id: 42,
            side: 1,
            peer: Some(rugen_proto::MachineSummary {
                engine: "rugen 0.7.0".into(),
                os: "Windows 11 10.0.22631".into(),
                arch: "x86_64".into(),
                ..Default::default()
            }),
        });
        let e = r.engine.clone().unwrap();
        assert_eq!(e.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(r.mode.as_deref(), Some("arcade"));
        let at = r.recorded_at.clone().unwrap();
        assert!(crate::machine::parse_rfc3339(&at).is_some(), "{at}");
        let back: Replay = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back.engine, r.engine);
        assert_eq!(back.machine, r.machine);
        assert_eq!(back.online, r.online);
        assert_eq!(back.recorded_at, r.recorded_at);
        // the header never changes the match
        let (a, b) = (old.setup(), back.setup());
        assert_eq!(
            (a.p1, a.p2, a.ai, a.fighter_size, a.life),
            (b.p1, b.p2, b.ai, b.fighter_size, b.life)
        );
    }

    #[test]
    fn paths_name_their_fighter() {
        let first = |p: &str| ids_of(Path::new(p)).into_iter().next().unwrap_or_default();
        assert_eq!(
            first("/home/u/RUGEN/chars/G6_Madara.rgc/G6_Madara.def"),
            "G6_Madara"
        );
        assert_eq!(
            first(r"C:\Users\u\RUGEN\chars\G6_Obito.rgc\obito.def"),
            "G6_Obito"
        );
        assert_eq!(
            first("/home/u/RUGEN/cache/chars/kfm-0123456789abcdef/kfm.def"),
            "kfm"
        );
        assert_eq!(first("/old/chars/kfm/kfm.def"), "kfm");
        assert_eq!(first("G6_Madara"), "G6_Madara");
        assert_eq!(first("chars/Forest.rgs/stage.def"), "Forest");
        let root = Path::new("/home/u/RUGEN");
        assert_eq!(
            private_path(Path::new("/home/u/RUGEN/chars/a.rgc/a.def"), root, "a"),
            PathBuf::from("chars/a.rgc/a.def")
        );
        assert_eq!(
            private_path(Path::new("/home/u/.cache/x/b.rgc/b.def"), root, "b"),
            PathBuf::from("b")
        );
        assert_eq!(private_path(Path::new("a"), root, "a"), PathBuf::from("a"));
    }

    /// A written replay holds no home folder, user name or hostname: the
    /// fighters and stage go by root-relative path or id.
    #[test]
    fn written_replays_hold_nothing_personal() {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/home/tester".into());
        let home = PathBuf::from(home);
        let root = home.join("RUGEN");
        let mut setup = Replay {
            version: REPLAY_VERSION,
            engine: None,
            machine: None,
            mode: None,
            recorded_at: None,
            online: None,
            p1: root.join("chars/G6_Madara.rgc/G6_Madara.def"),
            p2: home.join(".cache/elsewhere/Sakura.rgc/Sakura.def"),
            stage: Some(root.join("stages/Forest.rgs/Forest.def")),
            ai: [0, 4],
            fighter_size: FighterSize::None,
            arena_ai: false,
            round_time: None,
            teams: None,
            run: None,
            seed: None,
            edits: Vec::new(),
            life: None,
            sim: Default::default(),
            content: None,
            inputs: Vec::new(),
        }
        .setup();
        setup.teams = Some(crate::play::TeamSetup {
            mode: [
                rugen_sim::team::TeamMode::Turns,
                rugen_sim::team::TeamMode::Single,
            ],
            partners: [vec![root.join("chars/Kakashi.rgc/Kakashi.def")], vec![]],
            ids: Default::default(),
        });
        let mut r = Replay::new(&setup, FighterSize::None, false, 60, 9, None);
        r.inputs = vec![[1, 2]; 90];
        let dir = std::env::temp_dir().join(format!("rugen-replay-privacy-{}", std::process::id()));
        let file = dir.join("r.json");
        r.save(&file, &root).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!text.contains(&*home.to_string_lossy()), "{text}");
        let personal = crate::machine::Personal::current();
        assert_eq!(personal.leaks(&text), None, "{text}");
        let back: Replay = serde_json::from_str(&text).unwrap();
        assert_eq!(back.p1, PathBuf::from("chars/G6_Madara.rgc/G6_Madara.def"));
        assert_eq!(
            back.p2,
            PathBuf::from("Sakura"),
            "outside the root: the id only"
        );
        assert_eq!(
            back.stage,
            Some(PathBuf::from("stages/Forest.rgs/Forest.def"))
        );
        let refs = back.content.unwrap();
        assert_eq!(
            (refs.p1.id.as_str(), refs.p2.id.as_str()),
            ("G6_Madara", "Sakura")
        );
        assert_eq!(refs.partners[0][0].id, "Kakashi");
        assert_eq!(
            back.teams.unwrap().partners[0][0],
            PathBuf::from("chars/Kakashi.rgc/Kakashi.def")
        );
        // engine and machine are written, still nothing personal in them
        assert!(text.contains(r#""engine":{"#) && text.contains(r#""machine":{"#));
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                copy_dir(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }

    /// An old replay (absolute paths, no header) plays to the checksum it
    /// was recorded with; so does one whose fighter moved (found by id),
    /// and a new one stored by id outside the game root.
    #[test]
    fn old_and_moved_replays_play_the_same_match() {
        use crate::config::Config;
        use crate::play::{FightSetup, Match};
        use rugen_sim::Buttons;
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let reference = base.join("reference/mugen-1.1b1");
        let kfm = std::path::absolute(reference.join("chars/kfm/kfm.def")).unwrap();
        if !kfm.exists() {
            return;
        }
        let motif = base.join("motif/themes/ink");
        let data = reference.join("data");
        let cfg = Config::default();
        let setup = FightSetup {
            sim: Default::default(),
            life: None,
            p1: kfm.clone(),
            p2: kfm.clone(),
            stage: None,
            ai: [0, 0],
            net: None,
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: Some(crate::game::Mode::Versus),
            fighter_size: None,
            arena_ai: None,
            round_time: None,
            teams: None,
            run: None,
            seed: Some(5),
        };
        let mut audio = None;
        let mut m = Match::load(&setup, &motif, &data, &cfg).expect("KFM match");
        m.record = Some(Replay::new(
            &setup,
            m.world.fighter_size,
            m.world.arena_ai,
            m.world.round.config.round_time,
            m.world.seed(),
            m.world.life,
        ));
        for f in 0..500 {
            let b = Buttons {
                r: f % 70 < 30,
                x: f % 70 == 40,
                ..Default::default()
            };
            let c = Buttons {
                l: f % 50 < 20,
                a: f % 50 == 30,
                ..Default::default()
            };
            m.tick([b, c], &mut audio);
        }
        let want = m.world.checksum();
        let rec = m.record.take().expect("recorded");
        // what a replay written before the header and the ids looked like
        let mut v = serde_json::to_value(&rec).unwrap();
        for k in ["engine", "machine", "mode", "recorded_at", "content"] {
            v.as_object_mut().unwrap().remove(k);
        }
        let old: Replay = serde_json::from_value(v).unwrap();
        assert!(old.content.is_none() && old.p1.is_absolute());
        let root = std::env::temp_dir().join(format!("rugen-replay-moved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_dir(&reference.join("chars/kfm"), &root.join("chars/kfm"));
        let lib = rugen_content::Library::discover(Some(&root));
        let play = |r: &Replay| {
            let setup = r.resolve(&lib);
            let mut p = Match::load(&setup, &motif, &data, &cfg).expect("replay");
            p.edit_plan = r.edits.clone();
            let n = r.inputs.len();
            p.playback = Some((r.inputs.clone(), 0));
            let mut audio = None;
            for _ in 0..n {
                p.tick([Buttons::default(); 2], &mut audio);
            }
            (setup, p.world.checksum())
        };
        let (s, sum) = play(&old);
        assert_eq!(s.p1, kfm, "the old path still exists: kept");
        assert_eq!(sum, want, "an old replay plays the same match");
        // the fighter moved: the old path is gone, its id finds it
        let gone = PathBuf::from("/gone/home/someone/RUGEN/chars/kfm/kfm.def");
        let moved = Replay {
            p1: gone.clone(),
            p2: gone.clone(),
            ..old.clone()
        };
        let (s, sum) = play(&moved);
        assert_ne!(s.p1, gone);
        assert!(rugen_formats::vfs::exists(&s.p1), "{}", s.p1.display());
        assert_eq!(sum, want, "a moved fighter plays the same match");
        // a new replay outside the game root goes by id + fingerprint
        let stored: Replay =
            serde_json::from_slice(&serde_json::to_vec(&rec.private(&root)).unwrap()).unwrap();
        assert_eq!(stored.p1, PathBuf::from("kfm"));
        let (_, sum) = play(&stored);
        assert_eq!(sum, want, "a replay stored by id plays the same match");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replay_names_are_readable_and_safe() {
        assert_eq!(name_part("G6_Obito Rikudou"), "G6_Obito_Rikudou");
        assert_eq!(name_part("Forest of Dead Trees!"), "Forest_of_Dead_Trees");
        assert_eq!(name_part("../../etc/passwd"), "etcpasswd");
        assert_eq!(name_part("ナルト"), "unknown");
        assert_eq!(name_part(&"x".repeat(80)).len(), NAME_PART_MAX);
        assert_eq!(side_part("Goku", &["KFM".into()]), "Goku_and_KFM");
        assert_eq!(
            side_part(
                "G6_Obito_Rikudou",
                &["G6_Kakashi".into(), "G6_Sakura".into()]
            ),
            "G6_Obito_Rikudou_team"
        );
        let mut r: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"/home/u/RUGEN/chars/G6_Obito Rikudou.rgc/obito.def",
                "p2":"chars/G6_Sakura.rgc/G6_Sakura.def",
                "stage":"/x/stages/Forest of Dead Trees.rgs/forest.def",
                "ai":[0,4],"seed":7,"inputs":[[1,2],[3,4]]}"#,
        )
        .unwrap();
        let stem = r.file_stem();
        assert!(
            stem.starts_with("G6_Obito_Rikudou-vs-G6_Sakura-Forest_of_Dead_Trees-"),
            "{stem}"
        );
        assert_eq!(
            stem.len(),
            "G6_Obito_Rikudou-vs-G6_Sakura-Forest_of_Dead_Trees-".len() + 6
        );
        assert!(stem.len() + 5 < 120);
        // stable for the same match, another match another name
        assert_eq!(r.file_stem(), stem);
        r.inputs.push([5, 6]);
        assert_ne!(r.file_stem(), stem);
        // the longest names stay under 120 characters
        let long = "N".repeat(200);
        let mut big = r.clone();
        big.p1 = PathBuf::from(&long);
        big.p2 = PathBuf::from(&long);
        big.stage = Some(PathBuf::from(&long));
        assert!(big.file_stem().len() + 5 < 120, "{}", big.file_stem());
    }

    /// Saving the same match twice gives `-2`; old `<unix time>.json` files
    /// still load and list, sorted with the new ones by when they were
    /// recorded.
    #[test]
    fn replay_files_stay_unique_and_old_names_list() {
        let dir = std::env::temp_dir().join(format!("rugen-replay-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.clone();
        let mut r: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"a","p2":"b","stage":null,"ai":[0,4],"seed":3,"inputs":[[1,2]]}"#,
        )
        .unwrap();
        r.recorded_at = Some("2026-10-14T09:30:00Z".into());
        let first = free_path(&dir, &r.file_stem());
        r.save(&first, &root).unwrap();
        let second = free_path(&dir, &r.file_stem());
        assert_ne!(first, second);
        assert!(
            second.to_string_lossy().ends_with("-2.json"),
            "{}",
            second.display()
        );
        r.recorded_at = Some("2026-10-14T09:40:00Z".into());
        r.save(&second, &root).unwrap();
        // a replay from before readable names: no header, a timestamp name
        let old = dir.join("1700000000.json");
        std::fs::write(
            &old,
            r#"{"version":1,"p1":"/home/u/RUGEN/chars/kfm/kfm.def","p2":"b","stage":null,"ai":[0,4],"inputs":[[1,2]]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("notes.json"), "not a replay").unwrap();
        let list = list_saved(&dir);
        let names: Vec<String> = list
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        // the old file was written now (mtime): newest
        assert_eq!(names[0], "1700000000.json");
        assert!(names[1].ends_with("-2.json") && !names[2].ends_with("-2.json"));
        assert!(Replay::load(&old).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn info_reads_the_header() {
        let old: Replay = serde_json::from_str(
            r#"{"version":1,"p1":"/home/u/RUGEN/chars/kfm/kfm.def","p2":"b","stage":null,"ai":[0,4],"inputs":[[1,2]]}"#,
        )
        .unwrap();
        let text = old.info();
        assert!(text.contains("not recorded (an older rugen)"), "{text}");
        assert!(
            text.contains("p1        kfm · /home/u/RUGEN/chars/kfm/kfm.def"),
            "{text}"
        );
        let r = Replay::new(&old.setup(), FighterSize::None, false, 99, 4, None);
        let text = r.info();
        assert!(
            text.contains(&format!("rugen {}", env!("CARGO_PKG_VERSION"))),
            "{text}"
        );
        assert!(
            text.contains("machine") && text.contains(std::env::consts::ARCH),
            "{text}"
        );
    }

    /// An offline replay keeps the camera and movement it was played with:
    /// changing them in OPTIONS afterwards does not change the replay.
    #[test]
    fn replays_keep_their_camera_and_movement() {
        use crate::config::Config;
        use crate::play::{FightSetup, Match};
        use rugen_sim::Buttons;
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let reference = base.join("reference/mugen-1.1b1");
        let kfm = std::path::absolute(reference.join("chars/kfm/kfm.def")).unwrap();
        if !kfm.exists() {
            return;
        }
        let motif = base.join("motif/themes/ink");
        let data = reference.join("data");
        let mut then = Config::default();
        (then.camera.zoom_out, then.camera.zoom_in) = (0.3, 2.5);
        then.game.air_jumps = 3;
        then.game.air_dash = true;
        let setup = FightSetup {
            sim: Default::default(),
            life: None,
            p1: kfm.clone(),
            p2: kfm,
            stage: None,
            ai: [0, 0],
            net: None,
            tutorial: false,
            ids: [String::new(), String::new()],
            mode: Some(crate::game::Mode::Versus),
            fighter_size: None,
            arena_ai: None,
            round_time: None,
            teams: None,
            run: None,
            seed: Some(11),
        };
        let mut audio = None;
        let mut m = Match::load(&setup, &motif, &data, &then).expect("KFM match");
        let mut rec = Replay::new(
            &setup,
            m.world.fighter_size,
            m.world.arena_ai,
            m.world.round.config.round_time,
            m.world.seed(),
            m.world.life,
        );
        rec.sim = m.sim_rules;
        m.record = Some(rec);
        for f in 0..700 {
            let b = Buttons {
                l: f % 120 < 90,
                u: f % 120 == 100 || f % 120 == 108,
                ..Default::default()
            };
            let c = Buttons {
                r: f % 100 < 70,
                ..Default::default()
            };
            m.tick([b, c], &mut audio);
        }
        let want = m.world.checksum();
        let rec: Replay =
            serde_json::from_slice(&serde_json::to_vec(&m.record.take().unwrap()).unwrap())
                .unwrap();
        assert_eq!(rec.sim.zoom, Some([0.3, 2.5]));
        assert_eq!(rec.sim.movement.map(|m| m.air_jumps), Some(3));
        assert!(!rec.sim.online && rec.sim.match_wins.is_none());
        // watched later with the default options
        let now = Config::default();
        let play = |r: &Replay| {
            let mut p = Match::load(&r.setup(), &motif, &data, &now).expect("replay");
            p.playback = Some((r.inputs.clone(), 0));
            let mut audio = None;
            for _ in 0..r.inputs.len() {
                p.tick([Buttons::default(); 2], &mut audio);
            }
            p.world.checksum()
        };
        assert_eq!(play(&rec), want);
        let mut old = rec.clone();
        old.sim = Default::default();
        assert_ne!(play(&old), want, "an older replay follows the options");
    }
}
