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
use crate::options::OptionsScreen;
use crate::play::{FightSetup, Match};
use crate::select::SelectScreen;
use crate::system::SystemData;
use crate::title::TitleScreen;
use crate::uiscreen::{MouseEvent, UiKind, UiScreen};
use crate::versus::{Portrait, VersusScreen};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);
const SYSTEM_SND: u32 = u32::MAX - 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Arcade,
    Versus,
    Training,
    Tutorial,
    Watch,
    Netplay,
    Options,
    Exit,
}

#[derive(Debug)]
pub enum Transition {
    Title,
    Select(Mode),
    /// Online match from the `[net]` section of rugen.toml.
    Netplay,
    Options,
    /// Show the VS screen, then fight.
    Versus(FightSetup, [Portrait; 2]),
    Fight(FightSetup),
    Quit,
}

enum Screen {
    Blank,
    Title(Box<TitleScreen>),
    Options(Box<OptionsScreen>),
    /// TOML-driven screen from `motif/ui/`.
    Ui(Box<UiScreen>),
    Select(Box<SelectScreen>),
    Versus(Box<VersusScreen>),
    Match(Box<Match>),
}

/// Automated-run options.
#[derive(Default)]
pub struct Harness {
    pub shot: Option<PathBuf>,
    pub frames: Option<u32>,
    pub script: Option<Vec<Buttons>>,
    /// Write a replay of every match here (quick play) or into `replays/`.
    pub record: Option<PathBuf>,
}

pub struct Shell {
    config: Config,
    lib: Library,
    data_dir: PathBuf,
    motif_dir: PathBuf,
    gpu: Option<Gpu>,
    input: InputState,
    prev: [Buttons; 2],
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
    /// Arcade ladder: P1's character and wins so far.
    arcade: Option<(PathBuf, u32)>,
    fullscreen: bool,
    /// Black overlay alpha for screen fades (1 = black).
    fade: f32,
}

impl Shell {
    pub fn new(
        config: Config,
        lib: Library,
        start: Transition,
        quick: bool,
        harness: Harness,
        replay: Option<crate::replay::Replay>,
    ) -> Self {
        let data_dir = lib.data_dir();
        let motif_dir = lib.motif_dir();
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
            config,
            lib,
            data_dir,
            motif_dir,
            gpu: None,
            prev: [Buttons::default(); 2],
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
            arcade: None,
            fullscreen: config_fullscreen,
            fade: 1.0,
        }
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    /// Take the edited config out of the options screen, apply and save it.
    fn apply_options(&mut self) {
        let (dirty, config) = match &self.screen {
            Screen::Options(o) => (o.dirty, o.config.clone()),
            Screen::Ui(u) if u.kind == UiKind::Options => (u.dirty, u.config.clone()),
            _ => return,
        };
        if !dirty {
            return;
        }
        self.config = config;
        self.input.rebind(&self.config.input);
        if let Some(a) = self.audio.as_mut() {
            a.set_master_volume(self.config.audio.master);
            a.set_music_volume(self.config.audio.music);
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
        match self.config.save(&self.lib.root) {
            Ok(()) => log::info!("settings saved to rugen.toml"),
            Err(e) => log::warn!("settings not saved: {e}"),
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

    /// Arcade: after a win, line up the next random opponent; after a loss, the title.
    fn arcade_next(&mut self) -> Option<Transition> {
        let (p1, wins) = self.arcade.clone()?;
        let Screen::Match(m) = &self.screen else {
            return None;
        };
        let won = m.world.round.wins[0] > m.world.round.wins[1];
        if !won || wins + 1 >= 6 {
            return None;
        }
        self.arcade = Some((p1.clone(), wins + 1));
        let chars = self.lib.chars();
        if chars.is_empty() {
            return None;
        }
        let pick = chars[(wins as usize * 7 + 3) % chars.len()].clone();
        let (dir, man) = self.lib.prepare_char(&pick).ok()?;
        let p2 = dir.join(&man.def);
        let stages = self.lib.stages();
        let stage = stages
            .get((wins as usize) % stages.len().max(1))
            .and_then(|e| self.lib.prepare_stage(e).ok().map(|(d, m)| d.join(m.def)))
            .or_else(|| {
                let p = self
                    .lib
                    .root
                    .join("reference/mugen-1.1b1/stages/stage0.def");
                p.exists().then_some(p)
            });
        let portrait = |def: &PathBuf| {
            let dir = def.parent()?;
            let text = std::fs::read_to_string(dir.join("rugen.json")).ok()?;
            let man: rugen_content::Manifest = serde_json::from_str(&text).ok()?;
            let pb = man.portrait_big.as_ref()?;
            let (w, h, rgba) = rugen_content::read_png(&dir.join(&pb.file)).ok()?;
            Some((man.display_name.clone(), (w, h, rgba, pb.axis)))
        };
        let side = |def: &PathBuf| match portrait(def) {
            Some((name, img)) => Portrait::new(name, Some(img)),
            None => Portrait::new(String::new(), None),
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
            },
            [side(&p1), side(&p2)],
        ))
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
            source: SYSTEM_SND,
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

    fn transition(&mut self, t: Transition) {
        // every new screen fades in from black
        self.fade = 1.0;
        match t {
            Transition::Quit => self.quit = true,
            Transition::Netplay => {
                let n = self.config.net.clone();
                let resolve = |me: &Self, id: &str| -> Option<PathBuf> {
                    let e = me.lib.find_char(id)?;
                    me.lib
                        .prepare_char(&e)
                        .map(|(d, m)| d.join(m.def))
                        .map_err(|err| log::error!("{id}: {err}"))
                        .ok()
                };
                let side = usize::from(n.side == 2);
                let net = if !n.relay.trim().is_empty() {
                    match n.relay.parse::<std::net::SocketAddr>() {
                        Ok(server) => crate::play::NetMode::Rendezvous {
                            port: n.port,
                            server,
                            room: if n.room.trim().is_empty() {
                                "rugen".into()
                            } else {
                                n.room.trim().to_string()
                            },
                            side,
                        },
                        Err(e) => {
                            log::error!("[net] relay {:?}: {e}", n.relay);
                            self.transition(Transition::Title);
                            return;
                        }
                    }
                } else {
                    match n.remote.parse::<std::net::SocketAddr>() {
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
                    .and_then(|e| self.lib.prepare_stage(&e).ok().map(|(d, m)| d.join(m.def)))
                    .or_else(|| {
                        let p = self
                            .lib
                            .root
                            .join("reference/mugen-1.1b1/stages/stage0.def");
                        p.exists().then_some(p)
                    });
                self.transition(Transition::Fight(FightSetup {
                    p1,
                    p2,
                    stage,
                    ai: [0, 0],
                    net: Some(net),
                    tutorial: false,
                    ids: [String::new(), String::new()],
                }));
            }
            Transition::Title => {
                self.save_replay();
                self.arcade = None;
                self.play_system_music("title");
                let netplay = !self.config.net.remote.trim().is_empty()
                    || !self.config.net.relay.trim().is_empty();
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Title) => {
                        match UiScreen::load(
                            UiKind::Title,
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            netplay,
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
                            false,
                            &self.lib,
                        ) {
                            Ok(u) => Screen::Ui(Box::new(u)),
                            Err(e) => {
                                log::error!("ui/options.toml: {e:#}");
                                Screen::Options(Box::new(OptionsScreen::new(
                                    self.config.clone(),
                                    sys,
                                )))
                            }
                        }
                    }
                    Some(sys) => {
                        Screen::Options(Box::new(OptionsScreen::new(self.config.clone(), sys)))
                    }
                    None => Screen::Blank,
                };
            }
            Transition::Select(mode) => {
                self.play_system_music("select");
                self.screen = match self.system.as_ref() {
                    Some(sys) if UiScreen::available(&self.motif_dir, UiKind::Select(mode)) => {
                        match UiScreen::load(
                            UiKind::Select(mode),
                            &self.motif_dir,
                            &self.lib.root,
                            sys,
                            &self.config,
                            false,
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
                    Some(sys) => Screen::Versus(Box::new(VersusScreen::new(setup, sides, sys))),
                    None => {
                        self.transition(Transition::Fight(setup));
                        return;
                    }
                };
            }
            Transition::Fight(setup) => {
                match Match::load(&setup, &self.motif_dir, &self.data_dir, &self.config) {
                    Ok(mut m) => {
                        if setup.tutorial {
                            m.tutorial = Some(crate::tutorial::Tutorial::new(
                                &m.world.data[0],
                                &self.config.input.p1,
                            ));
                        }
                        if let Some(a) = self.audio.as_mut() {
                            a.play_music(m.bgm.as_deref(), 1.0, true);
                        }
                        if setup.net.is_none() {
                            m.record = Some(crate::replay::Replay::new(&setup));
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
                        if self.harness.script.is_some() && self.harness.frames.is_none() {
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

    fn tick(&mut self) {
        if self.fade > 0.0 {
            self.fade -= 1.0 / 12.0;
        }
        self.input.poll();
        let mut now = self.input.buttons();
        if let Some(script) = self.harness.script.as_mut() {
            if script.is_empty() {
                self.harness.script = None;
                if self.harness.frames.is_none() {
                    self.shot_armed = true;
                    if self.harness.shot.is_none() {
                        self.quit = true;
                    }
                }
            } else {
                now[0] = script.remove(0);
            }
        }
        let pressed = [
            Buttons::from_bits(now[0].bits() & !self.prev[0].bits()),
            Buttons::from_bits(now[1].bits() & !self.prev[1].bits()),
        ];
        self.prev = now;
        let mut sfx_queue: Vec<Option<(i32, i32)>> = Vec::new();
        let mut next: Option<Transition> = None;
        let mut advanced = true;
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
                    self.apply_options();
                }
            }
            Screen::Ui(u) => {
                if let (Some(sys), Some(g)) = (self.system.as_mut(), self.gpu.as_mut()) {
                    next = u.tick(pressed, sys, &mut g.renderer, &mut |snd| {
                        sfx_queue.push(snd)
                    });
                }
                let is_options = u.kind == UiKind::Options;
                let arcade_pick = match (&next, u.mode()) {
                    (Some(Transition::Versus(setup, _)), Some(Mode::Arcade)) => {
                        Some(setup.p1.clone())
                    }
                    _ => None,
                };
                if next.is_some() && is_options {
                    self.apply_options();
                }
                if let Some(p1) = arcade_pick {
                    self.arcade = Some((p1, 0));
                }
            }
            Screen::Select(s) => {
                if let Some(sys) = self.system.as_ref() {
                    next = s.tick(pressed, sys, &mut |snd| sfx_queue.push(snd));
                }
                if let Some(Transition::Versus(setup, _)) = &next
                    && s.mode() == Mode::Arcade
                {
                    self.arcade = Some((setup.p1.clone(), 0));
                }
            }
            Screen::Versus(v) => next = v.tick(pressed),
            Screen::Match(m) => {
                advanced = m.tick(now, &mut self.audio);
                if m.leave {
                    next = Some(if self.quick {
                        Transition::Quit
                    } else {
                        Transition::Title
                    });
                }
            }
        }
        if next.is_some() {
            self.record_result();
        }
        if matches!(next, Some(Transition::Title))
            && let Some(t) = self.arcade_next()
        {
            self.save_replay();
            next = Some(t);
        }
        if matches!(next, Some(Transition::Quit)) {
            self.save_replay();
        }
        for s in sfx_queue {
            self.system_sfx(s);
        }
        if let Some(t) = next {
            self.transition(t);
        }
        if advanced {
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
        r.begin();
        let (cam, clear) = match (&mut self.screen, self.system.as_mut()) {
            (Screen::Title(t), Some(sys)) => t.render(r, sys),
            (Screen::Select(s), Some(sys)) => s.render(r, sys),
            (Screen::Options(o), Some(sys)) => o.render(r, sys),
            (Screen::Ui(u), Some(sys)) => u.render(r, sys),
            (Screen::Versus(v), Some(sys)) => v.render(r, sys),
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
        match r.end(&cam, clear) {
            Ok(()) => {}
            Err(rugen_render::SurfaceError::Lost | rugen_render::SurfaceError::Outdated) => {
                let s = gpu.window.inner_size();
                gpu.renderer.resize(s.width, s.height);
            }
            Err(e) => log::warn!("render: {e}"),
        }
    }

    fn key(&mut self, key: &Key, pressed: bool, event_loop: &ActiveEventLoop) {
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
            if pressed && let Some(sys) = self.system.as_ref() {
                u.raw_key(key, sys);
            }
            return;
        }
        let lower = match key {
            Key::Character(c) => Key::Character(c.to_lowercase().into()),
            k => k.clone(),
        };
        if self.input.key(&lower, pressed) {
            return;
        }
        if let Screen::Match(m) = &mut self.screen
            && m.key(key, pressed)
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
            return;
        }
        if pressed && key == &Key::Named(NamedKey::Escape) {
            match self.screen {
                Screen::Title(_) | Screen::Blank => event_loop.exit(),
                Screen::Ui(ref u) if u.kind == UiKind::Title => event_loop.exit(),
                Screen::Ui(_) => {
                    self.apply_options();
                    self.transition(Transition::Title);
                }
                Screen::Select(_) => {
                    let snd = self.system.as_ref().and_then(|s| s.select.cancel_snd);
                    self.system_sfx(snd);
                    self.transition(Transition::Title);
                }
                Screen::Options(_) => {
                    self.apply_options();
                    self.transition(Transition::Title);
                }
                Screen::Versus(_) | Screen::Match(_) => {
                    if self.quick {
                        event_loop.exit();
                    } else {
                        self.transition(Transition::Title);
                    }
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
        let mut gpu = Gpu::create(event_loop, size, self.config.video.fullscreen);
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gpu {
                    g.renderer.resize(s.width, s.height);
                }
            }
            WindowEvent::RedrawRequested => {
                self.render();
                if self.quit {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                let key = event.logical_key.clone();
                self.key(&key, event.state == ElementState::Pressed, event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Screen::Ui(u) = &mut self.screen {
                    u.mouse(MouseEvent::Move(position.x as f32, position.y as f32));
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Screen::Ui(u) = &mut self.screen {
                    u.mouse(MouseEvent::Left);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                if let Screen::Ui(u) = &mut self.screen {
                    u.mouse(MouseEvent::Button(state == ElementState::Pressed));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                if let Screen::Ui(u) = &mut self.screen {
                    u.mouse(MouseEvent::Wheel(lines));
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        self.tick_acc += now - self.last;
        self.last = now;
        while self.tick_acc >= TICK {
            self.tick_acc -= TICK;
            self.tick();
            if self.quit {
                break;
            }
        }
        if self.tick_acc > TICK * 4 {
            self.tick_acc = Duration::ZERO;
        }
        if self.quit && self.harness.shot.is_none() {
            event_loop.exit();
        }
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }
}

/// Cache manifest id for a prepared character .def (empty when unknown).
fn manifest_id(def: &std::path::Path) -> String {
    def.parent()
        .and_then(|d| std::fs::read_to_string(d.join("rugen.json")).ok())
        .and_then(|t| serde_json::from_str::<rugen_content::Manifest>(&t).ok())
        .map(|m| m.id)
        .unwrap_or_default()
}
