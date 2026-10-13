//! Character facts and match stats for the menu screens: the move list shown
//! by the pause menu (read from the character's `.cmd`), victory quotes for
//! the result screen (`[Quotes]` in its `.cns`), and a match-stats observer
//! that only reads the simulation (never feeds anything back into it).

use std::path::{Path, PathBuf};

use rugen_sim::World;

/// Where a move goes in the move list (in display order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Super,
    Special,
    System,
    Movement,
    Combo,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Super => "SUPERS",
            Group::Special => "SPECIALS",
            Group::System => "SYSTEM",
            Group::Movement => "MOVEMENT",
            Group::Combo => "COMBOS",
        }
    }
}

/// A piece of a move's notation: arrows / words, or a button drawn as a key cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Button(String),
}

/// One move for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub name: String,
    /// The notation as plain text, e.g. `↓↘→ X` or `HOLD ↓ + A`.
    pub notation: String,
    /// The same notation in pieces (buttons apart for key caps).
    pub parts: Vec<Part>,
    pub group: Group,
    /// Power the move spends, e.g. `2 BARS` (supers only).
    pub cost: Option<String>,
}

/// Strip a `;` comment and surrounding blanks.
fn strip(line: &str) -> &str {
    line.split(';').next().unwrap_or("").trim()
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

/// `key = value` entries of the `[Files]` group of a character .def.
fn def_files(def: &Path) -> Vec<(String, String)> {
    let Ok(text) = read_lossy(def) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_files = false;
    for raw in text.lines() {
        let line = strip(raw);
        if line.starts_with('[') {
            in_files = line.eq_ignore_ascii_case("[files]");
            continue;
        }
        if in_files && let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_ascii_lowercase(), unquote(v)));
        }
    }
    out
}

/// A text file from disk or from inside a `.rgc` content file (characters
/// are loaded from `cache/rgc/chars/x.rgc/x.def`, never from a plain folder).
fn read_lossy(p: &Path) -> std::io::Result<String> {
    rugen_formats::vfs::read(p)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Resolve a file named in a .def relative to the character folder
/// (case-insensitive, `\` accepted, inside content files too); a bare file
/// name found anywhere in the folder is the last resort.
fn resolve(def: &Path, name: &str) -> Option<PathBuf> {
    let dir = def.parent()?;
    if let Some(p) = rugen_formats::vfs::resolve(dir, name) {
        return Some(p);
    }
    let base = name.replace('\\', "/");
    let want = base.rsplit('/').next()?;
    rugen_formats::vfs::read_dir(dir)
        .into_iter()
        .find(|(n, is_dir)| !is_dir && n.eq_ignore_ascii_case(want))
        .map(|(n, _)| dir.join(n))
}

/// The character's command file.
pub fn cmd_path(def: &Path) -> Option<PathBuf> {
    let files = def_files(def);
    let name = files.iter().find(|(k, _)| k == "cmd")?.1.clone();
    resolve(def, &name)
}

fn arrow(t: &str) -> Option<&'static str> {
    // directions are upper case, buttons lower case (`B` back, `b` button)
    Some(match t {
        "U" => "↑",
        "D" => "↓",
        "F" => "→",
        "B" => "←",
        "UF" => "↗",
        "DF" => "↘",
        "UB" => "↖",
        "DB" => "↙",
        _ => return None,
    })
}

/// Push a text piece, merged into a text piece before it.
fn push_text(parts: &mut Vec<Part>, t: &str) {
    if t.is_empty() {
        return;
    }
    if let Some(Part::Text(prev)) = parts.last_mut() {
        prev.push(' ');
        prev.push_str(t);
    } else {
        parts.push(Part::Text(t.to_string()));
    }
}

/// One MUGEN command string in pieces: `~D, DF, F, x` → `↓↘→` `[X]`;
/// `/$D` → `HOLD ↓`; `/s` → `HOLD` `[S]`; `a+b` → `[A]` `+` `[B]`.
pub fn command_parts(cmd: &str) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    let mut run = String::new();
    for tok in cmd.split(',') {
        let tok = tok.trim();
        let prefix: String = tok
            .chars()
            .take_while(|c| "~$/>".contains(*c) || c.is_ascii_digit())
            .collect();
        let core = tok[prefix.len()..].trim();
        if core.is_empty() {
            continue;
        }
        let hold = prefix.contains('/');
        let pieces: Vec<&str> = core.split('+').map(str::trim).collect();
        let dirs_only = pieces
            .iter()
            .all(|p| arrow(p.trim_start_matches(['~', '$', '/', '>'])).is_some());
        if dirs_only && pieces.len() == 1 && !hold {
            run.push_str(arrow(pieces[0].trim_start_matches(['~', '$', '/', '>'])).unwrap_or(""));
            continue;
        }
        push_text(&mut parts, &std::mem::take(&mut run));
        if hold {
            push_text(&mut parts, "HOLD");
        }
        for (i, p) in pieces.iter().enumerate() {
            let p = p.trim_start_matches(['~', '$', '/', '>']);
            if i > 0 {
                push_text(&mut parts, "+");
            }
            match arrow(p) {
                Some(a) => push_text(&mut parts, a),
                None => parts.push(Part::Button(p.to_ascii_uppercase())),
            }
        }
    }
    push_text(&mut parts, &run);
    parts
}

/// Plain-text form of notation pieces.
pub fn parts_text(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) | Part::Button(t) => t.as_str(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rename every button piece (`A` → the player's key) and rebuild the
/// plain-text notation.
pub fn relabel(moves: &mut [Move], label: impl Fn(&str) -> String) {
    for m in moves {
        for p in &mut m.parts {
            if let Part::Button(b) = p {
                *b = label(b);
            }
        }
        m.notation = parts_text(&m.parts);
    }
}

/// MUGEN command notation → arrows and buttons: `~D, DF, F, x` → `↓↘→ X`.
#[cfg(test)]
fn notation(cmd: &str) -> String {
    parts_text(&command_parts(cmd))
}

/// `TripleKFPalm` / `QCF_x` → `TRIPLE K F PALM` / `QCF X` (fallback when a block has no title).
fn pretty(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '-' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_ascii_lowercase();
        out.push(c.to_ascii_uppercase());
    }
    out.trim().to_string()
}

/// `command = "x"` references of a trigger line (not `command != "x"`).
fn command_refs(line: &str) -> Vec<String> {
    let low = line.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut rest = low.as_str();
    while let Some(i) = rest.find("command") {
        rest = &rest[i + 7..];
        let r = rest.trim_start();
        if let Some(r) = r.strip_prefix('=')
            && let Some(start) = r.find('"')
            && r[..start].trim().is_empty()
            && let Some(len) = r[start + 1..].find('"')
        {
            out.push(r[start + 1..start + 1 + len].to_string());
        }
    }
    out
}

/// `power >= N` / `power > N` threshold, or `power = N` (a full-bar check:
/// `None` threshold, `true`) of a trigger line.
fn power_check(line: &str) -> (Option<i32>, bool) {
    let low = line.to_ascii_lowercase();
    let mut threshold = None;
    let mut full = low.contains("powermax");
    let mut rest = low.as_str();
    while let Some(i) = rest.find("power") {
        let before = rest[..i].chars().last();
        rest = &rest[i + 5..];
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ',') {
            continue;
        }
        let r = rest.trim_start();
        let (op, r) = if let Some(r) = r.strip_prefix(">=") {
            (">=", r)
        } else if let Some(r) = r.strip_prefix('>') {
            (">", r)
        } else if let Some(r) = r.strip_prefix('=') {
            ("=", r)
        } else {
            continue;
        };
        let digits: String = r
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let Ok(n) = digits.parse::<i32>() else {
            continue;
        };
        if op == "=" {
            full |= n >= 1000;
        } else {
            threshold = Some(threshold.unwrap_or(0).max(n));
        }
    }
    (threshold, full)
}

/// `2000` → `2 BARS`, `1000` → `1 BAR`, `2500` → `2.5 BARS`.
fn bars(power: i32) -> String {
    let tenths = (power + 50) / 100;
    let n = if tenths % 10 == 0 {
        (tenths / 10).to_string()
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    };
    format!("{n} {}", if tenths == 10 { "BAR" } else { "BARS" })
}

/// A `[State -1]` block as far as the move list cares.
#[derive(Default)]
struct Block {
    title: String,
    kind: String,
    /// `triggerall` command checks: each entry is one requirement, its
    /// strings the alternatives (`command = "a" || command = "b"`).
    all: Vec<Vec<String>>,
    /// `triggerN` command checks by N.
    numbered: std::collections::BTreeMap<u32, Vec<Vec<String>>>,
    power: Option<i32>,
    full: bool,
    /// Tag-team / partner system block.
    tag: bool,
}

fn is_tag_title(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    ["assist", "partner", "tag", "quick change", "cross"]
        .iter()
        .any(|w| t.contains(w))
}

/// The moves of a `.cmd`: every `[State -1, Title]` block that changes state
/// on a command, with all the commands it checks combined (`holddown` +
/// `a` → `HOLD ↓ + [A]`), grouped (supers by their power check, specials,
/// system S-button moves, movement, combos) in display order. Tag-team
/// system blocks and plain normals are left out.
pub fn parse_moves(text: &str) -> Vec<Move> {
    let mut commands: Vec<(String, String)> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    enum Sec {
        None,
        Command,
        State,
    }
    let mut sec = Sec::None;
    let mut cur_name = String::new();
    let mut cur_cmd = String::new();
    for raw in text.lines() {
        let line = strip(raw);
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if matches!(sec, Sec::Command) && !cur_name.is_empty() {
                commands.push((std::mem::take(&mut cur_name), std::mem::take(&mut cur_cmd)));
            }
            cur_name.clear();
            cur_cmd.clear();
            let inner = line.trim_matches(['[', ']']).trim();
            let lower = inner.to_ascii_lowercase();
            if lower == "command" {
                sec = Sec::Command;
            } else if lower.starts_with("state -1") || lower.starts_with("state-1") {
                let title = inner
                    .split_once(',')
                    .map(|(_, t)| t.trim().to_string())
                    .unwrap_or_default();
                blocks.push(Block {
                    tag: is_tag_title(&title),
                    title,
                    ..Default::default()
                });
                sec = Sec::State;
            } else {
                sec = Sec::None;
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        match sec {
            Sec::Command if key == "name" => cur_name = unquote(v),
            Sec::Command if key == "command" => cur_cmd = v.trim().to_string(),
            Sec::State if key == "type" => {
                if let Some(b) = blocks.last_mut() {
                    b.kind = v.trim().to_ascii_lowercase();
                }
            }
            Sec::State if key.starts_with("trigger") => {
                let Some(b) = blocks.last_mut() else { continue };
                let low = line.to_ascii_lowercase();
                // a move that needs a partner (`NumPartner`, `Partner, StateNo`,
                // `teammode = Simul`); `!NumPartner || ...` guards solo moves
                if !low.contains("!numpartner")
                    && (low.contains("partner") || low.contains("teammode ="))
                {
                    b.tag = true;
                }
                let refs = command_refs(line);
                if !refs.is_empty() {
                    let reqs: Vec<Vec<String>> = if low.contains("||") {
                        vec![refs]
                    } else {
                        refs.into_iter().map(|r| vec![r]).collect()
                    };
                    let rest = &key[7..];
                    if rest == "all" {
                        b.all.extend(reqs);
                    } else if let Ok(n) = rest.trim().parse::<u32>() {
                        b.numbered.entry(n).or_default().extend(reqs);
                    }
                }
                let (th, full) = power_check(line);
                if let Some(t) = th {
                    b.power = Some(b.power.unwrap_or(0).max(t));
                }
                b.full |= full;
            }
            _ => {}
        }
    }
    if matches!(sec, Sec::Command) && !cur_name.is_empty() {
        commands.push((cur_name, cur_cmd));
    }
    let find = |name: &str| {
        commands
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, c)| c.clone())
    };
    let mut moves: Vec<Move> = Vec::new();
    for b in blocks {
        if b.tag || !matches!(b.kind.as_str(), "changestate" | "selfstate") {
            continue;
        }
        let mut reqs = b.all.clone();
        if let Some((_, first)) = b.numbered.iter().next() {
            reqs.extend(first.iter().cloned());
        }
        // each requirement in pieces; alternatives join with ` / `
        let mut pieces: Vec<Vec<Part>> = Vec::new();
        let mut first_name = String::new();
        for alts in &reqs {
            let mut alt_parts: Vec<Vec<Part>> = Vec::new();
            for a in alts {
                if let Some(c) = find(a) {
                    if first_name.is_empty() {
                        first_name = a.clone();
                    }
                    alt_parts.push(command_parts(&c));
                }
            }
            let mut joined: Vec<Part> = Vec::new();
            for (i, p) in alt_parts.into_iter().enumerate() {
                if i > 0 {
                    push_text(&mut joined, "/");
                }
                for part in p {
                    match part {
                        Part::Text(t) => push_text(&mut joined, &t),
                        b => joined.push(b),
                    }
                }
            }
            if !joined.is_empty() && !pieces.contains(&joined) {
                pieces.push(joined);
            }
        }
        if pieces.is_empty() {
            continue;
        }
        let has_button = |p: &Vec<Part>| p.iter().any(|x| matches!(x, Part::Button(_)));
        // held directions and motions first, then the buttons
        pieces.sort_by_key(has_button);
        let mut parts: Vec<Part> = Vec::new();
        for (i, p) in pieces.iter().enumerate() {
            // a motion runs straight into its button (`↓↘→ [C]`); a held
            // direction or a second command joins with `+`
            let motion = i > 0
                && pieces[i - 1].len() == 1
                && matches!(&pieces[i - 1][0], Part::Text(t) if !t.starts_with("HOLD") && t.chars().count() > 1);
            if i > 0 && !motion {
                push_text(&mut parts, "+");
            }
            for part in p.iter().cloned() {
                match part {
                    Part::Text(t) => push_text(&mut parts, &t),
                    b => parts.push(b),
                }
            }
        }
        let buttons: Vec<&str> = parts
            .iter()
            .filter_map(|p| match p {
                Part::Button(b) => Some(b.as_str()),
                _ => None,
            })
            .collect();
        let dirs: usize = parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.chars().filter(|c| "↑↓→←↗↘↖↙".contains(*c)).count(),
                _ => 0,
            })
            .sum();
        let single_button = buttons.len() == 1 && dirs == 0;
        // only held directions and one button: a crouching / standing normal
        let held_only = pieces.iter().all(|p| {
            if has_button(p) {
                p.len() == 1
            } else {
                matches!(p.as_slice(), [Part::Text(t)] if t.starts_with("HOLD"))
            }
        });
        let title = b.title.to_ascii_uppercase();
        let uses_s = buttons.iter().any(|b| b.eq_ignore_ascii_case("s"));
        let group = if uses_s {
            Group::System
        } else if b.power.is_some_and(|p| p >= 1000) {
            Group::Super
        } else if buttons.is_empty() {
            if dirs < 2 {
                continue;
            }
            Group::Movement
        } else if title.contains("COMBO") && dirs == 0 {
            Group::Combo
        } else if single_button {
            // a plain normal (`STAND LIGHT PUNCH`)
            continue;
        } else if buttons.len() == 1 && held_only && !title.contains("THROW") {
            // crouching / standing normals (`HOLD ↓ + [X]`)
            continue;
        } else {
            Group::Special
        };
        if uses_s && b.full {
            push_text(&mut parts, "AT FULL POWER");
        }
        let notation = parts_text(&parts);
        let name = match (group, notation.as_str()) {
            (Group::Movement, "→→") => "RUN".to_string(),
            (Group::Movement, "←←") => "BACK DASH".to_string(),
            _ if title.is_empty() => pretty(&first_name),
            _ => title,
        };
        // the same input again under the same name (or a mode variant of
        // it: `RUN AWAKENING`, `COMBO 1 BIJUU MODE`) is listed once
        // (an untitled block repeating a listed input adds nothing either)
        let untitled = b.title.is_empty();
        if moves.iter().any(|m| {
            m.name == name
                || (m.notation == notation
                    && (untitled || name.starts_with(&format!("{} ", m.name))))
        }) {
            continue;
        }
        moves.push(Move {
            cost: (group == Group::Super).then(|| bars(b.power.unwrap_or(1000))),
            name,
            notation,
            parts,
            group,
        });
    }
    moves.sort_by_key(|m| m.group);
    moves
}

/// The move list of a character .def and the command file's name.
pub fn move_list(def: &Path) -> (String, Vec<Move>) {
    let Some(cmd) = cmd_path(def) else {
        return (String::new(), Vec::new());
    };
    let name = cmd
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let moves = read_lossy(&cmd)
        .map(|t| parse_moves(&t))
        .unwrap_or_default();
    (name, moves)
}

/// `victoryN` lines of the undecorated `[Quotes]` group of the character's files.
pub fn victory_quotes(def: &Path) -> Vec<String> {
    let mut files: Vec<PathBuf> = def_files(def)
        .into_iter()
        .filter(|(k, _)| k == "cns" || k.starts_with("st"))
        .filter_map(|(_, v)| resolve(def, &v))
        .collect();
    files.insert(0, def.to_path_buf());
    for f in files {
        let Ok(text) = read_lossy(&f) else { continue };
        let mut quotes = Vec::new();
        let mut inside = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('[') {
                inside = strip(line).eq_ignore_ascii_case("[quotes]");
                continue;
            }
            if !inside {
                continue;
            }
            // quotes may contain ';' inside the string: only cut comments after the closing quote
            if let Some((k, v)) = line.split_once('=')
                && k.trim().to_ascii_lowercase().starts_with("victory")
            {
                let v = v.trim();
                let q = match v.strip_prefix('"') {
                    Some(r) => r.split('"').next().unwrap_or("").to_string(),
                    None => strip(v).to_string(),
                };
                if !q.trim().is_empty() {
                    quotes.push(q.trim().to_string());
                }
            }
        }
        if !quotes.is_empty() {
            return quotes;
        }
    }
    Vec::new()
}

/// Read-only match observer: rounds, best combo, damage, time and supers per side.
#[derive(Clone, Debug, Default)]
pub struct MatchStats {
    pub max_combo: [i32; 2],
    pub damage: [i32; 2],
    pub supers: [u32; 2],
    /// Ticks of live fighting (round state 2).
    pub ticks: u32,
    last_life: [i32; 2],
    last_power: [i32; 2],
    started: bool,
}

impl MatchStats {
    /// Root players (helpers excluded) of both sides.
    fn roots(w: &World) -> Option<[usize; 2]> {
        let mut it = w
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0);
        let a = it.next()?.0;
        let b = it.next()?.0;
        Some([a, b])
    }

    /// Look at the world after a simulation step.
    pub fn observe(&mut self, w: &World) {
        let Some(roots) = Self::roots(w) else { return };
        let life = roots.map(|i| w.players[i].life);
        let power = roots.map(|i| w.players[i].power);
        if !self.started {
            self.started = true;
            self.last_life = life;
            self.last_power = power;
        }
        #[allow(clippy::needless_range_loop)]
        for side in 0..2 {
            let other = 1 - side;
            // damage dealt by `side` = life the other side lost (new rounds refill it)
            let lost = self.last_life[other] - life[other];
            if lost > 0 {
                self.damage[side] += lost;
            }
            let hits = w.players[roots[other]].received_hits;
            self.max_combo[side] = self.max_combo[side].max(hits);
            if self.last_power[side] - power[side] >= 1000 {
                self.supers[side] += 1;
            }
        }
        if w.round.round_state() == 2 {
            self.ticks += 1;
        }
        self.last_life = life;
        self.last_power = power;
    }

    /// `(label, value)` rows for `side`'s match report.
    pub fn rows(&self, side: usize, wins: [u8; 2]) -> Vec<(String, String)> {
        let secs = self.ticks / 60;
        vec![
            (
                "ROUNDS".into(),
                format!("{} - {}", wins[side], wins[1 - side]),
            ),
            (
                "MAX COMBO".into(),
                match self.max_combo[side].max(0) {
                    1 => "1 HIT".to_string(),
                    n => format!("{n} HITS"),
                },
            ),
            ("DAMAGE DEALT".into(), self.damage[side].to_string()),
            ("TIME".into(), format!("{}:{:02}", secs / 60, secs % 60)),
            ("SUPERS".into(), self.supers[side].to_string()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notation_turns_motions_into_arrows() {
        assert_eq!(notation("~D, DF, F, x"), "↓↘→ X");
        assert_eq!(notation("F, F, a+b"), "→→ A + B");
        assert_eq!(notation("$F,x"), "→ X");
        assert_eq!(notation("~D, DB, B, D, DB, B, x"), "↓↙←↓↙← X");
        assert_eq!(notation("/$D"), "HOLD ↓");
        assert_eq!(notation("/s"), "HOLD S");
        // `B` is back, `b` the button
        assert_eq!(notation("B, b"), "← B");
        assert_eq!(
            command_parts("~D, DF, F, c"),
            vec![Part::Text("↓↘→".into()), Part::Button("C".into())]
        );
    }

    const SYSTEM_CMD: &str = r#"
[Command]
name = "SPECIAL 1"
command = ~D,DF,F,a
[Command]
name = "FF"
command = F, F
[Command]
name = "BB"
command = B, B
[Command]
name = "Super Jump"
command = ~D,U
[Command]
name = "holddown"
command = /$D
[Command]
name = "holdfwd"
command = /$F
[Command]
name = "a"
command = a
[Command]
name = "b"
command = b
[Command]
name = "x"
command = x
[Command]
name = "y"
command = y
[Command]
name = "s"
command = s
[Command]
name = "hold_s"
command = /s
[Command]
name = "turtleuno"
command = b+c
[Statedef -1]
[State -1, Partner Assist Animation]
type = ChangeState
triggerall = NumPartner
triggerall = Power >= 1000
triggerall = command = "turtleuno"
[State -1, TagIn]
type = ChangeState
triggerall = command = "turtleuno"
[State -1, C_6]
type = CtrlSet
trigger1 = command="x"||command="y"
[State -1, Correr Adelante]
type = ChangeState
trigger1 = command = "FF"
[State -1, Correr Atras]
type = ChangeState
trigger1 = command = "BB"
[State -1, Super Jump]
type = ChangeState
trigger1 = command = "Super Jump"
[State -1, Correr Adelante Awakening]
type = ChangeState
trigger1 = command = "FF"
[State -1, Oodama Rasengan]
type = ChangeState
triggerall = !NumPartner || NumPartner && (sysvar(4) != 12)
Triggerall = power >= 2000
triggerall = command = "holddown"
triggerall = command = "a"
[State -1, Naruto Rendan]
type = ChangeState
Triggerall = power >= 2500
triggerall = command = "holddown"
triggerall = command = "b"
[State -1, Sempou Rasengan]
type = ChangeState
Triggerall = power >= 500
triggerall = command = "SPECIAL 1"
[State -1, Kung Fu Throw]
type = ChangeState
triggerall = command = "y"
trigger1 = command = "holdfwd"
trigger2 = command = "holdback"
[State -1, Crouching Light Punch]
type = ChangeState
triggerall = command = "x"
triggerall = command = "holddown"
[State -1, Stand Light Punch]
type = ChangeState
triggerall = command = "x"
triggerall = command != "holddown"
[State -1, Combo 1]
type = ChangeState
triggerall = command = "a"
[State -1, Combo 1 Bijuu Mode]
type = ChangeState
triggerall = command = "a"
[State -1, Power Charge]
type = ChangeState
Triggerall = power < 3000
triggerall = command = "s"
[State -1, Chakra Hold]
type = ChangeState
triggerall = command = "hold_s"
[State -1, Awakening Mode]
type = ChangeState
Triggerall = power = 3000
triggerall = command = "s"
"#;

    fn find<'a>(m: &'a [Move], name: &str) -> &'a Move {
        m.iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {m:#?}"))
    }

    #[test]
    fn blocks_combine_their_commands_and_group() {
        let m = parse_moves(SYSTEM_CMD);
        let names: Vec<&str> = m.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "OODAMA RASENGAN",
                "NARUTO RENDAN",
                "SEMPOU RASENGAN",
                "KUNG FU THROW",
                "POWER CHARGE",
                "CHAKRA HOLD",
                "AWAKENING MODE",
                "RUN",
                "BACK DASH",
                "SUPER JUMP",
                "COMBO 1",
            ]
        );
        let o = find(&m, "OODAMA RASENGAN");
        assert_eq!(o.group, Group::Super);
        assert_eq!(o.notation, "HOLD ↓ + A");
        assert_eq!(
            o.parts,
            vec![Part::Text("HOLD ↓ +".into()), Part::Button("A".into())]
        );
        assert_eq!(o.cost.as_deref(), Some("2 BARS"));
        assert_eq!(find(&m, "NARUTO RENDAN").cost.as_deref(), Some("2.5 BARS"));
        let s = find(&m, "SEMPOU RASENGAN");
        assert_eq!((s.group, s.cost.clone()), (Group::Special, None));
        assert_eq!(s.notation, "↓↘→ A");
        assert_eq!(find(&m, "KUNG FU THROW").notation, "HOLD → + Y");
        assert_eq!(find(&m, "POWER CHARGE").notation, "S");
        assert_eq!(find(&m, "POWER CHARGE").group, Group::System);
        assert_eq!(find(&m, "CHAKRA HOLD").notation, "HOLD S");
        assert_eq!(find(&m, "AWAKENING MODE").notation, "S AT FULL POWER");
        assert_eq!(find(&m, "RUN").notation, "→→");
        assert_eq!(find(&m, "BACK DASH").group, Group::Movement);
        assert_eq!(find(&m, "SUPER JUMP").notation, "↓↑");
        assert_eq!(find(&m, "COMBO 1").group, Group::Combo);
    }

    #[test]
    fn moves_come_from_state_minus_one_titles() {
        let cmd = r#"
[Command]
name = "TripleKFPalm"
command = ~D, DF, F, D, DF, F, x
[Command]
name = "QCF_x"
command = ~D, DF, F, x
[Command]
name = "a"
command = a
[Statedef -1]
[State -1, Triple Kung Fu Palm]
type = ChangeState
triggerall = command = "TripleKFPalm"
triggerall = power >= 1000
[State -1, Light Kung Fu Palm]
type = ChangeState
triggerall = command = "QCF_x"
[State -1]
type = ChangeState
trigger1 = command = "QCF_x"
[State -1, Stand Light Punch]
type = ChangeState
trigger1 = command = "a"
"#;
        let m = parse_moves(cmd);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "TRIPLE KUNG FU PALM");
        assert_eq!(m[0].group, Group::Super);
        assert_eq!(m[0].cost.as_deref(), Some("1 BAR"));
        assert_eq!(m[1].notation, "↓↘→ X");
        assert_eq!(m[1].group, Group::Special);
    }

    const KFM_LIKE_DEF: &str = "[Files]\ncmd = Sub\\Tiny.CMD\n";
    const KFM_LIKE_CMD: &str = "[Command]\nname = \"qcf_x\"\ncommand = ~D, DF, F, x\n[Statedef -1]\n[State -1, Fireball]\ntype = ChangeState\ntrigger1 = command = \"qcf_x\"\n";

    #[test]
    fn move_list_reads_inside_content_files() {
        use rugen_formats::rgc::{self, Entry, Kind, SectionKind, Writer};
        let dir = std::env::temp_dir().join(format!("rugen-moves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("Tiny.rgc");
        let mut w = Writer::create(&p, Kind::Char).unwrap();
        for (path, body) in [("Tiny.def", KFM_LIKE_DEF), ("sub/tiny.cmd", KFM_LIKE_CMD)] {
            let mut e = Entry::new(SectionKind::File, path);
            e.hash = rgc::sha256(body.as_bytes());
            w.add(e, body.as_bytes(), 3).unwrap();
        }
        w.finish([0; 32]).unwrap();
        let (file, moves) = move_list(&p.join("Tiny.def"));
        assert_eq!(file, "tiny.cmd");
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].name, "FIREBALL");
        // a plain folder with different case still works
        let f = dir.join("folder");
        std::fs::create_dir_all(f.join("Sub")).unwrap();
        std::fs::write(f.join("tiny.def"), KFM_LIKE_DEF).unwrap();
        std::fs::write(f.join("Sub/tiny.cmd"), KFM_LIKE_CMD).unwrap();
        assert_eq!(move_list(&f.join("tiny.def")).1.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
