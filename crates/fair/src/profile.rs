//! Static analysis of a character's CNS → [`FairnessProfile`].
//!
//! States are read like the engine loads them (cmd, cns, st, st0..st9, then
//! the common states; the first definition of a number wins). Everything is
//! conservative and rough by design: it has to be instant and it only has
//! to tell a fair fight from an obviously lopsided or cheated one.
//!
//! **Cheat flags** (static, see [`analyze_states`]):
//!
//! | code               | what                                                        | severity |
//! |--------------------|-------------------------------------------------------------|----------|
//! | `permanent_invuln` | NotHitBy / HitBy in state -2 / -3 with situation-free triggers | block |
//! | `regen`            | LifeAdd > 0 on itself in -2 / -3                            | block (always) / warn (gated) |
//! | `life_lock`        | LifeSet on itself in -2 / -3                                | block / warn |
//! | `no_ko`            | AssertSpecial noko / globalnoko in -2 / -3                  | block / warn |
//! | `invisible`        | AssertSpecial invisible in -2 / -3, always                  | block |
//! | `round_stall`      | AssertSpecial timerfreeze / roundnotover in -2 / -3, always | block |
//! | `no_push`          | PlayerPush value = 0 in -2 / -3, always                     | block |
//! | `pause_loop`       | Pause / SuperPause in -2 / -3, always                       | block |
//! | `long_pause`       | Pause / SuperPause time ≥ 600 anywhere                      | block |
//! | `target_life_set`  | TargetLifeSet anywhere                                      | block |
//! | `target_life_add`  | TargetLifeAdd ≤ -600                                        | block |
//! | `custom_state_kill`| a custom (p2stateno / TargetState) state LifeSets the opponent or takes ≥ 600 | block |
//! | `custom_state_vars`| a custom state writes the opponent's variables              | warn |
//! | `enemy_redirect`   | a modifying controller with `redirectid` aimed at an enemy  | block |
//! | `one_hit_kill`     | a single hit ≥ 700 damage                                   | block |
//! | `heavy_hit`        | a single hit ≥ 400 damage                                   | warn |
//! | `extreme_consts`   | life > 3000, attack / defence > 250                         | block |
//! | `high_consts`      | life > 1500, attack / defence > 150                         | warn |
//! | `long_invuln`      | NotHitBy ≥ 300 frames in a move                             | warn |
//! | `power_regen`      | PowerAdd > 0 in -2 / -3, always                             | warn |

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use rugen_cns::code::Op;
use rugen_cns::state::{Controller, StateDef};
use rugen_formats::def::Def;
use rugen_formats::text::decode;
use rugen_proto::{CheatFlag, FAIR_VERSION, FairnessProfile, Severity};

use crate::files::{ContentFiles, Kind, resolve};
use crate::{Error, Result};

/// Triggers that do not narrow a controller down to a situation: a
/// controller gated only by these runs (nearly) all the time.
const GENERIC_TRIGGERS: &[&str] = &[
    "Alive",
    "Life",
    "LifeMax",
    "Time",
    "GameTime",
    "RoundState",
    "RoundNo",
    "RoundsExisted",
    "Random",
    "Power",
    "PowerMax",
    "IsHelper",
    "MatchNo",
    "MatchOver",
    "TeamSide",
    "TicksPerSecond",
    "Win",
    "Lose",
    "Facing",
    "Const",
    "P2Life",
    "NumEnemy",
    "AILevel",
    "PalNo",
    "Id",
    "NameIs",
    "DrawGame",
    "IsHomeTeam",
];

/// Analyse the character described by `files` (from [`crate::files::char_files`]).
pub fn analyze(files: &ContentFiles) -> Result<FairnessProfile> {
    if files.kind != Kind::Char {
        return Err(Error::Content("not a character".into()));
    }
    let def_entry = files
        .get(&files.def.to_ascii_lowercase())
        .ok_or_else(|| Error::Content("def missing from the file set".into()))?;
    let def = Def::parse(&decode(&std::fs::read(&def_entry.source)?));
    let root = def_entry.source.parent().unwrap_or(Path::new("."));
    let read = |name: &str| -> Option<String> {
        let canon = crate::files::canon(name);
        files
            .get(&canon)
            .map(|e| e.source.clone())
            .or_else(|| resolve(root, name))
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| decode(&b))
    };
    let f = def
        .section("files")
        .ok_or_else(|| Error::Content("no [Files]".into()))?;
    let get = |k: &str| f.raw(k).map(str::trim).filter(|s| !s.is_empty());
    let localcoord = def
        .section("info")
        .and_then(|i| i.f32s("localcoord"))
        .filter(|v| !v.is_empty() && v[0] > 0.0)
        .map_or(320.0, |v| v[0]);
    let cns_name = get("cns").or_else(|| get("st")).unwrap_or("");
    let cns_text = read(cns_name).unwrap_or_default();
    let mut texts: Vec<(String, String)> = Vec::new();
    if let Some(c) = get("cmd") {
        texts.push((c.to_string(), read(c).unwrap_or_default()));
    }
    let mut state_files: Vec<String> = Vec::new();
    for key in ["cns", "st"] {
        if let Some(n) = get(key) {
            state_files.push(n.to_string());
        }
    }
    for i in 0..=9 {
        if let Some(n) = get(&format!("st{i}")) {
            state_files.push(n.to_string());
        }
    }
    state_files.dedup();
    for n in state_files {
        let t = if n == cns_name {
            cns_text.clone()
        } else {
            read(&n).unwrap_or_default()
        };
        texts.push((n, t));
    }
    let common = get("stcommon").unwrap_or("common1.cns");
    texts.push((common.to_string(), read(common).unwrap_or_default()));
    let consts = Consts::from_cns(&Def::parse(&cns_text), localcoord);
    Ok(analyze_texts(consts, &texts))
}

/// [`analyze`] on a def path (resolving files like [`crate::files::char_files`]).
pub fn analyze_def(def: &Path, data_dir: Option<&Path>) -> Result<FairnessProfile> {
    analyze(&crate::files::char_files(def, data_dir)?)
}

/// The constants the profile reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Consts {
    pub life: i32,
    pub power: i32,
    pub attack: i32,
    pub defence: i32,
    pub walk_fwd: f32,
    pub run_fwd: f32,
    pub jump_y: f32,
}

impl Default for Consts {
    fn default() -> Self {
        Self {
            life: 1000,
            power: 3000,
            attack: 100,
            defence: 100,
            walk_fwd: 2.4,
            run_fwd: 4.6,
            jump_y: -8.4,
        }
    }
}

impl Consts {
    pub fn from_cns(cns: &Def, localcoord_w: f32) -> Self {
        let d = Self::default();
        let data = cns.section("data");
        let vel = cns.section("velocity");
        let int = |k: &str, dflt: i32| data.and_then(|s| s.i32(k)).unwrap_or(dflt);
        let scale = 320.0 / localcoord_w.max(1.0);
        let v = |k: &str, i: usize, dflt: f32| {
            vel.and_then(|s| s.f32s(k))
                .and_then(|v| v.get(i).copied())
                .map_or(dflt, |x| x * scale)
        };
        Self {
            life: int("life", d.life),
            power: int("power", d.power),
            attack: int("attack", d.attack),
            defence: int("defence", d.defence),
            walk_fwd: v("walk.fwd", 0, d.walk_fwd),
            run_fwd: v("run.fwd", 0, d.run_fwd),
            jump_y: v("jump.neu", 1, d.jump_y),
        }
    }
}

/// Analyse state texts `(file name, text)` in load order.
pub fn analyze_texts(consts: Consts, texts: &[(String, String)]) -> FairnessProfile {
    let mut states: BTreeMap<i32, StateDef> = BTreeMap::new();
    for (name, text) in texts {
        for s in rugen_cns::state::parse_states(name, text).states {
            states.entry(s.no).or_insert(s);
        }
    }
    analyze_states(consts, &states)
}

/// Every trigger name (Debug name of the variant) a controller looks at,
/// plus `Var` for variables and `Redirect:<kind>` for redirects.
fn trigger_names(c: &Controller) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for code in c.triggerall.iter().chain(c.triggers.iter().flatten()) {
        for op in &code.ops {
            match op {
                Op::Trigger(t) => {
                    let s = format!("{t:?}");
                    let name: String = s.chars().take_while(|c| c.is_alphanumeric()).collect();
                    out.insert(name);
                }
                Op::GetVar(_) | Op::SetVar(_) => {
                    out.insert("Var".into());
                }
                Op::Redirect { kind, .. } => {
                    out.insert(format!("Redirect:{kind:?}"));
                }
                _ => {}
            }
        }
    }
    out
}

/// Can this controller fire? `false` when a trigger group is constant false
/// everywhere (e.g. `trigger1 = 0`) or there is no trigger at all.
fn can_fire(c: &Controller) -> bool {
    let konst = |code: &rugen_cns::Code| code.as_const().map(|v| v.to_b());
    if c.triggerall.iter().any(|t| konst(t) == Some(false)) {
        return false;
    }
    c.triggers
        .iter()
        .any(|g| !g.iter().any(|t| konst(t) == Some(false)))
}

/// Fires (nearly) every frame: only generic triggers, no variables, no
/// situation (state, move type, animation, commands ...).
fn unconditional(c: &Controller) -> bool {
    can_fire(c)
        && trigger_names(c)
            .iter()
            .all(|n| GENERIC_TRIGGERS.contains(&n.as_str()))
}

/// First number of a parameter: the constant value of its first expression,
/// else the first numeric literal in the text.
fn num(c: &Controller, key: &str) -> Option<f32> {
    let raw = c.param(key)?;
    let first = raw.split(',').next().unwrap_or("").trim();
    if let Ok(code) = rugen_cns::compile(first)
        && let Some(v) = code.as_const()
    {
        return Some(v.to_f());
    }
    literal(first)
}

fn literal(s: &str) -> Option<f32> {
    let mut start = None;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            let mut b = i;
            if i > 0 && s.as_bytes()[i - 1] == b'-' {
                b = i - 1;
            }
            start = Some(b);
            break;
        }
    }
    let rest = &s[start?..];
    let end = rest
        .char_indices()
        .skip(1)
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].parse().ok()
}

fn konst_i(c: &Controller, key: &str) -> Option<i32> {
    let raw = c.param(key)?;
    rugen_cns::compile(raw.split(',').next()?.trim())
        .ok()?
        .as_const()
        .map(|v| v.to_i())
}

fn params_lower(c: &Controller, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|k| c.param(k))
        .collect::<Vec<_>>()
        .join(",")
        .to_ascii_lowercase()
}

fn is_throw(c: &Controller) -> bool {
    c.param("p1stateno").is_some() || c.param("p2stateno").is_some() || {
        let attr = params_lower(c, &["attr"]);
        attr.contains("nt") || attr.contains("st") || attr.contains("ht")
    }
}

const MODIFYING: &[&str] = &[
    "lifeadd",
    "lifeset",
    "varset",
    "varadd",
    "varrangeset",
    "parentvarset",
    "parentvaradd",
    "selfstate",
    "changestate",
    "posset",
    "posadd",
    "poweradd",
    "powerset",
    "nothitby",
    "hitby",
    "assertspecial",
    "targetlifeadd",
    "targetstate",
];

struct Flags(Vec<CheatFlag>);

impl Flags {
    fn add(&mut self, code: &str, severity: Severity, detail: String) {
        if let Some(f) = self.0.iter_mut().find(|f| f.code == code) {
            if severity == Severity::Block && f.severity == Severity::Warn {
                f.severity = Severity::Block;
                f.detail = detail;
            }
            return;
        }
        self.0.push(CheatFlag {
            code: code.into(),
            severity,
            detail,
        });
    }
}

/// The analysis proper, on parsed states.
pub fn analyze_states(consts: Consts, states: &BTreeMap<i32, StateDef>) -> FairnessProfile {
    let mut flags = Flags(Vec::new());
    let mut p = FairnessProfile {
        version: FAIR_VERSION,
        life: consts.life,
        power_max: consts.power,
        attack: consts.attack,
        defence: consts.defence,
        walk_fwd: consts.walk_fwd,
        run_fwd: consts.run_fwd,
        jump_y: consts.jump_y,
        states: states.keys().filter(|&&n| n >= 0).count() as u32,
        ..Default::default()
    };

    // constants
    let c = consts;
    if c.life > 3000 || c.life < 1 || c.attack > 250 || c.defence > 250 {
        flags.add(
            "extreme_consts",
            Severity::Block,
            format!("life {} attack {} defence {}", c.life, c.attack, c.defence),
        );
    } else if c.life > 1500 || c.attack > 150 || c.defence > 150 {
        flags.add(
            "high_consts",
            Severity::Warn,
            format!("life {} attack {} defence {}", c.life, c.attack, c.defence),
        );
    }

    // custom states: run by the opponent with our state data
    let mut custom: BTreeSet<i32> = BTreeSet::new();
    for s in states.values() {
        for ctl in &s.controllers {
            match ctl.kind.as_str() {
                "hitdef" => {
                    if let Some(v) = konst_i(ctl, "p2stateno") {
                        custom.insert(v);
                    }
                }
                "targetstate" => {
                    if let Some(v) = konst_i(ctl, "value") {
                        custom.insert(v);
                    }
                }
                _ => {}
            }
        }
    }
    // helper states: helpers are often meant to be unhittable
    let helper_states: BTreeSet<i32> = states
        .values()
        .flat_map(|s| s.controllers.iter())
        .filter(|c| c.kind == "helper")
        .filter_map(|c| konst_i(c, "stateno"))
        .collect();
    // damage the opponent takes inside a custom state (throws)
    let mut custom_damage: HashMap<i32, i32> = HashMap::new();
    for &no in &custom {
        let Some(s) = states.get(&no) else { continue };
        for ctl in s.controllers.iter().filter(|c| can_fire(c)) {
            match ctl.kind.as_str() {
                "lifeset" => flags.add(
                    "custom_state_kill",
                    Severity::Block,
                    format!("custom state {no}: LifeSet on the opponent"),
                ),
                "lifeadd" => {
                    let v = num(ctl, "value").unwrap_or(0.0) as i32;
                    if v < 0 {
                        *custom_damage.entry(no).or_default() += -v;
                    }
                }
                "varset" | "varadd" | "varrangeset" | "parentvarset" | "parentvaradd" => flags.add(
                    "custom_state_vars",
                    Severity::Warn,
                    format!("custom state {no}: {} on the opponent", ctl.kind),
                ),
                _ => {}
            }
        }
    }
    for (&no, &d) in &custom_damage {
        if d >= 600 {
            flags.add(
                "custom_state_kill",
                Severity::Block,
                format!("custom state {no}: takes {d} life"),
            );
        }
    }

    let mut supers: BTreeMap<i32, i32> = BTreeMap::new();
    let mut state_damage: HashMap<i32, i32> = HashMap::new();
    let mut edges: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut cancels: BTreeSet<i32> = BTreeSet::new();

    for s in states.values() {
        let no = s.no;
        let always_state = no == -2 || no == -3;
        if no >= 0
            && let Some(pa) = s.poweradd.as_ref().and_then(|c| c.as_const())
            && pa.to_i() <= -500
        {
            supers.insert(no, -pa.to_i());
        }
        let is_custom = custom.contains(&no);
        for ctl in &s.controllers {
            if !can_fire(ctl) {
                continue;
            }
            let kind = ctl.kind.as_str();
            let always = always_state && unconditional(ctl);
            let where_ = format!("state {no}: {}", ctl.kind);
            // enemy-aimed redirect of a modifying controller (IKEMEN `redirectid`)
            if let Some(r) = ctl.param("redirectid")
                && MODIFYING.contains(&kind)
            {
                let r = r.to_ascii_lowercase();
                let enemy = ["enemy", "p2", "target", "playerid"]
                    .iter()
                    .any(|w| r.contains(w));
                flags.add(
                    "enemy_redirect",
                    if enemy {
                        Severity::Block
                    } else {
                        Severity::Warn
                    },
                    format!("{where_} redirected to {r}"),
                );
            }
            match kind {
                "hitdef" => {
                    p.hitdefs += 1;
                    let dmg = num(ctl, "damage").unwrap_or(0.0).max(0.0) as i32;
                    let throw = is_throw(ctl);
                    let mut hit = dmg;
                    if throw {
                        p.throws += 1;
                        if let Some(t) = konst_i(ctl, "p2stateno") {
                            hit += custom_damage.get(&t).copied().unwrap_or(0);
                        }
                    } else if params_lower(ctl, &["guardflag"]).trim().is_empty() {
                        p.unblockable += 1;
                    }
                    p.max_hit_damage = p.max_hit_damage.max(hit);
                    *state_damage.entry(no).or_default() += hit;
                    if hit >= 700 {
                        flags.add(
                            "one_hit_kill",
                            Severity::Block,
                            format!("{where_} damage {hit}"),
                        );
                    } else if hit >= 400 {
                        flags.add(
                            "heavy_hit",
                            Severity::Warn,
                            format!("{where_} damage {hit}"),
                        );
                    }
                }
                "projectile" => {
                    p.projectiles += 1;
                    let dmg = num(ctl, "damage").unwrap_or(0.0).max(0.0) as i32;
                    p.max_hit_damage = p.max_hit_damage.max(dmg);
                    *state_damage.entry(no).or_default() += dmg;
                }
                "helper" => p.helpers += 1,
                "changestate" | "selfstate" => {
                    if let Some(v) = konst_i(ctl, "value") {
                        if no == -1 {
                            let names = trigger_names(ctl);
                            if names.iter().any(|n| {
                                matches!(n.as_str(), "MoveContact" | "MoveHit" | "MoveGuarded")
                            }) {
                                cancels.insert(v);
                            }
                        } else if no >= 0 {
                            edges.entry(no).or_default().push(v);
                        }
                    }
                }
                "poweradd" if no >= 0 => {
                    if let Some(v) = konst_i(ctl, "value")
                        && v <= -500
                    {
                        supers.entry(no).or_insert(-v);
                    }
                }
                "poweradd" if always && num(ctl, "value").unwrap_or(0.0) > 0.0 => flags.add(
                    "power_regen",
                    Severity::Warn,
                    format!("{where_}: power every frame"),
                ),
                "targetlifeset" => flags.add(
                    "target_life_set",
                    Severity::Block,
                    format!("{where_}: sets the opponent's life"),
                ),
                "targetlifeadd" => {
                    let v = num(ctl, "value").unwrap_or(0.0) as i32;
                    if v <= -600 {
                        flags.add(
                            "target_life_add",
                            Severity::Block,
                            format!("{where_} value {v}"),
                        );
                    }
                    if v < 0 {
                        p.max_hit_damage = p.max_hit_damage.max(-v);
                        *state_damage.entry(no).or_default() += -v;
                    }
                }
                "pause" | "superpause" => {
                    let t = num(ctl, "time").unwrap_or(30.0);
                    if t >= 600.0 {
                        flags.add("long_pause", Severity::Block, format!("{where_} time {t}"));
                    }
                    if always {
                        flags.add(
                            "pause_loop",
                            Severity::Block,
                            format!("{where_} every frame"),
                        );
                    }
                }
                "nothitby" | "hitby" => {
                    let t = num(ctl, "time").unwrap_or(1.0);
                    if always {
                        flags.add(
                            "permanent_invuln",
                            Severity::Block,
                            format!("{where_} every frame"),
                        );
                    } else if no >= 0 && !is_custom && !helper_states.contains(&no) {
                        let full = params_lower(ctl, &["value", "value2"]).contains("sca");
                        if full || kind == "nothitby" {
                            p.invuln_frames = p.invuln_frames.max(t.clamp(0.0, 9999.0) as u32);
                        }
                        if t >= 300.0 {
                            flags.add("long_invuln", Severity::Warn, format!("{where_} time {t}"));
                        }
                    }
                }
                "lifeadd" | "lifeset" if always_state => {
                    let v = num(ctl, "value").unwrap_or(0.0);
                    let (code, what) = if kind == "lifeadd" {
                        if v <= 0.0 {
                            continue;
                        }
                        ("regen", "regenerates life")
                    } else {
                        ("life_lock", "sets its own life")
                    };
                    flags.add(
                        code,
                        if always {
                            Severity::Block
                        } else {
                            Severity::Warn
                        },
                        format!("{where_}: {what}"),
                    );
                }
                "assertspecial" if always_state => {
                    let fl = params_lower(ctl, &["flag", "flag2", "flag3"]);
                    let has_flag = |f: &str| fl.split([',', ' ', '\t']).any(|t| t.trim() == f);
                    let sev = if always {
                        Severity::Block
                    } else {
                        Severity::Warn
                    };
                    if has_flag("noko") || has_flag("globalnoko") {
                        flags.add("no_ko", sev, format!("{where_} noko"));
                    }
                    if always && has_flag("invisible") {
                        flags.add("invisible", Severity::Block, format!("{where_} invisible"));
                    }
                    if always && (has_flag("timerfreeze") || has_flag("roundnotover")) {
                        flags.add("round_stall", Severity::Block, format!("{where_} {fl}"));
                    }
                }
                "playerpush" if always && num(ctl, "value") == Some(0.0) => flags.add(
                    "no_push",
                    Severity::Block,
                    format!("{where_} value 0 every frame"),
                ),
                _ => {}
            }
        }
    }

    p.supers = supers.len() as u32;
    p.min_super_cost = supers.values().copied().min().unwrap_or(0);
    p.est_combo_damage = combo_estimate(&state_damage, &edges, &cancels).max(p.max_hit_damage);
    p.flags = flags.0;
    score(&mut p);
    p
}

/// Best damage along a chain of at most 4 attack states: follow-ups by
/// ChangeState, and hit-confirm cancels from the command state (only into
/// moves that hit at least as hard, like light → heavy → special → super).
fn combo_estimate(
    damage: &HashMap<i32, i32>,
    edges: &HashMap<i32, Vec<i32>>,
    cancels: &BTreeSet<i32>,
) -> i32 {
    fn best(
        s: i32,
        depth: u32,
        damage: &HashMap<i32, i32>,
        edges: &HashMap<i32, Vec<i32>>,
        cancels: &BTreeSet<i32>,
        memo: &mut HashMap<(i32, u32), i32>,
    ) -> i32 {
        let d = damage.get(&s).copied().unwrap_or(0);
        if depth == 0 {
            return d;
        }
        if let Some(&m) = memo.get(&(s, depth)) {
            return m;
        }
        memo.insert((s, depth), d);
        let mut next = 0;
        for &t in edges.get(&s).into_iter().flatten() {
            if t != s {
                next = next.max(best(t, depth - 1, damage, edges, cancels, memo));
            }
        }
        if d > 0 {
            for &t in cancels {
                let dt = damage.get(&t).copied().unwrap_or(0);
                if t != s && dt >= d {
                    next = next.max(best(t, depth - 1, damage, edges, cancels, memo));
                }
            }
        }
        let v = d + next;
        memo.insert((s, depth), v);
        v
    }
    let mut memo = HashMap::new();
    damage
        .keys()
        .map(|&s| best(s, 3, damage, edges, cancels, &mut memo))
        .max()
        .unwrap_or(0)
}

/// Raw score of Kung Fu Man (reference/mugen-1.1b1), so it lands at 100.
const KFM_RAW: f32 = 1.204;

/// Power score and human-readable reasons.
pub fn score(p: &mut FairnessProfile) {
    let durability = (p.life.max(1) as f32 / 1000.0) * (p.defence.max(1) as f32 / 100.0);
    let combo = (p.est_combo_damage.max(p.max_hit_damage).max(40) as f32 / 300.0).clamp(0.2, 6.0);
    let offense = (p.attack.max(1) as f32 / 100.0) * libm::sqrtf(combo);
    let mobility = libm::powf(
        ((p.walk_fwd.abs() / 2.4 + p.run_fwd.abs() / 4.6 + p.jump_y.abs() / 8.4) / 3.0)
            .clamp(0.5, 2.5),
        0.25,
    );
    let tools = 1.0
        + 0.04 * p.projectiles.min(5) as f32
        + 0.03 * p.supers.min(6) as f32
        + 0.05 * p.unblockable.min(4) as f32
        + 0.02 * p.throws.min(3) as f32
        + p.invuln_frames.min(60) as f32 / 300.0;
    let warns = p
        .flags
        .iter()
        .filter(|f| f.severity == Severity::Warn)
        .count() as f32;
    let raw = libm::sqrtf(durability * offense) * mobility * tools * (1.0 + 0.04 * warns.min(5.0));
    p.power_score = libm::roundf(raw / KFM_RAW * 100.0 * 10.0) / 10.0;
    let mut r = vec![
        format!(
            "life {} / attack {} / defence {}",
            p.life, p.attack, p.defence
        ),
        format!(
            "best hit {} / est. combo {}",
            p.max_hit_damage, p.est_combo_damage
        ),
        format!(
            "{} projectiles, {} supers, {} throws, {} unblockable",
            p.projectiles, p.supers, p.throws, p.unblockable
        ),
    ];
    if p.invuln_frames > 0 {
        r.push(format!(
            "invulnerable moves up to {} frames",
            p.invuln_frames
        ));
    }
    for f in &p.flags {
        r.push(format!(
            "{}: {}",
            match f.severity {
                Severity::Block => "CHEAT",
                Severity::Warn => "warning",
            },
            f.detail
        ));
    }
    p.reasons = r;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(cns: &str) -> FairnessProfile {
        let texts = vec![("t.cns".to_string(), cns.to_string())];
        analyze_texts(Consts::default(), &texts)
    }

    fn has(p: &FairnessProfile, code: &str, sev: Severity) -> bool {
        p.flags.iter().any(|f| f.code == code && f.severity == sev)
    }

    const ATTACK: &str = "
[Statedef 200]
type = S
movetype = A
[State 200, hit]
type = HitDef
trigger1 = AnimElem = 2
damage = 30, 2
guardflag = MA
[State 200, end]
type = ChangeState
trigger1 = AnimTime = 0
value = 0
";

    #[test]
    fn clean_char_has_no_flags() {
        let p = profile(ATTACK);
        assert!(p.flags.is_empty(), "{:?}", p.flags);
        assert_eq!(p.hitdefs, 1);
        assert_eq!(p.max_hit_damage, 30);
        assert!(!p.blocked());
    }

    #[test]
    fn regen_in_minus_two() {
        let p = profile(
            "[Statedef -2]
[State -2, regen]
type = LifeAdd
trigger1 = Life < LifeMax
value = 5
",
        );
        assert!(has(&p, "regen", Severity::Block), "{:?}", p.flags);
        assert!(p.blocked());
        // gated by a situation: only a warning
        let p = profile(
            "[Statedef -2]
[State -2, regen]
type = LifeAdd
trigger1 = StateNo = 5120
value = 5
",
        );
        assert!(has(&p, "regen", Severity::Warn), "{:?}", p.flags);
        assert!(!p.blocked());
    }

    #[test]
    fn permanent_nothitby() {
        let p = profile(
            "[Statedef -3]
[State -3, god]
type = NotHitBy
trigger1 = 1
value = SCA
time = 1
",
        );
        assert!(
            has(&p, "permanent_invuln", Severity::Block),
            "{:?}",
            p.flags
        );
        // wake-up invulnerability gated by state is fine
        let p = profile(
            "[Statedef -2]
[State -2, wake]
type = NotHitBy
trigger1 = StateNo = 5120
value = SCA
time = 2
",
        );
        assert!(!p.blocked(), "{:?}", p.flags);
    }

    #[test]
    fn target_life_set_and_custom_state_kill() {
        let p = profile(
            "[Statedef 800]
movetype = A
[State 800, kill]
type = TargetLifeSet
trigger1 = NumTarget
value = 0
",
        );
        assert!(has(&p, "target_life_set", Severity::Block));
        let p = profile(
            "[Statedef 810]
movetype = A
[State 810, throw]
type = HitDef
trigger1 = AnimElem = 2
attr = S, NT
damage = 0
p2stateno = 820
[Statedef 820]
type = A
movetype = H
[State 820, ko]
type = LifeSet
trigger1 = Time = 0
value = 0
",
        );
        assert!(
            has(&p, "custom_state_kill", Severity::Block),
            "{:?}",
            p.flags
        );
        assert_eq!(p.throws, 1);
    }

    #[test]
    fn enemy_redirect_extreme_consts_and_pauses() {
        let p = profile(
            "[Statedef 900]
[State 900, x]
type = VarSet
trigger1 = Time = 0
redirectid = enemy, id
v = 1
value = 9
[Statedef -2]
[State -2, p]
type = Pause
trigger1 = 1
time = 2
",
        );
        assert!(has(&p, "enemy_redirect", Severity::Block));
        assert!(has(&p, "pause_loop", Severity::Block));
        let consts = Consts {
            attack: 400,
            ..Consts::default()
        };
        let p = analyze_texts(consts, &[("t".into(), ATTACK.into())]);
        assert!(has(&p, "extreme_consts", Severity::Block));
    }

    #[test]
    fn combos_follow_cancels_upward() {
        let p = profile(
            "[Statedef -1]
[State -1, cancel]
type = ChangeState
trigger1 = MoveContact
trigger1 = Command = \"x\"
value = 210
[State -1, cancel2]
type = ChangeState
trigger1 = MoveHit
value = 1000
[Statedef 200]
movetype = A
[State 200, h]
type = HitDef
trigger1 = 1
damage = 20
guardflag = MA
[Statedef 210]
movetype = A
[State 210, h]
type = HitDef
trigger1 = 1
damage = 50
guardflag = MA
[Statedef 1000]
movetype = A
poweradd = -1000
[State 1000, h]
type = HitDef
trigger1 = 1
damage = 200
guardflag = MA
",
        );
        assert_eq!(p.est_combo_damage, 270);
        assert_eq!(p.supers, 1);
        assert_eq!(p.min_super_cost, 1000);
    }

    #[test]
    fn literals() {
        assert_eq!(literal("ifelse(var(1), 30, 20)"), Some(1.0));
        assert_eq!(literal("-45 * 2"), Some(-45.0));
        assert_eq!(literal("abc"), None);
    }
}
