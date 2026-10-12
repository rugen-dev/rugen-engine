//! Host for `rugen-ui` screens: loads `motif/ui/<name>.toml`, paints with the
//! renderer and the motif fonts, decodes WebP/PNG/SFF assets, maps the mouse,
//! and turns actions into shell transitions. The title and options screens
//! live here when the motif ships `ui/title.toml` / `ui/options.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rugen_render::{Camera, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::Buttons;
use rugen_ui::{Event, Nav, Painter, Rect, ScreenDef, TextKey, Ui, VIRTUAL_H, VIRTUAL_W};
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
    netplay: bool,
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

/// Virtual viewport for a window: 1280 wide on 16:9, letterboxed otherwise.
pub(crate) fn viewport(window: (u32, u32)) -> (f32, f32) {
    let aspect = window.0.max(1) as f32 / window.1.max(1) as f32;
    if aspect >= VIRTUAL_W / VIRTUAL_H {
        (VIRTUAL_H * aspect, VIRTUAL_H)
    } else {
        (VIRTUAL_W, VIRTUAL_W / aspect)
    }
}

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
        let mut s = Self::load(UiKind::Versus, motif, root, sys, config, false, lib)?;
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
        let mut s = Self::load(UiKind::Result, motif, root, sys, config, false, lib)?;
        for (i, art) in info.arts.iter().enumerate() {
            if let Some(img) = art {
                s.mem.insert(format!("p{}", i + 1), img.clone());
            }
        }
        s.result = Some(info);
        s.refresh_data(sys);
        Ok(s)
    }

    /// Pause menu.
    pub fn load_pause(
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        lib: &rugen_content::Library,
    ) -> anyhow::Result<Self> {
        Self::load(UiKind::Pause, motif, root, sys, config, false, lib)
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
        netplay: bool,
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
            netplay,
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
            room: "LOCAL".into(),
            versus: None,
            result: None,
            mem: HashMap::new(),
            settings: SettingsPage {
                root: root.to_path_buf(),
                controls: (kind == UiKind::Controls).then(|| crate::settings::Controls::new(0)),
                ..Default::default()
            },
        };
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
                Some(r) => r.data(&self.profile, &self.room),
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
                    json!({ "name": name(i), "art": format!("mem:p{i}"), "has_art": has })
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
                    })
                }
                None => Value::Null,
            },
            UiKind::Pause => json!({
                "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                "version": env!("CARGO_PKG_VERSION"),
            }),
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
                if !self.config.net.server.trim().is_empty() {
                    let at = menu
                        .iter()
                        .position(|m| m["id"] == "options")
                        .unwrap_or(menu.len());
                    menu.insert(
                        at,
                        json!({ "id": "online", "label": "ONLINE", "action": "online" }),
                    );
                }
                if self.netplay {
                    let at = menu
                        .iter()
                        .position(|m| m["id"] == "exit")
                        .unwrap_or(menu.len());
                    menu.insert(
                        at,
                        json!({ "id": "netplay", "label": "NETPLAY", "action": "netplay" }),
                    );
                }
                json!({
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
            _ => Some(Transition::Title),
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
            if let Some(k) = text_key(key) {
                self.ui.text_key(k);
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
                "title" | "quit" => Some(Transition::Title),
                _ => None,
            };
        }
        let mut parts = a.splitn(3, ':');
        let head = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("");
        let arg2 = parts.next().unwrap_or("");
        match head {
            "quit" => Some(Transition::Quit),
            "netplay" => Some(Transition::Netplay),
            "online" => Some(Transition::Online),
            "back" | "title" => Some(Transition::Title),
            "mode" => Some(match arg {
                "arcade" => Transition::Select(Mode::Arcade),
                "versus" => Transition::Select(Mode::Versus),
                "training" => Transition::Select(Mode::Training),
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
        self.window = r.size();
        if let Some((px, py)) = self.mouse_px {
            let (vw, vh) = viewport(self.window);
            let vx =
                VIRTUAL_W / 2.0 + (px - self.window.0 as f32 / 2.0) * vw / self.window.0 as f32;
            let vy =
                VIRTUAL_H / 2.0 + (py - self.window.1 as f32 / 2.0) * vh / self.window.1 as f32;
            self.ui.mouse_move(Some((vx, vy)));
        }
        let of = |b: &Buttons| Nav {
            up: b.u,
            down: b.d,
            left: b.l,
            right: b.r,
            accept: b.a || b.b || b.c || b.x || b.y || b.z || b.s,
            back: false,
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
            // fight buttons only light the test pad here; Enter / clicks press
            nav.accept = false;
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
            if *left <= 0 || nav.accept || nav2.accept {
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
            };
            self.ui.tick2(nav, nav2, &mut host)
        };
        let mut out = None;
        for e in events {
            match e {
                Event::Moved | Event::Moved2 => sfx(sys.title.move_snd),
                Event::Back => out = Some(Transition::Title),
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
                    }
                }
                Event::Input(id, text) => {
                    if id == "search" {
                        self.action(&format!("query:{text}"), sys);
                    }
                }
            }
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
            && let Some(rest) = f.strip_prefix("cells[")
            && let Some(end) = rest.find(']')
            && let Ok(i) = rest[..end].parse::<usize>()
            && r.hover_order(i)
        {
            self.refresh_data(sys);
        }
        out
    }

    pub fn render(&mut self, r: &mut Renderer, sys: &mut SystemData) -> (Camera, [f64; 4]) {
        self.window = r.size();
        {
            let mut host = Host {
                r,
                fonts: &mut sys.fonts,
                sheet: &mut sys.sheet,
                textures: &mut self.textures,
                motif: &self.motif,
                mem: Some(&self.mem),
            };
            self.ui.draw(&mut host);
        }
        let (vw, vh) = viewport(self.window);
        let bg = self.ui.background();
        (
            Camera {
                screen: [vw, vh],
                center: [VIRTUAL_W / 2.0, VIRTUAL_H / 2.0],
                zoom: 1.0,
            },
            [bg[0] as f64, bg[1] as f64, bg[2] as f64, 1.0],
        )
    }
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
        "webp" => {
            let file = std::io::BufReader::new(std::fs::File::open(path)?);
            let mut dec = image_webp::WebPDecoder::new(file)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            let (w, h) = dec.dimensions();
            let mut buf = vec![0u8; dec.output_buffer_size().unwrap_or(0)];
            dec.read_image(&mut buf)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
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
        "png" => {
            let (w, h, rgba) = rugen_content::read_png(path)?;
            Ok((w, h, rgba))
        }
        _ => anyhow::bail!("{}: unsupported image type", path.display()),
    }
}

/// Stage display name and thumbnail path for a prepared stage .def.
fn stage_card(def: &Path) -> (String, String) {
    let dir = def.parent();
    let man = dir
        .and_then(|d| std::fs::read_to_string(d.join("rugen.json")).ok())
        .and_then(|t| serde_json::from_str::<rugen_content::Manifest>(&t).ok());
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
        d.color = [1.0, 1.0, 1.0, alpha];
        d.skew = skew;
        d.clip = Self::clip_of(clip);
        self.r.draw(d);
    }

    fn draw_rect(&mut self, dst: Rect, color: [f32; 4], skew: f32, clip: Option<Rect>) {
        let tex = self.r.white();
        let mut d = Draw::new(tex, None, [dst.x, dst.y], [0.0, 0.0]);
        d.scale = [dst.w, dst.h];
        d.color = color;
        d.skew = skew;
        d.clip = Self::clip_of(clip);
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
        if let Some(f) = self.fonts.get_mut(&font) {
            f.draw_clipped(
                self.r,
                text,
                [x, y],
                bank,
                align,
                scale,
                Self::clip_of(clip),
                alpha,
            );
        }
    }
}
