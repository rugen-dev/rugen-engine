//! The application shell: window, input, audio and the screen state machine
//! (title → select → match → title). Also hosts the `--shot` / `--frames` /
//! `--script` harness used for automated screenshots.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rugen_audio::{Audio, SoundKey};
use rugen_content::Library;
use rugen_sim::Buttons;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::config::Config;
use crate::gpu::Gpu;
use crate::input::InputState;
use crate::online::{OnlineAuto, OnlineMatch, OnlineScreen};
use crate::options::OptionsScreen;
use crate::play::{FightSetup, Match};
use crate::select::SelectScreen;
use crate::system::SystemData;
use crate::title::TitleScreen;
use crate::uiscreen::{MouseEvent, UiKind, UiScreen};
use crate::versus::{Portrait, VersusScreen};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Win lines for characters without `[Quotes]`.
const STOCK_QUOTES: [&str; 4] = [
    "Is that all you've got?",
    "Train harder and come back!",
    "Next chapter, same result.",
    "Victory is written in ink!",
];
const SYSTEM_SND: u32 = rugen_audio::SYSTEM_BANK;
/// Ticks between the last options change and writing rugen.toml.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Arcade,
    Versus,
    Training,
    Tutorial,
    Watch,
    /// Smash-style fight on an `[Arena]` stage: stocks, blast zones, ring-out.
    Arena,
    Netplay,
    Options,
    Exit,
    /// Pick the fighter you bring to an online matchup (one cursor, local roster).
    Online,
    /// CHALLENGE: one life bar against an endless line of random CPU
    /// opponents, one round each, a little life back between fights.
    Survival,
    /// CHALLENGE: the bosses (`boss = 1` in select.def, else the five
    /// strongest fighters by the balance verdict) back to back.
    BossRush,
    /// CHALLENGE: the arcade ladder against a run timer.
    TimeAttack,
}

impl Mode {
    /// Screen title of the mode (`""` for entries that are not modes).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Arcade => "ARCADE",
            Mode::Versus => "VS MODE",
            Mode::Training => "TRAINING",
            Mode::Tutorial => "TUTORIAL",
            Mode::Watch => "WATCH",
            Mode::Arena => "ARENA",
            Mode::Online => "ONLINE",
            Mode::Survival => "SURVIVAL",
            Mode::BossRush => "BOSS RUSH",
            Mode::TimeAttack => "TIME ATTACK",
            Mode::Netplay | Mode::Options | Mode::Exit => "",
        }
    }

    /// A ladder of CPU opponents the shell lines up one after another
    /// (P1 picks alone, no stage select, CONTINUE after a win).
    pub fn is_ladder(self) -> bool {
        matches!(
            self,
            Mode::Arcade | Mode::Survival | Mode::BossRush | Mode::TimeAttack
        )
    }

    /// One of the CHALLENGE modes (records in the profile).
    pub fn is_challenge(self) -> bool {
        matches!(self, Mode::Survival | Mode::BossRush | Mode::TimeAttack)
    }

    /// Never fought on an `[Arena]` stage: the ladders (Arcade and the
    /// CHALLENGE runs) play classic rounds; stocks and blast zones belong
    /// to ARENA.
    pub fn avoids_arenas(self) -> bool {
        self.is_ladder()
    }
}

#[derive(Debug)]
pub enum Transition {
    Title,
    Select(Mode),
    /// Back from the stage select: the fighter select with these picks
    /// (manifest ids) locked again.
    SelectAgain(Mode, Box<crate::roster::Picks>),
    /// Both fighters picked: choose the stage (`ui/stages.toml`).
    StageSelect(Box<crate::stagesel::StagePlan>),
    /// Online match from the `[net]` section of rugen.toml.
    Netplay,
    /// ONLINE menu: rooms, ranked, leaderboard (needs `[net] server`).
    Online,
    /// PROFILE: accounts on the online servers (the ONLINE screen's page).
    Profile,
    /// The online screen wants the fighter pick (select screen in online mode).
    OnlineSelect,
    /// The online fighter pick came back with this character: the host goes
    /// on to the stage select, the other side straight back to the matchup.
    OnlineFighter(String),
    /// Back to the matchup with the character id and the host's stage choice.
    OnlinePicked(String, Option<String>),
    Options,
    /// Key bindings page for player 0 / 1 (`ui/controls.toml`).
    Controls(usize),
    /// Show the VS screen, then fight.
    Versus(FightSetup, [Portrait; 2]),
    Fight(FightSetup),
    /// Post-match screen.
    Result(Box<ResultInfo>),
    /// Result screen "continue": next arcade opponent or the title.
    Continue,
    /// CHALLENGE menu: a card per challenge mode (`ui/challenge.toml`).
    Challenge,
    /// Play a storyboard (cutscene), then go on with the transition.
    Story(PathBuf, Box<Transition>),
    /// Pause menu: back to the match.
    Resume,
    /// Pause menu: restart the current round.
    Restart,
    /// Training pause menu: back to the match and RECORD / PLAY / LOOP.
    Training(crate::training::Command),
    Quit,
}

/// What the result screen shows after a match.
#[derive(Debug)]
pub struct ResultInfo {
    pub setup: FightSetup,
    pub winner: Option<usize>,
    pub wins: [u8; 2],
    pub names: [String; 2],
    pub arts: [Option<crate::uiscreen::MemImage>; 2],
    /// Launched with `rugen play`: offer Rematch / Quit only.
    pub quick: bool,
    pub arcade: bool,
    pub rating: String,
    /// Winner's victory quote (or a stock line).
    pub quote: String,
    /// Winner's match report rows.
    pub stats: Vec<(String, String)>,
}

/// Raw result data pulled out of a finished match: setup, winner, wins, names, arts, quick, arcade.
type ResultCard = (
    FightSetup,
    Option<usize>,
    [u8; 2],
    [String; 2],
    [Option<crate::uiscreen::MemImage>; 2],
    bool,
    bool,
);

trait ResultSource {
    fn result_card(&self, quick: bool, arcade: bool) -> ResultCard;
}

impl ResultSource for Match {
    fn result_card(&self, quick: bool, arcade: bool) -> ResultCard {
        let setup = self.setup().clone();
        let (n1, a1) = char_card(&setup.p1);
        let (n2, a2) = char_card(&setup.p2);
        (
            setup,
            self.winner(),
            self.world.round.wins,
            [n1, n2],
            [a1, a2],
            quick,
            arcade,
        )
    }
}

/// Display name and big art for a prepared character .def (from its cache manifest).
pub(crate) fn char_card(def: &std::path::Path) -> (String, Option<crate::uiscreen::MemImage>) {
    let found = rugen_content::manifest_for(def);
    let Some((dir, man)) = found else {
        return (
            def.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            None,
        );
    };
    let file = man
        .art
        .clone()
        .or_else(|| man.portrait_big.as_ref().map(|p| p.file.clone()));
    let img = file.and_then(|f| crate::uiscreen::load_image(&dir.join(f)).ok());
    (man.display_name, img)
}

enum Screen {
    Blank,
    Title(Box<TitleScreen>),
    Options(Box<OptionsScreen>),
    /// TOML-driven screen from `motif/ui/`.
    Ui(Box<UiScreen>),
    Online(Box<OnlineScreen>),
    Select(Box<SelectScreen>),
    Versus(Box<VersusScreen>),
    Match(Box<Match>),
    /// A storyboard, and where to go once it ends.
    Story(Box<crate::storyboard::StoryScreen>, Box<Transition>),
}

/// Automated-run options.
#[derive(Default)]
pub struct Harness {
    pub shot: Option<PathBuf>,
    pub frames: Option<u32>,
    pub script: Option<Vec<Buttons>>,
    /// Second player's scripted inputs.
    pub script2: Option<Vec<Buttons>>,
    /// Write a replay of every match here (quick play) or into `replays/`.
    pub record: Option<PathBuf>,
    /// Raw keys to press: `(tick, key name, ticks held)` (`--keys 90:Enter,120:k*40`).
    pub keys: Vec<(u32, String, u32)>,
    /// Mouse actions at ticks (`--mouse 60:640,360,90:click`), window pixels.
    pub mouse: Vec<(u32, MouseAct)>,
    /// Online runs: scripts and `--frames` count only once the fight is on
    /// screen (the matchup before it takes as long as it takes), and the
    /// end of a script never quits (leaving mid-match would be a flee).
    pub in_match: bool,
}

/// One scripted mouse action (`--mouse`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseAct {
    Move(f32, f32),
    /// Press, released 3 ticks later.
    Click,
    Wheel(f32),
}

/// `--mouse "60:640,360,90:click,120:wheel=-1"`: the cursor moves to window
/// pixel `x,y`, clicks or turns the wheel at shell ticks.
pub fn parse_mouse(s: &str) -> Vec<(u32, MouseAct)> {
    let mut out = Vec::new();
    let mut parts = s.split(',').map(str::trim).peekable();
    while let Some(p) = parts.next() {
        let Some((n, what)) = p.split_once(':') else {
            continue;
        };
        let Ok(t) = n.trim().parse::<u32>() else {
            continue;
        };
        let act = if what == "click" {
            MouseAct::Click
        } else if let Some(w) = what.strip_prefix("wheel=") {
            match w.parse() {
                Ok(v) => MouseAct::Wheel(v),
                Err(_) => continue,
            }
        } else {
            match (what.parse::<f32>(), parts.next().map(str::parse::<f32>)) {
                (Ok(x), Some(Ok(y))) => MouseAct::Move(x, y),
                _ => continue,
            }
        };
        out.push((t, act));
    }
    out
}

/// `--keys "90:Enter,120:k*40"`: raw key presses at shell ticks, released
/// 3 ticks later (or after `*n` ticks: a held key).
pub fn parse_keys(s: &str) -> Vec<(u32, String, u32)> {
    s.split(',')
        .filter_map(|t| {
            let (n, k) = t.trim().split_once(':')?;
            let (k, hold) = match k.rsplit_once('*') {
                Some((k, h)) => (k, h.trim().parse().ok()?),
                None => (k, 3),
            };
            Some((n.trim().parse().ok()?, k.trim().to_string(), hold))
        })
        .collect()
}

pub struct Shell {
    config: Config,
    lib: Library,
    data_dir: PathBuf,
    /// The online match the current fight belongs to.
    online_match: Option<OnlineMatch>,
    /// ONLINE screen parked while its fighter pick (select screen) is open.
    parked_online: Option<Box<OnlineScreen>>,
    /// `rugen play --server`: the ONLINE screen drives itself.
    online_auto: Option<OnlineAuto>,
    /// Result reports in flight: (request, we won).
    reports: Vec<(
        rugen_net::online::Pending<rugen_net::online::ResultAck>,
        bool,
    )>,
    motif_dir: PathBuf,
    /// The active user theme's files, watched for live reload.
    theme_watch: Option<crate::themes::Watch>,
    gpu: Option<Gpu>,
    input: InputState,
    prev: [Buttons; 2],
    /// START held last tick, per player.
    prev_start: [bool; 2],
    audio: Option<Audio>,
    system: Option<SystemData>,
    screen: Screen,
    start: Option<Transition>,
    /// Quit when the match ends (launched with `rugen play`).
    quick: bool,
    tick_acc: Duration,
    last: Instant,
    harness: Harness,
    shot_armed: bool,
    ticks: u32,
    quit: bool,
    /// Replay to feed into the next match.
    replay: Option<crate::replay::Replay>,
    /// Arcade ladder or CHALLENGE run in progress.
    run: Option<crate::challenge::Run>,
    fullscreen: bool,
    /// Black overlay alpha for screen fades (1 = black).
    fade: f32,
    /// Pause menu shown over a local match.
    pause: Option<Box<UiScreen>>,
    /// Match parked while its pause menu opened the options.
    parked: Option<Box<Match>>,
    /// Transition waiting for a TOML screen's leave animation.
    deferred: Option<Transition>,
    /// Options tab to open next (back from the controls page).
    reopen_tab: Option<usize>,
    /// Shell ticks for `--keys`, and harness key releases due.
    key_ticks: u32,
    key_releases: Vec<(u32, Key)>,
    /// The menu mouse beside the keyboard and pads (see [`crate::mouse`]).
    mouse: crate::mouse::MouseGate,
    /// Scripted mouse button releases (`--mouse` clicks).
    mouse_releases: Vec<u32>,
    /// Held menu directions per player (see [`crate::repeat`]).
    menu_repeat: [crate::repeat::Repeat; 2],
    /// Stats of the match being played (read-only observer).
    match_stats: crate::screeninfo::MatchStats,
    /// TRAINING tool settings, kept from one training match to the next.
    training_cfg: crate::training::TrainingCfg,
    /// The settings as rugen.toml holds them: the options screens preview
    /// edits live and only SAVE writes them (DISCARD goes back to these).
    saved: Config,
    /// Menu base keys held (arrows, Enter) that no player mapping claims.
    menu_keys: Buttons,
    /// Menu base keys pressed since the last tick (a quick tap still counts).
    menu_taps: Buttons,
    /// Ctrl (or Cmd) held: Ctrl+V pastes a room code.
    ctrl: bool,
    /// Mouse cursor visibility last applied to the window.
    cursor_shown: Option<bool>,
    /// Something changed since the last frame was drawn (a tick ran, a
    /// window event arrived): low-latency mode draws only then.
    dirty: bool,
    /// `RUGEN_LATENCY=1`: input-to-display latency measurements.
    latency: crate::latency::Probe,
    /// Low latency on a FIFO swapchain: ticks run inside the redraw, right
    /// after the vsync wait (acquire, simulate, draw, present).
    vsync_paced: bool,
    last_redraw: Instant,
    /// `RUGEN_FRAMESTATS=1`: per-phase frame timings.
    frame_stats: crate::framestats::Reporter,
    /// The screenpack's logo / intro storyboards were offered (first title).
    booted: bool,
}

impl Shell {
    /// Start on a self-driving ONLINE screen (`rugen play --server --room`).
    pub fn with_online_auto(mut self, auto: OnlineAuto) -> Self {
        self.online_auto = Some(auto);
        self
    }

    pub fn new(
        config: Config,
        lib: Library,
        start: Transition,
        quick: bool,
        harness: Harness,
        replay: Option<crate::replay::Replay>,
    ) -> Self {
        let mut lib = lib;
        lib.keep_originals = config.content.keep_originals;
        crate::themes::init(&lib);
        // convert what is new in chars/ and stages/ while the title shows
        // (menus that need an entry first convert it themselves)
        if !quick {
            let l = lib.clone();
            let _ = std::thread::Builder::new()
                .name("import".into())
                .spawn(move || {
                    for i in l.import_new() {
                        if let Err(e) = i.result {
                            log::warn!("{}: {e}", i.id);
                        }
                    }
                });
        }
        let data_dir = lib.data_dir();
        let motif_dir = lib.motif_dir_for(&config.video.theme);
        let config_fullscreen = config.video.fullscreen;
        let system = match SystemData::load(&motif_dir, &data_dir) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("system.def not loaded: {e:#}");
                None
            }
        };
        let audio = match Audio::new() {
            Ok(mut a) => {
                a.set_master_volume(config.audio.master);
                a.set_music_volume(config.audio.music);
                Some(a)
            }
            Err(e) => {
                log::warn!("audio disabled: {e}");
                None
            }
        };
        Self {
            input: InputState::new(&config.input),
            saved: config.clone(),
            config,
            lib,
            data_dir,
            theme_watch: crate::themes::is_user(&motif_dir)
                .then(|| crate::themes::Watch::new(&motif_dir)),
            motif_dir,
            gpu: None,
            prev: [Buttons::default(); 2],
            prev_start: [false; 2],
            audio,
            system,
            screen: Screen::Blank,
            start: Some(start),
            quick,
            tick_acc: Duration::ZERO,
            last: Instant::now(),
            harness,
            shot_armed: false,
            ticks: 0,
            quit: false,
            replay,
            run: None,
            fullscreen: config_fullscreen,
            online_match: None,
            parked_online: None,
            online_auto: None,
            reports: Vec::new(),
            fade: 1.0,
            pause: None,
            parked: None,
            deferred: None,
            reopen_tab: None,
            key_ticks: 0,
            key_releases: Vec::new(),
            mouse: crate::mouse::MouseGate::default(),
            mouse_releases: Vec::new(),
            menu_repeat: Default::default(),
            match_stats: Default::default(),
            training_cfg: Default::default(),
            menu_keys: Buttons::default(),
            menu_taps: Buttons::default(),
            ctrl: false,
            cursor_shown: None,
            dirty: true,
            latency: crate::latency::Probe::new(),
            vsync_paced: false,
            last_redraw: Instant::now(),
            frame_stats: crate::framestats::Reporter::default(),
            booted: false,
        }
    }

    /// A menu is on screen (everything but a running fight; the pause menu counts).
    fn in_menu(&self) -> bool {
        !matches!(self.screen, Screen::Match(_)) || self.pause.is_some()
    }

    /// Show the mouse cursor only where it does something: menus with
    /// `[input] mouse` on, while the mouse (not a key or pad) drives them.
    fn update_cursor(&mut self) {
        let want = self.config.input.mouse
            && self.in_menu()
            && self.harness.script.is_none()
            && self.mouse.visible();
        if self.cursor_shown == Some(want) {
            return;
        }
        let Some(g) = &self.gpu else { return };
        g.window.set_cursor_visible(want);
        self.cursor_shown = Some(want);
        if !want {
            // forget the last position so a still cursor cannot hold a hover
            match &mut self.screen {
                Screen::Ui(u) => u.mouse(MouseEvent::Left),
                Screen::Online(o) => o.mouse(MouseEvent::Left),
                _ => {}
            }
            if let Some(p) = self.pause.as_mut() {
                p.mouse(MouseEvent::Left);
            }
        }
    }

    /// Hand a mouse event to the screen that takes it: the pause menu over
    /// a match, a UI screen or the online screen.
    fn mouse_to_screen(&mut self, ev: MouseEvent) {
        match &mut self.screen {
            Screen::Match(_) => {
                if let Some(p) = self.pause.as_mut() {
                    p.mouse(ev);
                }
            }
            Screen::Ui(u) => u.mouse(ev),
            Screen::Online(o) => o.mouse(ev),
            _ => {}
        }
    }

    /// The cursor moved (window pixels); it hovers only once it really moves.
    fn mouse_moved(&mut self, x: f32, y: f32) {
        if self.mouse.moved(x, y, u64::from(self.key_ticks)) {
            self.mouse_to_screen(MouseEvent::Move(x, y));
        }
    }

    /// A click or a wheel turn wakes the mouse where it rests.
    fn mouse_wake(&mut self) {
        if let Some((x, y)) = self.mouse.pressed(u64::from(self.key_ticks)) {
            self.mouse_to_screen(MouseEvent::Move(x, y));
        }
    }

    fn mouse_button(&mut self, down: bool) {
        if down {
            self.mouse_wake();
        }
        self.mouse_to_screen(MouseEvent::Button(down));
    }

    fn mouse_wheel(&mut self, lines: f32) {
        self.mouse_wake();
        self.mouse_to_screen(MouseEvent::Wheel(lines));
    }

    /// A key or pad press: the keyboard takes the focus back from the mouse.
    fn mouse_yields(&mut self) {
        if self.mouse.keyboard() {
            self.mouse_to_screen(MouseEvent::Left);
        }
    }

    /// Open the pause menu over the current match (local matches only); `side` paused.
    fn open_pause(&mut self, side: usize) {
        let Screen::Match(m) = &mut self.screen else {
            return;
        };
        if m.is_net() || !UiScreen::available(&self.motif_dir, UiKind::Pause) {
            return;
        }
        let Some(sys) = self.system.as_ref() else {
            return;
        };
        // freeze the current frame under the menu
        let scene = self.gpu.as_mut().map(|g| {
            let r = &mut g.renderer;
            m.refresh_gpu(r);
            r.begin();
            // the menu covers the fight, not the bars: freeze it without them
            m.hud_hidden = true;
            let (cam, clear) = m.render(r);
            m.hud_hidden = false;
            let (w, h) = r.size();
            (w, h, r.end_to_image(&cam, clear, w, h))
        });
        let setup = m.setup().clone();
        let def = if side == 0 { &setup.p1 } else { &setup.p2 };
        let (name, art) = char_card(def);
        let (cmd, mut moves) = crate::screeninfo::move_list(def);
        // buttons as this player presses them: `[A]` → `[J]` (or the pad's)
        let keys =
            crate::keylabel::Labels::new(&self.config.input, side, self.input.devices()[side]);
        crate::screeninfo::relabel(&mut moves, |b| keys.button(b));
        // the menu stops a recording (what was recorded stays)
        if let Some(t) = m.training.as_mut() {
            t.stop_recording();
        }
        let start =
            crate::keylabel::Labels::new(&self.config.input, 0, self.input.devices()[0]).start();
        let training = m.training.as_ref().map(|t| crate::uiscreen::TrainingInfo {
            cfg: t.cfg.clone(),
            slots: t.slot_frames(),
            start: start.clone(),
            fighters: [0, 1].map(|i| {
                let w = &m.world;
                w.players
                    .get(i)
                    .and_then(|p| w.data.get(p.data))
                    .map(|d| d.display_name.clone())
                    .unwrap_or_default()
            }),
        });
        let info = crate::uiscreen::PauseInfo {
            side,
            name,
            art,
            cmd,
            moves,
            scene,
            mode: setup.mode.filter(|_| !setup.tutorial),
            training,
        };
        match UiScreen::load_pause(
            &self.motif_dir,
            &self.lib.root,
            sys,
            &self.config,
            &self.lib,
            info,
        ) {
            Ok(u) => self.pause = Some(Box::new(u)),
            Err(e) => log::warn!("ui/pause.toml: {e:#}"),
        }
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    /// Leaving the options: apply what is left, then carry out a SAVE /
    /// DISCARD picked on the way out.
    fn apply_options(&mut self) {
        if let Some(c) = self.take_options_config()
            && self.apply_config(c)
        {
            self.reload_motif();
        }
        self.settings_request();
    }

    /// SAVE writes the edited settings to rugen.toml; DISCARD puts the live
    /// settings back to what rugen.toml holds.
    fn settings_request(&mut self) {
        let Screen::Ui(u) = &mut self.screen else {
            return;
        };
        let Some(req) = u.take_request() else {
            return;
        };
        let edited = u.config.clone();
        let theme_changed = match req {
            crate::uiscreen::SaveRequest::Save => {
                u.dirty = false;
                let changed = self.apply_config(edited);
                self.save_config();
                self.saved = self.config.clone();
                if let (Screen::Ui(u), Some(sys)) = (&mut self.screen, self.system.as_ref()) {
                    u.saved(sys);
                }
                changed
            }
            crate::uiscreen::SaveRequest::Discard => {
                let back = self.saved.clone();
                u.config = back.clone();
                u.dirty = false;
                log::info!("settings: unsaved changes discarded");
                self.apply_config(back)
            }
        };
        if theme_changed {
            self.reload_motif();
        }
    }

    /// Options / controls on screen: whether their edits differ from rugen.toml.
    fn track_unsaved(&mut self) {
        if let (Screen::Ui(u), Some(sys)) = (&mut self.screen, self.system.as_ref())
            && matches!(u.kind, UiKind::Options | UiKind::Controls)
        {
            let unsaved = u.config != self.saved;
            u.set_unsaved(unsaved, sys);
        }
    }

    /// Classic (flat motif) options: no save button, leaving saves.
    fn save_classic_options(&mut self) {
        if matches!(self.screen, Screen::Options(_)) {
            self.apply_options();
            self.save_config();
            self.saved = self.config.clone();
        }
    }

    /// The options screen's config when it changed since the last call.
    fn take_options_config(&mut self) -> Option<Config> {
        match &mut self.screen {
            Screen::Options(o) if o.dirty => {
                o.dirty = false;
                Some(o.config.clone())
            }
            Screen::Ui(u) if matches!(u.kind, UiKind::Options | UiKind::Controls) && u.dirty => {
                u.dirty = false;
                Some(u.config.clone())
            }
            _ => None,
        }
    }

    /// Options screens: every change takes effect the tick it is made (theme,
    /// window, audio, bindings; gameplay values on the next fight) as a
    /// preview; SAVE writes it to rugen.toml.
    fn live_options(&mut self) {
        if let Some(config) = self.take_options_config()
            && self.apply_config(config)
        {
            self.reload_motif();
            self.reopen_options();
        }
        self.settings_request();
        self.track_unsaved();
    }

    /// Re-open the options in the theme just switched to, on the same tab and row.
    fn reopen_options(&mut self) {
        let (tab, focus) = match &self.screen {
            Screen::Ui(u) => (u.options_tab(), u.focus_key()),
            _ => (crate::settings::INTERFACE_TAB, None),
        };
        self.reopen_tab = Some(tab);
        self.transition(Transition::Options);
        self.fade = 0.0;
        if let (Screen::Ui(u), Some(key)) = (&mut self.screen, focus) {
            u.restore_focus(&key);
        }
        self.play_system_music("title");
    }

    /// Make `config` the live one: bindings, volumes, window size and
    /// fullscreen follow at once. True when the UI theme changed.
    fn apply_config(&mut self, config: Config) -> bool {
        let theme_changed = !self
            .config
            .video
            .theme
            .trim()
            .eq_ignore_ascii_case(config.video.theme.trim());
        self.config = config;
        self.input.rebind(&self.config.input);
        if let Some(a) = self.audio.as_mut() {
            a.set_master_volume(self.config.audio.master);
            a.set_music_volume(self.config.audio.music);
        }
        if let Some(g) = self.gpu.as_mut()
            && !self.fullscreen
            && !self.config.video.fullscreen
        {
            // logical pixels, like the size the window was created with
            let want = winit::dpi::LogicalSize::new(
                self.config.video.width.max(320),
                self.config.video.height.max(240),
            );
            let have: winit::dpi::LogicalSize<u32> =
                g.window.inner_size().to_logical(g.window.scale_factor());
            if have != want {
                // applied at once (Wayland): no Resized event follows
                if let Some(s) = g.window.request_inner_size(want) {
                    g.renderer.resize(s.width, s.height);
                }
            }
        }
        if self.fullscreen != self.config.video.fullscreen {
            self.fullscreen = self.config.video.fullscreen;
            if let Some(g) = &self.gpu {
                g.window.set_fullscreen(
                    self.fullscreen
                        .then_some(winit::window::Fullscreen::Borderless(None)),
                );
            }
        }
        theme_changed
    }

    fn save_config(&self) {
        match self.config.save(&self.lib.root) {
            Ok(()) => log::info!("settings saved to rugen.toml"),
            Err(e) => log::warn!("settings not saved: {e}"),
        }
    }

    /// Switch to the configured UI theme: new motif dir, screenpack and fonts.
    fn reload_motif(&mut self) {
        self.motif_dir = self.lib.motif_dir_for(&self.config.video.theme);
        match SystemData::load(&self.motif_dir, &self.data_dir) {
            Ok(s) => self.system = Some(s),
            Err(e) => log::warn!("system.def not loaded: {e:#}"),
        }
        self.theme_watch = crate::themes::is_user(&self.motif_dir)
            .then(|| crate::themes::Watch::new(&self.motif_dir));
        log::info!("ui theme: {}", self.motif_dir.display());
    }

    /// A file of the active user theme was saved: reload `system.def` (fonts,
    /// sprites) and the screens on display in place.
    fn live_reload_theme(&mut self) {
        log::info!("theme: {} changed, reloading", self.motif_dir.display());
        match SystemData::load(&self.motif_dir, &self.data_dir) {
            Ok(s) => self.system = Some(s),
            Err(e) => log::warn!("theme: system.def not loaded: {e:#}"),
        }
        let Some(sys) = self.system.as_ref() else {
            return;
        };
        let mut r = self.gpu.as_mut().map(|g| &mut g.renderer);
        if let Screen::Ui(u) = &mut self.screen {
            u.reload_theme(r.as_deref_mut(), sys, &self.lib);
        }
        if let Some(p) = self.pause.as_mut() {
            p.reload_theme(r.as_deref_mut(), sys, &self.lib);
        }
        if let Screen::Online(o) = &mut self.screen {
            o.reload_theme(r);
        }
    }

    /// system.def `[Music]`: `<screen>.bgm`, `<screen>.bgm.volume`, `<screen>.bgm.loop`.
    fn play_system_music(&mut self, screen: &str) {
        let Some(sys) = self.system.as_ref() else {
            return;
        };
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let def = rugen_formats::def::Def::open(&sys.def_path).ok();
        let music = def.as_ref().and_then(|d| d.section("music"));
        let name = music
            .and_then(|m| m.raw(&format!("{screen}.bgm")))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            audio.stop_music();
            return;
        }
        let volume = music
            .and_then(|m| m.f32(&format!("{screen}.bgm.volume")))
            .unwrap_or(100.0)
            / 100.0;
        let looping = music
            .and_then(|m| m.i32(&format!("{screen}.bgm.loop")))
            .unwrap_or(1)
            != 0;
        let root = self.data_dir.parent().unwrap_or(&self.data_dir);
        let path = [root.join(&name), self.data_dir.join(&name)]
            .into_iter()
            .find(|p| p.exists());
        audio.play_music(path.as_deref(), volume, looping);
    }

    /// A ladder mode's fighter select is done: the run starts with P1's
    /// pick. The select's opponent is the first one, except in Boss Rush,
    /// where the first boss is.
    /// Returns the transition to take instead of the select's own (its
    /// fight with the run's rules).
    fn start_run(&mut self, mode: Mode, next: Transition) -> Transition {
        let Transition::Versus(mut setup, sides) = next else {
            return next;
        };
        let name = rugen_content::manifest_for(&setup.p1)
            .map(|(_, m)| m.display_name)
            .unwrap_or_default();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9, |d| d.subsec_nanos());
        let mut run =
            crate::challenge::Run::new(mode, setup.p1.clone(), setup.ids[0].clone(), name, seed);
        let chars = self.lib.chars();
        run.first_foe(
            chars
                .iter()
                .position(|e| e.id.eq_ignore_ascii_case(&setup.ids[1])),
        );
        if mode == Mode::BossRush {
            run.bosses = crate::challenge::bosses(&self.lib).0;
            let first_is_boss = run.next_boss() == Some(setup.ids[1].as_str());
            self.run = Some(run);
            if !first_is_boss {
                // the classic select's random opponent: the first boss instead
                return self
                    .run_fight(setup.stage.clone())
                    .unwrap_or(Transition::Title);
            }
        } else {
            self.run = Some(run);
        }
        setup.mode = Some(mode);
        setup.run = self.run.as_ref().and_then(|r| r.rules());
        // ARCADE: the fighter's intro before the first fight
        if mode == Mode::Arcade
            && let Some(p) = crate::storyboard::char_storyboard(&setup.p1, "intro.storyboard")
        {
            return Transition::Story(p, Box::new(Transition::Versus(setup, sides)));
        }
        // a screenpack select may have drawn an arena stage: not for a ladder
        if mode.avoids_arenas() {
            let stages = crate::challenge::fight_stages(&self.lib);
            if setup.stage.as_ref().is_none_or(|s| !stages.contains(s)) {
                setup.stage = stages.first().cloned().or(setup.stage);
            }
        }
        Transition::Versus(setup, sides)
    }

    /// A cleared arcade ladder: P1's ending storyboard, if it has one.
    fn arcade_ending(&self) -> Option<PathBuf> {
        let r = self
            .run
            .as_ref()
            .filter(|r| r.mode == Mode::Arcade && r.over == Some(true))?;
        crate::storyboard::char_storyboard(&r.p1, "ending.storyboard")
    }

    /// After a win in a run: the next opponent (None once the run is over).
    fn run_next(&mut self) -> Option<Transition> {
        let run = self.run.as_ref()?;
        if run.over.is_some() {
            return None;
        }
        self.run_fight(None)
    }

    /// The run's next fight on `stage` (else the ladder's next stage).
    fn run_fight(&mut self, stage: Option<PathBuf>) -> Option<Transition> {
        let chars = self.lib.chars();
        let run = self.run.as_mut()?;
        if chars.is_empty() {
            return None;
        }
        let pick = if run.mode == Mode::BossRush {
            let id = run.next_boss()?;
            chars
                .iter()
                .find(|e| e.id.eq_ignore_ascii_case(id))?
                .clone()
        } else {
            chars[run.next_foe(chars.len())].clone()
        };
        let p1 = run.p1.clone();
        let mode = run.mode;
        let rules = run.rules();
        let (dir, man) = self.lib.prepare_char(&pick).ok()?;
        let p2 = man.def_path(&dir);
        let stage = match stage {
            Some(s) => Some(s),
            // the ladders walk every stage but the arenas
            None if mode.avoids_arenas() => {
                let stages = crate::challenge::fight_stages(&self.lib);
                let at = self.run.as_ref().map_or(0, |r| r.next_stage(stages.len()));
                stages.get(at).cloned().or_else(|| self.lib.default_stage())
            }
            None => {
                let at = self
                    .run
                    .as_ref()
                    .map_or(0, |r| r.next_stage(self.lib.stages().len()));
                self.lib
                    .stages()
                    .get(at)
                    .and_then(|e| self.lib.prepare_stage(e).ok().map(|(d, m)| m.def_path(&d)))
                    .or_else(|| self.lib.default_stage())
            }
        };
        let side = |def: &PathBuf| {
            let (name, img) = char_card(def);
            Portrait::new(name, img.map(|(w, h, rgba)| (w, h, rgba, [0, 0])))
        };
        Some(Transition::Versus(
            FightSetup {
                p1: p1.clone(),
                p2: p2.clone(),
                stage,
                ai: [0, self.config.game.ai_level],
                net: None,
                tutorial: false,
                ids: [manifest_id(&p1), man.id.clone()],
                mode: Some(mode),
                fighter_size: None,
                arena_ai: None,
                round_time: None,
                teams: None,
                run: rules,
                seed: None,
            },
            [side(&p1), side(&p2)],
        ))
    }

    /// A run's fight ended (P1 `won` with `life` of `life_max` left): count
    /// it, keep an ended CHALLENGE run's best in the profile, and return
    /// the result screen's run rows.
    fn finish_run_fight(&mut self, won: bool, life: i32, life_max: i32) -> Vec<(String, String)> {
        let Some(run) = self.run.as_mut() else {
            return Vec::new();
        };
        run.finish(won, life, life_max);
        let mut profile = crate::uiscreen::Profile::load(&self.lib.root);
        let mut best = profile
            .challenge
            .get(&run.p1_id)
            .cloned()
            .unwrap_or_default();
        if run.over.is_some() && run.mode.is_challenge() && !run.p1_id.is_empty() {
            run.record(&mut best);
            if run.new_best {
                profile.challenge.insert(run.p1_id.clone(), best.clone());
                profile.save(&self.lib.root);
            }
        }
        run.rows(&best)
    }

    /// Finished match → profile stats for P1's character (local, non-tutorial matches).
    fn record_result(&mut self) {
        let Screen::Match(m) = &mut self.screen else {
            return;
        };
        if !m.world.round.match_over || m.stats_recorded || m.is_tutorial() {
            return;
        }
        m.stats_recorded = true;
        let won = m.world.round.wins[0] > m.world.round.wins[1];
        let mut p = crate::uiscreen::Profile::load(&self.lib.root);
        p.record(&m.setup_ids()[0], won);
        p.save(&self.lib.root);
    }

    /// Write the current match's recording (quick play: to `--record`, else `replays/`).
    fn save_replay(&mut self) {
        let Screen::Match(m) = &mut self.screen else {
            return;
        };
        let Some(rec) = m.record.take() else { return };
        if rec.inputs.len() < 60 {
            return;
        }
        let path = match &self.harness.record {
            Some(p) => p.clone(),
            None => {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                self.lib.root.join("replays").join(format!("{t}.json"))
            }
        };
        match rec.save(&path) {
            Ok(()) => log::info!("replay saved to {}", path.display()),
            Err(e) => log::warn!("replay not saved: {e}"),
        }
    }

    fn system_sfx(&mut self, snd: Option<(i32, i32)>) {
        let Some((g, n)) = snd else { return };
        let (Some(audio), Some(sys)) = (self.audio.as_mut(), self.system.as_ref()) else {
            return;
        };
        let key = SoundKey {
            bank: SYSTEM_SND,
            group: g,
            number: n,
        };
        if !audio.is_loaded(key) {
            let Some(bytes) = sys.snd.as_ref().and_then(|s| s.get(g, n)) else {
                return;
            };
            audio.load(key, bytes);
        }
        audio.play(key, 0, -1, 100, 0.0, false);
    }

    /// Close the pause menu and give back its textures.
    fn drop_pause(&mut self) {
        if let Some(mut p) = self.pause.take()
            && let Some(g) = self.gpu.as_mut()
        {
            p.release_gpu(&mut g.renderer);
        }
    }

    fn transition(&mut self, t: Transition) {
        self.transition_inner(t);
        // no match left (on screen or parked under the options): its fighters'
        // voices stop and their samples go, so nothing carries into the next fight
        if !matches!(self.screen, Screen::Match(_))
            && self.parked.is_none()
            && let Some(a) = self.audio.as_mut()
        {
            a.release_fight();
        }
    }

    fn transition_inner(&mut self, t: Transition) {
        // every new screen fades in from black
        self.fade = 1.0;
        // the screen being left gives back its textures (a pause menu keeps
        // the match underneath, so only replaced UI screens)
        if let (Screen::Ui(u), Some(g)) = (&mut self.screen, self.gpu.as_mut())
            && !matches!(
                t,
                Transition::Controls(_)
                    | Transition::Resume
                    | Transition::Restart
                    | Transition::Training(_)
            )
        {
            u.release_gpu(&mut g.renderer);
        }
        if let (Screen::Story(st, _), Some(g)) = (&mut self.screen, self.gpu.as_mut()) {
            st.release_gpu(&mut g.renderer);
        }
        // BUTTON CONFIG opened from the pause menu: leaving it goes straight
        // back to the paused match, not through the options
        let t = match t {
            Transition::Options
                if self.parked.is_some()
                    && matches!(&self.screen, Screen::Ui(u) if u.kind == UiKind::Controls) =>
            {
                self.reopen_tab = None;
                Transition::Title
            }
            t => t,
        };
        match t {
            Transition::Quit => {
                log::info!("quit requested");
                self.quit = true;
            }
            Transition::Netplay => {
                let n = self.config.net.clone();
                let resolve = |me: &Self, id: &str| -> Option<PathBuf> {
                    let e = me.lib.find_char(id)?;
                    me.lib
                        .prepare_char(&e)
                        .map(|(d, m)| m.def_path(&d))
                        .map_err(|err| log::error!("{id}: {err}"))
                        .ok()
                };
                let side = usize::from(n.side == 2);
                let net = match n.remote.parse::<std::net::SocketAddr>() {
                    Ok(remote) => crate::play::NetMode::P2P {
                        port: n.port,
                        remote,
                        side,
                    },
                    Err(e) => {
                        log::error!("[net] remote {:?}: {e}", n.remote);
                        self.transition(Transition::Title);
                        return;
                    }
                };
                let (Some(p1), Some(p2)) = (resolve(self, &n.p1), resolve(self, &n.p2)) else {
                    self.transition(Transition::Title);
                    return;
                };
                let stage = self
                    .lib
                    .stages()
                    .into_iter()
                    .find(|e| e.id.eq_ignore_ascii_case(&n.stage))
                    .or_else(|| self.lib.stages().into_iter().next())
                    .and_then(|e| self.lib.prepare_stage(&e).ok().map(|(d, m)| m.def_path(&d)))
                    .or_else(|| self.lib.default_stage());
                self.transition(Transition::Fight(FightSetup {
                    p1,
                    p2,
                    stage,
                    ai: [0, 0],
                    net: Some(net),
                    tutorial: false,
                    ids: [String::new(), String::new()],
                    mode: None,
                    fighter_size: None,
                    arena_ai: None,
                    round_time: None,
                    teams: None,
                    run: None,
                    seed: None,
                }));
            }
            Transition::Resume => {
                self.drop_pause();
                self.fade = 0.0;
            }
            Transition::Training(cmd) => {
                self.drop_pause();
                self.fade = 0.0;
                if let Screen::Match(m) = &mut self.screen
                    && let Some(t) = m.training.as_mut()
                {
                    t.command(cmd);
                }
            }
            Transition::Restart => {
                self.drop_pause();
                self.fade = 0.0;
                if let Some(a) = self.audio.as_mut() {
                    a.stop_fight_voices();
                }
                if let Screen::Match(m) = &mut self.screen {
                    m.world.start_round();
                    // the recording no longer replays from the first frame
                    m.record = None;
                }
            }
            Transition::Continue => match self.run_next() {
                Some(t) => self.transition(t),
                // a cleared arcade ladder: the fighter's ending
                None if self.arcade_ending().is_some() => {
                    if let Some(p) = self.arcade_ending() {
                        self.transition(Transition::Story(p, Box::new(Transition::Title)));
                    }
                }
                // an ended CHALLENGE run goes back to its menu (the new best on it)
                None if self.run.as_ref().is_some_and(|r| r.mode.is_challenge())
                    && UiScreen::available(&self.motif_dir, UiKind::Challenge) =>
                {
                    self.transition(Transition::Challenge)
                }
                None => self.transition(Transition::Title),
            },
            Transition::Story(path, then) => {
                match crate::storyboard::StoryScreen::load(&path, &self.data_dir) {
                    Ok(st) => {
                        log::info!("storyboard {}", path.display());
                        self.screen = Screen::Story(Box::new(st), then);
                    }
                    Err(e) => {
                        log::warn!("storyboard not played: {e:#}");
                        self.transition(*then);
                    }
                }
            }
            Transition::Challenge => {
                self.run = None;
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Challenge) => {
                        match UiScreen::load(
                            UiKind::Challenge,
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/challenge.toml: {e:#}");
                                self.transition(Transition::Title);
                                return;
                            }
                        }
                    }
                    _ => {
                        self.transition(Transition::Title);
                        return;
                    }
                };
            }
            Transition::Result(info) => {
                log::info!(
                    "match over: winner {:?} rounds {:?}",
                    info.winner,
                    info.wins
                );
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Result) => {
                        match UiScreen::load_result(
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                            *info,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/result.toml: {e:#}");
                                self.transition(Transition::Continue);
                                return;
                            }
                        }
                    }
                    _ => {
                        self.transition(if self.quick {
                            Transition::Quit
                        } else {
                            Transition::Continue
                        });
                        return;
                    }
                };
            }
            Transition::Online if self.parked_online.is_some() => {
                // back from the fighter pick without a pick
                if let Some(mut o) = self.parked_online.take() {
                    o.pick_cancelled();
                    self.screen = Screen::Online(o);
                }
            }
            Transition::OnlineFighter(id) => match self.parked_online.as_ref() {
                Some(o) if o.hosts_stage() => {
                    let names = o.stage_names(&id);
                    let plan = crate::stagesel::StagePlan::online(id, o.arena_room(), names);
                    self.transition(Transition::StageSelect(Box::new(plan)));
                }
                Some(_) => self.transition(Transition::OnlinePicked(id, None)),
                None => self.transition(Transition::Online),
            },
            Transition::OnlinePicked(id, stage) => match self.parked_online.take() {
                Some(mut o) => {
                    o.picked(&id, stage);
                    self.screen = Screen::Online(o);
                }
                None => self.transition(Transition::Online),
            },
            Transition::OnlineSelect => {
                // from the online screen, or back from the host's stage pick
                let mut o = match self.parked_online.take() {
                    Some(o) => o,
                    None => match std::mem::replace(&mut self.screen, Screen::Blank) {
                        Screen::Online(o) => o,
                        other => {
                            self.screen = other;
                            return;
                        }
                    },
                };
                let ui = self
                    .system
                    .as_ref()
                    .filter(|_| UiScreen::available(&self.motif_dir, UiKind::Select(Mode::Online)))
                    .and_then(|sys| {
                        UiScreen::load(
                            UiKind::Select(Mode::Online),
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                        )
                        .map_err(|e| log::warn!("ui/select.toml: {e:#}"))
                        .ok()
                    });
                match ui {
                    Some(mut u) => {
                        if let Some(sys) = self.system.as_ref() {
                            let (name, fighter) = o.opponent();
                            u.set_online_foe(name, fighter, sys);
                        }
                        self.parked_online = Some(o);
                        self.screen = Screen::Ui(Box::new(u));
                    }
                    None => {
                        // no pick screen in this theme: bring the main fighter
                        o.pick_default();
                        self.screen = Screen::Online(o);
                    }
                }
            }
            Transition::Online => {
                let r = match self.online_auto.take() {
                    Some(auto) => OnlineScreen::load_auto(
                        &self.motif_dir,
                        &self.lib.root,
                        &self.lib,
                        &self.config,
                        auto,
                    ),
                    None => {
                        OnlineScreen::load(&self.motif_dir, &self.lib.root, &self.lib, &self.config)
                    }
                };
                self.screen = match r {
                    Ok(o) => Screen::Online(Box::new(o)),
                    Err(e) => {
                        log::error!("online: {e:#}");
                        self.transition(Transition::Title);
                        return;
                    }
                };
            }
            Transition::Profile => {
                let r = OnlineScreen::load_profile(
                    &self.motif_dir,
                    &self.lib.root,
                    &self.lib,
                    &self.config,
                );
                self.screen = match r {
                    Ok(o) => Screen::Online(Box::new(o)),
                    Err(e) => {
                        log::error!("profile: {e:#}");
                        self.transition(Transition::Title);
                        return;
                    }
                };
            }
            Transition::Title if self.parked.is_some() => {
                // the options were opened from the pause menu: back to the match
                if let Some(m) = self.parked.take() {
                    self.screen = Screen::Match(m);
                    self.open_pause(0);
                }
            }
            Transition::Title if !self.booted && !self.quick => {
                // the screenpack's logo and intro once, before the first title
                self.booted = true;
                let stories = self
                    .system
                    .as_ref()
                    .map(|sys| crate::storyboard::system_storyboards(&sys.def_path, &self.data_dir))
                    .unwrap_or_default();
                let t = stories
                    .into_iter()
                    .rev()
                    .fold(Transition::Title, |then, p| {
                        Transition::Story(p, Box::new(then))
                    });
                self.transition(t);
            }
            Transition::Title => {
                // leaving the online flow drops a parked matchup (the worker leaves it)
                self.parked_online = None;
                self.online_match = None;
                self.save_replay();
                self.drop_pause();
                self.run = None;
                self.play_system_music("title");
                let netplay = !self.config.net.remote.trim().is_empty();
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Title) => {
                        match UiScreen::load(
                            UiKind::Title,
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/title.toml: {e:#}");
                                Screen::Title(Box::new(TitleScreen::new(sys, netplay)))
                            }
                        }
                    }
                    Some(sys) => Screen::Title(Box::new(TitleScreen::new(sys, netplay))),
                    None => {
                        log::error!("no system.def: cannot show the title screen");
                        self.quit = true;
                        Screen::Blank
                    }
                };
            }
            Transition::Options => {
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Options) => {
                        match UiScreen::load(
                            UiKind::Options,
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/options.toml: {e:#}");
                                Screen::Options(Box::new(OptionsScreen::new(
                                    self.config.clone(),
                                    sys,
                                    self.lib.themes(),
                                )))
                            }
                        }
                    }
                    Some(sys) => Screen::Options(Box::new(OptionsScreen::new(
                        self.config.clone(),
                        sys,
                        self.lib.themes(),
                    ))),
                    None => Screen::Blank,
                };
                if let (Some(tab), Screen::Ui(u), Some(sys)) = (
                    self.reopen_tab.take(),
                    &mut self.screen,
                    self.system.as_ref(),
                ) {
                    u.set_tab(tab, sys);
                }
            }
            Transition::Controls(player) => {
                let loaded = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Controls) => {
                        match UiScreen::load(
                            UiKind::Controls,
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                        ) {
                            Ok(mut u) => {
                                u.set_player(player, sys);
                                u.set_from_pause(self.parked.is_some(), sys);
                                Some(u)
                            }
                            Err(e) => {
                                log::error!("ui/controls.toml: {e:#}");
                                None
                            }
                        }
                    }
                    _ => None,
                };
                match loaded {
                    Some(u) => self.screen = Screen::Ui(Box::new(u)),
                    None => {
                        log::warn!("no ui/controls.toml in this theme");
                        self.transition(Transition::Options);
                    }
                }
            }
            Transition::SelectAgain(mode, picks) => {
                self.transition(Transition::Select(mode));
                if let (Screen::Ui(u), Some(sys)) = (&mut self.screen, self.system.as_ref()) {
                    u.restore_picks(&picks, sys);
                }
            }
            Transition::StageSelect(plan) => {
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Stages) => {
                        match UiScreen::load_stages(
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                            *plan,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/stages.toml: {e:#}");
                                self.transition(Transition::Title);
                                return;
                            }
                        }
                    }
                    // a theme without a stage screen: a random stage, straight to the VS screen
                    _ => {
                        let mut plan = *plan;
                        let stages = self.lib.stages();
                        let k = (self.ticks as usize) % stages.len().max(1);
                        plan.setup.stage = stages
                            .get(k)
                            .and_then(|e| self.lib.prepare_stage(e).ok())
                            .map(|(d, m)| m.def_path(&d))
                            .or_else(|| self.lib.default_stage());
                        self.transition(Transition::Versus(plan.setup, plan.portraits));
                        return;
                    }
                };
            }
            Transition::Select(mode) => {
                // a run ends where its fighter select opens again
                self.run = None;
                self.play_system_music("select");
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Select(mode)) => {
                        match UiScreen::load(
                            UiKind::Select(mode),
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::warn!("ui/select.toml: {e:#}; using the screenpack select");
                                Screen::Select(Box::new(SelectScreen::new(
                                    mode,
                                    sys,
                                    &self.lib,
                                    self.config.game.ai_level,
                                )))
                            }
                        }
                    }
                    Some(sys) => Screen::Select(Box::new(SelectScreen::new(
                        mode,
                        sys,
                        &self.lib,
                        self.config.game.ai_level,
                    ))),
                    None => Screen::Blank,
                };
            }
            Transition::Versus(setup, sides) => {
                self.play_system_music("vs");
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Versus) => {
                        match UiScreen::load_versus(
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            &self.lib,
                            setup.clone(),
                            sides,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/vs.toml: {e:#}");
                                self.transition(Transition::Fight(setup));
                                return;
                            }
                        }
                    }
                    Some(sys) => Screen::Versus(Box::new(VersusScreen::new(setup, sides, sys))),
                    None => {
                        self.transition(Transition::Fight(setup));
                        return;
                    }
                };
            }
            Transition::Fight(setup) => {
                self.drop_pause();
                self.parked = None;
                // the fight being replaced (rematch, next arcade stage, online
                // rematch) takes its voices and samples with it
                if let Some(a) = self.audio.as_mut() {
                    a.release_fight();
                }
                self.match_stats = Default::default();
                // online: both sides must simulate with the same settings
                let cfg = match (&setup.net, &self.online_match) {
                    (Some(_), Some(om)) => crate::online::match_config(&self.config, &om.rules),
                    _ => self.config.clone(),
                };
                match Match::load(&setup, &self.motif_dir, &self.data_dir, &cfg) {
                    Ok(mut m) => {
                        m.start_label = crate::keylabel::Labels::new(
                            &self.config.input,
                            0,
                            self.input.devices()[0],
                        )
                        .start();
                        // Tag teams: what each side presses for TAG
                        for side in 0..2 {
                            m.tag_labels[side] = crate::keylabel::cap(
                                &self.config.input,
                                side,
                                crate::keylabel::TAG,
                                self.input.devices()[side],
                            )
                            .plain();
                            // arena hints: X jumps off a ledge, B grabs an item
                            m.arena_caps[side] = [7, 5].map(|slot| {
                                crate::keylabel::cap(
                                    &self.config.input,
                                    side,
                                    slot,
                                    self.input.devices()[side],
                                )
                                .plain()
                            });
                        }
                        if setup.tutorial {
                            let keys = crate::keylabel::Labels::new(
                                &self.config.input,
                                0,
                                self.input.devices()[0],
                            );
                            m.tutorial =
                                Some(crate::tutorial::Tutorial::new(&m.world.data[0], &keys));
                        }
                        if let Some(a) = self.audio.as_mut() {
                            a.play_music(m.bgm.as_deref(), 1.0, true);
                        }
                        if setup.net.is_some() {
                            m.ranked = self.online_match.as_ref().map(|om| om.ranked);
                        }
                        // the training lab: local TRAINING matches only
                        if crate::training::applies(&setup) {
                            let mut t = crate::training::Training::new(self.training_cfg.clone());
                            let devices = self.input.devices();
                            let labels = [0, 1].map(|i| {
                                crate::keylabel::Labels::new(&self.config.input, i, devices[i])
                            });
                            t.set_labels([&labels[0], &labels[1]]);
                            m.training = Some(t);
                        }
                        if setup.net.is_none() {
                            m.record = Some(crate::replay::Replay::new(
                                &setup,
                                m.world.fighter_size,
                                m.world.arena_ai,
                                m.world.round.config.round_time,
                                m.world.seed(),
                            ));
                        }
                        if let Some(r) = self.replay.take() {
                            m.playback = Some((r.inputs, 0));
                        }
                        let initial = m.world.events.clone();
                        m.handle_events(initial, &mut self.audio);
                        if let Some(g) = self.gpu.as_mut() {
                            m.init_gpu(&mut g.renderer);
                        }
                        // scripted screenshots start once the round is live
                        if self.harness.script.is_some()
                            && self.harness.frames.is_none()
                            && setup.net.is_none()
                        {
                            let mut guard = 0;
                            while m.world.round.round_state() != 2 && guard < 2000 {
                                m.world.step(&[Buttons::default(), Buttons::default()]);
                                guard += 1;
                            }
                            let p = &mut m.world.players[0];
                            p.power = p.power_max;
                        }
                        self.screen = Screen::Match(Box::new(m));
                    }
                    Err(e) => {
                        log::error!("could not start the match: {e:#}");
                        if self.quick {
                            self.quit = true;
                        } else {
                            self.transition(Transition::Title);
                        }
                    }
                }
            }
        }
    }

    /// One shell tick (menus or a match), measured by the latency probe.
    fn tick(&mut self) {
        let start = Instant::now();
        self.tick_inner();
        if crate::framestats::enabled() {
            crate::framestats::add(crate::framestats::Phase::Update, start.elapsed());
        }
        self.dirty = true;
        self.latency
            .ticked(start, self.harness.script.is_some() && self.is_match());
    }

    /// Run the ticks that are due by the wall clock.
    fn run_ticks(&mut self) {
        let now = Instant::now();
        self.tick_acc += now - self.last;
        self.last = now;
        // the tick length can stretch a little during online matches
        // (time sync); it is read again before every tick
        let mut len = self.tick_len();
        while self.tick_acc >= len {
            self.tick_acc -= len;
            self.tick();
            len = self.tick_len();
            // a harness screenshot shows exactly frame N, even after a slow frame
            if self.quit || (self.shot_armed && self.harness.shot.is_some()) {
                break;
            }
        }
        if self.tick_acc > len * 4 {
            self.tick_acc = Duration::ZERO;
        }
    }

    fn is_match(&self) -> bool {
        matches!(self.screen, Screen::Match(_))
    }

    /// Length of the current tick: 1/60 s, stretched slightly by an online
    /// match's time sync while this side runs ahead of the other player.
    fn tick_len(&self) -> Duration {
        match &self.screen {
            Screen::Match(m) if self.pause.is_none() => m.tick_duration(),
            _ => TICK,
        }
    }

    fn tick_inner(&mut self) {
        if self.fade > 0.0 {
            self.fade -= 1.0 / 12.0;
        }
        if self
            .theme_watch
            .as_mut()
            .is_some_and(crate::themes::Watch::poll)
        {
            self.live_reload_theme();
        }
        self.input.poll();
        self.key_ticks += 1;
        let due: Vec<(String, u32)> = self
            .harness
            .keys
            .iter()
            .filter(|(t, _, _)| *t == self.key_ticks)
            .map(|(_, k, hold)| (k.clone(), *hold))
            .collect();
        for (name, hold) in due {
            let key = crate::input::parse_key(&name);
            self.key(&key, true);
            self.key_releases.push((self.key_ticks + hold.max(1), key));
        }
        let t = self.key_ticks;
        let (up, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.key_releases)
            .into_iter()
            .partition(|(at, _)| *at <= t);
        self.key_releases = keep;
        for (_, key) in up {
            self.key(&key, false);
        }
        // scripted mouse (`--mouse`)
        let acts: Vec<MouseAct> = self
            .harness
            .mouse
            .iter()
            .filter(|(at, _)| *at == t)
            .map(|(_, a)| *a)
            .collect();
        for a in acts {
            match a {
                MouseAct::Move(x, y) => self.mouse_moved(x, y),
                MouseAct::Click => {
                    self.mouse_button(true);
                    self.mouse_releases.push(t + 3);
                }
                MouseAct::Wheel(l) => self.mouse_wheel(l),
            }
        }
        if self.mouse_releases.contains(&t) {
            self.mouse_releases.retain(|at| *at != t);
            self.mouse_button(false);
        }
        if self.mouse.tick(u64::from(t)) {
            self.mouse_to_screen(MouseEvent::Left);
        }
        let raw = self.input.raw();
        let mut now = raw.map(Buttons::from_bits);
        // START: pause / menus, never a fighter input (see `input::START_BIT`)
        let mut start_now = raw.map(|b| b & crate::input::START_BIT != 0);
        self.input.end_tick();
        let menu_taps = std::mem::take(&mut self.menu_taps);
        let pad_pause = self.input.take_pause();
        let scripting = !self.harness.in_match || self.is_match();
        if let Some(script) = self.harness.script.as_mut().filter(|_| scripting) {
            if script.is_empty() {
                self.harness.script = None;
                if self.harness.frames.is_none() && !self.harness.in_match {
                    self.shot_armed = true;
                    if self.harness.shot.is_none() {
                        self.quit = true;
                    }
                }
            } else {
                now[0] = script.remove(0);
            }
        }
        if let Some(script) = self.harness.script2.as_mut().filter(|_| scripting) {
            if script.is_empty() {
                self.harness.script2 = None;
            } else {
                now[1] = script.remove(0);
            }
        }
        if self.in_menu() {
            // menu base keys (arrows / Enter no player claims) and pads with no
            // player slot drive the menu focus as player 1
            let spare = self.input.spare_pads();
            start_now[0] |= spare & crate::input::START_BIT != 0;
            let extra = self.menu_keys.bits() | menu_taps.bits() | spare;
            now[0] = Buttons::from_bits(now[0].bits() | extra);
            // START confirms in menus as A does (the pause menu is the
            // exception: there START resumes); S (a pad's L1) stays itself,
            // the stage strip pages with it
            if self.pause.is_none() {
                for (b, start) in now.iter_mut().zip(start_now) {
                    b.a |= start;
                }
            }
        }
        let start_pressed = [
            start_now[0] && !self.prev_start[0],
            start_now[1] && !self.prev_start[1],
        ];
        self.prev_start = start_now;
        let start_side = start_pressed.iter().position(|&p| p);
        let mut pressed = [
            Buttons::from_bits(now[0].bits() & !self.prev[0].bits()),
            Buttons::from_bits(now[1].bits() & !self.prev[1].bits()),
        ];
        self.prev = now;
        // a pad (or a mapped key) pressed: the mouse lets go of the focus
        if pressed.iter().any(|b| b.bits() != 0) || start_pressed.iter().any(|&p| p) {
            self.mouse_yields();
        }
        // held directions repeat in menus (the stage strip runs its own,
        // faster one)
        let strip = matches!(&self.screen, Screen::Ui(u) if u.kind == UiKind::Stages);
        let repeats = self.in_menu() && !strip;
        for (i, r) in self.menu_repeat.iter_mut().enumerate() {
            if repeats {
                let rep = r.tick(now[i]);
                pressed[i] = Buttons::from_bits(pressed[i].bits() | rep.bits());
            } else {
                r.clear();
            }
        }
        let mut sfx_queue: Vec<Option<(i32, i32)>> = Vec::new();
        let mut next: Option<Transition> = None;
        let mut advanced = true;
        let mut open_pause: Option<usize> = None;
        let mut run_end: Option<(bool, i32, i32)> = None;
        let mut result: Option<ResultCard> = None;
        match &mut self.screen {
            Screen::Blank => {}
            Screen::Title(t) => {
                if let Some(sys) = self.system.as_ref() {
                    next = t.tick(pressed, sys, &mut |s| sfx_queue.push(s));
                }
            }
            Screen::Options(o) => {
                if let Some(sys) = self.system.as_ref() {
                    next = o.tick(pressed, sys, &mut |snd| sfx_queue.push(snd));
                }
                if next.is_some() {
                    self.save_classic_options();
                }
            }
            Screen::Ui(u) => {
                if u.kind == UiKind::Controls
                    && let Some(sys) = self.system.as_ref()
                {
                    u.set_pads(self.input.pads(), sys);
                }
                u.set_held(self.prev);
                // the online fighter pick: the opponent card follows the matchup
                if let (Some(o), Some(sys)) = (self.parked_online.as_mut(), self.system.as_ref())
                    && u.kind == UiKind::Select(Mode::Online)
                {
                    o.refresh_snapshot();
                    let (name, fighter) = o.opponent();
                    u.set_online_foe(name, fighter, sys);
                }
                if let Some(sys) = self.system.as_ref() {
                    u.set_devices(self.input.devices(), sys);
                }
                if let (Some(sys), Some(g)) = (self.system.as_mut(), self.gpu.as_mut()) {
                    next = u.tick(pressed, sys, &mut g.renderer, &mut |snd| {
                        sfx_queue.push(snd)
                    });
                }
                // play the leave animation before switching screens
                if let Some(t) = next.take() {
                    if u.leaving() {
                        next = Some(t);
                    } else {
                        u.leave();
                        self.deferred = Some(t);
                    }
                }
                if self.deferred.is_some() && u.left() {
                    next = self.deferred.take();
                }
                let is_options = matches!(u.kind, UiKind::Options | UiKind::Controls);
                if u.kind == UiKind::Controls && matches!(next, Some(Transition::Options)) {
                    self.reopen_tab = Some(crate::settings::CONTROLS_TAB);
                }
                let run_mode = match (&next, u.mode()) {
                    (Some(Transition::Versus(..)), Some(m)) if m.is_ladder() => Some(m),
                    _ => None,
                };
                if next.is_some() && is_options {
                    self.apply_options();
                }
                if let Some(m) = run_mode
                    && let Some(t) = next.take()
                {
                    next = Some(self.start_run(m, t));
                }
            }
            Screen::Online(o) => {
                // `rugen play --server --shot`: the matchup screen is the picture
                let auto_shot =
                    self.harness.shot.is_some() && self.harness.frames.is_none() && o.is_auto();
                o.hold_for_shot = auto_shot;
                if let (Some(sys), Some(g)) = (self.system.as_mut(), self.gpu.as_mut()) {
                    next = o.tick(pressed, sys, &mut g.renderer, &mut |snd| {
                        sfx_queue.push(snd)
                    });
                }
                if matches!(next, Some(Transition::Fight(_) | Transition::Versus(..))) {
                    self.online_match = o.started.take();
                }
                // `--shot` of the matchup screen once its verdict settled
                if auto_shot && o.shot_ready() {
                    self.shot_armed = true;
                }
            }
            Screen::Select(s) => {
                if let Some(sys) = self.system.as_ref() {
                    next = s.tick(pressed, sys, &mut |snd| sfx_queue.push(snd));
                }
                let mode = s.mode();
                if mode.is_ladder()
                    && let Some(t @ Transition::Versus(..)) = next.take()
                {
                    next = Some(self.start_run(mode, t));
                }
            }
            Screen::Versus(v) => next = v.tick(pressed),
            Screen::Story(st, then) => {
                // START skips the storyboard, a fight button the scene
                if start_side.is_some() {
                    st.skip();
                } else if pressed
                    .iter()
                    .any(|b| b.a || b.b || b.c || b.x || b.y || b.z)
                {
                    st.next_scene();
                }
                st.tick(&mut self.audio);
                if st.done() {
                    next = Some(std::mem::replace(then.as_mut(), Transition::Title));
                }
            }
            Screen::Match(_) if self.pause.is_some() => {
                // the pause menu runs instead of the simulation
                let mut resume = None;
                if let (Some(p), Some(sys), Some(g)) =
                    (self.pause.as_mut(), self.system.as_mut(), self.gpu.as_mut())
                {
                    p.set_devices(self.input.devices(), sys);
                    resume = p.tick(pressed, sys, &mut g.renderer, &mut |snd| {
                        sfx_queue.push(snd)
                    });
                    // training settings edited in the menu apply at once
                    if let Some(cfg) = p.training_cfg() {
                        if let Screen::Match(m) = &mut self.screen
                            && let Some(t) = m.training.as_mut()
                        {
                            t.cfg = cfg.clone();
                        }
                        self.training_cfg = cfg;
                    }
                    // START, Esc and a pad's Select / Back resume
                    if resume.is_none() && (pad_pause.is_some() || start_side.is_some()) {
                        resume = Some(Transition::Resume);
                    }
                }
                advanced = false;
                match resume {
                    Some(t @ (Transition::Options | Transition::Controls(_))) => {
                        if let Screen::Match(m) = std::mem::replace(&mut self.screen, Screen::Blank)
                        {
                            self.parked = Some(m);
                        }
                        self.drop_pause();
                        next = Some(t);
                    }
                    other => next = other,
                }
            }
            Screen::Match(m) => {
                // START, Esc or a pad's Select/Back opens the pause menu in
                // local matches (S is the fighters' button: power charge, taunts)
                // START ending a training recording does nothing else
                let used = start_side.is_some() && m.start_pressed();
                let pause_req = if used { None } else { start_side.or(pad_pause) };
                if let Some(side) = pause_req
                    && !m.is_net()
                    && !m.is_tutorial()
                    && !m.world.round.match_over
                    && m.world.round.round_state() == 2
                {
                    open_pause = Some(side);
                }
                // netplay: Select/Back opens the online menu instead (the
                // match keeps running under it)
                if pause_req.is_some() && m.is_netplay_seat() {
                    m.toggle_flee_menu();
                }
                let mut now = now;
                if m.is_netplay_seat() {
                    // one local fighter: every local control drives it,
                    // whichever seat the server gave us
                    now = crate::input::seat_inputs(now);
                    harness_flee(m);
                    if m.flee_menu.is_some() {
                        m.flee_menu_input(crate::input::seat_inputs(pressed)[0]);
                        // the menu has the controls: the fighter stands still
                        now = [Buttons::default(); 2];
                    }
                }
                advanced = m.tick(now, &mut self.audio);
                if advanced {
                    self.match_stats.observe(&m.world);
                }
                // a run: the strip over the fight (and Time Attack's clock)
                if let Some(run) = self.run.as_mut() {
                    if advanced && !m.world.round.match_over {
                        run.tick();
                    }
                    m.run_hud = run.hud();
                }
                // F1 toggles a training tool in the match: the next one keeps it
                if let Some(t) = m.training.as_ref() {
                    self.training_cfg = t.cfg.clone();
                }
                if m.leave {
                    if let Some(om) = self.online_match.take() {
                        // a match played out, or ended early by a flee /
                        // disconnect (`Match::winner` knows both)
                        let winner = m.winner();
                        let won = winner == Some(om.my_side);
                        if let Some(p) = crate::online::report_result(
                            &self.lib.root,
                            &self.config,
                            &om,
                            winner,
                            m.evidence(),
                        ) {
                            self.reports.push((p, won));
                        }
                        // back to the online screen after an online match
                        next = Some(if self.quick {
                            Transition::Quit
                        } else {
                            Transition::Online
                        });
                    } else if m.is_tutorial() {
                        next = Some(if self.quick {
                            Transition::Quit
                        } else {
                            Transition::Title
                        });
                    } else {
                        let p = &m.world.players[0];
                        run_end = Some((m.winner() == Some(0), p.life, p.life_max));
                        result = Some(m.result_card(self.quick, self.run.is_some()));
                    }
                }
            }
        }
        if let Some(side) = open_pause {
            self.open_pause(side);
        }
        let run_rows = match run_end {
            Some((won, life, max)) => self.finish_run_fight(won, life, max),
            None => Vec::new(),
        };
        if let Some((setup, winner, wins, names, arts, quick, arcade)) = result {
            let profile = crate::uiscreen::Profile::load(&self.lib.root);
            let rating = if setup.net.is_some() {
                String::new()
            } else {
                profile.rating_text()
            };
            let side = winner.unwrap_or(0);
            let quote = {
                let def = if side == 0 { &setup.p1 } else { &setup.p2 };
                let quotes = crate::screeninfo::victory_quotes(def);
                match quotes.len() {
                    0 => STOCK_QUOTES[self.match_stats.ticks as usize % STOCK_QUOTES.len()]
                        .to_string(),
                    n => quotes[self.match_stats.ticks as usize % n].clone(),
                }
            };
            let mut stats = self.match_stats.rows(side, wins);
            // a CHALLENGE run: its rows instead of the time and supers
            if !run_rows.is_empty() {
                stats.truncate(3);
                stats.extend(run_rows);
            }
            next = Some(Transition::Result(Box::new(ResultInfo {
                setup,
                winner,
                wins,
                names,
                arts,
                quick,
                arcade,
                rating,
                quote,
                stats,
            })));
        }
        if next.is_some() {
            self.record_result();
        }
        // finished result reports
        let root = self.lib.root.clone();
        self.reports.retain_mut(|(p, won)| match p.take() {
            Some(ack) => {
                crate::online::apply_result(&root, &ack, *won);
                false
            }
            None => true,
        });
        if matches!(next, Some(Transition::Result(_)) | Some(Transition::Quit)) {
            self.save_replay();
        }
        for s in sfx_queue {
            self.system_sfx(s);
        }
        if next.is_none() {
            self.live_options();
        }
        if let Some(t) = next {
            self.transition(t);
        }
        // a pause menu holds the match but still counts toward `--frames`
        // (so a harness run can screenshot it)
        let counted = !self.harness.in_match || self.is_match();
        if (advanced || self.pause.is_some()) && counted {
            self.ticks += 1;
            if let Some(n) = self.harness.frames
                && self.ticks >= n
            {
                self.shot_armed = true;
                if self.harness.shot.is_none() {
                    self.quit = true;
                }
            }
        }
    }

    fn render(&mut self) {
        if let Screen::Match(m) = &mut self.screen
            && let Some(g) = self.gpu.as_mut()
        {
            m.refresh_gpu(&mut g.renderer);
        }
        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = &mut gpu.renderer;
        let build_start = Instant::now();
        r.begin();
        let (cam, clear) = match (&mut self.screen, self.system.as_mut()) {
            (Screen::Match(_), Some(sys)) if self.pause.is_some() => match self.pause.as_mut() {
                Some(p) => p.render(r, sys),
                None => unreachable!(),
            },
            (Screen::Title(t), Some(sys)) => t.render(r, sys),
            (Screen::Select(s), Some(sys)) => s.render(r, sys),
            (Screen::Options(o), Some(sys)) => o.render(r, sys),
            (Screen::Ui(u), Some(sys)) => u.render(r, sys),
            (Screen::Online(o), Some(sys)) => o.render(r, sys),
            (Screen::Versus(v), Some(sys)) => v.render(r, sys),
            (Screen::Story(st, _), _) => st.render(r),
            (Screen::Match(m), _) => m.render(r),
            _ => (
                rugen_render::Camera {
                    screen: [320.0, 240.0],
                    center: [0.0, 120.0],
                    zoom: 1.0,
                },
                [0.0, 0.0, 0.0, 1.0],
            ),
        };
        if let Screen::Match(m) = &self.screen
            && let Some(t) = m.debug_title()
        {
            gpu.window.set_title(&t);
        }
        if self.fade > 0.0 {
            r.rect(
                [-100_000.0, -100_000.0],
                [100_000.0, 100_000.0],
                [0.0, 0.0, 0.0, self.fade.min(1.0)],
            );
        }
        let profiling = crate::framestats::enabled();
        if profiling {
            crate::framestats::add(crate::framestats::Phase::Build, build_start.elapsed());
        }
        if self.shot_armed
            && let Some(path) = self.harness.shot.take()
        {
            let (w, h) = r.size();
            let rgba = r.end_to_image(&cam, clear, w, h);
            match crate::play::write_png(&path, w, h, &rgba) {
                Ok(()) => log::info!("wrote {}", path.display()),
                Err(e) => log::error!("--shot {}: {e}", path.display()),
            }
            self.quit = true;
            return;
        }
        let submit_start = Instant::now();
        let ended = r.end(&cam, clear);
        if profiling {
            crate::framestats::add(crate::framestats::Phase::Submit, submit_start.elapsed());
            let name = match &self.screen {
                Screen::Blank => "blank".to_string(),
                Screen::Title(_) => "title(legacy)".into(),
                Screen::Options(_) => "options(legacy)".into(),
                Screen::Ui(u) => format!("{:?}", u.kind).to_lowercase(),
                Screen::Online(_) => "online".into(),
                Screen::Select(_) => "select(legacy)".into(),
                Screen::Versus(_) => "versus(legacy)".into(),
                Screen::Match(_) => "match".into(),
                Screen::Story(..) => "storyboard".into(),
            };
            self.frame_stats.frame(&name, r);
        }
        match ended {
            Ok(()) => {
                let t = r.timing();
                if let Some(at) = t.presented {
                    self.latency.presented(at, t.acquire, t.acquired);
                }
            }
            Err(rugen_render::SurfaceError::Lost | rugen_render::SurfaceError::Outdated) => {
                let s = gpu.window.inner_size();
                gpu.renderer.resize(s.width, s.height);
            }
            Err(e) => log::warn!("render: {e}"),
        }
    }

    fn key(&mut self, key: &Key, pressed: bool) {
        // a key press takes the focus back from the mouse (modifiers alone do not)
        if pressed
            && !matches!(
                key,
                Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super)
            )
        {
            self.mouse_yields();
        }
        // a release always reaches the held-key state, whatever screen (text
        // capture, key listening, a page that owns the key) takes the press:
        // a lost release would leave the key held and eat its next press
        if !pressed {
            let lower = lowercase(key);
            self.input.key(&lower, false);
            if let Some(bit) = menu_bit(&lower) {
                self.menu_keys = set_menu_bit(self.menu_keys, bit, false);
            }
        }
        // controls page: held keys light the test pad, page keys act
        let mut page = None;
        if let Screen::Ui(u) = &mut self.screen
            && u.kind == UiKind::Controls
            && !u.capturing()
            && let Some(sys) = self.system.as_ref()
        {
            let owns = u.controls_owns(key);
            let t = u.controls_key(key, pressed, sys);
            page = Some((owns, t));
        }
        if let Some((owns, t)) = page {
            if let Some(t) = t {
                self.apply_options();
                self.transition(t);
                return;
            }
            if owns {
                return;
            }
        }
        if let Screen::Options(o) = &mut self.screen
            && o.capturing()
        {
            if pressed {
                o.raw_key(key);
            }
            return;
        }
        if let Screen::Ui(u) = &mut self.screen
            && u.capturing()
        {
            // keys the capture hands back (arrows leave the roster search) go on
            let consumed = match self.system.as_ref() {
                Some(sys) if pressed => u.raw_key(key, sys),
                _ => true,
            };
            if consumed {
                return;
            }
        }
        // Ctrl+S: save on the options / controls pages (F2 too)
        if pressed
            && self.ctrl
            && lowercase(key) == Key::Character("s".into())
            && let Screen::Ui(u) = &mut self.screen
            && matches!(u.kind, UiKind::Options | UiKind::Controls)
            && let Some(sys) = self.system.as_ref()
        {
            u.hotkey(&Key::Named(NamedKey::F2), sys);
            self.settings_request();
            return;
        }
        // Ctrl+V: paste a room code
        if pressed
            && self.ctrl
            && lowercase(key) == Key::Character("v".into())
            && let Screen::Online(o) = &mut self.screen
            && o.paste()
        {
            return;
        }
        if let Screen::Online(o) = &mut self.screen
            && o.capturing()
            && (!pressed || o.raw_key(key))
        {
            return;
        }
        let lower = lowercase(key);
        // Esc backs out of every screen whatever the bindings say
        if key == &Key::Named(NamedKey::Escape) {
            if pressed {
                self.escape();
            }
            if !self.input.bound(&lower) {
                return;
            }
        }
        // arrows / Enter work in every menu; a player mapping that uses them keeps them
        if let Some(bit) = menu_bit(&lower)
            && !self.input.bound(&lower)
        {
            self.menu_keys = set_menu_bit(self.menu_keys, bit, pressed);
            if pressed {
                self.menu_taps = set_menu_bit(self.menu_taps, bit, true);
            }
        }
        if self.input.key(&lower, pressed) {
            return;
        }
        if let Screen::Match(m) = &mut self.screen
            && self.pause.is_none()
            && m.key(key, pressed)
        {
            return;
        }
        if pressed
            && let Screen::Ui(u) = &mut self.screen
            && let Some(sys) = self.system.as_ref()
            && u.hotkey(key, sys)
        {
            return;
        }
        // ONLINE: letter keys no player binding uses (P PROFILE, W WATCH...)
        if pressed
            && let Screen::Online(o) = &mut self.screen
            && o.hotkey(&lower)
        {
            return;
        }
        if pressed && key == &Key::Named(NamedKey::F11) {
            self.fullscreen = !self.fullscreen;
            if let Some(g) = &self.gpu {
                g.window.set_fullscreen(
                    self.fullscreen
                        .then_some(winit::window::Fullscreen::Borderless(None)),
                );
            }
            // keep an open options screen from switching it back on its next change
            self.config.video.fullscreen = self.fullscreen;
            match &mut self.screen {
                Screen::Options(o) => o.config.video.fullscreen = self.fullscreen,
                Screen::Ui(u) if matches!(u.kind, UiKind::Options | UiKind::Controls) => {
                    u.config.video.fullscreen = self.fullscreen;
                }
                _ => {}
            }
        }
    }

    /// Esc: back / cancel on every screen, the pause menu in a fight.
    fn escape(&mut self) {
        match self.screen {
            Screen::Title(_) | Screen::Blank => self.quit = true,
            // the first Esc on the title only moves to EXIT
            Screen::Ui(ref mut u) if u.kind == UiKind::Title => {
                if !u.focus_exit() {
                    self.quit = true;
                }
            }
            Screen::Ui(ref mut u) => {
                let kind = u.kind;
                let t = match self.system.as_ref() {
                    Some(sys) => u.escape(sys),
                    None => Some(Transition::Title),
                };
                if let Some(t) = t {
                    if kind == UiKind::Controls {
                        self.reopen_tab = Some(crate::settings::CONTROLS_TAB);
                    }
                    self.apply_options();
                    self.transition(t);
                }
            }
            Screen::Online(ref mut o) => {
                if o.back() {
                    self.transition(Transition::Title);
                }
            }
            Screen::Select(_) => {
                let snd = self.system.as_ref().and_then(|s| s.select.cancel_snd);
                self.system_sfx(snd);
                self.transition(Transition::Title);
            }
            Screen::Options(_) => {
                self.save_classic_options();
                self.transition(Transition::Title);
            }
            Screen::Match(ref m) if self.pause.is_some() => {
                let _ = m;
                self.drop_pause();
            }
            // netplay: the online menu (RESUME / FLEE); once the match is
            // over, Esc leaves the normal way (the result is reported)
            Screen::Match(ref mut m) if m.is_netplay_seat() => {
                if m.world.round.match_over || m.early_end().is_some() {
                    m.leave = true;
                } else {
                    m.toggle_flee_menu();
                }
            }
            Screen::Match(ref m)
                if !m.is_net()
                    && !m.is_tutorial()
                    && !m.world.round.match_over
                    && UiScreen::available(&self.motif_dir, UiKind::Pause) =>
            {
                self.open_pause(0);
            }
            Screen::Story(ref mut st, _) => st.skip(),
            Screen::Versus(_) | Screen::Match(_) => {
                if self.quick {
                    self.quit = true;
                } else {
                    self.transition(Transition::Title);
                }
            }
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.save_replay();
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let size = (
            self.config.video.width.max(320),
            self.config.video.height.max(240),
        );
        let presentation = if self.config.video.low_latency {
            rugen_render::Presentation::LowLatency
        } else {
            rugen_render::Presentation::Vsync
        };
        let mut gpu = Gpu::create(event_loop, size, self.config.video.fullscreen, presentation);
        self.latency.set_mode(
            format!("{:?}", gpu.renderer.present_mode()),
            gpu.renderer.frame_latency(),
        );
        self.vsync_paced = self.config.video.low_latency
            && gpu.renderer.acquire_blocks()
            && std::env::var_os("RUGEN_NO_VSYNC_PACING").is_none();
        if let Screen::Match(m) = &mut self.screen {
            m.init_gpu(&mut gpu.renderer);
        }
        self.gpu = Some(gpu);
        if let Some(t) = self.start.take() {
            self.transition(t);
        }
        self.last = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.dirty = true;
        }
        match event {
            WindowEvent::CloseRequested => {
                // unsaved options / controls: ask SAVE / DISCARD / CANCEL
                // first, as leaving the screen does (the classic options
                // screen saves on the way out, as leaving it does)
                self.track_unsaved();
                let ask = match (&mut self.screen, self.system.as_ref()) {
                    (Screen::Ui(u), Some(sys)) => u.ask_before_quit(sys),
                    _ => false,
                };
                if !ask {
                    self.save_classic_options();
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gpu {
                    g.renderer.resize(s.width, s.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if self.vsync_paced {
                    // wait for the vblank first, then simulate: the frame
                    // drawn next carries the newest input to the screen
                    let ok = self
                        .gpu
                        .as_mut()
                        .is_some_and(|g| g.renderer.acquire().is_ok());
                    if ok {
                        self.run_ticks();
                    }
                    self.last_redraw = Instant::now();
                }
                self.render();
                if self.quit {
                    event_loop.exit();
                }
            }
            // keys released while another window has the focus never send their
            // release here: forget them all instead of leaving them held
            WindowEvent::ModifiersChanged(m) => {
                self.ctrl = m.state().control_key() || m.state().super_key();
            }
            WindowEvent::Focused(false) => {
                self.ctrl = false;
                self.input.release_all();
                self.menu_keys = Buttons::default();
                self.menu_taps = Buttons::default();
            }
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                if event.state == ElementState::Pressed {
                    self.latency.key();
                }
                let key = event.logical_key.clone();
                self.key(&key, event.state == ElementState::Pressed);
                if self.quit && self.harness.shot.is_none() {
                    event_loop.exit();
                }
            }
            // a scripted run is driven by the script alone; a stray cursor would steal
            // focus. `[input] mouse = false` (the default) keeps the mouse out entirely.
            WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
                if self.harness.script.is_some() || !self.config.input.mouse => {}
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::CursorLeft { .. } => {
                self.mouse.left();
                self.mouse_to_screen(MouseEvent::Left);
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => self.mouse_button(state == ElementState::Pressed),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.mouse_wheel(lines);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.vsync_paced && self.gpu.is_some() {
            // ticks run in the redraw; keep them going when the compositor
            // stops sending frames (minimised window: an online match must
            // not stall)
            if now.duration_since(self.last_redraw) > Duration::from_millis(50) {
                self.run_ticks();
            }
            if self.quit && self.harness.shot.is_none() {
                event_loop.exit();
            }
            self.update_cursor();
            if let Some(g) = &self.gpu {
                g.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(20)));
            return;
        }
        self.run_ticks();
        let len = self.tick_len();
        if self.quit && self.harness.shot.is_none() {
            event_loop.exit();
        }
        self.update_cursor();
        let low_latency = self.config.video.low_latency;
        // low latency: draw right after a tick (newest state, nothing
        // queued behind it) instead of spinning frames between ticks
        if let Some(g) = &self.gpu
            && (self.dirty || !low_latency)
        {
            self.dirty = false;
            g.window.request_redraw();
        }
        if low_latency {
            // sleep until shortly before the next tick, then poll to hit it
            let left = len.saturating_sub(self.tick_acc);
            if left > Duration::from_millis(2) {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    now + left - Duration::from_millis(1),
                ));
            } else {
                event_loop.set_control_flow(ControlFlow::Poll);
            }
        }
    }
}

/// Harness: `RUGEN_HARNESS_FLEE_MENU=n` opens the online menu at match
/// frame `n` (`nc`: straight on its flee question); `RUGEN_HARNESS_FLEE=n`
/// flees at frame `n`.
fn harness_flee(m: &mut Match) {
    let frame = m.world.frame;
    let var = |name: &str| std::env::var(name).ok();
    if let Some(v) = var("RUGEN_HARNESS_FLEE_MENU") {
        let confirm = v.ends_with('c');
        if v.trim_end_matches('c').parse::<u32>().ok() == Some(frame) && m.flee_menu.is_none() {
            m.toggle_flee_menu();
            if confirm && let Some(menu) = m.flee_menu.as_mut() {
                menu.confirm = true;
                menu.sel = 1;
            }
        }
    }
    if var("RUGEN_HARNESS_FLEE").and_then(|v| v.parse::<u32>().ok()) == Some(frame) {
        m.flee();
    }
}

/// Menu base key → bit (0..3 = up, down, left, right; 4 = confirm).
fn menu_bit(key: &Key) -> Option<u8> {
    match key {
        Key::Named(NamedKey::ArrowUp) => Some(0),
        Key::Named(NamedKey::ArrowDown) => Some(1),
        Key::Named(NamedKey::ArrowLeft) => Some(2),
        Key::Named(NamedKey::ArrowRight) => Some(3),
        Key::Named(NamedKey::Enter) => Some(4),
        _ => None,
    }
}

/// Set one menu base key bit (see [`menu_bit`]).
fn set_menu_bit(mut b: Buttons, bit: u8, on: bool) -> Buttons {
    match bit {
        0 => b.u = on,
        1 => b.d = on,
        2 => b.l = on,
        3 => b.r = on,
        _ => b.a = on,
    }
    b
}

/// Character keys compare lower-case (Shift or Caps Lock must not unbind them).
fn lowercase(key: &Key) -> Key {
    match key {
        Key::Character(c) => Key::Character(c.to_lowercase().into()),
        k => k.clone(),
    }
}

/// Cache manifest id for a prepared character .def (empty when unknown).
fn manifest_id(def: &std::path::Path) -> String {
    rugen_content::manifest_for(def)
        .map(|(_, m)| m.id)
        .unwrap_or_default()
}
