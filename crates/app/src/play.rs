//! The match screen: runs a [`World`] (locally or through rollback netplay),
//! draws players, explods, projectiles, the stage and the fight.def HUD.
//!
//! Player keys come from `rugen.toml` (see [`crate::config`]); gamepads work
//! out of the box. Debug keys: Space pause, `.` step, F2 boxes, F1 debug,
//! +/- zoom.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rugen_audio::{Audio, SoundKey};
use rugen_formats::air::Blend as AirBlend;
use rugen_formats::sff::Pixels;
use rugen_net::{Netplay, Options as NetOptions};
use rugen_render::{Blend, Camera, Draw, PaletteId, Renderer, TextureId, Tint};
use rugen_sim::explod::{AnimSrc, Trans};
use rugen_sim::fx::PalFxOut;
use rugen_sim::world::Event;
use rugen_sim::world::Stage;
use rugen_sim::{Buttons, CharData, FxData, World};
use winit::keyboard::{Key, NamedKey};

use crate::config::Config;

/// How the match is driven.
#[derive(Clone, Debug)]
pub enum NetMode {
    /// Online: our UDP port, the peer, and which side we play (0 = P1, 1 = P2).
    P2P {
        port: u16,
        remote: SocketAddr,
        side: usize,
    },
    /// Online through a `rugen-server` rendezvous: server, room code, side.
    Rendezvous {
        port: u16,
        server: SocketAddr,
        room: String,
        side: usize,
    },
    /// Local two-player game that rolls back every frame to prove determinism.
    SyncTest,
}

/// Everything needed to start a match.
#[derive(Clone, Debug)]
pub struct FightSetup {
    pub p1: PathBuf,
    pub p2: PathBuf,
    pub stage: Option<PathBuf>,
    /// Computer level per side (0 = human).
    pub ai: [i32; 2],
    pub net: Option<NetMode>,
    /// Guided tutorial for P1 (P2 stands still, no clock).
    pub tutorial: bool,
    /// Cache manifest ids of both characters (for per-character stats); may be empty.
    pub ids: [String; 2],
    /// Game mode the match was started from (for "character select" after the result).
    pub mode: Option<crate::game::Mode>,
}

/// GPU texture for a sprite plus the SFF palette index it was authored with.
type Loaded = (TextureId, Option<usize>);

struct CharGpu {
    textures: HashMap<(u16, u16), Option<Loaded>>,
    palettes: Vec<PaletteId>,
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
    blend: Blend,
    angle: f32,
    tint: Tint,
}

/// Final blend for a sprite: the `Trans` controller wins over the AIR frame.
fn resolve_blend(frame: &rugen_formats::air::Frame, trans: Trans, alpha: [i32; 2]) -> Blend {
    let a = |v: i32, def: u8| if v < 0 { def } else { v.min(255) as u8 };
    match trans {
        Trans::Default => match frame.blend {
            AirBlend::None => Blend::Alpha,
            AirBlend::Add => Blend::Add {
                src: frame.src_alpha,
                dst: frame.dst_alpha,
            },
            AirBlend::Sub | AirBlend::SubAdd => Blend::Sub {
                src: frame.src_alpha,
                dst: frame.dst_alpha,
            },
        },
        Trans::None => Blend::Alpha,
        Trans::Add => Blend::Add { src: 255, dst: 255 },
        Trans::Add1 => Blend::Add { src: 255, dst: 128 },
        Trans::AddAlpha => Blend::Add {
            src: a(alpha[0], 255),
            dst: a(alpha[1], 0),
        },
        Trans::Sub => Blend::Sub { src: 255, dst: 255 },
    }
}

fn tint(p: PalFxOut) -> Tint {
    Tint {
        mul: p.mul,
        add: p.add,
        color: p.color,
        invert: p.invert,
    }
}

pub struct Match {
    pub world: World,
    /// One per `world.data`, plus one for fightfx at the end (filled by `init_gpu`).
    chars: Vec<CharGpu>,
    net: Option<Netplay>,
    net_status: String,
    paused: bool,
    step: bool,
    zoom: f32,
    pub show_boxes: bool,
    debug: bool,
    /// Announcement text and ticks left to show it (fallback when fight.def is missing).
    banner: Option<(String, i32)>,
    stage_visual: Option<crate::stage::StageVisual>,
    lifebar: Option<crate::lifebar::Lifebar>,
    /// Ticks since the match ended.
    over_ticks: i32,
    /// Start pressed after the match ended.
    pub leave: bool,
    /// Inputs recorded this match (local play only).
    pub record: Option<crate::replay::Replay>,
    /// Inputs to replay instead of live input, with a cursor.
    pub playback: Option<(Vec<[u16; 2]>, usize)>,
    /// F3: state / frame data / input history overlay.
    overlay: bool,
    input_history: [Vec<Buttons>; 2],
    setup: FightSetup,
    data_dir: PathBuf,
    /// Hot reload: character dirs and the newest mtime seen there.
    watch: Vec<(usize, PathBuf, u64)>,
    reload_tick: u32,
    /// Data slots whose textures must be re-uploaded (after a reload).
    gpu_dirty: Vec<usize>,
    /// Stage music file, if any.
    pub bgm: Option<PathBuf>,
    pub tutorial: Option<crate::tutorial::Tutorial>,
    prev_start: bool,
    /// Profile stats already written for this match.
    pub stats_recorded: bool,
}

impl Match {
    pub fn load(
        setup: &FightSetup,
        motif: &Path,
        data_dir: &Path,
        cfg: &Config,
    ) -> anyhow::Result<Self> {
        let (stage, visual) = match &setup.stage {
            Some(p) => {
                let def =
                    rugen_formats::def::Def::open(p).with_context(|| p.display().to_string())?;
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
        let mut stage = stage;
        stage.zoom_out = stage.zoom_out.min(cfg.camera.zoom_out.clamp(0.2, 1.0));
        stage.zoom_in = stage.zoom_in.max(cfg.camera.zoom_in.clamp(1.0, 3.0));
        let d1 = CharData::load_with_data(&setup.p1, data_dir)
            .with_context(|| setup.p1.display().to_string())?;
        let d2 = if setup.p2 == setup.p1 {
            d1.clone()
        } else {
            CharData::load_with_data(&setup.p2, data_dir)
                .with_context(|| setup.p2.display().to_string())?
        };
        for d in [&d1, &d2] {
            for w in &d.warnings {
                log::warn!("{}: {w}", d.name);
            }
        }
        let mut world = World::new(stage, 1);
        match FxData::load_with_motif(motif, data_dir) {
            Ok(fx) => {
                let mut round = fx.round.clone();
                // the options' round time; online peers keep the motif's so both sides agree
                if setup.net.is_none() {
                    round.round_time = cfg.game.round_time;
                }
                world.round = rugen_sim::RoundState::new(round);
                world.fx = Some(fx);
            }
            Err(e) => log::warn!("fightfx not loaded from {}: {e}", data_dir.display()),
        }
        world.add_player(d1, 0, 1);
        world.add_player(d2, 1, 2);
        world.movement = rugen_sim::world::Movement {
            air_jumps: world
                .stage
                .arena
                .air_jumps
                .unwrap_or(cfg.game.air_jumps)
                .clamp(0, 8),
            air_dash: world.stage.arena.air_dash.unwrap_or(cfg.game.air_dash),
            fast_fall: world.stage.arena.fast_fall.unwrap_or(cfg.game.fast_fall),
        };
        if setup.net.is_none() {
            world.players[0].ai_level = setup.ai[0].clamp(0, 8);
            world.players[1].ai_level = setup.ai[1].clamp(0, 8);
        }
        world.start_round();
        if setup.tutorial {
            world.round.time_left = -1;
        }
        log::info!("{} vs {}", world.data[0].name, world.data[1].name);

        let net = match &setup.net {
            Some(NetMode::P2P { port, remote, side }) => {
                let o = NetOptions {
                    input_delay: cfg.net.input_delay,
                    max_prediction: cfg.net.max_prediction,
                    ..Default::default()
                };
                let n = Netplay::p2p(*port, *remote, *side, &o)
                    .with_context(|| format!("netplay on port {port} to {remote}"))?;
                log::info!(
                    "netplay: listening on {port}, peer {remote}, playing P{}",
                    side + 1
                );
                Some(n)
            }
            Some(NetMode::Rendezvous {
                port,
                server,
                room,
                side,
            }) => {
                let o = NetOptions {
                    input_delay: cfg.net.input_delay,
                    max_prediction: cfg.net.max_prediction,
                    ..Default::default()
                };
                let mut socket = rugen_net::Socket::bind(*port)?;
                let remote = socket.rendezvous(
                    *server,
                    room,
                    *side,
                    std::time::Duration::from_secs(90),
                    &mut |s| log::info!("netplay: {s}"),
                )?;
                let n = Netplay::p2p_with_socket(socket, remote, *side, &o)
                    .with_context(|| format!("netplay with {remote}"))?;
                Some(n)
            }
            Some(NetMode::SyncTest) => Some(Netplay::synctest(7, &NetOptions::default())?),
            None => None,
        };
        // stage music: paths are relative to the game root (`sound/x.mp3`), else the stage dir
        let bgm = setup.stage.as_ref().and_then(|p| {
            let def = rugen_formats::def::Def::open(p).ok()?;
            let name = def.section("music")?.raw("bgmusic")?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let root = data_dir.parent().unwrap_or(data_dir);
            let stage_dir = p.parent().unwrap_or(Path::new("."));
            [root.join(&name), stage_dir.join(&name)]
                .into_iter()
                .find(|c| c.exists())
        });
        let lifebar = match crate::lifebar::Lifebar::load(motif, data_dir) {
            Ok(lb) => Some(lb),
            Err(e) => {
                log::warn!("fight.def HUD not loaded: {e:#}");
                None
            }
        };
        Ok(Self {
            world,
            chars: Vec::new(),
            net,
            net_status: String::new(),
            paused: false,
            step: false,
            zoom: 1.0,
            show_boxes: false,
            debug: true,
            banner: None,
            stage_visual: visual,
            lifebar,
            over_ticks: 0,
            leave: false,
            record: None,
            playback: None,
            overlay: std::env::var_os("RUGEN_OVERLAY").is_some(),
            input_history: [Vec::new(), Vec::new()],
            watch: [(0usize, &setup.p1), (1, &setup.p2)]
                .into_iter()
                .filter(|(i, _)| *i == 0 || setup.p2 != setup.p1)
                .map(|(i, p)| {
                    let dir = p.parent().unwrap_or(Path::new(".")).to_path_buf();
                    let m = newest_mtime(&dir);
                    (i, dir, m)
                })
                .collect(),
            setup: setup.clone(),
            data_dir: data_dir.to_path_buf(),
            reload_tick: 0,
            gpu_dirty: Vec::new(),
            bgm,
            tutorial: None,
            prev_start: false,
            stats_recorded: false,
        })
    }

    pub fn is_tutorial(&self) -> bool {
        self.tutorial.is_some()
    }

    pub fn setup_ids(&self) -> [String; 2] {
        self.setup.ids.clone()
    }

    /// Re-upload palettes/textures for data slots touched by a hot reload.
    pub fn refresh_gpu(&mut self, r: &mut Renderer) {
        if self.chars.is_empty() {
            self.init_gpu(r);
            return;
        }
        for i in std::mem::take(&mut self.gpu_dirty) {
            let Some(d) = self.world.data.get(i) else {
                continue;
            };
            let mut palettes: Vec<PaletteId> =
                d.sff.palettes.iter().map(|p| r.add_palette(p)).collect();
            if palettes.is_empty() {
                palettes.push(r.add_palette(&[[0; 4]; 256]));
            }
            if let Some(cg) = self.chars.get_mut(i) {
                cg.textures.clear();
                cg.palettes = palettes;
            }
        }
    }

    /// Reload a character whose source files changed on disk (creator workflow).
    fn check_reload(&mut self) {
        self.reload_tick += 1;
        if self.reload_tick % 60 != 0 || self.net.is_some() {
            return;
        }
        for k in 0..self.watch.len() {
            let (slot, dir, seen) = self.watch[k].clone();
            let now = newest_mtime(&dir);
            if now <= seen {
                continue;
            }
            self.watch[k].2 = now;
            let def = if slot == 0 {
                &self.setup.p1
            } else {
                &self.setup.p2
            };
            let def = def.clone();
            match CharData::load_with_data(&def, &self.data_dir) {
                Ok(d) => {
                    for w in &d.warnings {
                        log::warn!("{}: {w}", d.name);
                    }
                    log::info!("reloaded {}", d.name);
                    // both slots share data when the same character fights itself
                    for (i, slot_data) in self.world.data.iter_mut().enumerate() {
                        if i == slot || (self.setup.p1 == self.setup.p2 && i < 2) {
                            *slot_data = d.clone();
                            self.gpu_dirty.push(i);
                        }
                    }
                }
                Err(e) => log::error!("reload failed: {e}"),
            }
        }
    }

    /// Upload palettes once the renderer exists.
    pub fn init_gpu(&mut self, r: &mut Renderer) {
        if !self.chars.is_empty() {
            return;
        }
        let mut sffs: Vec<&rugen_formats::sff::Sff> =
            self.world.data.iter().map(|d| &d.sff).collect();
        if let Some(fx) = &self.world.fx {
            sffs.push(&fx.sff);
        }
        self.chars = sffs
            .iter()
            .map(|sff| {
                let mut palettes: Vec<PaletteId> =
                    sff.palettes.iter().map(|p| r.add_palette(p)).collect();
                if palettes.is_empty() {
                    palettes.push(r.add_palette(&[[0; 4]; 256]));
                }
                CharGpu {
                    textures: HashMap::new(),
                    palettes,
                }
            })
            .collect();
        if let Some(sv) = self.stage_visual.as_mut() {
            sv.init_gpu(r);
        }
    }

    /// Advance one tick with both players' inputs. Returns false when the
    /// frame did not advance (paused, or netplay waiting on the peer).
    pub fn tick(&mut self, inputs: [Buttons; 2], audio: &mut Option<Audio>) -> bool {
        if self.paused && !self.step {
            return false;
        }
        self.step = false;
        self.check_reload();
        let mut inputs = inputs;
        if let Some((frames, cursor)) = self.playback.as_mut() {
            match frames.get(*cursor) {
                Some(f) => {
                    inputs = [Buttons::from_bits(f[0]), Buttons::from_bits(f[1])];
                    *cursor += 1;
                }
                None => self.leave = true,
            }
        }
        if let Some(rec) = self.record.as_mut() {
            rec.inputs.push([inputs[0].bits(), inputs[1].bits()]);
        }
        for (h, b) in self.input_history.iter_mut().zip(inputs) {
            h.push(b);
            if h.len() > 24 {
                h.remove(0);
            }
        }
        let advanced = match self.net.as_mut() {
            Some(net) => {
                net.poll();
                match net.advance(&mut self.world, inputs) {
                    Ok(adv) => {
                        for n in &adv.notices {
                            log::info!("netplay: {n}");
                            self.net_status = n.clone();
                        }
                        let st = net.stats();
                        if st.synchronized && adv.notices.is_empty() {
                            self.net_status = format!(
                                "ping {} ms  rollbacks {}  ahead {}",
                                st.ping_ms, st.rollbacks, st.frames_ahead
                            );
                        }
                        let moved = adv.new_frames > 0;
                        self.handle_events(adv.events, audio);
                        moved
                    }
                    Err(e) => {
                        log::error!("netplay: {e}");
                        self.net_status = format!("netplay error: {e}");
                        self.net = None;
                        false
                    }
                }
            }
            None => {
                // a panicking character is disabled instead of taking the match down
                if let Some(culprit) = self.world.step_checked(&inputs) {
                    log::error!(
                        "P{} crashed the simulation and was disabled; see `rugen verify`",
                        culprit + 1
                    );
                }
                let ev = self.world.events.clone();
                self.handle_events(ev, audio);
                true
            }
        };
        if !advanced {
            return false;
        }
        if let Some(sv) = self.stage_visual.as_mut() {
            sv.step();
        }
        if let Some(lb) = self.lifebar.as_mut() {
            lb.step();
        }
        if let Some((_, t)) = self.banner.as_mut() {
            *t -= 1;
        }
        if self.banner.as_ref().is_some_and(|(_, t)| *t <= 0) {
            self.banner = None;
        }
        // harness: `RUGEN_HARNESS_LOWLIFE=1` makes P2 drop in one hit so matches end fast
        if std::env::var_os("RUGEN_HARNESS_LOWLIFE").is_some()
            && self.world.round.round_state() == 2
            && self.world.players.len() > 1
            && self.world.players[1].life > 1
        {
            self.world.players[1].life = 1;
        }
        if self.world.round.match_over {
            self.over_ticks += 1;
            // linger on the win pose, then hand over to the result screen
            if self.over_ticks > 150 || (self.over_ticks > 30 && (inputs[0].s || inputs[1].s)) {
                self.leave = true;
            }
        }
        if let Some(t) = self.tutorial.as_mut() {
            if self.world.round.time_left >= 0 {
                self.world.round.time_left = -1;
            }
            t.tick(&self.world);
            if inputs[0].s && !self.prev_start {
                if t.finished {
                    self.leave = true;
                } else {
                    t.skip();
                }
            }
        }
        self.prev_start = inputs[0].s;
        if self.debug {
            for w in self.world.warnings.drain(..) {
                log::warn!("{w}");
            }
        }
        true
    }

    pub fn setup(&self) -> &FightSetup {
        &self.setup
    }

    pub fn is_net(&self) -> bool {
        self.net.is_some()
    }

    /// Winning side (0/1) once the match is over, None for a draw or unfinished match.
    pub fn winner(&self) -> Option<usize> {
        let w = self.world.round.wins;
        if !self.world.round.match_over || w[0] == w[1] {
            None
        } else {
            Some(usize::from(w[1] > w[0]))
        }
    }

    /// Debug keys. Returns true when consumed.
    pub fn key(&mut self, key: &Key, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        match key.as_ref() {
            Key::Named(NamedKey::Space) => self.paused = !self.paused,
            Key::Character(".") => self.step = true,
            Key::Named(NamedKey::F2) => self.show_boxes = !self.show_boxes,
            Key::Named(NamedKey::F1) => self.debug = !self.debug,
            Key::Named(NamedKey::F3) => self.overlay = !self.overlay,
            Key::Character("+") | Key::Character("=") => self.zoom = (self.zoom * 1.25).min(8.0),
            Key::Character("-") => self.zoom = (self.zoom / 1.25).max(0.25),
            _ => return false,
        }
        true
    }

    pub fn debug_title(&self) -> Option<String> {
        if !self.debug {
            return None;
        }
        let p = &self.world.players[0];
        let d = &self.world.data[p.data];
        let anim = self.world.data[p.anim_owner].air.get(p.anim.action);
        Some(format!(
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
        ))
    }

    /// Play one sound event through the audio crate.
    #[allow(clippy::too_many_arguments)]
    fn play_sound(
        &mut self,
        audio: &mut Option<Audio>,
        player: usize,
        own: bool,
        group: i32,
        number: i32,
        channel: i32,
        volume: i32,
        announcer: bool,
    ) {
        let Some(audio) = audio.as_mut() else {
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

    pub fn handle_events(&mut self, events: Vec<Event>, audio: &mut Option<Audio>) {
        if let Some(lb) = self.lifebar.as_mut() {
            for e in &events {
                lb.event(&self.world, e);
            }
        }
        for e in events {
            match e {
                Event::Sound {
                    player,
                    own,
                    group,
                    number,
                    channel,
                    volume,
                } => self.play_sound(audio, player, own, group, number, channel, volume, false),
                Event::StopSound { player, channel } => {
                    if let Some(a) = audio.as_mut() {
                        a.stop_channel(player, channel);
                    }
                }
                Event::RoundStart(n) => {
                    self.banner = Some((format!("ROUND {n}"), 90));
                    self.play_sound(audio, 0, false, 0, n, -1, 100, true);
                }
                Event::Fight => {
                    self.banner = Some(("FIGHT".into(), 45));
                    self.play_sound(audio, 0, false, 1, 0, -1, 100, true);
                }
                Event::KO => {
                    self.banner = Some(("K.O.".into(), 90));
                    self.play_sound(audio, 0, false, 2, 0, -1, 100, true);
                }
                Event::TimeOver => {
                    self.banner = Some(("TIME OVER".into(), 90));
                    self.play_sound(audio, 0, false, 2, 2, -1, 100, true);
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
        if let Some(a) = audio.as_mut() {
            a.gc();
        }
    }

    /// GPU sprite from a data slot (`None` = fightfx).
    fn sprite_from(
        &mut self,
        r: &mut Renderer,
        src: Option<usize>,
        group: i32,
        number: i32,
    ) -> Option<(TextureId, Option<PaletteId>, [f32; 2])> {
        let key = (u16::try_from(group).ok()?, u16::try_from(number).ok()?);
        let (sff, slot) = match src {
            Some(d) => (&self.world.data[d].sff, d),
            None => (&self.world.fx.as_ref()?.sff, self.world.data.len()),
        };
        let cg = self.chars.get_mut(slot)?;
        let spr = sff.get(key.0, key.1)?;
        let entry = cg.textures.entry(key).or_insert_with(|| {
            let (w, h) = (spr.width as u32, spr.height as u32);
            match &spr.pixels {
                Pixels::Indexed(px) => Some((r.add_indexed(w, h, px), spr.palette)),
                Pixels::Rgba(px) => Some((r.add_rgba(w, h, px), None)),
            }
        });
        let (tex, pal) = (*entry)?;
        let pal = pal.map(|p| cg.palettes[p.min(cg.palettes.len() - 1)]);
        Some((tex, pal, [spr.offset[0] as f32, spr.offset[1] as f32]))
    }

    fn collect_items(&mut self, r: &mut Renderer) -> Vec<Item> {
        let stage = self.world.stage.clone();
        let all_pfx = self.world.allpalfx.sample();
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
            let blend = resolve_blend(&frame, p.trans, p.alpha);
            let angle = if p.angle_drawn {
                p.angle * p.facing
            } else {
                0.0
            };
            let ascale = p.angle_scale;
            let pfx = tint(p.palfx.sample().stacked(all_pfx));
            // afterimages first so they sit behind the player
            let aimg = p
                .afterimage
                .as_ref()
                .map(|ai| (ai.visible(p.spr_priority), ai.trans, ai.alpha));
            if let Some((visible, ai_trans, ai_alpha)) = aimg {
                for (img, img_pfx, img_prio) in visible {
                    let d = &self.world.data[img.anim_owner];
                    let Some(f) = d
                        .air
                        .get(img.action)
                        .and_then(|a| a.frames.get(img.elem))
                        .cloned()
                    else {
                        continue;
                    };
                    let spr = self.sprite_from(r, Some(img.sprite_data), f.group, f.number);
                    let blend = resolve_blend(&f, ai_trans, ai_alpha);
                    items.push(Item {
                        spr,
                        pos: [img.pos[0] * scale, img.pos[1] * scale],
                        facing: img.facing,
                        vfacing: 1.0,
                        scale: [scale * img.scale[0], scale * img.scale[1]],
                        frame: f,
                        boxes: false,
                        layer: 0,
                        prio: img_prio,
                        seq: items.len(),
                        blend,
                        angle: img.angle * img.facing,
                        tint: tint(img_pfx.stacked(all_pfx)),
                    });
                }
            }
            let spr = self.sprite_from(r, Some(sprite_data), frame.group, frame.number);
            items.push(Item {
                spr,
                pos,
                facing,
                vfacing: 1.0,
                scale: [scale * ascale[0], scale * ascale[1]],
                frame,
                boxes: true,
                layer: 0,
                prio,
                seq: items.len(),
                blend,
                angle,
                tint: pfx,
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
            let spr = self.sprite_from(r, src, frame.group, frame.number);
            let scale = stage.width / lc;
            let blend = resolve_blend(&frame, e.trans, e.alpha);
            let owner_pfx = self
                .world
                .players
                .get(e.owner)
                .map_or(PalFxOut::default(), |p| p.palfx.sample());
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
                blend,
                angle: e.angle * e.facing,
                tint: tint(owner_pfx.stacked(all_pfx)),
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
            let spr = self.sprite_from(r, Some(d), frame.group, frame.number);
            let scale = stage.width / lc;
            let blend = resolve_blend(&frame, Trans::Default, [255, 0]);
            let owner_pfx = self.world.players[pr.owner].palfx.sample();
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
                blend,
                angle: 0.0,
                tint: tint(owner_pfx.stacked(all_pfx)),
            });
        }
        items.sort_by_key(|it| (it.layer, it.prio, it.seq));
        items
    }

    /// F3 overlay: state and frame data per player plus recent inputs.
    fn draw_overlay(
        &self,
        r: &mut Renderer,
        hud: &dyn Fn(f32, f32) -> [f32; 2],
        u: f32,
        screen_w: f32,
    ) {
        let w = &self.world;
        let roots: Vec<usize> = w
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.helper_index == 0)
            .map(|(i, _)| i)
            .take(2)
            .collect();
        for (side, &pi) in roots.iter().enumerate() {
            let p = &w.players[pi];
            let d = &w.data[p.anim_owner];
            let anim = d.air.get(p.anim.action);
            let lines = [
                format!(
                    "P{} ST {} T {} {:?} {:?} CTRL {}",
                    side + 1,
                    p.state_no,
                    p.state_time,
                    p.state_type,
                    p.move_type,
                    p.ctrl as u8
                ),
                format!(
                    "ANIM {} EL {}/{} AT {}",
                    p.anim.action,
                    p.anim.elem + 1,
                    anim.map_or(0, |a| a.frames.len()),
                    anim.map_or(0, |a| p.anim.anim_time(a))
                ),
                format!(
                    "POS {:.1} {:.1} VEL {:.2} {:.2}",
                    p.pos[0], p.pos[1], p.vel[0], p.vel[1]
                ),
                format!(
                    "LIFE {} POW {} HIT {} JUG {}",
                    p.life, p.power, p.hit_count, p.juggle
                ),
            ];
            let x = if side == 0 { 6.0 } else { screen_w - 6.0 };
            for (k, line) in lines.iter().enumerate() {
                let y = 40.0 + k as f32 * 7.0;
                let wpx = line.len() as f32 * 4.0;
                let cx = if side == 0 {
                    x + wpx / 2.0
                } else {
                    x - wpx / 2.0
                };
                draw_text(r, line, hud(cx, y), u, [1.0, 1.0, 0.6, 0.95]);
            }
            // input history, newest at the bottom
            let hist = &self.input_history[side];
            for (k, b) in hist.iter().rev().take(12).enumerate() {
                let mut s = String::new();
                for (on, ch) in [
                    (b.u, 'U'),
                    (b.d, 'D'),
                    (b.l, 'L'),
                    (b.r, 'R'),
                    (b.a, 'A'),
                    (b.b, 'B'),
                    (b.c, 'C'),
                    (b.x, 'X'),
                    (b.y, 'Y'),
                    (b.z, 'Z'),
                    (b.s, 'S'),
                ] {
                    s.push(if on { ch } else { '.' });
                }
                let y = 160.0 - k as f32 * 6.0;
                let wpx = s.len() as f32 * 4.0;
                let cx = if side == 0 {
                    x + wpx / 2.0
                } else {
                    x - wpx / 2.0
                };
                let alpha = 1.0 - k as f32 * 0.07;
                draw_text(r, &s, hud(cx, y), u, [0.8, 0.9, 1.0, alpha]);
            }
        }
    }

    /// Queue this frame's draws; the shell calls `begin` before and `end` after.
    pub fn render(&mut self, r: &mut Renderer) -> (Camera, [f64; 4]) {
        let stage = self.world.stage.clone();
        let ground = stage.zoffset;
        let items = self.collect_items(r);
        let show_boxes = self.show_boxes;
        let brightness = self.world.superbrightness;
        let cam_x = self.world.cam_x;

        let shake = self.world.envshake.offset;
        let cam_x = cam_x + shake[0];
        let cam_y = self.world.cam_y + shake[1];
        let zoom = self.world.cam_zoom * self.zoom;
        let bg_tint = tint(self.world.bgpalfx.sample());
        let portraits = [
            self.sprite_from(r, Some(self.world.players[0].data), 9000, 0),
            self.sprite_from(r, Some(self.world.players[1].data), 9000, 0),
        ];
        let mut stage_visual = self.stage_visual.take();
        let mut lifebar = self.lifebar.take();
        let (w, h) = r.size();
        let aspect = w as f32 / h as f32;
        let screen_w = stage.height * aspect;
        // zoom is anchored on the ground line; HUD coordinates are "screen" units
        // mapped back into the zoomed world so bars never scale with the camera
        let center_y = ground - (ground - stage.height / 2.0) / zoom;
        let u = 1.0 / zoom;
        let hud_left = cam_x - screen_w * u / 2.0;
        let hud_w = screen_w * u;
        let hud_top = center_y - stage.height * u / 2.0;
        let hud = |sx: f32, sy: f32| [hud_left + sx * u, hud_top + sy * u];
        match stage_visual.as_mut() {
            Some(sv) => sv.draw(
                r,
                0,
                [cam_x, cam_y],
                stage.width / sv.localcoord[0],
                screen_w * u,
                bg_tint,
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
                d.blend = it.blend;
                d.angle = it.angle;
                d.tint = it.tint;
                if it.angle != 0.0 {
                    // rotate about the player's axis, not the sprite's
                    let (dx, dy) = (d.pos[0] - origin[0], d.pos[1] - origin[1]);
                    let rad = it.angle * (std::f32::consts::PI / 180.0);
                    let (sn, cs) = (libm::sinf(rad), libm::cosf(rad));
                    d.pos = [origin[0] + dx * cs + dy * sn, origin[1] - dx * sn + dy * cs];
                }
                r.draw(d);
            }
            if show_boxes && it.boxes {
                for pl in &stage.platforms {
                    r.rect(
                        [pl.x1, ground + pl.y - cam_y - 1.0],
                        [pl.x2, ground + pl.y - cam_y + 1.0],
                        [0.2, 1.0, 0.4, 0.9],
                    );
                }
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
                screen_w * u,
                bg_tint,
            );
        }
        if self.overlay {
            self.draw_overlay(r, &hud, u, screen_w);
        }
        if let Some(t) = self.tutorial.as_ref() {
            let lines = t.text();
            r.rect(
                hud(screen_w / 2.0 - 150.0, 64.0),
                hud(screen_w / 2.0 + 150.0, 108.0),
                [0.0, 0.0, 0.0, 0.55],
            );
            // screenpack fonts when the fight.def has them, else the built-in 3x5 font
            let fonts = [(4, 0, 88.0), (3, 1, 98.0), (1, 5, 106.0)];
            for (k, (index, bank, y)) in fonts.iter().enumerate() {
                let drawn = lifebar.as_mut().is_some_and(|lb| {
                    lb.text_at(r, *index, &lines[k], hud(screen_w / 2.0, *y), *bank, 0, u)
                });
                if !drawn {
                    let (px, y, col) = match k {
                        0 => (2.0, 70.0, [1.0, 0.9, 0.3, 1.0]),
                        1 => (1.2, 86.0, [1.0, 1.0, 1.0, 1.0]),
                        _ => (1.0, 98.0, [0.7, 0.8, 1.0, 1.0]),
                    };
                    draw_text(r, &lines[k], hud(screen_w / 2.0, y), px * u, col);
                }
            }
        }
        if brightness < 1.0 {
            r.rect(
                [-4000.0, -4000.0],
                [4000.0, 4000.0],
                [0.0, 0.0, 0.0, 1.0 - brightness],
            );
        }
        if let Some(lb) = lifebar.as_mut() {
            // fight.def space (usually 320x240) scaled to the stage height and centred
            let lc = lb.localcoord;
            // never wider than the window: narrow windows shrink the HUD instead of cutting it
            let ratio = (stage.height / lc[1].max(1.0)).min(screen_w / lc[0].max(1.0));
            let x_off = (screen_w - lc[0] * ratio) / 2.0;
            let lb_hud = |x: f32, y: f32| hud(x * ratio + x_off, y * ratio);
            // wider than the fight.def: each side's bars move out to its edge
            lb.draw(r, &self.world, &lb_hud, u * ratio, portraits, x_off / ratio);
        } else {
            // life bars (screen units via `hud`)
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
                    (10.0, 10.0 + bar_w)
                } else {
                    (screen_w - 10.0 - bar_w, screen_w - 10.0)
                };
                r.rect(hud(x0, 8.0), hud(x1, 16.0), [0.1, 0.1, 0.1, 0.9]);
                let (fx0, fx1) = if i == 0 {
                    (x1 - bar_w * frac, x1)
                } else {
                    (x0, x0 + bar_w * frac)
                };
                r.rect(hud(fx0, 8.0), hud(fx1, 16.0), [0.9, 0.8, 0.2, 1.0]);
                let pf = (p.power as f32 / p.power_max.max(1) as f32).clamp(0.0, 1.0);
                let (px0, px1) = if i == 0 {
                    (x0, x0 + bar_w * pf)
                } else {
                    (x1 - bar_w * pf, x1)
                };
                r.rect(
                    hud(x0, stage.height - 14.0),
                    hud(x1, stage.height - 10.0),
                    [0.1, 0.1, 0.1, 0.9],
                );
                r.rect(
                    hud(px0, stage.height - 14.0),
                    hud(px1, stage.height - 10.0),
                    [0.3, 0.6, 1.0, 1.0],
                );
            }
            // round clock, win marks, announcements
            let clock = self.world.round.clock();
            if clock >= 0 {
                draw_number(
                    r,
                    clock,
                    hud(screen_w / 2.0, 8.0),
                    2.0 * u,
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            for (i, wins) in self.world.round.wins.iter().enumerate() {
                for k in 0..*wins {
                    let x = if i == 0 {
                        10.0 + k as f32 * 8.0
                    } else {
                        screen_w - 15.0 - k as f32 * 8.0
                    };
                    r.rect(hud(x, 20.0), hud(x + 5.0, 25.0), [1.0, 0.9, 0.3, 1.0]);
                }
            }
            if !self.net_status.is_empty() {
                draw_text(
                    r,
                    &self.net_status,
                    hud(screen_w / 2.0, stage.height - 4.0),
                    u,
                    [0.8, 0.9, 1.0, 0.9],
                );
            }
            if let Some((text, _)) = &self.banner {
                draw_text(
                    r,
                    text,
                    hud(screen_w / 2.0, stage.height * 0.35),
                    3.0 * u,
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
        }
        let _ = hud_w;
        let cam = Camera {
            screen: [screen_w, stage.height],
            center: [cam_x, center_y],
            zoom,
        };
        let clear = stage_visual
            .as_ref()
            .map_or([0.08, 0.08, 0.1, 1.0], |sv| sv.clear_color);
        self.stage_visual = stage_visual;
        self.lifebar = lifebar;
        (cam, clear)
    }
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
/// `--script`: comma-separated tokens of key letters held for one tick
/// (`U D L R a b c x y z s`, `.` = nothing) with an optional `*n` repeat,
/// e.g. `R*40,.*20,D,DR,R,xy,.*10`.
pub fn parse_script(s: &str) -> Vec<Buttons> {
    let mut out = Vec::new();
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let (keys, n) = match tok.split_once('*') {
            Some((k, n)) => (k, n.trim().parse().unwrap_or(1)),
            None => (tok, 1),
        };
        let mut b = Buttons::default();
        for ch in keys.chars() {
            match ch {
                'U' => b.u = true,
                'D' => b.d = true,
                'L' => b.l = true,
                'R' => b.r = true,
                'a' => b.a = true,
                'b' => b.b = true,
                'c' => b.c = true,
                'x' => b.x = true,
                'y' => b.y = true,
                'z' => b.z = true,
                's' => b.s = true,
                _ => {}
            }
        }
        for _ in 0..n {
            out.push(b);
        }
    }
    out
}

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

/// Newest modification time (seconds) of any state/anim/command file under `dir`.
fn newest_mtime(dir: &Path) -> u64 {
    let mut newest = 0u64;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .map(|x| x.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(
            ext.as_str(),
            "cns" | "cmd" | "air" | "def" | "st" | "sff" | "snd"
        ) {
            continue;
        }
        if let Ok(m) = e.metadata()
            && let Ok(t) = m.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            newest = newest.max(d.as_secs());
        }
    }
    newest
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
