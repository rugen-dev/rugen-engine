//! Host for `rugen-ui` screens: loads `motif/ui/<name>.toml`, paints with the
//! renderer and the motif fonts, decodes WebP/PNG/SFF assets, maps the mouse,
//! and turns actions into shell transitions. The title and options screens
//! live here when the motif ships `ui/title.toml` / `ui/options.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rugen_render::{Camera, Draw, PaletteId, Renderer, TextureId};
use rugen_sim::Buttons;
use rugen_ui::{Event, Nav, Painter, Rect, ScreenDef, Ui, VIRTUAL_H, VIRTUAL_W};
use serde_json::{Value, json};
use winit::keyboard::Key;

use crate::config::Config;
use crate::game::{Mode, Transition};
use crate::sprites::{FontGpu, SpriteSheet};
use crate::system::SystemData;

/// Which hardcoded behaviour backs the screen's actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKind {
    Title,
    Options,
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
    /// `rugen-server` anonymous account (empty until first online use).
    pub online_key: String,
    pub online_id: u64,
    /// Character brought into online matches (empty = `[net] p1`).
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
            online_key: String::new(),
            online_id: 0,
            main_char: String::new(),
        }
    }
}

impl Profile {
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

    pub fn save(&self, root: &Path) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(root.join("profile.json"), text);
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
    /// Options screen: edited config, key capture `(player, button index)`.
    pub config: Config,
    capture: Option<(usize, usize)>,
    pub dirty: bool,
    netplay: bool,
    profile: Profile,
    news: Value,
    window: (u32, u32),
    mouse_px: Option<(f32, f32)>,
    pending: Option<Transition>,
}

const BUTTON_NAMES: [&str; 11] = [
    "UP", "DOWN", "LEFT", "RIGHT", "A", "B", "C", "X", "Y", "Z", "START",
];

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
    /// True when the motif provides a TOML for this screen.
    pub fn available(motif: &Path, kind: UiKind) -> bool {
        motif.join("ui").join(Self::file(kind)).exists()
    }

    fn file(kind: UiKind) -> &'static str {
        match kind {
            UiKind::Title => "title.toml",
            UiKind::Options => "options.toml",
        }
    }

    pub fn load(
        kind: UiKind,
        motif: &Path,
        root: &Path,
        sys: &SystemData,
        config: &Config,
        netplay: bool,
    ) -> anyhow::Result<Self> {
        let path = motif.join("ui").join(Self::file(kind));
        let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        let def = ScreenDef::parse(&text).with_context(|| path.display().to_string())?;
        let news = std::fs::read(motif.join("news.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or_else(|| json!([]));
        let mut s = Self {
            kind,
            ui: Ui::new(def, Value::Null),
            motif: motif.to_path_buf(),
            textures: HashMap::new(),
            config: config.clone(),
            capture: None,
            dirty: false,
            netplay,
            profile: Profile::load(root),
            news,
            window: (1280, 720),
            mouse_px: None,
            pending: None,
        };
        s.refresh_data(sys);
        Ok(s)
    }

    /// Rebuild the data context the TOML binds to.
    fn refresh_data(&mut self, sys: &SystemData) {
        let data = match self.kind {
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
            UiKind::Options => {
                let c = &self.config;
                let row = |id: &str, label: &str, value: String, adjustable: bool| {
                    json!({
                        "id": id,
                        "label": label,
                        "value": value,
                        "press": if adjustable { format!("adjust:{id}:1") } else { format!("open:{id}") },
                        "left": if adjustable { format!("adjust:{id}:-1") } else { String::new() },
                        "right": if adjustable { format!("adjust:{id}:1") } else { String::new() },
                    })
                };
                let rows = vec![
                    row(
                        "master",
                        "MASTER VOLUME",
                        format!("{}%", libm::roundf(c.audio.master * 100.0)),
                        true,
                    ),
                    row(
                        "music",
                        "MUSIC VOLUME",
                        format!("{}%", libm::roundf(c.audio.music * 100.0)),
                        true,
                    ),
                    row(
                        "zoom_out",
                        "CAMERA ZOOM OUT",
                        format!("{:.2}", c.camera.zoom_out),
                        true,
                    ),
                    row(
                        "zoom_in",
                        "CAMERA ZOOM IN",
                        format!("{:.2}", c.camera.zoom_in),
                        true,
                    ),
                    row(
                        "fullscreen",
                        "FULLSCREEN",
                        if c.video.fullscreen { "ON" } else { "OFF" }.into(),
                        true,
                    ),
                    row("ai", "COMPUTER LEVEL", c.game.ai_level.to_string(), true),
                    row(
                        "delay",
                        "NET INPUT DELAY",
                        format!("{} F", c.net.input_delay),
                        true,
                    ),
                    row("keys1", "P1 KEYS", short_keys(&c.input.p1), false),
                    row("keys2", "P2 KEYS", short_keys(&c.input.p2), false),
                ];
                let capture = match self.capture {
                    Some((p, i)) => json!({
                        "active": true,
                        "prompt": format!("P{} : PRESS KEY FOR {}", p + 1, BUTTON_NAMES[i]),
                        "progress": format!("{} / 11", i + 1),
                    }),
                    None => json!({ "active": false, "prompt": "", "progress": "" }),
                };
                json!({
                    "rows": rows,
                    "capture": capture,
                    "profile": serde_json::to_value(&self.profile).unwrap_or(Value::Null),
                    "version": env!("CARGO_PKG_VERSION"),
                })
            }
        };
        self.ui.set_data(data);
    }

    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Raw key while rebinding. Returns true when consumed.
    pub fn raw_key(&mut self, key: &Key, sys: &SystemData) -> bool {
        let Some((player, idx)) = self.capture else {
            return false;
        };
        if *key == Key::Named(winit::keyboard::NamedKey::Escape) {
            self.capture = None;
            self.refresh_data(sys);
            return true;
        }
        let Some(name) = crate::input::key_name(key) else {
            return true;
        };
        let keys = if player == 0 {
            &mut self.config.input.p1
        } else {
            &mut self.config.input.p2
        };
        keys.set(idx, &name);
        self.dirty = true;
        self.capture = (idx + 1 < 11).then_some((player, idx + 1));
        self.refresh_data(sys);
        true
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

    fn adjust(&mut self, id: &str, dir: i32) {
        let c = &mut self.config;
        let step = dir as f32;
        match id {
            "master" => c.audio.master = (c.audio.master + 0.1 * step).clamp(0.0, 1.0),
            "music" => c.audio.music = (c.audio.music + 0.1 * step).clamp(0.0, 1.0),
            "zoom_out" => c.camera.zoom_out = (c.camera.zoom_out + 0.05 * step).clamp(0.3, 1.0),
            "zoom_in" => c.camera.zoom_in = (c.camera.zoom_in + 0.05 * step).clamp(1.0, 2.0),
            "fullscreen" => c.video.fullscreen = !c.video.fullscreen,
            "ai" => c.game.ai_level = (c.game.ai_level + dir).clamp(1, 8),
            "delay" => c.net.input_delay = (c.net.input_delay as i32 + dir).clamp(0, 8) as usize,
            _ => return,
        }
        self.dirty = true;
    }

    fn action(&mut self, a: &str, sys: &SystemData) -> Option<Transition> {
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
                self.adjust(arg, arg2.parse().unwrap_or(1));
                self.refresh_data(sys);
                None
            }
            "open" => {
                match arg {
                    "keys1" => self.capture = Some((0, 0)),
                    "keys2" => self.capture = Some((1, 0)),
                    _ => {}
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
        let nav = if self.capture.is_some() {
            Nav::default()
        } else {
            let any = |f: fn(&Buttons) -> bool| pressed.iter().any(f);
            Nav {
                up: any(|b| b.u),
                down: any(|b| b.d),
                left: any(|b| b.l),
                right: any(|b| b.r),
                accept: any(|b| b.a || b.b || b.c || b.x || b.y || b.z || b.s),
                back: false,
            }
        };
        let events = {
            let mut host = Host {
                r,
                fonts: &mut sys.fonts,
                sheet: &mut sys.sheet,
                textures: &mut self.textures,
                motif: &self.motif,
            };
            self.ui.tick(nav, &mut host)
        };
        let mut out = None;
        for e in events {
            match e {
                Event::Moved => sfx(sys.title.move_snd),
                Event::Back => out = Some(Transition::Title),
                Event::Action(a) => {
                    sfx(sys.title.done_snd);
                    if let Some(t) = self.action(&a, sys) {
                        out = Some(t);
                    }
                }
            }
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

fn short_keys(k: &crate::config::PlayerKeys) -> String {
    let short = |s: &str| -> String {
        match s {
            "ArrowUp" => "UP".into(),
            "ArrowDown" => "DN".into(),
            "ArrowLeft" => "LT".into(),
            "ArrowRight" => "RT".into(),
            "Enter" => "ENT".into(),
            "Space" => "SPC".into(),
            "Shift" => "SHF".into(),
            "Control" => "CTL".into(),
            other => other.to_uppercase(),
        }
    };
    k.names()
        .iter()
        .map(|s| short(s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decode a WebP or PNG file to straight RGBA.
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

/// Painter over the renderer, motif fonts and the system sprite sheet.
pub(crate) struct Host<'a> {
    pub(crate) r: &'a mut Renderer,
    pub(crate) fonts: &'a mut HashMap<i32, FontGpu>,
    pub(crate) sheet: &'a mut SpriteSheet,
    pub(crate) textures: &'a mut HashMap<String, Cached>,
    pub(crate) motif: &'a Path,
}

impl Host<'_> {
    fn texture(&mut self, key: &str) -> Cached {
        if let Some(c) = self.textures.get(key) {
            return *c;
        }
        let loaded: Cached = if let Some(spec) = key.strip_prefix("sff:") {
            let mut it = spec
                .split(',')
                .map(|s| s.trim().parse::<i32>().unwrap_or(0));
            let (g, n) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
            self.sheet
                .get(self.r, g, n)
                .map(|p| (p.tex, p.size[0] as u32, p.size[1] as u32, p.pal))
        } else {
            match load_image(&self.motif.join(key)) {
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
