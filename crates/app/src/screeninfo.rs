//! Character facts and match stats for the menu screens: the move list shown
//! by the pause menu (read from the character's `.cmd`), victory quotes for
//! the result screen (`[Quotes]` in its `.cns`), and a match-stats observer
//! that only reads the simulation (never feeds anything back into it).

use std::path::{Path, PathBuf};

use rugen_sim::World;

/// One special or super move for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub name: String,
    /// Arrows and buttons, e.g. `↓↘→ + X`.
    pub notation: String,
    /// Needs a full power bar (super / hyper).
    pub super_move: bool,
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

/// MUGEN command notation → arrows and buttons: `~D, DF, F, x` → `↓↘→ + X`.
pub fn notation(cmd: &str) -> String {
    let mut dirs = String::new();
    let mut out: Vec<String> = Vec::new();
    for tok in cmd.split(',') {
        let t = tok
            .trim()
            .trim_start_matches(['~', '$', '/', '>'])
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim();
        if t.is_empty() {
            continue;
        }
        let arrow = match t {
            "U" => Some('↑'),
            "D" => Some('↓'),
            "F" => Some('→'),
            "B" => Some('←'),
            "UF" => Some('↗'),
            "DF" => Some('↘'),
            "UB" => Some('↖'),
            "DB" => Some('↙'),
            _ => None,
        };
        match arrow {
            Some(a) => dirs.push(a),
            None => {
                let buttons: Vec<String> = t
                    .split('+')
                    .map(|b| {
                        let b = b.trim().trim_start_matches(['~', '$', '/', '>']);
                        match b {
                            "U" => "↑".into(),
                            "D" => "↓".into(),
                            "F" => "→".into(),
                            "B" => "←".into(),
                            other => other.to_ascii_uppercase(),
                        }
                    })
                    .collect();
                if !dirs.is_empty() {
                    out.push(std::mem::take(&mut dirs));
                }
                out.push(buttons.join("+"));
            }
        }
    }
    if !dirs.is_empty() {
        out.push(dirs);
    }
    out.join(" + ")
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

/// A command worth listing: a motion or a button chord, not a single press or hold.
fn is_move(cmd: &str) -> bool {
    let toks: Vec<&str> = cmd
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let has_button = toks
        .iter()
        .any(|t| t.chars().any(|c| "abcxyzs".contains(c)));
    has_button && (toks.len() >= 2 || toks.iter().any(|t| t.contains('+')))
}

/// Special and super moves from a `.cmd`: `[State -1, Title]` blocks that
/// trigger on a motion command. Supers (a `power >= 1000` trigger) come first.
pub fn parse_moves(text: &str) -> Vec<Move> {
    let mut commands: Vec<(String, String)> = Vec::new();
    // (title, commands referenced, needs power >= 1000)
    let mut blocks: Vec<(String, Vec<String>, bool)> = Vec::new();
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
                blocks.push((title, Vec::new(), false));
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
            Sec::State if key.starts_with("trigger") => {
                let low = line.to_ascii_lowercase();
                let Some(b) = blocks.last_mut() else { continue };
                let mut rest = low.as_str();
                while let Some(i) = rest.find("command") {
                    rest = &rest[i + 7..];
                    let r = rest.trim_start();
                    if let Some(r) = r.strip_prefix('=')
                        && let Some(start) = r.find('"')
                        && let Some(len) = r[start + 1..].find('"')
                    {
                        b.1.push(r[start + 1..start + 1 + len].to_string());
                    }
                }
                if let Some(i) = low.find("power") {
                    let r = &low[i + 5..];
                    let digits: String = r
                        .trim_start_matches([' ', '>', '=', '\t'])
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if r.trim_start().starts_with('>')
                        && digits.parse::<i32>().is_ok_and(|n| n >= 999)
                    {
                        b.2 = true;
                    }
                }
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
            .find(|(n, c)| n.eq_ignore_ascii_case(name) && is_move(c))
            .map(|(_, c)| c.clone())
    };
    let mut moves: Vec<Move> = Vec::new();
    for (title, cmds, sup) in blocks {
        let Some((cname, cmd)) = cmds.iter().find_map(|c| find(c).map(|cmd| (c, cmd))) else {
            continue;
        };
        let name = if title.is_empty() {
            pretty(cname)
        } else {
            title.to_ascii_uppercase()
        };
        if moves.iter().any(|m| m.name == name) {
            continue;
        }
        moves.push(Move {
            name,
            notation: notation(&cmd),
            super_move: sup,
        });
    }
    moves.sort_by_key(|m| !m.super_move);
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
        assert_eq!(notation("~D, DF, F, x"), "↓↘→ + X");
        assert_eq!(notation("F, F, a+b"), "→→ + A+B");
        assert_eq!(notation("$F,x"), "→ + X");
        assert_eq!(notation("~D, DB, B, D, DB, B, x"), "↓↙←↓↙← + X");
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
[State -1, Stand Light Punch]
type = ChangeState
trigger1 = command = "a"
"#;
        let m = parse_moves(cmd);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "TRIPLE KUNG FU PALM");
        assert!(m[0].super_move);
        assert_eq!(m[1].notation, "↓↘→ + X");
        assert!(!m[1].super_move);
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
