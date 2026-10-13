//! Host for `rugen-ui` screens: loads `motif/ui/<name>.toml`, paints with the
//! renderer and the motif fonts, decodes WebP/PNG/SFF assets, maps the mouse,
//! and turns actions into shell transitions. The title and options screens
//! live here when the motif ships `ui/title.toml` / `ui/options.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rugen_render::{Camera, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::Buttons;
use rugen_ui::{Event, Nav, Painter, Rect, ScreenDef, TextKey, Ui};
use serde_json::{Value, json};
use winit::keyboard::{Key, NamedKey};

use crate::config::Config;
use crate::game::{Mode, ResultInfo, Transition};
use crate::play::FightSetup;
use crate::roster::{Outcome, Roster};
use crate::sprites::{FontGpu, SpriteSheet};
use crate::system::SystemData;
use crate::versus::Portrait;

/// Which hardcoded behaviour backs the screen's actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKind {
    Title,
    Options,
    /// Character / stage select for a game mode.
    Select(Mode),
    /// VS screen before a fight (`ui/vs.toml`).
    Versus,
    /// Post-match result (`ui/result.toml`).
    Result,
    /// In-match pause menu (`ui/pause.toml`).
    Pause,
    /// Key bindings page for one player (`ui/controls.toml`).
    Controls,
}

/// A decoded RGBA image kept in memory and drawn through a `mem:<name>` asset key.
pub type MemImage = (u32, u32, Vec<u8>);

/// Map a winit key to a text-input key.
pub fn text_key(key: &Key) -> Option<TextKey> {
    Some(match key {
        Key::Named(NamedKey::Backspace) => TextKey::Backspace,
        Key::Named(NamedKey::Enter) => TextKey::Enter,
        Key::Named(NamedKey::Escape) => TextKey::Escape,
        Key::Named(NamedKey::Space) => TextKey::Char(' '),
        Key::Character(c) => TextKey::Char(c.chars().next()?),
        _ => return None,
    })
}

/// Per-character record in the profile.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CharStats {
    pub plays: u32,
    pub wins: u32,
}

/// A server's rating of this player, kept for the title screen rank card.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RankSnap {
    pub server: String,
    /// The server proved it is the official one (global ranking).
    pub official: bool,
    pub rating: i32,
    pub rank: String,
    pub wins: u32,
    pub losses: u32,
}

/// Local player profile (`profile.json` in the game root).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub level: u32,
    pub xp: u32,
    pub wins: u32,
    pub losses: u32,
    pub shards: u32,
    pub avatar: String,
    /// Per-character plays / wins keyed by cache manifest id.
    #[serde(default)]
    pub chars: std::collections::BTreeMap<String, CharStats>,
    /// `rugen-server` anonymous account (empty until first online use).
    #[serde(default)]
    pub online_key: String,
    #[serde(default)]
    pub online_id: u64,
    /// Character brought into online matches (empty = `[net] p1`).
    #[serde(default)]
    pub main_char: String,
    /// Accounts on other servers, keyed by server address: (id, key). The
    /// key is all this file holds of an account: passwords are never stored.
    #[serde(default)]
    pub online_accounts: std::collections::BTreeMap<String, (u64, String)>,
    /// Username of the named account each server address is logged in to
    /// (shown offline; absent = an anonymous account or none).
    #[serde(default)]
    pub online_names: std::collections::BTreeMap<String, String>,
    /// Last known online rating per server address.
    #[serde(default)]
    pub online_ranks: std::collections::BTreeMap<String, RankSnap>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "PLAYER".into(),
            level: 1,
            xp: 0,
            wins: 0,
            losses: 0,
            shards: 0,
            avatar: "ui/avatar.webp".into(),
            chars: Default::default(),
            online_key: String::new(),
            online_id: 0,
            main_char: String::new(),
            online_accounts: Default::default(),
            online_names: Default::default(),
            online_ranks: Default::default(),
        }
    }
}

impl Profile {
    pub fn save(&self, root: &Path) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(root.join("profile.json"), text);
        }
    }

    /// Record a finished match for character `id`; levels up every 100 xp.
    pub fn record(&mut self, id: &str, won: bool) {
        if won {
            self.wins += 1;
            self.xp += 20;
            self.shards += 5;
        } else {
            self.losses += 1;
            self.xp += 8;
            self.shards += 1;
        }
        self.level = 1 + self.xp / 100;
        if !id.is_empty() {
            let e = self.chars.entry(id.to_string()).or_default();
            e.plays += 1;
            if won {
                e.wins += 1;
            }
        }
    }

    /// Placeholder until ranked exists: a rating derived from wins.
    pub fn rating_text(&self) -> String {
        if self.wins + self.losses == 0 {
            "UNRANKED".into()
        } else {
            format!("{}", 1000 + self.wins as i64 * 25 - self.losses as i64 * 15)
        }
    }

    /// Title screen rank card: (title, rating, line). The official rating
    /// when known, else the best community one (labelled), else local.
    pub fn rank_card(&self) -> (String, String, String) {
        let official = self.online_ranks.values().find(|r| r.official);
        let community = self
            .online_ranks
            .values()
            .filter(|r| !r.official && r.wins + r.losses > 0)
            .max_by_key(|r| r.rating);
        match (official, community) {
            (Some(r), _) => (
                "RANKED · OFFICIAL".into(),
                r.rating.to_string(),
                format!("{} · {}W {}L", r.rank, r.wins, r.losses),
            ),
            (None, Some(r)) => (
                "RANKED · COMMUNITY".into(),
                r.rating.to_string(),
                format!(
                    "{} · {}",
                    r.server.to_uppercase().chars().take(14).collect::<String>(),
                    r.rank
                ),
            ),
            (None, None) => (
                "RANKED · LOCAL".into(),
                if self.wins + self.losses == 0 {
                    "1500".into()
                } else {
                    self.rating_text()
                },
                format!(
                    "{} · {}W {}L",
                    if self.wins + self.losses == 0 {
                        "UNRANKED"
                    } else {
                        "LOCAL"
                    },
                    self.wins,
                    self.losses
                ),
            ),
        }
    }

    pub fn load(root: &Path) -> Self {
        let path = root.join("profile.json");
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(_) => {
                let p = Self::default();
                p.save(root);
                p
            }
        }
    }
}

/// Decoded image cache entry: texture, size and (for SFF sprites) palette.
pub(crate) type Cached = Option<(TextureId, u32, u32, Option<PaletteId>)>;

/// Mouse events in window pixels.
#[derive(Clone, Copy, Debug)]
pub enum MouseEvent {
    Move(f32, f32),
    Left,
    Button(bool),
    Wheel(f32),
}

pub struct UiScreen {
    pub kind: UiKind,
    ui: Ui,
    motif: PathBuf,
    textures: HashMap<String, Cached>,
    /// Options / controls screens: the edited config.
    pub config: Config,
    pub dirty: bool,
    profile: Profile,
    news: Value,
    /// Installed UI themes for the options row.
    themes: Vec<String>,
    window: (u32, u32),
    mouse_px: Option<(f32, f32)>,
    pending: Option<Transition>,
    /// Select screens only.
    roster: Option<Roster>,
    room: String,
    /// VS screen: the fight to start and the ticks left.
    versus: Option<(FightSetup, i32)>,
    /// Result screen.
    result: Option<ResultInfo>,
    /// In-memory images (`mem:p1`...) for portraits that are not files.
    mem: HashMap<String, MemImage>,
    /// Options / controls pages (see `settings.rs`).
    settings: SettingsPage,
    /// Pause menu: who paused, their move list, what the menu offers.
    pause: Option<PauseInfo>,
    /// Move list page shown by the pause menu.
    moves_page: usize,
    /// Title: installed fighters the feature panel cycles through (newest first).
    featured: Vec<Featured>,
    feature_at: usize,
    /// Title: arena facts, computed once.
    arenas: Value,
    /// Title: stage for the featured fighter's training run.
    try_stage: Option<PathBuf>,
    ticks: u64,
}

/// What the pause menu shows.
pub struct PauseInfo {
    /// Side that paused (0 = 1P).
    pub side: usize,
    pub name: String,
    pub art: Option<MemImage>,
    /// Command file the move list came from.
    pub cmd: String,
    pub moves: Vec<crate::screeninfo::Move>,
    /// The frozen fight under the menu.
    pub scene: Option<MemImage>,
    /// Mode to go back to for CHARACTER SELECT (None: no select screen).
    pub mode: Option<Mode>,
}

/// A fighter for the title's feature panel.
struct Featured {
    name: String,
    art: String,
    source: String,
    def: PathBuf,
}

/// Rows of the pause move list shown per page.
const MOVES_PER_PAGE: usize = 9;
/// Ticks each featured fighter stays on the title.
const FEATURE_TICKS: u64 = 300;

/// Short key label for a bound key name (`ArrowUp` → `↑`, `j` → `J`).
fn key_label(k: &str) -> String {
    match k {
        "ArrowUp" => "↑".into(),
        "ArrowDown" => "↓".into(),
        "ArrowLeft" => "←".into(),
        "ArrowRight" => "→".into(),
        "Enter" => "ENTER".into(),
        "Space" => "SPACE".into(),
        "Escape" => "ESC".into(),
        "Shift" => "SHIFT".into(),
        "Control" => "CTRL".into(),
        other => other.to_uppercase(),
    }
}

/// One-line description of a title menu entry.
fn menu_desc(id: &str) -> &'static str {
    match id {
        "arcade" => "Climb the ladder vs the CPU",
        "versus" => "Two players, one keyboard",
        "arena" => "Platforms and stocks: knock them off the stage",
        "training" => "Free practice, no clock",
        "tutorial" => "Learn the basics step by step",
        "watch" => "Sit back: CPU vs CPU",
        "online" => "Room codes, ranked and LAN play",
        "profile" => "Your online accounts: username and password",
        "netplay" => "Direct match over the network",
        "options" => "Video, audio and controls",
        "exit" => "See you next chapter",
        _ => "",
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// State of the options tabs and the controls page.
#[derive(Default)]
struct SettingsPage {
    /// Active options tab.
    tab: usize,
    /// Focus key seen last tick (help line and tab follow it).
    last_focus: Option<String>,
    controls: Option<crate::settings::Controls>,
    root: PathBuf,
    ticks: u64,
}

pub(crate) use rugen_ui::viewport;

impl UiScreen {
    /// True when the motif (or the flat `motif/` it falls back to) provides a TOML for this screen.
    pub fn available(motif: &Path, kind: UiKind) -> bool {
        motif_file(motif, &format!("ui/{}", Self::file(kind))).is_some()
    }

    fn file(kind: UiKind) -> &'static str {
        match kind {
            UiKind::Title => "title.toml",
            UiKind::Options => "options.toml",
            UiKind::Select(_) => "select.toml",
            UiKind::Versus => "vs.toml",
            UiKind::Result => "result.toml",
            UiKind::Pause => "pause.toml",
            UiKind::Controls => "controls.toml",
        }
    }

    /// Start the leave animation; the shell waits for [`left`](Self::left).
    pub fn leave(&mut self) {
        self.ui.leave();
    }
    pub fn leaving(&self) -> bool {
        self.ui.leaving()
    }
    pub fn left(&self) -> bool {
        self.ui.left()
    }

    /// VS screen for `setup` with both sides' portraits.
    #[allow(clippy::too_many_arguments)]
    pub fn load_versus(
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        lib: &rugen_content::Library,
        setup: FightSetup,
        sides: [Portrait; 2],
    ) -> anyhow::Result<Self> {
        let mut s = Self::load(UiKind::Versus, motif, root, sys, config, lib)?;
        let [a, b] = sides;
        for (i, p) in [a, b].into_iter().enumerate() {
            if let Some((w, h, rgba, _)) = p.image {
                s.mem.insert(format!("p{}", i + 1), (w, h, rgba));
            }
            s.mem
                .insert(format!("name{}", i + 1), (0, 0, p.name.into_bytes()));
        }
        s.versus = Some((setup, sys.versus.time.max(60)));
        s.refresh_data(sys);
        Ok(s)
    }

    /// Result screen after a match.
    pub fn load_result(
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        lib: &rugen_content::Library,
        info: ResultInfo,
    ) -> anyhow::Result<Self> {
        let mut s = Self::load(UiKind::Result, motif, root, sys, config, lib)?;
        for (i, art) in info.arts.iter().enumerate() {
            if let Some(img) = art {
                s.mem.insert(format!("p{}", i + 1), img.clone());
            }
        }
        s.result = Some(info);
        s.refresh_data(sys);
        Ok(s)
    }

    /// Pause menu over a frozen match.
    pub fn load_pause(
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        lib: &rugen_content::Library,
        mut info: PauseInfo,
    ) -> anyhow::Result<Self> {
        let mut s = Self::load(UiKind::Pause, motif, root, sys, config, lib)?;
        if let Some(img) = info.scene.take() {
            s.mem.insert("scene".into(), img);
        }
        if let Some(img) = info.art.take() {
            s.mem.insert("pauser".into(), img);
        }
        s.pause = Some(info);
        s.refresh_data(sys);
        Ok(s)
    }

    /// Title facts: featured fighters (newest archive first) and the arenas.
    fn load_title_facts(&mut self, lib: &rugen_content::Library) {
        let mut feats: Vec<(std::time::SystemTime, Featured)> = Vec::new();
        for e in lib.chars() {
            let Ok((dir, m)) = lib.prepare_char(&e) else {
                continue;
            };
            let file = m
                .art
                .clone()
                .or_else(|| m.portrait_big.as_ref().map(|p| p.file.clone()));
            let Some(file) = file else { continue };
            let when = std::fs::metadata(&e.source)
                .and_then(|md| md.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let source = e
                .source
                .strip_prefix(&lib.root)
                .unwrap_or(&e.source)
                .display()
                .to_string();
            feats.push((
                when,
                Featured {
                    name: m.display_name.to_uppercase(),
                    art: dir.join(file).display().to_string(),
                    source,
                    def: m.def_path(&dir),
                },
            ));
        }
        feats.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        self.featured = feats.into_iter().map(|(_, f)| f).collect();
        let stages = lib.stages();
        let first = stages
            .iter()
            .find(|e| e.id.eq_ignore_ascii_case("training"))
            .or_else(|| stages.first())
            .and_then(|e| lib.prepare_stage(e).ok());
        self.try_stage = lib.default_stage();
        let (count, name) = match first {
            Some((_, m)) => (stages.len(), m.display_name.to_uppercase()),
            None if self.try_stage.is_some() => (1, "TRAINING ROOM".to_string()),
            None => (0, "NO ARENAS YET".to_string()),
        };
        self.arenas = json!({
            "count": count,
            "name": name,
            "line": format!("1 / {}", count.max(1)),
        });
        let fighters = lib.chars().len();
        self.arenas["counts"] = Value::from(format!(
            "{} · {}",
            plural(fighters, "FIGHTER", "FIGHTERS"),
            plural(count, "ARENA", "ARENAS")
        ));
    }

    /// Fighter on the title's feature panel.
    fn featured_json(&self) -> Value {
        match self.featured.get(self.feature_at) {
            Some(f) => json!({
                "name": f.name,
                "art": f.art,
                "source": f.source,
                "present": true,
            }),
            None => json!({ "name": "", "art": "", "source": "", "present": false }),
        }
    }

    /// Key labels of P1's bindings for hint keycaps.
    fn keys_json(&self) -> Value {
        let k = &self.config.input.p1;
        json!({
            "a": key_label(&k.a),
            "b": key_label(&k.b),
            "c": key_label(&k.c),
            "x": key_label(&k.x),
            "start": key_label(&k.start),
        })
    }

    /// Function keys a screen handles itself (TAB search / F1 sort on the roster).
    pub fn hotkey(&mut self, key: &Key, sys: &SystemData) -> bool {
        if self.roster.is_none() {
            return false;
        }
        let action = match key {
            Key::Named(NamedKey::Tab) => "search",
            Key::Named(NamedKey::F1) => "sort",
            _ => return false,
        };
        self.action(action, sys);
        true
    }

    /// The game mode a select screen was opened for.
    pub fn mode(&self) -> Option<Mode> {
        match self.kind {
            UiKind::Select(m) => Some(m),
            _ => None,
        }
    }

    pub fn load(
        kind: UiKind,
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        lib: &rugen_content::Library,
    ) -> anyhow::Result<Self> {
        let path = motif_file(motif, &format!("ui/{}", Self::file(kind)))
            .unwrap_or_else(|| motif.join("ui").join(Self::file(kind)));
        let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        let def = ScreenDef::parse(&text).with_context(|| path.display().to_string())?;
        let news = motif_file(motif, "news.json")
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or_else(|| json!([]));
        let mut s = Self {
            kind,
            ui: Ui::new(def, Value::Null),
            motif: motif.to_path_buf(),
            textures: HashMap::new(),
            config: config.clone(),
            dirty: false,
            profile: Profile::load(root),
            themes: lib.themes(),
            news,
            window: (1280, 720),
            mouse_px: None,
            pending: None,
            roster: match kind {
                UiKind::Select(mode) => Some(Roster::new(mode, lib, config.game.ai_level)),
                _ => None,
            },
            room: if kind == UiKind::Select(Mode::Online) {
                "ONLINE".into()
            } else {
                "LOCAL".into()
            },
            versus: None,
            result: None,
            mem: HashMap::new(),
            settings: SettingsPage {
                root: lib.assets.clone(),
                controls: (kind == UiKind::Controls).then(|| crate::settings::Controls::new(0)),
                ..Default::default()
            },
            pause: None,
            moves_page: 0,
            featured: Vec::new(),
            feature_at: 0,
            arenas: Value::Null,
            try_stage: None,
            ticks: 0,
        };
        if kind == UiKind::Title {
            s.load_title_facts(lib);
        }
        if let Some(r) = &s.roster
            && r.dual()
        {
            s.ui.set_dual(true);
        }
        if let Some(r) = &s.roster
            && r.is_empty()
        {
            log::warn!("no characters in chars/");
            s.pending = Some(Transition::Title);
        }
        s.refresh_data(sys);
        Ok(s)
    }

    /// Rebuild the data context the TOML binds to.
    fn refresh_data(&mut self, sys: &SystemData) {
        let data = match self.kind {
            UiKind::Select(_) => match &self.roster {
                Some(r) => {
                    let mut d = r.data(&self.profile, &self.room);
                    d["keys"] = self.keys_json();
                    d["searching"] = json!(self.ui.text_focused());
                    d
                }
                None => Value::Null,
            },
            UiKind::Versus => {
                let (setup, left) = match &self.versus {
                    Some(v) => (Some(&v.0), v.1),
                    None => (None, 0),
                };
                let name = |i: usize| {
                    self.mem
                        .get(&format!("name{i}"))
                        .map(|(_, _, b)| String::from_utf8_lossy(b).to_string())
                        .unwrap_or_default()
                };
                let side = |i: usize| {
                    let has = self.mem.contains_key(&format!("p{i}"));
                    json!({ "name": name(i), "name_up": name(i).to_uppercase(), "art": format!("mem:p{i}"), "has_art": has })
                };
                let (stage_name, stage_thumb) = setup
                    .and_then(|s| s.stage.as_ref())
                    .map(|p| stage_card(p))
                    .unwrap_or_default();
                json!({
                    "p1": side(1),
                    "p2": side(2),
                    "stage": { "name": stage_name, "thumb": stage_thumb, "has_thumb": !stage_thumb.is_empty() },
                    "countdown": (left + 59) / 60,
                    "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                })
            }
            UiKind::Result => match &self.result {
                Some(r) => {
                    let w = r.winner;
                    let winner_name = w.map(|i| r.names[i].clone()).unwrap_or_default();
                    let has_art = w.is_some_and(|i| r.arts[i].is_some());
                    let headline = match w {
                        Some(_) => "WINNER",
                        None => "DRAW",
                    };
                    json!({
                        "headline": headline,
                        "winner_up": winner_name.to_uppercase(),
                        "winner": winner_name,
                        "winner_art": w.map(|i| format!("mem:p{}", i + 1)).unwrap_or_default(),
                        "has_art": has_art,
                        "p1": r.names[0],
                        "p2": r.names[1],
                        "rounds": format!("{} - {}", r.wins[0], r.wins[1]),
                        "rating": r.rating,
                        "has_rating": !r.rating.is_empty(),
                        "quick": r.quick,
                        "arcade": r.arcade,
                        "can_select": !r.quick && !r.arcade && r.setup.mode.is_some(),
                        "can_rematch": !r.arcade,
                        "can_title": !r.quick,
                        "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                        "quote": r.quote,
                        "stats": r.stats.iter().map(|(k, v)| json!({ "label": k, "value": v })).collect::<Vec<_>>(),
                        "loser": match w {
                            Some(i) => json!({
                                "name": r.names[1 - i],
                                "art": format!("mem:p{}", 2 - i),
                                "has_art": r.arts[1 - i].is_some(),
                            }),
                            None => json!({ "name": "", "art": "", "has_art": false }),
                        },
                        "draw": w.is_none(),
                        "menu": result_menu(r),
                    })
                }
                None => Value::Null,
            },
            UiKind::Pause => {
                let info = self.pause.as_ref();
                let moves = info.map(|p| p.moves.as_slice()).unwrap_or(&[]);
                let pages = moves.len().div_ceil(MOVES_PER_PAGE).max(1);
                let page = self.moves_page % pages;
                let shown: Vec<Value> = moves
                    .iter()
                    .skip(page * MOVES_PER_PAGE)
                    .take(MOVES_PER_PAGE)
                    .map(|m| json!({ "name": m.name, "notation": m.notation, "super": m.super_move }))
                    .collect();
                let mut menu = vec![
                    json!({ "id": "resume", "label": "RESUME", "action": "resume", "style": "main" }),
                    json!({ "id": "moves", "label": "MOVE LIST", "action": "moves", "style": "main" }),
                    json!({ "id": "restart", "label": "RESTART ROUND", "action": "restart", "style": "main" }),
                    json!({ "id": "options", "label": "OPTIONS", "action": "options", "style": "main" }),
                ];
                if info.is_some_and(|p| p.mode.is_some()) {
                    menu.push(json!({ "id": "select", "label": "CHARACTER SELECT", "action": "select", "style": "main" }));
                }
                menu.push(json!({ "id": "quit", "label": "QUIT TO MENU", "action": "quit", "style": "paper", "paper": true }));
                let cmd = info.map(|p| p.cmd.clone()).unwrap_or_default();
                json!({
                    "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                    "version": env!("CARGO_PKG_VERSION"),
                    "menu": menu,
                    "side": format!("{}P", info.map_or(1, |p| p.side + 1)),
                    "name": info.map(|p| p.name.to_uppercase()).unwrap_or_default(),
                    "art": "mem:pauser",
                    "has_art": self.mem.contains_key("pauser"),
                    "scene": "mem:scene",
                    "has_scene": self.mem.contains_key("scene"),
                    "moves": shown,
                    "has_moves": !moves.is_empty(),
                    "no_moves": moves.is_empty(),
                    "source": if cmd.is_empty() { "NO COMMAND FILE".to_string() } else { format!("MOVE LIST · FROM {cmd}") },
                    "page": if pages > 1 { format!("PAGE {} / {}", page + 1, pages) } else { String::new() },
                    "keys": self.keys_json(),
                })
            }
            UiKind::Title => {
                let mut menu: Vec<Value> = sys
                    .title
                    .items
                    .iter()
                    .filter_map(|(k, label)| {
                        let action = match k.as_str() {
                            "arcade" | "versus" | "training" | "watch" | "options" => {
                                format!("mode:{k}")
                            }
                            "exit" => "quit".to_string(),
                            _ => return None,
                        };
                        Some(json!({ "id": k, "label": label, "action": action }))
                    })
                    .collect();
                let pos = |menu: &[Value], id: &str| {
                    menu.iter()
                        .position(|m| m["id"] == id)
                        .map(|i| i + 1)
                        .unwrap_or(menu.len())
                };
                let at = pos(&menu, "training");
                menu.insert(
                    at,
                    json!({ "id": "tutorial", "label": "TUTORIAL", "action": "mode:tutorial" }),
                );
                // ARENA (platform fighter rules) right after the two-player mode
                let at = pos(&menu, "versus");
                menu.insert(
                    at,
                    json!({ "id": "arena", "label": "ARENA", "action": "mode:arena" }),
                );
                if !menu.iter().any(|m| m["id"] == "options") {
                    let at = menu
                        .iter()
                        .position(|m| m["id"] == "exit")
                        .unwrap_or(menu.len());
                    menu.insert(
                        at,
                        json!({ "id": "options", "label": "OPTIONS", "action": "mode:options" }),
                    );
                }
                // always listed: the online screen explains what it needs when no server is set
                {
                    let at = menu
                        .iter()
                        .position(|m| m["id"] == "options")
                        .unwrap_or(menu.len());
                    menu.insert(
                        at,
                        json!({ "id": "online", "label": "ONLINE", "action": "online" }),
                    );
                    menu.insert(
                        at + 1,
                        json!({ "id": "profile", "label": "PROFILE", "action": "profile" }),
                    );
                }
                let can_try = !self.featured.is_empty();
                for m in &mut menu {
                    let id = m["id"].as_str().unwrap_or("").to_string();
                    m["desc"] = Value::from(menu_desc(&id));
                    m["style"] = Value::from(if id == "exit" { "exit" } else { "main" });
                    m["exit"] = Value::from(id == "exit");
                    if can_try {
                        m["right"] = Value::from("try");
                    }
                }
                let version = env!("CARGO_PKG_VERSION");
                let short: Vec<&str> = version.split('.').take(2).collect();
                let p = &self.profile;
                let rank = p.rank_card();
                json!({
                    "featured": self.featured_json(),
                    "arenas": self.arenas,
                    "vol": format!("VOL. {}", short.join(".")),
                    "ranked": { "title": rank.0, "rating": rank.1, "line": rank.2 },
                    "keys": self.keys_json(),
                    "menu": menu,
                    "news": self.news,
                    "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                    "version": env!("CARGO_PKG_VERSION"),
                })
            }
            UiKind::Options | UiKind::Controls => {
                let mut d = match &self.settings.controls {
                    Some(ctl) => ctl.data(&self.config, self.settings.ticks),
                    None => crate::settings::options_data(
                        self.settings.tab,
                        &self.config,
                        &self.themes,
                        &self.settings.root,
                        self.ui.focused(),
                    ),
                };
                if let Value::Object(m) = &mut d {
                    m.insert(
                        "profile".into(),
                        serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                    );
                    m.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
                }
                d
            }
        };
        self.ui.set_data(data);
    }

    pub fn capturing(&self) -> bool {
        self.ui.text_focused()
            || self
                .settings
                .controls
                .as_ref()
                .is_some_and(|c| c.listen.is_some())
    }

    // ------------------------------------------------------------ options / controls

    /// Open the options on tab `tab` with the focus on its tab slab.
    pub fn set_tab(&mut self, tab: usize, sys: &SystemData) {
        self.settings.tab = tab.min(crate::settings::TABS.len() - 1);
        self.ui
            .set_focus(Some(&format!("tabs[{}].tab", self.settings.tab)));
        self.refresh_data(sys);
    }

    /// Active options tab.
    pub fn options_tab(&self) -> usize {
        self.settings.tab
    }

    /// Key of the focused node, to restore it after a reload.
    pub fn focus_key(&self) -> Option<String> {
        self.ui.focused().map(str::to_string)
    }

    /// Focus `key` again (the options re-opened in another theme share keys).
    pub fn restore_focus(&mut self, key: &str) {
        self.ui.set_focus(Some(key));
    }

    /// Show `player`'s bindings on the controls page.
    pub fn set_player(&mut self, player: usize, sys: &SystemData) {
        if let Some(c) = self.settings.controls.as_mut() {
            c.player = player.min(1);
        }
        self.ui.set_focus(Some("binds[0].row"));
        self.refresh_data(sys);
    }

    /// Connected gamepads (name, held buttons) for the controls page device chip / test pad.
    pub fn set_pads(&mut self, pads: Vec<(String, Buttons)>, sys: &SystemData) {
        if let Some(c) = self.settings.controls.as_mut()
            && (c.pads != pads)
        {
            c.pads = pads;
            if c.device > c.pads.len() {
                c.device = 0;
            }
            self.refresh_data(sys);
        }
    }

    /// Escape: `None` when the screen handled it (options content → tab column),
    /// else where to go.
    pub fn escape(&mut self, sys: &SystemData) -> Option<Transition> {
        match self.kind {
            UiKind::Options if self.settings.controls.is_none() => {
                let on_tabs = self.ui.focused().is_some_and(|f| f.starts_with("tabs["));
                if on_tabs || !self.ui.data()["tabs"].is_array() {
                    return Some(Transition::Title);
                }
                self.set_tab(self.settings.tab, sys);
                None
            }
            UiKind::Controls => Some(Transition::Options),
            UiKind::Select(mode) => {
                // take back the last pick first, leave when there is none
                if self.roster.as_mut().is_some_and(|r| r.cancel(None)) {
                    self.refresh_data(sys);
                    return None;
                }
                Some(if mode == Mode::Online {
                    Transition::Online
                } else {
                    Transition::Title
                })
            }
            _ => Some(Transition::Title),
        }
    }

    /// Title: Esc first moves the focus onto EXIT; true when it did (stay).
    pub fn focus_exit(&mut self) -> bool {
        if self.kind != UiKind::Title {
            return false;
        }
        let Some(i) = self.ui.data()["menu"]
            .as_array()
            .and_then(|m| m.iter().position(|m| m["id"] == "exit"))
        else {
            return false;
        };
        let key = format!("menu[{i}].exit");
        if self.ui.focused() == Some(key.as_str()) {
            return false;
        }
        self.ui.set_focus(Some(&key));
        true
    }

    /// A player's B button (pad East): cancel / back, per screen. `side` is the
    /// player on a two-cursor select screen.
    fn back(
        &mut self,
        side: Option<usize>,
        sys: &SystemData,
        sfx: &mut dyn FnMut(Option<(i32, i32)>),
    ) -> Option<Transition> {
        match self.kind {
            UiKind::Select(mode) => {
                sfx(sys.select.cancel_snd);
                if self.roster.as_mut().is_some_and(|r| r.cancel(side)) {
                    self.refresh_data(sys);
                    return None;
                }
                Some(if mode == Mode::Online {
                    Transition::Online
                } else {
                    Transition::Title
                })
            }
            UiKind::Pause => Some(Transition::Resume),
            UiKind::Options => self.escape(sys),
            // B is a fight button: it must not quit the game, leave a result or the bindings page
            UiKind::Title | UiKind::Result | UiKind::Controls | UiKind::Versus => None,
        }
    }

    /// Raw keys on the controls page (held keys light the test pad; Tab, Backspace,
    /// Delete, F5, Enter are page keys). Returns a transition when Enter pressed a link.
    pub fn controls_key(
        &mut self,
        key: &Key,
        pressed: bool,
        sys: &SystemData,
    ) -> Option<Transition> {
        let focused = self.ui.focused().map(str::to_string);
        let ctl = self.settings.controls.as_mut()?;
        let before = (self.config.input.p1.clone(), self.config.input.p2.clone());
        let out = ctl.key(&mut self.config, key, pressed, focused.as_deref());
        let changed = before.0.names() != self.config.input.p1.names()
            || before.1.names() != self.config.input.p2.names();
        if changed {
            self.dirty = true;
        }
        let mut t = None;
        if out && let Some(a) = self.ui.focused_action() {
            t = self.action(&a, sys);
        }
        self.refresh_data(sys);
        t
    }

    /// True for the keys `controls_key` keeps from the player mapping.
    pub fn controls_owns(&self, key: &Key) -> bool {
        self.settings.controls.is_some()
            && matches!(
                key,
                Key::Named(
                    NamedKey::Tab
                        | NamedKey::Backspace
                        | NamedKey::Delete
                        | NamedKey::F5
                        | NamedKey::Enter
                )
            )
    }

    /// Raw key while rebinding or editing a text input. Returns true when consumed.
    pub fn raw_key(&mut self, key: &Key, sys: &SystemData) -> bool {
        if self.ui.text_focused() {
            if self.roster.is_some() {
                match key {
                    // arrows (and Tab again) close the roster search and navigate:
                    // the filter stays, the cursor never gets stuck in the box
                    Key::Named(
                        NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::ArrowLeft
                        | NamedKey::ArrowRight
                        | NamedKey::Tab,
                    ) => {
                        self.ui.stop_edit();
                        self.refresh_data(sys);
                        return matches!(key, Key::Named(NamedKey::Tab));
                    }
                    // Esc drops the filter as well
                    Key::Named(NamedKey::Escape) => {
                        self.ui.set_input("search", "");
                        self.ui.stop_edit();
                        self.action("query:", sys);
                        self.refresh_data(sys);
                        return true;
                    }
                    _ => {}
                }
            }
            if let Some(k) = text_key(key) {
                self.ui.text_key(k);
            }
            if !self.ui.text_focused() {
                self.refresh_data(sys);
            }
            return true;
        }
        if let Some(ctl) = self.settings.controls.as_mut()
            && ctl.listen.is_some()
        {
            let before = self.config.input.clone();
            ctl.bind(&mut self.config, key);
            if before.p1.names() != self.config.input.p1.names()
                || before.p2.names() != self.config.input.p2.names()
            {
                self.dirty = true;
            }
            self.refresh_data(sys);
            return true;
        }
        false
    }

    pub fn mouse(&mut self, ev: MouseEvent) {
        match ev {
            MouseEvent::Move(x, y) => self.mouse_px = Some((x, y)),
            MouseEvent::Left => {
                self.mouse_px = None;
                self.ui.mouse_move(None);
            }
            MouseEvent::Button(down) => self.ui.mouse_button(down),
            MouseEvent::Wheel(lines) => self.ui.mouse_wheel(lines),
        }
    }

    fn adjust(&mut self, id: &str, dir: i32, wrap: bool) {
        if crate::settings::adjust(&mut self.config, id, dir, wrap) {
            self.dirty = true;
        }
    }

    fn action(&mut self, a: &str, sys: &SystemData) -> Option<Transition> {
        if let Some(r) = self.roster.as_mut() {
            if a == "search" {
                self.ui.edit("search");
                self.refresh_data(sys);
                return None;
            }
            return match r.action(a) {
                Outcome::None => None,
                Outcome::Refresh => {
                    self.refresh_data(sys);
                    None
                }
                Outcome::Go(t) => Some(*t),
            };
        }
        if let Some((setup, _)) = &self.versus {
            return match a {
                "fight" | "skip" => Some(Transition::Fight(setup.clone())),
                "back" | "title" => Some(Transition::Title),
                _ => None,
            };
        }
        if let Some(r) = &self.result {
            return match a {
                "rematch" => Some(Transition::Fight(r.setup.clone())),
                "select" => Some(match r.setup.mode {
                    Some(m) => Transition::Select(m),
                    None => Transition::Title,
                }),
                "continue" => Some(Transition::Continue),
                "title" | "back" => Some(Transition::Title),
                "quit" => Some(Transition::Quit),
                _ => None,
            };
        }
        if let Some(ctl) = self.settings.controls.as_mut() {
            let before = self.config.input.clone();
            if ctl.action(&mut self.config, a) {
                if before.p1.names() != self.config.input.p1.names()
                    || before.p2.names() != self.config.input.p2.names()
                {
                    self.dirty = true;
                }
                self.refresh_data(sys);
                return None;
            }
            return match a {
                "back" => Some(Transition::Options),
                _ => None,
            };
        }
        if self.kind == UiKind::Pause {
            return match a {
                "resume" | "back" => Some(Transition::Resume),
                "options" => Some(Transition::Options),
                "restart" => Some(Transition::Restart),
                "moves" => {
                    self.moves_page += 1;
                    self.refresh_data(sys);
                    None
                }
                "select" => self
                    .pause
                    .as_ref()
                    .and_then(|p| p.mode)
                    .map(Transition::Select),
                "title" | "quit" => Some(Transition::Title),
                _ => None,
            };
        }
        if self.kind == UiKind::Title && a == "try" {
            return self.try_featured();
        }
        let mut parts = a.splitn(3, ':');
        let head = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("");
        let arg2 = parts.next().unwrap_or("");
        match head {
            "quit" => Some(Transition::Quit),
            "netplay" => Some(Transition::Netplay),
            "online" => Some(Transition::Online),
            "profile" => Some(Transition::Profile),
            "back" | "title" => Some(Transition::Title),
            "mode" => Some(match arg {
                "arcade" => Transition::Select(Mode::Arcade),
                "versus" => Transition::Select(Mode::Versus),
                "training" => Transition::Select(Mode::Training),
                "arena" => Transition::Select(Mode::Arena),
                "tutorial" => Transition::Select(Mode::Tutorial),
                "watch" => Transition::Select(Mode::Watch),
                "options" => Transition::Options,
                _ => return None,
            }),
            "adjust" => {
                self.adjust(arg, arg2.parse().unwrap_or(1), false);
                self.refresh_data(sys);
                None
            }
            "cycle" => {
                self.adjust(arg, 1, true);
                self.refresh_data(sys);
                None
            }
            "open" => match arg {
                "keys1" | "controls" | "controls1" => Some(Transition::Controls(0)),
                "keys2" | "controls2" => Some(Transition::Controls(1)),
                _ => None,
            },
            "tab" => {
                let i = arg.parse::<usize>().unwrap_or(0);
                if i == crate::settings::CONTROLS_TAB {
                    return Some(Transition::Controls(0));
                }
                self.settings.tab = i.min(crate::settings::TABS.len() - 1);
                self.refresh_data(sys);
                // into the tab: first row, or the first theme card
                let first = if self.ui.data()["show_rows"] == json!(true) {
                    "rows[0].row"
                } else {
                    "themes[0].card"
                };
                self.ui.set_focus(Some(first));
                None
            }
            "theme" => {
                if !arg.is_empty() && !self.config.video.theme.eq_ignore_ascii_case(arg) {
                    self.config.video.theme = arg.to_string();
                    self.dirty = true;
                }
                self.refresh_data(sys);
                None
            }
            _ => {
                log::warn!("ui: unknown action {a:?}");
                None
            }
        }
    }

    /// Title: jump into training with the featured fighter.
    fn try_featured(&self) -> Option<Transition> {
        let f = self.featured.get(self.feature_at)?;
        let stage = self.try_stage.clone();
        let id = rugen_content::manifest_for(&f.def)
            .map(|(_, m)| m.id)
            .unwrap_or_default();
        Some(Transition::Fight(FightSetup {
            p1: f.def.clone(),
            p2: f.def.clone(),
            stage,
            ai: [0, 0],
            net: None,
            tutorial: false,
            ids: [id.clone(), id],
            mode: Some(Mode::Training),
        }))
    }

    /// One tick: navigation from both players, mouse, actions → transition.
    pub fn tick(
        &mut self,
        pressed: [Buttons; 2],
        sys: &mut SystemData,
        r: &mut Renderer,
        sfx: &mut dyn FnMut(Option<(i32, i32)>),
    ) -> Option<Transition> {
        if let Some(t) = self.pending.take() {
            return Some(t);
        }
        self.ticks += 1;
        if self.kind == UiKind::Title && self.featured.len() > 1 && self.ticks % FEATURE_TICKS == 0
        {
            self.feature_at = (self.feature_at + 1) % self.featured.len();
            self.refresh_data(sys);
        }
        self.window = r.size();
        let (vw, vh) = viewport(self.window);
        self.ui.set_size(vw, vh);
        if let Some((px, py)) = self.mouse_px {
            self.ui
                .mouse_move(Some(rugen_ui::window_to_virtual(self.window, px, py)));
        }
        // A (and the other attack buttons, Start) accept; B backs out
        let of = |b: &Buttons| Nav {
            up: b.u,
            down: b.d,
            left: b.l,
            right: b.r,
            accept: b.a || b.c || b.x || b.y || b.z || b.s,
            back: b.b,
        };
        let dual = self.roster.as_ref().is_some_and(|r| r.dual());
        let listening = self
            .settings
            .controls
            .as_ref()
            .is_some_and(|c| c.listen.is_some());
        let (mut nav, nav2) = if listening {
            (Nav::default(), Nav::default())
        } else if dual {
            (of(&pressed[0]), of(&pressed[1]))
        } else {
            let merged = Buttons::from_bits(pressed[0].bits() | pressed[1].bits());
            (of(&merged), Nav::default())
        };
        if self.kind == UiKind::Controls {
            // fight buttons only light the test pad here; Enter / clicks press, Esc leaves
            nav.accept = false;
            nav.back = false;
            self.settings.ticks += 1;
            if self.settings.ticks % 10 == 0 {
                self.refresh_data(sys);
            }
        }
        if self.kind == UiKind::Options && (nav.up || nav.right) {
            let f = self.ui.focused().unwrap_or("").to_string();
            if nav.up && (f == "rows[0].row" || f.starts_with("themes[")) {
                // up from the top of a tab goes back to the tab column
                self.ui
                    .set_focus(Some(&format!("tabs[{}].tab", self.settings.tab)));
                nav.up = false;
                sfx(sys.title.move_snd);
            } else if nav.right
                && f.starts_with("tabs[")
                && self.settings.tab != crate::settings::CONTROLS_TAB
            {
                // right from a tab enters it at the top
                let first = if self.ui.data()["show_rows"] == json!(true) {
                    "rows[0].row"
                } else {
                    "themes[0].card"
                };
                self.ui.set_focus(Some(first));
                nav.right = false;
                sfx(sys.title.move_snd);
            }
        }
        // VS screen: count down, any button skips
        let mut vs_done = None;
        let mut vs_refresh = false;
        if let Some((setup, left)) = self.versus.as_mut() {
            *left -= 1;
            if *left <= 0 || nav.accept || nav2.accept || nav.back || nav2.back {
                vs_done = Some(setup.clone());
            }
            vs_refresh = *left % 60 == 0;
        }
        if let Some(s) = vs_done {
            return Some(Transition::Fight(s));
        }
        if vs_refresh {
            self.refresh_data(sys);
        }
        let events = {
            let mut host = Host {
                r,
                fonts: &mut sys.fonts,
                sheet: &mut sys.sheet,
                textures: &mut self.textures,
                motif: &self.motif,
                mem: Some(&self.mem),
                rot: None,
            };
            self.ui.tick2(nav, nav2, &mut host)
        };
        let mut out = None;
        for e in events {
            match e {
                Event::Moved | Event::Moved2 => sfx(sys.title.move_snd),
                Event::Back => {
                    let side = dual.then_some(0);
                    if let Some(t) = self.back(side, sys, sfx) {
                        out = Some(t);
                    }
                }
                Event::Action(a) => {
                    if !a.starts_with("query:") {
                        sfx(sys.title.done_snd);
                    }
                    if let Some(t) = self.action(&a, sys) {
                        out = Some(t);
                    }
                }
                Event::Action2(a) => {
                    // the second cursor only picks (and unpicks) on the roster
                    if let Some(rest) = a.strip_prefix("pick:") {
                        sfx(sys.title.done_snd);
                        if let Some(t) = self.action(&format!("pick2:{rest}"), sys) {
                            out = Some(t);
                        }
                    } else if a == "random" {
                        sfx(sys.title.done_snd);
                        if let Some(t) = self.action("random2", sys) {
                            out = Some(t);
                        }
                    } else if a == "confirm" || a.starts_with("stage:") {
                        // FIGHT!! and the stage picker answer either player
                        sfx(sys.title.done_snd);
                        if let Some(t) = self.action(&a, sys) {
                            out = Some(t);
                        }
                    }
                }
                Event::Input(id, text) => {
                    if id == "search" {
                        self.action(&format!("query:{text}"), sys);
                    }
                }
            }
        }
        if dual
            && nav2.back
            && out.is_none()
            && let Some(t) = self.back(Some(1), sys, sfx)
        {
            out = Some(t);
        }
        // options: the focused tab is the tab on display; the help line follows the focus
        if self.kind == UiKind::Options {
            let f = self.ui.focused().map(str::to_string);
            if f != self.settings.last_focus {
                if let Some(i) = f
                    .as_deref()
                    .and_then(|f| f.strip_prefix("tabs["))
                    .and_then(|r| r.split(']').next())
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    self.settings.tab = i.min(crate::settings::TABS.len() - 1);
                }
                self.settings.last_focus = f;
                self.refresh_data(sys);
            }
        }
        // roster: the focused cell is the character on display
        if let Some(r) = self.roster.as_mut()
            && let Some(f) = self.ui.focused()
            && let Some(rest) = f.strip_prefix("cells[").or_else(|| f.strip_prefix("grid["))
            && let Some(end) = rest.find(']')
            && let Ok(i) = rest[..end].parse::<usize>()
            && r.hover_order(i)
        {
            self.refresh_data(sys);
        }
        if let Some(r) = self.roster.as_mut()
            && let Some(f) = self.ui.focused2()
            && let Some(rest) = f.strip_prefix("grid[")
            && let Some(end) = rest.find(']')
            && let Ok(i) = rest[..end].parse::<usize>()
            && r.hover2_order(i)
        {
            self.refresh_data(sys);
        }
        out
    }

    pub fn render(&mut self, r: &mut Renderer, sys: &mut SystemData) -> (Camera, [f64; 4]) {
        self.window = r.size();
        let (vw, vh) = viewport(self.window);
        self.ui.set_size(vw, vh);
        {
            let mut host = Host {
                r,
                fonts: &mut sys.fonts,
                sheet: &mut sys.sheet,
                textures: &mut self.textures,
                motif: &self.motif,
                mem: Some(&self.mem),
                rot: None,
            };
            self.ui.draw(&mut host);
        }
        let bg = self.ui.background();
        (
            Camera {
                screen: [vw, vh],
                center: [vw / 2.0, vh / 2.0],
                zoom: 1.0,
            },
            [bg[0] as f64, bg[1] as f64, bg[2] as f64, 1.0],
        )
    }
}

/// Buttons under the result screen.
fn result_menu(r: &ResultInfo) -> Vec<Value> {
    let mut menu = Vec::new();
    let mut add = |id: &str, label: &str| {
        menu.push(json!({ "id": id, "label": label, "action": id, "style": "main" }));
    };
    if r.arcade {
        add("continue", "CONTINUE");
    }
    if !r.arcade {
        add("rematch", "REMATCH");
    }
    if !r.quick && !r.arcade && r.setup.mode.is_some() {
        add("select", "CHARACTER SELECT");
    }
    if !r.quick {
        add("title", "MAIN MENU");
    }
    if r.quick {
        add("quit", "QUIT");
    }
    menu
}

/// Decode a WebP or PNG file to straight RGBA.
/// Resolve a relative screenpack file through the theme → flat `motif/` chain.
pub fn motif_file(motif: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let mut dirs = vec![motif.to_path_buf()];
    if let Some(parent) = motif.parent()
        && parent.file_name().is_some_and(|n| n == "themes")
        && let Some(flat) = parent.parent()
    {
        dirs.push(flat.to_path_buf());
    }
    dirs.into_iter().map(|d| d.join(rel)).find(|p| p.exists())
}

pub fn load_image(path: &Path) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "webp" => decode_webp(&std::fs::read(path)?)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display())),
        "png" => {
            let (w, h, rgba) = rugen_content::read_png(path)?;
            Ok((w, h, rgba))
        }
        _ => anyhow::bail!("{}: unsupported image type", path.display()),
    }
}

/// Decode a WebP (file contents or a card portrait off the wire) to RGBA.
pub fn decode_webp(bytes: &[u8]) -> anyhow::Result<MemImage> {
    let mut dec = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| anyhow::anyhow!("webp: {e}"))?;
    let (w, h) = dec.dimensions();
    if w > 8192 || h > 8192 {
        anyhow::bail!("webp: {w}x{h} is too large");
    }
    let mut buf = vec![0u8; dec.output_buffer_size().unwrap_or(0)];
    dec.read_image(&mut buf)
        .map_err(|e| anyhow::anyhow!("webp: {e}"))?;
    if !dec.has_alpha() {
        // RGB → RGBA
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for px in buf.chunks_exact(3) {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        buf = rgba;
    }
    Ok((w, h, buf))
}

/// Stage display name and thumbnail path for a prepared stage .def.
fn stage_card(def: &Path) -> (String, String) {
    let found = rugen_content::manifest_for(def);
    let dir = found.as_ref().map(|(d, _)| d.as_path());
    let man = found.as_ref().map(|(_, m)| m.clone());
    let name = man
        .as_ref()
        .map(|m| m.display_name.clone())
        .or_else(|| def.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default()
        .to_uppercase();
    let thumb = man
        .as_ref()
        .and_then(|m| m.thumb.as_ref())
        .and_then(|t| dir.map(|d| d.join(&t.file).display().to_string()))
        .unwrap_or_default();
    (name, thumb)
}

/// Painter over the renderer, motif fonts and the system sprite sheet.
pub(crate) struct Host<'a> {
    pub(crate) r: &'a mut Renderer,
    pub(crate) fonts: &'a mut HashMap<i32, FontGpu>,
    pub(crate) sheet: &'a mut SpriteSheet,
    pub(crate) textures: &'a mut HashMap<String, Cached>,
    pub(crate) motif: &'a Path,
    /// `mem:<name>` images owned by the screen.
    pub(crate) mem: Option<&'a HashMap<String, MemImage>>,
    /// Rotation of the subtree being drawn (`angle` nodes).
    pub(crate) rot: Option<rugen_ui::Rot>,
}

impl Host<'_> {
    fn texture(&mut self, key: &str) -> Cached {
        if let Some(c) = self.textures.get(key) {
            return *c;
        }
        let loaded: Cached = if let Some(name) = key.strip_prefix("mem:") {
            self.mem
                .and_then(|m| m.get(name))
                .filter(|(w, h, _)| *w > 0 && *h > 0)
                .map(|(w, h, rgba)| (self.r.add_rgba(*w, *h, rgba), *w, *h, None))
        } else if let Some(spec) = key.strip_prefix("sff:") {
            let mut it = spec
                .split(',')
                .map(|s| s.trim().parse::<i32>().unwrap_or(0));
            let (g, n) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
            self.sheet
                .get(self.r, g, n)
                .map(|p| (p.tex, p.size[0] as u32, p.size[1] as u32, p.pal))
        } else {
            let path = motif_file(self.motif, key).unwrap_or_else(|| self.motif.join(key));
            match load_image(&path) {
                Ok((w, h, rgba)) => Some((self.r.add_rgba(w, h, &rgba), w, h, None)),
                Err(e) => {
                    log::warn!("ui asset {key}: {e}");
                    None
                }
            }
        };
        self.textures.insert(key.to_string(), loaded);
        loaded
    }

    fn clip_of(c: Option<Rect>) -> Option<[f32; 4]> {
        c.map(|c| [c.x, c.y, c.x + c.w, c.y + c.h])
    }

    /// Rotate `d` around the current subtree pivot, keeping its unrotated placement.
    fn apply_rot(&self, d: &mut Draw) {
        let Some(rot) = self.rot else {
            return;
        };
        let (ax, ay) = (d.axis[0] * d.scale[0], d.axis[1] * d.scale[1]);
        let nax = if d.hflip {
            d.pos[0] + ax - rot.cx
        } else {
            rot.cx - d.pos[0] + ax
        };
        let nay = if d.vflip {
            d.pos[1] + ay - rot.cy
        } else {
            rot.cy - d.pos[1] + ay
        };
        d.axis = [
            nax / d.scale[0].max(f32::EPSILON),
            nay / d.scale[1].max(f32::EPSILON),
        ];
        d.pos = [rot.cx, rot.cy];
        d.angle = -rot.deg;
        d.clip_local = true;
    }
}

impl Painter for Host<'_> {
    fn image_size(&mut self, key: &str) -> Option<(u32, u32)> {
        self.texture(key).map(|(_, w, h, _)| (w, h))
    }

    fn draw_image(
        &mut self,
        key: &str,
        src: Option<[u32; 4]>,
        dst: Rect,
        skew: f32,
        alpha: f32,
        clip: Option<Rect>,
    ) {
        let fx = rugen_ui::ImageFx {
            skew,
            alpha,
            flip: false,
            grey: false,
        };
        self.draw_image_fx(key, src, dst, &fx, clip);
    }

    fn draw_image_fx(
        &mut self,
        key: &str,
        src: Option<[u32; 4]>,
        dst: Rect,
        fx: &rugen_ui::ImageFx,
        clip: Option<Rect>,
    ) {
        let Some((tex, w, h, pal)) = self.texture(key) else {
            return;
        };
        let (sw, sh) = src.map_or((w, h), |s| (s[2], s[3]));
        if sw == 0 || sh == 0 {
            return;
        }
        let mut d = Draw::new(tex, pal, [dst.x, dst.y], [0.0, 0.0]);
        d.src = src;
        d.scale = [dst.w / sw as f32, dst.h / sh as f32];
        d.color = [1.0, 1.0, 1.0, fx.alpha];
        d.skew = fx.skew;
        d.clip = Self::clip_of(clip);
        if fx.flip {
            d.hflip = true;
            d.pos[0] = dst.x + dst.w;
        }
        if fx.grey {
            d.tint.color = 0.0;
        }
        self.apply_rot(&mut d);
        self.r.draw(d);
    }

    fn set_transform(&mut self, rot: Option<rugen_ui::Rot>) {
        self.rot = rot;
    }

    fn draw_rect(&mut self, dst: Rect, color: [f32; 4], skew: f32, clip: Option<Rect>) {
        let tex = self.r.white();
        let mut d = Draw::new(tex, None, [dst.x, dst.y], [0.0, 0.0]);
        d.scale = [dst.w, dst.h];
        d.color = color;
        d.skew = skew;
        d.clip = Self::clip_of(clip);
        self.apply_rot(&mut d);
        self.r.draw(d);
    }

    fn text_width(&mut self, font: i32, text: &str, scale: f32) -> f32 {
        self.fonts
            .get(&font)
            .map_or(text.len() as f32 * 8.0 * scale, |f| f.width(text, scale))
    }

    fn text_height(&mut self, font: i32, scale: f32) -> f32 {
        self.fonts
            .get(&font)
            .map_or(12.0 * scale, |f| f.font.size[1] as f32 * scale)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        font: i32,
        text: &str,
        x: f32,
        y: f32,
        bank: i32,
        align: i32,
        scale: f32,
        alpha: f32,
        clip: Option<Rect>,
    ) {
        // start the text on a whole window pixel: pixel fonts scaled by a
        // fractional virtual-to-window factor (1.5 at 1080p) then keep every
        // font pixel the same width
        let win = self.r.size();
        let (vw, vh) = viewport(win);
        let k = [win.0 as f32 / vw, win.1 as f32 / vh];
        if let Some(f) = self.fonts.get_mut(&font) {
            f.snap = Some(k);
            f.draw_xf(
                self.r,
                text,
                [x, y],
                bank,
                align,
                scale,
                Self::clip_of(clip),
                alpha,
                self.rot.map(|r| (-r.deg, [r.cx, r.cy])),
            );
            f.snap = None;
        }
    }
}
