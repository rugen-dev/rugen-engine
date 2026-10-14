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
    /// Stage select between the fighter select and the VS screen (`ui/stages.toml`).
    Stages,
    /// CHALLENGE menu: a card per challenge mode (`ui/challenge.toml`).
    Challenge,
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
    /// CHALLENGE bests per fighter (cache manifest id); profiles from
    /// before the modes have none.
    #[serde(default)]
    pub challenge: std::collections::BTreeMap<String, crate::challenge::ChallengeBest>,
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
            challenge: Default::default(),
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
    /// Stage select only.
    stages: Option<crate::stagesel::StageSel>,
    /// Buttons held this tick (auto-repeat on the stage strip).
    held: [Buttons; 2],
    /// Wheel turned over the stage select since the last tick.
    stage_wheel: f32,
    /// What each player last pressed with: the hints name its keys or the
    /// pad's buttons.
    devices: [crate::input::Device; 2],
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
    /// TRAINING pause menu: the panel on display (`moves`, `display`, ...).
    panel: &'static str,
    /// Focus key seen last tick (the training panel follows the menu focus).
    pause_focus: Option<String>,
    /// Title: installed fighters the feature panel cycles through (newest first).
    featured: Vec<Featured>,
    feature_at: usize,
    /// Title: arena facts, computed once.
    arenas: Value,
    /// Title: stage for the featured fighter's training run.
    try_stage: Option<PathBuf>,
    ticks: u64,
    /// Stage select: the library (default stage for an empty RANDOM).
    lib: Option<rugen_content::Library>,
    /// Controls page opened from the pause menu (Esc goes back to the match).
    from_pause: bool,
    /// CHALLENGE menu: the mode cards (rules and bests), computed once.
    challenge: Value,
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
    /// TRAINING: the training lab's settings, edited by the menu.
    pub training: Option<TrainingInfo>,
}

/// The pause menu of a TRAINING match: its tool settings and who fights.
#[derive(Clone, Debug)]
pub struct TrainingInfo {
    pub cfg: crate::training::TrainingCfg,
    /// Display names of P1 and the dummy (P2).
    pub fighters: [String; 2],
    /// Frames recorded in each slot.
    pub slots: [usize; crate::training::SLOTS],
    /// What P1 presses for START (it stops a recording).
    pub start: String,
}

impl TrainingInfo {
    /// The DUMMY panel's recorder block.
    fn rec_json(&self) -> Value {
        use crate::training::{RECORD_FRAMES, SLOTS};
        let slot = self.cfg.slot.min(SLOTS - 1);
        let n = self.slots[slot];
        let line = if n == 0 {
            "EMPTY · UP TO 10 S".to_string()
        } else {
            format!("{:.1} S OF 10 S · {n} FRAMES", n as f32 / 60.0)
        };
        json!({
            "title": format!("RECORDING · SLOT {} OF {SLOTS}", slot + 1),
            "pct": format!("{:.1}", n as f32 * 100.0 / RECORD_FRAMES as f32),
            "line": line,
            "help": format!(
                "RECORD GIVES YOU CONTROL OF THE DUMMY. PRESS {} TO STOP.",
                self.start.to_uppercase()
            ),
            "has": n > 0,
        })
    }
}

/// Panels of the training pause menu (beside the menu, instead of the move list).
const TRAINING_PANELS: [&str; 2] = ["dummy", "display"];

/// A fighter for the title's feature panel.
struct Featured {
    name: String,
    art: String,
    source: String,
    def: PathBuf,
}

/// Pause move list layout (virtual pixels): two columns this tall per page,
/// a group label row and a move row.
const MOVE_COL_H: f32 = 494.0;
const MOVE_GROUP_H: f32 = 30.0;
const MOVE_ROW_H: f32 = 38.0;
/// A move whose name and notation do not fit one line: notation underneath.
const MOVE_TALL_H: f32 = 62.0;
const MOVE_COL_W: f32 = 358.0;

/// The move list as pages of two columns of rows: group labels (marked
/// `(CONT.)` at the top of a column that continues a group) and moves. `width(font,
/// text)` measures text (font 1 = small, 2 = caption).
fn move_pages(
    moves: &[crate::screeninfo::Move],
    width: &dyn Fn(i32, &str) -> f32,
) -> Vec<[Vec<Value>; 2]> {
    // combos are the optional tail: left out when they alone need a page
    let all = layout_moves(moves, width);
    let core: Vec<crate::screeninfo::Move> = moves
        .iter()
        .filter(|m| m.group != crate::screeninfo::Group::Combo)
        .cloned()
        .collect();
    if core.len() < moves.len() && !core.is_empty() {
        let without = layout_moves(&core, width);
        if without.len() < all.len() {
            return without;
        }
    }
    all
}

fn layout_moves(
    moves: &[crate::screeninfo::Move],
    width: &dyn Fn(i32, &str) -> f32,
) -> Vec<[Vec<Value>; 2]> {
    use crate::screeninfo::Part;
    let mut cols: Vec<Vec<Value>> = vec![Vec::new()];
    let mut used = 0.0f32;
    let mut group = None;
    for m in moves {
        let new_group = group != Some(m.group);
        // the row as pause.toml draws it: name, cost tag; notation pieces
        // (key caps padded 7 px a side) 4 px apart
        let cost_w = m.cost.as_deref().map_or(0.0, |c| width(1, c) + 10.0 + 8.0);
        let notation_w: f32 = m
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => width(2, t),
                Part::Button(b) => width(2, b) + 14.0,
            })
            .sum::<f32>()
            + 4.0 * m.parts.len().saturating_sub(1) as f32;
        let tall = width(2, &m.name) + cost_w + 16.0 + notation_w > MOVE_COL_W;
        let row_h = if tall { MOVE_TALL_H } else { MOVE_ROW_H };
        let need = row_h + if new_group { MOVE_GROUP_H } else { 0.0 };
        if used + need > MOVE_COL_H && used > 0.0 {
            cols.push(Vec::new());
            used = 0.0;
        }
        let col = cols.last_mut().expect("one column");
        if new_group || col.is_empty() {
            // a group carried over from the previous column says so
            // instead of repeating its heading as if it started again
            let label = if new_group {
                m.group.label().to_string()
            } else {
                format!("{} (CONT.)", m.group.label())
            };
            col.push(json!({ "group": true, "label": label, "cont": !new_group }));
            used += MOVE_GROUP_H;
        }
        let parts: Vec<Value> = m
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => json!({ "text": t, "cap": false }),
                Part::Button(b) => json!({ "text": b, "cap": true }),
            })
            .collect();
        col.push(json!({
            "group": false,
            "tall": tall,
            "short": !tall,
            "name": m.name,
            "cost": m.cost.clone().unwrap_or_default(),
            "parts": parts,
        }));
        used += row_h;
        group = Some(m.group);
    }
    let mut pages = Vec::new();
    let mut it = cols.into_iter();
    while let Some(a) = it.next() {
        pages.push([a, it.next().unwrap_or_default()]);
    }
    pages
}
/// Ticks each featured fighter stays on the title.
const FEATURE_TICKS: u64 = 300;

/// Menu confirm / back key names for a device: Enter / Esc, or the pad's
/// South / East buttons by its family's names.
fn menu_hints(device: crate::input::Device) -> (String, String) {
    match device {
        crate::input::Device::Pad(kind) => (
            crate::keylabel::pad_cap(kind, 4).plain(),
            crate::keylabel::pad_cap(kind, 5).plain(),
        ),
        crate::input::Device::Keyboard => ("ENTER".into(), "ESC".into()),
    }
}

/// One-line description of a title menu entry.
fn menu_desc(id: &str) -> &'static str {
    match id {
        "arcade" => "Climb the ladder vs the CPU",
        "challenge" => "Runs against the CPU, best scores kept",
        "survival" => "One life bar, endless opponents",
        "bossrush" => "The toughest fighters back to back",
        "timeattack" => "Clear the ladder against the clock",
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

/// `1 ROOM`, `2 ROOMS`: a count with its noun in the right number.
pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
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
    /// The edits differ from rugen.toml (the host compares every tick).
    unsaved: bool,
    /// Ticks left of the SAVED toast.
    toast: u32,
    /// Leaving with unsaved changes: the SAVE / DISCARD / CANCEL dialog is
    /// up, holding where to go once it is answered.
    confirm: Option<Transition>,
    /// Controls opened from the pause menu: leaving goes back to the match.
    from_pause: bool,
    /// What the host has to do with the edits (see [`SaveRequest`]).
    request: Option<SaveRequest>,
}

/// Options / controls: the player asked to write the edits to rugen.toml or
/// to drop them (the live preview goes back to the saved settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveRequest {
    Save,
    Discard,
}

/// Ticks the SAVED toast stays up.
const SAVED_TOAST: u32 = 120;

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
            UiKind::Stages => "stages.toml",
            UiKind::Challenge => "challenge.toml",
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

    /// Stage select for the fight the fighter select lined up.
    pub fn load_stages(
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        lib: &rugen_content::Library,
        plan: crate::stagesel::StagePlan,
    ) -> anyhow::Result<Self> {
        let mut s = Self::load(UiKind::Stages, motif, root, sys, config, lib)?;
        s.stages = Some(crate::stagesel::StageSel::new(plan, lib));
        s.lib = Some(lib.clone());
        s.refresh_data(sys);
        Ok(s)
    }

    /// Back on the fighter select from the stage select: both picks locked again.
    pub fn restore_picks(&mut self, picks: &crate::roster::Picks, sys: &SystemData) {
        if let Some(r) = self.roster.as_mut() {
            r.restore(picks);
        }
        self.refresh_data(sys);
    }

    /// The controls page was opened from the pause menu.
    pub fn set_from_pause(&mut self, from_pause: bool, sys: &SystemData) {
        self.from_pause = from_pause;
        self.settings.from_pause = from_pause;
        self.refresh_data(sys);
    }

    /// Online fighter pick: who the opponent is (and their fighter once picked).
    pub fn set_online_foe(&mut self, name: String, fighter: Option<String>, sys: &SystemData) {
        let Some(r) = self.roster.as_mut() else {
            return;
        };
        if r.foe() != (&name, &fighter) {
            r.set_foe(name, fighter);
            self.refresh_data(sys);
        }
    }

    /// Buttons held this tick (before [`tick`](Self::tick)).
    pub fn set_held(&mut self, held: [Buttons; 2]) {
        self.held = held;
    }

    /// Free the GPU textures this screen uploaded (it is being replaced);
    /// sprite-sheet entries belong to the system sheet and stay.
    pub fn release_gpu(&mut self, r: &mut Renderer) {
        if let Some(st) = self.stages.as_mut() {
            st.release(r, &mut self.textures);
        }
        for (key, c) in self.textures.drain() {
            if let Some((tex, ..)) = c
                && !key.starts_with("sff:")
            {
                r.free_texture(tex);
            }
        }
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

    /// TRAINING pause menu: the training settings as the menu left them.
    pub fn training_cfg(&self) -> Option<crate::training::TrainingCfg> {
        self.pause
            .as_ref()
            .and_then(|p| p.training.as_ref())
            .map(|t| t.cfg.clone())
    }

    /// Focus inside a training panel (`display[2].row`): the index of that
    /// panel's entry in the menu.
    fn training_menu_at(&self, focus: &str) -> Option<usize> {
        let id = TRAINING_PANELS
            .iter()
            .find(|p| focus.starts_with(&format!("{p}[")))?;
        self.ui.data()["menu"]
            .as_array()?
            .iter()
            .position(|m| m["id"] == *id)
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
        // a system package keeps player files in the user data dir: say where
        let chars = if lib.root == lib.assets {
            "CHARS/".to_string()
        } else {
            let dir = lib.chars_dir();
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
                Some(rel) => format!("~/{}/", rel.display()),
                None => format!("{}/", dir.display()),
            }
        };
        self.arenas["drop"] = Value::from(format!("DROP .ZIP OR .RAR FIGHTERS INTO {chars}"));
        let fighters = lib.chars().len();
        self.arenas["counts"] = Value::from(format!(
            "{} · {}",
            plural(fighters, "FIGHTER", "FIGHTERS"),
            plural(count, "ARENA", "ARENAS")
        ));
    }

    /// The profile for screens, plus `record` (`LV 3 · 1 WIN`).
    fn profile_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.profile).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.insert(
                "record".into(),
                Value::from(format!(
                    "LV {} · {}",
                    self.profile.level,
                    plural(self.profile.wins as usize, "WIN", "WINS")
                )),
            );
        }
        v
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

    /// The players' last input devices (keyboard or a pad family): hints
    /// follow them, as fight overlays do.
    pub fn set_devices(&mut self, devices: [crate::input::Device; 2], sys: &SystemData) {
        if devices != self.devices {
            self.devices = devices;
            self.refresh_data(sys);
        }
    }

    /// What `player` presses for binding `slot` (keys or pad buttons).
    fn hint_cap(&self, player: usize, slot: usize) -> String {
        crate::keylabel::cap(&self.config.input, player, slot, self.devices[player]).plain()
    }

    /// Key labels of P1's bindings for hint keycaps.
    fn keys_json(&self) -> Value {
        json!({
            "a": self.hint_cap(0, 4),
            "b": self.hint_cap(0, 5),
            "c": self.hint_cap(0, 6),
            "x": self.hint_cap(0, 7),
            "s": self.hint_cap(0, 10),
            "team": format!("{} / {}", self.hint_cap(0, 7), self.hint_cap(0, 8)),
            "start": self.hint_cap(0, crate::keylabel::START),
        })
    }

    /// Menu confirm / back hints (`hint.ok`, `hint.back`): Enter and Esc on
    /// the keyboard, the pad's A and B (by its family's names) on a pad.
    fn hint_json(&self) -> Value {
        let (ok, back) = menu_hints(self.devices[0]);
        json!({
            "ok": ok,
            "back": back,
            "pad": matches!(self.devices[0], crate::input::Device::Pad(_)),
        })
    }

    /// Function keys a screen handles itself (TAB search / F1 sort on the roster).
    pub fn hotkey(&mut self, key: &Key, sys: &SystemData) -> bool {
        // options / controls: F2 saves
        if matches!(self.kind, UiKind::Options | UiKind::Controls)
            && *key == Key::Named(NamedKey::F2)
        {
            self.action("save", sys);
            return true;
        }
        // stage strip: Page Up / Down, Home / End, a letter jumps by name
        if let Some(st) = self.stages.as_mut() {
            return match st.key(key) {
                Some(moved) => {
                    if moved {
                        self.refresh_data(sys);
                    }
                    true
                }
                None => false,
            };
        }
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
            stages: None,
            held: [Buttons::default(); 2],
            stage_wheel: 0.0,
            devices: [crate::input::Device::Keyboard; 2],
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
            panel: "moves",
            pause_focus: None,
            featured: Vec::new(),
            feature_at: 0,
            arenas: Value::Null,
            try_stage: None,
            ticks: 0,
            lib: None,
            from_pause: false,
            challenge: Value::Null,
        };
        if kind == UiKind::Title {
            s.load_title_facts(lib);
        }
        if kind == UiKind::Challenge {
            let (bosses, flagged) = crate::challenge::bosses(lib);
            s.challenge = crate::challenge::cards(&s.profile.challenge, bosses.len(), flagged);
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
        let t = crate::framestats::enabled().then(std::time::Instant::now);
        self.refresh_data_inner(sys);
        if let Some(t) = t {
            crate::framestats::add(crate::framestats::Phase::Data, t.elapsed());
        }
    }

    fn refresh_data_inner(&mut self, sys: &SystemData) {
        let data = match self.kind {
            UiKind::Select(_) => match &self.roster {
                Some(r) => {
                    let mut d = r.data(&self.profile, &self.room);
                    d["keys"] = self.keys_json();
                    // the pick hint names the key of whoever picks now: both
                    // players' while two cursors pick at once, none when done
                    let open = r.open_picks();
                    let label = |p: usize| self.hint_cap(p, 4);
                    let pick = match (r.dual(), open) {
                        (_, [false, false]) => String::new(),
                        (true, [true, true]) => format!("{} / {}", label(0), label(1)),
                        (true, [false, true]) => label(1),
                        _ => label(0),
                    };
                    d["pick_key"] = Value::from(pick);
                    d["searching"] = json!(self.ui.text_focused());
                    d
                }
                None => Value::Null,
            },
            UiKind::Stages => match &self.stages {
                Some(st) => {
                    let mut d = st.data();
                    d["keys"] = self.keys_json();
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
                // a team: the mode and the partners under the name
                let team = |i: usize| {
                    setup
                        .and_then(|s| s.teams.as_ref())
                        .map(|t| t.line(i - 1))
                        .unwrap_or_default()
                };
                let side = |i: usize| {
                    let has = self.mem.contains_key(&format!("p{i}"));
                    json!({ "name": name(i), "name_up": name(i).to_uppercase(), "art": format!("mem:p{i}"), "has_art": has, "team": team(i) })
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
                    "profile": self.profile_json(),
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
                        "winner_line": format!("{} WINS", winner_name.to_uppercase()),
                        "winner_big": w.is_some() && winner_name.chars().count() <= 12,
                        "winner_small": w.is_some() && winner_name.chars().count() > 12,
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
                        "profile": self.profile_json(),
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
                let width = |font: i32, t: &str| {
                    sys.fonts
                        .get(&font)
                        .map_or(t.chars().count() as f32 * 9.0, |f| f.width(t, 1.0))
                };
                let pages = move_pages(moves, &width);
                let count = pages.len().max(1);
                let page = self.moves_page % count;
                let [col1, col2] = pages.into_iter().nth(page).unwrap_or_default();
                let training = info.and_then(|p| p.training.as_ref());
                let mut menu = vec![
                    json!({ "id": "resume", "label": "RESUME", "action": "resume" }),
                    json!({ "id": "moves", "label": "MOVE LIST", "action": "moves" }),
                    json!({ "id": "restart", "label": "RESTART ROUND", "action": "restart" }),
                    json!({ "id": "controls", "label": "BUTTON CONFIG", "action": "controls" }),
                ];
                if training.is_some() {
                    // the training lab: its panels after RESUME, a reset that says what it does
                    for (k, id) in TRAINING_PANELS.iter().enumerate() {
                        let label = id.to_uppercase();
                        let go = format!("panel:{id}");
                        menu.insert(
                            1 + k,
                            json!({ "id": id, "label": label, "action": go, "right": go }),
                        );
                    }
                    if let Some(m) = menu.iter_mut().find(|m| m["id"] == "restart") {
                        m["label"] = json!("RESET POSITIONS");
                    }
                }
                // Survival carries P1's life between fights: a restarted round
                // would hand it back full
                if info.is_some_and(|p| p.mode == Some(Mode::Survival)) {
                    menu.retain(|m| m["id"] != "restart");
                }
                if info.is_some_and(|p| p.mode.is_some()) {
                    menu.push(
                        json!({ "id": "select", "label": "CHARACTER SELECT", "action": "select" }),
                    );
                }
                menu.push(json!({ "id": "quit", "label": "QUIT TO MENU", "action": "quit" }));
                // left / right page the move list, except on an entry with its own panel
                for m in &mut menu {
                    if m.get("right").is_none() {
                        m["right"] = json!("page:1");
                    }
                    m["left"] = json!("page:-1");
                }
                let cmd = info.map(|p| p.cmd.to_uppercase()).unwrap_or_default();
                let name = info.map(|p| p.name.to_uppercase()).unwrap_or_default();
                let title = format!("{name} · MOVE LIST");
                let empty = if cmd.is_empty() {
                    "THIS FIGHTER HAS NO COMMAND FILE".to_string()
                } else {
                    format!("NO SPECIAL MOVES FOUND IN {cmd}")
                };
                let who = match training {
                    Some(t) => format!(
                        "P1 · {} VS DUMMY · {}",
                        t.fighters[0].to_uppercase(),
                        t.fighters[1].to_uppercase()
                    ),
                    None => format!("P{} · {name}", info.map_or(1, |p| p.side + 1)),
                };
                let panel = if training.is_some() {
                    self.panel
                } else {
                    "moves"
                };
                json!({
                    "profile": self.profile_json(),
                    "version": env!("CARGO_PKG_VERSION"),
                    "menu": menu,
                    "training": training.is_some(),
                    "show_moves": panel == "moves",
                    "show_display": panel == "display",
                    "show_dummy": panel == "dummy",
                    "display": training.map_or(json!([]), |t| t.cfg.display_json()),
                    "dummy": training.map_or(json!([]), |t| t.cfg.dummy_json()),
                    "rec": training.map_or(json!({}), TrainingInfo::rec_json),
                    "who": who,
                    "name": name,
                    // the header shrinks for long names so it never reaches the page count
                    "title_big": title.chars().count() <= 20,
                    "title": title,
                    "art": "mem:pauser",
                    "has_art": self.mem.contains_key("pauser"),
                    "scene": "mem:scene",
                    "has_scene": self.mem.contains_key("scene"),
                    "col1": col1,
                    "col2": col2,
                    "has_moves": !moves.is_empty(),
                    "no_moves": moves.is_empty(),
                    "empty": empty,
                    "source": if cmd.is_empty() || moves.is_empty() { String::new() } else {
                        format!("READ FROM {cmd} · COSTS FROM THE MOVE'S POWER CHECK")
                    },
                    "facing": if count > 1 {
                        format!("FACING RIGHT · PAGE {} / {count}", page + 1)
                    } else {
                        "FACING RIGHT".to_string()
                    },
                    "paged": count > 1 && panel == "moves",
                    "panel_open": panel != "moves",
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
                            "arcade" | "versus" | "training" | "watch" | "options" | "survival"
                            | "bossrush" | "timeattack" => format!("mode:{k}"),
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
                // CHALLENGE (the runs against the CPU) right after arcade
                if UiScreen::available(&self.motif, UiKind::Challenge) {
                    let at = pos(&menu, "arcade");
                    menu.insert(
                        at,
                        json!({ "id": "challenge", "label": "CHALLENGE", "action": "challenge" }),
                    );
                }
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
                    "profile": self.profile_json(),
                    "version": env!("CARGO_PKG_VERSION"),
                })
            }
            UiKind::Challenge => json!({
                "cards": self.challenge,
                "profile": self.profile_json(),
                "keys": self.keys_json(),
                "version": env!("CARGO_PKG_VERSION"),
            }),
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
                    let st = &self.settings;
                    m.insert(
                        "save".into(),
                        json!({
                            "unsaved": st.unsaved,
                            "clean": !st.unsaved,
                            "toast": st.toast > 0,
                            "confirm": st.confirm.is_some(),
                            "status": if st.unsaved { "UNSAVED CHANGES" } else { "ALL SAVED" },
                        }),
                    );
                    m.insert("profile".into(), self.profile_json());
                    m.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
                    // where Esc goes: opened from the pause menu it resumes the match
                    let back = if self.from_pause {
                        "BACK TO THE MATCH"
                    } else {
                        "BACK TO OPTIONS"
                    };
                    m.insert("back_to".into(), json!(back));
                }
                d
            }
        };
        let mut data = data;
        if let Value::Object(m) = &mut data {
            m.insert("hint".into(), self.hint_json());
        }
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
    pub fn set_pads(&mut self, pads: Vec<(String, crate::input::PadKind, u16)>, sys: &SystemData) {
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

    // ------------------------------------------------------------ save flow

    /// The host compares the edits with rugen.toml: true = unsaved changes.
    pub fn set_unsaved(&mut self, unsaved: bool, sys: &SystemData) {
        if self.settings.unsaved != unsaved {
            self.settings.unsaved = unsaved;
            self.refresh_data(sys);
        }
    }

    /// Controls opened from the pause menu (leaving returns to the match).
    /// SAVE / DISCARD asked this tick, for the host to carry out.
    pub fn take_request(&mut self) -> Option<SaveRequest> {
        self.settings.request.take()
    }

    /// The host wrote rugen.toml: show the SAVED toast.
    pub fn saved(&mut self, sys: &SystemData) {
        self.settings.unsaved = false;
        self.settings.toast = SAVED_TOAST;
        self.refresh_data(sys);
    }

    /// A transition out of the options / controls: with unsaved changes it
    /// opens the SAVE / DISCARD / CANCEL dialog instead (`None`) and goes
    /// once that is answered. Moving between the two pages is not leaving.
    fn guard(&mut self, t: Transition, sys: &SystemData) -> Option<Transition> {
        if !matches!(self.kind, UiKind::Options | UiKind::Controls) || !self.settings.unsaved {
            return Some(t);
        }
        let inside = match t {
            Transition::Controls(_) => true,
            Transition::Options => !(self.kind == UiKind::Controls && self.settings.from_pause),
            _ => false,
        };
        if inside {
            return Some(t);
        }
        self.settings.confirm = Some(t);
        if let Some(c) = self.settings.controls.as_mut() {
            c.listen = None;
        }
        self.refresh_data(sys);
        self.ui.set_focus(Some("dlg_save"));
        None
    }

    /// The window is being closed: with unsaved options / controls the
    /// SAVE / DISCARD / CANCEL dialog opens (true) and the game quits once
    /// SAVE or DISCARD is picked; false = nothing to ask, close now.
    pub fn ask_before_quit(&mut self, sys: &SystemData) -> bool {
        self.guard(Transition::Quit, sys).is_none()
    }

    /// `save`, `leave:save|discard|cancel`; None when `a` is not one of them.
    fn save_action(&mut self, a: &str, sys: &SystemData) -> Option<Option<Transition>> {
        let st = &mut self.settings;
        let out = match a {
            "save" => {
                st.request = Some(SaveRequest::Save);
                None
            }
            "leave:save" => {
                st.request = Some(SaveRequest::Save);
                st.confirm.take()
            }
            "leave:discard" => {
                st.request = Some(SaveRequest::Discard);
                st.confirm.take()
            }
            "leave:cancel" => {
                st.confirm = None;
                None
            }
            _ => return None,
        };
        self.refresh_data(sys);
        Some(out)
    }

    /// Escape: `None` when the screen handled it (options content → tab column),
    /// else where to go.
    pub fn escape(&mut self, sys: &SystemData) -> Option<Transition> {
        if self.settings.confirm.is_some() {
            // Esc in the leave dialog: CANCEL
            self.settings.confirm = None;
            self.refresh_data(sys);
            return None;
        }
        match self.kind {
            UiKind::Options if self.settings.controls.is_none() => {
                let on_tabs = self.ui.focused().is_some_and(|f| f.starts_with("tabs["));
                if on_tabs || !self.ui.data()["tabs"].is_array() {
                    return self.guard(Transition::Title, sys);
                }
                self.set_tab(self.settings.tab, sys);
                None
            }
            UiKind::Controls => self.guard(Transition::Options, sys),
            // back to the fighters, both picks still locked
            UiKind::Stages => self.stages.as_mut().map(|s| s.back()),
            UiKind::Select(mode) => {
                // Esc is player 1's: take back their pick (one cursor: the
                // last pick), leave when there is none
                let side = self.roster.as_ref().is_some_and(|r| r.dual()).then_some(0);
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
            UiKind::Pause => {
                // B inside a training panel: back to its menu entry
                let f = self.ui.focused().unwrap_or("").to_string();
                if let Some(at) = self.training_menu_at(&f) {
                    sfx(sys.title.cancel_snd);
                    self.ui.set_focus(Some(&format!("menu[{at}].slab")));
                    return None;
                }
                Some(Transition::Resume)
            }
            UiKind::Options => self.escape(sys),
            UiKind::Challenge => {
                sfx(sys.title.cancel_snd);
                Some(Transition::Title)
            }
            // B is a fight button: it must not quit the game, leave a result or the bindings page
            UiKind::Title | UiKind::Result | UiKind::Controls | UiKind::Versus | UiKind::Stages => {
                None
            }
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
        if self.settings.confirm.is_some() {
            // the leave dialog has the keys: Enter presses its focused button
            if pressed
                && *key == Key::Named(NamedKey::Enter)
                && let Some(a) = self.ui.focused_action()
            {
                return self.action(&a, sys);
            }
            return None;
        }
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
            // the stage select's strip steps with the wheel anywhere on it
            MouseEvent::Wheel(lines) if self.stages.is_some() => self.stage_wheel += lines,
            MouseEvent::Wheel(lines) => self.ui.mouse_wheel(lines),
        }
    }

    fn adjust(&mut self, id: &str, dir: i32, wrap: bool) {
        if crate::settings::adjust(&mut self.config, id, dir, wrap) {
            self.dirty = true;
        }
    }

    fn action(&mut self, a: &str, sys: &SystemData) -> Option<Transition> {
        // a click on a stage strip tile
        if let (Some(st), Some(lib), Some(i)) = (
            self.stages.as_mut(),
            self.lib.as_ref(),
            a.strip_prefix("tile:")
                .and_then(|i| i.parse::<usize>().ok()),
        ) {
            let t = st.click(i, lib);
            self.refresh_data(sys);
            return t;
        }
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
        if matches!(self.kind, UiKind::Options | UiKind::Controls)
            && let Some(t) = self.save_action(a, sys)
        {
            return t;
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
                "back" => self.guard(Transition::Options, sys),
                _ => None,
            };
        }
        if self.kind == UiKind::Pause {
            if let Some(t) = self.pause.as_mut().and_then(|p| p.training.as_mut()) {
                if t.cfg.action(a) {
                    self.refresh_data(sys);
                    return None;
                }
                let cmd = match a {
                    "rec:record" => Some(crate::training::Command::Record),
                    "rec:play" => Some(crate::training::Command::Play),
                    "rec:loop" => Some(crate::training::Command::Loop),
                    _ => None,
                };
                if let Some(c) = cmd {
                    let empty = t.slots[t.cfg.slot.min(crate::training::SLOTS - 1)] == 0;
                    // nothing to play yet: stay in the menu
                    if empty && c != crate::training::Command::Record {
                        return None;
                    }
                    return Some(Transition::Training(c));
                }
                if let Some(id) = a.strip_prefix("panel:")
                    && let Some(&id) = TRAINING_PANELS.iter().find(|p| **p == id)
                {
                    // into the panel, at its first row
                    self.panel = id;
                    self.refresh_data(sys);
                    self.ui.set_focus(Some(&format!("{id}[0].row")));
                    return None;
                }
            }
            return match a {
                "resume" | "back" => Some(Transition::Resume),
                "options" => Some(Transition::Options),
                "restart" => Some(Transition::Restart),
                "moves" | "page:1" | "page:-1" => {
                    let width = |font: i32, t: &str| {
                        sys.fonts
                            .get(&font)
                            .map_or(t.chars().count() as f32 * 9.0, |f| f.width(t, 1.0))
                    };
                    let count = self
                        .pause
                        .as_ref()
                        .map_or(1, |p| move_pages(&p.moves, &width).len().max(1));
                    let step = if a == "page:-1" { count - 1 } else { 1 };
                    self.moves_page = (self.moves_page % count + step) % count;
                    self.refresh_data(sys);
                    None
                }
                "controls" => Some(Transition::Controls(
                    self.pause.as_ref().map_or(0, |p| p.side),
                )),
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
            "challenge" => Some(Transition::Challenge),
            "back" | "title" if matches!(self.kind, UiKind::Options) => {
                self.guard(Transition::Title, sys)
            }
            "back" | "title" => Some(Transition::Title),
            "mode" => Some(match arg {
                "arcade" => Transition::Select(Mode::Arcade),
                "versus" => Transition::Select(Mode::Versus),
                "training" => Transition::Select(Mode::Training),
                "arena" => Transition::Select(Mode::Arena),
                "tutorial" => Transition::Select(Mode::Tutorial),
                "watch" => Transition::Select(Mode::Watch),
                "survival" => Transition::Select(Mode::Survival),
                "bossrush" => Transition::Select(Mode::BossRush),
                "timeattack" => Transition::Select(Mode::TimeAttack),
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
            // a click (or drag) on a row's pips / choice / slider
            "pick" => {
                if crate::settings::pick(&mut self.config, arg, arg2) {
                    self.dirty = true;
                }
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
            fighter_size: None,
            arena_ai: None,
            round_time: None,
            teams: None,
            run: None,
            seed: None,
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
        // fighter select with team modes: X / Y step the team (VS: each
        // player their own side), they do not pick
        let mut pressed = pressed;
        if let Some(r) = self.roster.as_mut().filter(|r| r.teams_ok())
            && !self.ui.text_focused()
        {
            let mut changed = false;
            for (p, b) in pressed.iter_mut().enumerate() {
                let dir = i32::from(b.y) - i32::from(b.x);
                if b.x || b.y {
                    let side = if dual { p } else { r.team_side() };
                    changed |= dir != 0 && r.step_team(side, dir);
                    b.x = false;
                    b.y = false;
                }
            }
            if changed {
                sfx(sys.title.move_snd);
                self.refresh_data(sys);
            }
        }
        let listening = self
            .settings
            .controls
            .as_ref()
            .is_some_and(|c| c.listen.is_some());
        let (mut nav, mut nav2) = if listening {
            (Nav::default(), Nav::default())
        } else if dual {
            (of(&pressed[0]), of(&pressed[1]))
        } else {
            let merged = Buttons::from_bits(pressed[0].bits() | pressed[1].bits());
            (of(&merged), Nav::default())
        };
        if matches!(self.kind, UiKind::Options | UiKind::Controls) && self.settings.toast > 0 {
            self.settings.toast -= 1;
            if self.settings.toast == 0 {
                self.refresh_data(sys);
            }
        }
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
        // stage select: the players drive the strip; the UI engine only draws it
        let mut stage_out = None;
        if self.stages.is_some() {
            let strip_w = self.ui.rect_of("strip").map(|r| r.w);
            if let (Some(st), Some(lib)) = (self.stages.as_mut(), self.lib.as_ref()) {
                if let Some(w) = strip_w {
                    st.set_strip_width(w);
                }
                st.upkeep(r, &mut self.textures);
                if !self.ui.leaving() {
                    let snd = &sys.select;
                    // one cursor: either player's keys drive it (the
                    // keyboard's arrows / Enter belong to P2 by default)
                    let any = |b: &[Buttons; 2]| Buttons::from_bits(b[0].bits() | b[1].bits());
                    stage_out = st.tick(any(&pressed), any(&self.held), lib, &mut |e| {
                        sfx(match e {
                            crate::stagesel::Sfx::Move => snd.stage_move_snd,
                            crate::stagesel::Sfx::Done => snd.stage_done_snd,
                            crate::stagesel::Sfx::Cancel => snd.cancel_snd,
                        })
                    });
                    if st.wheel(std::mem::take(&mut self.stage_wheel)) {
                        sfx(snd.stage_move_snd);
                    }
                }
            }
            nav = Nav::default();
            nav2 = Nav::default();
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
                stages: None,
            };
            crate::framestats::time(crate::framestats::Phase::Layout, || {
                self.ui.tick2(nav, nav2, &mut host)
            })
        };
        let mut out = None;
        // the stage select: its keys never reach the UI focus, so a focus
        // move there is the mouse resting on a strip tile
        let hovered_tile =
            self.stages.is_some() && events.iter().any(|e| matches!(e, Event::Moved));
        for e in events {
            match e {
                Event::Moved if self.stages.is_some() => {}
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
                    } else if a == "confirm" {
                        // FIGHT!! answers either player
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
        if stage_out.is_some() {
            return stage_out;
        }
        if hovered_tile
            && let Some(i) = self
                .ui
                .focused_action()
                .and_then(|a| a.strip_prefix("tile:")?.parse::<usize>().ok())
            && let Some(st) = self.stages.as_mut()
            && st.hover(i)
        {
            sfx(sys.select.stage_move_snd);
            self.refresh_data(sys);
        }
        // everyone picked: after a beat, on to the stage select / VS screen
        if out.is_none()
            && !self.ui.leaving()
            && let Some(r) = self.roster.as_mut()
        {
            match r.tick() {
                Outcome::Go(t) => out = Some(*t),
                Outcome::Refresh => self.refresh_data(sys),
                Outcome::None => {}
            }
        }
        if dual
            && nav2.back
            && out.is_none()
            && let Some(t) = self.back(Some(1), sys, sfx)
        {
            out = Some(t);
        }
        // training pause menu: the focused entry's panel is the one on display
        if self.kind == UiKind::Pause && self.pause.as_ref().is_some_and(|p| p.training.is_some()) {
            let f = self.ui.focused().map(str::to_string);
            if f != self.pause_focus {
                let entry = f
                    .as_deref()
                    .and_then(|f| f.strip_prefix("menu["))
                    .and_then(|r| r.split(']').next())
                    .and_then(|i| i.parse::<usize>().ok())
                    .and_then(|i| self.ui.data()["menu"].get(i))
                    .and_then(|m| m["id"].as_str())
                    .map(str::to_string);
                if let Some(id) = entry {
                    let panel = TRAINING_PANELS
                        .iter()
                        .find(|p| **p == id)
                        .copied()
                        .unwrap_or("moves");
                    if panel != self.panel {
                        self.panel = panel;
                        self.refresh_data(sys);
                    }
                }
                self.pause_focus = f;
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
                stages: self.stages.as_mut(),
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
    /// Stage select: draws the `stage_preview` slot.
    pub(crate) stages: Option<&'a mut crate::stagesel::StageSel>,
}

impl Host<'_> {
    fn texture(&mut self, key: &str) -> Cached {
        if let Some(c) = self.textures.get(key) {
            return *c;
        }
        // stage thumbnails are registered by the stage select (and freed by it)
        if key.starts_with("stagethumb:") {
            return None;
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

    /// Window pixels per virtual unit (1.25 at 1600x900, 1.5 at 1080p).
    fn px_scale(&self) -> [f32; 2] {
        let win = self.r.size();
        let (vw, vh) = viewport(win);
        [win.0 as f32 / vw, win.1 as f32 / vh]
    }

    /// `r` with its edges on whole window pixels. At fractional scales a
    /// frame and the picture inside it otherwise round to different pixels
    /// (a picture a pixel too high / off-centre in its frame).
    fn snap(&self, r: Rect) -> Rect {
        let k = self.px_scale();
        let edge = |v: f32, k: f32| libm::roundf(v * k) / k;
        let (x0, y0) = (edge(r.x, k[0]), edge(r.y, k[1]));
        let (x1, y1) = (edge(r.x + r.w, k[0]), edge(r.y + r.h, k[1]));
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
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
        // upright pictures land on whole pixels (rotated / slanted ones keep their maths)
        let (dst, clip) = if self.rot.is_none() && fx.skew == 0.0 {
            (self.snap(dst), clip.map(|c| self.snap(c)))
        } else {
            (dst, clip)
        };
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

    fn draw_slot(&mut self, id: &str, dst: Rect, _alpha: f32, _clip: Option<Rect>) {
        if id == "stage_preview" {
            let dst = self.snap(dst);
            if let Some(st) = self.stages.as_mut() {
                st.draw_preview(self.r, dst);
            }
        }
    }

    fn draw_rect(&mut self, dst: Rect, color: [f32; 4], skew: f32, clip: Option<Rect>) {
        let (dst, clip) = if self.rot.is_none() && skew == 0.0 {
            (self.snap(dst), clip.map(|c| self.snap(c)))
        } else {
            (dst, clip)
        };
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

    fn text_center_dy(&mut self, font: i32, scale: f32) -> f32 {
        self.fonts.get(&font).map_or(0.0, |f| f.center_dy(scale))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The DUMMY panel's recorder line follows the chosen slot.
    #[test]
    fn training_recorder_line() {
        let mut t = TrainingInfo {
            cfg: Default::default(),
            fighters: ["A".into(), "B".into()],
            slots: [0, 228, 600],
            start: "p".into(),
        };
        let r = t.rec_json();
        assert_eq!(r["title"], "RECORDING · SLOT 1 OF 3");
        assert_eq!(r["line"], "EMPTY · UP TO 10 S");
        assert_eq!(r["has"], false);
        assert!(r["help"].as_str().unwrap().contains("PRESS P TO STOP"));
        t.cfg.slot = 1;
        let r = t.rec_json();
        assert_eq!(r["title"], "RECORDING · SLOT 2 OF 3");
        assert_eq!(r["line"], "3.8 S OF 10 S · 228 FRAMES");
        assert_eq!(r["pct"], "38.0");
    }

    #[test]
    fn menu_hints_follow_the_device() {
        use crate::input::{Device, PadKind};
        let h = |d| menu_hints(d);
        assert_eq!(h(Device::Keyboard), ("ENTER".into(), "ESC".into()));
        assert_eq!(h(Device::Pad(PadKind::Xbox)), ("A".into(), "B".into()));
        assert_eq!(
            h(Device::Pad(PadKind::PlayStation)),
            ("X".into(), "O".into())
        );
        assert_eq!(h(Device::Pad(PadKind::Nintendo)), ("B".into(), "A".into()));
    }

    /// A group running into the next column is marked as continued, not repeated.
    #[test]
    fn continued_move_group_is_marked() {
        use crate::screeninfo::{Group, Move, Part};
        let moves: Vec<Move> = (0..20)
            .map(|i| Move {
                name: format!("MOVE {i}"),
                notation: "X".into(),
                parts: vec![Part::Button("X".into())],
                group: Group::Special,
                cost: None,
            })
            .collect();
        let pages = move_pages(&moves, &|_, t: &str| t.len() as f32 * 9.0);
        let [col1, col2] = &pages[0];
        assert_eq!(col1[0]["label"], "SPECIALS");
        assert_eq!(col2[0]["label"], "SPECIALS (CONT.)");
        assert_eq!(col2[0]["cont"], true);
    }

    /// Closing the window with unsaved options asks SAVE / DISCARD /
    /// CANCEL like leaving the screen, and quits once it is answered.
    #[test]
    fn window_close_asks_before_dropping_unsaved_options() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let lib = rugen_content::Library::discover(Some(&repo));
        // the player profile is written under `root`: keep it out of the repo
        let root = std::env::temp_dir().join(format!("rugen-close-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let config = Config::default();
        for theme in ["ink", "dojo"] {
            let motif = lib.motif_dir_for(theme);
            let Ok(sys) = SystemData::load(&motif, &lib.data_dir()) else {
                return; // no MUGEN data (reference/ not fetched)
            };
            let mut ui = UiScreen::load(UiKind::Options, &motif, &root, &sys, &config, &lib)
                .unwrap_or_else(|e| panic!("{theme}: {e:#}"));
            assert!(!ui.ask_before_quit(&sys), "{theme}: nothing to save, close");
            ui.set_unsaved(true, &sys);
            assert!(ui.ask_before_quit(&sys), "{theme}: unsaved, ask first");
            assert!(matches!(ui.save_action("leave:cancel", &sys), Some(None)));
            assert!(ui.ask_before_quit(&sys));
            assert!(matches!(
                ui.save_action("leave:discard", &sys),
                Some(Some(Transition::Quit))
            ));
            assert_eq!(ui.take_request(), Some(SaveRequest::Discard));
            assert!(ui.ask_before_quit(&sys));
            assert!(matches!(
                ui.save_action("leave:save", &sys),
                Some(Some(Transition::Quit))
            ));
            assert_eq!(ui.take_request(), Some(SaveRequest::Save));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plural_counts_one_and_many() {
        assert_eq!(plural(1, "ROOM", "ROOMS"), "1 ROOM");
        assert_eq!(plural(0, "ROOM", "ROOMS"), "0 ROOMS");
        assert_eq!(plural(2, "ISSUE", "ISSUES"), "2 ISSUES");
    }
}
