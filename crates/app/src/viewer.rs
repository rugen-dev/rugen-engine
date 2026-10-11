//! Sprite / animation viewer. Loads a char `.def` (sff + air) or a bare `.sff`.
//!
//! Keys: ←/→ action, ↑/↓ palette, Space pause, `.` step, +/- zoom, B boxes, R reset, Esc quit.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use rugen_formats::air::{Action, Air};
use rugen_formats::def::Def;
use rugen_formats::sff::{Pixels, Sff};
use rugen_render::{Camera, Draw, PaletteId, Renderer, TextureId};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);

pub fn run(
    path: &Path,
    action: Option<i32>,
    shot: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let content = Content::load(path)?;
    log::info!(
        "{}: {} sprites, {} palettes, {} actions, localcoord {:?}",
        path.display(),
        content.sff.sprites.len(),
        content.sff.palettes.len(),
        content.air.actions.len(),
        content.localcoord
    );
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(content, action);
    app.shot = shot;
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct Content {
    sff: Sff,
    air: Air,
    localcoord: [f32; 2],
    title: String,
}

impl Content {
    fn load(path: &Path) -> anyhow::Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "sff" {
            let sff = Sff::open(path, true).with_context(|| path.display().to_string())?;
            let mut air = Air::default();
            // one action per sprite so ←/→ browses the file
            for (i, (key, _)) in sff.sprites.iter().enumerate() {
                let mut a = Action::default();
                a.frames.push(rugen_formats::air::Frame {
                    group: key.0 as i32,
                    number: key.1 as i32,
                    time: -1,
                    ..Default::default()
                });
                a.total_time = -1;
                air.actions.insert(i as i32, a);
            }
            return Ok(Self {
                sff,
                air,
                localcoord: [320.0, 240.0],
                title: path.display().to_string(),
            });
        }
        let def = Def::open(path).with_context(|| path.display().to_string())?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let files = def.section("files").context("no [Files] section")?;
        let sprite = files.raw("sprite").context("no sprite= in [Files]")?;
        let anim = files.raw("anim").context("no anim= in [Files]")?;
        let sff = Sff::open(resolve(dir, sprite)?, true).with_context(|| sprite.to_string())?;
        let air = Air::open(resolve(dir, anim)?).with_context(|| anim.to_string())?;
        let info = def.section("info");
        let localcoord = info
            .and_then(|i| i.f32s("localcoord"))
            .filter(|v| v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0)
            .map_or([320.0, 240.0], |v| [v[0], v[1]]);
        let title = info
            .and_then(|i| i.str("name").map(str::to_string))
            .unwrap_or_else(|| path.display().to_string());
        Ok(Self {
            sff,
            air,
            localcoord,
            title,
        })
    }
}

/// Case-insensitive path resolution: MUGEN content mixes case freely and Linux does not.
fn resolve(dir: &Path, name: &str) -> anyhow::Result<std::path::PathBuf> {
    let name = name.replace('\\', "/");
    let direct = dir.join(&name);
    if direct.exists() {
        return Ok(direct);
    }
    let mut cur = dir.to_path_buf();
    for part in name.split('/').filter(|p| !p.is_empty()) {
        let found = std::fs::read_dir(&cur)
            .ok()
            .and_then(|rd| {
                rd.flatten()
                    .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))
            })
            .map(|e| e.path())
            .with_context(|| format!("{} not found in {}", part, cur.display()))?;
        cur = found;
    }
    Ok(cur)
}

/// GPU texture for a sprite plus the SFF palette index it was authored with.
type Loaded = (TextureId, Option<usize>);

struct Gpu {
    window: Arc<Window>,
    renderer: Renderer,
    textures: HashMap<(u16, u16), Option<Loaded>>,
    palettes: Vec<PaletteId>,
}

struct App {
    content: Content,
    gpu: Option<Gpu>,
    actions: Vec<i32>,
    action_ix: usize,
    frame_ix: usize,
    frame_time: i32,
    tick_acc: Duration,
    last: Instant,
    paused: bool,
    step: bool,
    zoom: f32,
    palette_ix: usize,
    show_boxes: bool,
    shot: Option<std::path::PathBuf>,
    quit: bool,
}

impl App {
    fn new(content: Content, action: Option<i32>) -> Self {
        let actions: Vec<i32> = content.air.actions.keys().copied().collect();
        let action_ix = action
            .and_then(|a| actions.iter().position(|&n| n == a))
            .unwrap_or(0);
        Self {
            content,
            gpu: None,
            actions,
            action_ix,
            frame_ix: 0,
            frame_time: 0,
            tick_acc: Duration::ZERO,
            last: Instant::now(),
            paused: false,
            step: false,
            zoom: 1.0,
            palette_ix: 0,
            show_boxes: true,
            shot: None,
            quit: false,
        }
    }

    fn action(&self) -> Option<&Action> {
        self.actions
            .get(self.action_ix)
            .and_then(|n| self.content.air.get(*n))
    }

    fn set_action(&mut self, ix: usize) {
        self.action_ix = ix.min(self.actions.len().saturating_sub(1));
        self.frame_ix = 0;
        self.frame_time = 0;
        self.set_title();
    }

    fn set_title(&self) {
        if let Some(g) = &self.gpu {
            let a = self.actions.get(self.action_ix).copied().unwrap_or(0);
            let (f, n) = self
                .action()
                .map_or((0, 0), |a| (self.frame_ix + 1, a.frames.len()));
            g.window.set_title(&format!(
                "rugen — {} — action {a} frame {f}/{n} pal {}{}",
                self.content.title,
                self.palette_ix + 1,
                if self.paused { " [paused]" } else { "" }
            ));
        }
    }

    fn tick(&mut self) {
        let Some(a) = self.action() else { return };
        if a.frames.is_empty() {
            return;
        }
        let (ftime, nframes, loop_start) =
            (a.frames[self.frame_ix].time, a.frames.len(), a.loop_start);
        self.frame_time += 1;
        if ftime >= 0 && self.frame_time >= ftime {
            self.frame_time = 0;
            self.frame_ix += 1;
            if self.frame_ix >= nframes {
                self.frame_ix = loop_start.min(nframes - 1);
            }
        }
    }

    fn sprite(
        &mut self,
        group: i32,
        number: i32,
    ) -> Option<(TextureId, Option<PaletteId>, [f32; 2])> {
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let gpu = self.gpu.as_mut()?;
        let spr = self.content.sff.get(key.0, key.1)?;
        let entry = gpu.textures.entry(key).or_insert_with(|| {
            let (w, h) = (spr.width as u32, spr.height as u32);
            match &spr.pixels {
                Pixels::Indexed(px) => Some((gpu.renderer.add_indexed(w, h, px), spr.palette)),
                Pixels::Rgba(px) => Some((gpu.renderer.add_rgba(w, h, px), None)),
            }
        });
        let (tex, pal) = (*entry)?;
        // palette ↑/↓ swaps the char's base palette (sprites bound to palette 0 = pal.defaults slot);
        // other palettes (fx) stay as authored
        let pal = pal.map(|p| {
            if p == 0 {
                gpu.palettes[self.palette_ix.min(gpu.palettes.len() - 1)]
            } else {
                gpu.palettes[p]
            }
        });
        Some((tex, pal, [spr.offset[0] as f32, spr.offset[1] as f32]))
    }

    fn render(&mut self) {
        let Some(a) = self.action().cloned() else {
            return;
        };
        let lc = self.content.localcoord;
        let origin = [lc[0] / 2.0, lc[1] * 0.8];
        let Some(frame) = a.frames.get(self.frame_ix).cloned() else {
            return;
        };

        let spr = self.sprite(frame.group, frame.number);
        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = &mut gpu.renderer;
        r.begin();
        // ground line
        r.rect(
            [0.0, origin[1]],
            [lc[0], origin[1] + 1.0],
            [0.3, 0.3, 0.3, 1.0],
        );
        if let Some((tex, pal, axis)) = spr {
            let mut d = Draw::new(
                tex,
                pal,
                [origin[0] + frame.x as f32, origin[1] + frame.y as f32],
                axis,
            );
            d.hflip = frame.hflip;
            d.vflip = frame.vflip;
            d.scale = [frame.xscale, frame.yscale];
            r.draw(d);
        }
        if self.show_boxes {
            for (boxes, color) in [
                (&frame.clsn2, [0.2, 0.4, 1.0, 0.9]),
                (&frame.clsn1, [1.0, 0.2, 0.2, 0.9]),
            ] {
                for b in boxes {
                    let min = [origin[0] + b[0] as f32, origin[1] + b[1] as f32];
                    let max = [origin[0] + b[2] as f32, origin[1] + b[3] as f32];
                    r.rect_outline(min, max, 1.0, color);
                    r.rect(min, max, [color[0], color[1], color[2], 0.15]);
                }
            }
            r.rect(
                [origin[0] - 3.0, origin[1] - 0.5],
                [origin[0] + 3.0, origin[1] + 0.5],
                [1.0, 1.0, 0.0, 1.0],
            );
            r.rect(
                [origin[0] - 0.5, origin[1] - 3.0],
                [origin[0] + 0.5, origin[1] + 3.0],
                [1.0, 1.0, 0.0, 1.0],
            );
        }
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let screen = [lc[1] * aspect, lc[1]];
        let cam = Camera {
            screen,
            center: [lc[0] / 2.0, lc[1] / 2.0],
            zoom: self.zoom,
        };
        if let Some(path) = self.shot.take() {
            let rgba = r.end_to_image(&cam, [0.08, 0.08, 0.1, 1.0], w, h);
            if let Err(e) = write_png(&path, w, h, &rgba) {
                log::error!("--shot {}: {e}", path.display());
            } else {
                log::info!("wrote {}", path.display());
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
        let palettes = self
            .content
            .sff
            .palettes
            .iter()
            .map(|p| renderer.add_palette(p))
            .collect::<Vec<_>>();
        let palettes = if palettes.is_empty() {
            vec![renderer.add_palette(&[[0; 4]; 256])]
        } else {
            palettes
        };
        self.gpu = Some(Gpu {
            window,
            renderer,
            textures: HashMap::new(),
            palettes,
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
                self.render();
                if self.quit {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let n = self.actions.len();
                match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::ArrowRight) => {
                        self.set_action((self.action_ix + 1) % n.max(1))
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.set_action((self.action_ix + n.max(1) - 1) % n.max(1))
                    }
                    Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::ArrowDown) => {
                        let np = self.gpu.as_ref().map_or(1, |g| g.palettes.len()).max(1);
                        self.palette_ix =
                            if matches!(event.logical_key, Key::Named(NamedKey::ArrowUp)) {
                                (self.palette_ix + 1) % np
                            } else {
                                (self.palette_ix + np - 1) % np
                            };
                        self.set_title();
                    }
                    Key::Named(NamedKey::Space) => {
                        self.paused = !self.paused;
                        self.set_title();
                    }
                    Key::Character(".") => self.step = true,
                    Key::Character("+") | Key::Character("=") => {
                        self.zoom = (self.zoom * 1.25).min(16.0)
                    }
                    Key::Character("-") => self.zoom = (self.zoom / 1.25).max(0.25),
                    Key::Character("b") | Key::Character("B") => self.show_boxes = !self.show_boxes,
                    Key::Character("r") | Key::Character("R") => self.set_action(self.action_ix),
                    _ => {}
                }
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
                self.tick();
                self.step = false;
                ticked = true;
            }
        }
        if self.tick_acc > TICK * 4 {
            self.tick_acc = Duration::ZERO; // dropped frames, don't spiral
        }
        if ticked {
            self.set_title();
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
