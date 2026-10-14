//! The fighter debugger: `rugen debug <fighter> [opponent]`.
//!
//! A local match with a panel beside it (`ui/debug.toml`, ink and dojo):
//! pause, step one frame forward or back, slow motion, collision boxes, and
//! for the selected player (click it, or Tab) its state, animation, the
//! main values, its variables (the ones that changed this frame in red)
//! and every controller the state machine looked at this frame: run or
//! not, and why (the trigger group that held, or the trigger that failed
//! with the value of its left side).
//!
//! The simulation records what it does into `World::trace` (never read by
//! the simulation, never checksummed); trigger texts come from the
//! fighter's own files, found by each controller's file and line. Stepping
//! back restores a copy of the world kept after every frame (the last
//! [`HISTORY`] frames).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use rugen_sim::World;
use rugen_sim::world::CtrlOutcome;
use serde_json::{Value, json};

/// Frames kept for STEP BACK (10 s).
pub const HISTORY: usize = 600;

/// Slow motion steps: 1, 1/2, 1/4, 1/8.
const SPEEDS: [u32; 4] = [1, 2, 4, 8];

/// Rows of the CONTROLLERS THIS FRAME list at most.
const MAX_ROWS: usize = 48;

/// Variable cells shown at most (the last one says how many more).
const MAX_VARS: usize = 16;

pub struct Debugger {
    /// The world after each recent frame, oldest first.
    history: VecDeque<World>,
    speed: usize,
    gate: u32,
    /// The inspected player (an index of `World::players`).
    pub selected: usize,
    data_dir: PathBuf,
    /// Source files by path, as lines.
    sources: HashMap<PathBuf, Option<Vec<String>>>,
}

/// One controller as its file has it.
#[derive(Clone, Debug, Default, PartialEq)]
struct CtrlSrc {
    /// `type` as written (`ChangeState`).
    kind: String,
    triggerall: Vec<String>,
    /// `trigger1`, `trigger2`... lines, in order.
    groups: Vec<Vec<String>>,
}

impl Debugger {
    /// Start tracing `world` (the debugger's own match).
    pub fn new(world: &mut World, data_dir: &Path) -> Self {
        world.trace = Some(Vec::new());
        let mut d = Self {
            history: VecDeque::new(),
            speed: 0,
            gate: 0,
            selected: 0,
            data_dir: data_dir.to_path_buf(),
            sources: HashMap::new(),
        };
        d.record(world);
        d
    }

    /// Keep a copy of the world after a frame.
    pub fn record(&mut self, world: &World) {
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(world.clone());
    }

    /// STEP BACK: the world one frame earlier, when one is kept.
    pub fn back(&mut self) -> Option<World> {
        if self.history.len() < 2 {
            return None;
        }
        self.history.pop_back();
        self.history.back().cloned()
    }

    /// Frames that STEP BACK can still undo.
    pub fn depth(&self) -> usize {
        self.history.len().saturating_sub(1)
    }

    /// Slow motion: whether this tick advances the match.
    pub fn gate(&mut self) -> bool {
        self.gate = self.gate.wrapping_add(1);
        self.gate % SPEEDS[self.speed] == 0
    }

    /// The next slow motion step (1 → 1/2 → 1/4 → 1/8 → 1).
    pub fn cycle_speed(&mut self) {
        self.speed = (self.speed + 1) % SPEEDS.len();
        self.gate = 0;
    }

    pub fn speed_label(&self) -> String {
        match SPEEDS[self.speed] {
            1 => "SPEED 1/1".into(),
            n => format!("SPEED 1/{n}"),
        }
    }

    /// Tab: the next player still in the match (helpers included).
    pub fn select_next(&mut self, world: &World) {
        let n = world.players.len();
        for k in 1..=n {
            let i = (self.selected + k) % n;
            if !world.players[i].removed {
                self.selected = i;
                return;
            }
        }
    }

    /// The selected player, or player 1 when it is gone.
    fn current(&mut self, world: &World) -> usize {
        if world.players.get(self.selected).is_none_or(|p| p.removed) {
            self.selected = 0;
        }
        self.selected
    }

    /// `P1 ROOT`, `P1 HELPER 1201`.
    pub fn who(world: &World, i: usize) -> String {
        let p = &world.players[i];
        let side = world.players.get(p.root).map_or(p.team, |r| r.team) + 1;
        if p.helper_index == 0 {
            format!("P{side} ROOT")
        } else {
            format!("P{side} HELPER {}", p.helper_id)
        }
    }

    fn lines(&mut self, path: &Path) -> Option<&Vec<String>> {
        self.sources
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                rugen_formats::vfs::read(path)
                    .ok()
                    .map(|b| rugen_formats::text::decode(&b))
                    .map(|t| t.lines().map(str::to_string).collect())
            })
            .as_ref()
    }

    /// The file a state of `data` came from.
    fn state_file(&self, world: &World, data: usize, state: i32) -> Option<PathBuf> {
        let d = world.data.get(data)?;
        let st = d.states.get(&state)?;
        rugen_sim::chardata::resolve_in(&[d.dir.as_path(), self.data_dir.as_path()], &st.file).ok()
    }

    /// Controller `index` of `state` as written in its file.
    fn ctrl_src(
        &mut self,
        world: &World,
        data: usize,
        state: i32,
        index: usize,
    ) -> Option<CtrlSrc> {
        let line = world
            .data
            .get(data)?
            .states
            .get(&state)?
            .ctrls
            .get(index)?
            .line;
        let path = self.state_file(world, data, state)?;
        let lines = self.lines(&path)?;
        Some(parse_ctrl(lines, line))
    }

    /// `KFM.CNS · STAND`: the state's file and the comment that names it.
    fn state_where(&mut self, world: &World, data: usize, state: i32) -> String {
        let Some(st) = world.data.get(data).and_then(|d| d.states.get(&state)) else {
            return "NO SUCH STATE".into();
        };
        let file = Path::new(&st.file.replace('\\', "/"))
            .file_name()
            .map(|f| f.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let line = st.def.line;
        let name = self
            .state_file(world, data, state)
            .and_then(|p| self.lines(&p).map(|l| state_name(l, line)))
            .unwrap_or_default();
        if name.is_empty() {
            file
        } else {
            format!("{file} · {}", name.to_uppercase())
        }
    }

    /// Everything `ui/debug.toml` binds to.
    pub fn data(&mut self, world: &World, paused: bool, boxes: bool) -> Value {
        let i = self.current(world);
        let p = &world.players[i];
        let name = world
            .data
            .get(p.data)
            .map(|d| d.display_name.to_uppercase())
            .unwrap_or_default();
        let anim = world.data[p.anim_owner].air.get(p.anim.action);
        let kv = [
            format!("anim {}", p.anim.action),
            format!(
                "elem {}/{}",
                p.anim.elem + 1,
                anim.map_or(0, |a| a.frames.len())
            ),
            format!("time {}", p.state_time),
            format!("ctrl {}", u8::from(p.ctrl)),
            format!("type {:?}", p.state_type),
            format!("movetype {:?}", p.move_type),
            format!("physics {:?}", p.physics),
            format!("power {}", p.power),
            format!("pos {:.0},{:.0}", p.pos[0], p.pos[1]),
            format!("vel {},{}", num(p.vel[0]), num(p.vel[1])),
            format!("facing {}", p.facing as i32),
            format!("life {}", p.life),
        ];
        let before = self
            .history
            .len()
            .checked_sub(2)
            .and_then(|k| self.history.get(k))
            .and_then(|w| w.players.get(i))
            .filter(|q| q.id == p.id);
        let vars = vars_json(p, before);
        let ctrls = self.ctrls_json(world, i);
        let state = p.state_no;
        let owner = p.state_owner;
        json!({
            "title": format!("RUGEN DEBUG · {name}"),
            "status": format!("{} · FRAME {}", if paused { "PAUSED" } else { "RUNNING" }, world.frame),
            "paused": paused,
            "running": !paused,
            "play": if paused { "PLAY" } else { "PAUSE" },
            "speed": self.speed_label(),
            "boxes": if boxes { "BOXES ON" } else { "BOXES OFF" },
            "back": format!("STEP BACK ({})", self.depth()),
            "can_back": self.depth() > 0,
            "who": format!("SELECTED · {}", Self::who(world, i)),
            "state": format!("STATE {state}"),
            "where": self.state_where(world, owner, state),
            "kv": kv.iter().map(|t| json!({ "t": t })).collect::<Vec<_>>(),
            "vars": vars,
            "has_vars": vars.as_array().is_some_and(|v| !v.is_empty()),
            "no_vars": vars.as_array().is_none_or(Vec::is_empty),
            "ctrls_title": format!("CONTROLLERS THIS FRAME · {}", ctrls.len()),
            "has_ctrls": !ctrls.is_empty(),
            "no_ctrls": ctrls.is_empty(),
            "ctrls": ctrls,
            "keys": "SPACE PLAY · . STEP · , BACK · TAB PLAYER · F2 BOXES · F4 SPEED",
        })
    }

    /// The selected player's controllers this frame: the states it ran
    /// (every controller), and the special states' (-1, -2, -3) that fired.
    fn ctrls_json(&mut self, world: &World, i: usize) -> Vec<Value> {
        let Some(trace) = world.trace.as_ref() else {
            return Vec::new();
        };
        let mine: Vec<_> = trace
            .iter()
            .filter(|t| t.player == i)
            .filter(|t| t.state >= 0 || matches!(t.outcome, CtrlOutcome::Fired { .. }))
            .copied()
            .collect();
        let last_fired = mine
            .iter()
            .rposition(|t| matches!(t.outcome, CtrlOutcome::Fired { .. }));
        // the values behind failed triggers come from a copy (Random moves the rng)
        let mut probe: Option<World> = None;
        let mut out = Vec::new();
        for (k, t) in mine.iter().enumerate().take(MAX_ROWS) {
            let src = self
                .ctrl_src(world, t.data, t.state, t.index as usize)
                .unwrap_or_default();
            let kind = if src.kind.is_empty() {
                world
                    .data
                    .get(t.data)
                    .and_then(|d| d.states.get(&t.state))
                    .and_then(|s| s.ctrls.get(t.index as usize))
                    .map(|c| c.kind.clone())
                    .unwrap_or_default()
            } else {
                src.kind.clone()
            };
            let prefix = if t.state < 0 {
                format!("[{}] ", t.state)
            } else {
                String::new()
            };
            let (ok, skip, why, note) = match t.outcome {
                CtrlOutcome::Fired { group } => (
                    true,
                    false,
                    src.groups
                        .get(group as usize)
                        .map(|g| g.join(" && "))
                        .unwrap_or_default(),
                    String::new(),
                ),
                CtrlOutcome::TriggerAll { line } | CtrlOutcome::NoGroup { line } => {
                    let all = matches!(t.outcome, CtrlOutcome::TriggerAll { .. });
                    let expr = if all {
                        src.triggerall.get(line as usize)
                    } else {
                        src.groups.first().and_then(|g| g.get(line as usize))
                    }
                    .cloned()
                    .unwrap_or_default();
                    let probe = probe.get_or_insert_with(|| {
                        let mut w = world.clone();
                        w.trace = None;
                        w
                    });
                    let note = lhs_value(probe, i, &expr)
                        .map(|(lhs, v)| format!("({lhs} = {v})"))
                        .unwrap_or_default();
                    let why = if all {
                        format!("triggerall {expr}")
                    } else {
                        expr
                    };
                    (false, false, why, note)
                }
                CtrlOutcome::Persistent => {
                    (false, true, "skipped: persistent".into(), String::new())
                }
                CtrlOutcome::HitPause => (false, true, "skipped: hit pause".into(), String::new()),
            };
            out.push(json!({
                "ok": ok,
                "no": !ok,
                "skip": skip,
                "hl": Some(k) == last_fired,
                "plain": Some(k) != last_fired,
                "mark": if ok { "+" } else { "x" },
                "text": format!("{prefix}{kind} · {why}"),
                "note": note,
            }));
        }
        if mine.len() > MAX_ROWS {
            out.push(json!({
                "ok": false, "no": true, "skip": true, "hl": false, "plain": true, "mark": "",
                "text": format!("… {} more", mine.len() - MAX_ROWS), "note": "",
            }));
        }
        out
    }
}

/// `1.3`, `-0.25`, `0`: a float without trailing zeros.
fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The variable cells: every var, fvar and sysvar that is not zero or
/// changed since the frame before (changed ones flagged, kept first when
/// there are too many).
fn vars_json(p: &rugen_sim::player::Player, before: Option<&rugen_sim::player::Player>) -> Value {
    let mut cells: Vec<(String, bool)> = Vec::new();
    for (k, v) in p.vars.iter().enumerate() {
        let changed = before.is_some_and(|b| b.vars.get(k) != Some(v));
        if *v != 0 || changed {
            cells.push((format!("var({k})={v}"), changed));
        }
    }
    for (k, v) in p.fvars.iter().enumerate() {
        let changed =
            before.is_some_and(|b| b.fvars.get(k).is_none_or(|o| o.to_bits() != v.to_bits()));
        if *v != 0.0 || changed {
            cells.push((format!("fvar({k})={}", num(*v)), changed));
        }
    }
    for (k, v) in p.sysvars.iter().enumerate() {
        let changed = before.is_some_and(|b| b.sysvars.get(k) != Some(v));
        if *v != 0 || changed {
            cells.push((format!("sysvar({k})={v}"), changed));
        }
    }
    for (k, v) in p.sysfvars.iter().enumerate() {
        let changed =
            before.is_some_and(|b| b.sysfvars.get(k).is_none_or(|o| o.to_bits() != v.to_bits()));
        if *v != 0.0 || changed {
            cells.push((format!("sysfvar({k})={}", num(*v)), changed));
        }
    }
    if cells.len() > MAX_VARS {
        let more = cells.len() - (MAX_VARS - 1);
        // changed ones first, then the rest in order
        let (mut keep, rest): (Vec<_>, Vec<_>) = cells.into_iter().partition(|c| c.1);
        keep.extend(rest);
        keep.truncate(MAX_VARS - 1);
        keep.push((format!("+{more} more"), false));
        cells = keep;
    }
    Value::Array(
        cells
            .into_iter()
            .map(|(t, changed)| json!({ "t": t, "changed": changed, "same": !changed }))
            .collect(),
    )
}

/// Parse the controller whose `[State ...]` header is on (1-based) `line`.
fn parse_ctrl(lines: &[String], line: usize) -> CtrlSrc {
    let mut src = CtrlSrc::default();
    let mut groups: Vec<(u32, Vec<String>)> = Vec::new();
    for raw in lines.iter().skip(line) {
        let l = rugen_formats::text::strip_comment(raw);
        if l.starts_with('[') {
            break;
        }
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
        if k == "type" {
            src.kind = v;
        } else if k == "triggerall" {
            src.triggerall.push(v);
        } else if let Some(n) = k
            .strip_prefix("trigger")
            .and_then(|n| n.parse::<u32>().ok())
        {
            match groups.iter_mut().find(|(g, _)| *g == n) {
                Some((_, g)) => g.push(v),
                None => groups.push((n, vec![v])),
            }
        }
    }
    groups.sort_by_key(|(n, _)| *n);
    // trigger1, trigger2... without gaps, as the compiler keeps them
    for (want, (n, g)) in (1..).zip(groups) {
        if n != want {
            break;
        }
        src.groups.push(g);
    }
    src
}

/// The name of the state whose `[Statedef]` is on `line`: the comment on
/// that line, else a comment line right above it.
fn state_name(lines: &[String], line: usize) -> String {
    let comment = |l: &str| l.split_once(';').map(|(_, c)| c.trim().to_string());
    let here = lines.get(line.wrapping_sub(1)).and_then(|l| comment(l));
    let above = lines
        .get(line.wrapping_sub(2))
        .filter(|l| l.trim_start().starts_with(';'))
        .and_then(|l| comment(l));
    here.filter(|c| !c.is_empty())
        .or(above)
        .map(|c| {
            c.trim_matches(|ch: char| ch == '-' || ch == '=' || ch.is_whitespace())
                .to_string()
        })
        .unwrap_or_default()
}

/// For `var(3) > 20`: `("var(3)", "12")`, the left side of a top-level
/// comparison and its value now. `None` for anything else.
fn lhs_value(world: &mut World, i: usize, expr: &str) -> Option<(String, String)> {
    let lhs = compare_lhs(expr)?;
    let code = rugen_cns::compile(lhs).ok()?;
    let v = match world.debug_eval(i, &code) {
        rugen_cns::Value::Int(n) => n.to_string(),
        rugen_cns::Value::Float(f) => num(f),
        rugen_cns::Value::Bottom => "bottom".into(),
    };
    Some((lhs.to_string(), v))
}

/// The left side of the first comparison (`=`, `!=`, `<`, `>`, `<=`, `>=`)
/// outside brackets and quotes; `None` when there is none or it is joined
/// with `&&` / `||` to something else.
fn compare_lhs(expr: &str) -> Option<&str> {
    let b = expr.as_bytes();
    if expr.contains("&&") || expr.contains("||") {
        return None;
    }
    let (mut depth, mut quoted) = (0i32, false);
    for (k, &c) in b.iter().enumerate() {
        match c {
            b'"' => quoted = !quoted,
            _ if quoted => {}
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'=' | b'<' | b'>' | b'!' if depth == 0 => {
                if c == b'!' && b.get(k + 1) != Some(&b'=') {
                    continue;
                }
                let lhs = expr[..k].trim();
                return (!lhs.is_empty()).then_some(lhs);
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controllers_are_read_from_their_files() {
        let text = "[Statedef 200] ; Light punch\ntype = S\n\n[State 200, hit]\n\
                    type = HitDef ; the hit\ntriggerall = var(3) > 2\ntrigger1 = AnimElem = 3\n\
                    trigger1 = !NumHelper(1201)\ntrigger2 = Time = 9\ntrigger4 = 1\n\
                    damage = 20\n\n[State 200, end]\ntype = ChangeState\ntrigger1 = AnimTime = 0\n";
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let c = parse_ctrl(&lines, 4);
        assert_eq!(c.kind, "HitDef");
        assert_eq!(c.triggerall, ["var(3) > 2"]);
        assert_eq!(
            c.groups,
            [
                vec!["AnimElem = 3".to_string(), "!NumHelper(1201)".into()],
                vec!["Time = 9".into()]
            ],
            "trigger4 after a gap is dropped, as the compiler does"
        );
        assert_eq!(parse_ctrl(&lines, 13).kind, "ChangeState");
        assert_eq!(state_name(&lines, 1), "Light punch");
    }

    #[test]
    fn the_left_side_of_a_comparison() {
        assert_eq!(compare_lhs("var(3) > 20"), Some("var(3)"));
        assert_eq!(compare_lhs("AnimTime = 0"), Some("AnimTime"));
        assert_eq!(
            compare_lhs("ifelse(a = 1, 2, 3) != 4"),
            Some("ifelse(a = 1, 2, 3)")
        );
        assert_eq!(compare_lhs("command = \"a!=b\""), Some("command"));
        assert_eq!(compare_lhs("!NumHelper(1201)"), None);
        assert_eq!(compare_lhs("Time = 1 && Var(2)"), None);
        assert_eq!(compare_lhs("1"), None);
    }

    #[test]
    fn floats_read_short() {
        assert_eq!(num(1.30), "1.3");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(4.0), "4");
    }

    /// A real fighter: after a few frames the panel names its state and
    /// file, the controllers that ran with their triggers, and STEP BACK
    /// gives the frame before.
    #[test]
    fn the_panel_reads_a_running_match() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/mugen-1.1b1");
        let kfm = root.join("chars/kfm/kfm.def");
        if !kfm.exists() {
            return;
        }
        let data = root.join("data");
        let spec = rugen_sim::headless::MatchSpec {
            life: None,
            p1: &kfm,
            p2: &kfm,
            stage: None,
            data_dir: &data,
            motif: &data,
            zoom: rugen_sim::headless::ONLINE_ZOOM,
            movement: Default::default(),
            round_time: None,
            ai: [0, 0],
            fighter_size: Default::default(),
            seed: 1,
        };
        let mut w = rugen_sim::headless::build_world(&spec).unwrap();
        let mut d = Debugger::new(&mut w, &data);
        for _ in 0..200 {
            w.step(&[rugen_sim::Buttons::default(); 2]);
            d.record(&w);
        }
        let v = d.data(&w, true, false);
        assert_eq!(v["state"], "STATE 0");
        assert!(
            v["where"].as_str().unwrap().starts_with("COMMON1.CNS"),
            "{}",
            v["where"]
        );
        assert_eq!(v["who"], "SELECTED · P1 ROOT");
        let rows = v["ctrls"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert!(
            rows.iter()
                .any(|r| r["ok"] == true && r["text"].as_str().unwrap().contains("VarSet"))
        );
        let frame = w.frame;
        let back = d.back().unwrap();
        assert_eq!(back.frame, frame - 1);
        d.select_next(&w);
        assert_eq!(d.data(&w, true, false)["who"], "SELECTED · P2 ROOT");
    }
}
