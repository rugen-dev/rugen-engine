//! `rugen play <p1.def> [p2.def] [--stage stage.def]`: local match, keyboard P1.
//!
//! Keys: arrows move, Z X C = a b c, A S D = x y z, Enter = start,
//! Space pause, `.` step, B boxes, F1 debug, +/- zoom, Esc quit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use rugen_formats::sff::Pixels;
use rugen_render::{Camera, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::world::Stage;
use rugen_sim::{Buttons, CharData, World};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);

pub fn run(
    p1: &Path,
    p2: Option<&Path>,
    stage: Option<&Path>,
    shot: Option<PathBuf>,
) -> anyhow::Result<()> {
    let stage = match stage {
        Some(p) => Stage::from_def(
            &rugen_formats::def::Def::open(p).with_context(|| p.display().to_string())?,
        ),
        None => Stage::default(),
    };
    let d1 = CharData::load(p1).with_context(|| p1.display().to_string())?;
    let d2 = match p2 {
        Some(p) => CharData::load(p).with_context(|| p.display().to_string())?,
        None => d1.clone(),
    };
    for d in [&d1, &d2] {
        for w in &d.warnings {
            log::warn!("{}: {w}", d.name);
        }
    }
    let mut world = World::new(stage, 1);
    world.add_player(d1, 0, 1);
    world.add_player(d2, 1, 2);
    log::info!("{} vs {}", world.data[0].name, world.data[1].name);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(world);
    app.shot = shot;
    event_loop.run_app(&mut app)?;
    Ok(())
}

type Loaded = (TextureId, Option<usize>);

struct CharGpu {
    textures: HashMap<(u16, u16), Option<Loaded>>,
    palettes: Vec<PaletteId>,
}

struct Gpu {
    window: Arc<Window>,
    renderer: Renderer,
    chars: Vec<CharGpu>,
}

struct App {
    world: World,
    gpu: Option<Gpu>,
    held: Buttons,
    tick_acc: Duration,
    last: Instant,
    paused: bool,
    step: bool,
    zoom: f32,
    show_boxes: bool,
    debug: bool,
    /// Screenshot after a scripted 90 ticks, then quit.
    shot: Option<PathBuf>,
    quit: bool,
}

impl App {
    fn new(world: World) -> Self {
        Self {
            world,
            gpu: None,
            held: Buttons::default(),
            tick_acc: Duration::ZERO,
            last: Instant::now(),
            paused: false,
            step: false,
            zoom: 1.0,
            show_boxes: false,
            debug: true,
            shot: None,
            quit: false,
        }
    }

    fn set_title(&self) {
        let Some(g) = &self.gpu else { return };
        let p = &self.world.players[0];
        let d = &self.world.data[p.data];
        let anim = d.air.get(p.anim.action);
        g.window.set_title(&format!(
            "rugen — {} — state {} t={} anim {} elem {} animtime {} pos ({:.1},{:.1}) vel ({:.2},{:.2}) ctrl {}{}",
            d.name,
            p.state_no,
            p.state_time,
            p.anim.action,
            p.anim.elem + 1,
            anim.map_or(0, |a| p.anim.anim_time(a)),
            p.pos[0],
            p.pos[1],
            p.vel[0],
            p.vel[1],
            p.ctrl as u8,
            if self.paused { " [paused]" } else { "" }
        ));
    }

    fn sprite(
        &mut self,
        pi: usize,
        group: i32,
        number: i32,
    ) -> Option<(TextureId, Option<PaletteId>, [f32; 2])> {
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let gpu = self.gpu.as_mut()?;
        let data = &self.world.data[self.world.players[pi].data];
        let cg = &mut gpu.chars[self.world.players[pi].data];
        let spr = data.sff.get(key.0, key.1)?;
        let entry = cg.textures.entry(key).or_insert_with(|| {
            let (w, h) = (spr.width as u32, spr.height as u32);
            match &spr.pixels {
                Pixels::Indexed(px) => Some((gpu.renderer.add_indexed(w, h, px), spr.palette)),
                Pixels::Rgba(px) => Some((gpu.renderer.add_rgba(w, h, px), None)),
            }
        });
        let (tex, pal) = (*entry)?;
        let pal = pal.map(|p| cg.palettes[p.min(cg.palettes.len() - 1)]);
        Some((tex, pal, [spr.offset[0] as f32, spr.offset[1] as f32]))
    }

    fn render(&mut self) {
        let stage = self.world.stage.clone();
        let ground = stage.zoffset;
        let mut order: Vec<usize> = (0..self.world.players.len()).collect();
        order.sort_by_key(|&i| self.world.players[i].spr_priority);

        struct Item {
            spr: Option<(TextureId, Option<PaletteId>, [f32; 2])>,
            pos: [f32; 2],
            facing: f32,
            scale: f32,
            frame: Option<rugen_formats::air::Frame>,
        }
        let mut items = Vec::new();
        for &i in &order {
            let p = &self.world.players[i];
            let d = &self.world.data[p.data];
            let scale = stage.width / d.localcoord[0];
            let frame = d
                .air
                .get(p.anim.action)
                .and_then(|a| p.anim.frame(a))
                .cloned();
            let (fg, fn_) = frame.as_ref().map_or((-1, 0), |f| (f.group, f.number));
            let spr = if fg >= 0 {
                self.sprite(i, fg, fn_)
            } else {
                None
            };
            let p = &self.world.players[i];
            items.push(Item {
                spr,
                pos: [p.pos[0] * scale, p.pos[1] * scale],
                facing: p.facing,
                scale,
                frame,
            });
        }

        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = &mut gpu.renderer;
        r.begin();
        // floor
        r.rect(
            [-2000.0, ground],
            [2000.0, ground + 1.0],
            [0.35, 0.35, 0.35, 1.0],
        );
        for it in &items {
            let Some(frame) = &it.frame else { continue };
            let origin = [it.pos[0], ground + it.pos[1]];
            if let Some((tex, pal, axis)) = it.spr {
                let mut d = Draw::new(
                    tex,
                    pal,
                    [
                        origin[0] + frame.x as f32 * it.scale * it.facing,
                        origin[1] + frame.y as f32 * it.scale,
                    ],
                    axis,
                );
                d.hflip = (it.facing < 0.0) != frame.hflip;
                d.vflip = frame.vflip;
                d.scale = [frame.xscale * it.scale, frame.yscale * it.scale];
                r.draw(d);
            }
            if self.show_boxes {
                for (boxes, color) in [
                    (&frame.clsn2, [0.2, 0.4, 1.0, 0.9]),
                    (&frame.clsn1, [1.0, 0.2, 0.2, 0.9]),
                ] {
                    for b in boxes {
                        let (x1, x2) = (
                            b[0] as f32 * it.scale * it.facing,
                            b[2] as f32 * it.scale * it.facing,
                        );
                        let min = [origin[0] + x1.min(x2), origin[1] + b[1] as f32 * it.scale];
                        let max = [origin[0] + x1.max(x2), origin[1] + b[3] as f32 * it.scale];
                        r.rect_outline(min, max, 1.0, color);
                        r.rect(min, max, [color[0], color[1], color[2], 0.15]);
                    }
                }
            }
        }
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let cam = Camera {
            screen: [stage.height * aspect, stage.height],
            center: [self.world.cam_x, stage.height / 2.0],
            zoom: self.zoom,
        };
        if let Some(path) = self.shot.take() {
            let rgba = r.end_to_image(&cam, [0.08, 0.08, 0.1, 1.0], w, h);
            match write_png(&path, w, h, &rgba) {
                Ok(()) => log::info!("wrote {}", path.display()),
                Err(e) => log::error!("--shot {}: {e}", path.display()),
            }
            self.quit = true;
            return;
        }
        match r.end(&cam, [0.08, 0.08, 0.1, 1.0]) {
            Ok(()) => {}
            Err(rugen_render::SurfaceError::Lost | rugen_render::SurfaceError::Outdated) => {
                let s = gpu.window.inner_size();
                gpu.renderer.resize(s.width, s.height);
            }
            Err(e) => log::warn!("render: {e}"),
        }
    }

    fn key(&mut self, key: &Key, pressed: bool, event_loop: &ActiveEventLoop) {
        let h = &mut self.held;
        match key.as_ref() {
            Key::Named(NamedKey::ArrowUp) => h.u = pressed,
            Key::Named(NamedKey::ArrowDown) => h.d = pressed,
            Key::Named(NamedKey::ArrowLeft) => h.l = pressed,
            Key::Named(NamedKey::ArrowRight) => h.r = pressed,
            Key::Character("z") | Key::Character("Z") => h.a = pressed,
            Key::Character("x") | Key::Character("X") => h.b = pressed,
            Key::Character("c") | Key::Character("C") => h.c = pressed,
            Key::Character("a") | Key::Character("A") => h.x = pressed,
            Key::Character("s") | Key::Character("S") => h.y = pressed,
            Key::Character("d") | Key::Character("D") => h.z = pressed,
            Key::Named(NamedKey::Enter) => h.s = pressed,
            _ if !pressed => {}
            Key::Named(NamedKey::Escape) => event_loop.exit(),
            Key::Named(NamedKey::Space) => self.paused = !self.paused,
            Key::Character(".") => self.step = true,
            Key::Character("b") | Key::Character("B") => self.show_boxes = !self.show_boxes,
            Key::Named(NamedKey::F1) => self.debug = !self.debug,
            Key::Character("+") | Key::Character("=") => self.zoom = (self.zoom * 1.25).min(8.0),
            Key::Character("-") => self.zoom = (self.zoom / 1.25).max(0.25),
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("rugen")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let size = window.inner_size();
        let mut renderer =
            pollster::block_on(Renderer::new(window.clone(), size.width, size.height))
                .expect("renderer");
        let chars = self
            .world
            .data
            .iter()
            .map(|d| {
                let mut palettes: Vec<PaletteId> = d
                    .sff
                    .palettes
                    .iter()
                    .map(|p| renderer.add_palette(p))
                    .collect();
                if palettes.is_empty() {
                    palettes.push(renderer.add_palette(&[[0; 4]; 256]));
                }
                CharGpu {
                    textures: HashMap::new(),
                    palettes,
                }
            })
            .collect();
        self.gpu = Some(Gpu {
            window,
            renderer,
            chars,
        });
        self.last = Instant::now();
        self.set_title();
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
                // scripted input for screenshots: walk forward, then punch
                if self.shot.is_some() {
                    for t in 0..90 {
                        let b = if t < 40 {
                            Buttons {
                                r: true,
                                ..Default::default()
                            }
                        } else if t == 60 {
                            Buttons {
                                x: true,
                                ..Default::default()
                            }
                        } else {
                            Buttons::default()
                        };
                        self.world.step(&[b, Buttons::default()]);
                    }
                    self.show_boxes = true;
                }
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        self.tick_acc += now - self.last;
        self.last = now;
        let mut ticked = false;
        while self.tick_acc >= TICK {
            self.tick_acc -= TICK;
            if !self.paused || self.step {
                let inputs = [self.held, Buttons::default()];
                self.world.step(&inputs);
                self.step = false;
                ticked = true;
            }
        }
        if self.tick_acc > TICK * 4 {
            self.tick_acc = Duration::ZERO;
        }
        if ticked && self.debug {
            self.set_title();
            for w in self.world.warnings.drain(..) {
                log::warn!("{w}");
            }
        }
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
