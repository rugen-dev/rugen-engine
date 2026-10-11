//! `rugen play <char> [char2] [--stage s]`: local match, keyboard P1.
//!
//! Keys: arrows move, Z X C = a b c, A S D = x y z, Enter = start,
//! Space pause, `.` step, B boxes, F1 debug, +/- zoom, Esc quit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use rugen_audio::{Audio, SoundKey};
use rugen_formats::sff::Pixels;
use rugen_render::{Camera, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::explod::AnimSrc;
use rugen_sim::world::Event;
use rugen_sim::world::Stage;
use rugen_sim::{Buttons, CharData, FxData, World};
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
    data_dir: &Path,
) -> anyhow::Result<()> {
    let (stage, visual) = match stage {
        Some(p) => {
            let def = rugen_formats::def::Def::open(p).with_context(|| p.display().to_string())?;
            let visual = match crate::stage::StageVisual::load(p) {
                Ok(v) => Some(v),
                Err(e) => {
                    log::warn!("stage background not loaded: {e:#}");
                    None
                }
            };
            (Stage::from_def(&def), visual)
        }
        None => (Stage::default(), None),
    };
    let d1 = CharData::load_with_data(p1, data_dir).with_context(|| p1.display().to_string())?;
    let d2 = match p2 {
        Some(p) => {
            CharData::load_with_data(p, data_dir).with_context(|| p.display().to_string())?
        }
        None => d1.clone(),
    };
    for d in [&d1, &d2] {
        for w in &d.warnings {
            log::warn!("{}: {w}", d.name);
        }
    }
    let mut world = World::new(stage, 1);
    match FxData::load(data_dir) {
        Ok(fx) => {
            world.round = rugen_sim::RoundState::new(fx.round.clone());
            world.fx = Some(fx);
        }
        Err(e) => log::warn!("fightfx not loaded from {}: {e}", data_dir.display()),
    }
    world.add_player(d1, 0, 1);
    world.add_player(d2, 1, 2);
    world.start_round();
    log::info!("{} vs {}", world.data[0].name, world.data[1].name);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(world);
    app.stage_visual = visual;
    app.shot = shot;
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// GPU texture for a sprite plus the SFF palette index it was authored with.
type Loaded = (TextureId, Option<usize>);

struct CharGpu {
    textures: HashMap<(u16, u16), Option<Loaded>>,
    palettes: Vec<PaletteId>,
}

struct Gpu {
    // declared before `window` so the surface is dropped first
    renderer: Renderer,
    window: Arc<Window>,
    /// One per `world.data`, plus one for fightfx at the end.
    chars: Vec<CharGpu>,
}

/// One sprite to draw this frame, in stage coordinates.
struct Item {
    spr: Option<(TextureId, Option<PaletteId>, [f32; 2])>,
    pos: [f32; 2],
    facing: f32,
    vfacing: f32,
    scale: [f32; 2],
    frame: rugen_formats::air::Frame,
    boxes: bool,
    layer: i32,
    prio: i32,
    seq: usize,
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
    /// Screenshot after a scripted sequence, then quit.
    shot: Option<PathBuf>,
    quit: bool,
    audio: Option<Audio>,
    /// Announcement text and ticks left to show it.
    banner: Option<(String, i32)>,
    stage_visual: Option<crate::stage::StageVisual>,
    cam_y: f32,
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
            audio: match Audio::new() {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("audio disabled: {e}");
                    None
                }
            },
            banner: None,
            stage_visual: None,
            cam_y: 0.0,
        }
    }

    /// Play one sound event through the audio crate.
    #[allow(clippy::too_many_arguments)]
    fn play_sound(
        &mut self,
        player: usize,
        own: bool,
        group: i32,
        number: i32,
        channel: i32,
        volume: i32,
        announcer: bool,
    ) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let (source, bytes) = if announcer {
            (
                u32::MAX - 1,
                self.world
                    .fx
                    .as_ref()
                    .and_then(|f| f.fight_snd.as_ref())
                    .and_then(|s| s.get(group, number)),
            )
        } else if own {
            let d = self.world.players[player].data;
            (
                d as u32,
                self.world.data[d]
                    .snd
                    .as_ref()
                    .and_then(|s| s.get(group, number)),
            )
        } else {
            (
                u32::MAX,
                self.world
                    .fx
                    .as_ref()
                    .and_then(|f| f.snd.as_ref())
                    .and_then(|s| s.get(group, number)),
            )
        };
        let key = SoundKey {
            source,
            group,
            number,
        };
        if !audio.is_loaded(key) {
            match bytes {
                Some(b) => {
                    audio.load(key, b);
                }
                None => return,
            }
        }
        let root = self.world.players[player].root;
        let pan = ((self.world.players[player].pos[0] - self.world.cam_x)
            / (self.world.stage.width / 2.0))
            .clamp(-1.0, 1.0)
            * 0.5;
        audio.play(key, root, channel, volume, pan, false);
    }

    fn handle_events(&mut self) {
        let events: Vec<Event> = self.world.events.clone();
        for e in events {
            match e {
                Event::Sound {
                    player,
                    own,
                    group,
                    number,
                    channel,
                    volume,
                } => self.play_sound(player, own, group, number, channel, volume, false),
                Event::RoundStart(n) => {
                    self.banner = Some((format!("ROUND {n}"), 90));
                    self.play_sound(0, false, 0, n, -1, 100, true);
                }
                Event::Fight => {
                    self.banner = Some(("FIGHT".into(), 45));
                    self.play_sound(0, false, 1, 0, -1, 100, true);
                }
                Event::KO => {
                    self.banner = Some(("K.O.".into(), 90));
                    self.play_sound(0, false, 2, 0, -1, 100, true);
                }
                Event::TimeOver => {
                    self.banner = Some(("TIME OVER".into(), 90));
                    self.play_sound(0, false, 2, 2, -1, 100, true);
                }
                Event::RoundWon(_) => {}
                Event::MatchOver(t) => {
                    let name = if t >= 0 {
                        self.world.data[self.world.players[t as usize].data]
                            .display_name
                            .clone()
                    } else {
                        "DRAW".into()
                    };
                    self.banner = Some((format!("{name} WINS"), 100_000));
                }
                Event::Spark { .. } => {}
            }
        }
        if let Some(a) = self.audio.as_mut() {
            a.gc();
        }
    }

    fn set_title(&self) {
        let Some(g) = &self.gpu else { return };
        let p = &self.world.players[0];
        let d = &self.world.data[p.data];
        let anim = self.world.data[p.anim_owner].air.get(p.anim.action);
        g.window.set_title(&format!(
            "rugen — {} — state {} t={} anim {} elem {} animtime {} pos ({:.1},{:.1}) vel ({:.2},{:.2}) ctrl {} life {}/{}{}",
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
            p.life,
            self.world.players[1].life,
            if self.paused { " [paused]" } else { "" }
        ));
    }

    /// GPU sprite from a data slot (`None` = fightfx).
    fn sprite_from(
        &mut self,
        src: Option<usize>,
        group: i32,
        number: i32,
    ) -> Option<(TextureId, Option<PaletteId>, [f32; 2])> {
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let gpu = self.gpu.as_mut()?;
        let (sff, slot) = match src {
            Some(d) => (&self.world.data[d].sff, d),
            None => (&self.world.fx.as_ref()?.sff, self.world.data.len()),
        };
        let cg = gpu.chars.get_mut(slot)?;
        let spr = sff.get(key.0, key.1)?;
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

    fn collect_items(&mut self) -> Vec<Item> {
        let stage = self.world.stage.clone();
        let mut items: Vec<Item> = Vec::new();
        for i in 0..self.world.players.len() {
            let p = &self.world.players[i];
            if p.removed {
                continue;
            }
            let d = &self.world.data[p.anim_owner];
            let scale = stage.width / d.localcoord[0];
            let Some(frame) = d
                .air
                .get(p.anim.action)
                .and_then(|a| p.anim.frame(a))
                .cloned()
            else {
                continue;
            };
            let sprite_data = p.data;
            let (pos, facing, prio) = (
                [p.pos[0] * scale, p.pos[1] * scale],
                p.facing,
                p.spr_priority,
            );
            let spr = self.sprite_from(Some(sprite_data), frame.group, frame.number);
            items.push(Item {
                spr,
                pos,
                facing,
                vfacing: 1.0,
                scale: [scale, scale],
                frame,
                boxes: true,
                layer: 0,
                prio,
                seq: items.len(),
            });
        }
        let explods: Vec<rugen_sim::explod::Explod> = self
            .world
            .explods
            .iter()
            .filter(|e| !e.removed)
            .cloned()
            .collect();
        for e in explods {
            let (src, lc) = match e.src {
                AnimSrc::Player(d) => (Some(d), self.world.data[d].localcoord[0]),
                AnimSrc::Fx => (
                    None,
                    self.world.fx.as_ref().map_or(320.0, |f| f.localcoord[0]),
                ),
            };
            let Some(frame) = e
                .air(&self.world)
                .and_then(|a| a.get(e.anim.action))
                .and_then(|a| e.anim.frame(a))
                .cloned()
            else {
                continue;
            };
            let spr = self.sprite_from(src, frame.group, frame.number);
            let scale = stage.width / lc;
            items.push(Item {
                spr,
                pos: e.draw_pos(),
                facing: e.facing,
                vfacing: e.vfacing,
                scale: [scale * e.scale[0], scale * e.scale[1]],
                frame,
                boxes: false,
                layer: if e.ontop {
                    1
                } else if e.under {
                    -1
                } else {
                    0
                },
                prio: e.sprpriority,
                seq: items.len(),
            });
        }
        let projs: Vec<rugen_sim::projectile::Projectile> = self
            .world
            .projs
            .iter()
            .filter(|p| !p.removed && p.anim_no >= 0)
            .cloned()
            .collect();
        for pr in projs {
            let d = self.world.players[pr.owner].data;
            let lc = self.world.data[d].localcoord[0];
            let Some(frame) = self.world.data[d]
                .air
                .get(pr.anim_no)
                .and_then(|a| pr.anim.frame(a))
                .cloned()
            else {
                continue;
            };
            let spr = self.sprite_from(Some(d), frame.group, frame.number);
            let scale = stage.width / lc;
            items.push(Item {
                spr,
                pos: pr.pos,
                facing: pr.facing,
                vfacing: 1.0,
                scale: [scale * pr.scale[0], scale * pr.scale[1]],
                frame,
                boxes: true,
                layer: 0,
                prio: pr.sprpriority,
                seq: items.len(),
            });
        }
        items.sort_by_key(|it| (it.layer, it.prio, it.seq));
        items
    }

    fn render(&mut self) {
        let stage = self.world.stage.clone();
        let ground = stage.zoffset;
        let items = self.collect_items();
        let show_boxes = self.show_boxes;
        let brightness = self.world.superbrightness;
        let cam_x = self.world.cam_x;

        let cam_y = self.cam_y;
        let mut stage_visual = self.stage_visual.take();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = &mut gpu.renderer;
        r.begin();
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let screen_w = stage.height * aspect;
        match stage_visual.as_mut() {
            Some(sv) => sv.draw(
                r,
                0,
                [cam_x, cam_y],
                stage.width / sv.localcoord[0],
                screen_w,
            ),
            None => r.rect(
                [-4000.0, ground],
                [4000.0, ground + 1.0],
                [0.35, 0.35, 0.35, 1.0],
            ),
        }
        for it in &items {
            let frame = &it.frame;
            let origin = [it.pos[0], ground + it.pos[1] - cam_y];
            if let Some((tex, pal, axis)) = it.spr {
                let mut d = Draw::new(
                    tex,
                    pal,
                    [
                        origin[0] + frame.x as f32 * it.scale[0] * it.facing,
                        origin[1] + frame.y as f32 * it.scale[1] * it.vfacing,
                    ],
                    axis,
                );
                d.hflip = (it.facing < 0.0) != frame.hflip;
                d.vflip = (it.vfacing < 0.0) != frame.vflip;
                d.scale = [frame.xscale * it.scale[0], frame.yscale * it.scale[1]];
                r.draw(d);
            }
            if show_boxes && it.boxes {
                for (boxes, color) in [
                    (&frame.clsn2, [0.2, 0.4, 1.0, 0.9]),
                    (&frame.clsn1, [1.0, 0.2, 0.2, 0.9]),
                ] {
                    for b in boxes {
                        let (x1, x2) = (
                            b[0] as f32 * it.scale[0] * it.facing,
                            b[2] as f32 * it.scale[0] * it.facing,
                        );
                        let min = [
                            origin[0] + x1.min(x2),
                            origin[1] + b[1] as f32 * it.scale[1],
                        ];
                        let max = [
                            origin[0] + x1.max(x2),
                            origin[1] + b[3] as f32 * it.scale[1],
                        ];
                        r.rect_outline(min, max, 1.0, color);
                        r.rect(min, max, [color[0], color[1], color[2], 0.15]);
                    }
                }
            }
        }
        if let Some(sv) = stage_visual.as_mut() {
            sv.draw(
                r,
                1,
                [cam_x, cam_y],
                stage.width / sv.localcoord[0],
                screen_w,
            );
        }
        if brightness < 1.0 {
            r.rect(
                [-4000.0, -4000.0],
                [4000.0, 4000.0],
                [0.0, 0.0, 0.0, 1.0 - brightness],
            );
        }
        // life bars
        let left = cam_x - screen_w / 2.0;
        for (i, p) in self
            .world
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0)
        {
            let frac = (p.life.max(0) as f32 / p.life_max.max(1) as f32).clamp(0.0, 1.0);
            let bar_w = screen_w * 0.4;
            let (x0, x1) = if i == 0 {
                (left + 10.0, left + 10.0 + bar_w)
            } else {
                (left + screen_w - 10.0 - bar_w, left + screen_w - 10.0)
            };
            r.rect([x0, 8.0], [x1, 16.0], [0.1, 0.1, 0.1, 0.9]);
            let (fx0, fx1) = if i == 0 {
                (x1 - bar_w * frac, x1)
            } else {
                (x0, x0 + bar_w * frac)
            };
            r.rect([fx0, 8.0], [fx1, 16.0], [0.9, 0.8, 0.2, 1.0]);
            let pf = (p.power as f32 / p.power_max.max(1) as f32).clamp(0.0, 1.0);
            let (px0, px1) = if i == 0 {
                (x0, x0 + bar_w * pf)
            } else {
                (x1 - bar_w * pf, x1)
            };
            r.rect(
                [x0, stage.height - 14.0],
                [x1, stage.height - 10.0],
                [0.1, 0.1, 0.1, 0.9],
            );
            r.rect(
                [px0, stage.height - 14.0],
                [px1, stage.height - 10.0],
                [0.3, 0.6, 1.0, 1.0],
            );
        }
        // round clock, win marks, announcements
        let clock = self.world.round.clock();
        if clock >= 0 {
            draw_number(r, clock, [cam_x, 8.0], 2.0, [1.0, 1.0, 1.0, 1.0]);
        }
        for (i, wins) in self.world.round.wins.iter().enumerate() {
            for k in 0..*wins {
                let x = if i == 0 {
                    left + 10.0 + k as f32 * 8.0
                } else {
                    left + screen_w - 15.0 - k as f32 * 8.0
                };
                r.rect([x, 20.0], [x + 5.0, 25.0], [1.0, 0.9, 0.3, 1.0]);
            }
        }
        if let Some((text, _)) = &self.banner {
            draw_text(
                r,
                text,
                [cam_x, stage.height * 0.35],
                3.0,
                [1.0, 1.0, 1.0, 1.0],
            );
        }
        let cam = Camera {
            screen: [screen_w, stage.height],
            center: [cam_x, stage.height / 2.0],
            zoom: self.zoom,
        };
        self.stage_visual = stage_visual;
        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = &mut gpu.renderer;
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
        let mut sffs: Vec<&rugen_formats::sff::Sff> =
            self.world.data.iter().map(|d| &d.sff).collect();
        if let Some(fx) = &self.world.fx {
            sffs.push(&fx.sff);
        }
        let chars = sffs
            .iter()
            .map(|sff| {
                let mut palettes: Vec<PaletteId> = sff
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
        if let Some(sv) = self.stage_visual.as_mut() {
            sv.init_gpu(&mut renderer);
        }
        self.gpu = Some(Gpu {
            renderer,
            window,
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
                // scripted input for screenshots: walk forward, then punch, capture during the spark
                if self.shot.is_some() {
                    let mut guard = 0;
                    while self.world.round.round_state() != 2 && guard < 2000 {
                        self.world.step(&[Buttons::default(), Buttons::default()]);
                        guard += 1;
                    }
                    for t in 0..66 {
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
                self.handle_events();
                if let Some(sv) = self.stage_visual.as_mut() {
                    sv.step();
                    // vertical follow: track the highest root player
                    let highest = self
                        .world
                        .players
                        .iter()
                        .filter(|p| !p.removed && p.helper_index == 0)
                        .map(|p| p.pos[1])
                        .fold(0.0f32, f32::min);
                    let target = ((highest + sv.floor_tension).min(0.0) * sv.vertical_follow)
                        .max(sv.bound_high);
                    self.cam_y += (target - self.cam_y) * 0.2;
                }
                if let Some((_, t)) = self.banner.as_mut() {
                    *t -= 1;
                }
                if self.banner.as_ref().is_some_and(|(_, t)| *t <= 0) {
                    self.banner = None;
                }
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

/// 3x5 pixel font, enough for the HUD.
fn glyph(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b110, 0b101, 0b101, 0b101, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        _ => [0; 5],
    }
}

/// Draw centered text with the pixel font; `px` = pixel size in logical units.
fn draw_text(r: &mut Renderer, text: &str, center: [f32; 2], px: f32, color: [f32; 4]) {
    let w = text.chars().count() as f32 * 4.0 * px;
    let mut x = center[0] - w / 2.0;
    for c in text.chars() {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    let x0 = x + col as f32 * px;
                    let y0 = center[1] + row as f32 * px;
                    r.rect([x0, y0], [x0 + px, y0 + px], color);
                }
            }
        }
        x += 4.0 * px;
    }
}

fn draw_number(r: &mut Renderer, n: i32, center: [f32; 2], px: f32, color: [f32; 4]) {
    draw_text(r, &format!("{n:02}"), center, px, color);
}
