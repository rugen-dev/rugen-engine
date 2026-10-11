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
use crate::play::{FightSetup, Match};
use crate::select::SelectScreen;
use crate::system::SystemData;
use crate::title::TitleScreen;
use crate::versus::{Portrait, VersusScreen};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);
const SYSTEM_SND: u32 = u32::MAX - 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Arcade,
    Versus,
    Training,
    Watch,
    Exit,
}

#[derive(Debug)]
pub enum Transition {
    Title,
    Select(Mode),
    /// Show the VS screen, then fight.
    Versus(FightSetup, [Portrait; 2]),
    Fight(FightSetup),
    Quit,
}

enum Screen {
    Blank,
    Title(TitleScreen),
    Select(SelectScreen),
    Versus(VersusScreen),
    Match(Box<Match>),
}

/// Automated-run options.
#[derive(Default)]
pub struct Harness {
    pub shot: Option<PathBuf>,
    pub frames: Option<u32>,
    pub script: Option<Vec<Buttons>>,
}

pub struct Shell {
    config: Config,
    lib: Library,
    data_dir: PathBuf,
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
}

impl Shell {
    pub fn new(
        config: Config,
        lib: Library,
        start: Transition,
        quick: bool,
        harness: Harness,
    ) -> Self {
        let data_dir = lib.data_dir();
        let system = match SystemData::load(&data_dir) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("system.def not loaded: {e:#}");
                None
            }
        };
        Self {
            input: InputState::new(&config.input),
            config,
            lib,
            data_dir,
            gpu: None,
            prev: [Buttons::default(); 2],
            audio: match Audio::new() {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("audio disabled: {e}");
                    None
                }
            },
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
        }
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::Poll);
        event_loop.run_app(&mut self)?;
        Ok(())
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
        match t {
            Transition::Quit => self.quit = true,
            Transition::Title => {
                self.screen = match self.system.as_ref() {
                    Some(sys) => Screen::Title(TitleScreen::new(sys)),
                    None => {
                        log::error!("no system.def: cannot show the title screen");
                        self.quit = true;
                        Screen::Blank
                    }
                };
            }
            Transition::Select(mode) => {
                self.screen = match self.system.as_ref() {
                    Some(sys) => Screen::Select(SelectScreen::new(mode, sys, &self.lib)),
                    None => Screen::Blank,
                };
            }
            Transition::Versus(setup, sides) => {
                self.screen = match self.system.as_ref() {
                    Some(sys) => Screen::Versus(VersusScreen::new(setup, sides, sys)),
                    None => {
                        self.transition(Transition::Fight(setup));
                        return;
                    }
                };
            }
            Transition::Fight(setup) => match Match::load(&setup, &self.data_dir, &self.config) {
                Ok(mut m) => {
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
            },
        }
    }

    fn tick(&mut self) {
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
            Screen::Select(s) => {
                if let Some(sys) = self.system.as_ref() {
                    next = s.tick(pressed, sys, &mut |snd| sfx_queue.push(snd));
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
        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = &mut gpu.renderer;
        r.begin();
        let (cam, clear) = match (&mut self.screen, self.system.as_mut()) {
            (Screen::Title(t), Some(sys)) => t.render(r, sys),
            (Screen::Select(s), Some(sys)) => s.render(r, sys),
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
        if pressed && key == &Key::Named(NamedKey::Escape) {
            match self.screen {
                Screen::Title(_) | Screen::Blank => event_loop.exit(),
                Screen::Select(_) => {
                    let snd = self.system.as_ref().and_then(|s| s.select.cancel_snd);
                    self.system_sfx(snd);
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
